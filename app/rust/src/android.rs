//! Android: the JVM and the application context that cpal's Android
//! backend (oboe) reaches through `ndk_context` to list audio devices.
//! A NativeActivity sets that up; a Flutter app does not, and Dart's FFI
//! opens this library with `dlopen`, which runs no `JNI_OnLoad`. So
//! `MainActivity` loads the library itself and hands both over here.

use jni::objects::{JClass, JObject};
use jni::JNIEnv;

/// `AnvilNative.init(context)` in the app's Kotlin. Called once per
/// process, before Dart starts.
#[no_mangle]
pub extern "system" fn Java_com_thevoiceguy_anvil_AnvilNative_init(
    env: JNIEnv,
    _class: JClass,
    context: JObject,
) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let Ok(vm) = env.get_java_vm() else { return };
        let Ok(context) = env.new_global_ref(context) else {
            return;
        };
        // SAFETY: the VM outlives the process's use of it, and the global
        // reference is leaked so the context stays valid for good.
        unsafe {
            ndk_context::initialize_android_context(
                vm.get_java_vm_pointer().cast(),
                context.as_obj().as_raw().cast(),
            );
        }
        std::mem::forget(context);
    });
}
