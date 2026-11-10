//! Cameras on macOS and iOS, through AVFoundation: listed, opened by
//! unique id, and captured as NV12 on a dispatch queue of their own, each
//! picture handed to the source's thread.
//!
//! Access is the user's to give (the app's `NSCameraUsageDescription`, and
//! on macOS its camera entitlement): asked the first time a call sends
//! video, on the source's thread, so the call goes on while the system
//! asks.
//!
//! On an iPhone the cameras are the built-in front and back ones, the
//! front one by default; pictures come upright for a phone held upright,
//! and the capture leaves the app's audio session to the call.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::time::{Duration, Instant};

use anvil_core::video::host::{CameraInfo, Resolution};
use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, ProtocolObject};
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass};
use objc2_av_foundation::{
    AVAuthorizationStatus, AVCaptureConnection, AVCaptureDevice, AVCaptureDeviceInput,
    AVCaptureOutput, AVCaptureSession, AVCaptureSessionPreset1280x720,
    AVCaptureSessionPreset640x480, AVCaptureVideoDataOutput,
    AVCaptureVideoDataOutputSampleBufferDelegate, AVMediaTypeVideo,
};
use objc2_core_foundation::CFString;
use objc2_core_media::CMSampleBuffer;
use objc2_core_video::{
    kCVPixelBufferPixelFormatTypeKey, kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
    CVPixelBufferGetBaseAddressOfPlane, CVPixelBufferGetBytesPerRowOfPlane, CVPixelBufferGetHeight,
    CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
    CVPixelBufferUnlockBaseAddress,
};
use objc2_foundation::{NSDictionary, NSNumber, NSObject, NSObjectProtocol, NSString};

use crate::camera::{Cameras, Capture};
use crate::picture::Picture;

/// How long the user has to answer the system's camera prompt.
const ASKING: Duration = Duration::from_secs(120);

/// A camera that has sent nothing for this long has gone.
const SILENT: Duration = Duration::from_secs(5);

/// The device's cameras.
#[derive(Debug, Clone, Copy, Default)]
pub struct AvfCameras;

fn video() -> Option<&'static NSString> {
    // SAFETY: a constant AVFoundation exports.
    unsafe { AVMediaTypeVideo }
}

#[cfg(target_os = "macos")]
fn devices() -> Vec<Retained<AVCaptureDevice>> {
    let Some(video) = video() else {
        return Vec::new();
    };
    // Deprecated for the discovery session, whose device types are newer
    // than some of the macOS releases the app runs on; this lists the same
    // cameras on every one.
    #[allow(deprecated)]
    // SAFETY: a class method with a media type.
    let list = unsafe { AVCaptureDevice::devicesWithMediaType(video) };
    list.to_vec()
}

/// The built-in wide-angle cameras, front first.
#[cfg(target_os = "ios")]
fn devices() -> Vec<Retained<AVCaptureDevice>> {
    use objc2_av_foundation::{
        AVCaptureDeviceDiscoverySession, AVCaptureDevicePosition,
        AVCaptureDeviceTypeBuiltInWideAngleCamera,
    };
    use objc2_foundation::NSArray;
    // SAFETY: a constant AVFoundation exports, and a discovery session
    // over it.
    let mut list = unsafe {
        let types = NSArray::from_slice(&[AVCaptureDeviceTypeBuiltInWideAngleCamera]);
        AVCaptureDeviceDiscoverySession::discoverySessionWithDeviceTypes_mediaType_position(
            &types,
            video(),
            AVCaptureDevicePosition::Unspecified,
        )
        .devices()
        .to_vec()
    };
    // SAFETY: a property of a device.
    list.sort_by_key(|d| unsafe { d.position() } != AVCaptureDevicePosition::Front);
    list
}

/// The camera a call opens unless told: the Mac's default, an iPhone's
/// front one.
fn default_device(video: &NSString) -> Option<Retained<AVCaptureDevice>> {
    #[cfg(target_os = "macos")]
    // SAFETY: a class method with a media type.
    let device = unsafe { AVCaptureDevice::defaultDeviceWithMediaType(video) };
    #[cfg(target_os = "ios")]
    let device = {
        let _ = video;
        devices().into_iter().next()
    };
    device
}

/// Where the user allows the camera.
const ALLOW_AT: &str = if cfg!(target_os = "ios") {
    "Settings → Privacy & Security → Camera"
} else {
    "System Settings → Privacy & Security → Camera"
};

impl Cameras for AvfCameras {
    fn list(&self) -> Vec<CameraInfo> {
        devices()
            .iter()
            .map(|d| CameraInfo {
                // SAFETY: properties of a device.
                id: unsafe { d.uniqueID() }.to_string(),
                name: unsafe { d.localizedName() }.to_string(),
            })
            .collect()
    }

    fn open(
        &self,
        id: Option<&str>,
        want: Resolution,
        _fps: u32,
    ) -> Result<Box<dyn Capture>, String> {
        let video = video().ok_or("no video media type")?;
        let device = match id {
            // SAFETY: a class method with an id.
            Some(id) => unsafe { AVCaptureDevice::deviceWithUniqueID(&NSString::from_str(id)) },
            None => default_device(video),
        }
        .ok_or_else(|| format!("no camera {}", id.unwrap_or("here")))?;
        // SAFETY: a class method with a media type.
        let status = unsafe { AVCaptureDevice::authorizationStatusForMediaType(video) };
        if status == AVAuthorizationStatus::Denied || status == AVAuthorizationStatus::Restricted {
            return Err(format!("the camera is not allowed ({ALLOW_AT})"));
        }
        Ok(Box::new(AvfCapture {
            device: Some(Unsent(device)),
            want,
            running: None,
            pictures: None,
        }))
    }
}

/// An Objective-C object the capture keeps, used from the capture's own
/// thread.
struct Unsent<T>(T);

// SAFETY: AVFoundation's capture objects may be used from any thread; each
// is used by the one thread that owns its capture.
unsafe impl<T> Send for Unsent<T> {}

struct Running {
    session: Retained<AVCaptureSession>,
    // Kept alive while the session sends to them.
    _output: Retained<AVCaptureVideoDataOutput>,
    _delegate: Retained<Delegate>,
    _queue: dispatch2::DispatchRetained<DispatchQueue>,
}

struct AvfCapture {
    device: Option<Unsent<Retained<AVCaptureDevice>>>,
    want: Resolution,
    running: Option<Unsent<Running>>,
    pictures: Option<Receiver<Picture>>,
}

impl Drop for AvfCapture {
    fn drop(&mut self) {
        if let Some(r) = self.running.take() {
            // SAFETY: ours; the camera, and its light, go off.
            unsafe { r.0.session.stopRunning() };
        }
    }
}

/// Wait for the user to answer the camera prompt; whether they allowed it.
fn ask() -> bool {
    let Some(video) = video() else {
        return false;
    };
    // SAFETY: a class method with a media type.
    match unsafe { AVCaptureDevice::authorizationStatusForMediaType(video) } {
        AVAuthorizationStatus::Authorized => return true,
        AVAuthorizationStatus::NotDetermined => {}
        _ => return false,
    }
    let (tx, rx) = mpsc::channel();
    let answer = RcBlock::new(move |granted: Bool| {
        let _ = tx.send(granted.as_bool());
    });
    // SAFETY: a class method with a media type and a block it calls once.
    unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(video, &answer) };
    rx.recv_timeout(ASKING).unwrap_or(false)
}

impl AvfCapture {
    /// Ask for access if need be, then start the session.
    fn start(&mut self) -> Result<(), String> {
        let device = self.device.take().ok_or("the camera has stopped")?.0;
        if !ask() {
            return Err("the camera was not allowed".into());
        }
        let (tx, rx) = mpsc::sync_channel(2);
        // SAFETY: AVFoundation's documented set-up of a capture session:
        // one input, one output with a delegate on a serial queue, all
        // configured before it runs.
        unsafe {
            let session = AVCaptureSession::new();
            session.beginConfiguration();
            // The call's audio session is CallKit's and the phone's: the
            // camera must not reconfigure it.
            #[cfg(target_os = "ios")]
            session.setAutomaticallyConfiguresApplicationAudioSession(false);
            let preset = if self.want.height >= 720 {
                AVCaptureSessionPreset1280x720
            } else {
                AVCaptureSessionPreset640x480
            };
            if session.canSetSessionPreset(preset) {
                session.setSessionPreset(preset);
            }
            let input = AVCaptureDeviceInput::deviceInputWithDevice_error(&device)
                .map_err(|e| format!("the camera would not open: {}", e.localizedDescription()))?;
            if !session.canAddInput(&input) {
                return Err("the camera cannot be captured".into());
            }
            session.addInput(&input);

            let output = AVCaptureVideoDataOutput::new();
            let key: &CFString = kCVPixelBufferPixelFormatTypeKey;
            // CFString and NSString are the same object (toll-free bridged).
            let key: &NSString = &*(key as *const CFString).cast::<NSString>();
            let nv12 = NSNumber::new_u32(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange);
            let settings =
                NSDictionary::<NSString, AnyObject>::from_slices(&[key], &[&*nv12 as &AnyObject]);
            output.setVideoSettings(Some(&settings));
            output.setAlwaysDiscardsLateVideoFrames(true);
            let delegate = Delegate::new(tx);
            let queue = DispatchQueue::new("anvil.camera", None);
            output.setSampleBufferDelegate_queue(
                Some(ProtocolObject::from_ref(&*delegate)),
                Some(&queue),
            );
            if !session.canAddOutput(&output) {
                return Err("the camera's pictures cannot be taken".into());
            }
            session.addOutput(&output);
            #[cfg(target_os = "ios")]
            upright(&output);
            session.commitConfiguration();
            session.startRunning();
            self.running = Some(Unsent(Running {
                session,
                _output: output,
                _delegate: delegate,
                _queue: queue,
            }));
        }
        self.pictures = Some(rx);
        Ok(())
    }
}

/// An iPhone's pictures upright for the phone held upright (a sensor's
/// are landscape).
#[cfg(target_os = "ios")]
#[allow(deprecated)]
unsafe fn upright(output: &AVCaptureVideoDataOutput) {
    use objc2_av_foundation::AVCaptureVideoOrientation;
    let Some(video) = video() else {
        return;
    };
    // SAFETY: the output's connection, set before the session runs. The
    // orientation is deprecated for a rotation angle from iOS 17; the app
    // runs on 15.
    unsafe {
        if let Some(connection) = output.connectionWithMediaType(video) {
            if connection.isVideoOrientationSupported() {
                connection.setVideoOrientation(AVCaptureVideoOrientation::Portrait);
            }
        }
    }
}

impl Capture for AvfCapture {
    fn next(&mut self) -> Option<Picture> {
        if self.pictures.is_none() {
            if let Err(e) = self.start() {
                tracing::warn!("camera: {e}");
                return None;
            }
        }
        let pictures = self.pictures.as_ref()?;
        match pictures.recv_timeout(SILENT) {
            Ok(mut p) => {
                p.captured = Instant::now();
                Some(p)
            }
            Err(RecvTimeoutError::Timeout) => {
                tracing::warn!("camera: no picture for {SILENT:?}");
                None
            }
            Err(RecvTimeoutError::Disconnected) => None,
        }
    }
}

/// Where AVFoundation sends the camera's pictures.
struct Frames {
    tx: SyncSender<Picture>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and `Delegate`
    // does not implement Drop.
    #[unsafe(super(NSObject))]
    #[name = "AnvilCameraDelegate"]
    #[ivars = Frames]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl AVCaptureVideoDataOutputSampleBufferDelegate for Delegate {
        #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
        fn did_output(
            &self,
            _output: &AVCaptureOutput,
            sample: &CMSampleBuffer,
            _connection: &AVCaptureConnection,
        ) {
            if let Some(p) = nv12(sample) {
                // A source behind drops a picture rather than queue it.
                let _ = self.ivars().tx.try_send(p);
            }
        }
    }
);

impl Delegate {
    fn new(tx: SyncSender<Picture>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(Frames { tx });
        // SAFETY: NSObject's init.
        unsafe { msg_send![super(this), init] }
    }
}

/// A sample's NV12 picture as a `Picture`.
fn nv12(sample: &CMSampleBuffer) -> Option<Picture> {
    // SAFETY: the sample's own image, read while locked, each plane its
    // rows times its bytes per row long.
    unsafe {
        let image = sample.image_buffer()?;
        let flags = CVPixelBufferLockFlags::ReadOnly;
        CVPixelBufferLockBaseAddress(&image, flags);
        let (w, h) = (
            CVPixelBufferGetWidth(&image),
            CVPixelBufferGetHeight(&image),
        );
        let y = CVPixelBufferGetBaseAddressOfPlane(&image, 0).cast::<u8>();
        let uv = CVPixelBufferGetBaseAddressOfPlane(&image, 1).cast::<u8>();
        let picture = (!y.is_null() && !uv.is_null() && w >= 2 && h >= 2).then(|| {
            let ys = CVPixelBufferGetBytesPerRowOfPlane(&image, 0);
            let uvs = CVPixelBufferGetBytesPerRowOfPlane(&image, 1);
            Picture::from_nv12(
                std::slice::from_raw_parts(y, ys * h),
                ys,
                std::slice::from_raw_parts(uv, uvs * h.div_ceil(2)),
                uvs,
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

    /// A Mac without a camera (CI's) lists none and opens none; one with
    /// cameras lists each with an id it opens by.
    #[test]
    fn the_cameras_listed_are_the_ones_that_open() {
        let cameras = AvfCameras.list();
        let want = Resolution::P720;
        if cameras.is_empty() {
            assert!(AvfCameras.open(None, want, 30).is_err());
        } else {
            for c in &cameras {
                assert!(!c.id.is_empty() && !c.name.is_empty(), "{c:?}");
            }
            // Opening asks nothing yet: the session starts at the first
            // picture.
            let _ = AvfCameras.open(Some(&cameras[0].id), want, 30);
        }
        assert!(AvfCameras.open(Some("no-such-camera"), want, 30).is_err());
    }
}
