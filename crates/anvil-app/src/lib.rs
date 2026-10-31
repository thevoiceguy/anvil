//! The running phone (`docs/APP.md` §3).
//!
//! [`Phone`] owns an [`anvil_core::Anvil`] and, when signed in, an
//! [`anvil_fcp::FcpClient`]. It keeps the [`State`] a screen shows, publishes
//! a [`Change`] for every event, and carries out [`Command`]s. The app and
//! the CLI are thin over it, so they cannot behave differently.
//! [`control`] serves it on a local socket: `anvil run` serves it, and
//! `anvil call …` drives whatever does.

// Unsafe only where Windows' API needs it, in its own modules.
#![deny(unsafe_code)]

mod capture;
mod clip;
mod command;
mod contact_file;
pub mod control;
mod data;
pub mod managed;
#[cfg(windows)]
#[allow(unsafe_code)]
mod managed_windows;
mod ring;
mod settings;
mod state;
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
pub mod system_brand;
#[cfg(windows)]
#[allow(unsafe_code)]
mod user_only;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use anvil_core::{Anvil, AnvilConfig, CallId, EndReason, Event, EventStream};
use anvil_fcp::FcpClient;
use parking_lot::Mutex;
use tokio::sync::{broadcast, RwLock};

pub use anvil_fcp::{CallingSettings, CallingUpdate};
pub use capture::{CaptureMode, CaptureState};
pub use clip::{decode_wav, Clip};
pub use command::{AudioKind, Command, Imported, Response};
pub use state::{
    AppearanceView, AudioDevice, AudioDevices, AudioProcessing, AudioTest, AudioTestStage,
    AudioVolume, Brand, CallState, CallView, Change, Contact, ContactNumber, Direction,
    DirectoryListing, LineView, MessageWaiting, MyPresence, OwnStatus, Person, Push, Quality,
    Recent, Registration, Ringtone, State, Voicemail,
};

use settings::LocalSettings;

/// What can go wrong running the phone.
#[derive(Debug, thiserror::Error)]
pub enum PhoneError {
    #[error("{0}")]
    Anvil(#[from] anvil_core::AnvilError),
    #[error("{0}")]
    Fcp(#[from] anvil_fcp::FcpError),
    #[error("{0}")]
    Refused(String),
}

/// The biggest contacts file read, as FCP takes.
const CONTACT_FILE_LIMIT: usize = 10 * 1024 * 1024;

/// How to start the phone.
pub struct PhoneConfig {
    pub anvil: AnvilConfig,
    /// FCP, when signed in: do not disturb and the rest of the user's data.
    pub fcp: Option<Arc<FcpClient>>,
    /// Register on start (a phone talking only peer to peer does not).
    pub register: bool,
    /// Where this device's own settings are kept (favourites, the audio
    /// devices chosen); `None` keeps them only while the phone runs.
    pub settings_path: Option<PathBuf>,
    /// Ring here for a call ringing in (a desktop); a phone whose system
    /// rings for it (CallKit, ConnectionService) does not.
    pub ring: bool,
    /// The brand while FCP has none: the installed profile's
    /// ([`managed::Managed::brand`]).
    pub fallback_brand: Option<anvil_core::brand::BrandProfile>,
}

/// The phone: cheap to clone, every clone the same phone.
#[derive(Clone)]
pub struct Phone {
    inner: Arc<Inner>,
}

struct Inner {
    anvil: RwLock<Option<Anvil>>,
    fcp: Option<Arc<FcpClient>>,
    /// The domain a bare number or name is dialled in.
    domain: String,
    sip_address: std::net::SocketAddr,
    state: Mutex<State>,
    changes: broadcast::Sender<Change>,
    events: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Following FCP's live events.
    fcp_events: Mutex<Option<tokio::task::JoinHandle<()>>>,
    settings_path: Option<PathBuf>,
    local: Mutex<LocalSettings>,
    /// The contacts on the user's FCP account, as last read or written.
    account_contacts: Mutex<Vec<Contact>>,
    /// FCP's feature codes by name (`park`, `retrieve`, …).
    features: Mutex<BTreeMap<String, String>>,
    /// The voicemail message playing: stopped by aborting it.
    playback: Mutex<Option<tokio::task::AbortHandle>>,
    /// Whether this phone rings (`PhoneConfig::ring`).
    ring: bool,
    /// What is sounding: stopped by aborting it.
    ringer: Mutex<Option<(ring::Sound, tokio::task::AbortHandle)>>,
    /// Held while deciding what sounds, so a decision made on an older
    /// state cannot start after a newer one.
    sounding: tokio::sync::Mutex<()>,
    /// The brand's ringtones that could be played: name, whether the
    /// brand's default, the sound.
    brand_ringtones: Mutex<Vec<(String, bool, ring::Tone)>>,
    /// The ringtones added from files, kept beside the settings: file
    /// name, sound.
    file_ringtones: Mutex<Vec<(String, ring::Tone)>>,
    /// The audio test under way: stopped by aborting it.
    audio_test: Mutex<Option<tokio::task::AbortHandle>>,
    /// The profile's brand, shown while FCP has none.
    fallback_brand: Option<anvil_core::brand::BrandProfile>,
    /// The app's device on FCP, as `/me/softphone` names it: what a
    /// capture is uploaded under.
    device_id: Mutex<Option<String>>,
}

impl Phone {
    /// Start the phone: Anvil up, events followed, registered when asked
    /// (and message waiting subscribed). Signed in to FCP, the calling
    /// settings are read before it returns, and the rest of the user's data
    /// and FCP's live events follow.
    pub async fn start(cfg: PhoneConfig) -> Result<Self, PhoneError> {
        let aor = cfg.anvil.account.aor.clone();
        let domain = domain_of(&aor);
        let local = cfg
            .settings_path
            .as_deref()
            .map(LocalSettings::load)
            .unwrap_or_default();
        let (anvil, events) = Anvil::start(cfg.anvil).await?;
        if local.ring_output.is_some() {
            if let Err(e) = anvil.choose_ring_device(local.ring_output.as_deref()) {
                tracing::warn!(%e, "the speaker chosen to ring on is not here; using the default");
            }
        }
        let processing = anvil_core::audio::Processing {
            echo_cancellation: local.echo_cancellation,
            noise_suppression: local.noise_suppression,
        };
        if anvil.audio_processing().is_some_and(|p| p != processing) {
            if let Err(e) = anvil.set_audio_processing(processing) {
                tracing::warn!(%e, "the microphone's cleaning was not set");
            }
        }
        let volume = anvil_core::audio::Volume {
            microphone: local.microphone_volume,
            speaker: local.speaker_volume,
        };
        if anvil.audio_volume().is_some_and(|v| v != volume) {
            if let Err(e) = anvil.set_audio_volume(volume) {
                tracing::warn!(%e, "the volumes were not set");
            }
        }
        if local.audio_input.is_some() || local.audio_output.is_some() {
            if let Err(e) = anvil
                .choose_audio_devices(local.audio_input.as_deref(), local.audio_output.as_deref())
            {
                tracing::warn!(%e, "the audio devices chosen are not here; using the defaults");
            }
        }
        // Captures past their week, or over the folder's limit, go.
        let capture_folder = capture::folder(cfg.settings_path.as_deref());
        let capture_state = match &capture_folder {
            Some(dir) => {
                capture::prune(
                    dir,
                    capture::keep_bytes(0),
                    None,
                    std::time::SystemTime::now(),
                );
                let (saved, saved_bytes) = capture::summary(dir);
                CaptureState {
                    mode: local.capture_calls,
                    saved,
                    saved_bytes,
                    folder: Some(dir.display().to_string()),
                    ..CaptureState::default()
                }
            }
            None => CaptureState::default(),
        };
        let (changes, _) = broadcast::channel(256);
        let file_ringtones = ringtone_dir(cfg.settings_path.as_deref())
            .map(|d| ring::from_dir(&d))
            .unwrap_or_default();
        let inner = Arc::new(Inner {
            sip_address: anvil.sip_address(),
            anvil: RwLock::new(Some(anvil)),
            fcp: cfg.fcp,
            domain,
            state: Mutex::new(State {
                contacts: local.contacts.clone(),
                capture: capture_state,
                ..State::new(aor)
            }),
            changes,
            events: Mutex::new(None),
            fcp_events: Mutex::new(None),
            settings_path: cfg.settings_path,
            local: Mutex::new(local),
            account_contacts: Mutex::new(Vec::new()),
            features: Mutex::new(BTreeMap::new()),
            playback: Mutex::new(None),
            ring: cfg.ring,
            ringer: Mutex::new(None),
            sounding: tokio::sync::Mutex::new(()),
            brand_ringtones: Mutex::new(Vec::new()),
            file_ringtones: Mutex::new(file_ringtones),
            audio_test: Mutex::new(None),
            fallback_brand: cfg.fallback_brand,
            device_id: Mutex::new(None),
        });
        if let Some(fallback) = &inner.fallback_brand {
            inner.set_brand(fallback);
        }
        let follower = tokio::spawn(follow(Arc::downgrade(&inner), events));
        *inner.events.lock() = Some(follower);
        let phone = Self { inner };
        phone.read_audio_devices().await;

        if cfg.register {
            phone.anvil().await?.register().await?;
            if let Err(e) = phone.anvil().await?.subscribe_mwi().await {
                tracing::debug!(%e, "no message waiting subscription");
            }
        }
        if let Some(fcp) = phone.inner.fcp.clone() {
            data::refresh_calling(&phone.inner, &fcp).await;
            let inner = Arc::clone(&phone.inner);
            let first = Arc::clone(&fcp);
            tokio::spawn(async move {
                data::refresh_softphone(&inner, &first).await;
                tokio::join!(
                    data::refresh_recents(&inner, &first),
                    data::refresh_people(&inner, &first),
                    data::refresh_voicemail(&inner, &first),
                    data::refresh_contacts(&inner, &first),
                );
            });
            let task = tokio::spawn(data::follow(Arc::downgrade(&phone.inner), fcp));
            *phone.inner.fcp_events.lock() = Some(task);
        }
        Ok(phone)
    }

    /// The phone as it is now.
    pub fn state(&self) -> State {
        self.inner.state.lock().clone()
    }

    /// Every change from now on.
    pub fn changes(&self) -> broadcast::Receiver<Change> {
        self.inner.changes.subscribe()
    }

    /// Where peers reach this phone over SIP.
    pub fn sip_address(&self) -> std::net::SocketAddr {
        self.inner.sip_address
    }

    /// Carry out a command.
    pub async fn execute(&self, command: Command) -> Result<Response, PhoneError> {
        match command {
            Command::Call { target, line } => {
                let target = self.dial_target(&target);
                // From one of the lines, by its key: its address is the
                // INVITE's From. The account's own line is the account.
                let from = match line.as_deref() {
                    None | Some("own") => None,
                    Some(key) => Some(
                        self.inner
                            .state
                            .lock()
                            .lines
                            .iter()
                            .find(|l| l.line == key)
                            .map(|l| l.aor.clone())
                            .ok_or_else(|| PhoneError::Refused(format!("no line {key}")))?,
                    ),
                };
                self.hold_the_others(None).await?;
                let to = target.clone();
                let call = self
                    .anvil()
                    .await?
                    .place_call_from(&to, from.as_deref())
                    .await?;
                self.update_call(
                    call.0,
                    |c| {
                        c.remote = target.clone();
                        c.line = line.clone();
                    },
                    || CallView {
                        line: line.clone(),
                        ..CallView::new(call.0, Direction::Outgoing, &target, CallState::Dialing)
                    },
                );
                Ok(Response {
                    call: Some(call.0),
                    ..Response::ok()
                })
            }
            Command::Answer { call } => {
                let id = self.pick(call, "no call to answer", |c| {
                    c.direction == Direction::Incoming && c.state == CallState::Ringing
                })?;
                // Quiet at once, not when the call is set up.
                self.inner.stop_sound();
                self.hold_the_others(Some(id)).await?;
                let answered = self.anvil().await?.answer(CallId(id)).await.map(|_| ());
                self.inner.sound().await;
                answered?;
                Ok(Response::ok())
            }
            Command::Decline { call } => {
                let id = self.pick(call, "no call to decline", |c| {
                    c.direction == Direction::Incoming && c.state == CallState::Ringing
                })?;
                self.anvil().await?.reject(CallId(id), 603).await?;
                Ok(Response::ok())
            }
            Command::Hangup { call } => {
                let id = self
                    .pick(call, "no call to hang up", |c| {
                        c.state == CallState::Connected && !c.held
                    })
                    .or_else(|_| self.pick(call, "no call to hang up", |_| true))?;
                self.anvil().await?.hangup(CallId(id)).await?;
                Ok(Response::ok())
            }
            Command::Hold { call } | Command::Resume { call } => {
                let on = matches!(command, Command::Hold { .. });
                let id = self.pick(
                    call,
                    if on {
                        "no call to hold"
                    } else {
                        "no held call"
                    },
                    |c| c.state == CallState::Connected && c.held != on,
                )?;
                if !on {
                    // Back to a held call puts the one up on hold: a swap.
                    self.hold_the_others(Some(id)).await?;
                }
                self.anvil().await?.hold(CallId(id), on).await?;
                self.update_call(id, |c| c.held = on, || unreachable_view(id));
                Ok(Response::ok())
            }
            Command::Mute { call } | Command::Unmute { call } => {
                let on = matches!(command, Command::Mute { .. });
                let id = self.pick(call, "no call to mute", |c| c.state == CallState::Connected)?;
                self.anvil().await?.mute(CallId(id), on)?;
                self.update_call(id, |c| c.muted = on, || unreachable_view(id));
                Ok(Response::ok())
            }
            Command::Dtmf { digits, call } => {
                let id = self.pick(call, "no call to send digits on", |c| {
                    c.state == CallState::Connected
                })?;
                for digit in digits.chars() {
                    self.anvil().await?.send_dtmf(CallId(id), digit).await?;
                }
                Ok(Response::ok())
            }
            Command::Transfer { target, call } => {
                let id = self.pick(call, "no call to transfer", |c| {
                    c.state == CallState::Connected
                })?;
                let target = self.dial_target(&target);
                self.anvil().await?.transfer(CallId(id), &target).await?;
                Ok(Response::ok())
            }
            Command::TransferAttended { call, to } => {
                for id in [call, to] {
                    if self.inner.state.lock().call(id).is_none() {
                        return Err(PhoneError::Refused(format!("no call {id}")));
                    }
                }
                self.anvil()
                    .await?
                    .transfer_attended(CallId(call), CallId(to))
                    .await?;
                Ok(Response::ok())
            }
            Command::Push { address } => {
                self.anvil().await?.set_push(address).await?;
                Ok(Response::ok())
            }
            Command::Register => {
                self.anvil().await?.register().await?;
                Ok(Response::ok())
            }
            Command::Dnd { on } => {
                let fcp = self.inner.fcp.as_ref().ok_or_else(|| {
                    PhoneError::Refused("do not disturb needs a phone signed in to FCP".into())
                })?;
                let settings = fcp
                    .set_calling(&anvil_fcp::CallingUpdate {
                        dnd: Some(on),
                        ..Default::default()
                    })
                    .await?;
                self.inner.set_calling(settings);
                Ok(Response::ok())
            }
            Command::Calling { update } => {
                let settings = self.fcp("calling settings")?.set_calling(&update).await?;
                self.inner.set_calling(settings);
                Ok(Response::ok())
            }
            Command::Park { call } => {
                let id = self.pick(call, "no call to park", |c| c.state == CallState::Connected)?;
                let code = self.inner.features.lock().get("park").cloned();
                let code = code
                    .ok_or_else(|| PhoneError::Refused("park needs FCP's feature codes".into()))?;
                // As a desk phone parks: the call held and the park code
                // dialled. FCP parks the far party and says the orbit on
                // this call; a transfer to the code (REFER) left nobody on
                // the line to hear where it went.
                let held = self
                    .inner
                    .state
                    .lock()
                    .calls
                    .iter()
                    .any(|c| c.id == id && c.held);
                if !held {
                    self.anvil().await?.hold(CallId(id), true).await?;
                    self.update_call(id, |c| c.held = true, || unreachable_view(id));
                }
                let target = self.dial_target(&code);
                let placed = self.anvil().await?.place_call(&target).await?;
                self.update_call(
                    placed.0,
                    |c| c.remote = target.clone(),
                    || CallView::new(placed.0, Direction::Outgoing, &target, CallState::Dialing),
                );
                Ok(Response {
                    call: Some(placed.0),
                    ..Response::ok()
                })
            }
            Command::Heard { id } => {
                self.mark_heard(&id).await?;
                Ok(Response::ok())
            }
            Command::DeleteVoicemail { id } => {
                if self.inner.state.lock().playing.as_deref() == Some(&id) {
                    self.stop_playing();
                }
                self.fcp("voicemail")?.delete_voicemail(&id).await?;
                self.inner
                    .update_voicemail(|list| list.retain(|m| m.id != id));
                Ok(Response::ok())
            }
            Command::Play { id } => {
                let message = self
                    .inner
                    .state
                    .lock()
                    .voicemail
                    .iter()
                    .find(|m| m.id == id)
                    .cloned()
                    .ok_or_else(|| PhoneError::Refused(format!("no message {id}")))?;
                let wav = self.voicemail_audio(&id).await?;
                let clip = clip::decode_wav(&wav).map_err(PhoneError::Refused)?;
                self.stop_playing();
                let task = self.anvil().await?.play(clip.samples, clip.sample_rate)?;
                *self.inner.playback.lock() = Some(task.abort_handle());
                self.inner.set_playing(Some(id.clone()));
                let inner = Arc::downgrade(&self.inner);
                let playing = id.clone();
                tokio::spawn(async move {
                    // Played out (not stopped): nothing is playing now.
                    if task.await.is_ok() {
                        if let Some(inner) = inner.upgrade() {
                            if inner.state.lock().playing.as_deref() == Some(&playing) {
                                *inner.playback.lock() = None;
                                inner.set_playing(None);
                            }
                        }
                    }
                });
                if message.new {
                    // Heard once it plays, as a desk phone marks it.
                    self.mark_heard(&id).await?;
                }
                Ok(Response::ok())
            }
            Command::Stop => {
                self.stop_playing();
                Ok(Response::ok())
            }
            Command::RingtoneFile { path } => {
                let dir = ringtone_dir(self.inner.settings_path.as_deref())
                    .ok_or_else(|| PhoneError::Refused("this phone keeps no files".into()))?;
                let (name, tone) = ring::add_file(std::path::Path::new(&path), &dir)
                    .map_err(PhoneError::Refused)?;
                {
                    let mut files = self.inner.file_ringtones.lock();
                    files.retain(|(n, _)| *n != name);
                    files.push((name.clone(), tone));
                    files.sort_by_key(|a| a.0.to_lowercase());
                }
                self.inner.local.lock().ringtone = Some(ring::file_id(&name));
                self.inner.save_settings();
                self.read_audio_devices().await;
                self.inner.preview().await;
                Ok(Response::ok())
            }
            Command::RemoveRingtone { id } => {
                let name = id
                    .strip_prefix("file:")
                    .filter(|n| self.inner.file_ringtones.lock().iter().any(|(f, _)| f == n))
                    .ok_or_else(|| {
                        PhoneError::Refused(format!("no ringtone {id:?} added from a file"))
                    })?
                    .to_string();
                self.inner.file_ringtones.lock().retain(|(n, _)| *n != name);
                if let Some(dir) = ringtone_dir(self.inner.settings_path.as_deref()) {
                    if let Err(e) = std::fs::remove_file(dir.join(&name)) {
                        tracing::warn!(%e, ringtone = %name, "the ringtone's file not removed");
                    }
                }
                {
                    let mut local = self.inner.local.lock();
                    if local.ringtone.as_deref() == Some(id.as_str()) {
                        local.ringtone = None;
                    }
                }
                self.inner.save_settings();
                self.read_audio_devices().await;
                Ok(Response::ok())
            }
            Command::TestMicrophone => {
                self.start_audio_test(AudioTestStage::Recording)?;
                Ok(Response::ok())
            }
            Command::TestSpeaker { ring } => {
                self.start_audio_test(if ring {
                    AudioTestStage::Ring
                } else {
                    AudioTestStage::Speaker
                })?;
                Ok(Response::ok())
            }
            Command::StopTest => {
                self.inner.stop_audio_test();
                Ok(Response::ok())
            }
            Command::SetCapture { mode } => {
                let allowed = self.inner.state.lock().capture.allowed;
                if !allowed && mode != CaptureMode::Off {
                    return Err(PhoneError::Refused(
                        "your organisation does not allow this app to capture calls".into(),
                    ));
                }
                self.inner.update_capture(|c| c.mode = mode);
                Ok(Response::ok())
            }
            Command::DeleteCaptures => {
                let dir = capture::folder(self.inner.settings_path.as_deref())
                    .ok_or_else(|| PhoneError::Refused("this phone keeps no captures".into()))?;
                // One still being written is left on Windows (it is open)
                // and unlinked elsewhere; either way its upload finds it
                // gone or not.
                capture::delete_all(&dir);
                self.inner.update_capture(|_| {});
                Ok(Response::ok())
            }
            Command::Record { call, on } => {
                let id = self.pick(call, "no call to record", |c| {
                    c.state == CallState::Connected
                })?;
                let fcp = self.fcp("recording a call")?;
                let server_call = self
                    .inner
                    .state
                    .lock()
                    .call(id)
                    .and_then(|c| c.server_call.clone())
                    .ok_or_else(|| {
                        PhoneError::Refused("the server did not name this call".into())
                    })?;
                // FCP answers once the parties have heard that the call
                // is recorded and the recorder runs.
                if on {
                    fcp.start_recording(&server_call).await?;
                } else {
                    fcp.stop_recording(&server_call).await?;
                }
                self.update_call(id, |c| c.recording = on, || unreachable_view(id));
                Ok(Response::ok())
            }
            Command::SaveContact { mut contact } => {
                contact.name = contact.name.trim().to_string();
                if contact.name.is_empty() {
                    return Err(PhoneError::Refused("a contact needs a name".into()));
                }
                for n in &mut contact.numbers {
                    n.number = n.number.trim().to_string();
                    n.label = n.label.trim().to_string();
                }
                contact.numbers.retain(|n| !n.number.is_empty());
                for field in [&mut contact.company, &mut contact.email, &mut contact.notes] {
                    *field = field
                        .take()
                        .map(|v| v.trim().to_string())
                        .filter(|v| !v.is_empty());
                }
                let on_device = !contact.id.is_empty()
                    && self
                        .inner
                        .local
                        .lock()
                        .contacts
                        .iter()
                        .any(|c| c.id == contact.id);
                let on_account = !contact.id.is_empty()
                    && self
                        .inner
                        .account_contacts
                        .lock()
                        .iter()
                        .any(|c| c.id == contact.id);
                if on_account && !contact.on_account {
                    return Err(PhoneError::Refused(
                        "a contact on the account stays on the account".into(),
                    ));
                }
                if contact.on_account {
                    let fcp = self.fcp("contacts on the account")?;
                    let fields = data::contact_fields(&contact);
                    let kept = if on_account {
                        fcp.update_contact(&contact.id, &fields).await?
                    } else {
                        fcp.create_contact(&fields).await?
                    };
                    self.inner.keep_account_contact(data::contact(&kept));
                    // Moved to the account: the device's copy goes once
                    // FCP has it.
                    if on_device {
                        self.inner
                            .local
                            .lock()
                            .contacts
                            .retain(|c| c.id != contact.id);
                        self.inner.save_settings();
                    }
                    self.inner.show_contacts();
                    return Ok(Response::ok());
                }
                if contact.id.is_empty() {
                    contact.id = new_contact_id();
                }
                {
                    let mut local = self.inner.local.lock();
                    match local.contacts.iter_mut().find(|c| c.id == contact.id) {
                        Some(kept) => *kept = contact,
                        None => local.contacts.push(contact),
                    }
                    local.contacts.sort_by_key(|c| c.name.to_lowercase());
                }
                self.inner.save_settings();
                self.inner.show_contacts();
                Ok(Response::ok())
            }
            Command::DeleteContact { id } => {
                let on_account = self
                    .inner
                    .account_contacts
                    .lock()
                    .iter()
                    .any(|c| c.id == id);
                if on_account {
                    self.fcp("contacts on the account")?
                        .delete_contact(&id)
                        .await?;
                    self.inner.account_contacts.lock().retain(|c| c.id != id);
                } else {
                    {
                        let mut local = self.inner.local.lock();
                        let before = local.contacts.len();
                        local.contacts.retain(|c| c.id != id);
                        if local.contacts.len() == before {
                            return Err(PhoneError::Refused(format!("no contact {id}")));
                        }
                    }
                    self.inner.save_settings();
                }
                self.inner.show_contacts();
                Ok(Response::ok())
            }
            Command::ImportContacts { path, on_account } => {
                let file = tokio::fs::read(&path)
                    .await
                    .map_err(|e| PhoneError::Refused(format!("{path}: {e}")))?;
                if file.len() > CONTACT_FILE_LIMIT {
                    return Err(PhoneError::Refused(format!(
                        "{path} is too big for a contacts file (10 MB at most)"
                    )));
                }
                let imported = if on_account {
                    let fcp = self.fcp("contacts on the account")?;
                    let done = fcp.import_contacts(file).await?;
                    data::refresh_contacts(&self.inner, fcp).await;
                    Imported {
                        imported: done.imported as u32,
                        duplicates: done.duplicates as u32,
                        skipped: done
                            .skipped
                            .into_iter()
                            .map(|s| format!("entry {}: {}", s.entry, s.reason))
                            .collect(),
                    }
                } else {
                    let imported = self.inner.import_to_device(&file);
                    self.inner.save_settings();
                    self.inner.show_contacts();
                    imported
                };
                Ok(Response {
                    imported: Some(imported),
                    ..Response::ok()
                })
            }
            Command::ExportContacts { path } => {
                let format = if path.to_ascii_lowercase().ends_with(".csv") {
                    contact_file::Format::Csv
                } else {
                    contact_file::Format::VCard
                };
                let contacts = self.inner.state.lock().contacts.clone();
                tokio::fs::write(&path, contact_file::write(format, &contacts))
                    .await
                    .map_err(|e| PhoneError::Refused(format!("{path}: {e}")))?;
                Ok(Response::ok())
            }
            Command::TakeAppearance { line, index } => {
                let (aor, remote) = {
                    let state = self.inner.state.lock();
                    let found = state
                        .lines
                        .iter()
                        .find(|l| l.line == line && l.shared)
                        .ok_or_else(|| PhoneError::Refused(format!("no shared line {line}")))?;
                    let remote = found
                        .in_use
                        .iter()
                        .find(|a| a.index == index)
                        .and_then(|a| a.remote.clone());
                    (found.aor.clone(), remote)
                };
                self.hold_the_others(None).await?;
                let call = self.anvil().await?.take_appearance(&aor, index).await?;
                // Shown as the far party the line named, until the call
                // says otherwise.
                let shown = remote.unwrap_or_else(|| aor.clone());
                self.update_call(
                    call.0,
                    |c| c.line = Some(line.clone()),
                    || CallView {
                        line: Some(line.clone()),
                        ..CallView::new(call.0, Direction::Outgoing, &shown, CallState::Dialing)
                    },
                );
                Ok(Response {
                    call: Some(call.0),
                    ..Response::ok()
                })
            }
            Command::RenameLine { line, label } => {
                let fcp = Arc::clone(self.fcp("naming lines")?);
                fcp.set_line_label(&line, label.trim()).await?;
                data::refresh_softphone(&self.inner, &fcp).await;
                Ok(Response::ok())
            }
            Command::DirectoryListed { on } => {
                let fcp = self.fcp("the directory")?;
                fcp.set_directory_listed(on).await?;
                let may_change = self
                    .inner
                    .state
                    .lock()
                    .directory
                    .is_some_and(|d| d.may_change);
                self.inner.set_directory(Some(DirectoryListing {
                    listed: on,
                    may_change,
                }));
                Ok(Response::ok())
            }
            Command::Presence {
                status,
                note,
                until,
            } => {
                let fcp = self.fcp("presence")?;
                let status = self.status_id(&status)?;
                let now = fcp
                    .set_presence(&anvil_fcp::PresenceUpdate {
                        status: Some(status),
                        note: Some(note),
                        until: Some(until),
                        idle: None,
                    })
                    .await?;
                self.inner.set_presence(Some(now.into()));
                Ok(Response::ok())
            }
            Command::PresenceNote { note } => {
                let fcp = self.fcp("presence")?;
                let now = fcp
                    .set_presence(&anvil_fcp::PresenceUpdate {
                        note: Some(note),
                        ..Default::default()
                    })
                    .await?;
                self.inner.set_presence(Some(now.into()));
                Ok(Response::ok())
            }
            Command::PresenceIdle { idle } => {
                let fcp = self.fcp("presence")?;
                let now = fcp
                    .set_presence(&anvil_fcp::PresenceUpdate {
                        idle: Some(idle),
                        ..Default::default()
                    })
                    .await?;
                self.inner.set_presence(Some(now.into()));
                Ok(Response::ok())
            }
            Command::StatusAdd { label, state } => {
                let fcp = self.fcp("presence")?;
                fcp.create_status(&label, &state).await?;
                data::refresh_presence(&self.inner, fcp).await;
                Ok(Response::ok())
            }
            Command::StatusEdit {
                status,
                label,
                state,
            } => {
                let fcp = self.fcp("presence")?;
                let id = self.own_status_id(&status)?;
                fcp.update_status(&id, label.as_deref(), state.as_deref())
                    .await?;
                data::refresh_presence(&self.inner, fcp).await;
                Ok(Response::ok())
            }
            Command::StatusRemove { status } => {
                let fcp = self.fcp("presence")?;
                let id = self.own_status_id(&status)?;
                fcp.delete_status(&id).await?;
                data::refresh_presence(&self.inner, fcp).await;
                Ok(Response::ok())
            }
            Command::StatusOrder { ids } => {
                let fcp = self.fcp("presence")?;
                fcp.order_statuses(&ids).await?;
                data::refresh_presence(&self.inner, fcp).await;
                Ok(Response::ok())
            }
            Command::Favourite { who, on } => {
                let key = self.person_key(&who)?;
                {
                    let mut local = self.inner.local.lock();
                    if on {
                        local.favourites.insert(key.clone());
                    } else {
                        local.favourites.remove(&key);
                    }
                }
                self.inner.save_settings();
                self.inner.update_person_by_key(&key, |p| p.favourite = on);
                Ok(Response::ok())
            }
            Command::ClearRecents => {
                let (account, recents) = {
                    let state = self.inner.state.lock();
                    (state.aor.clone(), state.recents.clone())
                };
                self.inner
                    .local
                    .lock()
                    .clear_recents(&account, recents.iter().map(|r| r.started_at.as_str()));
                self.inner.save_settings();
                self.inner.set_recents(recents);
                Ok(Response::ok())
            }
            Command::HideRecent { id } => {
                self.inner.local.lock().hidden_recents.insert(id);
                self.inner.save_settings();
                let recents = self.inner.state.lock().recents.clone();
                self.inner.set_recents(recents);
                Ok(Response::ok())
            }
            Command::Audio {
                kind: AudioKind::Ring,
                device,
            } => {
                self.anvil().await?.choose_ring_device(device.as_deref())?;
                self.inner.local.lock().ring_output = device;
                self.inner.save_settings();
                // Ringing now: on the new speaker.
                self.inner.stop_sound();
                self.inner.sound().await;
                self.read_audio_devices().await;
                Ok(Response::ok())
            }
            Command::Volume {
                microphone,
                speaker,
            } => {
                let anvil = self.anvil().await?;
                let Some(mut volume) = anvil.audio_volume() else {
                    return Err(PhoneError::Refused(
                        "this phone's volume is set by the system".into(),
                    ));
                };
                if let Some(percent) = microphone {
                    volume.microphone = percent;
                }
                if let Some(percent) = speaker {
                    volume.speaker = percent;
                }
                anvil.set_audio_volume(volume)?;
                drop(anvil);
                {
                    let mut local = self.inner.local.lock();
                    local.microphone_volume = volume.microphone;
                    local.speaker_volume = volume.speaker;
                }
                self.inner.save_settings();
                self.read_audio_devices().await;
                Ok(Response::ok())
            }
            Command::Processing {
                echo_cancellation,
                noise_suppression,
            } => {
                let anvil = self.anvil().await?;
                let Some(mut processing) = anvil.audio_processing() else {
                    return Err(PhoneError::Refused(
                        "this phone's audio does no echo cancellation or noise suppression of its own"
                            .into(),
                    ));
                };
                if let Some(on) = echo_cancellation {
                    processing.echo_cancellation = on;
                }
                if let Some(on) = noise_suppression {
                    processing.noise_suppression = on;
                }
                anvil.set_audio_processing(processing)?;
                drop(anvil);
                {
                    let mut local = self.inner.local.lock();
                    local.echo_cancellation = processing.echo_cancellation;
                    local.noise_suppression = processing.noise_suppression;
                }
                self.inner.save_settings();
                self.read_audio_devices().await;
                Ok(Response::ok())
            }
            Command::Ringtone { id } => {
                if let Some(id) = &id {
                    if self.inner.tone(id).is_none() {
                        return Err(PhoneError::Refused(format!("no ringtone {id:?}")));
                    }
                }
                self.inner.local.lock().ringtone = id;
                self.inner.save_settings();
                self.read_audio_devices().await;
                self.inner.preview().await;
                Ok(Response::ok())
            }
            Command::Audio { kind, device } => {
                let (mut input, mut output) = self.anvil().await?.chosen_audio_devices();
                match kind {
                    AudioKind::Input => input = device,
                    AudioKind::Output | AudioKind::Ring => output = device,
                }
                self.anvil()
                    .await?
                    .choose_audio_devices(input.as_deref(), output.as_deref())?;
                {
                    let mut local = self.inner.local.lock();
                    local.audio_input = input;
                    local.audio_output = output;
                }
                self.inner.save_settings();
                self.read_audio_devices().await;
                Ok(Response::ok())
            }
            Command::Refresh => {
                let fcp = Arc::clone(self.fcp("the user's data")?);
                data::refresh_all(&self.inner, &fcp).await;
                self.read_audio_devices().await;
                Ok(Response::ok())
            }
            Command::Status => Ok(Response {
                state: Some(self.state()),
                ..Response::ok()
            }),
            Command::Subscribe => Err(PhoneError::Refused(
                "subscribe is for the control socket".into(),
            )),
        }
    }

    /// A voicemail message's audio (WAV), to play.
    pub async fn voicemail_audio(&self, id: &str) -> Result<Vec<u8>, PhoneError> {
        Ok(self.fcp("voicemail")?.voicemail_audio(id).await?)
    }

    async fn mark_heard(&self, id: &str) -> Result<(), PhoneError> {
        self.fcp("voicemail")?
            .set_voicemail_status(id, "heard")
            .await?;
        self.inner.update_voicemail(|list| {
            if let Some(m) = list.iter_mut().find(|m| m.id == id) {
                m.new = false;
            }
        });
        Ok(())
    }

    /// Stop the message playing, if one is.
    fn stop_playing(&self) {
        if let Some(task) = self.inner.playback.lock().take() {
            task.abort();
        }
        self.inner.set_playing(None);
    }

    /// Stop the phone: its calls end, it unregisters.
    pub async fn shutdown(&self) {
        self.stop_playing();
        self.inner.stop_sound();
        if let Some(task) = self.inner.events.lock().take() {
            task.abort();
        }
        if let Some(task) = self.inner.fcp_events.lock().take() {
            task.abort();
        }
        if let Some(anvil) = self.inner.anvil.write().await.take() {
            let _ = anvil.shutdown().await;
        }
    }

    /// The Anvil, while the phone runs.
    async fn anvil(&self) -> Result<tokio::sync::RwLockReadGuard<'_, Anvil>, PhoneError> {
        tokio::sync::RwLockReadGuard::try_map(self.inner.anvil.read().await, |a| a.as_ref())
            .map_err(|_| PhoneError::Refused("the phone has stopped".into()))
    }

    /// The call a command means: the one named, or the only one that fits.
    fn pick(
        &self,
        call: Option<u64>,
        none: &str,
        fits: impl Fn(&CallView) -> bool,
    ) -> Result<u64, PhoneError> {
        let state = self.inner.state.lock();
        match call {
            Some(id) => state
                .call(id)
                .map(|c| c.id)
                .ok_or_else(|| PhoneError::Refused(format!("no call {id}"))),
            None => {
                let fitting: Vec<u64> = state
                    .calls
                    .iter()
                    .filter(|c| fits(c))
                    .map(|c| c.id)
                    .collect();
                match fitting.as_slice() {
                    [id] => Ok(*id),
                    [] => Err(PhoneError::Refused(none.to_string())),
                    _ => Err(PhoneError::Refused(format!(
                        "{} calls fit; name one by its id",
                        fitting.len()
                    ))),
                }
            }
        }
    }

    /// A bare number or name dialled in the account's domain.
    fn dial_target(&self, target: &str) -> String {
        dial_target(target, &self.inner.domain)
    }

    fn update_call(
        &self,
        id: u64,
        change: impl FnOnce(&mut CallView),
        new: impl FnOnce() -> CallView,
    ) {
        update_call(&self.inner, id, change, new);
    }

    /// FCP, for what needs it.
    fn fcp(&self, what: &str) -> Result<&Arc<FcpClient>, PhoneError> {
        self.inner
            .fcp
            .as_ref()
            .ok_or_else(|| PhoneError::Refused(format!("{what} needs a phone signed in to FCP")))
    }

    /// Hold every connected call but `except`: a second call placed or
    /// answered puts the first on hold, as a desk phone does.
    async fn hold_the_others(&self, except: Option<u64>) -> Result<(), PhoneError> {
        let others: Vec<u64> = self
            .inner
            .state
            .lock()
            .calls
            .iter()
            .filter(|c| Some(c.id) != except && c.state == CallState::Connected && !c.held)
            .map(|c| c.id)
            .collect();
        for id in others {
            self.anvil().await?.hold(CallId(id), true).await?;
            self.update_call(id, |c| c.held = true, || unreachable_view(id));
        }
        Ok(())
    }

    /// A person by key, extension or name, as the directory has them.
    /// A status as FCP names it: a built-in one, or one of the user's own
    /// by id or label.
    fn status_id(&self, given: &str) -> Result<String, PhoneError> {
        let given = given.trim();
        let builtin = ["available", "away", "busy", "dnd"];
        if let Some(b) = builtin.iter().find(|b| b.eq_ignore_ascii_case(given)) {
            return Ok(b.to_string());
        }
        self.own_status_id(given)
    }

    /// One of the user's own statuses, by id or label.
    fn own_status_id(&self, given: &str) -> Result<String, PhoneError> {
        let state = self.inner.state.lock();
        let own = state
            .presence
            .as_ref()
            .map(|p| p.statuses.as_slice())
            .unwrap_or_default();
        own.iter()
            .find(|s| s.id == given)
            .or_else(|| own.iter().find(|s| s.label.eq_ignore_ascii_case(given)))
            .map(|s| s.id.clone())
            .ok_or_else(|| PhoneError::Refused(format!("no status {given:?}")))
    }

    fn person_key(&self, who: &str) -> Result<String, PhoneError> {
        let state = self.inner.state.lock();
        state
            .people
            .iter()
            .find(|p| p.key == who || p.extension.as_deref() == Some(who))
            .or_else(|| {
                let mut named = state
                    .people
                    .iter()
                    .filter(|p| p.name.eq_ignore_ascii_case(who));
                match (named.next(), named.next()) {
                    (Some(p), None) => Some(p),
                    _ => None,
                }
            })
            .map(|p| p.key.clone())
            .ok_or_else(|| PhoneError::Refused(format!("{who:?} is not in the directory")))
    }

    /// The audio devices as the host has them now.
    /// Start an audio test (ending one under way): refused while a call is
    /// up or ringing, and a ringing test on a phone that does not ring.
    fn start_audio_test(&self, stage: AudioTestStage) -> Result<(), PhoneError> {
        if !self.inner.state.lock().calls.is_empty() {
            return Err(PhoneError::Refused(
                "not while a call is up or ringing".into(),
            ));
        }
        if stage == AudioTestStage::Ring && !self.inner.ring {
            return Err(PhoneError::Refused(
                "this phone does not ring itself".into(),
            ));
        }
        self.inner.stop_audio_test();
        let inner = Arc::clone(&self.inner);
        let task = tokio::spawn(async move {
            if let Err(e) = inner.run_audio_test(stage).await {
                tracing::warn!(%e, "the audio test failed");
            }
            inner.set_audio_test(None);
        });
        *self.inner.audio_test.lock() = Some(task.abort_handle());
        Ok(())
    }

    async fn read_audio_devices(&self) {
        let Ok(anvil) = self.anvil().await else {
            return;
        };
        let (input, output) = anvil.chosen_audio_devices();
        let mut audio = AudioDevices {
            input,
            output,
            ring: anvil.chosen_ring_device(),
            ringtones: self.inner.ringtones(),
            ringtone: self.inner.ringtone(),
            processing: anvil.audio_processing().map(|p| AudioProcessing {
                echo_cancellation: p.echo_cancellation,
                noise_suppression: p.noise_suppression,
            }),
            volume: anvil.audio_volume().map(|v| AudioVolume {
                microphone: v.microphone,
                speaker: v.speaker,
            }),
            ..Default::default()
        };
        for d in anvil.audio_devices() {
            let device = AudioDevice {
                id: d.id,
                name: d.name,
                default: d.is_default,
            };
            if d.is_input {
                audio.inputs.push(device);
            } else {
                audio.outputs.push(device);
            }
        }
        drop(anvil);
        let changed = {
            let mut state = self.inner.state.lock();
            let changed = state.audio != audio;
            state.audio = audio.clone();
            changed
        };
        if changed {
            let _ = self.inner.changes.send(Change::Audio { audio });
        }
    }
}

impl Inner {
    /// The ringtones to choose from: Anvil's, then the brand's.
    fn ringtones(&self) -> Vec<Ringtone> {
        let own = ring::BUILT_IN.iter().map(|(id, name)| Ringtone {
            id: id.to_string(),
            name: name.to_string(),
        });
        let brand: Vec<Ringtone> = self
            .brand_ringtones
            .lock()
            .iter()
            .map(|(name, _, _)| Ringtone {
                id: ring::brand_id(name),
                name: name.clone(),
            })
            .collect();
        let files: Vec<Ringtone> = self
            .file_ringtones
            .lock()
            .iter()
            .map(|(name, _)| Ringtone {
                id: ring::file_id(name),
                name: name.clone(),
            })
            .collect();
        own.chain(brand).chain(files).collect()
    }

    /// The ringtone that rings: the one chosen while it is there, else the
    /// brand's default, else Anvil's.
    fn ringtone(&self) -> String {
        let chosen = self.local.lock().ringtone.clone();
        if let Some(id) = chosen.filter(|id| self.tone(id).is_some()) {
            return id;
        }
        self.brand_ringtones
            .lock()
            .iter()
            .find(|(_, default, _)| *default)
            .map(|(name, _, _)| ring::brand_id(name))
            .unwrap_or_else(|| ring::DEFAULT.to_string())
    }

    /// The ringtone for the call ringing in: its contact's while this
    /// device has that one, else the phone's.
    fn ringtone_ringing_in(&self) -> String {
        let contacts = {
            let state = self.state.lock();
            state
                .calls
                .iter()
                .find(|c| c.direction == Direction::Incoming && c.state == CallState::Ringing)
                .and_then(|call| state.contacts.iter().find(|c| c.has_number(&call.remote)))
                .and_then(|c| c.ringtone.clone())
        };
        contacts
            .filter(|id| self.tone(id).is_some())
            .unwrap_or_else(|| self.ringtone())
    }

    /// A ringtone's sound, by id.
    fn tone(&self, id: &str) -> Option<ring::Tone> {
        if let Some(name) = id.strip_prefix("file:") {
            return self
                .file_ringtones
                .lock()
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, tone)| tone.clone());
        }
        match id.strip_prefix("brand:") {
            Some(name) => self
                .brand_ringtones
                .lock()
                .iter()
                .find(|(n, _, _)| n == name)
                .map(|(_, _, tone)| tone.clone()),
            None => ring::built_in(id),
        }
    }

    /// Stop the audio test under way, if any.
    fn stop_audio_test(&self) {
        if let Some(task) = self.audio_test.lock().take() {
            task.abort();
            self.set_audio_test(None);
        }
    }

    fn set_audio_test(&self, test: Option<AudioTest>) {
        let changed = {
            let mut state = self.state.lock();
            let changed = state.audio_test != test;
            state.audio_test = test;
            changed
        };
        if changed {
            let _ = self.changes.send(Change::AudioTest { test });
        }
    }

    /// An audio test, to its end: the microphone recorded with its level
    /// shown then heard back, or a sound on a speaker.
    async fn run_audio_test(&self, stage: AudioTestStage) -> Result<(), PhoneError> {
        /// How long the microphone is recorded.
        const LENGTH: std::time::Duration = std::time::Duration::from_secs(5);
        const RATE: u32 = 16_000;
        let show = |stage, level| self.set_audio_test(Some(AudioTest { stage, level }));
        show(stage, 0);
        let guard = self.anvil.read().await;
        let Some(anvil) = guard.as_ref() else {
            return Ok(());
        };
        let (samples, rate, ring) = match stage {
            AudioTestStage::Recording | AudioTestStage::PlayingBack => {
                // The meter moves ten times a second, at the loudest heard.
                let mut loudest = 0f32;
                let mut shown = std::time::Instant::now();
                let recorded = anvil
                    .record_microphone(LENGTH, RATE, |level| {
                        loudest = loudest.max(level);
                        if shown.elapsed() >= std::time::Duration::from_millis(100) {
                            show(AudioTestStage::Recording, meter(loudest));
                            loudest = 0.0;
                            shown = std::time::Instant::now();
                        }
                    })
                    .await?;
                show(AudioTestStage::PlayingBack, 0);
                (recorded, RATE, false)
            }
            AudioTestStage::Speaker => {
                let tone = ring::built_in(ring::DEFAULT).expect("Anvil's own ringtone");
                (tone.clip.samples, tone.clip.sample_rate, false)
            }
            AudioTestStage::Ring => {
                let tone = self
                    .tone(&self.ringtone())
                    .or_else(|| ring::built_in(ring::DEFAULT))
                    .expect("Anvil's own ringtone");
                (tone.clip.samples, tone.clip.sample_rate, true)
            }
        };
        let length = std::time::Duration::from_millis(samples.len() as u64 * 1000 / rate as u64);
        // Stopped with the test: aborting this task drops the guard.
        let playing = AbortOnDrop(if ring {
            anvil.ring(samples, rate, std::time::Duration::from_millis(300), false)?
        } else {
            anvil.play(samples, rate)?
        });
        drop(guard);
        tokio::time::sleep(length + std::time::Duration::from_millis(300)).await;
        drop(playing);
        Ok(())
    }

    /// Stop what is sounding.
    fn stop_sound(&self) {
        if let Some((_, task)) = self.ringer.lock().take() {
            task.abort();
        }
    }

    /// Sound what the calls want: a ring, the call waiting tone, or
    /// nothing (a preview playing on unless a call rings).
    async fn sound(&self) {
        // A call stops an audio test: the speaker and microphone are its.
        if !self.state.lock().calls.is_empty() {
            self.stop_audio_test();
        }
        if !self.ring {
            return;
        }
        let _deciding = self.sounding.lock().await;
        let wanted = ring::wanted(&self.state.lock().calls);
        let now = self.ringer.lock().as_ref().map(|(sound, _)| *sound);
        if now == wanted || (wanted.is_none() && now == Some(ring::Sound::Preview)) {
            return;
        }
        self.stop_sound();
        let Some(sound) = wanted else { return };
        let tone = match sound {
            ring::Sound::Waiting => ring::waiting(),
            _ => self
                .tone(&self.ringtone_ringing_in())
                .or_else(|| ring::built_in(ring::DEFAULT))
                .expect("Anvil's own ringtone"),
        };
        let anvil = self.anvil.read().await;
        let Some(anvil) = anvil.as_ref() else { return };
        match anvil.ring(
            tone.clip.samples,
            tone.clip.sample_rate,
            tone.gap,
            sound == ring::Sound::Waiting,
        ) {
            Ok(task) => *self.ringer.lock() = Some((sound, task.abort_handle())),
            Err(e) => tracing::warn!(%e, "not ringing"),
        }
    }

    /// The ringtone just chosen, heard once on the ringing speaker; not
    /// while a call is up or ringing.
    async fn preview(&self) {
        let _deciding = self.sounding.lock().await;
        if !self.ring || !self.state.lock().calls.is_empty() {
            return;
        }
        let Some(tone) = self.tone(&self.ringtone()) else {
            return;
        };
        self.stop_sound();
        let length = std::time::Duration::from_millis(
            tone.clip.samples.len() as u64 * 1000 / tone.clip.sample_rate as u64,
        );
        let anvil = self.anvil.read().await;
        let Some(anvil) = anvil.as_ref() else { return };
        let task = match anvil.ring(
            tone.clip.samples,
            tone.clip.sample_rate,
            std::time::Duration::from_millis(300),
            false,
        ) {
            Ok(task) => task.abort_handle(),
            Err(e) => {
                tracing::warn!(%e, "the ringtone not played");
                return;
            }
        };
        *self.ringer.lock() = Some((ring::Sound::Preview, task.clone()));
        // Once through, then quiet (a call ringing in meanwhile has
        // already replaced it).
        tokio::spawn(async move {
            tokio::time::sleep(length + std::time::Duration::from_millis(300)).await;
            task.abort();
        });
    }

    /// Show `profile`'s brand: FCP's, or the installed profile's.
    fn set_brand(&self, profile: &anvil_core::brand::BrandProfile) {
        let brand = Brand {
            app_name: profile.app_name.clone(),
            primary: profile.colors.primary.map(|c| c.to_hex()),
            accent: profile.colors.accent.map(|c| c.to_hex()),
            logo: profile
                .logo
                .as_ref()
                .map(|a| a.bytes.clone())
                .filter(|b| !b.is_empty()),
            icon: profile
                .icon
                .as_ref()
                .map(|a| a.bytes.clone())
                .filter(|b| !b.is_empty()),
        };
        let app_name = Some(brand.app_name.clone());
        self.state.lock().brand = Some(brand);
        let _ = self.changes.send(Change::Brand { app_name });
        self.set_brand_ringtones(&profile.ringtones);
    }

    fn set_brand_ringtones(&self, ringtones: &[anvil_core::brand::Ringtone]) {
        *self.brand_ringtones.lock() = ring::from_brand(ringtones);
    }

    fn set_dnd(&self, on: bool) {
        let changed = {
            let mut state = self.state.lock();
            let changed = state.dnd != Some(on);
            state.dnd = Some(on);
            changed
        };
        if changed {
            let _ = self.changes.send(Change::Dnd { on });
        }
    }

    fn set_playing(&self, id: Option<String>) {
        let changed = {
            let mut state = self.state.lock();
            let changed = state.playing != id;
            state.playing = id.clone();
            changed
        };
        if changed {
            let _ = self.changes.send(Change::Playing { id });
        }
    }

    fn set_calling(&self, settings: CallingSettings) {
        self.set_dnd(settings.dnd);
        let changed = {
            let mut state = self.state.lock();
            let changed = state.calling.as_ref() != Some(&settings);
            state.calling = Some(settings.clone());
            changed
        };
        if changed {
            let _ = self.changes.send(Change::Calling { settings });
        }
    }

    /// The person at `aor` (by its user part: their name or extension).
    fn update_person(&self, aor: &str, change: impl FnOnce(&mut Person)) {
        let user = data::user_of(aor);
        let key = self
            .state
            .lock()
            .people
            .iter()
            .find(|p| p.key == user || p.extension.as_deref() == Some(user))
            .map(|p| p.key.clone());
        if let Some(key) = key {
            self.update_person_by_key(&key, change);
        }
    }

    fn update_person_by_key(&self, key: &str, change: impl FnOnce(&mut Person)) {
        let person = {
            let mut state = self.state.lock();
            let Some(p) = state.people.iter_mut().find(|p| p.key == key) else {
                return;
            };
            let before = p.clone();
            change(p);
            (*p != before).then(|| p.clone())
        };
        if let Some(person) = person {
            let _ = self.changes.send(Change::Person { person });
        }
    }

    /// Show `recents`, less those cleared or removed here.
    /// Change the lines `which` picks, telling the app when any changed.
    fn update_line(&self, which: impl Fn(&LineView) -> bool, change: impl Fn(&mut LineView)) {
        let lines = {
            let mut state = self.state.lock();
            let before = state.lines.clone();
            for line in state.lines.iter_mut().filter(|l| which(l)) {
                change(line);
            }
            (state.lines != before).then(|| state.lines.clone())
        };
        if let Some(lines) = lines {
            let _ = self.changes.send(Change::Lines { lines });
        }
    }

    /// The lines FCP says the app has: a new line registered, a gone one
    /// let go, each keeping what is known of its registration and messages.
    pub(crate) async fn set_lines(&self, lines: Vec<anvil_fcp::SoftphoneLine>, may_rename: bool) {
        let (added, removed, changed) = {
            let mut state = self.state.lock();
            let own = state.registration;
            let own_mw = state.message_waiting;
            let before = std::mem::take(&mut state.lines);
            let next: Vec<LineView> = lines
                .iter()
                .map(|l| {
                    let kept = before.iter().find(|b| b.line == l.line && b.aor == l.aor);
                    let own_line = l.kind == "own";
                    LineView {
                        line: l.line.clone(),
                        kind: l.kind.clone(),
                        aor: l.aor.clone(),
                        label: l.label.clone(),
                        default_label: l.default_label.clone(),
                        registration: if own_line {
                            own
                        } else {
                            kept.map(|k| k.registration)
                                .unwrap_or(Registration::Unregistered)
                        },
                        message_waiting: if own_line {
                            own_mw
                        } else {
                            kept.and_then(|k| k.message_waiting)
                        },
                        ring: l.ring,
                        shared: l.shared,
                        appearances: l.appearances,
                        barge_in: l.barge_in,
                        in_use: kept.map(|k| k.in_use.clone()).unwrap_or_default(),
                    }
                })
                .collect();
            let others = |ls: &[LineView]| -> Vec<String> {
                ls.iter()
                    .filter(|l| l.kind != "own")
                    .map(|l| l.aor.clone())
                    .collect()
            };
            let (was, now) = (others(&before), others(&next));
            let added: Vec<String> = now.iter().filter(|a| !was.contains(a)).cloned().collect();
            let removed: Vec<String> = was.iter().filter(|a| !now.contains(a)).cloned().collect();
            let changed = next != before || state.may_rename_lines != may_rename;
            state.lines = next;
            state.may_rename_lines = may_rename;
            (added, removed, changed)
        };
        if changed {
            let lines = self.state.lock().lines.clone();
            let _ = self.changes.send(Change::Lines { lines });
        }
        if added.is_empty() && removed.is_empty() {
            return;
        }
        let anvil = self.anvil.read().await;
        let Some(anvil) = anvil.as_ref() else { return };
        for aor in removed {
            anvil.remove_line(&aor).await;
        }
        let shared: Vec<String> = lines
            .iter()
            .filter(|l| l.shared)
            .map(|l| l.aor.clone())
            .collect();
        for aor in added {
            if let Err(e) = anvil.add_line(&aor, shared.contains(&aor)).await {
                tracing::warn!(%e, %aor, "a line did not register");
            }
        }
    }

    /// What FCP says of capturing: the app's device, and whether it may.
    fn set_capture_allowed(&self, device_id: &str, allowed: &anvil_fcp::SoftphoneCapture) {
        *self.device_id.lock() = Some(device_id.to_string());
        self.update_capture(|c| {
            c.allowed = allowed.allowed;
            c.until = allowed.until.clone().filter(|_| allowed.allowed);
            c.max_mb = allowed.max_mb;
        });
    }

    /// Change what the app shows of captures, and tell Anvil which calls to
    /// capture now: only while FCP allows it and the user chose to.
    fn update_capture(&self, change: impl FnOnce(&mut CaptureState)) {
        let (before, now) = {
            let mut state = self.state.lock();
            let before = state.capture.clone();
            change(&mut state.capture);
            if let Some(dir) = capture::folder(self.settings_path.as_deref()) {
                (state.capture.saved, state.capture.saved_bytes) = capture::summary(&dir);
            }
            (before, state.capture.clone())
        };
        if self.local.lock().capture_calls != now.mode {
            self.local.lock().capture_calls = now.mode;
            self.save_settings();
        }
        let settings = match (
            now.allowed,
            now.mode,
            capture::folder(self.settings_path.as_deref()),
        ) {
            (true, CaptureMode::NextCall | CaptureMode::EveryCall, Some(dir)) => {
                Some(anvil_core::CaptureSettings {
                    dir,
                    max_bytes: u64::from(now.max_mb.max(1)) * 1024 * 1024,
                    every_call: now.mode == CaptureMode::EveryCall,
                    application: format!("Anvil {}", env!("CARGO_PKG_VERSION")),
                })
            }
            _ => None,
        };
        match self.anvil.try_read() {
            Ok(anvil) => {
                if let Some(anvil) = anvil.as_ref() {
                    if anvil.capture_settings() != settings {
                        anvil.set_capture(settings);
                    }
                }
            }
            Err(_) => tracing::debug!("captures not set: the phone is stopping"),
        }
        if before != now {
            let _ = self.changes.send(Change::Capture { capture: now });
        }
    }

    fn set_may_record(&self, on: bool) {
        let changed = std::mem::replace(&mut self.state.lock().may_record, on) != on;
        if changed {
            let _ = self.changes.send(Change::MayRecord { on });
        }
    }

    fn set_presence(&self, presence: Option<MyPresence>) {
        let changed = {
            let mut state = self.state.lock();
            let changed = state.presence != presence;
            state.presence = presence.clone();
            changed
        };
        if changed {
            let _ = self.changes.send(Change::Presence { presence });
        }
    }

    fn set_directory(&self, listing: Option<DirectoryListing>) {
        let changed = std::mem::replace(&mut self.state.lock().directory, listing) != listing;
        if changed {
            let _ = self.changes.send(Change::Directory { listing });
        }
    }

    /// Keep the contacts in a vCard or CSV file on this device, but those
    /// it already has by name and numbers, or the file has twice.
    fn import_to_device(&self, file: &[u8]) -> Imported {
        let text = String::from_utf8_lossy(file);
        let read = contact_file::read(contact_file::Format::guess(&text), &text);
        let mut imported = Imported {
            skipped: read
                .skipped
                .into_iter()
                .map(|(entry, why)| format!("entry {entry}: {why}"))
                .collect(),
            ..Imported::default()
        };
        let mut local = self.local.lock();
        let mut made: Vec<contact_file::ImportedContact> = Vec::new();
        for c in read.contacts {
            let kept = local
                .contacts
                .iter()
                .any(|k| c.same_as(&k.name, &k.numbers));
            if kept || made.iter().any(|m| m.same_as(&c.name, &c.numbers)) {
                imported.duplicates += 1;
                continue;
            }
            local.contacts.push(Contact {
                id: new_contact_id(),
                on_account: false,
                name: c.name.clone(),
                company: c.company.clone(),
                numbers: c.numbers.clone(),
                email: c.email.clone(),
                notes: c.notes.clone(),
                ringtone: None,
            });
            imported.imported += 1;
            made.push(c);
        }
        local.contacts.sort_by_key(|c| c.name.to_lowercase());
        imported
    }

    /// Show the device's contacts and the account's together, by name.
    fn show_contacts(&self) {
        let mut contacts = self.local.lock().contacts.clone();
        contacts.extend(self.account_contacts.lock().iter().cloned());
        contacts.sort_by_key(|c| c.name.to_lowercase());
        let count = contacts.len() as u32;
        self.state.lock().contacts = contacts;
        let _ = self.changes.send(Change::Contacts { count });
    }

    /// The account's contacts as FCP has them now.
    fn set_account_contacts(&self, contacts: Vec<Contact>) {
        *self.account_contacts.lock() = contacts;
        self.show_contacts();
    }

    /// One of the account's contacts as FCP kept it, new or changed.
    fn keep_account_contact(&self, contact: Contact) {
        let mut kept = self.account_contacts.lock();
        match kept.iter_mut().find(|c| c.id == contact.id) {
            Some(old) => *old = contact,
            None => kept.push(contact),
        }
    }

    fn set_recents(&self, mut recents: Vec<Recent>) {
        let missed = {
            let local = self.local.lock();
            let mut state = self.state.lock();
            let account = state.aor.clone();
            recents.retain(|r| !local.hides_recent(&account, &r.id, &r.started_at));
            state.recents = recents;
            state.recents.iter().filter(|r| r.missed).count() as u32
        };
        let _ = self.changes.send(Change::Recents { missed });
    }

    fn update_voicemail(&self, change: impl FnOnce(&mut Vec<Voicemail>)) {
        let (new, total) = {
            let mut state = self.state.lock();
            change(&mut state.voicemail);
            (
                state.voicemail.iter().filter(|m| m.new).count() as u32,
                state.voicemail.len() as u32,
            )
        };
        let _ = self.changes.send(Change::Voicemail { new, total });
    }

    fn save_settings(&self) {
        let Some(path) = &self.settings_path else {
            return;
        };
        let local = self.local.lock().clone();
        if let Err(e) = local.save(path) {
            tracing::warn!(%e, path = %path.display(), "settings not saved");
        }
    }
}

/// Where the ringtones added from files are kept: beside the settings.
fn ringtone_dir(settings_path: Option<&std::path::Path>) -> Option<PathBuf> {
    Some(settings_path?.parent()?.join("ringtones"))
}

/// A microphone's peak level (0.0–1.0) as a meter shows it, 0 to 100: by
/// decibels, from 60 below full scale, as a voice's level is heard.
fn meter(peak: f32) -> u8 {
    if peak <= 0.0 {
        return 0;
    }
    let db = 20.0 * peak.log10();
    (((db + 60.0) / 60.0).clamp(0.0, 1.0) * 100.0).round() as u8
}

/// A task aborted when this is dropped.
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn unreachable_view(id: u64) -> CallView {
    CallView::new(id, Direction::Outgoing, "", CallState::Connected)
}

fn update_call(
    inner: &Inner,
    id: u64,
    change: impl FnOnce(&mut CallView),
    new: impl FnOnce() -> CallView,
) {
    let view = {
        let mut state = inner.state.lock();
        match state.call_mut(id) {
            Some(call) => {
                change(call);
                call.clone()
            }
            None => {
                let mut view = new();
                change(&mut view);
                // Anvil started a capture as the call began, when captures
                // were on; one for the next call only is spent.
                view.capturing = inner
                    .anvil
                    .try_read()
                    .ok()
                    .and_then(|a| a.as_ref().map(|a| a.is_capturing(CallId(id))))
                    .unwrap_or(false);
                state.calls.push(view.clone());
                view
            }
        }
    };
    if view.capturing && inner.state.lock().capture.mode == CaptureMode::NextCall {
        inner.update_capture(|c| c.mode = CaptureMode::Off);
    }
    let _ = inner.changes.send(Change::Call { call: view });
}

/// A call's capture closed: the folder pruned (never this one) and the
/// capture uploaded, when signed in to FCP.
fn finished_capture(
    inner: &Arc<Inner>,
    done: anvil_core::FinishedCapture,
    call: Option<&CallView>,
) {
    tracing::info!(path = %done.path.display(), bytes = done.bytes, full = done.full, "capture finished");
    if let Some(dir) = done.path.parent() {
        let max_mb = inner.state.lock().capture.max_mb;
        capture::prune(
            dir,
            capture::keep_bytes(max_mb),
            Some(&done.path),
            std::time::SystemTime::now(),
        );
    }
    inner.update_capture(|_| {});
    let device = inner.device_id.lock().clone();
    if let (Some(fcp), Some(device)) = (inner.fcp.clone(), device) {
        let server_call = call.and_then(|c| c.server_call.clone());
        tokio::spawn(capture::upload(fcp, device, server_call, done.path));
    }
}

/// Follow Anvil's events into the state, one change each.
async fn follow(inner: std::sync::Weak<Inner>, mut events: EventStream) {
    while let Some(event) = events.recv().await {
        let Some(inner) = inner.upgrade() else { return };
        match event {
            Event::RegistrationChanged { state, reason } => {
                let state = Registration::from(state);
                inner.state.lock().registration = state;
                let _ = inner.changes.send(Change::Registration { state, reason });
                // The account's own line is the account.
                inner.update_line(|l| l.kind == "own", |l| l.registration = state);
            }
            Event::PushChanged { state } => {
                let push = Push::from(state);
                inner.state.lock().push = push.clone();
                let _ = inner.changes.send(Change::Push { push });
            }
            Event::IncomingCall {
                call,
                from,
                display_name,
                server_call,
                line,
            } => {
                // A line that does not ring this app shows its call, quietly.
                let silent = line.as_deref().is_some_and(|key| {
                    inner
                        .state
                        .lock()
                        .lines
                        .iter()
                        .any(|l| l.line == key && !l.ring)
                });
                let view = CallView {
                    display_name,
                    server_call,
                    line,
                    silent,
                    ..CallView::new(call.0, Direction::Incoming, &from, CallState::Ringing)
                };
                let v = view.clone();
                update_call(&inner, call.0, |_| {}, move || v);
            }
            Event::CallRinging { call } => update_call(
                &inner,
                call.0,
                |c| c.state = CallState::Ringing,
                || outgoing(call.0),
            ),
            Event::CallEstablished { call, codec } => {
                let (encrypted, server_call) = match inner.anvil.read().await.as_ref() {
                    Some(anvil) => (anvil.is_encrypted(call), anvil.server_call_id(call)),
                    None => (false, None),
                };
                update_call(
                    &inner,
                    call.0,
                    |c| {
                        c.state = CallState::Connected;
                        c.connected_at.get_or_insert_with(unix_now);
                        c.codec = Some(format!("{codec:?}").to_ascii_lowercase());
                        c.encrypted = encrypted;
                        // A call placed is named by the server's answer.
                        if c.server_call.is_none() {
                            c.server_call = server_call;
                        }
                    },
                    || outgoing(call.0),
                )
            }
            Event::MediaStats { call, stats } => {
                let quality = quality_of(&stats);
                let known = inner.state.lock().call(call.0).is_some();
                if known {
                    update_call(
                        &inner,
                        call.0,
                        |c| c.quality = Some(Box::new(quality)),
                        || outgoing(call.0),
                    );
                }
            }
            Event::CallEnded { call, reason } => {
                let ended = {
                    let mut state = inner.state.lock();
                    let ended = state.call(call.0).cloned();
                    state.calls.retain(|c| c.id != call.0);
                    ended
                };
                // The call's final figures, to FCP (CALL_DIAGNOSTICS §2): what
                // an administrator troubleshoots from, sent for every call.
                let report = ended
                    .as_ref()
                    .and_then(|c| data::quality_report(c, unix_now()));
                if let (Some(fcp), Some((server_call, report))) = (inner.fcp.clone(), report) {
                    tokio::spawn(async move {
                        if let Err(e) = fcp.report_call_quality(&server_call, &report).await {
                            tracing::debug!(%e, call = %server_call, "call quality not reported");
                        }
                    });
                }
                // The call's capture: closed, the folder kept within its
                // limits, uploaded under the server's id for the call.
                let finished = inner
                    .anvil
                    .read()
                    .await
                    .as_ref()
                    .and_then(|a| a.finish_capture(call));
                if let Some(done) = finished {
                    finished_capture(&inner, done, ended.as_ref());
                }
                let _ = inner.changes.send(Change::CallEnded {
                    id: call.0,
                    reason: end_reason(&reason),
                });
                // The call's record, once FCP has written it.
                if let Some(fcp) = inner.fcp.clone() {
                    let inner = Arc::clone(&inner);
                    tokio::spawn(async move {
                        tokio::time::sleep(data::RECORD_LAG).await;
                        data::refresh_recents(&inner, &fcp).await;
                    });
                }
            }
            Event::LineRegistration { aor, state, .. } => {
                let state = Registration::from(state);
                inner.update_line(
                    |l| l.kind != "own" && l.aor == aor,
                    |l| l.registration = state,
                );
            }
            Event::LineMessageWaiting { aor, summary } => {
                let mw = MessageWaiting {
                    waiting: summary.waiting,
                    new: summary.new,
                    old: summary.old,
                };
                inner.update_line(
                    |l| l.kind != "own" && l.aor.eq_ignore_ascii_case(&aor),
                    |l| l.message_waiting = Some(mw),
                );
            }
            Event::LineAppearances { aor, appearances } => {
                let in_use: Vec<AppearanceView> = appearances
                    .into_iter()
                    .map(|a| AppearanceView {
                        index: a.index,
                        state: a.state,
                        remote: a.remote,
                    })
                    .collect();
                inner.update_line(
                    |l| l.shared && l.aor.eq_ignore_ascii_case(&aor),
                    |l| l.in_use = in_use.clone(),
                );
            }
            Event::MessageWaiting { summary } => {
                let mw = MessageWaiting {
                    waiting: summary.waiting,
                    new: summary.new,
                    old: summary.old,
                };
                inner.state.lock().message_waiting = Some(mw);
                let _ = inner.changes.send(Change::MessageWaiting {
                    message_waiting: mw,
                });
            }
            Event::BrandUpdated { profile } => {
                inner.set_brand(&profile);
                Phone {
                    inner: Arc::clone(&inner),
                }
                .read_audio_devices()
                .await;
            }
            Event::BrandCleared => {
                match &inner.fallback_brand {
                    Some(fallback) => inner.set_brand(fallback),
                    None => {
                        inner.state.lock().brand = None;
                        let _ = inner.changes.send(Change::Brand { app_name: None });
                        inner.set_brand_ringtones(&[]);
                    }
                }
                Phone {
                    inner: Arc::clone(&inner),
                }
                .read_audio_devices()
                .await;
            }
            Event::TransferProgress { call, code, reason } => {
                let _ = inner.changes.send(Change::TransferProgress {
                    id: call.0,
                    code,
                    reason,
                });
            }
            _ => {}
        }
        // A call rang in, was answered or went: ring, or stop.
        inner.sound().await;
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn outgoing(id: u64) -> CallView {
    CallView::new(id, Direction::Outgoing, "", CallState::Dialing)
}

/// A second's media stats as the app shows them.
fn quality_of(stats: &anvil_core::MediaStats) -> Quality {
    let c = &stats.call;
    let permille = |pct: f32| (pct * 10.0).round() as u32;
    let hundredths = |m: anvil_core::quality::Mos| (m.score * 100.0).round() as u16;
    Quality {
        jitter_ms: c.jitter_ms.round() as u32,
        packet_loss_permille: permille(c.loss_pct()),
        rtt_ms: c.rtt_ms.map(|r| r.round() as u32),
        packets_received: c.receive.received,
        packets_played: c.receive.played,
        packets_dropped: c.receive.dropped(),
        unexpected_pt: c
            .receive
            .last_unexpected_pt
            .map(|pt| (pt, c.receive.expected_pt)),
        last_source: c.receive.last_source,
        recent_loss_permille: permille(stats.recent_loss_pct),
        packets_sent: c.packets_sent,
        bytes_sent: c.bytes_sent,
        rtp_received: c.packets_received,
        bytes_received: c.bytes_received,
        packets_expected: c.packets_expected,
        packets_lost: c.packets_lost,
        send_kbps: stats.send_kbps.round() as u32,
        recv_kbps: stats.recv_kbps.round() as u32,
        send_pps: stats.send_pps.round() as u32,
        recv_pps: stats.recv_pps.round() as u32,
        far_loss_permille: c.remote.map(|r| permille(r.fraction_lost_pct)),
        far_jitter_ms: c.remote.map(|r| r.jitter_ms.round() as u32),
        far_lost: c.remote.map(|r| i64::from(r.cumulative_lost)),
        rtcp: c.rtcp,
        rtcp_sent: c.rtcp_sent,
        rtcp_received: c.rtcp_received,
        clock_rate: c.clock_rate,
        ptime_ms: c.ptime_ms,
        local_rtp: c.local_rtp,
        remote_rtp: Some(c.remote_rtp),
        mos_hundredths: hundredths(stats.mos),
        far_mos_hundredths: stats.far_mos.map(hundredths),
    }
}

fn end_reason(reason: &EndReason) -> String {
    match reason {
        EndReason::LocalHangup => "hung up".into(),
        EndReason::RemoteHangup => "the other side hung up".into(),
        EndReason::Rejected(code) => format!("refused ({code})"),
        EndReason::Timeout => "no answer".into(),
        EndReason::MediaFailure(e) => format!("media failed: {e}"),
        EndReason::Error(e) => e.clone(),
        EndReason::Parked(orbit) => format!("parked on {orbit}"),
    }
}

/// The domain of an address of record (`sip:alice@example.com`).
fn domain_of(aor: &str) -> String {
    let rest = aor
        .strip_prefix("sips:")
        .or_else(|| aor.strip_prefix("sip:"))
        .unwrap_or(aor);
    rest.split_once('@')
        .map(|(_, host)| host.split(';').next().unwrap_or(host).to_string())
        .unwrap_or_else(|| rest.to_string())
}

/// What to dial: a SIP or tel URI as given, anything else in `domain`. A
/// phone number is dialled as its digits: letters by the keypad
/// (`800555FOOD` → `8005553663`), spaces, dashes, dots and brackets dropped.
fn dial_target(target: &str, domain: &str) -> String {
    let t = target.trim();
    if t.starts_with("sip:") || t.starts_with("sips:") || t.starts_with("tel:") {
        t.to_string()
    } else if t.contains('@') {
        format!("sip:{t}")
    } else {
        let number = phone_number(t);
        let t = number.as_deref().unwrap_or(t);
        format!("sip:{t}@{domain}")
    }
}

/// The digits of a phone number as written, or `None` for what is not one:
/// a number starts with a digit or `+` and holds nothing but digits, keypad
/// letters, `*`, `#` and separators. A name (`alice`) is not one.
fn phone_number(t: &str) -> Option<String> {
    let first = t.chars().find(|c| !matches!(c, '(' | ' '))?;
    if !(first.is_ascii_digit() || first == '+') {
        return None;
    }
    let mut digits = String::with_capacity(t.len());
    for c in t.chars() {
        match c {
            '0'..='9' | '+' | '*' | '#' => digits.push(c),
            ' ' | '-' | '.' | '(' | ')' => {}
            c if c.is_ascii_alphabetic() => digits.push(keypad_digit(c)),
            _ => return None,
        }
    }
    Some(digits)
}

/// The key a letter is on (ITU E.161).
fn keypad_digit(letter: char) -> char {
    match letter.to_ascii_uppercase() {
        'A'..='C' => '2',
        'D'..='F' => '3',
        'G'..='I' => '4',
        'J'..='L' => '5',
        'M'..='O' => '6',
        'P'..='S' => '7',
        'T'..='V' => '8',
        _ => '9',
    }
}

/// A new contact's id, unique on this device: the time and a count.
fn new_contact_id() -> String {
    static MADE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let n = MADE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("c{nanos:x}{n:x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_number_is_dialled_in_the_accounts_domain() {
        assert_eq!(domain_of("sip:alice@pbx.example.com"), "pbx.example.com");
        assert_eq!(
            dial_target("1002", "pbx.example.com"),
            "sip:1002@pbx.example.com"
        );
        assert_eq!(dial_target("bob@other.net", "x"), "sip:bob@other.net");
        assert_eq!(dial_target("sip:carol@y", "x"), "sip:carol@y");
        assert_eq!(dial_target("tel:+15551234", "x"), "tel:+15551234");
    }

    #[test]
    fn a_number_is_dialled_as_its_digits() {
        assert_eq!(dial_target("800555FOOD", "d"), "sip:8005553663@d");
        assert_eq!(dial_target("1-800-flowers", "d"), "sip:18003569377@d");
        assert_eq!(dial_target("(800) 555-1234", "d"), "sip:8005551234@d");
        assert_eq!(dial_target("+44 20 7946 0958", "d"), "sip:+442079460958@d");
        assert_eq!(dial_target("*70", "d"), "sip:*70@d");
        assert_eq!(dial_target("alice", "d"), "sip:alice@d");
        assert_eq!(dial_target("2x_3", "d"), "sip:2x_3@d");
    }
}
