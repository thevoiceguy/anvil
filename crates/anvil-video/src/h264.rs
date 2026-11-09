//! H.264 constrained baseline through Cisco's OpenH264, loaded from the
//! binary Cisco publishes (see [`crate::openh264_fetch`]). The library is
//! checked against Cisco's hashes each time it is loaded.

use std::path::{Path, PathBuf};

use openh264::decoder::{Decoder as OhDecoder, DecoderConfig};
use openh264::encoder::{
    BitRate, Encoder as OhEncoder, EncoderConfig, FrameRate, FrameType, RateControlMode, UsageType,
};
use openh264::formats::YUVSource;
use openh264::OpenH264API;

use crate::codec::{Decoder, Encoded, Encoder};
use crate::picture::Picture;

fn api(path: &Path) -> Result<OpenH264API, String> {
    OpenH264API::from_blob_path(path).map_err(|e| format!("OpenH264 at {}: {e}", path.display()))
}

/// A [`Picture`] as OpenH264 reads one.
struct Yuv<'a>(&'a Picture);

impl YUVSource for Yuv<'_> {
    fn dimensions(&self) -> (usize, usize) {
        (self.0.width as usize, self.0.height as usize)
    }
    fn strides(&self) -> (usize, usize, usize) {
        let c = self.0.chroma_width();
        (self.0.width as usize, c, c)
    }
    fn y(&self) -> &[u8] {
        self.0.y()
    }
    fn u(&self) -> &[u8] {
        self.0.u()
    }
    fn v(&self) -> &[u8] {
        self.0.v()
    }
}

pub(crate) struct H264Encoder {
    path: PathBuf,
    encoder: Option<OhEncoder>,
    /// The size the encoder was made for.
    size: (u32, u32),
    kbps: u32,
    fps: u32,
    rate_changed: bool,
}

impl H264Encoder {
    pub(crate) fn new(path: &Path, kbps: u32, fps: u32) -> Result<Self, String> {
        // Loaded once now, so a library that fails its check fails here.
        api(path)?;
        Ok(Self {
            path: path.to_owned(),
            encoder: None,
            size: (0, 0),
            kbps: kbps.max(30),
            fps: fps.max(1),
            rate_changed: false,
        })
    }

    fn make(&self) -> Result<OhEncoder, String> {
        let config = EncoderConfig::new()
            .usage_type(UsageType::CameraVideoRealTime)
            .rate_control_mode(RateControlMode::Bitrate)
            .bitrate(BitRate::from_bps(self.kbps * 1000))
            .max_frame_rate(FrameRate::from_hz(self.fps as f32))
            .skip_frames(true);
        OhEncoder::with_api_config(api(&self.path)?, config).map_err(|e| e.to_string())
    }

    fn apply_rate(encoder: &mut OhEncoder, kbps: u32, fps: u32) {
        let bps = (kbps * 1000) as i32;
        let mut target = openh264_sys2::TagBitrateInfo {
            iLayer: openh264_sys2::SPATIAL_LAYER_ALL,
            iBitrate: bps,
        };
        let mut max = openh264_sys2::TagBitrateInfo {
            iLayer: openh264_sys2::SPATIAL_LAYER_ALL,
            // Room above the target for a keyframe; the core's REMB is
            // the ceiling over a second, not per frame.
            iBitrate: bps + bps / 2,
        };
        let mut rate = fps as f32;
        // SAFETY: the options are OpenH264's own, with the argument types
        // its documentation gives (SBitrateInfo, float); the encoder is
        // initialised (it has encoded a frame at this size).
        unsafe {
            let raw = encoder.raw_api();
            raw.set_option(
                openh264_sys2::ENCODER_OPTION_MAX_BITRATE,
                (&raw mut max).cast(),
            );
            raw.set_option(
                openh264_sys2::ENCODER_OPTION_BITRATE,
                (&raw mut target).cast(),
            );
            raw.set_option(
                openh264_sys2::ENCODER_OPTION_FRAME_RATE,
                (&raw mut rate).cast(),
            );
        }
    }
}

impl Encoder for H264Encoder {
    fn encode(&mut self, picture: &Picture, keyframe: bool) -> Result<Option<Encoded>, String> {
        let size = (picture.width, picture.height);
        if self.encoder.is_none() || self.size != size {
            // A new size is a new stream: made with the rate as it is now,
            // its first frame an IDR with its parameter sets.
            self.encoder = Some(self.make()?);
            self.size = size;
            self.rate_changed = false;
        }
        let Some(encoder) = self.encoder.as_mut() else {
            return Ok(None);
        };
        if keyframe {
            encoder.force_intra_frame();
        }
        let stream = encoder.encode(&Yuv(picture)).map_err(|e| e.to_string())?;
        let kind = stream.frame_type();
        let out = match kind {
            FrameType::Skip | FrameType::Invalid => None,
            _ => Some(Encoded {
                data: stream.to_vec(),
                keyframe: kind == FrameType::IDR,
            }),
        };
        if self.rate_changed {
            Self::apply_rate(encoder, self.kbps, self.fps);
            self.rate_changed = false;
        }
        Ok(out)
    }

    fn set_rate(&mut self, kbps: u32, fps: u32) {
        let (kbps, fps) = (kbps.max(30), fps.max(1));
        if (kbps, fps) != (self.kbps, self.fps) {
            self.kbps = kbps;
            self.fps = fps;
            self.rate_changed = true;
        }
    }
}

pub(crate) struct H264Decoder {
    decoder: OhDecoder,
}

impl H264Decoder {
    pub(crate) fn new(path: &Path) -> Result<Self, String> {
        let decoder = OhDecoder::with_api_config(api(path)?, DecoderConfig::new())
            .map_err(|e| e.to_string())?;
        Ok(Self { decoder })
    }
}

impl Decoder for H264Decoder {
    fn decode(&mut self, data: &[u8]) -> Result<Option<Picture>, String> {
        let Some(yuv) = self.decoder.decode(data).map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let (w, h) = yuv.dimensions();
        let (ys, us, vs) = yuv.strides();
        Ok(Some(Picture::from_planes(
            w as u32,
            h as u32,
            (yuv.y(), ys),
            (yuv.u(), us),
            (yuv.v(), vs),
        )))
    }
}
