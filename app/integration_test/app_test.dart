// The app as built, its Rust core loaded: it starts and asks to sign in, and
// the desktop around it (window, tray, notifications, start at login)
// starts with it. Run on a desktop: `flutter test integration_test -d linux`
// (or windows, macos).

import 'package:anvil/src/app.dart';
import 'package:anvil/src/desktop/native_shell.dart';
import 'package:anvil/src/desktop/shell.dart';
import 'package:anvil/src/phone/rust_phone.dart';
import 'package:anvil/src/rust/frb_generated.dart';
import 'package:anvil/src/update/rust_updates.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  setUpAll(() async => await RustLib.init());

  testWidgets('the app starts with its Rust core and asks to sign in', (
    tester,
  ) async {
    await tester.pumpWidget(AnvilApp(phone: RustPhone()));
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('signIn')), findsOneWidget);
    // The updater knows this copy's version (the workspace's).
    expect(RustUpdates().version(), matches(RegExp(r'^\d+\.\d+\.\d+$')));
  });

  testWidgets('the desktop shell starts and takes the tray and a notice', (
    tester,
  ) async {
    final shell = NativeShell();
    await shell.init(appName: 'Anvil');
    await shell.setTray(
      TrayState.ready,
      'Anvil — Ready',
      const TrayMenu(
        show: 'Show Anvil',
        dnd: 'Do not disturb',
        dndOn: false,
        dndAvailable: false,
        quit: 'Quit',
      ),
    );
    await shell.showIncoming(
      const IncomingNotice(
        call: 1,
        title: 'Incoming call',
        body: 'Ann Lee (1002)',
        answer: 'Answer',
        decline: 'Decline',
      ),
    );
    await shell.clearIncoming(1);
    // Whether it may start at login is the system's to say; asking must
    // not fail.
    await shell.startsAtLogin();
    await tester.pumpWidget(AnvilApp(phone: RustPhone(), shell: shell));
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('signIn')), findsOneWidget);
  });
}
