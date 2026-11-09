//! Cameras: what there is, and pictures from one.

use std::time::{Duration, Instant};

use anvil_core::video::host::{CameraInfo, Resolution};

use crate::picture::Picture;

/// A platform's cameras.
pub trait Cameras: Send + Sync {
    fn list(&self) -> Vec<CameraInfo>;
    /// Open a camera (`None`, the first), as near `want` and `fps` as it
    /// goes. Dropping the capture closes it (and its light).
    fn open(
        &self,
        id: Option<&str>,
        want: Resolution,
        fps: u32,
    ) -> Result<Box<dyn Capture>, String>;
}

/// An open camera.
pub trait Capture: Send {
    /// The next picture, waiting for it; `None` when the camera is gone.
    fn next(&mut self) -> Option<Picture>;
}

/// A camera that films a moving test pattern: for machines without one,
/// and for tests that need real pictures through real codecs.
#[derive(Debug, Clone, Copy)]
pub struct PatternCamera {
    size: Resolution,
}

impl Default for PatternCamera {
    fn default() -> Self {
        Self {
            size: Resolution {
                width: 640,
                height: 360,
            },
        }
    }
}

impl PatternCamera {
    pub fn new(size: Resolution) -> Self {
        Self { size }
    }
}

impl Cameras for PatternCamera {
    fn list(&self) -> Vec<CameraInfo> {
        vec![CameraInfo {
            id: "pattern".into(),
            name: "Test pattern".into(),
        }]
    }

    fn open(
        &self,
        _: Option<&str>,
        want: Resolution,
        fps: u32,
    ) -> Result<Box<dyn Capture>, String> {
        Ok(Box::new(PatternCapture {
            width: self.size.width.min(want.width) & !1,
            height: self.size.height.min(want.height) & !1,
            every: Duration::from_secs(1) / fps.clamp(1, 60),
            next_at: Instant::now(),
            n: 0,
        }))
    }
}

struct PatternCapture {
    width: u32,
    height: u32,
    every: Duration,
    next_at: Instant,
    n: u32,
}

impl Capture for PatternCapture {
    fn next(&mut self) -> Option<Picture> {
        let now = Instant::now();
        if self.next_at > now {
            std::thread::sleep(self.next_at - now);
        }
        // A capture that fell behind does not try to catch up.
        self.next_at = (self.next_at + self.every).max(Instant::now());
        let mut p = Picture::pattern(self.width, self.height, self.n);
        p.captured = Instant::now();
        self.n += 1;
        Some(p)
    }
}

/// No camera at all.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoCameras;

impl Cameras for NoCameras {
    fn list(&self) -> Vec<CameraInfo> {
        Vec::new()
    }
    fn open(&self, _: Option<&str>, _: Resolution, _: u32) -> Result<Box<dyn Capture>, String> {
        Err("no camera".into())
    }
}
