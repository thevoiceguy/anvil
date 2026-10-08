// The app as built, its Rust core loaded: it starts and asks to sign in.
// Run on a desktop: `flutter test integration_test -d linux` (or windows,
// macos).

import 'package:anvil/src/app.dart';
import 'package:anvil/src/phone/rust_phone.dart';
import 'package:anvil/src/rust/frb_generated.dart';
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
  });
}
