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

/// What a stage shows, as a room that forwards each person's stream is
/// asked for it (`PhoneApi.setRoomView`).
class StageView {
  const StageView({required this.members, this.speaker = false, this.pinned});

  /// Everyone in the room, by id (the room leaves out our own).
  final List<String> members;

  /// The one who speaks, large.
  final bool speaker;

  /// This member large: the user's pin, or a host's spotlight.
  final String? pinned;

  String get _key => '${members.join(',')}|$speaker|$pinned';
}

/// A call's pictures: theirs filling it, ours in a corner the user can
/// drag. In a room that forwards each person's stream the app lays the
/// tiles out: a grid, or one large (the user's pin, a host's spotlight, or
/// the speaker) with the others beside it; each tile named.
class CallVideoStage extends StatefulWidget {
  const CallVideoStage({
    super.key,
    required this.tiles,
    required this.call,
    required this.name,
    this.onView,
  });

  final VideoTiles tiles;
  final PhoneCall call;

  /// Whom the call is with: said while their camera is off.
  final String name;

  /// In a room that forwards, what to send it, each time that changes.
  final void Function(StageView view)? onView;

  @override
  State<CallVideoStage> createState() => _CallVideoStageState();
}

class _CallVideoStageState extends State<CallVideoStage> {
  /// Where the self-view is, from the stage's top left; null until the
  /// stage is first laid out (then bottom right).
  Offset? _self;

  /// The member the user pinned, by id.
  String? _pinned;

  /// The speaker large rather than a grid.
  bool _speakerView = false;

  /// The view last sent, as a key.
  String? _sent;

  static const _margin = 12.0;

  @override
  void initState() {
    super.initState();
    _sendView();
  }

  @override
  void didUpdateWidget(CallVideoStage old) {
    super.didUpdateWidget(old);
    _sendView();
  }

  bool get _forwards => widget.call.room?.forwards ?? false;

  /// Whom to show large: the user's pin, else a host's spotlight.
  String? _large(RoomInfo room) => _pinned ?? room.spotlit?.id;

  void _sendView() {
    final room = widget.call.room;
    final onView = widget.onView;
    if (room == null || !room.forwards || !widget.call.isConnected) return;
    // A pin on someone who left is let go.
    if (_pinned != null && room.member(_pinned) == null) _pinned = null;
    final large = _large(room);
    final view = StageView(
      members: [for (final m in room.members) m.id],
      speaker: large == null && _speakerView,
      pinned: large,
    );
    if (view._key == _sent || onView == null) return;
    _sent = view._key;
    onView(view);
  }

  void _change(void Function() change) {
    setState(change);
    _sendView();
  }

  /// A tile tapped: its person pinned, or let go if they were.
  void _pin(String? member) {
    if (member == null) return;
    _change(() => _pinned = _pinned == member ? null : member);
  }

  /// The stream to show large, where the room forwards: the pinned or
  /// spotlit person's, else (in the speaker's view) the speaker's.
  int? _largeStream(RoomInfo room, List<int> streams) {
    final large = _large(room);
    RoomStream? found;
    for (final st in room.streams) {
      if (!streams.contains(st.stream)) continue;
      if (large != null && st.member == large) {
        // Their own stream before the speaker's that shows them now.
        if (found == null || found.speaker) found = st;
      } else if (large == null && _speakerView && st.speaker) {
        found = st;
      }
    }
    return found?.stream;
  }

  Widget _tile(int stream) {
    final room = widget.call.room;
    if (room == null || !room.forwards) {
      return VideoTile(
        key: ValueKey('tile-$stream'),
        tiles: widget.tiles,
        stream: stream,
      );
    }
    final st = room.stream(stream);
    final member = room.member(st?.member);
    return _NamedTile(
      key: ValueKey('tile-$stream'),
      tiles: widget.tiles,
      stream: stream,
      member: member,
      pinned: member != null && member.id == _large(room),
      onTap: () => _pin(member?.id),
    );
  }

  Widget _theirs(BuildContext context) {
    final call = widget.call;
    final streams = call.streams;
    final content = call.contentStream;
    if (content != null) {
      return _Large(
        key: const Key('sharedStage'),
        large: VideoTile(
          key: ValueKey('content-$content'),
          tiles: widget.tiles,
          stream: content,
        ),
        others: [for (final st in streams.take(4)) _tile(st)],
      );
    }
    if (streams.isEmpty) {
      return ColoredBox(
        key: const Key('videoTheirsOff'),
        color: Colors.black87,
        child: Center(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              const Icon(Icons.videocam_off, color: Colors.white54, size: 40),
              const SizedBox(height: 8),
              Text(widget.name, style: const TextStyle(color: Colors.white70)),
            ],
          ),
        ),
      );
    }
    final room = call.room;
    if (room != null && room.forwards) {
      final large = _largeStream(room, streams);
      if (large != null) {
        return _Large(
          key: const Key('largeStage'),
          large: _tile(large),
          others: [
            for (final st in streams.where((st) => st != large).take(4))
              _tile(st),
          ],
        );
      }
    }
    return _Grid(streams: streams, tile: _tile);
  }

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    final call = widget.call;
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
                Positioned.fill(child: _theirs(context)),
                if (_forwards && call.contentStream == null)
                  Positioned(
                    right: 8,
                    top: 8,
                    child: _StageButton(
                      key: const Key('stageLayout'),
                      icon: _speakerView && _pinned == null
                          ? Icons.grid_view
                          : Icons.co_present,
                      tooltip: _speakerView && _pinned == null
                          ? s.stageGrid
                          : s.stageSpeaker,
                      onPressed: () => _change(() {
                        _speakerView = !(_speakerView && _pinned == null);
                        _pinned = null;
                      }),
                    ),
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

/// A small round button on the stage.
class _StageButton extends StatelessWidget {
  const _StageButton({
    super.key,
    required this.icon,
    required this.tooltip,
    required this.onPressed,
  });
  final IconData icon;
  final String tooltip;
  final VoidCallback onPressed;

  @override
  Widget build(BuildContext context) => Material(
    color: Colors.black54,
    shape: const CircleBorder(),
    child: IconButton(
      icon: Icon(icon, color: Colors.white, size: 20),
      tooltip: tooltip,
      onPressed: onPressed,
    ),
  );
}

/// One person's tile in a room that forwards: their picture, their name,
/// a ring while they speak, and marks for muted, pinned and held back.
/// Tapped, it is pinned (or let go).
class _NamedTile extends StatelessWidget {
  const _NamedTile({
    super.key,
    required this.tiles,
    required this.stream,
    required this.member,
    required this.pinned,
    required this.onTap,
  });
  final VideoTiles tiles;
  final int stream;

  /// Whom it shows; null until the roster says.
  final RoomMember? member;
  final bool pinned;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    final m = member;
    return GestureDetector(
      onTap: onTap,
      child: Stack(
        fit: StackFit.expand,
        children: [
          VideoTile(tiles: tiles, stream: stream),
          if (m != null && m.speaking)
            IgnorePointer(
              child: DecoratedBox(
                key: ValueKey('speaking-$stream'),
                decoration: BoxDecoration(
                  border: Border.all(color: Colors.greenAccent, width: 3),
                ),
              ),
            ),
          if (m != null)
            Positioned(
              left: 6,
              bottom: 6,
              right: 6,
              child: Align(
                alignment: Alignment.bottomLeft,
                child: DecoratedBox(
                  decoration: BoxDecoration(
                    color: Colors.black54,
                    borderRadius: BorderRadius.circular(4),
                  ),
                  child: Padding(
                    padding: const EdgeInsets.symmetric(
                      horizontal: 6,
                      vertical: 2,
                    ),
                    child: Row(
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        if (m.muted)
                          const Padding(
                            padding: EdgeInsets.only(right: 4),
                            child: Icon(
                              Icons.mic_off,
                              color: Colors.white70,
                              size: 14,
                            ),
                          ),
                        Flexible(
                          child: Text(
                            m.name,
                            key: ValueKey('name-$stream'),
                            overflow: TextOverflow.ellipsis,
                            style: const TextStyle(
                              color: Colors.white,
                              fontSize: 12,
                            ),
                          ),
                        ),
                        if (pinned)
                          Padding(
                            padding: const EdgeInsets.only(left: 4),
                            child: Icon(
                              Icons.push_pin,
                              key: ValueKey('pinned-$stream'),
                              semanticLabel: s.stagePinned,
                              color: Colors.white70,
                              size: 14,
                            ),
                          ),
                        if (m.paused)
                          Padding(
                            padding: const EdgeInsets.only(left: 4),
                            child: Tooltip(
                              message: s.stagePaused,
                              child: Icon(
                                Icons
                                    .signal_cellular_connected_no_internet_0_bar,
                                key: ValueKey('paused-$stream'),
                                color: Colors.white70,
                                size: 14,
                              ),
                            ),
                          ),
                      ],
                    ),
                  ),
                ),
              ),
            ),
        ],
      ),
    );
  }
}

/// One tile large, and up to four beside it.
class _Large extends StatelessWidget {
  const _Large({super.key, required this.large, required this.others});
  final Widget large;
  final List<Widget> others;

  @override
  Widget build(BuildContext context) {
    if (others.isEmpty) return large;
    return Row(
      children: [
        Expanded(flex: 3, child: large),
        const SizedBox(width: 2),
        Expanded(
          child: Column(
            children: [
              for (final other in others)
                Expanded(
                  child: Padding(
                    padding: const EdgeInsets.only(bottom: 2),
                    child: other,
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
  const _Grid({required this.streams, required this.tile});
  final List<int> streams;
  final Widget Function(int stream) tile;

  @override
  Widget build(BuildContext context) {
    if (streams.length == 1) return tile(streams.first);
    final columns = streams.length <= 4
        ? 2
        : streams.length <= 9
        ? 3
        : 4;
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
                            child: tile(streams[r * columns + c]),
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
