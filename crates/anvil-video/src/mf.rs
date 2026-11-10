//! H.264 through Media Foundation, on Windows: Microsoft's H.264 encoder
//! and decoder transforms, which every Windows with its media features has
//! (the N editions need the Media Feature Pack). Windows' licence covers
//! H.264, so nothing is fetched.
//!
//! The encoder is set for real time (low latency, CBR, baseline, a
//! keyframe when asked); both speak Annex B with the parameter sets in the
//! stream, as the core does. Each transform is made on the thread that
//! uses it (the source's or the sink's), in the multithreaded apartment.

use std::mem::ManuallyDrop;

use anvil_core::video::host::{VideoCodec, VideoCodecCaps};
use windows::core::{Interface, GUID};
use windows::Win32::Foundation::SIZE;
use windows::Win32::Media::MediaFoundation::{
    eAVEncCommonRateControlMode_CBR, eAVEncH264VProfile_Base, CLSID_MSH264DecoderMFT,
    CLSID_MSH264EncoderMFT, CODECAPI_AVEncCommonMeanBitRate, CODECAPI_AVEncCommonRateControlMode,
    CODECAPI_AVEncMPVGOPSize, CODECAPI_AVEncVideoForceKeyFrame, CODECAPI_AVLowLatencyMode,
    ICodecAPI, IMFMediaType, IMFSample, IMFTransform, MFCreateMediaType, MFCreateMemoryBuffer,
    MFCreateSample, MFMediaType_Video, MFStartup, MFVideoArea, MFVideoFormat_H264,
    MFVideoFormat_NV12, MFVideoInterlace_Progressive, MFSTARTUP_FULL, MFT_OUTPUT_DATA_BUFFER,
    MFT_OUTPUT_STREAM_PROVIDES_SAMPLES, MF_E_TRANSFORM_NEED_MORE_INPUT,
    MF_E_TRANSFORM_STREAM_CHANGE, MF_LOW_LATENCY, MF_MT_AVG_BITRATE, MF_MT_DEFAULT_STRIDE,
    MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE,
    MF_MT_MINIMUM_DISPLAY_APERTURE, MF_MT_MPEG2_PROFILE, MF_MT_SUBTYPE, MF_VERSION,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::System::Variant::{VARIANT, VT_BOOL, VT_UI4};

use crate::codec::{Codecs, Decoder, Encoded, Encoder};
use crate::picture::Picture;

/// Media Foundation's codecs: H.264 both ways.
pub(crate) struct MfCodecs;

impl Codecs for MfCodecs {
    fn caps(&self) -> Vec<VideoCodecCaps> {
        if !available() {
            return Vec::new();
        }
        vec![VideoCodecCaps {
            codec: VideoCodec::H264,
            encode: true,
            decode: true,
        }]
    }

    fn encoder(&self, codec: VideoCodec, kbps: u32, fps: u32) -> Result<Box<dyn Encoder>, String> {
        match codec {
            VideoCodec::H264 => Ok(Box::new(MfEncoder::new(kbps, fps))),
            other => Err(format!("no {other:?} encoder on Windows")),
        }
    }

    fn decoder(&self, codec: VideoCodec) -> Result<Box<dyn Decoder>, String> {
        match codec {
            VideoCodec::H264 => Ok(Box::new(MfDecoder::default())),
            other => Err(format!("no {other:?} decoder on Windows")),
        }
    }
}

fn err(what: &str) -> impl Fn(windows::core::Error) -> String + '_ {
    move |e| format!("Media Foundation {what}: {e}")
}

/// COM on this thread (the multithreaded apartment), and Media Foundation
/// in the process.
pub(crate) fn start() -> Result<(), String> {
    static STARTED: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    // SAFETY: joining the multithreaded apartment; a thread already in
    // one (or in another) keeps it.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    STARTED
        .get_or_init(|| {
            // SAFETY: once per process; never shut down (the codecs live
            // as long as the app).
            unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) }.map_err(err("startup"))
        })
        .clone()
}

/// Whether Media Foundation's H.264 transforms are here (an N edition
/// without the Media Feature Pack has none).
fn available() -> bool {
    static FOUND: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FOUND.get_or_init(|| {
        std::thread::spawn(|| {
            start().is_ok()
                && make(&CLSID_MSH264EncoderMFT).is_ok()
                && make(&CLSID_MSH264DecoderMFT).is_ok()
        })
        .join()
        .unwrap_or(false)
    })
}

fn make(clsid: &GUID) -> Result<IMFTransform, String> {
    start()?;
    // SAFETY: an in-process COM class, on a thread in the MTA.
    unsafe { CoCreateInstance(clsid, None, CLSCTX_INPROC_SERVER) }.map_err(err("transform"))
}

fn variant_u32(v: u32) -> VARIANT {
    let mut x = VARIANT::default();
    // SAFETY: a VT_UI4 variant, its value in ulVal.
    unsafe {
        let inner = &mut *x.Anonymous.Anonymous;
        inner.vt = VT_UI4;
        inner.Anonymous.ulVal = v;
    }
    x
}

fn variant_bool(v: bool) -> VARIANT {
    let mut x = VARIANT::default();
    // SAFETY: a VT_BOOL variant, its value in boolVal.
    unsafe {
        let inner = &mut *x.Anonymous.Anonymous;
        inner.vt = VT_BOOL;
        inner.Anonymous.boolVal = v.into();
    }
    x
}

fn pack(hi: u32, lo: u32) -> u64 {
    (u64::from(hi) << 32) | u64::from(lo)
}

fn video_type(subtype: &GUID, width: u32, height: u32, fps: u32) -> Result<IMFMediaType, String> {
    // SAFETY: attributes of a new media type.
    unsafe {
        let t = MFCreateMediaType().map_err(err("media type"))?;
        t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(err("type"))?;
        t.SetGUID(&MF_MT_SUBTYPE, subtype).map_err(err("type"))?;
        if width > 0 {
            t.SetUINT64(&MF_MT_FRAME_SIZE, pack(width, height))
                .map_err(err("type"))?;
            t.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1))
                .map_err(err("type"))?;
            t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
                .map_err(err("type"))?;
        }
        Ok(t)
    }
}

/// A sample holding `data`, at `time` (100 ns units).
fn sample(data: &[u8], time: i64, duration: i64) -> Result<IMFSample, String> {
    // SAFETY: a new buffer, written while locked within its length.
    unsafe {
        let buffer = MFCreateMemoryBuffer(data.len() as u32).map_err(err("buffer"))?;
        let mut at = std::ptr::null_mut();
        buffer.Lock(&mut at, None, None).map_err(err("lock"))?;
        std::ptr::copy_nonoverlapping(data.as_ptr(), at, data.len());
        buffer.Unlock().map_err(err("unlock"))?;
        buffer
            .SetCurrentLength(data.len() as u32)
            .map_err(err("length"))?;
        let s = MFCreateSample().map_err(err("sample"))?;
        s.AddBuffer(&buffer).map_err(err("sample"))?;
        s.SetSampleTime(time).map_err(err("time"))?;
        s.SetSampleDuration(duration).map_err(err("time"))?;
        Ok(s)
    }
}

/// A sample's bytes.
fn bytes(s: &IMFSample) -> Result<Vec<u8>, String> {
    // SAFETY: the sample's contiguous buffer, read while locked.
    unsafe {
        let buffer = s.ConvertToContiguousBuffer().map_err(err("buffer"))?;
        let mut at = std::ptr::null_mut();
        let mut len = 0u32;
        buffer
            .Lock(&mut at, None, Some(&mut len))
            .map_err(err("lock"))?;
        let out = std::slice::from_raw_parts(at, len as usize).to_vec();
        buffer.Unlock().map_err(err("unlock"))?;
        Ok(out)
    }
}

/// What a transform has to give: each output sample until it wants more
/// input. A transform that does not make its own samples is given one of
/// the size it asks for.
enum Output {
    Sample(IMFSample),
    NeedsInput,
    StreamChanged,
}

fn pull(t: &IMFTransform) -> Result<Output, String> {
    // SAFETY: the transform's own stream info, and an output buffer of the
    // size it asks for when it does not provide one.
    unsafe {
        let info = t.GetOutputStreamInfo(0).map_err(err("stream info"))?;
        let provides = info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 != 0;
        let given = if provides {
            None
        } else {
            let buffer = MFCreateMemoryBuffer(info.cbSize.max(1 << 20)).map_err(err("buffer"))?;
            let s = MFCreateSample().map_err(err("sample"))?;
            s.AddBuffer(&buffer).map_err(err("sample"))?;
            Some(s)
        };
        let mut out = [MFT_OUTPUT_DATA_BUFFER {
            dwStreamID: 0,
            pSample: ManuallyDrop::new(given),
            dwStatus: 0,
            pEvents: ManuallyDrop::new(None),
        }];
        let mut status = 0u32;
        let r = t.ProcessOutput(0, &mut out, &mut status);
        let sample = ManuallyDrop::take(&mut out[0].pSample);
        drop(ManuallyDrop::take(&mut out[0].pEvents));
        match r {
            Ok(()) => sample
                .map(Output::Sample)
                .ok_or_else(|| "Media Foundation gave no sample".to_string()),
            Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => Ok(Output::NeedsInput),
            Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => Ok(Output::StreamChanged),
            Err(e) => Err(err("output")(e)),
        }
    }
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
        while end > begin && data[end - 1] == 0 {
            end -= 1;
        }
        if end > begin {
            nals.push(&data[begin..end]);
        }
    }
    nals
}

/// A COM object used from the one thread that owns its codec.
struct Owned<T>(T);

// SAFETY: made in the multithreaded apartment, whose objects may be
// called from any thread in it; each is used by one thread at a time.
unsafe impl<T> Send for Owned<T> {}

pub(crate) struct MfEncoder {
    transform: Option<Owned<IMFTransform>>,
    size: (u32, u32),
    kbps: u32,
    fps: u32,
    time: i64,
    /// NV12 for the encoder, kept between pictures.
    nv12: Vec<u8>,
}

impl MfEncoder {
    pub(crate) fn new(kbps: u32, fps: u32) -> Self {
        Self {
            transform: None,
            size: (0, 0),
            kbps: kbps.max(30),
            fps: fps.max(1),
            time: 0,
            nv12: Vec::new(),
        }
    }

    fn make(&self, width: u32, height: u32) -> Result<IMFTransform, String> {
        let t = make(&CLSID_MSH264EncoderMFT)?;
        // SAFETY: the encoder's documented set-up: its codec API first (low
        // latency is taken only before the media types), then the output
        // type, then the input type.
        unsafe {
            if let Ok(attributes) = t.GetAttributes() {
                let _ = attributes.SetUINT32(&MF_LOW_LATENCY, 1);
            }
            if let Ok(api) = t.cast::<ICodecAPI>() {
                let _ = api.SetValue(&CODECAPI_AVLowLatencyMode, &variant_bool(true));
                let _ = api.SetValue(
                    &CODECAPI_AVEncCommonRateControlMode,
                    &variant_u32(eAVEncCommonRateControlMode_CBR.0 as u32),
                );
                let _ = api.SetValue(
                    &CODECAPI_AVEncCommonMeanBitRate,
                    &variant_u32(self.kbps * 1000),
                );
                // Keyframes when the far end asks; one a minute regardless.
                let _ = api.SetValue(&CODECAPI_AVEncMPVGOPSize, &variant_u32(60 * self.fps));
            }
            let out = video_type(&MFVideoFormat_H264, width, height, self.fps)?;
            out.SetUINT32(&MF_MT_AVG_BITRATE, self.kbps * 1000)
                .map_err(err("type"))?;
            out.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Base.0 as u32)
                .map_err(err("type"))?;
            t.SetOutputType(0, &out, 0).map_err(err("output type"))?;
            let input = video_type(&MFVideoFormat_NV12, width, height, self.fps)?;
            t.SetInputType(0, &input, 0).map_err(err("input type"))?;
        }
        Ok(t)
    }

    fn rate(t: &IMFTransform, kbps: u32) {
        if let Ok(api) = t.cast::<ICodecAPI>() {
            // SAFETY: a documented property and its type.
            let _ = unsafe {
                api.SetValue(&CODECAPI_AVEncCommonMeanBitRate, &variant_u32(kbps * 1000))
            };
        }
    }
}

/// A picture as NV12: the Y plane, then U and V interleaved.
fn to_nv12(p: &Picture, out: &mut Vec<u8>) {
    out.clear();
    out.extend_from_slice(p.y());
    for (u, v) in p.u().iter().zip(p.v()) {
        out.push(*u);
        out.push(*v);
    }
}

impl Encoder for MfEncoder {
    fn encode(&mut self, picture: &Picture, keyframe: bool) -> Result<Option<Encoded>, String> {
        let size = (picture.width, picture.height);
        if self.transform.is_none() || self.size != size {
            // A new size is a new stream, from an IDR.
            self.transform = Some(Owned(self.make(size.0, size.1)?));
            self.size = size;
        }
        let Some(Owned(t)) = &self.transform else {
            return Ok(None);
        };
        if keyframe {
            if let Ok(api) = t.cast::<ICodecAPI>() {
                // SAFETY: a documented property and its type.
                let _ = unsafe { api.SetValue(&CODECAPI_AVEncVideoForceKeyFrame, &variant_u32(1)) };
            }
        }
        to_nv12(picture, &mut self.nv12);
        let duration = 10_000_000 / i64::from(self.fps);
        let input = sample(&self.nv12, self.time, duration)?;
        self.time += duration;
        // SAFETY: a sample of the input type's size.
        unsafe { t.ProcessInput(0, &input, 0) }.map_err(err("input"))?;
        let mut data = Vec::new();
        loop {
            match pull(t)? {
                Output::Sample(s) => data.extend_from_slice(&bytes(&s)?),
                Output::NeedsInput => break,
                Output::StreamChanged => {
                    return Err("the encoder changed its stream".into());
                }
            }
        }
        if data.is_empty() {
            // Held back by rate control (or the encoder's delay).
            return Ok(None);
        }
        let keyframe = nal_units(&data)
            .iter()
            .any(|n| n.first().is_some_and(|b| b & 0x1f == 5));
        Ok(Some(Encoded { data, keyframe }))
    }

    fn set_rate(&mut self, kbps: u32, fps: u32) {
        let kbps = kbps.max(30);
        self.fps = fps.max(1);
        if kbps != self.kbps {
            self.kbps = kbps;
            if let Some(Owned(t)) = &self.transform {
                Self::rate(t, kbps);
            }
        }
    }
}

#[derive(Default)]
pub(crate) struct MfDecoder {
    transform: Option<Owned<IMFTransform>>,
    /// The output: its buffer's size, stride, and the picture within it.
    format: Option<Format>,
    time: i64,
}

#[derive(Debug, Clone, Copy)]
struct Format {
    /// The buffer's height (the coded height: 360 lines decode into 368).
    height: u32,
    stride: u32,
    /// The picture in it.
    show: (u32, u32, u32, u32),
}

impl MfDecoder {
    fn make() -> Result<IMFTransform, String> {
        let t = make(&CLSID_MSH264DecoderMFT)?;
        // SAFETY: the decoder's set-up: H.264 in; its output chosen once
        // it knows the stream.
        unsafe {
            if let Ok(attributes) = t.GetAttributes() {
                let _ = attributes.SetUINT32(&MF_LOW_LATENCY, 1);
            }
            let input = video_type(&MFVideoFormat_H264, 0, 0, 30)?;
            t.SetInputType(0, &input, 0).map_err(err("input type"))?;
        }
        // Before the stream says its size there may be none to choose; the
        // stream's change chooses it then.
        let _ = Self::choose_output(&t);
        Ok(t)
    }

    /// NV12 out, of the size the stream has now.
    fn choose_output(t: &IMFTransform) -> Result<Option<Format>, String> {
        // SAFETY: the transform's own output types.
        unsafe {
            for i in 0.. {
                let Ok(ty) = t.GetOutputAvailableType(0, i) else {
                    return Err("the decoder offers no NV12".into());
                };
                if ty.GetGUID(&MF_MT_SUBTYPE).ok() != Some(MFVideoFormat_NV12) {
                    continue;
                }
                t.SetOutputType(0, &ty, 0).map_err(err("output type"))?;
                return Ok(format(&ty));
            }
        }
        Ok(None)
    }
}

/// An output type's sizes, once the stream has given them.
fn format(ty: &IMFMediaType) -> Option<Format> {
    // SAFETY: attributes of a media type.
    unsafe {
        let size = ty.GetUINT64(&MF_MT_FRAME_SIZE).ok()?;
        let (width, height) = ((size >> 32) as u32, size as u32);
        if width == 0 || height == 0 {
            return None;
        }
        let stride = ty
            .GetUINT32(&MF_MT_DEFAULT_STRIDE)
            .ok()
            .map(|s| s as i32)
            .filter(|s| *s > 0)
            .map_or(width, |s| s as u32);
        let mut area = MFVideoArea {
            OffsetX: Default::default(),
            OffsetY: Default::default(),
            Area: SIZE { cx: 0, cy: 0 },
        };
        let blob = std::slice::from_raw_parts_mut(
            (&mut area as *mut MFVideoArea).cast::<u8>(),
            std::mem::size_of::<MFVideoArea>(),
        );
        let show = match ty.GetBlob(&MF_MT_MINIMUM_DISPLAY_APERTURE, blob, None) {
            Ok(()) if area.Area.cx > 0 && area.Area.cy > 0 => (
                area.OffsetX.value.max(0) as u32,
                area.OffsetY.value.max(0) as u32,
                area.Area.cx as u32,
                area.Area.cy as u32,
            ),
            _ => (0, 0, width, height),
        };
        Some(Format {
            height,
            stride,
            show,
        })
    }
}

/// The picture in a decoded NV12 buffer.
fn from_nv12(data: &[u8], f: Format) -> Option<Picture> {
    let (x, y, w, h) = f.show;
    let (x, y, w, h) = (
        x as usize & !1,
        y as usize & !1,
        w as usize & !1,
        h as usize & !1,
    );
    let stride = f.stride as usize;
    let luma = stride * f.height as usize;
    let chroma_rows = (f.height as usize).div_ceil(2);
    if w == 0 || h == 0 || data.len() < luma + stride * chroma_rows {
        return None;
    }
    let ys = &data[y * stride + x..luma];
    let uvs = &data[luma + (y / 2) * stride + x..];
    Some(Picture::from_nv12(
        ys, stride, uvs, stride, w as u32, h as u32,
    ))
}

impl Decoder for MfDecoder {
    fn decode(&mut self, data: &[u8]) -> Result<Option<Picture>, String> {
        if self.transform.is_none() {
            self.transform = Some(Owned(Self::make()?));
        }
        let Some(Owned(t)) = &self.transform else {
            return Ok(None);
        };
        let input = sample(data, self.time, 333_333)?;
        self.time += 333_333;
        // SAFETY: a sample of Annex B H.264.
        unsafe { t.ProcessInput(0, &input, 0) }.map_err(err("input"))?;
        let mut picture = None;
        loop {
            match pull(t)? {
                Output::Sample(s) => {
                    let format = match self.format {
                        Some(f) => f,
                        None => {
                            // SAFETY: the transform's current output type.
                            let ty = unsafe { t.GetOutputCurrentType(0) }.map_err(err("type"))?;
                            let f = format(&ty).ok_or("the decoder's output has no size")?;
                            self.format = Some(f);
                            f
                        }
                    };
                    picture = from_nv12(&bytes(&s)?, format);
                }
                Output::NeedsInput => break,
                Output::StreamChanged => {
                    // A new size: the output type chosen again.
                    self.format = Self::choose_output(t)?;
                }
            }
        }
        Ok(picture)
    }
}
