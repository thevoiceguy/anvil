// A call's pictures (FCP's `docs/SOFTPHONE_VIDEO.md` §2.5): each stream
// coming in is a tile of its own, laid out by the app, and the self-view is
// a corner tile that can be dragged. Every tile is a texture the platform
// fills; pictures never cross into Dart.

import 'dart:async';

import 'package:flutter/material.dart';

import '../../l10n/app_localizations.dart';
import '../phone/phone_api.dart';

/// One tile: an incoming stream, or (null) the self-view.
class VideoTile extends StatefulWidget {
  const VideoTile({
    super.key,
    required this.tiles,
    required this.stream,
    this.fit = BoxFit.contain,
    this.mirror = false,
  });

  final VideoTiles tiles;
  final int? stream;

  /// How the picture fills the tile: whole (`contain`) or cropped to fill
  /// it (`cover`, the self-view).
  final BoxFit fit;

  /// Shown as in a mirror (the self-view, as people expect to see
  /// themselves).
  final bool mirror;

  @override
  State<VideoTile> createState() => _VideoTileState();

  /// The texture each incoming stream is shown on now, by stream: what a
  /// phone's picture-in-picture shows (`mobile/bridge.dart`).
  static final showing = ValueNotifier<Map<int, int>>(const {});
}

class _VideoTileState extends State<VideoTile> {
  VideoTexture? _texture;
  TileInfo? _info;
  Timer? _poll;
  bool _gone = false;

  @override
  void initState() {
    super.initState();
    _open();
    // The picture's size (it changes as the network allows) and whether
    // it is frozen, read twice a second.
    _poll = Timer.periodic(const Duration(milliseconds: 500), (_) => _read());
    _read();
  }

  Future<void> _open() async {
    final texture = await widget.tiles.show(widget.stream);
    if (_gone) {
      await texture?.close();
      return;
    }
    setState(() => _texture = texture);
    final stream = widget.stream;
    if (texture != null && stream != null) {
      VideoTile.showing.value = {
        ...VideoTile.showing.value,
        stream: texture.textureId,
      };
    }
  }

  void _read() {
    final info = widget.tiles.info(widget.stream);
    if (info != _info && mounted) setState(() => _info = info);
  }

  @override
  void dispose() {
    _gone = true;
    _poll?.cancel();
    final (texture, stream) = (_texture, widget.stream);
    if (texture != null &&
        stream != null &&
        VideoTile.showing.value[stream] == texture.textureId) {
      VideoTile.showing.value = {...VideoTile.showing.value}..remove(stream);
    }
    texture?.close();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final texture = _texture;
    final info = _info;
    if (texture == null || info == null) {
      return const ColoredBox(
        color: Colors.black,
        child: Center(child: Icon(Icons.videocam, color: Colors.white24)),
      );
    }
    Widget picture = SizedBox(
      width: info.width.toDouble(),
      height: info.height.toDouble(),
      child: Texture(textureId: texture.textureId),
    );
    if (widget.mirror) picture = Transform.flip(flipX: true, child: picture);
    return ColoredBox(
      color: Colors.black,
      child: Stack(
        fit: StackFit.expand,
        children: [
          ClipRect(
            child: FittedBox(fit: widget.fit, child: picture),
          ),
          if (info.frozen)
            Positioned(
              right: 8,
              top: 8,
              child: Tooltip(
                message: Strings.of(context).videoFrozen,
                child: const Icon(
                  Icons.hourglass_top,
                  key: Key('videoFrozen'),
                  color: Colors.white70,
                  size: 18,
                ),
              ),
            ),
        ],
      ),
    );
  }
}

/// A call's pictures: theirs filling it, ours in a corner the user can
/// drag.
class CallVideoStage extends StatefulWidget {
  const CallVideoStage({
    super.key,
    required this.tiles,
    required this.call,
    required this.name,
  });

  final VideoTiles tiles;
  final PhoneCall call;

  /// Whom the call is with: said while their camera is off.
  final String name;

  @override
  State<CallVideoStage> createState() => _CallVideoStageState();
}

class _CallVideoStageState extends State<CallVideoStage> {
  /// Where the self-view is, from the stage's top left; null until the
  /// stage is first laid out (then bottom right).
  Offset? _self;

  static const _margin = 12.0;

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    final call = widget.call;
    final streams = call.streams;
    return AspectRatio(
      aspectRatio: 16 / 9,
      child: ClipRRect(
        borderRadius: BorderRadius.circular(12),
        child: LayoutBuilder(
          builder: (context, box) {
            final selfWidth = (box.maxWidth / 4).clamp(96.0, 240.0);
            final selfSize = Size(selfWidth, selfWidth * 9 / 16);
            final limit = Offset(
              box.maxWidth - selfSize.width - _margin,
              box.maxHeight - selfSize.height - _margin,
            );
            final at = _clamp(_self ?? limit, limit);
            return Stack(
              children: [
                Positioned.fill(
                  child: call.contentStream != null
                      ? _Shared(
                          tiles: widget.tiles,
                          content: call.contentStream!,
                          streams: streams,
                        )
                      : streams.isEmpty
                      ? ColoredBox(
                          key: const Key('videoTheirsOff'),
                          color: Colors.black87,
                          child: Center(
                            child: Column(
                              mainAxisSize: MainAxisSize.min,
                              children: [
                                const Icon(
                                  Icons.videocam_off,
                                  color: Colors.white54,
                                  size: 40,
                                ),
                                const SizedBox(height: 8),
                                Text(
                                  widget.name,
                                  style: const TextStyle(color: Colors.white70),
                                ),
                              ],
                            ),
                          ),
                        )
                      : _Grid(tiles: widget.tiles, streams: streams),
                ),
                if (call.camera)
                  Positioned(
                    left: at.dx,
                    top: at.dy,
                    width: selfSize.width,
                    height: selfSize.height,
                    child: GestureDetector(
                      key: const Key('selfView'),
                      onPanUpdate: (d) =>
                          setState(() => _self = _clamp(at + d.delta, limit)),
                      child: Tooltip(
                        message: s.selfView,
                        child: DecoratedBox(
                          position: DecorationPosition.foreground,
                          decoration: BoxDecoration(
                            border: Border.all(color: Colors.white54),
                            borderRadius: BorderRadius.circular(8),
                          ),
                          child: ClipRRect(
                            borderRadius: BorderRadius.circular(8),
                            child: VideoTile(
                              tiles: widget.tiles,
                              stream: null,
                              fit: BoxFit.cover,
                              mirror: true,
                            ),
                          ),
                        ),
                      ),
                    ),
                  ),
              ],
            );
          },
        ),
      ),
    );
  }

  static Offset _clamp(Offset at, Offset limit) => Offset(
    at.dx.clamp(_margin, limit.dx < _margin ? _margin : limit.dx),
    at.dy.clamp(_margin, limit.dy < _margin ? _margin : limit.dy),
  );
}

/// A shared screen (a room's) large, and the cameras beside it.
class _Shared extends StatelessWidget {
  const _Shared({
    required this.tiles,
    required this.content,
    required this.streams,
  });
  final VideoTiles tiles;
  final int content;
  final List<int> streams;

  @override
  Widget build(BuildContext context) {
    final shared = VideoTile(
      key: ValueKey('content-$content'),
      tiles: tiles,
      stream: content,
    );
    if (streams.isEmpty) return shared;
    return Row(
      key: const Key('sharedStage'),
      children: [
        Expanded(flex: 3, child: shared),
        const SizedBox(width: 2),
        Expanded(
          child: Column(
            children: [
              for (final stream in streams.take(4))
                Expanded(
                  child: Padding(
                    padding: const EdgeInsets.only(bottom: 2),
                    child: VideoTile(
                      key: ValueKey('tile-$stream'),
                      tiles: tiles,
                      stream: stream,
                    ),
                  ),
                ),
            ],
          ),
        ),
      ],
    );
  }
}

/// The streams coming in: one fills the stage; more share it.
class _Grid extends StatelessWidget {
  const _Grid({required this.tiles, required this.streams});
  final VideoTiles tiles;
  final List<int> streams;

  @override
  Widget build(BuildContext context) {
    if (streams.length == 1) {
      return VideoTile(
        key: ValueKey('tile-${streams.first}'),
        tiles: tiles,
        stream: streams.first,
      );
    }
    final columns = streams.length <= 4 ? 2 : 3;
    final rows = (streams.length / columns).ceil();
    return Column(
      children: [
        for (var r = 0; r < rows; r++)
          Expanded(
            child: Row(
              children: [
                for (var c = 0; c < columns; c++)
                  Expanded(
                    child: r * columns + c < streams.length
                        ? Padding(
                            padding: const EdgeInsets.all(1),
                            child: VideoTile(
                              key: ValueKey('tile-${streams[r * columns + c]}'),
                              tiles: tiles,
                              stream: streams[r * columns + c],
                            ),
                          )
                        : const SizedBox.shrink(),
                  ),
              ],
            ),
          ),
      ],
    );
  }
}
