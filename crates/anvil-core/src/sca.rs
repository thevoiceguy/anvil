//! Shared call appearances, as FCP speaks them (its
//! `docs/ENTERPRISE_CALLING.md` §3.9): a shared line's phones watch its
//! `call-info` and are sent the line's appearances in a NOTIFY's
//! `Call-Info`; an INVITE from the line naming a held appearance picks
//! that call up, and naming an active one joins it where the line allows.

/// The event package a shared line's appearances are watched with.
pub const EVENT: &str = "call-info";

/// One appearance of a shared line in use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Appearance {
    /// 1 for the first.
    pub index: u8,
    /// `alerting`, `progressing`, `active`, `held` or `seized`.
    pub state: String,
    /// The far party, when the server names one (`"Bob" <sip:bob@x>`).
    pub remote: Option<String>,
}

/// The appearances in use that a `Call-Info` value lists, by index; the
/// `appearance-index=*` entry (the rest idle) and idle ones are left out.
pub fn parse(call_info: &str) -> Vec<Appearance> {
    let mut out: Vec<Appearance> = entries(call_info)
        .into_iter()
        .filter_map(|entry| {
            let (mut index, mut state, mut remote) = (None, None, None);
            for param in params(entry) {
                let (name, value) = param.split_once('=').unwrap_or((param, ""));
                let value = value.trim();
                match name.trim().to_ascii_lowercase().as_str() {
                    "appearance-index" => index = value.parse::<u8>().ok(),
                    "appearance-state" => state = Some(value.to_ascii_lowercase()),
                    "appearance-uri" => remote = Some(unquote(value)),
                    _ => {}
                }
            }
            let state = state?;
            (state != "idle").then_some(Appearance {
                index: index?,
                state,
                remote,
            })
        })
        .collect();
    out.sort_by_key(|a| a.index);
    out
}

/// What an INVITE names to take appearance `index` of the line at `host`.
pub fn take(host: &str, index: u8) -> String {
    format!("<sip:{host}>;appearance-index={index}")
}

/// A `Call-Info` value split at the commas between its entries, not those
/// inside a quoted string or `<…>`.
fn entries(value: &str) -> Vec<&str> {
    split_outside(value, ',')
}

/// An entry's parameters: what follows its `<uri>`, split at `;`.
fn params(entry: &str) -> Vec<&str> {
    let rest = match entry.find('>') {
        Some(end) => &entry[end + 1..],
        None => entry,
    };
    split_outside(rest, ';')
        .into_iter()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect()
}

fn split_outside(value: &str, sep: char) -> Vec<&str> {
    let (mut out, mut start, mut quoted, mut angle, mut escaped) = (vec![], 0, false, 0u32, false);
    for (i, c) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            '<' if !quoted => angle += 1,
            '>' if !quoted => angle = angle.saturating_sub(1),
            c if c == sep && !quoted && angle == 0 => {
                out.push(&value[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(&value[start..]);
    out
}

/// A quoted parameter value, its escapes undone.
fn unquote(value: &str) -> String {
    let Some(inner) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) else {
        return value.to_string();
    };
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lamps_fcp_sends_are_read_in_order_without_the_idle_rest() {
        // As FCP writes them (fcp-core `call_info_value`).
        let value = "<sip:10.0.0.1>;appearance-index=2;appearance-state=held;appearance-uri=\"\\\"Bob\\\" <sip:bob@x>\",\
             <sip:10.0.0.1>;appearance-index=1;appearance-state=active;appearance-uri=\"<sip:+15550001@x>\",\
             <sip:10.0.0.1>;appearance-index=*;appearance-state=idle";
        assert_eq!(
            parse(value),
            vec![
                Appearance {
                    index: 1,
                    state: "active".into(),
                    remote: Some("<sip:+15550001@x>".into()),
                },
                Appearance {
                    index: 2,
                    state: "held".into(),
                    remote: Some("\"Bob\" <sip:bob@x>".into()),
                },
            ]
        );
    }

    #[test]
    fn an_idle_line_has_none_and_a_seize_names_nobody() {
        assert!(parse("<sip:h>;appearance-index=*;appearance-state=idle").is_empty());
        assert_eq!(
            parse("<sip:h>;appearance-index=3;appearance-state=seized"),
            vec![Appearance {
                index: 3,
                state: "seized".into(),
                remote: None,
            }]
        );
    }

    #[test]
    fn an_invite_names_the_appearance_it_takes() {
        assert_eq!(
            take("pbx.example.com", 2),
            "<sip:pbx.example.com>;appearance-index=2"
        );
    }
}
