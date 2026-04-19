# Anvil — project context for Claude Code

Anvil is a cross-platform softphone (iOS, Android, Windows, macOS, Linux) built on two
sibling Rust workspaces:

- `../siphon-rs` — RFC 3261 SIP signaling stack (pure Rust, Tokio)
- `../forge-media` — RTP / codec / media engine (Tokio; some Linux-only features)

The Anvil workspace provides the integration layer: call orchestration, platform audio
I/O, an FFI boundary for mobile UIs, and a reference CLI client.

## Workspace layout

```
anvil/
├── crates/
│   ├── anvil-core    # call state machine; orchestrates siphon-rs + forge-media
│   ├── anvil-audio   # cpal mic/speaker I/O, AEC/NS/AGC, device routing
│   ├── anvil-brand   # FCP brand provisioning (HTTP + filesystem cache)
│   ├── anvil-codec   # Opus wrapper (forge provides G.711/G.722)
│   └── anvil-ffi     # C ABI for mobile UI integration
└── bins/
    └── anvil-cli     # terminal softphone — reference UAC and integration test
```

UI layer (Flutter + flutter_rust_bridge recommended) lives **outside** this Rust
workspace so Rust builds stay hermetic. See `docs/ARCHITECTURE.md`.

## Design rules

### Dependency direction

```
anvil-cli → anvil-core → {siphon-rs crates, forge-media crates}
                       ↘ anvil-audio → cpal
                       ↘ anvil-brand → reqwest
                       ↘ anvil-codec → opus
anvil-ffi → anvil-core (re-exports a C ABI)
```

`anvil-core` never depends on `anvil-audio`, `anvil-brand`, or `anvil-codec`
directly — it declares traits (`AudioHost`, `BrandProvider`, `BrandCache`, codec
types) that the platform layer supplies. This keeps the core testable without
devices and lets mobile hosts inject their own audio pipeline (AVAudioEngine,
Oboe) and their own HTTP stack (URLSession, OkHttp) when the host OS owns those
concerns.

### Platform gating

- Anvil core must compile on all five targets. No `cfg(target_os = ...)` in core logic.
- Platform code lives in `anvil-audio` behind cfg gates, or in FFI consumers.
- Avoid forge-media's Linux-only crates (`forge-kernel`, `forge-kernel-ebpf`). The
  features we use (`forge-engine`, `forge-rtp`, `forge-codecs`, `forge-dtmf`,
  `forge-sdp`, `forge-injection`) are portable.

### Async

Everything is Tokio. No separate runtimes. Long-running tasks spawn onto the shared
runtime; callers provide a `Handle` if embedding.

### Error handling

- Library crates: `thiserror` with a per-crate `Error` enum.
- Binaries / examples: `anyhow`.
- Never leak `siphon`/`forge` error types across `anvil-core`'s public API — wrap them.

### FFI

- `anvil-ffi` is `#[no_mangle] extern "C"` only. No panics across the boundary;
  every entry point is `catch_unwind`-guarded.
- Handles are opaque (`*mut AnvilHandle`); freed via `anvil_destroy(handle)`.
- Events cross the FFI via a single C callback registered at init time.

## Key upstream APIs

Softphone-relevant siphon-rs entry points (read the crate docs before extending):

- `sip_uac::UserAgentClient` — builds REGISTER, INVITE, BYE, REFER, PRACK, SUBSCRIBE.
  Handles 401/407 auto-retry.
- `sip_uas::UserAgentServer` — response builders + `accept_invite()` for incoming calls.
- `sip_dialog::Dialog` / `DialogManager` — early/confirmed/terminated state, route set.
- `sip_transaction::TransactionManager` — retransmissions, timers.
- `sip_sdp` — RFC 4566 / 3264 offer–answer.

Softphone-relevant forge-media entry points:

- `forge_engine::SessionManager` — create media sessions, negotiate codecs, forward RTP.
- `forge_rtp::RtpSession` — per-call RTP socket + jitter buffer.
- `forge_sdp` — SDP negotiation helpers that complement `sip_sdp`.
- `forge_codecs` — G.711 (PCMU/PCMA), G.722, internal Opus path via `opus` crate.
- `forge_dtmf` — RFC 2833 detect/generate.
- `forge_injection` — file playback, tone generation, resampling.

## Build & dev loop

```bash
# Desktop dev (Linux / macOS / Windows)
cargo build --workspace
cargo test  --workspace
cargo run   -p anvil-cli -- --help

# Mobile cross-compile (Phase 3)
cargo build -p anvil-ffi --target aarch64-apple-ios --release
cargo build -p anvil-ffi --target aarch64-linux-android --release
```

Install once: `cargo install cargo-ndk` (Android) and the usual `rustup target add`s.

## Conventions

- Rust 2021, MSRV 1.75 (matches forge-media).
- `rustfmt` on save. `clippy -- -D warnings` in CI.
- Public types: derive `Debug`; derive `Clone` only when cheap. Never derive `Copy`
  on anything containing a handle or `Arc`.
- Prefer `tracing` spans over ad-hoc logging; softphone flows are async and
  interleaved, so structured spans matter.
- No `unwrap()` outside tests and `main`.

## Tenant branding (FCP)

Anvil is designed to be brandable per FCP tenant. On login the softphone fetches a
`BrandProfile` from the FCP provisioning endpoint (logo, colors, app name, support
links, optional dial-plan hints) and emits `Event::BrandUpdated`. The UI re-themes
in response.

- Protocol lives in `docs/BRANDING.md`.
- Types + traits in `anvil_core::brand`.
- Default desktop implementations in `anvil-brand`.
- Branding is optional — if no provisioning URL is available, the UI renders with
  its built-in default theme and no error is raised.
- Never block REGISTER on brand fetch for returning users — cache is served first,
  fresh fetch races REGISTER.

## Non-goals (for now)

- No proxy / B2BUA / registrar mode. Anvil is a UAC/UAS endpoint only.
- No video. Audio-only softphone.
- No conferencing within Anvil — forge-media has it, but composing it here is out of scope.
- No in-app call recording until after Phase 2 (storage + legal review).
- No Windows SIP TLS cert pinning until forge-media/siphon-rs standardize it.

## Where things are

- `docs/ARCHITECTURE.md` — full design, crate API sketches, call-flow diagrams.
- `docs/ROADMAP.md` — phased delivery plan.
- `docs/PLATFORM_INTEGRATION.md` — per-OS notes (CallKit, ConnectionService, push).
- `docs/BRANDING.md` — FCP brand provisioning wire protocol.
