// The phone around the app on Android and iOS: what the app asks of the
// system there (the microphone, the audio session) and the system's own
// call screen (CallKit, Android's Telecom), which shows the app's calls and
// hands back what the user does there. None on desktop and in the screens'
// tests.

import 'dart:async';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

/// A call as the system's call screen shows it.
class SystemCall {
  const SystemCall({
    required this.id,
    required this.name,
    required this.number,
  });

  /// The phone's call id.
  final int id;

  /// Who it is with, as a person reads it, and the number behind it.
  final String name;
  final String number;

  Map<String, Object> toMap() => {'id': id, 'name': name, 'number': number};
}

/// What happened at the system's call screen.
enum SystemCallAction {
  /// The system took the call (it rings, or shows as dialling).
  shown,

  /// The system would not take the call: `reason` says why.
  failed,

  /// The user answered, ended, held, muted or pressed a key there.
  answer,
  end,
  hold,
  mute,
  dtmf,
}

class SystemCallEvent {
  const SystemCallEvent(
    this.action,
    this.call, {
    this.on,
    this.digits,
    this.reason,
  });

  final SystemCallAction action;
  final int call;

  /// For `hold` and `mute`.
  final bool? on;

  /// For `dtmf`.
  final String? digits;

  /// For `failed`: `filtered` (Do Not Disturb, a blocked caller: the call
  /// should not ring), `reset` (the system dropped its calls; they go on in
  /// the app), or another reason the system could not show it.
  final String? reason;
}

abstract class MobilePlatform {
  /// Ask for the microphone (and, where the system asks separately, to show
  /// the call's notification) if the system hasn't been asked yet; whether
  /// the app may use the microphone.
  Future<bool> requestMicrophone();

  /// Set the system's audio up for calls (iOS: play and record, voice
  /// chat), before the phone starts.
  Future<void> startAudio();

  /// A call ringing in, for the system to ring and show.
  Future<void> reportIncoming(SystemCall call);

  /// A call the user placed, for the system to show as dialling.
  Future<void> reportOutgoing(SystemCall call);

  /// The call connected.
  Future<void> reportConnected(int call);

  /// The call was held or taken off hold here.
  Future<void> reportHeld(int call, bool on);

  /// The call is over.
  Future<void> reportEnded(int call);

  /// What the user does at the system's call screen.
  Stream<SystemCallEvent> get callEvents;
}

/// The app's own channel to `MainActivity` (Kotlin) and `AppDelegate`
/// (Swift).
class NativeMobile implements MobilePlatform {
  /// One for the app: the channel has one handler for what the system says.
  factory NativeMobile() => _instance;

  NativeMobile._() {
    _channel.setMethodCallHandler(_fromSystem);
  }

  static final _instance = NativeMobile._();
  static const _channel = MethodChannel('anvil/mobile');
  final _events = StreamController<SystemCallEvent>.broadcast();

  static bool get supported =>
      !kIsWeb && (Platform.isAndroid || Platform.isIOS);

  @override
  Future<bool> requestMicrophone() async =>
      await _channel.invokeMethod<bool>('requestMicrophone') ?? false;

  @override
  Future<void> startAudio() => _channel.invokeMethod<void>('startAudio');

  @override
  Future<void> reportIncoming(SystemCall call) =>
      _channel.invokeMethod<void>('reportIncoming', call.toMap());

  @override
  Future<void> reportOutgoing(SystemCall call) =>
      _channel.invokeMethod<void>('reportOutgoing', call.toMap());

  @override
  Future<void> reportConnected(int call) =>
      _channel.invokeMethod<void>('reportConnected', {'id': call});

  @override
  Future<void> reportHeld(int call, bool on) =>
      _channel.invokeMethod<void>('reportHeld', {'id': call, 'on': on});

  @override
  Future<void> reportEnded(int call) =>
      _channel.invokeMethod<void>('reportEnded', {'id': call});

  @override
  Stream<SystemCallEvent> get callEvents => _events.stream;

  /// `callEvent` from the native side: `{action, id, on?, digits?, reason?}`.
  Future<void> _fromSystem(MethodCall call) async {
    if (call.method != 'callEvent') return;
    final m = Map<String, Object?>.from(call.arguments as Map);
    final action = SystemCallAction.values
        .where((a) => a.name == m['action'])
        .firstOrNull;
    if (action == null) return;
    _events.add(
      SystemCallEvent(
        action,
        (m['id'] as num).toInt(),
        on: m['on'] as bool?,
        digits: m['digits'] as String?,
        reason: m['reason'] as String?,
      ),
    );
  }
}
