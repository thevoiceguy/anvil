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
    fn make_capture(&self, cfg: AudioFormat) -> Result<Box<dyn AudioSource>, AnvilError>;
    /// Open a speaker stream.
    fn make_playback(&self, cfg: AudioFormat) -> Result<Box<dyn AudioSink>, AnvilError>;
    /// Enumerate devices. Returns an empty vec on hosts that don't expose enumeration.
    fn devices(&self) -> Vec<DeviceInfo>;
    /// Use these devices (by [`DeviceInfo::id`]; `None` is the system's
    /// default) for the streams opened from now on. A host with no choice
    /// of device refuses.
    fn choose_devices(&self, input: Option<&str>, output: Option<&str>) -> Result<(), AnvilError> {
        let _ = (input, output);
        Err(AnvilError::AudioDevice(
            "this audio host has no choice of device".into(),
        ))
    }
    /// The devices chosen, `None` for the system's default.
    fn chosen_devices(&self) -> (Option<String>, Option<String>) {
        (None, None)
    }
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

/// Play mono PCM through `host`'s speaker, one 20 ms frame every 20 ms, then
/// a little silence so the speaker's buffer is heard out before it closes.
pub(crate) fn play(
    host: &dyn AudioHost,
    samples: Vec<i16>,
    sample_rate: u32,
) -> Result<tokio::task::JoinHandle<()>, AnvilError> {
    if !(8_000..=192_000).contains(&sample_rate) {
        return Err(AnvilError::AudioDevice(format!(
            "a clip at {sample_rate} Hz cannot be played"
        )));
    }
    let format = AudioFormat {
        sample_rate,
        channels: 1,
        frame_ms: 20,
    };
    let mut sink = host.make_playback(format)?;
    let per_frame = (sample_rate / 50) as usize;
    Ok(tokio::spawn(async move {
        let tail = vec![0i16; per_frame * 15];
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(20));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        for chunk in samples.chunks(per_frame).chain(tail.chunks(per_frame)) {
            tick.tick().await;
            let mut frame = chunk.to_vec();
            frame.resize(per_frame, 0);
            if let Err(e) = sink
                .write_frame(&AudioFrame {
                    samples: frame,
                    format,
                })
                .await
            {
                tracing::warn!(%e, "playback ended early");
                return;
            }
        }
    }))
}

#[cfg(test)]
mod play_tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// A host whose speaker keeps every frame written to it.
    struct Keeping(Arc<Mutex<Vec<AudioFrame>>>);

    struct KeepingSink(Arc<Mutex<Vec<AudioFrame>>>);

    #[async_trait]
    impl AudioSink for KeepingSink {
        async fn write_frame(&mut self, frame: &AudioFrame) -> Result<(), AnvilError> {
            self.0.lock().unwrap().push(frame.clone());
            Ok(())
        }
    }

    impl AudioHost for Keeping {
        fn make_capture(&self, _: AudioFormat) -> Result<Box<dyn AudioSource>, AnvilError> {
            Err(AnvilError::AudioDevice("no microphone".into()))
        }
        fn make_playback(&self, _: AudioFormat) -> Result<Box<dyn AudioSink>, AnvilError> {
            Ok(Box::new(KeepingSink(Arc::clone(&self.0))))
        }
        fn devices(&self) -> Vec<DeviceInfo> {
            Vec::new()
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_clip_is_played_in_whole_frames_then_silence() {
        let kept = Arc::new(Mutex::new(Vec::new()));
        let host = Keeping(Arc::clone(&kept));
        let clip: Vec<i16> = (0..1000).map(|i| i as i16 + 1).collect();
        play(&host, clip, 8000).unwrap().await.unwrap();
        let frames = kept.lock().unwrap();
        // 1000 samples are seven 160-sample frames, the last padded; then
        // 300 ms of silence.
        assert_eq!(frames.len(), 7 + 15);
        assert!(frames.iter().all(|f| f.samples.len() == 160));
        assert_eq!(frames[0].samples[0], 1);
        assert_eq!(frames[6].samples[39], 1000);
        assert_eq!(frames[6].samples[40], 0);
        assert!(frames[7..]
            .iter()
            .all(|f| f.samples.iter().all(|&s| s == 0)));
        assert!(play(&host, vec![0; 10], 100).is_err());
    }
}
