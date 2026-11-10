//! Android: pictures from an `AImageReader`, which a camera or a decoder
//! draws into. Its images are YUV_420_888, whose layout each image says
//! (planar or interleaved, padded rows), so nothing is guessed.

use std::ffi::c_void;
use std::ptr::{self, NonNull};
use std::time::{Duration, Instant};

use ndk_sys::{
    media_status_t, AImage, AImageReader, AImageReader_ImageListener, ANativeWindow, AIMAGE_FORMATS,
};
use parking_lot::{Condvar, Mutex};

use crate::picture::Picture;

/// Images have come since the last was taken.
#[derive(Default)]
struct Ready {
    count: Mutex<u64>,
    cv: Condvar,
}

/// An image reader, and the pictures drawn into it.
pub(crate) struct Reader {
    reader: NonNull<AImageReader>,
    /// Boxed: its address is the listener's context.
    ready: Box<Ready>,
}

// SAFETY: an AImageReader may be used from any thread.
unsafe impl Send for Reader {}

pub(crate) fn ok(what: &str, s: media_status_t) -> Result<(), String> {
    if s.0 == 0 {
        Ok(())
    } else {
        Err(format!("{what}: media status {}", s.0))
    }
}

unsafe extern "C" fn image_available(context: *mut c_void, _: *mut AImageReader) {
    // SAFETY: the context is the reader's boxed `Ready`, which outlives
    // the listener (cleared before the reader is deleted).
    let ready = unsafe { &*(context as *const Ready) };
    *ready.count.lock() += 1;
    ready.cv.notify_all();
}

impl Reader {
    /// A reader of `width` × `height` YUV_420_888 images (a producer
    /// may draw others), holding at most `images`.
    pub(crate) fn new(width: u32, height: u32, images: i32) -> Result<Self, String> {
        let mut raw = ptr::null_mut();
        // SAFETY: plain out-parameter call.
        ok("image reader", unsafe {
            ndk_sys::AImageReader_new(
                width as i32,
                height as i32,
                AIMAGE_FORMATS::AIMAGE_FORMAT_YUV_420_888.0 as i32,
                images,
                &mut raw,
            )
        })?;
        let reader = NonNull::new(raw).ok_or("no image reader")?;
        let ready = Box::<Ready>::default();
        let mut listener = AImageReader_ImageListener {
            context: &*ready as *const Ready as *mut c_void,
            onImageAvailable: Some(image_available),
        };
        // SAFETY: the listener is copied; its context lives as long as
        // the reader.
        ok("image listener", unsafe {
            ndk_sys::AImageReader_setImageListener(reader.as_ptr(), &mut listener)
        })?;
        Ok(Self { reader, ready })
    }

    /// Where a producer draws (owned by the reader).
    pub(crate) fn window(&self) -> Result<*mut ANativeWindow, String> {
        let mut window = ptr::null_mut();
        // SAFETY: plain out-parameter call.
        ok("image reader's window", unsafe {
            ndk_sys::AImageReader_getWindow(self.reader.as_ptr(), &mut window)
        })?;
        if window.is_null() {
            return Err("the image reader has no window".into());
        }
        Ok(window)
    }

    /// The newest picture, waiting up to `wait` for one; older ones are
    /// let go. `None` when none came.
    pub(crate) fn next(&self, wait: Duration) -> Option<Picture> {
        let until = Instant::now() + wait;
        loop {
            {
                let mut count = self.ready.count.lock();
                if *count == 0 && self.ready.cv.wait_until(&mut count, until).timed_out() {
                    return None;
                }
                *count = 0;
            }
            let mut image = ptr::null_mut();
            // SAFETY: plain out-parameter call.
            let s = unsafe {
                ndk_sys::AImageReader_acquireLatestImage(self.reader.as_ptr(), &mut image)
            };
            if let (true, Some(image)) = (s.0 == 0, NonNull::new(image)) {
                let picture = picture(image);
                // SAFETY: acquired above, released once.
                unsafe { ndk_sys::AImage_delete(image.as_ptr()) };
                if picture.is_some() {
                    return picture;
                }
            }
            if Instant::now() >= until {
                return None;
            }
        }
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        // SAFETY: ours; the listener is cleared so no callback sees the
        // box after it goes.
        unsafe {
            ndk_sys::AImageReader_setImageListener(self.reader.as_ptr(), ptr::null_mut());
            ndk_sys::AImageReader_delete(self.reader.as_ptr());
        }
    }
}

/// An image's picture, cropped.
fn picture(image: NonNull<AImage>) -> Option<Picture> {
    let image = image.as_ptr();
    let plane = |i: i32| -> Option<(&[u8], usize, usize)> {
        let (mut data, mut len, mut row, mut step) = (ptr::null_mut(), 0, 0, 0);
        // SAFETY: plain out-parameter calls on an acquired image; the
        // data lives until the image is deleted, after this function.
        unsafe {
            ok(
                "plane",
                ndk_sys::AImage_getPlaneData(image, i, &mut data, &mut len),
            )
            .ok()?;
            ok(
                "row stride",
                ndk_sys::AImage_getPlaneRowStride(image, i, &mut row),
            )
            .ok()?;
            ok(
                "pixel stride",
                ndk_sys::AImage_getPlanePixelStride(image, i, &mut step),
            )
            .ok()?;
            if data.is_null() {
                return None;
            }
            Some((
                std::slice::from_raw_parts(data, len as usize),
                row as usize,
                step as usize,
            ))
        }
    };
    let mut crop = ndk_sys::AImageCropRect {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: plain out-parameter call.
    ok("crop", unsafe {
        ndk_sys::AImage_getCropRect(image, &mut crop)
    })
    .ok()?;
    let (y, ys, _) = plane(0)?;
    let (u, uvs, step) = plane(1)?;
    let (v, _, _) = plane(2)?;
    let (left, top) = (
        crop.left.max(0) as usize & !1,
        crop.top.max(0) as usize & !1,
    );
    let width = ((crop.right - crop.left).max(0) as u32) & !1;
    let height = ((crop.bottom - crop.top).max(0) as u32) & !1;
    if width == 0 || height == 0 {
        return None;
    }
    let (cl, ct) = (left / 2 * step, top / 2 * uvs);
    Some(Picture::from_yuv_420_888(
        width,
        height,
        (y.get(top * ys + left..)?, ys),
        (u.get(ct + cl..)?, v.get(ct + cl..)?),
        (uvs, step),
    ))
}
