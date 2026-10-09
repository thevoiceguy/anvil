//! What the app calls (`docs/APP.md` §3): signing in, the running phone,
//! its commands and its changes. flutter_rust_bridge turns this module into
//! Dart (`lib/src/rust`); everything here is a thin layer over `anvil-app`,
//! so the app and `anvil-cli` drive the same phone the same way.

use std::sync::{Arc, OnceLock};

use anvil_app::{control, Command, Phone, PhoneConfig};
use anvil_fcp::{FcpClient, FileTokenStore, TokenStore};
use flutter_rust_bridge::frb;
use parking_lot::Mutex;

use crate::frb_generated::StreamSink;

/// The phone, once started. One per app.
static PHONE: Mutex<Option<Phone>> = Mutex::new(None);

/// The runtime the phone runs on: its tasks outlive any one call from Dart.
fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("anvil-app")
            .build()
            .expect("a tokio runtime")
    })
}

/// [`on_runtime`] for the other API modules.
#[frb(ignore)]
pub(crate) async fn on_runtime_pub<T: Send + 'static>(
    fut: impl std::future::Future<Output = anyhow::Result<T>> + Send + 'static,
) -> anyhow::Result<T> {
    on_runtime(fut).await
}

/// Run `fut` on the phone's runtime and wait for it.
async fn on_runtime<T: Send + 'static>(
    fut: impl std::future::Future<Output = anyhow::Result<T>> + Send + 'static,
) -> anyhow::Result<T> {
    runtime().spawn(fut).await?
}

fn phone() -> anyhow::Result<Phone> {
    PHONE
        .lock()
        .clone()
        .ok_or_else(|| anyhow::anyhow!("the phone is not running"))
}

/// Who is signed in.
pub struct Account {
    pub username: String,
    /// The FCP server, e.g. `https://pbx.example.com`.
    pub server: String,
}

/// The session kept at `session_path`, if there is one.
pub async fn saved_account(session_path: String) -> anyhow::Result<Option<Account>> {
    on_runtime(async move {
        Ok(FileTokenStore::new(session_path).load()?.map(|s| Account {
            username: s.username,
            server: s.server.public_url,
        }))
    })
    .await
}

/// Sign in with a username and password (`place` is the server or an email
/// address), keeping the session at `session_path`.
pub async fn sign_in(
    place: String,
    username: String,
    password: String,
    totp: Option<String>,
    session_path: String,
) -> anyhow::Result<Account> {
    on_runtime(async move {
        let store = FileTokenStore::new(session_path);
        let install = store
            .load()
            .ok()
            .flatten()
            .map(|s| s.client.install_id)
            .unwrap_or_else(anvil_fcp::AppClient::new_install_id);
        let server = anvil_fcp::discover(&place).await?;
        let session = anvil_fcp::password_sign_in(
            server,
            anvil_fcp::AppClient::this_machine(install),
            &username,
            &password,
            totp.as_deref(),
        )
        .await?;
        store.save(&session)?;
        Ok(Account {
            username: session.username,
            server: session.server.public_url,
        })
    })
    .await
}

/// Start the phone from the session at `session_path`: registered, ringing,
/// and serving the control socket so `anvil-cli` can drive the app.
pub async fn start_phone(session_path: String) -> anyhow::Result<()> {
    if PHONE.lock().is_some() {
        return Ok(());
    }
    let settings_path = std::path::Path::new(&session_path).with_file_name("settings.json");
    let phone = on_runtime(async move {
        let store = Arc::new(FileTokenStore::new(session_path));
        let session = store
            .load()?
            .ok_or_else(|| anyhow::anyhow!("not signed in"))?;
        let client = Arc::new(FcpClient::new(session, Some(store as Arc<dyn TokenStore>)));
        let config = client.softphone().await?;
        let account = client.account_config(&config).await?;
        let brand = anvil_fcp::brand_config(Arc::clone(&client), None)?;
        let audio =
            anvil_audio::CpalHost::new().map_err(|e| anyhow::anyhow!("no audio devices: {e}"))?;
        let phone = Phone::start(PhoneConfig {
            anvil: anvil_core::AnvilConfig {
                account,
                media: config.media_config(),
                audio: Box::new(audio),
                brand,
            },
            fcp: Some(client),
            register: true,
            settings_path: Some(settings_path),
        })
        .await?;
        let served = phone.clone();
        tokio::spawn(async move {
            if let Err(e) = control::serve(served, control::Endpoint::default_for_user()).await {
                tracing::warn!(%e, "the control socket is not served");
            }
        });
        Ok(phone)
    })
    .await?;
    *PHONE.lock() = Some(phone);
    Ok(())
}

/// Stop the phone; the session stays signed in.
pub async fn stop_phone() -> anyhow::Result<()> {
    let phone = PHONE.lock().take();
    if let Some(phone) = phone {
        on_runtime(async move {
            phone.shutdown().await;
            Ok(())
        })
        .await?;
    }
    Ok(())
}

/// Sign out: the phone stops and the app's session ends at FCP.
pub async fn sign_out(session_path: String) -> anyhow::Result<()> {
    stop_phone().await?;
    on_runtime(async move {
        let store = Arc::new(FileTokenStore::new(session_path));
        if let Some(session) = store.load()? {
            FcpClient::new(session, Some(store as Arc<dyn TokenStore>))
                .sign_out()
                .await?;
        }
        Ok(())
    })
    .await
}

/// Whether the account is registered.
pub enum Registration {
    Unregistered,
    Registering,
    Registered,
    Failed,
}

/// Which way a call goes.
pub enum Direction {
    Incoming,
    Outgoing,
}

/// Where a call is.
pub enum CallState {
    Dialing,
    Ringing,
    Connected,
}

/// One call, as a screen shows it.
pub struct Call {
    pub id: u64,
    pub direction: Direction,
    pub remote: String,
    pub display_name: Option<String>,
    pub state: CallState,
    pub held: bool,
    pub muted: bool,
    pub codec: Option<String>,
    /// The media is encrypted (SRTP).
    pub encrypted: bool,
    /// How the media is doing, once measured.
    pub quality: Option<Quality>,
    /// When it connected, in Unix seconds.
    pub connected_at: Option<u64>,
}

/// A connected call's media, as last measured.
pub struct Quality {
    pub jitter_ms: u32,
    pub packet_loss_permille: u32,
    pub rtt_ms: Option<u32>,
}

/// One call in the user's history.
pub struct Recent {
    pub id: String,
    pub direction: Direction,
    pub remote: String,
    pub display_name: Option<String>,
    pub missed: bool,
    /// RFC 3339.
    pub started_at: String,
    pub duration_secs: Option<u64>,
}

/// Someone in the tenant's directory.
pub struct Person {
    pub key: String,
    pub name: String,
    pub extension: Option<String>,
    pub department: Option<String>,
    pub job_title: Option<String>,
    /// `available`, `busy`, `away`, `dnd`, `offline`, … when shown.
    pub presence: Option<String>,
    pub on_call: bool,
    pub favourite: bool,
    /// What to dial.
    pub dial: String,
}

/// One voicemail message.
pub struct Voicemail {
    pub id: String,
    pub caller: String,
    pub caller_name: Option<String>,
    pub new: bool,
    pub urgent: bool,
    pub duration_secs: u64,
    pub transcription: Option<String>,
    /// RFC 3339.
    pub received_at: String,
}

/// A microphone or a speaker.
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    pub default: bool,
}

/// The user's calling settings, as FCP has them.
pub struct CallingSettings {
    pub dnd: bool,
    pub call_waiting: bool,
    pub forward_all: Option<String>,
    pub forward_busy: Option<String>,
    pub forward_no_answer: Option<String>,
    pub forward_unreachable: Option<String>,
    pub no_answer_secs: Option<u32>,
}

/// A change to the calling settings: each field left out is kept, and an
/// empty forward clears it.
pub struct CallingChange {
    pub call_waiting: Option<bool>,
    pub forward_all: Option<String>,
    pub forward_busy: Option<String>,
    pub forward_no_answer: Option<String>,
    pub forward_unreachable: Option<String>,
    pub no_answer_secs: Option<u32>,
}

/// The phone as a screen shows it.
pub struct PhoneState {
    pub aor: String,
    pub registration: Registration,
    pub calls: Vec<Call>,
    pub voicemail_new: u32,
    pub dnd: Option<bool>,
    /// The tenant's brand: its app name, colours (`#RRGGBB`) and logo.
    pub brand_name: Option<String>,
    pub brand_primary: Option<String>,
    pub brand_logo: Option<Vec<u8>>,
    /// Signed in to FCP: the calling settings, once read.
    pub calling: Option<CallingSettings>,
    /// The latest calls, newest first.
    pub recents: Vec<Recent>,
    pub people: Vec<Person>,
    /// The mailbox's messages, newest first.
    pub voicemail: Vec<Voicemail>,
    pub inputs: Vec<AudioDevice>,
    pub outputs: Vec<AudioDevice>,
    /// The devices chosen; `None` is the system's default.
    pub input: Option<String>,
    pub output: Option<String>,
    /// The voicemail message playing.
    pub playing: Option<String>,
}

fn direction(d: anvil_app::Direction) -> Direction {
    match d {
        anvil_app::Direction::Incoming => Direction::Incoming,
        anvil_app::Direction::Outgoing => Direction::Outgoing,
    }
}

fn devices(list: Vec<anvil_app::AudioDevice>) -> Vec<AudioDevice> {
    list.into_iter()
        .map(|d| AudioDevice {
            id: d.id,
            name: d.name,
            default: d.default,
        })
        .collect()
}

/// What changed: the screen redraws from [`phone_state`] and may say why.
pub struct PhoneChange {
    /// `registration`, `call`, `call_ended`, `message_waiting`, `dnd`,
    /// `brand`, `transfer_progress`, `calling`, `recents`, `people`,
    /// `person`, `voicemail`, `audio`, `playing`.
    pub kind: String,
    /// The call it is about, if any.
    pub call: Option<u64>,
    /// Why a call ended, or a registration failed.
    pub reason: Option<String>,
}

/// The phone as it is now.
#[frb(sync)]
pub fn phone_state() -> anyhow::Result<PhoneState> {
    let s = phone()?.state();
    Ok(PhoneState {
        aor: s.aor,
        registration: match s.registration {
            anvil_app::Registration::Unregistered => Registration::Unregistered,
            anvil_app::Registration::Registering => Registration::Registering,
            anvil_app::Registration::Registered => Registration::Registered,
            anvil_app::Registration::Failed => Registration::Failed,
        },
        calls: s
            .calls
            .into_iter()
            .map(|c| Call {
                id: c.id,
                direction: direction(c.direction),
                remote: c.remote,
                display_name: c.display_name,
                state: match c.state {
                    anvil_app::CallState::Dialing => CallState::Dialing,
                    anvil_app::CallState::Ringing => CallState::Ringing,
                    anvil_app::CallState::Connected => CallState::Connected,
                },
                held: c.held,
                muted: c.muted,
                codec: c.codec,
                encrypted: c.encrypted,
                quality: c.quality.map(|q| Quality {
                    jitter_ms: q.jitter_ms,
                    packet_loss_permille: q.packet_loss_permille,
                    rtt_ms: q.rtt_ms,
                }),
                connected_at: c.connected_at,
            })
            .collect(),
        voicemail_new: s.message_waiting.map(|m| m.new).unwrap_or(0),
        dnd: s.dnd,
        brand_name: s.brand.as_ref().map(|b| b.app_name.clone()),
        brand_primary: s.brand.as_ref().and_then(|b| b.primary.clone()),
        brand_logo: s.brand.and_then(|b| b.logo),
        calling: s.calling.map(|c| CallingSettings {
            dnd: c.dnd,
            call_waiting: c.call_waiting,
            forward_all: c.forward_all,
            forward_busy: c.forward_busy,
            forward_no_answer: c.forward_no_answer,
            forward_unreachable: c.forward_unreachable,
            no_answer_secs: c.no_answer_secs,
        }),
        recents: s
            .recents
            .into_iter()
            .map(|r| Recent {
                id: r.id,
                direction: direction(r.direction),
                remote: r.remote,
                display_name: r.display_name,
                missed: r.missed,
                started_at: r.started_at,
                duration_secs: r.duration_secs,
            })
            .collect(),
        people: s
            .people
            .into_iter()
            .map(|p| Person {
                dial: p.dial().to_string(),
                key: p.key,
                name: p.name,
                extension: p.extension,
                department: p.department,
                job_title: p.job_title,
                presence: p.presence,
                on_call: p.on_call,
                favourite: p.favourite,
            })
            .collect(),
        voicemail: s
            .voicemail
            .into_iter()
            .map(|m| Voicemail {
                id: m.id,
                caller: m.caller,
                caller_name: m.caller_name,
                new: m.new,
                urgent: m.urgent,
                duration_secs: m.duration_secs,
                transcription: m.transcription,
                received_at: m.received_at,
            })
            .collect(),
        inputs: devices(s.audio.inputs),
        outputs: devices(s.audio.outputs),
        input: s.audio.input,
        output: s.audio.output,
        playing: s.playing,
    })
}

/// Every change from now on, until the phone stops.
pub fn phone_changes(sink: StreamSink<PhoneChange>) -> anyhow::Result<()> {
    let mut changes = phone()?.changes();
    runtime().spawn(async move {
        use anvil_app::Change as C;
        loop {
            let change = match changes.recv().await {
                Ok(c) => c,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => return,
            };
            let out = match change {
                C::Registration { reason, .. } => PhoneChange {
                    kind: "registration".into(),
                    call: None,
                    reason,
                },
                C::Call { call } => PhoneChange {
                    kind: "call".into(),
                    call: Some(call.id),
                    reason: None,
                },
                C::CallEnded { id, reason } => PhoneChange {
                    kind: "call_ended".into(),
                    call: Some(id),
                    reason: Some(reason),
                },
                C::MessageWaiting { .. } => PhoneChange {
                    kind: "message_waiting".into(),
                    call: None,
                    reason: None,
                },
                C::Dnd { .. } => PhoneChange {
                    kind: "dnd".into(),
                    call: None,
                    reason: None,
                },
                C::Brand { .. } => PhoneChange {
                    kind: "brand".into(),
                    call: None,
                    reason: None,
                },
                C::TransferProgress { id, code, reason } => PhoneChange {
                    kind: "transfer_progress".into(),
                    call: Some(id),
                    reason: Some(format!("{code} {reason}")),
                },
                // The screens read what changed from the state.
                C::Calling { .. } => kind_only("calling"),
                C::Recents { .. } => kind_only("recents"),
                C::People { .. } => kind_only("people"),
                C::Person { .. } => kind_only("person"),
                C::Voicemail { .. } => kind_only("voicemail"),
                C::Audio { .. } => kind_only("audio"),
                C::Playing { .. } => kind_only("playing"),
            };
            if sink.add(out).is_err() {
                return;
            }
        }
    });
    Ok(())
}

fn kind_only(kind: &str) -> PhoneChange {
    PhoneChange {
        kind: kind.into(),
        call: None,
        reason: None,
    }
}

/// Carry out a command on the running phone; the call a `call` placed.
async fn execute(command: Command) -> anyhow::Result<Option<u64>> {
    let phone = phone()?;
    on_runtime(async move {
        let answer = phone.execute(command).await?;
        Ok(answer.call)
    })
    .await
}

/// Dial a number, an extension, a name or an address; the new call's id.
pub async fn place_call(target: String) -> anyhow::Result<u64> {
    execute(Command::Call { target })
        .await?
        .ok_or_else(|| anyhow::anyhow!("no call was placed"))
}

pub async fn answer(call: Option<u64>) -> anyhow::Result<()> {
    execute(Command::Answer { call }).await.map(|_| ())
}

pub async fn decline(call: Option<u64>) -> anyhow::Result<()> {
    execute(Command::Decline { call }).await.map(|_| ())
}

pub async fn hangup(call: Option<u64>) -> anyhow::Result<()> {
    execute(Command::Hangup { call }).await.map(|_| ())
}

pub async fn hold(call: Option<u64>, on: bool) -> anyhow::Result<()> {
    let command = if on {
        Command::Hold { call }
    } else {
        Command::Resume { call }
    };
    execute(command).await.map(|_| ())
}

pub async fn mute(call: Option<u64>, on: bool) -> anyhow::Result<()> {
    let command = if on {
        Command::Mute { call }
    } else {
        Command::Unmute { call }
    };
    execute(command).await.map(|_| ())
}

pub async fn send_digits(call: Option<u64>, digits: String) -> anyhow::Result<()> {
    execute(Command::Dtmf { digits, call }).await.map(|_| ())
}

/// Blind-transfer a call to a number, an extension or an address.
pub async fn transfer(call: Option<u64>, target: String) -> anyhow::Result<()> {
    execute(Command::Transfer { target, call })
        .await
        .map(|_| ())
}

/// Join `call`'s party to `to`'s (attended transfer), ending both of ours.
pub async fn transfer_attended(call: u64, to: u64) -> anyhow::Result<()> {
    execute(Command::TransferAttended { call, to })
        .await
        .map(|_| ())
}

/// Park a call with FCP's park code.
pub async fn park(call: Option<u64>) -> anyhow::Result<()> {
    execute(Command::Park { call }).await.map(|_| ())
}

pub async fn set_dnd(on: bool) -> anyhow::Result<()> {
    execute(Command::Dnd { on }).await.map(|_| ())
}

/// Change the calling settings: forwards, call waiting.
pub async fn set_calling(change: CallingChange) -> anyhow::Result<()> {
    execute(Command::Calling {
        update: anvil_app::CallingUpdate {
            dnd: None,
            call_waiting: change.call_waiting,
            forward_all: change.forward_all,
            forward_busy: change.forward_busy,
            forward_no_answer: change.forward_no_answer,
            forward_unreachable: change.forward_unreachable,
            no_answer_secs: change.no_answer_secs,
        },
    })
    .await
    .map(|_| ())
}

pub async fn mark_heard(id: String) -> anyhow::Result<()> {
    execute(Command::Heard { id }).await.map(|_| ())
}

pub async fn delete_voicemail(id: String) -> anyhow::Result<()> {
    execute(Command::DeleteVoicemail { id }).await.map(|_| ())
}

/// Play a voicemail message through the speaker calls use.
pub async fn play_voicemail(id: String) -> anyhow::Result<()> {
    execute(Command::Play { id }).await.map(|_| ())
}

pub async fn stop_playing() -> anyhow::Result<()> {
    execute(Command::Stop).await.map(|_| ())
}

/// Make someone (by key, extension or name) a favourite, or not.
pub async fn favourite(who: String, on: bool) -> anyhow::Result<()> {
    execute(Command::Favourite { who, on }).await.map(|_| ())
}

/// Use this microphone (`input`) or speaker by id, `None` for the system's
/// default, for the calls set up from now on.
pub async fn choose_audio(input: bool, device: Option<String>) -> anyhow::Result<()> {
    let kind = if input {
        anvil_app::AudioKind::Input
    } else {
        anvil_app::AudioKind::Output
    };
    execute(Command::Audio { kind, device }).await.map(|_| ())
}

/// Read the user's data from FCP again.
pub async fn refresh() -> anyhow::Result<()> {
    execute(Command::Refresh).await.map(|_| ())
}

#[frb(init)]
pub fn init_app() {
    flutter_rust_bridge::setup_default_user_utils();
}
