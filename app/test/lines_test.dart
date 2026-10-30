// The account's lines (FCP's docs/DEVICE_LINES.md, D8 findings 40 and 42)
// against a fake phone: a column of them on a wide window and a picker on a
// narrow one, calls placed from the line chosen, a call shown on its line,
// and renaming only where the tenant allows.

import 'package:anvil/src/app.dart';
import 'package:anvil/src/phone/phone_api.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'fake_phone.dart';

const lines = [
  PhoneLine(line: 'own', label: '1001 - Bob', defaultLabel: '1001 - Bob'),
  PhoneLine(
    line: 'extension:1005',
    kind: 'extension',
    label: 'Sales',
    defaultLabel: '1005 - Bob',
    voicemailNew: 2,
  ),
  PhoneLine(
    line: 'on_behalf:mary',
    kind: 'on_behalf',
    label: '1003 - Mary',
    defaultLabel: '1003 - Mary',
    ring: false,
    registration: Registration.failed,
  ),
];

Future<FakePhone> running(
  WidgetTester tester, {
  required Size size,
  List<PhoneLine> withLines = lines,
  bool mayRename = false,
}) async {
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  final phone = FakePhone()
    ..saved = const SignedInAccountFake()
    ..seed = (s) => s.copyWith(
      calling: const CallingSettings(),
      lines: withLines,
      mayRenameLines: mayRename,
    );
  await tester.pumpWidget(AnvilApp(phone: phone));
  await tester.pumpAndSettle();
  return phone;
}

Future<void> tapKey(WidgetTester tester, String key) async {
  await tester.tap(find.byKey(Key(key)).last);
  await tester.pumpAndSettle();
}

Future<void> dial(WidgetTester tester, String number) async {
  await tester.enterText(find.byKey(const Key('target')), number);
  await tapKey(tester, 'call');
}

String status(WidgetTester tester, String line) =>
    tester.widget<Text>(find.byKey(Key('lineStatus-$line'))).data!;

const wide = Size(1200, 900);
const narrow = Size(420, 900);

void main() {
  testWidgets('one line shows no lines and dials from the account', (
    tester,
  ) async {
    final phone = await running(tester, size: wide, withLines: [lines.first]);
    expect(find.byKey(const Key('lines')), findsNothing);
    expect(find.byKey(const Key('linePicker')), findsNothing);
    await dial(tester, '1002');
    expect(phone.log, contains('call 1002'));
  });

  testWidgets('a wide window lists the lines and dials from the one chosen', (
    tester,
  ) async {
    final phone = await running(tester, size: wide);
    expect(find.byKey(const Key('lines')), findsOneWidget);
    expect(find.byKey(const Key('linePicker')), findsNothing);
    expect(find.text('Sales'), findsOneWidget);
    expect(status(tester, 'own'), 'Ready');
    expect(status(tester, 'extension:1005'), 'Ready · 2 new');
    expect(
      status(tester, 'on_behalf:mary'),
      "Not connected · Doesn't ring here",
    );
    // No rename where the tenant does not allow it.
    expect(find.byKey(const Key('lineMenu-own')), findsNothing);

    await dial(tester, '1002');
    expect(phone.log.last, 'call 1002');
    await tapKey(tester, 'hangup');

    await tapKey(tester, 'line-extension:1005');
    await dial(tester, '1002');
    expect(phone.log.last, 'call 1002 on extension:1005');
    expect(find.byKey(const Key('callLine')), findsOneWidget);
    expect(find.text('On Sales'), findsOneWidget);
    expect(status(tester, 'extension:1005'), 'On a call · 2 new');
  });

  testWidgets('a narrow window picks the line above the keypad', (
    tester,
  ) async {
    final phone = await running(tester, size: narrow);
    expect(find.byKey(const Key('lines')), findsNothing);
    expect(find.text('From 1001 - Bob'), findsOneWidget);

    await tapKey(tester, 'linePicker');
    expect(find.byKey(const Key('lineSheet')), findsOneWidget);
    await tapKey(tester, 'line-on_behalf:mary');
    expect(find.byKey(const Key('lineSheet')), findsNothing);
    expect(find.text('From 1003 - Mary'), findsOneWidget);

    await dial(tester, '+15551234567');
    expect(phone.log.last, 'call +15551234567 on on_behalf:mary');
  });

  testWidgets('a call ringing in says which line it is on', (tester) async {
    final phone = await running(tester, size: wide);
    phone.ring('1002', name: 'Alice', line: 'on_behalf:mary', silent: true);
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('incomingLine')), findsOneWidget);
    expect(find.text('On 1003 - Mary'), findsOneWidget);
    expect(find.byIcon(Icons.notifications_off_outlined), findsOneWidget);
    expect(status(tester, 'on_behalf:mary'), startsWith('Ringing'));

    // A call FCP named no line for is on the account's own.
    phone.ring('1004');
    await tester.pumpAndSettle();
    expect(find.text('On 1001 - Bob'), findsOneWidget);
  });

  testWidgets('a line is renamed and reset where the tenant allows', (
    tester,
  ) async {
    final phone = await running(tester, size: wide, mayRename: true);
    await tapKey(tester, 'lineMenu-own');
    await tapKey(tester, 'lineRename-own');
    // Its own name, not yet changed: nothing to reset.
    expect(find.byKey(const Key('lineNameReset')), findsNothing);
    await tester.enterText(find.byKey(const Key('lineName')), '  My desk ');
    await tapKey(tester, 'lineNameSave');
    expect(phone.log.last, 'rename own "My desk"');
    expect(find.text('My desk'), findsOneWidget);

    await tapKey(tester, 'lineMenu-own');
    await tapKey(tester, 'lineRename-own');
    await tapKey(tester, 'lineNameReset');
    expect(phone.log.last, 'rename own ""');
    expect(find.text('1001 - Bob'), findsOneWidget);
  });

  testWidgets('a line taken away falls back to the account\'s own', (
    tester,
  ) async {
    final phone = await running(tester, size: wide);
    await tapKey(tester, 'line-extension:1005');
    phone.set(phone.state.copyWith(lines: [lines.first, lines.last]), 'lines');
    await tester.pumpAndSettle();
    await dial(tester, '1002');
    expect(phone.log.last, 'call 1002');
  });
}
