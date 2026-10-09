import 'dart:async';

import 'package:anvil/src/desktop/shell.dart';

/// A desktop for the tests: what the app asked of it is kept, and a test
/// can click the tray or a notification.
class FakeShell implements DesktopShell {
  final _events = StreamController<ShellEvent>.broadcast();
  final List<String> log = [];
  TrayState? tray;
  String? tooltip;
  TrayMenu? menu;
  final Map<int, IncomingNotice> notices = {};
  bool login = false;

  void click(ShellAction action, {int? call}) =>
      _events.add(ShellEvent(action, call: call));

  @override
  Future<void> init({required String appName}) async => log.add('init');

  @override
  Stream<ShellEvent> get events => _events.stream;

  @override
  Future<void> setTray(TrayState state, String tooltip, TrayMenu menu) async {
    tray = state;
    this.tooltip = tooltip;
    this.menu = menu;
  }

  @override
  Future<void> showIncoming(IncomingNotice notice) async =>
      notices[notice.call] = notice;

  @override
  Future<void> clearIncoming(int call) async => notices.remove(call);

  @override
  Future<void> showWindow() async => log.add('show');

  @override
  Future<void> quit() async => log.add('quit');

  @override
  bool get canStartAtLogin => true;

  @override
  Future<bool> startsAtLogin() async => login;

  @override
  Future<void> setStartsAtLogin(bool on) async {
    log.add('login $on');
    login = on;
  }
}
