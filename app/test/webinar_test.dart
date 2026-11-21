// A room's audience against a fake phone (FCP's `docs/WEBINARS.md` A7):
// the lobby, an attendee's hand and questions, the stage, chat in every
// room, and a host's webinar controls.

import 'package:anvil/src/phone/phone_api.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'room_test.dart' show inRoom, withMeetings;
import 'screens_test.dart' show signedIn, tapKey;

/// Join room 4000 and put the call in `room`.
Future<dynamic> joined(WidgetTester tester, RoomInfo room) async {
  final phone = await signedIn(tester, seed: withMeetings);
  await tapKey(tester, 'meetings');
  await tester.enterText(find.byKey(const Key('joinTarget')), '4000');
  await tester.pump();
  await tapKey(tester, 'join');
  inRoom(phone, 1, room);
  await tester.pumpAndSettle();
  return phone;
}

void main() {
  testWidgets('an attendee waits in the lobby, then raises a hand and asks', (
    tester,
  ) async {
    final phone = await joined(
      tester,
      const RoomInfo(
        number: '4000',
        role: 'attendee',
        webinar: 'practice',
        lobby: true,
      ),
    );
    expect(find.text('Waiting for the webinar to start'), findsOneWidget);
    expect(find.byKey(const Key('webinarHand')), findsNothing);
    expect(find.byKey(const Key('webinarGoLive')), findsNothing);

    inRoom(
      phone,
      1,
      const RoomInfo(
        number: '4000',
        role: 'attendee',
        webinar: 'live',
        audience: 250,
        questions: [
          RoomQuestion(
            id: 'q-1',
            text: 'When?',
            asker: 'Ben',
            upvotes: 2,
            published: true,
          ),
        ],
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('250 watching'), findsOneWidget);
    expect(find.byKey(const Key('webinarListening')), findsOneWidget);
    await tapKey(tester, 'webinarHand');
    expect(phone.log, contains('audience 1 hand on'));

    await tapKey(tester, 'roomQuestions');
    await tapKey(tester, 'roomUpvote-q-1');
    expect(phone.log, contains('audience 1 upvote q-1 on'));
    expect(
      find.byKey(const Key('roomModerate-q-1')),
      findsNothing,
      reason: 'not the stage',
    );
    await tester.enterText(
      find.byKey(const Key('roomAskField')),
      'Is it open source?',
    );
    await tapKey(tester, 'roomAskAnonymous');
    await tapKey(tester, 'roomAskSend');
    expect(phone.log, contains('audience 1 ask "Is it open source?" on'));
  });

  testWidgets('brought to the stage, an attendee is told the room hears them', (
    tester,
  ) async {
    await joined(
      tester,
      const RoomInfo(
        number: '4000',
        role: 'attendee',
        webinar: 'live',
        onStage: true,
      ),
    );
    expect(find.byKey(const Key('webinarOnStage')), findsOneWidget);
    expect(find.byKey(const Key('webinarListening')), findsNothing);
    expect(find.byKey(const Key('webinarHand')), findsNothing);
  });

  testWidgets('chat in a meeting: posted to the room or the hosts, and muted', (
    tester,
  ) async {
    final phone = await joined(
      tester,
      const RoomInfo(
        number: '4000',
        chat: [
          RoomChatMessage(
            id: 'm-1',
            text: 'Hello all',
            sender: 'Ann',
            senderId: 'p1',
          ),
        ],
      ),
    );
    expect(
      find.byKey(const Key('webinarBanner')),
      findsNothing,
      reason: 'a meeting',
    );
    expect(find.byKey(const Key('roomQuestions')), findsNothing);
    await tapKey(tester, 'roomChat');
    expect(find.text('Hello all'), findsOneWidget);
    await tester.enterText(find.byKey(const Key('roomChatField')), 'Hi Ann');
    await tapKey(tester, 'roomChatSend');
    expect(phone.log, contains('audience 1 chat "Hi Ann"'));
    await tapKey(tester, 'roomChatToHosts');
    await tester.enterText(find.byKey(const Key('roomChatField')), 'A word');
    await tapKey(tester, 'roomChatSend');
    expect(phone.log, contains('audience 1 chat "A word" on'));

    inRoom(phone, 1, const RoomInfo(number: '4000', chatMuted: true));
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('roomChatMuted')), findsOneWidget);
    expect(find.byKey(const Key('roomChatField')), findsNothing);
  });

  testWidgets(
    "a host takes it live, brings a hand to the stage, moderates and ends it",
    (tester) async {
      final phone = await joined(
        tester,
        const RoomInfo(
          number: '4000',
          host: true,
          role: 'host',
          webinar: 'practice',
          members: [
            RoomMember(
              id: 'p-dan',
              name: 'Dan',
              attendee: true,
              hand: '2026-11-21T10:00:00.000Z',
            ),
          ],
          questions: [RoomQuestion(id: 'q-1', text: 'When?', asker: 'Dan')],
          chat: [
            RoomChatMessage(
              id: 'm-1',
              text: 'spam',
              sender: 'Dan',
              senderId: 'p-dan',
            ),
          ],
        ),
      );
      await tapKey(tester, 'webinarGoLive');
      expect(phone.log, contains('room 1 goLive'));
      expect(find.text('Raised hands (1)'), findsOneWidget);
      await tapKey(tester, 'webinarToStage-p-dan');
      expect(phone.log, contains('room 1 stage p-dan'));
      await tapKey(tester, 'webinarLowerAll');
      expect(phone.log, contains('room 1 lowerAllHands'));

      await tapKey(tester, 'roomQuestions');
      await tapKey(tester, 'roomModerate-q-1');
      await tester.tap(find.text('Show to everyone'));
      await tester.pumpAndSettle();
      expect(phone.log, contains('audience 1 publish q-1'));

      await tapKey(tester, 'roomChat');
      await tapKey(tester, 'roomMessageMenu-m-1');
      await tester.tap(find.text('Mute their chat'));
      await tester.pumpAndSettle();
      expect(phone.log, contains('room 1 muteChat p-dan'));

      await tapKey(tester, 'webinarEnd');
      await tapKey(tester, 'webinarEndConfirm');
      expect(phone.log, contains('room 1 endWebinar'));
    },
  );
}
