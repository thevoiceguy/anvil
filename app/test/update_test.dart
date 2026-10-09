// The app updating itself, against a fake updater: found half a minute after
// start, taken from the banner or settings (never during a call), or, for a
// copy that cannot update itself, pointed at.

import 'package:anvil/src/app.dart';
import 'package:anvil/src/phone/phone_api.dart';
import 'package:anvil/src/update/updates.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'fake_phone.dart';
import 'fake_shell.dart';
import 'fake_updates.dart';

const installable = UpdateInfo(
  version: '0.2.0',
  notes: 'https://example.com/v0.2.0',
  canInstall: true,
);

Future<(FakePhone, FakeShell, FakeUpdates)> running(
  WidgetTester tester, {
  UpdateInfo? next = installable,
}) async {
  tester.view.physicalSize = const Size(1000, 1400);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  final phone = FakePhone()
    ..saved = const SignedInAccountFake()
    ..seed = (s) => s.copyWith(calling: const CallingSettings());
  final shell = FakeShell();
  final updates = FakeUpdates()..next = next;
  await tester.pumpWidget(
    AnvilApp(phone: phone, shell: shell, updates: updates),
  );
  await tester.pumpAndSettle();
  return (phone, shell, updates);
}

void main() {
  testWidgets('an update found after start is taken from the banner', (
    tester,
  ) async {
    final (phone, shell, updates) = await running(tester);
    expect(updates.log, isEmpty, reason: 'not at once');
    expect(find.byKey(const Key('updateBanner')), findsNothing);

    await tester.pump(const Duration(seconds: 31));
    await tester.pumpAndSettle();
    expect(updates.log, ['check']);
    expect(find.text('Anvil 0.2.0 is available'), findsOneWidget);

    await tester.tap(find.byKey(const Key('updateNow')));
    await tester.pumpAndSettle();
    expect(updates.log, ['check', 'download', 'install']);
    expect(phone.log.last, 'stop phone', reason: 'it unregisters first');
    expect(shell.log.last, 'quit');
  });

  testWidgets('not during a call', (tester) async {
    final (phone, _, updates) = await running(tester);
    await tester.pump(const Duration(seconds: 31));
    phone.ring('1002');
    await phone.answer(1);
    await tester.pumpAndSettle();
    expect(find.text('After the call'), findsOneWidget);
    expect(
      tester.widget<FilledButton>(find.byKey(const Key('updateNow'))).onPressed,
      isNull,
    );
    await phone.hangup(1);
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('updateNow')));
    await tester.pumpAndSettle();
    expect(updates.log, contains('install'));
  });

  testWidgets('later puts the banner away until a newer release', (
    tester,
  ) async {
    final (_, _, updates) = await running(tester);
    await tester.pump(const Duration(seconds: 31));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('updateLater')));
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('updateBanner')), findsNothing);

    // Twelve hours on, the same release: still away. A newer one: back.
    await tester.pump(const Duration(hours: 12));
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('updateBanner')), findsNothing);
    updates.next = const UpdateInfo(
      version: '0.3.0',
      notes: 'https://example.com/v0.3.0',
      canInstall: true,
    );
    await tester.pump(const Duration(hours: 12));
    await tester.pumpAndSettle();
    expect(find.text('Anvil 0.3.0 is available'), findsOneWidget);
  });

  testWidgets('a copy that cannot update itself points at the file', (
    tester,
  ) async {
    await running(
      tester,
      next: const UpdateInfo(
        version: '0.2.0',
        notes: 'https://example.com/v0.2.0',
        canInstall: false,
        reason: 'installed from the .deb: install the new one with apt',
        download: 'https://example.com/anvil_v0.2.0_amd64.deb',
      ),
    );
    await tester.pump(const Duration(seconds: 31));
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('updateNow')), findsNothing);
    expect(find.byKey(const Key('updateDownload')), findsOneWidget);

    await tester.tap(find.text('Settings').last);
    await tester.pumpAndSettle();
    await tester.ensureVisible(find.byKey(const Key('updates')));
    expect(find.textContaining('install the new one with apt'), findsOneWidget);
  });

  testWidgets('settings check by hand and say how it went', (tester) async {
    final (_, _, updates) = await running(tester, next: null);
    await tester.tap(find.text('Settings').last);
    await tester.pumpAndSettle();
    await tester.ensureVisible(find.byKey(const Key('updates')));
    expect(find.text('Anvil 0.1.0'), findsOneWidget);

    await tester.tap(find.byKey(const Key('checkForUpdates')));
    await tester.pumpAndSettle();
    expect(find.text('Anvil is up to date'), findsOneWidget);

    updates.failWith = Exception('offline');
    await tester.tap(find.byKey(const Key('checkForUpdates')));
    await tester.pumpAndSettle();
    expect(find.textContaining('Could not update'), findsOneWidget);

    updates
      ..failWith = null
      ..next = installable;
    await tester.tap(find.byKey(const Key('checkForUpdates')));
    await tester.pumpAndSettle();
    await tester.ensureVisible(find.byKey(const Key('settingsUpdateNow')));
    await tester.tap(find.byKey(const Key('settingsUpdateNow')));
    await tester.pumpAndSettle();
    expect(updates.log, containsAllInOrder(['download', 'install']));
  });
}
