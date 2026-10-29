// Away when idle (FCP's docs/PRESENCE.md §4): on a desktop, no keyboard or
// mouse for the user's idle time (Settings, 10 minutes by default, or
// off) tells FCP the person is idle; input again says they are back. FCP
// lets an app's idleness lapse after 15 minutes, so it is said again every
// ten while it lasts. How long the system has seen no input:
// GetLastInputInfo on Windows; GNOME's IdleMonitor, else the freedesktop
// screen saver's idle time, over D-Bus on Linux; `anvil/idle` (the
// runner's CGEventSource) on macOS. A system that does not say leaves the
// user as they are.

import 'dart:async';
import 'dart:convert';
import 'dart:ffi';
import 'dart:io';

import 'package:dbus/dbus.dart';
import 'package:ffi/ffi.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

/// How long since the system last saw the keyboard or mouse.
abstract class IdleClock {
  /// Null where the system does not say.
  Future<Duration?> idleFor();
}

/// The idle time the user chose, in minutes; 0 is off.
class IdleSetting extends ChangeNotifier {
  IdleSetting({this.minutes = defaultMinutes, this.save});

  static const defaultMinutes = 10;
  static const choices = [0, 5, 10, 15, 30];

  int minutes;

  /// Where a change is kept.
  final Future<void> Function(int minutes)? save;

  void set(int value) {
    if (value == minutes) return;
    minutes = value;
    notifyListeners();
    unawaited(save?.call(value));
  }

  /// The setting kept in `file` (`{"minutes": 10}`); the default without.
  static Future<IdleSetting> read(File file) async {
    var minutes = defaultMinutes;
    try {
      final kept = jsonDecode(await file.readAsString());
      if (kept is Map && kept['minutes'] is int) minutes = kept['minutes'];
    } catch (_) {}
    return IdleSetting(
      minutes: minutes,
      save: (m) async {
        try {
          await file.writeAsString(jsonEncode({'minutes': m}));
        } catch (e) {
          debugPrint('anvil: the idle setting was not kept: $e');
        }
      },
    );
  }
}

/// Watches the system's idle time and tells FCP through `report`.
class IdleWatcher {
  IdleWatcher({
    required this.clock,
    required this.setting,
    required this.report,
    this.every = const Duration(seconds: 30),
  });

  final IdleClock clock;
  final IdleSetting setting;

  /// Tell FCP the person is idle (true) or back (false).
  final Future<void> Function(bool idle) report;
  final Duration every;

  /// How often idleness is said again while it lasts.
  static const repeat = Duration(minutes: 10);

  Timer? _timer;
  bool _idle = false;
  Duration _since = Duration.zero;

  bool get idle => _idle;

  void start() {
    _timer?.cancel();
    _timer = Timer.periodic(every, (_) => unawaited(check()));
    setting.addListener(_settingChanged);
  }

  void stop() {
    _timer?.cancel();
    _timer = null;
    setting.removeListener(_settingChanged);
  }

  void _settingChanged() => unawaited(check());

  /// Look once.
  Future<void> check() async {
    final after = setting.minutes;
    final idleFor = after == 0 ? null : await clock.idleFor();
    final idle = idleFor != null && idleFor >= Duration(minutes: after);
    if (idle != _idle) {
      _idle = idle;
      _since = Duration.zero;
      await _say(idle);
    } else if (idle) {
      _since += every;
      if (_since >= repeat) {
        _since = Duration.zero;
        await _say(true);
      }
    }
  }

  Future<void> _say(bool idle) async {
    try {
      await report(idle);
    } catch (e) {
      debugPrint('anvil: idleness was not said: $e');
    }
  }
}

/// The system's idle time, as each desktop says it.
class SystemIdleClock implements IdleClock {
  static const _mac = MethodChannel('anvil/idle');

  @override
  Future<Duration?> idleFor() async {
    try {
      if (Platform.isWindows) return _windows();
      if (Platform.isMacOS) {
        final secs = await _mac.invokeMethod<double>('idleSeconds');
        return secs == null
            ? null
            : Duration(milliseconds: (secs * 1000).round());
      }
      if (Platform.isLinux) return await _linux();
    } catch (e) {
      debugPrint('anvil: the idle time is not known: $e');
    }
    return null;
  }

  static Duration? _windows() {
    final user32 = DynamicLibrary.open('user32.dll');
    final kernel32 = DynamicLibrary.open('kernel32.dll');
    final getLastInputInfo = user32
        .lookupFunction<
          Int32 Function(Pointer<_LastInputInfo>),
          int Function(Pointer<_LastInputInfo>)
        >('GetLastInputInfo');
    final getTickCount = kernel32
        .lookupFunction<Uint32 Function(), int Function()>('GetTickCount');
    final info = calloc<_LastInputInfo>();
    try {
      info.ref.cbSize = sizeOf<_LastInputInfo>();
      if (getLastInputInfo(info) == 0) return null;
      // Both wrap at 2^32 milliseconds.
      final ms = (getTickCount() - info.ref.dwTime) & 0xFFFFFFFF;
      return Duration(milliseconds: ms);
    } finally {
      calloc.free(info);
    }
  }

  DBusClient? _bus;

  Future<Duration?> _linux() async {
    final bus = _bus ??= DBusClient.session();
    try {
      final reply = await bus.callMethod(
        destination: 'org.gnome.Mutter.IdleMonitor',
        path: DBusObjectPath('/org/gnome/Mutter/IdleMonitor/Core'),
        interface: 'org.gnome.Mutter.IdleMonitor',
        name: 'GetIdletime',
        replySignature: DBusSignature('t'),
      );
      return Duration(milliseconds: (reply.values.first as DBusUint64).value);
    } catch (_) {}
    try {
      final reply = await bus.callMethod(
        destination: 'org.freedesktop.ScreenSaver',
        path: DBusObjectPath('/org/freedesktop/ScreenSaver'),
        interface: 'org.freedesktop.ScreenSaver',
        name: 'GetSessionIdleTime',
        replySignature: DBusSignature('u'),
      );
      // KDE answers in milliseconds.
      return Duration(milliseconds: (reply.values.first as DBusUint32).value);
    } catch (_) {}
    return null;
  }
}

final class _LastInputInfo extends Struct {
  @Uint32()
  external int cbSize;
  @Uint32()
  external int dwTime;
}

/// The idle setting, for Settings to show (desktop only).
class IdleScope extends InheritedNotifier<IdleSetting> {
  const IdleScope({
    super.key,
    required IdleSetting setting,
    required super.child,
  }) : super(notifier: setting);

  static IdleSetting? maybeOf(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<IdleScope>()?.notifier;
}
