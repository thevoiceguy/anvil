// On a phone the app asks for the microphone and sets the audio up before
// the phone starts, and says so when the microphone is refused.

import 'package:anvil/src/app.dart';
import 'package:anvil/src/mobile/platform.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'fake_phone.dart';

class FakeMobile implements MobilePlatform {
  FakeMobile({required this.allow});
  final bool allow;
  final log = <String>[];

  @override
  Future<bool> requestMicrophone() async {
    log.add('requestMicrophone');
    return allow;
  }

  @override
  Future<void> startAudio() async => log.add('startAudio');
}

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
}
