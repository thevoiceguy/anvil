# Anvil

A cross-platform softphone built in Rust.

Anvil is a SIP user agent that runs on **Linux, macOS, Windows, iOS, and Android**
from a single Rust core. It builds on two existing workspaces:

- [**siphon-rs**](../siphon-rs) — RFC 3261 SIP signaling stack
- [**forge-media**](../forge-media) — RTP / codec / media engine

Anvil is the integration layer on top: call orchestration, platform audio I/O, an
FFI bridge for mobile UIs, and a reference CLI client.

## Status

Pre-alpha. See [`docs/ROADMAP.md`](docs/ROADMAP.md) for the phased plan.

## Architecture (at a glance)

```
┌───────────────────────────────────────────────────────────┐
│                      UI (per platform)                    │
│         Flutter · SwiftUI · Jetpack · Tauri · TUI         │
├───────────────────────────────────────────────────────────┤
│                 anvil-ffi  (C ABI, opaque handles)        │
├───────────────────────────────────────────────────────────┤
│                        anvil-core                         │
│   Account · Call · Registration · Event · Dial plan       │
├──────────────────────┬──────────────────┬─────────────────┤
│      siphon-rs       │   forge-media    │   anvil-audio   │
│  (SIP signaling)     │  (RTP / codecs)  │  (mic/speaker)  │
└──────────────────────┴──────────────────┴─────────────────┘
```

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the full design.

## Build

```bash
cargo build --workspace
cargo run   -p anvil-cli
```

## Crates

| Crate         | Purpose                                                       |
|---------------|---------------------------------------------------------------|
| `anvil-core`  | Softphone state machine; orchestrates SIP + media             |
| `anvil-audio` | Cross-platform mic/speaker via `cpal`, plus AEC/NS/AGC        |
| `anvil-brand` | FCP tenant branding — HTTP fetch + filesystem cache           |
| `anvil-codec` | Opus wrapper (forge-media already provides G.711/G.722)       |
| `anvil-ffi`   | C ABI surface for mobile/native UI hosts                      |
| `anvil-cli`   | Terminal softphone — reference UAC and dev-loop test harness  |

## Tenant branding

A company deploying FCP can brand Anvil with its own logo, colors, and app
name. The softphone downloads the brand profile on login and the UI re-themes
without any custom build. Protocol: [`docs/BRANDING.md`](docs/BRANDING.md).

## License

Dual-licensed under MIT OR Apache-2.0.
