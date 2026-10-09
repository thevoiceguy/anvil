# The Anvil app

Designed 2026-10-07; agreed the same day on every recommendation in §8 (Flutter, `app/` in this repository, the desktop app serving the control socket, desktop first, one design everywhere, self-updating desktop builds, strings externalised from U1). Certificates and store accounts (§8 Q6) are the owner's to obtain; builds stay unsigned until then. Progress: U0 done: `anvil-app` (the running phone, its state and changes, the commands; mute added to `anvil-core`), the control socket, and `anvil-cli run` with the commands that drive it. U1 done: `app/` (Flutter 3.47, `flutter_rust_bridge` 2.13 over `anvil-app` in `app/rust`), sign-in, keypad, incoming and in-call screens, Anvil's theme with Inter bundled, strings in `lib/l10n`, CI building all three desktops. U2a done: the user's data in `anvil-app`. U2b done: the screens (in a call, transfer, a second call, recents, people, voicemail, settings) and voicemail played through the call speaker. U2c done: tray, notifications, start at login, golden screenshots on all three desktops; U2 complete.

Anvil works: it signs in to FCP, registers, places and takes calls, holds,
transfers, encrypts its media, shows message waiting, presence and busy lamps,
and reads the user's history, directory, voicemail, settings and live events.
It does all of that from `anvil-cli` and through `anvil-ffi`, and nobody will
adopt it that way. This document is the app people use: one design that looks
and behaves the same on Windows, macOS, Linux, iOS and Android. It also covers
a CLI phone that keeps running and takes commands (`anvil call 1002`,
`anvil hangup`), which the people who run FCP will want as much as the users
want the app.

## 1. What there is

- **`anvil-core`**: the SIP and media client. Calls, hold, transfer, DTMF,
  SRTP, MWI, presence, branding, over UDP, TCP or TLS, tested against FCP.
- **`anvil-fcp`**: FCP's client: discovery, browser and password sign-in, the
  app session, `/me/softphone`, history, directory, voicemail, calling
  settings, live events.
- **`anvil-ffi`**: a C ABI for both, made for a host that brings its own UI.
- **`anvil-cli`**: one process, one call. It registers, places a call if told,
  prints events and exits on Ctrl+C. Its data subcommands each run once. You
  cannot tell a running `anvil-cli` to answer, hang up or transfer.
- **Releases**: a tag publishes `anvil-cli.exe` for Windows (v0.1.0). CI
  builds and tests the workspace on Linux and Windows.

What is missing:

- **A UI**, the subject of this document.
- **A phone that keeps running.** Everything the app does between calls —
  ringing, lamps, message waiting, a second call — needs a long-lived process
  that holds the state, and today the only long-lived thing is one CLI call.
- **Mobile lifecycle.** PushKit and CallKit on iOS, FCM and ConnectionService
  on Android, waking to ring a backgrounded app. This needs push from FCP,
  which FCP does not have (`docs/SOFTPHONE.md` §5 in FCP names it as the next
  project there).
- **Signing and stores.** An unsigned exe is fine for a first release. An app
  users install needs signing on every platform, and the stores need
  developer accounts.

## 2. The choice of toolkit

"The same on every device" rules most options out. A toolkit that hands
drawing to each platform's own widgets or web view looks different on each
platform, however careful the CSS.

| | Desktop | Mobile | Same look everywhere | Calls into Rust |
|---|---|---|---|---|
| **Flutter** | Windows, macOS, Linux: stable | iOS, Android: stable | yes, it draws every pixel itself (Impeller/Skia) | `flutter_rust_bridge` generates Dart for a Rust API, async and streams included |
| Tauri 2 | stable | young | no: WebView2, WKWebView and WebKitGTK render differently | native |
| Slint | stable | Android experimental, iOS early | yes | native |
| Compose Multiplatform | JVM | Android stable, iOS newer | yes | through JNI and a C ABI |
| React Native | weak on desktop | stable | partly | through a C ABI |
| A native UI per platform | | | no, and five UIs to keep in step | through the C ABI |

**Recommendation: Flutter**, as Anvil's roadmap already says. It is the only
option that is stable on all five platforms *and* draws identically on all of
them. It also has the softphone plumbing a VoIP app needs: CallKit,
ConnectionService, push, audio session, tray icons and local notifications all
have maintained plugins. `flutter_rust_bridge` lets the app call a Rust API
directly, with structs, enums, async functions and streams, instead of the C
ABI's JSON strings. The cost is a second language (Dart) and Flutter's runtime
in every build: roughly 20 MB on desktop and 10 MB on mobile.

## 3. The model

```
┌──────────────────────────────┐   ┌───────────────────────────────┐
│ Flutter app (Dart)           │   │ anvil-cli                      │
│ every screen, every platform │   │ `anvil run` · `anvil call …`   │
└──────────────┬───────────────┘   └───────────────┬───────────────┘
               │ flutter_rust_bridge                │ control socket
┌──────────────┴────────────────────────────────────┴───────────────┐
│ anvil-app (Rust): the running phone                               │
│ accounts · calls · history · directory · presence · voicemail ·   │
│ settings · brand · one state model, one stream of changes         │
│ serves the control socket                                         │
├──────────────────────────┬───────────────────────────────────────┤
│ anvil-core (SIP, media)  │ anvil-fcp (FCP's API)                  │
└──────────────────────────┴───────────────────────────────────────┘
```

- **`anvil-app`** is a new crate: the phone as a long-lived thing. It owns the
  `Anvil` and `FcpClient`, keeps the state a screen shows, and publishes every
  change on one stream:
  - the account and its registration;
  - the calls and their state;
  - history, the directory with presence, voicemail and calling settings,
    refreshed from FCP's live events;
  - the brand.

  The screens and the CLI only read that state and send it commands. The app
  is therefore thin, and the CLI and the app cannot behave differently.
- **The control socket.** `anvil-app` can serve a local socket (a Unix socket
  on Linux and macOS, a named pipe on Windows) that only the same user can
  open. On it, one-line JSON commands (`call`, `answer`, `hangup`, `hold`,
  `transfer`, `dtmf`, `mute`, `dnd`, `status`) get one-line answers, plus a
  subscription to the event stream.
- **The CLI phone:**
  - `anvil run` starts `anvil-app` in the terminal and serves the socket, with
    an interactive prompt that takes the same commands.
  - `anvil call 1002`, `anvil hangup`, `anvil status` and the rest talk to
    whatever is serving the socket.
  - With the desktop app serving it too (§8 Q3), `anvil call 1002` from a
    script dials through the app the user is looking at.
- **The Flutter app** lives in `app/` in this repository, outside the Cargo
  workspace so Rust builds stay hermetic. It calls `anvil-app` through
  `flutter_rust_bridge`. On mobile, `anvil-app` runs inside the app. On
  desktop, likewise.

## 4. The app

One design system on every platform: Anvil's own components, type scale,
spacing and motion, with the tenant's brand (colors, logo, app name,
ringtones) applied at run time from FCP's branding. Platform conventions apply
only where users would notice them missing: window controls and the menu bar
on desktop, the system call screen (CallKit, ConnectionService) on mobile, the
share sheet.

- **Sign in.** Server address or email address (FCP's discovery), the system
  browser for SSO or a password, a one-time code when asked. The install
  becomes a device of the user's, as it does today.
- **Keypad.** Dial a number, an extension or a name.
- **In a call.** Mute, hold, keypad (DTMF), speaker or headset, blind and
  attended transfer, park, add a second call and swap, end. Call quality and
  encryption shown.
- **Incoming.** Answer, decline, send to voicemail, the caller's name from the
  directory. A second call while one is up.
- **Recents.** History, missed calls marked, call back.
- **People.** The tenant's directory with presence, searchable, busy lamps for
  favourites.
- **Voicemail.** Messages with transcription, play, mark heard, delete, the
  count on the tab.
- **Settings.** Do not disturb, forwarding, call waiting, audio devices and
  ringtone, start at login (desktop), sign out.
- **Desktop.** A tray icon with presence and DND, a notification for an
  incoming call that answers from the notification, a compact in-call window,
  a global answer/hang-up shortcut.

## 5. Platforms and distribution

| | Package | Signing | Needs |
|---|---|---|---|
| Windows | MSIX or an installer (`.msi`) | a code-signing certificate | a certificate (EV or OV) |
| macOS | `.dmg`, notarized | Developer ID + notarization | an Apple Developer account |
| Linux | AppImage and Flatpak; `.deb` | none required | nothing |
| iOS | TestFlight, App Store | Apple | an Apple Developer account |
| Android | APK, then Play | an upload key | a Google Play developer account |

Every release builds all of them in CI from one tag, as `release.yml` does for
the CLI today. Desktop gets an auto-updater. Until the certificates and
accounts exist, builds are unsigned. Linux needs nothing, and Windows and
macOS warn before running.

## 6. How it is proved

- **`anvil-app`'s own tests:** the state model and the control socket, against
  FCP as `anvil-fcp`'s tests are. The CLI phone is tested through the socket.
- **Flutter widget tests** for each screen, driven by a fake `anvil-app`.
- **Integration tests** that run the app against FCP and place a call. These
  run in CI on Linux, Windows and macOS for desktop, and Android in an
  emulator.
- **Golden screenshots** of every screen on every platform, compared in CI:
  this is what "consistent across devices" means, checked rather than hoped.

## 7. Phases

Each phase is a branch and a PR, as before.

### U0: the running phone and the CLI that drives it (Rust)
- `anvil-app`: the state model, the change stream, the commands.
- The control socket (Unix socket, named pipe), and `anvil run` with its
  prompt.
- `anvil call`, `anvil answer`, `anvil hangup`, `anvil hold`, `anvil transfer`,
  `anvil dtmf`, `anvil mute`, `anvil dnd`, `anvil status`.
- Tested against FCP.

### U1: the app's skeleton, on desktop
- `app/` with Flutter and `flutter_rust_bridge` over `anvil-app`.
- Sign in, register, keypad, place, answer and end a call; the design system's
  first components; the brand applied.
- Builds for Windows, macOS and Linux in CI.

### U2: the desktop app
- In-call controls, transfer, a second call, recents, people with presence,
  voicemail, settings.
- Tray, notifications, start at login, audio devices.
- Golden screenshots on all three desktops.

Three PRs, the screens thin over the state as §3 has it:

- **U2a, the data in `anvil-app`.** The state gains the calling settings,
  the recent calls, the directory as people (presence, a busy lamp,
  favourites), the mailbox's messages, the audio devices, and on each call
  whether it is encrypted and how its media is doing. FCP's data is read at
  start and again when `/me/events` says it changed (a call ended, a
  message, settings changed elsewhere); a status or a lamp changes one
  person in place. New commands: `calling` (forwards, call waiting),
  `park` (FCP's park code), `heard`, `delete_voicemail`, `favourite`,
  `audio` and `refresh`; a call placed or answered while another is up
  holds that one, as a desk phone does. Favourites and the devices chosen
  are this device's own, kept beside the session (`settings.json`); a
  device chosen applies to the calls set up after it. FCP has no
  "send to voicemail": declining is what sends a call there, when the user
  has a mailbox and no busy forward. Lamps for the whole directory come
  from FCP's events, not a SIP subscription per person.
- **U2b, the screens:** in a call, transfer, a second call, recents,
  people, voicemail, settings.
  - The navigation is a rail beside the page, or a bar below it on a
    narrow window: keypad, recents (missed calls counted), people,
    voicemail (new messages counted), settings. A call ringing in shows
    over every page, and so does the call up, which leads back to it.
  - In a call: mute, hold, a keypad sending each digit as pressed, park,
    the call's timer, a lock when its media is encrypted, its quality as
    good, fair or poor, its codec. Transfer takes a number or a person
    from the directory, then goes at once (blind) or after talking to
    them first: the call is held, the second placed, and "complete
    transfer" joins the two (attended). "Add call" places a second call
    and holds the first; resuming a held call holds the other, so the two
    swap, in `anvil-app` as at the CLI.
  - Recents and voicemail name callers from the directory; a tap calls
    back. Voicemail plays through the speaker calls use, decoded from
    FCP's WAV (PCM, float, A-law, μ-law) in `anvil-app`, so `anvil play
    <id>` and `anvil stop` do the same; a message played is heard.
  - Settings: do not disturb, call waiting, the four forwards (and how
    long to ring before "not answered"), the microphone and speaker for
    the next call, the account and sign-out.
- **U2c, the desktop:** tray, notifications, start at login, golden
  screenshots.
  - `lib/src/desktop`: the app talks to a `DesktopShell` (a fake in the
    tests); `NativeShell` is tray_manager 0.5.3 (the method-channel line;
    0.6 onward is a new implementation still changing daily), window_manager,
    flutter_local_notifications and launch_at_startup, each part logged and
    left out when the desktop lacks it. `DesktopBridge` keeps it in step
    with the phone.
  - The tray's icon is the phone's state: offline, ready, do not disturb,
    ringing, in a call (`tool/tray_icons.py` draws them). Its menu: show
    the window, do not disturb, quit. A click shows the window; closing the
    window leaves the phone running in the tray, and quit stops it (it
    unregisters) before the app goes.
  - A call ringing in is a notification, named from the directory, with
    Answer and Decline (Windows' call scenario, a critical notification on
    Linux, time-sensitive on macOS); it goes when the call stops ringing.
    Answering from it brings the window up. macOS registers the actions'
    titles at start, in English until U6.
  - Start at login is a switch under Settings → This computer: the
    registry on Windows, an autostart entry on Linux, `SMAppService` on
    macOS 13 and later (the channel launch_at_startup calls, answered in
    `MainFlutterWindow.swift`).
  - Golden screenshots of every screen, light and dark, wide and narrow,
    drawn with the app's fonts at a fixed clock: one set made on Linux,
    compared exactly on Linux and on Windows and macOS within what text
    rasterisation explains (under 0.3% of pixels differing by more than
    96/255; the first runs measured 0.03–0.06% there, against 2.4–2.6% of
    pixels differing at all). Making them
    found Inter bundled but never declared (every platform drew its own
    font), the keypad's hint cut off, and Call and End dark on dark.
  - macOS: the sandboxed app had no network or microphone entitlement, and
    no microphone usage text; both are added.
  - The global answer/hang-up shortcut and the compact in-call window
    (§4) are not in U2.

### U3: desktop distribution
- Installers for the three desktops built on a tag, the auto-updater, signing
  as soon as the certificates exist.

Three PRs:

- **U3a, installers on a tag.** `release.yml` builds, on a tag `vX.Y.Z`
  (the workspace's version, which also names the app's build):
  - Windows: an Inno Setup installer (`app/packaging/windows.iss`) for this
    user only, without administrator rights, in
    `%LOCALAPPDATA%\Programs\Anvil`, with a Start menu entry and an
    uninstaller that also removes start at login. The Visual C++ runtime
    goes beside the exe, so nothing else needs installing.
  - macOS: a disk image (`app/packaging/macos.sh`) with the app, universal
    (Apple Silicon and Intel), ad hoc signed as Flutter builds it.
  - Linux: a .deb (in `/opt/anvil`, `anvil` on the path, depending on GTK,
    ALSA and the AppIndicator library) and an AppImage
    (`app/packaging/linux.sh`; appimagetool 1.9.1 and the type2 runtime
    pinned by digest).
  - Each is installed or mounted, started (still running ten seconds on),
    and removed again in CI before anything is published, with
    `SHA256SUMS` over the release. A pull request touching the packaging
    builds and checks the same without publishing.
  - The app is called Anvil (not Flutter's `anvil` and `com.thevoiceguy`)
    and has its own icon (`tool/icons.py`), on every platform.
- **U3b, the app updates itself.**
  - Each release carries `latest.json` (the version, the release's page,
    each installer by target with its URL, size and SHA-256;
    `app/packaging/manifest.py`) and `latest.json.sig`, an Ed25519
    signature over it made in the release workflow with the
    `ANVIL_UPDATE_KEY` secret and checked there against
    `app/packaging/update-key.pub.pem` before anything is published.
  - `crates/anvil-update` reads both from the latest release, accepts the
    manifest only under the public key built in (`UPDATE_KEY`; a test holds
    it to the committed PEM), downloads this copy's file and checks its size
    and SHA-256 (a file that is not the release's is deleted), and installs
    it the way this copy was installed:
    - the Windows setup: the new setup runs silently (`/relaunch=1`
      starts the app again when done);
    - an AppImage: the new file takes the old one's place, started once the
      app has gone;
    - a macOS bundle in a folder the user may write: swapped once the app has
      quit, then opened. The app is sandboxed, which allows no such write,
      so today a Mac copy is told of the release and given the disk image —
      self-updating needs the sandbox dropped for the direct download
      (App Store builds would keep it and update through the store): the
      owner's decision;
    - the .deb, or a build run from its folder: told of the release and
      given the file.
  - The app checks half a minute after it starts and twice a day. A banner
    offers "Update and restart" (not during a call; the phone unregisters
    first) or "Download" for a copy that cannot update itself, and Later.
    Settings show the version, check by hand and say how it went.
  - The key's private half is the repository secret and one offline copy
    kept by the owner. Lost, existing installs cannot take updates until
    they are reinstalled by hand with a build carrying a new key.
- **U3c, signing**: Windows Authenticode, macOS Developer ID and
  notarization, once the certificates exist (§8 Q6).

### U4: the app on mobile
Works while the app is open; ringing a closed app is U5.

- **U4a, the app builds and runs on a phone.**
  - Android and iOS projects in `app/android` and `app/ios`, the same
    application id as the desktop (`com.thevoiceguy.anvil`).
  - The Rust core cross-built by cargokit for every Android ABI and the
    iOS device and simulators; libopus by CMake against the NDK.
  - Android: `MainActivity` loads the library and hands it the JVM and the
    application context, which cpal's Android audio needs and a Flutter
    app does not otherwise set up.
  - The microphone asked for before the phone starts (refused, the phone
    still rings and the user is told why); on iOS the audio session set
    to play and record, voice chat.
  - The desktop's pieces stay off a phone: no tray or window, no control
    socket, no self-update (a store updates the app).
  - The install names itself by the phone's model in the user's devices
    ("Anvil on Google Pixel 8").
  - CI builds the APK (debug-signed) and the iOS app (unsigned), and runs
    the mobile integration test in an Android emulator and an iOS
    simulator.
- **U4b, a phone's call.** CallKit and ConnectionService for the call
  screen, the audio routed per call (earpiece, speaker, Bluetooth), the
  proximity sensor.

### U5: ringing a sleeping phone (with FCP)
- FCP's push project: RFC 8599 push parameters in REGISTER, APNs and FCM
  senders, holding an INVITE until the woken app registers.
- PushKit and FCM in the app.
- TestFlight and a Play test track.

### U6: finish
- Accessibility (VoiceOver, TalkBack, keyboard navigation), localisation,
  the store listings.

## 8. Questions

1. **The toolkit.** *Recommendation:* Flutter with `flutter_rust_bridge` (§2).
2. **Where the app lives.** *Recommendation:* `app/` in this repository, so a
   change to `anvil-app` and the screen that uses it land in one PR. A
   separate repository is the alternative.
3. **The desktop app serves the control socket too.** *Recommendation:* yes,
   so `anvil call` drives the app the user has open. Local only, same user
   only, and off with a setting.
4. **Desktop first, mobile after.** *Recommendation:* yes. Desktop needs
   nothing FCP lacks. Mobile adoption needs push (U5, which is FCP work too),
   and the stores need accounts.
5. **One design everywhere, or adaptive.** *Recommendation:* Anvil's own
   design on every platform, branded by the tenant, with only the platform
   conventions listed in §4. The alternative is Material on Android and
   Cupertino on iOS, which is consistent within a platform but not across
   them, which you asked against.
6. **Certificates and accounts.** A Windows code-signing certificate, an Apple
   Developer account and a Google Play account are yours to obtain. Until
   then, unsigned builds. When do you expect to have them?
7. **Desktop updates.** *Recommendation:* the app updates itself from GitHub
   releases (signed manifests). Stores handle mobile.
8. **Languages.** *Recommendation:* every string externalised from U1, English
   only at first.
