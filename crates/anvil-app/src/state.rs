//! What the phone shows: its registration, its calls, message waiting and
//! do not disturb, and, signed in to FCP, the user's recent calls, the
//! directory with presence, voicemail and calling settings — and the changes
//! to them, as the app and the CLI see them.

use serde::{Deserialize, Serialize};

/// The phone as it is now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    /// The account's address of record.
    pub aor: String,
    pub registration: Registration,
    /// The calls, oldest first.
    pub calls: Vec<CallView>,
    pub message_waiting: Option<MessageWaiting>,
    /// Do not disturb, as FCP has it; `None` without FCP.
    pub dnd: Option<bool>,
    /// The tenant's brand, once fetched.
    pub brand: Option<Brand>,
    /// The user's calling settings, as FCP has them; `None` without FCP.
    pub calling: Option<anvil_fcp::CallingSettings>,
    /// The latest calls, newest first.
    pub recents: Vec<Recent>,
    /// The tenant's directory, with presence where the tenant shows it.
    pub people: Vec<Person>,
    /// The mailbox's messages, newest first.
    pub voicemail: Vec<Voicemail>,
    pub audio: AudioDevices,
}

/// The tenant's brand, as a screen applies it (`docs/BRANDING.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Brand {
    pub app_name: String,
    /// `#RRGGBB`.
    pub primary: Option<String>,
    pub accent: Option<String>,
    /// The logo's bytes (PNG or SVG as the tenant uploaded it); not sent on
    /// the control socket.
    #[serde(skip)]
    pub logo: Option<Vec<u8>>,
}

impl State {
    pub(crate) fn new(aor: String) -> Self {
        Self {
            aor,
            registration: Registration::Unregistered,
            calls: Vec::new(),
            message_waiting: None,
            dnd: None,
            brand: None,
            calling: None,
            recents: Vec::new(),
            people: Vec::new(),
            voicemail: Vec::new(),
            audio: AudioDevices::default(),
        }
    }

    /// Where call `id` is, while it lasts.
    pub fn call_state(&self, id: u64) -> Option<CallState> {
        self.call(id).map(|c| c.state)
    }

    pub(crate) fn call(&self, id: u64) -> Option<&CallView> {
        self.calls.iter().find(|c| c.id == id)
    }

    pub(crate) fn call_mut(&mut self, id: u64) -> Option<&mut CallView> {
        self.calls.iter_mut().find(|c| c.id == id)
    }
}

/// Whether the account is registered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Registration {
    Unregistered,
    Registering,
    Registered,
    Failed,
}

impl From<anvil_core::RegState> for Registration {
    fn from(s: anvil_core::RegState) -> Self {
        match s {
            anvil_core::RegState::Unregistered => Self::Unregistered,
            anvil_core::RegState::Registering => Self::Registering,
            anvil_core::RegState::Registered => Self::Registered,
            anvil_core::RegState::Failed => Self::Failed,
        }
    }
}

/// One call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallView {
    pub id: u64,
    pub direction: Direction,
    /// Who the call is with: the caller's address, or what was dialled.
    pub remote: String,
    pub display_name: Option<String>,
    pub state: CallState,
    pub held: bool,
    pub muted: bool,
    /// The codec in use once connected (`opus`, `pcmu`, …).
    pub codec: Option<String>,
    /// The media is encrypted (SRTP).
    #[serde(default)]
    pub encrypted: bool,
    /// How the media is doing, once measured.
    #[serde(default)]
    pub quality: Option<Quality>,
}

impl CallView {
    /// A call just seen: not held, not muted, nothing measured yet.
    pub fn new(id: u64, direction: Direction, remote: &str, state: CallState) -> Self {
        Self {
            id,
            direction,
            remote: remote.to_string(),
            display_name: None,
            state,
            held: false,
            muted: false,
            codec: None,
            encrypted: false,
            quality: None,
        }
    }
}

/// A connected call's media, as last measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quality {
    pub jitter_ms: u32,
    /// Packets lost, per thousand.
    pub packet_loss_permille: u32,
    pub rtt_ms: Option<u32>,
}

/// One call in the user's history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recent {
    /// FCP's record id.
    pub id: String,
    pub direction: Direction,
    /// Who the call was with: what to dial back.
    pub remote: String,
    pub display_name: Option<String>,
    /// An incoming call nobody answered.
    pub missed: bool,
    /// RFC 3339.
    pub started_at: String,
    pub duration_secs: Option<u64>,
}

/// Someone in the tenant's directory: a user, or an extension that is not
/// a user's (a queue, a ring group).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Person {
    /// The user's login name, or the extension when it is not a user's.
    pub key: String,
    pub name: String,
    pub extension: Option<String>,
    pub department: Option<String>,
    pub job_title: Option<String>,
    /// `available`, `busy`, `away`, `dnd`, `offline`, … when shown.
    pub presence: Option<String>,
    /// On a call now (their busy lamp).
    pub on_call: bool,
    /// One of this user's favourites.
    pub favourite: bool,
}

impl Person {
    /// What to dial: the extension, else the user's name.
    pub fn dial(&self) -> &str {
        self.extension.as_deref().unwrap_or(&self.key)
    }
}

/// One voicemail message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Voicemail {
    pub id: String,
    pub caller: String,
    pub caller_name: Option<String>,
    /// Not yet heard.
    pub new: bool,
    pub urgent: bool,
    pub duration_secs: u64,
    pub transcription: Option<String>,
    /// RFC 3339.
    pub received_at: String,
}

/// The microphones and speakers, and which are in use.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioDevices {
    pub inputs: Vec<AudioDevice>,
    pub outputs: Vec<AudioDevice>,
    /// The chosen microphone's id; `None` is the system's default.
    pub input: Option<String>,
    pub output: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    /// The system's default.
    pub default: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Incoming,
    Outgoing,
}

/// Where a call is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallState {
    /// Outgoing, sent, nothing heard yet.
    Dialing,
    /// Ringing: an incoming call to answer, or an outgoing one ringing there.
    Ringing,
    Connected,
}

/// The mailbox's counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageWaiting {
    pub waiting: bool,
    pub new: u32,
    pub old: u32,
}

/// A change to the phone, one per event: what the app redraws, what
/// `anvil watch` prints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Change {
    Registration {
        state: Registration,
        reason: Option<String>,
    },
    /// A call appeared or changed.
    Call {
        call: CallView,
    },
    CallEnded {
        id: u64,
        reason: String,
    },
    MessageWaiting {
        message_waiting: MessageWaiting,
    },
    Dnd {
        on: bool,
    },
    /// The brand arrived or changed; `None` when the tenant has none.
    Brand {
        app_name: Option<String>,
    },
    /// A transfer of `id` progressed: the server's status for it.
    TransferProgress {
        id: u64,
        code: u16,
        reason: String,
    },
    /// The calling settings changed (here or elsewhere).
    Calling {
        settings: anvil_fcp::CallingSettings,
    },
    /// The recent calls were read again; `missed` of them unanswered.
    Recents {
        missed: u32,
    },
    /// The directory was read again.
    People {
        count: u32,
    },
    /// One person's presence, busy lamp or favourite changed.
    Person {
        person: Person,
    },
    /// The mailbox's messages were read again.
    Voicemail {
        new: u32,
        total: u32,
    },
    /// The devices, or the choice of them, changed.
    Audio {
        audio: AudioDevices,
    },
}
