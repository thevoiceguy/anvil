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

import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:path_provider/path_provider.dart';

/// The app's support directory, where its session is kept.
late String supportDir;

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
    expect(find.byKey(const Key('continue')), findsOneWidget);
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
  /// (thirty on iOS) is null.
  Future<SystemCallEvent?> next(int call) async {
    try {
      return await NativeMobile().callEvents
          .firstWhere((e) => e.call == call)
          .timeout(Duration(seconds: Platform.isAndroid ? 10 : 30));
    } catch (_) {
      return null;
    }
  }

  // The system's call screen takes the app's calls: Telecom on Android
  // (the emulator has it); CallKit on iOS, which a simulator may refuse —
  // then the app rings on its own, and nothing breaks. An app with the voip
  // background mode may also hear nothing back from a simulator's CallKit
  // for its first call (null).
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
        isIn([SystemCallAction.shown, SystemCallAction.failed, null]),
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

  // FCP wakes the app with a push (FCP's docs/PUSH.md §2.5): a call push
  // rings at once while the phone wakes and registers, and the phone's
  // call takes its place. The emulator has no Firebase project and PushKit
  // does not reach a simulator, so the pushes are handed in as FCM or
  // PushKit would hand them. A simulator's CallKit may drop the call it was
  // given (`failed`), as above.
  File session() => File('$supportDir/session.json');

  /// The call a push showed is gone: ended, or on a simulator dropped.
  final over = Platform.isAndroid
      ? contains(SystemCallAction.ended)
      : anyOf(
          contains(SystemCallAction.ended),
          contains(SystemCallAction.failed),
        );

  /// The events for the call a push shows (ids below zero) until it is no
  /// longer shown, or fifteen seconds. Telecom keeps an ended call a moment
  /// after it ends and refuses a new one as busy until it lets it go — the
  /// last test's call, here — so [deliver] is tried again while it does.
  Future<List<SystemCallAction>> pushed(Future<void> Function() deliver) async {
    final seen = <SystemCallAction>[];
    final over = Completer<void>();
    var busy = Completer<void>();
    final events = NativeMobile().callEvents.where((e) => e.call < 0).listen((
      e,
    ) {
      seen.add(e.action);
      if (e.action == SystemCallAction.failed &&
          e.reason == 'busy' &&
          !busy.isCompleted) {
        busy.complete();
      }
      if (e.action == SystemCallAction.ended && !over.isCompleted) {
        over.complete();
      }
    });
    for (var tries = 0; tries < 20; tries++) {
      busy = Completer<void>();
      await deliver();
      final refused = await Future.any([
        busy.future.then((_) => true),
        over.future.then((_) => false),
        Future<bool>.delayed(const Duration(seconds: 15), () => false),
      ]);
      if (!refused) break;
      seen.clear();
      await Future<void>.delayed(const Duration(milliseconds: 250));
    }
    unawaited(events.cancel());
    return seen;
  }

  testWidgets('a push to an app signed out rings nothing', (tester) async {
    supportDir = (await getApplicationSupportDirectory()).path;
    if (session().existsSync()) session().deleteSync();
    final seen = <SystemCallAction>[];
    final events = NativeMobile().callEvents
        .where((e) => e.call < 0)
        .listen((e) => seen.add(e.action));
    await NativeMobile().deliverPush({
      'type': 'call',
      'call': 'push-signed-out',
      'from': '1002',
      'name': 'Ann Lee',
    });
    await Future<void>.delayed(const Duration(seconds: 3));
    unawaited(events.cancel());
    expect(seen, isEmpty);
  });

  testWidgets('a call push rings, and stops when the phone cannot wake', (
    tester,
  ) async {
    supportDir = (await getApplicationSupportDirectory()).path;
    // A session the phone cannot start from: no server to register with.
    session().writeAsStringSync('not a session');
    addTearDown(() => session().deleteSync());
    final seen = await pushed(
      () => NativeMobile().deliverPush({
        'type': 'call',
        'call': 'push-no-phone',
        'from': '1002',
        'name': 'Ann Lee',
      }),
    );
    debugPrint('pushed call: ${seen.map((a) => a.name).join(' ')}');
    expect(seen, over);
  });

  testWidgets('a cancel push ends the call a push showed', (tester) async {
    supportDir = (await getApplicationSupportDirectory()).path;
    session().writeAsStringSync('not a session');
    addTearDown(() => session().deleteSync());
    final seen = await pushed(() async {
      await NativeMobile().deliverPush({
        'type': 'call',
        'call': 'push-cancelled',
        'from': '1002',
      });
      await NativeMobile().deliverPush({
        'type': 'cancel',
        'call': 'push-cancelled',
      });
    });
    debugPrint('cancelled call: ${seen.map((a) => a.name).join(' ')}');
    expect(seen, over);
  });

  testWidgets('a test push is taken', (tester) async {
    await NativeMobile().deliverPush({'type': 'test', 'call': 'test-1'});
  });
}
