//! What the phone can be told to do: as JSON on the control socket, and as
//! a line at `anvil run`'s prompt.

use serde::{Deserialize, Serialize};

use crate::state::State;

/// A command. `call` names a call by id; left out, the phone picks the one
/// meant (the ringing call to answer, the connected one to hold, …).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// Dial a number, an extension, a name or a SIP address; from one of
    /// the account's lines by its key (`extension:1005`), else its own.
    Call {
        target: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        line: Option<String>,
        /// A video call (audio where this device has no video).
        #[serde(default, skip_serializing_if = "is_false")]
        video: bool,
    },
    /// Name one of the lines (FCP keeps it, for every device the line is
    /// on); an empty name gives it back its administrator's label.
    RenameLine { line: String, label: String },
    /// Take appearance `index` of a shared line (by key): pick up the call
    /// held there, or join the one up where the line allows barging in.
    TakeAppearance { line: String, index: u8 },
    /// Answer; `video` false answers a video call with audio only (left
    /// out: with video when it offers some and this device has video).
    Answer {
        #[serde(default)]
        call: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        video: Option<bool>,
    },
    /// Join an FCP room by its number or a join link (`fcp://join/…`, the
    /// browser's join page), with video when asked and there is any.
    Join {
        target: String,
        #[serde(default, skip_serializing_if = "is_false")]
        video: bool,
    },
    /// A link to send for one of the user's meetings (by id or room
    /// number): a guest's, or with `host` a host's. The answer's `link`.
    JoinLink {
        meeting: String,
        #[serde(default, skip_serializing_if = "is_false")]
        host: bool,
    },
    /// A host's control over the room a call is in.
    Room {
        #[serde(default)]
        call: Option<u64>,
        action: RoomAction,
    },
    /// Our voice in the room a call is in (FCP's `docs/WEBINARS.md` §2.5):
    /// a hand, a question, chat; and for the stage, moderating them.
    Audience {
        #[serde(default)]
        call: Option<u64>,
        action: AudienceAction,
    },
    /// What a room that forwards each person's stream sends us: these
    /// people, and with `speaker` whoever speaks; neither, the room's
    /// default (the speaker large, those who spoke last small).
    View {
        #[serde(default)]
        call: Option<u64>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        members: Vec<String>,
        #[serde(default, skip_serializing_if = "is_false")]
        speaker: bool,
        /// One of them large and kept first (the user's pin), the rest
        /// small.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pinned: Option<String>,
    },
    /// Our camera on or off in a call: a re-INVITE. On, an audio call
    /// becomes a video call.
    Video {
        #[serde(default)]
        call: Option<u64>,
        on: bool,
    },
    /// The camera video calls use, by id (`None`, the system's first); a
    /// call sending switches to it.
    Camera {
        #[serde(default)]
        camera: Option<String>,
    },
    /// Share our screen in a room, or stop (a call joined with video).
    Share {
        #[serde(default)]
        call: Option<u64>,
        on: bool,
    },
    /// The screen a share shows, by id (`None`, the system's main one); a
    /// share running switches to it.
    Screen {
        #[serde(default)]
        screen: Option<String>,
    },
    /// Allow H.264 from Cisco's OpenH264 (fetched from Cisco if need be),
    /// or not: where it is fetched (Linux).
    H264 { on: bool },
    /// The app went to the background (a phone's), or came back: our
    /// camera stops while it is away and starts again on its return.
    Background { on: bool },
    /// The phone's network: cellular or not (Wi-Fi, a wire). On cellular
    /// a call sends at most 360p, and none with video only on Wi-Fi.
    Network { cellular: bool },
    /// Send video only on Wi-Fi (a phone's setting), or on cellular too.
    WifiOnly { on: bool },
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
    /// Capture the next call, every call, or none (FCP's
    /// `docs/CALL_DIAGNOSTICS.md` §3): refused unless FCP allows it.
    SetCapture { mode: crate::CaptureMode },
    /// Remove the captures kept on this device.
    DeleteCaptures,
    /// Keep a contact: a new one (no id) or a change to one, on this device
    /// or, with `on_account`, on the FCP account (a device's contact saved
    /// so moves there).
    SaveContact { contact: crate::state::Contact },
    /// Delete a contact, from this device or from the account.
    DeleteContact { id: String },
    /// Read the contacts in a vCard or CSV file into this device's
    /// contacts or, with `on_account`, the FCP account's. One already kept
    /// there, by name and numbers, is not made again.
    ImportContacts {
        path: String,
        #[serde(default)]
        on_account: bool,
    },
    /// Write every contact, the device's and the account's, to a file: CSV
    /// when its name ends `.csv`, else vCard.
    ExportContacts { path: String },
    /// Be listed in the tenant's directory, or not (needs FCP, and the
    /// tenant's leave: `directory.may_change`).
    DirectoryListed { on: bool },
    /// Pick a status (needs an FCP that keeps presence): a built-in one
    /// (`available`, `away`, `busy`, `dnd`) or one of the user's own by id
    /// or label, with a note and an RFC 3339 time it clears at. A `dnd`
    /// status turns do not disturb on; any other turns it off.
    Presence {
        status: String,
        #[serde(default)]
        note: Option<String>,
        #[serde(default)]
        until: Option<String>,
    },
    /// Set the note beside the status, or clear it (`None`).
    PresenceNote { note: Option<String> },
    /// Say the user is idle (no keyboard or mouse for a while), or back.
    PresenceIdle { idle: bool },
    /// Keep a status of the user's own: a label on `available`, `away`,
    /// `busy` or `dnd`.
    StatusAdd { label: String, state: String },
    /// Rename one of the user's statuses (by id or label), or change its
    /// state.
    StatusEdit {
        status: String,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        state: Option<String>,
    },
    /// Delete one of the user's statuses; in use, they are available.
    StatusRemove { status: String },
    /// Put the user's statuses in this order, by id: every one, once.
    StatusOrder { ids: Vec<String> },
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

/// What a room's host can do to it, and to the people in it (by the
/// room's id for them).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum RoomAction {
    /// Everyone the same size (`grid`), or the speaker large.
    Layout { grid: bool },
    /// Keep one person in the large tile (`None`: no one).
    Pin {
        #[serde(default)]
        member: Option<String>,
    },
    /// Show one person alone to everyone (`None`: stop).
    Spotlight {
        #[serde(default)]
        member: Option<String>,
    },
    /// Mute one person for the room, or unmute them.
    Mute { member: String, on: bool },
    /// Let one person's camera reach the room, or stop it.
    MemberVideo { member: String, on: bool },
    /// Put one person out of the room.
    Remove { member: String },
    /// End whoever's share is on.
    StopShare,
    /// Take the webinar live: the lobby is let in.
    GoLive,
    /// End the webinar: everyone is released.
    EndWebinar,
    /// Lower one person's raised hand.
    LowerHand { member: String },
    /// Lower every raised hand.
    LowerAllHands,
    /// Bring an attendee to the stage, heard by the room, or send them back.
    Stage { member: String, on: bool },
    /// Mute one person's chat for the sitting, or unmute it.
    MuteChat { member: String, on: bool },
}

/// What we say in a room: our own hand, questions and chat, and for a host
/// or panelist, moderating questions and chat. Each is an INFO in the call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum AudienceAction {
    /// Raise our hand, or lower it.
    Hand { raised: bool },
    /// Ask a question, anonymously where the room allows.
    Ask {
        text: String,
        #[serde(default)]
        anonymous: bool,
    },
    /// Upvote a question, or take the vote back.
    Upvote { id: String, on: bool },
    /// For the stage: show a question to the room.
    Publish { id: String },
    /// For the stage: answer in text, or say it was answered aloud.
    Answer {
        id: String,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        live: bool,
    },
    /// For the stage: drop a question.
    Dismiss { id: String },
    /// Post to the room, or to its hosts and panelists alone.
    Chat {
        text: String,
        #[serde(default)]
        to_hosts: bool,
    },
    /// For the stage: delete a message for everyone.
    DeleteMessage { id: String },
}

/// `room <what> …` at the prompt.
fn room_line(words: &[&str]) -> Result<Command, String> {
    let usage = "room layout grid|speaker, room pin|spotlight <id>|off, \
                 room mute|camera <id> on|off, room remove <id>, room stop-share, \
                 room live|end, room lower <id>|all, room stage|chat-mute <id> on|off, \
                 room view default|<id>|speaker|pin <id> … [call]";
    let word = |i: usize| words.get(i).map(|w| w.to_string());
    let call_at = |i: usize| -> Result<Option<u64>, String> {
        words
            .get(i)
            .map(|w| w.parse().map_err(|_| format!("{w:?} is not a call id")))
            .transpose()
    };
    let member_or_off = |i: usize| -> Result<Option<String>, String> {
        let m = word(i).ok_or(usage)?;
        Ok((!m.eq_ignore_ascii_case("off")).then_some(m))
    };
    let what = word(1).ok_or(usage)?.to_ascii_lowercase();
    // Our own view, not a host's control: `room view p1 speaker 4`.
    if what == "view" {
        let mut rest: Vec<&str> = words.get(2..).unwrap_or_default().to_vec();
        if rest.is_empty() {
            return Err(usage.into());
        }
        let call = match rest.last().and_then(|w| w.parse::<u64>().ok()) {
            Some(id) => {
                rest.pop();
                Some(id)
            }
            None => None,
        };
        let mut members = Vec::new();
        let mut speaker = false;
        let mut pinned = None;
        let mut words = rest.into_iter();
        while let Some(w) = words.next() {
            match w.to_ascii_lowercase().as_str() {
                "default" => {}
                "speaker" => speaker = true,
                // `pin p1`: p1, large.
                "pin" => {
                    let m = words.next().ok_or(usage)?;
                    pinned = Some(m.to_string());
                    members.push(m.to_string());
                }
                _ => members.push(w.to_string()),
            }
        }
        return Ok(Command::View {
            call,
            members,
            speaker,
            pinned,
        });
    }
    let (action, call) = match what.as_str() {
        "layout" => (
            RoomAction::Layout {
                grid: match word(2).map(|w| w.to_ascii_lowercase()).as_deref() {
                    Some("grid") => true,
                    Some("speaker") => false,
                    _ => return Err(usage.into()),
                },
            },
            call_at(3)?,
        ),
        "pin" => (
            RoomAction::Pin {
                member: member_or_off(2)?,
            },
            call_at(3)?,
        ),
        "spotlight" => (
            RoomAction::Spotlight {
                member: member_or_off(2)?,
            },
            call_at(3)?,
        ),
        "mute" | "camera" => {
            let member = word(2).ok_or(usage)?;
            let on = on_off(words.get(3)).ok_or(usage)?;
            let action = if what == "mute" {
                RoomAction::Mute { member, on }
            } else {
                RoomAction::MemberVideo { member, on }
            };
            (action, call_at(4)?)
        }
        "remove" => (
            RoomAction::Remove {
                member: word(2).ok_or(usage)?,
            },
            call_at(3)?,
        ),
        "stop-share" => (RoomAction::StopShare, call_at(2)?),
        "live" => (RoomAction::GoLive, call_at(2)?),
        "end" => (RoomAction::EndWebinar, call_at(2)?),
        "lower" => match word(2).ok_or(usage)? {
            all if all.eq_ignore_ascii_case("all") => (RoomAction::LowerAllHands, call_at(3)?),
            member => (RoomAction::LowerHand { member }, call_at(3)?),
        },
        "stage" | "chat-mute" => {
            let member = word(2).ok_or(usage)?;
            let on = on_off(words.get(3)).ok_or(usage)?;
            let action = if what == "stage" {
                RoomAction::Stage { member, on }
            } else {
                RoomAction::MuteChat { member, on }
            };
            (action, call_at(4)?)
        }
        _ => return Err(usage.into()),
    };
    Ok(Command::Room { call, action })
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
    /// What `import_contacts` did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported: Option<Imported>,
    /// The link `join_link` made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What a contacts import did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Imported {
    /// Contacts made.
    pub imported: u32,
    /// Entries already kept, or twice in the file: not made again.
    pub duplicates: u32,
    /// Entries left out, each with its place in the file and why.
    #[serde(default)]
    pub skipped: Vec<String>,
}

impl Response {
    pub fn ok() -> Self {
        Self {
            ok: true,
            call: None,
            state: None,
            imported: None,
            link: None,
            error: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            call: None,
            state: None,
            imported: None,
            link: None,
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
    /// `contact import [account] <file>`, `contact export <file>`,
    /// `directory off`, `presence away`, `presence WFH 2h`,
    /// `presence note back at 2`, `presence note off`, `presence idle on`,
    /// `presence add away At lunch`, `presence edit WFH busy`,
    /// `presence remove WFH`,
    /// `waiting off`, `heard <id>`, `delete <id>`, `play <id>`, `stop`,
    /// `favourite alice`,
    /// `unfavourite alice`, `recents clear`, `recents hide <id>`, `audio in USB Headset`, `audio out default`,
    /// `audio ring Speakers`, `audio echo off`, `audio noise on`,
    /// `ringtone classic`, `ringtone default`, `ringtone file <path>`,
    /// `ringtone remove <id>`, `test mic`, `test speaker`, `test ring`,
    /// `test stop`,
    /// `refresh`, `register`, `status`, `take <line> <appearance>`. A
    /// trailing number names the call.
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
            // `call 1002`, or `call 1002 on extension:1005` from a line;
            // `vcall 1002` with video.
            "call" | "dial" | "vcall" | "video-call" => Command::Call {
                target: need(words.get(1), "a number or address")?,
                line: match (words.get(2), words.get(3)) {
                    (Some(on), Some(line)) if on.eq_ignore_ascii_case("on") => {
                        Some(line.to_string())
                    }
                    (None, _) => None,
                    _ => return Err(format!("{verb} <number> [on <line>]")),
                },
                video: matches!(verb.to_ascii_lowercase().as_str(), "vcall" | "video-call"),
            },
            // `take shared:+15551234567 2`
            "take" | "take-appearance" => Command::TakeAppearance {
                line: need(words.get(1), "a line")?,
                index: need(words.get(2), "an appearance")?
                    .parse()
                    .map_err(|_| "an appearance is a number".to_string())?,
            },
            "rename-line" => Command::RenameLine {
                line: need(words.get(1), "a line")?,
                label: words[2.min(words.len())..].join(" "),
            },
            // `answer`, `answer 3`, `answer audio`, `answer 3 video`.
            "answer" => {
                let media = |w: &&str| match w.to_ascii_lowercase().as_str() {
                    "video" => Some(true),
                    "audio" => Some(false),
                    _ => None,
                };
                let video = words.get(1..).unwrap_or(&[]).iter().find_map(media);
                let call = words.get(1).filter(|w| media(w).is_none());
                Command::Answer {
                    call: id(call)?,
                    video,
                }
            }
            // `join 3000`, `join fcp://join/<token> video`.
            "join" | "vjoin" => Command::Join {
                target: need(words.get(1), "a room or a link")?,
                video: verb.eq_ignore_ascii_case("vjoin")
                    || words
                        .get(2)
                        .is_some_and(|w| w.eq_ignore_ascii_case("video")),
            },
            // `link 3000`, `link 3000 host`.
            "link" | "join-link" => Command::JoinLink {
                meeting: need(words.get(1), "a meeting")?,
                host: words.get(2).is_some_and(|w| w.eq_ignore_ascii_case("host")),
            },
            // `room layout grid`, `room pin p2`, `room pin off`,
            // `room spotlight p2`, `room mute p2 on`, `room camera p2 off`,
            // `room remove p2`, `room stop-share`; a trailing call id names
            // the call.
            "room" => room_line(&words)?,
            // Our own voice in a room: `hand up`, `ask When?`, `chat Hello`.
            "hand" => Command::Audience {
                call: None,
                action: AudienceAction::Hand {
                    raised: match words.get(1).map(|w| w.to_ascii_lowercase()).as_deref() {
                        Some("up") => true,
                        Some("down") => false,
                        _ => return Err("hand up|down".into()),
                    },
                },
            },
            "ask" | "chat" | "chat-hosts" => {
                let text = words.get(1..).unwrap_or_default().join(" ");
                if text.trim().is_empty() {
                    return Err(format!("{} <text>", words[0]));
                }
                Command::Audience {
                    call: None,
                    action: match words[0] {
                        "ask" => AudienceAction::Ask {
                            text,
                            anonymous: false,
                        },
                        "chat" => AudienceAction::Chat {
                            text,
                            to_hosts: false,
                        },
                        _ => AudienceAction::Chat {
                            text,
                            to_hosts: true,
                        },
                    },
                }
            }
            // `video on`, `video off 3`.
            "video" => Command::Video {
                on: on_off(words.get(1)).ok_or("video needs on or off")?,
                call: id(words.get(2))?,
            },
            // `share on`, `share off 3`.
            "share" => Command::Share {
                on: on_off(words.get(1)).ok_or("share needs on or off")?,
                call: id(words.get(2))?,
            },
            // `screen DP-1`, `screen default`.
            "screen" => Command::Screen {
                screen: rest(&words, 1).filter(|c| !c.eq_ignore_ascii_case("default")),
            },
            // `camera /dev/video2`, `camera default`.
            "camera" => Command::Camera {
                camera: rest(&words, 1).filter(|c| !c.eq_ignore_ascii_case("default")),
            },
            "background" => Command::Background {
                on: on_off(words.get(1)).ok_or("background needs on or off")?,
            },
            // `network cellular`, `network wifi`.
            "network" => Command::Network {
                cellular: match words.get(1).map(|w| w.to_ascii_lowercase()).as_deref() {
                    Some("cellular") => true,
                    Some("wifi") => false,
                    _ => return Err("network needs cellular or wifi".into()),
                },
            },
            "wifi-only" => Command::WifiOnly {
                on: on_off(words.get(1)).ok_or("wifi-only needs on or off")?,
            },
            "h264" => Command::H264 {
                on: on_off(words.get(1)).ok_or("h264 needs on or off")?,
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
            "presence" => match words.get(1).map(|w| w.to_ascii_lowercase()).as_deref() {
                None => return Err("presence needs a status".into()),
                Some("note") => Command::PresenceNote {
                    note: rest(&words, 2).filter(|n| !n.eq_ignore_ascii_case("off")),
                },
                Some("idle") => Command::PresenceIdle {
                    idle: on_off(words.get(2)).ok_or("presence idle needs on or off")?,
                },
                Some("add") => Command::StatusAdd {
                    state: need(words.get(2), "available, away, busy or dnd")?,
                    label: rest(&words, 3).ok_or("presence add needs a state and a label")?,
                },
                Some("remove") => Command::StatusRemove {
                    status: rest(&words, 2).ok_or("presence remove needs a status")?,
                },
                Some("edit") => {
                    let state = need(words.get(3), "a state, or -")?;
                    Command::StatusEdit {
                        status: need(words.get(2), "a status")?,
                        state: (state != "-").then_some(state),
                        label: rest(&words, 4),
                    }
                }
                Some(_) => {
                    // `presence <status> [30m|1h|2h|<RFC 3339>]`.
                    let last = words[words.len() - 1];
                    let until = (words.len() > 2).then(|| until_of(last)).flatten();
                    let end = if until.is_some() {
                        words.len() - 1
                    } else {
                        words.len()
                    };
                    Command::Presence {
                        status: words[1..end].join(" "),
                        note: None,
                        until,
                    }
                }
            },
            "directory" => Command::DirectoryListed {
                on: on_off(words.get(1)).ok_or("directory needs on or off")?,
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
                            ringtone: None,
                        },
                    }
                }
                Some("import") => {
                    // `contact import [account] <file>`.
                    let on_account = words
                        .get(2)
                        .is_some_and(|w| w.eq_ignore_ascii_case("account"));
                    let at = if on_account { 3 } else { 2 };
                    Command::ImportContacts {
                        path: rest(&words, at).ok_or("contact import needs a file")?,
                        on_account,
                    }
                }
                Some("export") => Command::ExportContacts {
                    path: rest(&words, 2).ok_or("contact export needs a file")?,
                },
                Some("delete") => Command::DeleteContact {
                    id: need(words.get(2), "a contact id")?,
                },
                _ => {
                    return Err(
                        "contact add [account] <number> <name>, contact delete <id>, \
                         contact import [account] <file> or contact export <file>"
                            .into(),
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

/// A time a status clears at, as the prompt gives it: `30m`, `1h`, `2h`
/// (from now) or RFC 3339.
fn until_of(word: &str) -> Option<String> {
    let minutes = match word.to_ascii_lowercase().as_str() {
        "30m" => 30,
        "1h" => 60,
        "2h" => 120,
        _ => {
            return chrono::DateTime::parse_from_rfc3339(word)
                .ok()
                .map(|_| word.to_string())
        }
    };
    let at = std::time::SystemTime::now() + std::time::Duration::from_secs(minutes * 60);
    Some(chrono::DateTime::<chrono::Utc>::from(at).to_rfc3339())
}

fn is_false(b: &bool) -> bool {
    !*b
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
    fn rooms_are_joined_linked_and_hosted_from_the_prompt() {
        let p = |l: &str| Command::parse_line(l).unwrap();
        assert_eq!(
            p("join 3000"),
            Command::Join {
                target: "3000".into(),
                video: false
            }
        );
        assert_eq!(
            p("join fcp://join/abc video"),
            Command::Join {
                target: "fcp://join/abc".into(),
                video: true
            }
        );
        assert_eq!(
            p("link 3000 host"),
            Command::JoinLink {
                meeting: "3000".into(),
                host: true
            }
        );
        assert_eq!(
            p("room layout grid"),
            Command::Room {
                call: None,
                action: RoomAction::Layout { grid: true }
            }
        );
        assert_eq!(
            p("room pin off 4"),
            Command::Room {
                call: Some(4),
                action: RoomAction::Pin { member: None }
            }
        );
        assert_eq!(
            p("room mute p2 on"),
            Command::Room {
                call: None,
                action: RoomAction::Mute {
                    member: "p2".into(),
                    on: true
                }
            }
        );
        assert_eq!(
            p("room camera p2 off"),
            Command::Room {
                call: None,
                action: RoomAction::MemberVideo {
                    member: "p2".into(),
                    on: false
                }
            }
        );
        assert_eq!(
            p("room stop-share"),
            Command::Room {
                call: None,
                action: RoomAction::StopShare
            }
        );
        assert_eq!(
            p("room view p1 speaker 4"),
            Command::View {
                call: Some(4),
                members: vec!["p1".into()],
                speaker: true,
                pinned: None,
            }
        );
        assert_eq!(
            p("room view p1 pin p2 p3"),
            Command::View {
                call: None,
                members: vec!["p1".into(), "p2".into(), "p3".into()],
                speaker: false,
                pinned: Some("p2".into()),
            }
        );
        assert!(Command::parse_line("room view pin").is_err());
        assert_eq!(
            p("room view default"),
            Command::View {
                call: None,
                members: vec![],
                speaker: false,
                pinned: None,
            }
        );
        assert!(Command::parse_line("room view").is_err());
        assert!(Command::parse_line("room mute p2").is_err());
        assert!(Command::parse_line("room dance").is_err());
        // As JSON on the control socket.
        let json = r#"{"cmd":"room","action":{"action":"remove","member":"p3"}}"#;
        assert_eq!(
            serde_json::from_str::<Command>(json).unwrap(),
            Command::Room {
                call: None,
                action: RoomAction::Remove {
                    member: "p3".into()
                }
            }
        );
    }

    #[test]
    fn a_line_is_a_command() {
        assert_eq!(
            Command::parse_line("call 1002").unwrap(),
            Command::Call {
                target: "1002".into(),
                line: None,
                video: false,
            }
        );
        assert_eq!(
            Command::parse_line("call 1002 on extension:1005").unwrap(),
            Command::Call {
                target: "1002".into(),
                line: Some("extension:1005".into()),
                video: false,
            }
        );
        assert_eq!(
            Command::parse_line("vcall 1002").unwrap(),
            Command::Call {
                target: "1002".into(),
                line: None,
                video: true,
            }
        );
        for (line, call, video) in [
            ("answer", None, None),
            ("answer 3", Some(3), None),
            ("answer audio", None, Some(false)),
            ("answer 3 video", Some(3), Some(true)),
        ] {
            assert_eq!(
                Command::parse_line(line).unwrap(),
                Command::Answer { call, video },
                "{line}"
            );
        }
        assert_eq!(
            Command::parse_line("video off 3").unwrap(),
            Command::Video {
                call: Some(3),
                on: false
            }
        );
        assert_eq!(
            Command::parse_line("camera /dev/video2").unwrap(),
            Command::Camera {
                camera: Some("/dev/video2".into())
            }
        );
        assert_eq!(
            Command::parse_line("camera default").unwrap(),
            Command::Camera { camera: None }
        );
        assert_eq!(
            Command::parse_line("share on").unwrap(),
            Command::Share {
                call: None,
                on: true
            }
        );
        assert_eq!(
            Command::parse_line("share off 3").unwrap(),
            Command::Share {
                call: Some(3),
                on: false
            }
        );
        assert_eq!(
            Command::parse_line("screen DP-1").unwrap(),
            Command::Screen {
                screen: Some("DP-1".into())
            }
        );
        assert_eq!(
            Command::parse_line("screen default").unwrap(),
            Command::Screen { screen: None }
        );
        let json = r#"{"cmd":"share","on":true}"#;
        assert_eq!(
            serde_json::from_str::<Command>(json).unwrap(),
            Command::Share {
                call: None,
                on: true
            }
        );
        assert_eq!(
            Command::parse_line("h264 off").unwrap(),
            Command::H264 { on: false }
        );
        assert_eq!(
            Command::parse_line("background on").unwrap(),
            Command::Background { on: true }
        );
        assert_eq!(
            Command::parse_line("network cellular").unwrap(),
            Command::Network { cellular: true }
        );
        assert_eq!(
            Command::parse_line("network wifi").unwrap(),
            Command::Network { cellular: false }
        );
        assert_eq!(
            Command::parse_line("wifi-only on").unwrap(),
            Command::WifiOnly { on: true }
        );
        assert_eq!(
            Command::parse_line("rename-line own My desk").unwrap(),
            Command::RenameLine {
                line: "own".into(),
                label: "My desk".into(),
            }
        );
        assert_eq!(
            Command::parse_line("take shared:+15551234567 2").unwrap(),
            Command::TakeAppearance {
                line: "shared:+15551234567".into(),
                index: 2,
            }
        );
        assert!(Command::parse_line("take shared:+15551234567 two").is_err());
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
    fn presence_lines_are_commands() {
        let p = |line: &str| Command::parse_line(line).unwrap();
        assert_eq!(
            p("presence away"),
            Command::Presence {
                status: "away".into(),
                note: None,
                until: None
            }
        );
        assert_eq!(
            p("presence At lunch"),
            Command::Presence {
                status: "At lunch".into(),
                note: None,
                until: None
            },
            "a label of several words"
        );
        match p("presence WFH 2h") {
            Command::Presence { status, until, .. } => {
                assert_eq!(status, "WFH");
                assert!(until.is_some_and(|u| chrono::DateTime::parse_from_rfc3339(&u).is_ok()));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            p("presence busy 2026-10-29T17:00:00Z"),
            Command::Presence {
                status: "busy".into(),
                note: None,
                until: Some("2026-10-29T17:00:00Z".into())
            }
        );
        assert_eq!(
            p("presence note back at 2"),
            Command::PresenceNote {
                note: Some("back at 2".into())
            }
        );
        assert_eq!(p("presence note off"), Command::PresenceNote { note: None });
        assert_eq!(p("presence idle on"), Command::PresenceIdle { idle: true });
        assert_eq!(
            p("presence add away At lunch"),
            Command::StatusAdd {
                label: "At lunch".into(),
                state: "away".into()
            }
        );
        assert_eq!(
            p("presence edit WFH busy"),
            Command::StatusEdit {
                status: "WFH".into(),
                label: None,
                state: Some("busy".into())
            }
        );
        assert_eq!(
            p("presence edit WFH - Home"),
            Command::StatusEdit {
                status: "WFH".into(),
                label: Some("Home".into()),
                state: None
            }
        );
        assert_eq!(
            p("presence remove At lunch"),
            Command::StatusRemove {
                status: "At lunch".into()
            }
        );
        assert!(Command::parse_line("presence").is_err());
        assert!(Command::parse_line("presence idle maybe").is_err());
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
        assert_eq!(
            back,
            Command::Answer {
                call: None,
                video: None,
            }
        );
        assert_eq!(
            serde_json::to_string(&Response::ok()).unwrap(),
            r#"{"ok":true}"#
        );
    }

    #[test]
    fn our_voice_in_a_room_and_a_webinar_s_controls_are_read() {
        assert_eq!(
            Command::parse_line("hand up").unwrap(),
            Command::Audience {
                call: None,
                action: AudienceAction::Hand { raised: true }
            }
        );
        assert_eq!(
            Command::parse_line("ask When does it ship?").unwrap(),
            Command::Audience {
                call: None,
                action: AudienceAction::Ask {
                    text: "When does it ship?".into(),
                    anonymous: false
                }
            }
        );
        assert_eq!(
            Command::parse_line("chat-hosts Slides please").unwrap(),
            Command::Audience {
                call: None,
                action: AudienceAction::Chat {
                    text: "Slides please".into(),
                    to_hosts: true
                }
            }
        );
        assert!(Command::parse_line("chat").is_err());
        assert!(Command::parse_line("hand sideways").is_err());
        assert_eq!(
            Command::parse_line("room live").unwrap(),
            Command::Room {
                call: None,
                action: RoomAction::GoLive
            }
        );
        assert_eq!(
            Command::parse_line("room lower all 4").unwrap(),
            Command::Room {
                call: Some(4),
                action: RoomAction::LowerAllHands
            }
        );
        assert_eq!(
            Command::parse_line("room stage p-9 on").unwrap(),
            Command::Room {
                call: None,
                action: RoomAction::Stage {
                    member: "p-9".into(),
                    on: true
                }
            }
        );
        let json = r#"{"cmd":"audience","action":{"action":"answer","id":"q-1","live":true}}"#;
        assert_eq!(
            serde_json::from_str::<Command>(json).unwrap(),
            Command::Audience {
                call: None,
                action: AudienceAction::Answer {
                    id: "q-1".into(),
                    text: None,
                    live: true
                }
            }
        );
    }
}
