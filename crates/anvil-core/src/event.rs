//! Events emitted by `Anvil` to the application layer.

use tokio::sync::mpsc;

use crate::{brand::BrandProfile, config::Codec, error::AnvilError, CallId};

/// Event stream receiver handed to the application from `Anvil::start`.
pub type EventStream = mpsc::Receiver<Event>;

/// Registration state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegState {
    Unregistered,
    Registering,
    Registered,
    Failed,
}

/// Reason a call ended.
#[derive(Debug, Clone)]
pub enum EndReason {
    LocalHangup,
    RemoteHangup,
    Rejected(u16),
    Timeout,
    MediaFailure(String),
    Error(String),
}

/// Periodic media statistics for an active call.
#[derive(Debug, Clone, Copy)]
pub struct MediaStats {
    pub codec: Codec,
    pub jitter_ms: f32,
    pub packet_loss_pct: f32,
    pub rtt_ms: Option<f32>,
    pub recv_kbps: f32,
    pub send_kbps: f32,
}

/// Events delivered to the application.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Event {
    RegistrationChanged {
        state: RegState,
        reason: Option<String>,
    },
    IncomingCall {
        call: CallId,
        from: String,
        display_name: Option<String>,
    },
    CallRinging {
        call: CallId,
    },
    CallEstablished {
        call: CallId,
        codec: Codec,
    },
    CallEnded {
        call: CallId,
        reason: EndReason,
    },
    DtmfReceived {
        call: CallId,
        digit: char,
    },
    MediaStats {
        call: CallId,
        stats: MediaStats,
    },
    /// Tenant brand profile was (re)fetched from FCP. Delivered once after
    /// login and again if the server publishes an update (new ETag on
    /// re-fetch, or push via a future NOTIFY channel).
    BrandUpdated {
        profile: Box<BrandProfile>,
    },
    /// The tenant has no brand (any more), or the server refused the
    /// credential: the UI goes back to its own theme.
    BrandCleared,
    /// The mailbox's counts (RFC 3842), from an unsolicited NOTIFY or a
    /// [`subscribe_mwi`](crate::Anvil::subscribe_mwi) subscription.
    MessageWaiting {
        summary: crate::mwi::MessageSummary,
    },
    /// A transfer of `call` progressed (RFC 3515): the server's status for
    /// the transferred party's new call — 1xx while it is placed, a final
    /// when done. On success the server then ends `call`.
    TransferProgress {
        call: CallId,
        code: u16,
        reason: String,
    },
    /// Someone watched with [`WatchKind::Presence`](crate::watch::WatchKind)
    /// changed: `aor` as their server names them.
    PresenceChanged {
        aor: String,
        presence: crate::watch::Presence,
    },
    /// A busy lamp watched with [`WatchKind::Dialog`](crate::watch::WatchKind)
    /// changed.
    LineStateChanged {
        aor: String,
        state: crate::watch::LineState,
    },
    Error {
        call: Option<CallId>,
        error: AnvilError,
    },
}
