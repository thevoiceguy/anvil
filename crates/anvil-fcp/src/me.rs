//! What a user sees through FCP's API, beyond what SIP carries (FCP's
//! `docs/SOFTPHONE.md` §2.7): call history, the directory with presence,
//! voicemail, and the user's own live events.

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio_tungstenite::tungstenite::Message;

use crate::{FcpClient, FcpError};

/// One page of a list: FCP's `{data, next_cursor, total}`. Pass
/// `next_cursor` back for the next page; `None` means this was the last.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Page<T> {
    pub data: Vec<T>,
    pub next_cursor: Option<String>,
    pub total: Option<u64>,
}

/// One call in the user's history (`/me/calls`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct CallRecord {
    pub cdr_id: String,
    pub call_id: String,
    /// `incoming` or `outgoing`, from the user's side.
    pub direction: String,
    /// Who the call was with: a name, an extension or a number.
    pub other_party: String,
    pub other_party_name: Option<String>,
    /// How it ended (`answered`, `no_answer`, `busy`, `failed`, …).
    pub disposition: String,
    /// An inbound call the user did not answer.
    pub missed: bool,
    /// RFC 3339.
    pub start_time: String,
    pub answer_time: Option<String>,
    pub end_time: Option<String>,
    pub duration_seconds: Option<u64>,
}

/// One entry in the tenant's directory (`/me/directory`): a user, or an
/// extension that is not a user's (a queue, a ring group, a flow).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct DirectoryEntry {
    /// `user` or `extension`.
    pub kind: String,
    pub user_id: Option<String>,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub extension: Option<String>,
    pub email: Option<String>,
    pub department: Option<String>,
    pub job_title: Option<String>,
    /// The user's presence (`available`, `busy`, `away`, `dnd`, `offline`,
    /// …) when the tenant shows it.
    pub presence: Option<String>,
    pub presence_note: Option<String>,
}

/// One voicemail message (`/me/voicemail/messages`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct VoicemailMessage {
    pub id: String,
    pub mailbox_id: String,
    pub caller: String,
    pub caller_name: Option<String>,
    /// `new`, `heard` or `saved`.
    pub status: String,
    /// `normal` or `urgent`.
    pub priority: String,
    /// Seconds.
    pub duration: u64,
    pub transcription: Option<String>,
    /// RFC 3339.
    pub created_at: String,
    pub accessed_at: Option<String>,
}

/// The mailbox's counts (`/me/voicemail/stats`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct MailboxStats {
    pub mailbox_id: String,
    pub total_messages: u64,
    pub new_messages: u64,
    pub heard_messages: u64,
    pub saved_messages: u64,
}

/// Which calls [`FcpClient::calls`] lists.
#[derive(Debug, Clone, Default)]
pub struct CallQuery {
    pub limit: Option<u32>,
    pub cursor: Option<String>,
    /// Only the calls the user missed.
    pub missed: bool,
}

/// One of the user's live events (`/me/events`): FCP's event as it came,
/// named `{event_type}.{type}` — `call.answered`, `voicemail.mwi_updated`,
/// `user.calling_settings_changed`, `presence.dialog_changed`, …. FCP
/// sends each flat: `{"event_type": "call", "type": "answered",
/// "call_id": …}`.
#[derive(Debug, Clone, PartialEq)]
pub struct UserEvent {
    pub name: String,
    /// The event's own fields (`call_id`, `aor`, counts, …).
    pub body: serde_json::Value,
}

impl UserEvent {
    /// The event from FCP's message: flat, `{"event_type": "call", "type":
    /// "answered", …}`, or nested under its family as older documents show
    /// it. `None` for a message that is not an event.
    pub fn from_message(message: &serde_json::Value) -> Option<Self> {
        let family = message.get("event_type")?.as_str()?;
        let body = match message.get(family) {
            Some(nested) if nested.is_object() => nested.clone(),
            _ => message.clone(),
        };
        let kind = body.get("type").and_then(|t| t.as_str()).unwrap_or("");
        Some(Self {
            name: if kind.is_empty() {
                family.to_string()
            } else {
                format!("{family}.{kind}")
            },
            body,
        })
    }

    /// The call the event is about, when it is about one.
    pub fn call_id(&self) -> Option<&str> {
        self.body.get("call_id").and_then(|v| v.as_str())
    }
}

/// The user's live events: a WebSocket to `/me/events`, signed in. Read
/// with [`next`](Self::next); when it answers `None` the stream has ended
/// and the app connects again.
pub struct UserEvents {
    socket: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
}

impl UserEvents {
    /// The next event; `None` once the stream has closed.
    pub async fn next(&mut self) -> Option<UserEvent> {
        while let Some(message) = self.socket.next().await {
            match message {
                Ok(Message::Text(text)) => {
                    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
                        continue;
                    };
                    if let Some(event) = UserEvent::from_message(&value) {
                        return Some(event);
                    }
                }
                Ok(Message::Ping(payload)) => {
                    let _ = self.socket.send(Message::Pong(payload)).await;
                }
                Ok(Message::Close(_)) | Err(_) => return None,
                Ok(_) => {}
            }
        }
        None
    }

    /// Close the stream.
    pub async fn close(mut self) {
        let _ = self.socket.close(None).await;
    }
}

/// `path` with its query, the empty and unset parameters left out.
fn with_query(path: &str, params: &[(&str, Option<String>)]) -> String {
    let mut url = url::Url::parse("http://x").expect("a base URL parses");
    url.set_path(path);
    {
        let mut q = url.query_pairs_mut();
        for (name, value) in params {
            if let Some(value) = value.as_deref().filter(|v| !v.is_empty()) {
                q.append_pair(name, value);
            }
        }
    }
    match url.query().filter(|q| !q.is_empty()) {
        Some(q) => format!("{path}?{q}"),
        None => path.to_string(),
    }
}

/// The WebSocket URL for an API path: `https` becomes `wss`.
fn websocket_url(api: &str, path: &str) -> Result<String, FcpError> {
    let base = api.trim_end_matches('/');
    let ws = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        return Err(FcpError::Invalid(format!(
            "the API address {api} is not http(s)"
        )));
    };
    Ok(format!("{ws}{path}"))
}

async fn json<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
    what: &str,
) -> Result<T, FcpError> {
    response
        .json()
        .await
        .map_err(|e| FcpError::Invalid(format!("{what}: {e}")))
}

impl FcpClient {
    /// The user's call history, newest first.
    pub async fn calls(&self, query: &CallQuery) -> Result<Page<CallRecord>, FcpError> {
        let path = with_query(
            "/me/calls",
            &[
                ("limit", query.limit.map(|l| l.to_string())),
                ("cursor", query.cursor.clone()),
                ("missed", query.missed.then(|| "true".to_string())),
            ],
        );
        json(
            self.api_with(reqwest::Method::GET, &path, None).await?,
            "/me/calls",
        )
        .await
    }

    /// The tenant's directory, matching `search` when given (a name,
    /// username, extension or email).
    pub async fn directory(
        &self,
        search: Option<&str>,
        limit: Option<u32>,
        cursor: Option<&str>,
    ) -> Result<Page<DirectoryEntry>, FcpError> {
        let path = with_query(
            "/me/directory",
            &[
                ("q", search.map(str::to_string)),
                ("limit", limit.map(|l| l.to_string())),
                ("cursor", cursor.map(str::to_string)),
            ],
        );
        json(
            self.api_with(reqwest::Method::GET, &path, None).await?,
            "/me/directory",
        )
        .await
    }

    /// The user's voicemail messages, newest first.
    pub async fn voicemail(
        &self,
        limit: Option<u32>,
        cursor: Option<&str>,
    ) -> Result<Page<VoicemailMessage>, FcpError> {
        let path = with_query(
            "/me/voicemail/messages",
            &[
                ("limit", limit.map(|l| l.to_string())),
                ("cursor", cursor.map(str::to_string)),
            ],
        );
        json(
            self.api_with(reqwest::Method::GET, &path, None).await?,
            "/me/voicemail/messages",
        )
        .await
    }

    /// The mailbox's counts.
    pub async fn voicemail_stats(&self) -> Result<MailboxStats, FcpError> {
        json(
            self.api_with(reqwest::Method::GET, "/me/voicemail/stats", None)
                .await?,
            "/me/voicemail/stats",
        )
        .await
    }

    /// A message's recording (WAV).
    pub async fn voicemail_audio(&self, id: &str) -> Result<Vec<u8>, FcpError> {
        let response = self
            .api_with(
                reqwest::Method::GET,
                &format!("/me/voicemail/messages/{id}/audio"),
                None,
            )
            .await?;
        Ok(response.bytes().await?.to_vec())
    }

    /// Mark a message `heard`, `saved` or `new`.
    pub async fn set_voicemail_status(
        &self,
        id: &str,
        status: &str,
    ) -> Result<VoicemailMessage, FcpError> {
        let body = serde_json::json!({ "status": status });
        json(
            self.api_with(
                reqwest::Method::PUT,
                &format!("/me/voicemail/messages/{id}"),
                Some(&body),
            )
            .await?,
            "/me/voicemail/messages",
        )
        .await
    }

    /// Delete a message.
    pub async fn delete_voicemail(&self, id: &str) -> Result<(), FcpError> {
        self.api_with(
            reqwest::Method::DELETE,
            &format!("/me/voicemail/messages/{id}"),
            None,
        )
        .await?;
        Ok(())
    }

    /// Open the user's live events (`/me/events`): the WebSocket signed in
    /// with the session's access token.
    pub async fn events(&self) -> Result<UserEvents, FcpError> {
        let api = self.session().await.server.api_url;
        let url = websocket_url(&api, "/me/events")?;
        let (mut socket, _) = tokio_tungstenite::connect_async(url.as_str())
            .await
            .map_err(|e| FcpError::Unreachable(format!("{url}: {e}")))?;
        let auth = serde_json::json!({ "type": "auth", "token": self.bearer().await? });
        socket
            .send(Message::Text(auth.to_string()))
            .await
            .map_err(|e| FcpError::Unreachable(format!("{url}: {e}")))?;
        // The answer to the sign-in comes first: `auth_ok` or `auth_error`.
        let answer = tokio::time::timeout(std::time::Duration::from_secs(10), socket.next())
            .await
            .map_err(|_| FcpError::Unreachable(format!("{url}: no answer to signing in")))?;
        match answer {
            Some(Ok(Message::Text(text))) => {
                let value: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
                match value["type"].as_str() {
                    Some("auth_ok") => Ok(UserEvents { socket }),
                    Some("auth_error") => Err(FcpError::Refused {
                        status: 401,
                        message: value["message"].as_str().unwrap_or_default().to_string(),
                    }),
                    _ => Err(FcpError::Invalid(format!("/me/events answered {text}"))),
                }
            }
            other => Err(FcpError::Unreachable(format!(
                "{url}: the stream closed while signing in ({other:?})"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_event_is_named_by_its_family_and_type() {
        let flat = serde_json::json!({
            "event_type": "call", "type": "initiated", "call_id": "call-2",
            "from": "sip:a@x", "to": "<sip:b@x>"
        });
        let event = UserEvent::from_message(&flat).unwrap();
        assert_eq!(event.name, "call.initiated");
        assert_eq!(event.call_id(), Some("call-2"));
        let message = serde_json::json!({
            "event_type": "call",
            "call": {"type": "answered", "call_id": "call-1"}
        });
        let event = UserEvent::from_message(&message).unwrap();
        assert_eq!(event.name, "call.answered");
        assert_eq!(event.call_id(), Some("call-1"));
        assert!(UserEvent::from_message(&serde_json::json!({"type": "connected"})).is_none());
    }

    #[test]
    fn a_query_leaves_out_what_is_unset() {
        assert_eq!(with_query("/me/calls", &[("limit", None)]), "/me/calls");
        assert_eq!(
            with_query(
                "/me/directory",
                &[
                    ("q", Some("ann lee".into())),
                    ("cursor", Some(String::new()))
                ]
            ),
            "/me/directory?q=ann+lee"
        );
    }

    #[test]
    fn the_events_url_follows_the_api() {
        assert_eq!(
            websocket_url("https://pbx.example/api/v1", "/me/events").unwrap(),
            "wss://pbx.example/api/v1/me/events"
        );
        assert_eq!(
            websocket_url("http://127.0.0.1:18080/api/v1/", "/me/events").unwrap(),
            "ws://127.0.0.1:18080/api/v1/me/events"
        );
    }
}
