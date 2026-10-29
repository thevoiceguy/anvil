//! Contacts as a file (`docs/APP.md` §Contacts; FCP's
//! `fcp_core::contact_file`, which the account's import and export use): vCard 3.0 (RFC 2426), which phones, Outlook and Google read
//! and write, and CSV (RFC 4180) with Google's columns. What is read is
//! what a contact holds: a name, a company, numbers with their labels, an
//! email and notes; anything else in the file is left.
//!
//! A number's label is kept as the apps keep it (`mobile`, `work`, `home`,
//! `main`, `work fax`, `other`, or the user's own); a file's types are read
//! into those and written back from them.

use crate::state::{Contact, ContactNumber};

/// The most numbers one contact has, as FCP keeps.
const MAX_NUMBERS: usize = 20;

/// A contact read from a file, not yet kept.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportedContact {
    pub name: String,
    pub company: Option<String>,
    pub numbers: Vec<ContactNumber>,
    pub email: Option<String>,
    pub notes: Option<String>,
}

impl ImportedContact {
    /// What makes two contacts the same for an import: the name (case
    /// aside) and the numbers' digits, in any order.
    pub fn same_as(&self, name: &str, numbers: &[ContactNumber]) -> bool {
        fn digits(numbers: &[ContactNumber]) -> Vec<String> {
            let mut d: Vec<String> = numbers
                .iter()
                .map(|n| {
                    n.number
                        .chars()
                        .filter(|c| c.is_ascii_digit() || *c == '+')
                        .collect()
                })
                .collect();
            d.sort();
            d
        }
        self.name.trim().to_lowercase() == name.trim().to_lowercase()
            && digits(&self.numbers) == digits(numbers)
    }
}

/// What a file held: the contacts read, and the entries that could not be
/// (their place in the file and why).
#[derive(Debug, Default)]
pub struct Read {
    pub contacts: Vec<ImportedContact>,
    pub skipped: Vec<(usize, String)>,
}

/// A file's format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    VCard,
    Csv,
}

impl Format {
    /// A file's format by what it starts with: vCard's `BEGIN:VCARD`, else
    /// CSV.
    pub fn guess(text: &str) -> Self {
        let start = text.trim_start_matches('\u{feff}').trim_start();
        if start.len() >= 11 && start[..11].eq_ignore_ascii_case("BEGIN:VCARD") {
            Self::VCard
        } else {
            Self::Csv
        }
    }
}

/// Read `text` as `format`.
pub fn read(format: Format, text: &str) -> Read {
    match format {
        Format::VCard => read_vcard(text),
        Format::Csv => read_csv(text),
    }
}

/// Write `contacts` as `format`.
pub fn write(format: Format, contacts: &[Contact]) -> String {
    match format {
        Format::VCard => write_vcard(contacts),
        Format::Csv => write_csv(contacts),
    }
}

/// A label as the apps keep it: one of theirs, lower case, or the file's
/// own as written.
fn label_of(raw: &str) -> String {
    let l = raw.trim();
    match l.to_ascii_lowercase().as_str() {
        "mobile" | "cell" | "cellular" | "iphone" => "mobile".into(),
        "work" | "business" | "office" => "work".into(),
        "home" | "personal" => "home".into(),
        "main" | "primary" | "company main" | "pref" => "main".into(),
        "work fax" | "business fax" | "fax" | "home fax" => "work fax".into(),
        "other" | "voice" => "other".into(),
        _ => l.to_string(),
    }
}

fn tidy(v: &str) -> Option<String> {
    let v = v.trim();
    (!v.is_empty()).then(|| v.to_string())
}

/// A contact read, checked: a name (else its company, its email or its
/// first number stands in), at most [`MAX_NUMBERS`] numbers.
fn finish(mut c: ImportedContact) -> Result<ImportedContact, String> {
    c.numbers.retain(|n| !n.number.trim().is_empty());
    c.numbers.truncate(MAX_NUMBERS);
    if c.name.trim().is_empty() {
        c.name = c
            .company
            .clone()
            .or_else(|| c.email.clone())
            .or_else(|| c.numbers.first().map(|n| n.number.clone()))
            .ok_or("an entry with no name, company, email or number")?;
    }
    c.name = c.name.trim().to_string();
    Ok(c)
}

// ---- vCard ----

/// vCard's lines unfolded: a line starting with a space or tab goes on the
/// one before.
fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in text.trim_start_matches('\u{feff}').split('\n') {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(rest) = raw.strip_prefix([' ', '\t']) {
            if let Some(last) = lines.last_mut() {
                last.push_str(rest);
                continue;
            }
        }
        lines.push(raw.to_string());
    }
    lines
}

/// A vCard text value's escapes undone.
fn unescape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') | Some('N') => out.push('\n'),
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Split at `sep` where it is not escaped.
fn split_unescaped(v: &str, sep: char) -> Vec<String> {
    let mut parts = vec![String::new()];
    let mut escaped = false;
    for c in v.chars() {
        if escaped {
            parts.last_mut().unwrap().push('\\');
            parts.last_mut().unwrap().push(c);
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == sep {
            parts.push(String::new());
        } else {
            parts.last_mut().unwrap().push(c);
        }
    }
    parts
}

/// A TEL's label from its TYPE parameters (and an Apple `X-ABLabel` for
/// its group, which names a custom one).
fn tel_label(types: &[String], custom: Option<&str>) -> String {
    if let Some(custom) = custom {
        let c = custom
            .trim_start_matches("_$!<")
            .trim_end_matches(">!$_")
            .trim();
        if !c.is_empty() {
            return label_of(c);
        }
    }
    let has = |t: &str| types.iter().any(|x| x.eq_ignore_ascii_case(t));
    if has("fax") {
        "work fax".into()
    } else if has("cell") || has("mobile") || has("iphone") {
        "mobile".into()
    } else if has("work") {
        "work".into()
    } else if has("home") {
        "home".into()
    } else if has("main") {
        "main".into()
    } else {
        "other".into()
    }
}

/// A TEL as read: its group (`item1`), its types, the number.
struct Tel {
    group: Option<String>,
    types: Vec<String>,
    number: String,
}

/// A card between BEGIN and END: the contact so far, the name N gives
/// (used when there is no FN), the numbers (labelled at END, once any
/// group's `X-ABLabel` is known).
#[derive(Default)]
struct Card {
    contact: ImportedContact,
    n: Option<String>,
    tels: Vec<Tel>,
}

fn read_vcard(text: &str) -> Read {
    let mut read = Read::default();
    let mut current: Option<Card> = None;
    let mut labels: Vec<(String, String)> = Vec::new();
    let mut index = 0;
    for line in unfold(text) {
        let Some((head, value)) = line.split_once(':') else {
            continue;
        };
        // `item1.TEL;TYPE=CELL` → group, name, parameters.
        let mut params = head.split(';');
        let full = params.next().unwrap_or_default();
        let (group, name) = match full.rsplit_once('.') {
            Some((g, n)) => (Some(g.to_ascii_lowercase()), n),
            None => (None, full),
        };
        let name = name.to_ascii_uppercase();
        let types: Vec<String> = params
            .flat_map(|p| {
                let p = p.trim();
                let v = match p.split_once('=') {
                    Some((k, v)) if k.eq_ignore_ascii_case("type") => v,
                    Some(_) => return Vec::new(),
                    None => p,
                };
                v.trim_matches('"')
                    .split(',')
                    .map(|t| t.trim().to_string())
                    .collect()
            })
            .collect();
        match name.as_str() {
            "BEGIN" if value.trim().eq_ignore_ascii_case("VCARD") => {
                index += 1;
                labels.clear();
                current = Some(Card::default());
            }
            "END" if value.trim().eq_ignore_ascii_case("VCARD") => {
                if let Some(Card {
                    contact: mut c,
                    n,
                    tels,
                }) = current.take()
                {
                    if c.name.is_empty() {
                        c.name = n.unwrap_or_default();
                    }
                    for tel in tels {
                        let custom = tel.group.as_ref().and_then(|g| {
                            labels
                                .iter()
                                .find(|(lg, _)| lg == g)
                                .map(|(_, l)| l.as_str())
                        });
                        c.numbers.push(ContactNumber {
                            label: tel_label(&tel.types, custom),
                            number: tel.number,
                        });
                    }
                    match finish(c) {
                        Ok(c) => read.contacts.push(c),
                        Err(e) => read.skipped.push((index, e)),
                    }
                }
            }
            _ => {
                let Some(Card {
                    contact: c,
                    n,
                    tels,
                }) = current.as_mut()
                else {
                    continue;
                };
                match name.as_str() {
                    "FN" => c.name = unescape(value).trim().to_string(),
                    "N" => {
                        // Family;Given;Additional;Prefix;Suffix → "Given Family".
                        let parts: Vec<String> = split_unescaped(value, ';')
                            .iter()
                            .map(|p| unescape(p).trim().to_string())
                            .collect();
                        let given = parts.get(1).cloned().unwrap_or_default();
                        let family = parts.first().cloned().unwrap_or_default();
                        let joined = [given, family]
                            .into_iter()
                            .filter(|p| !p.is_empty())
                            .collect::<Vec<_>>()
                            .join(" ");
                        *n = tidy(&joined);
                    }
                    "ORG" => {
                        c.company = split_unescaped(value, ';')
                            .first()
                            .and_then(|o| tidy(&unescape(o)))
                    }
                    "TEL" => {
                        let number = value.trim().trim_start_matches("tel:").to_string();
                        if !number.is_empty() {
                            tels.push(Tel {
                                group,
                                types,
                                number,
                            });
                        }
                    }
                    "EMAIL" if c.email.is_none() => c.email = tidy(&unescape(value)),
                    "NOTE" => c.notes = tidy(&unescape(value)),
                    "X-ABLABEL" => {
                        if let Some(g) = group {
                            labels.push((g, unescape(value)));
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    read
}

/// A vCard text value escaped.
fn escape(v: &str) -> String {
    v.replace('\\', "\\\\")
        .replace(',', "\\,")
        .replace(';', "\\;")
        .replace("\r\n", "\\n")
        .replace('\n', "\\n")
}

/// A content line folded at 75 octets, on a character boundary.
fn fold(line: &str, out: &mut String) {
    let mut width = 0;
    for c in line.chars() {
        let len = c.len_utf8();
        if width + len > 75 {
            out.push_str("\r\n ");
            width = 1;
        }
        out.push(c);
        width += len;
    }
    out.push_str("\r\n");
}

fn tel_types(label: &str) -> &'static str {
    match label {
        "mobile" => "CELL,VOICE",
        "work" => "WORK,VOICE",
        "home" => "HOME,VOICE",
        "main" => "MAIN,VOICE",
        "work fax" => "WORK,FAX",
        _ => "VOICE",
    }
}

fn write_vcard(contacts: &[Contact]) -> String {
    let mut out = String::new();
    for c in contacts {
        fold("BEGIN:VCARD", &mut out);
        fold("VERSION:3.0", &mut out);
        fold(&format!("FN:{}", escape(&c.name)), &mut out);
        // N is required by 3.0: the last word the family name.
        let (given, family) = match c.name.trim().rsplit_once(' ') {
            Some((g, f)) => (g.to_string(), f.to_string()),
            None => (c.name.trim().to_string(), String::new()),
        };
        fold(
            &format!("N:{};{};;;", escape(&family), escape(&given)),
            &mut out,
        );
        if let Some(org) = &c.company {
            fold(&format!("ORG:{}", escape(org)), &mut out);
        }
        for (i, n) in c.numbers.iter().enumerate() {
            let known = matches!(
                n.label.as_str(),
                "mobile" | "work" | "home" | "main" | "work fax" | "other" | ""
            );
            if known {
                fold(
                    &format!("TEL;TYPE={}:{}", tel_types(&n.label), n.number),
                    &mut out,
                );
            } else {
                // The user's own label, as Apple and Google write one.
                fold(
                    &format!("item{}.TEL;TYPE=VOICE:{}", i + 1, n.number),
                    &mut out,
                );
                fold(
                    &format!("item{}.X-ABLabel:{}", i + 1, escape(&n.label)),
                    &mut out,
                );
            }
        }
        if let Some(email) = &c.email {
            fold(&format!("EMAIL;TYPE=INTERNET:{}", escape(email)), &mut out);
        }
        if let Some(notes) = &c.notes {
            fold(&format!("NOTE:{}", escape(notes)), &mut out);
        }
        fold("END:VCARD", &mut out);
    }
    out
}

// ---- CSV ----

/// CSV's records (RFC 4180): fields split at commas outside quotes, `""`
/// a quote inside them, records at line ends outside quotes.
fn csv_records(text: &str) -> Vec<Vec<String>> {
    let mut records = Vec::new();
    let mut record = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.trim_start_matches('\u{feff}').chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' => quoted = true,
            ',' => record.push(std::mem::take(&mut field)),
            '\r' => {}
            '\n' => {
                record.push(std::mem::take(&mut field));
                records.push(std::mem::take(&mut record));
            }
            _ => field.push(c),
        }
    }
    if !field.is_empty() || !record.is_empty() {
        record.push(field);
        records.push(record);
    }
    records.retain(|r| r.iter().any(|f| !f.trim().is_empty()));
    records
}

/// A column a single number is in, by its header, and the label it gives:
/// Outlook's, and this module's own simple ones.
fn phone_column(header: &str) -> Option<&'static str> {
    Some(match header {
        "mobile phone" | "mobile" | "cell phone" => "mobile",
        "business phone" | "business phone 2" | "work phone" | "work" => "work",
        "home phone" | "home phone 2" | "home" => "home",
        "company main phone" | "primary phone" | "main" => "main",
        "business fax" | "work fax" | "fax" => "work fax",
        "other phone" | "other" | "phone" => "other",
        _ => return None,
    })
}

fn read_csv(text: &str) -> Read {
    let mut read = Read::default();
    let mut records = csv_records(text).into_iter();
    let Some(header) = records.next() else {
        return read;
    };
    let header: Vec<String> = header.iter().map(|h| h.trim().to_lowercase()).collect();
    let col = |names: &[&str]| header.iter().position(|h| names.contains(&h.as_str()));
    let name = col(&["name", "full name", "display name"]);
    let given = col(&["first name", "given name"]);
    let middle = col(&["middle name", "additional name"]);
    let family = col(&["last name", "family name", "surname"]);
    let company = col(&[
        "company",
        "organization 1 - name",
        "organization name",
        "organization",
    ]);
    let email = col(&[
        "e-mail address",
        "e-mail 1 - value",
        "email",
        "email address",
        "e-mail",
    ]);
    let notes = col(&["notes", "note"]);
    // Google's "Phone N - Type" / "Phone N - Value" pairs.
    let mut pairs: Vec<(Option<usize>, usize)> = Vec::new();
    for (i, h) in header.iter().enumerate() {
        if let Some(n) = h
            .strip_prefix("phone ")
            .and_then(|r| r.strip_suffix(" - value"))
        {
            let ty = header
                .iter()
                .position(|t| *t == format!("phone {n} - type"));
            pairs.push((ty, i));
        }
    }
    let singles: Vec<(usize, &'static str)> = header
        .iter()
        .enumerate()
        .filter_map(|(i, h)| phone_column(h).map(|l| (i, l)))
        .collect();
    if name.is_none() && given.is_none() && family.is_none() && company.is_none() {
        read.skipped
            .push((1, "no Name, First Name, Last Name or Company column".into()));
        return read;
    }
    for (row, record) in records.enumerate() {
        let get = |i: Option<usize>| i.and_then(|i| record.get(i)).and_then(|v| tidy(v));
        let full = get(name).unwrap_or_else(|| {
            [get(given), get(middle), get(family)]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" ")
        });
        let mut numbers = Vec::new();
        for (ty, value) in &pairs {
            let label = get(*ty)
                .map(|t| label_of(&t))
                .unwrap_or_else(|| "other".into());
            // Google puts two numbers of one type in one cell.
            for number in get(Some(*value)).unwrap_or_default().split(" ::: ") {
                if let Some(number) = tidy(number) {
                    numbers.push(ContactNumber {
                        label: label.clone(),
                        number,
                    });
                }
            }
        }
        for (i, label) in &singles {
            if let Some(number) = get(Some(*i)) {
                numbers.push(ContactNumber {
                    label: (*label).to_string(),
                    number,
                });
            }
        }
        let contact = ImportedContact {
            name: full,
            company: get(company),
            numbers,
            email: get(email),
            notes: get(notes),
        };
        // Row 1 is the header.
        match finish(contact) {
            Ok(c) => read.contacts.push(c),
            Err(e) => read.skipped.push((row + 2, e)),
        }
    }
    read
}

fn csv_field(v: &str) -> String {
    if v.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", v.replace('"', "\"\""))
    } else {
        v.to_string()
    }
}

/// The label as Google's CSV writes a type.
fn csv_type(label: &str) -> String {
    match label {
        "mobile" => "Mobile".into(),
        "work" => "Work".into(),
        "home" => "Home".into(),
        "main" => "Main".into(),
        "work fax" => "Work Fax".into(),
        "other" | "" => "Other".into(),
        own => own.to_string(),
    }
}

fn write_csv(contacts: &[Contact]) -> String {
    let phones = contacts
        .iter()
        .map(|c| c.numbers.len())
        .max()
        .unwrap_or(0)
        .max(1);
    let mut header: Vec<String> = ["Name", "Organization 1 - Name", "E-mail 1 - Value", "Notes"]
        .into_iter()
        .map(String::from)
        .collect();
    for n in 1..=phones {
        header.push(format!("Phone {n} - Type"));
        header.push(format!("Phone {n} - Value"));
    }
    let mut out = header.join(",");
    out.push_str("\r\n");
    for c in contacts {
        let mut fields = vec![
            csv_field(&c.name),
            csv_field(c.company.as_deref().unwrap_or("")),
            csv_field(c.email.as_deref().unwrap_or("")),
            csv_field(c.notes.as_deref().unwrap_or("")),
        ];
        for i in 0..phones {
            match c.numbers.get(i) {
                Some(n) => {
                    fields.push(csv_field(&csv_type(&n.label)));
                    fields.push(csv_field(&n.number));
                }
                None => fields.extend([String::new(), String::new()]),
            }
        }
        out.push_str(&fields.join(","));
        out.push_str("\r\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contact(name: &str, numbers: &[(&str, &str)]) -> Contact {
        Contact {
            id: "c".into(),
            on_account: false,
            name: name.into(),
            company: Some("Acme, Inc.".into()),
            numbers: numbers
                .iter()
                .map(|(l, n)| ContactNumber {
                    label: (*l).into(),
                    number: (*n).into(),
                })
                .collect(),
            email: Some("jo@example.com".into()),
            notes: Some("Line one\nline two; with \"quotes\"".into()),
            ringtone: None,
        }
    }

    fn numbers(c: &ImportedContact) -> Vec<(String, String)> {
        c.numbers
            .iter()
            .map(|n| (n.label.clone(), n.number.clone()))
            .collect()
    }

    #[test]
    fn a_contact_survives_both_formats() {
        let kept = vec![
            contact(
                "Jo Smith",
                &[
                    ("mobile", "+61 412 345 678"),
                    ("work", "02 9999 8888"),
                    ("work fax", "02 9999 8889"),
                    ("Boat", "0400 111 222"),
                ],
            ),
            contact("Prince", &[("home", "1004")]),
        ];
        for format in [Format::VCard, Format::Csv] {
            let text = write(format, &kept);
            assert_eq!(Format::guess(&text), format, "{text}");
            let read = read(format, &text);
            assert!(read.skipped.is_empty(), "{format:?}: {:?}", read.skipped);
            assert_eq!(read.contacts.len(), 2, "{format:?}");
            let jo = &read.contacts[0];
            assert_eq!(jo.name, "Jo Smith", "{format:?}");
            assert_eq!(jo.company.as_deref(), Some("Acme, Inc."), "{format:?}");
            assert_eq!(
                numbers(jo),
                [
                    ("mobile".into(), "+61 412 345 678".into()),
                    ("work".into(), "02 9999 8888".into()),
                    ("work fax".into(), "02 9999 8889".into()),
                    ("Boat".into(), "0400 111 222".into()),
                ],
                "{format:?}"
            );
            assert_eq!(jo.email.as_deref(), Some("jo@example.com"));
            assert_eq!(
                jo.notes.as_deref(),
                Some("Line one\nline two; with \"quotes\""),
                "{format:?}"
            );
            assert_eq!(read.contacts[1].name, "Prince");
        }
    }

    #[test]
    fn vcards_from_phones_are_read() {
        // Apple's: folded lines, a custom label in a group, N without FN.
        // Unfolding drops the one space a continuation starts with.
        let apple = "BEGIN:VCARD\r\nVERSION:3.0\r\nN:Park;Kim;;;\r\nORG:Boats\r\n  Ltd;\r\n\
                     item1.TEL;type=pref:0400 1\r\n 11 222\r\nitem1.X-ABLabel:_$!<Mobile>!$_\r\n\
                     item2.TEL:555\r\nitem2.X-ABLabel:Boat\r\nTEL;type=WORK;type=FAX:777\r\n\
                     END:VCARD\r\n";
        let read = read(Format::guess(apple), apple);
        assert_eq!(read.contacts.len(), 1, "{:?}", read.skipped);
        let kim = &read.contacts[0];
        assert_eq!(kim.name, "Kim Park");
        assert_eq!(kim.company.as_deref(), Some("Boats Ltd"));
        let numbers: Vec<_> = kim
            .numbers
            .iter()
            .map(|n| (n.label.as_str(), n.number.as_str()))
            .collect();
        assert_eq!(
            numbers,
            [
                ("mobile", "0400 111 222"),
                ("Boat", "555"),
                ("work fax", "777")
            ]
        );
        // A card with nothing to call it by is skipped, by its place.
        let empty = "BEGIN:VCARD\nVERSION:4.0\nFN:A\nEND:VCARD\nBEGIN:VCARD\nNOTE:x\nEND:VCARD\n";
        let read = super::read(Format::VCard, empty);
        assert_eq!(read.contacts.len(), 1);
        assert_eq!(read.skipped.len(), 1);
        assert_eq!(read.skipped[0].0, 2);
    }

    #[test]
    fn outlooks_and_googles_csv_are_read() {
        let outlook = "\u{feff}First Name,Last Name,Company,E-mail Address,Mobile Phone,Business Phone,Business Fax,Notes\r\n\
                       Jo,Smith,Acme,jo@x.com,0412,0299,,\"Met at, the \"\"expo\"\"\"\r\n\
                       ,,,,,,,\r\n";
        let read = read(Format::guess(outlook), outlook);
        assert_eq!(read.contacts.len(), 1, "{:?}", read.skipped);
        let jo = &read.contacts[0];
        assert_eq!(jo.name, "Jo Smith");
        assert_eq!(jo.notes.as_deref(), Some("Met at, the \"expo\""));
        assert_eq!(
            jo.numbers
                .iter()
                .map(|n| n.label.as_str())
                .collect::<Vec<_>>(),
            ["mobile", "work"]
        );

        let google = "Name,Organization 1 - Name,Phone 1 - Type,Phone 1 - Value\n\
                      Kim Park,,Mobile,0400 ::: 0401\n";
        let read = super::read(Format::Csv, google);
        assert_eq!(read.contacts[0].numbers.len(), 2);
        assert!(read.contacts[0].numbers.iter().all(|n| n.label == "mobile"));

        let nothing = "Colour,Size\nred,big\n";
        assert_eq!(super::read(Format::Csv, nothing).skipped.len(), 1);
    }

    #[test]
    fn duplicates_are_the_name_and_the_numbers_digits() {
        let c = ImportedContact {
            name: "Jo Smith".into(),
            numbers: vec![
                ContactNumber {
                    label: "work".into(),
                    number: "02 9999".into(),
                },
                ContactNumber {
                    label: "mobile".into(),
                    number: "+61-412".into(),
                },
            ],
            ..Default::default()
        };
        let same = [
            ContactNumber {
                label: "x".into(),
                number: "+61412".into(),
            },
            ContactNumber {
                label: "y".into(),
                number: "029999".into(),
            },
        ];
        assert!(c.same_as(" jo smith", &same));
        assert!(!c.same_as("Jo Smith", &same[..1]));
        assert!(!c.same_as("Jo Smyth", &same));
    }
}
