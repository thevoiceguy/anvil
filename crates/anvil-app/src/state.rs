//! What the phone shows: its registration, its calls, message waiting and
//! do not disturb, and, signed in to FCP, the user's recent calls, the
//! directory with presence, voicemail and calling settings — and the changes
//! to them, as the app and the CLI see them.

use serde::{Deserialize, Serialize};

/// The phone as it is now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    /// The account's address of record.
    pub aor: String,
    /// The account's lines, its own first (FCP's `docs/DEVICE_LINES.md`);
    /// empty without FCP or from an older one: the account alone.
    #[serde(default)]
    pub lines: Vec<LineView>,
    /// The tenant lets the user name their lines.
    #[serde(default)]
    pub may_rename_lines: bool,
    pub registration: Registration,
    /// The calls, oldest first.
    pub calls: Vec<CallView>,
    pub message_waiting: Option<MessageWaiting>,
    /// Do not disturb, as FCP has it; `None` without FCP.
    pub dnd: Option<bool>,
    /// The tenant's brand, once fetched.
    pub brand: Option<Brand>,
    /// The user's calling settings, as FCP has them; `None` without FCP.
    pub calling: Option<anvil_fcp::CallingSettings>,
    /// The latest calls, newest first.
    pub recents: Vec<Recent>,
    /// The tenant's directory, with presence where the tenant shows it.
    pub people: Vec<Person>,
    /// The mailbox's messages, newest first.
    pub voicemail: Vec<Voicemail>,
    pub audio: AudioDevices,
    /// The voicemail message playing, by id.
    #[serde(default)]
    pub playing: Option<String>,
    /// Whether the server can wake this phone by push.
    #[serde(default)]
    pub push: Push,
    /// The user may record a call they are on (FCP says so); the record
    /// button shows only then.
    #[serde(default)]
    pub may_record: bool,
    /// The user's contacts, by name: those kept on this device and, signed
    /// in to FCP, those kept on their account.
    #[serde(default)]
    pub contacts: Vec<Contact>,
    /// Signed in to FCP: whether the user is in the tenant's directory.
    #[serde(default)]
    pub directory: Option<DirectoryListing>,
    /// Signed in to an FCP that keeps it: the status the user chose and
    /// what others see of them (FCP's `docs/PRESENCE.md`).
    #[serde(default)]
    pub presence: Option<MyPresence>,
    /// A test of the microphone or a speaker under way, if any.
    #[serde(default)]
    pub audio_test: Option<AudioTest>,
}

/// A test of this device's audio, under way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioTest {
    pub stage: AudioTestStage,
    /// While recording: how loud the microphone is now, 0 to 100.
    pub level: u8,
}

/// What an audio test is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioTestStage {
    /// Recording the microphone, its level shown.
    Recording,
    /// Playing the recording back on the speaker calls use.
    PlayingBack,
    /// A sound on the speaker calls use.
    Speaker,
    /// A sound on the speaker that rings.
    Ring,
}

/// A contact the user keeps: on this device, with any account, FCP's or
/// not; or on their FCP account, where every app they sign in to has it
/// (`docs/APP.md` §Contacts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contact {
    /// Made when the contact is first saved; empty in a new one.
    #[serde(default)]
    pub id: String,
    /// Kept on the user's FCP account, not on this device. Saving a
    /// device's contact with this set moves it to the account.
    #[serde(default)]
    pub on_account: bool,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub company: Option<String>,
    #[serde(default)]
    pub numbers: Vec<ContactNumber>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// The ringtone a call from one of its numbers rings with, by id
    /// (`anvil`, `brand:…`, `file:…`); `None`, or one this device does not
    /// have, rings with the phone's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ringtone: Option<String>,
}

impl Contact {
    /// Whether `remote` (an address or a number, as a call shows it) is
    /// one of its numbers.
    pub fn has_number(&self, remote: &str) -> bool {
        let dialled = crate::data::user_of(remote);
        self.numbers.iter().any(|n| n.number == dialled)
    }
}

/// The user's presence: what others see, the status they chose, and
/// their own statuses to choose from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MyPresence {
    /// What others see: `available`, `away`, `busy`, `dnd`, `ringing`,
    /// `on_call` or `offline`.
    pub shown: String,
    /// Their own status's label beside it ("WFH").
    pub shown_label: Option<String>,
    /// Away because this or another of their apps says they are idle.
    pub idle: bool,
    /// What they chose: `available`, `away`, `busy`, `dnd`, or one of
    /// `statuses`' ids.
    pub status: String,
    /// The chosen status's state.
    pub status_state: String,
    pub note: Option<String>,
    /// RFC 3339: when the chosen status clears back to available.
    pub until: Option<String>,
    pub statuses: Vec<OwnStatus>,
}

/// One of the user's own statuses: a label on a state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnStatus {
    pub id: String,
    pub label: String,
    /// `available`, `away`, `busy` or `dnd`.
    pub state: String,
}

impl From<anvil_fcp::MyPresence> for MyPresence {
    fn from(p: anvil_fcp::MyPresence) -> Self {
        Self {
            shown: p.presence.state,
            shown_label: p.presence.label,
            idle: p.presence.idle,
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
        }
    }
}

/// Whether the user is in the tenant's directory, and may change that
/// (FCP says; `docs/APP.md` §Settings).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectoryListing {
    pub listed: bool,
    pub may_change: bool,
}

/// One of a contact's numbers: what it is (`mobile`, `work`, …) and what
/// to dial.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactNumber {
    #[serde(default)]
    pub label: String,
    pub number: String,
}

/// The tenant's brand, as a screen applies it (`docs/BRANDING.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Brand {
    pub app_name: String,
    /// `#RRGGBB`.
    pub primary: Option<String>,
    pub accent: Option<String>,
    /// The logo's bytes (PNG or SVG as the tenant uploaded it); not sent on
    /// the control socket.
    #[serde(skip)]
    pub logo: Option<Vec<u8>>,
    /// The icon's bytes (PNG, JPEG or WebP), for the system's icons; not
    /// sent on the control socket.
    #[serde(skip)]
    pub icon: Option<Vec<u8>>,
}

impl State {
    /// A phone for `aor` as it starts: unregistered, with nothing yet.
    pub fn new(aor: String) -> Self {
        Self {
            aor,
            lines: Vec::new(),
            may_rename_lines: false,
            registration: Registration::Unregistered,
            calls: Vec::new(),
            message_waiting: None,
            dnd: None,
            brand: None,
            calling: None,
            recents: Vec::new(),
            people: Vec::new(),
            voicemail: Vec::new(),
            audio: AudioDevices::default(),
            playing: None,
            push: Push::Off,
            may_record: false,
            contacts: Vec::new(),
            directory: None,
            presence: None,
            audio_test: None,
        }
    }

    /// Where call `id` is, while it lasts.
    pub fn call_state(&self, id: u64) -> Option<CallState> {
        self.call(id).map(|c| c.state)
    }

    pub(crate) fn call(&self, id: u64) -> Option<&CallView> {
        self.calls.iter().find(|c| c.id == id)
    }

    pub(crate) fn call_mut(&mut self, id: u64) -> Option<&mut CallView> {
        self.calls.iter_mut().find(|c| c.id == id)
    }
}

/// Whether the account is registered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Registration {
    Unregistered,
    Registering,
    Registered,
    Failed,
}

impl From<anvil_core::RegState> for Registration {
    fn from(s: anvil_core::RegState) -> Self {
        match s {
            anvil_core::RegState::Unregistered => Self::Unregistered,
            anvil_core::RegState::Registering => Self::Registering,
            anvil_core::RegState::Registered => Self::Registered,
            anvil_core::RegState::Failed => Self::Failed,
        }
    }
}

/// Whether the server can wake this phone by push (RFC 8599).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Push {
    /// No push address is registered.
    #[default]
    Off,
    /// The server took the push address: a call wakes the phone.
    Accepted,
    /// The server cannot push to this phone; it pushes through
    /// `supported`. The phone rings only while it is awake.
    Refused { supported: Vec<String> },
}

impl From<anvil_core::PushState> for Push {
    fn from(s: anvil_core::PushState) -> Self {
        match s {
            anvil_core::PushState::Off => Self::Off,
            anvil_core::PushState::Accepted => Self::Accepted,
            anvil_core::PushState::Refused { supported } => Self::Refused { supported },
        }
    }
}

/// One call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallView {
    pub id: u64,
    pub direction: Direction,
    /// Who the call is with: the caller's address, or what was dialled.
    pub remote: String,
    pub display_name: Option<String>,
    pub state: CallState,
    pub held: bool,
    pub muted: bool,
    /// The codec in use once connected (`opus`, `pcmu`, …).
    pub codec: Option<String>,
    /// The media is encrypted (SRTP).
    #[serde(default)]
    pub encrypted: bool,
    /// How the media is doing, once measured (boxed: it is most of a
    /// call's size, and a call rides in every change).
    #[serde(default)]
    pub quality: Option<Box<Quality>>,
    /// When it connected, in Unix seconds: what a call timer counts from.
    #[serde(default)]
    pub connected_at: Option<u64>,
    /// The server's id for a call coming in, when it gave one: what a push
    /// that woke the app named it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_call: Option<String>,
    /// The user is recording the call (they asked; FCP said yes).
    #[serde(default)]
    pub recording: bool,
    /// The line the call is on: its key (`own`, `extension:1005`, …), as
    /// the server named it ringing in or the user chose dialling out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
    /// Ringing on a line that does not ring this app: shown, not sounded.
    #[serde(default)]
    pub silent: bool,
}

impl CallView {
    /// A call just seen: not held, not muted, nothing measured yet.
    pub fn new(id: u64, direction: Direction, remote: &str, state: CallState) -> Self {
        Self {
            id,
            direction,
            remote: remote.to_string(),
            display_name: None,
            state,
            held: false,
            muted: false,
            codec: None,
            encrypted: false,
            quality: None,
            connected_at: None,
            server_call: None,
            recording: false,
            line: None,
            silent: false,
        }
    }
}

/// One of the account's lines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineView {
    /// Its key: what a call for it is named by, and a name is set under.
    pub line: String,
    /// `own`, `extension`, `shared` or `on_behalf`.
    pub kind: String,
    pub aor: String,
    /// What it is called: the user's name, the administrator's, else
    /// `default_label`.
    pub label: String,
    pub default_label: String,
    /// The account's own line follows [`State::registration`].
    pub registration: Registration,
    pub message_waiting: Option<MessageWaiting>,
    pub ring: bool,
    pub shared: bool,
    pub appearances: u32,
    /// A shared line whose active calls the app may join.
    #[serde(default)]
    pub barge_in: bool,
    /// A shared line's appearances in use, as the server last said, by
    /// index; empty when it is idle.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub in_use: Vec<AppearanceView>,
}

/// One appearance of a shared line in use (FCP's
/// `docs/ENTERPRISE_CALLING.md` §3.9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppearanceView {
    /// 1 for the first.
    pub index: u8,
    /// `alerting`, `progressing`, `active`, `held` or `seized`.
    pub state: String,
    /// The far party, when the server names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
}

impl AppearanceView {
    /// Held: any phone on the line may pick it up.
    pub fn held(&self) -> bool {
        self.state == "held"
    }

    /// Up: joined by barging in, where the line allows it.
    pub fn active(&self) -> bool {
        self.state == "active"
    }
}

impl State {
    /// Whether an appearance of `line` (by key) is one of this app's own
    /// calls: a call on the line with the appearance's far party.
    pub fn appearance_is_ours(&self, line: &str, appearance: &AppearanceView) -> bool {
        let Some(remote) = appearance.remote.as_deref() else {
            return false;
        };
        let user = |address: &str| {
            let inner = match (address.find('<'), address.rfind('>')) {
                (Some(a), Some(b)) if b > a => &address[a + 1..b],
                _ => address,
            };
            let inner = inner.trim();
            let inner = inner
                .strip_prefix("sips:")
                .or_else(|| inner.strip_prefix("sip:"))
                .or_else(|| inner.strip_prefix("tel:"))
                .unwrap_or(inner);
            inner
                .split(['@', ';'])
                .next()
                .unwrap_or("")
                .trim_start_matches('+')
                .to_ascii_lowercase()
        };
        let theirs = user(remote);
        !theirs.is_empty()
            && self
                .calls
                .iter()
                .any(|c| c.line.as_deref() == Some(line) && user(&c.remote) == theirs)
    }
}

/// A connected call's media, as last measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quality {
    pub jitter_ms: u32,
    /// Packets lost, per thousand.
    pub packet_loss_permille: u32,
    pub rtt_ms: Option<u32>,
    /// RTP datagrams received since the call began, of any kind.
    #[serde(default)]
    pub packets_received: u64,
    /// Audio frames played.
    #[serde(default)]
    pub packets_played: u64,
    /// Packets dropped: the wrong payload type, failed SRTP, not RTP.
    #[serde(default)]
    pub packets_dropped: u64,
    /// The payload type of the last packet dropped for it, and the one
    /// expected: a far end sending a codec this call did not agree on.
    #[serde(default)]
    pub unexpected_pt: Option<(u8, u8)>,
    /// Where the last packet came from.
    #[serde(default)]
    pub last_source: Option<std::net::SocketAddr>,
    /// The far end's packets lost over the last five seconds, per thousand.
    #[serde(default)]
    pub recent_loss_permille: u32,
    /// RTP sent and received (that played or not), since the call began.
    #[serde(default)]
    pub packets_sent: u64,
    #[serde(default)]
    pub bytes_sent: u64,
    #[serde(default)]
    pub rtp_received: u64,
    #[serde(default)]
    pub bytes_received: u64,
    /// The far end's packets its sequence numbers say were sent, and those
    /// that never arrived.
    #[serde(default)]
    pub packets_expected: u64,
    #[serde(default)]
    pub packets_lost: u64,
    /// What flowed in the last second.
    #[serde(default)]
    pub send_kbps: u32,
    #[serde(default)]
    pub recv_kbps: u32,
    #[serde(default)]
    pub send_pps: u32,
    #[serde(default)]
    pub recv_pps: u32,
    /// What the far end last reported of ours (RTCP): its loss in its last
    /// interval per thousand, its jitter, and what it has lost in all.
    #[serde(default)]
    pub far_loss_permille: Option<u32>,
    #[serde(default)]
    pub far_jitter_ms: Option<u32>,
    #[serde(default)]
    pub far_lost: Option<i64>,
    /// The call has RTCP, and the reports sent and read.
    #[serde(default)]
    pub rtcp: bool,
    #[serde(default)]
    pub rtcp_sent: u64,
    #[serde(default)]
    pub rtcp_received: u64,
    /// The codec's RTP clock in Hz and its audio per packet in ms.
    #[serde(default)]
    pub clock_rate: u32,
    #[serde(default)]
    pub ptime_ms: u32,
    /// Where media flows between: our RTP socket and the far end's.
    #[serde(default)]
    pub local_rtp: Option<std::net::SocketAddr>,
    #[serde(default)]
    pub remote_rtp: Option<std::net::SocketAddr>,
    /// The estimated score (E-model MOS, 1 to 4.5) in hundredths of what
    /// the user hears, and of what the far end hears once it has reported.
    #[serde(default)]
    pub mos_hundredths: u16,
    #[serde(default)]
    pub far_mos_hundredths: Option<u16>,
}

/// One call in the user's history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recent {
    /// FCP's record id.
    pub id: String,
    pub direction: Direction,
    /// Who the call was with: what to dial back.
    pub remote: String,
    pub display_name: Option<String>,
    /// An incoming call nobody answered.
    pub missed: bool,
    /// RFC 3339.
    pub started_at: String,
    pub duration_secs: Option<u64>,
    /// How it went, when one of the user's phones reported it: the lower
    /// score in hundredths, and whether it was poor (under 3.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mos_hundredths: Option<u16>,
    #[serde(default)]
    pub poor: bool,
}

/// Someone in the tenant's directory: a user, or an extension that is not
/// a user's (a queue, a ring group).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Person {
    /// The user's login name, or the extension when it is not a user's.
    pub key: String,
    pub name: String,
    pub extension: Option<String>,
    pub department: Option<String>,
    pub job_title: Option<String>,
    /// `available`, `busy`, `away`, `dnd`, `ringing`, `on_call`,
    /// `offline`, … when shown.
    pub presence: Option<String>,
    /// Their own status's label beside it ("WFH").
    #[serde(default)]
    pub presence_label: Option<String>,
    /// On a call now (their busy lamp).
    pub on_call: bool,
    /// One of this user's favourites.
    pub favourite: bool,
}

impl Person {
    /// What to dial: the extension, else the user's name.
    pub fn dial(&self) -> &str {
        self.extension.as_deref().unwrap_or(&self.key)
    }
}

/// One voicemail message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Voicemail {
    pub id: String,
    pub caller: String,
    pub caller_name: Option<String>,
    /// Not yet heard.
    pub new: bool,
    pub urgent: bool,
    pub duration_secs: u64,
    pub transcription: Option<String>,
    /// RFC 3339.
    pub received_at: String,
}

/// The microphones and speakers, and which are in use; the ringtones, and
/// which rings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioDevices {
    pub inputs: Vec<AudioDevice>,
    pub outputs: Vec<AudioDevice>,
    /// The chosen microphone's id; `None` is the system's default.
    pub input: Option<String>,
    pub output: Option<String>,
    /// The speaker chosen to ring on; `None` is the system's default.
    #[serde(default)]
    pub ring: Option<String>,
    /// Anvil's own ringtones, then the brand's.
    #[serde(default)]
    pub ringtones: Vec<Ringtone>,
    /// The ringtone that rings, by id.
    #[serde(default)]
    pub ringtone: String,
    /// What is done to the microphone; `None` when the audio host does
    /// nothing of its own (the platform's call audio does it, or nothing
    /// does).
    #[serde(default)]
    pub processing: Option<AudioProcessing>,
    /// The microphone's and the speaker's volume; `None` when the audio
    /// host sets none of its own (a phone's own volume keys do).
    #[serde(default)]
    pub volume: Option<AudioVolume>,
}

/// How loud the microphone is sent and the speaker plays, in percent: 100
/// the sound as it is, 0 silence, up to 200 twice as loud.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioVolume {
    pub microphone: u16,
    pub speaker: u16,
}

/// Echo cancellation and noise suppression, each on or off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioProcessing {
    pub echo_cancellation: bool,
    pub noise_suppression: bool,
}

/// A ringtone to choose: `anvil`, `classic`, or `brand:` and the brand's
/// name for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ringtone {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    /// The system's default.
    pub default: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Incoming,
    Outgoing,
}

/// Where a call is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallState {
    /// Outgoing, sent, nothing heard yet.
    Dialing,
    /// Ringing: an incoming call to answer, or an outgoing one ringing there.
    Ringing,
    Connected,
}

/// The mailbox's counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageWaiting {
    pub waiting: bool,
    pub new: u32,
    pub old: u32,
}

/// A change to the phone, one per event: what the app redraws, what
/// `anvil watch` prints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Change {
    Registration {
        state: Registration,
        reason: Option<String>,
    },
    /// The lines, or one line's registration or messages, changed.
    Lines {
        lines: Vec<LineView>,
    },
    /// A call appeared or changed.
    Call {
        call: CallView,
    },
    CallEnded {
        id: u64,
        reason: String,
    },
    MessageWaiting {
        message_waiting: MessageWaiting,
    },
    Dnd {
        on: bool,
    },
    /// The brand arrived or changed; `None` when the tenant has none.
    Brand {
        app_name: Option<String>,
    },
    /// A transfer of `id` progressed: the server's status for it.
    TransferProgress {
        id: u64,
        code: u16,
        reason: String,
    },
    /// The calling settings changed (here or elsewhere).
    Calling {
        settings: anvil_fcp::CallingSettings,
    },
    /// The recent calls were read again; `missed` of them unanswered.
    Recents {
        missed: u32,
    },
    /// The directory was read again.
    People {
        count: u32,
    },
    /// One person's presence, busy lamp or favourite changed.
    Person {
        person: Person,
    },
    /// The mailbox's messages were read again.
    Voicemail {
        new: u32,
        total: u32,
    },
    /// The devices, or the choice of them, changed.
    Audio {
        audio: AudioDevices,
    },
    /// A voicemail message started playing, or (`None`) the playing stopped.
    Playing {
        id: Option<String>,
    },
    /// What the server made of the phone's push address.
    Push {
        push: Push,
    },
    /// Whether the user may record calls, as FCP said.
    MayRecord {
        on: bool,
    },
    /// The contacts changed, on this device or on the account.
    Contacts {
        count: u32,
    },
    /// The user's listing in the tenant's directory, as FCP said.
    Directory {
        listing: Option<DirectoryListing>,
    },
    /// The user's presence, as FCP said.
    Presence {
        presence: Option<MyPresence>,
    },
    /// An audio test moved on (or, `None`, ended).
    AudioTest {
        test: Option<AudioTest>,
    },
}
