// Signing in through the system browser (single sign-on): the address is
// opened, the sign-in finishes when the browser comes back, and waiting can
// be given up.

import 'package:anvil/l10n/app_localizations.dart';
import 'package:anvil/src/phone/phone_api.dart';
import 'package:anvil/src/phone/phone_model.dart';
import 'package:anvil/src/screens/sign_in.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'fake_phone.dart';

Future<(FakePhone, PhoneModel, List<String>)> signInScreen(
  WidgetTester tester,
) async {
  final phone = FakePhone();
  final model = PhoneModel(phone);
  final opened = <String>[];
  await tester.pumpWidget(
    MaterialApp(
      localizationsDelegates: Strings.localizationsDelegates,
      supportedLocales: Strings.supportedLocales,
      home: ListenableBuilder(
        listenable: model,
        builder: (context, _) =>
            SignInScreen(model: model, openUrl: (url) async => opened.add(url)),
      ),
    ),
  );
  await tester.pumpAndSettle();
  return (phone, model, opened);
}

void main() {
  testWidgets('the browser signs in and the phone starts', (tester) async {
    final (phone, model, opened) = await signInScreen(tester);
    await tester.tap(find.byKey(const Key('signInBrowser')));
    await tester.pumpAndSettle();
    expect(
      find.byKey(const Key('signInError')),
      findsOneWidget,
      reason: 'the server is asked for first',
    );

    await tester.enterText(find.byKey(const Key('server')), 'pbx.example.com');
    await tester.tap(find.byKey(const Key('signInBrowser')));
    await tester.pump();
    expect(opened, ['https://pbx.example.com/auth/app/authorize?state=x']);
    expect(find.byKey(const Key('browserWaiting')), findsOneWidget);

    phone.browser.complete(
      const SignedInAccount('alice', 'https://pbx.example.com'),
    );
    await tester.pumpAndSettle();
    expect(model.account?.username, 'alice');
    expect(phone.log, containsAllInOrder(['browser pbx.example.com', 'start']));
    expect(find.byKey(const Key('browserWaiting')), findsNothing);
  });

  testWidgets('waiting for the browser can be given up', (tester) async {
    final (phone, model, _) = await signInScreen(tester);
    await tester.enterText(find.byKey(const Key('server')), 'pbx.example.com');
    await tester.tap(find.byKey(const Key('signInBrowser')));
    await tester.pump();
    await tester.tap(find.byKey(const Key('cancelBrowser')));
    await tester.pumpAndSettle();
    expect(phone.log, contains('browser cancelled'));
    expect(model.account, isNull);
    expect(find.byKey(const Key('signInBrowser')), findsOneWidget);
    expect(find.byKey(const Key('signInError')), findsOneWidget);
  });

  testWidgets('a phone without it shows no browser sign-in', (tester) async {
    final phone = FakePhone()..canSignInWithBrowser = false;
    await tester.pumpWidget(
      MaterialApp(
        localizationsDelegates: Strings.localizationsDelegates,
        supportedLocales: Strings.supportedLocales,
        home: SignInScreen(model: PhoneModel(phone)),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('signInBrowser')), findsNothing);
  });
}
