import 'dart:async';

import 'package:anvil/src/phone/phone_api.dart';

/// A phone for the screens' tests: commands change its snapshot, and a test
/// can ring it.
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

  void _set(PhoneSnapshot s, String kind, {int? call, String? reason}) {
    state = s;
    _events.add(PhoneEvent(kind, call: call, reason: reason));
  }

  PhoneSnapshot _with(List<PhoneCall> calls) => PhoneSnapshot(
    aor: state.aor,
    registration: state.registration,
    calls: calls,
    voicemailNew: state.voicemailNew,
    dnd: state.dnd,
    brandName: state.brandName,
    brandPrimary: state.brandPrimary,
  );

  /// The tenant's brand arrives.
  void brand(String name, String primaryHex) {
    _set(
      PhoneSnapshot(
        aor: state.aor,
        registration: state.registration,
        calls: state.calls,
        voicemailNew: state.voicemailNew,
        dnd: state.dnd,
        brandName: name,
        brandPrimary: PhoneSnapshot.colorFromHex(primaryHex),
      ),
      'brand',
    );
  }

  void ring(String from) {
    final id = _next++;
    _set(
      _with([
        ...state.calls,
        PhoneCall(
          id: id,
          direction: Direction.incoming,
          remote: 'sip:$from@x',
          state: CallState.ringing,
        ),
      ]),
      'call',
      call: id,
    );
  }

  PhoneCall _call(int id) => state.calls.firstWhere((c) => c.id == id);

  void _replace(int id, PhoneCall Function(PhoneCall) f) => _set(
    _with([for (final c in state.calls) c.id == id ? f(c) : c]),
    'call',
    call: id,
  );

  PhoneCall _copy(PhoneCall c, {CallState? state, bool? held, bool? muted}) =>
      PhoneCall(
        id: c.id,
        direction: c.direction,
        remote: c.remote,
        displayName: c.displayName,
        state: state ?? c.state,
        held: held ?? c.held,
        muted: muted ?? c.muted,
        codec: c.codec,
      );

  void _refuse() {
    if (refuseWith != null) throw Exception(refuseWith);
  }

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
    state = PhoneSnapshot(
      aor: 'sip:${saved!.username}@x',
      registration: Registration.registered,
    );
  }

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
    _set(
      _with([
        ...state.calls,
        PhoneCall(
          id: id,
          direction: Direction.outgoing,
          remote: 'sip:$target@x',
          state: CallState.dialing,
        ),
      ]),
      'call',
      call: id,
    );
    return id;
  }

  @override
  Future<void> answer([int? call]) async {
    log.add('answer $call');
    _replace(call!, (c) => _copy(c, state: CallState.connected));
  }

  @override
  Future<void> decline([int? call]) async {
    log.add('decline $call');
    _set(
      _with(state.calls.where((c) => c.id != call).toList()),
      'call_ended',
      call: call,
      reason: 'declined',
    );
  }

  @override
  Future<void> hangup([int? call]) async {
    log.add('hangup $call');
    _set(
      _with(state.calls.where((c) => c.id != call).toList()),
      'call_ended',
      call: call,
      reason: 'hung up',
    );
  }

  @override
  Future<void> hold(int call, bool on) async {
    log.add('hold $call $on');
    _replace(call, (c) => _copy(c, held: on));
  }

  @override
  Future<void> mute(int call, bool on) async {
    log.add('mute $call $on');
    _replace(call, (c) => _copy(c, muted: on));
  }

  @override
  Future<void> sendDigits(int call, String digits) async =>
      log.add('dtmf $call $digits');

  PhoneCall callOf(int id) => _call(id);

  /// The far end answers an outgoing call.
  void answerOutgoing(int id) =>
      _replace(id, (c) => _copy(c, state: CallState.connected));
}

/// A saved session, for tests that start signed in.
class SignedInAccountFake extends SignedInAccount {
  const SignedInAccountFake() : super('alice', 'pbx.example.com');
}
