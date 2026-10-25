import 'package:flutter/material.dart';

import 'src/app.dart';
import 'src/desktop/native_shell.dart';
import 'src/mobile/platform.dart';
import 'src/phone/rust_phone.dart';
import 'src/rust/frb_generated.dart';
import 'src/update/rust_updates.dart';

Future<void> main(List<String> args) async {
  WidgetsFlutterBinding.ensureInitialized();
  await RustLib.init();
  final shell = NativeShell.supported ? NativeShell(args: args) : null;
  final phone = RustPhone();
  await shell?.init(
    appName: 'Anvil',
    displayName: phone.managed.brandName,
    iconPath: phone.managed.brandIconPath,
  );
  runApp(
    AnvilApp(
      phone: phone,
      shell: shell,
      // A store updates the app on a phone.
      updates: NativeMobile.supported ? null : RustUpdates(),
      mobile: NativeMobile.supported ? NativeMobile() : null,
    ),
  );
}
