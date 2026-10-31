// The real phone: `anvil-app` through flutter_rust_bridge.

import 'dart:async';
import 'dart:io';

import 'package:path_provider/path_provider.dart';

import '../rust/api/phone.dart' as rust;
import 'phone_api.dart';

class RustPhone implements PhoneApi {
  String? _sessionPath;

  /// Where the app keeps its files: the session, settings, the log.
  static Future<String> directory() async {
    final dir = (await getApplicationSupportDirectory()).path;
    return Platform.isWindows ? await localOnWindows(dir) : dir;
  }

  Future<String> _session() async {
    if (_sessionPath != null) return _sessionPath!;
    final dir = await directory();
    try {
      // The log goes beside the session, from the first thing the app does.
      rust.startLog(dir: dir);
    } catch (_) {
      // No log file is no reason not to run.
    }
    return _sessionPath = '$dir${Platform.pathSeparator}session.json';
  }

  @override
  String? get logPath => rust.logPath();

  /// Windows keeps the app's files — the session's tokens, its settings —
  /// in the local profile (`%LOCALAPPDATA%`), not the roaming one, which
  /// follows the user to every machine of a domain. What an older version
  /// kept in the roaming profile is moved.
  static Future<String> localOnWindows(
    String roaming, {
    Map<String, String>? environment,
  }) async {
    final env = environment ?? Platform.environment;
    final from = env['APPDATA'], to = env['LOCALAPPDATA'];
    if (from == null || to == null || !roaming.startsWith(from)) return roaming;
    final local = Directory(to + roaming.substring(from.length));
    final old = Directory(roaming);
    final sep = Platform.pathSeparator;
    if (!File('${local.path}${sep}session.json').existsSync() &&
        File('$roaming${sep}session.json').existsSync()) {
      try {
        local.createSync(recursive: true);
        for (final entry in old.listSync()) {
          final name = entry.path.substring(old.path.length + 1);
          entry.renameSync('${local.path}$sep$name');
        }
      } on FileSystemException {
        // Left where it was: the user signs in again.
      }
    }
    local.createSync(recursive: true);
    return local.path;
  }

  @override
  Future<SignedInAccount?> savedAccount() async {
    final a = await rust.savedAccount(sessionPath: await _session());
    return a == null ? null : SignedInAccount(a.username, a.server);
  }

  @override
  Future<RememberedSignIn?> lastSignIn() async {
    final r = await rust.lastSignIn(sessionPath: await _session());
    return r == null ? null : RememberedSignIn(r.place, r.username);
  }

  @override
  Future<SignInMethods> signInMethods({
    required String place,
    required String username,
  }) async {
    final m = await rust.signInMethods(place: place, username: username);
    return SignInMethods(password: m.password, sso: m.sso, known: m.known);
  }

  @override
  Future<SignedInAccount?> signIn({
    required String place,
    required String username,
    required String password,
  }) async {
    final a = await rust.signIn(
      place: place,
      username: username,
      password: password,
      sessionPath: await _session(),
    );
    return a == null ? null : SignedInAccount(a.username, a.server);
  }

  @override
  Future<SignedInAccount> signInWithCode(String code) async {
    final a = await rust.signInWithCode(
      code: code,
      sessionPath: await _session(),
    );
    return SignedInAccount(a.username, a.server);
  }

  @override
  late final Managed managed = _managed();

  static Managed _managed() {
    final m = rust.managedSettings();
    return Managed(
      server: m.server,
      lockServer: m.lockServer,
      loginDomain: m.loginDomain,
      signIn: ManagedSignIn.values.byName(m.signIn),
      username: m.username,
      updates: ManagedUpdates.values.byName(m.updates),
      brandName: m.brandName,
      brandPrimary: PhoneSnapshot.colorFromHex(m.brandPrimary),
      brandLogo: m.brandLogo,
      brandIcon: m.brandIcon,
      brandIconPath: m.brandIconPath,
      sources: m.sources,
    );
  }

  @override
  bool get canSignInWithBrowser =>
      Platform.isWindows || Platform.isMacOS || Platform.isLinux;

  @override
  Future<String> startBrowserSignIn({
    required String place,
    String username = '',
  }) async => rust.startBrowserSignIn(
    place: place,
    username: username,
    sessionPath: await _session(),
  );

  @override
  Future<SignedInAccount> finishBrowserSignIn() async {
    final a = await rust.finishBrowserSignIn(sessionPath: await _session());
    return SignedInAccount(a.username, a.server);
  }

  @override
  Future<void> cancelBrowserSignIn() async => rust.cancelBrowserSignIn();

  @override
  Future<void> start() async => rust.startPhone(sessionPath: await _session());

  @override
  Future<void> stop() => rust.stopPhone();

  @override
  Future<void> signOut() async => rust.signOut(sessionPath: await _session());

  @override
  PhoneSnapshot snapshot() {
    final s = rust.phoneState();
    final calling = s.calling;
    return PhoneSnapshot(
      aor: s.aor,
      registration: Registration.values[s.registration.index],
      lines: [
        for (final l in s.lines)
          PhoneLine(
            line: l.line,
            kind: l.kind,
            aor: l.aor,
            label: l.label,
            defaultLabel: l.defaultLabel,
            registration: Registration.values[l.registration.index],
            voicemailNew: l.voicemailNew,
            ring: l.ring,
            shared: l.shared,
            appearances: l.appearances,
            bargeIn: l.bargeIn,
            inUse: [
              for (final a in l.inUse)
                LineAppearance(
                  index: a.index,
                  state: a.state,
                  remote: a.remote,
                  ours: a.ours,
                ),
            ],
          ),
      ],
      mayRenameLines: s.mayRenameLines,
      calls: s.calls.map(_call).toList(),
      voicemailNew: s.voicemailNew,
      dnd: s.dnd,
      brandName: s.brandName,
      brandPrimary: PhoneSnapshot.colorFromHex(s.brandPrimary),
      brandLogo: s.brandLogo,
      brandIcon: s.brandIcon,
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
              presenceLabel: p.presenceLabel,
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
      ringDevice: s.ringDevice,
      ringtones: [
        for (final r in s.ringtones) Ringtone(id: r.id, name: r.name),
      ],
      ringtone: s.ringtone,
      echoCancellation: s.echoCancellation,
      noiseSuppression: s.noiseSuppression,
      microphoneVolume: s.microphoneVolume,
      speakerVolume: s.speakerVolume,
      playing: s.playing,
      mayRecord: s.mayRecord,
      directoryListed: s.directoryListed,
      mayChangeDirectoryListing: s.mayChangeDirectoryListing,
      presence: switch (s.presence) {
        null => null,
        final p => MyPresence(
          shown: p.shown,
          shownLabel: p.shownLabel,
          idle: p.idle,
          status: p.status,
          statusState: p.statusState,
          note: p.note,
          until: p.until == null
              ? null
              : DateTime.tryParse(p.until!)?.toLocal(),
          statuses: [
            for (final o in p.statuses)
              OwnStatus(id: o.id, label: o.label, state: o.state),
          ],
        ),
      },
      audioTest: AudioTest.parse(s.audioTest),
      audioTestLevel: s.audioTestLevel,
      contacts: [
        for (final c in s.contacts)
          Contact(
            id: c.id,
            onAccount: c.onAccount,
            name: c.name,
            company: c.company,
            numbers: [
              for (final n in c.numbers)
                ContactNumber(label: n.label, number: n.number),
            ],
            email: c.email,
            notes: c.notes,
            ringtone: c.ringtone,
          ),
      ],
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
              packetsReceived: q.packetsReceived.toInt(),
              packetsPlayed: q.packetsPlayed.toInt(),
              packetsDropped: q.packetsDropped.toInt(),
              unexpectedPt: q.unexpectedPt,
              expectedPt: q.expectedPt,
              lastSource: q.lastSource,
            ),
      connectedAt: at == null
          ? null
          : DateTime.fromMillisecondsSinceEpoch(at.toInt() * 1000),
      recording: c.recording,
      line: c.line,
      silent: c.silent,
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
  Future<int> call(String target, {String? line}) async =>
      (await rust.placeCall(target: target, line: line)).toInt();

  @override
  Future<int> takeAppearance(String line, int index) async =>
      (await rust.takeAppearance(line: line, index: index)).toInt();

  @override
  Future<void> renameLine(String line, String label) =>
      rust.renameLine(line: line, label: label);

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
  Future<void> record(int call, bool on) =>
      rust.record(call: _id(call), on_: on);

  @override
  Future<void> saveContact(Contact contact) => rust.saveContact(
    contact: rust.Contact(
      id: contact.id,
      onAccount: contact.onAccount,
      name: contact.name,
      company: contact.company,
      numbers: [
        for (final n in contact.numbers)
          rust.ContactNumber(label: n.label, number: n.number),
      ],
      email: contact.email,
      notes: contact.notes,
      ringtone: contact.ringtone,
    ),
  );

  @override
  Future<void> deleteContact(String id) => rust.deleteContact(id: id);

  @override
  Future<ContactsImported> importContacts(
    String path, {
    required bool onAccount,
  }) async {
    final done = await rust.importContacts(path: path, onAccount: onAccount);
    return ContactsImported(
      imported: done.imported,
      duplicates: done.duplicates,
      skipped: done.skipped,
    );
  }

  @override
  Future<void> exportContacts(String path) => rust.exportContacts(path: path);

  @override
  Future<void> setDirectoryListed(bool on) => rust.setDirectoryListed(on_: on);

  @override
  Future<void> setPresence(String status, {String? note, DateTime? until}) =>
      rust.setPresence(
        status: status,
        note: note,
        until: until?.toUtc().toIso8601String(),
      );

  @override
  Future<void> setPresenceNote(String? note) =>
      rust.setPresenceNote(note: note);

  @override
  Future<void> setPresenceIdle(bool idle) => rust.setPresenceIdle(idle: idle);

  @override
  Future<void> addStatus(String label, String state) =>
      rust.addStatus(label: label, state: state);

  @override
  Future<void> editStatus(String id, {String? label, String? state}) =>
      rust.editStatus(id: id, label: label, state: state);

  @override
  Future<void> removeStatus(String id) => rust.removeStatus(id: id);

  @override
  Future<void> orderStatuses(List<String> ids) => rust.orderStatuses(ids: ids);

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
  Future<void> clearRecents() => rust.clearRecents();

  @override
  Future<void> hideRecent(String id) => rust.hideRecent(id: id);

  @override
  Future<void> chooseAudio({required bool input, String? device}) =>
      rust.chooseAudio(input: input, device: device);

  @override
  Future<void> chooseRingDevice(String? device) =>
      rust.chooseRingDevice(device: device);

  @override
  Future<void> chooseRingtone(String? id) => rust.chooseRingtone(id: id);

  @override
  Future<void> addRingtoneFile(String path) => rust.addRingtoneFile(path: path);

  @override
  Future<void> removeRingtone(String id) => rust.removeRingtone(id: id);

  @override
  Future<void> testMicrophone() => rust.testMicrophone();

  @override
  Future<void> testSpeaker({required bool ring}) =>
      rust.testSpeaker(ring: ring);

  @override
  Future<void> stopAudioTest() => rust.stopAudioTest();

  @override
  Future<void> setAudioProcessing({
    bool? echoCancellation,
    bool? noiseSuppression,
  }) => rust.setAudioProcessing(
    echoCancellation: echoCancellation,
    noiseSuppression: noiseSuppression,
  );

  @override
  Future<void> setAudioVolume({int? microphone, int? speaker}) =>
      rust.setAudioVolume(microphone: microphone, speaker: speaker);

  @override
  Future<void> refresh() => rust.refresh();
}
