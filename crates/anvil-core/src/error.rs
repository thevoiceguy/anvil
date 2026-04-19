//! Error type for `anvil-core`.

use crate::CallId;

/// Errors returned by the Anvil API. Upstream siphon-rs and forge-media errors are
/// wrapped rather than re-exported so callers do not depend on those crates.
#[derive(thiserror::Error, Debug, Clone)]
pub enum AnvilError {
    #[error("not registered")]
    NotRegistered,

    #[error("no such call: {0:?}")]
    NoSuchCall(CallId),

    #[error("transport error: {0}")]
    Transport(String),

    #[error("authentication rejected: {0}")]
    AuthRejected(String),

    #[error("media error: {0}")]
    Media(String),

    #[error("audio device error: {0}")]
    AudioDevice(String),

    #[error("codec error: {0}")]
    Codec(String),

    #[error("SDP negotiation failed: {0}")]
    SdpNegotiation(String),

    #[error("invalid configuration: {0}")]
    Config(String),

    #[error("internal error: {0}")]
    Internal(String),
}
