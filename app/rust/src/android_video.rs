//! Android: video tiles on Flutter textures. The runner's plugin
//! (`VideoTextures.kt`) makes a `SurfaceProducer` per tile and a slot here
//! for its surface, then hands Dart the slot and [`present`], as the other
//! platforms' plugins hand over theirs (`api/video.rs`). The producer's
//! surface comes and goes (Flutter lets it go while the app is away); the
//! slot holds whichever is current, and a picture with none is let go.

use std::ffi::c_void;
use std::ptr::{self, NonNull};

use jni::objects::{JClass, JObject};
use jni::sys::jlong;
use jni::JNIEnv;
use ndk_sys::{ANativeWindow, ANativeWindow_Buffer, ANativeWindow_LegacyFormat};
use parking_lot::Mutex;

/// A tile's surface, while it has one.
#[derive(Default)]
struct Slot {
    window: Mutex<Option<Window>>,
}

/// A surface as the NDK holds it, and the size its buffers were given.
struct Window {
    ptr: NonNull<ANativeWindow>,
    size: (u32, u32),
}

// SAFETY: an ANativeWindow may be used from any thread.
unsafe impl Send for Window {}

impl Drop for Window {
    fn drop(&mut self) {
        // SAFETY: the reference `ANativeWindow_fromSurface` gave, let go once.
        unsafe { ndk_sys::ANativeWindow_release(self.ptr.as_ptr()) };
    }
}

const RGBA: i32 = ANativeWindow_LegacyFormat::WINDOW_FORMAT_RGBA_8888.0 as i32;
const RGBX: i32 = ANativeWindow_LegacyFormat::WINDOW_FORMAT_RGBX_8888.0 as i32;

/// Draw `rgba` (`width` × `height`) on the slot's surface: what Dart hands
/// Rust as the texture plugin's `present`, with the slot as its texture.
unsafe extern "C" fn present(slot: *mut c_void, rgba: *const u8, width: u32, height: u32) {
    if slot.is_null() || rgba.is_null() || width == 0 || height == 0 {
        return;
    }
    // SAFETY: the slot `videoSlot` made, which `videoRelease` frees only
    // after Dart has detached the tile, so after the last call here.
    let slot = unsafe { &*(slot as *const Slot) };
    let mut window = slot.window.lock();
    let Some(window) = window.as_mut() else {
        return;
    };
    let ptr = window.ptr.as_ptr();
    // SAFETY: plain calls on a window we hold a reference to; the buffer
    // locked is written within its stride and height, then posted.
    unsafe {
        if window.size != (width, height) {
            if ndk_sys::ANativeWindow_setBuffersGeometry(ptr, width as i32, height as i32, RGBA)
                != 0
            {
                return;
            }
            window.size = (width, height);
        }
        let mut buffer: ANativeWindow_Buffer = std::mem::zeroed();
        if ndk_sys::ANativeWindow_lock(ptr, &mut buffer, ptr::null_mut()) != 0 {
            return;
        }
        if (buffer.format == RGBA || buffer.format == RGBX)
            && buffer.width as u32 >= width
            && buffer.height as u32 >= height
            && !buffer.bits.is_null()
        {
            let row = width as usize * 4;
            let stride = buffer.stride as usize * 4;
            let to = buffer.bits as *mut u8;
            for y in 0..height as usize {
                ptr::copy_nonoverlapping(rgba.add(y * row), to.add(y * stride), row);
            }
        }
        ndk_sys::ANativeWindow_unlockAndPost(ptr);
    }
}

/// `AnvilNative.videoSlot()`: a slot for a new tile's surface.
#[no_mangle]
pub extern "system" fn Java_com_thevoiceguy_anvil_AnvilNative_videoSlot(
    _env: JNIEnv,
    _class: JClass,
) -> jlong {
    Box::into_raw(Box::<Slot>::default()) as jlong
}

/// `AnvilNative.videoSurface(slot, surface)`: the tile's surface now, or
/// none (`null`) while Flutter has let it go.
#[no_mangle]
pub extern "system" fn Java_com_thevoiceguy_anvil_AnvilNative_videoSurface(
    env: JNIEnv,
    _class: JClass,
    slot: jlong,
    surface: JObject,
) {
    if slot == 0 {
        return;
    }
    // SAFETY: a slot `videoSlot` made and `videoRelease` has not freed.
    let slot = unsafe { &*(slot as *const Slot) };
    let window = if surface.is_null() {
        None
    } else {
        // SAFETY: a live `android.view.Surface` from the calling thread's
        // env; the window it gives carries a reference of its own.
        NonNull::new(unsafe {
            ndk_sys::ANativeWindow_fromSurface(env.get_raw().cast(), surface.as_raw().cast())
        })
        .map(|ptr| Window { ptr, size: (0, 0) })
    };
    // The old window is let go after the lock, outside any picture.
    let old = std::mem::replace(&mut *slot.window.lock(), window);
    drop(old);
}

/// `AnvilNative.videoRelease(slot)`: the tile is gone (Dart detached it).
#[no_mangle]
pub extern "system" fn Java_com_thevoiceguy_anvil_AnvilNative_videoRelease(
    _env: JNIEnv,
    _class: JClass,
    slot: jlong,
) {
    if slot != 0 {
        // SAFETY: made by `videoSlot`, freed once, after its last picture.
        drop(unsafe { Box::from_raw(slot as *mut Slot) });
    }
}

/// `AnvilNative.videoPresent()`: the address of [`present`], for Dart.
#[no_mangle]
pub extern "system" fn Java_com_thevoiceguy_anvil_AnvilNative_videoPresent(
    _env: JNIEnv,
    _class: JClass,
) -> jlong {
    present as unsafe extern "C" fn(*mut c_void, *const u8, u32, u32) as usize as jlong
}
