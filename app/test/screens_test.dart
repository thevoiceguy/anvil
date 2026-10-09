// The U2b screens against a fake phone: in a call, transfer, a second call,
// recents, people, voicemail and settings.

import 'package:anvil/src/app.dart';
import 'package:clock/clock.dart';
import 'package:anvil/src/phone/phone_api.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'fake_phone.dart';

const ann = Person(
  key: 'ann',
  name: 'Ann Lee',
  extension: '1002',
  department: 'Sales',
  presence: 'available',
  dial: '1002',
);
const bob = Person(
  key: 'bob',
  name: 'Bob Ray',
  extension: '1003',
  presence: 'busy',
  onCall: true,
  favourite: true,
  dial: '1003',
);
const cy = Person(key: 'cy', name: 'Cy Doe', dial: 'cy');

PhoneSnapshot withData(PhoneSnapshot s) => s.copyWith(
  people: const [ann, bob, cy],
  recents: [
    RecentCall(
      id: 'r1',
      direction: Direction.incoming,
      remote: 'sip:1002@x',
      missed: true,
      startedAt: clock.now(),
    ),
    RecentCall(
      id: 'r2',
      direction: Direction.outgoing,
      remote: 'sip:+15125550100@x',
      startedAt: clock.now().subtract(const Duration(days: 3)),
      durationSecs: 125,
    ),
  ],
  voicemail: const [
    VoicemailMessage(
      id: 'm1',
      caller: '+15125550100',
      callerName: 'Carol',
      isNew: true,
      urgent: true,
      durationSecs: 7,
      transcription: 'Call me back',
    ),
    VoicemailMessage(id: 'm2', caller: '1002', durationSecs: 30),
  ],
  calling: const CallingSettings(forwardBusy: '1009'),
  inputs: const [AudioDevice(id: 'usb', name: 'USB Headset')],
  outputs: const [AudioDevice(id: 'spk', name: 'Speakers', isDefault: true)],
);

Future<FakePhone> signedIn(
  WidgetTester tester, {
  Size size = const Size(1000, 1400),
}) async {
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  final phone = FakePhone()
    ..saved = const SignedInAccountFake()
    ..seed = withData;
  await tester.pumpWidget(AnvilApp(phone: phone));
  await tester.pumpAndSettle();
  return phone;
}

Future<void> tapKey(WidgetTester tester, String key) async {
  await tester.ensureVisible(find.byKey(Key(key)));
  await tester.tap(find.byKey(Key(key)));
  await tester.pumpAndSettle();
}

Future<void> go(WidgetTester tester, String page) async {
  await tester.tap(find.text(page).last);
  await tester.pumpAndSettle();
}

void main() {
  testWidgets('badges count missed calls and new messages', (tester) async {
    await signedIn(tester);
    Badge badge(String page) =>
        tester.widget<Badge>(find.byKey(Key('badge-$page')));
    expect(badge('recents').isLabelVisible, isTrue);
    expect(
      find.descendant(
        of: find.byKey(const Key('badge-recents')),
        matching: find.text('1'),
      ),
      findsOneWidget,
    );
    expect(badge('voicemail').isLabelVisible, isTrue);
    expect(badge('people').isLabelVisible, isFalse);
  });

  testWidgets('a call shows who, its controls, digits and the media', (
    tester,
  ) async {
    final phone = await signedIn(tester);
    await tester.enterText(find.byKey(const Key('target')), '1002');
    await tapKey(tester, 'call');
    expect(phone.log, contains('call 1002'));
    // The directory names the party.
    expect(find.text('Ann Lee'), findsOneWidget);
    expect(find.text('Calling…'), findsOneWidget);

    phone.set(
      phone.state.copyWith(
        calls: [
          phone
              .callOf(1)
              .copyWith(
                state: CallState.connected,
                encrypted: true,
                quality: const CallQuality(jitterMs: 80, lossPermille: 3),
              ),
        ],
      ),
      'call',
      call: 1,
    );
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('encrypted')), findsOneWidget);
    expect(
      tester
          .widget<Tooltip>(
            find.ancestor(
              of: find.byKey(const Key('quality')),
              matching: find.byType(Tooltip),
            ),
          )
          .message,
      'Poor connection',
    );

    await tapKey(tester, 'keypad');
    await tapKey(tester, 'key5');
    await tapKey(tester, 'key#');
    expect(phone.log, containsAllInOrder(['dtmf 1 5', 'dtmf 1 #']));
    expect(find.text('5#'), findsOneWidget);
    await tapKey(tester, 'hideKeypad');

    await tapKey(tester, 'park');
    expect(phone.log, contains('park 1'));
    expect(
      find.byKey(const Key('call')),
      findsOneWidget,
      reason: 'parked: back to the keypad',
    );
  });

  testWidgets('the timer counts from when the call connected', (tester) async {
    final phone = await signedIn(tester);
    phone.ring('1002');
    await tester.pumpAndSettle();
    await tapKey(tester, 'answer');
    phone.set(
      phone.state.copyWith(
        calls: [
          phone
              .callOf(1)
              .copyWith(
                connectedAt: clock.now().subtract(const Duration(seconds: 65)),
              ),
        ],
      ),
      'call',
      call: 1,
    );
    // A ticking timer never settles: pump (the change, then its frame).
    await tester.pump();
    await tester.pump();
    expect(find.text('1:05'), findsOneWidget);
    await tester.pump(const Duration(seconds: 1));
    expect(find.text('1:06'), findsOneWidget);
    await phone.hangup(1);
    await tester.pumpAndSettle();
  });

  testWidgets('a second call holds the first, and the two swap', (
    tester,
  ) async {
    final phone = await signedIn(tester);
    await tester.enterText(find.byKey(const Key('target')), '1002');
    await tapKey(tester, 'call');
    phone.answerOutgoing(1);
    await tester.pumpAndSettle();

    await tapKey(tester, 'addCall');
    expect(find.byKey(const Key('backToCall')), findsOneWidget);
    await tester.enterText(find.byKey(const Key('target')), '1003');
    await tapKey(tester, 'call');
    phone.answerOutgoing(2);
    await tester.pumpAndSettle();
    expect(phone.callOf(1).held, isTrue);
    expect(find.byKey(const Key('callName')), findsOneWidget);
    expect(
      tester.widget<Text>(find.byKey(const Key('callName'))).data,
      'Bob Ray',
    );
    expect(find.byKey(const Key('other-1')), findsOneWidget);

    await tapKey(tester, 'swap-1');
    expect(phone.log, contains('hold 1 false'));
    expect(phone.callOf(2).held, isTrue);
    expect(
      tester.widget<Text>(find.byKey(const Key('callName'))).data,
      'Ann Lee',
    );

    await tapKey(tester, 'hangup-2');
    expect(phone.log, contains('hangup 2'));
  });

  testWidgets('a blind transfer picks from the directory', (tester) async {
    final phone = await signedIn(tester);
    phone.ring('+15125550100', name: 'Carol');
    await tester.pumpAndSettle();
    await tapKey(tester, 'answer');

    await tapKey(tester, 'transfer');
    expect(find.text('Transfer Carol'), findsOneWidget);
    await tester.enterText(find.byKey(const Key('transferTarget')), 'ann');
    await tester.pumpAndSettle();
    await tapKey(tester, 'transferPick-ann');
    await tapKey(tester, 'transferNow');
    expect(phone.log, contains('transfer 1 1002'));

    phone.transferProgress(1, '200 OK');
    await tester.pumpAndSettle();
    expect(find.text('Transfer: 200 OK'), findsOneWidget);
  });

  testWidgets('an attended transfer talks first, then joins the two', (
    tester,
  ) async {
    final phone = await signedIn(tester);
    phone.ring('+15125550100', name: 'Carol');
    await tester.pumpAndSettle();
    await tapKey(tester, 'answer');

    await tapKey(tester, 'transfer');
    await tester.enterText(find.byKey(const Key('transferTarget')), '1003');
    await tapKey(tester, 'transferConsult');
    expect(phone.log, contains('call 1003'));
    expect(phone.callOf(1).held, isTrue);
    expect(
      find.byKey(const Key('completeTransfer')),
      findsNothing,
      reason: 'not until Bob answers',
    );

    phone.answerOutgoing(2);
    await tester.pumpAndSettle();
    await tapKey(tester, 'completeTransfer');
    expect(phone.log, contains('attended 1 2'));
    expect(phone.state.calls, isEmpty);
  });

  testWidgets('a call up shows over other pages and leads back', (
    tester,
  ) async {
    final phone = await signedIn(tester);
    phone.ring('1002');
    await tester.pumpAndSettle();
    await tapKey(tester, 'answer');
    await go(tester, 'People');
    expect(find.byKey(const Key('callBar')), findsOneWidget);
    await tapKey(tester, 'callBar');
    expect(find.byKey(const Key('hangup')), findsOneWidget);
  });

  testWidgets('recents mark the missed and call back', (tester) async {
    final phone = await signedIn(tester);
    await go(tester, 'Recents');
    expect(
      find.text('Ann Lee'),
      findsOneWidget,
      reason: 'named from the directory',
    );
    expect(find.textContaining('Missed'), findsOneWidget);
    expect(find.textContaining('2:05'), findsOneWidget);
    await tapKey(tester, 'callBack-r2');
    expect(phone.log, contains('call sip:+15125550100@x'));
    expect(
      find.byKey(const Key('hangup')),
      findsOneWidget,
      reason: 'the phone page shows the call',
    );
  });

  testWidgets('people are searched, lit, made favourites and called', (
    tester,
  ) async {
    final phone = await signedIn(tester);
    await go(tester, 'People');
    expect(find.text('Favourites'), findsOneWidget);
    expect(find.byKey(const Key('lamp-bob')), findsOneWidget);
    expect(find.byKey(const Key('dot-ann')), findsOneWidget);
    expect(find.textContaining('On a call'), findsOneWidget);

    await tapKey(tester, 'favourite-ann');
    expect(phone.log, contains('favourite ann true'));
    expect(phone.state.people.first.favourite, isTrue);

    await tester.enterText(find.byKey(const Key('peopleSearch')), 'sales');
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('person-ann')), findsOneWidget);
    expect(find.byKey(const Key('person-bob')), findsNothing);

    await tapKey(tester, 'dial-ann');
    expect(phone.log, contains('call 1002'));
  });

  testWidgets('voicemail plays, stops, is marked heard and deleted', (
    tester,
  ) async {
    final phone = await signedIn(tester);
    await go(tester, 'Voicemail');
    expect(find.text('Call me back'), findsOneWidget);
    expect(find.text('Urgent'), findsOneWidget);
    expect(find.byKey(const Key('new-m1')), findsOneWidget);

    await tapKey(tester, 'play-m1');
    expect(phone.log, contains('play m1'));
    expect(find.byKey(const Key('stop-m1')), findsOneWidget);
    expect(
      find.byKey(const Key('new-m1')),
      findsNothing,
      reason: 'played is heard',
    );
    await tapKey(tester, 'stop-m1');
    expect(find.byKey(const Key('play-m1')), findsOneWidget);

    await tapKey(tester, 'delete-m2');
    await tapKey(tester, 'confirmDelete');
    expect(phone.log, contains('delete m2'));
    expect(find.byKey(const Key('message-m2')), findsNothing);
  });

  testWidgets('settings change dnd, call waiting, forwards and audio', (
    tester,
  ) async {
    final phone = await signedIn(tester);
    await go(tester, 'Settings');
    expect(find.text('1009'), findsOneWidget, reason: 'forwarded when busy');

    await tapKey(tester, 'dnd');
    expect(phone.log, contains('dnd true'));
    expect(find.byKey(const Key('dndChip')), findsOneWidget);

    await tapKey(tester, 'callWaiting');
    expect(phone.log.last, startsWith('calling waiting=false'));

    await tapKey(tester, 'forward-noAnswer');
    await tester.enterText(find.byKey(const Key('forwardTarget')), '1010');
    await tester.enterText(find.byKey(const Key('forwardSecs')), '20');
    await tapKey(tester, 'forwardSave');
    expect(phone.log.last, contains('noAnswer=1010'));
    expect(phone.log.last, contains('secs=20'));
    expect(find.text('1010'), findsOneWidget);

    await tapKey(tester, 'forward-busy');
    await tapKey(tester, 'forwardClear');
    expect(phone.log.last, contains('busy= '));
    expect(phone.state.calling!.forwardBusy, isNull);

    await tapKey(tester, 'microphone');
    await tester.tap(find.text('USB Headset').last);
    await tester.pumpAndSettle();
    expect(phone.log, contains('audio in usb'));

    await tapKey(tester, 'signOut');
    expect(phone.log, contains('signOut'));
    expect(find.byKey(const Key('signIn')), findsOneWidget);
  });

  testWidgets('a narrow window navigates from the bottom', (tester) async {
    await signedIn(tester, size: const Size(400, 800));
    expect(find.byType(NavigationBar), findsOneWidget);
    expect(find.byType(NavigationRail), findsNothing);
    await go(tester, 'Voicemail');
    expect(find.text('Call me back'), findsOneWidget);
  });
}
