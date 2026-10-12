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

#[cfg(feature = "apm")]
use anvil_core::audio::Processing;
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
    /// The device chosen for ringing; `None` is the system's default.
    ring: Mutex<Option<String>>,
    /// What is done to the microphone, read by each call's canceller.
    #[cfg(feature = "apm")]
    processing: std::sync::Arc<Mutex<Processing>>,
    /// What calls' speakers play, for the cancellers.
    #[cfg(feature = "apm")]
    reference: std::sync::Arc<crate::echo::Reference>,
}

/// What a stream is for, which decides its default device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    /// A call's audio: on Windows the default device for communications.
    Call,
    /// Ringing: the system's default (on Windows the speakers, not a
    /// headset).
    Ring,
}

impl CpalHost {
    /// Create a host using the system default input and output devices,
    /// with a no-op capture processor.
    pub fn new() -> Result<Self, AnvilError> {
        Ok(Self {
            capture_processor: Box::new(|| Box::new(NullProcessor)),
            chosen: Mutex::new((None, None)),
            ring: Mutex::new(None),
            #[cfg(feature = "apm")]
            processing: std::sync::Arc::new(Mutex::new(Processing::default())),
            #[cfg(feature = "apm")]
            reference: crate::echo::Reference::new(),
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
        #[cfg(feature = "apm")]
        let processor = Box::new(crate::echo::Canceller::new(
            &self.reference,
            std::sync::Arc::clone(&self.processing),
            processor,
        ));
        let device = self.chosen.lock().0.clone();
        CpalCapture::open(cfg, processor, device).map(|h| Box::new(h) as Box<dyn AudioSource>)
    }

    fn make_playback(&self, cfg: AudioFormat) -> Result<Box<dyn AudioSink>, AnvilError> {
        let device = self.chosen.lock().1.clone();
        #[cfg(feature = "apm")]
        let tap = Some(self.reference.tap(cfg.sample_rate));
        #[cfg(not(feature = "apm"))]
        let tap = None;
        CpalPlayback::open(cfg, device, Role::Call, tap).map(|h| Box::new(h) as Box<dyn AudioSink>)
    }

    fn make_ringer(&self, cfg: AudioFormat) -> Result<Box<dyn AudioSink>, AnvilError> {
        let device = self.ring.lock().clone();
        CpalPlayback::open(cfg, device, Role::Ring, None).map(|h| Box::new(h) as Box<dyn AudioSink>)
    }

    #[cfg(feature = "apm")]
    fn set_processing(&self, processing: Processing) -> Result<(), AnvilError> {
        *self.processing.lock() = processing;
        Ok(())
    }

    #[cfg(feature = "apm")]
    fn processing(&self) -> Option<Processing> {
        Some(*self.processing.lock())
    }

    fn choose_ring_device(&self, output: Option<&str>) -> Result<(), AnvilError> {
        if let Some(id) = output {
            if !self.devices().iter().any(|d| d.id == id && !d.is_input) {
                return Err(AnvilError::AudioDevice(format!("no output device {id:?}")));
            }
        }
        *self.ring.lock() = output.map(String::from);
        Ok(())
    }

    fn chosen_ring_device(&self) -> Option<String> {
        self.ring.lock().clone()
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
            let default_name = default_name(&host, true, Role::Call);
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
            let default_name = default_name(&host, false, Role::Call);
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

/// The device named `wanted` among `devices`, quietly.
fn find_named<I>(devices: Result<I, cpal::DevicesError>, wanted: &str) -> Option<cpal::Device>
where
    I: Iterator<Item = cpal::Device>,
{
    devices
        .ok()?
        .find(|d| d.name().ok().as_deref() == Some(wanted))
}

/// The device named `wanted`, for capture (`input`) or playback.
fn named(host: &cpal::Host, input: bool, wanted: &str) -> Option<cpal::Device> {
    if input {
        find_named(host.input_devices(), wanted)
    } else {
        find_named(host.output_devices(), wanted)
    }
}

/// The default device for a call: on Windows the one for communications
/// (a headset), else the system's default; for ringing, the system's
/// default.
fn default_device(host: &cpal::Host, input: bool, role: Role) -> Option<cpal::Device> {
    #[cfg(windows)]
    if role == Role::Call {
        if let Some(device) = crate::comms::default_name(input).and_then(|n| named(host, input, &n))
        {
            return Some(device);
        }
    }
    #[cfg(not(windows))]
    let _ = role;
    if input {
        host.default_input_device()
    } else {
        host.default_output_device()
    }
}

/// The default device's name.
fn default_name(host: &cpal::Host, input: bool, role: Role) -> Option<String> {
    default_device(host, input, role).and_then(|d| d.name().ok())
}

/// The device a stream opens on: the one chosen, while it is there; else
/// the default for its role, which the stream then follows (`true`).
fn pick(
    host: &cpal::Host,
    input: bool,
    wanted: Option<&str>,
    role: Role,
) -> Option<(cpal::Device, bool)> {
    if let Some(wanted) = wanted {
        if let Some(device) = named(host, input, wanted) {
            return Some((device, false));
        }
        tracing::warn!(
            device = wanted,
            "the chosen audio device is gone; using the default"
        );
    }
    default_device(host, input, role).map(|d| (d, true))
}

/// Whether a stream on the default, `current`, should move: the default
/// is another device now, or the device chosen is back.
fn should_move(
    host: &cpal::Host,
    input: bool,
    wanted: Option<&str>,
    current: &str,
    role: Role,
) -> bool {
    if let Some(wanted) = wanted {
        if named(host, input, wanted).is_some() {
            tracing::info!(device = wanted, "the chosen audio device is back");
            return true;
        }
    }
    match default_name(host, input, role) {
        Some(default) if default != current => {
            tracing::info!(from = current, to = %default, "the default audio device changed");
            true
        }
        _ => false,
    }
}

/// How often a stream on the default checks whether it should move.
const FOLLOW_EVERY: std::time::Duration = std::time::Duration::from_secs(2);

/// A stream error callback that marks the stream as failed (a device
/// unplugged), for its thread to open another.
fn on_error(
    what: &'static str,
    failed: &Arc<std::sync::atomic::AtomicBool>,
) -> impl FnMut(cpal::StreamError) + Send + 'static {
    let failed = Arc::clone(failed);
    move |err| {
        tracing::warn!(?err, "{what} stream error");
        failed.store(true, std::sync::atomic::Ordering::Relaxed);
    }
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

/// Open a capture stream on `device` feeding `frame_tx`; `failed` is set
/// when it stops working.
fn open_capture(
    device: &cpal::Device,
    cfg: AudioFormat,
    frame_tx: &mpsc::Sender<AudioFrame>,
    failed: &Arc<std::sync::atomic::AtomicBool>,
) -> Result<(cpal::Stream, u32, u16), String> {
    let default_config = device
        .default_input_config()
        .map_err(|e| format!("default_input_config: {e}"))?;

    let device_rate = default_config.sample_rate().0;
    let channels = default_config.channels();
    let sample_format = default_config.sample_format();
    let frame_samples = (cfg.sample_rate as u64 * cfg.frame_ms as u64 / 1000) as usize;

    let stream_config: StreamConfig = StreamConfig {
        channels,
        sample_rate: SampleRate(device_rate),
        buffer_size: cpal::BufferSize::Default,
    };

    let mut resampler = Resampler::new(device_rate, cfg.sample_rate, 1)
        .map_err(|e| format!("resampler at {device_rate} Hz: {e}"))?;
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
        SampleFormat::F32 => device.build_input_stream(
            &stream_config,
            callback_f32,
            on_error("capture", failed),
            None,
        ),
        SampleFormat::I16 => {
            let tx = tx.clone();
            let buf = Arc::clone(&buf_for_cb);
            // Reuse the same pipeline: i16 → f32 → same path. Allocating is
            // OK on Linux/desktop; real-time audio constraints on mobile are
            // handled by the host natively, not by cpal.
            let mut rs = Resampler::new(device_rate, cfg.sample_rate, 1)
                .map_err(|e| format!("resampler: {e}"))?;
            device.build_input_stream(
                &stream_config,
                move |data: &[i16], _info: &cpal::InputCallbackInfo| {
                    let mono = downmix_i16(data, channels);
                    push_frames(&mut rs, mono, &buf, frame_samples, fmt_for_frames, &tx);
                },
                on_error("capture", failed),
                None,
            )
        }
        SampleFormat::U16 => {
            let tx = tx.clone();
            let buf = Arc::clone(&buf_for_cb);
            let mut rs = Resampler::new(device_rate, cfg.sample_rate, 1)
                .map_err(|e| format!("resampler: {e}"))?;
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
                on_error("capture", failed),
                None,
            )
        }
        other => return Err(format!("unsupported input sample format {other:?}")),
    };

    let stream = stream.map_err(|e| format!("build_input_stream: {e}"))?;
    stream
        .play()
        .map_err(|e| format!("input stream play: {e}"))?;
    Ok((stream, device_rate, channels))
}

/// Capture for a call until its source is dropped: on the device chosen or
/// the default, opened again on another when it fails (unplugged) or, on
/// the default, when the default changes.
fn run_capture(cfg: AudioFormat, frame_tx: mpsc::Sender<AudioFrame>, wanted: Option<String>) {
    let host = cpal::default_host();
    while !frame_tx.is_closed() {
        let failed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let opened = pick(&host, true, wanted.as_deref(), Role::Call)
            .ok_or_else(|| "no input device".to_string())
            .and_then(|(device, following)| {
                let name = device.name().unwrap_or_default();
                open_capture(&device, cfg, &frame_tx, &failed).map(|s| (s, name, following))
            });
        let ((stream, device_rate, channels), name, following) = match opened {
            Ok(opened) => opened,
            Err(e) => {
                tracing::error!(%e, "audio capture not opened; trying again");
                thread::sleep(std::time::Duration::from_secs(1));
                continue;
            }
        };
        tracing::info!(
            device = %name,
            device_rate,
            channels,
            target_rate = cfg.sample_rate,
            "audio capture running"
        );
        let mut checked = std::time::Instant::now();
        // Until the source is dropped, the stream fails, or it should move.
        loop {
            if frame_tx.is_closed() {
                tracing::debug!("audio capture stopping");
                return;
            }
            if failed.load(std::sync::atomic::Ordering::Relaxed) {
                tracing::warn!(device = %name, "the capture device failed; opening another");
                break;
            }
            if following && checked.elapsed() >= FOLLOW_EVERY {
                checked = std::time::Instant::now();
                if should_move(&host, true, wanted.as_deref(), &name, Role::Call) {
                    break;
                }
            }
            thread::park_timeout(std::time::Duration::from_millis(200));
        }
        drop(stream);
        thread::sleep(std::time::Duration::from_millis(200));
    }
}

// ─── Playback ───────────────────────────────────────────────────────────────

struct CpalPlayback {
    frame_tx: mpsc::Sender<AudioFrame>,
    _worker: thread::JoinHandle<()>,
    /// Where what a call's speaker plays is kept for the echo canceller.
    tap: Option<Tap>,
}

#[cfg(feature = "apm")]
type Tap = crate::echo::Tap;
/// No canceller to tell.
#[cfg(not(feature = "apm"))]
struct Tap;
#[cfg(not(feature = "apm"))]
impl Tap {
    fn played(&self, _: &AudioFrame) {}
}

impl CpalPlayback {
    fn open(
        cfg: AudioFormat,
        device: Option<String>,
        role: Role,
        tap: Option<Tap>,
    ) -> Result<Self, AnvilError> {
        let (frame_tx, frame_rx) = mpsc::channel::<AudioFrame>(8);

        let worker = thread::Builder::new()
            .name("anvil-playback".into())
            .spawn(move || run_playback(cfg, frame_rx, device, role))
            .map_err(|e| AnvilError::AudioDevice(format!("spawn playback thread: {e}")))?;

        Ok(Self {
            frame_tx,
            _worker: worker,
            tap,
        })
    }
}

#[async_trait]
impl AudioSink for CpalPlayback {
    async fn write_frame(&mut self, frame: &AudioFrame) -> Result<(), AnvilError> {
        if let Some(tap) = &self.tap {
            tap.played(frame);
        }
        // Clone the frame; the channel owns it until the playback side pops it.
        self.frame_tx
            .send(frame.clone())
            .await
            .map_err(|_| AnvilError::AudioDevice("playback channel closed".into()))
    }
}

/// Open a playback stream on `device`, fed from its own queue; `failed`
/// is set when it stops working.
#[allow(clippy::type_complexity)]
fn open_playback(
    device: &cpal::Device,
    failed: &Arc<std::sync::atomic::AtomicBool>,
) -> Result<(cpal::Stream, Arc<Mutex<VecDeque<i16>>>, u32, u16), String> {
    let default_config = device
        .default_output_config()
        .map_err(|e| format!("default_output_config: {e}"))?;
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
        SampleFormat::F32 => device.build_output_stream(
            &stream_config,
            callback_f32,
            on_error("playback", failed),
            None,
        ),
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
                on_error("playback", failed),
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
                on_error("playback", failed),
                None,
            )
        }
        other => return Err(format!("unsupported output sample format {other:?}")),
    };

    let stream = stream.map_err(|e| format!("build_output_stream: {e}"))?;
    stream
        .play()
        .map_err(|e| format!("output stream play: {e}"))?;
    Ok((stream, out_buf, device_rate, channels))
}

/// Play a call (or a ring) until its sink is dropped: on the device chosen
/// or the default, opened again on another when it fails (unplugged) or,
/// on the default, when the default changes.
fn run_playback(
    cfg: AudioFormat,
    mut frame_rx: mpsc::Receiver<AudioFrame>,
    wanted: Option<String>,
    role: Role,
) {
    let host = cpal::default_host();
    loop {
        let failed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let opened = pick(&host, false, wanted.as_deref(), role)
            .ok_or_else(|| "no output device".to_string())
            .and_then(|(device, following)| {
                let name = device.name().unwrap_or_default();
                open_playback(&device, &failed).map(|s| (s, name, following))
            })
            .and_then(|(opened, name, following)| {
                Resampler::new(cfg.sample_rate, opened.2, 1)
                    .map(|up| (opened, up, name, following))
                    .map_err(|e| format!("output resampler: {e}"))
            });
        let ((stream, out_buf, device_rate, channels), mut upsampler, name, following) =
            match opened {
                Ok(opened) => opened,
                Err(e) => {
                    tracing::error!(%e, "audio playback not opened; trying again");
                    // The call's audio is dropped meanwhile, not queued.
                    let until = std::time::Instant::now() + std::time::Duration::from_secs(1);
                    while std::time::Instant::now() < until {
                        match frame_rx.try_recv() {
                            Err(mpsc::error::TryRecvError::Disconnected) => return,
                            _ => thread::sleep(std::time::Duration::from_millis(20)),
                        }
                    }
                    continue;
                }
            };
        tracing::info!(
            device = %name,
            device_rate,
            channels,
            source_rate = cfg.sample_rate,
            "audio playback running"
        );

        // Drain frames, resample up to device rate, push into out_buf,
        // until the sink is dropped, the stream fails, or it should move.
        let mut checked = std::time::Instant::now();
        let moved = loop {
            let Some(frame) = frame_rx.blocking_recv() else {
                break false;
            };
            match upsampler.resample(&frame.samples) {
                Ok(upsampled) => {
                    let mut guard = out_buf.lock();
                    guard.extend(upsampled);
                    // Cap at 200ms to keep latency bounded if the producer
                    // runs ahead.
                    let cap = (device_rate as usize * 200) / 1000;
                    while guard.len() > cap {
                        guard.pop_front();
                    }
                }
                Err(e) => tracing::warn!(%e, "playback resample"),
            }
            if failed.load(std::sync::atomic::Ordering::Relaxed) {
                tracing::warn!(device = %name, "the playback device failed; opening another");
                break true;
            }
            if following && checked.elapsed() >= FOLLOW_EVERY {
                checked = std::time::Instant::now();
                if should_move(&host, false, wanted.as_deref(), &name, role) {
                    break true;
                }
            }
        };
        drop(stream);
        if !moved {
            tracing::debug!("audio playback stopping");
            return;
        }
    }
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
