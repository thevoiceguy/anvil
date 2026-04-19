//! anvil-cli — terminal softphone.
//!
//! Reference UAC that drives `anvil-core` from a plain interactive prompt.
//! Intended as the dev loop for Phase 1: prove REGISTER → INVITE → audio → BYE
//! works before investing in any GUI.

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,anvil=debug".into()),
        )
        .init();

    tracing::info!("anvil-cli starting (stub)");
    println!("anvil-cli is a skeleton. See docs/ROADMAP.md — Phase 1.");
    Ok(())
}
