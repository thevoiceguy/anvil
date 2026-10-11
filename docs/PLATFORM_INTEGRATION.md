# Anvil — platform integration notes

What each of the five target platforms needs from the integration shim that sits
between the Rust core and the UI. The Rust core itself is identical everywhere;
everything below lives in the platform-native wrapper that consumes `anvil-ffi`.

---

## Linux

- **Audio:** ALSA or PulseAudio via `cpal`. PipeWire exposes both interfaces.
- **Build:** `cargo build --release -p anvil-cli` produces a normal ELF binary.
- **Packaging:** Flatpak or AppImage for distros. Deb/RPM if we ship a headed app.
- **Gotcha:** PulseAudio + cpal under Wayland sometimes picks the wrong default
  device. Enumerate + let the user pick in settings.

## macOS

- **Audio:** CoreAudio via `cpal`. Request mic entitlement
  (`NSMicrophoneUsageDescription`).
- **Build:** universal binary `lipo`'d from `x86_64-apple-darwin` +
  `aarch64-apple-darwin`.
- **Signing/notarization:** required for Gatekeeper-friendly distribution.
- **Gotcha:** the hardened runtime + notary service requires the `com.apple.security.
  device.audio-input` entitlement in the plist, not just the Info.plist key.

## Windows

- **Audio:** WASAPI via `cpal`. `Windows.Media.Devices` for enumeration if we want
  system-level default-device change events.
- **Build:** `x86_64-pc-windows-msvc` target. MSVC toolchain (not MinGW).
- **Packaging:** MSIX preferred; plain `.msi` via WiX as fallback.
- **Gotcha:** Windows firewall prompts on first bind to 0.0.0.0 — consider
  binding to a specific local address if the user has picked one.

---

## iOS

This is the hardest platform. CallKit is **mandatory** for VoIP apps; Apple rejects
apps that roll their own incoming-call UI.

### Core pieces

| Concern            | Apple framework              | Notes                                           |
|--------------------|------------------------------|-------------------------------------------------|
| System call UI     | `CallKit`                    | Report incoming calls via `CXProvider`          |
| Background wake    | `PushKit` (VoIP push via APNs) | Process wakes, has seconds to report to CallKit |
| Audio graph        | `AVAudioEngine` + `AVAudioSession` | Voice-processing mode provides AEC for free |
| Audio interruption | `AVAudioSession.interruptionNotification` | Phone calls, Siri, etc.              |

### Flow for an incoming call

1. FCP pushes to APNs itself (its `docs/PUSH.md`; no relay), a VoIP push
   naming the call.
2. APNs delivers it; iOS wakes the app, in the background if it slept.
3. The app reports the call to `CXProvider` at once, from the payload —
   every VoIP push must report a call, or iOS kills the app and stops
   delivering them — and starts the phone from its saved session.
4. The phone registers; FCP sends the INVITE, naming the call in
   `X-FCP-Call-Id`, and it takes the place of the call on the screen.
5. Answered at CallKit → `CallSystem` → `PhoneLink.act` → the Rust core.
6. CallKit activates the audio session (a call's, set up when the answer
   is performed); the phone's audio runs.

### Flow for an outgoing call

1. The app places the call on the phone; the phone's event reaches
   `CallSystem`, which asks CallKit (`CXStartCallAction`).
2. CallKit activates the audio session; the phone's INVITE and RTP run.

### Required plist entries

```xml
<key>UIBackgroundModes</key>
<array>
    <string>voip</string>
</array>
<key>NSMicrophoneUsageDescription</key>
<string>Anvil uses the microphone so the people you call can hear you.</string>
<key>AnvilApnsTeam</key>
<string>$(DEVELOPMENT_TEAM)</string>
```

and `aps-environment` in `Runner.entitlements`.

### Gotchas

- CallKit is disallowed in mainland China — ship a region-gated fallback if
  that market matters.
- `AVAudioSession` must be set to `.playAndRecord` with the
  `.voiceChat` mode (or `.videoChat`) to enable Apple's AEC. Do **not** run our
  own AEC on top of that; it cancels itself.
- Apple's AEC only works on the built-in mic/speaker path; headset + Bluetooth
  routing can bypass it. Surface this to the user.

---

## Android

Android is messier than iOS because the device landscape is more varied and
background restrictions tighten every release.

### Core pieces

| Concern              | Android framework                   | Notes                                      |
|----------------------|-------------------------------------|--------------------------------------------|
| System call UI       | `ConnectionService` / Telecom       | Adds Anvil as a "calling account"          |
| Background wake      | FCM high-priority messages          | Wake app to receive INVITE                 |
| Foreground keep-alive| `ForegroundService` (type `phoneCall`) | Required while call is active           |
| Audio                | Oboe (preferred) or AudioRecord/AudioTrack | Low-latency; voice-comm preset        |
| AEC                  | `AcousticEchoCanceler` effect       | Not universally available; test per device |

### Flow for an incoming call

1. FCP sends an FCM high-priority data message straight to the device
   (see "Push" below); no app backend sits between.
2. `AnvilMessagingService` hands it to `Push.handle`, which adds the call
   to Telecom and starts the call's service at once, inside the window a
   high-priority message gives.
3. `PhoneLink.wake` starts the phone in the Rust core from the saved
   session (or registers again if it runs); the INVITE lands and takes the
   ringing call's place.
4. Answer or Decline at the call's notification, or at Telecom, goes to the
   phone through `PhoneLink.act`, Flutter running or not.

An app awake rings the same way from step 3: the phone's calls reach
Telecom from the Rust core (`app/rust/src/system.rs`), never from Dart.

### Flow for an outgoing call

1. UI builds a `PhoneAccountHandle`, calls `TelecomManager.placeCall`.
2. `ConnectionService.onCreateOutgoingConnection` returns our `Connection`.
3. Connection calls `anvilPlaceCall`.

### Manifest essentials

```xml
<uses-permission android:name="android.permission.INTERNET"/>
<uses-permission android:name="android.permission.RECORD_AUDIO"/>
<uses-permission android:name="android.permission.MANAGE_OWN_CALLS"/>
<uses-permission android:name="android.permission.FOREGROUND_SERVICE"/>
<uses-permission android:name="android.permission.FOREGROUND_SERVICE_PHONE_CALL"/>

<service android:name=".AnvilConnectionService"
         android:permission="android.permission.BIND_TELECOM_CONNECTION_SERVICE"
         android:foregroundServiceType="phoneCall">
    <intent-filter>
        <action android:name="android.telecom.ConnectionService"/>
    </intent-filter>
</service>
```

### Gotchas

- OEM skins (MIUI, ColorOS, One UI) aggressively kill background apps. Document a
  "battery optimisation exemption" path for users.
- `MANAGE_OWN_CALLS` is enough for most cases; `READ_PHONE_STATE` is **not**
  required and asking for it will scare users during review.
- FCM data-only messages may be de-prioritised; use `priority: "high"` and send a
  `notification` payload as a visible fallback.
- Oboe + the `VOICE_COMMUNICATION` preset gives us the hardware AEC on most
  modern devices. Still run software AEC as a fallback when the effect isn't
  supported — detect with `AcousticEchoCanceler.isAvailable()`.

---

## Audio backend matrix

| Platform | Backend (via cpal)     | Voice-comm mode / AEC source                 |
|----------|------------------------|----------------------------------------------|
| Linux    | ALSA (PipeWire/Pulse)  | software APM in `anvil-audio` (`apm` feature) |
| macOS    | CoreAudio              | software APM (voice-processing IO unit later) |
| Windows  | WASAPI                 | software APM                                 |
| iOS      | (host-owned, not cpal) | AVAudioSession `.voiceChat` mode (hardware)  |
| Android  | (host-owned, not cpal) | Oboe + `VOICE_COMMUNICATION` preset          |

Software APM = `webrtc-audio-processing` wrapped in `anvil-audio`.

---

## Push: how the server wakes the app

FCP pushes to the app itself (RFC 8599; FCP's `docs/PUSH.md`): it holds the
publisher's APNs key and FCM service account, and no relay sits between it
and the device. Anvil's part is to say where it is woken:

1. **The push address in every REGISTER.** `AccountConfig::push` (or
   `Anvil::set_push` when the token arrives or is renewed, `Command::Push`
   in `anvil-app`) puts `;pn-provider=…;pn-param=…;pn-prid=…` inside the
   Contact's brackets: `apns` with `<team id>.<topic>` and the PushKit VoIP
   token on iOS, `fcm` with the Firebase project id and the FCM token on
   Android. A new token is registered at once.
2. **The install's instance.** `AccountConfig::instance_id`
   (`<urn:uuid:…>`, RFC 5626 `+sip.instance` with `reg-id`): signed in to
   FCP it is made from the install id, the same on every start, so the
   woken app's REGISTER replaces its sleeping binding rather than adding
   another.
3. **555.** A server that cannot push to the app answers 555 Push
   Notification Service Not Supported, naming in `Feature-Caps` what it
   pushes through. Anvil registers again without the address and says so
   (`Event::PushChanged` → `PushState::Refused`); the app rings only while
   it is awake. A 2xx to a REGISTER carrying the address is
   `PushState::Accepted`.
4. **Woken.** The push's payload names the call; the app reports it to
   CallKit or Telecom at once, starts the phone and registers; FCP rings
   the new binding, naming the call in `X-FCP-Call-Id` as the push did
   (`Event::IncomingCall::server_call`), so the app knows which INVITE is
   the call on the screen. A cancel push ends a call whose INVITE never
   came; one with no INVITE in a minute ends by itself.

### Android (phase A2)

- **The token.** `Push.register` asks Firebase for the install's FCM
  token at each start of the app's screen, and `onNewToken` hears a
  renewed one; `PhoneLink.setPush` keeps it in `push.json` beside the
  session (`app/rust/src/system.rs`), which every start of the phone reads,
  and registers it at once if the phone runs.
- **The Firebase project** is the publisher's and is given at build time,
  not kept in the repository: `ANVIL_FCM_APP_ID`, `ANVIL_FCM_SENDER_ID`,
  `ANVIL_FCM_API_KEY` and `ANVIL_FCM_PROJECT_ID` in the environment (or
  `-P`), the values a `google-services.json` would hold. Built without
  them, the app has no token and registers without a push address.
- **The phone without Flutter.** Kotlin reaches the Rust core through JNI
  (`AnvilNative.wake`, `setPush`, `nextCallEvent`, `act`;
  `app/rust/src/android.rs`). The phone's calls come to `CallSystem` as
  events, whether Dart or a push started the phone, and Dart's
  `MobileBridge` leaves Telecom alone on Android.
- **A call shown from a push** has an id below zero until the phone's
  call arrives; answered before then, it is answered when it does;
  declined, its INVITE is declined when it comes.
- **Tested** on the emulator (`integration_test/mobile_test.dart`) with
  pushes handed in as FCM would (`deliverPush`, debug builds only): a
  signed-out app rings nothing, a call push rings and stops when the phone
  cannot wake, a cancel push ends it. Real delivery needs the publisher's
  Firebase project and a device with Google Play services.

### iOS (phase A3)

- **The token.** `Push` (a `PKPushRegistryDelegate`, made at launch as
  PushKit requires) hears the install's VoIP token and its renewal;
  `PhoneLink.setPush` keeps it in `push.json` beside the session, which
  every start of the phone reads, and registers it at once if the phone
  runs. An invalidated token is forgotten.
- **The topic** is the app's bundle ID with `.voip`, and `pn-param` is the
  signing team's ID, a dot, the topic — the team from `DEVELOPMENT_TEAM`
  through Info.plist's `AnvilApnsTeam`. An unsigned build (CI's) has no
  team and registers without a push address. The Push Notifications
  capability is `aps-environment` in `Runner.entitlements`.
- **Every push reports a call** (Apple's rule): a call push rings; a
  cancel push reports the call it ends, or one ended at once; a test push,
  or one for an app signed out, is shown and ended at once.
- **The phone without Flutter.** Swift reaches the Rust core through a C
  ABI (`anvil_wake`, `anvil_set_push`, `anvil_next_call_event`,
  `anvil_act`, `anvil_string_free`; `app/rust/src/ios.rs`, declared in
  `Runner-Bridging-Header.h`). The phone's calls come to `CallSystem` as
  the same events as Android's, whether Dart or a push started the phone,
  and Dart's `MobileBridge` leaves CallKit alone.
- **A call shown from a push** has an id below zero until the phone's
  call arrives, matched by `X-FCP-Call-Id`; answered before then, it is
  answered when it does; declined, its INVITE is declined when it comes;
  the INVITE's caller replaces a minimal payload's blank one.
- **Tested** on the simulator (`integration_test/mobile_test.dart`, the
  same tests as Android's) with pushes handed in as PushKit would
  (`deliverPush`, debug builds only); the C ABI by the Rust crate's own
  tests. Real delivery needs the publisher's Apple Developer account, an
  APNs key in FCP's `[[push.apns]]`, and a device.

`anvil-cli --push fcm:<project>:<token>` registers a push address by hand.
