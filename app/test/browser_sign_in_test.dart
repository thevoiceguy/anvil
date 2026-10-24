// Signing in through the system browser (single sign-on): offered when the
// server says so, opened at once for an account that only signs in that
// way, finished when the browser comes back, and given up on request.

import 'package:anvil/l10n/app_localizations.dart';
import 'package:anvil/src/phone/phone_api.dart';
import 'package:anvil/src/phone/phone_model.dart';
import 'package:anvil/src/screens/sign_in.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'fake_phone.dart';

Future<(FakePhone, PhoneModel, List<String>)> signInScreen(
  WidgetTester tester, {
  FakePhone? phone,
}) async {
  phone ??= FakePhone();
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

Future<void> identify(
  WidgetTester tester, {
  String server = 'pbx.example.com',
  String user = 'alice',
}) async {
  await tester.enterText(find.byKey(const Key('server')), server);
  await tester.enterText(find.byKey(const Key('username')), user);
  await tester.tap(find.byKey(const Key('continue')));
  await tester.pump();
}

void main() {
  testWidgets('an account that signs in only by single sign-on goes '
      'straight to the browser', (tester) async {
    final phone = FakePhone()
      ..methods = const SignInMethods(password: false, sso: true);
    final (_, model, opened) = await signInScreen(tester, phone: phone);
    await identify(tester, user: 'alice@example.com');
    await tester.pump();
    expect(opened, ['https://pbx.example.com/auth/app/authorize?state=x']);
    expect(find.byKey(const Key('browserWaiting')), findsOneWidget);
    expect(find.byKey(const Key('password')), findsNothing);

    phone.browser.complete(
      const SignedInAccount('alice', 'https://pbx.example.com'),
    );
    await tester.pumpAndSettle();
    expect(model.account?.username, 'alice');
    expect(
      phone.log,
      containsAllInOrder([
        'methods pbx.example.com alice@example.com',
        'browser pbx.example.com',
        'start',
      ]),
    );
  });

  testWidgets('single sign-on beside a password is a small link', (
    tester,
  ) async {
    final phone = FakePhone()
      ..methods = const SignInMethods(password: true, sso: true);
    final (_, model, opened) = await signInScreen(tester, phone: phone);
    await identify(tester);
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('password')), findsOneWidget);
    expect(find.byKey(const Key('signIn')), findsOneWidget);
    final link = find.byKey(const Key('signInBrowser'));
    expect(link, findsOneWidget);
    expect(tester.widget(link), isA<TextButton>());
    expect(find.text('Use single sign-on'), findsOneWidget);

    await tester.tap(link);
    await tester.pump();
    expect(opened, hasLength(1));
    phone.browser.complete(
      const SignedInAccount('alice', 'https://pbx.example.com'),
    );
    await tester.pumpAndSettle();
    expect(model.account?.username, 'alice');
  });

  testWidgets('a password-only account is offered no single sign-on', (
    tester,
  ) async {
    await signInScreen(tester);
    await identify(tester);
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('password')), findsOneWidget);
    expect(find.byKey(const Key('signInBrowser')), findsNothing);
  });

  testWidgets('a server that cannot say offers the password and the link', (
    tester,
  ) async {
    final phone = FakePhone()
      ..methods = const SignInMethods(password: true, sso: true, known: false);
    await signInScreen(tester, phone: phone);
    await identify(tester);
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('password')), findsOneWidget);
    expect(find.byKey(const Key('signInBrowser')), findsOneWidget);
  });

  testWidgets('waiting for the browser can be given up', (tester) async {
    final phone = FakePhone()
      ..methods = const SignInMethods(password: false, sso: true);
    final (_, model, _) = await signInScreen(tester, phone: phone);
    await identify(tester);
    await tester.pump();
    await tester.tap(find.byKey(const Key('cancelBrowser')));
    await tester.pumpAndSettle();
    expect(phone.log, contains('browser cancelled'));
    expect(model.account, isNull);
    expect(
      find.byKey(const Key('signInBrowser')),
      findsOneWidget,
      reason: 'the browser can be tried again',
    );
    expect(find.byKey(const Key('signInError')), findsOneWidget);
  });

  testWidgets('a phone without a browser sign-in says single sign-on is '
      'not here yet', (tester) async {
    final phone = FakePhone()
      ..canSignInWithBrowser = false
      ..methods = const SignInMethods(password: true, sso: true);
    await signInScreen(tester, phone: phone);
    await identify(tester);
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('password')), findsOneWidget);
    expect(find.byKey(const Key('signInBrowser')), findsNothing);

    await tester.tap(find.byKey(const Key('changeAccount')));
    await tester.pumpAndSettle();
    phone.methods = const SignInMethods(password: false, sso: true);
    await tester.tap(find.byKey(const Key('continue')));
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('signInError')), findsOneWidget);
    expect(find.textContaining('single sign-on'), findsOneWidget);
    expect(phone.log, isNot(contains(startsWith('browser'))));
  });
}
