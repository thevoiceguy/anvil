# Anvil — architecture

This is the design doc. It captures the crate boundaries, the public surface of
`anvil-core`, how SIP signaling is glued to RTP media, the threading model, and the
extension points for platform integration.

---

## 1. Goals and non-goals

**Goals.** One Rust core that runs on Linux, macOS, Windows, iOS, and Android, exposing
a stable API that platform-native UIs consume. SIP-based calling (REGISTER, INVITE,
BYE, REFER, basic SUBSCRIBE/NOTIFY for presence). Audio-only, G.711 + G.722 + Opus.
RFC 2833 DTMF. NAT-friendly over UDP, TCP, TLS.

**Non-goals.** Video. In-app recording beyond basic RTP tap. Acting as a proxy, B2BUA,
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
depend on `anvil-audio`, `anvil-brand`, or `anvil-codec`. Instead it defines traits
(`AudioHost`, `BrandProvider`, `BrandCache`, and codec types) and those crates
supply implementations. This lets the core stay testable without real devices or
network access, and lets mobile hosts drop in their own audio pipeline
(AVAudioEngine on iOS, Oboe on Android) and HTTP stack (URLSession, OkHttp) when
the host OS owns those concerns.

### Feature flags

| Crate         | Feature         | Purpose                                              |
|---------------|-----------------|------------------------------------------------------|
| `anvil-core`  | `tls`           | Enable TLS transport via siphon-rs (on by default)   |
| `anvil-core`  | `srtp`          | SDES-SRTP for non-WebRTC SIP peers                   |
| `anvil-audio` | `apm`           | Link WebRTC audio-processing (AEC/NS/AGC)            |
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

- `anvil-audio` — desktop, `cpal` behind the host. Applies APM (AEC/NS/AGC) when the
  `apm` feature is on.
- **iOS**: host backed by `AVAudioEngine`, owned by Swift; bridged through FFI.
  Apple already does AEC for us when using the voice-processing audio unit.
- **Android**: host backed by Oboe / AudioRecord, bridged through JNI.
- **Testing**: `InMemoryHost` that replays wav fixtures and records sink output.

Keeping the host trait out of `anvil-core`'s own crate graph means the core compiles
even when `cpal` is unavailable (Android targets without NDK setup, CI without audio).

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
