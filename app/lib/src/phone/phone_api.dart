// The phone as the screens see it: an interface over `anvil-app`, so a
// screen never touches the bindings and a test can stand in for the phone.

import 'dart:async';
import 'dart:typed_data';

enum Registration { unregistered, registering, registered, failed }

enum Direction { incoming, outgoing }

enum CallState { dialing, ringing, connected }

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
  });

  final int id;
  final Direction direction;
  final String remote;
  final String? displayName;
  final CallState state;
  final bool held;
  final bool muted;
  final String? codec;

  /// Who the call is with, as a person reads it.
  String get who => displayName ?? _short(remote);

  bool get isRingingIn =>
      direction == Direction.incoming && state == CallState.ringing;

  static String _short(String uri) {
    final s = uri.replaceFirst(RegExp(r'^sips?:'), '');
    return s.split('@').first;
  }
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

  /// `#RRGGBB` as an opaque ARGB value.
  static int? colorFromHex(String? hex) {
    if (hex == null || !RegExp(r'^#[0-9a-fA-F]{6}').hasMatch(hex)) return null;
    return 0xFF000000 | int.parse(hex.substring(1, 7), radix: 16);
  }

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
}
