# The Anvil app

Anvil's desktop and mobile app (`../docs/APP.md`): Flutter over `anvil-app`
through `flutter_rust_bridge`. The screens are in `lib/src/screens`, the phone
they drive is `lib/src/phone` (the real one is `anvil-app`, in `rust/`), the
design is `lib/src/design`, and every string is in `lib/l10n`.

```bash
flutter pub get
flutter run -d linux            # or windows, macos
flutter test                    # the screens, against a fake phone
flutter test integration_test -d linux   # the app as built, its Rust core loaded
```

After changing `rust/src/api`, regenerate the bindings:

```bash
flutter_rust_bridge_codegen generate   # 2.13.0
```

The font is Inter (SIL Open Font License, `assets/fonts/OFL.txt`), bundled so
the app reads the same on every platform.
