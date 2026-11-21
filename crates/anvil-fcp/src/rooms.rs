//! The user's video rooms on FCP (its `docs/SOFTPHONE_VIDEO.md` §2.6):
//! the meetings they own, a join link to send for one, and a host's
//! controls over a room while it runs.
//!
//! A host is the meeting's owner, or anyone holding a host join token for
//! the room while it is valid ([`RoomHost::token`], sent as `X-FCP-Join`).
//! FCP refuses (403) anyone else, and 404 a room that is not running.

use serde::{Deserialize, Serialize};

use crate::me::{json, Page};
use crate::{FcpClient, FcpError};

/// A meeting the user owns (`/me/conferences`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Meeting {
    pub id: String,
    /// The number it is dialed at.
    pub room_number: String,
    pub name: String,
    #[serde(default)]
    pub guest_pin: Option<String>,
    #[serde(default)]
    pub host_pin: Option<String>,
    #[serde(default)]
    pub enabled: bool,
}

/// Who a join link admits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JoinRole {
    #[default]
    Guest,
    Host,
}

/// A join link for a meeting: its token, and the gateway's page when FCP
/// has one, to send to whoever is to join.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct JoinLink {
    pub token: String,
    pub room_number: String,
    pub display_name: String,
    pub role: JoinRole,
    /// When it stops admitting (RFC 3339).
    pub expires_at: String,
    /// The browser's join page, when FCP names one.
    #[serde(default)]
    pub join_url: Option<String>,
}

impl JoinLink {
    /// The link an app opens: the browser's page when there is one, else
    /// `fcp://join/<token>`.
    pub fn link(&self) -> String {
        self.join_url
            .clone()
            .unwrap_or_else(|| format!("fcp://join/{}", self.token))
    }
}

/// A room's layout for everyone in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomLayout {
    /// The speaker large.
    Speaker,
    /// Everyone the same size.
    Grid,
}

impl RoomLayout {
    fn name(self) -> &'static str {
        match self {
            RoomLayout::Speaker => "speaker",
            RoomLayout::Grid => "grid",
        }
    }
}

/// A host's controls over the running room `room`.
pub struct RoomHost<'a> {
    client: &'a FcpClient,
    room: String,
    /// A host join token for the room, when the user is not its owner.
    token: Option<String>,
}

/// What FCP accepts as a room or participant in a path.
fn segment(id: &str) -> Result<&str, FcpError> {
    let ok = !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'@' | b'+'));
    if ok {
        Ok(id)
    } else {
        Err(FcpError::Invalid(format!(
            "{id:?} is not a room or participant id"
        )))
    }
}

impl FcpClient {
    /// The meetings the user owns, by room number.
    pub async fn meetings(&self) -> Result<Vec<Meeting>, FcpError> {
        let mut all = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let path = match &cursor {
                Some(c) => format!("/me/conferences?limit=200&cursor={c}"),
                None => "/me/conferences?limit=200".to_string(),
            };
            let page: Page<Meeting> = json(
                self.api_with(reqwest::Method::GET, &path, None).await?,
                "/me/conferences",
            )
            .await?;
            all.extend(page.data);
            match page.next_cursor {
                Some(c) if !c.is_empty() => cursor = Some(c),
                _ => return Ok(all),
            }
        }
    }

    /// A join link for one of the user's meetings: a guest's, or a host's
    /// (whoever holds it can use [`RoomHost`]), valid for `ttl_secs` (FCP's
    /// default, 15 minutes, when `None`; a day at most).
    pub async fn join_link(
        &self,
        meeting: &str,
        role: JoinRole,
        display_name: Option<&str>,
        ttl_secs: Option<u64>,
    ) -> Result<JoinLink, FcpError> {
        let body = serde_json::json!({
            "role": role,
            "display_name": display_name,
            "ttl_secs": ttl_secs,
        });
        json(
            self.api_with(
                reqwest::Method::POST,
                &format!("/me/conferences/{}/join-tokens", segment(meeting)?),
                Some(&body),
            )
            .await?,
            "join-tokens",
        )
        .await
    }

    /// The runs of one of the user's meetings, newest first: what the
    /// attendance report picks among (FCP's `/me/conferences/{id}/sittings`).
    pub async fn sittings(&self, meeting: &str) -> Result<Vec<Sitting>, FcpError> {
        let page: Page<Sitting> = json(
            self.api_with(
                reqwest::Method::GET,
                &format!("/me/conferences/{}/sittings?limit=50", segment(meeting)?),
                None,
            )
            .await?,
            "sittings",
        )
        .await?;
        Ok(page.data)
    }

    /// Who came to a run of one of the user's meetings (the newest when
    /// `sitting` is `None`), how long, and who watched its stream: FCP's
    /// `/me/conferences/{id}/attendance`.
    pub async fn attendance(
        &self,
        meeting: &str,
        sitting: Option<&str>,
    ) -> Result<Attendance, FcpError> {
        let mut path = format!("/me/conferences/{}/attendance", segment(meeting)?);
        if let Some(s) = sitting {
            path.push_str(&format!("?sitting={}", segment(s)?));
        }
        json(
            self.api_with(reqwest::Method::GET, &path, None).await?,
            "attendance",
        )
        .await
    }

    /// A host's controls over the running room `room` (its number), as
    /// its owner or, with `token`, a host join token's holder.
    pub fn room_host(&self, room: &str, token: Option<&str>) -> RoomHost<'_> {
        RoomHost {
            client: self,
            room: room.to_string(),
            token: token.map(str::to_string),
        }
    }
}

impl RoomHost<'_> {
    async fn send(
        &self,
        method: reqwest::Method,
        rest: &str,
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, FcpError> {
        let path = format!("/me/conferences/live/{}{rest}", segment(&self.room)?);
        let token = self.token.clone();
        let response = self
            .client
            .api_request(method, &path, move |mut request| {
                if let Some(token) = &token {
                    request = request.header("X-FCP-Join", token);
                }
                match &body {
                    Some(body) => request.json(body),
                    None => request,
                }
            })
            .await?;
        let bytes = response
            .bytes()
            .await
            .map_err(|e| FcpError::Invalid(format!("the room's answer: {e}")))?;
        if bytes.is_empty() {
            return Ok(serde_json::Value::Null);
        }
        serde_json::from_slice(&bytes)
            .map_err(|e| FcpError::Invalid(format!("the room's answer: {e}")))
    }

    /// The room as it is now: its participants, video and floor, as the
    /// conference server says them.
    pub async fn room(&self) -> Result<serde_json::Value, FcpError> {
        self.send(reqwest::Method::GET, "", None).await
    }

    pub async fn set_layout(&self, layout: RoomLayout) -> Result<(), FcpError> {
        let body = serde_json::json!({ "layout": layout.name() });
        self.send(reqwest::Method::PUT, "/video/layout", Some(body))
            .await
            .map(drop)
    }

    /// Keep `participant` in the large tile.
    pub async fn pin(&self, participant: &str) -> Result<(), FcpError> {
        let path = format!("/video/pin/{}", segment(participant)?);
        self.send(reqwest::Method::POST, &path, None)
            .await
            .map(drop)
    }

    pub async fn unpin(&self) -> Result<(), FcpError> {
        self.send(reqwest::Method::DELETE, "/video/pin", None)
            .await
            .map(drop)
    }

    /// Show `participant` alone, to everyone.
    pub async fn spotlight(&self, participant: &str) -> Result<(), FcpError> {
        let path = format!("/video/spotlight/{}", segment(participant)?);
        self.send(reqwest::Method::POST, &path, None)
            .await
            .map(drop)
    }

    pub async fn unspotlight(&self) -> Result<(), FcpError> {
        self.send(reqwest::Method::DELETE, "/video/spotlight", None)
            .await
            .map(drop)
    }

    /// Mute `participant` (`true`) for the room, or unmute them.
    pub async fn mute(&self, participant: &str, on: bool) -> Result<(), FcpError> {
        let path = format!(
            "/participants/{}/{}",
            segment(participant)?,
            if on { "mute" } else { "unmute" }
        );
        self.send(reqwest::Method::POST, &path, None)
            .await
            .map(drop)
    }

    /// Stop `participant`'s camera reaching the room (`false`), or let it
    /// again.
    pub async fn participant_video(&self, participant: &str, on: bool) -> Result<(), FcpError> {
        let path = format!(
            "/participants/{}/video/{}",
            segment(participant)?,
            if on { "on" } else { "off" }
        );
        self.send(reqwest::Method::POST, &path, None)
            .await
            .map(drop)
    }

    /// Put `participant` out of the room.
    pub async fn remove(&self, participant: &str) -> Result<(), FcpError> {
        let path = format!("/participants/{}", segment(participant)?);
        self.send(reqwest::Method::DELETE, &path, None)
            .await
            .map(drop)
    }

    /// End whoever's share is on.
    pub async fn stop_content(&self) -> Result<(), FcpError> {
        self.send(reqwest::Method::POST, "/video/content/stop", None)
            .await
            .map(drop)
    }

    // A webinar's room controls (FCP's `docs/WEBINARS.md` §2.6, A7b): what
    // the call has no words for. One's own hand, questions and chat go in
    // the call (anvil-core's `room_qa` and `room_chat`).

    /// The webinar's sitting now: its phase, lobby, stream and push.
    pub async fn webinar(&self) -> Result<serde_json::Value, FcpError> {
        self.send(reqwest::Method::GET, "/webinar", None).await
    }

    /// Take the webinar live: the lobby is let in, and it records and
    /// streams if its meeting says to.
    pub async fn go_live(&self) -> Result<(), FcpError> {
        self.send(reqwest::Method::POST, "/webinar/live", None)
            .await
            .map(drop)
    }

    /// End the webinar: everyone in it is released.
    pub async fn end_webinar(&self) -> Result<(), FcpError> {
        self.send(reqwest::Method::POST, "/webinar/end", None)
            .await
            .map(drop)
    }

    /// The audience by page: `limit` of them after `cursor`.
    pub async fn audience(
        &self,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<Page<AudienceMember>, FcpError> {
        let mut rest = format!("/audience?limit={}", limit.clamp(1, 200));
        if let Some(c) = cursor {
            // Opaque: whatever it holds, sent as it was given.
            let encoded: String = c
                .bytes()
                .map(|b| match b {
                    b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                        (b as char).to_string()
                    }
                    _ => format!("%{b:02X}"),
                })
                .collect();
            rest.push_str(&format!("&cursor={encoded}"));
        }
        let value = self.send(reqwest::Method::GET, &rest, None).await?;
        serde_json::from_value(value).map_err(|e| FcpError::Invalid(format!("the audience: {e}")))
    }

    /// Lower `participant`'s raised hand.
    pub async fn lower_hand(&self, participant: &str) -> Result<(), FcpError> {
        let path = format!("/participants/{}/hand", segment(participant)?);
        self.send(reqwest::Method::DELETE, &path, None)
            .await
            .map(drop)
    }

    /// Lower every raised hand.
    pub async fn lower_all_hands(&self) -> Result<(), FcpError> {
        self.send(reqwest::Method::DELETE, "/hands", None)
            .await
            .map(drop)
    }

    /// Bring an attendee to the stage, heard by the room (`true`), or
    /// send them back to the audience.
    pub async fn stage(&self, participant: &str, on: bool) -> Result<(), FcpError> {
        let path = format!("/participants/{}/stage", segment(participant)?);
        let method = if on {
            reqwest::Method::POST
        } else {
            reqwest::Method::DELETE
        };
        self.send(method, &path, None).await.map(drop)
    }

    /// Mute `participant`'s chat for the rest of the sitting, or unmute it.
    pub async fn mute_chat(&self, participant: &str, on: bool) -> Result<(), FcpError> {
        let path = format!("/participants/{}/chat-mute", segment(participant)?);
        self.send(
            reqwest::Method::POST,
            &path,
            Some(serde_json::json!({ "on": on })),
        )
        .await
        .map(drop)
    }
}

/// One run of a meeting's room.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Sitting {
    pub id: String,
    #[serde(default)]
    pub room_number: String,
    /// `meeting` or `webinar`.
    #[serde(default)]
    pub kind: String,
    /// RFC 3339.
    pub started_at: String,
    #[serde(default)]
    pub live_at: Option<String>,
    #[serde(default)]
    pub ended_at: Option<String>,
    #[serde(default)]
    pub ended_by: Option<String>,
}

/// A sitting's attendance, as FCP reports it to the meeting's host.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Attendance {
    pub sitting: Sitting,
    /// Distinct people in the room, the lobby aside.
    #[serde(default)]
    pub attendees: u64,
    /// The most in the room at once.
    #[serde(default)]
    pub peak: u64,
    #[serde(default)]
    pub legs: Vec<AttendanceLeg>,
    /// A webinar's registrations, each with whether they came or watched.
    #[serde(default)]
    pub registrants: Vec<AttendanceRegistrant>,
    /// Who watched the stream by a registrant's link.
    #[serde(default)]
    pub stream_viewers: Vec<StreamViewer>,
}

/// One leg of a sitting: someone's time in the room.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct AttendanceLeg {
    #[serde(default)]
    pub participant_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    /// Where they called from.
    #[serde(default)]
    pub from: String,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub registrant: Option<String>,
    pub joined_at: String,
    #[serde(default)]
    pub left_at: Option<String>,
    #[serde(default)]
    pub duration_secs: u64,
    /// They never left the lobby.
    #[serde(default)]
    pub lobby: bool,
    #[serde(default)]
    pub questions: u32,
    #[serde(default)]
    pub messages: u32,
}

/// A registration and what became of it in the sitting.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct AttendanceRegistrant {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub attended: bool,
    #[serde(default)]
    pub attended_secs: u64,
    #[serde(default)]
    pub watched: bool,
    #[serde(default)]
    pub watched_secs: u64,
    #[serde(default)]
    pub revoked_at: Option<String>,
}

/// A registrant who watched the stream by their own link.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct StreamViewer {
    pub registrant_id: String,
    #[serde(default)]
    pub name: Option<String>,
    pub first_at: String,
    pub last_at: String,
    #[serde(default)]
    pub watched_secs: u64,
}

/// One of a webinar's audience, as the stage's host lists them.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct AudienceMember {
    pub participant_id: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub caller_id: Option<String>,
    /// The registration they joined by.
    #[serde(default)]
    pub registrant: Option<String>,
    /// RFC 3339.
    pub joined_at: String,
    /// Since when their hand is up.
    #[serde(default)]
    pub hand: Option<String>,
    /// Waiting in the lobby.
    #[serde(default)]
    pub lobby: bool,
    /// Brought to the stage.
    #[serde(default)]
    pub on_stage: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_ids_go_in_a_path() {
        assert!(segment("3000").is_ok());
        assert!(segment("call-1@host").is_ok());
        assert!(segment("").is_err());
        assert!(segment("../admin").is_err());
        assert!(segment("a/b").is_err());
        assert!(segment("a b").is_err());
    }

    #[test]
    fn a_join_link_opens_the_browsers_page_or_the_apps() {
        let mut link = JoinLink {
            token: "tok".into(),
            room_number: "3000".into(),
            display_name: "Guest".into(),
            role: JoinRole::Guest,
            expires_at: "2026-11-10T12:00:00Z".into(),
            join_url: None,
        };
        assert_eq!(link.link(), "fcp://join/tok");
        link.join_url = Some("https://meet.example.com/join?token=tok".into());
        assert_eq!(link.link(), "https://meet.example.com/join?token=tok");
        let parsed: JoinLink = serde_json::from_str(
            r#"{"token":"t","room_number":"3000","display_name":"Host","role":"host","expires_at":"2026-11-10T12:00:00Z"}"#,
        )
        .unwrap();
        assert_eq!(parsed.role, JoinRole::Host);
    }
}
