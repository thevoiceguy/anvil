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

1. PBX sends push notification to APNs (needs a server-side relay — the SIP stack
   itself can't receive APNs).
2. APNs delivers VoIP push; iOS wakes the app.
3. App **must** report the incoming call to `CXProvider` within ~5 seconds or
   iOS will terminate it and future pushes may be throttled.
4. App opens the SIP socket, expects the INVITE (or has already received it via
   a warm push payload).
5. User taps accept in CallKit UI → Swift calls `anvil_answer(call_id)`.
6. CallKit activates the audio session; Anvil starts RTP I/O.

### Flow for an outgoing call

1. UI calls `anvil_place_call(target)` → Swift calls `CXProvider.reportNewOutgoing`.
2. CallKit activates audio session, Anvil starts INVITE + RTP.

### Required plist entries

```xml
<key>UIBackgroundModes</key>
<array>
    <string>voip</string>
    <string>audio</string>
</array>
<key>NSMicrophoneUsageDescription</key>
<string>Anvil needs the microphone for calls.</string>
```

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

1. PBX → app backend → FCM high-priority push.
2. Android delivers push; app has ~10 s to either show a notification or start
   a foreground service. If we miss the window we get throttled.
3. Service brings Rust up, SIP INVITE lands.
4. `ConnectionService.onCreateIncomingConnection` returns an Anvil `Connection`.
5. Telecom shows the system call UI; user taps accept → Kotlin calls
   `anvilAnswer(callId)`.

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
   the new binding. Later phases (A2 Android, A3 iOS) do this part.

`anvil-cli --push fcm:<project>:<token>` registers a push address by hand.
