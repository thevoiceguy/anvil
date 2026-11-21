//! A room's audience on FCP (FCP's `docs/WEBINARS.md` §2.5 and §2.9, A7):
//! what a room call says and is told beyond its floor and view.
//!
//! - **`fcp-qa`** ([`QA_PACKAGE`]): one's raised hand, being brought to the
//!   stage, questions, and a webinar's phase. The room tells us our hand
//!   and whether we are on the stage, the questions we may see, and the
//!   webinar's phase, whether we wait in its lobby, and our role.
//! - **`fcp-chat`** ([`CHAT_PACKAGE`]): chat in every room, meetings as
//!   well as webinars. The room sends the messages we may see, and tells
//!   us when our chat is muted.
//!
//! Both travel as INFOs in the call (RFC 6086), each a JSON object with an
//! `op`. A host's or panelist's question moderation, deleting a message
//! and muting someone's chat travel the same way. A host's room controls
//! (going live and ending, the stage, hands other than one's own) are
//! FCP's `/me` routes, not the call's.

use serde::{Deserialize, Serialize};

/// The info package of hands, the stage, questions and a webinar's phase.
pub const QA_PACKAGE: &str = "fcp-qa";
/// Its body's type.
pub const QA_TYPE: &str = "application/vnd.fcp.qa+json";
/// The info package of chat.
pub const CHAT_PACKAGE: &str = "fcp-chat";
/// Its body's type.
pub const CHAT_TYPE: &str = "application/vnd.fcp.chat+json";

/// Where a webinar's sitting is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebinarPhase {
    /// The stage is in the room; attendees wait in the lobby.
    Practice,
    /// The audience is in.
    Live,
    /// Over: everyone is released.
    Ended,
}

/// Who one is in a room, as the room says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomRole {
    /// A host: the room's controls.
    Host,
    /// A webinar's panelist: on the stage, moderating questions and chat.
    Panelist,
    /// A webinar's attendee: hears the stage, is not heard.
    Attendee,
    /// A meeting's ordinary participant.
    Guest,
}

impl RoomRole {
    /// Whether one moderates questions and chat: a host or a panelist.
    pub fn moderates(self) -> bool {
        matches!(self, RoomRole::Host | RoomRole::Panelist)
    }
}

/// A webinar as the room tells it to us.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebinarState {
    pub phase: WebinarPhase,
    /// We wait in the lobby: a waiting screen until we are let in.
    pub lobby: bool,
    /// Who we are, when the room says.
    pub role: Option<RoomRole>,
}

/// Where a question is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionStatus {
    Pending,
    Answered,
    Dismissed,
}

/// A question as the room shows it to us.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Question {
    /// `q-<uuid>`: what upvoting, publishing and answering name it by.
    pub id: String,
    pub text: String,
    /// The asker's name; `None` when asked anonymously or nameless.
    #[serde(default)]
    pub asker: Option<String>,
    /// We asked it.
    #[serde(default)]
    pub mine: bool,
    pub status: QuestionStatus,
    /// The whole room sees it.
    #[serde(default)]
    pub published: bool,
    #[serde(default)]
    pub upvotes: u32,
    /// We upvoted it.
    #[serde(default)]
    pub upvoted: bool,
    /// The written answer.
    #[serde(default)]
    pub answer: Option<String>,
    /// Answered aloud.
    #[serde(default)]
    pub answered_live: bool,
    /// When it was asked, RFC 3339.
    #[serde(default)]
    pub asked_at: String,
}

/// Whom a chat message is to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatTo {
    /// Everyone in the room.
    Room,
    /// The hosts and panelists alone.
    Hosts,
}

/// A chat message as the room shows it to us.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ChatMessage {
    /// `m-<uuid>`.
    pub id: String,
    /// Empty once deleted.
    #[serde(default)]
    pub text: String,
    /// The sender's name.
    #[serde(default)]
    pub sender: Option<String>,
    /// The sender's participant id in this sitting: what muting names.
    #[serde(default)]
    pub sender_id: String,
    /// Sent by a host or panelist.
    #[serde(default)]
    pub sender_moderates: bool,
    /// We sent it.
    #[serde(default)]
    pub mine: bool,
    pub to: ChatTo,
    /// Deleted by a host or panelist: drop it.
    #[serde(default)]
    pub deleted: bool,
    /// When it was posted, RFC 3339.
    #[serde(default)]
    pub sent_at: String,
}

/// What an `fcp-qa` INFO from the room says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QaNotice {
    /// Our hand is up or down: lowered by us, a host, or our being brought
    /// to the stage.
    Hand { raised: bool },
    /// We were brought to the stage (heard by the room), or sent back.
    Stage { on: bool },
    /// The questions we may see, as they are now.
    Questions(Vec<Question>),
    /// The webinar's phase, our lobby and our role.
    Webinar(WebinarState),
}

/// What an `fcp-chat` INFO from the room says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatNotice {
    /// New messages, or ones changed (deleted); the backlog on joining.
    Messages(Vec<ChatMessage>),
    /// Our chat was muted, or unmuted, for the rest of the sitting.
    Muted { on: bool },
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum QaIn {
    Hand {
        raised: bool,
    },
    Stage {
        on: bool,
    },
    Questions {
        #[serde(default)]
        questions: Vec<Question>,
    },
    Webinar {
        phase: WebinarPhase,
        #[serde(default)]
        lobby: bool,
        #[serde(default)]
        role: Option<RoomRole>,
    },
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum ChatIn {
    Messages {
        #[serde(default)]
        messages: Vec<ChatMessage>,
    },
    Muted {
        on: bool,
    },
}

/// An `fcp-qa` body from the room; `None` when it is not one we read.
pub fn parse_qa(body: &[u8]) -> Option<QaNotice> {
    Some(match serde_json::from_slice::<QaIn>(body).ok()? {
        QaIn::Hand { raised } => QaNotice::Hand { raised },
        QaIn::Stage { on } => QaNotice::Stage { on },
        QaIn::Questions { questions } => QaNotice::Questions(questions),
        QaIn::Webinar { phase, lobby, role } => {
            QaNotice::Webinar(WebinarState { phase, lobby, role })
        }
    })
}

/// An `fcp-chat` body from the room; `None` when it is not one we read.
pub fn parse_chat(body: &[u8]) -> Option<ChatNotice> {
    Some(match serde_json::from_slice::<ChatIn>(body).ok()? {
        ChatIn::Messages { messages } => ChatNotice::Messages(messages),
        ChatIn::Muted { on } => ChatNotice::Muted { on },
    })
}

/// What we say to the room in `fcp-qa`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum QaRequest {
    /// Raise or lower our hand.
    Hand { raised: bool },
    /// Ask a question; `anonymous` where the room allows it.
    Ask { text: String, anonymous: bool },
    /// Upvote another's question, or take the vote back.
    Upvote { id: String, on: bool },
    /// For the stage: let the whole room see a question.
    Publish { id: String },
    /// For the stage: answer in text, or say it was answered aloud.
    Answer {
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        live: bool,
    },
    /// For the stage: drop a question.
    Dismiss { id: String },
}

/// What we say to the room in `fcp-chat`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ChatRequest {
    /// Post to the room, or to its hosts and panelists.
    Post { text: String, to: ChatTo },
    /// For the stage: delete a message for everyone.
    Delete { id: String },
    /// For the stage: mute a participant's chat for the sitting, or not.
    Mute { participant_id: String, on: bool },
}

impl QaRequest {
    pub fn body(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("a request serializes")
    }
}

impl ChatRequest {
    pub fn body(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("a request serializes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_room_s_qa_notices_are_read() {
        assert_eq!(
            parse_qa(br#"{"op":"hand","raised":false}"#),
            Some(QaNotice::Hand { raised: false })
        );
        assert_eq!(
            parse_qa(br#"{"op":"stage","on":true}"#),
            Some(QaNotice::Stage { on: true })
        );
        assert_eq!(
            parse_qa(br#"{"op":"webinar","phase":"practice","lobby":true,"role":"attendee"}"#),
            Some(QaNotice::Webinar(WebinarState {
                phase: WebinarPhase::Practice,
                lobby: true,
                role: Some(RoomRole::Attendee),
            }))
        );
        // A room from before roles: none said.
        assert_eq!(
            parse_qa(br#"{"op":"webinar","phase":"live"}"#),
            Some(QaNotice::Webinar(WebinarState {
                phase: WebinarPhase::Live,
                lobby: false,
                role: None,
            }))
        );
        let Some(QaNotice::Questions(q)) = parse_qa(
            br#"{"op":"questions","questions":[{"id":"q-1","text":"When?","asker":null,
            "mine":true,"status":"answered","published":true,"upvotes":3,"upvoted":false,
            "answer":"In May.","answered_live":false,"asked_at":"2026-11-21T10:00:00.000Z"}]}"#,
        ) else {
            panic!("questions")
        };
        assert_eq!(q[0].id, "q-1");
        assert_eq!(q[0].status, QuestionStatus::Answered);
        assert_eq!(q[0].answer.as_deref(), Some("In May."));
        assert!(q[0].mine && q[0].published && q[0].asker.is_none());
        assert_eq!(
            parse_qa(br#"{"op":"ask","text":"x"}"#),
            None,
            "not the room's"
        );
        assert_eq!(parse_qa(b"not json"), None);
    }

    #[test]
    fn the_room_s_chat_is_read() {
        let Some(ChatNotice::Messages(m)) = parse_chat(
            br#"{"op":"messages","messages":[{"id":"m-1","text":"Hello","sender":"Ann",
            "sender_id":"p-1","sender_moderates":false,"mine":false,"to":"room",
            "deleted":false,"sent_at":"2026-11-21T10:00:00.000Z"},
            {"id":"m-2","text":"","sender":"Ben","sender_id":"p-2","sender_moderates":false,
            "mine":false,"to":"hosts","deleted":true,"sent_at":"2026-11-21T10:00:01.000Z"}]}"#,
        ) else {
            panic!("messages")
        };
        assert_eq!(m[0].to, ChatTo::Room);
        assert_eq!(m[0].sender.as_deref(), Some("Ann"));
        assert!(m[1].deleted && m[1].text.is_empty());
        assert_eq!(
            parse_chat(br#"{"op":"muted","on":true}"#),
            Some(ChatNotice::Muted { on: true })
        );
    }

    #[test]
    fn our_requests_are_what_the_room_reads() {
        let body = |r: QaRequest| String::from_utf8(r.body()).unwrap();
        assert_eq!(
            body(QaRequest::Hand { raised: true }),
            r#"{"op":"hand","raised":true}"#
        );
        assert_eq!(
            body(QaRequest::Ask {
                text: "When?".into(),
                anonymous: false
            }),
            r#"{"op":"ask","text":"When?","anonymous":false}"#
        );
        assert_eq!(
            body(QaRequest::Answer {
                id: "q-1".into(),
                text: None,
                live: true
            }),
            r#"{"op":"answer","id":"q-1","live":true}"#
        );
        let chat = |r: ChatRequest| String::from_utf8(r.body()).unwrap();
        assert_eq!(
            chat(ChatRequest::Post {
                text: "Hi".into(),
                to: ChatTo::Hosts
            }),
            r#"{"op":"post","text":"Hi","to":"hosts"}"#
        );
        assert_eq!(
            chat(ChatRequest::Mute {
                participant_id: "p-2".into(),
                on: true
            }),
            r#"{"op":"mute","participant_id":"p-2","on":true}"#
        );
        assert!(RoomRole::Panelist.moderates() && !RoomRole::Attendee.moderates());
    }
}
