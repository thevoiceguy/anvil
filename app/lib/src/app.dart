import 'package:flutter/material.dart';
import 'package:flutter_localizations/flutter_localizations.dart';

import '../l10n/app_localizations.dart';
import 'design/theme.dart';
import 'desktop/bridge.dart';
import 'desktop/shell.dart';
import 'phone/phone_api.dart';
import 'phone/phone_model.dart';
import 'screens/home.dart';
import 'screens/sign_in.dart';

class AnvilApp extends StatefulWidget {
  const AnvilApp({super.key, required this.phone, this.shell});
  final PhoneApi phone;

  /// The desktop around the app (tray, notifications); none on mobile and
  /// in the screens' tests.
  final DesktopShell? shell;

  @override
  State<AnvilApp> createState() => _AnvilAppState();
}

class _AnvilAppState extends State<AnvilApp> {
  late final PhoneModel model = PhoneModel(widget.phone);

  @override
  void initState() {
    super.initState();
    model.boot();
  }

  @override
  void dispose() {
    model.dispose();
    super.dispose();
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
      builder: widget.shell == null
          ? null
          : (context, child) => DesktopBridge(
              model: model,
              shell: widget.shell!,
              child: child ?? const SizedBox.shrink(),
            ),
      home: ListenableBuilder(
        listenable: model,
        builder: (context, _) => model.account == null
            ? SignInScreen(model: model)
            : HomeScreen(model: model),
      ),
    );
  }
}
