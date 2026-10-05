# Contributing to Anvil

Thanks for your interest. Anvil is in pre-alpha; the design is still in motion.

## Before you open a PR

- Read [`CLAUDE.md`](CLAUDE.md) for the crate layout and dependency rules.
- Read [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the public API shape and
  the SIP↔media glue.
- Open an issue for anything more than a small fix — the scope of the project is
  narrow on purpose and we'd rather discuss direction first.

## Dev loop

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace   # see CLAUDE.md for the tests against a running FCP
cargo run  -p anvil-cli
```

## Commit style

- Imperative mood, short subject line, optional body explaining *why*.
- Reference issues with `Fixes #N` or `Refs #N`.

## Licensing

By contributing you agree your work will be dual-licensed under MIT OR Apache-2.0.
