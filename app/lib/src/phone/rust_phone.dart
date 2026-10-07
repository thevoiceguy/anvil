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
    return PhoneSnapshot(
      aor: s.aor,
      registration: Registration.values[s.registration.index],
      calls: s.calls
          .map(
            (c) => PhoneCall(
              id: c.id.toInt(),
              direction: Direction.values[c.direction.index],
              remote: c.remote,
              displayName: c.displayName,
              state: CallState.values[c.state.index],
              held: c.held,
              muted: c.muted,
              codec: c.codec,
            ),
          )
          .toList(),
      voicemailNew: s.voicemailNew,
      dnd: s.dnd,
      brandName: s.brandName,
      brandPrimary: PhoneSnapshot.colorFromHex(s.brandPrimary),
      brandLogo: s.brandLogo,
    );
  }

  @override
  Stream<PhoneEvent> events() => rust.phoneChanges().map(
    (c) => PhoneEvent(c.kind, call: c.call?.toInt(), reason: c.reason),
  );

  @override
  Future<int> call(String target) async =>
      (await rust.placeCall(target: target)).toInt();

  @override
  Future<void> answer([int? call]) =>
      rust.answer(call: call == null ? null : BigInt.from(call));

  @override
  Future<void> decline([int? call]) =>
      rust.decline(call: call == null ? null : BigInt.from(call));

  @override
  Future<void> hangup([int? call]) =>
      rust.hangup(call: call == null ? null : BigInt.from(call));

  @override
  Future<void> hold(int call, bool on) =>
      rust.hold(call: BigInt.from(call), on_: on);

  @override
  Future<void> mute(int call, bool on) =>
      rust.mute(call: BigInt.from(call), on_: on);

  @override
  Future<void> sendDigits(int call, String digits) =>
      rust.sendDigits(call: BigInt.from(call), digits: digits);
}
