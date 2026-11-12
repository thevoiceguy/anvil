//! iOS: the phone's screen, shared through a ReplayKit broadcast upload
//! extension. iOS captures the whole screen only in an extension (its own
//! process, started by the user from the system's broadcast picker), so
//! the extension (`app/ios/ScreenShare/SampleHandler.swift`) sends each
//! picture to the app over a Unix socket in the app group's shared
//! container, where a share listens.
//!
//! Each picture is a 32-byte header — `ANVS`, a version (1), the turn
//! that makes it upright (degrees clockwise), width, height, the two
//! planes' row strides, the format (NV12, video or full range) — then the
//! planes. ReplayKit sends a picture only when the screen changes, so a
//! still screen repeats its last one at the share's rate. The share ends
//! when the extension goes (the user stopped the broadcast) or never came
//! within a minute (the picker was dismissed); stopping the share closes
//! the socket, and the extension ends its broadcast.

use std::io::{ErrorKind, Read};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anvil_core::video::host::{Resolution, ScreenInfo};
use parking_lot::{Condvar, Mutex};

use crate::camera::Capture;
use crate::picture::Picture;
use crate::screen::Screens;

/// The socket's name in the shared container.
pub const SOCKET: &str = "anvil-share.sock";
/// The Info.plist key naming the app group the app shares with its
/// extension.
const GROUP_KEY: &str = "AnvilAppGroup";
/// How long a share waits for the user to start the broadcast.
const WAIT_FOR_EXTENSION: Duration = Duration::from_secs(60);

const MAGIC: &[u8; 4] = b"ANVS";
const VERSION: u16 = 1;
/// NV12 with Y in 16–235 (`420v`).
pub const NV12_VIDEO: u32 = 1;
/// NV12 with Y in 0–255 (`420f`, what ReplayKit sends).
pub const NV12_FULL: u32 = 2;

/// The app group's shared container, from the app's Info.plist.
#[cfg(target_os = "ios")]
fn shared_container() -> Option<PathBuf> {
    use objc2_foundation::{NSBundle, NSFileManager, NSString};
    let bundle = NSBundle::mainBundle();
    let group = bundle.objectForInfoDictionaryKey(&NSString::from_str(GROUP_KEY))?;
    let group = group.downcast::<NSString>().ok()?;
    let url = NSFileManager::defaultManager()
        .containerURLForSecurityApplicationGroupIdentifier(&group)?;
    Some(PathBuf::from(url.path()?.to_string()))
}

#[cfg(not(target_os = "ios"))]
fn shared_container() -> Option<PathBuf> {
    let _ = GROUP_KEY;
    None
}

/// The phone's screen, while the app has a container to share it through.
#[derive(Debug, Clone, Default)]
pub struct BroadcastScreens {
    dir: Option<PathBuf>,
}

impl BroadcastScreens {
    /// Through the app group the app's Info.plist names.
    pub fn platform() -> Self {
        Self {
            dir: shared_container(),
        }
    }

    /// Through `dir` (tests).
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: Some(dir.into()),
        }
    }
}

const SCREEN: &str = "screen";

impl Screens for BroadcastScreens {
    fn list(&self) -> Vec<ScreenInfo> {
        if self.dir.is_none() {
            return Vec::new();
        }
        vec![ScreenInfo {
            id: SCREEN.into(),
            name: "This phone's screen".into(),
        }]
    }

    fn open(
        &self,
        id: Option<&str>,
        _want: Resolution,
        fps: u32,
    ) -> Result<Box<dyn Capture>, String> {
        if id.is_some_and(|id| id != SCREEN) {
            return Err(format!("no screen {id:?}"));
        }
        let dir = self.dir.as_deref().ok_or("no screen to share")?;
        Ok(Box::new(Broadcast::listen(&dir.join(SOCKET), fps)?))
    }
}

/// What the reader thread hands the share.
#[derive(Default)]
struct Latest {
    picture: Mutex<(Option<Picture>, bool)>,
    cv: Condvar,
}

/// A share running: a thread accepting the extension and reading its
/// pictures.
struct Broadcast {
    path: PathBuf,
    latest: Arc<Latest>,
    stop: Arc<AtomicBool>,
    stream: Arc<Mutex<Option<UnixStream>>>,
    every: Duration,
    next_at: Instant,
    last: Option<Picture>,
}

impl Broadcast {
    fn listen(path: &Path, fps: u32) -> Result<Self, String> {
        // A socket left by a share before is no one's now.
        let _ = std::fs::remove_file(path);
        let listener = UnixListener::bind(path).map_err(|e| format!("the share's socket: {e}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|e| format!("the share's socket: {e}"))?;
        let latest = Arc::new(Latest::default());
        let stop = Arc::new(AtomicBool::new(false));
        let stream = Arc::new(Mutex::new(None));
        let (l, s, st) = (latest.clone(), stop.clone(), stream.clone());
        std::thread::Builder::new()
            .name("anvil-share-read".into())
            .spawn(move || {
                read(listener, &l, &s, &st);
                l.picture.lock().1 = true;
                l.cv.notify_all();
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            path: path.to_owned(),
            latest,
            stop,
            stream,
            every: Duration::from_secs(1) / fps.clamp(1, 30),
            next_at: Instant::now(),
            last: None,
        })
    }
}

/// Accept the extension (within [`WAIT_FOR_EXTENSION`]), then read its
/// pictures until it goes or the share stops.
fn read(
    listener: UnixListener,
    latest: &Latest,
    stop: &AtomicBool,
    keep: &Mutex<Option<UnixStream>>,
) {
    let until = Instant::now() + WAIT_FOR_EXTENSION;
    let mut stream = loop {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                if Instant::now() >= until {
                    tracing::info!("the broadcast never started");
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                tracing::warn!("the share's socket: {e}");
                return;
            }
        }
    };
    if stream.set_nonblocking(false).is_err() {
        return;
    }
    if let Ok(clone) = stream.try_clone() {
        *keep.lock() = Some(clone);
    }
    if stop.load(Ordering::SeqCst) {
        return;
    }
    tracing::info!("the broadcast started");
    loop {
        match next_picture(&mut stream) {
            Ok(Some(picture)) => {
                latest.picture.lock().0 = Some(picture);
                latest.cv.notify_all();
            }
            Ok(None) => continue,
            Err(e) => {
                if !stop.load(Ordering::SeqCst) {
                    tracing::info!("the broadcast ended: {e}");
                }
                return;
            }
        }
    }
}

/// The next picture on `from`; `None` for one that cannot be shown
/// (skipped), an error when the stream ends or makes no sense.
pub fn next_picture(from: &mut impl Read) -> std::io::Result<Option<Picture>> {
    let mut header = [0u8; 32];
    from.read_exact(&mut header)?;
    let u16_at = |i: usize| u16::from_le_bytes([header[i], header[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes(header[i..i + 4].try_into().unwrap_or_default());
    if &header[..4] != MAGIC || u16_at(4) != VERSION {
        return Err(std::io::Error::new(ErrorKind::InvalidData, "not a picture"));
    }
    let turn = u32::from(u16_at(6));
    let (width, height) = (u32_at(8), u32_at(12));
    let (ys, uvs) = (u32_at(16) as usize, u32_at(20) as usize);
    let format = u32_at(24);
    if width == 0 || height == 0 || width > 8192 || height > 8192 {
        return Err(std::io::Error::new(
            ErrorKind::InvalidData,
            "a picture's size",
        ));
    }
    let (w, h) = (width as usize, height as usize);
    let chroma_rows = h.div_ceil(2);
    if ys < w || uvs < w.div_ceil(2) * 2 || ys > 4 * 8192 || uvs > 4 * 8192 {
        return Err(std::io::Error::new(
            ErrorKind::InvalidData,
            "a picture's strides",
        ));
    }
    let mut y = vec![0u8; ys * h];
    let mut uv = vec![0u8; uvs * chroma_rows];
    from.read_exact(&mut y)?;
    from.read_exact(&mut uv)?;
    let (ew, eh) = (width & !1, height & !1);
    let mut picture = match format {
        NV12_VIDEO | NV12_FULL => Picture::from_nv12(&y, ys, &uv, uvs, ew.max(2), eh.max(2)),
        _ => return Ok(None),
    };
    if format == NV12_FULL {
        to_video_range(&mut picture);
    }
    if turn % 360 != 0 {
        picture = picture.rotated(turn % 360);
    }
    Ok(Some(picture))
}

/// Full-range samples (0–255) brought to video range (Y 16–235, chroma
/// 16–240), which the rest of Anvil assumes.
fn to_video_range(picture: &mut Picture) {
    let (y, u, v) = picture.planes_mut();
    for s in y {
        *s = (16 + u32::from(*s) * 219 / 255) as u8;
    }
    for s in u.iter_mut().chain(v.iter_mut()) {
        *s = (128 + (i32::from(*s) - 128) * 224 / 255) as u8;
    }
}

impl Capture for Broadcast {
    fn next(&mut self) -> Option<Picture> {
        loop {
            {
                let mut latest = self.latest.picture.lock();
                if latest.0.is_none() && !latest.1 {
                    let _ = self.latest.cv.wait_until(&mut latest, self.next_at);
                }
                if let Some(mut picture) = latest.0.take() {
                    drop(latest);
                    self.next_at = Instant::now() + self.every;
                    picture.captured = Instant::now();
                    self.last = Some(picture.clone());
                    return Some(picture);
                }
                if latest.1 {
                    return None;
                }
            }
            if Instant::now() < self.next_at {
                continue;
            }
            self.next_at = Instant::now() + self.every;
            // The screen is still: its last picture again (none yet: wait).
            if let Some(last) = &self.last {
                let mut picture = last.clone();
                picture.captured = Instant::now();
                return Some(picture);
            }
        }
    }
}

impl Drop for Broadcast {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // The extension sees the socket close and ends its broadcast.
        if let Some(stream) = self.stream.lock().take() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A picture as the extension writes it.
    fn frame(width: u32, height: u32, turn: u16, format: u32, y: u8) -> Vec<u8> {
        let (ys, uvs) = (width as usize + 16, width as usize + 16);
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&turn.to_le_bytes());
        for v in [width, height, ys as u32, uvs as u32, format, 0] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend(std::iter::repeat_n(y, ys * height as usize));
        out.extend(std::iter::repeat_n(
            128,
            uvs * (height as usize).div_ceil(2),
        ));
        out
    }

    #[test]
    fn a_picture_is_read_turned_upright_and_in_video_range() {
        let mut bytes = frame(64, 32, 90, NV12_FULL, 255);
        bytes.extend(frame(64, 32, 0, NV12_VIDEO, 100));
        let mut from = std::io::Cursor::new(bytes);
        let first = next_picture(&mut from).unwrap().unwrap();
        assert_eq!((first.width, first.height), (32, 64), "turned upright");
        assert!(first.y().iter().all(|&y| y == 235), "full range to video");
        let second = next_picture(&mut from).unwrap().unwrap();
        assert_eq!((second.width, second.height), (64, 32));
        assert!(second.y().iter().all(|&y| y == 100), "video range as it is");
        assert!(next_picture(&mut from).is_err(), "the end");
        assert!(next_picture(&mut std::io::Cursor::new(
            b"nonsense, not a picture".repeat(2)
        ))
        .is_err());
    }

    #[test]
    fn a_share_reads_the_extension_and_ends_when_it_goes() {
        let dir = std::env::temp_dir().join(format!("anvil-share-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let screens = BroadcastScreens::at(&dir);
        assert_eq!(screens.list().len(), 1);
        let mut share = screens
            .open(
                None,
                Resolution {
                    width: 1920,
                    height: 1080,
                },
                30,
            )
            .unwrap();
        let mut extension = UnixStream::connect(dir.join(SOCKET)).unwrap();
        extension
            .write_all(&frame(64, 32, 0, NV12_VIDEO, 50))
            .unwrap();
        let first = share.next().expect("a picture");
        assert!(first.y().iter().all(|&y| y == 50));
        // Nothing new: the last again.
        let again = share.next().expect("the last again");
        assert_eq!(again.y(), first.y());
        // The extension goes: the share ends.
        drop(extension);
        let until = Instant::now() + Duration::from_secs(5);
        while share.next().is_some() {
            assert!(Instant::now() < until, "the share ends");
        }
        drop(share);
        assert!(!dir.join(SOCKET).exists(), "the socket is removed");
    }
}
