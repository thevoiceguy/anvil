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
    /// The phone as it is now.
    Status,
    /// On the control socket: every change from now on, one per line.
    Subscribe,
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
    /// `dnd on`, `status`. A trailing number names the call.
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
                on: match words.get(1).map(|w| w.to_ascii_lowercase()).as_deref() {
                    Some("on") | Some("true") | Some("yes") => true,
                    Some("off") | Some("false") | Some("no") => false,
                    _ => return Err("dnd needs on or off".into()),
                },
            },
            "status" => Command::Status,
            other => return Err(format!("{other:?} is not a command")),
        })
    }
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
