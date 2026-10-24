// The sign-in screen: who and where first, then the password, then a
// one-time code only when the server asks for one; filled in with the last
// sign-in when it comes back.

import 'package:anvil/src/app.dart';
import 'package:anvil/src/phone/phone_api.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'fake_phone.dart';

String fieldText(WidgetTester tester, String key) =>
    tester.widget<TextField>(find.byKey(Key(key))).controller!.text;

Future<void> continueAs(
  WidgetTester tester, {
  String server = 'pbx.example.com',
  String user = 'alice',
}) async {
  await tester.enterText(find.byKey(const Key('server')), server);
  await tester.enterText(find.byKey(const Key('username')), user);
  await tester.tap(find.byKey(const Key('continue')));
  await tester.pumpAndSettle();
}

void main() {
  testWidgets('the first form asks who and where, not a password or code', (
    tester,
  ) async {
    await tester.pumpWidget(AnvilApp(phone: FakePhone()));
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('server')), findsOneWidget);
    expect(find.byKey(const Key('username')), findsOneWidget);
    expect(find.byKey(const Key('password')), findsNothing);
    expect(find.byKey(const Key('code')), findsNothing);
    expect(find.byKey(const Key('signInBrowser')), findsNothing);
  });

  testWidgets('the server or an email address is asked for first', (
    tester,
  ) async {
    final phone = FakePhone();
    await tester.pumpWidget(AnvilApp(phone: phone));
    await tester.pumpAndSettle();
    await continueAs(tester, server: '', user: 'alice');
    expect(find.byKey(const Key('signInError')), findsOneWidget);
    expect(phone.log, isEmpty);

    // An email address as the username is enough to find the server.
    await continueAs(tester, server: '', user: 'alice@example.com');
    expect(phone.log, ['methods  alice@example.com']);
    expect(find.byKey(const Key('password')), findsOneWidget);
  });

  testWidgets('a server that cannot be found is said at once', (tester) async {
    final phone = FakePhone()..discoverError = 'FCP could not be reached';
    await tester.pumpWidget(AnvilApp(phone: phone));
    await tester.pumpAndSettle();
    await continueAs(tester);
    expect(find.textContaining('FCP could not be reached'), findsOneWidget);
    expect(find.byKey(const Key('password')), findsNothing);
  });

  testWidgets('a one-time code is asked for only when the server wants it', (
    tester,
  ) async {
    final phone = FakePhone()..wantsCode = true;
    await tester.pumpWidget(AnvilApp(phone: phone));
    await tester.pumpAndSettle();
    await continueAs(tester);
    expect(find.byKey(const Key('code')), findsNothing);
    expect(find.text('alice on pbx.example.com'), findsOneWidget);

    await tester.enterText(find.byKey(const Key('password')), 'right');
    await tester.tap(find.byKey(const Key('signIn')));
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('code')), findsOneWidget);
    expect(find.byKey(const Key('password')), findsNothing);

    await tester.enterText(find.byKey(const Key('code')), '000000');
    await tester.tap(find.byKey(const Key('verifyCode')));
    await tester.pumpAndSettle();
    expect(find.textContaining('The code is not right'), findsOneWidget);
    expect(
      find.byKey(const Key('code')),
      findsOneWidget,
      reason: 'another try',
    );

    await tester.enterText(find.byKey(const Key('code')), '123456');
    await tester.tap(find.byKey(const Key('verifyCode')));
    await tester.pumpAndSettle();
    expect(
      phone.log,
      containsAllInOrder([
        'signIn pbx.example.com alice',
        'code 000000',
        'code 123456',
        'start',
      ]),
    );
    expect(find.text('Ready'), findsOneWidget);
  });

  testWidgets('without a second factor the password signs straight in', (
    tester,
  ) async {
    final phone = FakePhone();
    await tester.pumpWidget(AnvilApp(phone: phone));
    await tester.pumpAndSettle();
    await continueAs(tester);
    await tester.enterText(find.byKey(const Key('password')), 'right');
    await tester.tap(find.byKey(const Key('signIn')));
    await tester.pumpAndSettle();
    expect(phone.log, isNot(contains(startsWith('code'))));
    expect(find.text('Ready'), findsOneWidget);
  });

  testWidgets('the account can be changed before the password', (tester) async {
    await tester.pumpWidget(AnvilApp(phone: FakePhone()));
    await tester.pumpAndSettle();
    await continueAs(tester);
    await tester.tap(find.byKey(const Key('changeAccount')));
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('password')), findsNothing);
    expect(fieldText(tester, 'server'), 'pbx.example.com');
    expect(fieldText(tester, 'username'), 'alice');
  });

  testWidgets('it comes back filled in with the last server and username', (
    tester,
  ) async {
    final phone = FakePhone()
      ..remembered = const RememberedSignIn('pbx.example.com', 'alice');
    await tester.pumpWidget(AnvilApp(phone: phone));
    await tester.pumpAndSettle();
    expect(fieldText(tester, 'server'), 'pbx.example.com');
    expect(fieldText(tester, 'username'), 'alice');
  });

  testWidgets('after signing out the sign-in is filled in, the password not', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(1000, 1400);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    final phone = FakePhone();
    await tester.pumpWidget(AnvilApp(phone: phone));
    await tester.pumpAndSettle();
    await continueAs(tester, server: 'pbx.example.com', user: 'bob');
    await tester.enterText(find.byKey(const Key('password')), 'right');
    await tester.tap(find.byKey(const Key('signIn')));
    await tester.pumpAndSettle();
    expect(find.text('Ready'), findsOneWidget);

    await tester.tap(find.text('Settings').last);
    await tester.pumpAndSettle();
    await tester.ensureVisible(find.byKey(const Key('signOut')));
    await tester.tap(find.byKey(const Key('signOut')));
    await tester.pumpAndSettle();
    expect(fieldText(tester, 'server'), 'pbx.example.com');
    expect(fieldText(tester, 'username'), 'bob');
    expect(find.byKey(const Key('password')), findsNothing);
  });
}
