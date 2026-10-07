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

Pre-alpha, without a UI: everything is driven from `anvil-cli` or through
`anvil-ffi`. See [`docs/ROADMAP.md`](docs/ROADMAP.md) for the phased plan.

What works today, tested against FCP:

- **Signing in** through the browser (OAuth 2.0 with PKCE) or a password;
  the install is a device with its own SIP account, kept registered.
- **Calls** over UDP, TCP or TLS: Opus, G.722 and G.711, RFC 2833 DTMF,
  hold, blind and attended transfer, park, call waiting.
- **Encrypted media** (SDES-SRTP), optional or required; on by default when
  FCP serves the account over TLS.
- **Message waiting, presence and busy lamps** (SUBSCRIBE/NOTIFY).
- **The user's data from FCP**: call history, the directory with presence,
  voicemail, calling settings (do not disturb, forwards), and live events.
- **The tenant's brand**: logo, colors, ringtones, app name.

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

# A phone that keeps running, and commands it takes from anywhere.
cargo run -p anvil-cli -- run                     # registered, ringing, a prompt
cargo run -p anvil-cli -- call 1002               # from another terminal or a script
cargo run -p anvil-cli -- answer | hangup | hold | resume | mute | transfer 1003 | status
cargo run -p anvil-cli -- watch                   # its changes as they happen

# The user's own data.
cargo run -p anvil-cli -- calls --missed
cargo run -p anvil-cli -- directory ann
cargo run -p anvil-cli -- voicemail
cargo run -p anvil-cli -- settings --dnd true
cargo run -p anvil-cli -- events
```

## Windows

Each release has `anvil-cli` for Windows (x86_64) on its GitHub release page:
unzip it and run

```powershell
.\anvil-cli.exe login https://pbx.example.com
.\anvil-cli.exe --call sip:1002@example.com
```

It needs nothing installed (the C runtime is linked in). It is not
code-signed yet, so SmartScreen may ask before running it. The first time it
opens its network ports, Windows Firewall asks to allow it.

To build it yourself: Visual Studio Build Tools with "Desktop development
with C++", Rust from rustup (the MSVC toolchain), CMake (for libopus), then
`cargo build --release -p anvil-cli`. CI builds and tests the workspace on
Windows on every change.

## Testing against FCP

`cargo test --workspace` needs nothing. The tests that need a running FCP
(`anvil-fcp/tests/fcp.rs`, `anvil-ffi/tests/fcp.rs`) skip unless told where
it is:

```bash
ANVIL_FCP_ADMIN=http://127.0.0.1:8080 ANVIL_FCP_TOKEN=<admin token> \
ANVIL_FCP_SIP=127.0.0.1:5060 \
ANVIL_FCP_SIPS=127.0.0.1:5061 ANVIL_FCP_CA=/path/to/ca.pem \
    cargo test -p anvil-fcp -p anvil-ffi --test fcp
```

The TLS pair is only for the encrypted-call test. FCP's `/me/events` carries
call events only when its admin and call manager share an events server
(`[events] server_url`).

## Crates

| Crate         | Purpose                                                       |
|---------------|---------------------------------------------------------------|
| `anvil-core`  | Softphone state machine; orchestrates SIP + media             |
| `anvil-audio` | Cross-platform mic/speaker via `cpal`, plus AEC/NS/AGC        |
| `anvil-brand` | FCP tenant branding — HTTP fetch + filesystem cache           |
| `anvil-codec` | Opus wrapper (forge-media already provides G.711/G.722)       |
| `anvil-fcp`   | FCP's client: discovery, sign-in, the app session, settings, the user's data and events |
| `anvil-ffi`   | C ABI surface for mobile/native UI hosts                      |
| `anvil-cli`   | Terminal softphone — reference UAC and dev-loop test harness  |

## Tenant branding

A company deploying FCP can brand Anvil with its own logo, colors, and app
name. The softphone downloads the brand profile on login and the UI re-themes
without any custom build. Protocol: [`docs/BRANDING.md`](docs/BRANDING.md).

## License

Dual-licensed under MIT OR Apache-2.0.
