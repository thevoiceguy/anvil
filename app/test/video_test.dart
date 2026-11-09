// Video calls against a fake phone (FCP's `docs/SOFTPHONE_VIDEO.md` §2.5):
// the keypad's video call, a video call answered either way, the stage with
// its tiles and self-view, the camera on and off, the camera picker, the
// far end asking, and the settings.

import 'package:anvil/src/phone/phone_api.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'screens_test.dart' show signedIn, tapKey, go;

const cameras = [
  CameraDevice(id: '/dev/video0', name: 'Built-in'),
  CameraDevice(id: '/dev/video2', name: 'USB Camera'),
];

PhoneSnapshot withVideo(PhoneSnapshot s) => s.copyWith(
  video: const VideoSettings(
    available: true,
    cameras: cameras,
    h264Allowed: true,
    h264Ready: true,
    h264Notice: 'OpenH264 Video Codec provided by Cisco Systems, Inc.',
  ),
);

void main() {
  testWidgets('without video the keypad places audio calls only', (
    tester,
  ) async {
    await signedIn(tester);
    expect(find.byKey(const Key('videoCall')), findsNothing);
  });

  testWidgets('a video call from the keypad shows the self-view', (
    tester,
  ) async {
    final phone = await signedIn(tester, seed: withVideo);
    await tester.enterText(find.byKey(const Key('target')), '1002');
    await tapKey(tester, 'videoCall');
    expect(phone.log, contains('video call 1002'));
    expect(phone.callOf(1).camera, isTrue);
    expect(find.byKey(const Key('videoStage')), findsOneWidget);
    expect(find.byKey(const Key('selfView')), findsOneWidget);
    expect(phone.tiles.open, [null], reason: 'the self-view on a texture');
    // Their camera is off until a stream comes in.
    expect(find.byKey(const Key('videoTheirsOff')), findsOneWidget);

    phone.set(
      phone.state.copyWith(
        calls: [phone.callOf(1).copyWith(state: CallState.connected)],
      ),
      'call',
      call: 1,
    );
    phone.streamIn(1, 7);
    phone.tiles.sizes[7] = (width: 640, height: 360, frozen: false);
    await tester.pumpAndSettle();
    await tester.pump(const Duration(milliseconds: 600));
    expect(phone.tiles.open, containsAll([null, 7]));
    expect(find.byKey(const Key('videoTheirsOff')), findsNothing);
    expect(find.byType(Texture), findsOneWidget, reason: 'theirs, sized');

    // Frozen until a keyframe: said so.
    phone.tiles.sizes[7] = (width: 640, height: 360, frozen: true);
    await tester.pump(const Duration(milliseconds: 600));
    expect(find.byKey(const Key('videoFrozen')), findsOneWidget);

    // The far end's camera off: its tile closes.
    phone.streamOut(1, 7);
    await tester.pumpAndSettle();
    expect(phone.tiles.closed, contains(7));

    // The self-view is dragged and stays on the stage.
    final stage = tester.getRect(find.byKey(const Key('videoStage')));
    await tester.drag(
      find.byKey(const Key('selfView')),
      const Offset(-5000, -5000),
    );
    await tester.pumpAndSettle();
    final self = tester.getRect(find.byKey(const Key('selfView')));
    expect(stage.contains(self.topLeft), isTrue);
    expect(self.left - stage.left, closeTo(12, 1));

    // Our camera off: the self-view goes, and with no pictures, the stage.
    await tapKey(tester, 'video');
    expect(phone.log, contains('video 1 off'));
    expect(phone.tiles.closed, contains(null));
    expect(find.byKey(const Key('videoStage')), findsNothing);

    await tapKey(tester, 'hangup');
    expect(phone.tiles.open, isEmpty);
  });

  testWidgets('a video call rings in and is answered with audio or video', (
    tester,
  ) async {
    final phone = await signedIn(tester, seed: withVideo);
    phone.ring('1002', video: true);
    await tester.pumpAndSettle();
    expect(find.text('Incoming video call'), findsOneWidget);
    expect(find.byKey(const Key('answer')), findsNothing);
    await tapKey(tester, 'answerAudio');
    expect(phone.log, contains('answer 1 video=false'));
    expect(find.byKey(const Key('videoStage')), findsNothing);
    await tapKey(tester, 'hangup');

    phone.ring('1003', video: true);
    await tester.pumpAndSettle();
    await tapKey(tester, 'answerVideo');
    expect(phone.log, contains('answer 2 video=true'));
    expect(find.byKey(const Key('videoStage')), findsOneWidget);
  });

  testWidgets('a video call to a phone without video is answered as audio', (
    tester,
  ) async {
    final phone = await signedIn(tester);
    phone.ring('1002', video: true);
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('answerVideo')), findsNothing);
    await tapKey(tester, 'answer');
    expect(phone.log, contains('answer 1'));
  });

  testWidgets('the far end asks for video, and the camera is chosen', (
    tester,
  ) async {
    final phone = await signedIn(tester, seed: withVideo);
    await tester.enterText(find.byKey(const Key('target')), '1002');
    await tapKey(tester, 'call');
    phone.set(
      phone.state.copyWith(
        calls: [phone.callOf(1).copyWith(state: CallState.connected)],
      ),
      'call',
      call: 1,
    );
    phone.streamIn(1, 3);
    phone.videoAsked(1);
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('videoAsked')), findsOneWidget);
    expect(find.text('1002 turned on video. Start yours?'), findsOneWidget);
    await tapKey(tester, 'startVideo');
    expect(phone.log, contains('video 1 on'));
    expect(find.byKey(const Key('videoAsked')), findsNothing);
    expect(find.byKey(const Key('selfView')), findsOneWidget);

    await tapKey(tester, 'camera');
    await tapKey(tester, 'camera-/dev/video2');
    expect(phone.log, contains('camera /dev/video2'));
  });

  testWidgets('settings choose the camera and allow H.264, with its notice', (
    tester,
  ) async {
    final phone = await signedIn(tester, seed: withVideo);
    await go(tester, 'Settings');
    await tester.ensureVisible(find.byKey(const Key('h264')));
    expect(
      find.textContaining('OpenH264 Video Codec provided by Cisco Systems'),
      findsOneWidget,
    );
    await tapKey(tester, 'h264');
    expect(phone.log, contains('h264 off'));
    expect(find.textContaining('video calls use VP8'), findsOneWidget);

    await tapKey(tester, 'camera');
    await tester.tap(find.text('USB Camera').last);
    await tester.pumpAndSettle();
    expect(phone.log, contains('camera /dev/video2'));
  });

  testWidgets('settings show no video section without video', (tester) async {
    await signedIn(tester);
    await go(tester, 'Settings');
    expect(find.text('Video'), findsNothing);
  });
}
