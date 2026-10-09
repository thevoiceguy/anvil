// On a phone the app asks for the microphone and sets the audio up before
// the phone starts, and says so when the microphone is refused; and the
// system's call screen follows the phone's calls, and what the user does
// there reaches the phone.

import 'package:anvil/src/app.dart';
import 'package:anvil/src/mobile/platform.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'fake_mobile.dart';
import 'fake_phone.dart';

void main() {
  testWidgets('the microphone is asked for and the audio set up first', (
    tester,
  ) async {
    final phone = FakePhone()..saved = const SignedInAccountFake();
    final mobile = FakeMobile(allow: true);
    await tester.pumpWidget(AnvilApp(phone: phone, mobile: mobile));
    await tester.pumpAndSettle();
    expect(mobile.log, ['requestMicrophone', 'startAudio']);
    expect(phone.log, contains('start'));
    expect(find.byKey(const Key('notice')), findsNothing);
  });

  testWidgets('a refused microphone still starts the phone, and says why', (
    tester,
  ) async {
    final phone = FakePhone()..saved = const SignedInAccountFake();
    final mobile = FakeMobile(allow: false);
    await tester.pumpWidget(AnvilApp(phone: phone, mobile: mobile));
    await tester.pumpAndSettle();
    expect(phone.log, contains('start'));
    expect(find.textContaining('may not use the microphone'), findsOneWidget);
  });

  Future<(FakePhone, FakeMobile)> signedIn(WidgetTester tester) async {
    final phone = FakePhone()..saved = const SignedInAccountFake();
    final mobile = FakeMobile();
    await tester.pumpWidget(AnvilApp(phone: phone, mobile: mobile));
    await tester.pumpAndSettle();
    mobile.log.clear();
    return (phone, mobile);
  }

  testWidgets('a call ringing in rings at the system, and is answered there', (
    tester,
  ) async {
    final (phone, mobile) = await signedIn(tester);
    phone.ring('1002', name: 'Ann Lee');
    await tester.pumpAndSettle();
    expect(mobile.log, ['incoming 1 Ann Lee 1002']);

    mobile.act(const SystemCallEvent(SystemCallAction.answer, 1));
    await tester.pumpAndSettle();
    expect(phone.log, contains('answer 1'));
    expect(mobile.log, ['incoming 1 Ann Lee 1002', 'connected 1']);

    mobile.act(const SystemCallEvent(SystemCallAction.end, 1));
    await tester.pumpAndSettle();
    expect(phone.log, contains('hangup 1'));
    expect(mobile.log.last, 'ended 1');
  });

  testWidgets('ending a call ringing in at the system declines it', (
    tester,
  ) async {
    final (phone, mobile) = await signedIn(tester);
    phone.ring('1002');
    await tester.pumpAndSettle();
    mobile.act(const SystemCallEvent(SystemCallAction.end, 1));
    await tester.pumpAndSettle();
    expect(phone.log, contains('decline 1'));
    expect(mobile.log.last, 'ended 1');
  });

  testWidgets('a call placed shows as dialling, then connected and held', (
    tester,
  ) async {
    final (phone, mobile) = await signedIn(tester);
    for (final k in ['1', '0', '0', '3']) {
      await tester.tap(find.byKey(Key('key$k')));
    }
    await tester.tap(find.byKey(const Key('call')));
    await tester.pumpAndSettle();
    expect(mobile.log, ['outgoing 1 1003 1003']);

    phone.answerOutgoing(1);
    await tester.pumpAndSettle();
    expect(mobile.log.last, 'connected 1');

    // Held in the app: the system is told.
    await tester.tap(find.byKey(const Key('hold')));
    await tester.pumpAndSettle();
    expect(mobile.log.last, 'held 1 true');

    // Taken off hold at the system: the phone does it, and the system is
    // not told back what it already knows.
    mobile.act(const SystemCallEvent(SystemCallAction.hold, 1, on: false));
    await tester.pumpAndSettle();
    expect(phone.log, contains('hold 1 false'));
    expect(mobile.log.last, 'held 1 true');
    expect(phone.callOf(1).held, isFalse);
  });

  testWidgets('mute and keys at the system reach the call', (tester) async {
    final (phone, mobile) = await signedIn(tester);
    await phone.call('1003');
    phone.answerOutgoing(1);
    await tester.pumpAndSettle();
    mobile.act(const SystemCallEvent(SystemCallAction.mute, 1, on: true));
    mobile.act(const SystemCallEvent(SystemCallAction.dtmf, 1, digits: '5'));
    await tester.pumpAndSettle();
    expect(phone.log, containsAll(['mute 1 true', 'dtmf 1 5']));
    expect(phone.callOf(1).muted, isTrue);
  });

  testWidgets('a call the system filters is declined; one it cannot show '
      'still rings in the app', (tester) async {
    final (phone, mobile) = await signedIn(tester);
    phone.ring('1002');
    phone.ring('1004');
    await tester.pumpAndSettle();
    mobile.act(
      const SystemCallEvent(SystemCallAction.failed, 1, reason: 'filtered'),
    );
    mobile.act(
      const SystemCallEvent(SystemCallAction.failed, 2, reason: 'unavailable'),
    );
    await tester.pumpAndSettle();
    expect(phone.log, contains('decline 1'));
    expect(phone.log, isNot(contains('decline 2')));
    expect(phone.callOf(2).isRingingIn, isTrue);
  });
}
