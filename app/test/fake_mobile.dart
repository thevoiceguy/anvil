import 'dart:async';

import 'package:anvil/src/mobile/platform.dart';

/// The phone around the app, as a log of what it was told and a way for a
/// test to act at the system's call screen.
class FakeMobile implements MobilePlatform {
  FakeMobile({this.allow = true});
  final bool allow;
  final log = <String>[];
  final _events = StreamController<SystemCallEvent>.broadcast();

  /// The user does something at the system's call screen.
  void act(SystemCallEvent event) => _events.add(event);

  @override
  Future<bool> requestMicrophone() async {
    log.add('requestMicrophone');
    return allow;
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
}
