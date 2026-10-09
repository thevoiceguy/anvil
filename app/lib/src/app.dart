import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_localizations/flutter_localizations.dart';

import '../l10n/app_localizations.dart';
import 'design/theme.dart';
import 'desktop/bridge.dart';
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

  @override
  State<AnvilApp> createState() => _AnvilAppState();
}

class _AnvilAppState extends State<AnvilApp> {
  late final PhoneModel model = PhoneModel(widget.phone, mobile: widget.mobile);
  late final UpdateModel? updates = widget.updates == null
      ? null
      : UpdateModel(widget.updates!);

  @override
  void initState() {
    super.initState();
    model.boot();
    updates?.schedule();
  }

  @override
  void dispose() {
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
        final brand = model.snapshot.brandPrimary;
        final color = brand == null ? null : Color(brand);
        return _app(color);
      },
    );
  }

  Widget _app(Color? brand) {
    return MaterialApp(
      debugShowCheckedModeBanner: false,
      onGenerateTitle: (context) =>
          model.snapshot.brandName ?? Strings.of(context).appName,
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
