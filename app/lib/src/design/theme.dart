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
      'away' || 'idle' || 'brb' || 'ringing' => held,
      'busy' ||
      'dnd' ||
      'on-the-phone' ||
      'on_the_phone' ||
      'on_call' => hangup,
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

  /// A button's own background, kept on a theme that paints its buttons
  /// (Feryx's silver): the call buttons' green and red.
  static Widget ownColour(
    BuildContext context,
    Set<WidgetState> states,
    Widget? child,
  ) => child ?? const SizedBox.shrink();

  /// Feryx: feryx.io's charcoal, silver buttons and peach accent, always
  /// dark. A tenant's brand colour does not replace it (its name and logo
  /// still show).
  static ThemeData feryx() {
    const scheme = ColorScheme(
      brightness: Brightness.dark,
      primary: FeryxColors.accent,
      onPrimary: FeryxColors.background,
      primaryContainer: FeryxColors.warmBadge,
      onPrimaryContainer: FeryxColors.warmBadgeText,
      secondary: FeryxColors.muted,
      onSecondary: FeryxColors.background,
      secondaryContainer: FeryxColors.raised,
      onSecondaryContainer: FeryxColors.text,
      tertiary: FeryxColors.coolBadgeText,
      onTertiary: FeryxColors.background,
      tertiaryContainer: FeryxColors.coolBadge,
      onTertiaryContainer: FeryxColors.coolBadgeText,
      error: Color(0xFFF2A39B),
      onError: FeryxColors.background,
      errorContainer: Color(0xFF2A1716),
      onErrorContainer: Color(0xFFF2A39B),
      surface: FeryxColors.background,
      onSurface: FeryxColors.text,
      onSurfaceVariant: FeryxColors.muted,
      surfaceDim: FeryxColors.background,
      surfaceBright: FeryxColors.raised,
      surfaceContainerLowest: FeryxColors.background,
      surfaceContainerLow: FeryxColors.panel,
      surfaceContainer: FeryxColors.panel,
      surfaceContainerHigh: FeryxColors.raised,
      surfaceContainerHighest: FeryxColors.raised,
      outline: FeryxColors.buttonBorder,
      outlineVariant: FeryxColors.line,
      inverseSurface: FeryxColors.text,
      onInverseSurface: FeryxColors.background,
      inversePrimary: FeryxColors.accent,
      shadow: Colors.black,
      scrim: Colors.black,
      surfaceTint: Colors.transparent,
    );
    const small = BorderRadius.all(Radius.circular(3));
    const panel = BorderRadius.all(Radius.circular(6));
    final base = ThemeData(
      useMaterial3: true,
      colorScheme: scheme,
      fontFamily: 'Inter',
      platform: TargetPlatform.android,
      visualDensity: VisualDensity.standard,
      extensions: const [FeryxLook()],
    );
    final text = base.textTheme;
    TextStyle? heading(TextStyle? s) =>
        s?.copyWith(fontWeight: FontWeight.w700, letterSpacing: -0.5);
    return base.copyWith(
      scaffoldBackgroundColor: FeryxColors.background,
      textTheme: text.copyWith(
        displayLarge: heading(text.displayLarge),
        displayMedium: heading(text.displayMedium),
        displaySmall: heading(text.displaySmall),
        headlineLarge: heading(text.headlineLarge),
        headlineMedium: heading(text.headlineMedium),
        headlineSmall: heading(text.headlineSmall),
        // Section headers as the site's eyebrows: mono, spaced, muted.
        titleSmall: text.titleSmall?.copyWith(
          fontFamily: 'IBM Plex Mono',
          fontWeight: FontWeight.w500,
          letterSpacing: 1,
          color: FeryxColors.muted,
        ),
      ),
      appBarTheme: const AppBarTheme(
        backgroundColor: FeryxColors.background,
        foregroundColor: FeryxColors.text,
        surfaceTintColor: Colors.transparent,
        shape: Border(bottom: BorderSide(color: FeryxColors.line)),
      ),
      navigationBarTheme: const NavigationBarThemeData(
        backgroundColor: FeryxColors.panel,
        indicatorColor: FeryxColors.raised,
        surfaceTintColor: Colors.transparent,
      ),
      navigationRailTheme: const NavigationRailThemeData(
        backgroundColor: FeryxColors.panel,
        indicatorColor: FeryxColors.raised,
      ),
      cardTheme: const CardThemeData(
        color: FeryxColors.panel,
        surfaceTintColor: Colors.transparent,
        shape: RoundedRectangleBorder(
          borderRadius: panel,
          side: BorderSide(color: FeryxColors.line),
        ),
      ),
      dialogTheme: const DialogThemeData(
        backgroundColor: FeryxColors.panel,
        surfaceTintColor: Colors.transparent,
        shape: RoundedRectangleBorder(
          borderRadius: panel,
          side: BorderSide(color: FeryxColors.line),
        ),
      ),
      dividerTheme: const DividerThemeData(color: FeryxColors.line),
      chipTheme: const ChipThemeData(
        backgroundColor: FeryxColors.warmBadge,
        labelStyle: TextStyle(
          fontFamily: 'IBM Plex Mono',
          fontWeight: FontWeight.w500,
          color: FeryxColors.warmBadgeText,
        ),
        side: BorderSide(color: FeryxColors.warmBadgeBorder),
        shape: RoundedRectangleBorder(borderRadius: small),
      ),
      inputDecorationTheme: const InputDecorationTheme(
        border: OutlineInputBorder(
          borderRadius: small,
          borderSide: BorderSide(color: FeryxColors.buttonBorder),
        ),
        enabledBorder: OutlineInputBorder(
          borderRadius: small,
          borderSide: BorderSide(color: FeryxColors.buttonBorder),
        ),
        focusedBorder: OutlineInputBorder(
          borderRadius: small,
          borderSide: BorderSide(color: FeryxColors.accent, width: 2),
        ),
      ),
      filledButtonTheme: FilledButtonThemeData(
        style: ButtonStyle(
          minimumSize: const WidgetStatePropertyAll(Size(64, 48)),
          shape: const WidgetStatePropertyAll(
            RoundedRectangleBorder(borderRadius: small),
          ),
          foregroundColor: const WidgetStatePropertyAll(FeryxColors.onSilver),
          backgroundColor: const WidgetStatePropertyAll(Colors.transparent),
          overlayColor: const WidgetStatePropertyAll(Colors.transparent),
          textStyle: WidgetStatePropertyAll(
            text.labelLarge?.copyWith(fontWeight: FontWeight.w600),
          ),
          // The site's 135° silver, brightened 10 % under the pointer.
          backgroundBuilder: (context, states, child) {
            final disabled = states.contains(WidgetState.disabled);
            final lit =
                states.contains(WidgetState.hovered) ||
                states.contains(WidgetState.pressed);
            return DecoratedBox(
              decoration: BoxDecoration(
                borderRadius: small,
                gradient: FeryxColors.silver(lit: lit),
              ),
              child: Opacity(opacity: disabled ? 0.5 : 1, child: child),
            );
          },
        ),
      ),
      outlinedButtonTheme: OutlinedButtonThemeData(
        style: ButtonStyle(
          minimumSize: const WidgetStatePropertyAll(Size(64, 48)),
          shape: const WidgetStatePropertyAll(
            RoundedRectangleBorder(borderRadius: small),
          ),
          backgroundColor: const WidgetStatePropertyAll(FeryxColors.button),
          foregroundColor: const WidgetStatePropertyAll(FeryxColors.buttonText),
          side: WidgetStateProperty.resolveWith(
            (states) => BorderSide(
              color:
                  states.contains(WidgetState.hovered) ||
                      states.contains(WidgetState.focused)
                  ? FeryxColors.accent
                  : FeryxColors.buttonBorder,
            ),
          ),
        ),
      ),
      textButtonTheme: TextButtonThemeData(
        style: TextButton.styleFrom(foregroundColor: FeryxColors.accent),
      ),
    );
  }
}

/// feryx.io's palette (its stylesheet).
class FeryxColors {
  static const background = Color(0xFF0D0E10);
  static const panel = Color(0xFF131518);
  static const raised = Color(0xFF1E2125);
  static const line = Color(0xFF2C2F34);
  static const text = Color(0xFFF1F2F4);
  static const muted = Color(0xFFB2B6BF);
  static const accent = Color(0xFFF4A879);
  static const button = Color(0xFF141416);
  static const buttonText = Color(0xFFF2F2F2);
  static const buttonBorder = Color(0xFF3A3A40);
  static const onSilver = Color(0xFF0A0A0B);
  static const warmBadge = Color(0xFF251C18);
  static const warmBadgeText = Color(0xFFFFC09B);
  static const warmBadgeBorder = Color(0xFF634938);
  static const coolBadge = Color(0xFF1B2425);
  static const coolBadgeText = Color(0xFFA9D3CD);

  /// The primary buttons' silver (135°), brightened 10 % when lit.
  static LinearGradient silver({bool lit = false}) {
    Color c(int r, int g, int b) {
      int up(int v) => lit ? (v * 1.1).round().clamp(0, 255) : v;
      return Color.fromARGB(255, up(r), up(g), up(b));
    }

    return LinearGradient(
      begin: Alignment.topLeft,
      end: Alignment.bottomRight,
      colors: [c(0xF2, 0xF2, 0xF2), c(0xBD, 0xBD, 0xC2), c(0x8C, 0x8C, 0x92)],
      stops: const [0, 0.55, 1],
    );
  }
}

/// On the Feryx theme: the screens show its emblem and wordmark where no
/// tenant's logo is shown.
class FeryxLook extends ThemeExtension<FeryxLook> {
  const FeryxLook();

  static bool of(BuildContext context) =>
      Theme.of(context).extension<FeryxLook>() != null;

  @override
  FeryxLook copyWith() => this;

  @override
  FeryxLook lerp(FeryxLook? other, double t) => this;
}
