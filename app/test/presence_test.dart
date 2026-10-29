// The user's presence (FCP's docs/PRESENCE.md, D8 finding 33) against a
// fake phone: the chip says what others see and picks a status, a note and
// when it clears; the user's own statuses are added, changed, reordered and
// deleted; people show each other's; away when idle.

import 'package:anvil/src/app.dart';
import 'package:anvil/src/desktop/idle.dart';
import 'package:anvil/src/design/format.dart';
import 'package:anvil/src/phone/phone_api.dart';
import 'package:anvil/src/screens/presence.dart';
import 'package:anvil/l10n/app_localizations_en.dart';
import 'package:clock/clock.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'fake_phone.dart';

const available = MyPresence(
  shown: 'available',
  status: 'available',
  statusState: 'available',
);

Future<FakePhone> running(
  WidgetTester tester, {
  MyPresence? presence = available,
  IdleSetting? idleSetting,
  IdleClock? idleClock,
}) async {
  tester.view.physicalSize = const Size(1000, 1400);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  final phone = FakePhone()
    ..saved = const SignedInAccountFake()
    ..seed = (s) {
      final signed = s.copyWith(calling: const CallingSettings());
      return presence == null ? signed : signed.copyWith(presence: presence);
    };
  await tester.pumpWidget(
    AnvilApp(phone: phone, idleSetting: idleSetting, idleClock: idleClock),
  );
  await tester.pumpAndSettle();
  return phone;
}

Future<void> tapKey(WidgetTester tester, String key) async {
  await tester.tap(find.byKey(Key(key)).last);
  await tester.pumpAndSettle();
}

String chip(WidgetTester tester) =>
    tester.widget<Text>(find.byKey(const Key('presenceLabel'))).data!;

class ScriptedIdle implements IdleClock {
  Duration? idle = Duration.zero;
  @override
  Future<Duration?> idleFor() async => idle;
}

void main() {
  testWidgets('without presence the chip is the registration', (tester) async {
    await running(tester, presence: null);
    expect(find.byKey(const Key('registration')), findsOneWidget);
    expect(find.byKey(const Key('presence')), findsNothing);
  });

  testWidgets('the chip picks a status, a note and when it clears', (
    tester,
  ) async {
    final phone = await running(tester);
    expect(chip(tester), 'Available');

    await tapKey(tester, 'presence');
    await tapKey(tester, 'status-busy');
    expect(phone.log, contains('presence busy'));
    expect(chip(tester), 'Busy');

    await tapKey(tester, 'presence');
    await tapKey(tester, 'status-dnd');
    expect(chip(tester), 'Do not disturb');
    expect(phone.state.dnd, isTrue, reason: 'a dnd status is DND');
    expect(find.byKey(const Key('dndChip')), findsNothing, reason: 'one chip');

    await tapKey(tester, 'presence');
    await tapKey(tester, 'presenceNote');
    await tester.enterText(
      find.byKey(const Key('presenceNoteText')),
      'back at 2',
    );
    await tapKey(tester, 'presenceNoteSave');
    expect(phone.log, contains('presence note back at 2'));
    expect(phone.state.presence!.note, 'back at 2');

    await withClock(Clock.fixed(DateTime(2026, 10, 29, 9)), () async {
      await tapKey(tester, 'presence');
      await tapKey(tester, 'presenceClearAfter');
      await tapKey(tester, 'clearAfter-1h');
    });
    expect(phone.log.last, 'presence dnd "back at 2" until');
    expect(phone.state.presence!.until, DateTime(2026, 10, 29, 10));

    // Something else others see lies over the chosen status.
    phone.set(
      phone.state.copyWith(
        presence: const MyPresence(
          shown: 'on_call',
          shownLabel: 'WFH',
          status: 'status-1',
          statusState: 'available',
          statuses: [
            OwnStatus(id: 'status-1', label: 'WFH', state: 'available'),
          ],
        ),
      ),
      'presence',
    );
    await tester.pumpAndSettle();
    expect(chip(tester), 'On a call · WFH');
  });

  testWidgets('the user keeps statuses of their own', (tester) async {
    final phone = await running(tester);
    await tapKey(tester, 'presence');
    await tapKey(tester, 'editStatuses');
    expect(find.text('Add your own, like WFH or At lunch.'), findsOneWidget);

    Future<void> add(String label, String state) async {
      await tapKey(tester, 'addStatus');
      await tester.enterText(find.byKey(const Key('statusLabel')), label);
      await tester.pumpAndSettle();
      // A new status shows as available unless another is chosen.
      if (state != 'available') {
        await tapKey(tester, 'statusState');
        await tester.tap(find.byKey(Key('statusState-$state')).last);
        await tester.pumpAndSettle();
      }
      await tapKey(tester, 'statusSave');
    }

    await add('WFH', 'available');
    await add('Focus', 'dnd');
    expect(
      phone.log,
      containsAll(['status add available WFH', 'status add dnd Focus']),
    );
    expect(find.text('WFH'), findsOneWidget);

    // Changed: Focus becomes busy.
    await tester.tap(find.text('Focus'));
    await tester.pumpAndSettle();
    await tapKey(tester, 'statusState');
    await tester.tap(find.text('Busy').last);
    await tester.pumpAndSettle();
    await tapKey(tester, 'statusSave');
    expect(phone.log, contains('status edit status-2 - busy'));

    await tester.tap(find.text('Done'));
    await tester.pumpAndSettle();

    // Picked from the menu, shown by its label.
    await tapKey(tester, 'presence');
    await tapKey(tester, 'status-status-1');
    expect(phone.log, contains('presence status-1'));
    expect(chip(tester), 'WFH');

    // Deleted while in use: available again.
    await tapKey(tester, 'presence');
    await tapKey(tester, 'editStatuses');
    await tapKey(tester, 'deleteStatus-status-1');
    expect(phone.log, contains('status remove status-1'));
    await tester.tap(find.text('Done'));
    await tester.pumpAndSettle();
    expect(chip(tester), 'Available');
  });

  test('people show a status of their own, and calls over it', () {
    final s = StringsEn();
    Person p(String presence, [String? label]) => Person(
      key: 'k',
      name: 'K',
      dial: 'k',
      presence: presence,
      presenceLabel: label,
    );
    expect(presenceLabel(s, p('available', 'WFH')), 'WFH');
    expect(presenceLabel(s, p('on_call', 'WFH')), 'On a call · WFH');
    expect(presenceLabel(s, p('ringing')), 'Ringing');
    expect(presenceLabel(s, p('dnd')), 'Do not disturb');
    expect(presenceLabel(s, p('offline')), 'Offline');
    expect(shownText(s, 'available', null, idle: true), 'Away (idle)');
  });

  test('a status clears at the time chosen', () {
    final at = DateTime(2026, 10, 29, 9, 15);
    expect(clearAt('30m', at), DateTime(2026, 10, 29, 9, 45));
    expect(clearAt('2h', at), DateTime(2026, 10, 29, 11, 15));
    expect(clearAt('today', at), DateTime(2026, 10, 29, 23, 59));
    expect(clearAt('never', at), isNull);
  });

  testWidgets('idle after the time chosen, said again, and back', (
    tester,
  ) async {
    final idle = ScriptedIdle();
    final setting = IdleSetting(minutes: 10);
    final said = <bool>[];
    final watcher = IdleWatcher(
      clock: idle,
      setting: setting,
      report: (i) async => said.add(i),
    );

    idle.idle = const Duration(minutes: 9);
    await watcher.check();
    expect(said, isEmpty);
    idle.idle = const Duration(minutes: 10);
    await watcher.check();
    expect(said, [true]);
    // Said again every ten minutes while it lasts (FCP lets it lapse at 15).
    for (var i = 0; i < 20; i++) {
      await watcher.check();
    }
    expect(said, [true, true]);
    idle.idle = const Duration(seconds: 3);
    await watcher.check();
    expect(said, [true, true, false]);

    // Off: never idle; a system that does not say, never idle either.
    setting.set(0);
    idle.idle = const Duration(hours: 1);
    await watcher.check();
    expect(said.last, isFalse);
    setting.set(5);
    idle.idle = null;
    await watcher.check();
    expect(said.length, 3);
  });

  testWidgets(
    'the app says idle only with presence, and settings choose when',
    (tester) async {
      final idle = ScriptedIdle()..idle = const Duration(minutes: 20);
      final setting = IdleSetting(minutes: 10);
      final phone = await running(
        tester,
        idleSetting: setting,
        idleClock: idle,
      );
      await tester.pump(const Duration(seconds: 31));
      await tester.pumpAndSettle();
      expect(phone.log, contains('presence idle true'));
      expect(chip(tester), 'Away (idle)');

      await tester.tap(find.text('Settings').last);
      await tester.pumpAndSettle();
      await tester.ensureVisible(find.byKey(const Key('idleMinutes')));
      await tapKey(tester, 'idleMinutes');
      await tapKey(tester, 'idleMinutes-30');
      expect(setting.minutes, 30);
      await tester.pumpAndSettle();
      expect(phone.log.last, 'presence idle false', reason: 'not idle for 30');
    },
  );
}
