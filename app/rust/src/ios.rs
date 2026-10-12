//! iOS: what the app's Swift asks of the phone when Dart may not be running
//! — a VoIP push that woke the app, the token PushKit gave, CallKit's call
//! screen (`system.rs`; FCP's `docs/PUSH.md` §2.5) — as a C ABI declared in
//! `ios/Runner/Runner-Bridging-Header.h`.
//!
//! Each call blocks until done and answers null, or what went wrong as a
//! string the caller frees with [`anvil_string_free`]; none is called on the
//! main thread. Built for the host's tests too.

use std::ffi::{c_char, CStr, CString};

fn text(s: *const c_char) -> String {
    if s.is_null() {
        return String::new();
    }
    // SAFETY: Swift passes a NUL-terminated string that outlives the call.
    unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned()
}

fn owned(s: String) -> *mut c_char {
    // A NUL inside is not expected; dropped rather than failing the answer.
    CString::new(s.replace('\0', ""))
        .map(CString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}

/// Null, or the error.
fn outcome(result: anyhow::Result<()>) -> *mut c_char {
    match result {
        Ok(()) => std::ptr::null_mut(),
        Err(e) => owned(format!("{e:#}")),
    }
}

/// A push came: start the phone from the saved session, or register again
/// if it runs.
#[no_mangle]
pub extern "C" fn anvil_wake(session_path: *const c_char) -> *mut c_char {
    let session_path = text(session_path);
    outcome(crate::api::phone::runtime().block_on(crate::api::phone::wake(session_path)))
}

/// Where this install is woken; an empty token is nowhere.
#[no_mangle]
pub extern "C" fn anvil_set_push(
    session_path: *const c_char,
    provider: *const c_char,
    param: *const c_char,
    token: *const c_char,
) -> *mut c_char {
    let session_path = text(session_path);
    let result =
        crate::system::push_address(&text(provider), &text(param), &text(token)).and_then(|a| {
            crate::api::phone::runtime().block_on(crate::api::phone::set_push(session_path, a))
        });
    outcome(result)
}

/// The next event for the system's call screen as JSON, or null when none
/// came within `timeout_ms`.
#[no_mangle]
pub extern "C" fn anvil_next_call_event(timeout_ms: i64) -> *mut c_char {
    let within = std::time::Duration::from_millis(timeout_ms.max(0) as u64);
    crate::system::next_event(within)
        .map(owned)
        .unwrap_or(std::ptr::null_mut())
}

/// What the user did at the system's call screen, as JSON.
#[no_mangle]
pub extern "C" fn anvil_act(json: *const c_char) -> *mut c_char {
    let json = text(json);
    outcome(crate::api::phone::runtime().block_on(crate::api::phone::act(json)))
}

/// Free a string this library answered with.
#[no_mangle]
pub extern "C" fn anvil_string_free(s: *mut c_char) {
    if !s.is_null() {
        // SAFETY: made by `CString::into_raw` here, and freed once.
        drop(unsafe { CString::from_raw(s) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(s: &str) -> CString {
        CString::new(s).unwrap()
    }

    /// The answer as Swift reads it, freed.
    fn answer(s: *mut c_char) -> Option<String> {
        if s.is_null() {
            return None;
        }
        let text = unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned();
        anvil_string_free(s);
        Some(text)
    }

    #[test]
    fn swift_keeps_the_push_address_through_the_abi() {
        // A directory not made yet: PushKit may answer before sign-in.
        let dir = std::env::temp_dir().join(format!("anvil-ios-{}", std::process::id()));
        let session = dir.join("support").join("session.json");
        let session_c = c(session.to_str().unwrap());
        let token = "8f2c00a91b";
        let param = "ABCDE12345.com.thevoiceguy.anvil.voip";
        assert_eq!(
            answer(anvil_set_push(
                session_c.as_ptr(),
                c("apns").as_ptr(),
                c(param).as_ptr(),
                c(token).as_ptr(),
            )),
            None
        );
        let saved = crate::system::load_push(&session).expect("kept");
        assert_eq!(saved.provider, anvil_core::PushProvider::Apns);
        assert_eq!((saved.param.as_str(), saved.prid.as_str()), (param, token));

        // An empty token is nowhere.
        assert_eq!(
            answer(anvil_set_push(
                session_c.as_ptr(),
                c("apns").as_ptr(),
                c(param).as_ptr(),
                c("").as_ptr(),
            )),
            None
        );
        assert_eq!(crate::system::load_push(&session), None);

        let refused = answer(anvil_set_push(
            session_c.as_ptr(),
            c("pigeon").as_ptr(),
            c(param).as_ptr(),
            c(token).as_ptr(),
        ));
        assert!(refused.unwrap().contains("pigeon"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn without_a_phone_swift_hears_why() {
        let woken = answer(anvil_wake(c("/nonexistent/anvil/session.json").as_ptr()));
        assert!(woken.is_some(), "no session, no phone");
        let acted = answer(anvil_act(c(r#"{"action":"answer","id":1}"#).as_ptr()));
        assert!(acted.is_some(), "no phone to act on");
        assert_eq!(answer(anvil_next_call_event(0)), None);
        anvil_string_free(std::ptr::null_mut());
    }
}
