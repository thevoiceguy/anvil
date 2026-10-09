// The phone and the system's call screen kept in step (CallKit on iOS,
// Telecom on Android): a call ringing in rings there too, a call placed
// shows as dialling, connected, held and over as the phone has it; and what
// the user does there (answer, end, hold, mute, a key) reaches the phone.

import 'dart:async';

import 'package:flutter/widgets.dart';

import '../phone/phone_api.dart';
import '../phone/phone_model.dart';
import '../screens/calls.dart' show nameFor;
import 'platform.dart';

class MobileBridge extends StatefulWidget {
  const MobileBridge({
    super.key,
    required this.model,
    required this.mobile,
    required this.child,
  });

  final PhoneModel model;
  final MobilePlatform mobile;
  final Widget child;

  @override
  State<MobileBridge> createState() => _MobileBridgeState();
}

/// What the system has been told of one call.
class _Told {
  _Told({required this.connected, required this.held});
  bool connected;
  bool held;
}

class _MobileBridgeState extends State<MobileBridge> {
  StreamSubscription<SystemCallEvent>? _events;
  final Map<int, _Told> _told = {};

  PhoneModel get model => widget.model;
  MobilePlatform get mobile => widget.mobile;

  @override
  void initState() {
    super.initState();
    model.addListener(_sync);
    _events = mobile.callEvents.listen(_act);
    _sync();
  }

  @override
  void dispose() {
    model.removeListener(_sync);
    unawaited(_events?.cancel());
    super.dispose();
  }

  void _sync() {
    final snap = model.snapshot;
    final calls = {for (final c in snap.calls) c.id: c};
    for (final call in calls.values) {
      final told = _told[call.id];
      if (told == null) {
        final system = SystemCall(
          id: call.id,
          name: nameFor(snap, call),
          number: shortAddress(call.remote),
        );
        _told[call.id] = _Told(connected: call.isConnected, held: call.held);
        if (call.direction == Direction.incoming && !call.isConnected) {
          unawaited(mobile.reportIncoming(system));
        } else {
          unawaited(mobile.reportOutgoing(system));
          if (call.isConnected) unawaited(mobile.reportConnected(call.id));
        }
        continue;
      }
      if (call.isConnected && !told.connected) {
        told.connected = true;
        unawaited(mobile.reportConnected(call.id));
      }
      if (call.held != told.held) {
        told.held = call.held;
        unawaited(mobile.reportHeld(call.id, call.held));
      }
    }
    for (final id in _told.keys.toList()) {
      if (!calls.containsKey(id)) {
        _told.remove(id);
        unawaited(mobile.reportEnded(id));
      }
    }
  }

  PhoneCall? _call(int id) =>
      model.snapshot.calls.where((c) => c.id == id).firstOrNull;

  Future<void> _act(SystemCallEvent event) async {
    final call = _call(event.call);
    if (call == null) return;
    switch (event.action) {
      case SystemCallAction.shown:
        break;
      case SystemCallAction.failed:
        // Do Not Disturb or a blocked caller: the user asked not to be rung.
        // Anything else (no system call screen here) leaves the app ringing.
        if (event.reason == 'filtered' && call.isRingingIn) {
          await model.run(() => model.api.decline(call.id));
        }
      case SystemCallAction.answer:
        if (call.isRingingIn) {
          await model.run(() => model.api.answer(call.id));
        }
      case SystemCallAction.end:
        if (call.isRingingIn) {
          await model.run(() => model.api.decline(call.id));
        } else {
          await model.run(() => model.api.hangup(call.id));
        }
      case SystemCallAction.hold:
        final on = event.on ?? true;
        if (call.isConnected && call.held != on) {
          _told[call.id]?.held = on;
          await model.run(() => model.api.hold(call.id, on));
        }
      case SystemCallAction.mute:
        final on = event.on ?? true;
        if (call.muted != on) {
          await model.run(() => model.api.mute(call.id, on));
        }
      case SystemCallAction.dtmf:
        final digits = event.digits;
        if (digits != null && digits.isNotEmpty && call.isConnected) {
          await model.run(() => model.api.sendDigits(call.id, digits));
        }
    }
  }

  @override
  Widget build(BuildContext context) => widget.child;
}
