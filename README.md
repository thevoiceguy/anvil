# Anvil

A cross-platform softphone built in Rust.

Anvil is a SIP user agent that runs on **Linux, macOS, Windows, iOS, and Android**
from a single Rust core. It builds on two existing workspaces:

- [**siphon-rs**](https://github.com/thevoiceguy/siphon-rs) — RFC 3261 SIP signaling stack
- [**forge-media**](https://github.com/thevoiceguy/forge-media) — RTP / codec / media engine

Both are git dependencies pinned to release tags. Anvil is the softphone of
[FCP](https://github.com/thevoiceguy/fcp), and is tested against it.

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

# Signed in to FCP: the account comes from FCP.
cargo run -p anvil-cli -- login https://pbx.example.com    # opens the browser
cargo run -p anvil-cli -- --call sip:bob@example.com

# Or an account by hand.
cargo run -p anvil-cli -- --aor sip:alice@example.com --registrar sip:example.com \
    --username alice --password …
```

## Crates

| Crate         | Purpose                                                       |
|---------------|---------------------------------------------------------------|
| `anvil-core`  | Softphone state machine; orchestrates SIP + media             |
| `anvil-audio` | Cross-platform mic/speaker via `cpal`, plus AEC/NS/AGC        |
| `anvil-brand` | FCP tenant branding — HTTP fetch + filesystem cache           |
| `anvil-codec` | Opus wrapper (forge-media already provides G.711/G.722)       |
| `anvil-fcp`   | FCP's client: discovery, sign-in, the app session, settings |
| `anvil-ffi`   | C ABI surface for mobile/native UI hosts                      |
| `anvil-cli`   | Terminal softphone — reference UAC and dev-loop test harness  |

## Tenant branding

A company deploying FCP can brand Anvil with its own logo, colors, and app
name. The softphone downloads the brand profile on login and the UI re-themes
without any custom build. Protocol: [`docs/BRANDING.md`](docs/BRANDING.md).

## License

Dual-licensed under MIT OR Apache-2.0.
