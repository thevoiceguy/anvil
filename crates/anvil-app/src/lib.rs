//! The running phone (`docs/APP.md` §3).
//!
//! [`Phone`] owns an [`anvil_core::Anvil`] and, when signed in, an
//! [`anvil_fcp::FcpClient`]. It keeps the [`State`] a screen shows, publishes
//! a [`Change`] for every event, and carries out [`Command`]s. The app and
//! the CLI are thin over it, so they cannot behave differently.
//! [`control`] serves it on a local socket: `anvil run` serves it, and
//! `anvil call …` drives whatever does.

#![forbid(unsafe_code)]

mod command;
pub mod control;
mod data;
mod settings;
mod state;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use anvil_core::{Anvil, AnvilConfig, CallId, EndReason, Event, EventStream};
use anvil_fcp::FcpClient;
use parking_lot::Mutex;
use tokio::sync::{broadcast, RwLock};

pub use anvil_fcp::{CallingSettings, CallingUpdate};
pub use command::{AudioKind, Command, Response};
pub use state::{
    AudioDevice, AudioDevices, Brand, CallState, CallView, Change, Direction, MessageWaiting,
    Person, Quality, Recent, Registration, State, Voicemail,
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
        });
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
                self.hold_the_others(Some(id)).await?;
                self.anvil().await?.answer(CallId(id)).await.map(|_| ())?;
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
                self.fcp("voicemail")?
                    .set_voicemail_status(&id, "heard")
                    .await?;
                self.inner.update_voicemail(|list| {
                    if let Some(m) = list.iter_mut().find(|m| m.id == id) {
                        m.new = false;
                    }
                });
                Ok(Response::ok())
            }
            Command::DeleteVoicemail { id } => {
                self.fcp("voicemail")?.delete_voicemail(&id).await?;
                self.inner
                    .update_voicemail(|list| list.retain(|m| m.id != id));
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
            Command::Audio { kind, device } => {
                let (mut input, mut output) = self.anvil().await?.chosen_audio_devices();
                match kind {
                    AudioKind::Input => input = device,
                    AudioKind::Output => output = device,
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

    /// Stop the phone: its calls end, it unregisters.
    pub async fn shutdown(&self) {
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
            Event::IncomingCall {
                call,
                from,
                display_name,
            } => {
                let view = CallView {
                    display_name,
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
                let brand = Brand {
                    app_name: profile.app_name.clone(),
                    primary: profile.colors.primary.map(|c| c.to_hex()),
                    accent: profile.colors.accent.map(|c| c.to_hex()),
                    logo: profile
                        .logo
                        .as_ref()
                        .map(|a| a.bytes.clone())
                        .filter(|b| !b.is_empty()),
                };
                let app_name = Some(brand.app_name.clone());
                inner.state.lock().brand = Some(brand);
                let _ = inner.changes.send(Change::Brand { app_name });
            }
            Event::BrandCleared => {
                inner.state.lock().brand = None;
                let _ = inner.changes.send(Change::Brand { app_name: None });
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
    }
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

/// What to dial: a SIP or tel URI as given, anything else in `domain`.
fn dial_target(target: &str, domain: &str) -> String {
    let t = target.trim();
    if t.starts_with("sip:") || t.starts_with("sips:") || t.starts_with("tel:") {
        t.to_string()
    } else if t.contains('@') {
        format!("sip:{t}")
    } else {
        format!("sip:{t}@{domain}")
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
}
