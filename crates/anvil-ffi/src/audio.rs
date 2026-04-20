//! Audio host stub for the M1 FFI.
//!
//! P3-M1 doesn't expose audio over the C ABI; mobile hosts will land
//! their own `AudioHost` implementations once P3-M5 wires the function-
//! pointer bridge. For now, we ship a no-op host so signaling-only flows
//! (REGISTER, OPTIONS, etc.) work end-to-end without any audio I/O.

use anvil_core::audio::{AudioFormat, AudioHost, AudioSink, AudioSource, DeviceInfo};
use anvil_core::AnvilError;

pub(crate) struct NullAudioHost;

impl AudioHost for NullAudioHost {
    fn make_capture(&self, _cfg: AudioFormat) -> Result<Box<dyn AudioSource>, AnvilError> {
        Err(AnvilError::AudioDevice(
            "anvil-ffi: no audio host attached (P3-M5)".into(),
        ))
    }
    fn make_playback(&self, _cfg: AudioFormat) -> Result<Box<dyn AudioSink>, AnvilError> {
        Err(AnvilError::AudioDevice(
            "anvil-ffi: no audio host attached (P3-M5)".into(),
        ))
    }
    fn devices(&self) -> Vec<DeviceInfo> {
        Vec::new()
    }
}
