//! anvil-cli — terminal softphone.
//!
//! Signed in to FCP (`anvil-cli login https://pbx.example.com`), it runs
//! with what FCP tells it; or give an account by hand with `--aor`,
//! `--registrar`, `--username` and `--password`.
//!
//! It registers, places a call when `--call` is given (with the microphone
//! and speaker, or a test tone with `--tone`), answers and prints every
//! event, and hangs up on Ctrl+C. The subcommands read and change the
//! user's data on FCP: `calls`, `directory`, `voicemail`, `settings`,
//! `events`.

use std::time::Duration;

use std::sync::Arc;

use anvil_audio::{CpalHost, ToneHost, ToneStats};
use anvil_core::{
    AccountConfig, Anvil, AnvilConfig, BrandConfig, CallId, Event, MediaConfig, Transport,
};
use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "anvil-cli",
    version,
    about = "Anvil, the softphone for FCP: a terminal client"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// The running phone's control socket (a path, or `\\.\pipe\name` on
    /// Windows); unset, this user's default.
    #[arg(long, env = "ANVIL_CONTROL", global = true)]
    control: Option<String>,

    /// Where the FCP session is kept (default: the user's config
    /// directory); the phone's own settings are kept beside it.
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

    /// Encrypt media (SDES-SRTP). Signed in to FCP the default is what FCP
    /// says (optional over TLS); otherwise off.
    #[arg(long, value_enum)]
    srtp: Option<SrtpArg>,

    /// Run captured mic frames through anvil-audio's SimpleAgc before
    /// encoding. Off by default; turn on for noisy / quiet mics.
    #[arg(long)]
    agc: bool,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
enum SrtpArg {
    Off,
    Optional,
    Required,
}

impl From<SrtpArg> for anvil_core::config::SrtpMode {
    fn from(a: SrtpArg) -> Self {
        match a {
            SrtpArg::Off => Self::Off,
            SrtpArg::Optional => Self::Optional,
            SrtpArg::Required => Self::Required,
        }
    }
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
    /// Your call history.
    Calls {
        /// Only the calls you missed.
        #[arg(long)]
        missed: bool,
        #[arg(long, default_value = "20")]
        limit: u32,
    },
    /// The directory, searched by name, extension or email.
    Directory { search: Option<String> },
    /// Your voicemail: the counts and the messages.
    Voicemail,
    /// Your calling settings; change do not disturb or call waiting.
    Settings {
        #[arg(long)]
        dnd: Option<bool>,
        #[arg(long)]
        call_waiting: Option<bool>,
    },
    /// Print your live events until Ctrl+C.
    Events,
    /// Run the phone: registered, ringing, and taking commands at a prompt
    /// and on its control socket, until `quit` or Ctrl+C.
    Run,
    /// Tell the running phone to call a number, an extension or an address.
    Call { target: String },
    /// Answer the ringing call.
    Answer {
        #[arg(long)]
        id: Option<u64>,
    },
    /// Decline the ringing call.
    Decline {
        #[arg(long)]
        id: Option<u64>,
    },
    /// Hang up.
    Hangup {
        #[arg(long)]
        id: Option<u64>,
    },
    /// Put the call on hold.
    Hold {
        #[arg(long)]
        id: Option<u64>,
    },
    /// Take the call off hold.
    Resume {
        #[arg(long)]
        id: Option<u64>,
    },
    /// Mute the microphone.
    Mute {
        #[arg(long)]
        id: Option<u64>,
    },
    /// Unmute the microphone.
    Unmute {
        #[arg(long)]
        id: Option<u64>,
    },
    /// Send DTMF digits.
    Dtmf {
        digits: String,
        #[arg(long)]
        id: Option<u64>,
    },
    /// Transfer the call to a number or address.
    Transfer {
        target: String,
        #[arg(long)]
        id: Option<u64>,
    },
    /// Join one call's party to another's (attended transfer).
    Attended { call: u64, to: u64 },
    /// Do not disturb on or off, on the running phone.
    Dnd {
        #[arg(value_parser = ["on", "off"])]
        state: String,
    },
    /// Park the call with FCP's park code.
    Park {
        #[arg(long)]
        id: Option<u64>,
    },
    /// Forward calls: always, when busy, unanswered or unreachable; `off`
    /// clears it.
    Forward {
        #[arg(value_parser = ["all", "busy", "no-answer", "unreachable"])]
        when: String,
        /// A number or address, or `off`.
        target: String,
    },
    /// Call waiting on or off.
    Waiting {
        #[arg(value_parser = ["on", "off"])]
        state: String,
    },
    /// Mark a voicemail message heard.
    Heard { id: String },
    /// Delete a voicemail message.
    Delete { id: String },
    /// Play a voicemail message on the running phone (and mark it heard).
    Play { id: String },
    /// Stop the message playing.
    Stop,
    /// Make someone in the directory a favourite (`--off` to stop).
    Favourite {
        /// Their name, login or extension.
        who: Vec<String>,
        #[arg(long)]
        off: bool,
    },
    /// Use this microphone (`in`) or speaker (`out`); `default` for the
    /// system's.
    Audio {
        #[arg(value_parser = ["in", "out"])]
        kind: String,
        device: Vec<String>,
    },
    /// Read the user's data from FCP again.
    Refresh,
    /// The running phone: registration, calls, message waiting.
    Status,
    /// Print the running phone's changes until Ctrl+C.
    Watch,
}

impl Command {
    /// The phone command this subcommand is, when it is one.
    fn phone_command(&self) -> Option<anvil_app::Command> {
        use anvil_app::Command as P;
        Some(match self {
            Command::Call { target } => P::Call {
                target: target.clone(),
            },
            Command::Answer { id } => P::Answer { call: *id },
            Command::Decline { id } => P::Decline { call: *id },
            Command::Hangup { id } => P::Hangup { call: *id },
            Command::Hold { id } => P::Hold { call: *id },
            Command::Resume { id } => P::Resume { call: *id },
            Command::Mute { id } => P::Mute { call: *id },
            Command::Unmute { id } => P::Unmute { call: *id },
            Command::Dtmf { digits, id } => P::Dtmf {
                digits: digits.clone(),
                call: *id,
            },
            Command::Transfer { target, id } => P::Transfer {
                target: target.clone(),
                call: *id,
            },
            Command::Attended { call, to } => P::TransferAttended {
                call: *call,
                to: *to,
            },
            Command::Dnd { state } => P::Dnd { on: state == "on" },
            Command::Park { id } => P::Park { call: *id },
            Command::Forward { when, target } => {
                return anvil_app::Command::parse_line(&format!("forward {when} {target}")).ok()
            }
            Command::Waiting { state } => {
                return anvil_app::Command::parse_line(&format!("waiting {state}")).ok()
            }
            Command::Heard { id } => P::Heard { id: id.clone() },
            Command::Delete { id } => P::DeleteVoicemail { id: id.clone() },
            Command::Play { id } => P::Play { id: id.clone() },
            Command::Stop => P::Stop,
            Command::Favourite { who, off } => P::Favourite {
                who: who.join(" "),
                on: !off,
            },
            Command::Audio { kind, device } => {
                return anvil_app::Command::parse_line(&format!(
                    "audio {kind} {}",
                    device.join(" ")
                ))
                .ok()
            }
            Command::Refresh => P::Refresh,
            Command::Status => P::Status,
            _ => return None,
        })
    }
}

/// The running phone's control socket.
fn control_endpoint(cli: &Cli) -> anvil_app::control::Endpoint {
    cli.control
        .as_deref()
        .map(anvil_app::control::Endpoint::named)
        .unwrap_or_else(anvil_app::control::Endpoint::default_for_user)
}

/// One command to the running phone; its answer printed.
async fn tell_phone(cli: &Cli, command: anvil_app::Command) -> Result<()> {
    let mut client = anvil_app::control::Client::connect(&control_endpoint(cli)).await?;
    let answer = client.request(&command).await?;
    if !answer.ok {
        anyhow::bail!("{}", answer.error.unwrap_or_else(|| "refused".into()));
    }
    if let Some(state) = answer.state {
        print_state(&state);
    } else if let Some(call) = answer.call {
        println!("calling (call {call})");
    } else {
        println!("ok");
    }
    Ok(())
}

/// The running phone's changes, until it goes or Ctrl+C.
async fn watch_phone(cli: &Cli) -> Result<()> {
    let mut client = anvil_app::control::Client::connect(&control_endpoint(cli)).await?;
    client.subscribe().await?;
    loop {
        tokio::select! {
            change = client.next_change() => match change? {
                Some(change) => print_change(&change),
                None => break,
            },
            _ = tokio::signal::ctrl_c() => break,
        }
    }
    Ok(())
}

fn print_state(state: &anvil_app::State) {
    println!("{}  {:?}", state.aor, state.registration);
    if let Some(dnd) = state.dnd {
        println!("do not disturb: {}", if dnd { "on" } else { "off" });
    }
    if let Some(mw) = state.message_waiting {
        println!("voicemail: {} new, {} old", mw.new, mw.old);
    }
    if let Some(calling) = &state.calling {
        let forwards: Vec<String> = [
            ("all", &calling.forward_all),
            ("busy", &calling.forward_busy),
            ("no answer", &calling.forward_no_answer),
            ("unreachable", &calling.forward_unreachable),
        ]
        .into_iter()
        .filter_map(|(when, to)| to.as_ref().map(|to| format!("{when} to {to}")))
        .collect();
        println!(
            "call waiting: {}; forward: {}",
            if calling.call_waiting { "on" } else { "off" },
            if forwards.is_empty() {
                "none".to_string()
            } else {
                forwards.join(", ")
            }
        );
    }
    if !state.recents.is_empty() {
        let missed = state.recents.iter().filter(|r| r.missed).count();
        println!("recent calls: {}, {missed} missed", state.recents.len());
    }
    let favourites: Vec<String> = state
        .people
        .iter()
        .filter(|p| p.favourite)
        .map(|p| {
            let mut about: Vec<&str> = p.presence.iter().map(String::as_str).collect();
            if p.on_call {
                about.push("on a call");
            }
            if about.is_empty() {
                p.name.clone()
            } else {
                format!("{} ({})", p.name, about.join(", "))
            }
        })
        .collect();
    if !favourites.is_empty() {
        println!("favourites: {}", favourites.join(", "));
    }
    if !state.audio.inputs.is_empty() || !state.audio.outputs.is_empty() {
        println!(
            "audio: in {}, out {}",
            state.audio.input.as_deref().unwrap_or("default"),
            state.audio.output.as_deref().unwrap_or("default"),
        );
    }
    if state.calls.is_empty() {
        println!("no calls");
    }
    for c in &state.calls {
        println!("{}", call_line(c));
    }
}

fn call_line(c: &anvil_app::CallView) -> String {
    let mut flags = Vec::new();
    if c.held {
        flags.push("held");
    }
    if c.muted {
        flags.push("muted");
    }
    if c.encrypted {
        flags.push("encrypted");
    }
    format!(
        "call {}  {:?} {:?}  {}{}{}",
        c.id,
        c.direction,
        c.state,
        c.display_name.as_deref().unwrap_or(&c.remote),
        c.codec
            .as_deref()
            .map(|k| format!("  [{k}]"))
            .unwrap_or_default(),
        if flags.is_empty() {
            String::new()
        } else {
            format!("  ({})", flags.join(", "))
        }
    )
}

fn print_change(change: &anvil_app::Change) {
    use anvil_app::Change as C;
    match change {
        C::Registration { state, reason } => println!(
            "[registration] {state:?}{}",
            reason
                .as_deref()
                .map(|r| format!(" ({r})"))
                .unwrap_or_default()
        ),
        C::Call { call } => println!("[call] {}", call_line(call)),
        C::CallEnded { id, reason } => println!("[call] {id} ended: {reason}"),
        C::MessageWaiting { message_waiting: m } => {
            println!("[voicemail] {} new, {} old", m.new, m.old)
        }
        C::Dnd { on } => println!("[dnd] {}", if *on { "on" } else { "off" }),
        C::Brand { app_name } => {
            println!("[brand] {}", app_name.as_deref().unwrap_or("none"))
        }
        C::TransferProgress { id, code, reason } => {
            println!("[transfer] call {id}: {code} {reason}")
        }
        C::Calling { settings } => {
            let forward = |f: &Option<String>| f.as_deref().unwrap_or("-").to_string();
            println!(
                "[calling] dnd {}, waiting {}, forward all {}, busy {}, no answer {}",
                if settings.dnd { "on" } else { "off" },
                if settings.call_waiting { "on" } else { "off" },
                forward(&settings.forward_all),
                forward(&settings.forward_busy),
                forward(&settings.forward_no_answer),
            )
        }
        C::Recents { missed } => println!("[recents] {missed} missed"),
        C::People { count } => println!("[people] {count}"),
        C::Person { person } => println!(
            "[person] {} {}{}{}",
            person.name,
            person.presence.as_deref().unwrap_or("-"),
            if person.on_call { ", on a call" } else { "" },
            if person.favourite { ", favourite" } else { "" },
        ),
        C::Voicemail { new, total } => println!("[voicemail] {new} new of {total}"),
        C::Audio { audio } => println!(
            "[audio] in {}, out {}",
            audio.input.as_deref().unwrap_or("default"),
            audio.output.as_deref().unwrap_or("default"),
        ),
        C::Playing { id } => match id {
            Some(id) => println!("[voicemail] playing {id}"),
            None => println!("[voicemail] stopped"),
        },
    }
}

/// `anvil-cli run`: the phone, its control socket and a prompt.
async fn run_phone(cli: &Cli) -> Result<()> {
    use tokio::io::AsyncBufReadExt;
    let (anvil, _tone, fcp) = phone_config(cli).await?;
    let phone = anvil_app::Phone::start(anvil_app::PhoneConfig {
        anvil,
        fcp,
        register: true,
        settings_path: Some(settings_path(cli)),
    })
    .await?;
    let endpoint = control_endpoint(cli);
    let server = tokio::spawn(anvil_app::control::serve(phone.clone(), endpoint.clone()));
    let mut changes = phone.changes();
    let printer = tokio::spawn(async move {
        while let Ok(change) = changes.recv().await {
            print_change(&change);
        }
    });
    println!(
        "Anvil is running as {} — commands here or `anvil-cli <command>` (control: {endpoint}).",
        phone.state().aor
    );
    println!("call <number>, answer, decline, hangup, hold, resume, mute, unmute, dtmf <digits>, transfer <number>, attended <call> <to>, dnd on|off, park, play <message>, stop, status, quit");
    let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    loop {
        tokio::select! {
            line = lines.next_line() => {
                let Some(line) = line? else { break };
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if matches!(line, "quit" | "exit") {
                    break;
                }
                match anvil_app::Command::parse_line(line) {
                    Ok(command) => match phone.execute(command).await {
                        Ok(answer) => match (answer.state, answer.call) {
                            (Some(state), _) => print_state(&state),
                            (None, Some(call)) => println!("calling (call {call})"),
                            _ => println!("ok"),
                        },
                        Err(e) => println!("! {e}"),
                    },
                    Err(e) => println!("! {e}"),
                }
            }
            _ = tokio::signal::ctrl_c() => break,
        }
        if server.is_finished() {
            break;
        }
    }
    printer.abort();
    if server.is_finished() {
        if let Ok(Err(e)) = server.await {
            eprintln!("control socket: {e}");
        }
    } else {
        server.abort();
    }
    phone.shutdown().await;
    println!("Stopped.");
    Ok(())
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

/// Where this device's phone settings live: beside the session.
fn settings_path(cli: &Cli) -> std::path::PathBuf {
    session_path(cli).with_file_name("settings.json")
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

/// A client for the kept FCP session.
fn fcp_client(cli: &Cli) -> Result<Arc<anvil_fcp::FcpClient>> {
    use anvil_fcp::TokenStore;
    let store = Arc::new(anvil_fcp::FileTokenStore::new(session_path(cli)));
    let session = store
        .load()?
        .ok_or_else(|| anyhow::anyhow!("not signed in: run `anvil-cli login <server>`"))?;
    Ok(Arc::new(anvil_fcp::FcpClient::new(
        session,
        Some(store as Arc<dyn TokenStore>),
    )))
}

/// The commands that read or change the user's own data.
async fn user_data(cli: &Cli, command: &Command) -> Result<()> {
    let client = fcp_client(cli)?;
    match command {
        Command::Calls { missed, limit } => {
            let page = client
                .calls(&anvil_fcp::CallQuery {
                    limit: Some(*limit),
                    missed: *missed,
                    ..Default::default()
                })
                .await?;
            for c in page.data {
                println!(
                    "{}  {:8} {:30} {:10} {}s{}",
                    c.start_time,
                    c.direction,
                    c.other_party_name.as_deref().unwrap_or(&c.other_party),
                    c.disposition,
                    c.duration_seconds.unwrap_or(0),
                    if c.missed { "  (missed)" } else { "" }
                );
            }
        }
        Command::Directory { search } => {
            let page = client.directory(search.as_deref(), None, None).await?;
            for e in page.data {
                println!(
                    "{:6} {:30} {:8} {}",
                    e.extension.as_deref().unwrap_or("-"),
                    e.display_name
                        .as_deref()
                        .or(e.username.as_deref())
                        .unwrap_or("-"),
                    e.presence.as_deref().unwrap_or(""),
                    e.presence_note.as_deref().unwrap_or("")
                );
            }
        }
        Command::Voicemail => {
            let stats = match client.voicemail_stats().await {
                Err(anvil_fcp::FcpError::Refused { status: 404, .. }) => {
                    println!("You have no voicemail box.");
                    return Ok(());
                }
                other => other?,
            };
            println!(
                "{} new, {} heard, {} saved",
                stats.new_messages, stats.heard_messages, stats.saved_messages
            );
            for m in client.voicemail(None, None).await?.data {
                println!(
                    "{}  {:6} {:30} {}s  {}",
                    m.created_at,
                    m.status,
                    m.caller_name.as_deref().unwrap_or(&m.caller),
                    m.duration,
                    m.transcription.as_deref().unwrap_or("")
                );
            }
        }
        Command::Settings { dnd, call_waiting } => {
            let settings = if dnd.is_some() || call_waiting.is_some() {
                client
                    .set_calling(&anvil_fcp::CallingUpdate {
                        dnd: *dnd,
                        call_waiting: *call_waiting,
                        ..Default::default()
                    })
                    .await?
            } else {
                client.calling().await?
            };
            println!("{settings:#?}");
        }
        Command::Events => {
            let mut events = client.events().await?;
            println!("Listening; Ctrl+C to stop.");
            loop {
                tokio::select! {
                    event = events.next() => match event {
                        Some(e) => println!("[{}] {}", e.name, e.body),
                        None => break,
                    },
                    _ = tokio::signal::ctrl_c() => break,
                }
            }
            events.close().await;
        }
        _ => unreachable!("handled in main"),
    }
    Ok(())
}

/// The account from the FCP session, refreshed and kept, and the tenant's
/// brand fetched with it.
async fn fcp_account(
    cli: &Cli,
) -> Result<(
    AccountConfig,
    BrandConfig,
    MediaConfig,
    Arc<anvil_fcp::FcpClient>,
)> {
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
    Ok((account, brand, config.media_config(), client))
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
        Some(Command::Run) => return run_phone(&cli).await,
        Some(Command::Watch) => return watch_phone(&cli).await,
        Some(command) if command.phone_command().is_some() => {
            let phone_command = command.phone_command().expect("checked");
            return tell_phone(&cli, phone_command).await;
        }
        Some(command) => return user_data(&cli, command).await,
        None => {}
    }

    let (cfg, tone_stats, _) = phone_config(&cli).await?;
    run(cli, cfg, tone_stats).await
}

/// The phone's configuration from the command line: an account by hand,
/// or FCP's (and the FCP client with it); the audio host; the media.
async fn phone_config(
    cli: &Cli,
) -> Result<(
    AnvilConfig,
    Option<Arc<ToneStats>>,
    Option<Arc<anvil_fcp::FcpClient>>,
)> {
    let tls_extra_ca_pem = if let Some(path) = cli.tls_ca.as_ref() {
        Some(std::fs::read(path).map_err(|e| anyhow::anyhow!("read --tls-ca: {e}"))?)
    } else {
        None
    };

    let (account, brand, mut media, fcp) =
        match (&cli.aor, &cli.registrar, &cli.username, &cli.password) {
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
                MediaConfig::default(),
                None,
            ),
            (None, None, None, None) => {
                let (mut account, brand, media, client) = fcp_account(cli).await?;
                account.bind_addr = cli.bind.clone();
                account.stun = cli.stun.clone();
                account.tls_extra_ca_pem = tls_extra_ca_pem;
                (account, brand, media, Some(client))
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

    if let Some(srtp) = cli.srtp {
        media.srtp = srtp.into();
    }
    let cfg = AnvilConfig {
        account,
        media,
        audio,
        brand,
    };

    Ok((cfg, tone_stats, fcp))
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
