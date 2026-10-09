// Goldens that hold on every desktop: one set of screenshots, made on Linux,
// compared on Linux, Windows and macOS.
//
// Text is rasterised a little differently on each: glyph edges come out a
// few shades lighter or darker, which touches 2–3% of a screen's pixels
// (more, the more text it has) but faintly — measured on Windows and macOS,
// at most 0.06% of pixels differ by more than 96 of 255 in a channel, and
// none by more than 192. A real change (a colour, a font, an element moved
// or gone) differs strongly. So only strong differences count, and a screen
// matches while they stay under [strongShare] of its pixels. On Linux,
// where the goldens are made, a screen must match exactly, so a change too
// faint to count elsewhere still fails there. Each run prints both figures,
// so the numbers can be checked against the runs.

import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/foundation.dart';
import 'package:flutter_test/flutter_test.dart';

/// How far a channel may differ (of 255) before a pixel counts as changed.
const strongDifference = 96;

/// The share of a screen's pixels that may be changed.
const strongShare = 0.003;

void useTolerantGoldens() {
  final local = goldenFileComparator as LocalFileComparator;
  goldenFileComparator = _Tolerant(local.basedir.resolve('goldens_test.dart'));
}

class _Tolerant extends LocalFileComparator {
  _Tolerant(super.testFile);

  @override
  Future<bool> compare(Uint8List imageBytes, Uri golden) async {
    final expected = await getGoldenBytes(golden);
    final result = await GoldenFileComparator.compareLists(
      imageBytes,
      expected,
    );
    if (result.passed) {
      result.dispose();
      return true;
    }
    if (Platform.isLinux) {
      final error = await generateFailureOutput(result, golden, basedir);
      result.dispose();
      throw FlutterError(error);
    }
    final strong = await _strongShare(imageBytes, Uint8List.fromList(expected));
    debugPrint(
      'golden $golden: ${_percent(result.diffPercent)} of pixels differ, '
      '${strong == null ? 'sizes differ' : '${_percent(strong)} strongly'}',
    );
    if (strong != null && strong <= strongShare) {
      result.dispose();
      return true;
    }
    final error = await generateFailureOutput(result, golden, basedir);
    result.dispose();
    throw FlutterError(error);
  }

  static String _percent(double share) =>
      '${(share * 100).toStringAsFixed(3)}%';

  /// The share of pixels differing strongly; null when the sizes differ.
  static Future<double?> _strongShare(Uint8List a, Uint8List b) async {
    final (aw, ah, ap) = await _rgba(a);
    final (bw, bh, bp) = await _rgba(b);
    if (aw != bw || ah != bh) return null;
    var changed = 0;
    for (var i = 0; i < ap.length; i += 4) {
      for (var c = 0; c < 4; c++) {
        if ((ap[i + c] - bp[i + c]).abs() > strongDifference) {
          changed++;
          break;
        }
      }
    }
    return changed / (aw * ah);
  }

  static Future<(int, int, Uint8List)> _rgba(Uint8List png) async {
    final codec = await ui.instantiateImageCodec(png);
    final image = (await codec.getNextFrame()).image;
    final data = await image.toByteData(format: ui.ImageByteFormat.rawRgba);
    final out = (image.width, image.height, data!.buffer.asUint8List());
    image.dispose();
    codec.dispose();
    return out;
  }
}
