//! FCP through the C ABI (`anvil-fcp`): signing in, the user's own data as
//! JSON, and the user's live events through a callback.
//!
//! Every function that returns data writes a NUL-terminated UTF-8 JSON
//! string to `*out_json`, owned by the caller and freed with
//! `anvil_string_free`. The JSON is FCP's own shape: a list is `{"data":
//! […], "next_cursor": …, "total": …}`.

use std::ffi::{c_char, c_void, CString};
use std::sync::Arc;

use anvil_fcp::{FcpClient, FcpError, FileTokenStore, TokenStore};
use parking_lot::Mutex;
use tokio::runtime::Runtime;

use crate::{catch_ffi, cstr_to_optional, cstr_to_string, AnvilStatus};

/// An FCP session: opaque to C.
pub struct AnvilFcp {
    runtime: Runtime,
    client: Arc<FcpClient>,
    events: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

/// Called for each live event: its name (`call.answered`, …) and its body
/// as JSON, both valid only during the call. The callback runs on a worker
/// thread of the session's runtime. After the stream ends it is called
/// once more with the name `stream.closed`.
pub type AnvilFcpEventCallback =
    unsafe extern "C" fn(name: *const c_char, body_json: *const c_char, user_data: *mut c_void);

/// The caller's pointer, carried to the events task.
#[derive(Clone, Copy)]
struct UserData(*mut c_void);
// SAFETY: the pointer is the caller's to make safe across threads; the
// documentation says the callback runs on another thread.
unsafe impl Send for UserData {}

fn runtime() -> Result<Runtime, AnvilStatus> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name("anvil-fcp-rt")
        .build()
        .map_err(|_| AnvilStatus::Internal)
}

fn status_of(e: &FcpError) -> AnvilStatus {
    match e {
        FcpError::Unreachable(_) => AnvilStatus::Transport,
        FcpError::SignedOut => AnvilStatus::AuthRejected,
        FcpError::TotpRequired => AnvilStatus::TotpRequired,
        FcpError::Refused {
            status: 401 | 403, ..
        } => AnvilStatus::AuthRejected,
        FcpError::Refused { status: 404, .. } => AnvilStatus::NotFound,
        FcpError::Invalid(_) => AnvilStatus::Config,
        _ => AnvilStatus::Failed,
    }
}

/// Write `value` as JSON to `*out`.
unsafe fn write_json(value: &impl serde::Serialize, out: *mut *mut c_char) -> AnvilStatus {
    let Ok(json) = serde_json::to_string(value) else {
        return AnvilStatus::Internal;
    };
    let Ok(c) = CString::new(json) else {
        return AnvilStatus::Internal;
    };
    unsafe { *out = c.into_raw() };
    AnvilStatus::Ok
}

/// Run `f` against the session and write what it answers to `*out_json`.
unsafe fn query<T, F, Fut>(handle: *mut AnvilFcp, out_json: *mut *mut c_char, f: F) -> AnvilStatus
where
    T: serde::Serialize,
    F: FnOnce(Arc<FcpClient>) -> Fut,
    Fut: std::future::Future<Output = Result<T, FcpError>>,
{
    let Some(fcp) = (unsafe { handle.as_ref() }) else {
        return AnvilStatus::NullArgument;
    };
    if out_json.is_null() {
        return AnvilStatus::NullArgument;
    }
    match fcp.runtime.block_on(f(Arc::clone(&fcp.client))) {
        Ok(value) => unsafe { write_json(&value, out_json) },
        Err(e) => status_of(&e),
    }
}

fn open(runtime: Runtime, store: Arc<FileTokenStore>, out: *mut *mut AnvilFcp) -> AnvilStatus {
    let session = match store.load() {
        Ok(Some(session)) => session,
        Ok(None) => return AnvilStatus::AuthRejected,
        Err(_) => return AnvilStatus::Config,
    };
    let client = Arc::new(FcpClient::new(session, Some(store as Arc<dyn TokenStore>)));
    let handle = Box::new(AnvilFcp {
        runtime,
        client,
        events: Mutex::new(None),
    });
    unsafe { *out = Box::into_raw(handle) };
    AnvilStatus::Ok
}

/// Open the FCP session kept at `session_path` (a sign-in wrote it). On
/// success writes the handle to `*out`; `AuthRejected` when no session is
/// kept there.
///
/// # Safety
/// `session_path` a NUL-terminated string; `out` writable.
#[no_mangle]
pub unsafe extern "C" fn anvil_fcp_open(
    session_path: *const c_char,
    out: *mut *mut AnvilFcp,
) -> AnvilStatus {
    catch_ffi(|| unsafe {
        if out.is_null() {
            return AnvilStatus::NullArgument;
        }
        let path = match cstr_to_string(session_path) {
            Ok(p) => p,
            Err(s) => return s,
        };
        let runtime = match runtime() {
            Ok(r) => r,
            Err(s) => return s,
        };
        open(runtime, Arc::new(FileTokenStore::new(path)), out)
    })
}

/// Sign in to FCP with a username and password, keep the session at
/// `session_path`, and open it. `place` is the server
/// (`https://pbx.example.com`) or the person's email address. `totp` may be
/// NULL; `TotpRequired` asks for it.
///
/// # Safety
/// The strings NUL-terminated (`totp` may be NULL); `out` writable.
#[no_mangle]
pub unsafe extern "C" fn anvil_fcp_sign_in(
    place: *const c_char,
    username: *const c_char,
    password: *const c_char,
    totp: *const c_char,
    session_path: *const c_char,
    out: *mut *mut AnvilFcp,
) -> AnvilStatus {
    catch_ffi(|| unsafe {
        if out.is_null() {
            return AnvilStatus::NullArgument;
        }
        let args = (|| {
            Ok::<_, AnvilStatus>((
                cstr_to_string(place)?,
                cstr_to_string(username)?,
                cstr_to_string(password)?,
                cstr_to_optional(totp)?,
                cstr_to_string(session_path)?,
            ))
        })();
        let (place, username, password, totp, path) = match args {
            Ok(a) => a,
            Err(s) => return s,
        };
        let runtime = match runtime() {
            Ok(r) => r,
            Err(s) => return s,
        };
        let store = Arc::new(FileTokenStore::new(path));
        // The same install signing in again keeps its device.
        let install = store
            .load()
            .ok()
            .flatten()
            .map(|s| s.client.install_id)
            .unwrap_or_else(anvil_fcp::AppClient::new_install_id);
        let signed_in = runtime.block_on(async {
            let server = anvil_fcp::discover(&place).await?;
            anvil_fcp::password_sign_in(
                server,
                anvil_fcp::AppClient::this_machine(install),
                &username,
                &password,
                totp.as_deref(),
            )
            .await
        });
        let session = match signed_in {
            Ok(s) => s,
            Err(e) => return status_of(&e),
        };
        if store.save(&session).is_err() {
            return AnvilStatus::Config;
        }
        open(runtime, store, out)
    })
}

/// Close the session handle (the session stays signed in and kept).
///
/// # Safety
/// `handle` from `anvil_fcp_open`/`anvil_fcp_sign_in`, not used after.
#[no_mangle]
pub unsafe extern "C" fn anvil_fcp_close(handle: *mut AnvilFcp) {
    let _ = catch_ffi(|| unsafe {
        if !handle.is_null() {
            let fcp = Box::from_raw(handle);
            if let Some(task) = fcp.events.lock().take() {
                task.abort();
            }
            drop(fcp);
        }
        AnvilStatus::Ok
    });
}

/// Sign out: the app's session ends at FCP.
///
/// # Safety
/// `handle` valid.
#[no_mangle]
pub unsafe extern "C" fn anvil_fcp_sign_out(handle: *mut AnvilFcp) -> AnvilStatus {
    catch_ffi(|| unsafe {
        let Some(fcp) = handle.as_ref() else {
            return AnvilStatus::NullArgument;
        };
        match fcp.runtime.block_on(fcp.client.sign_out()) {
            Ok(()) => AnvilStatus::Ok,
            Err(e) => status_of(&e),
        }
    })
}

/// The user's call history, newest first: `limit` (0 for FCP's default),
/// only missed calls when `missed != 0`, from `cursor` (NULL for the
/// first page).
///
/// # Safety
/// `handle` valid; `cursor` NULL or NUL-terminated; `out_json` writable.
#[no_mangle]
pub unsafe extern "C" fn anvil_fcp_calls(
    handle: *mut AnvilFcp,
    limit: u32,
    missed: u8,
    cursor: *const c_char,
    out_json: *mut *mut c_char,
) -> AnvilStatus {
    catch_ffi(|| unsafe {
        let cursor = match cstr_to_optional(cursor) {
            Ok(c) => c,
            Err(s) => return s,
        };
        let query = anvil_fcp::CallQuery {
            limit: (limit > 0).then_some(limit),
            cursor,
            missed: missed != 0,
        };
        self::query(handle, out_json, |c| async move { c.calls(&query).await })
    })
}

/// The tenant's directory with presence, matching `search` (NULL for
/// everyone), from `cursor`.
///
/// # Safety
/// `handle` valid; strings NULL or NUL-terminated; `out_json` writable.
#[no_mangle]
pub unsafe extern "C" fn anvil_fcp_directory(
    handle: *mut AnvilFcp,
    search: *const c_char,
    cursor: *const c_char,
    out_json: *mut *mut c_char,
) -> AnvilStatus {
    catch_ffi(|| unsafe {
        let (search, cursor) = match (cstr_to_optional(search), cstr_to_optional(cursor)) {
            (Ok(s), Ok(c)) => (s, c),
            (Err(s), _) | (_, Err(s)) => return s,
        };
        query(handle, out_json, |c| async move {
            c.directory(search.as_deref(), None, cursor.as_deref())
                .await
        })
    })
}

/// The user's voicemail messages, newest first, from `cursor`.
/// `NotFound` when the user has no voicemail box.
///
/// # Safety
/// `handle` valid; `cursor` NULL or NUL-terminated; `out_json` writable.
#[no_mangle]
pub unsafe extern "C" fn anvil_fcp_voicemail(
    handle: *mut AnvilFcp,
    cursor: *const c_char,
    out_json: *mut *mut c_char,
) -> AnvilStatus {
    catch_ffi(|| unsafe {
        let cursor = match cstr_to_optional(cursor) {
            Ok(c) => c,
            Err(s) => return s,
        };
        query(handle, out_json, |c| async move {
            c.voicemail(None, cursor.as_deref()).await
        })
    })
}

/// The voicemail box's counts. `NotFound` when the user has none.
///
/// # Safety
/// `handle` valid; `out_json` writable.
#[no_mangle]
pub unsafe extern "C" fn anvil_fcp_voicemail_stats(
    handle: *mut AnvilFcp,
    out_json: *mut *mut c_char,
) -> AnvilStatus {
    catch_ffi(|| unsafe {
        query(
            handle,
            out_json,
            |c| async move { c.voicemail_stats().await },
        )
    })
}

/// Mark a voicemail message `heard`, `saved` or `new`; the message after.
///
/// # Safety
/// `handle` valid; strings NUL-terminated; `out_json` writable.
#[no_mangle]
pub unsafe extern "C" fn anvil_fcp_set_voicemail_status(
    handle: *mut AnvilFcp,
    message_id: *const c_char,
    status: *const c_char,
    out_json: *mut *mut c_char,
) -> AnvilStatus {
    catch_ffi(|| unsafe {
        let (id, status) = match (cstr_to_string(message_id), cstr_to_string(status)) {
            (Ok(i), Ok(s)) => (i, s),
            (Err(s), _) | (_, Err(s)) => return s,
        };
        query(handle, out_json, |c| async move {
            c.set_voicemail_status(&id, &status).await
        })
    })
}

/// Delete a voicemail message.
///
/// # Safety
/// `handle` valid; `message_id` NUL-terminated.
#[no_mangle]
pub unsafe extern "C" fn anvil_fcp_delete_voicemail(
    handle: *mut AnvilFcp,
    message_id: *const c_char,
) -> AnvilStatus {
    catch_ffi(|| unsafe {
        let Some(fcp) = handle.as_ref() else {
            return AnvilStatus::NullArgument;
        };
        let id = match cstr_to_string(message_id) {
            Ok(i) => i,
            Err(s) => return s,
        };
        match fcp.runtime.block_on(fcp.client.delete_voicemail(&id)) {
            Ok(()) => AnvilStatus::Ok,
            Err(e) => status_of(&e),
        }
    })
}

/// The user's calling settings (do not disturb, call waiting, forwards).
///
/// # Safety
/// `handle` valid; `out_json` writable.
#[no_mangle]
pub unsafe extern "C" fn anvil_fcp_calling(
    handle: *mut AnvilFcp,
    out_json: *mut *mut c_char,
) -> AnvilStatus {
    catch_ffi(|| unsafe { query(handle, out_json, |c| async move { c.calling().await }) })
}

/// Change the calling settings: `update_json` names only what changes
/// (`{"dnd": true}`; a forward is cleared with `""`). The settings after.
///
/// # Safety
/// `handle` valid; `update_json` NUL-terminated; `out_json` writable.
#[no_mangle]
pub unsafe extern "C" fn anvil_fcp_set_calling(
    handle: *mut AnvilFcp,
    update_json: *const c_char,
    out_json: *mut *mut c_char,
) -> AnvilStatus {
    catch_ffi(|| unsafe {
        let update = match cstr_to_string(update_json) {
            Ok(u) => u,
            Err(s) => return s,
        };
        let Ok(update) = serde_json::from_str::<anvil_fcp::CallingUpdate>(&update) else {
            return AnvilStatus::Config;
        };
        query(
            handle,
            out_json,
            |c| async move { c.set_calling(&update).await },
        )
    })
}

/// Start delivering the user's live events to `callback`, replacing any
/// stream already running. Returns once the stream is open.
///
/// # Safety
/// `handle` valid; `callback` valid for as long as the stream runs
/// (`anvil_fcp_events_stop` or `anvil_fcp_close`); `user_data` passed back.
#[no_mangle]
pub unsafe extern "C" fn anvil_fcp_events_start(
    handle: *mut AnvilFcp,
    callback: Option<
        unsafe extern "C" fn(name: *const c_char, body_json: *const c_char, user_data: *mut c_void),
    >,
    user_data: *mut c_void,
) -> AnvilStatus {
    catch_ffi(|| unsafe {
        let (Some(fcp), Some(callback)) = (handle.as_ref(), callback) else {
            return AnvilStatus::NullArgument;
        };
        let mut events = match fcp.runtime.block_on(fcp.client.events()) {
            Ok(e) => e,
            Err(e) => return status_of(&e),
        };
        let user_data = UserData(user_data);
        let task = fcp.runtime.spawn(async move {
            while let Some(event) = events.next().await {
                deliver(callback, user_data, &event.name, &event.body.to_string());
            }
            deliver(callback, user_data, "stream.closed", "{}");
        });
        if let Some(old) = fcp.events.lock().replace(task) {
            old.abort();
        }
        AnvilStatus::Ok
    })
}

/// Hand one event to the caller's callback.
fn deliver(callback: AnvilFcpEventCallback, user_data: UserData, name: &str, body: &str) {
    let (Ok(name), Ok(body)) = (CString::new(name), CString::new(body)) else {
        return;
    };
    // SAFETY: the caller keeps the callback valid while the stream runs,
    // and the strings live for the call.
    unsafe { callback(name.as_ptr(), body.as_ptr(), user_data.0) };
}

/// Stop the live events.
///
/// # Safety
/// `handle` valid.
#[no_mangle]
pub unsafe extern "C" fn anvil_fcp_events_stop(handle: *mut AnvilFcp) -> AnvilStatus {
    catch_ffi(|| unsafe {
        let Some(fcp) = handle.as_ref() else {
            return AnvilStatus::NullArgument;
        };
        if let Some(task) = fcp.events.lock().take() {
            task.abort();
        }
        AnvilStatus::Ok
    })
}

/// Free a string an `anvil_fcp_*` function wrote.
///
/// # Safety
/// `s` from this library, or NULL; not used after.
#[no_mangle]
pub unsafe extern "C" fn anvil_string_free(s: *mut c_char) {
    if !s.is_null() {
        drop(unsafe { CString::from_raw(s) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fcp_errors_become_statuses() {
        assert_eq!(status_of(&FcpError::SignedOut), AnvilStatus::AuthRejected);
        assert_eq!(
            status_of(&FcpError::TotpRequired),
            AnvilStatus::TotpRequired
        );
        assert_eq!(
            status_of(&FcpError::Refused {
                status: 404,
                message: String::new()
            }),
            AnvilStatus::NotFound
        );
        assert_eq!(
            status_of(&FcpError::Unreachable("x".into())),
            AnvilStatus::Transport
        );
    }

    #[test]
    fn a_missing_session_is_not_signed_in() {
        let path = std::env::temp_dir().join(format!("anvil-ffi-none-{}.json", std::process::id()));
        let path = CString::new(path.to_string_lossy().into_owned()).unwrap();
        let mut handle: *mut AnvilFcp = std::ptr::null_mut();
        let status = unsafe { anvil_fcp_open(path.as_ptr(), &mut handle) };
        assert_eq!(status, AnvilStatus::AuthRejected);
        assert!(handle.is_null());
    }

    #[test]
    fn json_out_is_freed_by_the_caller() {
        let mut out: *mut c_char = std::ptr::null_mut();
        let status = unsafe { write_json(&serde_json::json!({"dnd": true}), &mut out) };
        assert_eq!(status, AnvilStatus::Ok);
        let text = unsafe { std::ffi::CStr::from_ptr(out) }
            .to_str()
            .unwrap()
            .to_string();
        assert_eq!(text, r#"{"dnd":true}"#);
        unsafe { anvil_string_free(out) };
    }
}
