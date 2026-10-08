//! What the phone can be told to do: as JSON on the control socket, and as
//! a line at `anvil run`'s prompt.

use serde::{Deserialize, Serialize};

use crate::state::State;

/// A command. `call` names a call by id; left out, the phone picks the one
/// meant (the ringing call to answer, the connected one to hold, …).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// Dial a number, an extension, a name or a SIP address.
    Call { target: String },
    Answer {
        #[serde(default)]
        call: Option<u64>,
    },
    Decline {
        #[serde(default)]
        call: Option<u64>,
    },
    Hangup {
        #[serde(default)]
        call: Option<u64>,
    },
    Hold {
        #[serde(default)]
        call: Option<u64>,
    },
    Resume {
        #[serde(default)]
        call: Option<u64>,
    },
    Mute {
        #[serde(default)]
        call: Option<u64>,
    },
    Unmute {
        #[serde(default)]
        call: Option<u64>,
    },
    /// Send DTMF digits (`0`–`9`, `*`, `#`, `A`–`D`).
    Dtmf {
        digits: String,
        #[serde(default)]
        call: Option<u64>,
    },
    /// Blind-transfer a call to `target`.
    Transfer {
        target: String,
        #[serde(default)]
        call: Option<u64>,
    },
    /// Join `call`'s party to `to`'s, ending both of ours.
    TransferAttended { call: u64, to: u64 },
    /// Do not disturb on or off (needs FCP).
    Dnd { on: bool },
    /// Change the calling settings: forwards, call waiting (needs FCP).
    Calling { update: anvil_fcp::CallingUpdate },
    /// Park a call with FCP's park code.
    Park {
        #[serde(default)]
        call: Option<u64>,
    },
    /// Mark a voicemail message heard.
    Heard { id: String },
    /// Delete a voicemail message.
    DeleteVoicemail { id: String },
    /// Make someone in the directory a favourite, or not.
    Favourite { who: String, on: bool },
    /// Use this microphone or speaker (by id; `None` for the system's
    /// default) for the calls set up from now on.
    Audio {
        kind: AudioKind,
        #[serde(default)]
        device: Option<String>,
    },
    /// Read the user's data from FCP again.
    Refresh,
    /// The phone as it is now.
    Status,
    /// On the control socket: every change from now on, one per line.
    Subscribe,
}

/// A microphone or a speaker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioKind {
    Input,
    Output,
}

/// What a command answered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    /// The call a `call` placed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call: Option<u64>,
    /// The phone, for `status`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<State>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Response {
    pub fn ok() -> Self {
        Self {
            ok: true,
            call: None,
            state: None,
            error: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            call: None,
            state: None,
            error: Some(message.into()),
        }
    }
}

impl Command {
    /// A prompt line: `call 1002`, `answer`, `hangup 3`, `hold`, `resume`,
    /// `mute`, `unmute`, `dtmf 123#`, `transfer 1003`, `attended 1 2`,
    /// `park`, `dnd on`, `forward busy 1003`, `forward all off`,
    /// `waiting off`, `heard <id>`, `delete <id>`, `favourite alice`,
    /// `unfavourite alice`, `audio in USB Headset`, `audio out default`,
    /// `refresh`, `status`. A trailing number names the call.
    pub fn parse_line(line: &str) -> Result<Self, String> {
        let words: Vec<&str> = line.split_whitespace().collect();
        let id = |w: Option<&&str>| -> Result<Option<u64>, String> {
            w.map(|w| w.parse().map_err(|_| format!("{w:?} is not a call id")))
                .transpose()
        };
        let need = |w: Option<&&str>, what: &str| -> Result<String, String> {
            w.map(|w| w.to_string())
                .ok_or_else(|| format!("{} needs {what}", words.first().copied().unwrap_or("")))
        };
        let Some(verb) = words.first() else {
            return Err("an empty line".into());
        };
        Ok(match verb.to_ascii_lowercase().as_str() {
            "call" | "dial" => Command::Call {
                target: need(words.get(1), "a number or address")?,
            },
            "answer" => Command::Answer {
                call: id(words.get(1))?,
            },
            "decline" | "reject" => Command::Decline {
                call: id(words.get(1))?,
            },
            "hangup" | "end" => Command::Hangup {
                call: id(words.get(1))?,
            },
            "hold" => Command::Hold {
                call: id(words.get(1))?,
            },
            "resume" | "unhold" => Command::Resume {
                call: id(words.get(1))?,
            },
            "mute" => Command::Mute {
                call: id(words.get(1))?,
            },
            "unmute" => Command::Unmute {
                call: id(words.get(1))?,
            },
            "dtmf" => Command::Dtmf {
                digits: need(words.get(1), "digits")?,
                call: id(words.get(2))?,
            },
            "transfer" => Command::Transfer {
                target: need(words.get(1), "a number or address")?,
                call: id(words.get(2))?,
            },
            "attended" => Command::TransferAttended {
                call: id(words.get(1))?.ok_or("attended needs two call ids")?,
                to: id(words.get(2))?.ok_or("attended needs two call ids")?,
            },
            "dnd" => Command::Dnd {
                on: on_off(words.get(1)).ok_or("dnd needs on or off")?,
            },
            "waiting" => Command::Calling {
                update: anvil_fcp::CallingUpdate {
                    call_waiting: Some(on_off(words.get(1)).ok_or("waiting needs on or off")?),
                    ..Default::default()
                },
            },
            "forward" => {
                let when = need(words.get(1), "all, busy, no-answer or unreachable")?;
                let to = need(words.get(2), "a number, or off")?;
                // An empty forward clears it.
                let to = Some(if to.eq_ignore_ascii_case("off") {
                    String::new()
                } else {
                    to
                });
                let mut update = anvil_fcp::CallingUpdate::default();
                match when.to_ascii_lowercase().as_str() {
                    "all" | "always" => update.forward_all = to,
                    "busy" => update.forward_busy = to,
                    "no-answer" | "noanswer" => update.forward_no_answer = to,
                    "unreachable" => update.forward_unreachable = to,
                    other => {
                        return Err(format!(
                            "{other:?}: forward all, busy, no-answer or unreachable"
                        ))
                    }
                }
                Command::Calling { update }
            }
            "park" => Command::Park {
                call: id(words.get(1))?,
            },
            "heard" => Command::Heard {
                id: need(words.get(1), "a message id")?,
            },
            "delete" => Command::DeleteVoicemail {
                id: need(words.get(1), "a message id")?,
            },
            "favourite" | "favorite" | "unfavourite" | "unfavorite" => Command::Favourite {
                who: rest(&words, 1).ok_or_else(|| format!("{verb} needs a name"))?,
                on: !verb.to_ascii_lowercase().starts_with("un"),
            },
            "audio" => {
                let kind = match words.get(1).map(|w| w.to_ascii_lowercase()).as_deref() {
                    Some("in") | Some("input") | Some("mic") => AudioKind::Input,
                    Some("out") | Some("output") | Some("speaker") => AudioKind::Output,
                    _ => return Err("audio needs in or out".into()),
                };
                let device = rest(&words, 2).ok_or("audio needs a device, or default")?;
                Command::Audio {
                    kind,
                    device: (!device.eq_ignore_ascii_case("default")).then_some(device),
                }
            }
            "refresh" => Command::Refresh,
            "status" => Command::Status,
            other => return Err(format!("{other:?} is not a command")),
        })
    }
}

/// `on` or `off` (or yes/no, true/false).
fn on_off(word: Option<&&str>) -> Option<bool> {
    match word.map(|w| w.to_ascii_lowercase()).as_deref() {
        Some("on") | Some("true") | Some("yes") => Some(true),
        Some("off") | Some("false") | Some("no") => Some(false),
        _ => None,
    }
}

/// The words from `from` on, as one (a device's or a person's name).
fn rest(words: &[&str], from: usize) -> Option<String> {
    (words.len() > from).then(|| words[from..].join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_a_command() {
        assert_eq!(
            Command::parse_line("call 1002").unwrap(),
            Command::Call {
                target: "1002".into()
            }
        );
        assert_eq!(
            Command::parse_line("hangup 3").unwrap(),
            Command::Hangup { call: Some(3) }
        );
        assert_eq!(
            Command::parse_line("dtmf 12# 2").unwrap(),
            Command::Dtmf {
                digits: "12#".into(),
                call: Some(2)
            }
        );
        assert_eq!(
            Command::parse_line("dnd on").unwrap(),
            Command::Dnd { on: true }
        );
        assert_eq!(
            Command::parse_line("forward busy 1003").unwrap(),
            Command::Calling {
                update: anvil_fcp::CallingUpdate {
                    forward_busy: Some("1003".into()),
                    ..Default::default()
                }
            }
        );
        let Command::Calling { update } = Command::parse_line("forward all off").unwrap() else {
            panic!("a calling command");
        };
        assert_eq!(update.forward_all.as_deref(), Some(""));
        assert_eq!(
            Command::parse_line("audio in USB Headset").unwrap(),
            Command::Audio {
                kind: AudioKind::Input,
                device: Some("USB Headset".into())
            }
        );
        assert_eq!(
            Command::parse_line("audio out default").unwrap(),
            Command::Audio {
                kind: AudioKind::Output,
                device: None
            }
        );
        assert_eq!(
            Command::parse_line("unfavourite Alice Smith").unwrap(),
            Command::Favourite {
                who: "Alice Smith".into(),
                on: false
            }
        );
        assert!(Command::parse_line("forward sometimes 1003").is_err());
        assert!(Command::parse_line("hangup x").is_err());
        assert!(Command::parse_line("call").is_err());
        assert!(Command::parse_line("fly").is_err());
    }

    #[test]
    fn a_command_is_one_json_object() {
        let json = serde_json::to_string(&Command::Transfer {
            target: "1003".into(),
            call: None,
        })
        .unwrap();
        assert_eq!(json, r#"{"cmd":"transfer","target":"1003","call":null}"#);
        let back: Command = serde_json::from_str(r#"{"cmd":"answer"}"#).unwrap();
        assert_eq!(back, Command::Answer { call: None });
        assert_eq!(
            serde_json::to_string(&Response::ok()).unwrap(),
            r#"{"ok":true}"#
        );
    }
}
