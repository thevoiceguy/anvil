import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_localizations/flutter_localizations.dart';

import '../l10n/app_localizations.dart';
import 'design/theme.dart';
import 'desktop/bridge.dart';
import 'desktop/idle.dart';
import 'desktop/shell.dart';
import 'mobile/bridge.dart';
import 'mobile/platform.dart';
import 'phone/phone_api.dart';
import 'phone/phone_model.dart';
import 'update/update_banner.dart';
import 'update/updates.dart';
import 'screens/home.dart';
import 'screens/sign_in.dart';

class AnvilApp extends StatefulWidget {
  const AnvilApp({
    super.key,
    required this.phone,
    this.shell,
    this.updates,
    this.mobile,
    this.idleClock,
    this.idleSetting,
  });
  final PhoneApi phone;

  /// The phone around the app (Android, iOS); none on desktop and in the
  /// screens' tests.
  final MobilePlatform? mobile;

  /// The desktop around the app (tray, notifications); none on mobile and
  /// in the screens' tests.
  final DesktopShell? shell;

  /// The app updating itself; none where a store updates it, and in the
  /// screens' tests unless they give one.
  final UpdateApi? updates;

  /// A desktop's idle time and the user's idle setting: away when idle
  /// (FCP's docs/PRESENCE.md); none on a phone and in the screens' tests
  /// unless they give them.
  final IdleClock? idleClock;
  final IdleSetting? idleSetting;

  @override
  State<AnvilApp> createState() => _AnvilAppState();
}

class _AnvilAppState extends State<AnvilApp> {
  late final PhoneModel model = PhoneModel(widget.phone, mobile: widget.mobile);
  // A managed install may never look for updates: the company installs
  // them.
  late final UpdateModel? updates =
      widget.updates == null ||
          widget.phone.managed.updates == ManagedUpdates.off
      ? null
      : UpdateModel(widget.updates!);

  /// Coming back to the app (on a desktop, its window taking focus) looks
  /// for a newer release again.
  AppLifecycleListener? _lifecycle;

  /// Tells FCP the person is idle, while it keeps their presence.
  late final IdleWatcher? _idle =
      widget.idleClock == null || widget.idleSetting == null
      ? null
      : IdleWatcher(
          clock: widget.idleClock!,
          setting: widget.idleSetting!,
          report: (idle) async {
            if (model.snapshot.presence == null) return;
            await widget.phone.setPresenceIdle(idle);
          },
        );

  @override
  void initState() {
    super.initState();
    model.boot();
    updates?.schedule();
    if (updates != null) {
      _lifecycle = AppLifecycleListener(onResume: updates!.resumed);
    }
    _idle?.start();
  }

  @override
  void dispose() {
    _idle?.stop();
    _lifecycle?.dispose();
    model.dispose();
    updates?.dispose();
    super.dispose();
  }

  /// Quit for good: through the desktop (the tray goes too), else at once.
  Future<void> _quit() async {
    final shell = widget.shell;
    if (shell != null) {
      await shell.quit();
    } else {
      exit(0);
    }
  }

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: model,
      builder: (context, _) {
        final brand = model.brandPrimary;
        final color = brand == null ? null : Color(brand);
        return _app(color);
      },
    );
  }

  Widget _app(Color? brand) {
    return MaterialApp(
      debugShowCheckedModeBanner: false,
      onGenerateTitle: (context) =>
          model.brandName ?? Strings.of(context).appName,
      theme: AnvilTheme.light(brand: brand),
      darkTheme: AnvilTheme.dark(brand: brand),
      localizationsDelegates: const [
        Strings.delegate,
        GlobalMaterialLocalizations.delegate,
        GlobalWidgetsLocalizations.delegate,
        GlobalCupertinoLocalizations.delegate,
      ],
      supportedLocales: Strings.supportedLocales,
      builder: (context, child) {
        var app = child ?? const SizedBox.shrink();
        if (updates != null) {
          app = UpdateScope(updates: updates!, quit: _quit, child: app);
        }
        if (widget.idleSetting != null) {
          app = IdleScope(setting: widget.idleSetting!, child: app);
        }
        if (widget.shell != null) {
          app = DesktopBridge(model: model, shell: widget.shell!, child: app);
        }
        if (widget.mobile != null) {
          app = MobileBridge(model: model, mobile: widget.mobile!, child: app);
        }
        return app;
      },
      home: ListenableBuilder(
        listenable: model,
        builder: (context, _) => model.account == null
            ? SignInScreen(model: model)
            : HomeScreen(model: model),
      ),
    );
  }
}
