//! Android: H.264 and VP8 through MediaCodec, the phone's own codecs (in
//! hardware where it has them; Android's software ones where not), so
//! nothing is fetched and the platform's licence covers H.264.
//!
//! Both run one picture or frame at a time, as the core feeds them. The
//! encoder takes 4:2:0 from buffers (NV12 or planar, whichever it
//! accepts), in real time, constant bit rate, baseline H.264, keyframes
//! when asked; each keyframe carries its parameter sets, which MediaCodec
//! gives once apart. The decoder draws into an [`aimage::Reader`], whose
//! images say their own layout, so no vendor's buffer layout is guessed.
//!
//! Keys are written as strings, not the NDK's constants, so the library
//! loads on Android 8 (the app's minimum) though some keys are newer:
//! a codec ignores a key it does not know.

use std::collections::VecDeque;
use std::ffi::CStr;
use std::ptr::{self, NonNull};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use anvil_core::video::host::{VideoCodec, VideoCodecCaps};
use ndk_sys::{media_status_t, AMediaCodec, AMediaCodecBufferInfo, AMediaFormat};

use crate::aimage::{ok, Reader};
use crate::codec::{Codecs, Decoder, Encoded, Encoder};
use crate::picture::Picture;

const AVC: &CStr = c"video/avc";
const VP8: &CStr = c"video/x-vnd.on2.vp8";

/// `MediaCodecInfo.CodecCapabilities`' 4:2:0 layouts.
const COLOR_PLANAR: i32 = 19;
const COLOR_SEMI_PLANAR: i32 = 21;
/// `MediaCodecInfo.CodecProfileLevel.AVCProfileBaseline`.
const AVC_BASELINE: i32 = 1;
/// `MediaCodecInfo.EncoderCapabilities.BITRATE_MODE_CBR`.
const CBR: i32 = 2;
const BUFFER_FLAG_KEY_FRAME: u32 = 1;
const BUFFER_FLAG_CODEC_CONFIG: u32 = 2;
const INFO_TRY_AGAIN_LATER: isize = -1;
const INFO_OUTPUT_FORMAT_CHANGED: isize = -2;
const INFO_OUTPUT_BUFFERS_CHANGED: isize = -3;
const CONFIGURE_ENCODE: u32 = 1;

fn mime(codec: VideoCodec) -> &'static CStr {
    match codec {
        VideoCodec::H264 => AVC,
        VideoCodec::Vp8 => VP8,
    }
}

/// The phone's codecs.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct McCodecs;

impl Codecs for McCodecs {
    fn caps(&self) -> Vec<VideoCodecCaps> {
        // Asked once: making a codec to see takes a moment.
        static CAPS: OnceLock<Vec<VideoCodecCaps>> = OnceLock::new();
        CAPS.get_or_init(|| {
            [VideoCodec::H264, VideoCodec::Vp8]
                .into_iter()
                .filter_map(|codec| {
                    let encode = Codec::encoder(mime(codec)).is_ok();
                    let decode = Codec::decoder(mime(codec)).is_ok();
                    (encode || decode).then_some(VideoCodecCaps {
                        codec,
                        encode,
                        decode,
                    })
                })
                .collect()
        })
        .clone()
    }

    fn encoder(&self, codec: VideoCodec, kbps: u32, fps: u32) -> Result<Box<dyn Encoder>, String> {
        Codec::encoder(mime(codec))?;
        Ok(Box::new(McEncoder::new(codec, kbps, fps)))
    }

    fn decoder(&self, codec: VideoCodec) -> Result<Box<dyn Decoder>, String> {
        Ok(Box::new(McDecoder::new(codec)?))
    }
}

/// A media format, ours to delete.
struct Format(NonNull<AMediaFormat>);

impl Format {
    fn new() -> Self {
        // SAFETY: plain constructor; it never fails.
        Self(NonNull::new(unsafe { ndk_sys::AMediaFormat_new() }).expect("a media format"))
    }

    fn set(&self, key: &CStr, value: i32) -> &Self {
        // SAFETY: both pointers live for the call; the key is copied.
        unsafe { ndk_sys::AMediaFormat_setInt32(self.0.as_ptr(), key.as_ptr(), value) };
        self
    }

    fn set_str(&self, key: &CStr, value: &CStr) -> &Self {
        // SAFETY: as in `set`; the value is copied.
        unsafe { ndk_sys::AMediaFormat_setString(self.0.as_ptr(), key.as_ptr(), value.as_ptr()) };
        self
    }

    fn get(&self, key: &CStr) -> Option<i32> {
        let mut v = 0;
        // SAFETY: plain out-parameter call.
        unsafe { ndk_sys::AMediaFormat_getInt32(self.0.as_ptr(), key.as_ptr(), &mut v) }
            .then_some(v)
    }
}

impl Drop for Format {
    fn drop(&mut self) {
        // SAFETY: ours.
        unsafe { ndk_sys::AMediaFormat_delete(self.0.as_ptr()) };
    }
}

/// A codec, stopped and deleted when dropped.
struct Codec {
    codec: NonNull<AMediaCodec>,
    started: bool,
}

// SAFETY: an AMediaCodec may be used from any thread, one at a time (the
// encoder or decoder that owns it is used by one thread).
unsafe impl Send for Codec {}

impl Codec {
    fn make(raw: *mut AMediaCodec, what: &CStr) -> Result<Self, String> {
        NonNull::new(raw)
            .map(|codec| Self {
                codec,
                started: false,
            })
            .ok_or_else(|| format!("this phone has no {}", what.to_string_lossy()))
    }

    fn encoder(mime: &CStr) -> Result<Self, String> {
        // SAFETY: plain constructor.
        Self::make(
            unsafe { ndk_sys::AMediaCodec_createEncoderByType(mime.as_ptr()) },
            mime,
        )
    }

    fn decoder(mime: &CStr) -> Result<Self, String> {
        // SAFETY: plain constructor.
        Self::make(
            unsafe { ndk_sys::AMediaCodec_createDecoderByType(mime.as_ptr()) },
            mime,
        )
    }

    fn ptr(&self) -> *mut AMediaCodec {
        self.codec.as_ptr()
    }

    fn configure(
        &self,
        format: &Format,
        window: *mut ndk_sys::ANativeWindow,
        flags: u32,
    ) -> Result<(), String> {
        // SAFETY: the format and window outlive the call; no crypto.
        ok("configure", unsafe {
            ndk_sys::AMediaCodec_configure(
                self.ptr(),
                format.0.as_ptr(),
                window,
                ptr::null_mut(),
                flags,
            )
        })
    }

    fn start(&mut self) -> Result<(), String> {
        // SAFETY: configured.
        ok("start", unsafe { ndk_sys::AMediaCodec_start(self.ptr()) })?;
        self.started = true;
        Ok(())
    }

    /// `data` as the next input at `pts_us`, waiting up to `wait` for room.
    fn queue(
        &self,
        wait: Duration,
        pts_us: u64,
        fill: impl FnOnce(&mut [u8]) -> usize,
    ) -> Result<(), String> {
        // SAFETY: plain calls on a started codec; the buffer is the codec's
        // until queued, and written within its size.
        unsafe {
            let index =
                ndk_sys::AMediaCodec_dequeueInputBuffer(self.ptr(), wait.as_micros() as i64);
            if index < 0 {
                return Err("the codec has no room".into());
            }
            let index = index as usize;
            let mut size = 0;
            let buffer = ndk_sys::AMediaCodec_getInputBuffer(self.ptr(), index, &mut size);
            if buffer.is_null() {
                return Err("the codec gave no input buffer".into());
            }
            let n = fill(std::slice::from_raw_parts_mut(buffer, size));
            ok(
                "queue",
                ndk_sys::AMediaCodec_queueInputBuffer(self.ptr(), index, 0, n, pts_us, 0),
            )
        }
    }

    /// The next output, waiting up to `wait`.
    fn dequeue(&self, wait: Duration) -> Output {
        // SAFETY: a zeroed info is filled by the call.
        let mut info: AMediaCodecBufferInfo = unsafe { std::mem::zeroed() };
        // SAFETY: plain out-parameter call on a started codec.
        let index = unsafe {
            ndk_sys::AMediaCodec_dequeueOutputBuffer(self.ptr(), &mut info, wait.as_micros() as i64)
        };
        match index {
            INFO_TRY_AGAIN_LATER | INFO_OUTPUT_BUFFERS_CHANGED => Output::None,
            INFO_OUTPUT_FORMAT_CHANGED => Output::Format,
            i if i < 0 => Output::Failed(format!("the codec failed ({i})")),
            i => Output::Buffer(i as usize, info),
        }
    }

    /// An output buffer's bytes.
    fn output(&self, index: usize, info: &AMediaCodecBufferInfo) -> &[u8] {
        let mut size = 0;
        // SAFETY: the buffer is the codec's until released, after the
        // caller is done with the slice (it borrows `self`).
        unsafe {
            let data = ndk_sys::AMediaCodec_getOutputBuffer(self.ptr(), index, &mut size);
            if data.is_null() {
                return &[];
            }
            let all = std::slice::from_raw_parts(data, size);
            let start = (info.offset.max(0) as usize).min(size);
            &all[start..(start + info.size.max(0) as usize).min(size)]
        }
    }

    fn release(&self, index: usize, render: bool) {
        // SAFETY: dequeued, released once.
        unsafe { ndk_sys::AMediaCodec_releaseOutputBuffer(self.ptr(), index, render) };
    }

    fn output_format(&self) -> Format {
        // SAFETY: plain call; the format is ours.
        Format(
            NonNull::new(unsafe { ndk_sys::AMediaCodec_getOutputFormat(self.ptr()) })
                .expect("a format"),
        )
    }

    fn set_params(&self, params: &Format) -> media_status_t {
        // SAFETY: plain call; the format is copied.
        unsafe { ndk_sys::AMediaCodec_setParameters(self.ptr(), params.0.as_ptr()) }
    }
}

impl Drop for Codec {
    fn drop(&mut self) {
        // SAFETY: ours, stopped before it is deleted.
        unsafe {
            if self.started {
                ndk_sys::AMediaCodec_stop(self.ptr());
            }
            ndk_sys::AMediaCodec_delete(self.ptr());
        }
    }
}

enum Output {
    None,
    Format,
    Buffer(usize, AMediaCodecBufferInfo),
    Failed(String),
}

/// One picture's wait for its frame: an encoder gives it within a frame
/// or two; a frame later than this is taken at the next picture.
const WAIT: Duration = Duration::from_millis(100);

pub(crate) struct McEncoder {
    codec_kind: VideoCodec,
    codec: Option<(Codec, i32)>,
    size: (u32, u32),
    kbps: u32,
    fps: u32,
    /// The pictures sent so far (their times are made from it).
    n: u64,
    /// H.264's parameter sets, put before each keyframe.
    config: Vec<u8>,
    /// Frames coded but not yet handed out.
    out: VecDeque<Encoded>,
    input: Vec<u8>,
}

impl McEncoder {
    fn new(codec: VideoCodec, kbps: u32, fps: u32) -> Self {
        Self {
            codec_kind: codec,
            codec: None,
            size: (0, 0),
            kbps: kbps.max(30),
            fps: fps.max(1),
            n: 0,
            config: Vec::new(),
            out: VecDeque::new(),
            input: Vec::new(),
        }
    }

    /// A codec for `width` × `height`, in the first layout it takes.
    fn make(&self, width: u32, height: u32) -> Result<(Codec, i32), String> {
        let mime = mime(self.codec_kind);
        let mut last = String::new();
        for color in [COLOR_SEMI_PLANAR, COLOR_PLANAR] {
            let mut codec = Codec::encoder(mime)?;
            let f = Format::new();
            f.set_str(c"mime", mime)
                .set(c"width", width as i32)
                .set(c"height", height as i32)
                .set(c"stride", width as i32)
                .set(c"slice-height", height as i32)
                .set(c"color-format", color)
                .set(c"bitrate", (self.kbps * 1000) as i32)
                .set(c"bitrate-mode", CBR)
                .set(c"frame-rate", self.fps as i32)
                // Keyframes when the far end asks; one a minute regardless.
                .set(c"i-frame-interval", 60)
                // Real time.
                .set(c"priority", 0)
                .set(c"latency", 1);
            if self.codec_kind == VideoCodec::H264 {
                f.set(c"profile", AVC_BASELINE);
            }
            match codec
                .configure(&f, ptr::null_mut(), CONFIGURE_ENCODE)
                .and_then(|()| codec.start())
            {
                Ok(()) => return Ok((codec, color)),
                Err(e) => last = e,
            }
        }
        Err(format!(
            "the encoder takes neither NV12 nor planar 4:2:0 ({last})"
        ))
    }

    /// What the codec has coded, into `out`; waits up to `wait` for the
    /// frame of the picture at `pts`.
    fn drain(&mut self, pts: u64, wait: Duration) -> Result<(), String> {
        let Some((codec, _)) = &self.codec else {
            return Ok(());
        };
        let until = Instant::now() + wait;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            match codec.dequeue(left) {
                Output::None if left.is_zero() => return Ok(()),
                Output::None | Output::Format => {}
                Output::Failed(e) => return Err(e),
                Output::Buffer(i, info) => {
                    let data = codec.output(i, &info).to_vec();
                    codec.release(i, false);
                    if info.flags & BUFFER_FLAG_CODEC_CONFIG != 0 {
                        self.config = data;
                        continue;
                    }
                    let keyframe = info.flags & BUFFER_FLAG_KEY_FRAME != 0;
                    let data = if keyframe && self.codec_kind == VideoCodec::H264 && !has_sps(&data)
                    {
                        [self.config.as_slice(), &data].concat()
                    } else {
                        data
                    };
                    self.out.push_back(Encoded { data, keyframe });
                    if info.presentationTimeUs as u64 >= pts {
                        return Ok(());
                    }
                }
            }
        }
    }

    fn pts(&self) -> u64 {
        self.n * 1_000_000 / u64::from(self.fps)
    }
}

/// Whether Annex B `data` carries a sequence parameter set.
fn has_sps(data: &[u8]) -> bool {
    data.windows(4)
        .any(|w| w[..3] == [0, 0, 1] && w[3] & 0x1F == 7)
}

impl Encoder for McEncoder {
    fn encode(&mut self, picture: &Picture, keyframe: bool) -> Result<Option<Encoded>, String> {
        let size = (picture.width, picture.height);
        let mut keyframe = keyframe;
        if self.codec.is_none() || size != self.size {
            // A new size is a new stream; its first frame is a keyframe.
            self.codec = None;
            self.out.clear();
            self.codec = Some(self.make(size.0, size.1)?);
            self.size = size;
            keyframe = false;
        }
        let (codec, color) = self.codec.as_ref().expect("made above");
        // Codec2 takes the request at the picture after this one: a
        // keyframe a frame late, which a far end that asked waits for.
        if keyframe {
            let p = Format::new();
            p.set(c"request-sync", 0);
            if let Err(e) = ok("keyframe", codec.set_params(&p)) {
                tracing::debug!(%e, "the encoder took no keyframe request");
            }
        }
        layout(picture, *color, &mut self.input);
        let pts = self.pts();
        let input = &self.input;
        codec.queue(WAIT, pts, |buf| {
            let n = input.len().min(buf.len());
            buf[..n].copy_from_slice(&input[..n]);
            n
        })?;
        self.n += 1;
        self.drain(pts, WAIT)?;
        Ok(self.out.pop_front())
    }

    fn set_rate(&mut self, kbps: u32, fps: u32) {
        self.kbps = kbps.max(30);
        if let Some((codec, _)) = &self.codec {
            let p = Format::new();
            p.set(c"video-bitrate", (self.kbps * 1000) as i32);
            if let Err(e) = ok("bit rate", codec.set_params(&p)) {
                tracing::debug!(%e, "the encoder kept its bit rate");
            }
        }
        // The frame rate is the times the pictures carry; it changes
        // when the codec is next made.
        self.fps = fps.max(1);
    }
}

/// `p` as an encoder takes it: NV12 (`COLOR_SEMI_PLANAR`) or I420.
fn layout(p: &Picture, color: i32, out: &mut Vec<u8>) {
    out.clear();
    out.extend_from_slice(p.y());
    if color == COLOR_PLANAR {
        out.extend_from_slice(p.u());
        out.extend_from_slice(p.v());
    } else {
        out.extend(p.u().iter().zip(p.v()).flat_map(|(&u, &v)| [u, v]));
    }
}

pub(crate) struct McDecoder {
    codec: Codec,
    /// Where it draws; made again when the stream's size changes.
    reader: Reader,
    size: (u32, u32),
    n: u64,
}

impl McDecoder {
    fn new(kind: VideoCodec) -> Result<Self, String> {
        let mut codec = Codec::decoder(mime(kind))?;
        let size = (640, 480);
        let reader = Reader::new(size.0, size.1, 4)?;
        let f = Format::new();
        f.set_str(c"mime", mime(kind))
            .set(c"width", size.0 as i32)
            .set(c"height", size.1 as i32)
            .set(c"low-latency", 1)
            .set(c"priority", 0);
        codec.configure(&f, reader.window()?, 0)?;
        codec.start()?;
        Ok(Self {
            codec,
            reader,
            size,
            n: 0,
        })
    }

    /// The stream's size changed: a reader of that size.
    fn resize(&mut self) -> Result<(), String> {
        let f = self.codec.output_format();
        let (Some(w), Some(h)) = (f.get(c"width"), f.get(c"height")) else {
            return Ok(());
        };
        let size = (w.max(2) as u32, h.max(2) as u32);
        if size == self.size {
            return Ok(());
        }
        let reader = Reader::new(size.0, size.1, 4)?;
        // SAFETY: the window is the new reader's, which the decoder now
        // draws into; the old reader goes after.
        ok("output surface", unsafe {
            ndk_sys::AMediaCodec_setOutputSurface(self.codec.ptr(), reader.window()?)
        })?;
        self.reader = reader;
        self.size = size;
        Ok(())
    }
}

impl Decoder for McDecoder {
    fn decode(&mut self, data: &[u8]) -> Result<Option<Picture>, String> {
        let pts = self.n * 33_333;
        self.n += 1;
        self.codec.queue(WAIT, pts, |buf| {
            let n = data.len().min(buf.len());
            buf[..n].copy_from_slice(&data[..n]);
            n
        })?;
        let until = Instant::now() + WAIT;
        let mut drawn = false;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            match self.codec.dequeue(left) {
                Output::None if left.is_zero() => break,
                Output::None => {}
                Output::Format => self.resize()?,
                Output::Failed(e) => return Err(e),
                Output::Buffer(i, info) => {
                    let show = info.size > 0;
                    self.codec.release(i, show);
                    drawn |= show;
                    if info.presentationTimeUs as u64 >= pts {
                        break;
                    }
                }
            }
        }
        if !drawn {
            return Ok(None);
        }
        Ok(self.reader.next(WAIT))
    }
}
