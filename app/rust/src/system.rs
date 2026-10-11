//! The phone's calls for the system's call screen, and the push address it
//! registers, without Dart (FCP's `docs/PUSH.md` §2.5, phases A2 and A3).
//!
//! A push starts the phone in a process where Flutter may never run: the
//! calls Android's Telecom and iOS's CallKit show then come from here, as events native code
//! reads one at a time ([`next_event`]), and what the user does there comes
//! back as an action on the phone ([`act`]). The push address is a file
//! beside the session, written by native code when the platform hands it a
//! token and read whenever the phone starts, whoever starts it.
//!
//! Android's native code reads them through JNI (`android.rs`), iOS's
//! through a C ABI (`ios.rs`, phase A3). Kept to plain Rust so the host's
//! tests run it.
#![cfg_attr(
    not(any(target_os = "android", target_os = "ios", test)),
    allow(dead_code)
)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::OnceLock;
use std::time::Duration;

use anvil_app::{CallState, Change, Command, Direction, Phone, State};
use anvil_core::{PushAddress, PushProvider};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// The push address kept beside the session at `session_path`.
pub(crate) fn push_path(session_path: &Path) -> PathBuf {
    session_path.with_file_name("push.json")
}

/// The push address saved for this install, if there is one.
pub(crate) fn load_push(session_path: &Path) -> Option<PushAddress> {
    let text = std::fs::read_to_string(push_path(session_path)).ok()?;
    match serde_json::from_str(&text) {
        Ok(address) => Some(address),
        Err(e) => {
            tracing::warn!(%e, "the saved push address is unreadable; registering without");
            None
        }
    }
}

/// Keep the push address for the next start, or forget it.
pub(crate) fn save_push(session_path: &Path, address: Option<&PushAddress>) -> anyhow::Result<()> {
    let path = push_path(session_path);
    match address {
        Some(address) => {
            // The platform may hand over a token before anyone has signed
            // in, when the app's directory is not there yet.
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(path, serde_json::to_vec(address)?)?
        }
        None => match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        },
    }
    Ok(())
}

/// A push address from what native code has: an empty token is none.
pub(crate) fn push_address(
    provider: &str,
    param: &str,
    token: &str,
) -> anyhow::Result<Option<PushAddress>> {
    if token.is_empty() {
        return Ok(None);
    }
    let provider = match provider {
        "fcm" => PushProvider::Fcm,
        "apns" => PushProvider::Apns,
        other => anyhow::bail!("{other:?} is not a push provider"),
    };
    Ok(Some(PushAddress {
        provider,
        param: param.to_string(),
        prid: token.to_string(),
    }))
}

/// What the system's call screen is told.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum SystemEvent {
    /// A call ringing in, and the server's id for it when it gave one
    /// (what a push named it).
    Incoming {
        id: u64,
        name: String,
        number: String,
        server_call: Option<String>,
    },
    /// A call placed, or one already up when the screen first hears of it.
    Outgoing {
        id: u64,
        name: String,
        number: String,
    },
    Connected {
        id: u64,
    },
    Held {
        id: u64,
        on: bool,
    },
    Ended {
        id: u64,
    },
    /// The phone stopped: every call it had is over.
    Reset,
}

/// What the screen has been told of one call.
struct Told {
    connected: bool,
    held: bool,
}

/// The calls as the system's call screen has them, kept in step with the
/// phone's.
#[derive(Default)]
pub(crate) struct SystemCalls {
    told: HashMap<u64, Told>,
}

impl SystemCalls {
    /// What the screen needs to hear to show the phone's calls as `state`
    /// has them.
    pub(crate) fn sync(&mut self, state: &State) -> Vec<SystemEvent> {
        let mut said = Vec::new();
        for call in &state.calls {
            let connected = call.state == CallState::Connected;
            let Some(told) = self.told.get_mut(&call.id) else {
                let name = name_for(state, call);
                let number = short_address(&call.remote);
                if call.direction == Direction::Incoming && !connected {
                    said.push(SystemEvent::Incoming {
                        id: call.id,
                        name,
                        number,
                        server_call: call.server_call.clone(),
                    });
                } else {
                    said.push(SystemEvent::Outgoing {
                        id: call.id,
                        name,
                        number,
                    });
                    if connected {
                        said.push(SystemEvent::Connected { id: call.id });
                    }
                }
                if call.held {
                    said.push(SystemEvent::Held {
                        id: call.id,
                        on: true,
                    });
                }
                self.told.insert(
                    call.id,
                    Told {
                        connected,
                        held: call.held,
                    },
                );
                continue;
            };
            if connected && !told.connected {
                told.connected = true;
                said.push(SystemEvent::Connected { id: call.id });
            }
            if call.held != told.held {
                told.held = call.held;
                said.push(SystemEvent::Held {
                    id: call.id,
                    on: call.held,
                });
            }
        }
        let mut gone: Vec<u64> = self
            .told
            .keys()
            .filter(|id| !state.calls.iter().any(|c| c.id == **id))
            .copied()
            .collect();
        gone.sort_unstable();
        for id in gone {
            self.told.remove(&id);
            said.push(SystemEvent::Ended { id });
        }
        said
    }
}

/// Who a call is with, as a person reads it: the name it came with, the
/// directory's, or the number.
fn name_for(state: &State, call: &anvil_app::CallView) -> String {
    let number = short_address(&call.remote);
    call.display_name
        .clone()
        .filter(|n| !n.is_empty())
        .or_else(|| {
            state
                .people
                .iter()
                .find(|p| p.extension.as_deref() == Some(number.as_str()) || p.key == number)
                .map(|p| p.name.clone())
        })
        .unwrap_or(number)
}

/// `sip:1002@pbx.example.com;transport=tls` → `1002`, as the app shows it.
fn short_address(uri: &str) -> String {
    let uri = uri.trim().trim_start_matches('<').trim_end_matches('>');
    let rest = ["sips:", "sip:", "tel:"]
        .iter()
        .find_map(|scheme| uri.strip_prefix(scheme))
        .unwrap_or(uri);
    let user = rest.split('@').next().unwrap_or(rest);
    user.split(';').next().unwrap_or(user).to_string()
}

/// The events not yet read, one JSON object each.
fn queue() -> &'static (mpsc::Sender<String>, Mutex<mpsc::Receiver<String>>) {
    static QUEUE: OnceLock<(mpsc::Sender<String>, Mutex<mpsc::Receiver<String>>)> = OnceLock::new();
    QUEUE.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        (tx, Mutex::new(rx))
    })
}

fn say(events: Vec<SystemEvent>) {
    for event in events {
        match serde_json::to_string(&event) {
            Ok(json) => {
                let _ = queue().0.send(json);
            }
            Err(e) => tracing::warn!(%e, "a call event did not serialize"),
        }
    }
}

/// The next event for the system's call screen, waiting up to `within`.
pub(crate) fn next_event(within: Duration) -> Option<String> {
    queue().1.lock().recv_timeout(within).ok()
}

/// Follow `phone`'s calls for the system's call screen until aborted.
pub(crate) fn follow(phone: Phone) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut changes = phone.changes();
        let mut calls = SystemCalls::default();
        say(calls.sync(&phone.state()));
        loop {
            match changes.recv().await {
                Ok(Change::Call { .. } | Change::CallEnded { .. })
                | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    say(calls.sync(&phone.state()));
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    })
}

/// The phone stopped: the screen ends every call it was showing.
pub(crate) fn reset() {
    say(vec![SystemEvent::Reset]);
}

/// What the user did at the system's call screen.
#[derive(Debug, Deserialize)]
pub(crate) struct Action {
    action: String,
    id: u64,
    #[serde(default)]
    on: Option<bool>,
    #[serde(default)]
    digits: Option<String>,
    #[serde(default)]
    reason: Option<String>,
}

/// The phone command for `action` on the phone as `state` has it, if any:
/// ending a call ringing in declines it; Do Not Disturb at the system
/// declines a call it would not let ring.
pub(crate) fn command_for(action: &Action, state: &State) -> Option<Command> {
    let call = state.calls.iter().find(|c| c.id == action.id)?;
    let ringing_in = call.direction == Direction::Incoming && call.state != CallState::Connected;
    let id = Some(action.id);
    Some(match action.action.as_str() {
        "answer" if ringing_in => Command::Answer { call: id },
        "end" if ringing_in => Command::Decline { call: id },
        "end" => Command::Hangup { call: id },
        "failed" if ringing_in && action.reason.as_deref() == Some("filtered") => {
            Command::Decline { call: id }
        }
        "hold" if call.state == CallState::Connected => match action.on.unwrap_or(true) {
            true if !call.held => Command::Hold { call: id },
            false if call.held => Command::Resume { call: id },
            _ => return None,
        },
        "mute" => match action.on.unwrap_or(true) {
            true if !call.muted => Command::Mute { call: id },
            false if call.muted => Command::Unmute { call: id },
            _ => return None,
        },
        "dtmf" if call.state == CallState::Connected => Command::Dtmf {
            digits: action.digits.clone().filter(|d| !d.is_empty())?,
            call: id,
        },
        _ => return None,
    })
}

/// Carry out what the user did at the system's call screen (`json` an
/// [`Action`]).
pub(crate) async fn act(phone: &Phone, json: &str) -> anyhow::Result<()> {
    let action: Action = serde_json::from_str(json)?;
    if let Some(command) = command_for(&action, &phone.state()) {
        phone.execute(command).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anvil_app::CallView;

    fn state(calls: Vec<CallView>) -> State {
        let mut state = State::new("sip:bob@pbx.test".into());
        state.calls = calls;
        state
    }

    fn ringing_in(id: u64) -> CallView {
        CallView {
            display_name: Some("Ann Lee".into()),
            server_call: Some("6f1c".into()),
            ..CallView::new(
                id,
                Direction::Incoming,
                "sip:1002@pbx.test",
                CallState::Ringing,
            )
        }
    }

    #[test]
    fn a_call_is_told_as_it_goes() {
        let mut calls = SystemCalls::default();
        let mut call = ringing_in(7);
        assert_eq!(
            calls.sync(&state(vec![call.clone()])),
            vec![SystemEvent::Incoming {
                id: 7,
                name: "Ann Lee".into(),
                number: "1002".into(),
                server_call: Some("6f1c".into()),
            }]
        );
        assert!(
            calls.sync(&state(vec![call.clone()])).is_empty(),
            "told once"
        );
        call.state = CallState::Connected;
        assert_eq!(
            calls.sync(&state(vec![call.clone()])),
            vec![SystemEvent::Connected { id: 7 }]
        );
        call.held = true;
        assert_eq!(
            calls.sync(&state(vec![call.clone()])),
            vec![SystemEvent::Held { id: 7, on: true }]
        );
        assert_eq!(
            calls.sync(&state(vec![])),
            vec![SystemEvent::Ended { id: 7 }]
        );
    }

    #[test]
    fn a_call_already_up_is_told_as_placed_and_connected() {
        let mut calls = SystemCalls::default();
        let mut call = CallView::new(
            3,
            Direction::Outgoing,
            "sip:2001@pbx.test",
            CallState::Connected,
        );
        call.display_name = None;
        let mut people_state = state(vec![call]);
        people_state.people.push(anvil_app::Person {
            key: "carol".into(),
            name: "Carol".into(),
            extension: Some("2001".into()),
            department: None,
            job_title: None,
            presence: None,
            on_call: false,
            favourite: false,
        });
        assert_eq!(
            calls.sync(&people_state),
            vec![
                SystemEvent::Outgoing {
                    id: 3,
                    name: "Carol".into(),
                    number: "2001".into(),
                },
                SystemEvent::Connected { id: 3 },
            ]
        );
    }

    fn action(json: &str) -> Action {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn what_the_user_does_there_is_a_phone_command() {
        let ringing = state(vec![ringing_in(7)]);
        assert!(matches!(
            command_for(&action(r#"{"action":"answer","id":7}"#), &ringing),
            Some(Command::Answer { call: Some(7) })
        ));
        assert!(matches!(
            command_for(&action(r#"{"action":"end","id":7}"#), &ringing),
            Some(Command::Decline { call: Some(7) })
        ));
        assert!(matches!(
            command_for(
                &action(r#"{"action":"failed","id":7,"reason":"filtered"}"#),
                &ringing
            ),
            Some(Command::Decline { call: Some(7) })
        ));
        assert!(command_for(
            &action(r#"{"action":"failed","id":7,"reason":"busy"}"#),
            &ringing
        )
        .is_none());
        assert!(command_for(&action(r#"{"action":"answer","id":8}"#), &ringing).is_none());

        let mut up = ringing_in(7);
        up.state = CallState::Connected;
        let up = state(vec![up]);
        assert!(matches!(
            command_for(&action(r#"{"action":"end","id":7}"#), &up),
            Some(Command::Hangup { call: Some(7) })
        ));
        assert!(matches!(
            command_for(&action(r#"{"action":"hold","id":7,"on":true}"#), &up),
            Some(Command::Hold { call: Some(7) })
        ));
        assert!(command_for(&action(r#"{"action":"hold","id":7,"on":false}"#), &up).is_none());
        assert!(matches!(
            command_for(&action(r#"{"action":"dtmf","id":7,"digits":"5"}"#), &up),
            Some(Command::Dtmf { call: Some(7), .. })
        ));
    }

    #[test]
    fn the_push_address_is_kept_beside_the_session() {
        let dir = std::env::temp_dir().join(format!("anvil-push-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let session = dir.join("session.json");
        assert_eq!(load_push(&session), None);
        let address = push_address("fcm", "anvil-12345", "token-1").unwrap();
        save_push(&session, address.as_ref()).unwrap();
        assert_eq!(load_push(&session), address);
        assert_eq!(push_address("fcm", "anvil-12345", "").unwrap(), None);
        assert!(push_address("pigeon", "x", "y").is_err());
        save_push(&session, None).unwrap();
        save_push(&session, None).unwrap();
        assert_eq!(load_push(&session), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_number_is_the_user_part() {
        assert_eq!(short_address("sip:1002@pbx.test;transport=tls"), "1002");
        assert_eq!(short_address("<sips:ann@pbx.test>"), "ann");
        assert_eq!(short_address("tel:+15125550100"), "+15125550100");
    }
}
