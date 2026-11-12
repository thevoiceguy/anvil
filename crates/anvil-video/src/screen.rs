//! Screens: what can be shared, and pictures of one (FCP's
//! `docs/SOFTPHONE_VIDEO.md` §2.6). A screen is captured as a camera is
//! — a [`Capture`] of pictures — and sent through the same source, so a
//! share switches screens as a call switches cameras.

use anvil_core::video::host::{CameraInfo, Resolution, ScreenInfo};

use crate::camera::{Cameras, Capture, PatternCamera};

/// A platform's screens.
pub trait Screens: Send + Sync {
    fn list(&self) -> Vec<ScreenInfo>;
    /// Start capturing a screen (`None`, the main one), at most `want`
    /// and `fps`. Dropping the capture stops it.
    fn open(
        &self,
        id: Option<&str>,
        want: Resolution,
        fps: u32,
    ) -> Result<Box<dyn Capture>, String>;
}

impl Screens for Box<dyn Screens> {
    fn list(&self) -> Vec<ScreenInfo> {
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

/// Nothing to share.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoScreens;

impl Screens for NoScreens {
    fn list(&self) -> Vec<ScreenInfo> {
        Vec::new()
    }
    fn open(&self, _: Option<&str>, _: Resolution, _: u32) -> Result<Box<dyn Capture>, String> {
        Err("no screen to share".into())
    }
}

/// A screen showing the moving test pattern: for machines without a
/// display, and for tests that need real pictures through real codecs.
#[derive(Debug, Clone, Copy)]
pub struct PatternScreen {
    pattern: PatternCamera,
}

impl Default for PatternScreen {
    fn default() -> Self {
        Self::new(Resolution {
            width: 1280,
            height: 720,
        })
    }
}

impl PatternScreen {
    pub fn new(size: Resolution) -> Self {
        Self {
            pattern: PatternCamera::new(size),
        }
    }
}

impl Screens for PatternScreen {
    fn list(&self) -> Vec<ScreenInfo> {
        vec![ScreenInfo {
            id: "pattern".into(),
            name: "Test pattern".into(),
        }]
    }
    fn open(
        &self,
        id: Option<&str>,
        want: Resolution,
        fps: u32,
    ) -> Result<Box<dyn Capture>, String> {
        self.pattern.open(id, want, fps)
    }
}

/// Screens seen as cameras, for the source that sends either.
pub(crate) struct AsCameras(pub std::sync::Arc<dyn Screens>);

impl Cameras for AsCameras {
    fn list(&self) -> Vec<CameraInfo> {
        self.0
            .list()
            .into_iter()
            .map(|s| CameraInfo {
                id: s.id,
                name: s.name,
            })
            .collect()
    }
    fn open(
        &self,
        id: Option<&str>,
        want: Resolution,
        fps: u32,
    ) -> Result<Box<dyn Capture>, String> {
        self.0.open(id, want, fps)
    }
}
