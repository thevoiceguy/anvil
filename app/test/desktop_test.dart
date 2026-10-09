// The desktop bridge against a fake shell: the tray follows the phone, a
// call ringing in is a notification until it stops ringing, and the tray's
// and notifications' actions reach the phone.

import 'package:anvil/src/app.dart';
import 'package:anvil/src/desktop/shell.dart';
import 'package:anvil/src/phone/phone_api.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'fake_phone.dart';
import 'fake_shell.dart';

Future<(FakePhone, FakeShell)> running(WidgetTester tester) async {
  tester.view.physicalSize = const Size(1000, 1400);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  final phone = FakePhone()
    ..saved = const SignedInAccountFake()
    ..seed = (s) => s.copyWith(
      calling: const CallingSettings(),
      people: const [
        Person(key: 'ann', name: 'Ann Lee', extension: '1002', dial: '1002'),
      ],
    );
  final shell = FakeShell();
  await tester.pumpWidget(AnvilApp(phone: phone, shell: shell));
  await tester.pumpAndSettle();
  return (phone, shell);
}

void main() {
  testWidgets('the tray follows the phone', (tester) async {
    final (phone, shell) = await running(tester);
    expect(shell.log.where((l) => l == 'allow notifications'), hasLength(1));
    expect(shell.tray, TrayState.ready);
    expect(shell.tooltip, 'Anvil — Ready');
    expect(shell.menu!.dndAvailable, isTrue);

    await phone.setDnd(true);
    await tester.pumpAndSettle();
    expect(shell.tray, TrayState.dnd);
    expect(shell.menu!.dndOn, isTrue);

    phone.ring('1002');
    await tester.pumpAndSettle();
    expect(shell.tray, TrayState.ringing);
    await phone.answer(1);
    await tester.pumpAndSettle();
    expect(shell.tray, TrayState.inCall);

    phone.set(
      phone.state.copyWith(calls: const [], registration: Registration.failed),
      'registration',
    );
    await tester.pumpAndSettle();
    expect(shell.tray, TrayState.offline);

    phone.brand('Acme Phone', '#AA2200');
    await tester.pumpAndSettle();
    expect(shell.tooltip, 'Acme Phone — Offline');
    expect(shell.menu!.show, 'Show Acme Phone');
  });

  testWidgets('a call ringing in is a notice until it stops ringing', (
    tester,
  ) async {
    final (phone, shell) = await running(tester);
    phone.ring('1002');
    await tester.pumpAndSettle();
    expect(
      shell.notices[1]!.body,
      'Ann Lee (1002)',
      reason: 'named from the directory',
    );
    expect(shell.notices[1]!.answer, 'Answer');

    phone.ring('+15125550100', name: 'Carol');
    await tester.pumpAndSettle();
    expect(shell.notices.keys, [1, 2]);

    await tester.tap(find.byKey(const Key('answer')).first);
    await tester.pumpAndSettle();
    expect(shell.notices.keys, [2], reason: 'answered: its notice goes');
    phone.hungUp(2);
    await tester.pumpAndSettle();
    expect(shell.notices, isEmpty, reason: 'given up: its notice goes');
  });

  testWidgets(
    'a notice answers or declines, the tray shows, toggles and quits',
    (tester) async {
      final (phone, shell) = await running(tester);
      phone.ring('1002');
      phone.ring('1003');
      await tester.pumpAndSettle();

      shell.click(ShellAction.answer, call: 2);
      await tester.pumpAndSettle();
      expect(phone.log, contains('answer 2'));
      expect(
        shell.log,
        contains('show'),
        reason: 'answering brings the window up',
      );

      shell.click(ShellAction.decline, call: 1);
      await tester.pumpAndSettle();
      expect(phone.log, contains('decline 1'));

      shell.click(ShellAction.toggleDnd);
      await tester.pumpAndSettle();
      expect(phone.log, contains('dnd true'));

      shell.click(ShellAction.quit);
      await tester.pumpAndSettle();
      expect(phone.log.last, 'stop phone');
      expect(shell.log.last, 'quit');
    },
  );

  testWidgets('settings start the app at login', (tester) async {
    final (_, shell) = await running(tester);
    await tester.tap(find.text('Settings').last);
    await tester.pumpAndSettle();
    await tester.ensureVisible(find.byKey(const Key('startAtLogin')));
    await tester.tap(find.byKey(const Key('startAtLogin')));
    await tester.pumpAndSettle();
    expect(shell.log, contains('login true'));
    expect(
      tester
          .widget<SwitchListTile>(find.byKey(const Key('startAtLogin')))
          .value,
      isTrue,
    );
  });
}
