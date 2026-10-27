// The phone as the screens see it: an interface over `anvil-app`, so a
// screen never touches the bindings and a test can stand in for the phone.

import 'dart:async';
import 'dart:typed_data';

enum Registration { unregistered, registering, registered, failed }

enum Direction { incoming, outgoing }

enum CallState { dialing, ringing, connected }

/// A connected call's media, as last measured.
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
  });

  final int jitterMs;
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

  /// Good, fair or poor, as a person would hear it.
  QualityLevel get level {
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

  /// Who the call is with, as a person reads it.
  String get who => displayName ?? shortAddress(remote);

  bool get isRingingIn =>
      direction == Direction.incoming && state == CallState.ringing;

  bool get isConnected => state == CallState.connected;

  PhoneCall copyWith({
    CallState? state,
    bool? held,
    bool? muted,
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
    codec: codec,
    encrypted: encrypted ?? this.encrypted,
    quality: quality ?? this.quality,
    connectedAt: connectedAt ?? this.connectedAt,
    recording: recording ?? this.recording,
  );
}

/// A contact kept on this device.
class Contact {
  const Contact({
    this.id = '',
    required this.name,
    this.company,
    this.numbers = const [],
    this.email,
    this.notes,
  });

  /// Empty in a new one; the phone gives it one when saved.
  final String id;
  final String name;
  final String? company;
  final List<ContactNumber> numbers;
  final String? email;
  final String? notes;

  /// What to dial: the first number, when it has one.
  String? get dial => numbers.isEmpty ? null : numbers.first.number;
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
  });

  final String id;
  final Direction direction;

  /// Who the call was with: what to dial back.
  final String remote;
  final String? displayName;
  final bool missed;
  final DateTime? startedAt;
  final int? durationSecs;

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
    this.onCall = false,
    this.favourite = false,
    required this.dial,
  });

  final String key;
  final String name;
  final String? extension;
  final String? department;
  final String? jobTitle;

  /// `available`, `busy`, `away`, `dnd`, `offline`, … when the tenant shows
  /// it.
  final String? presence;

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
    onCall: onCall ?? this.onCall,
    favourite: favourite ?? this.favourite,
    dial: dial,
  );
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
  });

  final String aor;
  final Registration registration;
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

  /// The contacts on this device, by name.
  final List<Contact> contacts;

  /// The contact one of whose numbers a call's address is.
  Contact? contactFor(String remote) {
    final dialled = shortAddress(remote);
    for (final c in contacts) {
      if (c.numbers.any((n) => n.number == dialled)) return c;
    }
    return null;
  }

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
  }) => PhoneSnapshot(
    aor: aor,
    registration: registration ?? this.registration,
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

  Future<int> call(String target);
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

  /// Keep a contact on this device: a new one (empty id) or a change.
  Future<void> saveContact(Contact contact);

  /// Forget a contact kept on this device.
  Future<void> deleteContact(String id);

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
