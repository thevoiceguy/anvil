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
    Error {
        call: Option<CallId>,
        error: AnvilError,
    },
}
