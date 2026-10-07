# The Anvil app

Designed 2026-10-07; agreed the same day on every recommendation in §8 (Flutter, `app/` in this repository, the desktop app serving the control socket, desktop first, one design everywhere, self-updating desktop builds, strings externalised from U1). Certificates and store accounts (§8 Q6) are the owner's to obtain; builds stay unsigned until then. Progress: U0 done: `anvil-app` (the running phone, its state and changes, the commands; mute added to `anvil-core`), the control socket, and `anvil-cli run` with the commands that drive it.

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

### U3: desktop distribution
- Installers for the three desktops built on a tag, the auto-updater, signing
  as soon as the certificates exist.

### U4: the app on mobile
- iOS and Android builds; CallKit and ConnectionService for the call screen;
  the audio session, Bluetooth and the proximity sensor.
- Works while the app is open.

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
