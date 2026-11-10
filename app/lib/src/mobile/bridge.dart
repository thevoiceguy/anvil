// The phone and the system's call screen kept in step (CallKit on iOS,
// Telecom on Android): a call ringing in rings there too, a call placed
// shows as dialling, connected, held and over as the phone has it; and what
// the user does there (answer, end, hold, mute, a key) reaches the phone.
// Also where a call's audio goes (shared with the screens through
// `MobileScope`), the proximity sensor while a call is at the ear, and the
// camera stopped while the app is out of sight.

import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';

import '../phone/phone_api.dart';
import '../phone/phone_model.dart';
import '../screens/calls.dart' show nameFor;
import '../screens/video.dart' show VideoTile;
import 'platform.dart';

class MobileBridge extends StatefulWidget {
  const MobileBridge({
    super.key,
    required this.model,
    required this.mobile,
    required this.child,
  });

  final PhoneModel model;
  final MobilePlatform mobile;
  final Widget child;

  @override
  State<MobileBridge> createState() => _MobileBridgeState();
}

/// What the system has been told of one call.
class _Told {
  _Told({required this.connected, required this.held});
  bool connected;
  bool held;
}

class _MobileBridgeState extends State<MobileBridge> {
  StreamSubscription<SystemCallEvent>? _events;
  StreamSubscription<AudioRoutes>? _routeChanges;
  StreamSubscription<bool>? _networkChanges;
  StreamSubscription<bool>? _pipChanges;
  Timer? _pipSize;
  PictureInPicture? _pip;
  final ValueNotifier<bool> _inPip = ValueNotifier(false);
  late final AppLifecycleListener _lifecycle;
  final Map<int, _Told> _told = {};
  final ValueNotifier<AudioRoutes?> _routes = ValueNotifier(null);
  bool? _proximity;

  PhoneModel get model => widget.model;
  MobilePlatform get mobile => widget.mobile;

  @override
  void initState() {
    super.initState();
    model.addListener(_sync);
    // Android's call screen follows the phone itself (platform.dart).
    if (!mobile.systemFollowsPhone) _events = mobile.callEvents.listen(_act);
    _routeChanges = mobile.audioRouteChanges.listen(_routed);
    // On cellular a call sends less video, or none (the user's setting).
    _networkChanges = mobile.cellularChanges.listen(_network);
    // Picture-in-picture while a call shows the other side's picture.
    _pipChanges = mobile.pictureInPictureChanges.listen(
      (on) => _inPip.value = on,
    );
    VideoTile.showing.addListener(_pipFor);
    unawaited(
      mobile.cellular().then((on) {
        if (on != null) _network(on);
      }),
    );
    // A phone's camera is the app's only while it is seen.
    _lifecycle = AppLifecycleListener(
      onHide: () => _background(true),
      onShow: () => _background(false),
    );
    // Seen now: the phone (out of sight until told) may use the camera.
    final now = WidgetsBinding.instance.lifecycleState;
    if (now == null ||
        now == AppLifecycleState.resumed ||
        now == AppLifecycleState.inactive) {
      _background(false);
    }
    _sync();
  }

  void _background(bool on) {
    unawaited(model.api.setBackground(on).catchError((Object _) {}));
  }

  void _network(bool cellular) {
    unawaited(
      model.api.setNetwork(cellular: cellular).catchError((Object _) {}),
    );
  }

  @override
  void dispose() {
    model.removeListener(_sync);
    _lifecycle.dispose();
    unawaited(_events?.cancel());
    unawaited(_routeChanges?.cancel());
    unawaited(_networkChanges?.cancel());
    unawaited(_pipChanges?.cancel());
    VideoTile.showing.removeListener(_pipFor);
    _pipSize?.cancel();
    _inPip.dispose();
    _routes.dispose();
    super.dispose();
  }

  Future<void> _setRoute(AudioRoute route) =>
      model.run(() => mobile.setAudioRoute(route));

  void _routed(AudioRoutes routes) {
    if (!mounted) return;
    _routes.value = routes;
    _proximityFor();
    _pipFor();
  }

  /// Where the audio goes changes with the calls (Telecom's routes come
  /// with a call): asked again whenever they change.
  Future<void> _askRoutes() async {
    final routes = await mobile.audioRoutes();
    if (mounted && routes != null) _routed(routes);
  }

  /// Picture-in-picture is allowed while a call is up, not held, and shows
  /// the other side's picture: their first stream, on the texture the
  /// call's stage shows it on, at its size (read again each second while
  /// allowed, as the network moves it).
  void _pipFor() {
    if (!mounted) return;
    final call = model.snapshot.calls
        .where((c) => c.isConnected && !c.held && c.streams.isNotEmpty)
        .firstOrNull;
    final stream = call?.streams.first;
    final texture = stream == null ? null : VideoTile.showing.value[stream];
    PictureInPicture? pip;
    if (stream != null && texture != null) {
      final info = model.api.videoTiles.info(stream);
      pip = PictureInPicture(
        texture: texture,
        width: info?.width ?? 16,
        height: info?.height ?? 9,
      );
    }
    if (pip == null) {
      _pipSize?.cancel();
      _pipSize = null;
    } else {
      _pipSize ??= Timer.periodic(const Duration(seconds: 1), (_) => _pipFor());
    }
    if (pip == _pip) return;
    _pip = pip;
    unawaited(mobile.setPictureInPicture(pip).catchError((Object _) {}));
  }

  /// The screen goes off at the ear: a call up or being placed, its audio
  /// on the earpiece (the phone's default when nothing says otherwise).
  void _proximityFor() {
    final calls = model.snapshot.calls;
    final atEar =
        calls.any((c) => c.isConnected || c.direction == Direction.outgoing) &&
        (_routes.value?.current ?? AudioRoute.earpiece) == AudioRoute.earpiece;
    if (atEar != _proximity) {
      _proximity = atEar;
      unawaited(mobile.setProximity(atEar));
    }
  }

  void _sync() {
    final snap = model.snapshot;
    final calls = {for (final c in snap.calls) c.id: c};
    if (calls.length != _told.length ||
        calls.values.any((c) => _told[c.id]?.connected != c.isConnected)) {
      unawaited(_askRoutes());
    }
    if (mobile.systemFollowsPhone) {
      // Kept only to know when the calls change, for the routes.
      _told
        ..removeWhere((id, _) => !calls.containsKey(id))
        ..addAll({
          for (final c in calls.values)
            c.id: _Told(connected: c.isConnected, held: c.held),
        });
      _proximityFor();
      _pipFor();
      return;
    }
    for (final call in calls.values) {
      final told = _told[call.id];
      if (told == null) {
        final system = SystemCall(
          id: call.id,
          name: nameFor(snap, call),
          number: shortAddress(call.remote),
        );
        _told[call.id] = _Told(connected: call.isConnected, held: call.held);
        if (call.direction == Direction.incoming && !call.isConnected) {
          unawaited(mobile.reportIncoming(system));
        } else {
          unawaited(mobile.reportOutgoing(system));
          if (call.isConnected) unawaited(mobile.reportConnected(call.id));
        }
        continue;
      }
      if (call.isConnected && !told.connected) {
        told.connected = true;
        unawaited(mobile.reportConnected(call.id));
      }
      if (call.held != told.held) {
        told.held = call.held;
        unawaited(mobile.reportHeld(call.id, call.held));
      }
    }
    for (final id in _told.keys.toList()) {
      if (!calls.containsKey(id)) {
        _told.remove(id);
        unawaited(mobile.reportEnded(id));
      }
    }
    _proximityFor();
    _pipFor();
  }

  PhoneCall? _call(int id) =>
      model.snapshot.calls.where((c) => c.id == id).firstOrNull;

  Future<void> _act(SystemCallEvent event) async {
    final call = _call(event.call);
    if (call == null) return;
    switch (event.action) {
      case SystemCallAction.shown:
      case SystemCallAction.ended:
        break;
      case SystemCallAction.failed:
        // Do Not Disturb or a blocked caller: the user asked not to be rung.
        // Anything else (no system call screen here) leaves the app ringing.
        if (event.reason == 'filtered' && call.isRingingIn) {
          await model.run(() => model.api.decline(call.id));
        }
      case SystemCallAction.answer:
        if (call.isRingingIn) {
          await model.run(() => model.api.answer(call.id));
        }
      case SystemCallAction.end:
        if (call.isRingingIn) {
          await model.run(() => model.api.decline(call.id));
        } else {
          await model.run(() => model.api.hangup(call.id));
        }
      case SystemCallAction.hold:
        final on = event.on ?? true;
        if (call.isConnected && call.held != on) {
          _told[call.id]?.held = on;
          await model.run(() => model.api.hold(call.id, on));
        }
      case SystemCallAction.mute:
        final on = event.on ?? true;
        if (call.muted != on) {
          await model.run(() => model.api.mute(call.id, on));
        }
      case SystemCallAction.dtmf:
        final digits = event.digits;
        if (digits != null && digits.isNotEmpty && call.isConnected) {
          await model.run(() => model.api.sendDigits(call.id, digits));
        }
    }
  }

  @override
  Widget build(BuildContext context) => MobileScope(
    routes: _routes,
    setRoute: _setRoute,
    // In Android's picture-in-picture the app's window shows only the
    // other side's picture; the app stays beneath, its tiles still open.
    child: ValueListenableBuilder(
      valueListenable: _inPip,
      builder: (context, inPip, child) {
        final pip = _pip;
        return Stack(
          fit: StackFit.expand,
          children: [
            child!,
            if (inPip && pip != null) _PipView(pip: pip),
          ],
        );
      },
      child: widget.child,
    ),
  );
}

/// The phone around the app, for a screen: where the call's audio goes and
/// a way to change it. Absent on desktop.
class MobileScope extends InheritedWidget {
  const MobileScope({
    super.key,
    required this.routes,
    required this.setRoute,
    required super.child,
  });

  final ValueListenable<AudioRoutes?> routes;
  final Future<void> Function(AudioRoute) setRoute;

  static MobileScope? maybeOf(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<MobileScope>();

  @override
  bool updateShouldNotify(MobileScope old) =>
      routes != old.routes || setRoute != old.setRoute;
}

/// The other side's picture, filling the window (picture-in-picture).
class _PipView extends StatelessWidget {
  const _PipView({required this.pip});
  final PictureInPicture pip;

  @override
  Widget build(BuildContext context) => ColoredBox(
    key: const Key('pictureInPicture'),
    color: const Color(0xFF000000),
    child: FittedBox(
      child: SizedBox(
        width: pip.width.toDouble(),
        height: pip.height.toDouble(),
        child: Texture(textureId: pip.texture),
      ),
    ),
  );
}
