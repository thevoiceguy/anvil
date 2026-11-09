//! A camera captured and encoded on a thread of its own, its frames
//! handed to the core.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anvil_core::video::host::{CodedFrame, EncodingId, Resolution, VideoSource};
use async_trait::async_trait;
use parking_lot::Mutex;
use tokio::sync::mpsc;

use crate::camera::{Cameras, Capture};
use crate::codec::Encoder;
use crate::tiles::{TileKey, Tiles};

/// What the core asks of a running source.
struct Control {
    stop: AtomicBool,
    keyframe: AtomicBool,
    /// (kbps, max, fps), and whether it changed since the encoder saw it.
    target: Mutex<((u32, Resolution, u32), bool)>,
    /// A camera to switch to (`Some(None)`, the default one).
    switch: Mutex<Option<Option<String>>>,
}

/// A running source's way to switch cameras, kept by the host.
#[derive(Clone)]
pub(crate) struct Switch(std::sync::Weak<Control>);

impl Switch {
    pub(crate) fn alive(&self) -> bool {
        self.0.strong_count() > 0
    }

    /// Ask the source to switch to `camera`; false once it has ended.
    pub(crate) fn to(&self, camera: Option<&str>) -> bool {
        match self.0.upgrade() {
            Some(c) => {
                *c.switch.lock() = Some(camera.map(str::to_owned));
                true
            }
            None => false,
        }
    }
}

/// The camera being captured, and what it was opened for.
pub(crate) struct Camera {
    pub(crate) cameras: Arc<dyn Cameras>,
    pub(crate) id: Option<String>,
    pub(crate) capture: Box<dyn Capture>,
    pub(crate) want: Resolution,
    pub(crate) fps: u32,
}

pub(crate) struct CameraSource {
    frames: mpsc::Receiver<(EncodingId, CodedFrame)>,
    control: Arc<Control>,
}

impl CameraSource {
    /// Start capturing: the camera is open, the encoder made. The
    /// self-view shows what is captured.
    pub(crate) fn start(
        camera: Camera,
        encoder: Box<dyn Encoder>,
        target: (u32, Resolution, u32),
        tiles: Tiles,
    ) -> Self {
        let (tx, frames) = mpsc::channel(4);
        let control = Arc::new(Control {
            stop: AtomicBool::new(false),
            keyframe: AtomicBool::new(false),
            target: Mutex::new((target, false)),
            switch: Mutex::new(None),
        });
        let c = Arc::clone(&control);
        let spawned = std::thread::Builder::new()
            .name("anvil-video-send".into())
            .spawn(move || run(camera, encoder, &c, &tx, &tiles));
        if let Err(e) = spawned {
            tracing::warn!("video send thread: {e}");
        }
        Self { frames, control }
    }

    pub(crate) fn switch(&self) -> Switch {
        Switch(Arc::downgrade(&self.control))
    }
}

impl Drop for CameraSource {
    fn drop(&mut self) {
        // The thread ends at its next picture, closing the camera.
        self.control.stop.store(true, Ordering::Relaxed);
    }
}

#[async_trait]
impl VideoSource for CameraSource {
    async fn next_frame(&mut self) -> Option<(EncodingId, CodedFrame)> {
        self.frames.recv().await
    }

    fn request_keyframe(&self) {
        self.control.keyframe.store(true, Ordering::Relaxed);
    }

    fn set_target(&self, kbps: u32, max: Resolution, fps: u32) {
        *self.control.target.lock() = ((kbps, max, fps), true);
    }
}

/// Close the camera and open `id`; if it will not open, the one there was
/// again. `false` when neither opens.
fn switch_camera(camera: &mut Camera, id: Option<String>) -> bool {
    if id == camera.id {
        return true;
    }
    // Closed first: a camera opened twice is busy.
    camera.capture = Box::new(Closed);
    match camera.cameras.open(id.as_deref(), camera.want, camera.fps) {
        Ok(c) => {
            tracing::info!(camera = ?id, "switched camera");
            camera.capture = c;
            camera.id = id;
            true
        }
        Err(e) => {
            tracing::warn!(camera = ?id, "camera would not open: {e}");
            match camera
                .cameras
                .open(camera.id.as_deref(), camera.want, camera.fps)
            {
                Ok(c) => {
                    camera.capture = c;
                    true
                }
                Err(e) => {
                    tracing::warn!(camera = ?camera.id, "camera would not open again: {e}");
                    false
                }
            }
        }
    }
}

/// No camera, between closing one and opening the next.
struct Closed;

impl Capture for Closed {
    fn next(&mut self) -> Option<crate::picture::Picture> {
        None
    }
}

fn run(
    mut camera: Camera,
    mut encoder: Box<dyn Encoder>,
    control: &Control,
    tx: &mpsc::Sender<(EncodingId, CodedFrame)>,
    tiles: &Tiles,
) {
    let mut last_sent: Option<Instant> = None;
    while !control.stop.load(Ordering::Relaxed) {
        let switch = control.switch.lock().take();
        if let Some(id) = switch {
            if !switch_camera(&mut camera, id) {
                break;
            }
            // The far end sees the new camera from a keyframe.
            control.keyframe.store(true, Ordering::Relaxed);
        }
        let Some(picture) = camera.capture.next() else {
            break;
        };
        tiles.present(TileKey::SelfView, picture.clone());

        let ((kbps, max, fps), changed) = {
            let mut t = control.target.lock();
            let v = *t;
            t.1 = false;
            v
        };
        if changed {
            encoder.set_rate(kbps, fps);
        }
        // The frame rate the network allows: a picture early by more
        // than a few ms waits for the next.
        let every = Duration::from_secs(1) / fps.max(1);
        if last_sent.is_some_and(|t| {
            picture.captured.saturating_duration_since(t) + Duration::from_millis(5) < every
        }) {
            continue;
        }
        let (w, h) = picture.fit(max);
        let picture = picture.scaled(w, h);
        let keyframe = control.keyframe.swap(false, Ordering::Relaxed);
        match encoder.encode(&picture, keyframe) {
            Ok(Some(encoded)) => {
                last_sent = Some(picture.captured);
                let frame = CodedFrame {
                    data: encoded.data.into(),
                    keyframe: encoded.keyframe,
                    captured: picture.captured,
                };
                if tx.blocking_send((EncodingId(0), frame)).is_err() {
                    break;
                }
            }
            Ok(None) => {
                if keyframe {
                    control.keyframe.store(true, Ordering::Relaxed);
                }
            }
            Err(e) => {
                tracing::warn!("video encoder: {e}");
                control.keyframe.store(true, Ordering::Relaxed);
            }
        }
    }
    tiles.end(TileKey::SelfView);
    tracing::debug!("camera closed");
}
