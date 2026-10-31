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

/// The system's call screen following the phone (Android: `system.rs`).
static FOLLOWER: Mutex<Option<tokio::task::JoinHandle<()>>> = Mutex::new(None);

/// The runtime the phone runs on: its tasks outlive any one call from Dart.
#[frb(ignore)]
pub(crate) fn runtime() -> &'static tokio::runtime::Runtime {
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

/// What this install was set up with (`docs/MANAGED_INSTALLS.md`): policy
/// and the installed profile, read once.
pub(crate) fn managed() -> &'static anvil_app::managed::Managed {
    static MANAGED: std::sync::OnceLock<anvil_app::managed::Managed> = std::sync::OnceLock::new();
    MANAGED.get_or_init(anvil_app::managed::load)
}

/// What the sign-in screen and the app follow on a managed install.
pub struct ManagedSettings {
    /// What the server field holds.
    pub server: Option<String>,
    /// The server field cannot be changed.
    pub lock_server: bool,
    /// Put after a bare username.
    pub login_domain: Option<String>,
    /// `auto`, `sso` or `password`.
    pub sign_in: String,
    /// The username to fill in, from the system.
    pub username: Option<String>,
    /// `auto`, `notify` or `off`.
    pub updates: String,
    /// The installed profile's brand, before FCP's.
    pub brand_name: Option<String>,
    /// `#RRGGBB`.
    pub brand_primary: Option<String>,
    pub brand_logo: Option<Vec<u8>>,
    /// The brand's icon (PNG, JPEG or WebP), for the system's icons.
    pub brand_icon: Option<Vec<u8>>,
    /// The brand's icon file for the system (Windows' notifications).
    pub brand_icon_path: Option<String>,
    /// Where each setting came from, for Settings.
    pub sources: Vec<String>,
}

/// This install's managed settings; an ordinary install's are all open.
#[frb(sync)]
pub fn managed_settings() -> ManagedSettings {
    use anvil_app::managed::{SignInMode, UpdatesMode};
    let m = managed();
    ManagedSettings {
        server: m.server.clone(),
        lock_server: m.lock_server,
        login_domain: m.login_domain.clone(),
        sign_in: match m.sign_in {
            SignInMode::Auto => "auto",
            SignInMode::Sso => "sso",
            SignInMode::Password => "password",
        }
        .into(),
        username: m.username.clone(),
        updates: match m.updates {
            UpdatesMode::Auto => "auto",
            UpdatesMode::Notify => "notify",
            UpdatesMode::Off => "off",
        }
        .into(),
        brand_name: m.brand.as_ref().map(|b| b.app_name.clone()),
        brand_primary: m
            .brand
            .as_ref()
            .and_then(|b| b.colors.primary)
            .map(|c| c.to_hex()),
        brand_logo: m
            .brand
            .as_ref()
            .and_then(|b| b.logo.as_ref())
            .map(|a| a.bytes.clone()),
        brand_icon: m
            .brand
            .as_ref()
            .and_then(|b| b.icon.as_ref())
            .map(|a| a.bytes.clone())
            .filter(|b| !b.is_empty()),
        brand_icon_path: m
            .system_icon
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned()),
        sources: m.sources.clone(),
    }
}

/// The brand's icon as the system shows it, made into files.
pub struct SystemLook {
    /// The window's: an ICO on Windows, a PNG elsewhere.
    pub window: String,
    /// A PNG, for the Dock.
    pub png: String,
    /// The tray's, by state (`offline`, `ready`, `dnd`, `ringing`,
    /// `in_call`).
    pub tray: Vec<TrayIcon>,
}

pub struct TrayIcon {
    pub state: String,
    pub path: String,
}

/// Show the brand `name` and `icon` where the system keeps the app's name
/// and icon outside it (Windows' shortcuts and app id, a Linux menu), or
/// Anvil's with neither (`docs/MANAGED_INSTALLS.md` §2.4). The icon's files
/// go in `dir`; they are returned for the window, the tray and the Dock,
/// or `None` with no icon (or one that is not an image, which is logged).
pub fn system_brand(
    dir: String,
    name: Option<String>,
    icon: Option<Vec<u8>>,
) -> Option<SystemLook> {
    #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
    {
        use anvil_app::system_brand;
        let look = icon.and_then(|icon| {
            system_brand::look(std::path::Path::new(&dir), &icon)
                .map_err(|e| tracing::warn!(%e, "the brand's icon is not shown"))
                .ok()
        });
        system_brand::apply(name.as_deref(), look.as_ref());
        let text = |p: &std::path::Path| p.to_string_lossy().into_owned();
        look.map(|l| SystemLook {
            window: text(&l.window),
            png: text(&l.png),
            tray: l
                .tray
                .iter()
                .map(|(state, path)| TrayIcon {
                    state: state.to_string(),
                    path: text(path),
                })
                .collect(),
        })
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        let _ = (dir, name, icon);
        None
    }
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

/// What the sign-in screen comes back filled in with: the last server (or
/// email address) and username signed in with.
pub struct RememberedSignIn {
    pub place: String,
    pub username: String,
}

/// The last sign-in remembered beside the session at `session_path`; it
/// outlives signing out. Never a password.
pub async fn last_sign_in(session_path: String) -> anyhow::Result<Option<RememberedSignIn>> {
    on_runtime(async move {
        let path = std::path::PathBuf::from(&session_path);
        let remembered =
            anvil_fcp::LastSignIn::load(&anvil_fcp::LastSignIn::path_beside(&path))
                // Signed in by a version that did not remember: the session says.
                .or_else(|| {
                    FileTokenStore::new(&path).load().ok().flatten().map(|s| {
                        anvil_fcp::LastSignIn {
                            place: s.server.public_url,
                            username: s.username,
                        }
                    })
                });
        Ok(remembered.map(|l| RememberedSignIn {
            place: l.place,
            username: l.username,
        }))
    })
    .await
}

/// Remember a sign-in beside the session: what the person typed for the
/// server, and who signed in.
fn remember(session_path: &str, place: &str, username: &str) {
    let path = anvil_fcp::LastSignIn::path_beside(std::path::Path::new(session_path));
    let last = anvil_fcp::LastSignIn {
        place: place.trim().to_string(),
        username: username.trim().to_string(),
    };
    if let Err(e) = last.save(&path) {
        tracing::warn!(%e, "the sign-in was not remembered");
    }
}

/// The server to find from what the person typed: the server field, or the
/// username when that is an email address and the server field is empty.
fn place_of(place: &str, username: &str) -> String {
    if place.trim().is_empty() && username.contains('@') {
        username.trim().to_string()
    } else {
        place.trim().to_string()
    }
}

/// How a person signs in on a server (`POST /auth/discover`).
pub struct SignInMethods {
    /// The server said (an older FCP has no discover route: both ways are
    /// offered).
    pub known: bool,
    pub password: bool,
    pub sso: bool,
}

/// Ask the server how `username` signs in (`place` is the server or an
/// email address).
pub async fn sign_in_methods(place: String, username: String) -> anyhow::Result<SignInMethods> {
    on_runtime(async move {
        let server = anvil_fcp::discover(&place_of(&place, &username)).await?;
        let login = anvil_fcp::discovery_login(&place, &username);
        Ok(match anvil_fcp::sign_in_methods(&server, &login).await? {
            Some(m) => SignInMethods {
                known: true,
                password: m.password,
                sso: m.sso,
            },
            None => SignInMethods {
                known: false,
                password: true,
                sso: true,
            },
        })
    })
    .await
}

/// A password sign-in waiting for its one-time code, and what to remember
/// once it is in.
static SECOND_FACTOR: parking_lot::Mutex<Option<(anvil_fcp::SecondFactor, String)>> =
    parking_lot::Mutex::new(None);

/// Sign in with a username and password (`place` is the server or an email
/// address), keeping the session at `session_path`. `None`: the password
/// was right and FCP wants a one-time code next, which
/// [`sign_in_with_code`] takes.
pub async fn sign_in(
    place: String,
    username: String,
    password: String,
    session_path: String,
) -> anyhow::Result<Option<Account>> {
    on_runtime(async move {
        let store = FileTokenStore::new(&session_path);
        let client = this_install(&store);
        let server = anvil_fcp::discover(&place_of(&place, &username)).await?;
        SECOND_FACTOR.lock().take();
        match anvil_fcp::begin_password_sign_in(server, client, username.trim(), &password).await? {
            anvil_fcp::PasswordStep::SignedIn(session) => {
                store.save(&session)?;
                remember(&session_path, &place, &username);
                Ok(Some(Account {
                    username: session.username,
                    server: session.server.public_url,
                }))
            }
            anvil_fcp::PasswordStep::CodeNeeded(second) => {
                *SECOND_FACTOR.lock() = Some((second, place));
                Ok(None)
            }
        }
    })
    .await
}

/// Finish a password sign-in with the one-time code it was asked for. A
/// wrong code may be tried again.
pub async fn sign_in_with_code(code: String, session_path: String) -> anyhow::Result<Account> {
    on_runtime(async move {
        let (second, place) = SECOND_FACTOR
            .lock()
            .clone()
            .ok_or_else(|| anyhow::anyhow!("sign in with your password first"))?;
        let session = second.finish(&code).await?;
        SECOND_FACTOR.lock().take();
        FileTokenStore::new(&session_path).save(&session)?;
        remember(&session_path, &place, &session.username);
        Ok(Account {
            username: session.username,
            server: session.server.public_url,
        })
    })
    .await
}

/// This install as FCP knows it: the same install signing in again keeps
/// its device.
fn this_install(store: &FileTokenStore) -> anvil_fcp::AppClient {
    let install = anvil_fcp::AppClient::install_id_beside(store.path());
    let mut client = anvil_fcp::AppClient::this_machine(install);
    if let Some(model) = crate::device::phone_model() {
        client.client_name = format!("Anvil on {model}");
    }
    client
}

/// A sign-in through the browser, started and not yet finished, and what
/// the person typed for the server.
static BROWSER_SIGN_IN: parking_lot::Mutex<Option<(anvil_fcp::BrowserSignIn, String)>> =
    parking_lot::Mutex::new(None);
/// Giving up the browser sign-in under way.
static BROWSER_CANCELLED: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// Start signing in through the system browser, the way single sign-on
/// works (`place` is the server or an email address, `username` what the
/// person typed, if anything): the address the app opens.
/// [`finish_browser_sign_in`] then waits for the browser to come back.
pub async fn start_browser_sign_in(
    place: String,
    username: String,
    session_path: String,
) -> anyhow::Result<String> {
    on_runtime(async move {
        let client = this_install(&FileTokenStore::new(session_path));
        let place = place_of(&place, &username);
        let server = anvil_fcp::discover(&place).await?;
        let sign_in = anvil_fcp::BrowserSignIn::start(server, client).await?;
        let url = sign_in.authorize_url().to_string();
        *BROWSER_SIGN_IN.lock() = Some((sign_in, place));
        Ok(url)
    })
    .await
}

/// Wait for the browser sign-in started (up to ten minutes) and keep its
/// session at `session_path`.
pub async fn finish_browser_sign_in(session_path: String) -> anyhow::Result<Account> {
    on_runtime(async move {
        let (sign_in, place) = BROWSER_SIGN_IN
            .lock()
            .take()
            .ok_or_else(|| anyhow::anyhow!("no sign-in through the browser was started"))?;
        let session = tokio::select! {
            session = sign_in.finish(std::time::Duration::from_secs(600)) => session?,
            _ = BROWSER_CANCELLED.notified() => anyhow::bail!("the sign-in was cancelled"),
        };
        FileTokenStore::new(&session_path).save(&session)?;
        remember(&session_path, &place, &session.username);
        Ok(Account {
            username: session.username,
            server: session.server.public_url,
        })
    })
    .await
}

/// Give up the browser sign-in under way.
pub fn cancel_browser_sign_in() {
    BROWSER_SIGN_IN.lock().take();
    BROWSER_CANCELLED.notify_waiters();
}

/// Start the phone from the session at `session_path`: registered, ringing,
/// and serving the control socket so `anvil-cli` can drive the app.
pub async fn start_phone(session_path: String) -> anyhow::Result<()> {
    // One start at a time: a push and the app's screen may both ask.
    static STARTING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _starting = STARTING.lock().await;
    if PHONE.lock().is_some() {
        return Ok(());
    }
    let settings_path = std::path::Path::new(&session_path).with_file_name("settings.json");
    // Where the platform wakes this install, when it has said (`system.rs`).
    let push = crate::system::load_push(std::path::Path::new(&session_path));
    let phone = on_runtime(async move {
        let store = Arc::new(FileTokenStore::new(session_path));
        let session = store
            .load()?
            .ok_or_else(|| anyhow::anyhow!("not signed in"))?;
        let client = Arc::new(FcpClient::new(session, Some(store as Arc<dyn TokenStore>)));
        let config = client.softphone().await?;
        let mut account = client.account_config(&config).await?;
        account.push = push;
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
            // A phone's system rings for it (CallKit, ConnectionService).
            ring: !cfg!(any(target_os = "android", target_os = "ios")),
            fallback_brand: managed().brand.clone(),
        })
        .await?;
        // The control socket is the desktop's (`anvil call 1002` from a
        // terminal); a phone has no terminal to drive it from.
        if !cfg!(any(target_os = "android", target_os = "ios")) {
            let served = phone.clone();
            tokio::spawn(async move {
                if let Err(e) = control::serve(served, control::Endpoint::default_for_user()).await
                {
                    tracing::warn!(%e, "the control socket is not served");
                }
            });
        }
        Ok(phone)
    })
    .await?;
    // Android's Telecom and iOS's CallKit show the calls from here, with or
    // without Dart: a push starts the phone with no Flutter running
    // (`system.rs`).
    if cfg!(any(target_os = "android", target_os = "ios")) {
        let _guard = runtime().enter();
        *FOLLOWER.lock() = Some(crate::system::follow(phone.clone()));
    }
    *PHONE.lock() = Some(phone);
    Ok(())
}

/// Stop the phone; the session stays signed in.
pub async fn stop_phone() -> anyhow::Result<()> {
    if let Some(follower) = FOLLOWER.lock().take() {
        follower.abort();
        crate::system::reset();
    }
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
        let store = Arc::new(FileTokenStore::new(&session_path));
        if let Some(session) = store.load()? {
            // Signed in by a version that did not remember: remembered now,
            // before the session goes.
            let last = anvil_fcp::LastSignIn::path_beside(std::path::Path::new(&session_path));
            if anvil_fcp::LastSignIn::load(&last).is_none() {
                remember(&session_path, &session.server.public_url, &session.username);
            }
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
    /// The user is recording it.
    pub recording: bool,
    /// The line it is on, by key (`own`, `extension:1005`, …); `None`
    /// without lines.
    pub line: Option<String>,
    /// Ringing on a line that does not ring this app: shown, not sounded.
    pub silent: bool,
}

/// One of the account's lines (FCP's `docs/DEVICE_LINES.md`).
pub struct Line {
    /// Its key: what a call on it is named by, and a name is set under.
    pub line: String,
    /// `own`, `extension`, `shared` or `on_behalf`.
    pub kind: String,
    pub aor: String,
    /// What it is called: the user's name, the administrator's, else
    /// `default_label`.
    pub label: String,
    pub default_label: String,
    pub registration: Registration,
    /// New messages in the line's mailbox, once it has said.
    pub voicemail_new: Option<u32>,
    /// Calls to it ring this app.
    pub ring: bool,
    pub shared: bool,
    pub appearances: u32,
    /// A shared line whose active calls the app may join.
    pub barge_in: bool,
    /// A shared line's appearances in use, by index.
    pub in_use: Vec<Appearance>,
}

/// One appearance of a shared line in use.
pub struct Appearance {
    pub index: u8,
    /// `alerting`, `progressing`, `active`, `held` or `seized`.
    pub state: String,
    /// The far party, when the server names one.
    pub remote: Option<String>,
    /// One of this app's own calls.
    pub ours: bool,
}

/// A connected call's media, as last measured.
pub struct Quality {
    pub jitter_ms: u32,
    pub packet_loss_permille: u32,
    pub rtt_ms: Option<u32>,
    /// RTP datagrams received since the call began, of any kind.
    pub packets_received: u64,
    /// Audio frames played.
    pub packets_played: u64,
    /// Packets dropped: the wrong payload type, failed SRTP, not RTP.
    pub packets_dropped: u64,
    /// The payload type of the last packet dropped for it.
    pub unexpected_pt: Option<u8>,
    /// The payload type the call's codec is sent with.
    pub expected_pt: Option<u8>,
    /// Where the last packet came from.
    pub last_source: Option<String>,
    /// The far end's packets lost over the last five seconds, per thousand.
    pub recent_loss_permille: u32,
    pub packets_sent: u64,
    pub bytes_sent: u64,
    /// RTP received, played or not.
    pub rtp_received: u64,
    pub bytes_received: u64,
    /// The far end's packets its sequence numbers say were sent, and those
    /// that never arrived.
    pub packets_expected: u64,
    pub packets_lost: u64,
    /// What flowed in the last second.
    pub send_kbps: u32,
    pub recv_kbps: u32,
    pub send_pps: u32,
    pub recv_pps: u32,
    /// What the far end last reported of ours (RTCP).
    pub far_loss_permille: Option<u32>,
    pub far_jitter_ms: Option<u32>,
    pub far_lost: Option<i64>,
    pub rtcp: bool,
    pub rtcp_sent: u64,
    pub rtcp_received: u64,
    pub clock_rate: u32,
    pub ptime_ms: u32,
    /// Where media flows between (`address:port`).
    pub local_rtp: Option<String>,
    pub remote_rtp: Option<String>,
    /// The estimated score (MOS) in hundredths: what the user hears, and
    /// what the far end hears once it has reported.
    pub mos_hundredths: u16,
    pub far_mos_hundredths: Option<u16>,
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
    /// How it went, when one of the user's phones reported it: the lower
    /// score (MOS) in hundredths, and whether it was poor (under 3.1).
    pub mos_hundredths: Option<u16>,
    pub poor: bool,
}

/// Someone in the tenant's directory.
pub struct Person {
    pub key: String,
    pub name: String,
    pub extension: Option<String>,
    pub department: Option<String>,
    pub job_title: Option<String>,
    /// `available`, `busy`, `away`, `dnd`, `ringing`, `on_call`,
    /// `offline`, … when shown.
    pub presence: Option<String>,
    /// Their own status's label beside it ("WFH").
    pub presence_label: Option<String>,
    pub on_call: bool,
    pub favourite: bool,
    /// What to dial.
    pub dial: String,
}

/// The user's presence (FCP's `docs/PRESENCE.md`).
pub struct MyPresence {
    /// What others see: `available`, `away`, `busy`, `dnd`, `ringing`,
    /// `on_call` or `offline`.
    pub shown: String,
    pub shown_label: Option<String>,
    /// Away because one of their apps says they are idle.
    pub idle: bool,
    /// What they chose: a built-in state or one of `statuses`' ids.
    pub status: String,
    pub status_state: String,
    pub note: Option<String>,
    /// RFC 3339.
    pub until: Option<String>,
    pub statuses: Vec<OwnStatus>,
}

/// One of the user's own statuses.
pub struct OwnStatus {
    pub id: String,
    pub label: String,
    /// `available`, `away`, `busy` or `dnd`.
    pub state: String,
}

/// A contact the user keeps, on this device or on their FCP account.
pub struct Contact {
    /// Empty in a new one; the phone (or FCP) gives it one when saved.
    pub id: String,
    /// Kept on the user's FCP account: every app they sign in to has it.
    /// Saving a device's contact with this set moves it there.
    pub on_account: bool,
    pub name: String,
    pub company: Option<String>,
    pub numbers: Vec<ContactNumber>,
    pub email: Option<String>,
    pub notes: Option<String>,
    /// The ringtone a call from one of its numbers rings with, by id;
    /// `None` (or one this device does not have) rings with the phone's.
    pub ringtone: Option<String>,
}

/// One of a contact's numbers: what it is (`mobile`, `work`, …) and what
/// to dial.
pub struct ContactNumber {
    pub label: String,
    pub number: String,
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

/// The system's microphones and speakers.
pub struct SystemAudio {
    pub inputs: Vec<AudioDevice>,
    pub outputs: Vec<AudioDevice>,
}

/// The system's microphones and speakers, before any phone starts: whether
/// the platform's audio is reachable at all (on Android, through the JVM
/// the app hands over at launch).
pub fn system_audio() -> anyhow::Result<SystemAudio> {
    use anvil_core::audio::AudioHost;
    let host = anvil_audio::CpalHost::new().map_err(|e| anyhow::anyhow!("no audio: {e}"))?;
    let (inputs, outputs): (Vec<_>, Vec<_>) = host.devices().into_iter().partition(|d| d.is_input);
    let list = |l: Vec<anvil_core::audio::DeviceInfo>| {
        l.into_iter()
            .map(|d| AudioDevice {
                id: d.id,
                name: d.name,
                default: d.is_default,
            })
            .collect()
    };
    Ok(SystemAudio {
        inputs: list(inputs),
        outputs: list(outputs),
    })
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
    /// The account's lines, its own first; empty without FCP (or from an
    /// older one): the account alone.
    pub lines: Vec<Line>,
    /// The tenant lets the user name their lines.
    pub may_rename_lines: bool,
    pub calls: Vec<Call>,
    pub voicemail_new: u32,
    pub dnd: Option<bool>,
    /// The tenant's brand: its app name, colours (`#RRGGBB`) and logo.
    pub brand_name: Option<String>,
    pub brand_primary: Option<String>,
    pub brand_logo: Option<Vec<u8>>,
    pub brand_icon: Option<Vec<u8>>,
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
    /// The speaker chosen to ring on; `None` is the system's default.
    pub ring_device: Option<String>,
    /// Anvil's ringtones, then the brand's; the one that rings, by id.
    pub ringtones: Vec<Ringtone>,
    pub ringtone: String,
    /// Echo cancellation and noise suppression on or off; `None` when the
    /// phone's audio does none of its own.
    pub echo_cancellation: Option<bool>,
    pub noise_suppression: Option<bool>,
    /// The microphone's and the speaker's volume in percent (100 the sound
    /// as it is, up to 200); `None` when the system sets them.
    pub microphone_volume: Option<u16>,
    pub speaker_volume: Option<u16>,
    /// The voicemail message playing.
    pub playing: Option<String>,
    /// The user may record calls (FCP says so): the record button shows.
    pub may_record: bool,
    /// The user's contacts, by name: on this device and on their account.
    pub contacts: Vec<Contact>,
    /// Signed in to FCP: whether the user is listed in the tenant's
    /// directory; `None` until FCP says.
    pub directory_listed: Option<bool>,
    /// The tenant lets the user take themselves out of the directory.
    pub may_change_directory_listing: bool,
    /// Signed in to an FCP that keeps it: the user's presence.
    pub presence: Option<MyPresence>,
    /// A test of the microphone or a speaker under way: `recording`,
    /// `playing_back`, `speaker` or `ring`; `None` when none is.
    pub audio_test: Option<String>,
    /// While recording: how loud the microphone is, 0 to 100.
    pub audio_test_level: u8,
}

/// A ringtone to choose: `anvil`, `classic`, `brand:` and its name, or
/// `file:` and the name of a file added.
pub struct Ringtone {
    pub id: String,
    pub name: String,
}

fn registration(r: anvil_app::Registration) -> Registration {
    match r {
        anvil_app::Registration::Unregistered => Registration::Unregistered,
        anvil_app::Registration::Registering => Registration::Registering,
        anvil_app::Registration::Registered => Registration::Registered,
        anvil_app::Registration::Failed => Registration::Failed,
    }
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
    /// `brand`, `transfer_progress`, `lines`, `calling`, `recents`, `people`,
    /// `person`, `voicemail`, `audio`, `playing`, `push`, `may_record`,
    /// `contacts`.
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
    let in_use = |l: &anvil_app::LineView| -> Vec<Appearance> {
        l.in_use
            .iter()
            .map(|a| Appearance {
                index: a.index,
                state: a.state.clone(),
                remote: a.remote.clone(),
                ours: s.appearance_is_ours(&l.line, a),
            })
            .collect()
    };
    let lines = s
        .lines
        .iter()
        .map(|l| Line {
            line: l.line.clone(),
            kind: l.kind.clone(),
            aor: l.aor.clone(),
            label: l.label.clone(),
            default_label: l.default_label.clone(),
            registration: registration(l.registration),
            voicemail_new: l.message_waiting.map(|m| m.new),
            ring: l.ring,
            shared: l.shared,
            appearances: l.appearances,
            barge_in: l.barge_in,
            in_use: in_use(l),
        })
        .collect();
    Ok(PhoneState {
        aor: s.aor,
        registration: registration(s.registration),
        lines,
        may_rename_lines: s.may_rename_lines,
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
                    packets_received: q.packets_received,
                    packets_played: q.packets_played,
                    packets_dropped: q.packets_dropped,
                    unexpected_pt: q.unexpected_pt.map(|(pt, _)| pt),
                    expected_pt: q.unexpected_pt.map(|(_, want)| want),
                    last_source: q.last_source.map(|a| a.to_string()),
                    recent_loss_permille: q.recent_loss_permille,
                    packets_sent: q.packets_sent,
                    bytes_sent: q.bytes_sent,
                    rtp_received: q.rtp_received,
                    bytes_received: q.bytes_received,
                    packets_expected: q.packets_expected,
                    packets_lost: q.packets_lost,
                    send_kbps: q.send_kbps,
                    recv_kbps: q.recv_kbps,
                    send_pps: q.send_pps,
                    recv_pps: q.recv_pps,
                    far_loss_permille: q.far_loss_permille,
                    far_jitter_ms: q.far_jitter_ms,
                    far_lost: q.far_lost,
                    rtcp: q.rtcp,
                    rtcp_sent: q.rtcp_sent,
                    rtcp_received: q.rtcp_received,
                    clock_rate: q.clock_rate,
                    ptime_ms: q.ptime_ms,
                    local_rtp: q.local_rtp.map(|a| a.to_string()),
                    remote_rtp: q.remote_rtp.map(|a| a.to_string()),
                    mos_hundredths: q.mos_hundredths,
                    far_mos_hundredths: q.far_mos_hundredths,
                }),
                connected_at: c.connected_at,
                recording: c.recording,
                line: c.line,
                silent: c.silent,
            })
            .collect(),
        voicemail_new: s.message_waiting.map(|m| m.new).unwrap_or(0),
        dnd: s.dnd,
        brand_name: s.brand.as_ref().map(|b| b.app_name.clone()),
        brand_primary: s.brand.as_ref().and_then(|b| b.primary.clone()),
        brand_logo: s.brand.as_ref().and_then(|b| b.logo.clone()),
        brand_icon: s.brand.and_then(|b| b.icon),
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
                mos_hundredths: r.mos_hundredths,
                poor: r.poor,
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
                presence_label: p.presence_label,
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
        ring_device: s.audio.ring,
        ringtones: s
            .audio
            .ringtones
            .into_iter()
            .map(|r| Ringtone {
                id: r.id,
                name: r.name,
            })
            .collect(),
        ringtone: s.audio.ringtone,
        echo_cancellation: s.audio.processing.map(|p| p.echo_cancellation),
        noise_suppression: s.audio.processing.map(|p| p.noise_suppression),
        microphone_volume: s.audio.volume.map(|v| v.microphone),
        speaker_volume: s.audio.volume.map(|v| v.speaker),
        playing: s.playing,
        may_record: s.may_record,
        directory_listed: s.directory.map(|d| d.listed),
        may_change_directory_listing: s.directory.is_some_and(|d| d.may_change),
        presence: s.presence.map(|p| MyPresence {
            shown: p.shown,
            shown_label: p.shown_label,
            idle: p.idle,
            status: p.status,
            status_state: p.status_state,
            note: p.note,
            until: p.until,
            statuses: p
                .statuses
                .into_iter()
                .map(|s| OwnStatus {
                    id: s.id,
                    label: s.label,
                    state: s.state,
                })
                .collect(),
        }),
        audio_test: s.audio_test.map(|t| {
            match t.stage {
                anvil_app::AudioTestStage::Recording => "recording",
                anvil_app::AudioTestStage::PlayingBack => "playing_back",
                anvil_app::AudioTestStage::Speaker => "speaker",
                anvil_app::AudioTestStage::Ring => "ring",
            }
            .to_string()
        }),
        audio_test_level: s.audio_test.map_or(0, |t| t.level),
        contacts: s
            .contacts
            .into_iter()
            .map(|c| Contact {
                id: c.id,
                on_account: c.on_account,
                name: c.name,
                company: c.company,
                numbers: c
                    .numbers
                    .into_iter()
                    .map(|n| ContactNumber {
                        label: n.label,
                        number: n.number,
                    })
                    .collect(),
                email: c.email,
                notes: c.notes,
                ringtone: c.ringtone,
            })
            .collect(),
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
                C::Lines { .. } => PhoneChange {
                    kind: "lines".into(),
                    call: None,
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
                C::Push { .. } => kind_only("push"),
                C::MayRecord { .. } => kind_only("may_record"),
                C::Contacts { .. } => kind_only("contacts"),
                C::Directory { .. } => kind_only("directory"),
                C::Presence { .. } => kind_only("presence"),
                C::AudioTest { .. } => kind_only("audio_test"),
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

/// Native code's way to the phone (`android.rs`, `ios.rs`): woken by a push, the
/// phone starts from the saved session, or registers again at once if it
/// runs — its connection to the registrar may be gone.
#[frb(ignore)]
pub(crate) async fn wake(session_path: String) -> anyhow::Result<()> {
    if PHONE.lock().is_some() {
        execute(Command::Register).await?;
        return Ok(());
    }
    start_phone(session_path).await
}

/// Native code's way to the phone: the push address the platform gave,
/// kept for every start and registered now if the phone runs.
#[frb(ignore)]
pub(crate) async fn set_push(
    session_path: String,
    address: Option<anvil_core::PushAddress>,
) -> anyhow::Result<()> {
    crate::system::save_push(std::path::Path::new(&session_path), address.as_ref())?;
    if PHONE.lock().is_some() {
        execute(Command::Push { address }).await?;
    }
    Ok(())
}

/// Native code's way to the phone: what the user did at the system's call
/// screen.
#[frb(ignore)]
pub(crate) async fn act(json: String) -> anyhow::Result<()> {
    let phone = phone()?;
    on_runtime(async move { crate::system::act(&phone, &json).await }).await
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

/// Dial a number, an extension, a name or an address, from one of the
/// lines by key (`None`: the account's own); the new call's id.
pub async fn place_call(target: String, line: Option<String>) -> anyhow::Result<u64> {
    execute(Command::Call { target, line })
        .await?
        .ok_or_else(|| anyhow::anyhow!("no call was placed"))
}

/// Take appearance `index` of a shared line (by key): pick up the call
/// held there, or join the one up where the line allows barging in; the
/// new call's id.
pub async fn take_appearance(line: String, index: u8) -> anyhow::Result<u64> {
    execute(Command::TakeAppearance { line, index })
        .await?
        .ok_or_else(|| anyhow::anyhow!("no appearance was taken"))
}

/// Name one of the lines, where the tenant allows it; empty gives it back
/// its administrator's label.
pub async fn rename_line(line: String, label: String) -> anyhow::Result<()> {
    execute(Command::RenameLine { line, label })
        .await
        .map(|_| ())
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

/// Record a call the user is on (`on`), or stop the recording they
/// started. FCP plays the recording announcement to both parties first.
pub async fn record(call: Option<u64>, on: bool) -> anyhow::Result<()> {
    execute(Command::Record { call, on }).await.map(|_| ())
}

/// Keep a contact: a new one (empty id) or a change to one, on this device
/// or on the account (`on_account`; a device's contact saved so moves).
pub async fn save_contact(contact: Contact) -> anyhow::Result<()> {
    execute(Command::SaveContact {
        contact: anvil_app::Contact {
            id: contact.id,
            on_account: contact.on_account,
            name: contact.name,
            company: contact.company,
            numbers: contact
                .numbers
                .into_iter()
                .map(|n| anvil_app::ContactNumber {
                    label: n.label,
                    number: n.number,
                })
                .collect(),
            email: contact.email,
            notes: contact.notes,
            ringtone: contact.ringtone,
        },
    })
    .await
    .map(|_| ())
}

/// What a contacts import did.
pub struct ContactsImported {
    /// Contacts made.
    pub imported: u32,
    /// Entries already kept, or twice in the file: not made again.
    pub duplicates: u32,
    /// Entries left out, each with its place in the file and why.
    pub skipped: Vec<String>,
}

/// Read the contacts in a vCard or CSV file into this device's contacts
/// or, with `on_account`, the FCP account's; one already kept there is not
/// made again.
pub async fn import_contacts(path: String, on_account: bool) -> anyhow::Result<ContactsImported> {
    let phone = phone()?;
    let imported = on_runtime(async move {
        let answer = phone
            .execute(Command::ImportContacts { path, on_account })
            .await?;
        Ok(answer.imported.unwrap_or_default())
    })
    .await?;
    Ok(ContactsImported {
        imported: imported.imported,
        duplicates: imported.duplicates,
        skipped: imported.skipped,
    })
}

/// Write every contact to a file: CSV when its name ends `.csv`, else
/// vCard.
pub async fn export_contacts(path: String) -> anyhow::Result<()> {
    execute(Command::ExportContacts { path }).await.map(|_| ())
}

/// Be listed in the tenant's directory, or not (FCP refuses unless the
/// tenant lets its users choose).
pub async fn set_directory_listed(on: bool) -> anyhow::Result<()> {
    execute(Command::DirectoryListed { on }).await.map(|_| ())
}

/// Pick a status: a built-in one (`available`, `away`, `busy`, `dnd`) or
/// one of the user's own by id, with a note and an RFC 3339 time it
/// clears at.
pub async fn set_presence(
    status: String,
    note: Option<String>,
    until: Option<String>,
) -> anyhow::Result<()> {
    execute(Command::Presence {
        status,
        note,
        until,
    })
    .await
    .map(|_| ())
}

/// Set the note beside the user's status, or clear it.
pub async fn set_presence_note(note: Option<String>) -> anyhow::Result<()> {
    execute(Command::PresenceNote { note }).await.map(|_| ())
}

/// Say the user is idle, or back.
pub async fn set_presence_idle(idle: bool) -> anyhow::Result<()> {
    execute(Command::PresenceIdle { idle }).await.map(|_| ())
}

/// Keep a status of the user's own.
pub async fn add_status(label: String, state: String) -> anyhow::Result<()> {
    execute(Command::StatusAdd { label, state })
        .await
        .map(|_| ())
}

/// Rename one of the user's statuses, or change its state.
pub async fn edit_status(
    id: String,
    label: Option<String>,
    state: Option<String>,
) -> anyhow::Result<()> {
    execute(Command::StatusEdit {
        status: id,
        label,
        state,
    })
    .await
    .map(|_| ())
}

/// Delete one of the user's statuses.
pub async fn remove_status(id: String) -> anyhow::Result<()> {
    execute(Command::StatusRemove { status: id })
        .await
        .map(|_| ())
}

/// Put the user's statuses in this order.
pub async fn order_statuses(ids: Vec<String>) -> anyhow::Result<()> {
    execute(Command::StatusOrder { ids }).await.map(|_| ())
}

/// Delete a contact, from this device or from the account.
pub async fn delete_contact(id: String) -> anyhow::Result<()> {
    execute(Command::DeleteContact { id }).await.map(|_| ())
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

/// Clear the recent calls shown on this device; FCP keeps the records.
pub async fn clear_recents() -> anyhow::Result<()> {
    execute(Command::ClearRecents).await.map(|_| ())
}

/// Remove one recent call, by record id, from those shown on this device.
pub async fn hide_recent(id: String) -> anyhow::Result<()> {
    execute(Command::HideRecent { id }).await.map(|_| ())
}

/// Use this microphone (`input`) or speaker by id, `None` for the system's
/// default; the call that is up moves to it.
pub async fn choose_audio(input: bool, device: Option<String>) -> anyhow::Result<()> {
    let kind = if input {
        anvil_app::AudioKind::Input
    } else {
        anvil_app::AudioKind::Output
    };
    execute(Command::Audio { kind, device }).await.map(|_| ())
}

/// Ring on this speaker by id, `None` for the system's default.
pub async fn choose_ring_device(device: Option<String>) -> anyhow::Result<()> {
    execute(Command::Audio {
        kind: anvil_app::AudioKind::Ring,
        device,
    })
    .await
    .map(|_| ())
}

/// Ring with this ringtone by id, `None` for the brand's (else Anvil's);
/// heard once now when no call is up.
pub async fn choose_ringtone(id: Option<String>) -> anyhow::Result<()> {
    execute(Command::Ringtone { id }).await.map(|_| ())
}

/// Add a ringtone from a sound file on this device (WAV, Ogg Vorbis or
/// MP3): kept with the app's files, chosen, and heard once.
pub async fn add_ringtone_file(path: String) -> anyhow::Result<()> {
    execute(Command::RingtoneFile { path }).await.map(|_| ())
}

/// Remove a ringtone added from a file (`file:` and its name).
pub async fn remove_ringtone(id: String) -> anyhow::Result<()> {
    execute(Command::RemoveRingtone { id }).await.map(|_| ())
}

/// Test the microphone: a few seconds recorded, its level shown, then
/// heard back on the speaker calls use.
pub async fn test_microphone() -> anyhow::Result<()> {
    execute(Command::TestMicrophone).await.map(|_| ())
}

/// Test a speaker: a sound on the one calls use, or (`ring`) on the one
/// that rings, with the ringtone.
pub async fn test_speaker(ring: bool) -> anyhow::Result<()> {
    execute(Command::TestSpeaker { ring }).await.map(|_| ())
}

/// Stop an audio test.
pub async fn stop_audio_test() -> anyhow::Result<()> {
    execute(Command::StopTest).await.map(|_| ())
}

/// Turn echo cancellation or noise suppression on or off (`None` leaves it
/// as it is), in the call that is up too.
pub async fn set_audio_processing(
    echo_cancellation: Option<bool>,
    noise_suppression: Option<bool>,
) -> anyhow::Result<()> {
    execute(Command::Processing {
        echo_cancellation,
        noise_suppression,
    })
    .await
    .map(|_| ())
}

/// Set the microphone's or the speaker's volume in percent (`None` leaves
/// it as it is), in the call that is up too.
pub async fn set_audio_volume(microphone: Option<u16>, speaker: Option<u16>) -> anyhow::Result<()> {
    execute(Command::Volume {
        microphone,
        speaker,
    })
    .await
    .map(|_| ())
}

/// Read the user's data from FCP again.
pub async fn refresh() -> anyhow::Result<()> {
    execute(Command::Refresh).await.map(|_| ())
}

/// Start the log file in `dir` (the support directory, beside the
/// session); its path. Once per app: later calls return the same path.
#[frb(sync)]
pub fn start_log(dir: String) -> anyhow::Result<String> {
    let path = crate::log::start(std::path::Path::new(&dir))?;
    Ok(path.to_string_lossy().into_owned())
}

/// The log file's path, once started.
#[frb(sync)]
pub fn log_path() -> Option<String> {
    crate::log::path().map(|p| p.to_string_lossy().into_owned())
}

#[frb(init)]
pub fn init_app() {
    flutter_rust_bridge::setup_default_user_utils();
    // Before any HTTPS request: the brand, the profile, an update check.
    anvil_core::install_default_crypto_provider();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_email_username_finds_the_server_when_none_is_typed() {
        assert_eq!(place_of("", "ann@example.com"), "ann@example.com");
        assert_eq!(
            place_of(" pbx.example.com ", "ann@example.com"),
            "pbx.example.com"
        );
        assert_eq!(place_of("", "ann"), "");
    }

    #[test]
    fn a_sign_in_is_remembered_beside_the_session() {
        let dir = std::env::temp_dir().join(format!(
            "anvil-bridge-last-{}-{}",
            std::process::id(),
            anvil_fcp::AppClient::new_install_id()
        ));
        let session = dir.join("session.json");
        let session_path = session.to_string_lossy().into_owned();
        let last = |p: &str| runtime().block_on(last_sign_in(p.to_string())).unwrap();
        assert!(last(&session_path).is_none());

        remember(&session_path, " pbx.example.com ", "ann");
        let r = last(&session_path).unwrap();
        assert_eq!(
            (r.place.as_str(), r.username.as_str()),
            ("pbx.example.com", "ann")
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
