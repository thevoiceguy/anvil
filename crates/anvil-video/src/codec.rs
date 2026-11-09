//! Encoders and decoders, behind one interface so each platform brings
//! its own: software on Linux (OpenH264 and libvpx); VideoToolbox and
//! Media Foundation later.

use std::path::PathBuf;
use std::sync::Arc;

use anvil_core::video::host::{VideoCodec, VideoCodecCaps};
use parking_lot::RwLock;

use crate::picture::Picture;

/// A coded frame from an encoder.
#[derive(Debug, Clone)]
pub struct Encoded {
    /// Annex B for H.264, the raw frame for VP8.
    pub data: Vec<u8>,
    pub keyframe: bool,
}

pub trait Encoder: Send {
    /// `picture` coded; `None` when the encoder dropped it (rate control).
    /// A picture of another size starts the stream again with a keyframe.
    fn encode(&mut self, picture: &Picture, keyframe: bool) -> Result<Option<Encoded>, String>;
    /// What the network allows now.
    fn set_rate(&mut self, kbps: u32, fps: u32);
}

pub trait Decoder: Send {
    /// A coded frame decoded; `None` when it gave no picture.
    fn decode(&mut self, data: &[u8]) -> Result<Option<Picture>, String>;
}

/// The codecs a platform has.
pub trait Codecs: Send + Sync {
    fn caps(&self) -> Vec<VideoCodecCaps>;
    fn encoder(&self, codec: VideoCodec, kbps: u32, fps: u32) -> Result<Box<dyn Encoder>, String>;
    fn decoder(&self, codec: VideoCodec) -> Result<Box<dyn Decoder>, String>;
}

/// Software codecs: H.264 once Cisco's OpenH264 is on disk, and VP8
/// where libvpx is built in (Linux).
#[derive(Clone, Default)]
pub struct SoftwareCodecs {
    openh264: Arc<RwLock<Option<PathBuf>>>,
}

impl SoftwareCodecs {
    /// H.264 from the OpenH264 library at `path` (checked against Cisco's
    /// hashes when it is loaded).
    pub fn with_openh264(path: Option<PathBuf>) -> Self {
        Self {
            openh264: Arc::new(RwLock::new(path)),
        }
    }

    /// OpenH264 has arrived (downloaded after the host was made).
    pub fn set_openh264(&self, path: PathBuf) {
        *self.openh264.write() = Some(path);
    }

    fn openh264(&self) -> Option<PathBuf> {
        self.openh264.read().clone()
    }
}

impl Codecs for SoftwareCodecs {
    fn caps(&self) -> Vec<VideoCodecCaps> {
        let mut caps = Vec::new();
        if self.openh264().is_some() {
            caps.push(VideoCodecCaps {
                codec: VideoCodec::H264,
                encode: true,
                decode: true,
            });
        }
        if cfg!(target_os = "linux") {
            caps.push(VideoCodecCaps {
                codec: VideoCodec::Vp8,
                encode: true,
                decode: true,
            });
        }
        caps
    }

    fn encoder(&self, codec: VideoCodec, kbps: u32, fps: u32) -> Result<Box<dyn Encoder>, String> {
        match codec {
            VideoCodec::H264 => {
                let path = self.openh264().ok_or("no OpenH264")?;
                Ok(Box::new(crate::h264::H264Encoder::new(&path, kbps, fps)?))
            }
            #[cfg(target_os = "linux")]
            VideoCodec::Vp8 => Ok(Box::new(crate::vpx::Vp8Encoder::new(kbps, fps))),
            #[cfg(not(target_os = "linux"))]
            VideoCodec::Vp8 => Err("no VP8 encoder".into()),
        }
    }

    fn decoder(&self, codec: VideoCodec) -> Result<Box<dyn Decoder>, String> {
        match codec {
            VideoCodec::H264 => {
                let path = self.openh264().ok_or("no OpenH264")?;
                Ok(Box::new(crate::h264::H264Decoder::new(&path)?))
            }
            #[cfg(target_os = "linux")]
            VideoCodec::Vp8 => Ok(Box::new(crate::vpx::Vp8Decoder::new()?)),
            #[cfg(not(target_os = "linux"))]
            VideoCodec::Vp8 => Err("no VP8 decoder".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::picture::Picture;

    /// OpenH264 for the tests: `ANVIL_OPENH264_DIR`, where it was fetched
    /// (CI sets it, so there it must be there).
    fn openh264_for_tests() -> Option<PathBuf> {
        let dir = std::env::var_os("ANVIL_OPENH264_DIR")?;
        let found = crate::openh264_fetch::installed(std::path::Path::new(&dir));
        assert!(found.is_some(), "no OpenH264 in ANVIL_OPENH264_DIR");
        found
    }

    /// Frames of the test pattern through `codec` and back: each decoded
    /// picture close to what was sent; keyframes when asked; a new size a
    /// new keyframe.
    fn round_trip(codecs: &SoftwareCodecs, codec: VideoCodec) {
        let mut enc = codecs.encoder(codec, 1200, 30).unwrap();
        let mut dec = codecs.decoder(codec).unwrap();
        let mut keyframes = Vec::new();
        let mut worst = f64::MAX;
        for n in 0..40u32 {
            let size = if n < 30 { (640, 360) } else { (320, 180) };
            let sent = Picture::pattern(size.0, size.1, n);
            if n == 20 {
                enc.set_rate(600, 30);
            }
            let Some(e) = enc.encode(&sent, n == 10).unwrap() else {
                continue;
            };
            if e.keyframe {
                keyframes.push(n);
            }
            let got = dec.decode(&e.data).unwrap().expect("a picture");
            assert_eq!((got.width, got.height), size);
            worst = worst.min(got.psnr(&sent));
        }
        assert_eq!(keyframes, [0, 10, 30], "{codec:?}");
        assert!(worst > 28.0, "{codec:?}: worst PSNR {worst:.1} dB");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn vp8_round_trips_the_pattern() {
        round_trip(&SoftwareCodecs::default(), VideoCodec::Vp8);
    }

    #[test]
    fn h264_round_trips_the_pattern_when_openh264_is_there() {
        let Some(path) = openh264_for_tests() else {
            eprintln!("skipped: ANVIL_OPENH264_DIR has no OpenH264");
            return;
        };
        let codecs = SoftwareCodecs::with_openh264(Some(path));
        round_trip(&codecs, VideoCodec::H264);
        // Each IDR carries its parameter sets (the core needs them).
        let mut enc = codecs.encoder(VideoCodec::H264, 800, 30).unwrap();
        let e = enc
            .encode(&Picture::pattern(320, 180, 0), true)
            .unwrap()
            .unwrap();
        let types: Vec<u8> = openh264::nal_units(&e.data)
            .map(|nal| nal[nal.iter().position(|&b| b == 1).unwrap() + 1] & 0x1F)
            .collect();
        assert!(
            types.starts_with(&[7, 8]) && types.contains(&5),
            "{types:?}"
        );
    }

    #[test]
    fn without_openh264_there_is_no_h264() {
        let codecs = SoftwareCodecs::default();
        assert!(codecs.caps().iter().all(|c| c.codec != VideoCodec::H264));
        assert!(codecs.encoder(VideoCodec::H264, 500, 30).is_err());
    }
}
