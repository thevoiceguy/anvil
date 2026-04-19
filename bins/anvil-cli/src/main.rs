//! anvil-cli — terminal softphone.
//!
//! Phase 1 Milestone 1: REGISTER smoke test. Connects to a SIP registrar,
//! sends REGISTER with Digest auth, prints every event, then unregisters on
//! Ctrl+C or after --hold seconds.
//!
//! Call handling, audio, and config files land with later milestones.

use std::time::Duration;

use anvil_core::{AccountConfig, Anvil, AnvilConfig, BrandConfig, Event, MediaConfig, Transport};
use anyhow::Result;
use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "anvil-cli", about = "Anvil terminal softphone (Phase 1 M1 smoke test)")]
struct Cli {
    /// Address-of-record, e.g. `sip:alice@example.com`.
    #[arg(long)]
    aor: String,

    /// Registrar host or URI, e.g. `sip:example.com` or `example.com:5060`.
    #[arg(long)]
    registrar: String,

    /// SIP username (often the same as the AOR user-part).
    #[arg(long)]
    username: String,

    /// SIP password.
    #[arg(long, env = "ANVIL_PASSWORD")]
    password: String,

    /// Seconds to stay registered before unregistering.
    #[arg(long, default_value = "30")]
    hold: u64,

    /// Local bind address. Defaults to `0.0.0.0:0` (ephemeral port).
    #[arg(long)]
    bind: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,anvil=debug".into()),
        )
        .init();

    let cli = Cli::parse();

    let cfg = AnvilConfig {
        account: AccountConfig {
            aor: cli.aor,
            registrar: cli.registrar,
            username: cli.username,
            password: cli.password,
            transport: Transport::Udp,
            outbound_proxy: None,
            stun: None,
            register_expires: Duration::from_secs(3600),
            user_agent: format!("Anvil/{} (phase1-m1)", env!("CARGO_PKG_VERSION")),
            bind_addr: cli.bind,
            provisioning_url: None,
        },
        media: MediaConfig::default(),
        audio: Box::new(NullAudioHost),
        brand: BrandConfig::default(),
    };

    tracing::info!("starting Anvil");
    let (anvil, mut events) = Anvil::start(cfg).await?;

    // Drain events to the console in the background.
    let printer = tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            match event {
                Event::RegistrationChanged { state, reason } => {
                    println!("[reg] {state:?}{}", reason.map(|r| format!(" ({r})")).unwrap_or_default());
                }
                other => println!("[event] {other:?}"),
            }
        }
    });

    tracing::info!("registering...");
    anvil.register().await?;

    tracing::info!(hold_seconds = cli.hold, "registered; holding");
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_secs(cli.hold)) => {}
        _ = tokio::signal::ctrl_c()                           => {
            tracing::info!("received Ctrl+C");
        }
    }

    tracing::info!("unregistering...");
    let _ = anvil.unregister().await;
    anvil.shutdown().await?;

    printer.abort();
    Ok(())
}

// -- Phase 1 M1 doesn't open audio yet; provide a host that refuses everything.
//    Replaced with anvil-audio::CpalHost once the audio milestone lands.
struct NullAudioHost;

impl anvil_core::audio::AudioHost for NullAudioHost {
    fn make_capture(
        &self,
        _cfg: anvil_core::audio::AudioFormat,
    ) -> std::result::Result<Box<dyn anvil_core::audio::AudioSource>, anvil_core::AnvilError> {
        Err(anvil_core::AnvilError::AudioDevice("no audio host in Phase 1 M1".into()))
    }

    fn make_playback(
        &self,
        _cfg: anvil_core::audio::AudioFormat,
    ) -> std::result::Result<Box<dyn anvil_core::audio::AudioSink>, anvil_core::AnvilError> {
        Err(anvil_core::AnvilError::AudioDevice("no audio host in Phase 1 M1".into()))
    }

    fn devices(&self) -> Vec<anvil_core::audio::DeviceInfo> { Vec::new() }
}
