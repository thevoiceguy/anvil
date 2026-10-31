// The phone as the screens see it: an interface over `anvil-app`, so a
// screen never touches the bindings and a test can stand in for the phone.

import 'dart:async';
import 'dart:typed_data';

enum Registration { unregistered, registering, registered, failed }

enum Direction { incoming, outgoing }

enum CallState { dialing, ringing, connected }

/// A connected call's media, as last measured (every second).
class CallQuality {
  const CallQuality({
    required this.jitterMs,
    required this.lossPermille,
    this.rttMs,
    this.packetsReceived = 0,
    this.packetsPlayed = 0,
    this.packetsDropped = 0,
    this.unexpectedPt,
    this.expectedPt,
    this.lastSource,
    this.recentLossPermille = 0,
    this.packetsSent = 0,
    this.bytesSent = 0,
    this.rtpReceived = 0,
    this.bytesReceived = 0,
    this.packetsExpected = 0,
    this.packetsLost = 0,
    this.sendKbps = 0,
    this.recvKbps = 0,
    this.sendPps = 0,
    this.recvPps = 0,
    this.farLossPermille,
    this.farJitterMs,
    this.farLost,
    this.rtcp = false,
    this.rtcpSent = 0,
    this.rtcpReceived = 0,
    this.clockRate = 0,
    this.ptimeMs = 0,
    this.localRtp,
    this.remoteRtp,
    this.mosHundredths = 0,
    this.farMosHundredths,
  });

  final int jitterMs;

  /// The far end's packets lost over the whole call, per thousand.
  final int lossPermille;
  final int? rttMs;

  /// RTP packets received since the call began, of any kind.
  final int packetsReceived;

  /// Audio frames played.
  final int packetsPlayed;

  /// Packets dropped: the wrong payload type, failed SRTP, not RTP.
  final int packetsDropped;

  /// The payload type of the last packet dropped for it, and the one the
  /// call's codec is sent with.
  final int? unexpectedPt;
  final int? expectedPt;

  /// Where the last packet came from (`address:port`).
  final String? lastSource;

  /// The far end's packets lost over the last five seconds, per thousand.
  final int recentLossPermille;

  final int packetsSent;
  final int bytesSent;

  /// RTP received, played or not.
  final int rtpReceived;
  final int bytesReceived;

  /// The far end's packets its sequence numbers say were sent, and those
  /// that never arrived.
  final int packetsExpected;
  final int packetsLost;

  /// What flowed in the last second.
  final int sendKbps;
  final int recvKbps;
  final int sendPps;
  final int recvPps;

  /// What the far end last reported of ours over RTCP: its loss in its
  /// last interval per thousand, its jitter, and its losses in all.
  final int? farLossPermille;
  final int? farJitterMs;
  final int? farLost;

  /// The call has RTCP; the reports sent and read.
  final bool rtcp;
  final int rtcpSent;
  final int rtcpReceived;

  /// The codec's clock in Hz and its audio per packet in ms.
  final int clockRate;
  final int ptimeMs;

  /// Where media flows between (`address:port`).
  final String? localRtp;
  final String? remoteRtp;

  /// The estimated score (E-model MOS, 1 to 4.5) in hundredths: what the
  /// user hears (0 before it is measured), and what the far end hears once
  /// it has reported.
  final int mosHundredths;
  final int? farMosHundredths;

  /// The lower of the two scores, once measured.
  double? get mos {
    if (mosHundredths == 0) return null;
    final far = farMosHundredths;
    final worst = far != null && far < mosHundredths ? far : mosHundredths;
    return worst / 100;
  }

  /// Good, fair or poor, as a person would hear it: by the score (FCP calls
  /// a call under 3.1 poor), else by loss, jitter and the round trip.
  QualityLevel get level {
    if (mos case final m?) {
      return m >= 4.0
          ? QualityLevel.good
          : m >= 3.1
          ? QualityLevel.fair
          : QualityLevel.poor;
    }
    if (lossPermille >= 50 || jitterMs >= 60 || (rttMs ?? 0) >= 400) {
      return QualityLevel.poor;
    }
    if (lossPermille >= 10 || jitterMs >= 30 || (rttMs ?? 0) >= 250) {
      return QualityLevel.fair;
    }
    return QualityLevel.good;
  }
}

enum QualityLevel { good, fair, poor }

class PhoneCall {
  const PhoneCall({
    required this.id,
    required this.direction,
    required this.remote,
    this.displayName,
    required this.state,
    this.held = false,
    this.muted = false,
    this.codec,
    this.encrypted = false,
    this.quality,
    this.connectedAt,
    this.recording = false,
    this.line,
    this.silent = false,
    this.capturing = false,
  });

  final int id;
  final Direction direction;
  final String remote;
  final String? displayName;
  final CallState state;
  final bool held;
  final bool muted;
  final String? codec;
  final bool encrypted;
  final CallQuality? quality;

  /// When it connected: what the call's timer counts from.
  final DateTime? connectedAt;

  /// The user is recording it.
  final bool recording;

  /// The line it is on, by key (`own`, `extension:1005`, …); null without
  /// lines.
  final String? line;

  /// Ringing on a line that does not ring this app: shown, not sounded.
  final bool silent;

  /// The call is being captured.
  final bool capturing;

  /// Who the call is with, as a person reads it.
  String get who => displayName ?? shortAddress(remote);

  bool get isRingingIn =>
      direction == Direction.incoming && state == CallState.ringing;

  bool get isConnected => state == CallState.connected;

  PhoneCall copyWith({
    CallState? state,
    bool? held,
    bool? muted,
    String? codec,
    bool? encrypted,
    CallQuality? quality,
    DateTime? connectedAt,
    bool? recording,
  }) => PhoneCall(
    id: id,
    direction: direction,
    remote: remote,
    displayName: displayName,
    state: state ?? this.state,
    held: held ?? this.held,
    muted: muted ?? this.muted,
    codec: codec ?? this.codec,
    encrypted: encrypted ?? this.encrypted,
    quality: quality ?? this.quality,
    connectedAt: connectedAt ?? this.connectedAt,
    recording: recording ?? this.recording,
    line: line,
    silent: silent,
    capturing: capturing,
  );
}

/// Which calls the user captures.
enum CaptureMode {
  off,
  nextCall,
  everyCall;

  static CaptureMode parse(String mode) => switch (mode) {
    'next_call' => nextCall,
    'every_call' => everyCall,
    _ => off,
  };

  String get wire => switch (this) {
    off => 'off',
    nextCall => 'next_call',
    everyCall => 'every_call',
  };
}

/// Capturing calls (FCP's `docs/CALL_DIAGNOSTICS.md` §3): only while the
/// user's organisation allows this app.
class CaptureInfo {
  const CaptureInfo({
    this.allowed = false,
    this.until,
    this.maxMb = 50,
    this.mode = CaptureMode.off,
    this.saved = 0,
    this.savedBytes = 0,
    this.folder,
  });

  /// The organisation allows this app to capture: the setting shows only
  /// then.
  final bool allowed;
  final DateTime? until;
  final int maxMb;
  final CaptureMode mode;

  /// The captures kept on this device, and their size.
  final int saved;
  final int savedBytes;
  final String? folder;
}

/// One of the account's lines, as on a desk phone (FCP's
/// `docs/DEVICE_LINES.md`): the user's own, another extension, a shared
/// line, or one they act for.
class PhoneLine {
  const PhoneLine({
    required this.line,
    this.kind = 'own',
    this.aor = '',
    required this.label,
    this.defaultLabel = '',
    this.registration = Registration.registered,
    this.voicemailNew,
    this.ring = true,
    this.shared = false,
    this.appearances = 0,
    this.bargeIn = false,
    this.inUse = const [],
  });

  /// Its key: what a call on it is named by, and a name is set under.
  final String line;

  /// `own`, `extension`, `shared` or `on_behalf`.
  final String kind;
  final String aor;

  /// What it is called: the user's name, the administrator's, else
  /// [defaultLabel].
  final String label;
  final String defaultLabel;
  final Registration registration;

  /// New messages in the line's mailbox, once it has said.
  final int? voicemailNew;

  /// Calls to it ring this app.
  final bool ring;
  final bool shared;
  final int appearances;

  /// A shared line whose active calls the app may join.
  final bool bargeIn;

  /// A shared line's appearances in use, by index.
  final List<LineAppearance> inUse;

  bool get isOwn => line == 'own';

  /// The user has named it something other than its label.
  bool get renamed => label != defaultLabel;

  PhoneLine copyWith({
    String? label,
    Registration? registration,
    List<LineAppearance>? inUse,
  }) => PhoneLine(
    line: line,
    kind: kind,
    aor: aor,
    label: label ?? this.label,
    defaultLabel: defaultLabel,
    registration: registration ?? this.registration,
    voicemailNew: voicemailNew,
    ring: ring,
    shared: shared,
    appearances: appearances,
    bargeIn: bargeIn,
    inUse: inUse ?? this.inUse,
  );
}

/// One appearance of a shared line in use, as FCP last said.
class LineAppearance {
  const LineAppearance({
    required this.index,
    required this.state,
    this.remote,
    this.ours = false,
  });

  /// 1 for the first.
  final int index;

  /// `alerting`, `progressing`, `active`, `held` or `seized`.
  final String state;

  /// The far party, when FCP names one (`"Bob" <sip:bob@x>`).
  final String? remote;

  /// One of this app's own calls.
  final bool ours;

  bool get held => state == 'held';
  bool get active => state == 'active';

  /// The far party as a person reads it: the name, else the number.
  String? get who {
    final r = remote;
    if (r == null) return null;
    final name = RegExp(r'^\s*"((?:[^"\\]|\\.)*)"').firstMatch(r)?.group(1);
    if (name != null && name.isNotEmpty) return name.replaceAll(r'\', '');
    final lt = r.indexOf('<'), gt = r.lastIndexOf('>');
    return shortAddress(lt >= 0 && gt > lt ? r.substring(lt + 1, gt) : r);
  }
}

/// A contact the user keeps: on this device, or on their FCP account where
/// every app they sign in to has it.
class Contact {
  const Contact({
    this.id = '',
    this.onAccount = false,
    required this.name,
    this.company,
    this.numbers = const [],
    this.email,
    this.notes,
    this.ringtone,
  });

  /// Empty in a new one; the phone (or FCP) gives it one when saved.
  final String id;

  /// Kept on the user's FCP account. Saving a device's contact with this
  /// set moves it there.
  final bool onAccount;
  final String name;
  final String? company;
  final List<ContactNumber> numbers;
  final String? email;
  final String? notes;

  /// The ringtone a call from one of its numbers rings with, by id; null
  /// (or one this device does not have) rings with the phone's.
  final String? ringtone;

  /// What to dial: the first number, when it has one.
  String? get dial => numbers.isEmpty ? null : numbers.first.number;

  /// The same contact, kept on the account.
  Contact toAccount() => Contact(
    id: id,
    onAccount: true,
    name: name,
    company: company,
    numbers: numbers,
    email: email,
    notes: notes,
    ringtone: ringtone,
  );
}

/// What a contacts import did.
class ContactsImported {
  const ContactsImported({
    this.imported = 0,
    this.duplicates = 0,
    this.skipped = const [],
  });

  /// Contacts made.
  final int imported;

  /// Entries already kept, or twice in the file: not made again.
  final int duplicates;

  /// Entries left out, each with its place in the file and why.
  final List<String> skipped;
}

/// One of a contact's numbers: what it is (mobile, work, …) and what to
/// dial.
class ContactNumber {
  const ContactNumber({this.label = '', required this.number});

  final String label;
  final String number;
}

/// `sip:1002@pbx.example.com` as a person reads it: `1002`.
String shortAddress(String uri) {
  final s = uri.replaceFirst(RegExp(r'^(sips?|tel):'), '');
  return s.split('@').first.split(';').first;
}

/// One call in the user's history.
class RecentCall {
  const RecentCall({
    required this.id,
    required this.direction,
    required this.remote,
    this.displayName,
    this.missed = false,
    required this.startedAt,
    this.durationSecs,
    this.mosHundredths,
    this.poor = false,
  });

  final String id;
  final Direction direction;

  /// Who the call was with: what to dial back.
  final String remote;
  final String? displayName;
  final bool missed;
  final DateTime? startedAt;
  final int? durationSecs;

  /// How it went, when one of the user's phones reported it: the lower
  /// score (MOS) in hundredths, and whether it was poor (under 3.1).
  final int? mosHundredths;
  final bool poor;

  String get who => displayName ?? shortAddress(remote);
}

/// Someone in the tenant's directory.
class Person {
  const Person({
    required this.key,
    required this.name,
    this.extension,
    this.department,
    this.jobTitle,
    this.presence,
    this.presenceLabel,
    this.onCall = false,
    this.favourite = false,
    required this.dial,
  });

  final String key;
  final String name;
  final String? extension;
  final String? department;
  final String? jobTitle;

  /// `available`, `busy`, `away`, `dnd`, `ringing`, `on_call`,
  /// `offline`, … when the tenant shows it.
  final String? presence;

  /// Their own status's label beside it ("WFH").
  final String? presenceLabel;

  /// Their busy lamp.
  final bool onCall;
  final bool favourite;

  /// What to dial to reach them.
  final String dial;

  bool matches(String query) {
    final q = query.trim().toLowerCase();
    if (q.isEmpty) return true;
    return name.toLowerCase().contains(q) ||
        (extension ?? '').contains(q) ||
        key.toLowerCase().contains(q) ||
        (department ?? '').toLowerCase().contains(q);
  }

  Person copyWith({bool? favourite, bool? onCall, String? presence}) => Person(
    key: key,
    name: name,
    extension: extension,
    department: department,
    jobTitle: jobTitle,
    presence: presence ?? this.presence,
    presenceLabel: presenceLabel,
    onCall: onCall ?? this.onCall,
    favourite: favourite ?? this.favourite,
    dial: dial,
  );
}

/// One of the user's own statuses: a label on a state.
class OwnStatus {
  const OwnStatus({required this.id, required this.label, required this.state});
  final String id;
  final String label;

  /// `available`, `away`, `busy` or `dnd`.
  final String state;
}

/// The user's presence (FCP's `docs/PRESENCE.md`): what others see, the
/// status they chose, and their own statuses to choose from.
class MyPresence {
  const MyPresence({
    required this.shown,
    this.shownLabel,
    this.idle = false,
    required this.status,
    required this.statusState,
    this.note,
    this.until,
    this.statuses = const [],
  });

  /// What others see: `available`, `away`, `busy`, `dnd`, `ringing`,
  /// `on_call` or `offline`.
  final String shown;
  final String? shownLabel;

  /// Away because one of the user's apps says they are idle.
  final bool idle;

  /// What they chose: `available`, `away`, `busy`, `dnd`, or one of
  /// [statuses]' ids.
  final String status;
  final String statusState;
  final String? note;

  /// When the chosen status clears back to available.
  final DateTime? until;
  final List<OwnStatus> statuses;

  /// The chosen status's own label, when it is one of theirs.
  String? get statusLabel {
    for (final s in statuses) {
      if (s.id == status) return s.label;
    }
    return null;
  }
}

/// One voicemail message.
class VoicemailMessage {
  const VoicemailMessage({
    required this.id,
    required this.caller,
    this.callerName,
    this.isNew = false,
    this.urgent = false,
    this.durationSecs = 0,
    this.transcription,
    this.receivedAt,
  });

  final String id;
  final String caller;
  final String? callerName;
  final bool isNew;
  final bool urgent;
  final int durationSecs;
  final String? transcription;
  final DateTime? receivedAt;

  String get who => callerName ?? shortAddress(caller);

  VoicemailMessage copyWith({bool? isNew}) => VoicemailMessage(
    id: id,
    caller: caller,
    callerName: callerName,
    isNew: isNew ?? this.isNew,
    urgent: urgent,
    durationSecs: durationSecs,
    transcription: transcription,
    receivedAt: receivedAt,
  );
}

/// A microphone or a speaker.
class AudioDevice {
  const AudioDevice({
    required this.id,
    required this.name,
    this.isDefault = false,
  });
  final String id;
  final String name;
  final bool isDefault;
}

/// What an audio test is doing.
enum AudioTest {
  recording,
  playingBack,
  speaker,
  ring;

  static AudioTest? parse(String? stage) => switch (stage) {
    'recording' => recording,
    'playing_back' => playingBack,
    'speaker' => speaker,
    'ring' => ring,
    _ => null,
  };
}

/// A ringtone to choose: Anvil's own (`anvil`, `classic`) or the brand's
/// (`brand:` and its name).
class Ringtone {
  const Ringtone({required this.id, required this.name});
  final String id;
  final String name;

  bool get isBrand => id.startsWith('brand:');
}

/// The user's calling settings, as FCP has them.
class CallingSettings {
  const CallingSettings({
    this.dnd = false,
    this.callWaiting = true,
    this.forwardAll,
    this.forwardBusy,
    this.forwardNoAnswer,
    this.forwardUnreachable,
    this.noAnswerSecs,
  });

  final bool dnd;
  final bool callWaiting;
  final String? forwardAll;
  final String? forwardBusy;
  final String? forwardNoAnswer;
  final String? forwardUnreachable;
  final int? noAnswerSecs;

  String? forward(Forward when) => switch (when) {
    Forward.all => forwardAll,
    Forward.busy => forwardBusy,
    Forward.noAnswer => forwardNoAnswer,
    Forward.unreachable => forwardUnreachable,
  };
}

/// When a call is forwarded.
enum Forward { all, busy, noAnswer, unreachable }

/// A change to the calling settings: each field left null is kept, and an
/// empty forward clears it.
class CallingChange {
  const CallingChange({
    this.callWaiting,
    this.forwardAll,
    this.forwardBusy,
    this.forwardNoAnswer,
    this.forwardUnreachable,
    this.noAnswerSecs,
  });

  /// The forward `when` set to `to` (empty clears it).
  factory CallingChange.forward(Forward when, String to) => switch (when) {
    Forward.all => CallingChange(forwardAll: to),
    Forward.busy => CallingChange(forwardBusy: to),
    Forward.noAnswer => CallingChange(forwardNoAnswer: to),
    Forward.unreachable => CallingChange(forwardUnreachable: to),
  };

  final bool? callWaiting;
  final String? forwardAll;
  final String? forwardBusy;
  final String? forwardNoAnswer;
  final String? forwardUnreachable;
  final int? noAnswerSecs;
}

class PhoneSnapshot {
  const PhoneSnapshot({
    required this.aor,
    required this.registration,
    this.lines = const [],
    this.mayRenameLines = false,
    this.calls = const [],
    this.voicemailNew = 0,
    this.dnd,
    this.brandName,
    this.brandPrimary,
    this.brandLogo,
    this.brandIcon,
    this.calling,
    this.recents = const [],
    this.people = const [],
    this.voicemail = const [],
    this.inputs = const [],
    this.outputs = const [],
    this.input,
    this.output,
    this.ringDevice,
    this.ringtones = const [],
    this.ringtone = '',
    this.echoCancellation,
    this.noiseSuppression,
    this.microphoneVolume,
    this.speakerVolume,
    this.playing,
    this.mayRecord = false,
    this.contacts = const [],
    this.directoryListed,
    this.mayChangeDirectoryListing = false,
    this.presence,
    this.audioTest,
    this.audioTestLevel = 0,
    this.capture = const CaptureInfo(),
  });

  final String aor;
  final Registration registration;

  /// The account's lines, its own first; empty without FCP (or from an
  /// older one): the account alone.
  final List<PhoneLine> lines;

  /// The tenant lets the user name their lines.
  final bool mayRenameLines;
  final List<PhoneCall> calls;
  final int voicemailNew;
  final bool? dnd;

  /// The tenant's brand: its app name, primary colour, logo and icon (for
  /// the system's icons).
  final String? brandName;
  final int? brandPrimary;
  final Uint8List? brandLogo;
  final Uint8List? brandIcon;

  /// Signed in to FCP: the calling settings, once read.
  final CallingSettings? calling;

  /// The latest calls, newest first.
  final List<RecentCall> recents;
  final List<Person> people;

  /// The mailbox's messages, newest first.
  final List<VoicemailMessage> voicemail;
  final List<AudioDevice> inputs;
  final List<AudioDevice> outputs;

  /// The devices chosen; null is the system's default.
  final String? input;
  final String? output;

  /// The speaker chosen to ring on; null is the system's default.
  final String? ringDevice;

  /// Anvil's ringtones, then the brand's; the one that rings, by id.
  final List<Ringtone> ringtones;
  final String ringtone;

  /// Echo cancellation and noise suppression on or off; null when the
  /// phone's audio does none of its own.
  final bool? echoCancellation;
  final bool? noiseSuppression;

  /// The microphone's and the speaker's volume in percent (100 the sound
  /// as it is, up to 200); null when the system sets them.
  final int? microphoneVolume;
  final int? speakerVolume;

  /// The voicemail message playing, by id.
  final String? playing;

  /// The user may record calls (FCP says so): the record button shows.
  final bool mayRecord;

  /// The user's contacts, by name: on this device and, signed in to FCP,
  /// on their account.
  final List<Contact> contacts;

  /// Signed in to FCP: whether the user is listed in the tenant's
  /// directory; null until FCP says.
  final bool? directoryListed;

  /// The tenant lets the user take themselves out of the directory.
  final bool mayChangeDirectoryListing;

  /// Signed in to an FCP that keeps it: the user's presence.
  final MyPresence? presence;

  /// A test of the microphone or a speaker under way, null when none is.
  final AudioTest? audioTest;

  /// While the microphone is recorded: how loud it is, 0 to 100.
  final int audioTestLevel;

  /// Capturing calls.
  final CaptureInfo capture;

  /// Signed in to FCP, so a contact can be kept on the account.
  bool get hasAccount => calling != null;

  /// The contact one of whose numbers a call's address is.
  Contact? contactFor(String remote) {
    final dialled = shortAddress(remote);
    for (final c in contacts) {
      if (c.numbers.any((n) => n.number == dialled)) return c;
    }
    return null;
  }

  /// More lines than the account's own: the screens show them.
  bool get hasLines => lines.length > 1;

  /// A line by key, when the account has it.
  PhoneLine? lineOf(String? key) {
    if (key == null) return null;
    for (final l in lines) {
      if (l.line == key) return l;
    }
    return null;
  }

  /// The calls on a line: ones FCP named no line for are the account's own.
  List<PhoneCall> callsOn(String key) =>
      calls.where((c) => (c.line ?? 'own') == key).toList();

  int get missedCalls => recents.where((r) => r.missed).length;

  /// New messages: the mailbox's own count once it has been read, else
  /// message waiting's.
  int get newVoicemail =>
      voicemail.isEmpty ? voicemailNew : voicemail.where((m) => m.isNew).length;

  /// The person a call's address names, when they are in the directory.
  Person? personFor(String remote) {
    final user = shortAddress(remote);
    for (final p in people) {
      if (p.key == user || p.extension == user) return p;
    }
    return null;
  }

  /// `#RRGGBB` as an opaque ARGB value.
  static int? colorFromHex(String? hex) {
    if (hex == null || !RegExp(r'^#[0-9a-fA-F]{6}').hasMatch(hex)) return null;
    return 0xFF000000 | int.parse(hex.substring(1, 7), radix: 16);
  }

  PhoneSnapshot copyWith({
    Registration? registration,
    List<PhoneLine>? lines,
    bool? mayRenameLines,
    List<PhoneCall>? calls,
    int? voicemailNew,
    bool? dnd,
    String? brandName,
    int? brandPrimary,
    Uint8List? brandIcon,
    bool clearBrand = false,
    CallingSettings? calling,
    List<RecentCall>? recents,
    List<Person>? people,
    List<VoicemailMessage>? voicemail,
    List<AudioDevice>? inputs,
    List<AudioDevice>? outputs,
    String? input,
    String? output,
    bool clearInput = false,
    bool clearOutput = false,
    String? ringDevice,
    bool clearRingDevice = false,
    List<Ringtone>? ringtones,
    String? ringtone,
    bool? echoCancellation,
    bool? noiseSuppression,
    int? microphoneVolume,
    int? speakerVolume,
    String? playing,
    bool clearPlaying = false,
    bool? mayRecord,
    List<Contact>? contacts,
    bool? directoryListed,
    bool? mayChangeDirectoryListing,
    MyPresence? presence,
    AudioTest? audioTest,
    bool clearAudioTest = false,
    int? audioTestLevel,
    CaptureInfo? capture,
  }) => PhoneSnapshot(
    aor: aor,
    registration: registration ?? this.registration,
    lines: lines ?? this.lines,
    mayRenameLines: mayRenameLines ?? this.mayRenameLines,
    calls: calls ?? this.calls,
    voicemailNew: voicemailNew ?? this.voicemailNew,
    dnd: dnd ?? this.dnd,
    brandName: clearBrand ? null : brandName ?? this.brandName,
    brandPrimary: clearBrand ? null : brandPrimary ?? this.brandPrimary,
    brandLogo: clearBrand ? null : brandLogo,
    brandIcon: clearBrand ? null : brandIcon ?? this.brandIcon,
    calling: calling ?? this.calling,
    recents: recents ?? this.recents,
    people: people ?? this.people,
    voicemail: voicemail ?? this.voicemail,
    inputs: inputs ?? this.inputs,
    outputs: outputs ?? this.outputs,
    input: clearInput ? null : input ?? this.input,
    output: clearOutput ? null : output ?? this.output,
    ringDevice: clearRingDevice ? null : ringDevice ?? this.ringDevice,
    ringtones: ringtones ?? this.ringtones,
    ringtone: ringtone ?? this.ringtone,
    echoCancellation: echoCancellation ?? this.echoCancellation,
    noiseSuppression: noiseSuppression ?? this.noiseSuppression,
    microphoneVolume: microphoneVolume ?? this.microphoneVolume,
    speakerVolume: speakerVolume ?? this.speakerVolume,
    playing: clearPlaying ? null : playing ?? this.playing,
    mayRecord: mayRecord ?? this.mayRecord,
    contacts: contacts ?? this.contacts,
    directoryListed: directoryListed ?? this.directoryListed,
    mayChangeDirectoryListing:
        mayChangeDirectoryListing ?? this.mayChangeDirectoryListing,
    presence: presence ?? this.presence,
    audioTest: clearAudioTest ? null : audioTest ?? this.audioTest,
    audioTestLevel: audioTestLevel ?? this.audioTestLevel,
    capture: capture ?? this.capture,
  );

  static const empty = PhoneSnapshot(
    aor: '',
    registration: Registration.unregistered,
  );
}

/// A change the screen may announce; the snapshot says the rest.
class PhoneEvent {
  const PhoneEvent(this.kind, {this.call, this.reason});
  final String kind;
  final int? call;
  final String? reason;
}

/// How a managed install's sign-in goes on (`docs/MANAGED_INSTALLS.md`).
enum ManagedSignIn { auto, sso, password }

/// Whether a managed install updates itself.
enum ManagedUpdates { auto, notify, off }

/// What the company set this install up with: IT's policy over the
/// profile installed with the app. An ordinary install's is [none].
class Managed {
  const Managed({
    this.server,
    this.lockServer = false,
    this.loginDomain,
    this.signIn = ManagedSignIn.auto,
    this.username,
    this.updates = ManagedUpdates.auto,
    this.brandName,
    this.brandPrimary,
    this.brandLogo,
    this.brandIcon,
    this.brandIconPath,
    this.sources = const [],
  });

  static const none = Managed();

  /// What the server field holds.
  final String? server;

  /// The server field cannot be changed.
  final bool lockServer;

  /// Put after a bare username: `james` signs in as `james@acme.example`.
  final String? loginDomain;
  final ManagedSignIn signIn;

  /// The username to fill in, from the system.
  final String? username;
  final ManagedUpdates updates;

  /// The installed profile's brand, the app's before FCP's.
  final String? brandName;
  final int? brandPrimary;
  final Uint8List? brandLogo;
  final Uint8List? brandIcon;

  /// The brand's icon file for the system (Windows' notifications).
  final String? brandIconPath;

  /// Where the settings came from.
  final List<String> sources;

  bool get isManaged => sources.isNotEmpty;

  /// The username to sign in with: [loginDomain] after a bare one.
  String login(String username) {
    final user = username.trim();
    final domain = loginDomain;
    if (domain == null || user.isEmpty || user.contains('@')) return user;
    return '$user@$domain';
  }
}

class SignedInAccount {
  const SignedInAccount(this.username, this.server);
  final String username;
  final String server;
}

/// The last server (or email address) and username signed in with, which
/// the sign-in screen comes back filled in with. Never a password.
class RememberedSignIn {
  const RememberedSignIn(this.place, this.username);
  final String place;
  final String username;
}

/// How a person signs in, as the server says (`POST /auth/discover`).
class SignInMethods {
  const SignInMethods({
    required this.password,
    required this.sso,
    this.known = true,
  });

  /// A password is taken.
  final bool password;

  /// Single sign-on through the browser is offered.
  final bool sso;

  /// The server said; an older server has no way to ask, and both ways are
  /// offered.
  final bool known;
}

/// Everything the app asks of the phone.
abstract class PhoneApi {
  Future<SignedInAccount?> savedAccount();

  /// The last sign-in, remembered across signing out.
  Future<RememberedSignIn?> lastSignIn();

  /// What this install was set up with; [Managed.none] for an ordinary one.
  Managed get managed;

  /// Ask the server how `username` signs in (`place` is the server or an
  /// email address; empty when the username is an email address).
  Future<SignInMethods> signInMethods({
    required String place,
    required String username,
  });

  /// Sign in with a password. Null: the password was right and the server
  /// wants a one-time code next ([signInWithCode]).
  Future<SignedInAccount?> signIn({
    required String place,
    required String username,
    required String password,
  });

  /// Finish a password sign-in with its one-time code; a wrong code may be
  /// tried again.
  Future<SignedInAccount> signInWithCode(String code);

  /// Whether this platform signs in through the system browser (single
  /// sign-on): a desktop, whose browser comes back to the app's loopback
  /// listener.
  bool get canSignInWithBrowser;

  /// Start signing in through the browser: the address to open.
  Future<String> startBrowserSignIn({
    required String place,
    String username = '',
  });

  /// Wait for the browser to come back, and keep the session.
  Future<SignedInAccount> finishBrowserSignIn();

  /// Give up the browser sign-in under way.
  Future<void> cancelBrowserSignIn();
  Future<void> start();

  /// Stop the phone (it unregisters); the session stays signed in.
  Future<void> stop();
  Future<void> signOut();

  PhoneSnapshot snapshot();
  Stream<PhoneEvent> events();

  /// Dial from one of the lines by key; null is the account's own.
  Future<int> call(String target, {String? line});

  /// Name one of the lines, where the tenant allows it; empty gives it
  /// back its administrator's label.
  Future<void> renameLine(String line, String label);

  /// Take appearance `index` of a shared line: pick up the call held
  /// there, or join the one up where the line allows barging in.
  Future<int> takeAppearance(String line, int index);
  Future<void> answer([int? call]);
  Future<void> decline([int? call]);
  Future<void> hangup([int? call]);
  Future<void> hold(int call, bool on);
  Future<void> mute(int call, bool on);
  Future<void> sendDigits(int call, String digits);

  /// Blind-transfer `call` to a number, an extension or an address.
  Future<void> transfer(int call, String target);

  /// Join `call`'s party to `to`'s, ending both of ours.
  Future<void> transferAttended(int call, int to);
  Future<void> park(int call);

  /// Record `call` (`on`), or stop the recording the user started; FCP
  /// plays the recording announcement to both parties first.
  Future<void> record(int call, bool on);

  /// Keep a contact: a new one (empty id) or a change, on this device or
  /// on the account (`onAccount`; a device's contact saved so moves).
  Future<void> saveContact(Contact contact);

  /// Delete a contact, from this device or from the account.
  Future<void> deleteContact(String id);

  /// Read the contacts in a vCard or CSV file into this device's contacts
  /// or (`onAccount`) the account's; one already kept is not made again.
  Future<ContactsImported> importContacts(
    String path, {
    required bool onAccount,
  });

  /// Write every contact to a file: CSV when its name ends `.csv`, else
  /// vCard.
  Future<void> exportContacts(String path);

  /// Be listed in the tenant's directory, or not (when the tenant lets the
  /// user choose).
  Future<void> setDirectoryListed(bool on);

  /// Capture the next call, every call, or none; refused unless the
  /// organisation allows it.
  Future<void> setCapture(CaptureMode mode);

  /// Remove the captures kept on this device.
  Future<void> deleteCaptures();

  /// Pick a status: a built-in one (`available`, `away`, `busy`, `dnd`) or
  /// one of the user's own by id, with a note and a time it clears at. A
  /// `dnd` status turns do not disturb on; any other turns it off.
  Future<void> setPresence(String status, {String? note, DateTime? until});

  /// Set the note beside the user's status, or clear it (null).
  Future<void> setPresenceNote(String? note);

  /// Say the user is idle (no keyboard or mouse for a while), or back.
  Future<void> setPresenceIdle(bool idle);

  /// Keep, change, delete and order the user's own statuses.
  Future<void> addStatus(String label, String state);
  Future<void> editStatus(String id, {String? label, String? state});
  Future<void> removeStatus(String id);
  Future<void> orderStatuses(List<String> ids);

  Future<void> setDnd(bool on);
  Future<void> setCalling(CallingChange change);
  Future<void> markHeard(String id);
  Future<void> deleteVoicemail(String id);
  Future<void> play(String id);
  Future<void> stopPlaying();
  Future<void> favourite(String who, bool on);

  /// Clear the recent calls shown on this device (FCP keeps the records).
  Future<void> clearRecents();

  /// Remove one recent call, by id, from those shown on this device.
  Future<void> hideRecent(String id);

  /// Use this microphone (`input`) or speaker by id, null for the system's
  /// default.
  Future<void> chooseAudio({required bool input, String? device});

  /// Ring on this speaker by id, null for the system's default.
  Future<void> chooseRingDevice(String? device);

  /// Ring with this ringtone by id, null for the brand's (else Anvil's);
  /// heard once now when no call is up.
  Future<void> chooseRingtone(String? id);

  /// Add a ringtone from a sound file (WAV, Ogg or MP3): kept with the
  /// app's files, chosen and heard once.
  Future<void> addRingtoneFile(String path);

  /// Remove a ringtone added from a file.
  Future<void> removeRingtone(String id);

  /// Test the microphone (recorded a few seconds, its level shown, then
  /// heard back) or a speaker (`ring`: the one that rings, with the
  /// ringtone).
  Future<void> testMicrophone();
  Future<void> testSpeaker({required bool ring});
  Future<void> stopAudioTest();

  /// Turn echo cancellation or noise suppression on or off (null leaves it
  /// as it is), in the call that is up too.
  Future<void> setAudioProcessing({
    bool? echoCancellation,
    bool? noiseSuppression,
  });

  /// Set the microphone's or the speaker's volume in percent (null leaves
  /// it as it is), in the call that is up too.
  Future<void> setAudioVolume({int? microphone, int? speaker});
  Future<void> refresh();

  /// The app's log file, once it is written: what a user sends when a call
  /// goes wrong.
  String? get logPath;
}
