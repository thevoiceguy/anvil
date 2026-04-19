//! anvil-cli — terminal softphone.
//!
//! Phase 1 M2: REGISTER, then place an outgoing call if `--call` is given.
//! Prints every event. Hangs up on Ctrl+C. No media flow yet (the INVITE
//! advertises an RTP port, but no audio frames cross the wire).

use std::time::Duration;

use std::sync::Arc;

use anvil_audio::{CpalHost, ToneHost, ToneStats};
use anvil_core::{
    AccountConfig, Anvil, AnvilConfig, BrandConfig, CallId, EndReason, Event, MediaConfig,
    Transport,
};
use anyhow::Result;
use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "anvil-cli", about = "Anvil terminal softphone (Phase 1 M2)")]
struct Cli {
    /// Address-of-record, e.g. `sip:alice@example.com`.
    #[arg(long)]
    aor: String,

    /// Registrar host or URI, e.g. `sip:example.com` or `example.com:5060`.
    #[arg(long)]
    registrar: String,

    /// SIP username.
    #[arg(long)]
    username: String,

    /// SIP password.
    #[arg(long, env = "ANVIL_PASSWORD")]
    password: String,

    /// Optional target to call after registering, e.g. `sip:bob@example.com`.
    /// If omitted, anvil-cli just holds the registration.
    #[arg(long)]
    call: Option<String>,

    /// Seconds to stay in the call before hanging up.
    #[arg(long, default_value = "10")]
    talk: u64,

    /// Seconds to stay registered before unregistering (when not calling).
    #[arg(long, default_value = "30")]
    hold: u64,

    /// Local bind address. Defaults to `0.0.0.0:0` (ephemeral port).
    #[arg(long)]
    bind: Option<String>,

    /// Use the synthetic tone generator instead of cpal (for headless CI
    /// and hardware-less dev boxes). When a call is established the
    /// sink's peak RMS is printed at hangup.
    #[arg(long)]
    tone: bool,
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

    let account = AccountConfig {
        aor: cli.aor.clone(),
        registrar: cli.registrar.clone(),
        username: cli.username.clone(),
        password: cli.password.clone(),
        transport: Transport::Udp,
        outbound_proxy: None,
        stun: None,
        register_expires: Duration::from_secs(3600),
        user_agent: format!("Anvil/{} (phase1-m3)", env!("CARGO_PKG_VERSION")),
        bind_addr: cli.bind.clone(),
        provisioning_url: None,
    };

    let (audio, tone_stats): (Box<dyn anvil_core::audio::AudioHost>, Option<Arc<ToneStats>>) =
        if cli.tone {
            tracing::info!("using synthetic tone audio host");
            let host = ToneHost::new();
            let stats = host.stats();
            (Box::new(host), Some(stats))
        } else {
            let host = CpalHost::new()
                .map_err(|e| anyhow::anyhow!("CpalHost init failed: {e}"))?;
            (Box::new(host), None)
        };

    let cfg = AnvilConfig {
        account,
        media: MediaConfig::default(),
        audio,
        brand: BrandConfig::default(),
    };

    run(cli, cfg, tone_stats).await
}

async fn run(cli: Cli, cfg: AnvilConfig, tone_stats: Option<Arc<ToneStats>>) -> Result<()> {

    tracing::info!("starting Anvil");
    let (anvil, mut events) = Anvil::start(cfg).await?;

    // Event drainer. Signals `call_ended` when a CallEnded event arrives so
    // the driver task can exit early if the remote hangs up first.
    let (call_ended_tx, mut call_ended_rx) = tokio::sync::mpsc::unbounded_channel::<CallId>();
    let printer = tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            match &event {
                Event::CallEnded { call, reason } => {
                    println!("[event] CallEnded {{ call: {call:?}, reason: {reason:?} }}");
                    match reason {
                        EndReason::LocalHangup => {}
                        _ => {
                            let _ = call_ended_tx.send(*call);
                        }
                    }
                }
                Event::RegistrationChanged { state, reason } => {
                    println!(
                        "[reg] {state:?}{}",
                        reason.as_ref().map(|r| format!(" ({r})")).unwrap_or_default()
                    );
                }
                other => println!("[event] {other:?}"),
            }
        }
    });

    tracing::info!("registering...");
    anvil.register().await?;

    if let Some(target) = cli.call.clone() {
        tracing::info!(%target, "placing call");
        match anvil.place_call(&target).await {
            Ok(call_id) => {
                tracing::info!(?call_id, "call placed; waiting");
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(cli.talk)) => {
                        tracing::info!("talk timer expired, hanging up");
                        if let Err(e) = anvil.hangup(call_id).await {
                            tracing::warn!(%e, "hangup failed");
                        }
                    }
                    _ = tokio::signal::ctrl_c() => {
                        tracing::info!("Ctrl+C, hanging up");
                        if let Err(e) = anvil.hangup(call_id).await {
                            tracing::warn!(%e, "hangup failed");
                        }
                    }
                    ended = call_ended_rx.recv() => {
                        tracing::info!(?ended, "call ended remotely");
                    }
                }
            }
            Err(e) => tracing::error!(%e, "place_call failed"),
        }
    } else {
        tracing::info!(hold_seconds = cli.hold, "registered; holding");
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(cli.hold)) => {}
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("received Ctrl+C");
            }
        }
    }

    if let Some(stats) = tone_stats.as_ref() {
        println!(
            "[tone] frames_rx={} last_rms={:.0} peak_rms={:.0}",
            stats.frames(),
            stats.last_rms(),
            stats.peak_rms()
        );
    }

    tracing::info!("unregistering...");
    let _ = anvil.unregister().await;
    anvil.shutdown().await?;

    printer.abort();
    Ok(())
}

