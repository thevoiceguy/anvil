//! Android: the phone's screen shared through MediaProjection. The app's
//! `ScreenCapture` (Kotlin) holds the projection the user allowed and its
//! virtual display; it registers itself here at start, and anvil-video's
//! `ProjectionScreens` reaches it through this [`Projector`]. Rust calls it
//! from the share's own thread, so each call attaches that thread to the
//! JVM; the object's methods are found through the object, which works
//! from any thread (a class looked up by name from a native thread is not
//! found: that thread sees only the system's classes).

use std::ptr::NonNull;
use std::sync::Arc;

use anvil_video::projection::{self, Projector};
use jni::objects::{GlobalRef, JClass, JIntArray, JObject, JString, JValue};
use jni::{JNIEnv, JavaVM};
use ndk_sys::ANativeWindow;

struct JavaProjector {
    vm: JavaVM,
    capture: GlobalRef,
}

/// A Java exception thrown by the last call, cleared, as an error.
fn thrown(env: &mut JNIEnv, what: &str) -> Option<String> {
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
        return Some(format!("{what}: the app's screen capture failed"));
    }
    None
}

impl Projector for JavaProjector {
    fn display(&self) -> Option<(u32, u32, u32)> {
        let mut env = self.vm.attach_current_thread().ok()?;
        let size = env.call_method(&self.capture, "display", "()[I", &[]);
        if thrown(&mut env, "display").is_some() {
            return None;
        }
        let size = JIntArray::from(size.ok()?.l().ok()?);
        if size.is_null() {
            return None;
        }
        let mut v = [0i32; 3];
        env.get_int_array_region(&size, 0, &mut v).ok()?;
        (v[0] > 0 && v[1] > 0).then(|| (v[0] as u32, v[1] as u32, v[2].max(1) as u32))
    }

    fn start(
        &self,
        window: NonNull<ANativeWindow>,
        width: u32,
        height: u32,
        dpi: u32,
    ) -> Result<(), String> {
        let mut env = self
            .vm
            .attach_current_thread()
            .map_err(|e| format!("the JVM: {e}"))?;
        // SAFETY: a live window (the share's image reader owns it) and this
        // thread's env; the Surface is a new local reference.
        let surface =
            unsafe { ndk_sys::ANativeWindow_toSurface(env.get_raw().cast(), window.as_ptr()) };
        if surface.is_null() {
            return Err("no surface for the screen's pictures".into());
        }
        // SAFETY: the local reference just made.
        let surface = unsafe { JObject::from_raw(surface.cast()) };
        let answer = env.call_method(
            &self.capture,
            "start",
            "(Landroid/view/Surface;III)Ljava/lang/String;",
            &[
                JValue::Object(&surface),
                JValue::Int(width as i32),
                JValue::Int(height as i32),
                JValue::Int(dpi as i32),
            ],
        );
        if let Some(e) = thrown(&mut env, "start") {
            return Err(e);
        }
        let error = JString::from(
            answer
                .map_err(|e| e.to_string())?
                .l()
                .map_err(|e| e.to_string())?,
        );
        if error.is_null() {
            return Ok(());
        }
        Err(env
            .get_string(&error)
            .map(Into::into)
            .unwrap_or_else(|_| "the screen would not share".into()))
    }

    fn stop(&self) {
        let Ok(mut env) = self.vm.attach_current_thread() else {
            return;
        };
        let _ = env.call_method(&self.capture, "stop", "()V", &[]);
        let _ = thrown(&mut env, "stop");
    }
}

/// `AnvilNative.screenCapture(capture)`: the app's `ScreenCapture`, once at
/// start.
#[no_mangle]
pub extern "system" fn Java_com_thevoiceguy_anvil_AnvilNative_screenCapture(
    env: JNIEnv,
    _class: JClass,
    capture: JObject,
) {
    let (Ok(vm), Ok(capture)) = (env.get_java_vm(), env.new_global_ref(capture)) else {
        return;
    };
    projection::set_projector(Arc::new(JavaProjector { vm, capture }));
}

/// `AnvilNative.screenCaptureEnded()`: the system ended the projection
/// (the user stopped it, or the screen locked); the share ends.
#[no_mangle]
pub extern "system" fn Java_com_thevoiceguy_anvil_AnvilNative_screenCaptureEnded(
    _env: JNIEnv,
    _class: JClass,
) {
    projection::projection_ended();
}
