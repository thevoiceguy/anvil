//! Video for the app (FCP's `docs/SOFTPHONE_VIDEO.md` §2.5): the camera
//! on or off in a call, the camera chosen, H.264 from Cisco's OpenH264,
//! and each picture put on a Flutter texture.
//!
//! Pictures never cross into Dart. The runner's texture plugin (Linux:
//! `linux/runner/video_texture.cc`) makes a texture and hands Dart its id
//! with a C function and its argument; Dart hands those here
//! ([`attach_tile`]), and the video host's threads call the function with
//! each picture, as RGBA, at the moment it is to be seen.

use anvil_app::Command;
use flutter_rust_bridge::frb;

use super::phone::run_command;

/// Our camera on or off in a call (the connected one when `None`).
pub async fn set_video(call: Option<u64>, on: bool) -> anyhow::Result<()> {
    run_command(Command::Video { call, on }).await.map(|_| ())
}

/// The camera video calls use, by id; `None` is the system's first.
pub async fn choose_camera(camera: Option<String>) -> anyhow::Result<()> {
    run_command(Command::Camera { camera }).await.map(|_| ())
}

/// Allow H.264 from Cisco's OpenH264 (fetched now if need be), or not.
pub async fn allow_h264(on: bool) -> anyhow::Result<()> {
    run_command(Command::H264 { on }).await.map(|_| ())
}

/// Show a tile on a texture: an incoming stream by id, or (`None`) the
/// self-view. `present` is the texture plugin's
/// `void present(void* texture, const uint8_t* rgba, uint32_t width,
/// uint32_t height)` and `texture` its argument; it is called on the
/// video host's threads, never after [`detach_tile`] returns. The
/// attachment's id.
#[frb(sync)]
pub fn attach_tile(stream: Option<u32>, present: usize, texture: usize) -> anyhow::Result<u64> {
    desktop::attach(stream, present, texture)
}

/// What a tile shows: its picture's size, and whether it is frozen (the
/// picture is broken until a keyframe).
pub struct TileInfo {
    pub width: u32,
    pub height: u32,
    pub frozen: bool,
}

/// A tile's picture, once it has one.
#[frb(sync)]
pub fn tile_info(stream: Option<u32>) -> Option<TileInfo> {
    desktop::info(stream)
}

/// Stop showing an attachment; its function is not called again.
#[frb(sync)]
pub fn detach_tile(attachment: u64) {
    desktop::detach(attachment);
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub(crate) mod desktop {
    use super::*;

    use std::collections::HashMap;
    use std::ffi::c_void;
    use std::future::Future;
    use std::path::{Path, PathBuf};
    use std::pin::Pin;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, OnceLock};

    use anvil_video::{openh264_fetch, DesktopVideoHost, Picture, Renderer, TileKey};
    use parking_lot::Mutex;

    type Present = unsafe extern "C" fn(*mut c_void, *const u8, u32, u32);

    /// The video host: one for the app's life, its tiles kept across the
    /// phone signing out and in.
    static HOST: OnceLock<DesktopVideoHost> = OnceLock::new();

    pub(crate) fn host(openh264_dir: &Path) -> DesktopVideoHost {
        HOST.get_or_init(|| DesktopVideoHost::platform(openh264_dir))
            .clone()
    }

    /// A host of the test's own, where none was made yet.
    #[cfg(test)]
    pub(crate) fn install(host: DesktopVideoHost) -> bool {
        HOST.set(host).is_ok()
    }

    /// Made by `start_phone`, before any call can show a tile.
    fn running() -> anyhow::Result<DesktopVideoHost> {
        HOST.get()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("the phone is not running"))
    }

    /// Where OpenH264 is kept: beside the session.
    pub(crate) fn openh264_dir(session_path: &str) -> PathBuf {
        Path::new(session_path).with_file_name("openh264")
    }

    /// OpenH264 for the phone, where Cisco publishes a build for here.
    pub(crate) fn openh264(
        host: DesktopVideoHost,
        dir: PathBuf,
    ) -> Option<Arc<dyn anvil_app::OpenH264>> {
        openh264_fetch::binary()?;
        Some(Arc::new(Fetched { host, dir }))
    }

    struct Fetched {
        host: DesktopVideoHost,
        dir: PathBuf,
    }

    impl anvil_app::OpenH264 for Fetched {
        fn notice(&self) -> String {
            openh264_fetch::NOTICE.into()
        }
        fn ready(&self) -> bool {
            self.host.has_h264()
        }
        fn enable(&self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> {
            let (host, dir) = (self.host.clone(), self.dir.clone());
            Box::pin(async move { host.fetch_openh264(&dir).await.map(|_| ()) })
        }
        fn disable(&self) {
            self.host.drop_openh264();
        }
    }

    /// A texture a tile is shown on. `buffer` is `None` once detached: the
    /// lock held while presenting is what makes "never after detach" so.
    struct Texture {
        present: Present,
        texture: usize,
        buffer: Mutex<Option<Vec<u8>>>,
    }

    impl Renderer for Texture {
        fn present(&self, picture: &Picture) {
            let mut buffer = self.buffer.lock();
            let Some(rgba) = buffer.as_mut() else {
                return;
            };
            picture.write_rgba(rgba);
            // SAFETY: the plugin's function, with the texture it gave for
            // it, called only until it is detached (the texture plugin
            // disposes of the texture after detaching), with a buffer of
            // width × height × 4 bytes that outlives the call.
            unsafe {
                (self.present)(
                    self.texture as *mut c_void,
                    rgba.as_ptr(),
                    picture.width,
                    picture.height,
                )
            };
        }
    }

    /// Each attachment by id: the tile and the texture it shows on.
    type Attached = HashMap<u64, (TileKey, Arc<Texture>)>;
    static ATTACHED: Mutex<Option<Attached>> = Mutex::new(None);
    static NEXT: AtomicU64 = AtomicU64::new(1);

    fn key(stream: Option<u32>) -> TileKey {
        match stream {
            Some(s) => TileKey::Stream(anvil_core::video::host::StreamId(s)),
            None => TileKey::SelfView,
        }
    }

    pub(crate) fn attach(
        stream: Option<u32>,
        present: usize,
        texture: usize,
    ) -> anyhow::Result<u64> {
        anyhow::ensure!(present != 0 && texture != 0, "no texture");
        let host = running()?;
        // SAFETY: Dart passes the address of the plugin's `present`, of
        // the type documented on `attach_tile`.
        let present: Present = unsafe { std::mem::transmute::<usize, Present>(present) };
        let key = key(stream);
        let renderer = Arc::new(Texture {
            present,
            texture,
            buffer: Mutex::new(Some(Vec::new())),
        });
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        ATTACHED
            .lock()
            .get_or_insert_with(HashMap::new)
            .insert(id, (key, Arc::clone(&renderer)));
        host.tiles().attach(key, renderer);
        Ok(id)
    }

    pub(crate) fn info(stream: Option<u32>) -> Option<TileInfo> {
        let stats = running().ok()?.tiles().stats(key(stream))?;
        (stats.presented > 0).then_some(TileInfo {
            width: stats.width,
            height: stats.height,
            frozen: stats.frozen,
        })
    }

    pub(crate) fn detach(attachment: u64) {
        let mut attached = ATTACHED.lock();
        let Some(map) = attached.as_mut() else {
            return;
        };
        let Some((key, texture)) = map.remove(&attachment) else {
            return;
        };
        // Not called again, from here on.
        *texture.buffer.lock() = None;
        // The tile is let go unless a newer attachment shows it.
        if !map.values().any(|(k, _)| *k == key) {
            if let Ok(host) = running() {
                host.tiles().detach(key);
            }
        }
    }
}

#[cfg(any(target_os = "android", target_os = "ios"))]
pub(crate) mod desktop {
    pub(crate) fn attach(_: Option<u32>, _: usize, _: usize) -> anyhow::Result<u64> {
        anyhow::bail!("no video on this platform yet")
    }
    pub(crate) fn detach(_: u64) {}
    pub(crate) fn info(_: Option<u32>) -> Option<super::TileInfo> {
        None
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    use std::ffi::c_void;
    use std::time::{Duration, Instant};

    use anvil_core::video::host::{
        EncodingId, Resolution, SendConfig, VideoCodec, VideoHost, VideoKind,
    };
    use anvil_video::{DesktopVideoHost, PatternCamera, SoftwareCodecs};
    use parking_lot::Mutex;

    /// What the texture plugin was given: each picture's size and first
    /// pixel.
    type Seen = Mutex<Vec<(u32, u32, [u8; 4])>>;

    unsafe extern "C" fn record(texture: *mut c_void, rgba: *const u8, width: u32, height: u32) {
        // SAFETY: `texture` is the `Seen` the test attached with, and
        // `rgba` holds width × height × 4 bytes.
        let seen = unsafe { &*(texture as *const Seen) };
        let all = unsafe { std::slice::from_raw_parts(rgba, (width * height * 4) as usize) };
        seen.lock()
            .push((width, height, [all[0], all[1], all[2], all[3]]));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_self_view_reaches_the_texture_as_rgba_and_nothing_after_detaching() {
        let host = DesktopVideoHost::new(
            PatternCamera::new(Resolution {
                width: 320,
                height: 180,
            }),
            SoftwareCodecs::default(),
        );
        assert!(desktop::install(host.clone()), "the first host");
        let seen: &'static Seen = Box::leak(Box::default());
        let present = record as unsafe extern "C" fn(*mut c_void, *const u8, u32, u32) as usize;
        let attachment = attach_tile(None, present, seen as *const Seen as usize).unwrap();

        let mut source = host
            .start_send(SendConfig {
                codec: VideoCodec::Vp8,
                kind: VideoKind::Camera,
                camera: None,
                max: Resolution::P720,
                kbps: 300,
                fps: 30,
                encodings: vec![EncodingId(0)],
            })
            .unwrap();
        let drain = tokio::spawn(async move { while source.next_frame().await.is_some() {} });
        let deadline = Instant::now() + Duration::from_secs(5);
        while seen.lock().len() < 5 {
            assert!(Instant::now() < deadline, "pictures on the texture");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let (w, h, pixel) = seen.lock()[0];
        assert_eq!((w, h), (320, 180));
        assert_eq!(pixel[3], 255, "RGBA, opaque");
        let info = tile_info(None).expect("the self-view's size");
        assert_eq!((info.width, info.height, info.frozen), (320, 180, false));

        detach_tile(attachment);
        let after = seen.lock().len();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(seen.lock().len(), after, "nothing after detaching");
        drain.abort();
    }
}
