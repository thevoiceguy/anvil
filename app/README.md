# The Anvil app

Anvil's desktop and mobile app (`../docs/APP.md`): Flutter over `anvil-app`
through `flutter_rust_bridge`. The screens are in `lib/src/screens`, the phone
they drive is `lib/src/phone` (the real one is `anvil-app`, in `rust/`), the
desktop around the app (tray, notifications, the window, start at login) is
`lib/src/desktop`, the phone around it on Android and iOS (the microphone,
the audio session, the system's call screen, where a call's audio goes,
the proximity sensor) is `lib/src/mobile` with its
native side in `android/app/src/main/kotlin` (`CallSystem`, the
`ConnectionService`, the call's foreground service) and
`ios/Runner/AppDelegate.swift` (CallKit, the audio routes), the design
is `lib/src/design`, and every string is in `lib/l10n`.

```bash
flutter pub get
flutter run -d linux            # or windows, macos
flutter test                    # the screens against a fake phone, and the goldens
flutter test integration_test/app_test.dart -d linux   # the app as built, its Rust core loaded
```

On a phone:

```bash
flutter build apk                     # Android: JDK 17, the SDK, NDK 28.2.13676358
flutter build ios --no-codesign       # iOS: Xcode, on a Mac
flutter test integration_test/mobile_test.dart -d <emulator or simulator>
```

The Rust core is built for each Android ABI and iOS target by cargokit
(`rustup target add aarch64-linux-android armv7-linux-androideabi
x86_64-linux-android i686-linux-android`, or `aarch64-apple-ios
aarch64-apple-ios-sim x86_64-apple-ios`); libopus is built with CMake,
which cargokit points at the NDK (`ANDROID_NDK_ROOT`). On Android,
`MainActivity` loads the library before Dart does and hands it the JVM and
the application context (`rust/src/android.rs`): cpal's Android audio
reaches the system through them.

After changing `rust/src/api`, regenerate the bindings:

```bash
flutter_rust_bridge_codegen generate   # 2.13.0
```

`test/goldens` holds a screenshot of every screen, light and dark, drawn with
the app's own fonts at a fixed clock. One set, made on Linux, is compared on
Linux (exactly) and on Windows and macOS, where text's edges rasterise a few
shades apart: there a screen matches while under 0.3% of its pixels differ
by more than 96 of 255 (`test/goldens/tolerant_comparator.dart`; each run
prints the figures). A screen that changes on purpose needs them made again:

```bash
flutter test test/goldens --update-goldens
```

A failed comparison leaves the images and their difference in
`test/goldens/failures` (CI keeps them as the run's artifact).

The tray's icons and the app's (desktop, Android, iOS) are drawn by
`tool/icons.py` (no dependencies): `python3 tool/icons.py`. On Linux the tray needs an AppIndicator host
and, to build, `libayatana-appindicator3-dev`.

The font is Inter (SIL Open Font License, `assets/fonts/OFL.txt`), declared in
`pubspec.yaml` and bundled so the app reads the same on every platform.
