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
//! - **Sharing.** A screen ([`Screens`]) is captured and sent as a camera
//!   is, through the same source, shown in a tile of its own
//!   ([`TileKey::ShareView`]).
//! - **Tiles.** [`Tiles`] keeps each stream's last picture and the
//!   self-view; the app attaches a [`Renderer`] (a Flutter texture) to
//!   those on its screen. Pictures never cross into Dart.
//!
//! **Linux**: V4L2 cameras; the X server's monitors; VP8 through libvpx; H.264 through Cisco's
//! OpenH264, fetched at first use ([`openh264_fetch`]). **macOS**:
//! AVFoundation cameras; H.264 through VideoToolbox. **Windows**: Media
//! Foundation cameras; H.264 through Media Foundation. **Android**:
//! Camera2 cameras through the NDK; H.264 and VP8 through MediaCodec.

#[cfg(target_os = "android")]
mod aimage;
#[cfg(target_vendor = "apple")]
pub mod avf;
pub mod camera;
pub mod codec;
mod h264;
#[cfg(target_os = "android")]
mod mc;
#[cfg(windows)]
mod mf;
#[cfg(windows)]
pub mod mfcam;
#[cfg(target_os = "android")]
pub mod ndkcam;
pub mod openh264_fetch;
pub mod picture;
pub mod png;
#[cfg(target_os = "macos")]
pub mod sck;
pub mod screen;
mod sink;
mod source;
pub mod tiles;
#[cfg(target_os = "linux")]
pub mod v4l2;
#[cfg(target_os = "linux")]
mod vpx;
#[cfg(target_vendor = "apple")]
mod vt;
#[cfg(target_os = "linux")]
pub mod wayland;
#[cfg(target_os = "linux")]
pub mod x11;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anvil_core::error::AnvilError;
use anvil_core::video::host::{
    CameraInfo, ReceiveConfig, Resolution, ScreenInfo, SendConfig, StreamId, VideoCodecCaps,
    VideoHost, VideoKind, VideoSink, VideoSource,
};
use parking_lot::Mutex;

pub use camera::{Cameras, Capture, NoCameras, PatternCamera};
pub use codec::{Codecs, Decoder, Encoded, Encoder, SoftwareCodecs};
pub use picture::Picture;
pub use screen::{NoScreens, PatternScreen, Screens};
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
    screens: Arc<dyn Screens>,
    /// The screen a share without one captures.
    screen: Arc<Mutex<Option<String>>>,
    /// The shares running, to switch screens.
    live_screens: Arc<Mutex<Vec<source::Switch>>>,
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
            screens: Arc::new(NoScreens),
            screen: Arc::default(),
            live_screens: Arc::default(),
        }
    }

    /// The platform's cameras, screens and codecs. OpenH264 is used from
    /// `openh264_dir` if it is there ([`Self::fetch_openh264`] puts it
    /// there).
    pub fn platform(openh264_dir: &Path) -> Self {
        Self::with_cameras(platform_cameras(), openh264_dir).with_screens(platform_screens())
    }

    /// The same host sharing `screens` (a test pattern, say).
    pub fn with_screens(mut self, screens: impl Screens + 'static) -> Self {
        self.screens = Arc::new(screens);
        self
    }

    /// The platform's codecs with other cameras (a test pattern).
    pub fn with_cameras(cameras: impl Cameras + 'static, openh264_dir: &Path) -> Self {
        // macOS: the system's own H.264 (VideoToolbox, in hardware where
        // the Mac has it), so nothing is fetched.
        #[cfg(target_vendor = "apple")]
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
        // Android: the phone's own codecs, H.264 and VP8.
        #[cfg(target_os = "android")]
        let (codecs, software): (Arc<dyn Codecs>, _) = {
            let _ = openh264_dir;
            (Arc::new(mc::McCodecs), None)
        };
        #[cfg(not(any(target_vendor = "apple", windows, target_os = "android")))]
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
            screens: Arc::new(NoScreens),
            screen: Arc::default(),
            live_screens: Arc::default(),
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
    #[cfg(target_vendor = "apple")]
    let cameras = Box::new(avf::AvfCameras);
    #[cfg(windows)]
    let cameras = Box::new(mfcam::MfCameras);
    #[cfg(target_os = "android")]
    let cameras = Box::new(ndkcam::NdkCameras);
    #[cfg(not(any(
        target_os = "linux",
        target_vendor = "apple",
        windows,
        target_os = "android"
    )))]
    let cameras = Box::new(NoCameras);
    cameras
}

/// The platform's screens: on Linux, the desktop portal's under Wayland
/// (the user chooses in the desktop's dialog), else the X server's
/// monitors (none without a display); a Mac's displays through
/// ScreenCaptureKit.
pub fn platform_screens() -> Box<dyn Screens> {
    #[cfg(target_os = "linux")]
    let screens: Box<dyn Screens> = if wayland::is_wayland() {
        Box::new(wayland::PortalScreens)
    } else {
        Box::new(x11::X11Screens)
    };
    #[cfg(target_os = "macos")]
    let screens: Box<dyn Screens> = Box::new(sck::SckScreens);
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let screens: Box<dyn Screens> = Box::new(NoScreens);
    screens
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

    fn screens(&self) -> Vec<ScreenInfo> {
        self.screens.list()
    }

    fn set_screen(&self, screen: Option<&str>) {
        *self.screen.lock() = screen.map(str::to_owned);
        self.live_screens.lock().retain(|s| s.to(screen));
    }

    fn start_send(&self, cfg: SendConfig) -> Result<Box<dyn VideoSource>, AnvilError> {
        let encoder = self
            .codecs
            .encoder(cfg.codec, cfg.kbps, cfg.fps)
            .map_err(AnvilError::Codec)?;
        // A screen is captured as a camera is, through the same source.
        let (devices, id, live, view): (Arc<dyn Cameras>, _, _, _) = match cfg.kind {
            VideoKind::Camera => (
                Arc::clone(&self.cameras),
                cfg.camera.clone().or_else(|| self.camera.lock().clone()),
                &self.live,
                TileKey::SelfView,
            ),
            VideoKind::Content => (
                Arc::new(screen::AsCameras(Arc::clone(&self.screens))),
                self.screen.lock().clone(),
                &self.live_screens,
                TileKey::ShareView,
            ),
        };
        // Opened here so a device that cannot be opened is an error now,
        // not a stream that never starts. It takes a moment.
        let capture = devices
            .open(id.as_deref(), cfg.max, cfg.fps)
            .map_err(AnvilError::Codec)?;
        let camera = source::Camera {
            cameras: devices,
            id,
            capture,
            want: cfg.max,
            fps: cfg.fps,
            view,
        };
        let source = source::CameraSource::start(
            camera,
            encoder,
            (cfg.kbps, cfg.max, cfg.fps),
            self.tiles.clone(),
        );
        let mut live = live.lock();
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

/// A test is a bare program, not an app: nothing has started the binder
/// threads that a camera's and a decoder's pictures arrive on, so this
/// does, as an app's runtime would (the call is not in the NDK's headers).
#[cfg(all(test, target_os = "android"))]
pub(crate) fn binder_threads() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // SAFETY: the system's binder library; the symbol takes nothing
        // and returns nothing.
        unsafe {
            let lib = libloading::Library::new("libbinder_ndk.so").expect("libbinder_ndk");
            let start: libloading::Symbol<unsafe extern "C" fn()> = lib
                .get(b"ABinderProcess_startThreadPool\0")
                .expect("ABinderProcess_startThreadPool");
            start();
            std::mem::forget(lib);
        }
    });
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

    #[tokio::test]
    async fn a_screen_is_shared_through_the_same_source_and_shown_apart() {
        let host = DesktopVideoHost::new(TwoCameras::default(), SoftwareCodecs::default())
            .with_screens(PatternScreen::new(Resolution {
                width: 320,
                height: 180,
            }));
        assert_eq!(host.screens().len(), 1);
        host.set_screen(Some("pattern"));
        let mut share = host
            .start_send(SendConfig {
                kind: VideoKind::Content,
                fps: 10,
                ..send_config()
            })
            .expect("the screen opens");
        let (_, first) = share.next_frame().await.unwrap();
        assert!(first.keyframe);
        for _ in 0..3 {
            share.next_frame().await.unwrap();
        }
        // Shown as our share, not as the camera's self-view.
        let tiles = host.tiles();
        let shown = tiles.stats(TileKey::ShareView).expect("the share's tile");
        assert_eq!((shown.width, shown.height), (320, 180));
        assert!(tiles.stats(TileKey::SelfView).is_none());
        // A camera alongside keeps its own tile.
        let mut camera = host.start_send(send_config()).unwrap();
        camera.next_frame().await.unwrap();
        assert!(tiles.stats(TileKey::SelfView).is_some());

        // A host without screens refuses a share.
        let bare = DesktopVideoHost::new(TwoCameras::default(), SoftwareCodecs::default());
        assert!(bare.screens().is_empty());
        assert!(bare
            .start_send(SendConfig {
                kind: VideoKind::Content,
                ..send_config()
            })
            .is_err());
    }
}
