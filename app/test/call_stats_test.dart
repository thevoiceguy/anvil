// A call's stats (FCP's docs/CALL_DIAGNOSTICS.md phase 2, D8 finding 43)
// against a fake phone: the score beside the quality indicator, the panel
// it opens as a dialog on a wide window and a sheet on a narrow one,
// updated as the stats change, and closed when the call ends.

import 'package:anvil/src/app.dart';
import 'package:anvil/src/phone/phone_api.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'fake_phone.dart';

const measured = CallQuality(
  jitterMs: 3,
  lossPermille: 2,
  rttMs: 42,
  packetsReceived: 1500,
  packetsPlayed: 1500,
  recentLossPermille: 0,
  packetsSent: 1510,
  bytesSent: 260000,
  rtpReceived: 1500,
  bytesReceived: 258000,
  packetsExpected: 1503,
  packetsLost: 3,
  sendKbps: 69,
  recvKbps: 68,
  sendPps: 50,
  recvPps: 50,
  farLossPermille: 5,
  farJitterMs: 2,
  farLost: 4,
  rtcp: true,
  rtcpSent: 6,
  rtcpReceived: 6,
  clockRate: 8000,
  ptimeMs: 20,
  localRtp: '192.168.1.20:40000',
  remoteRtp: '203.0.113.5:30000',
  mosHundredths: 438,
  farMosHundredths: 421,
);

Future<FakePhone> onACall(WidgetTester tester, {required Size size}) async {
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  final phone = FakePhone()..saved = const SignedInAccountFake();
  await tester.pumpWidget(AnvilApp(phone: phone));
  await tester.pumpAndSettle();
  await tester.enterText(find.byKey(const Key('target')), '1002');
  await tester.tap(find.byKey(const Key('call')).last);
  await tester.pumpAndSettle();
  measure(phone, measured);
  await tester.pumpAndSettle();
  return phone;
}

void measure(FakePhone phone, CallQuality q) => phone.set(
  phone.state.copyWith(
    calls: [
      phone
          .callOf(1)
          .copyWith(
            state: CallState.connected,
            codec: 'pcmu',
            encrypted: true,
            quality: q,
          ),
    ],
  ),
  'call',
  call: 1,
);

String stat(WidgetTester tester, String key) => tester
    .widget<SelectableText>(
      find.descendant(
        of: find.byKey(Key('stat-$key')),
        matching: find.byType(SelectableText),
      ),
    )
    .data!;

void main() {
  testWidgets('the indicator shows the score and opens the stats', (
    tester,
  ) async {
    final phone = await onACall(tester, size: const Size(1000, 1000));
    // The lower of the two: what the far end hears.
    expect(tester.widget<Text>(find.byKey(const Key('mos'))).data, '4.2');

    await tester.tap(find.byKey(const Key('callStatsButton')));
    await tester.pumpAndSettle();
    expect(find.byType(Dialog), findsOneWidget);
    expect(
      tester.widget<Text>(find.byKey(const Key('statsScore'))).data,
      '4.2 · Good connection',
    );
    expect(stat(tester, 'youHear'), '4.4');
    expect(stat(tester, 'theyHear'), '4.2');
    expect(stat(tester, 'audio'), 'PCMU · 8 kHz · 20 ms packets');
    expect(stat(tester, 'encryption'), 'SRTP');
    expect(stat(tester, 'media'), '192.168.1.20:40000 ↔ 203.0.113.5:30000');
    expect(
      stat(tester, 'sent'),
      '50 packets/s · 69 kb/s\n1510 packets · 260 KB in all',
    );
    expect(
      stat(tester, 'received'),
      '50 packets/s · 68 kb/s\n1500 packets · 258 KB in all',
    );
    expect(stat(tester, 'loss'), '0.0% last 5 s · 0.2% in all (3 of 1503)');
    expect(stat(tester, 'jitter'), '3 ms');
    expect(stat(tester, 'farEnd'), '0.5% lost · 2 ms jitter · 4 lost in all');
    expect(stat(tester, 'roundTrip'), '42 ms');
    expect(stat(tester, 'rtcp'), '6 sent · 6 received');
    // Nothing dropped: no row for it.
    expect(find.byKey(const Key('stat-dropped')), findsNothing);

    // Live: the next second's figures land in the open panel.
    measure(
      phone,
      const CallQuality(
        jitterMs: 45,
        lossPermille: 40,
        recentLossPermille: 80,
        packetsReceived: 1600,
        packetsPlayed: 1550,
        packetsDropped: 50,
        unexpectedPt: 8,
        expectedPt: 0,
        rtcp: true,
        mosHundredths: 250,
      ),
    );
    await tester.pumpAndSettle();
    expect(
      tester.widget<Text>(find.byKey(const Key('statsScore'))).data,
      '2.5 · Poor connection',
    );
    expect(stat(tester, 'theyHear'), 'Not yet measured');
    expect(stat(tester, 'farEnd'), 'No reports yet');
    expect(stat(tester, 'roundTrip'), 'Not yet measured');
    expect(
      stat(tester, 'dropped'),
      '1600 packets received, 1550 played, 50 dropped\n'
      "Payload type 8 arrived; this call's codec is 0",
    );

    // The call ends: its stats close.
    await phone.hangup(1);
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('callStats')), findsNothing);
  });

  testWidgets('a narrow window opens the stats as a sheet', (tester) async {
    await onACall(tester, size: const Size(420, 900));
    await tester.tap(find.byKey(const Key('callStatsButton')));
    await tester.pumpAndSettle();
    expect(find.byType(Dialog), findsNothing);
    expect(find.byType(BottomSheet), findsOneWidget);
    expect(stat(tester, 'youHear'), '4.4');
  });

  testWidgets('before the first second the stats say they are measuring', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(1000, 1000);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    final phone = FakePhone()..saved = const SignedInAccountFake();
    await tester.pumpWidget(AnvilApp(phone: phone));
    await tester.pumpAndSettle();
    await tester.enterText(find.byKey(const Key('target')), '1002');
    await tester.tap(find.byKey(const Key('call')).last);
    await tester.pumpAndSettle();
    phone.set(
      phone.state.copyWith(
        calls: [phone.callOf(1).copyWith(state: CallState.connected)],
      ),
      'call',
      call: 1,
    );
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('quality')), findsNothing);
    expect(find.byKey(const Key('mos')), findsNothing);
    await tester.tap(find.byKey(const Key('callStatsButton')));
    await tester.pumpAndSettle();
    expect(stat(tester, 'waiting'), 'Measuring…');
  });

  testWidgets('recents show how each reported call went', (tester) async {
    tester.view.physicalSize = const Size(1000, 1000);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    final phone = FakePhone()
      ..saved = const SignedInAccountFake()
      ..seed = (s) => s.copyWith(
        recents: [
          RecentCall(
            id: 'good',
            direction: Direction.outgoing,
            remote: 'sip:1002@x',
            startedAt: DateTime(2026, 10, 31, 9),
            durationSecs: 60,
            mosHundredths: 431,
          ),
          RecentCall(
            id: 'poor',
            direction: Direction.incoming,
            remote: 'sip:1003@x',
            startedAt: DateTime(2026, 10, 31, 10),
            durationSecs: 60,
            mosHundredths: 262,
            poor: true,
          ),
          RecentCall(
            id: 'unknown',
            direction: Direction.incoming,
            remote: 'sip:1004@x',
            startedAt: DateTime(2026, 10, 31, 11),
          ),
        ],
      );
    await tester.pumpWidget(AnvilApp(phone: phone));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Recents').last);
    await tester.pumpAndSettle();
    String details(String id) => tester
        .widget<ListTile>(find.byKey(Key('recent-$id')))
        .subtitle
        .toString();
    expect(find.textContaining(RegExp(r'· Audio 4\.3$')), findsOneWidget);
    expect(find.textContaining(RegExp(r'· Poor audio 2\.6$')), findsOneWidget);
    expect(details('unknown'), isNot(contains('udio')));
  });
}
