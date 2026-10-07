//! What the phone shows: its registration, its calls, message waiting and
//! do not disturb — and the changes to them, as the app and the CLI see them.

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
}

impl State {
    pub(crate) fn new(aor: String) -> Self {
        Self {
            aor,
            registration: Registration::Unregistered,
            calls: Vec::new(),
            message_waiting: None,
            dnd: None,
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
    /// A transfer of `id` progressed: the server's status for it.
    TransferProgress {
        id: u64,
        code: u16,
        reason: String,
    },
}
