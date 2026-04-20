//! anvil-cli — terminal softphone.
//!
//! Phase 1 M2: REGISTER, then place an outgoing call if `--call` is given.
//! Prints every event. Hangs up on Ctrl+C. No media flow yet (the INVITE
//! advertises an RTP port, but no audio frames cross the wire).

use std::time::Duration;

use std::sync::Arc;

use anvil_audio::{CpalHost, ToneHost, ToneStats};
use anvil_core::{
    AccountConfig, Anvil, AnvilConfig, BrandConfig, CallId, Event, MediaConfig, Transport,
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

    /// Auto-answer incoming calls instead of rejecting them. Useful for
    /// two-instance tests where one side calls and the other listens.
    #[arg(long)]
    auto_answer: bool,

    /// Send these DTMF digits once the call is established, one per second.
    /// Example: --dtmf "1*2#9". Allowed characters: 0-9, *, #, A-D.
    #[arg(long)]
    dtmf: Option<String>,

    /// Put the call on hold `hold_at` seconds after CallEstablished, then
    /// resume at `hold_at + 2`. Omit to skip the hold probe.
    #[arg(long)]
    hold_at: Option<u64>,
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

    tracing::info!("registering...");
    anvil.register().await?;

    // Optionally place a call. Either way, we then run a single event-drain
    // loop until the call ends / hold timer fires / Ctrl+C.
    let outbound_id = if let Some(target) = cli.call.clone() {
        tracing::info!(%target, "placing call");
        match anvil.place_call(&target).await {
            Ok(id) => {
                tracing::info!(call_id = ?id, "call placed");
                Some(id)
            }
            Err(e) => {
                tracing::error!(%e, "place_call failed");
                None
            }
        }
    } else {
        None
    };

    let deadline = if outbound_id.is_some() {
        tokio::time::Instant::now() + Duration::from_secs(cli.talk)
    } else {
        tokio::time::Instant::now() + Duration::from_secs(cli.hold)
    };

    let mut active_call: Option<CallId> = outbound_id;

    loop {
        tokio::select! {
            event = events.recv() => {
                let Some(event) = event else { break };
                match event {
                    Event::RegistrationChanged { state, reason } => {
                        println!(
                            "[reg] {state:?}{}",
                            reason.map(|r| format!(" ({r})")).unwrap_or_default()
                        );
                    }
                    Event::IncomingCall { call, from, display_name } => {
                        let label = display_name.as_deref().unwrap_or(&from);
                        println!("[event] IncomingCall from {label}");
                        if cli.auto_answer && active_call.is_none() {
                            tracing::info!(?call, "auto-answering");
                            if let Err(e) = anvil.answer(call).await {
                                tracing::warn!(%e, "answer failed");
                            } else {
                                active_call = Some(call);
                            }
                        } else {
                            tracing::info!(?call, "rejecting (auto-answer off or busy)");
                            let _ = anvil.reject(call, 486).await;
                        }
                    }
                    Event::CallEstablished { call, codec } => {
                        println!("[event] CallEstablished {{ call: {call:?}, codec: {codec:?} }}");
                        if let Some(digits) = cli.dtmf.as_deref() {
                            // One-shot inline burst. The event loop stalls
                            // for `digits.len()` seconds while we send, which
                            // is fine for a smoke test — no events arrive
                            // during normal DTMF play.
                            for d in digits.chars() {
                                tokio::time::sleep(Duration::from_millis(1000)).await;
                                match anvil.send_dtmf(call, d).await {
                                    Ok(()) => println!("[dtmf] sent {d}"),
                                    Err(e) => tracing::warn!(%e, digit = %d, "send_dtmf failed"),
                                }
                            }
                        }
                        if let Some(secs) = cli.hold_at {
                            tokio::time::sleep(Duration::from_secs(secs)).await;
                            match anvil.hold(call, true).await {
                                Ok(()) => println!("[hold] on"),
                                Err(e) => tracing::warn!(%e, "hold on failed"),
                            }
                            tokio::time::sleep(Duration::from_secs(2)).await;
                            match anvil.hold(call, false).await {
                                Ok(()) => println!("[hold] off"),
                                Err(e) => tracing::warn!(%e, "hold off failed"),
                            }
                        }
                    }
                    Event::DtmfReceived { call, digit } => {
                        println!("[event] DtmfReceived {{ call: {call:?}, digit: {digit:?} }}");
                    }
                    Event::MediaStats { call, stats } => {
                        println!(
                            "[stats] call={call:?} codec={:?} jitter={:.1}ms loss={:.1}% rx={:.1}kbps tx={:.1}kbps",
                            stats.codec,
                            stats.jitter_ms,
                            stats.packet_loss_pct,
                            stats.recv_kbps,
                            stats.send_kbps,
                        );
                    }
                    Event::CallEnded { call, reason } => {
                        println!("[event] CallEnded {{ call: {call:?}, reason: {reason:?} }}");
                        if active_call == Some(call) {
                            break;
                        }
                    }
                    other => println!("[event] {other:?}"),
                }
            }
            _ = tokio::time::sleep_until(deadline) => {
                if let Some(id) = active_call {
                    tracing::info!("talk timer expired, hanging up");
                    if let Err(e) = anvil.hangup(id).await {
                        tracing::warn!(%e, "hangup failed");
                    }
                }
                break;
            }
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("Ctrl+C");
                if let Some(id) = active_call {
                    if let Err(e) = anvil.hangup(id).await {
                        tracing::warn!(%e, "hangup failed");
                    }
                }
                break;
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

    let _ = cli; // silence unused when there are no more flags to read
    Ok(())
}

