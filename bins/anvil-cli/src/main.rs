//! anvil-cli — terminal softphone.
//!
//! Signed in to FCP (`anvil-cli login https://pbx.example.com`), it runs
//! with what FCP tells it; or give an account by hand with `--aor`,
//! `--registrar`, `--username` and `--password`.
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
use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "anvil-cli", about = "Anvil terminal softphone (Phase 1 M2)")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Where the FCP session is kept (default: the user's config directory).
    #[arg(long, env = "ANVIL_SESSION", global = true)]
    session: Option<std::path::PathBuf>,

    /// Address-of-record, e.g. `sip:alice@example.com`. Without it, the
    /// account comes from FCP (`anvil-cli login` first).
    #[arg(long)]
    aor: Option<String>,

    /// Registrar host or URI, e.g. `sip:example.com` or `example.com:5060`.
    #[arg(long)]
    registrar: Option<String>,

    /// SIP username.
    #[arg(long)]
    username: Option<String>,

    /// SIP password.
    #[arg(long, env = "ANVIL_PASSWORD")]
    password: Option<String>,

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

    /// Watch someone's presence and busy lamp (repeatable).
    #[arg(long = "watch", value_name = "AOR")]
    watch: Vec<String>,

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

    /// Blind-transfer the call to this address `transfer_after` seconds
    /// after CallEstablished; the server's progress prints as `[transfer]`.
    #[arg(long, value_name = "AOR")]
    transfer_to: Option<String>,

    /// Seconds into the call before `--transfer-to`.
    #[arg(long, default_value = "3")]
    transfer_after: u64,

    /// Use TLS (sips:) for SIP signaling. Default is UDP.
    #[arg(long)]
    tls: bool,

    /// Path to a PEM file holding extra trusted CA certs for TLS, on top
    /// of the system root store. Useful when the registrar uses a
    /// self-signed or private-CA cert.
    #[arg(long)]
    tls_ca: Option<std::path::PathBuf>,

    /// Outbound proxy every out-of-dialog request goes through, e.g.
    /// `sip:pbx.example.com:5060` — how an account in a domain reaches its
    /// provider's server (FCP's call manager).
    #[arg(long)]
    outbound_proxy: Option<String>,

    /// STUN server (`host:port`) for public-IP discovery. Result populates
    /// the `c=` line of outbound SDP. Example: `stun.l.google.com:19302`.
    #[arg(long)]
    stun: Option<String>,

    /// Run captured mic frames through anvil-audio's SimpleAgc before
    /// encoding. Off by default; turn on for noisy / quiet mics.
    #[arg(long)]
    agc: bool,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Sign in to FCP: through the browser, or with `--user` and a password.
    Login {
        /// The FCP server (`https://pbx.example.com`) or your email address.
        place: String,
        /// Sign in without a browser, as this user.
        #[arg(long)]
        user: Option<String>,
        /// The password for `--user`.
        #[arg(long, env = "ANVIL_FCP_PASSWORD")]
        password: Option<String>,
        /// The one-time code, when FCP asks for one.
        #[arg(long)]
        totp: Option<String>,
    },
    /// Sign out of FCP: the app's session ends.
    Logout,
}

/// Where the FCP session lives.
fn session_path(cli: &Cli) -> std::path::PathBuf {
    cli.session.clone().unwrap_or_else(|| {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join(".config")))
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        base.join("anvil").join("session.json")
    })
}

/// `anvil-cli login`: sign in, keep the session.
async fn login(
    cli: &Cli,
    place: &str,
    user: Option<&str>,
    password: Option<&str>,
    totp: Option<&str>,
) -> Result<()> {
    use anvil_fcp::TokenStore;
    let store = anvil_fcp::FileTokenStore::new(session_path(cli));
    // The same install signing in again keeps its device.
    let install = store
        .load()
        .ok()
        .flatten()
        .map(|s| s.client.install_id)
        .unwrap_or_else(anvil_fcp::AppClient::new_install_id);
    let client = anvil_fcp::AppClient::this_machine(install);
    let server = anvil_fcp::discover(place).await?;
    let session = match user {
        Some(user) => {
            let password = password.ok_or_else(|| {
                anyhow::anyhow!("--user needs --password (or ANVIL_FCP_PASSWORD)")
            })?;
            anvil_fcp::password_sign_in(server, client, user, password, totp).await?
        }
        None => {
            let signin = anvil_fcp::BrowserSignIn::start(server, client).await?;
            println!("Sign in in your browser:\n\n  {}\n", signin.authorize_url());
            open_browser(signin.authorize_url());
            signin.finish(Duration::from_secs(600)).await?
        }
    };
    store.save(&session)?;
    println!(
        "Signed in as {} on {} (device {}). Session kept in {}.",
        session.username,
        session.server.public_url,
        session
            .device
            .as_ref()
            .map(|d| d.sip_username.as_str())
            .unwrap_or("-"),
        store.path().display()
    );
    Ok(())
}

/// Ask the desktop to open `url`; the address was printed either way.
fn open_browser(url: &str) {
    let opener = if cfg!(target_os = "macos") {
        ("open", vec![url])
    } else if cfg!(target_os = "windows") {
        ("cmd", vec!["/C", "start", "", url])
    } else {
        ("xdg-open", vec![url])
    };
    let _ = std::process::Command::new(opener.0)
        .args(opener.1)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// The account from the FCP session, refreshed and kept, and the tenant's
/// brand fetched with it.
async fn fcp_account(cli: &Cli) -> Result<(AccountConfig, BrandConfig)> {
    use anvil_fcp::TokenStore;
    let store = Arc::new(anvil_fcp::FileTokenStore::new(session_path(cli)));
    let session = store.load()?.ok_or_else(|| {
        anyhow::anyhow!("not signed in: run `anvil-cli login <server>` or give --aor")
    })?;
    let client = Arc::new(anvil_fcp::FcpClient::new(
        session,
        Some(store as Arc<dyn TokenStore>),
    ));
    let config = client.softphone().await?;
    tracing::info!(aor = %config.account.aor, proxy = ?config.account.outbound_proxy, "account from FCP");
    let account = client.account_config(&config).await?;
    let brand = anvil_fcp::brand_config(Arc::clone(&client), None)?;
    Ok((account, brand))
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
    match &cli.command {
        Some(Command::Login {
            place,
            user,
            password,
            totp,
        }) => {
            return login(
                &cli,
                place,
                user.as_deref(),
                password.as_deref(),
                totp.as_deref(),
            )
            .await;
        }
        Some(Command::Logout) => {
            use anvil_fcp::TokenStore;
            let store = Arc::new(anvil_fcp::FileTokenStore::new(session_path(&cli)));
            if let Some(session) = store.load()? {
                anvil_fcp::FcpClient::new(session, Some(store as Arc<dyn TokenStore>))
                    .sign_out()
                    .await?;
            }
            println!("Signed out.");
            return Ok(());
        }
        None => {}
    }

    let tls_extra_ca_pem = if let Some(path) = cli.tls_ca.as_ref() {
        Some(std::fs::read(path).map_err(|e| anyhow::anyhow!("read --tls-ca: {e}"))?)
    } else {
        None
    };

    let (account, brand) = match (&cli.aor, &cli.registrar, &cli.username, &cli.password) {
        (Some(aor), Some(registrar), Some(username), Some(password)) => (
            AccountConfig {
                aor: aor.clone(),
                registrar: registrar.clone(),
                username: username.clone(),
                password: password.clone(),
                transport: if cli.tls {
                    Transport::Tls
                } else {
                    Transport::Udp
                },
                outbound_proxy: cli.outbound_proxy.clone(),
                stun: cli.stun.clone(),
                register_expires: Duration::from_secs(3600),
                user_agent: format!("Anvil/{}", env!("CARGO_PKG_VERSION")),
                bind_addr: cli.bind.clone(),
                tls_extra_ca_pem,
                provisioning_url: None,
            },
            BrandConfig::default(),
        ),
        (None, None, None, None) => {
            let (mut account, brand) = fcp_account(&cli).await?;
            account.bind_addr = cli.bind.clone();
            account.stun = cli.stun.clone();
            account.tls_extra_ca_pem = tls_extra_ca_pem;
            (account, brand)
        }
        _ => anyhow::bail!(
            "give all of --aor, --registrar, --username and --password, or none (signed in to FCP)"
        ),
    };

    let (audio, tone_stats): (
        Box<dyn anvil_core::audio::AudioHost>,
        Option<Arc<ToneStats>>,
    ) = if cli.tone {
        tracing::info!("using synthetic tone audio host");
        let host = ToneHost::new();
        let stats = host.stats();
        (Box::new(host), Some(stats))
    } else {
        let mut host = CpalHost::new().map_err(|e| anyhow::anyhow!("CpalHost init failed: {e}"))?;
        if cli.agc {
            host = host.with_capture_processor(|| Box::new(anvil_audio::SimpleAgc::new()));
        }
        (Box::new(host), None)
    };

    let cfg = AnvilConfig {
        account,
        media: MediaConfig::default(),
        audio,
        brand,
    };

    run(cli, cfg, tone_stats).await
}

async fn run(cli: Cli, cfg: AnvilConfig, tone_stats: Option<Arc<ToneStats>>) -> Result<()> {
    tracing::info!("starting Anvil");
    let (anvil, mut events) = Anvil::start(cfg).await?;

    tracing::info!("registering...");
    anvil.register().await?;
    // Message waiting: a server that does not take the subscription still
    // lights the lamp with unsolicited NOTIFYs.
    if let Err(e) = anvil.subscribe_mwi().await {
        tracing::info!(%e, "no message-summary subscription");
    }
    for aor in &cli.watch {
        for kind in [
            anvil_core::watch::WatchKind::Presence,
            anvil_core::watch::WatchKind::Dialog,
        ] {
            if let Err(e) = anvil.watch(aor, kind).await {
                tracing::warn!(%e, aor, ?kind, "cannot watch");
            }
        }
    }

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
                        if let Some(target) = &cli.transfer_to {
                            tokio::time::sleep(Duration::from_secs(cli.transfer_after)).await;
                            match anvil.transfer(call, target).await {
                                Ok(()) => println!("[transfer] to {target}: accepted"),
                                Err(e) => tracing::warn!(%e, "transfer failed"),
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
                    Event::BrandUpdated { profile } => {
                        println!(
                            "[brand] {} (primary {}, logo {} bytes, {} ringtone(s))",
                            profile.app_name,
                            profile.colors.primary.map(|c| c.to_hex()).unwrap_or_else(|| "-".into()),
                            profile.logo.as_ref().map_or(0, |l| l.bytes.len()),
                            profile.ringtones.len(),
                        );
                    }
                    Event::BrandCleared => println!("[brand] none: the default theme"),
                    Event::PresenceChanged { aor, presence } => println!(
                        "[presence] {aor}: {}{}",
                        if presence.open { "available" } else { "unavailable" },
                        presence
                            .activity
                            .as_ref()
                            .or(presence.note.as_ref())
                            .map(|w| format!(" ({w})"))
                            .unwrap_or_default()
                    ),
                    Event::LineStateChanged { aor, state } => {
                        println!("[lamp] {aor}: {state:?}")
                    }
                    Event::TransferProgress { call, code, reason } => {
                        println!("[transfer] call={call:?} {code} {reason}")
                    }
                    Event::MessageWaiting { summary } => println!(
                        "[mwi] {} new, {} old{}",
                        summary.new,
                        summary.old,
                        if summary.waiting { " (waiting)" } else { "" }
                    ),
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
