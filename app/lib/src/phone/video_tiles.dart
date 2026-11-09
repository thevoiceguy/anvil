// Tiles on Flutter textures (FCP's `docs/SOFTPHONE_VIDEO.md` §2.3): the
// runner's texture plugin makes a texture over `anvil/video` and hands back
// its id, the C function that fills it and the texture's own address; those
// two go to Rust (`app/rust/src/api/video.rs`), whose video threads put each
// picture on it. Pictures never cross into Dart. Where the runner has no
// plugin (Windows until its video host), nothing is shown.

import 'package:flutter/services.dart';

import '../rust/api/video.dart' as rust;
import 'phone_api.dart';

class TextureTiles implements VideoTiles {
  static const _channel = MethodChannel('anvil/video');

  @override
  Future<VideoTexture?> show(int? stream) async {
    final Map<Object?, Object?>? made;
    try {
      made = await _channel.invokeMapMethod<Object?, Object?>('create');
    } on MissingPluginException {
      return null;
    }
    final id = made?['texture'];
    final present = made?['present'];
    final handle = made?['handle'];
    if (id is! int || present is! int || handle is! int) return null;
    try {
      final attachment = rust.attachTile(
        stream: stream,
        present: BigInt.from(present),
        texture: BigInt.from(handle),
      );
      return _Texture(id, attachment);
    } catch (_) {
      await _channel.invokeMethod<void>('dispose', {'texture': id});
      return null;
    }
  }

  @override
  TileInfo? info(int? stream) {
    final i = rust.tileInfo(stream: stream);
    return i == null
        ? null
        : (width: i.width, height: i.height, frozen: i.frozen);
  }
}

class _Texture implements VideoTexture {
  _Texture(this.textureId, this._attachment);

  @override
  final int textureId;
  final BigInt _attachment;
  bool _closed = false;

  @override
  Future<void> close() async {
    if (_closed) return;
    _closed = true;
    // Rust lets go of it first: no picture is put on it after this.
    rust.detachTile(attachment: _attachment);
    await TextureTiles._channel.invokeMethod<void>('dispose', {
      'texture': textureId,
    });
  }
}
