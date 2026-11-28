// Which theme the app wears: the system's light or dark, either one always,
// or Feryx (feryx.io's palette, always dark). Kept on this device, in
// `theme.json` beside the app's other files.

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';

enum AnvilThemeChoice { system, light, dark, feryx }

class ThemeSetting extends ChangeNotifier {
  ThemeSetting({this.choice = AnvilThemeChoice.system, this.save});

  AnvilThemeChoice choice;

  /// Where a change is kept.
  final Future<void> Function(AnvilThemeChoice choice)? save;

  void set(AnvilThemeChoice value) {
    if (value == choice) return;
    choice = value;
    notifyListeners();
    unawaited(save?.call(value));
  }

  /// The setting kept in `file` (`{"theme": "feryx"}`); the system's without.
  static Future<ThemeSetting> read(File file) async {
    var choice = AnvilThemeChoice.system;
    try {
      final kept = jsonDecode(await file.readAsString());
      if (kept is Map) {
        choice = AnvilThemeChoice.values.firstWhere(
          (c) => c.name == kept['theme'],
          orElse: () => AnvilThemeChoice.system,
        );
      }
    } catch (_) {}
    return ThemeSetting(
      choice: choice,
      save: (c) async {
        try {
          await file.writeAsString(jsonEncode({'theme': c.name}));
        } catch (e) {
          debugPrint('anvil: the theme was not kept: $e');
        }
      },
    );
  }
}

/// The theme setting, for the Settings screen to change.
class ThemeScope extends InheritedNotifier<ThemeSetting> {
  const ThemeScope({
    super.key,
    required ThemeSetting setting,
    required super.child,
  }) : super(notifier: setting);

  static ThemeSetting? maybeOf(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<ThemeScope>()?.notifier;
}
