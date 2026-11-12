// The phone around the app on Android and iOS: what the app asks of the
// system there (the microphone, the audio session, where a call's audio
// goes, the proximity sensor) and the system's own call screen (CallKit,
// Android's Telecom), which shows the app's calls and hands back what the
// user does there. None on desktop and in the screens' tests.

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

  /// The system no longer shows the call.
  ended,
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

/// Where a call's audio goes: the phone held to the ear, its loudspeaker,
/// a Bluetooth headset or car, a headset on a wire.
enum AudioRoute { earpiece, speaker, bluetooth, wired }

/// The routes a call may take now, and the one it takes.
class AudioRoutes {
  const AudioRoutes({
    required this.current,
    required this.available,
    this.bluetoothName,
  });

  final AudioRoute current;
  final List<AudioRoute> available;

  /// The Bluetooth device's own name, when the system gives it.
  final String? bluetoothName;

  /// Only the earpiece and the speaker: a speaker switch is enough.
  bool get justSpeaker =>
      available.length == 2 &&
      available.contains(AudioRoute.earpiece) &&
      available.contains(AudioRoute.speaker);

  static AudioRoutes? fromMap(Object? raw) {
    if (raw is! Map) return null;
    AudioRoute? route(Object? name) =>
        AudioRoute.values.where((r) => r.name == name).firstOrNull;
    final current = route(raw['current']);
    if (current == null) return null;
    final available = [
      for (final r in (raw['available'] as List? ?? const [])) ?route(r),
    ];
    return AudioRoutes(
      current: current,
      available: available.contains(current)
          ? available
          : [...available, current],
      bluetoothName: raw['bluetoothName'] as String?,
    );
  }

  @override
  bool operator ==(Object other) =>
      other is AudioRoutes &&
      other.current == current &&
      listEquals(other.available, available) &&
      other.bluetoothName == bluetoothName;

  @override
  int get hashCode =>
      Object.hash(current, Object.hashAll(available), bluetoothName);
}

abstract class MobilePlatform {
  /// The system's call screen follows the phone itself, not the app: native
  /// code hears the calls from the Rust core (Telecom on Android, CallKit on
  /// iOS), so a phone a push woke rings with no Flutter running (FCP's
  /// `docs/PUSH.md` §2.5).
  /// The app then neither reports its calls nor acts on [callEvents].
  bool get systemFollowsPhone;

  /// Ask for the microphone (and, where the system asks separately, to show
  /// the call's notification) if the system hasn't been asked yet; whether
  /// the app may use the microphone.
  Future<bool> requestMicrophone();

  /// Ask for the camera, before a call first uses it, if the system hasn't
  /// been asked yet; whether the app may use it. (iOS asks itself when the
  /// camera opens.)
  Future<bool> requestCamera();

  /// Ask to capture the screen, before each share of it; whether the user
  /// allowed it. Android asks every time (its consent dialog); elsewhere
  /// the share asks for itself.
  Future<bool> requestScreenCapture();

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

  /// The routes a call's audio may take, as the system has them now (none
  /// known: null), and each change to them.
  Future<AudioRoutes?> audioRoutes();
  Stream<AudioRoutes> get audioRouteChanges;

  /// Send the call's audio this way.
  Future<void> setAudioRoute(AudioRoute route);

  /// Picture-in-picture while a call shows the other side's picture:
  /// allowed, with the picture (null: not now). Android shrinks the app's
  /// own window, which shows only the picture meanwhile; iOS shows the
  /// texture's pictures in a window of its own.
  Future<void> setPictureInPicture(PictureInPicture? pip);

  /// Whether the app's window is in picture-in-picture now (Android).
  Stream<bool> get pictureInPictureChanges;

  /// Whether the phone's network is cellular now (null: not known), and
  /// each change to it.
  Future<bool?> cellular();
  Stream<bool> get cellularChanges;

  /// Turn the screen off when the phone is held to the ear (a call up on
  /// the earpiece), or stop.
  Future<void> setProximity(bool on);
}

/// What picture-in-picture shows: a texture's pictures, and their size.
class PictureInPicture {
  const PictureInPicture({
    required this.texture,
    required this.width,
    required this.height,
  });

  final int texture;
  final int width;
  final int height;

  @override
  bool operator ==(Object other) =>
      other is PictureInPicture &&
      other.texture == texture &&
      other.width == width &&
      other.height == height;

  @override
  int get hashCode => Object.hash(texture, width, height);
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
  final _routes = StreamController<AudioRoutes>.broadcast();
  final _cellular = StreamController<bool>.broadcast();
  final _pip = StreamController<bool>.broadcast();

  static bool get supported =>
      !kIsWeb && (Platform.isAndroid || Platform.isIOS);

  @override
  bool get systemFollowsPhone => supported;

  /// A push's data as the push service delivers it (`type`, `call`,
  /// `from`, `name`): for the app's tests on a device with no push project
  /// or a simulator PushKit does not reach (debug builds).
  @visibleForTesting
  Future<void> deliverPush(Map<String, String> data) =>
      _channel.invokeMethod<void>('deliverPush', data);

  @override
  Future<bool> requestMicrophone() async =>
      await _channel.invokeMethod<bool>('requestMicrophone') ?? false;

  @override
  Future<bool> requestCamera() async {
    try {
      return await _channel.invokeMethod<bool>('requestCamera') ?? false;
    } on MissingPluginException {
      return true;
    }
  }

  @override
  Future<bool> requestScreenCapture() async {
    try {
      return await _channel.invokeMethod<bool>('requestScreenCapture') ?? false;
    } on MissingPluginException {
      return true;
    }
  }

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

  @override
  Future<AudioRoutes?> audioRoutes() async =>
      AudioRoutes.fromMap(await _channel.invokeMethod<Object>('audioRoutes'));

  @override
  Stream<AudioRoutes> get audioRouteChanges => _routes.stream;

  @override
  Future<void> setAudioRoute(AudioRoute route) =>
      _channel.invokeMethod<void>('setAudioRoute', {'route': route.name});

  @override
  Future<void> setProximity(bool on) =>
      _channel.invokeMethod<void>('setProximity', {'on': on});

  @override
  Future<void> setPictureInPicture(PictureInPicture? pip) async {
    try {
      await _channel.invokeMethod<void>(
        'setPictureInPicture',
        pip == null
            ? {'enabled': false}
            : {
                'enabled': true,
                'texture': pip.texture,
                'width': pip.width,
                'height': pip.height,
              },
      );
    } on MissingPluginException {
      // A system without it.
    }
  }

  @override
  Stream<bool> get pictureInPictureChanges => _pip.stream;

  @override
  Future<bool?> cellular() async {
    try {
      return await _channel.invokeMethod<bool>('cellular');
    } on MissingPluginException {
      return null;
    }
  }

  @override
  Stream<bool> get cellularChanges => _cellular.stream;

  /// `callEvent` from the native side: `{action, id, on?, digits?, reason?}`.
  Future<void> _fromSystem(MethodCall call) async {
    if (call.method == 'pictureInPicture') {
      final on = (call.arguments as Map?)?['on'];
      if (on is bool) _pip.add(on);
      return;
    }
    if (call.method == 'network') {
      final cellular = (call.arguments as Map?)?['cellular'];
      if (cellular is bool) _cellular.add(cellular);
      return;
    }
    if (call.method == 'audioRoutes') {
      final routes = AudioRoutes.fromMap(call.arguments);
      if (routes != null) _routes.add(routes);
      return;
    }
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
