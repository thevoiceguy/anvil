//! Android's streams, opened with Oboe as what they are: a call's
//! microphone and speaker as a voice call, a ringer as a ringtone.
//!
//! cpal opens every Android stream as media. A softphone that tells
//! Android about its calls (Telecom) puts the phone in call mode, and in
//! call mode some phones play media nowhere: the far end and the ringback
//! were silent while every stream said it was playing. A call's audio as
//! a voice call follows the call's route (earpiece, speaker, headset) and
//! its volume, and the microphone as a voice call gets Android's own echo
//! cancelling.
//!
//! The streams are mono 16-bit at [`RATE`], Oboe converting from the
//! device's when they differ.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use oboe::{
    AudioInputCallback, AudioInputStreamSafe, AudioOutputCallback, AudioOutputStreamSafe,
    AudioStream, AudioStreamBase, AudioStreamBuilder, ContentType, DataCallbackResult, InputPreset,
    Mono, SampleRateConversionQuality, Usage,
};

/// The streams' rate.
pub(crate) const RATE: u32 = 48_000;

/// What an output stream plays.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Kind {
    /// A call's audio.
    Call,
    /// A ring.
    Ring,
}

/// A stream open, until dropped.
pub(crate) type Held = Box<dyn std::any::Any>;

struct Input<F> {
    data: F,
    failed: Arc<AtomicBool>,
}

impl<F: FnMut(&[i16]) + Send + 'static> AudioInputCallback for Input<F> {
    type FrameType = (i16, Mono);

    fn on_error_after_close(&mut self, _: &mut dyn AudioInputStreamSafe, error: oboe::Error) {
        tracing::warn!(?error, "capture stream error");
        self.failed.store(true, Ordering::Relaxed);
    }

    fn on_audio_ready(
        &mut self,
        _: &mut dyn AudioInputStreamSafe,
        data: &[i16],
    ) -> DataCallbackResult {
        (self.data)(data);
        DataCallbackResult::Continue
    }
}

struct Output<F> {
    data: F,
    failed: Arc<AtomicBool>,
}

impl<F: FnMut(&mut [i16]) + Send + 'static> AudioOutputCallback for Output<F> {
    type FrameType = (i16, Mono);

    fn on_error_after_close(&mut self, _: &mut dyn AudioOutputStreamSafe, error: oboe::Error) {
        tracing::warn!(?error, "playback stream error");
        self.failed.store(true, Ordering::Relaxed);
    }

    fn on_audio_ready(
        &mut self,
        _: &mut dyn AudioOutputStreamSafe,
        data: &mut [i16],
    ) -> DataCallbackResult {
        (self.data)(data);
        DataCallbackResult::Continue
    }
}

/// A call's microphone, handing `data` its samples; `failed` is set when
/// it stops working.
pub(crate) fn input(
    data: impl FnMut(&[i16]) + Send + 'static,
    failed: &Arc<AtomicBool>,
) -> Result<Held, String> {
    let mut stream = AudioStreamBuilder::default()
        .set_input()
        .set_i16()
        .set_mono()
        .set_sample_rate(RATE as i32)
        .set_sample_rate_conversion_quality(SampleRateConversionQuality::Medium)
        .set_input_preset(InputPreset::VoiceCommunication)
        .set_callback(Input {
            data,
            failed: Arc::clone(failed),
        })
        .open_stream()
        .map_err(|e| format!("open the microphone: {e:?}"))?;
    check_rate(stream.get_sample_rate())?;
    stream
        .start()
        .map_err(|e| format!("start the microphone: {e:?}"))?;
    Ok(Box::new(stream))
}

/// An output of `kind`, filled by `data`; `failed` is set when it stops
/// working.
pub(crate) fn output(
    kind: Kind,
    data: impl FnMut(&mut [i16]) + Send + 'static,
    failed: &Arc<AtomicBool>,
) -> Result<Held, String> {
    let (usage, content) = match kind {
        Kind::Call => (Usage::VoiceCommunication, ContentType::Speech),
        Kind::Ring => (Usage::NotificationRingtone, ContentType::Sonification),
    };
    let mut stream = AudioStreamBuilder::default()
        .set_output()
        .set_i16()
        .set_mono()
        .set_sample_rate(RATE as i32)
        .set_sample_rate_conversion_quality(SampleRateConversionQuality::Medium)
        .set_usage(usage)
        .set_content_type(content)
        .set_callback(Output {
            data,
            failed: Arc::clone(failed),
        })
        .open_stream()
        .map_err(|e| format!("open the speaker: {e:?}"))?;
    check_rate(stream.get_sample_rate())?;
    stream
        .start()
        .map_err(|e| format!("start the speaker: {e:?}"))?;
    Ok(Box::new(stream))
}

/// A stream not at [`RATE`] would play at the wrong speed.
fn check_rate(rate: i32) -> Result<(), String> {
    if rate == RATE as i32 {
        Ok(())
    } else {
        Err(format!("the stream opened at {rate} Hz, not {RATE}"))
    }
}
