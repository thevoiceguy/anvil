// FCP's rooms against a fake phone (FCP's `docs/SOFTPHONE_VIDEO.md` §2.6):
// the meetings sheet (joining by number or link, a link to send), the room
// a call is in (who speaks, who shares, the people), a host's controls,
// and a shared screen shown large.

import 'package:anvil/src/phone/phone_api.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'screens_test.dart' show signedIn, tapKey, withData;
import 'video_test.dart' show withVideo;

const meetings = [
  Meeting(id: 'm1', roomNumber: '3000', name: 'Standup'),
  Meeting(id: 'm2', roomNumber: '3001', name: 'Review'),
];

const members = [
  RoomMember(id: 'p1', name: 'Ann', host: true, camera: 'on'),
  RoomMember(
    id: 'p2',
    name: 'Bob',
    audio: 'muted',
    camera: 'off',
    speaking: true,
  ),
];

PhoneSnapshot withMeetings(PhoneSnapshot s) =>
    withVideo(withData(s)).copyWith(meetings: meetings);

/// Put call `id` in its room as connected, with `room`.
void inRoom(dynamic phone, int id, RoomInfo room, {int? content}) {
  final call = (phone.callOf(id) as PhoneCall).copyWith(
    state: CallState.connected,
    room: room,
    contentStream: content,
  );
  phone.set(
    (phone.state as PhoneSnapshot).copyWith(calls: [call]),
    'call',
    call: id,
  );
}

void main() {
  testWidgets('a room is joined by its number from the meetings sheet', (
    tester,
  ) async {
    final phone = await signedIn(tester, seed: withMeetings);
    await tapKey(tester, 'meetings');
    expect(find.byKey(const Key('meetingsSheet')), findsOneWidget);
    expect(find.text('Standup'), findsOneWidget);
    expect(find.text('Review'), findsOneWidget);
    await tester.enterText(find.byKey(const Key('joinTarget')), '4000');
    await tester.pump();
    await tapKey(tester, 'joinVideo');
    expect(phone.log, contains('video join 4000'));
    expect(find.byKey(const Key('meetingsSheet')), findsNothing);
    expect(phone.callOf(1).room?.number, '4000');
    expect(phone.callOf(1).room?.host, isFalse);
  });

  testWidgets('one of my meetings is joined, and its links copied', (
    tester,
  ) async {
    final copied = <String>[];
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      SystemChannels.platform,
      (call) async {
        if (call.method == 'Clipboard.setData') {
          copied.add((call.arguments as Map)['text'] as String);
        }
        return null;
      },
    );
    final phone = await signedIn(tester, seed: withMeetings);
    await tapKey(tester, 'meetings');
    phone.link = 'fcp://join/guest-token';
    await tapKey(tester, 'meetingLinks-m1');
    await tapKey(tester, 'meetingLink-m1');
    expect(phone.log, contains('link m1'));
    expect(copied, ['fcp://join/guest-token']);
    expect(find.text('Link copied'), findsOneWidget);

    phone.link = 'fcp://join/host-token';
    await tapKey(tester, 'meetingLinks-m1');
    await tapKey(tester, 'meetingHostLink-m1');
    expect(phone.log, contains('link m1 host'));
    expect(copied.last, 'fcp://join/host-token');

    await tapKey(tester, 'meetingJoin-m2');
    expect(phone.log, contains('video join 3001'));
    expect(phone.callOf(1).room?.host, isTrue, reason: 'the owner hosts');
  });

  testWidgets('without meetings the sheet says so and still joins links', (
    tester,
  ) async {
    final phone = await signedIn(tester);
    await tapKey(tester, 'meetings');
    expect(find.byKey(const Key('noMeetings')), findsOneWidget);
    expect(find.byKey(const Key('joinVideo')), findsNothing);
    await tester.enterText(
      find.byKey(const Key('joinTarget')),
      'fcp://join/abc',
    );
    await tester.pump();
    await tapKey(tester, 'join');
    expect(phone.log, contains('join fcp://join/abc'));
  });

  testWidgets('the room shows who speaks and who is in it', (tester) async {
    final phone = await signedIn(tester, seed: withMeetings);
    await tapKey(tester, 'meetings');
    await tester.enterText(find.byKey(const Key('joinTarget')), '4000');
    await tester.pump();
    await tapKey(tester, 'join');
    inRoom(
      phone,
      1,
      const RoomInfo(number: '4000', members: members, speaker: 'p2'),
    );
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('roomPanel')), findsOneWidget);
    expect(find.text('Bob is speaking'), findsOneWidget);
    expect(find.text('People (2)'), findsOneWidget);
    expect(find.byKey(const Key('member-p1')), findsOneWidget);
    expect(find.byKey(const Key('speaking-p2')), findsOneWidget);
    expect(find.text('Muted · Camera off'), findsOneWidget);
    // Not a host: no controls.
    expect(find.byKey(const Key('roomHostBar')), findsNothing);
    expect(find.byKey(const Key('memberMenu-p2')), findsNothing);
  });

  testWidgets("a host's controls reach the room", (tester) async {
    final phone = await signedIn(tester, seed: withMeetings);
    await tapKey(tester, 'meetings');
    await tapKey(tester, 'meetingJoin-m1');
    inRoom(
      phone,
      1,
      const RoomInfo(
        number: '3000',
        host: true,
        members: members,
        speaker: 'p2',
        presenter: 'Ann',
      ),
    );
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('roomHostBar')), findsOneWidget);
    expect(find.text('Ann is sharing their screen'), findsOneWidget);

    await tapKey(tester, 'roomGrid');
    await tapKey(tester, 'roomSpeakerLayout');
    await tapKey(tester, 'roomUnpin');
    await tapKey(tester, 'roomStopSpotlight');
    await tapKey(tester, 'roomStopShare');
    await tapKey(tester, 'memberMenu-p2');
    await tapKey(tester, 'memberMute-p2');
    await tapKey(tester, 'memberMenu-p1');
    await tapKey(tester, 'memberCamera-p1');
    await tapKey(tester, 'memberMenu-p2');
    await tapKey(tester, 'memberSpotlight-p2');
    await tapKey(tester, 'memberMenu-p2');
    await tapKey(tester, 'memberRemove-p2');
    expect(phone.log.where((l) => l.startsWith('room ')), [
      'room 1 layout on',
      'room 1 layout off',
      'room 1 pin',
      'room 1 spotlight',
      'room 1 stopShare',
      'room 1 mute p2 off',
      'room 1 camera p1 off',
      'room 1 spotlight p2',
      'room 1 remove p2',
    ], reason: 'p2 is muted already, so the menu unmutes');
  });

  testWidgets('a shared screen fills the stage, the cameras beside it', (
    tester,
  ) async {
    final phone = await signedIn(tester, seed: withMeetings);
    await tapKey(tester, 'meetings');
    await tapKey(tester, 'meetingJoin-m1');
    inRoom(
      phone,
      1,
      const RoomInfo(number: '3000', host: true, members: members),
      content: 9,
    );
    phone.streamIn(1, 7);
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('sharedStage')), findsOneWidget);
    expect(find.byKey(const ValueKey('content-9')), findsOneWidget);
    expect(find.byKey(const ValueKey('tile-7')), findsOneWidget);
    expect(phone.tiles.open, containsAll([9, 7]));
  });

  testWidgets('our share stopped by the room is said', (tester) async {
    final phone = await signedIn(tester, seed: withMeetings);
    await tapKey(tester, 'meetings');
    await tapKey(tester, 'meetingJoin-m1');
    inRoom(
      phone,
      1,
      const RoomInfo(number: '3000', floorRefused: 'another share is on'),
    );
    await tester.pumpAndSettle();
    expect(
      find.text('Your share stopped: another share is on'),
      findsOneWidget,
    );
  });

  testWidgets('a screen is shared and stopped from the room, and chosen', (
    tester,
  ) async {
    const screens = [
      CameraDevice(id: 'DP-1', name: 'DP-1 (1920×1080)'),
      CameraDevice(id: 'HDMI-1', name: 'HDMI-1 (1280×720)'),
    ];
    final phone = await signedIn(
      tester,
      seed: (s) {
        final m = withMeetings(s);
        return m.copyWith(
          video: VideoSettings(
            available: true,
            cameras: m.video.cameras,
            screens: screens,
          ),
        );
      },
    );
    await tapKey(tester, 'meetings');
    await tapKey(tester, 'meetingJoin-m1');
    inRoom(phone, 1, const RoomInfo(number: '3000'));
    await tester.pumpAndSettle();
    expect(find.text('Share your screen'), findsOneWidget);
    expect(find.byKey(const Key('roomSharing')), findsNothing);

    await tapKey(tester, 'roomShare');
    expect(phone.log, contains('share 1 on'));
    expect(find.text('Stop sharing'), findsOneWidget);
    expect(find.byKey(const Key('roomSharing')), findsOneWidget);

    // Two screens: one is chosen.
    await tapKey(tester, 'roomScreen');
    await tester.tap(find.text('HDMI-1 (1280×720)').last);
    await tester.pumpAndSettle();
    expect(phone.log, contains('screen HDMI-1'));

    await tapKey(tester, 'roomShare');
    expect(phone.log, contains('share 1 off'));
    expect(find.text('Share your screen'), findsOneWidget);

    // A host stopped it: said in words.
    inRoom(phone, 1, const RoomInfo(number: '3000', floorRefused: 'host'));
    await tester.pumpAndSettle();
    expect(find.text('Your share stopped: a host stopped it'), findsOneWidget);
  });

  testWidgets('a device without screens offers no share', (tester) async {
    final phone = await signedIn(tester, seed: withMeetings);
    await tapKey(tester, 'meetings');
    await tapKey(tester, 'meetingJoin-m1');
    inRoom(phone, 1, const RoomInfo(number: '3000'));
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('roomShare')), findsNothing);
  });
}
