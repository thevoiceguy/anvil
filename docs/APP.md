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

- **Sign in.** Server address or email address (FCP's discovery) and the
  username first; FCP's `/auth/discover` then says how that person signs
  in: a password (with a small "Use single sign-on" link when the tenant
  has an identity provider too), or straight to the system browser when
  the tenant signs in only by single sign-on. An FCP without the route gets
  the password and the link. A one-time code is a second step, shown only
  when FCP asks for one. The last server and username are remembered
  beside the session (`last_sign_in.json`, never a password) and fill the
  form in after signing out. The install becomes a device of the user's,
  as it does today.
- **Keypad.** Dial a number, an extension or a name. Each key shows its
  letters (2 ABC … 9 WXYZ), and a number written with them is dialled as
  its digits (`800555FOOD` → `8005553663`, separators dropped); a name
  such as `alice` is dialled as it is.
- **In a call.** Mute, hold, keypad (DTMF), speaker or headset, blind and
  attended transfer, park, add a second call and swap, end. Call quality and
  encryption shown. On a desktop, **Audio** opens the microphone and the
  speaker and their volumes (0–200 %, 100 the sound as it is): the call
  moves to a device chosen at once (D8 findings 18 and 19). **Record**, when
  FCP lets the user (`may_record` on `/me/softphone`, an administrator's
  "May record calls"): FCP plays the recording announcement to both parties,
  then records; **Stop recording** stops only what the user started
  (`/me/calls/{id}/recording/{start,stop}`, by the id FCP named the call
  with, `X-FCP-Call-Id`; FCP's `docs/RECORD_AND_CONTACTS.md`, D8 finding 20).
  **More** opens a menu built from a list of actions, so a feature added
  later is one more entry; today it holds **Create contact** (D8 finding 21).
- **Incoming.** Answer, decline, send to voicemail, the caller's name from the
  directory. A second call while one is up.
- **Recents.** History, missed calls marked, call back. Clear them all, or
  remove one; this hides them on this device only (`settings.json`), as FCP
  keeps the call records. A number not yet a contact can be made one from
  the row's menu.
- **Contacts.** Two tabs under one search: the user's own contacts (**My
  contacts**, first when there are any) and the tenant's **Directory**
  with presence and busy lamps, favourites first; the tab chosen is kept
  while the app runs, and a contact just made is shown in My contacts. A
  contact has a name, a company, numbers with labels, an email and notes.
  A label is picked from a list (Mobile, Work, Home, Main, Work fax, Other;
  kept as `mobile`, `work`, … so FCP's portal shows the same) or typed
  after **Custom…**; a new contact's first number is Mobile, another Work.
  A contact with one number is dialled by a tap; one with more opens to a
  row for each number with its label and its own call button (D8 findings
  23, 26, 28). Signed in
  to FCP, a new contact goes on the user's account (FCP's `/me/contacts`,
  v0.25.467: every app they sign in to has it, and so does their site;
  `docs/RECORD_AND_CONTACTS.md` §3) unless they pick **This device**; each
  contact is marked with where it is kept, and one on the device can be
  moved to the account from its menu (FCP takes it, then the device's copy
  goes). With any other account, contacts are kept on this device
  (`settings.json`). Made by hand, from a call or from a recent call,
  changed and deleted from its menu. A call or a recent call from one of its
  numbers shows its name; the match is made by the app, so FCP's caller ID
  is never rewritten. A change made in another app or the portal arrives
  live (FCP's `user.contact_changed`); the account's contacts are also
  read when the phone starts, when its events reconnect, and on
  `refresh`. The CLI's `contact add account
  <number> <name>` puts one on the account. A contact may have a ringtone
  of its own, which a call from one of its numbers rings with; it is kept
  with the contact (on the account too), and a device without that
  ringtone (a file added on another device) rings with its own. The
  People page's import and export menu reads a vCard or CSV file into the
  account or this device (one already kept, by name and numbers, is not
  made again) and, on a desktop, writes every contact to one (D8 findings
  29, 31; `contact import [account] <file>`, `contact export <file>`).
- **Voicemail.** Messages with transcription, play, mark heard, delete, the
  count on the tab.
- **Settings.** Do not disturb, forwarding, call waiting, whether the user
  is listed in the tenant's directory (changed only when the tenant lets
  its users choose; FCP v0.25.471, D8 finding 27; `directory on|off`),
  audio devices and ringtone, start at login (desktop), sign out.
- **Desktop.** A tray icon with presence and DND, a notification for an
  incoming call that answers from the notification, a compact in-call window,
  a global answer/hang-up shortcut.

## 5. Platforms and distribution

| | Package | Signing | Needs |
|---|---|---|---|
| Windows | an installer for the user (Inno Setup, `app/packaging/windows.iss`) | a code-signing certificate | a certificate (EV or OV) |
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
  `park` (the call held and FCP's park code dialled, as a desk phone parks, so the orbit is heard; the call ends "parked on <orbit>" when FCP's answer names it in `X-FCP-Parked`), `heard`, `delete_voicemail`, `favourite`,
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

### Windows: W1, one app
- One Anvil per user session (`app/windows/runner/instance.cpp`): a second
  start — the Start menu while the app is in the tray, a link, start at
  login after a manual start — hands its arguments to the running app over
  `WM_COPYDATA` and leaves, so one device's credentials never register
  twice. The running app shows its window, or puts the link's number on
  the keypad (`lib/src/desktop/links.dart`).
- Links to call: the installer registers `Anvil.Url` for `tel:`, `sip:`,
  `sips:` and `callto:` and offers Anvil for each in Windows' default
  apps; `sip:`, `sips:` and `callto:` are Anvil's outright where nothing
  else opens them. A link's number goes on the keypad, over a call up if
  there is one, and is never called by itself: a web page can open a link
  without the user meaning to call.
- Start at login starts the app in the tray (`--hidden`; the runner leaves
  the window unshown, and shows it anyway if there is no tray). An entry an
  older version wrote is rewritten.
- Signing out of Windows or shutting down signs the phone out first: the
  runner answers `WM_ENDSESSION` by asking the app to stop the phone (it
  unregisters) and waits up to four seconds.
- The installer gives the shortcuts the notifications' app id and the
  uninstaller removes what the notifications registered.
- Linux still starts a second phone on a second start, and has no links.

### Windows: W2, secrets and the control pipe
- The session (its tokens and the device's SIP password) is sealed with
  DPAPI for the user (`anvil-fcp`'s `FileTokenStore`), so a copied file
  opens on no other account or machine; a file an older version kept in
  the clear is read and sealed. Unix keeps it at mode 0600, as before.
- The app's files live in the local profile (`%LOCALAPPDATA%\Anvil\Anvil`),
  not the roaming one that follows the user to every machine of a domain;
  an older version's are moved. The uninstaller removes them.
- The control pipe admits its own user only (a protected DACL naming the
  user's SID), and a client talks to it only when its server runs as the
  same user (`GetNamedPipeServerProcessId`), so another user of the
  machine can neither drive the phone nor pose as it; a pipe name another
  user took is reported as such.
- `anvil-cli` keeps its session in `%LOCALAPPDATA%\anvil` (it used the
  folder it ran in), and opens the sign-in page with `explorer`
  (`cmd /C start` cut the address at its first `&`). A session an older
  CLI left in some folder is not found: `anvil-cli login` again.
- A Windows device is named after the computer (`COMPUTERNAME`); it was
  "Anvil on this computer".
- CI lints the workspace on Windows too.
- Not done: the setting to turn the control socket off (§8).

### Windows: W3, updates that land
- The app updating itself started the setup at once and quit, racing it:
  the setup leaned on the Restart Manager to close a tray app that may not
  have gone yet, and a silent setup that met files in use failed with no
  one told. Now a hidden PowerShell waits for the app's process to go
  (thirty seconds, then ends it), and only then runs the setup silently
  with `/relaunch=1` (`crates/anvil-update/src/install.rs`).
- The setup has `AppMutex=Anvil.Softphone` (the running app's mutex, W1):
  an install by hand over a running Anvil asks for it to be closed.
- Proved: on Windows, a unit test runs the real script against a stand-in
  setup; the release workflow updates over a running app the same way and
  sees the app started again.

### W4a: trust and sign-in (every desktop)
- A desktop trusts the system's certificates as well as the bundled
  public roots: a company's own CA, or a proxy that inspects TLS, no
  longer fails every connection — FCP's API, its events, branding,
  updates, and SIP over TLS (`rustls-native-certs` beside `webpki-roots`;
  a store that cannot be read is skipped). Android and iOS keep the
  bundled roots.
- The app signs in through the system browser (`Sign in with your
  browser`), as `anvil-cli login` already did: whatever the server's
  sign-in page asks — single sign-on, a second factor — with PKCE and a
  loopback listener (`docs/APP_SIGN_IN.md` in FCP); waiting can be given
  up. Desktop only: a phone's app may be suspended while its browser is
  in front, so the loopback would not answer.

### W4b: audio devices
- On Windows a call uses the default device for communications — the one a
  headset user sets as "Default Communication Device" — not the default
  for everything, which is all cpal knows (`anvil-audio/src/comms.rs`,
  `IMMDeviceEnumerator` with `eCommunications`); the device list marks it
  as the default.
- A call's capture and playback survive their device: a stream that fails
  (a headset unplugged) is opened again on the default, at once rather than
  never; a stream on the default moves when the default changes (a headset
  plugged in, a new default chosen), and back to the device the user chose
  when it returns; checked every two seconds.
- Calls share one speaker stream: each call is a voice in it, mixed, and it
  stays open fifteen seconds after the last call ends (at once when another
  speaker is chosen). A network speaker or a Bluetooth headset wakes for a
  second or two on a new stream: a second call opening its own (the park
  code dialled from a held call) lost its first words, the whole of "Parked
  on 7 0 1" on a Sonos (FCP's D8).
- Not proved by a test: CI's runners have no audio devices to unplug.

### W4c: a ringtone of the app's own
- A call ringing in rings on the desktop, from the Rust core
  (`anvil-app/src/ring.rs`), not from the notification's sound: on the
  system's default device for sounds — on Windows the speakers, not the
  headset calls use — or the one chosen under Settings → Ring on
  (`audio ring <device>` from the CLI). A phone does not: its system call
  screen rings (`PhoneConfig::ring`).
- The ringtones are Anvil's two (a chime, `anvil`, and an electronic
  phone's warble, `classic`) and the brand's (`docs/BRANDING.md`): WAV,
  Ogg Vorbis or MP3 as FCP takes them, decoded once when the brand
  arrives, the first 30 seconds kept; one that cannot be decoded is
  logged and left out. Unchosen, the brand's default rings, else Anvil's.
  Choosing one plays it once (not while a call is up); the choice and the
  ring device are kept on this device.
- A second call ringing in while one is up (or being dialled) does not
  ring over it: two short beeps at 440 Hz every ten seconds in the call's
  own speaker, as a desk phone's call waiting tone.
- Ringing stops at once on answer, and when the call is declined,
  cancelled or answered elsewhere. Decisions run one at a time, so one
  made on an older state cannot start a ring after a newer one stopped it.
- Proved by `anvil-app/tests/phone.rs` (a call rings on the ringing
  speaker until answered, a second is a tone in the call until it goes,
  the ringtone and device kept) and the decoders' tests on real Ogg and
  MP3 files.

### D8 findings 24, 25, 30: audio tests and a ringtone of the user's own
- Settings → Test your audio (a desktop, whose audio host has volumes of
  its own): **Test microphone** records five seconds as a call would send
  it (the microphone's volume, its cleaning), its level shown as a meter
  in decibels from 60 below full scale, then plays it back on the speaker
  calls use at the speaker's volume; **Test speaker** plays Anvil's chime
  there; **Test ringing** plays the ringtone once on the speaker that
  rings. One test at a time, refused while a call is up or ringing, and a
  call ringing in stops it. From the CLI: `test mic`, `test speaker`,
  `test ring`, `test stop`.
- **Choose a file…** in the ringtone list takes a WAV, Ogg or MP3 file (20
  MB at most), decoded first so one that cannot play is refused, then
  copied into `ringtones/` beside the settings and chosen
  (`ringtone file <path>`). It is listed as `file:` and its name, read
  again at each start, and removed with the bin beside the list
  (`ringtone remove <id>`), Anvil's own ringing again if it was chosen.
- Proved by `anvil-app/tests/phone.rs`
  (`the_microphone_is_heard_back_and_the_speakers_tested`,
  `a_ringtone_from_a_file_rings_is_kept_and_can_be_removed`) and the
  widget tests.

### D8 findings 27, 29, 31: the directory, contact files, a contact's ringtone
- Settings → **Listed in the directory**, from `/me/softphone`'s
  `directory_listed` and `may_change_directory_listing` (read at start and
  on `refresh`), set through `PUT /me/profile`; greyed out, saying the
  administrator decides, unless the tenant lets its users choose.
- Contacts files: an import to the account is FCP's
  `POST /me/contacts/import` (FCP reads the file and skips what the
  account has); to this device, the app reads the file itself with the
  same reader (`anvil-app/src/contact_file.rs`, FCP's
  `fcp_core::contact_file`): vCard 3.0 and Apple's labels, Outlook's and
  Google's CSV, 10 MB at most. An export writes the contacts shown, the
  device's and the account's, as vCard, or CSV when the file's name ends
  `.csv`. A phone's system offers no place to save to, so export is a
  desktop's.
- A contact's ringtone is chosen in its dialog from the phone's
  ringtones; one chosen on another device is kept and shown as such.
  Ringing picks the ringtone of the contact one of whose numbers the
  ringing call is from, while this device has it.
- Proved by `anvil-app/tests/phone.rs`
  (`a_contacts_own_ringtone_rings_for_their_calls`,
  `contacts_are_read_from_and_written_to_files`), `tests/fcp.rs`
  (`contacts_are_imported_to_the_account_with_their_ringtones_kept`, which
  skips on an FCP before v0.25.471) and the widget tests.

### W5: echo cancellation and noise suppression
- The desktop's microphone is cleaned before it is sent: WebRTC's audio
  processing — AEC3, its noise suppressor and its high-pass filter — as
  `sonora` ports it to Rust, so nothing C++ is built on any platform
  (`anvil-audio/src/echo.rs`, the `apm` feature, on by default).
- The canceller hears what the speaker plays: each call's speaker stream
  queues what it is given, and the microphone takes the same length of it
  for each frame it cleans, so the reference comes at the microphone's
  pace however the network delivered it. AEC3 finds the delay through the
  speaker's buffer and the room itself. A call waiting tone is mixed in;
  the ringer is not (ringing stops at answer). With a held call and one
  up, only the newest call's canceller takes the reference.
- Both are on unless turned off, under Settings → Audio (or
  `audio echo on|off` and `audio noise on|off` from the CLI), kept on
  this device, and a change reaches the call that is up. A host that does
  no cleaning of its own (a phone's platform audio) shows neither and
  refuses the command.
- Measured by `echo.rs`'s tests on a room with 60 ms between speaker and
  microphone: the echo down by more than 60 dB after convergence, a
  person speaking in the far end's pauses down by under 1 dB, steady
  noise down by about 12 dB; about 1.5% of one core at 48 kHz. Proved
  through the phone by `anvil-app/tests/phone.rs` (the setting reaches the
  host, is kept for the next start, and is refused where there is none).
  How it sounds on real speakers and microphones is for the testing
  session.

### U3: desktop distribution
- Installers for the three desktops built on a tag, the auto-updater, signing
  as soon as the certificates exist.

Three PRs:

- **U3a, installers on a tag.** `release.yml` builds, on a tag `vX.Y.Z`
  (the workspace's version, which also names the app's build):
  - Windows: an Inno Setup installer (`app/packaging/windows.iss`) for this
    user only, without administrator rights, in
    `%LOCALAPPDATA%\Programs\Anvil`, with a Start menu entry and an
    uninstaller that also removes start at login. The uninstaller first runs
    `anvil.exe --sign-out`, which signs the app out at FCP without a window
    (ten seconds at most), so its session and device end there instead of
    staying behind once the session's file is deleted. The Visual C++ runtime
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
- **U4b, the system's call screen.**
  - The app's calls are the system's calls: CallKit on iOS, a self-managed
    `ConnectionService` with Telecom on Android. A call ringing in rings
    there (the lock screen too), a call placed shows as dialling, and
    connected, held and over follow the phone (`lib/src/mobile/bridge.dart`).
  - What the user does there reaches the phone: answer, end (a call ringing
    in is declined), hold, mute, a key.
  - A call the system filters (Do Not Disturb, a blocked caller) is
    declined; one it cannot show at all still rings in the app.
  - Android: a foreground service runs while there is a call, so the call
    keeps its microphone when the user leaves the app, and its call-style
    notification rings with Answer and Decline, then shows Hang up. The app
    comes over the lock screen only while there is a call. Android 8.0 or
    later (self-managed calls need it).
  - iOS: the system's Recents leave the app's calls out (the app keeps its
    own history).
- **U4c, a call's audio.**
  - Where the call's audio goes, as the system has it: Telecom's call
    endpoints (Android 14 on; the call audio state before) for the app's
    connection, the audio session's route on iOS. Earpiece, speaker,
    Bluetooth (by the device's name where the system gives it: always on
    Android 14, on iOS from the port), a wired headset.
  - In the call: a speaker switch when the earpiece and the speaker are all
    there is; with Bluetooth or a headset too, an Audio button showing where
    the sound is, opening the list to choose from. Settings keeps its
    microphone and speaker lists for the desktop only.
  - The proximity sensor turns the screen off at the ear while a call is up
    or being placed on the earpiece (not ringing, not on the speaker or a
    headset): a proximity wake lock on Android (released once the phone has
    left the ear, four hours at most), proximity monitoring on iOS.

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
