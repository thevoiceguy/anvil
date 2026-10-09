// Goldens compared with a small tolerance: one set of screenshots, made on
// Linux, holds on Windows and macOS too, where text is rasterised a little
// differently. How different each image is gets printed, so the tolerance
// can be judged from the runs.

import 'package:flutter/foundation.dart';
import 'package:flutter_test/flutter_test.dart';

/// The share of pixels an image may differ by and still match.
const tolerance = 0.02;

void useTolerantGoldens() {
  final local = goldenFileComparator as LocalFileComparator;
  goldenFileComparator = _Tolerant(local.basedir.resolve('goldens_test.dart'));
}

class _Tolerant extends LocalFileComparator {
  _Tolerant(super.testFile);

  @override
  Future<bool> compare(Uint8List imageBytes, Uri golden) async {
    final result = await GoldenFileComparator.compareLists(
      imageBytes,
      await getGoldenBytes(golden),
    );
    debugPrint(
      'golden $golden: ${(result.diffPercent * 100).toStringAsFixed(3)}% '
      'of pixels differ',
    );
    if (result.passed || result.diffPercent <= tolerance) {
      result.dispose();
      return true;
    }
    final error = await generateFailureOutput(result, golden, basedir);
    result.dispose();
    throw FlutterError(error);
  }
}
