# Anvil — roadmap

Five phases, each a usable milestone. Scope is deliberately narrow early so we can
prove the signal → media path works end to end before investing in UI or mobile.

---

## As FCP's client

Anvil becomes FCP's own softphone, desktop first. The design and its phases
live in FCP: `docs/SOFTPHONE.md` in `thevoiceguy/fcp`. Anvil's phases:

- [x] **A0** Today's stacks: siphon-rs and forge-media as git dependencies on
      release tags, the API breaks fixed, siphon-rs answering INVITE challenges,
      registration that lasts (refreshed before the granted expiry, retried with
      backoff, `423` answered with `Min-Expires`), a real outbound proxy, CANCEL,
      603 for a ringing call, a BYE for our own outgoing call, TCP (listener, pool,
      answers on the connection), the advertised Contact, the configured codecs and
      user agent; tests Anvil to Anvil and against FCP, and CI
- [x] **A1** Sign in to FCP: `anvil-fcp` (discovery through
      `/.well-known/fcp-provisioning`, the browser sign-in with a loopback
      listener and PKCE, the password sign-in, the app session kept by a
      `TokenStore` with a file store for desktops, refresh, `/me/softphone`
      into an `AccountConfig`, a new SIP password, sign-out); `anvil-cli login`
      / `logout` and runs from the session. The CLI's interactive commands
      move to A3, with the features they drive
- [x] **A2** Branding from FCP: `anvil-brand`'s HTTP provider and
      filesystem cache, the driver in `anvil-core` (cached then fetched,
      `X-FCP-Provisioning-Url`, hourly, `refresh_brand`, `BrandCleared`),
      `anvil_fcp::brand_config`, the CLI and the FFI events
      (`docs/BRANDING.md` §9a)
- [x] **A3** Calling features over SIP: MWI, BLF, presence, transfer, park, call waiting
  - [x] **A3a** Message waiting: NOTIFY `message-summary` (unsolicited or
        subscribed) → `Event::MessageWaiting`, `Anvil::subscribe_mwi()`
        renewed before it lapses, the CLI's `[mwi]`, the FFI's
        `MessageWaiting`; FCP's 489 to the SUBSCRIBE fixed in FCP #269.
        Other NOTIFYs: `check-sync` 200, anything else 481
  - [x] **A3b** Watching someone: `Anvil::watch(aor, WatchKind::{Presence,
        Dialog})` / `unwatch`, renewed before it lapses; PIDF and dialog-info
        read into `Event::PresenceChanged` / `Event::LineStateChanged`
        (idle, ringing, busy), NOTIFYs matched by entity (unwatched: 481);
        the CLI's `--watch`, the FFI's two kinds. Needs FCP #271 (presence
        per tenant, device credentials, one subscription per device) and
        FCP's ACK fix (a challenged call rang the callee repeatedly)
  - [x] **A3c** Transfer and the rest: `Anvil::transfer` (blind, a REFER in
        the call's dialog) and `transfer_attended` (REFER with `Replaces`
        naming the consultation call), the server's sipfrag NOTIFYs as
        `Event::TransferProgress`; a call's dialog kept after each in-dialog
        request so CSeq keeps rising; park as a transfer to
        `SoftphoneConfig::feature_uri("park")`; do not disturb, call waiting
        and forwards through `/me/calling` (`FcpClient::calling`,
        `set_calling`); RTP statistics restarting on a new stream (RFC 3550
        §A.1) rather than overflowing after a transfer; the CLI's
        `--transfer-to`, the FFI's `anvil_transfer`,
        `anvil_transfer_attended` and `TransferProgress`. Needs FCP #275 (the
        caller transferring the callee, the node's own legs under
        `auth_invites`, a user dialed by name, a dead watcher freezing lamps)
- [x] **A4** The app's data: call history, directory, voicemail, settings, live events
  - `anvil_fcp`: `FcpClient::calls` (`CallQuery`, missed only), `directory`
    (search, presence), `voicemail`, `voicemail_stats`, `voicemail_audio`,
    `set_voicemail_status`, `delete_voicemail`, `calling`/`set_calling`, and
    `events()` — the `/me/events` WebSocket signed in with the session's
    token, each event a `UserEvent` named `{event_type}.{type}`; lists as
    `Page<T>` with FCP's cursor
  - the CLI's `calls`, `directory`, `voicemail`, `settings` and `events`
  - the FFI's `anvil_fcp_*`: open a kept session or sign in with a password,
    the data as JSON (freed with `anvil_string_free`), live events through a
    callback, `NotFound` and `TotpRequired` statuses
  - FCP's `/me/events` needs its admin following an events server that the
    call manager posts to (`[events] server_url` on both)
- [ ] **A5** SDES-SRTP, and the cross-repo suite completed
  - [x] **A5a** SDES-SRTP (RFC 4568, RFC 3711): `MediaConfig::srtp` read —
        `Optional` and `Required` offer `RTP/SAVP` with a fresh
        AES_CM_128_HMAC_SHA1_80 key, `Required` hangs up on an answer in the
        clear; an encrypted offer answered with our key under its tag and
        suite, a plain offer refused 488 when SRTP is required and a secure
        one when it is off; every packet protected (DTMF too) and
        authenticated on receipt, the rest dropped; re-INVITEs keep the key
        (`anvil_core::srtp` over forge-rtp's `SrtpContext`).
        `SoftphoneConfig::media_config` takes FCP's codecs and SRTP
        (`optional` over TLS); the CLI's `--srtp`. Needs FCP #277 (an
        encrypted caller answered encrypted, its key kept off the callee's
        leg)
  - [x] **A5b** Called encrypted over TLS: FCP offers SDES-SRTP to a
        softphone registered over TLS (FCP #278); proved by
        `anvil_over_tls_is_called_encrypted` — Anvil over TLS requiring SRTP,
        called by a plain Anvil over UDP, both heard (`ANVIL_FCP_SIPS`,
        `ANVIL_FCP_CA`)
  - [ ] **A5c** The cross-repo suite completed: CI against FCP (the
        `FCP_READ_TOKEN` secret), a TLS listener in that stack

---

## The app (`docs/APP.md`)

- [x] **U0** The running phone: `anvil-app` (one state model, one stream of
      changes, the commands), mute in `anvil-core`, the control socket (a
      Unix socket or a named pipe, this user only), `anvil-cli run` with a
      prompt, and `anvil-cli call|answer|decline|hangup|hold|resume|mute|
      unmute|dtmf|transfer|attended|dnd|status|watch` driving it
- [ ] **U1** The app's skeleton on desktop (Flutter, `flutter_rust_bridge`)
- [ ] **U2** The desktop app
- [ ] **U3** Desktop distribution
- [ ] **U4** The app on mobile
- [ ] **U5** Ringing a sleeping phone (with FCP's push)
- [ ] **U6** Accessibility, localisation, store listings

---

## Phase 1 — desktop core MVP

**Target:** place and receive an audio call from `anvil-cli` on Linux/macOS/Windows.

- [ ] Workspace compiles, CI green on three desktop OSes
- [ ] `anvil-core::Anvil::start / register / place_call / hangup`
- [ ] REGISTER with Digest auth (siphon-rs `sip-uac` + `sip-auth`)
- [ ] Outgoing INVITE, SDP offer/answer, 200 OK → ACK → media (G.711 only)
- [ ] Incoming INVITE → `Event::IncomingCall` → `answer()` → media
- [ ] BYE in both directions
- [ ] `anvil-audio` cpal capture + playback at 8 kHz, 20 ms frames
- [ ] `anvil-cli` reads TOML config, prints events, accepts `call`/`answer`/`hangup`/`quit`

**Exit criterion:** two `anvil-cli` instances registered to a local Kamailio or the
`siphond` test server can hold a G.711 call for 60 seconds without drift or audible
glitches.

---

## Phase 1.5 — FCP brand provisioning

Slotted between Phase 1 and 2 because it's a visible product differentiator and
touches the login flow. Pure addition on top of the core from Phase 1 — no SIP
or media changes needed.

- [ ] `anvil_core::brand` types and traits finalized
- [ ] `anvil-brand::HttpBrandProvider` — reqwest + rustls, ETag support
- [ ] `anvil-brand::FsBrandCache` — per-user cache dir, content-addressed assets
- [ ] Asset downloader with SHA-256 verification and 5 MB size cap
- [ ] `Event::BrandUpdated` wired into `Anvil::start`
- [ ] Discovery from `X-FCP-Provisioning-Url` header on REGISTER 200 OK
- [ ] `Anvil::refresh_brand()` exposed for UI
- [ ] FCP-side endpoint spec agreed (see open questions in `ARCHITECTURE.md §12`)

**Exit criterion:** `anvil-cli --brand-debug` prints the tenant id, app name, and
logo sha256 after login against a stub FCP provisioning server, and a 304 is
served on second login.

---

## Phase 2 — codec + DTMF + NAT + quality

- [x] **M1**  Multi-codec pipeline + G.722 + SDP negotiation
- [x] **M2**  `MediaStats` event every 2 s (jitter, loss, kbps; RTT pending RTCP)
- [x] **M3**  Opus via `opus` crate (48 kHz fullband, voice mode)
- [x] **M4**  RFC 2833 DTMF send/receive
- [x] **M5**  Hold / resume via re-INVITE (sendonly / sendrecv mirroring)
- [x] **M6**  INVITE auth retry (Digest, mirrors siphon-rs's non-INVITE retry)
- [x] **M7**  TLS transport (sips:, system roots + extra-CA option, ring crypto)
- [x] **M8a** STUN public-IP discovery (RFC 5389 binding request)
- [x] **M8b** Pure-Rust capture-side AGC + `AudioProcessor` trait

Deferred (Phase 3+):
- In-band DTMF fallback (Goertzel detector exists in `forge-dtmf`; deferred — every
  modern SIP peer negotiates RFC 2833)
- RTT in `MediaStats` (needs RTCP RR/SR)
- Real AEC: platform-native on iOS / Android; `webrtc-audio-processing` for desktop
  needs `libwebrtc-audio-processing-dev` (system pkg) and render-stream wiring
- Per-call STUN / ICE proper for symmetric NAT (use `forge-ice`)
- Auth retry for in-dialog non-INVITEs (BYE etc.)

**Exit criterion (met):** two `anvil-cli` instances exchange Opus / G.722 / G.711 audio
end to end with negotiated codec, DTMF, hold, and stats. TLS REGISTER works against a
TLS-only registrar.

---

## Phase 3 — FFI and mobile cross-compilation

- [ ] `anvil-ffi` C header generated with `cbindgen`
- [ ] Every public entry point is `catch_unwind`-guarded
- [ ] Event delivery via a single C callback registered at init
- [ ] Cross-compile to `aarch64-apple-ios`, `aarch64-apple-ios-sim`,
      `aarch64-linux-android`, `armv7-linux-androideabi`
- [ ] iOS: Swift Package wrapping the `.xcframework`; uses `AVAudioEngine` and
      `AVAudioSession` voice-processing mode for Apple-provided AEC
- [ ] Android: Gradle module wrapping the `.so`s; uses Oboe for audio

**Exit criterion:** a throw-away SwiftUI / Jetpack app can register and place a call
on a physical iPhone and a physical Android device.

---

## Phase 4 — mobile lifecycle and telephony integration

- [ ] iOS: PushKit VoIP push, CallKit integration for incoming call UI,
      background audio session handling
- [ ] iOS: App Store-compliant background modes (`voip`, `audio`)
- [ ] Android: FCM high-priority push, foreground service with persistent
      notification, ConnectionService + Telecom framework
- [ ] Android: Doze mode / battery optimisation guidance
- [ ] Handle app kill / background → wake → re-REGISTER flow
- [ ] Keychain / Keystore account storage via an `AccountStore` trait in
      `anvil-core`

**Exit criterion:** incoming call reliably rings a backgrounded app on both platforms,
and the call survives a screen lock.

---

## Phase 5 — UI

Recommend Flutter + `flutter_rust_bridge` for one codebase across all five platforms.
Tauri 2 is a viable desktop-only alternative if Flutter proves too heavy.

- [ ] Dialpad, contacts, call history, settings, in-call UI
- [ ] Dark/light themes, accessibility (VoiceOver, TalkBack)
- [ ] Over-the-air config (SIP account from a URL/QR code)
- [ ] Consume `Event::BrandUpdated`: swap app name, logo, color tokens, icon,
      support links, and ringtones at runtime without restart

UI work is deliberately last. Everything until here is UI-less and exercised via
`anvil-cli` or per-platform smoke apps.

---

## Stretch / post-1.0

- SDES-SRTP and DTLS-SRTP for media encryption
- ICE / TURN for strict-NAT environments
- Push notification relay service (server-side) so the mobile client doesn't need to
  hold a TCP connection open
