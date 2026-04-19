//! cpal-backed implementation of `AudioHost`. Phase 1 stub.

use anvil_core::audio::{AudioFormat, AudioHost, AudioSink, AudioSource, DeviceInfo};
use anvil_core::error::AnvilError;

/// Desktop audio host using cpal.
pub struct CpalHost {
    _private: (),
}

impl CpalHost {
    /// Create a host using the system default input and output devices.
    pub fn new() -> Result<Self, AnvilError> {
        Ok(Self { _private: () })
    }
}

impl AudioHost for CpalHost {
    fn make_capture(&self, _cfg: AudioFormat) -> Result<Box<dyn AudioSource>, AnvilError> {
        Err(AnvilError::AudioDevice("CpalHost::make_capture not yet implemented".into()))
    }

    fn make_playback(&self, _cfg: AudioFormat) -> Result<Box<dyn AudioSink>, AnvilError> {
        Err(AnvilError::AudioDevice("CpalHost::make_playback not yet implemented".into()))
    }

    fn devices(&self) -> Vec<DeviceInfo> { Vec::new() }
}
