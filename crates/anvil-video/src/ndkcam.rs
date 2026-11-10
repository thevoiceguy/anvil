//! Android: cameras through the NDK's Camera2 (`ACameraManager`), each
//! picture drawn into an [`aimage::Reader`] and turned upright, since a
//! phone's sensor lies on its side. The front camera comes first and is
//! the one a call opens unless another is chosen. The app asks for the
//! camera permission; without it a camera does not open, and says where
//! the permission is.

use std::ffi::{c_void, CStr, CString};
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anvil_core::video::host::{CameraInfo, Resolution};
use ndk_sys::{
    acamera_metadata_enum_acamera_lens_facing as Facing, acamera_metadata_tag as Tag,
    camera_status_t, ACameraCaptureSession, ACameraCaptureSession_stateCallbacks, ACameraDevice,
    ACameraDevice_StateCallbacks, ACameraDevice_request_template, ACameraManager, ACameraMetadata,
    ACameraMetadata_const_entry, ACameraOutputTarget, ACaptureRequest, ACaptureSessionOutput,
    ACaptureSessionOutputContainer, AIMAGE_FORMATS,
};

use crate::aimage::Reader;
use crate::camera::{Cameras, Capture};
use crate::picture::Picture;

// The NDK's camera library (ndk-sys binds it but does not link it).
#[link(name = "camera2ndk")]
extern "C" {}

/// Where the user lets Anvil use the camera.
const ALLOW_AT: &str = "Settings → Apps → Anvil → Permissions → Camera";

fn ok(what: &str, s: camera_status_t) -> Result<(), String> {
    match s {
        camera_status_t::ACAMERA_OK => Ok(()),
        camera_status_t::ACAMERA_ERROR_PERMISSION_DENIED => Err(format!(
            "Anvil may not use the camera: allow it in {ALLOW_AT}"
        )),
        s => Err(format!("{what}: camera status {}", s.0)),
    }
}

/// The phone's cameras.
#[derive(Debug, Clone, Copy, Default)]
pub struct NdkCameras;

/// The camera service.
struct Manager(NonNull<ACameraManager>);

impl Manager {
    fn new() -> Result<Self, String> {
        // SAFETY: plain constructor.
        NonNull::new(unsafe { ndk_sys::ACameraManager_create() })
            .map(Self)
            .ok_or_else(|| "no camera service".into())
    }

    /// Each camera's id and what it is.
    fn cameras(&self) -> Vec<(String, Characteristics)> {
        let mut list = ptr::null_mut();
        // SAFETY: plain out-parameter call.
        let listed = unsafe { ndk_sys::ACameraManager_getCameraIdList(self.0.as_ptr(), &mut list) };
        if ok("camera list", listed).is_err() || list.is_null() {
            return Vec::new();
        }
        // SAFETY: the list was given above and is freed below.
        let ids: Vec<String> = unsafe {
            let l = &*list;
            (0..l.numCameras.max(0) as usize)
                .map(|i| {
                    CStr::from_ptr(*l.cameraIds.add(i))
                        .to_string_lossy()
                        .into_owned()
                })
                .collect()
        };
        // SAFETY: given above.
        unsafe { ndk_sys::ACameraManager_deleteCameraIdList(list) };
        let mut cameras: Vec<_> = ids
            .into_iter()
            .filter_map(|id| Some((id.clone(), self.characteristics(&id)?)))
            .collect();
        // Front, back, then the rest, each in the system's order.
        cameras.sort_by_key(|(_, c)| c.rank());
        cameras
    }

    fn characteristics(&self, id: &str) -> Option<Characteristics> {
        let cid = CString::new(id).ok()?;
        let mut meta = ptr::null_mut();
        // SAFETY: plain out-parameter call.
        let s = unsafe {
            ndk_sys::ACameraManager_getCameraCharacteristics(
                self.0.as_ptr(),
                cid.as_ptr(),
                &mut meta,
            )
        };
        ok("camera characteristics", s).ok()?;
        let meta = Metadata(NonNull::new(meta)?);
        let facing = meta.i32s(Tag::ACAMERA_LENS_FACING, true);
        let facing = facing.first().map(|&f| Facing(f as u32));
        // Each entry: format, width, height, whether an input.
        let sizes = meta
            .i32s(Tag::ACAMERA_SCALER_AVAILABLE_STREAM_CONFIGURATIONS, false)
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|c| c[0] == AIMAGE_FORMATS::AIMAGE_FORMAT_YUV_420_888.0 as i32 && c[3] == 0)
            .map(|c| (c[1] as u32, c[2] as u32))
            .collect();
        let fps = meta
            .i32s(Tag::ACAMERA_CONTROL_AE_AVAILABLE_TARGET_FPS_RANGES, false)
            .as_chunks::<2>()
            .0
            .iter()
            .map(|r| (r[0], r[1]))
            .collect();
        Some(Characteristics {
            facing,
            orientation: meta
                .i32s(Tag::ACAMERA_SENSOR_ORIENTATION, false)
                .first()
                .map_or(0, |&o| o.rem_euclid(360) as u32),
            sizes,
            fps,
        })
    }
}

impl Drop for Manager {
    fn drop(&mut self) {
        // SAFETY: ours.
        unsafe { ndk_sys::ACameraManager_delete(self.0.as_ptr()) };
    }
}

/// A camera's characteristics, as read.
struct Metadata(NonNull<ACameraMetadata>);

impl Metadata {
    /// An entry's numbers (`u8` entries widened when `bytes`).
    fn i32s(&self, tag: Tag, bytes: bool) -> Vec<i32> {
        // SAFETY: a zeroed entry is filled by the call.
        let mut entry: ACameraMetadata_const_entry = unsafe { std::mem::zeroed() };
        // SAFETY: plain out-parameter call; the data lives with `self`.
        unsafe {
            if ndk_sys::ACameraMetadata_getConstEntry(self.0.as_ptr(), tag.0, &mut entry)
                != camera_status_t::ACAMERA_OK
            {
                return Vec::new();
            }
            let n = entry.count as usize;
            if bytes {
                std::slice::from_raw_parts(entry.data.u8_, n)
                    .iter()
                    .map(|&b| i32::from(b))
                    .collect()
            } else {
                std::slice::from_raw_parts(entry.data.i32_, n).to_vec()
            }
        }
    }
}

impl Drop for Metadata {
    fn drop(&mut self) {
        // SAFETY: ours.
        unsafe { ndk_sys::ACameraMetadata_free(self.0.as_ptr()) };
    }
}

struct Characteristics {
    facing: Option<Facing>,
    /// Clockwise degrees that turn a picture upright.
    orientation: u32,
    /// The YUV sizes it gives.
    sizes: Vec<(u32, u32)>,
    /// The frame rate ranges it keeps to.
    fps: Vec<(i32, i32)>,
}

impl Characteristics {
    fn rank(&self) -> u8 {
        match self.facing {
            Some(Facing::ACAMERA_LENS_FACING_FRONT) => 0,
            Some(Facing::ACAMERA_LENS_FACING_BACK) => 1,
            _ => 2,
        }
    }

    fn name(&self) -> &'static str {
        match self.facing {
            Some(Facing::ACAMERA_LENS_FACING_FRONT) => "Front camera",
            Some(Facing::ACAMERA_LENS_FACING_BACK) => "Back camera",
            _ => "External camera",
        }
    }

    /// The smallest size that covers `want` (the sensor's way up),
    /// else the largest there is.
    fn size(&self, want: Resolution) -> Option<(u32, u32)> {
        let (ww, wh) = (want.width.max(want.height), want.width.min(want.height));
        let area = |&(w, h): &(u32, u32)| u64::from(w) * u64::from(h);
        self.sizes
            .iter()
            .copied()
            .filter(|&(w, h)| w >= ww && h >= wh)
            .min_by_key(area)
            .or_else(|| self.sizes.iter().copied().max_by_key(area))
    }

    /// The range that reaches `fps` and goes lowest (a dark room slows
    /// it rather than darkening it), else the fastest.
    fn fps_range(&self, fps: u32) -> Option<(i32, i32)> {
        let fps = fps as i32;
        self.fps
            .iter()
            .copied()
            .filter(|&(_, hi)| hi == fps)
            .min_by_key(|&(lo, _)| lo)
            .or_else(|| self.fps.iter().copied().max_by_key(|&(lo, hi)| (hi, lo)))
    }
}

impl Cameras for NdkCameras {
    fn list(&self) -> Vec<CameraInfo> {
        let Ok(manager) = Manager::new() else {
            return Vec::new();
        };
        let cameras = manager.cameras();
        let mut seen = [0u32; 3];
        cameras
            .into_iter()
            .map(|(id, c)| {
                let n = &mut seen[c.rank() as usize];
                *n += 1;
                let name = if *n == 1 {
                    c.name().to_string()
                } else {
                    format!("{} {n}", c.name())
                };
                CameraInfo { id, name }
            })
            .collect()
    }

    fn open(
        &self,
        id: Option<&str>,
        want: Resolution,
        fps: u32,
    ) -> Result<Box<dyn Capture>, String> {
        let manager = Manager::new()?;
        let (id, c) = {
            let mut cameras = manager.cameras();
            match id {
                Some(id) => {
                    let at = cameras
                        .iter()
                        .position(|(i, _)| i == id)
                        .ok_or_else(|| format!("no camera {id}"))?;
                    cameras.swap_remove(at)
                }
                None => cameras.into_iter().next().ok_or("no camera")?,
            }
        };
        let (width, height) = c.size(want).ok_or("the camera gives no YUV pictures")?;
        Ok(Box::new(NdkCapture::open(
            manager,
            &id,
            (width, height),
            c.fps_range(fps),
            c.orientation,
        )?))
    }
}

/// The camera was disconnected or failed.
#[derive(Default)]
struct Gone(AtomicBool);

unsafe extern "C" fn disconnected(context: *mut c_void, _: *mut ACameraDevice) {
    // SAFETY: the context is the capture's boxed `Gone`, alive until the
    // device is closed.
    unsafe { &*(context as *const Gone) }
        .0
        .store(true, Ordering::Relaxed);
}

unsafe extern "C" fn failed(context: *mut c_void, _: *mut ACameraDevice, error: i32) {
    tracing::warn!(error, "the camera failed");
    // SAFETY: as in `disconnected`.
    unsafe { &*(context as *const Gone) }
        .0
        .store(true, Ordering::Relaxed);
}

unsafe extern "C" fn session_state(_: *mut c_void, _: *mut ACameraCaptureSession) {}

/// An open camera, drawing into its reader. Every part is freed in drop,
/// in the order the NDK asks.
struct NdkCapture {
    reader: Reader,
    turn: u32,
    _manager: Manager,
    device: *mut ACameraDevice,
    container: *mut ACaptureSessionOutputContainer,
    output: *mut ACaptureSessionOutput,
    target: *mut ACameraOutputTarget,
    request: *mut ACaptureRequest,
    session: *mut ACameraCaptureSession,
    /// Boxed: their addresses are given to the NDK.
    gone: Box<Gone>,
    device_callbacks: Box<ACameraDevice_StateCallbacks>,
    session_callbacks: Box<ACameraCaptureSession_stateCallbacks>,
}

// SAFETY: the NDK's camera objects may be used from any thread.
unsafe impl Send for NdkCapture {}

impl NdkCapture {
    fn open(
        manager: Manager,
        id: &str,
        (width, height): (u32, u32),
        fps: Option<(i32, i32)>,
        turn: u32,
    ) -> Result<Self, String> {
        let reader = Reader::new(width, height, 4)?;
        let window = reader.window()?;
        let gone = Box::<Gone>::default();
        let device_callbacks = Box::new(ACameraDevice_StateCallbacks {
            context: &*gone as *const Gone as *mut c_void,
            onDisconnected: Some(disconnected),
            onError: Some(failed),
        });
        let session_callbacks = Box::new(ACameraCaptureSession_stateCallbacks {
            context: ptr::null_mut(),
            onClosed: Some(session_state),
            onReady: Some(session_state),
            onActive: Some(session_state),
        });
        let mut this = Self {
            reader,
            turn,
            _manager: manager,
            device: ptr::null_mut(),
            container: ptr::null_mut(),
            output: ptr::null_mut(),
            target: ptr::null_mut(),
            request: ptr::null_mut(),
            session: ptr::null_mut(),
            gone,
            device_callbacks,
            session_callbacks,
        };
        let cid = CString::new(id).map_err(|_| "a camera id with a NUL")?;
        // SAFETY: each call fills an out-parameter that `this` owns and
        // frees in drop (a failure part way frees what was made); the
        // callbacks and their contexts are boxed in `this`.
        unsafe {
            ok(
                "open the camera",
                ndk_sys::ACameraManager_openCamera(
                    this._manager.0.as_ptr(),
                    cid.as_ptr(),
                    &mut *this.device_callbacks,
                    &mut this.device,
                ),
            )?;
            ok(
                "outputs",
                ndk_sys::ACaptureSessionOutputContainer_create(&mut this.container),
            )?;
            ok(
                "output",
                ndk_sys::ACaptureSessionOutput_create(window, &mut this.output),
            )?;
            ok(
                "add the output",
                ndk_sys::ACaptureSessionOutputContainer_add(this.container, this.output),
            )?;
            ok(
                "request",
                ndk_sys::ACameraDevice_createCaptureRequest(
                    this.device,
                    ACameraDevice_request_template::TEMPLATE_RECORD,
                    &mut this.request,
                ),
            )?;
            ok(
                "target",
                ndk_sys::ACameraOutputTarget_create(window, &mut this.target),
            )?;
            ok(
                "add the target",
                ndk_sys::ACaptureRequest_addTarget(this.request, this.target),
            )?;
            if let Some((lo, hi)) = fps {
                let range = [lo, hi];
                ok(
                    "frame rate",
                    ndk_sys::ACaptureRequest_setEntry_i32(
                        this.request,
                        Tag::ACAMERA_CONTROL_AE_TARGET_FPS_RANGE.0,
                        2,
                        range.as_ptr(),
                    ),
                )?;
            }
            ok(
                "session",
                ndk_sys::ACameraDevice_createCaptureSession(
                    this.device,
                    this.container,
                    &*this.session_callbacks,
                    &mut this.session,
                ),
            )?;
            let mut requests = [this.request];
            ok(
                "start the camera",
                ndk_sys::ACameraCaptureSession_setRepeatingRequest(
                    this.session,
                    ptr::null_mut(),
                    1,
                    requests.as_mut_ptr(),
                    ptr::null_mut(),
                ),
            )?;
        }
        Ok(this)
    }
}

impl Capture for NdkCapture {
    fn next(&mut self) -> Option<Picture> {
        // A camera gives a picture every few tens of milliseconds; one
        // that gives none for seconds, or went, is gone.
        for _ in 0..5 {
            if self.gone.0.load(Ordering::Relaxed) {
                return None;
            }
            if let Some(p) = self.reader.next(Duration::from_secs(1)) {
                return Some(p.rotated(self.turn));
            }
        }
        None
    }
}

impl Drop for NdkCapture {
    fn drop(&mut self) {
        // SAFETY: each part is ours and freed once, the session before the
        // device, and both before the reader they draw into.
        unsafe {
            if !self.session.is_null() {
                ndk_sys::ACameraCaptureSession_stopRepeating(self.session);
                ndk_sys::ACameraCaptureSession_close(self.session);
            }
            if !self.request.is_null() {
                ndk_sys::ACaptureRequest_free(self.request);
            }
            if !self.target.is_null() {
                ndk_sys::ACameraOutputTarget_free(self.target);
            }
            if !self.device.is_null() {
                ndk_sys::ACameraDevice_close(self.device);
            }
            if !self.container.is_null() {
                ndk_sys::ACaptureSessionOutputContainer_free(self.container);
            }
            if !self.output.is_null() {
                ndk_sys::ACaptureSessionOutput_free(self.output);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cameras_listed_are_the_ones_that_open() {
        crate::binder_threads();
        let cameras = NdkCameras.list();
        let want = Resolution::P720;
        if cameras.is_empty() {
            assert!(NdkCameras.open(None, want, 30).is_err());
        }
        for c in &cameras {
            assert!(!c.id.is_empty() && !c.name.is_empty(), "{c:?}");
            match NdkCameras.open(Some(&c.id), want, 30) {
                Ok(mut capture) => {
                    let p = capture.next().expect("a picture");
                    assert!(p.width > 0 && p.height > 0, "{c:?}: {p:?}");
                }
                // A test is not an app, and may not be let use it.
                Err(e) if e.contains(ALLOW_AT) => eprintln!("{c:?}: {e}"),
                Err(e) => panic!("{c:?}: {e}"),
            }
        }
        assert!(NdkCameras.open(Some("no-such-camera"), want, 30).is_err());
    }
}
