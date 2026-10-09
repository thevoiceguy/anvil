# The Anvil app

Anvil's desktop and mobile app (`../docs/APP.md`): Flutter over `anvil-app`
through `flutter_rust_bridge`. The screens are in `lib/src/screens`, the phone
they drive is `lib/src/phone` (the real one is `anvil-app`, in `rust/`), the
desktop around the app (tray, notifications, the window, start at login) is
`lib/src/desktop`, the design is `lib/src/design`, and every string is in
`lib/l10n`.

```bash
flutter pub get
flutter run -d linux            # or windows, macos
flutter test                    # the screens against a fake phone, and the goldens
flutter test integration_test -d linux   # the app as built, its Rust core loaded
```

After changing `rust/src/api`, regenerate the bindings:

```bash
flutter_rust_bridge_codegen generate   # 2.13.0
```

`test/goldens` holds a screenshot of every screen, light and dark, drawn with
the app's own fonts at a fixed clock. One set, made on Linux, is compared on
Linux, Windows and macOS (within 2% of pixels; each run prints how far each
image is), so a screen that changes on purpose needs them made again:

```bash
flutter test test/goldens --update-goldens
```

A failed comparison leaves the images and their difference in
`test/goldens/failures` (CI keeps them as the run's artifact).

The tray's icons are drawn by `tool/tray_icons.py` (no dependencies):
`python3 tool/tray_icons.py`. On Linux the tray needs an AppIndicator host
and, to build, `libayatana-appindicator3-dev`.

The font is Inter (SIL Open Font License, `assets/fonts/OFL.txt`), declared in
`pubspec.yaml` and bundled so the app reads the same on every platform.
