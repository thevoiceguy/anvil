//! # anvil-core
//!
//! Softphone core. Orchestrates SIP signaling (`siphon-rs`) and the media engine
//! (`forge-media`). See `docs/ARCHITECTURE.md` in the workspace root for the design.
//!
//! This crate is pre-alpha. The public surface below is the target shape;
//! implementations are stubs until Phase 1 of the roadmap lands.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod audio;
pub mod brand;
pub mod config;
pub mod error;
pub mod event;

pub use brand::{
    BrandAsset, BrandCache, BrandColors, BrandCredential, BrandFetchOutcome,
    BrandLinks, BrandProfile, BrandProvider, BrandRequest, DialPlan, Ringtone, Rgba,
};
pub use config::{AccountConfig, AnvilConfig, BrandConfig, MediaConfig, Transport};
pub use error::AnvilError;
pub use event::{EndReason, Event, EventStream, MediaStats, RegState};

/// Opaque identifier for an active call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CallId(pub u64);

/// Softphone runtime handle.
pub struct Anvil {
    _private: (),
}

impl Anvil {
    /// Start the softphone. Returns the runtime handle and an event stream.
    pub async fn start(_cfg: AnvilConfig) -> Result<(Self, EventStream), AnvilError> {
        unimplemented!("Phase 1")
    }

    /// Send REGISTER to the configured registrar.
    pub async fn register(&self) -> Result<(), AnvilError> { unimplemented!() }

    /// Unregister (REGISTER with Expires: 0).
    pub async fn unregister(&self) -> Result<(), AnvilError> { unimplemented!() }

    /// Place an outgoing call.
    pub async fn place_call(&self, _target: &str) -> Result<CallId, AnvilError> {
        unimplemented!()
    }

    /// Answer an incoming call that the app was notified of via `Event::IncomingCall`.
    pub async fn answer(&self, _call: CallId) -> Result<(), AnvilError> { unimplemented!() }

    /// Reject an incoming call with the given SIP response code (e.g. 486 Busy Here).
    pub async fn reject(&self, _call: CallId, _code: u16) -> Result<(), AnvilError> {
        unimplemented!()
    }

    /// End an active call.
    pub async fn hangup(&self, _call: CallId) -> Result<(), AnvilError> { unimplemented!() }

    /// Send a DTMF digit on an active call.
    pub async fn send_dtmf(&self, _call: CallId, _digit: char) -> Result<(), AnvilError> {
        unimplemented!()
    }

    /// Put a call on hold (re-INVITE with `a=sendonly`) or resume it.
    pub async fn hold(&self, _call: CallId, _hold: bool) -> Result<(), AnvilError> {
        unimplemented!()
    }

    /// Blind-transfer a call via REFER.
    pub async fn transfer(&self, _call: CallId, _target: &str) -> Result<(), AnvilError> {
        unimplemented!()
    }

    /// Gracefully stop the runtime.
    pub async fn shutdown(self) -> Result<(), AnvilError> { unimplemented!() }
}
