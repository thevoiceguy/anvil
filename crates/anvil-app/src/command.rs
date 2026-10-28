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
    /// Where the server wakes this phone by push from now on (RFC 8599), or
    /// (`null`) nowhere: the app's push token, new or renewed.
    Push {
        #[serde(default)]
        address: Option<anvil_core::PushAddress>,
    },
    /// Do not disturb on or off (needs FCP).
    Dnd { on: bool },
    /// Change the calling settings: forwards, call waiting (needs FCP).
    Calling { update: anvil_fcp::CallingUpdate },
    /// Park a call with FCP's park code.
    Park {
        #[serde(default)]
        call: Option<u64>,
    },
    /// Record a call the user is on, or stop the recording they started
    /// (needs FCP, and its administrator's leave: `may_record`).
    Record {
        #[serde(default)]
        call: Option<u64>,
        on: bool,
    },
    /// Keep a contact: a new one (no id) or a change to one, on this device
    /// or, with `on_account`, on the FCP account (a device's contact saved
    /// so moves there).
    SaveContact { contact: crate::state::Contact },
    /// Delete a contact, from this device or from the account.
    DeleteContact { id: String },
    /// Mark a voicemail message heard.
    Heard { id: String },
    /// Delete a voicemail message.
    DeleteVoicemail { id: String },
    /// Play a voicemail message through the speaker calls use (and mark it
    /// heard); one plays at a time.
    Play { id: String },
    /// Stop the message playing.
    Stop,
    /// Make someone in the directory a favourite, or not.
    Favourite { who: String, on: bool },
    /// Clear the recent calls shown here (FCP keeps their records).
    ClearRecents,
    /// Remove one recent call, by record id, from those shown here.
    HideRecent { id: String },
    /// Use this microphone or speaker (by id; `None` for the system's
    /// default), the call that is up moving to it, or ring on this speaker.
    Audio {
        kind: AudioKind,
        #[serde(default)]
        device: Option<String>,
    },
    /// Turn echo cancellation or noise suppression on or off (what is not
    /// named stays as it is), in the call that is up too.
    Processing {
        #[serde(default)]
        echo_cancellation: Option<bool>,
        #[serde(default)]
        noise_suppression: Option<bool>,
    },
    /// Set the microphone's or the speaker's volume, in percent (0 to 200,
    /// 100 the sound as it is), in the call that is up too; what is not
    /// named stays as it is.
    Volume {
        #[serde(default)]
        microphone: Option<u16>,
        #[serde(default)]
        speaker: Option<u16>,
    },
    /// Ring with this ringtone (by id; `None` for the brand's, else
    /// Anvil's), heard once now when no call is up.
    Ringtone {
        #[serde(default)]
        id: Option<String>,
    },
    /// Add a ringtone from a sound file on this device (WAV, Ogg Vorbis or
    /// MP3, its first 30 seconds), kept beside the settings, and ring with
    /// it.
    RingtoneFile { path: String },
    /// Remove a ringtone added from a file (`file:` and its name).
    RemoveRingtone { id: String },
    /// Test the microphone: record a few seconds, its level shown, then
    /// hear it back on the speaker calls use. Not while a call is up.
    TestMicrophone,
    /// Test a speaker: a sound on the one calls use, or (`ring`) the one
    /// that rings. Not while a call is up.
    TestSpeaker {
        #[serde(default)]
        ring: bool,
    },
    /// Stop an audio test.
    StopTest,
    /// Read the user's data from FCP again.
    Refresh,
    /// Register again now: woken by a push, the app's connection to the
    /// registrar may be gone, and the call waits for the REGISTER.
    Register,
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
    /// The speaker calls ring on.
    Ring,
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
    /// `park`, `record on`, `record off`, `dnd on`, `forward busy 1003`,
    /// `forward all off`, `contact add 0412345678 Jo Smith`,
    /// `contact add account 0412345678 Jo Smith`, `contact delete <id>`,
    /// `waiting off`, `heard <id>`, `delete <id>`, `play <id>`, `stop`,
    /// `favourite alice`,
    /// `unfavourite alice`, `recents clear`, `recents hide <id>`, `audio in USB Headset`, `audio out default`,
    /// `audio ring Speakers`, `audio echo off`, `audio noise on`,
    /// `ringtone classic`, `ringtone default`, `ringtone file <path>`,
    /// `ringtone remove <id>`, `test mic`, `test speaker`, `test ring`,
    /// `test stop`,
    /// `refresh`, `register`, `status`. A trailing number names the call.
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
            "record" => Command::Record {
                on: on_off(words.get(1)).ok_or("record needs on or off")?,
                call: id(words.get(2))?,
            },
            "contact" => match words.get(1).map(|w| w.to_ascii_lowercase()).as_deref() {
                Some("add") => {
                    // `contact add account …` keeps it on the FCP account.
                    let on_account = words
                        .get(2)
                        .is_some_and(|w| w.eq_ignore_ascii_case("account"));
                    let at = if on_account { 3 } else { 2 };
                    Command::SaveContact {
                        contact: crate::state::Contact {
                            id: String::new(),
                            on_account,
                            name: rest(&words, at + 1)
                                .ok_or("contact add needs a number and a name")?,
                            company: None,
                            numbers: vec![crate::state::ContactNumber {
                                label: String::new(),
                                number: need(words.get(at), "a number")?,
                            }],
                            email: None,
                            notes: None,
                        },
                    }
                }
                Some("delete") => Command::DeleteContact {
                    id: need(words.get(2), "a contact id")?,
                },
                _ => {
                    return Err(
                        "contact add [account] <number> <name>, or contact delete <id>".into(),
                    )
                }
            },
            "heard" => Command::Heard {
                id: need(words.get(1), "a message id")?,
            },
            "delete" => Command::DeleteVoicemail {
                id: need(words.get(1), "a message id")?,
            },
            "play" => Command::Play {
                id: need(words.get(1), "a message id")?,
            },
            "stop" => Command::Stop,
            "favourite" | "favorite" | "unfavourite" | "unfavorite" => Command::Favourite {
                who: rest(&words, 1).ok_or_else(|| format!("{verb} needs a name"))?,
                on: !verb.to_ascii_lowercase().starts_with("un"),
            },
            "recents" => match words.get(1).map(|w| w.to_ascii_lowercase()).as_deref() {
                Some("clear") => Command::ClearRecents,
                Some("hide") => Command::HideRecent {
                    id: need(words.get(2), "a call record id")?,
                },
                _ => return Err("recents clear, or recents hide <id>".into()),
            },
            "audio"
                if matches!(
                    words.get(1).map(|w| w.to_ascii_lowercase()).as_deref(),
                    Some("echo") | Some("noise")
                ) =>
            {
                let on = on_off(words.get(2)).ok_or("audio echo or noise needs on or off")?;
                let echo = words[1].eq_ignore_ascii_case("echo");
                Command::Processing {
                    echo_cancellation: echo.then_some(on),
                    noise_suppression: (!echo).then_some(on),
                }
            }
            "audio" => {
                let kind = match words.get(1).map(|w| w.to_ascii_lowercase()).as_deref() {
                    Some("in") | Some("input") | Some("mic") => AudioKind::Input,
                    Some("out") | Some("output") | Some("speaker") => AudioKind::Output,
                    Some("ring") => AudioKind::Ring,
                    _ => return Err("audio needs in, out, ring, echo or noise".into()),
                };
                let device = rest(&words, 2).ok_or("audio needs a device, or default")?;
                Command::Audio {
                    kind,
                    device: (!device.eq_ignore_ascii_case("default")).then_some(device),
                }
            }
            "volume" => {
                let percent = words
                    .get(2)
                    .and_then(|w| w.trim_end_matches('%').parse::<u16>().ok())
                    .ok_or("volume needs mic or speaker, and a percent")?;
                match words.get(1).map(|w| w.to_ascii_lowercase()).as_deref() {
                    Some("mic") | Some("microphone") | Some("in") => Command::Volume {
                        microphone: Some(percent),
                        speaker: None,
                    },
                    Some("speaker") | Some("out") => Command::Volume {
                        microphone: None,
                        speaker: Some(percent),
                    },
                    _ => return Err("volume needs mic or speaker, and a percent".into()),
                }
            }
            "ringtone" => match words.get(1).map(|w| w.to_ascii_lowercase()).as_deref() {
                Some("file") => Command::RingtoneFile {
                    path: rest(&words, 2).ok_or("ringtone file needs a path")?,
                },
                Some("remove") => Command::RemoveRingtone {
                    id: rest(&words, 2).ok_or("ringtone remove needs a ringtone")?,
                },
                _ => {
                    let id = rest(&words, 1).ok_or("ringtone needs a ringtone, or default")?;
                    Command::Ringtone {
                        id: (!id.eq_ignore_ascii_case("default")).then_some(id),
                    }
                }
            },
            "test" => match words.get(1).map(|w| w.to_ascii_lowercase()).as_deref() {
                Some("mic") | Some("microphone") => Command::TestMicrophone,
                Some("speaker") => Command::TestSpeaker { ring: false },
                Some("ring") => Command::TestSpeaker { ring: true },
                Some("stop") => Command::StopTest,
                _ => return Err("test needs mic, speaker, ring or stop".into()),
            },
            "refresh" => Command::Refresh,
            "register" => Command::Register,
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
            Command::parse_line("audio ring Speakers").unwrap(),
            Command::Audio {
                kind: AudioKind::Ring,
                device: Some("Speakers".into())
            }
        );
        assert_eq!(
            Command::parse_line("ringtone brand:Acme Default").unwrap(),
            Command::Ringtone {
                id: Some("brand:Acme Default".into())
            }
        );
        assert_eq!(
            Command::parse_line("ringtone default").unwrap(),
            Command::Ringtone { id: None }
        );
        assert_eq!(
            Command::parse_line("ringtone file C:\\Users\\Jo\\My Tone.mp3").unwrap(),
            Command::RingtoneFile {
                path: "C:\\Users\\Jo\\My Tone.mp3".into()
            }
        );
        assert_eq!(
            Command::parse_line("ringtone remove file:My Tone.mp3").unwrap(),
            Command::RemoveRingtone {
                id: "file:My Tone.mp3".into()
            }
        );
        assert_eq!(
            Command::parse_line("test mic").unwrap(),
            Command::TestMicrophone
        );
        assert_eq!(
            Command::parse_line("test ring").unwrap(),
            Command::TestSpeaker { ring: true }
        );
        assert!(Command::parse_line("test").is_err());
        assert_eq!(
            Command::parse_line("volume mic 80").unwrap(),
            Command::Volume {
                microphone: Some(80),
                speaker: None
            }
        );
        assert_eq!(
            Command::parse_line("volume speaker 150%").unwrap(),
            Command::Volume {
                microphone: None,
                speaker: Some(150)
            }
        );
        assert!(Command::parse_line("volume speaker loud").is_err());
        assert_eq!(
            Command::parse_line("record on").unwrap(),
            Command::Record {
                call: None,
                on: true
            }
        );
        assert_eq!(
            Command::parse_line("record off 2").unwrap(),
            Command::Record {
                call: Some(2),
                on: false
            }
        );
        assert!(Command::parse_line("record").is_err());
        let Command::SaveContact { contact } =
            Command::parse_line("contact add 0412345678 Jo Smith").unwrap()
        else {
            panic!("a contact");
        };
        assert_eq!(contact.name, "Jo Smith");
        assert_eq!(contact.numbers[0].number, "0412345678");
        assert!(contact.id.is_empty(), "the phone gives it one");
        assert!(!contact.on_account, "on this device unless asked");
        let Command::SaveContact { contact } =
            Command::parse_line("contact add account 0412345678 Jo Smith").unwrap()
        else {
            panic!("a contact");
        };
        assert!(contact.on_account);
        assert_eq!(contact.name, "Jo Smith");
        assert_eq!(contact.numbers[0].number, "0412345678");
        assert_eq!(
            Command::parse_line("contact delete c1").unwrap(),
            Command::DeleteContact { id: "c1".into() }
        );
        assert!(Command::parse_line("contact add 0412345678").is_err());
        assert_eq!(
            Command::parse_line("audio echo off").unwrap(),
            Command::Processing {
                echo_cancellation: Some(false),
                noise_suppression: None
            }
        );
        assert_eq!(
            Command::parse_line("audio Noise on").unwrap(),
            Command::Processing {
                echo_cancellation: None,
                noise_suppression: Some(true)
            }
        );
        assert!(Command::parse_line("audio echo").is_err());
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
        assert_eq!(
            Command::parse_line("recents clear").unwrap(),
            Command::ClearRecents
        );
        assert_eq!(
            Command::parse_line("recents hide cdr-9").unwrap(),
            Command::HideRecent { id: "cdr-9".into() }
        );
        assert!(Command::parse_line("recents").is_err());
        assert_eq!(
            Command::parse_line("play vm-1").unwrap(),
            Command::Play { id: "vm-1".into() }
        );
        assert_eq!(Command::parse_line("stop").unwrap(), Command::Stop);
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
