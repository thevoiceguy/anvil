//! H.264 through Apple's VideoToolbox, on macOS: the system's encoder and
//! decoder (in hardware where the Mac has it), constrained baseline with
//! no frame reordering, real time, keyframes when asked.
//!
//! VideoToolbox speaks AVCC (length-prefixed NAL units, the parameter sets
//! in a format description); the core speaks Annex B (start codes, the
//! parameter sets in the stream before each IDR). Both ways are converted
//! here.

use std::collections::VecDeque;
use std::ffi::c_void;
use std::ptr::{self, NonNull};

use objc2_core_foundation::{
    kCFAllocatorNull, CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType,
};
use objc2_core_media::{
    kCMTimeInvalid, kCMVideoCodecType_H264, CMBlockBuffer, CMFormatDescription, CMSampleBuffer,
    CMTime, CMVideoFormatDescriptionCreateFromH264ParameterSets,
    CMVideoFormatDescriptionGetH264ParameterSetAtIndex,
};
use objc2_core_video::{
    kCVPixelBufferPixelFormatTypeKey, kCVPixelFormatType_420YpCbCr8Planar, kCVReturnSuccess,
    CVImageBuffer, CVPixelBuffer, CVPixelBufferCreate, CVPixelBufferGetBaseAddressOfPlane,
    CVPixelBufferGetBytesPerRowOfPlane, CVPixelBufferGetHeight, CVPixelBufferGetWidth,
    CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
};
use objc2_video_toolbox::{
    kVTCompressionPropertyKey_AllowFrameReordering, kVTCompressionPropertyKey_AverageBitRate,
    kVTCompressionPropertyKey_ExpectedFrameRate, kVTCompressionPropertyKey_MaxKeyFrameInterval,
    kVTCompressionPropertyKey_ProfileLevel, kVTCompressionPropertyKey_RealTime,
    kVTEncodeFrameOptionKey_ForceKeyFrame, kVTProfileLevel_H264_ConstrainedBaseline_AutoLevel,
    VTCompressionSession, VTDecodeFrameFlags, VTDecodeInfoFlags,
    VTDecompressionOutputCallbackRecord, VTDecompressionSession, VTEncodeInfoFlags,
    VTSessionSetProperty,
};
use parking_lot::Mutex;

use anvil_core::video::host::{VideoCodec, VideoCodecCaps};

use crate::codec::{Codecs, Decoder, Encoded, Encoder};
use crate::picture::Picture;

const START: [u8; 4] = [0, 0, 0, 1];

/// The Mac's codecs: H.264 both ways.
pub(crate) struct VtCodecs;

impl Codecs for VtCodecs {
    fn caps(&self) -> Vec<VideoCodecCaps> {
        vec![VideoCodecCaps {
            codec: VideoCodec::H264,
            encode: true,
            decode: true,
        }]
    }

    fn encoder(&self, codec: VideoCodec, kbps: u32, fps: u32) -> Result<Box<dyn Encoder>, String> {
        match codec {
            VideoCodec::H264 => Ok(Box::new(VtEncoder::new(kbps, fps))),
            other => Err(format!("no {other:?} encoder on macOS")),
        }
    }

    fn decoder(&self, codec: VideoCodec) -> Result<Box<dyn Decoder>, String> {
        match codec {
            VideoCodec::H264 => Ok(Box::new(VtDecoder::new())),
            other => Err(format!("no {other:?} decoder on macOS")),
        }
    }
}

/// A VideoToolbox session: used from one thread at a time (the source's or
/// the sink's), which is all VideoToolbox asks.
struct Session<T: 'static>(CFRetained<T>);

// SAFETY: VideoToolbox sessions are thread-safe objects; each is used by
// the one thread that owns its encoder or decoder.
unsafe impl<T> Send for Session<T> {}

fn status(what: &str, s: i32) -> Result<(), String> {
    if s == 0 {
        Ok(())
    } else {
        Err(format!("VideoToolbox {what}: OSStatus {s}"))
    }
}

fn number(v: i32) -> CFRetained<CFNumber> {
    CFNumber::new_i32(v)
}

fn set(session: &CFType, key: &CFString, value: &CFType) -> Result<(), String> {
    // SAFETY: a session, a key it knows and a value of the key's type.
    status("property", unsafe {
        VTSessionSetProperty(session, key, Some(value))
    })
}

/// What the encoder's callback hands back: each frame's NAL units in
/// Annex B, and whether it is a keyframe.
type Out = Mutex<VecDeque<Result<Encoded, String>>>;

pub(crate) struct VtEncoder {
    session: Option<Session<VTCompressionSession>>,
    size: (u32, u32),
    kbps: u32,
    fps: u32,
    /// The next frame's time, at 90 kHz.
    pts: i64,
    /// Boxed: its address is the callback's refcon.
    out: Box<Out>,
}

impl VtEncoder {
    pub(crate) fn new(kbps: u32, fps: u32) -> Self {
        Self {
            session: None,
            size: (0, 0),
            kbps: kbps.max(30),
            fps: fps.max(1),
            pts: 0,
            out: Box::new(Mutex::new(VecDeque::new())),
        }
    }

    fn make(&self, width: u32, height: u32) -> Result<Session<VTCompressionSession>, String> {
        let mut raw: *mut VTCompressionSession = ptr::null_mut();
        // SAFETY: the callback is `encoded` and its refcon the boxed `out`,
        // which outlives the session (invalidated in drop).
        let s = unsafe {
            VTCompressionSession::create(
                None,
                width as i32,
                height as i32,
                kCMVideoCodecType_H264,
                None,
                None,
                None,
                Some(encoded),
                &*self.out as *const Out as *mut c_void,
                NonNull::from(&mut raw),
            )
        };
        status("compression session", s)?;
        let session = NonNull::new(raw).ok_or("VideoToolbox gave no compression session")?;
        // SAFETY: created above, owned by us.
        let session = Session(unsafe { CFRetained::from_raw(session) });
        let ty: &CFType = &session.0;
        // SAFETY: the keys' documented value types.
        unsafe {
            set(ty, kVTCompressionPropertyKey_RealTime, CFBoolean::new(true))?;
            set(
                ty,
                kVTCompressionPropertyKey_ProfileLevel,
                kVTProfileLevel_H264_ConstrainedBaseline_AutoLevel,
            )?;
            set(
                ty,
                kVTCompressionPropertyKey_AllowFrameReordering,
                CFBoolean::new(false),
            )?;
            // Keyframes when the far end asks; one a minute regardless.
            set(
                ty,
                kVTCompressionPropertyKey_MaxKeyFrameInterval,
                &number(60 * self.fps as i32),
            )?;
        }
        Self::rate(ty, self.kbps, self.fps)?;
        Ok(session)
    }

    fn rate(session: &CFType, kbps: u32, fps: u32) -> Result<(), String> {
        // SAFETY: the keys' documented value types.
        unsafe {
            set(
                session,
                kVTCompressionPropertyKey_AverageBitRate,
                &number((kbps * 1000) as i32),
            )?;
            set(
                session,
                kVTCompressionPropertyKey_ExpectedFrameRate,
                &number(fps as i32),
            )
        }
    }
}

impl Drop for VtEncoder {
    fn drop(&mut self) {
        if let Some(s) = self.session.take() {
            // SAFETY: ours; no callback runs after it is invalidated.
            unsafe { s.0.invalidate() };
        }
    }
}

/// A picture as a pixel buffer VideoToolbox takes: planar 4:2:0.
fn pixel_buffer(picture: &Picture) -> Result<CFRetained<CVPixelBuffer>, String> {
    let mut raw: *mut CVPixelBuffer = ptr::null_mut();
    // SAFETY: a size and format, and somewhere to put the buffer.
    let r = unsafe {
        CVPixelBufferCreate(
            None,
            picture.width as usize,
            picture.height as usize,
            kCVPixelFormatType_420YpCbCr8Planar,
            None,
            NonNull::from(&mut raw),
        )
    };
    if r != kCVReturnSuccess {
        return Err(format!("CVPixelBufferCreate: {r}"));
    }
    let buffer = NonNull::new(raw).ok_or("no pixel buffer")?;
    // SAFETY: created above, owned by us.
    let buffer = unsafe { CFRetained::from_raw(buffer) };
    let (w, h) = (picture.width as usize, picture.height as usize);
    let (cw, ch) = (picture.chroma_width(), h.div_ceil(2));
    // SAFETY: locked while its planes are written, each row within the
    // plane's bytes per row.
    unsafe {
        CVPixelBufferLockBaseAddress(&buffer, CVPixelBufferLockFlags(0));
        for (plane, (src, width, rows)) in [
            (picture.y(), w, h),
            (picture.u(), cw, ch),
            (picture.v(), cw, ch),
        ]
        .into_iter()
        .enumerate()
        {
            let base = CVPixelBufferGetBaseAddressOfPlane(&buffer, plane).cast::<u8>();
            let stride = CVPixelBufferGetBytesPerRowOfPlane(&buffer, plane);
            if base.is_null() {
                CVPixelBufferUnlockBaseAddress(&buffer, CVPixelBufferLockFlags(0));
                return Err("a pixel buffer without planes".into());
            }
            for r in 0..rows {
                ptr::copy_nonoverlapping(src[r * width..].as_ptr(), base.add(r * stride), width);
            }
        }
        CVPixelBufferUnlockBaseAddress(&buffer, CVPixelBufferLockFlags(0));
    }
    Ok(buffer)
}

impl Encoder for VtEncoder {
    fn encode(&mut self, picture: &Picture, keyframe: bool) -> Result<Option<Encoded>, String> {
        let size = (picture.width, picture.height);
        if self.session.is_none() || self.size != size {
            if let Some(old) = self.session.take() {
                // SAFETY: ours, done with.
                unsafe { old.0.invalidate() };
            }
            // A new size is a new stream, from an IDR.
            self.session = Some(self.make(size.0, size.1)?);
            self.size = size;
        }
        let Some(session) = &self.session else {
            return Ok(None);
        };
        let buffer = pixel_buffer(picture)?;
        let force = CFDictionary::<CFString, CFType>::from_slices(
            // SAFETY: a static key.
            &[unsafe { kVTEncodeFrameOptionKey_ForceKeyFrame }],
            &[CFBoolean::new(true)],
        );
        let image: &CVImageBuffer = &buffer;
        let pts = self.pts;
        self.pts += 90_000 / i64::from(self.fps);
        // SAFETY: the session, a pixel buffer of its size, and the frame's
        // time; the output comes to `encoded` before `complete_frames`
        // returns.
        unsafe {
            status(
                "encode",
                session.0.encode_frame(
                    image,
                    CMTime::new(pts, 90_000),
                    kCMTimeInvalid,
                    keyframe.then_some(force.as_opaque()),
                    ptr::null_mut(),
                    ptr::null_mut(),
                ),
            )?;
            status("complete", session.0.complete_frames(kCMTimeInvalid))?;
        }
        match self.out.lock().pop_front() {
            Some(Ok(e)) => Ok(Some(e)),
            Some(Err(e)) => Err(e),
            // Dropped by rate control.
            None => Ok(None),
        }
    }

    fn set_rate(&mut self, kbps: u32, fps: u32) {
        let (kbps, fps) = (kbps.max(30), fps.max(1));
        if (kbps, fps) == (self.kbps, self.fps) {
            return;
        }
        self.kbps = kbps;
        self.fps = fps;
        if let Some(s) = &self.session {
            if let Err(e) = Self::rate(&s.0, kbps, fps) {
                tracing::debug!("VideoToolbox rate: {e}");
            }
        }
    }
}

/// VideoToolbox's output: a frame in AVCC, its parameter sets in its
/// format description.
unsafe extern "C-unwind" fn encoded(
    refcon: *mut c_void,
    _frame: *mut c_void,
    s: i32,
    _flags: VTEncodeInfoFlags,
    sample: *mut CMSampleBuffer,
) {
    // SAFETY: the refcon is the encoder's boxed `out`.
    let out = unsafe { &*(refcon as *const Out) };
    let result = if s != 0 {
        Err(format!("VideoToolbox encode: OSStatus {s}"))
    } else if sample.is_null() {
        // Dropped.
        return;
    } else {
        // SAFETY: a sample buffer VideoToolbox lends for this call.
        annex_b(unsafe { &*sample })
    };
    out.lock().push_back(result);
}

/// An encoded sample in Annex B, the parameter sets before an IDR.
fn annex_b(sample: &CMSampleBuffer) -> Result<Encoded, String> {
    // SAFETY: the sample's own data and description.
    let (data, format) = unsafe { (sample.data_buffer(), sample.format_description()) };
    let data = data.ok_or("a sample without data")?;
    let len = unsafe { data.data_length() };
    let mut avcc = vec![0u8; len];
    if len > 0 {
        status("copy", unsafe {
            data.copy_data_bytes(0, len, NonNull::from(&mut avcc[..]).cast())
        })?;
    }
    let mut nals = Vec::new();
    let mut header = 4usize;
    let mut sets = Vec::new();
    if let Some(format) = &format {
        let mut count = 0usize;
        let mut h: i32 = 4;
        // SAFETY: asks for the count and the length size.
        unsafe {
            CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
                format,
                0,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut count,
                &mut h,
            );
        }
        header = h.clamp(1, 4) as usize;
        for i in 0..count {
            let (mut p, mut n): (*const u8, usize) = (ptr::null(), 0);
            // SAFETY: the i-th set, lent for as long as the format lives.
            let s = unsafe {
                CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
                    format,
                    i,
                    &mut p,
                    &mut n,
                    ptr::null_mut(),
                    ptr::null_mut(),
                )
            };
            if s == 0 && !p.is_null() {
                sets.push(unsafe { std::slice::from_raw_parts(p, n) }.to_vec());
            }
        }
    }
    let mut at = 0;
    while at + header <= avcc.len() {
        let n = avcc[at..at + header]
            .iter()
            .fold(0usize, |n, b| (n << 8) | *b as usize);
        at += header;
        if n == 0 || at + n > avcc.len() {
            break;
        }
        nals.push(&avcc[at..at + n]);
        at += n;
    }
    let keyframe = nals
        .iter()
        .any(|n| n.first().is_some_and(|b| b & 0x1f == 5));
    let mut out = Vec::with_capacity(len + 64);
    if keyframe {
        for s in &sets {
            out.extend_from_slice(&START);
            out.extend_from_slice(s);
        }
    }
    for n in nals {
        out.extend_from_slice(&START);
        out.extend_from_slice(n);
    }
    Ok(Encoded {
        data: out,
        keyframe,
    })
}

/// The NAL units of an Annex B stream.
pub(crate) fn nal_units(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push((i, i + 3));
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut nals = Vec::with_capacity(starts.len());
    for (k, &(_, begin)) in starts.iter().enumerate() {
        let mut end = starts.get(k + 1).map_or(data.len(), |&(s, _)| s);
        // A four-byte start code's leading zero belongs to it.
        while end > begin && data[end - 1] == 0 {
            end -= 1;
        }
        if end > begin {
            nals.push(&data[begin..end]);
        }
    }
    nals
}

/// What the decoder's callback hands back.
type Decoded = Mutex<Option<Result<Picture, String>>>;

pub(crate) struct VtDecoder {
    sps: Vec<u8>,
    pps: Vec<u8>,
    format: Option<Session<CMFormatDescription>>,
    session: Option<Session<VTDecompressionSession>>,
    /// Boxed: its address is the callback's refcon.
    out: Box<Decoded>,
}

impl VtDecoder {
    pub(crate) fn new() -> Self {
        Self {
            sps: Vec::new(),
            pps: Vec::new(),
            format: None,
            session: None,
            out: Box::new(Mutex::new(None)),
        }
    }

    /// A session for the parameter sets there are now.
    fn make(&mut self) -> Result<(), String> {
        if let Some(old) = self.session.take() {
            // SAFETY: ours, done with.
            unsafe { old.0.invalidate() };
        }
        let sets = [self.sps.as_slice(), self.pps.as_slice()];
        let mut pointers = sets.map(|s| NonNull::from(&s[0]));
        let mut sizes = sets.map(<[u8]>::len);
        let mut raw: *const CMFormatDescription = ptr::null();
        // SAFETY: two parameter sets and their sizes, four-byte lengths.
        status("format", unsafe {
            CMVideoFormatDescriptionCreateFromH264ParameterSets(
                None,
                2,
                NonNull::from(&mut pointers[0]),
                NonNull::from(&mut sizes[0]),
                4,
                NonNull::from(&mut raw),
            )
        })?;
        let format = NonNull::new(raw.cast_mut()).ok_or("no format description")?;
        // SAFETY: created above, owned by us.
        let format = unsafe { CFRetained::from_raw(format) };
        // Planar 4:2:0 out, as `Picture` keeps it.
        let attrs = CFDictionary::<CFString, CFType>::from_slices(
            // SAFETY: a static key.
            &[unsafe { kCVPixelBufferPixelFormatTypeKey }],
            &[&number(kCVPixelFormatType_420YpCbCr8Planar as i32)],
        );
        let record = VTDecompressionOutputCallbackRecord {
            decompressionOutputCallback: Some(decoded),
            decompressionOutputRefCon: &*self.out as *const Decoded as *mut c_void,
        };
        let mut raw: *mut VTDecompressionSession = ptr::null_mut();
        // SAFETY: the format, the attributes, and the callback with the
        // boxed `out`, which outlives the session (invalidated in drop).
        status("decompression session", unsafe {
            VTDecompressionSession::create(
                None,
                &format,
                None,
                Some(attrs.as_opaque()),
                &record,
                NonNull::from(&mut raw),
            )
        })?;
        let session = NonNull::new(raw).ok_or("VideoToolbox gave no decompression session")?;
        // SAFETY: created above, owned by us.
        self.session = Some(Session(unsafe { CFRetained::from_raw(session) }));
        self.format = Some(Session(format));
        Ok(())
    }
}

impl Drop for VtDecoder {
    fn drop(&mut self) {
        if let Some(s) = self.session.take() {
            // SAFETY: ours; no callback runs after it is invalidated.
            unsafe { s.0.invalidate() };
        }
    }
}

impl Decoder for VtDecoder {
    fn decode(&mut self, data: &[u8]) -> Result<Option<Picture>, String> {
        let mut avcc = Vec::with_capacity(data.len() + 16);
        let mut sets_changed = false;
        for nal in nal_units(data) {
            match nal[0] & 0x1f {
                7 if nal != self.sps.as_slice() => {
                    self.sps = nal.to_vec();
                    sets_changed = true;
                }
                8 if nal != self.pps.as_slice() => {
                    self.pps = nal.to_vec();
                    sets_changed = true;
                }
                7 | 8 => {}
                _ => {
                    avcc.extend_from_slice(&(nal.len() as u32).to_be_bytes());
                    avcc.extend_from_slice(nal);
                }
            }
        }
        if sets_changed && !self.sps.is_empty() && !self.pps.is_empty() {
            self.make()?;
        }
        let (Some(session), Some(format)) = (&self.session, &self.format) else {
            // Nothing can be decoded before the parameter sets.
            return Ok(None);
        };
        if avcc.is_empty() {
            return Ok(None);
        }
        let mut block: *mut CMBlockBuffer = ptr::null_mut();
        // SAFETY: wraps `avcc` without copying or freeing it; the block and
        // the sample are gone before `avcc` is (decoding is synchronous).
        status("block", unsafe {
            CMBlockBuffer::create_with_memory_block(
                None,
                avcc.as_mut_ptr().cast(),
                avcc.len(),
                kCFAllocatorNull,
                ptr::null(),
                0,
                avcc.len(),
                0,
                NonNull::from(&mut block),
            )
        })?;
        let block = unsafe { CFRetained::from_raw(NonNull::new(block).ok_or("no block")?) };
        let mut sample: *mut CMSampleBuffer = ptr::null_mut();
        let size = avcc.len();
        // SAFETY: one sample of the block's size, no timing.
        status("sample", unsafe {
            CMSampleBuffer::create_ready(
                None,
                Some(&block),
                Some(&format.0),
                1,
                0,
                ptr::null(),
                1,
                &size,
                NonNull::from(&mut sample),
            )
        })?;
        let sample = unsafe { CFRetained::from_raw(NonNull::new(sample).ok_or("no sample")?) };
        let mut info = VTDecodeInfoFlags(0);
        // SAFETY: synchronous (no asynchronous flag): the picture is in
        // `out` when this returns.
        status("decode", unsafe {
            session
                .0
                .decode_frame(&sample, VTDecodeFrameFlags(0), ptr::null_mut(), &mut info)
        })?;
        match self.out.lock().take() {
            Some(Ok(p)) => Ok(Some(p)),
            Some(Err(e)) => Err(e),
            None => Ok(None),
        }
    }
}

unsafe extern "C-unwind" fn decoded(
    refcon: *mut c_void,
    _frame: *mut c_void,
    s: i32,
    _flags: VTDecodeInfoFlags,
    image: *mut CVImageBuffer,
    _pts: CMTime,
    _duration: CMTime,
) {
    // SAFETY: the refcon is the decoder's boxed `out`.
    let out = unsafe { &*(refcon as *const Decoded) };
    let result = if s != 0 {
        Err(format!("VideoToolbox decode: OSStatus {s}"))
    } else if image.is_null() {
        return;
    } else {
        // SAFETY: a pixel buffer VideoToolbox lends for this call.
        Ok(picture(unsafe { &*image }))
    };
    *out.lock() = Some(result);
}

/// A planar 4:2:0 pixel buffer as a `Picture`.
fn picture(buffer: &CVPixelBuffer) -> Picture {
    let (w, h) = (
        CVPixelBufferGetWidth(buffer),
        CVPixelBufferGetHeight(buffer),
    );
    let ch = h.div_ceil(2);
    // SAFETY: read while locked, each plane its rows times its bytes per
    // row long.
    unsafe {
        CVPixelBufferLockBaseAddress(buffer, CVPixelBufferLockFlags::ReadOnly);
        let plane = |i: usize, rows: usize| {
            let base = CVPixelBufferGetBaseAddressOfPlane(buffer, i).cast::<u8>();
            let stride = CVPixelBufferGetBytesPerRowOfPlane(buffer, i);
            (std::slice::from_raw_parts(base, stride * rows), stride)
        };
        let picture =
            Picture::from_planes(w as u32, h as u32, plane(0, h), plane(1, ch), plane(2, ch));
        CVPixelBufferUnlockBaseAddress(buffer, CVPixelBufferLockFlags::ReadOnly);
        picture
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annex_b_splits_at_three_and_four_byte_start_codes() {
        let stream = [
            0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65, 4, 0,
        ];
        assert_eq!(
            nal_units(&stream),
            [&[0x67, 1, 2][..], &[0x68, 3][..], &[0x65, 4][..]]
        );
        assert!(nal_units(&[1, 2, 3]).is_empty());
    }
}
