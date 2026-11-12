//! Android: the phone's screen, shared through MediaProjection. The app
//! asks the user (the system's consent dialog) before each share and holds
//! the projection in a foreground service, as Android 14 requires; here a
//! share's pictures come from an [`aimage::Reader`] the projection's
//! virtual display draws into. What only Java can do — the projection and
//! its display — is behind a [`Projector`] the app's bridge registers.
//!
//! A virtual display draws only when the screen changes, so a still
//! screen repeats its last picture at the share's rate. The share ends
//! when the projection does: the user stopping it from the system's
//! notification or status chip, or the screen locking.

use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anvil_core::video::host::{Resolution, ScreenInfo};
use ndk_sys::ANativeWindow;
use parking_lot::Mutex;

use crate::aimage::Reader;
use crate::camera::Capture;
use crate::picture::Picture;
use crate::screen::Screens;

/// What only the app's Java side can do with a projection.
pub trait Projector: Send + Sync {
    /// The phone's display as it is now: width and height in pixels, and
    /// its density (dots per inch).
    fn display(&self) -> Option<(u32, u32, u32)>;
    /// Draw the projection the user allowed onto `window`, `width` ×
    /// `height` at `dpi`. An error when there is no projection (the user
    /// said no, or it was used already).
    fn start(
        &self,
        window: NonNull<ANativeWindow>,
        width: u32,
        height: u32,
        dpi: u32,
    ) -> Result<(), String>;
    /// Stop drawing and let the projection go.
    fn stop(&self);
}

static PROJECTOR: Mutex<Option<Arc<dyn Projector>>> = Mutex::new(None);
/// Bumped each time a projection ends; a capture ends when it moves.
static ENDED: AtomicU64 = AtomicU64::new(0);

/// The app's projector (its bridge, once, at start).
pub fn set_projector(projector: Arc<dyn Projector>) {
    *PROJECTOR.lock() = Some(projector);
}

/// The projection ended (the system stopped it): the share running ends.
pub fn projection_ended() {
    ENDED.fetch_add(1, Ordering::SeqCst);
}

/// The phone's screen, while the app can project it.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProjectionScreens;

/// The one screen's id.
const SCREEN: &str = "screen";

impl Screens for ProjectionScreens {
    fn list(&self) -> Vec<ScreenInfo> {
        if PROJECTOR.lock().is_none() {
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
        want: Resolution,
        fps: u32,
    ) -> Result<Box<dyn Capture>, String> {
        if id.is_some_and(|id| id != SCREEN) {
            return Err(format!("no screen {id:?}"));
        }
        let projector = PROJECTOR.lock().clone().ok_or("no screen to share")?;
        let (width, height, dpi) = projector.display().ok_or("the display's size is unknown")?;
        let (width, height) = fit((width, height), want);
        let reader = Reader::rgba(width, height, 3)?;
        let window = NonNull::new(reader.window()?).ok_or("no window to draw on")?;
        let epoch = ENDED.load(Ordering::SeqCst);
        projector.start(window, width, height, dpi)?;
        Ok(Box::new(Projection {
            projector,
            reader,
            epoch,
            every: Duration::from_secs(1) / fps.clamp(1, 30),
            next_at: Instant::now(),
            last: None,
        }))
    }
}

/// `display` within `want` either way up (a phone held upright shares
/// upright), at most its own size, each side even.
fn fit(display: (u32, u32), want: Resolution) -> (u32, u32) {
    let (w, h) = (display.0.max(2), display.1.max(2));
    let (long, short) = (
        want.width.max(want.height) as f64,
        want.width.min(want.height) as f64,
    );
    let scale = (long / w.max(h) as f64)
        .min(short / w.min(h) as f64)
        .min(1.0);
    let even = |v: u32| ((f64::from(v) * scale) as u32).max(2) & !1;
    (even(w), even(h))
}

/// A share running.
struct Projection {
    projector: Arc<dyn Projector>,
    /// Dropped after the projector stops drawing into it.
    reader: Reader,
    epoch: u64,
    every: Duration,
    next_at: Instant,
    last: Option<Picture>,
}

impl Capture for Projection {
    fn next(&mut self) -> Option<Picture> {
        loop {
            if ENDED.load(Ordering::SeqCst) != self.epoch {
                tracing::info!("the screen's projection ended");
                return None;
            }
            let wait = self.next_at.saturating_duration_since(Instant::now());
            if let Some(mut picture) = self.reader.next(wait) {
                self.next_at = Instant::now() + self.every;
                picture.captured = Instant::now();
                self.last = Some(picture.clone());
                return Some(picture);
            }
            // Nothing drawn: the screen is still. Its last picture again,
            // at the share's rate (none yet: keep waiting).
            if let Some(last) = &self.last {
                self.next_at = Instant::now() + self.every;
                let mut picture = last.clone();
                picture.captured = Instant::now();
                return Some(picture);
            }
            self.next_at = Instant::now() + self.every;
        }
    }
}

impl Drop for Projection {
    fn drop(&mut self) {
        // The display stops drawing before its reader goes.
        self.projector.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_phones_screen_fits_the_share_either_way_up() {
        let p1080 = Resolution {
            width: 1920,
            height: 1080,
        };
        // Upright: its short side to 1080, kept upright.
        assert_eq!(fit((1080, 2400), p1080), (864, 1920));
        // On its side.
        assert_eq!(fit((2400, 1080), p1080), (1920, 864));
        // Smaller than asked: as it is.
        assert_eq!(fit((720, 1280), p1080), (720, 1280));
        // Odd sizes made even.
        assert_eq!(fit((721, 1281), p1080), (720, 1280));
    }
}
