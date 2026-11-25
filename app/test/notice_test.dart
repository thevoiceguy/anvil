// A call that ended says why for a few seconds and then clears itself
// (D8 finding 71: it stayed until dismissed); a refusal, which the user
// may need to act on, stays.

import 'package:anvil/src/app.dart';
import 'package:anvil/src/phone/phone_model.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'fake_phone.dart';

void main() {
  Future<FakePhone> signedIn(WidgetTester tester) async {
    final phone = FakePhone()..saved = const SignedInAccountFake();
    await tester.pumpWidget(AnvilApp(phone: phone));
    await tester.pumpAndSettle();
    return phone;
  }

  testWidgets('a call ended notice clears itself after a few seconds', (
    tester,
  ) async {
    final phone = await signedIn(tester);
    phone.set(phone.state, 'call_ended', call: 1, reason: 'Busy');
    // The event's stream delivers, then the banner builds.
    await tester.pump();
    await tester.pump();
    expect(find.byKey(const Key('notice')), findsOneWidget);

    await tester.pump(PhoneModel.noticeFades - const Duration(seconds: 1));
    expect(find.byKey(const Key('notice')), findsOneWidget);
    await tester.pump(const Duration(seconds: 2));
    expect(find.byKey(const Key('notice')), findsNothing);
  });

  testWidgets('a newer notice is not cleared by an older one\'s time', (
    tester,
  ) async {
    final phone = await signedIn(tester);
    phone.set(phone.state, 'call_ended', call: 1, reason: 'Busy');
    await tester.pump(PhoneModel.noticeFades - const Duration(seconds: 1));
    phone.set(phone.state, 'call_ended', call: 2, reason: 'Declined');
    await tester.pump(const Duration(seconds: 2));
    expect(
      find.byKey(const Key('notice')),
      findsOneWidget,
      reason: 'the second notice has its own few seconds',
    );
    await tester.pump(PhoneModel.noticeFades);
    expect(find.byKey(const Key('notice')), findsNothing);
  });
}
