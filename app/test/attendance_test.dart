// A host's attendance report of their own meeting (FCP's
// `docs/WEBINARS.md` W8.3), from the meetings sheet, against a fake phone.

import 'package:anvil/src/phone/phone_api.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'room_test.dart' show withMeetings;
import 'screens_test.dart' show signedIn, tapKey;

void main() {
  testWidgets('a host reads who came to their meeting and who watched', (
    tester,
  ) async {
    final phone = await signedIn(tester, seed: withMeetings);
    final started = DateTime(2026, 11, 21, 10);
    phone.attendance = AttendanceReport(
      sitting: MeetingSitting(id: 'sit-1', kind: 'webinar', startedAt: started),
      attendees: 2,
      peak: 2,
      legs: [
        AttendanceEntry(
          name: 'Hostess',
          role: 'host',
          joinedAt: started,
          durationSecs: 3600,
        ),
        AttendanceEntry(
          name: 'Ann',
          role: 'attendee',
          joinedAt: started,
          durationSecs: 1800,
        ),
        AttendanceEntry(name: 'Wait', joinedAt: started, lobby: true),
      ],
      registrants: const [
        AttendanceRegistrant(name: 'Ann', attended: true, attendedSecs: 1800),
        AttendanceRegistrant(name: 'Ben', watched: true, watchedSecs: 300),
      ],
      viewers: const [
        AttendanceRegistrant(name: 'Ben', watched: true, watchedSecs: 300),
      ],
    );
    await tapKey(tester, 'meetings');
    await tapKey(tester, 'meetingLinks-m1');
    await tapKey(tester, 'meetingAttendance-m1');
    expect(phone.log, contains('attendance m1'));
    expect(find.byKey(const Key('attendanceSheet')), findsOneWidget);
    expect(find.text('2 came · at most 2 at once'), findsOneWidget);
    expect(find.text('Hostess'), findsOneWidget);
    expect(find.textContaining('Waited in the lobby'), findsOneWidget);
    expect(find.text('Came, 30 min'), findsOneWidget);
    expect(
      find.textContaining('Did not come · Watched the stream, 5 min'),
      findsOneWidget,
    );
    expect(find.text('Watched the stream'), findsOneWidget);
  });

  testWidgets('a meeting nobody has been in says so', (tester) async {
    final phone = await signedIn(tester, seed: withMeetings);
    phone.attendance = null;
    await tapKey(tester, 'meetings');
    await tapKey(tester, 'meetingLinks-m2');
    await tapKey(tester, 'meetingAttendance-m2');
    expect(find.byKey(const Key('attendanceNone')), findsOneWidget);
  });
}
