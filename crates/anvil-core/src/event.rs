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

/// Whether the registrar can wake this install by push (RFC 8599).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushState {
    /// No push address is registered.
    Off,
    /// The registrar took the push address: a call wakes the app.
    Accepted,
    /// The registrar answered 555: it cannot push to this app. It pushes
    /// through `supported` (its `Feature-Caps`), perhaps none. The
    /// registration goes on without the address.
    Refused { supported: Vec<String> },
}

/// Reason a call ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndReason {
    LocalHangup,
    RemoteHangup,
    Rejected(u16),
    Timeout,
    MediaFailure(String),
    Error(String),
    /// The park code's call, ended by FCP once it parked the other call
    /// on this orbit.
    Parked(String),
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
    /// What arrived on the call's RTP socket so far, and what became of it.
    pub receive: ReceiveCounts,
}

/// A call's receive side counted from its start: every datagram on the RTP
/// socket, what was played, and what was dropped and why. When a call is
/// heard one way only, these say whether the far end's packets reach this
/// machine at all and, if they do, why they are not played.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReceiveCounts {
    /// Datagrams read from the RTP socket, of any kind.
    pub received: u64,
    /// Audio frames handed to the speaker.
    pub played: u64,
    /// Packets dropped because their payload type is not the call's codec
    /// (nor telephone-event).
    pub dropped_payload_type: u64,
    /// The payload type of the last packet dropped for it.
    pub last_unexpected_pt: Option<u8>,
    /// The call's codec's payload type: what a packet must carry to be played.
    pub expected_pt: u8,
    /// Packets that did not authenticate under the far end's SRTP key.
    pub dropped_srtp: u64,
    /// Datagrams that are not RTP.
    pub dropped_malformed: u64,
    /// Receives that reported an earlier datagram of ours refused (an ICMP
    /// port unreachable, seen as a connection reset on Windows).
    pub refused: u64,
    /// Where the last datagram came from.
    pub last_source: Option<std::net::SocketAddr>,
}

impl ReceiveCounts {
    /// Every packet dropped, whatever the reason.
    pub fn dropped(&self) -> u64 {
        self.dropped_payload_type + self.dropped_srtp + self.dropped_malformed
    }
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
        /// The server's id for the call (FCP's `X-FCP-Call-Id`): what a
        /// push that woke the app called it.
        server_call: Option<String>,
        /// Which of the account's lines the call is for (FCP's
        /// `X-FCP-Line`, a line's key: `own`, `extension:1005`, …).
        line: Option<String>,
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
    /// Another line's registration changed ([`crate::Anvil::add_line`]).
    LineRegistration {
        aor: String,
        state: RegState,
        reason: Option<String>,
    },
    /// Another line's message summary (its mailbox, as the server says).
    LineMessageWaiting {
        aor: String,
        summary: crate::mwi::MessageSummary,
    },
    /// A shared line's appearances in use, as the server last said
    /// ([`crate::sca`]); empty when the line is idle.
    LineAppearances {
        aor: String,
        appearances: Vec<crate::sca::Appearance>,
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
    /// What the registrar made of the push address (RFC 8599): sent when
    /// it changes.
    PushChanged {
        state: PushState,
    },
    Error {
        call: Option<CallId>,
        error: AnvilError,
    },
}
