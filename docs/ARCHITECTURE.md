# Anvil — architecture

This is the design doc. It captures the crate boundaries, the public surface of
`anvil-core`, how SIP signaling is glued to RTP media, the threading model, and the
extension points for platform integration.

---

## 1. Goals and non-goals

**Goals.** One Rust core that runs on Linux, macOS, Windows, iOS, and Android, exposing
a stable API that platform-native UIs consume. SIP-based calling (REGISTER, INVITE,
BYE, REFER, basic SUBSCRIBE/NOTIFY for presence). Audio in G.711 + G.722 + Opus;
video in H.264 and VP8 (§5a), one-to-one and in FCP's video rooms. RFC 2833 DTMF.
NAT-friendly over UDP, TCP, TLS.

**Non-goals.** In-app recording beyond basic RTP tap. Acting as a proxy, B2BUA,
or registrar — Anvil is a UAC/UAS endpoint. PSTN trunking. Kernel-offload RTP
(forge-kernel is Linux-only and not portable).

---

## 2. Crate layout

```
anvil/
├── Cargo.toml
├── crates/
│   ├── anvil-core    # softphone logic, SIP↔media orchestration, public API
│   ├── anvil-audio   # cpal I/O + AEC/NS/AGC + device enumeration
│   ├── anvil-video   # the desktop's video host: cameras, codecs, tiles
│   ├── anvil-brand   # FCP brand provisioning (HTTP fetch, fs cache)
│   ├── anvil-codec   # Opus (forge-media already provides G.711, G.722)
│   └── anvil-ffi     # C ABI for mobile / UI hosts
└── bins/
    └── anvil-cli     # terminal softphone reference client
```

### Dependency graph

```
                   anvil-cli
                       │
                       ▼
 anvil-ffi ──►    anvil-core    ◄── {siphon-rs::*, forge-media::*}
                 △   △   △
                 │   │   │
        anvil-audio  │   anvil-codec
          (cpal)     │     (opus)
                     │
                anvil-brand
                 (reqwest)
```

`anvil-core` depends on the siphon-rs and forge-media crates directly. It does **not**
depend on `anvil-audio`, `anvil-video`, `anvil-brand`, or `anvil-codec`. Instead it defines traits
(`AudioHost`, `VideoHost`, `BrandProvider`, `BrandCache`, and codec types) and those crates
supply implementations. This lets the core stay testable without real devices or
network access, and lets mobile hosts drop in their own audio pipeline
(AVAudioEngine on iOS, Oboe on Android) and HTTP stack (URLSession, OkHttp) when
the host OS owns those concerns.

### Feature flags

| Crate         | Feature         | Purpose                                              |
|---------------|-----------------|------------------------------------------------------|
| `anvil-core`  | `tls`           | Enable TLS transport via siphon-rs (on by default)   |
| `anvil-core`  | `srtp`          | SDES-SRTP for non-WebRTC SIP peers                   |
| `anvil-audio` | `apm`           | Echo cancellation and noise suppression (`sonora`, WebRTC's in Rust; on by default) |
| `anvil-audio` | `cpal-default`  | Use cpal as the audio backend (on by default)        |
| `anvil-brand` | `http`          | HTTP fetch via reqwest (on by default)               |
| `anvil-brand` | `fs-cache`      | Filesystem-backed brand cache (on by default)        |
| `anvil-codec` | `opus`          | Opus support (on by default)                         |

---

## 3. `anvil-core` public API sketch

```rust
// ─────────────── configuration ───────────────
pub struct AccountConfig {
    pub aor: String,           // "sip:alice@example.com"
    pub registrar: String,     // "sip.example.com" or "sip:..."
    pub username: String,
    pub password: Secret<String>,
    pub transport: Transport,  // Udp | Tcp | Tls
    pub outbound_proxy: Option<String>,
    pub stun: Option<String>,
    pub register_expires: Duration,  // default 3600s
    pub user_agent: String,          // e.g. "Anvil/0.1 (linux)"
}

pub struct MediaConfig {
    pub codecs: Vec<Codec>,    // ordered preference: [Opus, G722, Pcmu, Pcma]
    pub dtmf: DtmfMode,        // Rfc2833 | Inband | Both
    pub rtp_port_range: RangeInclusive<u16>,
    pub srtp: SrtpMode,        // Off | Optional | Required
    pub jitter_buffer_ms: u32, // default 60
}

pub struct AnvilConfig {
    pub account: AccountConfig,
    pub media:   MediaConfig,
    pub audio:   Box<dyn AudioHost>,   // see §5
}

// ─────────────── runtime ───────────────
pub struct Anvil { /* opaque */ }

impl Anvil {
    pub async fn start(cfg: AnvilConfig) -> Result<(Self, EventStream)>;

    pub async fn register(&self)   -> Result<()>;
    pub async fn unregister(&self) -> Result<()>;

    pub async fn place_call(&self, target: &str)       -> Result<CallId>;
    pub async fn answer   (&self, call: CallId)        -> Result<()>;
    pub async fn reject   (&self, call: CallId, code: u16) -> Result<()>;
    pub async fn hangup   (&self, call: CallId)        -> Result<()>;

    pub async fn send_dtmf(&self, call: CallId, digit: char) -> Result<()>;
    pub async fn hold     (&self, call: CallId, hold: bool)  -> Result<()>;
    pub async fn transfer (&self, call: CallId, target: &str) -> Result<()>;

    pub async fn shutdown(self) -> Result<()>;
}

// ─────────────── events ───────────────
pub enum Event {
    RegistrationChanged { state: RegState, reason: Option<String> },
    IncomingCall        { call: CallId, from: String, display_name: Option<String> },
    CallRinging         { call: CallId },
    CallEstablished     { call: CallId, codec: Codec },
    CallEnded           { call: CallId, reason: EndReason },
    DtmfReceived        { call: CallId, digit: char },
    MediaStats          { call: CallId, stats: MediaStats },  // periodic
    Error               { call: Option<CallId>, error: AnvilError },
}

pub type EventStream = tokio::sync::mpsc::Receiver<Event>;
```

### Why an mpsc event stream

Platform UIs want a single consumer. The FFI crate bridges this to one C callback;
the CLI drains it in a `select!` loop. A broadcast channel would force us to dedupe
events across multiple receivers for no benefit here.

---

## 4. SIP ↔ media glue (the heart of `anvil-core`)

### 4.1 Outgoing call

```
User calls place_call("sip:bob@example.com")
│
├─► MediaSession::new()                          (forge-engine)
│     └─ allocate RTP port from MediaConfig.rtp_port_range
│
├─► SdpOfferBuilder { codecs, ice?, srtp? }      (sip-sdp + forge-sdp)
│
├─► UserAgentClient::create_invite(target, sdp)  (sip-uac)
│     └─ TransactionManager handles retransmit + 401/407 auth retry
│
├─► on 100/180/183  -> emit CallRinging
│
├─► on 2xx answer:
│     ├─ parse SDP answer                        (forge-sdp::negotiate)
│     ├─ MediaSession::set_remote(answer)
│     ├─ MediaSession::start()                   (opens RTP, starts flow)
│     ├─ Dialog::confirmed                       (sip-dialog)
│     └─ emit CallEstablished { codec }
│
└─► on 4xx/5xx/6xx -> emit CallEnded { reason = Rejected(code) }
```

### 4.2 Incoming call

```
UserAgentServer observes INVITE (sip-uas)
│
├─► emit IncomingCall to app
│   (core does NOT auto-respond; UI drives answer/reject)
│
│   UI calls answer(call):
│     ├─ MediaSession::new() + allocate port
│     ├─ SdpAnswer from offered codec intersection
│     ├─ UserAgentServer::accept_invite(req, answer) → 200 OK
│     ├─ on ACK: MediaSession::start()
│     └─ emit CallEstablished
│
│   UI calls reject(call, 486):
│     └─ UserAgentServer::create_busy(req) → 486
```

### 4.3 Hangup

```
hangup(call_id):
├─ MediaSession::stop()  (closes RTP, flushes jitter buffer)
├─ UserAgentClient::create_bye(dialog)
└─ on 200 OK to BYE: drop dialog, emit CallEnded { reason: LocalHangup }
```

### 4.4 DTMF

RFC 2833 is mandatory. In-band is a fallback for gateways that refuse telephone-event.
`forge-dtmf` provides both directions.

- `send_dtmf`: push a `DtmfEvent` into the RTP session with PT 101.
- Incoming: the RTP receive path in `forge-engine` emits events that the core forwards
  as `Event::DtmfReceived`.

### 4.5 Hold / resume

Hold sends a re-INVITE with `a=sendonly` (or `a=inactive` for full hold). forge-engine
stops transmitting but keeps the RTP socket open. Resume sends another re-INVITE with
`a=sendrecv`.

### 4.6 Transfer

Blind transfer only in v1. `UserAgentClient::create_refer(dialog, refer_to)`, wait for
the NOTIFY sipfrag status, emit events. Attended transfer (REFER with Replaces) is
supported by sip-uac but deferred until the basic flow is stable.

---

## 5. Audio host trait

`anvil-core` does not open the mic. It depends on a host:

```rust
/// Provided by anvil-audio on desktop, or by the platform layer on mobile.
pub trait AudioHost: Send + Sync {
    /// Create a capture source sampled at `sample_rate`, framed at 20ms.
    fn make_capture (&self, cfg: AudioFormat) -> Result<Box<dyn AudioSource>>;
    /// Create a playback sink that accepts 20ms frames at `sample_rate`.
    fn make_playback(&self, cfg: AudioFormat) -> Result<Box<dyn AudioSink>>;
    fn devices(&self) -> Vec<DeviceInfo>;
}

pub trait AudioSource: Send {
    /// Block until the next 20ms frame is available.
    async fn next_frame(&mut self) -> Option<AudioFrame>;
}
pub trait AudioSink: Send {
    async fn write_frame(&mut self, frame: &AudioFrame) -> Result<()>;
}
```

Implementations:

- `anvil-audio` — desktop, `cpal` behind the host. Cancels echo and suppresses
  noise (WebRTC's AEC3 and noise suppressor, ported to Rust by `sonora`) when
  the `apm` feature is on, as it is by default.
- **iOS**: host backed by `AVAudioEngine`, owned by Swift; bridged through FFI.
  Apple already does AEC for us when using the voice-processing audio unit.
- **Android**: host backed by Oboe / AudioRecord, bridged through JNI.
- **Testing**: `InMemoryHost` that replays wav fixtures and records sink output.

Keeping the host trait out of `anvil-core`'s own crate graph means the core compiles
even when `cpal` is unavailable (Android targets without NDK setup, CI without audio).

---

## 5a. Video

The design is FCP's `docs/SOFTPHONE_VIDEO.md` (§2.3–2.4); the code is
`anvil-core/src/video/`. The core owns the network; a `VideoHost`, supplied by the
platform as `AnvilConfig::video`, owns the camera, the codecs and the screen. They
pass **coded frames** (Annex B H.264, raw VP8), because the codec that matters on
each platform is its hardware one.

```rust
pub trait VideoHost: Send + Sync {
    fn codecs(&self) -> Vec<VideoCodecCaps>;
    fn cameras(&self) -> Vec<CameraInfo>;
    fn start_send(&self, cfg: SendConfig) -> Result<Box<dyn VideoSource>>;
    fn start_receive(&self, stream: StreamId, cfg: ReceiveConfig) -> Result<Box<dyn VideoSink>>;
}
```

Three layers, so a second transport adds itself and rewrites nothing:

- **`host`**: the traits above. A sink is one incoming stream, whoever sent it.
- **`session`**: one source going out and any number of streams coming in, each
  SSRC with its own assembler, NACK and PLI state and its own sink. It is given a
  `StreamPlan` (codec, payload type, direction, the feedback the far end takes) and
  a `MediaTransport`, and never reads SDP or opens a socket.
- **SIP's transport**: `sdp` reads and writes the `m=video` sections; `SdesTransport`
  is a socket pair and SDES keys per section; `CallVideo` turns a call's offer,
  answer and re-INVITEs into plans. A WebRTC transport (ICE, DTLS-SRTP, BUNDLE) is
  to sit beside it for rooms that forward each participant's stream.

Within a session:

- **Bandwidth both ways.** What arrives feeds a `BandwidthEstimator`; today's is
  `RembEstimator`, receive-side and GCC-shaped (a delay trend for queues building,
  loss on first arrival, AIMD), sent to the far end as REMB each second and at once
  when it falls. A REMB from the far end sets the encoder's target through
  `VideoSource::set_target`, within what the call was set up to send, with the
  resolution stepped down the ladder (720p from 1 Mb/s, 360p from 450 kb/s, 180p
  below). Send-side estimation (transport-cc) is to sit behind the same interface.
- **Reports.** Every second, an SR (or RR) with a block on each incoming stream, the
  call's audio CNAME (so the far end pairs our video with our voice) and the REMB.
  The far end's reports give the round trip and its loss on ours.
- **Playout and lip sync** (`playout`). A frame is shown at its RTP time, carried
  across on the quickest recent trip, plus 60–150 ms grown on jitter. Once sender
  reports tie the stream's clock and the call's audio to the far end's NTP clock,
  video waits (up to 500 ms) to be shown when its audio is heard. The sink is given
  the moment to show each frame.
- **Loss.** A gap is asked for by NACK, each packet at most three times, retried
  and given up on the round trip's scale (400 ms to 1.5 s; 50 ms without NACK),
  after which the stream waits for a keyframe. A burst of more than 64 packets goes
  straight to a keyframe request. The sender resends a packet at most once a round
  trip, spends at most half its target on resends, and makes at most one requested
  keyframe each 500 ms — on a congested path, resends and keyframes on demand are
  what keep it congested.

On a call: `place_call_with_media(target, CallMedia::VIDEO)` offers a camera section;
`IncomingCall { video }` says an offer has video this softphone can take;
`answer_with_media(call, CallMedia::AUDIO)` declines it (port 0).
`set_video(call, on)` is a re-INVITE: on adds a camera section to an audio call (or
takes a stopped one up again, or starts sending on one we only receive); off makes
it receive-only while the far end still sends, and stops it (port 0) when neither
does. Video the far end adds mid-call is taken **receive-only**, with
`VideoRequested`: the camera opens only when the user says (`set_video`, or
answering with video), and a session opens it only while it sends, so hold closes
it. Hold stops video both ways (`inactive`, the tiles frozen) and resuming starts
on a keyframe; the far end's camera turned off (`recvonly`) removes its tile. A
re-INVITE that crosses an offer of ours is answered 491 (RFC 3261 §14.2), and
`set_video` tries again after 2.1–4 s when its own is; a refused offer leaves the
call as it was. The C interface has `anvil_place_call_media`, `anvil_answer_media`,
`anvil_set_video` and the `Video*` events; `anvil-cli --video synthetic` calls with
the synthetic host and prints what arrived. `VideoStarted`,
`VideoStats` (each second: rate, frame rate, loss, NACKs and keyframe requests, the
target or the estimate, jitter, playout delay, the offset from the audio, the round
trip) and `VideoStopped` report each stream. `SyntheticVideoHost` sends numbered frames and
records what arrives, so the core, the CLI and CI need no camera.

**The desktop's host** is `anvil-video`'s `DesktopVideoHost` (A2). A source captures
on a thread of its own, fits each picture to the size the ladder allows, encodes it
and hands it to the core; dropping it closes the camera. A sink decodes each
incoming stream on a thread of its own and shows each picture at the moment the core
gives, in the stream's tile (`Tiles`: each stream's last picture and the self-view,
which the app attaches a renderer to; stream ids are unique in the process, so
several calls' tiles do not collide). On Linux: V4L2 cameras (YUYV, NV12, or MJPEG
with the standard Huffman tables put back), VP8 through libvpx, and H.264 through
Cisco's OpenH264 binary, fetched from Cisco at first use into the user's data
directory and checked against Cisco's SHA-256 (only Cisco's binary carries Cisco's
patent licence; an app using it shows `openh264_fetch::NOTICE`); until it is there,
calls offer VP8. On macOS, H.264 goes through VideoToolbox (the system's codec, in
hardware where the Mac has it): constrained baseline, real time, no reordering,
keyframes when asked; it speaks AVCC and the core Annex B, so `vt.rs` moves the
parameter sets between the format description and the stream each way, and nothing
is fetched. Its cameras come through AVFoundation (`avf.rs`): listed by unique id,
captured as NV12 on a dispatch queue of their own by an Objective-C delegate defined
in Rust, a picture the source is not ready for dropped. Opening only checks that
access is not refused: the session starts, and the system asks for access the first
time, on the source's thread at its first picture, so a call goes on while the user
answers. On Windows, H.264 goes through Media Foundation's own encoder and decoder
transforms (`mf.rs`; Windows' licence covers H.264, so nothing is fetched): made on
the thread that uses each, in the multithreaded apartment; the encoder set for low
latency, CBR and baseline, a keyframe forced through its codec API; NV12 each way, the
decoder's output cropped from its coded size to the picture's. A Windows without them
(an N edition without the Media Feature Pack) offers no video. Its cameras come through Media Foundation (`mfcam.rs`): listed
by device link with their friendly names, read synchronously through a source reader
whose video processing turns whatever the camera sends (YUY2, MJPEG, NV12) into NV12
at the camera's largest size not above the one asked. Windows asks nothing at the
call: access is the user's setting (Privacy & security → Camera), and a camera it
refuses fails to open, saying where to allow it. `anvil-cli
--video camera` films with the computer's camera, `--video pattern` a moving test
pattern through the same codecs, and `--video-snapshot` writes the last picture
received as a PNG. The ladder steps the picture down only when the network is the
limit: a far end's estimate follows what it receives (at most 1.5× it), so while the
encoder sends well under its target (a still picture codes small) the picture keeps
its size. A new or resumed incoming stream reaches its sink only from its first
keyframe, and asks for one if that keyframe never came (a relay drops a stream's first
packets until it has latched).

**The app** shows video on Flutter textures, and pictures never cross into Dart. The
runner's texture plugin (`linux/runner/video_texture.cc`, `VideoTextures` in
`macos/Runner/MainFlutterWindow.swift`, or `windows/runner/video_texture.cpp`, over
`anvil/video`) makes a pixel-buffer
texture and hands Dart its id, the C function that fills it and the
texture's address; Dart hands those to the bridge (`attach_tile`), whose `Renderer`
converts each picture to RGBA (BGRA, Core Video's order, on macOS) and calls the function on the host's thread. Detaching
takes the renderer's lock, so the function is never called once `detach_tile`
returns, and only then is the texture disposed of. `anvil-app` keeps each call's
video (offered, our camera, our stream, the streams coming in, the far end asking)
from the core's events, and the device's (cameras, the one chosen, H.264). The core's
`set_camera` reaches the host, which switches a camera that is sending (closed, the
new one opened, a keyframe) and opens the chosen one for each new send. The chosen
camera and whether H.264 is allowed are kept with the phone's settings; with H.264
off, OpenH264 is neither fetched nor offered. libvpx is linked statically where its
archive is installed, so the packages need no libvpx of their own.

---

## 6. Threading and async

Everything is Tokio. One runtime, multi-threaded on desktop, current-thread on mobile
(so the host platform controls scheduling). Anvil core owns:

- one SIP transport task (sip-transport),
- one transaction manager (sip-transaction),
- N per-call tasks: one for SIP dialog work, one for media I/O pump.

Audio capture and playback run on dedicated OS threads (not Tokio) owned by the host —
classic real-time audio rules: no allocation, no locks, no awaits. Communication with
the async world is via fixed-size ring buffers.

```
 [audio in thread] ─ringbuf─► [rtp encode task] ─► socket
 socket ─► [rtp decode task] ─ringbuf─► [audio out thread]
```

This boundary is the single most performance-sensitive contract in the codebase. Keep
the ring buffers small (2–3 frames), use `crossbeam::ArrayQueue`, never block.

---

## 7. Error model

```rust
#[derive(thiserror::Error, Debug)]
pub enum AnvilError {
    #[error("not registered")]              NotRegistered,
    #[error("no such call: {0}")]           NoSuchCall(CallId),
    #[error("transport: {0}")]              Transport(#[source] SipTransportError),
    #[error("auth rejected: {0}")]          AuthRejected(String),
    #[error("media: {0}")]                  Media(#[source] ForgeEngineError),
    #[error("audio device: {0}")]           AudioDevice(String),
    #[error("codec: {0}")]                  Codec(String),
    #[error("sdp negotiation failed: {0}")] SdpNegotiation(String),
    #[error(transparent)]                   Other(#[from] anyhow::Error),
}
```

Upstream error types are wrapped, not re-exported. Callers should not have to import
`siphon-rs` or `forge-media` to match on Anvil errors.

---

## 8. Configuration and secrets

- Config is plain structs constructed in code. No file format is prescribed.
- Secrets (`password`) use the `secrecy` crate so they don't leak into `Debug` output.
- The CLI reads a TOML file into these structs and passes it to `Anvil::start`.

---

## 9. Observability

- `tracing` with structured spans: every call gets a span (`call_id = …`) and every
  SIP transaction gets a sub-span.
- `MediaStats` event is emitted every 2 seconds during active calls (jitter, loss,
  RTT if RTCP is enabled, codec).
- No metrics endpoint inside the library — hosts scrape `MediaStats` if they want a
  dashboard.

---

## 10. Platform strategy (summary — details in PLATFORM_INTEGRATION.md)

| Concern                 | Desktop                  | iOS                         | Android                           |
|-------------------------|--------------------------|-----------------------------|-----------------------------------|
| Audio                   | cpal + APM               | AVAudioEngine (Apple AEC)   | Oboe (OpenSL/AAudio under hood)   |
| Incoming wake           | always-on process        | PushKit VoIP push           | FCM high-prio + foreground svc    |
| System call UI          | app window               | CallKit (mandatory)         | ConnectionService                 |
| Background execution    | free                     | PushKit wakes the process   | foreground service + notification |
| NAT traversal           | STUN (forge-rtp)         | same                        | same                              |
| Build target            | native                   | `aarch64-apple-ios`         | `aarch64-linux-android` + NDK     |

Anvil's Rust core ships identically on all five. The differences sit in the
platform-native shim that consumes `anvil-ffi`.

---

## 11. Tenant branding (FCP)

A company deploying FCP can re-theme Anvil per tenant. The softphone fetches a
`BrandProfile` on login — logo, colors, app name, support links, optional dial
plan — and the UI re-shells in response. Full wire-protocol spec in
`docs/BRANDING.md`; this section covers only the Rust-side architecture.

### Types

`anvil_core::brand` defines the data model: `BrandProfile`, `BrandColors`,
`BrandAsset`, `Ringtone`, `BrandLinks`, `DialPlan`. Plus two traits:

```rust
#[async_trait]
pub trait BrandProvider: Send + Sync {
    async fn fetch(&self, req: BrandRequest)
        -> Result<BrandFetchOutcome, AnvilError>;
}

#[async_trait]
pub trait BrandCache: Send + Sync {
    async fn load (&self, tenant_id: &str) -> Result<Option<BrandProfile>, _>;
    async fn store(&self, profile: &BrandProfile) -> Result<(), _>;
    async fn clear(&self, tenant_id: &str)        -> Result<(), _>;
}
```

### Wiring

`AnvilConfig.brand: BrandConfig` carries an optional `BrandProvider` + `BrandCache`
+ credential. Desktop plugs in `anvil_brand::HttpBrandProvider` and
`anvil_brand::FsBrandCache`. Mobile hosts inject their own — iOS uses URLSession +
a Keychain-backed cache; Android uses OkHttp + EncryptedSharedPreferences. That
lets the OS keep TLS pinning, cert management, and secure storage under its own
control. Anvil core treats these as opaque trait objects.

### Flow

```
Anvil::start(cfg)
│
├─ if cfg.brand.provider.is_some():
│     ├─ cache.load(tenant) ─►  Some(cached) ─► emit BrandUpdated(cached)
│     │                        None           ─► continue
│     └─ spawn task: provider.fetch(if_none_match = cached.etag)
│              ├─ Updated(fresh)  ─► cache.store; emit BrandUpdated
│              └─ NotModified     ─► (no-op)
│
└─ concurrently: start SIP transport, REGISTER, etc.
```

Two delivery rule: UIs may receive `BrandUpdated` up to twice per login (cached
then fresh). They must be idempotent.

### Discovery

In order: explicit `AccountConfig.provisioning_url` → `X-FCP-Provisioning-Url`
header on the REGISTER 200 OK → `.well-known/fcp-provisioning`. No URL →
branding is skipped without error, host renders with its default theme.

The second path requires signaling hand-off from `sip-transport` into the
branding task. We wire this through an internal channel that the REGISTER
response handler pushes into; the branding task drains it if no explicit URL
is configured.

### Refresh

- On login, always.
- On REGISTER refresh (every `register_expires` seconds), only if > 1 hour since
  last fetch. Uses `If-None-Match` so most hits are 304.
- On explicit `Anvil::refresh_brand()` from the UI.

Server-push (SIP NOTIFY with event package `fcp-brand`) is deferred to v2.

### Asset handling

Assets are content-addressed by SHA-256. `anvil-brand` downloads them, verifies
the hash, and writes them into the cache directory keyed by hash so multiple
tenants sharing a ringtone or default logo don't duplicate on disk. The
delivered `BrandProfile` has `BrandAsset.bytes` populated for each asset that
fetched successfully; failed assets become `None` and the UI falls back to its
built-in equivalent.

---

## 12. Open questions (to resolve before coding starts)

1. **SRTP:** SDES-SRTP via siphon-rs/forge-media, or only the DTLS-SRTP path already
   present in `forge-webrtc`? Most SIP PBXes expect SDES; this needs confirmation.
2. **Opus location:** implement inside `anvil-codec`, or upstream into `forge-codecs`?
   Upstream is better long-term but slower to iterate on.
3. **ICE for plain SIP:** needed for carrier-grade NAT? Probably, but Phase 2+.
4. **UI choice:** Flutter is the recommendation, but that decision can wait until
   `anvil-cli` proves the core works end to end.
5. **Account storage on mobile:** Keychain (iOS) / Keystore (Android) wiring is the
   host's job, not the core's — is that contract clear enough in the API? Likely
   add an `AccountStore` trait if it isn't.
6. **Brand auth token exchange:** the `BrandCredential::Bearer` path assumes FCP
   hands the client a token somewhere. Do we spec a dedicated
   `{provisioning_url}/auth/token` endpoint, or piggy-back on the SIP
   `Authentication-Info` header? Needs an FCP-side decision.
7. **Signed brand profiles:** v1 of the protocol relies on TLS for integrity. A
   post-1.0 signature scheme would protect against a compromised CDN + server.
   Key distribution is the hard part — skip for v1, revisit if customers ask.
