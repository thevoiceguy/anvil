//! Watching someone (RFC 3856 presence, RFC 4235 dialog): what their PIDF
//! and dialog-info bodies say, read into what a UI shows — available or
//! not and why, and a busy lamp.

/// Which event package a watch subscribes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WatchKind {
    /// `presence`: available, away, busy, on the phone, with a note.
    Presence,
    /// `dialog`: the busy lamp — idle, ringing, busy.
    Dialog,
}

impl WatchKind {
    pub fn event(self) -> &'static str {
        match self {
            WatchKind::Presence => "presence",
            WatchKind::Dialog => "dialog",
        }
    }
}

/// Someone's presence, as their PIDF says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Presence {
    /// `<basic>open</basic>`.
    pub open: bool,
    /// An RPID activity: `busy`, `on-the-phone`, `away`, …
    pub activity: Option<String>,
    pub note: Option<String>,
}

/// A busy lamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineState {
    /// No call.
    Idle,
    /// A call ringing them.
    Ringing,
    /// A call up, or one they are placing.
    Busy,
}

/// An address reduced for comparing: `user@host`, lower-case, without a
/// scheme, port or parameters.
pub fn aor_key(aor: &str) -> String {
    let s = aor.trim().trim_start_matches('<');
    let s = s.split('>').next().unwrap_or(s);
    let s = ["sips:", "sip:", "pres:", "tel:"]
        .iter()
        .find_map(|p| s.strip_prefix(p))
        .unwrap_or(s);
    let s = s.split(';').next().unwrap_or(s);
    match s.split_once('@') {
        Some((user, host)) => format!(
            "{}@{}",
            user.to_ascii_lowercase(),
            host.split(':').next().unwrap_or(host).to_ascii_lowercase()
        ),
        None => s.to_ascii_lowercase(),
    }
}

/// An attribute of the first element named `tag`.
fn attribute(xml: &str, tag: &str, name: &str) -> Option<String> {
    let start = xml.find(&format!("<{tag}"))?;
    let end = start + xml[start..].find('>')?;
    let element = &xml[start..end];
    let at = element.find(&format!(" {name}="))? + name.len() + 2;
    let quote = element[at..].chars().next()?;
    let rest = &element[at + 1..];
    Some(rest[..rest.find(quote)?].to_string())
}

/// The text of each element whose name ends with `tag` (any prefix).
fn texts(xml: &str, tag: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find('<') {
        rest = &rest[i + 1..];
        let Some(end) = rest.find('>') else { break };
        let name = rest[..end].split_whitespace().next().unwrap_or("");
        let local = name.rsplit(':').next().unwrap_or(name);
        if local == tag && !name.starts_with('/') && !rest[..end].ends_with('/') {
            let body = &rest[end + 1..];
            if let Some(close) = body.find("</") {
                out.push(body[..close].trim().to_string());
            }
        }
        rest = &rest[end + 1..];
    }
    out
}

/// The names of the elements inside the first element whose name ends
/// with `tag` (RPID activities are empty elements: `<rpid:busy/>`).
fn children(xml: &str, tag: &str) -> Vec<String> {
    let Some(open) = xml
        .find(&format!(":{tag}>"))
        .or_else(|| xml.find(&format!("<{tag}>")))
    else {
        return Vec::new();
    };
    let body = &xml[open..];
    let body = &body[body.find('>').map_or(0, |i| i + 1)..];
    let close = body.find(&format!("{tag}>")).unwrap_or(body.len());
    let inner = &body[..close];
    inner
        .split('<')
        .filter_map(|e| {
            let name = e.split(['/', '>', ' ']).next()?.trim();
            (!name.is_empty()).then(|| name.rsplit(':').next().unwrap_or(name).to_string())
        })
        .collect()
}

/// A PIDF body: whose it is, and their presence.
pub fn parse_pidf(xml: &str) -> Option<(String, Presence)> {
    let entity = attribute(xml, "presence", "entity")?;
    let open = texts(xml, "basic")
        .first()
        .is_some_and(|b| b.eq_ignore_ascii_case("open"));
    let activity = children(xml, "activities").into_iter().next();
    let note = texts(xml, "note")
        .into_iter()
        .next()
        .filter(|n| !n.is_empty());
    Some((
        entity,
        Presence {
            open,
            activity,
            note,
        },
    ))
}

/// A dialog-info body: whose it is, and their lamp.
pub fn parse_dialog_info(xml: &str) -> Option<(String, LineState)> {
    let entity = attribute(xml, "dialog-info", "entity")?;
    let mut state = LineState::Idle;
    let mut rest = xml;
    while let Some(i) = rest.find("<dialog ") {
        let dialog = &rest[i..];
        let end = dialog.find("</dialog>").unwrap_or(dialog.len());
        let element = &dialog[..end];
        let direction = attribute(element, "dialog", "direction").unwrap_or_default();
        let s = texts(element, "state")
            .into_iter()
            .next()
            .unwrap_or_default();
        match s.as_str() {
            "confirmed" => state = LineState::Busy,
            "early" | "proceeding" | "trying" if direction == "recipient" => {
                if state == LineState::Idle {
                    state = LineState::Ringing;
                }
            }
            "early" | "proceeding" | "trying" => state = LineState::Busy,
            _ => {}
        }
        rest = &dialog[end.min(dialog.len())..];
        if end == dialog.len() {
            break;
        }
    }
    Some((entity, state))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_compare_by_user_and_host() {
        assert_eq!(aor_key("pres:Bob@A.test"), "bob@a.test");
        assert_eq!(aor_key("<sip:bob@a.test:5060;transport=tcp>"), "bob@a.test");
        assert_eq!(aor_key("sips:bob@a.test"), "bob@a.test");
    }

    #[test]
    fn fcps_pidf_is_read() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?><presence xmlns="urn:ietf:params:xml:ns:pidf" xmlns:dm="urn:ietf:params:xml:ns:pidf:data-model" xmlns:rpid="urn:ietf:params:xml:ns:pidf:rpid" entity="pres:bob@a.test"><tuple id="t1"><status><basic>open</basic></status><note>On the phone</note></tuple><dm:person id="p1"><rpid:activities><rpid:on-the-phone/></rpid:activities></dm:person></presence>"#;
        let (entity, p) = parse_pidf(xml).unwrap();
        assert_eq!(aor_key(&entity), "bob@a.test");
        assert!(p.open);
        assert_eq!(p.activity.as_deref(), Some("on-the-phone"));
        assert_eq!(p.note.as_deref(), Some("On the phone"));
        let closed = r#"<presence xmlns="urn:ietf:params:xml:ns:pidf" entity="pres:x@a"><tuple id="t1"><status><basic>closed</basic></status></tuple></presence>"#;
        let (_, p) = parse_pidf(closed).unwrap();
        assert!(!p.open && p.activity.is_none() && p.note.is_none());
    }

    #[test]
    fn a_lamp_is_read_from_dialog_info() {
        let idle = r#"<dialog-info xmlns="urn:ietf:params:xml:ns:dialog-info" version="0" state="full" entity="sip:bob@a.test"></dialog-info>"#;
        assert_eq!(parse_dialog_info(idle).unwrap().1, LineState::Idle);
        let ringing = r#"<dialog-info xmlns="x" version="1" state="full" entity="sip:bob@a.test"><dialog id="c1" direction="recipient"><state>early</state></dialog></dialog-info>"#;
        assert_eq!(parse_dialog_info(ringing).unwrap().1, LineState::Ringing);
        let busy = r#"<dialog-info xmlns="x" version="2" state="full" entity="sip:bob@a.test"><dialog id="c1" direction="initiator"><state>confirmed</state></dialog></dialog-info>"#;
        assert_eq!(parse_dialog_info(busy).unwrap().1, LineState::Busy);
        let over = r#"<dialog-info xmlns="x" version="3" state="full" entity="sip:bob@a.test"><dialog id="c1"><state>terminated</state></dialog></dialog-info>"#;
        assert_eq!(parse_dialog_info(over).unwrap().1, LineState::Idle);
    }
}
