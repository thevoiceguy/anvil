//! A camera captured and encoded on a thread of its own, its frames
//! handed to the core.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anvil_core::video::host::{CodedFrame, EncodingId, Resolution, VideoSource};
use async_trait::async_trait;
use parking_lot::Mutex;
use tokio::sync::mpsc;

use crate::camera::Capture;
use crate::codec::Encoder;
use crate::tiles::{TileKey, Tiles};

/// What the core asks of a running source.
struct Control {
    stop: AtomicBool,
    keyframe: AtomicBool,
    /// (kbps, max, fps), and whether it changed since the encoder saw it.
    target: Mutex<((u32, Resolution, u32), bool)>,
}

pub(crate) struct CameraSource {
    frames: mpsc::Receiver<(EncodingId, CodedFrame)>,
    control: Arc<Control>,
}

impl CameraSource {
    /// Start capturing: the camera is open, the encoder made. The
    /// self-view shows what is captured.
    pub(crate) fn start(
        capture: Box<dyn Capture>,
        encoder: Box<dyn Encoder>,
        target: (u32, Resolution, u32),
        tiles: Tiles,
    ) -> Self {
        let (tx, frames) = mpsc::channel(4);
        let control = Arc::new(Control {
            stop: AtomicBool::new(false),
            keyframe: AtomicBool::new(false),
            target: Mutex::new((target, false)),
        });
        let c = Arc::clone(&control);
        let spawned = std::thread::Builder::new()
            .name("anvil-video-send".into())
            .spawn(move || run(capture, encoder, &c, &tx, &tiles));
        if let Err(e) = spawned {
            tracing::warn!("video send thread: {e}");
        }
        Self { frames, control }
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

fn run(
    mut capture: Box<dyn Capture>,
    mut encoder: Box<dyn Encoder>,
    control: &Control,
    tx: &mpsc::Sender<(EncodingId, CodedFrame)>,
    tiles: &Tiles,
) {
    let mut last_sent: Option<Instant> = None;
    while !control.stop.load(Ordering::Relaxed) {
        let Some(picture) = capture.next() else {
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
