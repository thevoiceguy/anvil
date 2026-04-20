//! Audio host traits.
//!
//! `anvil-core` does not open the microphone itself. A host implementation is
//! injected at startup. Desktop apps plug in `anvil-audio::CpalHost`; mobile
//! hosts supply their own (AVAudioEngine on iOS, Oboe on Android).

use async_trait::async_trait;

use crate::error::AnvilError;

/// PCM sample format accepted by the media engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioFormat {
    pub sample_rate: u32,
    pub channels: u16,
    /// Frame length in milliseconds (20 ms is the only supported value today).
    pub frame_ms: u32,
}

/// A 20 ms PCM frame, interleaved if stereo.
#[derive(Debug, Clone)]
pub struct AudioFrame {
    pub samples: Vec<i16>,
    pub format: AudioFormat,
}

/// Metadata about an input/output device.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub is_input: bool,
    pub is_default: bool,
}

/// A factory for capture/playback streams, supplied to `Anvil::start`.
pub trait AudioHost: Send + Sync {
    /// Open a microphone stream.
    fn make_capture (&self, cfg: AudioFormat) -> Result<Box<dyn AudioSource>, AnvilError>;
    /// Open a speaker stream.
    fn make_playback(&self, cfg: AudioFormat) -> Result<Box<dyn AudioSink>, AnvilError>;
    /// Enumerate devices. Returns an empty vec on hosts that don't expose enumeration.
    fn devices(&self) -> Vec<DeviceInfo>;
}

/// Microphone-side stream. Produces one frame per call.
#[async_trait]
pub trait AudioSource: Send {
    async fn next_frame(&mut self) -> Option<AudioFrame>;
}

/// Speaker-side stream. Consumes one frame per call.
#[async_trait]
pub trait AudioSink: Send {
    async fn write_frame(&mut self, frame: &AudioFrame) -> Result<(), AnvilError>;
}

/// In-place audio post-processing. Applied to capture frames before they
/// reach the encoder, so AGC / NS / EQ effects ride on the wire.
///
/// Each call takes one 20 ms PCM frame and may rewrite its samples in
/// place. Implementations must keep the frame length and sample rate
/// unchanged.
///
/// AEC traditionally needs both a capture and a render stream — the
/// capture frame here is one half. Real echo cancellation requires
/// either a platform-native pipeline (AVAudioEngine voice-processing
/// IO on iOS, Oboe + AcousticEchoCanceler on Android) or wiring
/// webrtc-audio-processing's render-stream API to the playback path.
/// `AudioProcessor` covers the simpler half (NS / AGC) cleanly.
pub trait AudioProcessor: Send {
    /// Process one frame in place. Default is a no-op pass-through.
    fn process_capture(&mut self, _frame: &mut AudioFrame) {}
}

/// Default pass-through processor.
pub struct NullProcessor;

impl AudioProcessor for NullProcessor {}
