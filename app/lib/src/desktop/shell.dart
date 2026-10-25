// The desktop around the app (`docs/APP.md` §4, Desktop): the tray icon and
// its menu, a notification for a call ringing in that answers from the
// notification, the window kept in the tray when closed, and starting at
// login. An interface, so the screens' tests stand in for the desktop.

import 'dart:async';
import 'dart:typed_data';

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

/// What the user did at the tray or a notification, or a start of the app
/// asked for (`dial`: a tel: or sip: link).
enum ShellAction { show, answer, decline, toggleDnd, quit, dial }

class ShellEvent {
  const ShellEvent(this.action, {this.call, this.target});
  final ShellAction action;

  /// The call an answer or decline is for.
  final int? call;

  /// What a link asks to call, for the keypad.
  final String? target;

  @override
  bool operator ==(Object other) =>
      other is ShellEvent &&
      other.action == action &&
      other.call == call &&
      other.target == target;

  @override
  int get hashCode => Object.hash(action, call, target);

  @override
  String toString() => 'ShellEvent($action, call: $call, target: $target)';
}

abstract class DesktopShell {
  /// Set up the tray, notifications and the window; a part the desktop
  /// lacks (a Linux without a tray) is left out, not fatal.
  /// [appName] keys what the app registers (start at login); [displayName]
  /// and [iconPath] are what its notifications show, a managed install's
  /// brand (`docs/MANAGED_INSTALLS.md` §2.3).
  Future<void> init({
    required String appName,
    String? displayName,
    String? iconPath,
  });

  Stream<ShellEvent> get events;

  /// What the app's own start asked for (a link it was opened with), once:
  /// [events] has no listener that early.
  List<ShellEvent> takeStartEvents();

  /// `stop` signs the phone out when the user's session ends (Windows'
  /// sign-out or shutdown), before the system ends the app.
  void whenSessionEnds(Future<void> Function()? stop);

  Future<void> setTray(TrayState state, String tooltip, TrayMenu menu);

  /// The window's title: the tenant's name for the app, else Anvil.
  Future<void> setTitle(String title);

  /// The brand on the system (`docs/MANAGED_INSTALLS.md` §2.4): its name
  /// and [icon] (a PNG, JPEG or WebP image) on the window, the tray and the
  /// Dock, Windows' shortcuts and notifications, and a Linux menu; Anvil's
  /// own with neither.
  Future<void> setBrand(String? name, Uint8List? icon);

  /// Ask for leave to show notifications, where the system asks the user
  /// (macOS). Called once someone has signed in, never awaited: the answer
  /// is the user's to give whenever they like.
  Future<void> allowNotifications();

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
