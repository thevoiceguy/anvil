# Anvil — mobile cross-compilation

> This page is about `anvil-ffi`, the C ABI for a native iOS or Android app.
> Anvil's own app (Flutter, `app/`) builds its Rust core for phones through
> cargokit instead; see `app/README.md`.

Anvil's mobile artifacts come from `anvil-ffi`. This crate produces both
a `staticlib` (for iOS) and a `cdylib` (for Android), with a
cbindgen-generated `anvil.h` in `target/<profile>/build/anvil-ffi-*/out/`.

`anvil-ffi` does **not** depend on `anvil-audio` or `cpal`. Mobile hosts
implement the audio path natively (AVAudioEngine on iOS, Oboe on Android)
and supply a function-pointer audio host across the FFI — see
`docs/PLATFORM_INTEGRATION.md` and the P3-M5 milestone.

## Common prerequisites

```bash
rustup target add \
    aarch64-linux-android \
    armv7-linux-androideabi \
    aarch64-apple-ios \
    aarch64-apple-ios-sim
```

`anvil-ffi` pulls three C deps that need cross-toolchains:

- **opus** (via `audiopus_sys`) — built from source by CMake.
- **openssl** (via `openssl-sys`, transitively from `forge-media`) —
  uses the system OpenSSL by default; vendor with the `vendored` feature
  if needed (set via env: `OPENSSL_STATIC=1 OPENSSL_VENDORED=1`).
- **ring** — bundles its own assembly per target; usually painless.

## Android

### One-time setup

1. Install the [Android NDK](https://developer.android.com/ndk/downloads),
   r25c or newer. On Linux:

   ```bash
   sudo apt install android-ndk    # if your distro packages it
   # or download from developer.android.com/ndk/downloads
   ```

2. Export the NDK root:

   ```bash
   export ANDROID_NDK_HOME=/opt/android-ndk-r26d
   ```

3. Install `cargo-ndk` to skip the env-var dance:

   ```bash
   cargo install cargo-ndk
   ```

### Build

```bash
# arm64 device
cargo ndk -t arm64-v8a -p 24 \
    build -p anvil-ffi --release

# armv7 device
cargo ndk -t armeabi-v7a -p 24 \
    build -p anvil-ffi --release
```

The `-p 24` is `minSdkVersion`; bump in step with your app's manifest.

Output goes to `target/aarch64-linux-android/release/libanvil_ffi.so`
(and `.../armv7-linux-androideabi/release/libanvil_ffi.so`). Drop these
into `app/src/main/jniLibs/<arch>/` of your Android Studio project.

### Without `cargo-ndk`

If you'd rather not add the wrapper, set the linker / archiver / CC
envs directly. Uncomment the relevant block in `.cargo/config.toml` and
edit the paths to match your NDK install.

## iOS

### One-time setup

1. Install Xcode (full IDE — the Command Line Tools alone don't ship the
   iOS SDKs).
2. Accept the licence: `sudo xcodebuild -license accept`.
3. Verify `xcrun --show-sdk-path --sdk iphoneos` prints a path.

No extra env vars needed; the `cc` crate finds the SDK via `xcrun`.

### Build

```bash
# Device
cargo build -p anvil-ffi --target aarch64-apple-ios --release

# Apple Silicon simulator
cargo build -p anvil-ffi --target aarch64-apple-ios-sim --release
```

Output: `target/<triple>/release/libanvil_ffi.a`.

### Bundling for Swift / Xcode

Combine the device + simulator artefacts into an XCFramework:

```bash
xcodebuild -create-xcframework \
    -library target/aarch64-apple-ios/release/libanvil_ffi.a \
    -headers target/aarch64-apple-ios/release/build/anvil-ffi-*/out \
    -library target/aarch64-apple-ios-sim/release/libanvil_ffi.a \
    -headers target/aarch64-apple-ios-sim/release/build/anvil-ffi-*/out \
    -output Anvil.xcframework
```

P3-M6 wraps this in a Swift Package.

## Verifying the build

The generated `anvil.h` lives in
`target/<triple>/<profile>/build/anvil-ffi-*/out/anvil.h`. The host build
in this repo includes a small C smoke test at `/tmp/anvil_ffi_smoke.c`
(see git log for the snippet) — useful for sanity-checking the ABI on
desktop before crossing.

## Known issues / cross-compile gotchas

- **`opus` build via CMake** needs `cmake` plus the cross-target's C
  toolchain. With `cargo-ndk` this is automatic; without it, set
  `CMAKE_TOOLCHAIN_FILE` to the NDK's `build/cmake/android.toolchain.cmake`.
- **`openssl-sys`** assumes the host's OpenSSL on Linux. For Android,
  either bundle vendored OpenSSL (set `OPENSSL_STATIC=1
  OPENSSL_VENDORED=1`) or pin to a cross-built libssl in the NDK
  sysroot.
- **`ring` ≥ 0.17** uses inline assembly per target; if the build
  errors out with "no such instruction" you almost certainly have a
  toolchain mismatch (e.g. NDK clang too old).
- **Linker name on Android**: NDK ≥ r23 dropped the per-API-level
  `aarch64-linux-android24-clang` and merged into one binary that
  takes `-target aarch64-linux-android24` instead. `cargo-ndk` handles
  this; manual configs need a one-line wrapper script.
