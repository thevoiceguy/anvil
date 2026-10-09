import 'dart:async';

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

  /// The user's data the phone starts with.
  PhoneSnapshot Function(PhoneSnapshot)? seed;

  void set(PhoneSnapshot s, String kind, {int? call, String? reason}) {
    state = s;
    _events.add(PhoneEvent(kind, call: call, reason: reason));
  }

  /// The tenant's brand arrives.
  void brand(String name, String primaryHex) => set(
    state.copyWith(
      brandName: name,
      brandPrimary: PhoneSnapshot.colorFromHex(primaryHex),
    ),
    'brand',
  );

  void ring(String from, {String? name}) {
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

  @override
  Future<SignedInAccount> signIn({
    required String place,
    required String username,
    required String password,
    String? code,
  }) async {
    log.add('signIn $place $username');
    if (password != 'right') throw Exception('wrong password');
    return saved = SignedInAccount(username, place);
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
  Future<int> call(String target) async {
    _refuse();
    log.add('call $target');
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
          ),
        ],
      ),
      'call',
      call: id,
    );
    return id;
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
  Future<void> refresh() async => log.add('refresh');
}

/// A saved session, for tests that start signed in.
class SignedInAccountFake extends SignedInAccount {
  const SignedInAccountFake() : super('alice', 'pbx.example.com');
}
