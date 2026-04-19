//! # anvil-codec
//!
//! Opus encoder/decoder for Anvil. G.711 and G.722 live in `forge-codecs` and
//! don't need wrapping. Opus is broken out here so we can version-independently
//! tune our Opus configuration (complexity, VBR, FEC, PLC) without patching
//! forge-media.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Placeholder Opus encoder. Phase 2 target.
#[cfg(feature = "opus")]
pub struct OpusEncoder {
    _private: (),
}

/// Placeholder Opus decoder. Phase 2 target.
#[cfg(feature = "opus")]
pub struct OpusDecoder {
    _private: (),
}
