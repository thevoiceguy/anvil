// The desktop around the app (`docs/APP.md` §4, Desktop): the tray icon and
// its menu, a notification for a call ringing in that answers from the
// notification, the window kept in the tray when closed, and starting at
// login. An interface, so the screens' tests stand in for the desktop.

import 'dart:async';

import 'package:flutter/widgets.dart';

/// What the tray shows.
enum TrayState { offline, ready, dnd, ringing, inCall }

/// The tray's menu, in the user's language.
class TrayMenu {
  const TrayMenu({
    required this.show,
    required this.dnd,
    required this.dndOn,
    required this.dndAvailable,
    required this.quit,
  });

  final String show;
  final String dnd;
  final bool dndOn;

  /// Do not disturb needs FCP; without it the item is greyed out.
  final bool dndAvailable;
  final String quit;

  @override
  bool operator ==(Object other) =>
      other is TrayMenu &&
      other.show == show &&
      other.dnd == dnd &&
      other.dndOn == dndOn &&
      other.dndAvailable == dndAvailable &&
      other.quit == quit;

  @override
  int get hashCode => Object.hash(show, dnd, dndOn, dndAvailable, quit);
}

/// A notification for a call ringing in.
class IncomingNotice {
  const IncomingNotice({
    required this.call,
    required this.title,
    required this.body,
    required this.answer,
    required this.decline,
  });

  final int call;
  final String title;
  final String body;
  final String answer;
  final String decline;
}

/// What the user did at the tray or a notification.
enum ShellAction { show, answer, decline, toggleDnd, quit }

class ShellEvent {
  const ShellEvent(this.action, {this.call});
  final ShellAction action;

  /// The call an answer or decline is for.
  final int? call;
}

abstract class DesktopShell {
  /// Set up the tray, notifications and the window; a part the desktop
  /// lacks (a Linux without a tray) is left out, not fatal.
  Future<void> init({required String appName});

  Stream<ShellEvent> get events;

  Future<void> setTray(TrayState state, String tooltip, TrayMenu menu);
  Future<void> showIncoming(IncomingNotice notice);
  Future<void> clearIncoming(int call);

  Future<void> showWindow();

  /// Close for good: the tray goes and the window with it.
  Future<void> quit();

  bool get canStartAtLogin;
  Future<bool> startsAtLogin();
  Future<void> setStartsAtLogin(bool on);
}

/// The desktop shell, for the widgets that need it (settings).
class DesktopScope extends InheritedWidget {
  const DesktopScope({super.key, required this.shell, required super.child});
  final DesktopShell shell;

  static DesktopShell? maybeOf(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<DesktopScope>()?.shell;

  @override
  bool updateShouldNotify(DesktopScope old) => shell != old.shell;
}
