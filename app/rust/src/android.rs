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

// What native code asks of the phone when Dart may not be running: a push
// that woke the app, the token the platform gave, the system's call screen
// (`system.rs`; FCP's `docs/PUSH.md` §2.5). Each blocks its Java thread until
// done and answers `null` or what went wrong; none is called on the main
// thread.

use jni::objects::JString;
use jni::sys::{jlong, jstring};

fn text(env: &mut JNIEnv, s: &JString) -> String {
    env.get_string(s).map(Into::into).unwrap_or_default()
}

/// `null`, or the error as a Java string.
fn outcome(env: &mut JNIEnv, result: anyhow::Result<()>) -> jstring {
    match result {
        Ok(()) => std::ptr::null_mut(),
        Err(e) => env
            .new_string(format!("{e:#}"))
            .map(|s| s.into_raw())
            .unwrap_or(std::ptr::null_mut()),
    }
}

/// `AnvilNative.wake(sessionPath)`: a push came; start the phone from the
/// saved session, or register again if it runs.
#[no_mangle]
pub extern "system" fn Java_com_thevoiceguy_anvil_AnvilNative_wake(
    mut env: JNIEnv,
    _class: JClass,
    session_path: JString,
) -> jstring {
    let session_path = text(&mut env, &session_path);
    let result = crate::api::phone::runtime().block_on(crate::api::phone::wake(session_path));
    outcome(&mut env, result)
}

/// `AnvilNative.setPush(sessionPath, provider, param, token)`: where this
/// install is woken; an empty token is nowhere.
#[no_mangle]
pub extern "system" fn Java_com_thevoiceguy_anvil_AnvilNative_setPush(
    mut env: JNIEnv,
    _class: JClass,
    session_path: JString,
    provider: JString,
    param: JString,
    token: JString,
) -> jstring {
    let session_path = text(&mut env, &session_path);
    let provider = text(&mut env, &provider);
    let param = text(&mut env, &param);
    let token = text(&mut env, &token);
    let result = crate::system::push_address(&provider, &param, &token).and_then(|address| {
        crate::api::phone::runtime().block_on(crate::api::phone::set_push(session_path, address))
    });
    outcome(&mut env, result)
}

/// `AnvilNative.nextCallEvent(timeoutMs)`: the next event for the system's
/// call screen as JSON, or `null` when none came in time.
#[no_mangle]
pub extern "system" fn Java_com_thevoiceguy_anvil_AnvilNative_nextCallEvent(
    env: JNIEnv,
    _class: JClass,
    timeout_ms: jlong,
) -> jstring {
    let within = std::time::Duration::from_millis(timeout_ms.max(0) as u64);
    match crate::system::next_event(within) {
        Some(json) => env
            .new_string(json)
            .map(|s| s.into_raw())
            .unwrap_or(std::ptr::null_mut()),
        None => std::ptr::null_mut(),
    }
}

/// `AnvilNative.act(json)`: what the user did at the system's call screen.
#[no_mangle]
pub extern "system" fn Java_com_thevoiceguy_anvil_AnvilNative_act(
    mut env: JNIEnv,
    _class: JClass,
    json: JString,
) -> jstring {
    let json = text(&mut env, &json);
    let result = crate::api::phone::runtime().block_on(crate::api::phone::act(json));
    outcome(&mut env, result)
}
