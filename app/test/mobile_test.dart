// On a phone the app asks for the microphone and sets the audio up before
// the phone starts, and says so when the microphone is refused; and the
// system's call screen follows the phone's calls, and what the user does
// there reaches the phone; and the camera stops while the app is out of
// sight, and is asked for before a call first uses it; and the phone hears
// when it is on cellular, where the user may keep video to Wi-Fi; and a call
// showing the other side's picture may go to picture-in-picture.

import 'package:anvil/src/app.dart';
import 'package:anvil/src/mobile/platform.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'fake_mobile.dart';
import 'fake_phone.dart';
import 'screens_test.dart' show go, tapKey;
import 'video_test.dart' show withVideo;

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

  testWidgets("on Android the system's call screen follows the phone itself", (
    tester,
  ) async {
    // Native code reports the calls there and carries out what the user
    // does (FCP's docs/PUSH.md §2.5): the app does neither, or each call
    // would show twice and be answered twice.
    final phone = FakePhone()..saved = const SignedInAccountFake();
    final mobile = FakeMobile(systemFollowsPhone: true);
    await tester.pumpWidget(AnvilApp(phone: phone, mobile: mobile));
    await tester.pumpAndSettle();
    mobile.log.clear();
    phone.ring('1002', name: 'Ann Lee');
    await tester.pumpAndSettle();
    mobile.act(const SystemCallEvent(SystemCallAction.answer, 1));
    await tester.pumpAndSettle();
    expect(
      mobile.log.where(
        (l) => l.startsWith('incoming') || l.startsWith('connected'),
      ),
      isEmpty,
    );
    expect(phone.log, isNot(contains('answer 1')));
  });

  testWidgets('the camera stops while the app is out of sight', (tester) async {
    final (phone, _) = await signedIn(tester);
    expect(phone.log, contains('background off'), reason: 'seen at start');
    phone.log.clear();
    for (final state in [
      AppLifecycleState.inactive,
      AppLifecycleState.hidden,
      AppLifecycleState.paused,
    ]) {
      tester.binding.handleAppLifecycleStateChanged(state);
    }
    await tester.pump();
    expect(phone.log, ['background on']);
    for (final state in [
      AppLifecycleState.hidden,
      AppLifecycleState.inactive,
      AppLifecycleState.resumed,
    ]) {
      tester.binding.handleAppLifecycleStateChanged(state);
    }
    await tester.pump();
    expect(phone.log, ['background on', 'background off']);
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
      const SystemCallEvent(SystemCallAction.failed, 2, reason: 'reset'),
    );
    await tester.pumpAndSettle();
    expect(phone.log, contains('decline 1'));
    expect(phone.log, isNot(contains('decline 2')));
    expect(phone.callOf(2).isRingingIn, isTrue);
  });

  testWidgets('the screen goes off at the ear: a call up on the earpiece', (
    tester,
  ) async {
    final (phone, mobile) = await signedIn(tester);
    expect(mobile.proximity.lastOrNull, isNot(true));
    phone.ring('1002');
    await tester.pumpAndSettle();
    // Ringing: the phone is in the hand, not at the ear.
    expect(mobile.proximity.lastOrNull, isNot(true));
    await phone.answer(1);
    await tester.pumpAndSettle();
    expect(mobile.proximity.last, isTrue);

    mobile.route(
      const AudioRoutes(
        current: AudioRoute.speaker,
        available: [AudioRoute.earpiece, AudioRoute.speaker],
      ),
    );
    await tester.pumpAndSettle();
    expect(mobile.proximity.last, isFalse, reason: 'on the speaker');

    mobile.route(
      const AudioRoutes(
        current: AudioRoute.earpiece,
        available: [AudioRoute.earpiece, AudioRoute.speaker],
      ),
    );
    await tester.pumpAndSettle();
    expect(mobile.proximity.last, isTrue);

    phone.hungUp(1);
    await tester.pumpAndSettle();
    expect(mobile.proximity.last, isFalse);
  });

  testWidgets('with the earpiece and the speaker, the call has a speaker '
      'switch', (tester) async {
    final (phone, mobile) = await signedIn(tester);
    mobile.routes = const AudioRoutes(
      current: AudioRoute.earpiece,
      available: [AudioRoute.earpiece, AudioRoute.speaker],
    );
    await phone.call('1003');
    phone.answerOutgoing(1);
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('audioRoute')), findsNothing);

    await tester.tap(find.byKey(const Key('speakerRoute')));
    await tester.pumpAndSettle();
    expect(mobile.log.last, 'route speaker');
    await tester.tap(find.byKey(const Key('speakerRoute')));
    await tester.pumpAndSettle();
    expect(mobile.log.last, 'route earpiece');
  });

  testWidgets('with Bluetooth too, the call offers each way by name', (
    tester,
  ) async {
    final (phone, mobile) = await signedIn(tester);
    mobile.routes = const AudioRoutes(
      current: AudioRoute.bluetooth,
      available: [
        AudioRoute.earpiece,
        AudioRoute.speaker,
        AudioRoute.bluetooth,
      ],
      bluetoothName: 'Car',
    );
    await phone.call('1003');
    phone.answerOutgoing(1);
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('speakerRoute')), findsNothing);
    expect(
      find.descendant(
        of: find.byKey(const Key('audioRoute')),
        matching: find.text('Car'),
      ),
      findsOneWidget,
    );

    await tester.tap(find.byKey(const Key('audioRoute')));
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('route-earpiece')), findsOneWidget);
    expect(find.byKey(const Key('route-wired')), findsNothing);
    await tester.tap(find.byKey(const Key('route-speaker')));
    await tester.pumpAndSettle();
    expect(mobile.log.last, 'route speaker');
    expect(find.text('Speaker'), findsWidgets);
  });

  testWidgets('a phone keeps no microphone and speaker list in settings', (
    tester,
  ) async {
    await signedIn(tester);
    await tester.tap(find.text('Settings').last);
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('signOut')), findsOneWidget);
    expect(find.byKey(const Key('microphone')), findsNothing);
    expect(find.byKey(const Key('speaker')), findsNothing);
  });

  testWidgets('the camera is asked for before a call uses it, and a refusal '
      'is said while the call goes on', (tester) async {
    tester.view.physicalSize = const Size(1000, 1400);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    final phone = FakePhone()
      ..saved = const SignedInAccountFake()
      ..seed = withVideo;
    final mobile = FakeMobile(allowCamera: false);
    await tester.pumpWidget(AnvilApp(phone: phone, mobile: mobile));
    await tester.pumpAndSettle();
    mobile.log.clear();

    phone.ring('1002', video: true);
    await tester.pumpAndSettle();
    await tapKey(tester, 'answerAudio');
    expect(mobile.log, isNot(contains('requestCamera')));
    await tapKey(tester, 'hangup');

    phone.ring('1003', video: true);
    await tester.pumpAndSettle();
    await tapKey(tester, 'answerVideo');
    expect(mobile.log, contains('requestCamera'));
    expect(phone.log, contains('answer 2 video=true'));
    expect(find.textContaining('may not use the camera'), findsOneWidget);
  });

  testWidgets('the phone hears its network, and video may be kept to Wi-Fi', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(1000, 1400);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    final phone = FakePhone()
      ..saved = const SignedInAccountFake()
      ..seed = withVideo;
    final mobile = FakeMobile()..onCellular = true;
    await tester.pumpWidget(AnvilApp(phone: phone, mobile: mobile));
    await tester.pumpAndSettle();
    expect(phone.log, contains('network cellular'), reason: 'at start');

    mobile.network(cellular: false);
    await tester.pump();
    expect(phone.log.last, 'network wifi');

    await go(tester, 'Settings');
    expect(find.text('Send video only on Wi-Fi'), findsOneWidget);
    await tapKey(tester, 'wifiOnly');
    expect(phone.log.last, 'wifi-only on');
    mobile.network(cellular: true);
    await tester.pumpAndSettle();
    expect(find.textContaining('back on Wi-Fi'), findsOneWidget);
  });

  testWidgets('a desktop has no Wi-Fi only setting', (tester) async {
    tester.view.physicalSize = const Size(1000, 1400);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    final phone = FakePhone()
      ..saved = const SignedInAccountFake()
      ..seed = withVideo;
    await tester.pumpWidget(AnvilApp(phone: phone));
    await tester.pumpAndSettle();
    await go(tester, 'Settings');
    expect(find.byKey(const Key('wifiOnly')), findsNothing);
  });

  testWidgets('a call showing their picture may go to picture-in-picture', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(1000, 1400);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    final phone = FakePhone()
      ..saved = const SignedInAccountFake()
      ..seed = withVideo;
    final mobile = FakeMobile();
    await tester.pumpWidget(AnvilApp(phone: phone, mobile: mobile));
    await tester.pumpAndSettle();
    phone.ring('1002', video: true);
    await tester.pumpAndSettle();
    await tapKey(tester, 'answerVideo');
    expect(mobile.pip, isNull, reason: 'no picture of theirs yet');

    phone.tiles.sizes[7] = (width: 640, height: 360, frozen: false);
    phone.streamIn(1, 7);
    await tester.pumpAndSettle();
    final pip = mobile.pip!;
    expect((pip.width, pip.height), (640, 360));

    // Android shrinks the window: only their picture shows.
    mobile.pictureInPicture(true);
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('pictureInPicture')), findsOneWidget);
    mobile.pictureInPicture(false);
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('pictureInPicture')), findsNothing);

    // Their camera off: nothing for picture-in-picture to show.
    phone.streamOut(1, 7);
    await tester.pumpAndSettle();
    expect(mobile.pip, isNull);
    expect(mobile.log.last, 'pip off');
    await tapKey(tester, 'hangup');
  });
}
