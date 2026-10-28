// The U2b screens against a fake phone: in a call, transfer, a second call,
// recents, people, voicemail and settings.

import 'package:anvil/src/app.dart';
import 'package:clock/clock.dart';
import 'package:anvil/src/phone/phone_api.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
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
  ringtones: const [
    Ringtone(id: 'anvil', name: 'Anvil'),
    Ringtone(id: 'classic', name: 'Classic'),
    Ringtone(id: 'brand:Acme', name: 'Acme'),
  ],
  ringtone: 'anvil',
  echoCancellation: true,
  noiseSuppression: true,
  microphoneVolume: 100,
  speakerVolume: 100,
);

Future<FakePhone> signedIn(
  WidgetTester tester, {
  Size size = const Size(1000, 1400),
  PhoneSnapshot Function(PhoneSnapshot) seed = withData,
}) async {
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  final phone = FakePhone()
    ..saved = const SignedInAccountFake()
    ..seed = seed;
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
                quality: const CallQuality(
                  jitterMs: 80,
                  lossPermille: 3,
                  packetsReceived: 120,
                  packetsPlayed: 100,
                  packetsDropped: 20,
                  unexpectedPt: 8,
                  expectedPt: 111,
                  lastSource: '203.0.113.5:40000',
                ),
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
      'Poor connection\n'
      '120 packets received, 100 played, 20 dropped\n'
      "Payload type 8 arrived; this call's codec is 111\n"
      'From 203.0.113.5:40000',
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

  testWidgets('a call\'s Audio sheet sets the volumes while it is up', (
    tester,
  ) async {
    final phone = await signedIn(tester);
    await tester.enterText(find.byKey(const Key('target')), '1002');
    await tapKey(tester, 'call');
    phone.set(
      phone.state.copyWith(
        calls: [phone.callOf(1).copyWith(state: CallState.connected)],
      ),
      'call',
      call: 1,
    );
    await tester.pumpAndSettle();

    await tapKey(tester, 'audio');
    final sheet = find.byKey(const Key('audioSheet'));
    expect(sheet, findsOneWidget);
    for (final key in ['microphone', 'speaker', 'microphoneVolume']) {
      expect(
        find.descendant(of: sheet, matching: find.byKey(Key(key))),
        findsOneWidget,
      );
    }
    final speaker = find.descendant(
      of: find.byKey(const Key('speakerVolume')),
      matching: find.byType(Slider),
    );
    await tester.drag(speaker, const Offset(200, 0));
    await tester.pumpAndSettle();
    final sent = phone.log.lastWhere((l) => l.startsWith('speaker volume '));
    final percent = int.parse(sent.split(' ').last);
    expect(percent, greaterThan(100), reason: sent);
    expect(phone.state.speakerVolume, percent);
    expect(find.text('$percent%'), findsOneWidget);
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
    await go(tester, 'Contacts');
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

  testWidgets('recents are removed one at a time, then cleared', (
    tester,
  ) async {
    final phone = await signedIn(tester);
    await go(tester, 'Recents');
    await tapKey(tester, 'recentMenu-r2');
    await tapKey(tester, 'removeRecent-r2');
    expect(phone.log, contains('hide recent r2'));
    expect(find.byKey(const Key('recent-r2')), findsNothing);
    expect(find.text('Ann Lee'), findsOneWidget, reason: 'the other stays');

    await tapKey(tester, 'clearRecents');
    expect(find.textContaining('keeps its call records'), findsOneWidget);
    await tapKey(tester, 'confirmClearRecents');
    expect(phone.log, contains('clear recents'));
    expect(find.text('No recent calls'), findsOneWidget);
  });

  testWidgets('contacts are searched, lit, made favourites and called', (
    tester,
  ) async {
    final phone = await signedIn(tester);
    expect(find.text('People'), findsNothing, reason: 'the tab is Contacts');
    await go(tester, 'Contacts');
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

  testWidgets('a call is recorded when FCP lets the user, and stopped', (
    tester,
  ) async {
    final phone = await signedIn(tester);
    await tester.enterText(find.byKey(const Key('target')), '1002');
    await tapKey(tester, 'call');
    phone.set(
      phone.state.copyWith(
        calls: [phone.callOf(1).copyWith(state: CallState.connected)],
      ),
      'call',
      call: 1,
    );
    await tester.pumpAndSettle();
    expect(
      find.byKey(const Key('record')),
      findsNothing,
      reason: 'FCP has not said the user may record',
    );

    // The icon: under it, the label may run to two lines.
    Future<void> tapRecord() async {
      await tester.tap(
        find.descendant(
          of: find.byKey(const Key('record')),
          matching: find.byType(IconButton),
        ),
      );
      await tester.pumpAndSettle();
    }

    phone.set(phone.state.copyWith(mayRecord: true), 'may_record');
    await tester.pumpAndSettle();
    await tapRecord();
    expect(phone.log, contains('record 1 true'));
    expect(find.byKey(const Key('recordingNow')), findsOneWidget);
    expect(find.text('Stop recording'), findsOneWidget);

    await tapRecord();
    expect(phone.log, contains('record 1 false'));
    expect(find.byKey(const Key('recordingNow')), findsNothing);

    // A refusal (FCP's 403 or 409) is said, and the call is not marked.
    phone.refuseWith = 'FCP refused: 409 the call is not anchored';
    await tapRecord();
    expect(find.textContaining('not anchored'), findsOneWidget);
    expect(find.byKey(const Key('recordingNow')), findsNothing);
  });

  testWidgets('a call\'s More menu makes a contact, which then names it', (
    tester,
  ) async {
    final phone = await signedIn(tester);
    await tester.enterText(find.byKey(const Key('target')), '0412345678');
    await tapKey(tester, 'call');
    phone.set(
      phone.state.copyWith(
        calls: [phone.callOf(1).copyWith(state: CallState.connected)],
      ),
      'call',
      call: 1,
    );
    await tester.pumpAndSettle();

    await tapKey(tester, 'more');
    await tapKey(tester, 'createContact');
    expect(find.text('New contact'), findsOneWidget);
    final number = tester.widget<TextFormField>(
      find.byKey(const Key('contactNumber-0')),
    );
    expect(number.controller!.text, '0412345678', reason: 'filled in');

    await tapKey(tester, 'saveContact');
    expect(find.text('Enter a name'), findsOneWidget, reason: 'needs a name');
    await tester.enterText(find.byKey(const Key('contactName')), 'Jo Smith');
    await tapKey(tester, 'contactLabel-0');
    await tester.tap(find.byKey(const Key('contactLabel-0-mobile')).last);
    await tester.pumpAndSettle();
    await tapKey(tester, 'saveContact');
    expect(phone.log, contains('contact c1 Jo Smith 0412345678'));
    expect(phone.state.contacts.single.numbers.single.label, 'mobile');
    expect(find.byKey(const Key('callName')), findsOneWidget);
    expect(
      tester.widget<Text>(find.byKey(const Key('callName'))).data,
      'Jo Smith',
    );

    // Now a contact: More has nothing more to offer.
    expect(
      tester
          .widget<IconButton>(
            find.descendant(
              of: find.byKey(const Key('more')),
              matching: find.byType(IconButton),
            ),
          )
          .onPressed,
      isNull,
    );
  });

  testWidgets('a recent call becomes a contact', (tester) async {
    final phone = await signedIn(tester);
    await go(tester, 'Recents');
    await tapKey(tester, 'recentMenu-r2');
    await tapKey(tester, 'createContact-r2');
    await tester.enterText(find.byKey(const Key('contactName')), 'Carol');
    await tapKey(tester, 'saveContact');
    expect(phone.log, contains('contact c1 Carol +15125550100'));
    expect(
      find.descendant(
        of: find.byKey(const Key('recent-r2')),
        matching: find.text('Carol'),
      ),
      findsOneWidget,
    );
    await tapKey(tester, 'recentMenu-r2');
    expect(find.byKey(const Key('createContact-r2')), findsNothing);
  });

  testWidgets('my contacts are made, changed, called and deleted', (
    tester,
  ) async {
    final phone = await signedIn(tester);
    await go(tester, 'Contacts');
    await tapKey(tester, 'newContact');
    await tester.enterText(find.byKey(const Key('contactName')), 'Zed Zulu');
    await tester.enterText(
      find.byKey(const Key('contactNumber-0')),
      '0400111222',
    );
    await tapKey(tester, 'addNumber');
    await tester.enterText(
      find.byKey(const Key('contactNumber-1')),
      '0299998888',
    );
    await tester.enterText(find.byKey(const Key('contactCompany')), 'Acme');
    await tapKey(tester, 'saveContact');
    expect(phone.log, contains('contact c1 Zed Zulu 0400111222'));
    expect(
      phone.state.contacts.single.onAccount,
      isTrue,
      reason: 'on the account when there is one',
    );
    expect(find.byKey(const Key('contactPlace-c1')), findsOneWidget);
    expect(find.byKey(const Key('contact-c1')), findsOneWidget);
    expect(phone.state.contacts.single.numbers.map((n) => n.label), [
      'mobile',
      'work',
    ], reason: 'the first a mobile, the second work, unless chosen');
    expect(find.text('Acme — 2 numbers'), findsOneWidget);

    await tester.enterText(find.byKey(const Key('peopleSearch')), 'acme');
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('contact-c1')), findsOneWidget);
    expect(find.byKey(const Key('person-ann')), findsNothing);
    await tester.enterText(find.byKey(const Key('peopleSearch')), '');
    await tester.pumpAndSettle();

    // Two numbers: opened, each its own row to dial.
    await tapKey(tester, 'contact-c1');
    expect(find.text('Work'), findsOneWidget);
    await tapKey(tester, 'dialContact-c1-1');
    expect(phone.log, contains('call 0299998888'));
    phone.set(phone.state.copyWith(calls: const []), 'call_ended', call: 1);
    await tester.pumpAndSettle();
    await go(tester, 'Contacts');

    await tapKey(tester, 'contactMenu-c1');
    expect(
      find.byKey(const Key('moveContact-c1')),
      findsNothing,
      reason: 'already on the account',
    );
    await tapKey(tester, 'editContact-c1');
    expect(find.text('Edit contact'), findsOneWidget);
    expect(
      find.byKey(const Key('contactPlace')),
      findsNothing,
      reason: 'chosen when made',
    );
    await tapKey(tester, 'removeNumber-0');
    await tapKey(tester, 'saveContact');
    expect(phone.state.contacts.single.dial, '0299998888');

    await tapKey(tester, 'contactMenu-c1');
    await tapKey(tester, 'deleteContact-c1');
    expect(
      find.text(
        'Delete Zed Zulu from your contacts? '
        'They go from every app you sign in to.',
      ),
      findsOneWidget,
    );
    await tapKey(tester, 'confirmDeleteContact');
    expect(phone.log, contains('delete contact c1'));
    expect(find.byKey(const Key('contact-c1')), findsNothing);
    expect(
      find.text('No contacts yet. Add one with the button above.'),
      findsOneWidget,
    );
  });

  testWidgets('a number\'s label is chosen from a list or typed', (
    tester,
  ) async {
    final phone = await signedIn(tester);
    await go(tester, 'Contacts');
    await tapKey(tester, 'newContact');
    await tester.enterText(find.byKey(const Key('contactName')), 'Kim Park');
    await tester.enterText(find.byKey(const Key('contactNumber-0')), '1004');
    await tapKey(tester, 'contactLabel-0');
    await tester.tap(find.byKey(const Key('contactLabel-0-custom')).last);
    await tester.pumpAndSettle();
    await tester.enterText(
      find.byKey(const Key('contactLabelCustom-0')),
      'Boat',
    );
    await tapKey(tester, 'saveContact');
    expect(phone.state.contacts.single.numbers.single.label, 'Boat');
    expect(find.textContaining('1004 (Boat)'), findsOneWidget);

    // Opened again: the user's own label is shown as typed, and the list
    // is a tap away.
    await tapKey(tester, 'contactMenu-c1');
    await tapKey(tester, 'editContact-c1');
    final custom = tester.widget<TextFormField>(
      find.byKey(const Key('contactLabelCustom-0')),
    );
    expect(custom.controller!.text, 'Boat');
    await tapKey(tester, 'contactLabelList-0');
    expect(find.byKey(const Key('contactLabel-0')), findsOneWidget);
    await tapKey(tester, 'saveContact');
    expect(phone.state.contacts.single.numbers.single.label, 'mobile');
  });

  testWidgets('contacts and the directory are two tabs under one search', (
    tester,
  ) async {
    await signedIn(tester);
    await go(tester, 'Contacts');
    // No contacts yet: the directory is shown.
    expect(find.byKey(const Key('person-ann')), findsOneWidget);
    await tapKey(tester, 'tab-contacts');
    expect(
      find.text('No contacts yet. Add one with the button above.'),
      findsOneWidget,
    );
    await tapKey(tester, 'tab-directory');
    await tester.enterText(find.byKey(const Key('peopleSearch')), 'zzz');
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('person-ann')), findsNothing);
    expect(find.text('No one to show'), findsOneWidget);
  });

  testWidgets('a contact kept on this device moves to the account', (
    tester,
  ) async {
    final phone = await signedIn(tester);
    await go(tester, 'Contacts');
    await tapKey(tester, 'newContact');
    await tapKey(tester, 'contactPlace-device');
    await tester.enterText(find.byKey(const Key('contactName')), 'Dee Dunn');
    await tester.enterText(find.byKey(const Key('contactNumber-0')), '1004');
    await tapKey(tester, 'saveContact');
    expect(phone.state.contacts.single.onAccount, isFalse);
    expect(
      tester
          .widget<Icon>(find.byKey(const Key('contactPlace-c1')))
          .semanticLabel,
      'On this device',
    );

    await tapKey(tester, 'contactMenu-c1');
    await tapKey(tester, 'moveContact-c1');
    expect(phone.state.contacts.single.onAccount, isTrue);
    expect(phone.state.contacts.single.dial, '1004');
    expect(
      tester
          .widget<Icon>(find.byKey(const Key('contactPlace-c1')))
          .semanticLabel,
      'On your account',
    );
  });

  testWidgets('without an account, contacts stay on the device unmarked', (
    tester,
  ) async {
    final phone = await signedIn(
      tester,
      seed: (s) => s.copyWith(people: const [ann]),
    );
    await go(tester, 'Contacts');
    await tapKey(tester, 'newContact');
    expect(find.byKey(const Key('contactPlace')), findsNothing);
    await tester.enterText(find.byKey(const Key('contactName')), 'Dee Dunn');
    await tapKey(tester, 'saveContact');
    expect(phone.state.contacts.single.onAccount, isFalse);
    expect(find.byKey(const Key('contactPlace-c1')), findsNothing);
    await tapKey(tester, 'contactMenu-c1');
    expect(find.byKey(const Key('moveContact-c1')), findsNothing);
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

  testWidgets(
    'settings change dnd, call waiting, forwards, audio and ringing',
    (tester) async {
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

      final microphone = find.descendant(
        of: find.byKey(const Key('microphoneVolume')),
        matching: find.byType(Slider),
      );
      await tester.ensureVisible(microphone);
      await tester.drag(microphone, const Offset(-200, 0));
      await tester.pumpAndSettle();
      final sent = phone.log.lastWhere(
        (l) => l.startsWith('microphone volume '),
      );
      expect(int.parse(sent.split(' ').last), lessThan(100), reason: sent);

      await tapKey(tester, 'ringDevice');
      await tester.tap(find.text('Speakers').last);
      await tester.pumpAndSettle();
      expect(phone.log, contains('ring on spk'));

      await tapKey(tester, 'ringtone');
      await tester.tap(find.text('Acme').last);
      await tester.pumpAndSettle();
      expect(phone.log, contains('ringtone brand:Acme'));

      await tapKey(tester, 'echoCancellation');
      expect(phone.log.last, 'echo false');
      expect(phone.state.echoCancellation, isFalse);
      await tapKey(tester, 'noiseSuppression');
      expect(phone.log.last, 'noise false');

      await tapKey(tester, 'signOut');
      expect(phone.log, contains('signOut'));
      expect(find.byKey(const Key('continue')), findsOneWidget);
    },
  );

  testWidgets('settings show where the log file is, to copy', (tester) async {
    final phone = await signedIn(tester);
    phone.logPath = '/support/anvil.log';
    await go(tester, 'Settings');
    await tester.ensureVisible(find.byKey(const Key('logFile')));
    await tester.pumpAndSettle();
    expect(find.text('/support/anvil.log'), findsOneWidget);
    String? copied;
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      SystemChannels.platform,
      (call) async {
        if (call.method == 'Clipboard.setData') {
          copied = (call.arguments as Map)['text'] as String?;
        }
        return null;
      },
    );
    await tapKey(tester, 'logFileCopy');
    expect(copied, '/support/anvil.log');
    expect(find.text('Path copied'), findsOneWidget);
  });

  testWidgets('a narrow window navigates from the bottom', (tester) async {
    await signedIn(tester, size: const Size(400, 800));
    expect(find.byType(NavigationBar), findsOneWidget);
    expect(find.byType(NavigationRail), findsNothing);
    await go(tester, 'Voicemail');
    expect(find.text('Call me back'), findsOneWidget);
  });
}
