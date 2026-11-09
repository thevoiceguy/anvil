//! anvil-video — the desktop's [`VideoHost`]: cameras, codecs and tiles
//! for Anvil's video calls (FCP's `docs/SOFTPHONE_VIDEO.md` §2.3).
//!
//! `anvil-core` owns the network and trades coded frames with a host;
//! this crate is the desktop's host:
//!
//! - **Sending.** A camera ([`Cameras`]) is captured on a thread of its
//!   own, each picture fitted to the resolution the network allows,
//!   encoded, and handed to the core. Dropping the source closes the
//!   camera, and its light.
//! - **Receiving.** Each incoming stream is decoded on a thread of its
//!   own and its pictures shown, each at the moment the core gives
//!   (playout and lip sync are the core's), in the stream's tile.
//! - **Tiles.** [`Tiles`] keeps each stream's last picture and the
//!   self-view; the app attaches a [`Renderer`] (a Flutter texture) to
//!   those on its screen. Pictures never cross into Dart.
//!
//! **Linux** (this release): V4L2 cameras; VP8 through libvpx; H.264
//! through Cisco's OpenH264, fetched at first use ([`openh264_fetch`]).
//! macOS and Windows bring their hardware codecs next; until then their
//! host has no codecs, and calls there stay audio.

pub mod camera;
pub mod codec;
mod h264;
pub mod openh264_fetch;
pub mod picture;
pub mod png;
mod sink;
mod source;
pub mod tiles;
#[cfg(target_os = "linux")]
pub mod v4l2;
#[cfg(target_os = "linux")]
mod vpx;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anvil_core::error::AnvilError;
use anvil_core::video::host::{
    CameraInfo, ReceiveConfig, Resolution, SendConfig, StreamId, VideoCodecCaps, VideoHost,
    VideoKind, VideoSink, VideoSource,
};

pub use camera::{Cameras, Capture, NoCameras, PatternCamera};
pub use codec::{Codecs, Decoder, Encoded, Encoder, SoftwareCodecs};
pub use picture::Picture;
pub use tiles::{Renderer, TileKey, TileStats, Tiles};

/// The desktop's video host.
#[derive(Clone)]
pub struct DesktopVideoHost {
    cameras: Arc<dyn Cameras>,
    codecs: Arc<dyn Codecs>,
    software: Option<SoftwareCodecs>,
    tiles: Tiles,
}

impl DesktopVideoHost {
    /// A host from its parts.
    pub fn new(cameras: impl Cameras + 'static, codecs: impl Codecs + 'static) -> Self {
        Self {
            cameras: Arc::new(cameras),
            codecs: Arc::new(codecs),
            software: None,
            tiles: Tiles::default(),
        }
    }

    /// The platform's cameras and codecs. OpenH264 is used from
    /// `openh264_dir` if it is there ([`Self::fetch_openh264`] puts it
    /// there).
    pub fn platform(openh264_dir: &Path) -> Self {
        Self::with_cameras(platform_cameras(), openh264_dir)
    }

    /// The platform's codecs with other cameras (a test pattern).
    pub fn with_cameras(cameras: impl Cameras + 'static, openh264_dir: &Path) -> Self {
        let software = SoftwareCodecs::with_openh264(openh264_fetch::installed(openh264_dir));
        Self {
            cameras: Arc::new(cameras),
            codecs: Arc::new(software.clone()),
            software: Some(software),
            tiles: Tiles::default(),
        }
    }

    /// Fetch Cisco's OpenH264 into `dir` if it is not there, then offer
    /// H.264. Until it is, calls offer VP8.
    pub async fn fetch_openh264(&self, dir: &Path) -> Result<PathBuf, String> {
        let software = self
            .software
            .as_ref()
            .ok_or("this host's codecs are not software")?;
        let path = openh264_fetch::fetch(dir).await?;
        software.set_openh264(path.clone());
        Ok(path)
    }

    /// Where pictures are shown.
    pub fn tiles(&self) -> Tiles {
        self.tiles.clone()
    }
}

/// The platform's cameras.
pub fn platform_cameras() -> Box<dyn Cameras> {
    #[cfg(target_os = "linux")]
    let cameras = Box::new(v4l2::V4l2Cameras);
    #[cfg(not(target_os = "linux"))]
    let cameras = Box::new(NoCameras);
    cameras
}

impl Cameras for Box<dyn Cameras> {
    fn list(&self) -> Vec<CameraInfo> {
        (**self).list()
    }
    fn open(
        &self,
        id: Option<&str>,
        want: Resolution,
        fps: u32,
    ) -> Result<Box<dyn Capture>, String> {
        (**self).open(id, want, fps)
    }
}

impl VideoHost for DesktopVideoHost {
    fn codecs(&self) -> Vec<VideoCodecCaps> {
        self.codecs.caps()
    }

    fn cameras(&self) -> Vec<CameraInfo> {
        self.cameras.list()
    }

    fn start_send(&self, cfg: SendConfig) -> Result<Box<dyn VideoSource>, AnvilError> {
        if cfg.kind == VideoKind::Content {
            return Err(AnvilError::Codec(
                "screen share is not supported yet".into(),
            ));
        }
        let encoder = self
            .codecs
            .encoder(cfg.codec, cfg.kbps, cfg.fps)
            .map_err(AnvilError::Codec)?;
        // Opened here so a camera that cannot be opened is an error now,
        // not a stream that never starts. It takes a moment.
        let capture = self
            .cameras
            .open(cfg.camera.as_deref(), cfg.max, cfg.fps)
            .map_err(AnvilError::Codec)?;
        Ok(Box::new(source::CameraSource::start(
            capture,
            encoder,
            (cfg.kbps, cfg.max, cfg.fps),
            self.tiles.clone(),
        )))
    }

    fn start_receive(
        &self,
        stream: StreamId,
        cfg: ReceiveConfig,
    ) -> Result<Box<dyn VideoSink>, AnvilError> {
        let decoder = self.codecs.decoder(cfg.codec).map_err(AnvilError::Codec)?;
        Ok(Box::new(sink::RenderSink::start(
            stream,
            decoder,
            self.tiles.clone(),
        )))
    }
}
