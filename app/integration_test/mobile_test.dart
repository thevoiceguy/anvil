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

import 'dart:io';

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

  /// The next event the system sends for `call`; none within ten seconds
  /// is null.
  Future<SystemCallEvent?> next(int call) async {
    try {
      return await NativeMobile().callEvents
          .firstWhere((e) => e.call == call)
          .timeout(const Duration(seconds: 10));
    } catch (_) {
      return null;
    }
  }

  // The system's call screen takes the app's calls: Telecom on Android
  // (the emulator has it); CallKit on iOS, which a simulator may refuse —
  // then the app rings on its own, and nothing breaks.
  testWidgets('the system takes a call ringing in, and lets it go', (
    tester,
  ) async {
    final mobile = NativeMobile();
    // What the system says, for the log.
    final said = mobile.callEvents.listen(
      (e) => debugPrint('system: ${e.action.name} ${e.call} ${e.reason ?? ''}'),
    );
    addTearDown(said.cancel);
    final shown = next(901);
    await mobile.reportIncoming(
      const SystemCall(id: 901, name: 'Ann Lee', number: '1002'),
    );
    final event = await shown;
    if (Platform.isAndroid) {
      expect(event?.action, SystemCallAction.shown);
    } else {
      expect(
        event?.action,
        isIn([SystemCallAction.shown, SystemCallAction.failed]),
      );
    }
    await mobile.reportConnected(901);
    await mobile.reportHeld(901, true);
    await mobile.reportHeld(901, false);
    await mobile.reportEnded(901);
  });

  testWidgets('the system takes a call placed, and lets it go', (tester) async {
    final mobile = NativeMobile();
    final shown = next(902);
    await mobile.reportOutgoing(
      const SystemCall(id: 902, name: '1003', number: '1003'),
    );
    final event = await shown;
    if (Platform.isAndroid) {
      expect(event?.action, SystemCallAction.shown);
    } else {
      expect(
        event?.action,
        isIn([SystemCallAction.shown, SystemCallAction.failed]),
      );
    }
    await mobile.reportConnected(902);
    await mobile.reportEnded(902);
  });

  /// The routes once `ok` holds, asked now and at each change; none within
  /// ten seconds is null.
  Future<AudioRoutes?> routesWhen(bool Function(AudioRoutes) ok) async {
    final mobile = NativeMobile();
    final now = await mobile.audioRoutes();
    if (now != null && ok(now)) return now;
    try {
      return await mobile.audioRouteChanges
          .firstWhere(ok)
          .timeout(const Duration(seconds: 10));
    } catch (_) {
      final last = await mobile.audioRoutes();
      return last != null && ok(last) ? last : null;
    }
  }

  // Android's routes come with a Telecom call; iOS's from the audio session.
  testWidgets("a call's audio goes to the speaker and back", (tester) async {
    final mobile = NativeMobile();
    if (Platform.isAndroid) {
      final shown = next(903);
      await mobile.reportIncoming(
        const SystemCall(id: 903, name: 'Ann Lee', number: '1002'),
      );
      expect((await shown)?.action, SystemCallAction.shown);
      await mobile.reportConnected(903);
    }
    final routes = await routesWhen((r) => r.available.isNotEmpty);
    debugPrint(
      'routes: ${routes?.current.name} of '
      '${routes?.available.map((r) => r.name).join(',')}',
    );
    expect(routes, isNotNull);
    expect(routes!.available, contains(AudioRoute.speaker));

    await mobile.setAudioRoute(AudioRoute.speaker);
    if (Platform.isAndroid) {
      final speaker = await routesWhen((r) => r.current == AudioRoute.speaker);
      expect(
        speaker,
        isNotNull,
        reason: 'Telecom moved the call to the speaker',
      );
      if (routes.available.contains(AudioRoute.earpiece)) {
        await mobile.setAudioRoute(AudioRoute.earpiece);
        final back = await routesWhen((r) => r.current == AudioRoute.earpiece);
        expect(back, isNotNull, reason: 'and back to the earpiece');
      }
    }
    await mobile.setProximity(true);
    await mobile.setProximity(false);
    if (Platform.isAndroid) await mobile.reportEnded(903);
  });
}
