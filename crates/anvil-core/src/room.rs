//! A video room on FCP (FCP's `docs/SOFTPHONE_VIDEO.md` §2.6): what a
//! call to one carries beyond any call, and what the room tells it.
//!
//! - Joining: a join link's token goes in `X-FCP-Join`, so the PIN is
//!   skipped, and `Recv-Info: fcp-content` asks for the floor's notices
//!   (RFC 6086). The room's shared screen has its own section, offered
//!   from the start.
//! - The people: a `conference` subscription in the call's dialog
//!   (RFC 4575, RFC 6665). The first NOTIFY is the whole roster, later
//!   ones what changed, each numbered one above the last; a gap asks for
//!   the whole roster again.
//! - The floor: an INFO of [`INFO_TYPE`] in the call says who shares, and
//!   whether our own share was taken or refused.

use std::collections::BTreeMap;

use quick_xml::events::Event as Xml;
use quick_xml::{Reader, XmlVersion};

/// The header a join link's token is sent in.
pub(crate) const JOIN_HEADER: &str = "X-FCP-Join";
/// The info package the floor's notices are sent in.
pub(crate) const INFO_PACKAGE: &str = "fcp-content";
/// The floor notice's type.
pub(crate) const INFO_TYPE: &str = "application/vnd.fcp.content+json";
/// The roster's type (RFC 4575).
pub(crate) const ROSTER_TYPE: &str = "application/conference-info+xml";
/// How long a roster subscription is asked for; it is renewed before then.
pub(crate) const SUBSCRIBE_EXPIRES: u32 = 600;

/// How to join a room.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoomJoin {
    /// A join link's token (`X-FCP-Join`): the room admits without a PIN,
    /// as a guest or a host as the token says. `None`, dialed by number.
    pub token: Option<String>,
}

impl RoomJoin {
    /// The token of a join link: `fcp://join/<token>`, or the gateway's
    /// join page with `?token=<token>` (or `#token=`). A bare token is
    /// taken as it is.
    pub fn from_link(link: &str) -> Option<RoomJoin> {
        let link = link.trim();
        let token = if let Some(rest) = link.strip_prefix("fcp://join/") {
            rest.split(['?', '#'])
                .next()
                .unwrap_or_default()
                .to_string()
        } else if link.starts_with("http://") || link.starts_with("https://") {
            link.split(['?', '#', '&'])
                .skip(1)
                .find_map(|pair| pair.strip_prefix("token="))
                .map(percent_decode)?
        } else {
            link.to_string()
        };
        let token = token.trim().to_string();
        (!token.is_empty() && !token.contains(char::is_whitespace))
            .then_some(RoomJoin { token: Some(token) })
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => match (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                (Some(h), Some(l)) => {
                    out.push((h * 16 + l) as u8);
                    i += 3;
                    continue;
                }
                _ => out.push(b'%'),
            },
            b'+' => out.push(b' '),
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// How a participant's audio reaches the room.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomAudio {
    /// Heard.
    Sending,
    /// Muted, by themselves or a host: they hear the room, it does not
    /// hear them.
    Muted,
    /// On hold.
    Held,
}

/// One participant as the room lists them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomMember {
    /// The room's id for them: what a host's controls name them by.
    pub id: String,
    pub name: String,
    pub host: bool,
    pub audio: RoomAudio,
    /// Their camera as the room has it (`on`, `off`, `lost`, `disabled`,
    /// `failed`), when the room has video.
    pub camera: Option<String>,
    pub speaking: bool,
    /// They share their screen.
    pub presenting: bool,
}

/// What a roster document says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RosterDoc {
    pub full: bool,
    pub version: u32,
    /// Each user listed: in full, or `None` when they left.
    pub users: Vec<(String, Option<RoomMember>)>,
}

/// What applying a document did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Applied {
    /// The roster changed (or was given whole).
    Changed,
    /// An old or repeated document: nothing to tell.
    Stale,
    /// Documents were missed: ask for the whole roster again.
    Gap,
}

/// A room's roster as this call has been told it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Roster {
    members: BTreeMap<String, RoomMember>,
    version: Option<u32>,
    speaker: Option<String>,
}

impl Roster {
    pub(crate) fn apply(&mut self, doc: RosterDoc) -> Applied {
        match (doc.full, self.version) {
            (_, Some(v)) if doc.version <= v => return Applied::Stale,
            (false, None) => return Applied::Gap,
            (false, Some(v)) if doc.version != v + 1 => return Applied::Gap,
            (true, _) => self.members.clear(),
            (false, _) => {}
        }
        self.version = Some(doc.version);
        for (id, member) in doc.users {
            match member {
                Some(m) => {
                    self.members.insert(id, m);
                }
                None => {
                    self.members.remove(&id);
                }
            }
        }
        self.speaker = self.choose_speaker();
        Applied::Changed
    }

    /// Who speaks: the one already shown while they still speak, else the
    /// first speaking; with nobody speaking, the last speaker while they
    /// are still here.
    fn choose_speaker(&self) -> Option<String> {
        let speaking = |id: &String| self.members.get(id).is_some_and(|m| m.speaking);
        if let Some(current) = self.speaker.as_ref().filter(|id| speaking(id)) {
            return Some(current.clone());
        }
        self.members
            .values()
            .find(|m| m.speaking)
            .map(|m| m.id.clone())
            .or_else(|| {
                self.speaker
                    .clone()
                    .filter(|id| self.members.contains_key(id))
            })
    }

    pub(crate) fn members(&self) -> Vec<RoomMember> {
        self.members.values().cloned().collect()
    }

    pub(crate) fn speaker(&self) -> Option<String> {
        self.speaker.clone()
    }

    /// Forget what was told: the next document must be whole.
    pub(crate) fn reset(&mut self) {
        self.version = None;
    }
}

/// The participant id in a roster entity, `sip:<id>@<domain>`.
fn entity_id(entity: &str) -> String {
    let rest = entity
        .strip_prefix("sips:")
        .or_else(|| entity.strip_prefix("sip:"))
        .unwrap_or(entity);
    rest.split('@').next().unwrap_or(rest).to_string()
}

/// A user being read.
#[derive(Default)]
struct UserBuild {
    id: String,
    deleted: bool,
    name: String,
    host: bool,
    audio: Option<RoomAudio>,
    camera: Option<String>,
    speaking: bool,
    presenting: bool,
    /// The media element being read: its `<type>`, `<status>`, `<label>`.
    media: Option<(String, String, String)>,
}

impl UserBuild {
    fn finish(self) -> (String, Option<RoomMember>) {
        if self.deleted {
            return (self.id, None);
        }
        let member = RoomMember {
            id: self.id.clone(),
            name: self.name,
            host: self.host,
            audio: self.audio.unwrap_or(RoomAudio::Sending),
            camera: self.camera,
            speaking: self.speaking,
            presenting: self.presenting,
        };
        (self.id, Some(member))
    }
}

/// An RFC 4575 document, as FCP's rooms write it: `None` when it is not
/// one.
pub(crate) fn parse_roster(xml: &str) -> Option<RosterDoc> {
    let mut reader = Reader::from_str(xml);
    let mut path: Vec<String> = Vec::new();
    let mut doc: Option<RosterDoc> = None;
    let mut user: Option<UserBuild> = None;
    let mut text = String::new();
    loop {
        let event = reader.read_event().ok()?;
        match event {
            Xml::Start(ref e) | Xml::Empty(ref e) => {
                let empty = matches!(event, Xml::Empty(_));
                let name = e.local_name().as_ref().to_string();
                let attr = |key: &str| -> Option<String> {
                    e.try_get_attribute(key)
                        .ok()
                        .flatten()
                        .and_then(|a| a.normalized_value(XmlVersion::Implicit1_0).ok())
                        .map(|v| v.into_owned())
                };
                match name.as_str() {
                    "conference-info" if path.is_empty() => {
                        doc = Some(RosterDoc {
                            full: attr("state").as_deref() != Some("partial"),
                            version: attr("version")?.trim().parse().ok()?,
                            users: Vec::new(),
                        });
                    }
                    "user" if path.last().map(String::as_str) == Some("users") => {
                        user = Some(UserBuild {
                            id: entity_id(&attr("entity")?),
                            deleted: attr("state").as_deref() == Some("deleted"),
                            ..Default::default()
                        });
                    }
                    "media" => {
                        if let Some(u) = user.as_mut() {
                            u.media = Some(Default::default());
                        }
                    }
                    _ => {}
                }
                text.clear();
                if empty {
                    end(&name, &mut user, &mut doc, &mut path, &text);
                } else {
                    path.push(name);
                }
            }
            Xml::Text(t) => text.push_str(&t.xml10_content()),
            Xml::GeneralRef(r) => {
                let name = r.xml10_content();
                match r.resolve_char_ref().ok().flatten() {
                    Some(c) => text.push(c),
                    None => text.push_str(match name.as_ref() {
                        "amp" => "&",
                        "lt" => "<",
                        "gt" => ">",
                        "quot" => "\"",
                        "apos" => "'",
                        _ => "",
                    }),
                }
            }
            Xml::CData(c) => text.push_str(c.as_ref()),
            Xml::End(_) => {
                let name = path.pop()?;
                end(&name, &mut user, &mut doc, &mut path, &text);
                text.clear();
            }
            Xml::Eof => break,
            _ => {}
        }
    }
    doc
}

/// An element closed, `path` holding its parents.
fn end(
    name: &str,
    user: &mut Option<UserBuild>,
    doc: &mut Option<RosterDoc>,
    path: &mut [String],
    text: &str,
) {
    let parent = path.last().map(String::as_str);
    let value = text.trim();
    let Some(u) = user.as_mut() else {
        return;
    };
    match (name, parent) {
        ("user", _) => {
            if let (Some(u), Some(d)) = (user.take(), doc.as_mut()) {
                d.users.push(u.finish());
            }
        }
        ("display-text", Some("user")) => u.name = value.to_string(),
        ("entry", Some("roles")) => u.host |= value.eq_ignore_ascii_case("host"),
        ("type", Some("media")) => {
            if let Some(m) = u.media.as_mut() {
                m.0 = value.to_ascii_lowercase();
            }
        }
        ("status", Some("media")) => {
            if let Some(m) = u.media.as_mut() {
                m.1 = value.to_ascii_lowercase();
            }
        }
        ("label", Some("media")) => {
            if let Some(m) = u.media.as_mut() {
                m.2 = value.to_string();
            }
        }
        ("camera", Some("media")) => u.camera = Some(value.to_string()),
        ("media", _) => {
            if let Some((kind, status, label)) = u.media.take() {
                match kind.as_str() {
                    "audio" => {
                        u.audio = Some(match status.as_str() {
                            "recvonly" => RoomAudio::Muted,
                            "inactive" => RoomAudio::Held,
                            _ => RoomAudio::Sending,
                        })
                    }
                    // The camera's state is its `fcp:camera`; a room
                    // that does not say it has the status.
                    "video" if label != "content" && u.camera.is_none() => {
                        u.camera = Some(
                            if matches!(status.as_str(), "sendrecv" | "sendonly") {
                                "on"
                            } else {
                                "off"
                            }
                            .to_string(),
                        )
                    }
                    _ => {}
                }
            }
        }
        ("speaking", _) => u.speaking = value.eq_ignore_ascii_case("true"),
        ("presenting", _) => u.presenting = value.eq_ignore_ascii_case("true"),
        _ => {}
    }
}

/// What became of the floor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloorState {
    /// Someone shares now.
    Granted,
    /// They stopped, or lost it.
    Released,
    /// A share was turned down.
    Refused,
}

/// A notice of the room's floor (FCP's `application/vnd.fcp.content+json`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FloorNotice {
    pub state: FloorState,
    /// The participant it is about (the room's id for them).
    pub participant: String,
    /// The presenter's name while someone shares: the new one when
    /// granted, the one holding it when a share was refused for that.
    pub presenter: Option<String>,
    /// Why, when released or refused.
    pub reason: Option<String>,
    /// It is about our own share.
    pub mine: bool,
}

pub(crate) fn parse_floor(body: &[u8]) -> Option<FloorNotice> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    let text = |k: &str| v.get(k).and_then(|s| s.as_str()).map(str::to_string);
    let state = match v.get("floor")?.as_str()? {
        "granted" => FloorState::Granted,
        "released" => FloorState::Released,
        "refused" => FloorState::Refused,
        _ => return None,
    };
    Some(FloorNotice {
        state,
        participant: text("participant_id")?,
        presenter: text("presenter"),
        reason: text("reason"),
        mine: v.get("mine").and_then(|m| m.as_bool()).unwrap_or(false),
    })
}

/// Whether an INFO is the floor's: its package, or (from a room that
/// does not name one) its type.
pub(crate) fn is_floor_info(request: &sip_core::Request) -> bool {
    let package = request
        .headers()
        .get("Info-Package")
        .map(|p| p.split(';').next().unwrap_or_default().trim().to_string());
    let kind = request
        .headers()
        .get("Content-Type")
        .map(|t| t.split(';').next().unwrap_or_default().trim().to_string());
    match package {
        Some(p) => p.eq_ignore_ascii_case(INFO_PACKAGE),
        None => kind.is_some_and(|t| t.eq_ignore_ascii_case(INFO_TYPE)),
    }
}

/// A request in `dialog`: its CSeq, Request-URI, route set and Via are
/// the UAC's to set as it sends (`send_in_dialog_non_invite`).
pub(crate) fn in_dialog_request(
    dialog: &sip_dialog::Dialog,
    method: sip_core::Method,
    headers: &[(&str, String)],
    body: &[u8],
) -> Result<sip_core::Request, String> {
    let mut h = sip_core::Headers::new();
    let id = dialog.id();
    let fixed = [
        (
            "Via",
            format!(
                "SIP/2.0/UDP placeholder;branch=z9hG4bK{:016x}",
                rand::random::<u64>()
            ),
        ),
        ("Max-Forwards", "70".to_string()),
        (
            "From",
            format!("<{}>;tag={}", dialog.local_uri().as_str(), id.local_tag()),
        ),
        (
            "To",
            format!("<{}>;tag={}", dialog.remote_uri().as_str(), id.remote_tag()),
        ),
        ("Call-ID", id.call_id().to_string()),
        ("CSeq", format!("1 {}", method.as_str())),
    ];
    for (name, value) in fixed.iter().map(|(n, v)| (*n, v.clone())).chain(
        headers
            .iter()
            .map(|(n, v)| (*n, v.clone()))
            .chain(std::iter::once(("Content-Length", body.len().to_string()))),
    ) {
        h.push(name, value).map_err(|e| format!("{name}: {e}"))?;
    }
    sip_core::Request::new(
        sip_core::RequestLine::new(method.clone(), dialog.remote_target().clone()),
        h,
        bytes::Bytes::copy_from_slice(body),
    )
    .map_err(|e| format!("{}: {e}", method.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A roster as FCP's conference server writes one.
    fn fcp_doc(state: &str, version: u32, users: &str) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <conference-info xmlns=\"urn:ietf:params:xml:ns:conference-info\" \
             xmlns:fcp=\"urn:fcp:params:xml:ns:conference-info\" entity=\"sip:3000@conf.example.com\" \
             state=\"{state}\" version=\"{version}\">\n  \
             <conference-description>\n    <display-text>Conference 3000</display-text>\n  \
             </conference-description>\n  <conference-state>\n    <user-count>2</user-count>\n    \
             <active>true</active>\n  </conference-state>\n  <users>\n{users}  </users>\n\
             </conference-info>\n"
        )
    }

    fn fcp_user(
        id: &str,
        name: &str,
        host: bool,
        audio: &str,
        camera: &str,
        speaking: bool,
        presenting: bool,
    ) -> String {
        let content = if presenting {
            "        <media id=\"content\"><type>video</type><label>content</label><status>sendonly</status></media>\n"
        } else {
            ""
        };
        format!(
            "    <user entity=\"sip:{id}@conf.example.com\" state=\"full\">\n      \
             <display-text>{name}</display-text>\n      \
             <roles><entry>{role}</entry></roles>\n      \
             <endpoint entity=\"sip:{id}@conf.example.com\" state=\"full\">\n        \
             <status>connected</status>\n        \
             <joining-method>dialed-in</joining-method>\n        \
             <joining-info><when>2026-11-06T21:20:00Z</when></joining-info>\n        \
             <media id=\"audio\"><type>audio</type><status>{audio}</status></media>\n        \
             <media id=\"video\"><type>video</type><status>sendrecv</status><fcp:camera>{camera}</fcp:camera></media>\n\
             {content}        <fcp:speaking>{speaking}</fcp:speaking>\n        \
             <fcp:presenting>{presenting}</fcp:presenting>\n      </endpoint>\n    </user>\n",
            role = if host { "host" } else { "participant" },
        )
    }

    #[test]
    fn a_whole_roster_is_read_as_fcp_writes_it() {
        let xml = fcp_doc(
            "full",
            1,
            &(fcp_user(
                "p1",
                "Ann &lt;Lee&gt; &amp; Co",
                true,
                "sendrecv",
                "on",
                false,
                false,
            ) + &fcp_user("p2", "Bob", false, "recvonly", "off", true, true)),
        );
        let doc = parse_roster(&xml).expect("a roster");
        assert!(doc.full);
        assert_eq!(doc.version, 1);
        let ann = doc.users[0].1.clone().unwrap();
        assert_eq!(doc.users[0].0, "p1");
        assert_eq!(ann.name, "Ann <Lee> & Co");
        assert!(ann.host);
        assert_eq!(ann.audio, RoomAudio::Sending);
        assert_eq!(ann.camera.as_deref(), Some("on"));
        let bob = doc.users[1].1.clone().unwrap();
        assert!(!bob.host);
        assert_eq!(bob.audio, RoomAudio::Muted);
        assert_eq!(bob.camera.as_deref(), Some("off"));
        assert!(bob.speaking && bob.presenting);
    }

    #[test]
    fn updates_apply_in_order_and_a_gap_asks_again() {
        let mut roster = Roster::default();
        let partial = |v: u32, users: &str| parse_roster(&fcp_doc("partial", v, users)).unwrap();
        assert_eq!(
            roster.apply(partial(3, "")),
            Applied::Gap,
            "a partial before any whole roster"
        );
        let full = fcp_doc(
            "full",
            4,
            &(fcp_user("p1", "Ann", true, "sendrecv", "on", false, false)
                + &fcp_user("p2", "Bob", false, "sendrecv", "on", false, false)),
        );
        assert_eq!(roster.apply(parse_roster(&full).unwrap()), Applied::Changed);
        assert_eq!(roster.members().len(), 2);
        assert_eq!(roster.speaker(), None);

        // Bob speaks, then Ann leaves.
        let bob = fcp_user("p2", "Bob", false, "sendrecv", "on", true, false);
        assert_eq!(roster.apply(partial(5, &bob)), Applied::Changed);
        assert_eq!(roster.speaker().as_deref(), Some("p2"));
        let gone = "    <user entity=\"sip:p1@conf.example.com\" state=\"deleted\"/>\n";
        assert_eq!(roster.apply(partial(6, gone)), Applied::Changed);
        assert_eq!(
            roster
                .members()
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            ["p2"]
        );
        assert_eq!(roster.apply(partial(6, gone)), Applied::Stale, "a repeat");
        assert_eq!(roster.apply(partial(8, &bob)), Applied::Gap, "7 was missed");

        // Bob falls quiet: still the last speaker while he is here.
        let quiet = fcp_user("p2", "Bob", false, "sendrecv", "on", false, false);
        assert_eq!(roster.apply(partial(7, &quiet)), Applied::Changed);
        assert_eq!(roster.speaker().as_deref(), Some("p2"));

        // A whole roster again replaces everything.
        roster.reset();
        let again = fcp_doc(
            "full",
            9,
            &fcp_user("p3", "Cy", false, "inactive", "off", false, false),
        );
        assert_eq!(
            roster.apply(parse_roster(&again).unwrap()),
            Applied::Changed
        );
        let members = roster.members();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].audio, RoomAudio::Held);
        assert_eq!(roster.speaker(), None, "the last speaker left");
    }

    #[test]
    fn an_ended_room_lists_nobody() {
        let xml = fcp_doc("full", 7, "");
        let mut roster = Roster::default();
        assert_eq!(roster.apply(parse_roster(&xml).unwrap()), Applied::Changed);
        assert!(roster.members().is_empty());
        assert!(parse_roster("not xml <").is_none());
        assert!(parse_roster("<presence entity=\"x\"/>").is_none());
    }

    #[test]
    fn the_floors_notices_are_read() {
        let granted = br#"{"floor":"granted","participant_id":"p2","presenter":"Bob","reason":null,"mine":false}"#;
        assert_eq!(
            parse_floor(granted),
            Some(FloorNotice {
                state: FloorState::Granted,
                participant: "p2".into(),
                presenter: Some("Bob".into()),
                reason: None,
                mine: false,
            })
        );
        let refused = br#"{"floor":"refused","participant_id":"p1","presenter":"Bob","reason":"floor_held","mine":true}"#;
        let notice = parse_floor(refused).unwrap();
        assert_eq!(notice.state, FloorState::Refused);
        assert!(notice.mine);
        assert_eq!(notice.reason.as_deref(), Some("floor_held"));
        assert!(parse_floor(br#"{"floor":"pending","participant_id":"p1"}"#).is_none());
        assert!(parse_floor(b"nonsense").is_none());
    }

    #[test]
    fn a_join_links_token_is_found() {
        let token = |l: &str| RoomJoin::from_link(l).and_then(|j| j.token);
        assert_eq!(token("fcp://join/abc.def").as_deref(), Some("abc.def"));
        assert_eq!(
            token("https://meet.example.com/join?room=3000&token=abc%2Edef").as_deref(),
            Some("abc.def")
        );
        assert_eq!(
            token("https://meet.example.com/join#token=xyz").as_deref(),
            Some("xyz")
        );
        assert_eq!(token("abc.def").as_deref(), Some("abc.def"));
        assert_eq!(token("https://meet.example.com/join?room=3000"), None);
        assert_eq!(token("  "), None);
    }
}
