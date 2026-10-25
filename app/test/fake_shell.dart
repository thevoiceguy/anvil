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

  /// What the app's start asked for, as the app takes it.
  List<ShellEvent> start = [];

  /// What signs the phone out when the session ends.
  Future<void> Function()? sessionEnds;

  void click(ShellAction action, {int? call}) =>
      _events.add(ShellEvent(action, call: call));

  @override
  Future<void> init({
    required String appName,
    String? displayName,
    String? iconPath,
  }) async => log.add('init');

  @override
  Stream<ShellEvent> get events => _events.stream;

  /// A second start handed on, or a link opened.
  void started(ShellEvent event) => _events.add(event);

  @override
  List<ShellEvent> takeStartEvents() {
    final taken = start;
    start = [];
    return taken;
  }

  @override
  void whenSessionEnds(Future<void> Function()? stop) => sessionEnds = stop;

  String? title;

  @override
  Future<void> setTitle(String title) async => this.title = title;

  @override
  Future<void> setTray(TrayState state, String tooltip, TrayMenu menu) async {
    tray = state;
    this.tooltip = tooltip;
    this.menu = menu;
  }

  @override
  Future<void> allowNotifications() async => log.add('allow notifications');

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
