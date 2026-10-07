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
}

/// What changed: the screen redraws from [`phone_state`] and may say why.
pub struct PhoneChange {
    /// `registration`, `call`, `call_ended`, `message_waiting`, `dnd`,
    /// `transfer_progress`.
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
                direction: match c.direction {
                    anvil_app::Direction::Incoming => Direction::Incoming,
                    anvil_app::Direction::Outgoing => Direction::Outgoing,
                },
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
            })
            .collect(),
        voicemail_new: s.message_waiting.map(|m| m.new).unwrap_or(0),
        dnd: s.dnd,
        brand_name: s.brand.as_ref().map(|b| b.app_name.clone()),
        brand_primary: s.brand.as_ref().and_then(|b| b.primary.clone()),
        brand_logo: s.brand.and_then(|b| b.logo),
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
            };
            if sink.add(out).is_err() {
                return;
            }
        }
    });
    Ok(())
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

#[frb(init)]
pub fn init_app() {
    flutter_rust_bridge::setup_default_user_utils();
}
