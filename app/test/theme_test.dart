// The Feryx theme (feryx.io's palette): picked in Settings beside System,
// Light and Dark, always dark, kept on this device, and a tenant's brand
// colour not replacing it.

import 'dart:io';

import 'package:anvil/src/app.dart';
import 'package:anvil/src/design/theme.dart';
import 'package:anvil/src/design/theme_setting.dart';
import 'package:anvil/src/phone/phone_api.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'fake_phone.dart';

Future<void> open(
  WidgetTester tester,
  ThemeSetting setting, {
  PhoneSnapshot Function(PhoneSnapshot)? seed,
}) async {
  tester.view.physicalSize = const Size(1000, 1400);
  tester.view.devicePixelRatio = 1;
  tester.platformDispatcher.platformBrightnessTestValue = Brightness.light;
  addTearDown(tester.view.reset);
  addTearDown(tester.platformDispatcher.clearPlatformBrightnessTestValue);
  final phone = FakePhone()..saved = const SignedInAccountFake();
  if (seed != null) phone.seed = seed;
  await tester.pumpWidget(AnvilApp(phone: phone, themeSetting: setting));
  await tester.pumpAndSettle();
}

ThemeData shown(WidgetTester tester) =>
    Theme.of(tester.element(find.byKey(const Key('title'))));

void main() {
  testWidgets('Feryx is picked in Settings, kept, and always dark', (
    tester,
  ) async {
    final kept = <AnvilThemeChoice>[];
    final setting = ThemeSetting(save: (c) async => kept.add(c));
    await open(tester, setting);
    expect(shown(tester).brightness, Brightness.light);
    expect(shown(tester).extension<FeryxLook>(), isNull);

    await tester.tap(find.text('Settings').last);
    await tester.pumpAndSettle();
    await tester.scrollUntilVisible(
      find.byKey(const Key('themeChoice')),
      200,
      scrollable: find.byType(Scrollable).first,
    );
    await tester.tap(find.byKey(const Key('themeChoice')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('themeChoice-feryx')).last);
    await tester.pumpAndSettle();

    expect(kept, [AnvilThemeChoice.feryx]);
    final feryx = shown(tester);
    // Dark on a light system, in feryx.io's charcoal and peach.
    expect(feryx.brightness, Brightness.dark);
    expect(feryx.extension<FeryxLook>(), isNotNull);
    expect(feryx.colorScheme.surface, FeryxColors.background);
    expect(feryx.colorScheme.primary, FeryxColors.accent);
    expect(find.byKey(const Key('feryxEmblem')), findsOneWidget);

    // Back to Light: as it was, no emblem.
    setting.set(AnvilThemeChoice.light);
    await tester.pumpAndSettle();
    expect(shown(tester).extension<FeryxLook>(), isNull);
    expect(shown(tester).brightness, Brightness.light);
    expect(find.byKey(const Key('feryxEmblem')), findsNothing);
  });

  testWidgets("a brand's colour does not replace Feryx's", (tester) async {
    await open(
      tester,
      ThemeSetting(choice: AnvilThemeChoice.feryx),
      seed: (s) => s.copyWith(brandPrimary: 0xFF3355FF),
    );
    expect(shown(tester).colorScheme.primary, FeryxColors.accent);
  });

  test('the choice is kept in theme.json and read back', () async {
    final dir = await Directory.systemTemp.createTemp('anvil-theme');
    addTearDown(() => dir.delete(recursive: true));
    final file = File('${dir.path}/theme.json');

    // Nothing kept yet: the system's.
    final first = await ThemeSetting.read(file);
    expect(first.choice, AnvilThemeChoice.system);
    first.set(AnvilThemeChoice.feryx);
    await Future<void>.delayed(const Duration(milliseconds: 50));
    expect(await file.readAsString(), '{"theme":"feryx"}');
    expect((await ThemeSetting.read(file)).choice, AnvilThemeChoice.feryx);

    // Something unreadable is the system's again.
    await file.writeAsString('{"theme":"plaid"}');
    expect((await ThemeSetting.read(file)).choice, AnvilThemeChoice.system);
  });
}
