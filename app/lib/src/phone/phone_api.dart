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
  });

  final int jitterMs;
  final int lossPermille;
  final int? rttMs;

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
  );
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
    this.calling,
    this.recents = const [],
    this.people = const [],
    this.voicemail = const [],
    this.inputs = const [],
    this.outputs = const [],
    this.input,
    this.output,
    this.playing,
  });

  final String aor;
  final Registration registration;
  final List<PhoneCall> calls;
  final int voicemailNew;
  final bool? dnd;

  /// The tenant's brand: its app name, primary colour and logo.
  final String? brandName;
  final int? brandPrimary;
  final Uint8List? brandLogo;

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

  /// The voicemail message playing, by id.
  final String? playing;

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
    String? playing,
    bool clearPlaying = false,
  }) => PhoneSnapshot(
    aor: aor,
    registration: registration ?? this.registration,
    calls: calls ?? this.calls,
    voicemailNew: voicemailNew ?? this.voicemailNew,
    dnd: dnd ?? this.dnd,
    brandName: brandName ?? this.brandName,
    brandPrimary: brandPrimary ?? this.brandPrimary,
    brandLogo: brandLogo,
    calling: calling ?? this.calling,
    recents: recents ?? this.recents,
    people: people ?? this.people,
    voicemail: voicemail ?? this.voicemail,
    inputs: inputs ?? this.inputs,
    outputs: outputs ?? this.outputs,
    input: clearInput ? null : input ?? this.input,
    output: clearOutput ? null : output ?? this.output,
    playing: clearPlaying ? null : playing ?? this.playing,
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

class SignedInAccount {
  const SignedInAccount(this.username, this.server);
  final String username;
  final String server;
}

/// Everything the app asks of the phone.
abstract class PhoneApi {
  Future<SignedInAccount?> savedAccount();
  Future<SignedInAccount> signIn({
    required String place,
    required String username,
    required String password,
    String? code,
  });
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

  Future<void> setDnd(bool on);
  Future<void> setCalling(CallingChange change);
  Future<void> markHeard(String id);
  Future<void> deleteVoicemail(String id);
  Future<void> play(String id);
  Future<void> stopPlaying();
  Future<void> favourite(String who, bool on);

  /// Use this microphone (`input`) or speaker by id, null for the system's
  /// default.
  Future<void> chooseAudio({required bool input, String? device});
  Future<void> refresh();
}
