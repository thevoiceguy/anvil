// The app as built for a phone, its Rust core loaded: it starts and asks to
// sign in, and the audio session is set up for calls with the system's
// audio reachable from Rust (on Android through the JVM `MainActivity`
// hands over). Run on an emulator or simulator:
// `flutter test integration_test/mobile_test.dart -d <device>`.

import 'package:anvil/src/app.dart';
import 'package:anvil/src/mobile/platform.dart';
import 'package:anvil/src/phone/rust_phone.dart';
import 'package:anvil/src/rust/api/phone.dart' as rust;
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
    await tester.pumpWidget(
      AnvilApp(phone: RustPhone(), mobile: NativeMobile()),
    );
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('signIn')), findsOneWidget);
  });

  // As the app does before the phone starts: on iOS the microphone is an
  // input only once the session plays and records.
  testWidgets('the audio is set up for calls and Rust reaches it', (
    tester,
  ) async {
    expect(NativeMobile.supported, isTrue);
    await NativeMobile().startAudio();
    final audio = await rust.systemAudio();
    expect(audio.inputs, isNotEmpty, reason: 'a microphone');
    expect(audio.outputs, isNotEmpty, reason: 'a speaker');
  });
}
