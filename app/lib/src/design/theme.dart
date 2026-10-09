// Anvil's design (`docs/APP.md` §4): one set of colours, type and shapes on
// every platform. A tenant's brand replaces the colours at run time.

import 'package:flutter/material.dart';

class AnvilColors {
  static const ink = Color(0xFF14202B);
  static const accent = Color(0xFF0E8A7E);
  static const answer = Color(0xFF1F9D55);
  static const hangup = Color(0xFFD64545);
  static const surface = Color(0xFFF6F8FA);
  static const surfaceDark = Color(0xFF0F161D);
  static const held = Color(0xFFD99A1E);

  /// A person's presence as a dot: available, away, busy, offline.
  static Color presence(String? presence, {bool onCall = false}) {
    if (onCall) return hangup;
    return switch (presence) {
      'available' || 'online' || 'open' => answer,
      'away' || 'idle' || 'brb' => held,
      'busy' || 'dnd' || 'on-the-phone' => hangup,
      _ => const Color(0xFF9AA5B1),
    };
  }
}

class AnvilTheme {
  /// `brand` is the tenant's primary colour, when it has one.
  static ThemeData light({Color? brand}) => _theme(Brightness.light, brand);
  static ThemeData dark({Color? brand}) => _theme(Brightness.dark, brand);

  static ThemeData _theme(Brightness brightness, Color? brand) {
    final scheme = ColorScheme.fromSeed(
      seedColor: brand ?? AnvilColors.accent,
      brightness: brightness,
      surface: brightness == Brightness.light
          ? AnvilColors.surface
          : AnvilColors.surfaceDark,
    );
    return ThemeData(
      useMaterial3: true,
      colorScheme: scheme,
      fontFamily: 'Inter',
      // The same look and motion on every platform, not each one's own.
      platform: TargetPlatform.android,
      visualDensity: VisualDensity.standard,
      inputDecorationTheme: const InputDecorationTheme(
        border: OutlineInputBorder(
          borderRadius: BorderRadius.all(Radius.circular(12)),
        ),
      ),
      filledButtonTheme: FilledButtonThemeData(
        style: FilledButton.styleFrom(
          minimumSize: const Size(64, 48),
          shape: const RoundedRectangleBorder(
            borderRadius: BorderRadius.all(Radius.circular(12)),
          ),
        ),
      ),
    );
  }
}
