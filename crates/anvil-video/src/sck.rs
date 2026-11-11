//! Screens on macOS, through ScreenCaptureKit (macOS 12.3 and later):
//! each display listed by CoreGraphics, captured as BGRA by an `SCStream`
//! on a dispatch queue of its own, each picture handed to the source's
//! thread.
//!
//! Recording the screen is the user's to allow (System Settings → Privacy
//! & Security → Screen & System Audio Recording). Listing the displays
//! asks nothing; the first share asks, on the source's thread, and a share
//! the user has not allowed ends at once, saying where to allow it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anvil_core::video::host::{Resolution, ScreenInfo};
use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, ProtocolObject};
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass};
use objc2_core_graphics::{
    CGDirectDisplayID, CGDisplayIsBuiltin, CGDisplayPixelsHigh, CGDisplayPixelsWide,
    CGGetActiveDisplayList, CGMainDisplayID, CGPreflightScreenCaptureAccess,
    CGRequestScreenCaptureAccess,
};
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_core_video::{
    kCVPixelFormatType_32BGRA, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow,
    CVPixelBufferGetHeight, CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress,
    CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
};
use objc2_foundation::{NSArray, NSError, NSObject, NSObjectProtocol};
use objc2_screen_capture_kit::{
    SCContentFilter, SCDisplay, SCShareableContent, SCStream, SCStreamConfiguration,
    SCStreamDelegate, SCStreamOutput, SCStreamOutputType,
};

use crate::camera::Capture;
use crate::picture::Picture;
use crate::screen::Screens;

/// Where the user allows a share.
const ALLOW_AT: &str = "System Settings → Privacy & Security → Screen & System Audio Recording";

/// How long the system has to list what can be shared, and to start.
const STARTING: Duration = Duration::from_secs(10);

/// The Mac's displays.
#[derive(Debug, Clone, Copy, Default)]
pub struct SckScreens;

/// The active displays, the main one first.
fn displays() -> Vec<CGDirectDisplayID> {
    let mut ids = [0 as CGDirectDisplayID; 16];
    let mut n = 0u32;
    // SAFETY: a buffer of 16 ids and a count CoreGraphics fills.
    let err = unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut n) };
    if err.0 != 0 {
        return Vec::new();
    }
    let mut list = ids[..n as usize].to_vec();
    let main = CGMainDisplayID();
    list.sort_by_key(|&d| d != main);
    list
}

/// Whether this macOS has ScreenCaptureKit (12.3 and later).
fn available() -> bool {
    AnyClass::get(c"SCStream").is_some()
}

impl Screens for SckScreens {
    fn list(&self) -> Vec<ScreenInfo> {
        if !available() {
            return Vec::new();
        }
        displays()
            .into_iter()
            .enumerate()
            .map(|(i, d)| {
                let (w, h) = (CGDisplayPixelsWide(d), CGDisplayPixelsHigh(d));
                let what = if CGDisplayIsBuiltin(d) {
                    "Built-in display".to_string()
                } else {
                    format!("Display {}", i + 1)
                };
                ScreenInfo {
                    id: d.to_string(),
                    name: format!("{what} ({w}×{h})"),
                }
            })
            .collect()
    }

    fn open(
        &self,
        id: Option<&str>,
        want: Resolution,
        fps: u32,
    ) -> Result<Box<dyn Capture>, String> {
        if !available() {
            return Err("sharing a screen needs macOS 12.3 or later".into());
        }
        let display = match id {
            Some(id) => id
                .parse::<CGDirectDisplayID>()
                .ok()
                .filter(|d| displays().contains(d))
                .ok_or_else(|| format!("no display {id}"))?,
            None => *displays().first().ok_or("no display")?,
        };
        Ok(Box::new(SckCapture {
            display,
            want,
            fps: fps.clamp(1, 30),
            running: None,
            pictures: None,
            last: None,
            next_at: Instant::now(),
            failed: false,
        }))
    }
}

/// An Objective-C object the capture keeps, used from the capture's own
/// thread.
struct Unsent<T>(T);

// SAFETY: ScreenCaptureKit's stream may be started and stopped from any
// thread; it is used by the one thread that owns its capture.
unsafe impl<T> Send for Unsent<T> {}

struct Running {
    stream: Retained<SCStream>,
    // Kept alive while the stream sends to them.
    _output: Retained<Output>,
    _queue: dispatch2::DispatchRetained<DispatchQueue>,
    stopped: Arc<AtomicBool>,
}

struct SckCapture {
    display: CGDirectDisplayID,
    want: Resolution,
    fps: u32,
    running: Option<Unsent<Running>>,
    pictures: Option<Receiver<Picture>>,
    /// The last picture: a screen that does not change sends none.
    last: Option<Picture>,
    next_at: Instant,
    failed: bool,
}

impl Drop for SckCapture {
    fn drop(&mut self) {
        if let Some(r) = self.running.take() {
            // SAFETY: ours; the system's recording indicator goes off.
            unsafe { r.0.stream.stopCaptureWithCompletionHandler(None) };
        }
    }
}

/// Whether the user allows recording the screen, asking the first time.
/// The system's prompt does not wait for the answer: a share asked for
/// now ends, and the next one (after the user allows it) goes.
fn allowed() -> bool {
    CGPreflightScreenCaptureAccess() || CGRequestScreenCaptureAccess()
}

/// The display as ScreenCaptureKit knows it.
fn sc_display(id: CGDirectDisplayID) -> Result<Retained<SCDisplay>, String> {
    let (tx, rx) = mpsc::channel::<Result<Retained<SCDisplay>, String>>();
    let found = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            // SAFETY: the system hands one or the other, valid for the call.
            let answer = unsafe {
                match (content.as_ref(), error.as_ref()) {
                    (Some(content), _) => content
                        .displays()
                        .iter()
                        .find(|d| d.displayID() == id)
                        .ok_or_else(|| format!("display {id} cannot be shared")),
                    (None, Some(e)) => Err(e.localizedDescription().to_string()),
                    (None, None) => Err("nothing can be shared".into()),
                }
            };
            let _ = tx.send(answer);
        },
    );
    // SAFETY: a class method with a block it calls once.
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&found) };
    rx.recv_timeout(STARTING)
        .map_err(|_| "the system did not list the displays".to_string())?
}

/// The size to capture: the display's, fitted within `want`, even.
fn fitted(display: CGDirectDisplayID, want: Resolution) -> (usize, usize) {
    let (w, h) = (
        CGDisplayPixelsWide(display).max(2),
        CGDisplayPixelsHigh(display).max(2),
    );
    let scale = f64::min(
        1.0,
        f64::min(want.width as f64 / w as f64, want.height as f64 / h as f64),
    );
    (
        ((w as f64 * scale) as usize).max(2) & !1,
        ((h as f64 * scale) as usize).max(2) & !1,
    )
}

impl SckCapture {
    /// Ask to record if need be, then start the stream.
    fn start(&mut self) -> Result<(), String> {
        if !allowed() {
            return Err(format!("recording the screen is not allowed ({ALLOW_AT})"));
        }
        let display = sc_display(self.display)?;
        let (w, h) = fitted(self.display, self.want);
        let (tx, rx) = mpsc::sync_channel(2);
        let stopped = Arc::new(AtomicBool::new(false));
        // SAFETY: ScreenCaptureKit's documented set-up: a filter on one
        // display, a configuration, a stream with an output on a serial
        // queue, all before it starts.
        unsafe {
            let filter = SCContentFilter::initWithDisplay_excludingWindows(
                SCContentFilter::alloc(),
                &display,
                &NSArray::new(),
            );
            let config = SCStreamConfiguration::new();
            config.setWidth(w);
            config.setHeight(h);
            config.setPixelFormat(kCVPixelFormatType_32BGRA);
            config.setMinimumFrameInterval(CMTime::new(1, self.fps as i32));
            config.setShowsCursor(true);
            config.setQueueDepth(3);
            let output = Output::new(tx, Arc::clone(&stopped));
            let stream = SCStream::initWithFilter_configuration_delegate(
                SCStream::alloc(),
                &filter,
                &config,
                Some(ProtocolObject::from_ref(&*output)),
            );
            let queue = DispatchQueue::new("anvil.screen", None);
            stream
                .addStreamOutput_type_sampleHandlerQueue_error(
                    ProtocolObject::from_ref(&*output),
                    SCStreamOutputType::Screen,
                    Some(&queue),
                )
                .map_err(|e| format!("the screen's pictures: {}", e.localizedDescription()))?;
            let (started_tx, started) = mpsc::channel::<Option<String>>();
            let done = RcBlock::new(move |error: *mut NSError| {
                let error = error.as_ref().map(|e| e.localizedDescription().to_string());
                let _ = started_tx.send(error);
            });
            stream.startCaptureWithCompletionHandler(Some(&done));
            match started.recv_timeout(STARTING) {
                Ok(None) => {}
                Ok(Some(e)) => return Err(format!("the screen would not start: {e}")),
                Err(_) => return Err("the screen did not start".into()),
            }
            tracing::info!(display = self.display, w, h, "sharing a Mac's display");
            self.running = Some(Unsent(Running {
                stream,
                _output: output,
                _queue: queue,
                stopped,
            }));
        }
        self.pictures = Some(rx);
        Ok(())
    }
}

impl Capture for SckCapture {
    fn next(&mut self) -> Option<Picture> {
        if self.failed {
            return None;
        }
        if self.pictures.is_none() {
            if let Err(e) = self.start() {
                tracing::warn!("screen: {e}");
                self.failed = true;
                return None;
            }
        }
        let now = Instant::now();
        if self.next_at > now {
            std::thread::sleep(self.next_at - now);
        }
        self.next_at = (self.next_at + Duration::from_secs(1) / self.fps).max(Instant::now());
        if self
            .running
            .as_ref()
            .is_some_and(|r| r.0.stopped.load(Ordering::Relaxed))
        {
            tracing::info!("screen: the system stopped the share");
            self.failed = true;
            return None;
        }
        let pictures = self.pictures.as_ref()?;
        let mut newest = None;
        while let Ok(p) = pictures.try_recv() {
            newest = Some(p);
        }
        if newest.is_none() && self.last.is_none() {
            match pictures.recv_timeout(STARTING) {
                Ok(p) => newest = Some(p),
                Err(RecvTimeoutError::Timeout) => {
                    tracing::warn!("screen: no picture for {STARTING:?}");
                    self.failed = true;
                    return None;
                }
                Err(RecvTimeoutError::Disconnected) => return None,
            }
        }
        if let Some(p) = newest {
            self.last = Some(p);
        }
        let mut p = self.last.clone()?;
        p.captured = Instant::now();
        Some(p)
    }
}

/// Where ScreenCaptureKit sends the screen's pictures, and says it
/// stopped (the user ended it from the menu bar, the display went away).
struct Frames {
    tx: SyncSender<Picture>,
    stopped: Arc<AtomicBool>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and `Output`
    // does not implement Drop.
    #[unsafe(super(NSObject))]
    #[name = "AnvilScreenOutput"]
    #[ivars = Frames]
    struct Output;

    unsafe impl NSObjectProtocol for Output {}

    unsafe impl SCStreamOutput for Output {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn did_output(
            &self,
            _stream: &SCStream,
            sample: &CMSampleBuffer,
            kind: SCStreamOutputType,
        ) {
            if kind != SCStreamOutputType::Screen {
                return;
            }
            // A screen that has not changed sends a sample without an
            // image: nothing to hand on.
            if let Some(p) = bgra(sample) {
                // A source behind drops a picture rather than queue it.
                let _ = self.ivars().tx.try_send(p);
            }
        }
    }

    unsafe impl SCStreamDelegate for Output {
        #[unsafe(method(stream:didStopWithError:))]
        fn did_stop(&self, _stream: &SCStream, error: &NSError) {
            tracing::info!("the screen share stopped: {}", error.localizedDescription());
            self.ivars().stopped.store(true, Ordering::Relaxed);
        }
    }
);

impl Output {
    fn new(tx: SyncSender<Picture>, stopped: Arc<AtomicBool>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(Frames { tx, stopped });
        // SAFETY: NSObject's init.
        unsafe { msg_send![super(this), init] }
    }
}

/// A sample's BGRA picture as a `Picture`.
fn bgra(sample: &CMSampleBuffer) -> Option<Picture> {
    // SAFETY: the sample's own image, read while locked, its rows times its
    // bytes per row long.
    unsafe {
        let image = sample.image_buffer()?;
        let flags = CVPixelBufferLockFlags::ReadOnly;
        CVPixelBufferLockBaseAddress(&image, flags);
        let (w, h) = (
            CVPixelBufferGetWidth(&image),
            CVPixelBufferGetHeight(&image),
        );
        let base = CVPixelBufferGetBaseAddress(&image).cast::<u8>();
        let stride = CVPixelBufferGetBytesPerRow(&image);
        let picture = (!base.is_null() && w >= 2 && h >= 2 && stride >= w * 4).then(|| {
            Picture::from_bgrx(
                std::slice::from_raw_parts(base, stride * h),
                stride,
                (w & !1) as u32,
                (h & !1) as u32,
            )
        });
        CVPixelBufferUnlockBaseAddress(&image, flags);
        picture
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Mac lists its displays without asking anything, each with an id
    /// it opens by (opening asks nothing yet: the stream starts at the
    /// first picture); CI's runner has one.
    #[test]
    fn the_displays_listed_are_the_ones_that_open() {
        let screens = SckScreens.list();
        if !available() {
            assert!(screens.is_empty());
            return;
        }
        for s in &screens {
            assert!(!s.id.is_empty() && s.name.contains('×'), "{s:?}");
            assert!(SckScreens.open(Some(&s.id), Resolution::P1080, 10).is_ok());
        }
        assert!(SckScreens
            .open(Some("no-such-display"), Resolution::P1080, 10)
            .is_err());
    }

    /// Captured where recording is already allowed (a runner may allow it;
    /// nothing here can answer the system's prompt): pictures within the
    /// size asked, still coming while the screen does not change.
    #[test]
    fn the_main_display_is_captured_where_recording_is_allowed() {
        if !available() || !CGPreflightScreenCaptureAccess() {
            eprintln!("skipped: recording the screen is not allowed here");
            return;
        }
        let mut capture = SckScreens
            .open(None, Resolution::P720, 10)
            .expect("the main display opens");
        for _ in 0..5 {
            let p = capture.next().expect("a picture");
            assert!(p.width <= 1280 && p.height <= 720, "{p:?}");
            assert!(p.width >= 2 && p.height >= 2, "{p:?}");
        }
    }

    #[test]
    fn a_display_is_captured_within_the_size_asked() {
        let (w, h) = fitted(CGMainDisplayID(), Resolution::P720);
        assert!(w <= 1280 && h <= 720 && w % 2 == 0 && h % 2 == 0, "{w}×{h}");
    }
}
