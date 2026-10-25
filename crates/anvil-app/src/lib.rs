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

mod clip;
mod command;
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
pub use clip::{decode_wav, Clip};
pub use command::{AudioKind, Command, Response};
pub use state::{
    AudioDevice, AudioDevices, AudioProcessing, Brand, CallState, CallView, Change, Direction,
    MessageWaiting, Person, Push, Quality, Recent, Registration, Ringtone, State, Voicemail,
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
    /// The profile's brand, shown while FCP has none.
    fallback_brand: Option<anvil_core::brand::BrandProfile>,
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
        if local.audio_input.is_some() || local.audio_output.is_some() {
            if let Err(e) = anvil
                .choose_audio_devices(local.audio_input.as_deref(), local.audio_output.as_deref())
            {
                tracing::warn!(%e, "the audio devices chosen are not here; using the defaults");
            }
        }
        let (changes, _) = broadcast::channel(256);
        let inner = Arc::new(Inner {
            sip_address: anvil.sip_address(),
            anvil: RwLock::new(Some(anvil)),
            fcp: cfg.fcp,
            domain,
            state: Mutex::new(State::new(aor)),
            changes,
            events: Mutex::new(None),
            fcp_events: Mutex::new(None),
            settings_path: cfg.settings_path,
            local: Mutex::new(local),
            features: Mutex::new(BTreeMap::new()),
            playback: Mutex::new(None),
            ring: cfg.ring,
            ringer: Mutex::new(None),
            sounding: tokio::sync::Mutex::new(()),
            brand_ringtones: Mutex::new(Vec::new()),
            fallback_brand: cfg.fallback_brand,
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
                match first.softphone().await {
                    Ok(config) => *inner.features.lock() = config.feature_codes,
                    Err(e) => tracing::debug!(%e, "feature codes not read"),
                }
                tokio::join!(
                    data::refresh_recents(&inner, &first),
                    data::refresh_people(&inner, &first),
                    data::refresh_voicemail(&inner, &first),
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
            Command::Call { target } => {
                let target = self.dial_target(&target);
                self.hold_the_others(None).await?;
                let to = target.clone();
                let call = self.anvil().await?.place_call(&to).await?;
                self.update_call(
                    call.0,
                    |c| c.remote = target.clone(),
                    || CallView::new(call.0, Direction::Outgoing, &target, CallState::Dialing),
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
                let target = self.dial_target(&code);
                self.anvil().await?.transfer(CallId(id), &target).await?;
                Ok(Response::ok())
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
        own.chain(brand).collect()
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

    /// A ringtone's sound, by id.
    fn tone(&self, id: &str) -> Option<ring::Tone> {
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

    /// Stop what is sounding.
    fn stop_sound(&self) {
        if let Some((_, task)) = self.ringer.lock().take() {
            task.abort();
        }
    }

    /// Sound what the calls want: a ring, the call waiting tone, or
    /// nothing (a preview playing on unless a call rings).
    async fn sound(&self) {
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
                .tone(&self.ringtone())
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
                state.calls.push(view.clone());
                view
            }
        }
    };
    let _ = inner.changes.send(Change::Call { call: view });
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
            } => {
                let view = CallView {
                    display_name,
                    server_call,
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
                let encrypted = match inner.anvil.read().await.as_ref() {
                    Some(anvil) => anvil.is_encrypted(call),
                    None => false,
                };
                update_call(
                    &inner,
                    call.0,
                    |c| {
                        c.state = CallState::Connected;
                        c.connected_at.get_or_insert_with(unix_now);
                        c.codec = Some(format!("{codec:?}").to_ascii_lowercase());
                        c.encrypted = encrypted;
                    },
                    || outgoing(call.0),
                )
            }
            Event::MediaStats { call, stats } => {
                let quality = Quality {
                    jitter_ms: stats.jitter_ms.round() as u32,
                    packet_loss_permille: (stats.packet_loss_pct * 10.0).round() as u32,
                    rtt_ms: stats.rtt_ms.map(|r| r.round() as u32),
                    packets_received: stats.receive.received,
                    packets_played: stats.receive.played,
                    packets_dropped: stats.receive.dropped(),
                    unexpected_pt: stats
                        .receive
                        .last_unexpected_pt
                        .map(|pt| (pt, stats.receive.expected_pt)),
                    last_source: stats.receive.last_source,
                };
                let known = inner.state.lock().call(call.0).is_some();
                if known {
                    update_call(
                        &inner,
                        call.0,
                        |c| c.quality = Some(quality),
                        || outgoing(call.0),
                    );
                }
            }
            Event::CallEnded { call, reason } => {
                inner.state.lock().calls.retain(|c| c.id != call.0);
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

fn end_reason(reason: &EndReason) -> String {
    match reason {
        EndReason::LocalHangup => "hung up".into(),
        EndReason::RemoteHangup => "the other side hung up".into(),
        EndReason::Rejected(code) => format!("refused ({code})"),
        EndReason::Timeout => "no answer".into(),
        EndReason::MediaFailure(e) => format!("media failed: {e}"),
        EndReason::Error(e) => e.clone(),
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
