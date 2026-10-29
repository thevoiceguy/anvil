// The window's kept place (D8 finding 32): read back as written, put back
// where it was while that is on a screen, and brought onto one when it no
// longer is.

import 'dart:io';
import 'dart:ui';

import 'package:anvil/src/desktop/window_place.dart';
import 'package:flutter_test/flutter_test.dart';

const left = Rect.fromLTWH(0, 0, 1920, 1040);
const right = Rect.fromLTWH(1920, 0, 2560, 1400);

void main() {
  test('a place is read back as it was written', () async {
    final dir = await Directory.systemTemp.createTemp('anvil-window');
    addTearDown(() => dir.delete(recursive: true));
    final file = File('${dir.path}/window.json');
    expect(await WindowPlace.read(file), isNull, reason: 'none kept yet');

    const place = WindowPlace(
      bounds: Rect.fromLTWH(2100, 80, 900, 1200),
      maximized: true,
    );
    await place.write(file);
    expect(await WindowPlace.read(file), place);

    await file.writeAsString('{"x": 1');
    expect(await WindowPlace.read(file), isNull, reason: 'unreadable');
  });

  test('anything but a place the user could have chosen is let go', () {
    expect(WindowPlace.fromJson('a string'), isNull);
    expect(WindowPlace.fromJson({'x': 0, 'y': 0, 'width': 900}), isNull);
    expect(
      WindowPlace.fromJson({'x': 0, 'y': 0, 'width': 40, 'height': 30}),
      isNull,
      reason: 'too small',
    );
    expect(
      WindowPlace.fromJson({'x': 10, 'y': 20, 'width': 900, 'height': 700}),
      const WindowPlace(bounds: Rect.fromLTWH(10, 20, 900, 700)),
    );
  });

  test('a place on a screen is put back as it was', () {
    const onRight = WindowPlace(bounds: Rect.fromLTWH(2100, 80, 900, 1200));
    expect(onRight.on([left, right]), onRight);

    // Mostly off the edge, but its top can still be dragged.
    const hanging = WindowPlace(bounds: Rect.fromLTWH(1700, 900, 900, 700));
    expect(hanging.on([left]), hanging);
  });

  test('a place on a screen that is gone is centred on the first', () {
    const onRight = WindowPlace(
      bounds: Rect.fromLTWH(2100, 80, 900, 1200),
      maximized: true,
    );
    final put = onRight.on([left]);
    expect(put.bounds.size, const Size(900, 1040), reason: 'made to fit');
    expect(put.bounds.center, left.center);
    expect(put.maximized, isTrue);

    // Only its bottom left on a screen: its top cannot be reached.
    const below = WindowPlace(bounds: Rect.fromLTWH(100, -650, 800, 700));
    expect(below.on([left]).bounds.center, left.center);
  });
}
