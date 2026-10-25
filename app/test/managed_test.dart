// A managed install (`docs/MANAGED_INSTALLS.md`): the sign-in screen filled
// in and locked by what the company set, the login domain after a bare
// username, the sign-in mode taken without asking, the profile's brand
// before anyone signs in, and updates left to the company.

import 'dart:typed_data';

import 'package:anvil/l10n/app_localizations.dart';
import 'package:anvil/src/app.dart';
import 'package:anvil/src/design/theme.dart';
import 'package:anvil/src/phone/phone_api.dart';
import 'package:anvil/src/phone/phone_model.dart';
import 'package:anvil/src/screens/sign_in.dart';
import 'package:anvil/src/update/updates.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'fake_phone.dart';
import 'fake_updates.dart';

const acme = Managed(
  server: 'https://voice.acme.example',
  lockServer: true,
  loginDomain: 'acme.example',
  username: 'james@acme.example',
  brandName: 'Acme Voice',
  brandPrimary: 0xFFAA2200,
  sources: ['profile C:\\Anvil\\profile'],
);

String fieldText(WidgetTester tester, String key) =>
    tester.widget<TextField>(find.byKey(Key(key))).controller!.text;

Future<List<String>> signInScreen(WidgetTester tester, FakePhone phone) async {
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
  return opened;
}

void main() {
  testWidgets('the server is the install\'s and locked; the system names '
      'the user; the brand is the profile\'s', (tester) async {
    final phone = FakePhone()
      ..managed = acme
      ..remembered = const RememberedSignIn('other.example', 'ann');
    await signInScreen(tester, phone);
    expect(fieldText(tester, 'server'), 'https://voice.acme.example');
    expect(
      tester.widget<TextField>(find.byKey(const Key('server'))).enabled,
      isFalse,
    );
    expect(find.text('Set by your organisation'), findsOneWidget);
    expect(
      fieldText(tester, 'username'),
      'ann',
      reason: 'the last username comes back; the locked server does not',
    );
    expect(find.text('Acme Voice'), findsOneWidget);
  });

  testWidgets('a bare username signs in with the login domain', (tester) async {
    final phone = FakePhone()
      ..managed = const Managed(loginDomain: 'acme.example', sources: ['x']);
    await signInScreen(tester, phone);
    await tester.enterText(find.byKey(const Key('username')), 'james');
    await tester.pump();
    expect(find.text('@acme.example'), findsOneWidget);
    await tester.tap(find.byKey(const Key('continue')));
    await tester.pumpAndSettle();
    expect(phone.log, [
      'methods  james@acme.example',
    ], reason: 'the email address finds the tenant, and the server');

    await tester.tap(find.byKey(const Key('changeAccount')));
    await tester.pumpAndSettle();
    await tester.enterText(
      find.byKey(const Key('username')),
      'ann@other.example',
    );
    await tester.pump();
    expect(find.text('@acme.example'), findsNothing);
  });

  testWidgets('a password install goes to the password without asking', (
    tester,
  ) async {
    final phone = FakePhone()
      ..managed = const Managed(
        server: 'pbx.example.com',
        signIn: ManagedSignIn.password,
        sources: ['x'],
      );
    await signInScreen(tester, phone);
    await tester.enterText(find.byKey(const Key('username')), 'alice');
    await tester.tap(find.byKey(const Key('continue')));
    await tester.pumpAndSettle();
    expect(phone.log, isEmpty, reason: 'the server is not asked');
    expect(find.byKey(const Key('password')), findsOneWidget);
    expect(find.byKey(const Key('signInBrowser')), findsNothing);
  });

  testWidgets('a single sign-on install opens the browser at once', (
    tester,
  ) async {
    final phone = FakePhone()
      ..managed = const Managed(
        loginDomain: 'acme.example',
        signIn: ManagedSignIn.sso,
        username: 'james@acme.example',
        sources: ['x'],
      );
    final opened = await signInScreen(tester, phone);
    await tester.tap(find.byKey(const Key('continue')));
    await tester.pump();
    await tester.pump();
    expect(phone.log, ['browser ']);
    expect(opened, hasLength(1));
    expect(find.byKey(const Key('browserWaiting')), findsOneWidget);
  });

  testWidgets('the profile\'s brand colours the app before sign-in', (
    tester,
  ) async {
    final phone = FakePhone()
      ..managed = Managed(
        brandName: 'Acme Voice',
        brandPrimary: 0xFFAA2200,
        brandLogo: Uint8List.fromList([1, 2, 3]),
        sources: const ['x'],
      );
    await tester.pumpWidget(AnvilApp(phone: phone));
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('signInTitle')), findsOneWidget);
    expect(find.text('Acme Voice'), findsOneWidget);
    expect(find.byKey(const Key('signInLogo')), findsOneWidget);
    final app = tester.widget<MaterialApp>(find.byType(MaterialApp));
    expect(
      app.theme!.colorScheme.primary,
      AnvilTheme.light(brand: const Color(0xFFAA2200)).colorScheme.primary,
    );
  });

  testWidgets('updates left to the company are never looked for', (
    tester,
  ) async {
    final updates = FakeUpdates();
    final phone = FakePhone()
      ..saved = const SignedInAccountFake()
      ..managed = const Managed(updates: ManagedUpdates.off, sources: ['x']);
    await tester.pumpWidget(AnvilApp(phone: phone, updates: updates));
    await tester.pumpAndSettle();
    await tester.pump(const Duration(minutes: 1));
    expect(updates.log, isEmpty);
  });

  testWidgets('updates the company installs are only told of', (tester) async {
    final updates = FakeUpdates()
      ..next = const UpdateInfo(
        version: '9.9.9',
        notes: 'https://example.com/notes',
        canInstall: false,
      );
    final phone = FakePhone()
      ..saved = const SignedInAccountFake()
      ..managed = const Managed(
        updates: ManagedUpdates.notify,
        sources: ['policy HKLM'],
      );
    await tester.pumpWidget(AnvilApp(phone: phone, updates: updates));
    await tester.pumpAndSettle();
    await tester.pump(const Duration(seconds: 31));
    await tester.pumpAndSettle();
    expect(find.textContaining('installs updates'), findsOneWidget);
    expect(find.byKey(const Key('updateNow')), findsNothing);
    expect(find.byKey(const Key('updateDownload')), findsNothing);
  });
}
