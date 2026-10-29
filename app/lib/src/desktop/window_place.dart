// Where the window was and how big (D8 finding 32): kept in the app's data
// directory as `window.json` each time the user moves, resizes, maximizes
// or restores it, and put back at the next start. A place no longer on any
// screen (a monitor unplugged) keeps its size, made to fit, centred on the
// first screen.

import 'dart:convert';
import 'dart:io';
import 'dart:math' as math;
import 'dart:ui';

class WindowPlace {
  const WindowPlace({required this.bounds, this.maximized = false});

  /// The window's place when not maximized: what it goes back to when the
  /// user restores it.
  final Rect bounds;
  final bool maximized;

  /// Smaller than this was never a size the user chose.
  static const smallest = Size(320, 400);

  /// How much of the window has to be on a screen to be reached and dragged.
  static const reachable = Size(120, 48);

  Map<String, Object> toJson() => {
    'x': bounds.left,
    'y': bounds.top,
    'width': bounds.width,
    'height': bounds.height,
    'maximized': maximized,
  };

  /// What [toJson] wrote; null for anything else.
  static WindowPlace? fromJson(Object? json) {
    if (json is! Map) return null;
    double? n(String key) => switch (json[key]) {
      final num v when v.isFinite => v.toDouble(),
      _ => null,
    };
    final (x, y, w, h) = (n('x'), n('y'), n('width'), n('height'));
    if (x == null || y == null || w == null || h == null) return null;
    if (w < smallest.width || h < smallest.height) return null;
    return WindowPlace(
      bounds: Rect.fromLTWH(x, y, w, h),
      maximized: json['maximized'] == true,
    );
  }

  /// Where to put the window on `screens` (their visible areas): this place
  /// when enough of its top (where it is dragged from) is on one of them,
  /// else its size, made to fit, centred on the first.
  WindowPlace on(List<Rect> screens) {
    if (screens.isEmpty) return this;
    final top = Rect.fromLTWH(
      bounds.left,
      bounds.top,
      bounds.width,
      reachable.height,
    );
    for (final screen in screens) {
      final seen = top.intersect(screen);
      if (seen.width >= reachable.width && seen.height >= reachable.height) {
        return this;
      }
    }
    final screen = screens.first;
    final size = Size(
      math.min(bounds.width, screen.width),
      math.min(bounds.height, screen.height),
    );
    return WindowPlace(
      bounds: Rect.fromCenter(
        center: screen.center,
        width: size.width,
        height: size.height,
      ),
      maximized: maximized,
    );
  }

  /// The place kept in `file`; null when there is none, or it is unreadable.
  static Future<WindowPlace?> read(File file) async {
    try {
      return fromJson(jsonDecode(await file.readAsString()));
    } catch (_) {
      return null;
    }
  }

  /// Keep this place in `file`, replacing it whole.
  Future<void> write(File file) async {
    final part = File('${file.path}.part');
    await part.writeAsString(jsonEncode(toJson()), flush: true);
    await part.rename(file.path);
  }

  @override
  bool operator ==(Object other) =>
      other is WindowPlace &&
      other.bounds == bounds &&
      other.maximized == maximized;

  @override
  int get hashCode => Object.hash(bounds, maximized);

  @override
  String toString() => 'WindowPlace($bounds, maximized: $maximized)';
}
