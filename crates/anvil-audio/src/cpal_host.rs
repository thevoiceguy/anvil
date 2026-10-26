//! cpal-backed implementation of `AudioHost`.
//!
//! Each call opens its own microphone stream. The `AudioSource` handle
//! returned from `make_capture`, like a ringer's sink, owns a dedicated OS
//! thread that holds the `cpal::Stream` (streams are `!Send` on some
//! platforms); samples cross the thread boundary via bounded tokio
//! channels, and dropping the handle ends the thread and the stream.
//!
//! Calls share one speaker stream ([`CallOutput`]): each call's sink is a
//! voice in it, the stream mixes them, and it stays open for a while after
//! the last call. A network speaker or a Bluetooth headset wakes for a
//! second or two on a new stream; a second call (the park code dialled
//! from a held call) opening its own lost its first words to that.
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
    /// The speaker calls share, while it is open.
    output: Mutex<Option<Arc<CallOutput>>>,
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
            output: Mutex::new(None),
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
        // A voice in the open speaker, when it is open on the device
        // wanted; else a new speaker (the old one closes with its calls).
        let mut slot = self.output.lock();
        if let Some(output) = slot.as_ref() {
            if output.wanted == device {
                if let Some(voice) = output.join() {
                    return Ok(Box::new(CallVoice::new(cfg, output, voice, tap)));
                }
            } else {
                output.retire();
            }
        }
        let output = CallOutput::open(device)?;
        let voice = output.join().expect("a new speaker takes a voice");
        let sink = CallVoice::new(cfg, &output, voice, tap);
        *slot = Some(output);
        Ok(Box::new(sink))
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

// ─── The calls' speaker ──────────────────────────────────────────────────────

/// How long the calls' speaker stays open after its last call.
const OUTPUT_LINGER: std::time::Duration = std::time::Duration::from_secs(15);

/// How much a voice may queue ahead of the speaker, in ms.
const VOICE_QUEUE_MS: usize = 200;

/// One call's audio in the speaker: mono samples at the device's rate,
/// taken by the stream's callback.
type Voice = Mutex<VecDeque<i16>>;

/// The speaker calls share: one stream on one device, held by its own
/// thread, mixing every voice in it.
struct CallOutput {
    /// The device chosen when it opened; `None` is the default.
    wanted: Option<String>,
    /// The voices playing, and whether the speaker has closed.
    voices: Arc<Mutex<Voices>>,
    /// The device's rate, 0 until a device is open.
    device_rate: Arc<std::sync::atomic::AtomicU32>,
    /// No more voices: another device is wanted. It closes with its last.
    retired: Arc<std::sync::atomic::AtomicBool>,
}

#[derive(Default)]
struct Voices {
    playing: Vec<std::sync::Weak<Voice>>,
    closed: bool,
}

impl Voices {
    fn live(&mut self) -> usize {
        self.playing.retain(|v| v.strong_count() > 0);
        self.playing.len()
    }
}

impl CallOutput {
    fn open(wanted: Option<String>) -> Result<Arc<Self>, AnvilError> {
        let output = Arc::new(Self {
            wanted: wanted.clone(),
            voices: Arc::new(Mutex::new(Voices::default())),
            device_rate: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            retired: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });
        let voices = Arc::clone(&output.voices);
        let device_rate = Arc::clone(&output.device_rate);
        let retired = Arc::clone(&output.retired);
        thread::Builder::new()
            .name("anvil-speaker".into())
            .spawn(move || run_output(wanted, voices, device_rate, retired))
            .map_err(|e| AnvilError::AudioDevice(format!("spawn playback thread: {e}")))?;
        Ok(output)
    }

    /// A new voice, unless the speaker has closed or is closing.
    fn join(&self) -> Option<Arc<Voice>> {
        if self.retired.load(std::sync::atomic::Ordering::Relaxed) {
            return None;
        }
        let mut voices = self.voices.lock();
        if voices.closed {
            return None;
        }
        let voice = Arc::new(Mutex::new(VecDeque::new()));
        voices.playing.push(Arc::downgrade(&voice));
        Some(voice)
    }

    fn retire(&self) {
        self.retired
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// A call's sink: its frames resampled to the device's rate into its voice.
struct CallVoice {
    voice: Arc<Voice>,
    device_rate: Arc<std::sync::atomic::AtomicU32>,
    source_rate: u32,
    /// The resampler for the device's rate it was made for.
    resampler: Option<(u32, Resampler)>,
    tap: Option<Tap>,
}

impl CallVoice {
    fn new(cfg: AudioFormat, output: &CallOutput, voice: Arc<Voice>, tap: Option<Tap>) -> Self {
        Self {
            voice,
            device_rate: Arc::clone(&output.device_rate),
            source_rate: cfg.sample_rate,
            resampler: None,
            tap,
        }
    }
}

#[async_trait]
impl AudioSink for CallVoice {
    async fn write_frame(&mut self, frame: &AudioFrame) -> Result<(), AnvilError> {
        if let Some(tap) = &self.tap {
            tap.played(frame);
        }
        // Dropped while no device is open, not queued.
        let rate = self.device_rate.load(std::sync::atomic::Ordering::Relaxed);
        if rate == 0 {
            return Ok(());
        }
        if self.resampler.as_ref().map(|(r, _)| *r) != Some(rate) {
            let resampler = Resampler::new(self.source_rate, rate, 1)
                .map_err(|e| AnvilError::AudioDevice(format!("output resampler: {e}")))?;
            self.resampler = Some((rate, resampler));
        }
        let Some((_, resampler)) = self.resampler.as_mut() else {
            return Ok(());
        };
        match resampler.resample(&frame.samples) {
            Ok(samples) => {
                let mut queue = self.voice.lock();
                queue.extend(samples);
                // Bounded latency when the far end runs ahead.
                let cap = rate as usize * VOICE_QUEUE_MS / 1000;
                while queue.len() > cap {
                    queue.pop_front();
                }
            }
            Err(e) => tracing::warn!(%e, "playback resample"),
        }
        Ok(())
    }
}

/// What a stream's callback keeps between calls, so it does not allocate
/// once warm: the voices of this callback, and their mix.
#[derive(Default)]
struct Mixer {
    playing: Vec<Arc<Voice>>,
    mixed: Vec<i32>,
}

impl Mixer {
    /// The next `frames` samples of every voice in `voices`, summed.
    fn mix(&mut self, voices: &Mutex<Voices>, frames: usize) -> &[i32] {
        self.playing.clear();
        self.playing.extend(
            voices
                .lock()
                .playing
                .iter()
                .filter_map(std::sync::Weak::upgrade),
        );
        self.mixed.clear();
        self.mixed.resize(frames, 0);
        for voice in &self.playing {
            let mut queue = voice.lock();
            let take = frames.min(queue.len());
            for (sum, sample) in self.mixed.iter_mut().zip(queue.drain(..take)) {
                *sum += i32::from(sample);
            }
        }
        // A voice let go here, not in the next callback's lock.
        self.playing.clear();
        &self.mixed
    }
}

/// A mixed sample as an `i16`.
fn clip(sum: i32) -> i16 {
    sum.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

/// Open the calls' speaker on `device`, mixing `voices`.
fn open_output(
    device: &cpal::Device,
    voices: &Arc<Mutex<Voices>>,
    failed: &Arc<std::sync::atomic::AtomicBool>,
) -> Result<(cpal::Stream, u32, u16), String> {
    let default_config = device
        .default_output_config()
        .map_err(|e| format!("default_output_config: {e}"))?;
    let device_rate = default_config.sample_rate().0;
    let channels = default_config.channels();
    let stream_config = StreamConfig {
        channels,
        sample_rate: SampleRate(device_rate),
        buffer_size: cpal::BufferSize::Default,
    };
    let stream = match default_config.sample_format() {
        SampleFormat::F32 => {
            let voices = Arc::clone(voices);
            let mut mixer = Mixer::default();
            device.build_output_stream(
                &stream_config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    let frames = data.len() / channels as usize;
                    let mixed = mixer.mix(&voices, frames);
                    for (frame, sum) in data.chunks_mut(channels as usize).zip(mixed) {
                        frame.fill(f32::from(clip(*sum)) / f32::from(i16::MAX));
                    }
                },
                on_error("playback", failed),
                None,
            )
        }
        SampleFormat::I16 => {
            let voices = Arc::clone(voices);
            let mut mixer = Mixer::default();
            device.build_output_stream(
                &stream_config,
                move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                    let frames = data.len() / channels as usize;
                    let mixed = mixer.mix(&voices, frames);
                    for (frame, sum) in data.chunks_mut(channels as usize).zip(mixed) {
                        frame.fill(clip(*sum));
                    }
                },
                on_error("playback", failed),
                None,
            )
        }
        SampleFormat::U16 => {
            let voices = Arc::clone(voices);
            let mut mixer = Mixer::default();
            device.build_output_stream(
                &stream_config,
                move |data: &mut [u16], _: &cpal::OutputCallbackInfo| {
                    let frames = data.len() / channels as usize;
                    let mixed = mixer.mix(&voices, frames);
                    for (frame, sum) in data.chunks_mut(channels as usize).zip(mixed) {
                        frame.fill((i32::from(clip(*sum)) + 32768) as u16);
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
    Ok((stream, device_rate, channels))
}

/// Hold the calls' speaker open: on the device chosen or the default,
/// opened again on another when it fails or the default changes, until
/// it has had no voice for [`OUTPUT_LINGER`] (at once when retired).
fn run_output(
    wanted: Option<String>,
    voices: Arc<Mutex<Voices>>,
    device_rate: Arc<std::sync::atomic::AtomicU32>,
    retired: Arc<std::sync::atomic::AtomicBool>,
) {
    use std::sync::atomic::Ordering;
    let host = cpal::default_host();
    let mut idle_since: Option<std::time::Instant> = None;
    // Whether to close: no voice for long enough. Decided under the
    // voices' lock, so a voice joins an open speaker or a new one.
    let mut done = || {
        let mut voices = voices.lock();
        if voices.live() > 0 {
            idle_since = None;
            return false;
        }
        let since = *idle_since.get_or_insert_with(std::time::Instant::now);
        if retired.load(Ordering::Relaxed) || since.elapsed() >= OUTPUT_LINGER {
            voices.closed = true;
            return true;
        }
        false
    };
    loop {
        let failed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let opened = pick(&host, false, wanted.as_deref(), Role::Call)
            .ok_or_else(|| "no output device".to_string())
            .and_then(|(device, following)| {
                let name = device.name().unwrap_or_default();
                open_output(&device, &voices, &failed).map(|s| (s, name, following))
            });
        let ((stream, rate, channels), name, following) = match opened {
            Ok(opened) => opened,
            Err(e) => {
                tracing::error!(%e, "audio playback not opened; trying again");
                device_rate.store(0, Ordering::Relaxed);
                thread::sleep(std::time::Duration::from_secs(1));
                if done() {
                    return;
                }
                continue;
            }
        };
        device_rate.store(rate, Ordering::Relaxed);
        tracing::info!(device = %name, device_rate = rate, channels, "audio playback running");

        let mut checked = std::time::Instant::now();
        let reopen = loop {
            thread::sleep(std::time::Duration::from_millis(100));
            if failed.load(Ordering::Relaxed) {
                tracing::warn!(device = %name, "the playback device failed; opening another");
                break true;
            }
            if following && checked.elapsed() >= FOLLOW_EVERY {
                checked = std::time::Instant::now();
                if should_move(&host, false, wanted.as_deref(), &name, Role::Call) {
                    break true;
                }
            }
            if done() {
                break false;
            }
        };
        drop(stream);
        if !reopen {
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

#[cfg(test)]
mod output_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU32};

    fn output() -> CallOutput {
        CallOutput {
            wanted: None,
            voices: Arc::new(Mutex::new(Voices::default())),
            device_rate: Arc::new(AtomicU32::new(48_000)),
            retired: Arc::new(AtomicBool::new(false)),
        }
    }

    #[test]
    fn voices_are_mixed_and_a_short_one_is_silence_after() {
        let output = output();
        let a = output.join().unwrap();
        let b = output.join().unwrap();
        a.lock().extend([100, 200, 300]);
        b.lock().extend([1, 2]);
        let mut mixer = Mixer::default();
        assert_eq!(mixer.mix(&output.voices, 4), &[101, 202, 300, 0]);
        assert!(a.lock().is_empty() && b.lock().is_empty());
        assert_eq!(clip(40_000), i16::MAX);
        assert_eq!(clip(-40_000), i16::MIN);
    }

    #[test]
    fn a_call_gone_is_no_longer_heard() {
        let output = output();
        let a = output.join().unwrap();
        a.lock().extend([5; 4]);
        drop(a);
        assert_eq!(output.voices.lock().live(), 0);
        assert_eq!(Mixer::default().mix(&output.voices, 2), &[0, 0]);
    }

    #[test]
    fn a_closed_or_retired_speaker_takes_no_voice() {
        let closed = output();
        closed.voices.lock().closed = true;
        assert!(closed.join().is_none());
        let retired = output();
        retired.retire();
        assert!(retired.join().is_none());
    }

    #[tokio::test]
    async fn a_call_resamples_into_its_voice_once_a_device_is_open() {
        let output = output();
        output
            .device_rate
            .store(0, std::sync::atomic::Ordering::Relaxed);
        let voice = output.join().unwrap();
        let format = AudioFormat {
            sample_rate: 8_000,
            channels: 1,
            frame_ms: 20,
        };
        let mut sink = CallVoice::new(format, &output, Arc::clone(&voice), None);
        let frame = AudioFrame {
            samples: vec![1000; 160],
            format,
        };
        sink.write_frame(&frame).await.unwrap();
        assert!(voice.lock().is_empty(), "dropped while no device is open");
        output
            .device_rate
            .store(48_000, std::sync::atomic::Ordering::Relaxed);
        for _ in 0..20 {
            sink.write_frame(&frame).await.unwrap();
        }
        let queued = voice.lock().len();
        assert!(
            queued > 0 && queued <= 48_000 * VOICE_QUEUE_MS / 1000,
            "{queued} samples queued"
        );
    }
}
