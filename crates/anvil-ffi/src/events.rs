//! C-friendly event marshalling.
//!
//! `AnvilEvent` is a tagged union: `kind` says which event type fired,
//! and the relevant fields below are populated. Unused fields are zeroed
//! / NULL.
//!
//! String pointers in the event are valid only for the duration of the
//! callback. Callbacks that need them after returning must copy.

use std::ffi::{c_char, c_void, CString};

use anvil_core::{config::Codec, EndReason, Event, MediaStats, RegState};

/// All event variants the C ABI surfaces. Stable numbering — don't
/// renumber.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnvilEventKind {
    /// `RegistrationChanged`. Read `reg_state` and optionally `reason`.
    RegistrationChanged = 0,
    /// `IncomingCall`. Read `call_id`, `from`, optionally `display_name`.
    /// Application must `anvil_answer` or `anvil_reject` (in M3) to drive
    /// the call forward.
    IncomingCall = 1,
    /// `CallRinging`. Read `call_id`.
    CallRinging = 2,
    /// `CallEstablished`. Read `call_id` and `codec`.
    CallEstablished = 3,
    /// `CallEnded`. Read `call_id`, `end_reason`, optionally `reason`.
    CallEnded = 4,
    /// `DtmfReceived`. Read `call_id` and `digit` (UTF-32 code point).
    DtmfReceived = 5,
    /// `MediaStats`. Read `call_id` and the `stats` sub-struct.
    MediaStats = 6,
    /// `Error`. Read `reason` (always populated). `call_id` is 0 if the
    /// error is not call-scoped.
    Error = 7,
    /// `BrandUpdated`. Read `brand_json`: the profile as JSON
    /// (`docs/BRANDING.md` §5) with each asset's `local_path` where the
    /// cache holds its verified file, and no `bytes`.
    BrandUpdated = 8,
    /// `BrandCleared`: the tenant has no brand; show the app's own theme.
    BrandCleared = 9,
    /// `MessageWaiting`. Read `mwi_waiting`, `mwi_new`, `mwi_old`,
    /// `mwi_urgent_new` and `mwi_urgent_old`.
    MessageWaiting = 10,
}

/// Mirrors `anvil_core::RegState`. Stable numbering.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnvilRegState {
    Unregistered = 0,
    Registering = 1,
    Registered = 2,
    Failed = 3,
}

/// Mirrors `anvil_core::config::Codec`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnvilCodec {
    Pcmu = 0,
    Pcma = 1,
    G722 = 2,
    Opus = 3,
}

/// Mirrors `anvil_core::EndReason` (lossy: `Rejected(code)` puts the SIP
/// status into `rejected_code`; `Error(msg)` puts the message into
/// `event.reason`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnvilEndReason {
    LocalHangup = 0,
    RemoteHangup = 1,
    /// SIP status code lives in `AnvilEvent::rejected_code`.
    Rejected = 2,
    Timeout = 3,
    /// Free-form error message lives in `AnvilEvent::reason`.
    MediaFailure = 4,
    Error = 5,
}

/// `MediaStats` payload. All numeric, no string fields.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AnvilMediaStats {
    pub codec: AnvilCodec,
    pub jitter_ms: f32,
    pub packet_loss_pct: f32,
    /// 0 if RTT isn't yet known. RTT requires RTCP RR/SR (deferred).
    pub rtt_ms: f32,
    pub recv_kbps: f32,
    pub send_kbps: f32,
}

/// One event delivered to the C callback. Field validity depends on
/// `kind`; see `AnvilEventKind` doc.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AnvilEvent {
    pub kind: AnvilEventKind,
    /// Call this event refers to. 0 when not applicable
    /// (RegistrationChanged, top-level Error).
    pub call_id: u64,
    /// `RegistrationChanged` only.
    pub reg_state: AnvilRegState,
    /// `CallEnded` only — see `AnvilEndReason` for which kinds populate
    /// it.
    pub end_reason: AnvilEndReason,
    /// `CallEnded` only when `end_reason == Rejected`. The SIP status
    /// code, e.g. 486.
    pub rejected_code: u16,
    /// `CallEstablished` and `MediaStats` only.
    pub codec: AnvilCodec,
    /// `DtmfReceived` only. UTF-32 code point of the digit (`'0'`..`'9'`,
    /// `'*'`, `'#'`, `'A'`..`'D'`).
    pub digit: u32,
    /// `MediaStats` only.
    pub stats: AnvilMediaStats,
    /// `IncomingCall` only. NUL-terminated UTF-8, valid for the duration
    /// of the callback.
    pub from: *const c_char,
    /// `IncomingCall` only. May be NULL.
    pub display_name: *const c_char,
    /// `RegistrationChanged`, `CallEnded`, `Error`. May be NULL.
    pub reason: *const c_char,
    /// `BrandUpdated` only. NUL-terminated UTF-8 JSON, valid for the
    /// duration of the callback.
    pub brand_json: *const c_char,
    /// `MessageWaiting` only: 1 when messages are waiting.
    pub mwi_waiting: u8,
    /// `MessageWaiting` only: the voice message counts.
    pub mwi_new: u32,
    pub mwi_old: u32,
    pub mwi_urgent_new: u32,
    pub mwi_urgent_old: u32,
}

/// Function-pointer type for the user callback. Fires from a worker
/// thread of the FFI runtime — keep it short and don't call back into
/// `anvil_*` synchronously (re-entrancy on a multi-thread runtime is
/// safe but wastes a worker).
pub type AnvilEventCallback =
    unsafe extern "C" fn(event: *const AnvilEvent, user_data: *mut c_void);

/// Slot used by `AnvilHandle` to share the registered callback with the
/// drain task. Pointer + user data are wrapped in a newtype that's
/// manually `Send + Sync` because a raw `*mut c_void` isn't.
#[derive(Clone, Copy)]
pub(crate) struct CallbackSlot {
    pub cb: AnvilEventCallback,
    pub user_data: *mut c_void,
}

unsafe impl Send for CallbackSlot {}
unsafe impl Sync for CallbackSlot {}

/// Marshal one Rust event into a C `AnvilEvent` and invoke the callback.
/// Heap-allocates `CString`s for the string fields and frees them after
/// the callback returns.
pub(crate) fn dispatch(slot: CallbackSlot, event: &Event) {
    // Default-blank event we'll fill in per-variant.
    let mut ev = AnvilEvent {
        kind: AnvilEventKind::Error, // overwritten below
        call_id: 0,
        reg_state: AnvilRegState::Unregistered,
        end_reason: AnvilEndReason::LocalHangup,
        rejected_code: 0,
        codec: AnvilCodec::Pcmu,
        digit: 0,
        stats: AnvilMediaStats {
            codec: AnvilCodec::Pcmu,
            jitter_ms: 0.0,
            packet_loss_pct: 0.0,
            rtt_ms: 0.0,
            recv_kbps: 0.0,
            send_kbps: 0.0,
        },
        from: std::ptr::null(),
        display_name: std::ptr::null(),
        reason: std::ptr::null(),
        brand_json: std::ptr::null(),
        mwi_waiting: 0,
        mwi_new: 0,
        mwi_old: 0,
        mwi_urgent_new: 0,
        mwi_urgent_old: 0,
    };

    // Hold any allocated CStrings until after the callback returns.
    let mut keep_alive: Vec<CString> = Vec::new();

    match event {
        Event::RegistrationChanged { state, reason } => {
            ev.kind = AnvilEventKind::RegistrationChanged;
            ev.reg_state = match state {
                RegState::Unregistered => AnvilRegState::Unregistered,
                RegState::Registering => AnvilRegState::Registering,
                RegState::Registered => AnvilRegState::Registered,
                RegState::Failed => AnvilRegState::Failed,
            };
            if let Some(s) = reason.as_deref() {
                ev.reason = push_cstr(&mut keep_alive, s);
            }
        }
        Event::IncomingCall {
            call,
            from,
            display_name,
        } => {
            ev.kind = AnvilEventKind::IncomingCall;
            ev.call_id = call.0;
            ev.from = push_cstr(&mut keep_alive, from);
            if let Some(name) = display_name.as_deref() {
                ev.display_name = push_cstr(&mut keep_alive, name);
            }
        }
        Event::CallRinging { call } => {
            ev.kind = AnvilEventKind::CallRinging;
            ev.call_id = call.0;
        }
        Event::CallEstablished { call, codec } => {
            ev.kind = AnvilEventKind::CallEstablished;
            ev.call_id = call.0;
            ev.codec = codec_to_c(*codec);
        }
        Event::CallEnded { call, reason } => {
            ev.kind = AnvilEventKind::CallEnded;
            ev.call_id = call.0;
            match reason {
                EndReason::LocalHangup => ev.end_reason = AnvilEndReason::LocalHangup,
                EndReason::RemoteHangup => ev.end_reason = AnvilEndReason::RemoteHangup,
                EndReason::Rejected(code) => {
                    ev.end_reason = AnvilEndReason::Rejected;
                    ev.rejected_code = *code;
                }
                EndReason::Timeout => ev.end_reason = AnvilEndReason::Timeout,
                EndReason::MediaFailure(msg) => {
                    ev.end_reason = AnvilEndReason::MediaFailure;
                    ev.reason = push_cstr(&mut keep_alive, msg);
                }
                EndReason::Error(msg) => {
                    ev.end_reason = AnvilEndReason::Error;
                    ev.reason = push_cstr(&mut keep_alive, msg);
                }
            }
        }
        Event::DtmfReceived { call, digit } => {
            ev.kind = AnvilEventKind::DtmfReceived;
            ev.call_id = call.0;
            ev.digit = *digit as u32;
        }
        Event::MediaStats { call, stats } => {
            ev.kind = AnvilEventKind::MediaStats;
            ev.call_id = call.0;
            ev.codec = codec_to_c(stats.codec);
            ev.stats = stats_to_c(stats);
        }
        Event::Error { call, error } => {
            ev.kind = AnvilEventKind::Error;
            ev.call_id = call.map(|c| c.0).unwrap_or(0);
            ev.reason = push_cstr(&mut keep_alive, &error.to_string());
        }
        Event::BrandUpdated { profile } => {
            ev.kind = AnvilEventKind::BrandUpdated;
            ev.brand_json = push_cstr(&mut keep_alive, &brand_json(profile));
        }
        Event::BrandCleared => {
            ev.kind = AnvilEventKind::BrandCleared;
        }
        Event::MessageWaiting { summary } => {
            ev.kind = AnvilEventKind::MessageWaiting;
            ev.mwi_waiting = u8::from(summary.waiting);
            ev.mwi_new = summary.new;
            ev.mwi_old = summary.old;
            ev.mwi_urgent_new = summary.urgent_new;
            ev.mwi_urgent_old = summary.urgent_old;
        }
        // `Event` is `#[non_exhaustive]`; cover any future variants
        // gracefully by emitting an Error rather than panicking.
        _ => {
            ev.kind = AnvilEventKind::Error;
            ev.reason = push_cstr(
                &mut keep_alive,
                "unknown event variant (FFI needs an update)",
            );
        }
    }

    // SAFETY: the slot was registered with `anvil_set_event_callback`,
    // which trusts the caller's pointer. Strings are alive until
    // `keep_alive` drops below.
    unsafe {
        (slot.cb)(&ev, slot.user_data);
    }

    drop(keep_alive);
}

fn push_cstr(holder: &mut Vec<CString>, s: &str) -> *const c_char {
    // Replace interior NULs to keep CString::new infallible.
    let safe = s.replace('\0', "\u{FFFD}");
    let cstr = CString::new(safe).expect("nul-replaced string is CString-safe");
    let ptr = cstr.as_ptr();
    holder.push(cstr);
    ptr
}

fn codec_to_c(c: Codec) -> AnvilCodec {
    match c {
        Codec::Pcmu => AnvilCodec::Pcmu,
        Codec::Pcma => AnvilCodec::Pcma,
        Codec::G722 => AnvilCodec::G722,
        Codec::Opus => AnvilCodec::Opus,
    }
}

fn stats_to_c(s: &MediaStats) -> AnvilMediaStats {
    AnvilMediaStats {
        codec: codec_to_c(s.codec),
        jitter_ms: s.jitter_ms,
        packet_loss_pct: s.packet_loss_pct,
        rtt_ms: s.rtt_ms.unwrap_or(0.0),
        recv_kbps: s.recv_kbps,
        send_kbps: s.send_kbps,
    }
}

/// A profile as the C host reads it: the bytes left out (the host opens
/// `local_path`, or fetches `url`).
fn brand_json(profile: &anvil_core::BrandProfile) -> String {
    let mut profile = profile.clone();
    for asset in [&mut profile.logo, &mut profile.logo_dark, &mut profile.icon]
        .into_iter()
        .flatten()
    {
        asset.bytes.clear();
    }
    for ringtone in &mut profile.ringtones {
        ringtone.asset.bytes.clear();
    }
    serde_json::to_string(&profile).unwrap_or_default()
}
