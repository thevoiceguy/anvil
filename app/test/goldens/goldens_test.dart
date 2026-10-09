// Golden screenshots of every screen (`docs/APP.md` §6): one set, made on
// Linux, compared on Linux, Windows and macOS. "The same on every desktop"
// is this test passing on all three.
//
// Rendered with the app's own fonts (Inter, Material Icons) at a fixed
// clock. Regenerate after a deliberate change to a screen:
//   flutter test test/goldens --update-goldens

import 'dart:convert';

import 'package:anvil/src/app.dart';
import 'package:anvil/src/phone/phone_api.dart';
import 'package:clock/clock.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../fake_phone.dart';
import '../fake_shell.dart';
import 'tolerant_comparator.dart';

final now = DateTime(2026, 10, 8, 14, 30);

PhoneSnapshot seeded(PhoneSnapshot s) => s.copyWith(
  people: const [
    Person(
      key: 'ann',
      name: 'Ann Lee',
      extension: '1002',
      department: 'Sales',
      jobTitle: 'Account manager',
      presence: 'available',
      dial: '1002',
    ),
    Person(
      key: 'bob',
      name: 'Bob Ray',
      extension: '1003',
      department: 'Support',
      presence: 'busy',
      onCall: true,
      favourite: true,
      dial: '1003',
    ),
    Person(
      key: 'cy',
      name: 'Cy Doe',
      extension: '1004',
      presence: 'away',
      dial: '1004',
    ),
    Person(key: 'sales', name: 'Sales queue', extension: '2000', dial: '2000'),
  ],
  recents: [
    RecentCall(
      id: 'r1',
      direction: Direction.incoming,
      remote: 'sip:1002@x',
      missed: true,
      startedAt: now.subtract(const Duration(minutes: 20)),
    ),
    RecentCall(
      id: 'r2',
      direction: Direction.outgoing,
      remote: 'sip:+15125550100@x',
      startedAt: now.subtract(const Duration(days: 1)),
      durationSecs: 125,
    ),
    RecentCall(
      id: 'r3',
      direction: Direction.incoming,
      remote: 'sip:1004@x',
      startedAt: now.subtract(const Duration(days: 4)),
      durationSecs: 3725,
    ),
  ],
  voicemail: [
    VoicemailMessage(
      id: 'm1',
      caller: '+15125550100',
      callerName: 'Carol',
      isNew: true,
      urgent: true,
      durationSecs: 17,
      transcription: 'Hi, it is Carol. Call me back about the order, please.',
      receivedAt: now.subtract(const Duration(hours: 2)),
    ),
    VoicemailMessage(
      id: 'm2',
      caller: '1004',
      durationSecs: 42,
      receivedAt: now.subtract(const Duration(days: 2)),
    ),
  ],
  calling: const CallingSettings(forwardBusy: '1009', noAnswerSecs: 20),
  inputs: const [AudioDevice(id: 'usb', name: 'USB Headset')],
  outputs: const [AudioDevice(id: 'spk', name: 'Speakers', isDefault: true)],
);

/// The app's fonts, as the bundle declares them: tests otherwise draw text
/// in a placeholder font.
Future<void> loadFonts() async {
  final manifest =
      json.decode(await rootBundle.loadString('FontManifest.json')) as List;
  for (final family in manifest.cast<Map<String, dynamic>>()) {
    final loader = FontLoader(family['family'] as String);
    for (final font in (family['fonts'] as List).cast<Map<String, dynamic>>()) {
      loader.addFont(rootBundle.load(font['asset'] as String));
    }
    await loader.load();
  }
}

Future<FakePhone> open(
  WidgetTester tester, {
  required Size size,
  required Brightness brightness,
  bool signedIn = true,
}) async {
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1;
  tester.platformDispatcher.platformBrightnessTestValue = brightness;
  addTearDown(tester.view.reset);
  addTearDown(tester.platformDispatcher.clearPlatformBrightnessTestValue);
  final phone = FakePhone()..seed = seeded;
  if (signedIn) phone.saved = const SignedInAccountFake();
  await tester.pumpWidget(AnvilApp(phone: phone, shell: FakeShell()));
  await tester.pumpAndSettle();
  return phone;
}

Future<void> page(WidgetTester tester, String name) async {
  await tester.tap(find.text(name).last);
  await tester.pumpAndSettle();
}

/// A call to Ann, up, with Carol held.
void twoCalls(FakePhone phone) => phone.set(
  phone.state.copyWith(
    calls: const [
      PhoneCall(
        id: 1,
        direction: Direction.incoming,
        remote: 'sip:+15125550100@x',
        displayName: 'Carol',
        state: CallState.connected,
        held: true,
        codec: 'pcmu',
      ),
      PhoneCall(
        id: 2,
        direction: Direction.outgoing,
        remote: 'sip:1002@x',
        state: CallState.connected,
        codec: 'opus',
        encrypted: true,
        quality: CallQuality(jitterMs: 4, lossPermille: 0, rttMs: 30),
      ),
    ],
  ),
  'call',
);

void main() {
  setUpAll(() async {
    await loadFonts();
    useTolerantGoldens();
  });

  const wide = Size(1000, 700);
  const narrow = Size(400, 800);

  for (final brightness in Brightness.values) {
    final theme = brightness.name;

    Future<void> shot(
      WidgetTester tester,
      String name,
      Future<void> Function(FakePhone phone) arrange, {
      Size size = wide,
      bool signedIn = true,
    }) => withClock(Clock.fixed(now), () async {
      final phone = await open(
        tester,
        size: size,
        brightness: brightness,
        signedIn: signedIn,
      );
      await arrange(phone);
      await tester.pumpAndSettle();
      await expectLater(
        find.byType(AnvilApp),
        matchesGoldenFile('$name-$theme.png'),
      );
    });

    testWidgets('sign in, $theme', (tester) async {
      await shot(tester, 'sign_in', (_) async {}, signedIn: false);
    });
    testWidgets('keypad, $theme', (tester) async {
      await shot(tester, 'keypad', (_) async {});
    });
    testWidgets('incoming, $theme', (tester) async {
      await shot(tester, 'incoming', (phone) async {
        phone.ring('1002');
      });
    });
    testWidgets('in a call, $theme', (tester) async {
      await shot(tester, 'in_call', (phone) async => twoCalls(phone));
    });
    testWidgets('recents, $theme', (tester) async {
      await shot(tester, 'recents', (_) => page(tester, 'Recents'));
    });
    testWidgets('people, $theme', (tester) async {
      await shot(tester, 'people', (_) => page(tester, 'People'));
    });
    testWidgets('voicemail, $theme', (tester) async {
      await shot(tester, 'voicemail', (_) => page(tester, 'Voicemail'));
    });
    testWidgets('settings, $theme', (tester) async {
      await shot(tester, 'settings', (_) => page(tester, 'Settings'));
    });
    testWidgets('narrow in a call, $theme', (tester) async {
      await shot(
        tester,
        'narrow_in_call',
        (phone) async => twoCalls(phone),
        size: narrow,
      );
    });
  }
}
