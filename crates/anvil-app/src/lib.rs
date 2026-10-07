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
mod state;

use std::sync::Arc;

use anvil_core::{Anvil, AnvilConfig, CallId, EndReason, Event, EventStream};
use anvil_fcp::FcpClient;
use parking_lot::Mutex;
use tokio::sync::{broadcast, RwLock};

pub use command::{Command, Response};
pub use state::{CallState, CallView, Change, Direction, MessageWaiting, Registration, State};

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
}

impl Phone {
    /// Start the phone: Anvil up, events followed, registered when asked
    /// (and message waiting subscribed), do not disturb read from FCP.
    pub async fn start(cfg: PhoneConfig) -> Result<Self, PhoneError> {
        let aor = cfg.anvil.account.aor.clone();
        let domain = domain_of(&aor);
        let (anvil, events) = Anvil::start(cfg.anvil).await?;
        let (changes, _) = broadcast::channel(256);
        let inner = Arc::new(Inner {
            sip_address: anvil.sip_address(),
            anvil: RwLock::new(Some(anvil)),
            fcp: cfg.fcp,
            domain,
            state: Mutex::new(State::new(aor)),
            changes,
            events: Mutex::new(None),
        });
        let follower = tokio::spawn(follow(Arc::downgrade(&inner), events));
        *inner.events.lock() = Some(follower);
        let phone = Self { inner };

        if cfg.register {
            phone.anvil().await?.register().await?;
            if let Err(e) = phone.anvil().await?.subscribe_mwi().await {
                tracing::debug!(%e, "no message waiting subscription");
            }
        }
        if let Some(fcp) = &phone.inner.fcp {
            match fcp.calling().await {
                Ok(settings) => phone.set_dnd(settings.dnd),
                Err(e) => tracing::debug!(%e, "calling settings not read"),
            }
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
                let to = target.clone();
                let call = self.anvil().await?.place_call(&to).await?;
                self.update_call(
                    call.0,
                    |c| c.remote = target.clone(),
                    || CallView {
                        id: call.0,
                        direction: Direction::Outgoing,
                        remote: target.clone(),
                        display_name: None,
                        state: CallState::Dialing,
                        held: false,
                        muted: false,
                        codec: None,
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
                self.set_dnd(settings.dnd);
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

    /// Stop the phone: its calls end, it unregisters.
    pub async fn shutdown(&self) {
        if let Some(task) = self.inner.events.lock().take() {
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

    fn set_dnd(&self, on: bool) {
        let changed = {
            let mut state = self.inner.state.lock();
            let changed = state.dnd != Some(on);
            state.dnd = Some(on);
            changed
        };
        if changed {
            let _ = self.inner.changes.send(Change::Dnd { on });
        }
    }
}

fn unreachable_view(id: u64) -> CallView {
    CallView {
        id,
        direction: Direction::Outgoing,
        remote: String::new(),
        display_name: None,
        state: CallState::Connected,
        held: false,
        muted: false,
        codec: None,
    }
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
                    id: call.0,
                    direction: Direction::Incoming,
                    remote: from,
                    display_name,
                    state: CallState::Ringing,
                    held: false,
                    muted: false,
                    codec: None,
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
            Event::CallEstablished { call, codec } => update_call(
                &inner,
                call.0,
                |c| {
                    c.state = CallState::Connected;
                    c.codec = Some(format!("{codec:?}").to_ascii_lowercase());
                },
                || outgoing(call.0),
            ),
            Event::CallEnded { call, reason } => {
                inner.state.lock().calls.retain(|c| c.id != call.0);
                let _ = inner.changes.send(Change::CallEnded {
                    id: call.0,
                    reason: end_reason(&reason),
                });
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
    CallView {
        id,
        direction: Direction::Outgoing,
        remote: String::new(),
        display_name: None,
        state: CallState::Dialing,
        held: false,
        muted: false,
        codec: None,
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
