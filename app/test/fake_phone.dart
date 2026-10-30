import 'dart:async';
import 'dart:typed_data';

import 'package:anvil/src/phone/phone_api.dart';

/// A phone for the screens' tests: commands change its snapshot as
/// `anvil-app` would, every command is logged, and a test can ring it, answer
/// for the far end, or seed the user's data.
class FakePhone implements PhoneApi {
  SignedInAccount? saved;
  PhoneSnapshot state = const PhoneSnapshot(
    aor: '',
    registration: Registration.unregistered,
  );
  final _events = StreamController<PhoneEvent>.broadcast();
  final List<String> log = [];
  int _next = 1;
  String? refuseWith;

  @override
  String? logPath;

  /// The user's data the phone starts with.
  PhoneSnapshot Function(PhoneSnapshot)? seed;

  void set(PhoneSnapshot s, String kind, {int? call, String? reason}) {
    state = s;
    _events.add(PhoneEvent(kind, call: call, reason: reason));
  }

  /// The tenant's brand arrives.
  void brand(String name, String primaryHex, {Uint8List? icon}) => set(
    state.copyWith(
      brandName: name,
      brandPrimary: PhoneSnapshot.colorFromHex(primaryHex),
      brandIcon: icon,
    ),
    'brand',
  );

  /// The tenant's brand is taken away.
  void unbrand() => set(state.copyWith(clearBrand: true), 'brand');

  void ring(String from, {String? name, String? line, bool silent = false}) {
    final id = _next++;
    set(
      state.copyWith(
        calls: [
          ...state.calls,
          PhoneCall(
            id: id,
            direction: Direction.incoming,
            remote: 'sip:$from@x',
            displayName: name,
            state: CallState.ringing,
            line: line,
            silent: silent,
          ),
        ],
      ),
      'call',
      call: id,
    );
  }

  PhoneCall callOf(int id) => state.calls.firstWhere((c) => c.id == id);

  void _replace(int id, PhoneCall Function(PhoneCall) f) => set(
    state.copyWith(calls: [for (final c in state.calls) c.id == id ? f(c) : c]),
    'call',
    call: id,
  );

  /// Every call up but `except` held, as `anvil-app` does for a second call.
  List<PhoneCall> _othersHeld(int? except) => [
    for (final c in state.calls)
      c.id != except && c.isConnected ? c.copyWith(held: true) : c,
  ];

  void _end(int id, String reason) => set(
    state.copyWith(calls: state.calls.where((c) => c.id != id).toList()),
    'call_ended',
    call: id,
    reason: reason,
  );

  void _refuse() {
    if (refuseWith != null) throw Exception(refuseWith);
  }

  /// The far end answers an outgoing call.
  void answerOutgoing(int id) =>
      _replace(id, (c) => c.copyWith(state: CallState.connected));

  /// The server reports on a transfer.
  void transferProgress(int id, String reason) =>
      set(state, 'transfer_progress', call: id, reason: reason);

  /// The far end hangs up.
  void hungUp(int id) => _end(id, 'the other side hung up');

  @override
  Future<SignedInAccount?> savedAccount() async => saved;

  /// The last sign-in, as the app remembers it.
  RememberedSignIn? remembered;

  /// How the server says a person signs in; a server that cannot be found
  /// when `discoverError` is set.
  SignInMethods methods = const SignInMethods(password: true, sso: false);
  String? discoverError;

  /// The server asks for a one-time code (`123456`) after the password.
  bool wantsCode = false;
  String? _pending;

  @override
  Future<RememberedSignIn?> lastSignIn() async => remembered;

  @override
  Managed managed = Managed.none;

  @override
  Future<SignInMethods> signInMethods({
    required String place,
    required String username,
  }) async {
    log.add('methods $place $username');
    if (discoverError != null) throw Exception(discoverError);
    return methods;
  }

  @override
  Future<SignedInAccount?> signIn({
    required String place,
    required String username,
    required String password,
  }) async {
    log.add('signIn $place $username');
    if (password != 'right') throw Exception('wrong password');
    if (wantsCode) {
      _pending = place;
      return null;
    }
    remembered = RememberedSignIn(place, username);
    return saved = SignedInAccount(username, place);
  }

  @override
  Future<SignedInAccount> signInWithCode(String code) async {
    log.add('code $code');
    if (_pending == null) throw Exception('sign in with your password first');
    if (code != '123456') throw Exception('The code is not right');
    remembered = RememberedSignIn(_pending!, 'alice');
    return saved = SignedInAccount('alice', _pending!);
  }

  @override
  bool canSignInWithBrowser = true;

  /// What the browser comes back with: who signed in, or why not.
  final browser = Completer<SignedInAccount>();

  @override
  Future<String> startBrowserSignIn({
    required String place,
    String username = '',
  }) async {
    log.add('browser $place');
    return 'https://$place/auth/app/authorize?state=x';
  }

  @override
  Future<SignedInAccount> finishBrowserSignIn() async =>
      saved = await browser.future;

  @override
  Future<void> cancelBrowserSignIn() async {
    log.add('browser cancelled');
    if (!browser.isCompleted) browser.completeError(Exception('cancelled'));
  }

  @override
  Future<void> start() async {
    log.add('start');
    final started = PhoneSnapshot(
      aor: 'sip:${saved!.username}@x',
      registration: Registration.registered,
    );
    state = seed == null ? started : seed!(started);
  }

  @override
  Future<void> stop() async => log.add('stop phone');

  @override
  Future<void> signOut() async => log.add('signOut');

  @override
  PhoneSnapshot snapshot() => state;

  @override
  Stream<PhoneEvent> events() => _events.stream;

  @override
  Future<int> call(String target, {String? line}) async {
    _refuse();
    log.add(line == null ? 'call $target' : 'call $target on $line');
    final id = _next++;
    set(
      state.copyWith(
        calls: [
          ..._othersHeld(null),
          PhoneCall(
            id: id,
            direction: Direction.outgoing,
            remote: target.contains(':') ? target : 'sip:$target@x',
            state: CallState.dialing,
            line: line,
          ),
        ],
      ),
      'call',
      call: id,
    );
    return id;
  }

  @override
  Future<void> renameLine(String line, String label) async {
    _refuse();
    log.add('rename $line "$label"');
    set(
      state.copyWith(
        lines: [
          for (final l in state.lines)
            l.line == line
                ? l.copyWith(label: label.isEmpty ? l.defaultLabel : label)
                : l,
        ],
      ),
      'lines',
    );
  }

  @override
  Future<void> answer([int? call]) async {
    log.add('answer $call');
    state = state.copyWith(calls: _othersHeld(call));
    _replace(call!, (c) => c.copyWith(state: CallState.connected));
  }

  @override
  Future<void> decline([int? call]) async {
    log.add('decline $call');
    _end(call!, 'declined');
  }

  @override
  Future<void> hangup([int? call]) async {
    log.add('hangup $call');
    _end(call!, 'hung up');
  }

  @override
  Future<void> hold(int call, bool on) async {
    log.add('hold $call $on');
    if (!on) state = state.copyWith(calls: _othersHeld(call));
    _replace(call, (c) => c.copyWith(held: on));
  }

  @override
  Future<void> mute(int call, bool on) async {
    log.add('mute $call $on');
    _replace(call, (c) => c.copyWith(muted: on));
  }

  @override
  Future<void> sendDigits(int call, String digits) async =>
      log.add('dtmf $call $digits');

  @override
  Future<void> transfer(int call, String target) async {
    _refuse();
    log.add('transfer $call $target');
  }

  @override
  Future<void> transferAttended(int call, int to) async {
    log.add('attended $call $to');
    _end(call, 'transferred');
    _end(to, 'transferred');
  }

  @override
  Future<void> park(int call) async {
    log.add('park $call');
    _end(call, 'parked');
  }

  @override
  Future<void> record(int call, bool on) async {
    _refuse();
    log.add('record $call $on');
    _replace(call, (c) => c.copyWith(recording: on));
  }

  int _contacts = 0;

  @override
  Future<void> saveContact(Contact contact) async {
    _refuse();
    final saved = Contact(
      id: contact.id.isEmpty ? 'c${++_contacts}' : contact.id,
      onAccount: contact.onAccount,
      name: contact.name.trim(),
      company: contact.company,
      numbers: contact.numbers,
      email: contact.email,
      notes: contact.notes,
      ringtone: contact.ringtone,
    );
    log.add('contact ${saved.id} ${saved.name} ${saved.dial ?? ''}');
    final contacts = [
      for (final c in state.contacts)
        if (c.id != saved.id) c,
      saved,
    ]..sort((a, b) => a.name.toLowerCase().compareTo(b.name.toLowerCase()));
    set(state.copyWith(contacts: contacts), 'contacts');
  }

  @override
  Future<void> deleteContact(String id) async {
    log.add('delete contact $id');
    set(
      state.copyWith(
        contacts: state.contacts.where((c) => c.id != id).toList(),
      ),
      'contacts',
    );
  }

  @override
  Future<ContactsImported> importContacts(
    String path, {
    required bool onAccount,
  }) async {
    _refuse();
    log.add('import $path${onAccount ? ' account' : ''}');
    await saveContact(
      Contact(
        onAccount: onAccount,
        name: 'Imported Person',
        numbers: const [ContactNumber(label: 'mobile', number: '0400111222')],
      ),
    );
    return const ContactsImported(
      imported: 1,
      duplicates: 2,
      skipped: ['entry 4: nothing to call it by'],
    );
  }

  @override
  Future<void> exportContacts(String path) async {
    _refuse();
    log.add('export $path');
  }

  @override
  Future<void> setDirectoryListed(bool on) async {
    _refuse();
    log.add('directory $on');
    set(state.copyWith(directoryListed: on), 'directory');
  }

  /// The user's statuses kept, and the next id.
  int _statusIds = 0;

  MyPresence get _presence =>
      state.presence ??
      const MyPresence(
        shown: 'available',
        status: 'available',
        statusState: 'available',
      );

  /// The presence FCP would answer, from what was chosen.
  MyPresence _presenceOf({
    required String status,
    String? note,
    DateTime? until,
    bool? idle,
    List<OwnStatus>? statuses,
  }) {
    final own = statuses ?? _presence.statuses;
    final mine = own.where((s) => s.id == status).firstOrNull;
    final statusState = mine?.state ?? status;
    final isIdle = idle ?? _presence.idle;
    return MyPresence(
      shown: isIdle && statusState == 'available' ? 'away' : statusState,
      shownLabel: mine?.label,
      idle: isIdle && statusState == 'available',
      status: status,
      statusState: statusState,
      note: note,
      until: until,
      statuses: own,
    );
  }

  @override
  Future<void> setPresence(
    String status, {
    String? note,
    DateTime? until,
  }) async {
    _refuse();
    log.add(
      'presence $status${note == null ? '' : ' "$note"'}'
      '${until == null ? '' : ' until'}',
    );
    final now = _presenceOf(status: status, note: note, until: until);
    set(
      state.copyWith(presence: now, dnd: now.statusState == 'dnd'),
      'presence',
    );
  }

  @override
  Future<void> setPresenceNote(String? note) async {
    _refuse();
    log.add('presence note ${note ?? 'off'}');
    final p = _presence;
    set(
      state.copyWith(
        presence: _presenceOf(status: p.status, note: note, until: p.until),
      ),
      'presence',
    );
  }

  @override
  Future<void> setPresenceIdle(bool idle) async {
    _refuse();
    log.add('presence idle $idle');
    final p = _presence;
    set(
      state.copyWith(
        presence: _presenceOf(
          status: p.status,
          note: p.note,
          until: p.until,
          idle: idle,
        ),
      ),
      'presence',
    );
  }

  void _statuses(List<OwnStatus> statuses) {
    final p = _presence;
    final kept =
        statuses.any((s) => s.id == p.status) ||
        const ['available', 'away', 'busy', 'dnd'].contains(p.status);
    set(
      state.copyWith(
        presence: _presenceOf(
          status: kept ? p.status : 'available',
          note: kept ? p.note : null,
          until: kept ? p.until : null,
          statuses: statuses,
        ),
      ),
      'presence',
    );
  }

  @override
  Future<void> addStatus(String label, String state) async {
    _refuse();
    log.add('status add $state $label');
    _statuses([
      ..._presence.statuses,
      OwnStatus(id: 'status-${++_statusIds}', label: label, state: state),
    ]);
  }

  @override
  Future<void> editStatus(String id, {String? label, String? state}) async {
    _refuse();
    log.add('status edit $id ${label ?? '-'} ${state ?? '-'}');
    _statuses([
      for (final s in _presence.statuses)
        s.id == id
            ? OwnStatus(
                id: id,
                label: label ?? s.label,
                state: state ?? s.state,
              )
            : s,
    ]);
  }

  @override
  Future<void> removeStatus(String id) async {
    _refuse();
    log.add('status remove $id');
    _statuses([..._presence.statuses.where((s) => s.id != id)]);
  }

  @override
  Future<void> orderStatuses(List<String> ids) async {
    _refuse();
    log.add('status order ${ids.join(',')}');
    final byId = {for (final s in _presence.statuses) s.id: s};
    _statuses([for (final id in ids) byId[id]!]);
  }

  @override
  Future<void> setDnd(bool on) async {
    log.add('dnd $on');
    final c = state.calling ?? const CallingSettings();
    set(
      state.copyWith(
        dnd: on,
        calling: CallingSettings(
          dnd: on,
          callWaiting: c.callWaiting,
          forwardAll: c.forwardAll,
          forwardBusy: c.forwardBusy,
          forwardNoAnswer: c.forwardNoAnswer,
          forwardUnreachable: c.forwardUnreachable,
          noAnswerSecs: c.noAnswerSecs,
        ),
      ),
      'calling',
    );
  }

  @override
  Future<void> setCalling(CallingChange change) async {
    String? f(String? to, String? was) =>
        to == null ? was : (to.isEmpty ? null : to);
    final c = state.calling ?? const CallingSettings();
    log.add(
      'calling waiting=${change.callWaiting} all=${change.forwardAll} '
      'busy=${change.forwardBusy} noAnswer=${change.forwardNoAnswer} '
      'unreachable=${change.forwardUnreachable} secs=${change.noAnswerSecs}',
    );
    set(
      state.copyWith(
        calling: CallingSettings(
          dnd: c.dnd,
          callWaiting: change.callWaiting ?? c.callWaiting,
          forwardAll: f(change.forwardAll, c.forwardAll),
          forwardBusy: f(change.forwardBusy, c.forwardBusy),
          forwardNoAnswer: f(change.forwardNoAnswer, c.forwardNoAnswer),
          forwardUnreachable: f(
            change.forwardUnreachable,
            c.forwardUnreachable,
          ),
          noAnswerSecs: change.noAnswerSecs ?? c.noAnswerSecs,
        ),
      ),
      'calling',
    );
  }

  void _heard(String id) => state = state.copyWith(
    voicemail: [
      for (final m in state.voicemail)
        m.id == id ? m.copyWith(isNew: false) : m,
    ],
  );

  @override
  Future<void> markHeard(String id) async {
    log.add('heard $id');
    _heard(id);
    set(state, 'voicemail');
  }

  @override
  Future<void> deleteVoicemail(String id) async {
    log.add('delete $id');
    set(
      state.copyWith(
        voicemail: state.voicemail.where((m) => m.id != id).toList(),
      ),
      'voicemail',
    );
  }

  @override
  Future<void> play(String id) async {
    log.add('play $id');
    _heard(id);
    set(state.copyWith(playing: id), 'playing');
  }

  @override
  Future<void> stopPlaying() async {
    log.add('stop');
    set(state.copyWith(clearPlaying: true), 'playing');
  }

  @override
  Future<void> clearRecents() async {
    log.add('clear recents');
    set(state.copyWith(recents: const []), 'recents');
  }

  @override
  Future<void> hideRecent(String id) async {
    log.add('hide recent $id');
    set(
      state.copyWith(recents: state.recents.where((r) => r.id != id).toList()),
      'recents',
    );
  }

  @override
  Future<void> favourite(String who, bool on) async {
    log.add('favourite $who $on');
    set(
      state.copyWith(
        people: [
          for (final p in state.people)
            p.key == who ? p.copyWith(favourite: on) : p,
        ],
      ),
      'person',
    );
  }

  @override
  Future<void> chooseAudio({required bool input, String? device}) async {
    log.add('audio ${input ? 'in' : 'out'} ${device ?? 'default'}');
    set(
      input
          ? state.copyWith(input: device, clearInput: device == null)
          : state.copyWith(output: device, clearOutput: device == null),
      'audio',
    );
  }

  @override
  Future<void> chooseRingDevice(String? device) async {
    log.add('ring on ${device ?? 'default'}');
    set(
      state.copyWith(ringDevice: device, clearRingDevice: device == null),
      'audio',
    );
  }

  @override
  Future<void> chooseRingtone(String? id) async {
    log.add('ringtone ${id ?? 'default'}');
    set(state.copyWith(ringtone: id ?? 'anvil'), 'audio');
  }

  @override
  Future<void> addRingtoneFile(String path) async {
    log.add('ringtone file $path');
    final name = path.split(RegExp(r'[/\\]')).last;
    set(
      state.copyWith(
        ringtones: [
          ...state.ringtones,
          Ringtone(id: 'file:$name', name: name),
        ],
        ringtone: 'file:$name',
      ),
      'audio',
    );
  }

  @override
  Future<void> removeRingtone(String id) async {
    log.add('remove ringtone $id');
    set(
      state.copyWith(
        ringtones: state.ringtones.where((r) => r.id != id).toList(),
        ringtone: state.ringtone == id ? 'anvil' : state.ringtone,
      ),
      'audio',
    );
  }

  @override
  Future<void> testMicrophone() async {
    log.add('test microphone');
    set(
      state.copyWith(audioTest: AudioTest.recording, audioTestLevel: 0),
      'audio_test',
    );
  }

  @override
  Future<void> testSpeaker({required bool ring}) async {
    log.add('test ${ring ? 'ring' : 'speaker'}');
    set(
      state.copyWith(audioTest: ring ? AudioTest.ring : AudioTest.speaker),
      'audio_test',
    );
  }

  @override
  Future<void> stopAudioTest() async {
    log.add('stop test');
    set(state.copyWith(clearAudioTest: true, audioTestLevel: 0), 'audio_test');
  }

  @override
  Future<void> setAudioProcessing({
    bool? echoCancellation,
    bool? noiseSuppression,
  }) async {
    if (echoCancellation != null) log.add('echo $echoCancellation');
    if (noiseSuppression != null) log.add('noise $noiseSuppression');
    set(
      state.copyWith(
        echoCancellation: echoCancellation,
        noiseSuppression: noiseSuppression,
      ),
      'audio',
    );
  }

  @override
  Future<void> setAudioVolume({int? microphone, int? speaker}) async {
    if (microphone != null) log.add('microphone volume $microphone');
    if (speaker != null) log.add('speaker volume $speaker');
    set(
      state.copyWith(microphoneVolume: microphone, speakerVolume: speaker),
      'audio',
    );
  }

  @override
  Future<void> refresh() async => log.add('refresh');
}

/// A saved session, for tests that start signed in.
class SignedInAccountFake extends SignedInAccount {
  const SignedInAccountFake() : super('alice', 'pbx.example.com');
}
