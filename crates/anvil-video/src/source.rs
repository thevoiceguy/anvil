//! A camera captured and encoded on a thread of its own, its frames
//! handed to the core: once, or (simulcast) by an encoder per encoding,
//! each picture scaled to the encoding's size.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anvil_core::video::host::{CodedFrame, EncodingId, EncodingTarget, Resolution, VideoSource};
use async_trait::async_trait;
use parking_lot::Mutex;
use tokio::sync::mpsc;

use crate::camera::{Cameras, Capture};
use crate::codec::Encoder;
use crate::tiles::{TileKey, Tiles};

/// An encoding's (kbps, max, fps).
type Target = (u32, Resolution, u32);

/// What the core asks of one encoding.
struct Layer {
    id: EncodingId,
    /// Its height against the first encoding's, as asked: a camera
    /// smaller than the first's size scales every layer down with it, so
    /// the layers stay apart.
    scale: f64,
    keyframe: AtomicBool,
    /// (kbps, max, fps), `None` paused; and whether it changed since the
    /// encoder saw it.
    target: Mutex<(Option<Target>, bool)>,
}

/// What the core asks of a running source.
struct Control {
    stop: AtomicBool,
    /// Largest first; the first is the one a source without simulcast has.
    layers: Vec<Layer>,
    /// The first encoding's size, as asked.
    first: Resolution,
    /// A camera to switch to (`Some(None)`, the default one).
    switch: Mutex<Option<Option<String>>>,
}

impl Control {
    fn layer(&self, id: EncodingId) -> Option<&Layer> {
        self.layers.iter().find(|l| l.id == id)
    }

    fn keyframe_all(&self) {
        for l in &self.layers {
            l.keyframe.store(true, Ordering::Relaxed);
        }
    }
}

/// One encoding to make: its id, its encoder and what it starts at.
pub(crate) struct Encoding {
    pub(crate) id: EncodingId,
    pub(crate) encoder: Box<dyn Encoder>,
    pub(crate) target: Target,
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
    /// Where what is captured is shown: the self-view, or our shared
    /// screen's.
    pub(crate) view: TileKey,
}

pub(crate) struct CameraSource {
    frames: mpsc::Receiver<(EncodingId, CodedFrame)>,
    control: Arc<Control>,
}

impl CameraSource {
    /// Start capturing: the camera is open, an encoder made for each
    /// encoding (largest first). The self-view shows what is captured.
    pub(crate) fn start(camera: Camera, encodings: Vec<Encoding>, tiles: Tiles) -> Self {
        // A frame of each encoding per picture may wait.
        let (tx, frames) = mpsc::channel(4 * encodings.len().max(1));
        let first = encodings.first().map_or(camera.want, |e| e.target.1);
        let control = Arc::new(Control {
            stop: AtomicBool::new(false),
            first,
            layers: encodings
                .iter()
                .map(|e| Layer {
                    id: e.id,
                    scale: f64::from(e.target.1.height.min(e.target.1.width))
                        / f64::from(first.height.min(first.width).max(1)),
                    keyframe: AtomicBool::new(false),
                    target: Mutex::new((Some(e.target), false)),
                })
                .collect(),
            switch: Mutex::new(None),
        });
        let encoders = encodings.into_iter().map(|e| e.encoder).collect();
        let c = Arc::clone(&control);
        let spawned = std::thread::Builder::new()
            .name("anvil-video-send".into())
            .spawn(move || run(camera, encoders, &c, &tx, &tiles));
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
        self.control.keyframe_all();
    }

    fn set_target(&self, kbps: u32, max: Resolution, fps: u32) {
        if let Some(l) = self.control.layers.first() {
            *l.target.lock() = (Some((kbps, max, fps)), true);
        }
    }

    fn request_keyframe_of(&self, encoding: EncodingId) {
        if let Some(l) = self.control.layer(encoding) {
            l.keyframe.store(true, Ordering::Relaxed);
        }
    }

    fn set_encoding_target(&self, encoding: EncodingId, target: Option<EncodingTarget>) {
        if let Some(l) = self.control.layer(encoding) {
            let mut t = l.target.lock();
            // Back from a pause: from a keyframe.
            if t.0.is_none() && target.is_some() {
                l.keyframe.store(true, Ordering::Relaxed);
            }
            *t = (target.map(|t| (t.kbps, t.max, t.fps)), true);
        }
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
    mut encoders: Vec<Box<dyn Encoder>>,
    control: &Control,
    tx: &mpsc::Sender<(EncodingId, CodedFrame)>,
    tiles: &Tiles,
) {
    // When each encoding last sent a frame.
    let mut last_sent: Vec<Option<Instant>> = vec![None; encoders.len()];
    'capture: while !control.stop.load(Ordering::Relaxed) {
        let switch = control.switch.lock().take();
        if let Some(id) = switch {
            if !switch_camera(&mut camera, id) {
                break;
            }
            // The far end sees the new camera from a keyframe.
            control.keyframe_all();
        }
        let Some(picture) = camera.capture.next() else {
            break;
        };
        tiles.present(camera.view, picture.clone());
        // The first encoding's size for this picture; the others scale
        // from it.
        let (w0, h0) = picture.fit(control.first);

        for ((layer, encoder), last_sent) in control
            .layers
            .iter()
            .zip(encoders.iter_mut())
            .zip(last_sent.iter_mut())
        {
            let (target, changed) = {
                let mut t = layer.target.lock();
                let v = *t;
                t.1 = false;
                v
            };
            // Paused: nothing coded.
            let Some((kbps, max, fps)) = target else {
                continue;
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
            let (w, h) = if layer.scale < 1.0 {
                let own = Resolution {
                    width: (f64::from(w0) * layer.scale) as u32,
                    height: (f64::from(h0) * layer.scale) as u32,
                };
                picture.fit(if own.width * own.height < max.width * max.height {
                    own
                } else {
                    max
                })
            } else {
                picture.fit(max)
            };
            let scaled = picture.scaled(w, h);
            let keyframe = layer.keyframe.swap(false, Ordering::Relaxed);
            match encoder.encode(&scaled, keyframe) {
                Ok(Some(encoded)) => {
                    *last_sent = Some(scaled.captured);
                    let frame = CodedFrame {
                        data: encoded.data.into(),
                        keyframe: encoded.keyframe,
                        captured: scaled.captured,
                    };
                    if tx.blocking_send((layer.id, frame)).is_err() {
                        break 'capture;
                    }
                }
                Ok(None) => {
                    if keyframe {
                        layer.keyframe.store(true, Ordering::Relaxed);
                    }
                }
                Err(e) => {
                    tracing::warn!(encoding = layer.id.0, "video encoder: {e}");
                    layer.keyframe.store(true, Ordering::Relaxed);
                }
            }
        }
    }
    tiles.end(camera.view);
    tracing::debug!("camera closed");
}
