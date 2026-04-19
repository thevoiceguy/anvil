//! # anvil-audio
//!
//! Desktop audio host for Anvil. Wraps [`cpal`] to provide microphone capture and
//! speaker playback as `anvil_core::audio::AudioSource` / `AudioSink`. Optionally
//! applies acoustic echo cancellation, noise suppression, and AGC via WebRTC's
//! audio processing module when the `apm` feature is enabled.
//!
//! Mobile hosts (iOS AVAudioEngine, Android Oboe) implement `AudioHost` themselves
//! and do not use this crate.

#![forbid(unsafe_code)]

#[cfg(feature = "cpal-default")]
mod cpal_host;

#[cfg(feature = "cpal-default")]
pub use cpal_host::CpalHost;

#[cfg(feature = "tone")]
mod tone;

#[cfg(feature = "tone")]
pub use tone::{ToneHost, ToneStats};
