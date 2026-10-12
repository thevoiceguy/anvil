//! # anvil-audio
//!
//! Desktop audio host for Anvil. Wraps [`cpal`] to provide microphone capture and
//! speaker playback as `anvil_core::audio::AudioSource` / `AudioSink`. With the
//! `apm` feature (on by default) it cancels echo and suppresses noise with
//! WebRTC's audio processing, ported to Rust by `sonora` (`echo.rs`).
//!
//! Mobile hosts (iOS AVAudioEngine, Android Oboe) implement `AudioHost` themselves
//! and do not use this crate.

// Unsafe only where Windows' API needs it, in one module.
#![deny(unsafe_code)]

#[cfg(feature = "cpal-default")]
mod cpal_host;

#[cfg(feature = "cpal-default")]
pub use cpal_host::CpalHost;

#[cfg(all(windows, feature = "cpal-default"))]
#[allow(unsafe_code)]
mod comms;

#[cfg(feature = "tone")]
mod tone;

#[cfg(feature = "tone")]
pub use tone::{ToneHost, ToneStats};

#[cfg(feature = "apm")]
#[cfg_attr(not(feature = "cpal-default"), allow(dead_code))]
mod echo;

mod processor;
pub use processor::SimpleAgc;
