// The phone around the app on Android and iOS: what the app asks of the
// system there (the microphone today; the call screen and the audio
// session come with U4b). None on desktop and in the screens' tests.

import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

abstract class MobilePlatform {
  /// Ask for the microphone if the system hasn't been asked yet; whether
  /// the app may use it.
  Future<bool> requestMicrophone();

  /// Set the system's audio up for calls (iOS: play and record, voice
  /// chat), before the phone starts.
  Future<void> startAudio();
}

/// The app's own channel to `MainActivity` (Kotlin) and `AppDelegate`
/// (Swift).
class NativeMobile implements MobilePlatform {
  static const _channel = MethodChannel('anvil/mobile');

  static bool get supported =>
      !kIsWeb && (Platform.isAndroid || Platform.isIOS);

  @override
  Future<bool> requestMicrophone() async =>
      await _channel.invokeMethod<bool>('requestMicrophone') ?? false;

  @override
  Future<void> startAudio() => _channel.invokeMethod<void>('startAudio');
}
