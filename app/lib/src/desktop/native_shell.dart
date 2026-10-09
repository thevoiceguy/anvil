// The desktop shell on Windows, macOS and Linux: tray_manager for the tray,
// flutter_local_notifications for a call ringing in, window_manager to keep
// the window in the tray when closed, launch_at_startup for starting at
// login. Each part that fails to start is logged and left out.

import 'dart:async';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter_local_notifications/flutter_local_notifications.dart';
import 'package:launch_at_startup/launch_at_startup.dart';
import 'package:tray_manager/tray_manager.dart';
import 'package:window_manager/window_manager.dart';

import 'shell.dart';

class NativeShell with TrayListener, WindowListener implements DesktopShell {
  final _events = StreamController<ShellEvent>.broadcast();
  final _notifications = FlutterLocalNotificationsPlugin();
  bool _tray = false;
  bool _notify = false;
  bool _window = false;
  bool _login = false;

  static bool get supported =>
      !kIsWeb && (Platform.isWindows || Platform.isMacOS || Platform.isLinux);

  // Windows identifies the app's notifications by these.
  static const _aumid = 'Anvil.Softphone';
  static const _guid = '6d1c5a3e-8f0b-4c47-9a5e-2b7d0e4f1a93';
  static const _category = 'incoming_call';

  @override
  Stream<ShellEvent> get events => _events.stream;

  @override
  Future<void> init({required String appName}) async {
    await _part('window', () async {
      await windowManager.ensureInitialized();
      await windowManager.setPreventClose(true);
      windowManager.addListener(this);
      _window = true;
    });
    await _part('tray', () async {
      trayManager.addListener(this);
      await trayManager.setIcon(_icon(TrayState.offline));
      _tray = true;
    });
    await _part('notifications', () async {
      await _notifications.initialize(
        settings: InitializationSettings(
          linux: LinuxInitializationSettings(defaultActionName: appName),
          windows: WindowsInitializationSettings(
            appName: appName,
            appUserModelId: _aumid,
            guid: _guid,
          ),
          macOS: DarwinInitializationSettings(
            notificationCategories: [
              DarwinNotificationCategory(
                _category,
                actions: [
                  DarwinNotificationAction.plain(
                    'answer',
                    'Answer',
                    options: {DarwinNotificationActionOption.foreground},
                  ),
                  DarwinNotificationAction.plain('decline', 'Decline'),
                ],
              ),
            ],
          ),
        ),
        onDidReceiveNotificationResponse: _response,
      );
      _notify = true;
    });
    await _part('start at login', () async {
      launchAtStartup.setup(
        appName: appName,
        appPath: Platform.resolvedExecutable,
      );
      _login = true;
    });
  }

  Future<void> _part(String what, Future<void> Function() start) async {
    try {
      await start();
    } catch (e) {
      debugPrint('anvil: the desktop $what is not available: $e');
    }
  }

  /// The tray's icon for a state: ICO on Windows, PNG elsewhere.
  static String _icon(TrayState state) {
    final name = switch (state) {
      TrayState.offline => 'offline',
      TrayState.ready => 'ready',
      TrayState.dnd => 'dnd',
      TrayState.ringing => 'ringing',
      TrayState.inCall => 'in_call',
    };
    return 'assets/tray/$name.${Platform.isWindows ? 'ico' : 'png'}';
  }

  @override
  Future<void> setTray(TrayState state, String tooltip, TrayMenu menu) async {
    if (!_tray) return;
    await _part('tray', () async {
      await trayManager.setIcon(_icon(state));
      // Linux's indicators show no tooltip.
      if (!Platform.isLinux) await trayManager.setToolTip(tooltip);
      await trayManager.setContextMenu(
        Menu(
          items: [
            MenuItem(key: 'show', label: menu.show),
            MenuItem.separator(),
            MenuItem.checkbox(
              key: 'dnd',
              label: menu.dnd,
              checked: menu.dndOn,
              disabled: !menu.dndAvailable,
            ),
            MenuItem.separator(),
            MenuItem(key: 'quit', label: menu.quit),
          ],
        ),
      );
    });
  }

  @override
  Future<void> showIncoming(IncomingNotice notice) async {
    if (!_notify) return;
    await _part('notifications', () async {
      await _notifications.show(
        id: notice.call,
        title: notice.title,
        body: notice.body,
        payload: '${notice.call}',
        notificationDetails: NotificationDetails(
          linux: LinuxNotificationDetails(
            urgency: LinuxNotificationUrgency.critical,
            resident: true,
            actions: [
              LinuxNotificationAction(key: 'answer', label: notice.answer),
              LinuxNotificationAction(key: 'decline', label: notice.decline),
            ],
          ),
          windows: WindowsNotificationDetails(
            scenario: WindowsNotificationScenario.incomingCall,
            actions: [
              WindowsAction(content: notice.answer, arguments: 'answer'),
              WindowsAction(content: notice.decline, arguments: 'decline'),
            ],
          ),
          macOS: const DarwinNotificationDetails(
            categoryIdentifier: _category,
            interruptionLevel: InterruptionLevel.timeSensitive,
          ),
        ),
      );
    });
  }

  @override
  Future<void> clearIncoming(int call) async {
    if (!_notify) return;
    await _part('notifications', () => _notifications.cancel(id: call));
  }

  void _response(NotificationResponse response) {
    final call = int.tryParse(response.payload ?? '');
    final action = switch (response.actionId) {
      'answer' => ShellAction.answer,
      'decline' => ShellAction.decline,
      _ => ShellAction.show,
    };
    _events.add(ShellEvent(action, call: call));
  }

  @override
  Future<void> showWindow() async {
    if (!_window) return;
    await _part('window', () async {
      await windowManager.show();
      await windowManager.focus();
    });
  }

  @override
  Future<void> quit() async {
    if (_tray) await _part('tray', trayManager.destroy);
    if (_window) {
      await _part('window', () async {
        await windowManager.setPreventClose(false);
        await windowManager.destroy();
      });
    } else {
      exit(0);
    }
  }

  @override
  bool get canStartAtLogin => _login;

  @override
  Future<bool> startsAtLogin() async {
    if (!_login) return false;
    try {
      return await launchAtStartup.isEnabled();
    } catch (e) {
      debugPrint('anvil: start at login unknown: $e');
      return false;
    }
  }

  @override
  Future<void> setStartsAtLogin(bool on) async {
    if (!_login) return;
    on ? await launchAtStartup.enable() : await launchAtStartup.disable();
  }

  // The tray: a click opens the window, a right click the menu.
  @override
  void onTrayIconMouseDown() => _events.add(const ShellEvent(ShellAction.show));

  @override
  void onTrayIconRightMouseDown() => trayManager.popUpContextMenu();

  @override
  void onTrayMenuItemClick(MenuItem menuItem) {
    final action = switch (menuItem.key) {
      'show' => ShellAction.show,
      'dnd' => ShellAction.toggleDnd,
      'quit' => ShellAction.quit,
      _ => null,
    };
    if (action != null) _events.add(ShellEvent(action));
  }

  // Closing the window keeps the phone running in the tray.
  @override
  void onWindowClose() {
    if (_tray) {
      unawaited(windowManager.hide());
    } else {
      _events.add(const ShellEvent(ShellAction.quit));
    }
  }
}
