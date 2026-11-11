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
