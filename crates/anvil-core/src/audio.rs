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
    /// Open a speaker stream for ringing: the device chosen for it, else
    /// the system's default for sounds (on Windows the speakers, not the
    /// headset calls use). A host with no such choice rings where calls
    /// play.
    fn make_ringer(&self, cfg: AudioFormat) -> Result<Box<dyn AudioSink>, AnvilError> {
        self.make_playback(cfg)
    }
    /// Ring on this output device (by id; `None` is the system's default).
    /// A host with no choice of device refuses.
    fn choose_ring_device(&self, output: Option<&str>) -> Result<(), AnvilError> {
        let _ = output;
        Err(AnvilError::AudioDevice(
            "this audio host has no choice of device".into(),
        ))
    }
    /// The device chosen for ringing, `None` for the system's default.
    fn chosen_ring_device(&self) -> Option<String> {
        None
    }
    /// Clean the microphone with `processing`, from now on and in the calls
    /// already up. A host that cannot (or whose platform does it, as iOS's
    /// voice processing does) refuses.
    fn set_processing(&self, processing: Processing) -> Result<(), AnvilError> {
        let _ = processing;
        Err(AnvilError::AudioDevice(
            "this audio host does no processing of its own".into(),
        ))
    }
    /// What the host does to the microphone; `None` when it does nothing of
    /// its own.
    fn processing(&self) -> Option<Processing> {
        None
    }
}

/// What a host does to the microphone before it is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Processing {
    /// Take out what the speaker plays that the microphone hears.
    pub echo_cancellation: bool,
    /// Take out steady noise: fans, hum, the room.
    pub noise_suppression: bool,
}

impl Default for Processing {
    fn default() -> Self {
        Self {
            echo_cancellation: true,
            noise_suppression: true,
        }
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
/// Echo cancellation needs the speaker's audio too, so it is the host's
/// (`anvil-audio`'s `echo.rs`, set through [`AudioHost::set_processing`]).
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
    let format = clip_format(sample_rate)?;
    let sink = host.make_playback(format)?;
    Ok(tokio::spawn(play_out(sink, format, samples, None)))
}

/// Ring: the clip over and over, `gap` of silence after each, on the
/// ringing speaker or (`in_call`) the one calls use, as a tone heard in a
/// call, until the task is aborted.
pub(crate) fn ring(
    host: &dyn AudioHost,
    samples: Vec<i16>,
    sample_rate: u32,
    gap: std::time::Duration,
    in_call: bool,
) -> Result<tokio::task::JoinHandle<()>, AnvilError> {
    let format = clip_format(sample_rate)?;
    let sink = if in_call {
        host.make_playback(format)?
    } else {
        host.make_ringer(format)?
    };
    Ok(tokio::spawn(play_out(sink, format, samples, Some(gap))))
}

fn clip_format(sample_rate: u32) -> Result<AudioFormat, AnvilError> {
    if !(8_000..=192_000).contains(&sample_rate) {
        return Err(AnvilError::AudioDevice(format!(
            "a clip at {sample_rate} Hz cannot be played"
        )));
    }
    Ok(AudioFormat {
        sample_rate,
        channels: 1,
        frame_ms: 20,
    })
}

/// Write `samples` to `sink` in real time: once with 300 ms of silence
/// after, or (`repeat`) with that gap after each pass, for ever.
async fn play_out(
    mut sink: Box<dyn AudioSink>,
    format: AudioFormat,
    samples: Vec<i16>,
    repeat: Option<std::time::Duration>,
) {
    let per_frame = (format.sample_rate / 50) as usize;
    let gap = repeat.unwrap_or(std::time::Duration::from_millis(300));
    let tail = vec![0i16; per_frame * (gap.as_millis() as usize / 20)];
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(20));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
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
        if repeat.is_none() || samples.is_empty() {
            return;
        }
    }
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

    #[tokio::test(start_paused = true)]
    async fn a_ring_repeats_with_its_gap_until_stopped() {
        let kept = Arc::new(Mutex::new(Vec::new()));
        let host = Keeping(Arc::clone(&kept));
        let clip = vec![7i16; 320];
        let task = ring(
            &host,
            clip,
            8000,
            std::time::Duration::from_millis(100),
            false,
        )
        .unwrap();
        // Each pass: two frames of the clip, five of silence; the first frame
        // at once, one every 20 ms after.
        tokio::time::sleep(std::time::Duration::from_millis(20 * 7 * 3 - 10)).await;
        task.abort();
        let frames = kept.lock().unwrap();
        assert_eq!(frames.len(), 21);
        let loud: Vec<bool> = frames.iter().map(|f| f.samples[0] == 7).collect();
        for pass in loud.chunks(7) {
            assert_eq!(pass, [true, true, false, false, false, false, false]);
        }
    }
}
