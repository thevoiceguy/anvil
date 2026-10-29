import 'dart:io';

import 'package:flutter/material.dart';

import 'src/app.dart';
import 'src/desktop/idle.dart';
import 'src/desktop/links.dart';
import 'src/desktop/native_shell.dart';
import 'src/mobile/platform.dart';
import 'src/phone/rust_phone.dart';
import 'src/rust/frb_generated.dart';
import 'src/update/rust_updates.dart';

Future<void> main(List<String> args) async {
  WidgetsFlutterBinding.ensureInitialized();
  await RustLib.init();
  if (args.contains(signOutArgument)) {
    // The uninstaller: no window, and never kept waiting past ten seconds.
    // Offline, the session stays at FCP until it ends there.
    try {
      await RustPhone().signOut().timeout(const Duration(seconds: 10));
    } catch (_) {}
    exit(0);
  }
  final shell = NativeShell.supported ? NativeShell(args: args) : null;
  final phone = RustPhone();
  await shell?.init(
    appName: 'Anvil',
    displayName: phone.managed.brandName,
    iconPath: phone.managed.brandIconPath,
  );
  // Away when idle, on a desktop.
  final idleSetting = shell == null
      ? null
      : await IdleSetting.read(
          File(
            '${await RustPhone.directory()}${Platform.pathSeparator}idle.json',
          ),
        );
  runApp(
    AnvilApp(
      phone: phone,
      shell: shell,
      idleClock: shell == null ? null : SystemIdleClock(),
      idleSetting: idleSetting,
      // A store updates the app on a phone.
      updates: NativeMobile.supported ? null : RustUpdates(),
      mobile: NativeMobile.supported ? NativeMobile() : null,
    ),
  );
}
