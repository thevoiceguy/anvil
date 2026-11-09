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
//! **Linux**: V4L2 cameras; VP8 through libvpx; H.264 through Cisco's
//! OpenH264, fetched at first use ([`openh264_fetch`]). **macOS**:
//! AVFoundation cameras; H.264 through VideoToolbox. **Windows**: H.264
//! through Media Foundation (cameras next).

#[cfg(target_os = "macos")]
pub mod avf;
pub mod camera;
pub mod codec;
mod h264;
#[cfg(windows)]
mod mf;
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
#[cfg(target_os = "macos")]
mod vt;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anvil_core::error::AnvilError;
use anvil_core::video::host::{
    CameraInfo, ReceiveConfig, Resolution, SendConfig, StreamId, VideoCodecCaps, VideoHost,
    VideoKind, VideoSink, VideoSource,
};
use parking_lot::Mutex;

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
    /// The camera a send without one opens.
    camera: Arc<Mutex<Option<String>>>,
    /// The sources running, to switch cameras.
    live: Arc<Mutex<Vec<source::Switch>>>,
}

impl DesktopVideoHost {
    /// A host from its parts.
    pub fn new(cameras: impl Cameras + 'static, codecs: impl Codecs + 'static) -> Self {
        Self {
            cameras: Arc::new(cameras),
            codecs: Arc::new(codecs),
            software: None,
            tiles: Tiles::default(),
            camera: Arc::default(),
            live: Arc::default(),
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
        // macOS: the system's own H.264 (VideoToolbox, in hardware where
        // the Mac has it), so nothing is fetched.
        #[cfg(target_os = "macos")]
        let (codecs, software): (Arc<dyn Codecs>, _) = {
            let _ = openh264_dir;
            (Arc::new(vt::VtCodecs), None)
        };
        // Windows: Media Foundation's H.264, which Windows' licence covers.
        #[cfg(windows)]
        let (codecs, software): (Arc<dyn Codecs>, _) = {
            let _ = openh264_dir;
            (Arc::new(mf::MfCodecs), None)
        };
        #[cfg(not(any(target_os = "macos", windows)))]
        let (codecs, software): (Arc<dyn Codecs>, _) = {
            let software = SoftwareCodecs::with_openh264(openh264_fetch::installed(openh264_dir));
            (Arc::new(software.clone()), Some(software))
        };
        Self {
            cameras: Arc::new(cameras),
            codecs,
            software,
            tiles: Tiles::default(),
            camera: Arc::default(),
            live: Arc::default(),
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
        software.set_openh264(Some(path.clone()));
        Ok(path)
    }

    /// Stop offering H.264 from OpenH264 (the user turned it off); calls
    /// already using it keep it.
    pub fn drop_openh264(&self) {
        if let Some(software) = &self.software {
            software.set_openh264(None);
        }
    }

    /// Whether H.264 is offered.
    pub fn has_h264(&self) -> bool {
        self.codecs
            .caps()
            .iter()
            .any(|c| c.codec == anvil_core::video::host::VideoCodec::H264)
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
    #[cfg(target_os = "macos")]
    let cameras = Box::new(avf::AvfCameras);
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
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

    fn set_camera(&self, camera: Option<&str>) {
        *self.camera.lock() = camera.map(str::to_owned);
        self.live.lock().retain(|s| s.to(camera));
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
        let id = cfg.camera.clone().or_else(|| self.camera.lock().clone());
        let capture = self
            .cameras
            .open(id.as_deref(), cfg.max, cfg.fps)
            .map_err(AnvilError::Codec)?;
        let camera = source::Camera {
            cameras: Arc::clone(&self.cameras),
            id,
            capture,
            want: cfg.max,
            fps: cfg.fps,
        };
        let source = source::CameraSource::start(
            camera,
            encoder,
            (cfg.kbps, cfg.max, cfg.fps),
            self.tiles.clone(),
        );
        let mut live = self.live.lock();
        live.retain(source::Switch::alive);
        live.push(source.switch());
        Ok(Box::new(source))
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

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use anvil_core::video::host::{EncodingId, VideoCodec};

    /// Two pattern cameras, each opening recorded; one open at a time.
    #[derive(Clone, Default)]
    struct TwoCameras {
        opened: Arc<Mutex<Vec<Option<String>>>>,
    }

    impl Cameras for TwoCameras {
        fn list(&self) -> Vec<CameraInfo> {
            ["front", "side"]
                .map(|id| CameraInfo {
                    id: id.into(),
                    name: id.into(),
                })
                .to_vec()
        }
        fn open(
            &self,
            id: Option<&str>,
            want: Resolution,
            fps: u32,
        ) -> Result<Box<dyn Capture>, String> {
            if id == Some("broken") {
                return Err("busy".into());
            }
            self.opened.lock().push(id.map(str::to_owned));
            PatternCamera::new(Resolution {
                width: 160,
                height: 90,
            })
            .open(id, want, fps)
        }
    }

    fn send_config() -> SendConfig {
        SendConfig {
            codec: VideoCodec::Vp8,
            kind: VideoKind::Camera,
            camera: None,
            max: Resolution::P720,
            kbps: 300,
            fps: 30,
            encodings: vec![EncodingId(0)],
        }
    }

    #[tokio::test]
    async fn the_camera_chosen_is_opened_and_one_sending_switches_to_it_on_a_keyframe() {
        let cameras = TwoCameras::default();
        let host = DesktopVideoHost::new(cameras.clone(), SoftwareCodecs::default());
        host.set_camera(Some("front"));
        let mut source = host.start_send(send_config()).unwrap();
        let (_, first) = source.next_frame().await.unwrap();
        assert!(first.keyframe);
        assert_eq!(*cameras.opened.lock(), [Some("front".to_string())]);

        // Switched while sending: the side camera, from a keyframe.
        for _ in 0..5 {
            source.next_frame().await.unwrap();
        }
        host.set_camera(Some("side"));
        let mut keyed = false;
        for _ in 0..30 {
            let (_, f) = source.next_frame().await.unwrap();
            if f.keyframe {
                keyed = true;
                break;
            }
        }
        assert!(keyed, "a keyframe after the switch");
        assert_eq!(
            *cameras.opened.lock(),
            [Some("front".to_string()), Some("side".to_string())]
        );

        // One that will not open leaves the one there was.
        host.set_camera(Some("broken"));
        for _ in 0..10 {
            source.next_frame().await.unwrap();
        }
        assert_eq!(
            cameras.opened.lock().last().cloned().flatten().as_deref(),
            Some("side")
        );
        drop(source);
        // A new send opens the camera chosen last.
        host.set_camera(Some("front"));
        let mut again = host.start_send(send_config()).unwrap();
        again.next_frame().await.unwrap();
        assert_eq!(
            cameras.opened.lock().last().cloned().flatten().as_deref(),
            Some("front")
        );
    }
}
