// The desktop shell on Windows, macOS and Linux: tray_manager for the tray,
// flutter_local_notifications for a call ringing in, window_manager to keep
// the window in the tray when closed, launch_at_startup for starting at
// login. Each part that fails to start is logged and left out. On Windows a
// second start hands its arguments to this one (windows/runner/instance.cpp)
// and the session ending signs the phone out first, over `anvil/instance`.
// A brand's icon is made into the window's, the tray's and the Dock's by
// `anvil-app` (system_brand.rs), which also renames Windows' shortcuts and
// a Linux menu entry; the Dock's is set over `anvil/dock` (macOS).

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter_local_notifications/flutter_local_notifications.dart';
import 'package:launch_at_startup/launch_at_startup.dart';
import 'package:tray_manager/tray_manager.dart';
import 'package:window_manager/window_manager.dart';

import '../phone/rust_phone.dart';
import '../rust/api/phone.dart' as rust;
import 'links.dart';
import 'shell.dart';

class NativeShell with TrayListener, WindowListener implements DesktopShell {
  /// `args`: what the app was started with.
  NativeShell({List<String> args = const []})
    : _start = startEvents(args, handedOn: false),
      _hidden = Platform.isWindows && args.contains(hiddenArgument);

  static const _instance = MethodChannel('anvil/instance');
  static const _dock = MethodChannel('anvil/dock');
  // tray_manager's own: on macOS it takes an icon only from the app's
  // assets, so a brand's file is handed to it as its Dart side would.
  static const _trayChannel = MethodChannel('tray_manager');

  final _events = StreamController<ShellEvent>.broadcast();
  List<ShellEvent> _start;

  /// Started at login: the runner left the window unshown.
  final bool _hidden;
  Future<void> Function()? _sessionEnds;
  final _notifications = FlutterLocalNotificationsPlugin();
  bool _tray = false;
  bool _notify = false;
  bool _window = false;
  bool _login = false;

  /// The tray's state, and the brand's icon for each (none: Anvil's own).
  TrayState _trayState = TrayState.offline;
  Map<TrayState, String>? _brandTray;

  /// Each brand is shown in turn, never two at once.
  Future<void> _branding = Future.value();

  static bool get supported =>
      !kIsWeb && (Platform.isWindows || Platform.isMacOS || Platform.isLinux);

  // Windows identifies the app's notifications by these.
  static const _aumid = 'Anvil.Softphone';
  static const _guid = '6d1c5a3e-8f0b-4c47-9a5e-2b7d0e4f1a93';
  static const _category = 'incoming_call';

  @override
  Stream<ShellEvent> get events => _events.stream;

  @override
  List<ShellEvent> takeStartEvents() {
    final taken = _start;
    _start = [];
    return taken;
  }

  @override
  void whenSessionEnds(Future<void> Function()? stop) => _sessionEnds = stop;

  Future<Object?> _fromRunner(MethodCall call) async {
    switch (call.method) {
      case 'started':
        final args = [for (final a in call.arguments as List) '$a'];
        startEvents(args, handedOn: true).forEach(_events.add);
      case 'ending':
        await _sessionEnds?.call();
    }
    return null;
  }

  @override
  Future<void> init({
    required String appName,
    String? displayName,
    String? iconPath,
  }) async {
    final shown = displayName ?? appName;
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
    // Started at login with no tray to be found in: the window after all.
    if (_hidden && !_tray && _window) {
      await _part('window', windowManager.show);
    }
    if (Platform.isWindows) {
      _instance.setMethodCallHandler(_fromRunner);
    }
    await _part('notifications', () async {
      await _notifications.initialize(
        settings: InitializationSettings(
          linux: LinuxInitializationSettings(defaultActionName: shown),
          // Registered under the app id at every start: the installer's
          // name and icon would otherwise go back to Anvil's.
          windows: WindowsInitializationSettings(
            appName: shown,
            appUserModelId: _aumid,
            guid: _guid,
            iconPath: iconPath,
          ),
          // Not asked for at start: a prompt before anyone has signed in,
          // and one nobody answers on a build machine (it waits forever).
          macOS: DarwinInitializationSettings(
            requestAlertPermission: false,
            requestSoundPermission: false,
            requestBadgePermission: false,
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
      final path = Platform.resolvedExecutable;
      if (Platform.isWindows) {
        // Windows starts the app in the tray at login. The entry an older
        // version wrote (the bare path) is rewritten if it is on.
        launchAtStartup.setup(appName: appName, appPath: path);
        final before = await launchAtStartup.isEnabled();
        launchAtStartup.setup(
          appName: appName,
          appPath: '"$path"',
          args: const [hiddenArgument],
        );
        if (before) await launchAtStartup.enable();
      } else {
        launchAtStartup.setup(appName: appName, appPath: path);
      }
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

  /// The tray's icon for a state: the brand's, else Anvil's own (ICO on
  /// Windows, PNG elsewhere).
  String _icon(TrayState state) => _brandTray?[state] ?? _ownIcon(state);

  /// A state's name, as Anvil's tray icons and the brand's are named.
  static String _stateName(TrayState state) => switch (state) {
    TrayState.offline => 'offline',
    TrayState.ready => 'ready',
    TrayState.dnd => 'dnd',
    TrayState.ringing => 'ringing',
    TrayState.inCall => 'in_call',
  };

  static String _ownIcon(TrayState state) =>
      'assets/tray/${_stateName(state)}.${Platform.isWindows ? 'ico' : 'png'}';

  Future<void> _setTrayIcon(TrayState state) async {
    final icon = _icon(state);
    if (Platform.isMacOS && _brandTray != null) {
      await _trayChannel.invokeMethod('setIcon', {
        'id': '${state.name}-${icon.hashCode}',
        'iconPath': icon,
        'base64Icon': base64Encode(await File(icon).readAsBytes()),
        'isTemplate': false,
        'iconPosition': 'left',
        'iconSize': 18,
      });
    } else {
      await trayManager.setIcon(icon);
    }
  }

  @override
  Future<void> setTitle(String title) =>
      _part('window', () => windowManager.setTitle(title));

  @override
  Future<void> setBrand(String? name, Uint8List? icon) =>
      _branding = _branding.then((_) => _showBrand(name, icon));

  Future<void> _showBrand(String? name, Uint8List? icon) async {
    rust.SystemLook? look;
    await _part('brand', () async {
      final sep = Platform.pathSeparator;
      look = await rust.systemBrand(
        dir: '${await RustPhone.directory()}${sep}brand',
        name: name,
        icon: icon,
      );
    });
    final shown = look;
    if (shown == null) {
      _brandTray = null;
    } else {
      final byName = {for (final t in shown.tray) t.state: t.path};
      _brandTray = {
        for (final s in TrayState.values) s: ?byName[_stateName(s)],
      };
    }
    if (_window && !Platform.isMacOS) {
      await _part(
        'window',
        () => windowManager.setIcon(
          shown?.window ??
              'assets/app/anvil.${Platform.isWindows ? 'ico' : 'png'}',
        ),
      );
    }
    if (Platform.isMacOS) {
      await _part('dock', () => _dock.invokeMethod('setIcon', shown?.png));
    }
    if (_tray) await _part('tray', () => _setTrayIcon(_trayState));
  }

  @override
  Future<void> setTray(TrayState state, String tooltip, TrayMenu menu) async {
    if (!_tray) return;
    _trayState = state;
    await _part('tray', () async {
      await _setTrayIcon(state);
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
  Future<void> allowNotifications() async {
    if (!_notify || !Platform.isMacOS) return;
    await _part('notifications', () async {
      await _notifications
          .resolvePlatformSpecificImplementation<
            MacOSFlutterLocalNotificationsPlugin
          >()
          ?.requestPermissions(alert: true, sound: true);
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
