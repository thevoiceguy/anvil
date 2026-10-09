// The phone and the desktop kept in step: the tray shows how the phone is,
// a call ringing in gets a notification (gone once it stops ringing), and
// what the user does at the tray or a notification reaches the phone.

import 'dart:async';

import 'package:flutter/widgets.dart';

import '../../l10n/app_localizations.dart';
import '../phone/phone_api.dart';
import '../phone/phone_model.dart';
import '../screens/calls.dart' show nameFor;
import 'shell.dart';

class DesktopBridge extends StatefulWidget {
  const DesktopBridge({
    super.key,
    required this.model,
    required this.shell,
    required this.child,
  });

  final PhoneModel model;
  final DesktopShell shell;
  final Widget child;

  @override
  State<DesktopBridge> createState() => _DesktopBridgeState();
}

class _DesktopBridgeState extends State<DesktopBridge> {
  StreamSubscription<ShellEvent>? _events;
  (TrayState, String, TrayMenu)? _tray;
  final Set<int> _notified = {};

  PhoneModel get model => widget.model;
  DesktopShell get shell => widget.shell;

  @override
  void initState() {
    super.initState();
    model.addListener(_sync);
    _events = shell.events.listen(_act);
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _sync();
  }

  @override
  void dispose() {
    model.removeListener(_sync);
    unawaited(_events?.cancel());
    super.dispose();
  }

  static TrayState trayState(PhoneModel model) {
    final snap = model.snapshot;
    if (model.account == null || snap.registration != Registration.registered) {
      return TrayState.offline;
    }
    if (snap.calls.any((c) => c.isRingingIn)) return TrayState.ringing;
    if (snap.calls.isNotEmpty) return TrayState.inCall;
    if (snap.dnd == true) return TrayState.dnd;
    return TrayState.ready;
  }

  void _sync() {
    if (!mounted) return;
    final s = Strings.of(context);
    final snap = model.snapshot;
    final state = trayState(model);
    final status = switch (state) {
      TrayState.offline => s.registrationUnregistered,
      TrayState.ready => s.registrationRegistered,
      TrayState.dnd => s.dndOn,
      TrayState.ringing => s.incomingCall,
      TrayState.inCall => s.callConnected,
    };
    final app = snap.brandName ?? s.appName;
    final tray = (
      state,
      '$app — $status',
      TrayMenu(
        show: s.trayShow(app),
        dnd: s.dndOn,
        dndOn: snap.dnd == true,
        dndAvailable: snap.calling != null,
        quit: s.trayQuit,
      ),
    );
    if (tray != _tray) {
      _tray = tray;
      unawaited(shell.setTray(tray.$1, tray.$2, tray.$3));
    }

    final ringing = {
      for (final c in snap.calls)
        if (c.isRingingIn) c.id: c,
    };
    for (final call in ringing.values) {
      if (_notified.add(call.id)) {
        final name = nameFor(snap, call);
        final number = shortAddress(call.remote);
        unawaited(
          shell.showIncoming(
            IncomingNotice(
              call: call.id,
              title: s.incomingCall,
              body: name == number ? name : '$name ($number)',
              answer: s.answerButton,
              decline: s.declineButton,
            ),
          ),
        );
      }
    }
    for (final id in _notified.difference(ringing.keys.toSet()).toList()) {
      _notified.remove(id);
      unawaited(shell.clearIncoming(id));
    }
  }

  Future<void> _act(ShellEvent event) async {
    switch (event.action) {
      case ShellAction.show:
        await shell.showWindow();
      case ShellAction.answer:
        await shell.showWindow();
        await model.run(() => model.api.answer(event.call));
      case ShellAction.decline:
        await model.run(() => model.api.decline(event.call));
      case ShellAction.toggleDnd:
        if (model.snapshot.calling == null) return;
        await model.run(() => model.api.setDnd(model.snapshot.dnd != true));
      case ShellAction.quit:
        await model.stop();
        await shell.quit();
    }
  }

  @override
  Widget build(BuildContext context) =>
      DesktopScope(shell: shell, child: widget.child);
}
