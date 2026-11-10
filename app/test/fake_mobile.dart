import 'dart:async';

import 'package:anvil/src/mobile/platform.dart';

/// The phone around the app, as a log of what it was told and a way for a
/// test to act at the system's call screen.
class FakeMobile implements MobilePlatform {
  FakeMobile({
    this.allow = true,
    this.allowCamera = true,
    this.systemFollowsPhone = false,
  });
  final bool allow;
  final bool allowCamera;

  @override
  final bool systemFollowsPhone;
  final log = <String>[];
  final _events = StreamController<SystemCallEvent>.broadcast();
  final _routes = StreamController<AudioRoutes>.broadcast();
  final _cellular = StreamController<bool>.broadcast();
  final _pip = StreamController<bool>.broadcast();

  /// What picture-in-picture is allowed to show now.
  PictureInPicture? pip;

  @override
  Future<void> setPictureInPicture(PictureInPicture? pip) async {
    this.pip = pip;
    log.add(
      pip == null ? 'pip off' : 'pip ${pip.texture} ${pip.width}x${pip.height}',
    );
  }

  /// The system puts the app's window in picture-in-picture, or back.
  void pictureInPicture(bool on) => _pip.add(on);

  @override
  Stream<bool> get pictureInPictureChanges => _pip.stream;

  /// The phone's network: set by a test, changed with [network].
  bool? onCellular;

  /// The phone moves between cellular and Wi-Fi.
  void network({required bool cellular}) {
    onCellular = cellular;
    _cellular.add(cellular);
  }

  @override
  Future<bool?> cellular() async => onCellular;

  @override
  Stream<bool> get cellularChanges => _cellular.stream;

  /// The routes the system has now: set by a test, changed with [route].
  AudioRoutes? routes;

  /// The system's routes change (a headset, a choice).
  void route(AudioRoutes routes) {
    this.routes = routes;
    _routes.add(routes);
  }

  /// The user does something at the system's call screen.
  void act(SystemCallEvent event) => _events.add(event);

  @override
  Future<bool> requestMicrophone() async {
    log.add('requestMicrophone');
    return allow;
  }

  @override
  Future<bool> requestCamera() async {
    log.add('requestCamera');
    return allowCamera;
  }

  @override
  Future<void> startAudio() async => log.add('startAudio');

  @override
  Future<void> reportIncoming(SystemCall call) async =>
      log.add('incoming ${call.id} ${call.name} ${call.number}');

  @override
  Future<void> reportOutgoing(SystemCall call) async =>
      log.add('outgoing ${call.id} ${call.name} ${call.number}');

  @override
  Future<void> reportConnected(int call) async => log.add('connected $call');

  @override
  Future<void> reportHeld(int call, bool on) async => log.add('held $call $on');

  @override
  Future<void> reportEnded(int call) async => log.add('ended $call');

  @override
  Stream<SystemCallEvent> get callEvents => _events.stream;

  @override
  Future<AudioRoutes?> audioRoutes() async => routes;

  @override
  Stream<AudioRoutes> get audioRouteChanges => _routes.stream;

  @override
  Future<void> setAudioRoute(AudioRoute route) async {
    log.add('route ${route.name}');
    final now = routes;
    if (now != null) {
      this.route(
        AudioRoutes(
          current: route,
          available: now.available,
          bluetoothName: now.bluetoothName,
        ),
      );
    }
  }

  /// Each proximity setting, in order; the last is the sensor's state.
  final proximity = <bool>[];

  @override
  Future<void> setProximity(bool on) async => proximity.add(on);
}
