//! Audio host bridge for the C ABI.
//!
//! The C side describes the host as a struct of function pointers (open
//! capture / playback streams, push and pull frames, close streams).
//! `CHostAudioHost` adapts that struct to anvil-core's `AudioHost`
//! trait, so `anvil-core` doesn't have to know it's talking to a Swift
//! AVAudioEngine pipeline or an Oboe-driven Java thread.
//!
//! ## Threading model
//!
//! The C callbacks fire from the FFI tokio runtime's worker pool. They
//! must be safe to call from any thread. They may block briefly (a few
//! milliseconds — one audio frame's worth) but should not block on
//! anything that depends on Anvil to make progress, or the runtime
//! deadlocks.
//!
//! ## Frame ownership
//!
//! - **Capture (`pull_frame`):** Anvil supplies an `out` buffer sized
//!   for `samples_per_frame` `int16_t` elements. The host writes PCM
//!   into it and returns the number of samples written (0 = EOF; -1 =
//!   error). Anvil never frees the buffer; it lives on its stack.
//! - **Playback (`push_frame`):** Anvil supplies a borrowed `samples`
//!   pointer + length. The host copies into its own ring buffer before
//!   returning; the pointer is invalid afterwards.
//!
//! Both directions trade i16 PCM at the negotiated codec's sample rate
//! (8 kHz for PCMU/PCMA, 16 kHz for G.722, 48 kHz for Opus). The host
//! is responsible for any resampling between Anvil's request and its
//! own native rate.

use std::ffi::c_void;

use anvil_core::audio::{AudioFormat, AudioFrame, AudioHost, AudioSink, AudioSource, DeviceInfo};
use anvil_core::AnvilError;
use async_trait::async_trait;

/// PCM frame format passed to host open-stream calls. Same fields as
/// `anvil_core::audio::AudioFormat`; mirrored here so the C ABI doesn't
/// reach into `anvil-core`'s internals.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AnvilAudioFormat {
    pub sample_rate: u32,
    pub channels: u16,
    pub frame_ms: u32,
}

impl From<AudioFormat> for AnvilAudioFormat {
    fn from(f: AudioFormat) -> Self {
        Self {
            sample_rate: f.sample_rate,
            channels: f.channels,
            frame_ms: f.frame_ms,
        }
    }
}

/// C-side audio host. Pointer fields hold function pointers the host
/// supplies; `user_data` is opaque to Anvil and forwarded on every call.
///
/// All members must be non-NULL except `user_data`, which may be NULL
/// when the host doesn't need per-instance state. Streams returned by
/// `open_capture` / `open_playback` are also opaque — Anvil hands them
/// back unchanged to `pull_frame` / `push_frame` / `close_stream`.
#[repr(C)]
pub struct AnvilAudioHostC {
    pub user_data: *mut c_void,

    /// Open a capture stream. Return non-NULL on success, NULL on
    /// failure. The returned pointer is opaque to Anvil and lives until
    /// `close_stream` is called on it.
    pub open_capture: unsafe extern "C" fn(
        user_data: *mut c_void,
        fmt: *const AnvilAudioFormat,
    ) -> *mut c_void,

    /// Open a playback stream. Same semantics as `open_capture`.
    pub open_playback: unsafe extern "C" fn(
        user_data: *mut c_void,
        fmt: *const AnvilAudioFormat,
    ) -> *mut c_void,

    /// Pull one PCM frame from a capture stream into `out`. The buffer
    /// is sized for `out_capacity` `int16_t` elements (Anvil sizes it
    /// for the negotiated codec's `samples_per_frame`). Return:
    ///
    /// - the positive count of samples actually written on success,
    /// - `0` on end-of-stream (Anvil treats this as the call's mic
    ///   going silent permanently),
    /// - a negative value on transient error (Anvil logs and continues).
    pub pull_frame: unsafe extern "C" fn(
        user_data: *mut c_void,
        stream: *mut c_void,
        out: *mut i16,
        out_capacity: usize,
    ) -> i32,

    /// Push one PCM frame to a playback stream. `samples` points to
    /// `len` `int16_t` elements. The host must consume / copy before
    /// returning. Return 0 on success, negative on error.
    pub push_frame: unsafe extern "C" fn(
        user_data: *mut c_void,
        stream: *mut c_void,
        samples: *const i16,
        len: usize,
    ) -> i32,

    /// Close a capture or playback stream. Called from `Drop`. After
    /// this returns, the stream pointer must not be used again.
    pub close_stream: unsafe extern "C" fn(user_data: *mut c_void, stream: *mut c_void),
}

// `AnvilAudioHostC` carries raw pointers but the function pointers
// themselves are pure functions (Send + Sync) and `user_data` ownership
// is the host's. We promise to call only as documented.
unsafe impl Send for AnvilAudioHostC {}
unsafe impl Sync for AnvilAudioHostC {}

/// Marker type used in the FFI runtime when the C caller didn't supply
/// an audio host — signaling-only operation, every audio open call
/// errors out cleanly.
pub(crate) struct NullAudioHost;

impl AudioHost for NullAudioHost {
    fn make_capture(&self, _cfg: AudioFormat) -> Result<Box<dyn AudioSource>, AnvilError> {
        Err(AnvilError::AudioDevice(
            "anvil-ffi: no audio host supplied (set audio_host on AnvilConfig)".into(),
        ))
    }
    fn make_playback(&self, _cfg: AudioFormat) -> Result<Box<dyn AudioSink>, AnvilError> {
        Err(AnvilError::AudioDevice(
            "anvil-ffi: no audio host supplied (set audio_host on AnvilConfig)".into(),
        ))
    }
    fn devices(&self) -> Vec<DeviceInfo> {
        Vec::new()
    }
}

/// Wraps a C-supplied `AnvilAudioHostC` and presents it to anvil-core as
/// a regular `AudioHost`. Constructed by `anvil-ffi` when the caller
/// supplies a non-NULL `audio_host` on `AnvilConfig`.
pub(crate) struct CHostAudioHost {
    host: std::sync::Arc<AnvilAudioHostC>,
}

impl CHostAudioHost {
    /// Take ownership by copying the host struct out of the C-supplied
    /// pointer. The function pointers are stable for the lifetime of
    /// the program; `user_data` is the host's responsibility.
    ///
    /// # Safety
    /// `ptr` must point to a valid `AnvilAudioHostC` whose function
    /// pointers do what the doc-comment on `AnvilAudioHostC` says.
    pub(crate) unsafe fn from_c(ptr: *const AnvilAudioHostC) -> Self {
        // We can't move out of a raw pointer (the C side might rely on
        // its own copy persisting). Read by-value into a local Arc.
        let host = unsafe { std::ptr::read(ptr) };
        Self {
            host: std::sync::Arc::new(host),
        }
    }
}

impl AudioHost for CHostAudioHost {
    fn make_capture(&self, cfg: AudioFormat) -> Result<Box<dyn AudioSource>, AnvilError> {
        let fmt: AnvilAudioFormat = cfg.into();
        let stream = unsafe {
            (self.host.open_capture)(self.host.user_data, &fmt as *const _)
        };
        if stream.is_null() {
            return Err(AnvilError::AudioDevice("host open_capture returned NULL".into()));
        }
        Ok(Box::new(CCapture {
            host: std::sync::Arc::clone(&self.host),
            stream: StreamPtr(stream),
            samples_per_frame: samples_per_frame(cfg),
            fmt: cfg,
        }))
    }

    fn make_playback(&self, cfg: AudioFormat) -> Result<Box<dyn AudioSink>, AnvilError> {
        let fmt: AnvilAudioFormat = cfg.into();
        let stream = unsafe {
            (self.host.open_playback)(self.host.user_data, &fmt as *const _)
        };
        if stream.is_null() {
            return Err(AnvilError::AudioDevice("host open_playback returned NULL".into()));
        }
        Ok(Box::new(CPlayback {
            host: std::sync::Arc::clone(&self.host),
            stream: StreamPtr(stream),
        }))
    }

    fn devices(&self) -> Vec<DeviceInfo> {
        // Device enumeration over FFI lands when there's a use case.
        // Mobile hosts typically expose the selected device through
        // their own platform UI rather than a list.
        Vec::new()
    }
}

/// Newtype around a raw stream pointer so we can implement `Send + Sync`
/// at the type level rather than scattering `unsafe impl` over the
/// stream owners.
struct StreamPtr(*mut c_void);
unsafe impl Send for StreamPtr {}
unsafe impl Sync for StreamPtr {}

struct CCapture {
    host: std::sync::Arc<AnvilAudioHostC>,
    stream: StreamPtr,
    samples_per_frame: usize,
    fmt: AudioFormat,
}

impl Drop for CCapture {
    fn drop(&mut self) {
        unsafe { (self.host.close_stream)(self.host.user_data, self.stream.0) };
    }
}

#[async_trait]
impl AudioSource for CCapture {
    async fn next_frame(&mut self) -> Option<AudioFrame> {
        // Calls into C synchronously. The host is expected to have a
        // ready frame buffered; if it has to block briefly, that's OK.
        // For long blocks, switch to spawn_blocking — measurably worse
        // RT behaviour, but won't deadlock the runtime.
        let mut samples = vec![0i16; self.samples_per_frame];
        let n = unsafe {
            (self.host.pull_frame)(
                self.host.user_data,
                self.stream.0,
                samples.as_mut_ptr(),
                samples.len(),
            )
        };
        if n < 0 {
            tracing::warn!(n, "host pull_frame error; dropping frame");
            // Still return an empty frame so the encoder isn't starved
            // permanently; loud-and-keep-going beats silent EOF.
            return Some(AudioFrame {
                samples: vec![0i16; self.samples_per_frame],
                format: self.fmt,
            });
        }
        if n == 0 {
            return None; // EOF
        }
        samples.truncate(n as usize);
        Some(AudioFrame {
            samples,
            format: self.fmt,
        })
    }
}

struct CPlayback {
    host: std::sync::Arc<AnvilAudioHostC>,
    stream: StreamPtr,
}

impl Drop for CPlayback {
    fn drop(&mut self) {
        unsafe { (self.host.close_stream)(self.host.user_data, self.stream.0) };
    }
}

#[async_trait]
impl AudioSink for CPlayback {
    async fn write_frame(&mut self, frame: &AudioFrame) -> Result<(), AnvilError> {
        let n = unsafe {
            (self.host.push_frame)(
                self.host.user_data,
                self.stream.0,
                frame.samples.as_ptr(),
                frame.samples.len(),
            )
        };
        if n < 0 {
            return Err(AnvilError::AudioDevice(format!(
                "host push_frame returned {n}"
            )));
        }
        Ok(())
    }
}

fn samples_per_frame(fmt: AudioFormat) -> usize {
    (fmt.sample_rate as u64 * fmt.channels as u64 * fmt.frame_ms as u64 / 1000) as usize
}
