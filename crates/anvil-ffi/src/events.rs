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
    /// `PresenceChanged`. Read `from` (whose), `presence_open`, and `reason`
    /// (the RPID activity, else the note; may be NULL).
    PresenceChanged = 11,
    /// `LineStateChanged`. Read `from` (whose) and `line_state`.
    LineStateChanged = 12,
    /// `TransferProgress`: a transfer of `call_id` progressed. Read
    /// `status_code` (1xx while the new call is placed, then the final)
    /// and `reason`.
    TransferProgress = 13,
    /// `VideoStarted`: a video stream of `call_id` began. Read
    /// `video_stream`, `video_direction`, `video_kind` and `video_codec`.
    VideoStarted = 14,
    /// `VideoStopped`: a video stream ended. Read `video_stream`.
    VideoStopped = 15,
    /// `VideoStats`: a video stream's last second. Read `video_stream`
    /// and `video_stats` (with `video_direction`, `video_kind`,
    /// `video_codec`).
    VideoStats = 16,
    /// `VideoRequested`: the far end added video to `call_id` and would
    /// see our camera; ours is not sent until `anvil_set_video(…, 1)`.
    VideoRequested = 17,
}

/// `VideoStats` payload. A value not yet known is NaN (floats) or 0
/// (counts and rates, where 0 is not a reading).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AnvilVideoStats {
    pub kbps: f32,
    pub fps: f32,
    /// Incoming: packets the network lost, before any were sent again.
    /// Outgoing: the far end's last report on ours. NaN until known.
    pub loss_pct: f32,
    /// Asked for again (incoming) or asked of us (outgoing).
    pub nacks: u32,
    pub keyframe_requests: u32,
    /// Outgoing: what the encoder aims for. 0 for incoming.
    pub target_kbps: u32,
    /// Incoming: what we tell the far end it may send. 0 until known.
    pub estimate_kbps: u32,
    /// Incoming only; NaN otherwise.
    pub jitter_ms: f32,
    pub playout_delay_ms: f32,
    /// Incoming: how far the picture is behind its audio (+: late). NaN
    /// until sender reports tie them.
    pub av_offset_ms: f32,
    /// Outgoing: the round trip. NaN until measured.
    pub rtt_ms: f32,
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
    /// The orbit lives in `AnvilEvent::reason`.
    Parked = 6,
}

/// `MediaStats` payload. All numeric, no string fields.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AnvilMediaStats {
    pub codec: AnvilCodec,
    pub jitter_ms: f32,
    pub packet_loss_pct: f32,
    /// 0 until RTCP has measured the round trip.
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
    /// `PresenceChanged` only: 1 when available.
    pub presence_open: u8,
    /// `LineStateChanged` only: 0 idle, 1 ringing, 2 busy.
    pub line_state: u8,
    /// `TransferProgress` only: the SIP status of the transferred party's
    /// new call.
    pub status_code: u16,
    /// `IncomingCall` only: 1 when the call offers video this softphone
    /// can take (`anvil_answer_media` with `video = 0` declines it).
    pub video_offered: u8,
    /// The `Video*` events: the stream, unique within the call.
    pub video_stream: u32,
    /// 0 incoming, 1 outgoing.
    pub video_direction: u8,
    /// 0 camera, 1 content (a shared screen).
    pub video_kind: u8,
    /// 0 H.264, 1 VP8.
    pub video_codec: u8,
    /// `VideoStats` only.
    pub video_stats: AnvilVideoStats,
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
        presence_open: 0,
        line_state: 0,
        status_code: 0,
        video_offered: 0,
        video_stream: 0,
        video_direction: 0,
        video_kind: 0,
        video_codec: 0,
        video_stats: AnvilVideoStats {
            kbps: 0.0,
            fps: 0.0,
            loss_pct: f32::NAN,
            nacks: 0,
            keyframe_requests: 0,
            target_kbps: 0,
            estimate_kbps: 0,
            jitter_ms: f32::NAN,
            playout_delay_ms: f32::NAN,
            av_offset_ms: f32::NAN,
            rtt_ms: f32::NAN,
        },
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
            video,
            ..
        } => {
            ev.kind = AnvilEventKind::IncomingCall;
            ev.call_id = call.0;
            ev.video_offered = u8::from(*video);
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
                EndReason::Parked(orbit) => {
                    ev.end_reason = AnvilEndReason::Parked;
                    ev.reason = push_cstr(&mut keep_alive, orbit);
                }
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
            ev.codec = codec_to_c(stats.call.codec);
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
        Event::PresenceChanged { aor, presence } => {
            ev.kind = AnvilEventKind::PresenceChanged;
            ev.from = push_cstr(&mut keep_alive, aor);
            ev.presence_open = u8::from(presence.open);
            if let Some(why) = presence.activity.as_ref().or(presence.note.as_ref()) {
                ev.reason = push_cstr(&mut keep_alive, why);
            }
        }
        Event::LineStateChanged { aor, state } => {
            ev.kind = AnvilEventKind::LineStateChanged;
            ev.from = push_cstr(&mut keep_alive, aor);
            ev.line_state = match state {
                anvil_core::watch::LineState::Idle => 0,
                anvil_core::watch::LineState::Ringing => 1,
                anvil_core::watch::LineState::Busy => 2,
            };
        }
        Event::TransferProgress { call, code, reason } => {
            ev.kind = AnvilEventKind::TransferProgress;
            ev.call_id = call.0;
            ev.status_code = *code;
            ev.reason = push_cstr(&mut keep_alive, reason);
        }
        Event::MessageWaiting { summary } => {
            ev.kind = AnvilEventKind::MessageWaiting;
            ev.mwi_waiting = u8::from(summary.waiting);
            ev.mwi_new = summary.new;
            ev.mwi_old = summary.old;
            ev.mwi_urgent_new = summary.urgent_new;
            ev.mwi_urgent_old = summary.urgent_old;
        }
        Event::VideoStarted {
            call,
            stream,
            direction,
            kind,
            codec,
        } => {
            ev.kind = AnvilEventKind::VideoStarted;
            ev.call_id = call.0;
            ev.video_stream = stream.0;
            ev.video_direction = video_direction(*direction);
            ev.video_kind = video_kind(*kind);
            ev.video_codec = video_codec(*codec);
        }
        Event::VideoStopped { call, stream } => {
            ev.kind = AnvilEventKind::VideoStopped;
            ev.call_id = call.0;
            ev.video_stream = stream.0;
        }
        Event::VideoStats {
            call,
            stream,
            stats,
        } => {
            ev.kind = AnvilEventKind::VideoStats;
            ev.call_id = call.0;
            ev.video_stream = stream.0;
            ev.video_direction = video_direction(stats.direction);
            ev.video_kind = video_kind(stats.kind);
            ev.video_codec = video_codec(stats.codec);
            ev.video_stats = video_stats_to_c(stats);
        }
        Event::VideoRequested { call } => {
            ev.kind = AnvilEventKind::VideoRequested;
            ev.call_id = call.0;
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

fn video_direction(d: anvil_core::video::VideoStreamDirection) -> u8 {
    match d {
        anvil_core::video::VideoStreamDirection::Incoming => 0,
        anvil_core::video::VideoStreamDirection::Outgoing => 1,
    }
}

fn video_kind(k: anvil_core::video::host::VideoKind) -> u8 {
    match k {
        anvil_core::video::host::VideoKind::Camera => 0,
        anvil_core::video::host::VideoKind::Content => 1,
    }
}

fn video_codec(c: anvil_core::video::host::VideoCodec) -> u8 {
    match c {
        anvil_core::video::host::VideoCodec::H264 => 0,
        anvil_core::video::host::VideoCodec::Vp8 => 1,
    }
}

pub(crate) fn video_stats_to_c(s: &anvil_core::video::VideoStreamStats) -> AnvilVideoStats {
    AnvilVideoStats {
        kbps: s.kbps,
        fps: s.fps,
        loss_pct: s.loss_pct.unwrap_or(f32::NAN),
        nacks: s.nacks,
        keyframe_requests: s.keyframe_requests,
        target_kbps: s.target_kbps.unwrap_or(0),
        estimate_kbps: s.estimate_kbps.unwrap_or(0),
        jitter_ms: s.jitter_ms.unwrap_or(f32::NAN),
        playout_delay_ms: s.playout_delay_ms.unwrap_or(f32::NAN),
        av_offset_ms: s.av_offset_ms.unwrap_or(f32::NAN),
        rtt_ms: s.rtt_ms.unwrap_or(f32::NAN),
    }
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
        codec: codec_to_c(s.call.codec),
        jitter_ms: s.call.jitter_ms,
        packet_loss_pct: s.call.loss_pct(),
        rtt_ms: s.call.rtt_ms.unwrap_or(0.0),
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

#[cfg(test)]
mod tests {
    use super::*;
    use anvil_core::video::host::{StreamId, VideoCodec, VideoKind};
    use anvil_core::video::{VideoStreamDirection, VideoStreamStats};
    use anvil_core::CallId;

    unsafe extern "C" fn record(event: *const AnvilEvent, user_data: *mut c_void) {
        let seen = unsafe { &mut *(user_data as *mut Vec<AnvilEvent>) };
        seen.push(unsafe { *event });
    }

    fn marshal(events: &[Event]) -> Vec<AnvilEvent> {
        let mut seen: Vec<AnvilEvent> = Vec::new();
        let slot = CallbackSlot {
            cb: record,
            user_data: &mut seen as *mut _ as *mut c_void,
        };
        for e in events {
            dispatch(slot, e);
        }
        seen
    }

    #[test]
    fn video_events_reach_c_with_their_stream_and_stats() {
        let call = CallId(7);
        let stats = VideoStreamStats {
            direction: VideoStreamDirection::Incoming,
            kind: VideoKind::Content,
            codec: VideoCodec::Vp8,
            kbps: 812.5,
            fps: 29.9,
            loss_pct: Some(1.5),
            nacks: 3,
            keyframe_requests: 1,
            target_kbps: None,
            estimate_kbps: Some(1200),
            jitter_ms: Some(4.0),
            playout_delay_ms: Some(60.0),
            av_offset_ms: None,
            rtt_ms: None,
        };
        let seen = marshal(&[
            Event::VideoStarted {
                call,
                stream: StreamId(2),
                direction: VideoStreamDirection::Outgoing,
                kind: VideoKind::Camera,
                codec: VideoCodec::H264,
            },
            Event::VideoStats {
                call,
                stream: StreamId(3),
                stats,
            },
            Event::VideoRequested { call },
            Event::VideoStopped {
                call,
                stream: StreamId(2),
            },
        ]);
        assert_eq!(seen[0].kind, AnvilEventKind::VideoStarted);
        assert_eq!(
            (
                seen[0].call_id,
                seen[0].video_stream,
                seen[0].video_direction
            ),
            (7, 2, 1)
        );
        assert_eq!((seen[0].video_kind, seen[0].video_codec), (0, 0));

        let s = seen[1];
        assert_eq!(s.kind, AnvilEventKind::VideoStats);
        assert_eq!(
            (
                s.video_stream,
                s.video_direction,
                s.video_kind,
                s.video_codec
            ),
            (3, 0, 1, 1)
        );
        assert_eq!(s.video_stats.kbps, 812.5);
        assert_eq!(s.video_stats.loss_pct, 1.5);
        assert_eq!(s.video_stats.estimate_kbps, 1200);
        assert_eq!(s.video_stats.target_kbps, 0);
        assert!(s.video_stats.av_offset_ms.is_nan(), "not yet known");
        assert!(s.video_stats.rtt_ms.is_nan());

        assert_eq!(seen[2].kind, AnvilEventKind::VideoRequested);
        assert_eq!(seen[2].call_id, 7);
        assert_eq!(seen[3].kind, AnvilEventKind::VideoStopped);
        assert_eq!(seen[3].video_stream, 2);
    }
}
