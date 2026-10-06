# Anvil — agent conventions

Short notes for automated agents (Claude Code, CI bots, code reviewers) working in
this repo. Human-facing design lives in `docs/`; human contributor guidance is in
`CONTRIBUTING.md`. This file is deliberately terse.

## Ground rules

1. **Read `CLAUDE.md` first.** It defines crate boundaries and dependency direction.
2. **`anvil-core` has no dependency on `anvil-audio`, `anvil-brand`, or
   `anvil-codec`.** If a change makes it depend on any of them, it's wrong — add
   a trait in `anvil-core::audio`, `anvil-core::brand`, etc. and supply the
   implementation from the platform crate.
3. **Upstream workspaces.** siphon-rs and forge-media are git dependencies pinned to
   release tags. Don't patch them from here: change them in their own repos, release a
   tag, and move the tag in the root `Cargo.toml`.
4. **Platform code is gated.** `#[cfg(target_os = ...)]` lives in `anvil-audio` or
   the FFI consumers, never in `anvil-core`.
5. **Async runtime.** Tokio only. No `async-std`, no custom executors.
6. **Errors.** Library crates use `thiserror` with a local `Error` enum. Binaries
   use `anyhow`. Never re-export siphon-rs or forge-media error types from Anvil's
   public API.
7. **Unsafe.** `#![forbid(unsafe_code)]` everywhere except `anvil-ffi`. In `anvil-ffi`,
   every public entry point must `catch_unwind`.
8. **Testing.** Prefer integration tests under `crates/*/tests/` that use the
   `sip-testkit` helpers and an in-memory `AudioHost` (to be added). Don't mock the
   SIP parser — feed real messages.
9. **No `unwrap()` outside tests and binaries.**
10. **Formatting.** `cargo fmt --all` before commit. `cargo clippy --all-targets --
    -D warnings` must pass.

## Making changes

- **Workspace deps go in the root `Cargo.toml`.** Crate Cargo.tomls use
  `workspace = true`. Keeps versions aligned.
- **Don't bump MSRV without discussion.** Currently 1.97, the toolchain CI lints on.
- **Don't add UI crates to this workspace.** The UI (Flutter/Tauri/native) lives
  outside the Rust tree and consumes `anvil-ffi`.

## When the task is ambiguous

Stop and ask. Softphones interoperate with third-party PBXes and carriers; guessing
at SDP handling, auth edge cases, or NAT traversal strategy usually produces
something that works on localhost and fails in production.
