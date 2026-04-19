//! Headless audio host for automated end-to-end tests.
//!
//! `ToneHost` generates a pure sine tone on the capture side and records
//! the RMS of whatever arrives on the playback side into a shared counter.
//! It's useful for CI, for dev boxes without audio hardware, and for
//! proving the RTP pipeline end-to-end without eyes or ears.
//!
//! Not meant to be a substitute for `CpalHost` in any real usage.

use std::f32::consts::TAU;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::time::{sleep, Duration, Instant};

use anvil_core::audio::{AudioFormat, AudioFrame, AudioHost, AudioSink, AudioSource, DeviceInfo};
use anvil_core::error::AnvilError;

/// Rolling statistics for the playback side, readable while the call is live.
#[derive(Debug, Default)]
pub struct ToneStats {
    /// Number of frames received on the sink.
    pub frames_rx: AtomicU64,
    /// Most recent RMS (scaled ×1 000 so we can store it in an integer).
    pub last_rms_q: AtomicU64,
    /// Running peak RMS (same scale).
    pub peak_rms_q: AtomicU64,
}

impl ToneStats {
    pub fn last_rms(&self) -> f32 {
        (self.last_rms_q.load(Ordering::Relaxed) as f32) / 1000.0
    }
    pub fn peak_rms(&self) -> f32 {
        (self.peak_rms_q.load(Ordering::Relaxed) as f32) / 1000.0
    }
    pub fn frames(&self) -> u64 {
        self.frames_rx.load(Ordering::Relaxed)
    }
}

/// Tone-generating audio host.
pub struct ToneHost {
    /// Sine frequency in Hz. 440 Hz ("A4") is loud, unambiguous, and
    /// comfortably under the 4 kHz Nyquist ceiling for 8 kHz audio.
    pub frequency_hz: f32,
    /// Amplitude as a fraction of `i16::MAX`. 0.3 is plenty loud without
    /// clipping after any downstream gain.
    pub amplitude: f32,
    pub stats: Arc<ToneStats>,
}

impl Default for ToneHost {
    fn default() -> Self {
        Self {
            frequency_hz: 440.0,
            amplitude: 0.3,
            stats: Arc::new(ToneStats::default()),
        }
    }
}

impl ToneHost {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn stats(&self) -> Arc<ToneStats> {
        Arc::clone(&self.stats)
    }
}

impl AudioHost for ToneHost {
    fn make_capture(&self, fmt: AudioFormat) -> Result<Box<dyn AudioSource>, AnvilError> {
        Ok(Box::new(ToneCapture::new(fmt, self.frequency_hz, self.amplitude)))
    }

    fn make_playback(&self, fmt: AudioFormat) -> Result<Box<dyn AudioSink>, AnvilError> {
        Ok(Box::new(ToneSink {
            fmt,
            stats: Arc::clone(&self.stats),
        }))
    }

    fn devices(&self) -> Vec<DeviceInfo> {
        vec![
            DeviceInfo {
                id: "tone:in".into(),
                name: "Synthetic tone generator".into(),
                is_input: true,
                is_default: true,
            },
            DeviceInfo {
                id: "tone:out".into(),
                name: "RMS recorder".into(),
                is_input: false,
                is_default: true,
            },
        ]
    }
}

struct ToneCapture {
    fmt: AudioFormat,
    frame_samples: usize,
    frame_period: Duration,
    phase: f32,
    phase_step: f32,
    amplitude: f32,
    next_tick: Option<Instant>,
}

impl ToneCapture {
    fn new(fmt: AudioFormat, frequency_hz: f32, amplitude: f32) -> Self {
        let frame_samples = (fmt.sample_rate as u64 * fmt.frame_ms as u64 / 1000) as usize;
        let frame_period = Duration::from_millis(fmt.frame_ms as u64);
        let phase_step = TAU * frequency_hz / fmt.sample_rate as f32;
        Self {
            fmt,
            frame_samples,
            frame_period,
            phase: 0.0,
            phase_step,
            amplitude,
            next_tick: None,
        }
    }
}

#[async_trait]
impl AudioSource for ToneCapture {
    async fn next_frame(&mut self) -> Option<AudioFrame> {
        // Wall-clock pacing: deliver one 20 ms frame every 20 ms. Matches
        // what a real mic would produce, so downstream code (especially
        // the RTP sender) stays honest about timing.
        let target = self.next_tick.unwrap_or_else(Instant::now);
        let now = Instant::now();
        if target > now {
            sleep(target - now).await;
        }
        self.next_tick = Some(target + self.frame_period);

        let mut samples = Vec::with_capacity(self.frame_samples);
        let scale = self.amplitude * (i16::MAX as f32);
        for _ in 0..self.frame_samples {
            let s = (self.phase.sin() * scale) as i16;
            samples.push(s);
            self.phase += self.phase_step;
            if self.phase > TAU {
                self.phase -= TAU;
            }
        }

        Some(AudioFrame {
            samples,
            format: self.fmt,
        })
    }
}

struct ToneSink {
    fmt: AudioFormat,
    stats: Arc<ToneStats>,
}

#[async_trait]
impl AudioSink for ToneSink {
    async fn write_frame(&mut self, frame: &AudioFrame) -> Result<(), AnvilError> {
        let _ = self.fmt;
        let n = frame.samples.len().max(1) as f64;
        let sum_sq: f64 = frame
            .samples
            .iter()
            .map(|&s| (s as f64) * (s as f64))
            .sum();
        let rms = (sum_sq / n).sqrt() as f32;
        let q = (rms * 1000.0 / i16::MAX as f32 * 1000.0) as u64;

        self.stats.frames_rx.fetch_add(1, Ordering::Relaxed);
        self.stats.last_rms_q.store(q, Ordering::Relaxed);
        self.stats
            .peak_rms_q
            .fetch_max(q, Ordering::Relaxed);
        Ok(())
    }
}
