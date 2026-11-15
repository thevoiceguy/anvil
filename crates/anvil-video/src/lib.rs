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
#[cfg(any(target_os = "ios", all(test, target_os = "linux")))]
pub mod broadcast;
pub mod camera;
pub mod codec;
#[cfg(windows)]
pub mod dxgi;
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
#[cfg(target_os = "android")]
pub mod projection;
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
/// ScreenCaptureKit; a PC's through DXGI's Desktop Duplication; an
/// Android phone's through MediaProjection (the user asked at each share);
/// an iPhone's through the app's ReplayKit broadcast extension.
pub fn platform_screens() -> Box<dyn Screens> {
    #[cfg(target_os = "linux")]
    let screens: Box<dyn Screens> = if wayland::is_wayland() {
        Box::new(wayland::PortalScreens)
    } else {
        Box::new(x11::X11Screens)
    };
    #[cfg(target_os = "macos")]
    let screens: Box<dyn Screens> = Box::new(sck::SckScreens);
    #[cfg(windows)]
    let screens: Box<dyn Screens> = Box::new(dxgi::DxgiScreens);
    #[cfg(target_os = "android")]
    let screens: Box<dyn Screens> = Box::new(projection::ProjectionScreens);
    #[cfg(target_os = "ios")]
    let screens: Box<dyn Screens> = Box::new(broadcast::BroadcastScreens::platform());
    #[cfg(not(any(
        target_os = "linux",
        target_os = "macos",
        windows,
        target_os = "android",
        target_os = "ios"
    )))]
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

    fn simulcast(&self) -> usize {
        self.codecs.simulcast()
    }

    fn start_send(&self, cfg: SendConfig) -> Result<Box<dyn VideoSource>, AnvilError> {
        // An encoder per encoding asked for, as many as the codecs run.
        let mut wanted = if cfg.encodings.is_empty() {
            SendConfig::single(cfg.max, cfg.kbps)
        } else {
            cfg.encodings.clone()
        };
        wanted.truncate(self.codecs.simulcast().max(1));
        let encodings = wanted
            .iter()
            .map(|e| {
                Ok(source::Encoding {
                    id: e.id,
                    encoder: self.codecs.encoder(cfg.codec, e.kbps, cfg.fps)?,
                    target: (e.kbps, e.max, cfg.fps),
                })
            })
            .collect::<Result<Vec<_>, String>>()
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
        let source = source::CameraSource::start(camera, encodings, self.tiles.clone());
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
    use anvil_core::video::host::{Encoding, EncodingId, EncodingTarget, VideoCodec};

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
            encodings: SendConfig::single(Resolution::P720, 300),
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

    /// Software codecs whose encoders at `fails_at` kb/s fail every
    /// picture (a hardware encoder out of sessions), counting the tries.
    #[derive(Clone)]
    struct FailingAt {
        fails_at: u32,
        tries: Arc<std::sync::atomic::AtomicU32>,
    }

    struct Failing(Arc<std::sync::atomic::AtomicU32>);

    impl codec::Encoder for Failing {
        fn encode(
            &mut self,
            _: &picture::Picture,
            _: bool,
        ) -> Result<Option<codec::Encoded>, String> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Err("no session".into())
        }
        fn set_rate(&mut self, _: u32, _: u32) {}
    }

    impl Codecs for FailingAt {
        fn caps(&self) -> Vec<anvil_core::video::host::VideoCodecCaps> {
            SoftwareCodecs::default().caps()
        }
        fn encoder(
            &self,
            codec: VideoCodec,
            kbps: u32,
            fps: u32,
        ) -> Result<Box<dyn codec::Encoder>, String> {
            if kbps == self.fails_at {
                return Ok(Box::new(Failing(Arc::clone(&self.tries))));
            }
            SoftwareCodecs::default().encoder(codec, kbps, fps)
        }
        fn decoder(&self, codec: VideoCodec) -> Result<Box<dyn codec::Decoder>, String> {
            SoftwareCodecs::default().decoder(codec)
        }
        fn simulcast(&self) -> usize {
            3
        }
    }

    #[tokio::test]
    async fn an_encoding_whose_encoder_keeps_failing_is_let_go_and_the_others_go_on() {
        let tries = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let host = DesktopVideoHost::new(
            PatternCamera::new(Resolution {
                width: 640,
                height: 360,
            }),
            FailingAt {
                fails_at: 100,
                tries: Arc::clone(&tries),
            },
        );
        let size = |width, height| Resolution { width, height };
        let layers = [
            (0, size(640, 360), 600),
            (1, size(320, 180), 200),
            (2, size(160, 90), 100),
        ];
        let mut source = host
            .start_send(SendConfig {
                max: layers[0].1,
                kbps: layers[0].2,
                encodings: layers
                    .iter()
                    .map(|&(id, max, kbps)| Encoding {
                        id: EncodingId(id),
                        max,
                        kbps,
                    })
                    .collect(),
                ..send_config()
            })
            .unwrap();
        // Two frames a picture: past the pictures it takes to let go.
        let mut sent = [0u32; 3];
        for _ in 0..2 * (source::GIVE_UP + 10) {
            let (id, _) = source.next_frame().await.unwrap();
            sent[usize::from(id.0)] += 1;
        }
        assert_eq!(sent[2], 0, "{sent:?}");
        assert!(
            sent[0] > source::GIVE_UP && sent[1] > source::GIVE_UP,
            "{sent:?}"
        );
        let tried = tries.load(std::sync::atomic::Ordering::Relaxed);
        assert_eq!(tried, source::GIVE_UP, "tried once a picture, then let go");

        // Alone, an encoder that fails is never let go: it is the call's
        // video.
        let tries = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let host = DesktopVideoHost::new(
            PatternCamera::new(Resolution {
                width: 640,
                height: 360,
            }),
            FailingAt {
                fails_at: 300,
                tries: Arc::clone(&tries),
            },
        );
        let _source = host.start_send(send_config()).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while tries.load(std::sync::atomic::Ordering::Relaxed) < 2 * source::GIVE_UP {
            assert!(
                std::time::Instant::now() < deadline,
                "the only encoder let go"
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    #[tokio::test]
    async fn simulcast_codes_each_encoding_at_its_size_and_pauses_one_alone() {
        let host = DesktopVideoHost::new(
            PatternCamera::new(Resolution {
                width: 640,
                height: 360,
            }),
            SoftwareCodecs::default(),
        );
        assert_eq!(host.simulcast(), 3);
        let size = |width, height| Resolution { width, height };
        let layers = [
            (0, size(640, 360), 600),
            (1, size(320, 180), 200),
            (2, size(160, 90), 100),
        ];
        let mut source = host
            .start_send(SendConfig {
                max: layers[0].1,
                kbps: layers[0].2,
                encodings: layers
                    .iter()
                    .map(|&(id, max, kbps)| Encoding {
                        id: EncodingId(id),
                        max,
                        kbps,
                    })
                    .collect(),
                ..send_config()
            })
            .unwrap();
        let codecs = SoftwareCodecs::default();
        let mut decoders: Vec<_> = (0..3)
            .map(|_| codecs.decoder(VideoCodec::Vp8).unwrap())
            .collect();
        // Frames of the encodings asked for, decoded: their sizes, and
        // whether each was a keyframe.
        async fn take_from(
            source: &mut Box<dyn VideoSource>,
            decoders: &mut [Box<dyn codec::Decoder>],
            n: usize,
        ) -> Vec<(u8, (u32, u32), bool)> {
            let mut out = Vec::new();
            for _ in 0..n {
                let (id, frame) = source.next_frame().await.unwrap();
                let picture = decoders[usize::from(id.0)]
                    .decode(&frame.data)
                    .unwrap()
                    .expect("a picture");
                out.push((id.0, (picture.width, picture.height), frame.keyframe));
            }
            out
        }
        let first = take_from(&mut source, &mut decoders, 9).await;
        for (id, max, _) in layers {
            let of: Vec<_> = first.iter().filter(|f| f.0 == id).collect();
            assert!(of.len() >= 2, "encoding {id} sends: {first:?}");
            assert!(of[0].2, "each starts on a keyframe");
            assert!(of.iter().all(|f| f.1 == (max.width, max.height)), "{of:?}");
        }

        // 320x180 paused: the others go on without it.
        source.set_encoding_target(EncodingId(1), None);
        let paused = take_from(&mut source, &mut decoders, 12).await;
        let after = paused
            .iter()
            .skip_while(|f| f.0 == 1)
            .skip(2)
            .collect::<Vec<_>>();
        assert!(after.iter().all(|f| f.0 != 1), "{paused:?}");
        assert!(after.iter().any(|f| f.0 == 0) && after.iter().any(|f| f.0 == 2));

        // Back, from a keyframe; a keyframe of the smallest alone.
        source.set_encoding_target(
            EncodingId(1),
            Some(EncodingTarget {
                kbps: 200,
                max: layers[1].1,
                fps: 30,
            }),
        );
        let back = take_from(&mut source, &mut decoders, 9).await;
        let again = back.iter().find(|f| f.0 == 1).expect("320x180 again");
        assert!(again.2, "back on a keyframe: {back:?}");
        source.request_keyframe_of(EncodingId(2));
        let keyed = take_from(&mut source, &mut decoders, 9).await;
        assert!(keyed.iter().any(|f| f.0 == 2 && f.2), "{keyed:?}");
        assert!(
            keyed.iter().all(|f| f.0 == 2 || !f.2),
            "only the smallest's: {keyed:?}"
        );
    }

    #[tokio::test]
    async fn a_camera_smaller_than_the_first_layer_scales_every_layer_with_it() {
        // 320x180 under layers asked for at 720p, 360p and 180p.
        let host = DesktopVideoHost::new(
            PatternCamera::new(Resolution {
                width: 320,
                height: 180,
            }),
            SoftwareCodecs::default(),
        );
        let size = |width, height| Resolution { width, height };
        let layers = [
            (0, Resolution::P720),
            (1, size(640, 360)),
            (2, size(320, 180)),
        ];
        let mut source = host
            .start_send(SendConfig {
                encodings: layers
                    .iter()
                    .map(|&(id, max)| Encoding {
                        id: EncodingId(id),
                        max,
                        kbps: 200,
                    })
                    .collect(),
                ..send_config()
            })
            .unwrap();
        let codecs = SoftwareCodecs::default();
        let mut decoders: Vec<_> = (0..3)
            .map(|_| codecs.decoder(VideoCodec::Vp8).unwrap())
            .collect();
        let mut sizes = std::collections::BTreeMap::new();
        for _ in 0..9 {
            let (id, frame) = source.next_frame().await.unwrap();
            let p = decoders[usize::from(id.0)]
                .decode(&frame.data)
                .unwrap()
                .unwrap();
            sizes.insert(id.0, (p.width, p.height));
        }
        assert_eq!(
            sizes,
            [(0, (320, 180)), (1, (160, 90)), (2, (80, 44))].into(),
            "half and a quarter of what the camera gives"
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
