import 'package:anvil/src/app.dart';
import 'package:anvil/src/phone/phone_api.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'fake_phone.dart';

void main() {
  testWidgets('signing in starts the phone and shows it ready', (tester) async {
    final phone = FakePhone();
    await tester.pumpWidget(AnvilApp(phone: phone));
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('signIn')), findsOneWidget);

    await tester.enterText(find.byKey(const Key('server')), 'pbx.example.com');
    await tester.enterText(find.byKey(const Key('username')), 'alice');
    await tester.enterText(find.byKey(const Key('password')), 'wrong');
    await tester.tap(find.byKey(const Key('signIn')));
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('signInError')), findsOneWidget);

    await tester.enterText(find.byKey(const Key('password')), 'right');
    await tester.tap(find.byKey(const Key('signIn')));
    await tester.pumpAndSettle();
    expect(
      phone.log,
      containsAllInOrder(['signIn pbx.example.com alice', 'start']),
    );
    expect(find.text('Ready'), findsOneWidget);
    expect(find.byKey(const Key('call')), findsOneWidget);
  });

  Future<FakePhone> signedIn(WidgetTester tester) async {
    final phone = FakePhone()..saved = const SignedInAccountFake();
    await tester.pumpWidget(AnvilApp(phone: phone));
    await tester.pumpAndSettle();
    return phone;
  }

  testWidgets('the keypad dials, and the call can be muted, held and ended', (
    tester,
  ) async {
    final phone = await signedIn(tester);
    for (final k in ['1', '0', '0', '2']) {
      await tester.tap(find.byKey(Key('key$k')));
    }
    await tester.tap(find.byKey(const Key('call')));
    await tester.pumpAndSettle();
    expect(phone.log, contains('call 1002'));
    expect(find.text('Calling…'), findsOneWidget);

    phone.answerOutgoing(1);
    await tester.pumpAndSettle();
    expect(find.text('Connected'), findsOneWidget);

    await tester.tap(find.byKey(const Key('mute')));
    await tester.pumpAndSettle();
    expect(phone.callOf(1).muted, isTrue);
    expect(find.text('Unmute'), findsOneWidget);

    await tester.tap(find.byKey(const Key('hold')));
    await tester.pumpAndSettle();
    expect(find.text('On hold'), findsOneWidget);

    await tester.tap(find.byKey(const Key('hangup')));
    await tester.pumpAndSettle();
    expect(phone.state.calls, isEmpty);
    expect(
      find.byKey(const Key('call')),
      findsOneWidget,
      reason: 'back to the keypad',
    );
  });

  testWidgets('an incoming call is answered from its card', (tester) async {
    final phone = await signedIn(tester);
    phone.ring('bob');
    await tester.pumpAndSettle();
    expect(find.text('Incoming call'), findsOneWidget);
    expect(find.text('bob'), findsOneWidget);

    await tester.tap(find.byKey(const Key('answer')));
    await tester.pumpAndSettle();
    expect(phone.log, contains('answer 1'));
    expect(find.text('Connected'), findsOneWidget);
  });

  testWidgets('a refused command says why', (tester) async {
    final phone = await signedIn(tester);
    phone.refuseWith = 'not registered';
    await tester.enterText(find.byKey(const Key('target')), '1002');
    await tester.tap(find.byKey(const Key('call')));
    await tester.pumpAndSettle();
    expect(find.textContaining('not registered'), findsOneWidget);
  });

  testWidgets("the tenant's brand names and colours the app", (tester) async {
    final phone = await signedIn(tester);
    phone.brand('Acme Phone', '#AA2200');
    await tester.pumpAndSettle();
    expect(find.text('Acme Phone'), findsOneWidget);
    final context = tester.element(find.byKey(const Key('title')));
    final seeded = Theme.of(context).colorScheme.primary;
    expect(seeded, isNot(equals(Theme.of(context).colorScheme.surface)));
    expect(PhoneSnapshot.colorFromHex('#AA2200'), 0xFFAA2200);
    expect(PhoneSnapshot.colorFromHex('nope'), isNull);
  });
}
