//! cpal-backed implementation of `AudioHost`.
//!
//! Each call opens its own pair of streams. The `AudioSource` / `AudioSink`
//! handles returned from `make_capture` / `make_playback` each own a
//! dedicated OS thread that holds the `cpal::Stream` (streams are `!Send` on
//! some platforms). Audio samples cross the thread boundary via bounded
//! tokio channels; dropping the handle closes the channel and the worker
//! thread exits, dropping the stream.
//!
//! Phase 1 M3 targets 8 kHz mono 20 ms frames (160 samples). The device's
//! native rate is typically 48 kHz; `forge-resampler` bridges the gap.

use std::collections::VecDeque;
use std::sync::Arc;
use std::thread;

use async_trait::async_trait;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SampleRate, StreamConfig};
use forge_resampler::Resampler;
use parking_lot::Mutex;
use tokio::sync::mpsc;

use anvil_core::audio::{
    AudioFormat, AudioFrame, AudioHost, AudioProcessor, AudioSink, AudioSource, DeviceInfo,
    NullProcessor,
};
use anvil_core::error::AnvilError;

/// Factory that produces one fresh capture-side `AudioProcessor` per call.
/// Held in `CpalHost` and invoked from `make_capture`.
type ProcessorFactory = Box<dyn Fn() -> Box<dyn AudioProcessor> + Send + Sync>;

/// Desktop audio host using cpal.
pub struct CpalHost {
    /// Per-call capture-side processor factory. Defaults to a no-op
    /// passthrough; use `with_capture_processor` to attach AGC etc.
    capture_processor: ProcessorFactory,
    /// The devices chosen by name; `None` is the system's default.
    chosen: Mutex<(Option<String>, Option<String>)>,
}

impl CpalHost {
    /// Create a host using the system default input and output devices,
    /// with a no-op capture processor.
    pub fn new() -> Result<Self, AnvilError> {
        Ok(Self {
            capture_processor: Box::new(|| Box::new(NullProcessor)),
            chosen: Mutex::new((None, None)),
        })
    }

    /// Replace the per-call capture-side processor factory. Each call
    /// gets a fresh processor (AGC etc. don't share state across calls).
    pub fn with_capture_processor<F>(mut self, factory: F) -> Self
    where
        F: Fn() -> Box<dyn AudioProcessor> + Send + Sync + 'static,
    {
        self.capture_processor = Box::new(factory);
        self
    }
}

impl AudioHost for CpalHost {
    fn make_capture(&self, cfg: AudioFormat) -> Result<Box<dyn AudioSource>, AnvilError> {
        let processor = (self.capture_processor)();
        let device = self.chosen.lock().0.clone();
        CpalCapture::open(cfg, processor, device).map(|h| Box::new(h) as Box<dyn AudioSource>)
    }

    fn make_playback(&self, cfg: AudioFormat) -> Result<Box<dyn AudioSink>, AnvilError> {
        let device = self.chosen.lock().1.clone();
        CpalPlayback::open(cfg, device).map(|h| Box::new(h) as Box<dyn AudioSink>)
    }

    fn choose_devices(&self, input: Option<&str>, output: Option<&str>) -> Result<(), AnvilError> {
        let devices = self.devices();
        let known =
            |id: &str, is_input: bool| devices.iter().any(|d| d.id == id && d.is_input == is_input);
        for (id, is_input) in [(input, true), (output, false)] {
            if let Some(id) = id.filter(|id| !known(id, is_input)) {
                return Err(AnvilError::AudioDevice(format!(
                    "no {} device {id:?}",
                    if is_input { "input" } else { "output" }
                )));
            }
        }
        *self.chosen.lock() = (input.map(String::from), output.map(String::from));
        Ok(())
    }

    fn chosen_devices(&self) -> (Option<String>, Option<String>) {
        self.chosen.lock().clone()
    }

    fn devices(&self) -> Vec<DeviceInfo> {
        let host = cpal::default_host();
        let mut out = Vec::new();
        if let Ok(inputs) = host.input_devices() {
            let default_name = host.default_input_device().and_then(|d| d.name().ok());
            for dev in inputs {
                if let Ok(name) = dev.name() {
                    out.push(DeviceInfo {
                        id: name.clone(),
                        is_default: Some(&name) == default_name.as_ref(),
                        name,
                        is_input: true,
                    });
                }
            }
        }
        if let Ok(outputs) = host.output_devices() {
            let default_name = host.default_output_device().and_then(|d| d.name().ok());
            for dev in outputs {
                if let Ok(name) = dev.name() {
                    out.push(DeviceInfo {
                        id: name.clone(),
                        is_default: Some(&name) == default_name.as_ref(),
                        name,
                        is_input: false,
                    });
                }
            }
        }
        out
    }
}

/// The device named `wanted` among `devices`; `None` when none was asked
/// for, or the one asked for has gone (unplugged), so the default is used.
fn named<I>(devices: Result<I, cpal::DevicesError>, wanted: Option<&str>) -> Option<cpal::Device>
where
    I: Iterator<Item = cpal::Device>,
{
    let wanted = wanted?;
    let found = devices
        .ok()?
        .find(|d| d.name().ok().as_deref() == Some(wanted));
    if found.is_none() {
        tracing::warn!(
            device = wanted,
            "the chosen audio device is gone; using the default"
        );
    }
    found
}

// ─── Capture ────────────────────────────────────────────────────────────────

struct CpalCapture {
    frame_rx: mpsc::Receiver<AudioFrame>,
    _worker: thread::JoinHandle<()>,
    /// Capture-side processor (AGC etc.). Applied on the async side so
    /// the cpal callback stays lock-free.
    processor: Box<dyn AudioProcessor>,
}

impl CpalCapture {
    fn open(
        cfg: AudioFormat,
        processor: Box<dyn AudioProcessor>,
        device: Option<String>,
    ) -> Result<Self, AnvilError> {
        // 8 frames of headroom; beyond that we drop to avoid unbounded memory
        // growth if the RTP send task stalls.
        let (frame_tx, frame_rx) = mpsc::channel::<AudioFrame>(8);

        let worker = thread::Builder::new()
            .name("anvil-capture".into())
            .spawn(move || run_capture(cfg, frame_tx, device))
            .map_err(|e| AnvilError::AudioDevice(format!("spawn capture thread: {e}")))?;

        Ok(Self {
            frame_rx,
            _worker: worker,
            processor,
        })
    }
}

#[async_trait]
impl AudioSource for CpalCapture {
    async fn next_frame(&mut self) -> Option<AudioFrame> {
        let mut frame = self.frame_rx.recv().await?;
        self.processor.process_capture(&mut frame);
        Some(frame)
    }
}

fn run_capture(cfg: AudioFormat, frame_tx: mpsc::Sender<AudioFrame>, wanted: Option<String>) {
    let host = cpal::default_host();
    let device = match named(host.input_devices(), wanted.as_deref())
        .or_else(|| host.default_input_device())
    {
        Some(d) => d,
        None => {
            tracing::error!("no default input device");
            return;
        }
    };

    let default_config = match device.default_input_config() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(%e, "default_input_config failed");
            return;
        }
    };

    let device_rate = default_config.sample_rate().0;
    let channels = default_config.channels();
    let sample_format = default_config.sample_format();
    let frame_samples = (cfg.sample_rate as u64 * cfg.frame_ms as u64 / 1000) as usize;

    let stream_config: StreamConfig = StreamConfig {
        channels,
        sample_rate: SampleRate(device_rate),
        buffer_size: cpal::BufferSize::Default,
    };

    let mut resampler = match Resampler::new(device_rate, cfg.sample_rate, 1) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(%e, %device_rate, "resampler init");
            return;
        }
    };
    let frame_buf: Arc<Mutex<VecDeque<i16>>> = Arc::new(Mutex::new(VecDeque::new()));

    // The callback hands samples to a small inline pipeline: downmix to mono,
    // resample to 8 kHz, push into the shared frame buffer. Complete 160-sample
    // frames are flushed to the tokio channel via try_send. We use blocking_send
    // on the async side; on the callback side we use try_send to avoid ever
    // blocking the RT audio thread.
    let tx = frame_tx.clone();
    let fmt_for_frames = cfg;
    let buf_for_cb = Arc::clone(&frame_buf);

    let callback_f32 = {
        let tx = tx.clone();
        let buf = Arc::clone(&buf_for_cb);
        move |data: &[f32], _info: &cpal::InputCallbackInfo| {
            let mono = downmix_f32(data, channels);
            push_frames(
                &mut resampler,
                mono,
                &buf,
                frame_samples,
                fmt_for_frames,
                &tx,
            );
        }
    };

    // cpal can deliver samples in several formats depending on the driver.
    let stream = match sample_format {
        SampleFormat::F32 => {
            device.build_input_stream(&stream_config, callback_f32, report_input_err, None)
        }
        SampleFormat::I16 => {
            let tx = tx.clone();
            let buf = Arc::clone(&buf_for_cb);
            // Reuse the same pipeline: i16 → f32 → same path. Allocating is
            // OK on Linux/desktop; real-time audio constraints on mobile are
            // handled by the host natively, not by cpal.
            let mut rs = match Resampler::new(device_rate, cfg.sample_rate, 1) {
                Ok(r) => r,
                Err(e) => {
                    tracing::error!(%e, "resampler init (i16 path)");
                    return;
                }
            };
            device.build_input_stream(
                &stream_config,
                move |data: &[i16], _info: &cpal::InputCallbackInfo| {
                    let mono = downmix_i16(data, channels);
                    push_frames(&mut rs, mono, &buf, frame_samples, fmt_for_frames, &tx);
                },
                report_input_err,
                None,
            )
        }
        SampleFormat::U16 => {
            let tx = tx.clone();
            let buf = Arc::clone(&buf_for_cb);
            let mut rs = match Resampler::new(device_rate, cfg.sample_rate, 1) {
                Ok(r) => r,
                Err(e) => {
                    tracing::error!(%e, "resampler init (u16 path)");
                    return;
                }
            };
            device.build_input_stream(
                &stream_config,
                move |data: &[u16], _info: &cpal::InputCallbackInfo| {
                    let mono: Vec<i16> = data
                        .chunks(channels as usize)
                        .map(|frame| {
                            let sum: i32 = frame.iter().map(|&s| (s as i32) - 32768).sum();
                            (sum / channels as i32).clamp(-32768, 32767) as i16
                        })
                        .collect();
                    push_frames(&mut rs, mono, &buf, frame_samples, fmt_for_frames, &tx);
                },
                report_input_err,
                None,
            )
        }
        other => {
            tracing::error!(?other, "unsupported input sample format");
            return;
        }
    };

    let stream = match stream {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(%e, "build_input_stream failed");
            return;
        }
    };

    if let Err(e) = stream.play() {
        tracing::error!(%e, "input stream play failed");
        return;
    }

    tracing::info!(
        device_rate,
        channels,
        target_rate = cfg.sample_rate,
        "audio capture running"
    );

    // Stream runs until frame_tx is closed (the AudioSource was dropped).
    while !tx.is_closed() {
        thread::park_timeout(std::time::Duration::from_millis(200));
    }

    tracing::debug!("audio capture stopping");
}

// ─── Playback ───────────────────────────────────────────────────────────────

struct CpalPlayback {
    frame_tx: mpsc::Sender<AudioFrame>,
    _worker: thread::JoinHandle<()>,
}

impl CpalPlayback {
    fn open(cfg: AudioFormat, device: Option<String>) -> Result<Self, AnvilError> {
        let (frame_tx, frame_rx) = mpsc::channel::<AudioFrame>(8);

        let worker = thread::Builder::new()
            .name("anvil-playback".into())
            .spawn(move || run_playback(cfg, frame_rx, device))
            .map_err(|e| AnvilError::AudioDevice(format!("spawn playback thread: {e}")))?;

        Ok(Self {
            frame_tx,
            _worker: worker,
        })
    }
}

#[async_trait]
impl AudioSink for CpalPlayback {
    async fn write_frame(&mut self, frame: &AudioFrame) -> Result<(), AnvilError> {
        // Clone the frame; the channel owns it until the playback side pops it.
        self.frame_tx
            .send(frame.clone())
            .await
            .map_err(|_| AnvilError::AudioDevice("playback channel closed".into()))
    }
}

fn run_playback(
    cfg: AudioFormat,
    mut frame_rx: mpsc::Receiver<AudioFrame>,
    wanted: Option<String>,
) {
    let host = cpal::default_host();
    let device = match named(host.output_devices(), wanted.as_deref())
        .or_else(|| host.default_output_device())
    {
        Some(d) => d,
        None => {
            tracing::error!("no default output device");
            return;
        }
    };

    let default_config = match device.default_output_config() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(%e, "default_output_config failed");
            return;
        }
    };
    let device_rate = default_config.sample_rate().0;
    let channels = default_config.channels();
    let sample_format = default_config.sample_format();

    let stream_config: StreamConfig = StreamConfig {
        channels,
        sample_rate: SampleRate(device_rate),
        buffer_size: cpal::BufferSize::Default,
    };

    // Shared sample queue: resampled mono i16 at device rate, feed the
    // cpal callback. parking_lot::Mutex keeps lock cycles short.
    let out_buf: Arc<Mutex<VecDeque<i16>>> = Arc::new(Mutex::new(VecDeque::with_capacity(
        (device_rate as usize).max(8000), // ~1 s of slack
    )));

    let buf_for_cb = Arc::clone(&out_buf);
    let callback_f32 = move |data: &mut [f32], _info: &cpal::OutputCallbackInfo| {
        let mut guard = buf_for_cb.lock();
        for frame in data.chunks_mut(channels as usize) {
            let sample = guard.pop_front().unwrap_or(0);
            let f = (sample as f32) / (i16::MAX as f32);
            for ch in frame.iter_mut() {
                *ch = f;
            }
        }
    };

    let stream = match sample_format {
        SampleFormat::F32 => {
            device.build_output_stream(&stream_config, callback_f32, report_output_err, None)
        }
        SampleFormat::I16 => {
            let buf = Arc::clone(&out_buf);
            device.build_output_stream(
                &stream_config,
                move |data: &mut [i16], _info: &cpal::OutputCallbackInfo| {
                    let mut guard = buf.lock();
                    for frame in data.chunks_mut(channels as usize) {
                        let sample = guard.pop_front().unwrap_or(0);
                        for ch in frame.iter_mut() {
                            *ch = sample;
                        }
                    }
                },
                report_output_err,
                None,
            )
        }
        SampleFormat::U16 => {
            let buf = Arc::clone(&out_buf);
            device.build_output_stream(
                &stream_config,
                move |data: &mut [u16], _info: &cpal::OutputCallbackInfo| {
                    let mut guard = buf.lock();
                    for frame in data.chunks_mut(channels as usize) {
                        let sample = guard.pop_front().unwrap_or(0);
                        let u = ((sample as i32) + 32768).clamp(0, 65535) as u16;
                        for ch in frame.iter_mut() {
                            *ch = u;
                        }
                    }
                },
                report_output_err,
                None,
            )
        }
        other => {
            tracing::error!(?other, "unsupported output sample format");
            return;
        }
    };

    let stream = match stream {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(%e, "build_output_stream failed");
            return;
        }
    };
    if let Err(e) = stream.play() {
        tracing::error!(%e, "output stream play failed");
        return;
    }

    let mut upsampler = match Resampler::new(cfg.sample_rate, device_rate, 1) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(%e, "output resampler init");
            return;
        }
    };

    tracing::info!(
        device_rate,
        channels,
        source_rate = cfg.sample_rate,
        "audio playback running"
    );

    // Drain frames, resample up to device rate, push into out_buf.
    while let Some(frame) = frame_rx.blocking_recv() {
        let upsampled = match upsampler.resample(&frame.samples) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(%e, "playback resample");
                continue;
            }
        };
        let mut guard = out_buf.lock();
        guard.extend(upsampled);
        // Cap at 200ms to keep latency bounded if the producer runs ahead.
        let cap = (device_rate as usize * 200) / 1000;
        while guard.len() > cap {
            guard.pop_front();
        }
    }

    tracing::debug!("audio playback stopping");
}

// ─── helpers ────────────────────────────────────────────────────────────────

fn downmix_f32(data: &[f32], channels: u16) -> Vec<i16> {
    data.chunks(channels as usize)
        .map(|frame| {
            let avg = frame.iter().sum::<f32>() / (channels.max(1) as f32);
            (avg * i16::MAX as f32).clamp(i16::MIN as f32, i16::MAX as f32) as i16
        })
        .collect()
}

fn downmix_i16(data: &[i16], channels: u16) -> Vec<i16> {
    data.chunks(channels as usize)
        .map(|frame| {
            let sum: i32 = frame.iter().map(|&s| s as i32).sum();
            (sum / channels.max(1) as i32).clamp(-32768, 32767) as i16
        })
        .collect()
}

fn push_frames(
    resampler: &mut Resampler,
    mono: Vec<i16>,
    buf: &Arc<Mutex<VecDeque<i16>>>,
    frame_samples: usize,
    fmt: AudioFormat,
    tx: &mpsc::Sender<AudioFrame>,
) {
    let resampled = match resampler.resample(&mono) {
        Ok(v) => v,
        Err(_) => return,
    };
    let mut guard = buf.lock();
    guard.extend(resampled);
    while guard.len() >= frame_samples {
        let frame: Vec<i16> = guard.drain(..frame_samples).collect();
        if tx
            .try_send(AudioFrame {
                samples: frame,
                format: fmt,
            })
            .is_err()
        {
            // Consumer stalled; drop the frame to keep moving forward.
            // A warning would spam the log.
        }
    }
}

fn report_input_err(err: cpal::StreamError) {
    tracing::warn!(?err, "capture stream error");
}

fn report_output_err(err: cpal::StreamError) {
    tracing::warn!(?err, "playback stream error");
}
