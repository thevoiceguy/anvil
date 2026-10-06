//! # anvil-ffi
//!
//! C ABI for Anvil. Consumed by iOS (Swift / ObjC), Android (Kotlin / Java
//! via JNI), and native desktop wrappers that prefer FFI over a Rust
//! dependency.
//!
//! ## Threading
//!
//! `anvil_start` constructs a multi-threaded Tokio runtime and pins it to
//! the returned `AnvilHandle`. Every public entrypoint that runs `async`
//! work blocks on that runtime via `block_on`. C callers can therefore
//! treat each function as a normal blocking call — no manual runtime
//! plumbing required from Swift / Kotlin / etc.
//!
//! ## Safety
//!
//! Every entrypoint wraps its body in `std::panic::catch_unwind`, returning
//! `AnvilStatus::INTERNAL` on panic. Panics never cross the FFI boundary.
//!
//! Pointers handed back to the caller are owned by the caller; the
//! `_destroy` family frees them. Handles are opaque: callers pass the
//! pointer back unchanged.
//!
//! ## Status codes
//!
//! All entrypoints return a 32-bit status code. `AnvilStatus::OK` means
//! success; any other value is failure. There's no out-of-band error
//! channel because most consumers cross-language don't have a clean way
//! to surface a thread-local error.

#![allow(clippy::missing_safety_doc)]

mod audio;
mod events;
pub mod fcp;
mod handle;

pub use events::{
    AnvilCodec, AnvilEndReason, AnvilEvent, AnvilEventCallback, AnvilEventKind, AnvilMediaStats,
    AnvilRegState,
};

use std::ffi::{c_char, CStr};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;

use anvil_core::{AccountConfig, AnvilConfig, BrandConfig, MediaConfig, Transport};

pub use handle::AnvilHandle;

/// Status codes returned by every entrypoint. Stable across versions —
/// don't renumber.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnvilStatus {
    /// Success.
    Ok = 0,
    /// Caller passed a null pointer where a valid one was required.
    NullArgument = 1,
    /// Caller passed an invalid handle (e.g. already destroyed).
    InvalidHandle = 2,
    /// Configuration was malformed (bad SIP URI, unparseable bind
    /// address, etc.).
    Config = 3,
    /// Network / transport error (DNS, bind, send, etc.).
    Transport = 4,
    /// Authentication rejected by the peer.
    AuthRejected = 5,
    /// Internal Rust panic was caught at the boundary.
    Internal = 6,
    /// Generic "operation failed" — usually means the wrapped Anvil
    /// method returned an error that doesn't map to a more specific
    /// code.
    Failed = 7,
    /// FCP has no such thing for this user (no voicemail box, no such
    /// message).
    NotFound = 8,
    /// Signing in needs a one-time code: call again with `totp`.
    TotpRequired = 9,
}

/// SIP transport selector for the C API.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnvilTransport {
    Udp = 0,
    Tcp = 1,
    Tls = 2,
}

impl AnvilTransport {
    fn into_core(self) -> Transport {
        match self {
            AnvilTransport::Udp => Transport::Udp,
            AnvilTransport::Tcp => Transport::Tcp,
            AnvilTransport::Tls => Transport::Tls,
        }
    }
}

/// Configuration handed to `anvil_start`. Pointer fields point to
/// NUL-terminated UTF-8 strings owned by the caller; `anvil_start` makes
/// owned copies before returning.
#[repr(C)]
pub struct AnvilConfigC {
    /// Address-of-record, e.g. `sip:alice@example.com`. Required.
    pub aor: *const c_char,
    /// Registrar host or full SIP URI, e.g. `sip:example.com:5060`. Required.
    pub registrar: *const c_char,
    /// SIP username. Required.
    pub username: *const c_char,
    /// SIP password. Required (pass empty string for no auth).
    pub password: *const c_char,
    /// Transport selector.
    pub transport: AnvilTransport,
    /// Local bind address, e.g. `0.0.0.0:0`. Pass NULL for the default.
    pub bind_addr: *const c_char,
    /// STUN server (`host:port`) for public-IP discovery. Pass NULL to skip.
    pub stun: *const c_char,
    /// User-Agent header value. Pass NULL for the Anvil default.
    pub user_agent: *const c_char,
    /// Optional audio host. Pass NULL for signaling-only (every audio
    /// open call errors out cleanly). When non-NULL, the pointee is
    /// read by value during `anvil_start` and the function pointers
    /// inside it must remain valid for the handle's lifetime.
    pub audio_host: *const audio::AnvilAudioHostC,
}

// ─── lifecycle ──────────────────────────────────────────────────────────────

/// Start the softphone. On success writes a non-null handle to `*out` and
/// returns `Ok`. On failure leaves `*out` untouched.
///
/// # Safety
/// `cfg` must point to a valid `AnvilConfigC` whose string fields are
/// NUL-terminated UTF-8 (or NULL where the docs allow). `out` must point
/// to writable memory holding `*mut AnvilHandle`.
#[no_mangle]
pub unsafe extern "C" fn anvil_start(
    cfg: *const AnvilConfigC,
    out: *mut *mut AnvilHandle,
) -> AnvilStatus {
    catch_ffi(|| unsafe {
        if cfg.is_null() || out.is_null() {
            return AnvilStatus::NullArgument;
        }
        let cfg = &*cfg;

        let account = match build_account(cfg) {
            Ok(a) => a,
            Err(s) => return s,
        };

        // Pick an audio host: caller-supplied via the C struct, or the
        // null host for signaling-only operation.
        let audio: Box<dyn anvil_core::audio::AudioHost> = if cfg.audio_host.is_null() {
            Box::new(audio::NullAudioHost)
        } else {
            Box::new(audio::CHostAudioHost::from_c(cfg.audio_host))
        };

        let anvil_cfg = AnvilConfig {
            account,
            media: MediaConfig::default(),
            audio,
            brand: BrandConfig::default(),
        };

        match handle::AnvilHandle::start(anvil_cfg) {
            Ok(handle) => {
                *out = Box::into_raw(Box::new(handle));
                AnvilStatus::Ok
            }
            Err(e) => {
                tracing::warn!(%e, "anvil_start failed");
                map_anvil_err(&e)
            }
        }
    })
}

/// Tear the softphone down and free the handle. Idempotent on a NULL
/// argument; otherwise the handle must have come from `anvil_start` and
/// must not have been destroyed already.
///
/// # Safety
/// After this call returns, `handle` is invalid and must not be used
/// again. Free even if no other entrypoint succeeded — `anvil_start` may
/// have allocated state before failing.
#[no_mangle]
pub unsafe extern "C" fn anvil_destroy(handle: *mut AnvilHandle) {
    if handle.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let owned = unsafe { Box::from_raw(handle) };
        owned.shutdown();
    }));
}

// ─── registration ───────────────────────────────────────────────────────────

/// Send REGISTER. Blocks until the registrar responds (or the request times
/// out at the SIP layer).
///
/// # Safety
/// `handle` must be a valid pointer returned by `anvil_start` and not yet
/// destroyed.
#[no_mangle]
pub unsafe extern "C" fn anvil_register(handle: *mut AnvilHandle) -> AnvilStatus {
    catch_ffi(|| unsafe {
        let Some(handle) = handle.as_mut() else {
            return AnvilStatus::NullArgument;
        };
        match handle.block_on(handle.anvil().register()) {
            Ok(()) => AnvilStatus::Ok,
            Err(e) => map_anvil_err(&e),
        }
    })
}

/// Register a callback to receive Anvil events. Pass `NULL` for `cb` to
/// clear a previously-registered callback. `user_data` is passed
/// through unchanged on every invocation; the caller is responsible for
/// its lifetime.
///
/// The callback fires from a worker thread of the FFI runtime. Strings
/// inside the `AnvilEvent` are valid only for the call's duration —
/// copy them if needed past return. Calling other `anvil_*` functions
/// from inside the callback is allowed but will burn a runtime worker;
/// dispatch to your own thread for non-trivial work.
///
/// # Safety
/// `handle` must be a valid pointer returned by `anvil_start`.
#[no_mangle]
pub unsafe extern "C" fn anvil_set_event_callback(
    handle: *mut AnvilHandle,
    cb: Option<
        unsafe extern "C" fn(event: *const events::AnvilEvent, user_data: *mut std::ffi::c_void),
    >,
    user_data: *mut std::ffi::c_void,
) -> AnvilStatus {
    catch_ffi(|| unsafe {
        let Some(handle) = handle.as_ref() else {
            return AnvilStatus::NullArgument;
        };
        let slot = cb.map(|cb| events::CallbackSlot { cb, user_data });
        handle.set_callback(slot);
        AnvilStatus::Ok
    })
}

// ─── call control ───────────────────────────────────────────────────────────

/// Place an outgoing call to `target` (a SIP URI). On success writes the
/// call's runtime ID to `*out_call_id` for later use with
/// `anvil_hangup` / `anvil_send_dtmf` / `anvil_hold`.
///
/// Returns as soon as the INVITE transaction is started; observe
/// `CallRinging`, `CallEstablished`, and `CallEnded` events to track
/// the lifecycle.
///
/// # Safety
/// `handle` must be valid; `target` must be a NUL-terminated UTF-8
/// string; `out_call_id` must point to writable `uint64_t` storage.
#[no_mangle]
pub unsafe extern "C" fn anvil_place_call(
    handle: *mut AnvilHandle,
    target: *const c_char,
    out_call_id: *mut u64,
) -> AnvilStatus {
    catch_ffi(|| unsafe {
        let Some(handle) = handle.as_mut() else {
            return AnvilStatus::NullArgument;
        };
        if out_call_id.is_null() {
            return AnvilStatus::NullArgument;
        }
        let target = match cstr_to_string(target) {
            Ok(s) => s,
            Err(s) => return s,
        };
        match handle.block_on(handle.anvil().place_call(&target)) {
            Ok(call) => {
                *out_call_id = call.0;
                AnvilStatus::Ok
            }
            Err(e) => map_anvil_err(&e),
        }
    })
}

/// Answer an incoming call signalled by `IncomingCall`.
///
/// # Safety
/// `handle` must be valid; `call_id` must come from an `IncomingCall`
/// event delivered via the registered callback.
#[no_mangle]
pub unsafe extern "C" fn anvil_answer(handle: *mut AnvilHandle, call_id: u64) -> AnvilStatus {
    catch_ffi(|| unsafe {
        let Some(handle) = handle.as_mut() else {
            return AnvilStatus::NullArgument;
        };
        match handle.block_on(handle.anvil().answer(anvil_core::CallId(call_id))) {
            Ok(()) => AnvilStatus::Ok,
            Err(e) => map_anvil_err(&e),
        }
    })
}

/// Reject an incoming call with `code` (typical: 486 Busy Here, 603 Decline).
///
/// # Safety
/// `handle` must be valid; `call_id` must match an `IncomingCall`.
#[no_mangle]
pub unsafe extern "C" fn anvil_reject(
    handle: *mut AnvilHandle,
    call_id: u64,
    code: u16,
) -> AnvilStatus {
    catch_ffi(|| unsafe {
        let Some(handle) = handle.as_mut() else {
            return AnvilStatus::NullArgument;
        };
        match handle.block_on(handle.anvil().reject(anvil_core::CallId(call_id), code)) {
            Ok(()) => AnvilStatus::Ok,
            Err(e) => map_anvil_err(&e),
        }
    })
}

/// End an active call by sending BYE.
///
/// # Safety
/// `handle` must be valid; `call_id` must refer to an established call.
#[no_mangle]
pub unsafe extern "C" fn anvil_hangup(handle: *mut AnvilHandle, call_id: u64) -> AnvilStatus {
    catch_ffi(|| unsafe {
        let Some(handle) = handle.as_mut() else {
            return AnvilStatus::NullArgument;
        };
        match handle.block_on(handle.anvil().hangup(anvil_core::CallId(call_id))) {
            Ok(()) => AnvilStatus::Ok,
            Err(e) => map_anvil_err(&e),
        }
    })
}

/// Put a call on hold (`on != 0`) or resume it (`on == 0`). Drives a
/// re-INVITE under the hood.
///
/// # Safety
/// `handle` must be valid; `call_id` must refer to an established call.
#[no_mangle]
pub unsafe extern "C" fn anvil_hold(handle: *mut AnvilHandle, call_id: u64, on: u8) -> AnvilStatus {
    catch_ffi(|| unsafe {
        let Some(handle) = handle.as_mut() else {
            return AnvilStatus::NullArgument;
        };
        match handle.block_on(handle.anvil().hold(anvil_core::CallId(call_id), on != 0)) {
            Ok(()) => AnvilStatus::Ok,
            Err(e) => map_anvil_err(&e),
        }
    })
}

/// Blind-transfer a call to `target` (a NUL-terminated SIP URI). Progress
/// arrives as `TransferProgress` events; on success the server ends the
/// call.
///
/// # Safety
/// `handle` must be valid; `target` a valid NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn anvil_transfer(
    handle: *mut AnvilHandle,
    call_id: u64,
    target: *const c_char,
) -> AnvilStatus {
    catch_ffi(|| unsafe {
        let Some(handle) = handle.as_mut() else {
            return AnvilStatus::NullArgument;
        };
        if target.is_null() {
            return AnvilStatus::NullArgument;
        }
        let Ok(target) = CStr::from_ptr(target).to_str() else {
            return AnvilStatus::Config;
        };
        match handle.block_on(handle.anvil().transfer(anvil_core::CallId(call_id), target)) {
            Ok(()) => AnvilStatus::Ok,
            Err(e) => map_anvil_err(&e),
        }
    })
}

/// Attended transfer: `call_id`'s party replaces `to_call_id` — the two are
/// connected and both calls of this softphone end.
///
/// # Safety
/// `handle` must be valid; both calls established.
#[no_mangle]
pub unsafe extern "C" fn anvil_transfer_attended(
    handle: *mut AnvilHandle,
    call_id: u64,
    to_call_id: u64,
) -> AnvilStatus {
    catch_ffi(|| unsafe {
        let Some(handle) = handle.as_mut() else {
            return AnvilStatus::NullArgument;
        };
        match handle.block_on(
            handle
                .anvil()
                .transfer_attended(anvil_core::CallId(call_id), anvil_core::CallId(to_call_id)),
        ) {
            Ok(()) => AnvilStatus::Ok,
            Err(e) => map_anvil_err(&e),
        }
    })
}

/// Send a DTMF digit (`'0'`..`'9'`, `'*'`, `'#'`, `'A'`..`'D'`) on a
/// call as a single RFC 2833 event burst (~140 ms).
///
/// `digit` is a UTF-32 code point — pass the literal char value, e.g.
/// `'1'` from C, not its ASCII code 0x31 directly (they happen to be
/// the same for ASCII).
///
/// # Safety
/// `handle` must be valid; `call_id` must refer to an established call.
#[no_mangle]
pub unsafe extern "C" fn anvil_send_dtmf(
    handle: *mut AnvilHandle,
    call_id: u64,
    digit: u32,
) -> AnvilStatus {
    let Some(ch) = char::from_u32(digit) else {
        return AnvilStatus::Config;
    };
    catch_ffi(|| unsafe {
        let Some(handle) = handle.as_mut() else {
            return AnvilStatus::NullArgument;
        };
        match handle.block_on(handle.anvil().send_dtmf(anvil_core::CallId(call_id), ch)) {
            Ok(()) => AnvilStatus::Ok,
            Err(e) => map_anvil_err(&e),
        }
    })
}

/// Send REGISTER with `Expires: 0`.
///
/// # Safety
/// Same as `anvil_register`.
#[no_mangle]
pub unsafe extern "C" fn anvil_unregister(handle: *mut AnvilHandle) -> AnvilStatus {
    catch_ffi(|| unsafe {
        let Some(handle) = handle.as_mut() else {
            return AnvilStatus::NullArgument;
        };
        match handle.block_on(handle.anvil().unregister()) {
            Ok(()) => AnvilStatus::Ok,
            Err(e) => map_anvil_err(&e),
        }
    })
}

// ─── helpers ────────────────────────────────────────────────────────────────

fn catch_ffi(f: impl FnOnce() -> AnvilStatus) -> AnvilStatus {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(s) => s,
        Err(panic) => {
            let msg = panic
                .downcast_ref::<&'static str>()
                .copied()
                .or_else(|| panic.downcast_ref::<String>().map(|s| s.as_str()))
                .unwrap_or("(panic payload not a string)");
            tracing::error!(panic = msg, "FFI entrypoint panicked");
            AnvilStatus::Internal
        }
    }
}

fn map_anvil_err(e: &anvil_core::AnvilError) -> AnvilStatus {
    use anvil_core::AnvilError as E;
    match e {
        E::Config(_) => AnvilStatus::Config,
        E::Transport(_) => AnvilStatus::Transport,
        E::AuthRejected(_) => AnvilStatus::AuthRejected,
        _ => AnvilStatus::Failed,
    }
}

unsafe fn cstr_to_string(s: *const c_char) -> Result<String, AnvilStatus> {
    if s.is_null() {
        return Err(AnvilStatus::NullArgument);
    }
    unsafe { CStr::from_ptr(s) }
        .to_str()
        .map(|s| s.to_owned())
        .map_err(|_| AnvilStatus::Config)
}

unsafe fn cstr_to_optional(s: *const c_char) -> Result<Option<String>, AnvilStatus> {
    if s.is_null() {
        Ok(None)
    } else {
        unsafe { cstr_to_string(s) }.map(Some)
    }
}

unsafe fn build_account(cfg: &AnvilConfigC) -> Result<AccountConfig, AnvilStatus> {
    Ok(AccountConfig {
        aor: unsafe { cstr_to_string(cfg.aor)? },
        registrar: unsafe { cstr_to_string(cfg.registrar)? },
        username: unsafe { cstr_to_string(cfg.username)? },
        password: unsafe { cstr_to_string(cfg.password)? },
        transport: cfg.transport.into_core(),
        outbound_proxy: None,
        stun: unsafe { cstr_to_optional(cfg.stun)? },
        register_expires: std::time::Duration::from_secs(3600),
        user_agent: unsafe { cstr_to_optional(cfg.user_agent)? }
            .unwrap_or_else(|| format!("Anvil/{} (ffi)", env!("CARGO_PKG_VERSION"))),
        bind_addr: unsafe { cstr_to_optional(cfg.bind_addr)? },
        tls_extra_ca_pem: None,
        provisioning_url: None,
    })
}

// Touch unused items so the `mod` declarations and `pub use` exports stay
// honest after refactors. No-op at runtime.
#[allow(dead_code)]
fn _link() {
    let _ = ptr::null::<AnvilHandle>;
}
