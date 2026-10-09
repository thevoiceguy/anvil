// The real phone: `anvil-app` through flutter_rust_bridge.

import 'dart:async';

import 'package:path_provider/path_provider.dart';

import '../rust/api/phone.dart' as rust;
import 'phone_api.dart';

class RustPhone implements PhoneApi {
  String? _sessionPath;

  Future<String> _session() async {
    if (_sessionPath != null) return _sessionPath!;
    final dir = await getApplicationSupportDirectory();
    return _sessionPath = '${dir.path}/session.json';
  }

  @override
  Future<SignedInAccount?> savedAccount() async {
    final a = await rust.savedAccount(sessionPath: await _session());
    return a == null ? null : SignedInAccount(a.username, a.server);
  }

  @override
  Future<SignedInAccount> signIn({
    required String place,
    required String username,
    required String password,
    String? code,
  }) async {
    final a = await rust.signIn(
      place: place,
      username: username,
      password: password,
      totp: code,
      sessionPath: await _session(),
    );
    return SignedInAccount(a.username, a.server);
  }

  @override
  Future<void> start() async => rust.startPhone(sessionPath: await _session());

  @override
  Future<void> signOut() async => rust.signOut(sessionPath: await _session());

  @override
  PhoneSnapshot snapshot() {
    final s = rust.phoneState();
    final calling = s.calling;
    return PhoneSnapshot(
      aor: s.aor,
      registration: Registration.values[s.registration.index],
      calls: s.calls.map(_call).toList(),
      voicemailNew: s.voicemailNew,
      dnd: s.dnd,
      brandName: s.brandName,
      brandPrimary: PhoneSnapshot.colorFromHex(s.brandPrimary),
      brandLogo: s.brandLogo,
      calling: calling == null
          ? null
          : CallingSettings(
              dnd: calling.dnd,
              callWaiting: calling.callWaiting,
              forwardAll: calling.forwardAll,
              forwardBusy: calling.forwardBusy,
              forwardNoAnswer: calling.forwardNoAnswer,
              forwardUnreachable: calling.forwardUnreachable,
              noAnswerSecs: calling.noAnswerSecs,
            ),
      recents: s.recents
          .map(
            (r) => RecentCall(
              id: r.id,
              direction: Direction.values[r.direction.index],
              remote: r.remote,
              displayName: r.displayName,
              missed: r.missed,
              startedAt: DateTime.tryParse(r.startedAt)?.toLocal(),
              durationSecs: r.durationSecs?.toInt(),
            ),
          )
          .toList(),
      people: s.people
          .map(
            (p) => Person(
              key: p.key,
              name: p.name,
              extension: p.extension_,
              department: p.department,
              jobTitle: p.jobTitle,
              presence: p.presence,
              onCall: p.onCall,
              favourite: p.favourite,
              dial: p.dial,
            ),
          )
          .toList(),
      voicemail: s.voicemail
          .map(
            (m) => VoicemailMessage(
              id: m.id,
              caller: m.caller,
              callerName: m.callerName,
              isNew: m.new_,
              urgent: m.urgent,
              durationSecs: m.durationSecs.toInt(),
              transcription: m.transcription,
              receivedAt: DateTime.tryParse(m.receivedAt)?.toLocal(),
            ),
          )
          .toList(),
      inputs: s.inputs.map(_device).toList(),
      outputs: s.outputs.map(_device).toList(),
      input: s.input,
      output: s.output,
      playing: s.playing,
    );
  }

  static PhoneCall _call(rust.Call c) {
    final q = c.quality;
    final at = c.connectedAt;
    return PhoneCall(
      id: c.id.toInt(),
      direction: Direction.values[c.direction.index],
      remote: c.remote,
      displayName: c.displayName,
      state: CallState.values[c.state.index],
      held: c.held,
      muted: c.muted,
      codec: c.codec,
      encrypted: c.encrypted,
      quality: q == null
          ? null
          : CallQuality(
              jitterMs: q.jitterMs,
              lossPermille: q.packetLossPermille,
              rttMs: q.rttMs,
            ),
      connectedAt: at == null
          ? null
          : DateTime.fromMillisecondsSinceEpoch(at.toInt() * 1000),
    );
  }

  static AudioDevice _device(rust.AudioDevice d) =>
      AudioDevice(id: d.id, name: d.name, isDefault: d.default_);

  static BigInt? _id(int? call) => call == null ? null : BigInt.from(call);

  @override
  Stream<PhoneEvent> events() => rust.phoneChanges().map(
    (c) => PhoneEvent(c.kind, call: c.call?.toInt(), reason: c.reason),
  );

  @override
  Future<int> call(String target) async =>
      (await rust.placeCall(target: target)).toInt();

  @override
  Future<void> answer([int? call]) => rust.answer(call: _id(call));

  @override
  Future<void> decline([int? call]) => rust.decline(call: _id(call));

  @override
  Future<void> hangup([int? call]) => rust.hangup(call: _id(call));

  @override
  Future<void> hold(int call, bool on) => rust.hold(call: _id(call), on_: on);

  @override
  Future<void> mute(int call, bool on) => rust.mute(call: _id(call), on_: on);

  @override
  Future<void> sendDigits(int call, String digits) =>
      rust.sendDigits(call: _id(call), digits: digits);

  @override
  Future<void> transfer(int call, String target) =>
      rust.transfer(call: _id(call), target: target);

  @override
  Future<void> transferAttended(int call, int to) =>
      rust.transferAttended(call: BigInt.from(call), to: BigInt.from(to));

  @override
  Future<void> park(int call) => rust.park(call: _id(call));

  @override
  Future<void> setDnd(bool on) => rust.setDnd(on_: on);

  @override
  Future<void> setCalling(CallingChange change) => rust.setCalling(
    change: rust.CallingChange(
      callWaiting: change.callWaiting,
      forwardAll: change.forwardAll,
      forwardBusy: change.forwardBusy,
      forwardNoAnswer: change.forwardNoAnswer,
      forwardUnreachable: change.forwardUnreachable,
      noAnswerSecs: change.noAnswerSecs,
    ),
  );

  @override
  Future<void> markHeard(String id) => rust.markHeard(id: id);

  @override
  Future<void> deleteVoicemail(String id) => rust.deleteVoicemail(id: id);

  @override
  Future<void> play(String id) => rust.playVoicemail(id: id);

  @override
  Future<void> stopPlaying() => rust.stopPlaying();

  @override
  Future<void> favourite(String who, bool on) =>
      rust.favourite(who: who, on_: on);

  @override
  Future<void> chooseAudio({required bool input, String? device}) =>
      rust.chooseAudio(input: input, device: device);

  @override
  Future<void> refresh() => rust.refresh();
}
