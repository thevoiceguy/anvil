//! Two phones calling each other with no server: one driven only through its
//! control socket, as `anvil-cli call …` drives a running phone.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anvil_app::control::{Client, Endpoint};
use anvil_app::{
    AudioKind, AudioProcessing, AudioVolume, CallState, Change, Command, Phone, PhoneConfig,
};
use anvil_core::audio::{
    AudioFormat, AudioFrame, AudioHost, AudioSink, AudioSource, DeviceInfo, Processing, Volume,
};
use anvil_core::config::{AccountConfig, MediaConfig};
use anvil_core::{AnvilConfig, AnvilError, BrandConfig, Transport};
use async_trait::async_trait;

const SOON: Duration = Duration::from_secs(5);

struct Quiet;
struct QuietSource(AudioFormat, tokio::time::Interval);
struct QuietSink;

#[async_trait]
impl AudioSource for QuietSource {
    async fn next_frame(&mut self) -> Option<AudioFrame> {
        self.1.tick().await;
        let n = (self.0.sample_rate * self.0.frame_ms / 1000) as usize;
        Some(AudioFrame {
            samples: vec![0; n],
            format: self.0,
        })
    }
}

#[async_trait]
impl AudioSink for QuietSink {
    async fn write_frame(&mut self, _: &AudioFrame) -> Result<(), AnvilError> {
        Ok(())
    }
}

impl AudioHost for Quiet {
    fn make_capture(&self, cfg: AudioFormat) -> Result<Box<dyn AudioSource>, AnvilError> {
        Ok(Box::new(QuietSource(
            cfg,
            tokio::time::interval(Duration::from_millis(u64::from(cfg.frame_ms))),
        )))
    }
    fn make_playback(&self, _: AudioFormat) -> Result<Box<dyn AudioSink>, AnvilError> {
        Ok(Box::new(QuietSink))
    }
    fn devices(&self) -> Vec<DeviceInfo> {
        Vec::new()
    }
}

async fn phone(user: &str, ip: &str) -> Phone {
    phone_with(user, ip, Box::new(Quiet), None).await
}

async fn phone_with(
    user: &str,
    ip: &str,
    audio: Box<dyn AudioHost>,
    settings_path: Option<PathBuf>,
) -> Phone {
    start(user, ip, audio, settings_path, false, None).await
}

async fn start(
    user: &str,
    ip: &str,
    audio: Box<dyn AudioHost>,
    settings_path: Option<PathBuf>,
    ring: bool,
    fallback_brand: Option<anvil_core::brand::BrandProfile>,
) -> Phone {
    Phone::start(PhoneConfig {
        anvil: AnvilConfig {
            account: AccountConfig {
                aor: format!("sip:{user}@{ip}"),
                registrar: format!("sip:{ip}"),
                username: user.into(),
                password: "x".into(),
                transport: Transport::Udp,
                outbound_proxy: None,
                stun: None,
                register_expires: Duration::from_secs(3600),
                user_agent: "anvil-app-test".into(),
                bind_addr: Some(format!("{ip}:0")),
                tls_extra_ca_pem: None,
                provisioning_url: None,
                instance_id: None,
                push: None,
            },
            media: MediaConfig::default(),
            audio,
            brand: BrandConfig::default(),
        },
        fcp: None,
        register: false,
        settings_path,
        ring,
        fallback_brand,
    })
    .await
    .expect("the phone starts")
}

/// A control endpoint of this test's own.
fn endpoint(tag: &str) -> Endpoint {
    if cfg!(windows) {
        Endpoint::Pipe(format!(r"\\.\pipe\anvil-test-{tag}-{}", std::process::id()))
    } else {
        Endpoint::Unix(
            std::env::temp_dir()
                .join(format!("anvil-test-{tag}-{}", std::process::id()))
                .join("control.sock"),
        )
    }
}

/// The phones' SIP and call log, captured by the test harness and shown
/// only when the test fails (`ANVIL_LOG` replaces the filter).
fn log() {
    let filter = std::env::var("ANVIL_LOG").unwrap_or_else(|_| {
        "anvil_core=debug,anvil_app=debug,sip_uac=debug,sip_uas=debug,\
         sip_transaction=debug,sip_transport=info"
            .into()
    });
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
        .with_test_writer()
        .try_init();
}

async fn until(phone: &Phone, what: &str, done: impl Fn(&anvil_app::State) -> bool) {
    let deadline = tokio::time::Instant::now() + SOON;
    while !done(&phone.state()) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "{what}: {:?}",
            phone.state()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_running_phone_is_driven_through_its_control_socket() {
    log();
    let alice = phone("alice", "127.0.0.30").await;
    let bob = phone("bob", "127.0.0.31").await;
    let at = endpoint("drive");
    let server = tokio::spawn(anvil_app::control::serve(alice.clone(), at.clone()));
    tokio::time::sleep(Duration::from_millis(100)).await;

    // A second phone at the same endpoint is refused, not let in.
    assert!(matches!(
        anvil_app::control::serve(alice.clone(), at.clone()).await,
        Err(anvil_app::control::ControlError::AlreadyRunning(_))
    ));

    let mut control = Client::connect(&at).await.expect("connects");
    let mut watcher = Client::connect(&at).await.unwrap();
    watcher.subscribe().await.unwrap();

    // `call`: Bob rings; Bob answers; both connected.
    let placed = control
        .request(&Command::Call {
            target: format!("sip:bob@{}", bob.sip_address()),
        })
        .await
        .unwrap();
    assert!(placed.ok, "{placed:?}");
    let call = placed.call.expect("the call's id");
    until(&bob, "Bob rings", |s| {
        s.calls.iter().any(|c| c.state == CallState::Ringing)
    })
    .await;
    assert!(
        bob.execute(Command::Answer { call: None })
            .await
            .unwrap()
            .ok
    );
    until(&alice, "Alice's call is up", |s| {
        s.call_state(call) == Some(CallState::Connected)
    })
    .await;

    // `mute`, `hold`, `resume`, `dtmf`: each answered, each in the state.
    for (cmd, check) in [
        (Command::Mute { call: None }, "muted"),
        (Command::Hold { call: None }, "held"),
        (Command::Resume { call: None }, "resumed"),
        (
            Command::Dtmf {
                digits: "12#".into(),
                call: None,
            },
            "dtmf",
        ),
    ] {
        let answer = control.request(&cmd).await.unwrap();
        assert!(answer.ok, "{check}: {answer:?}");
    }
    let status = control.request(&Command::Status).await.unwrap();
    let view = status
        .state
        .unwrap()
        .calls
        .into_iter()
        .find(|c| c.id == call)
        .unwrap();
    assert!(view.muted && !view.held, "{view:?}");

    // A command that cannot be done says why.
    let refused = control
        .request(&Command::Answer { call: None })
        .await
        .unwrap();
    assert!(!refused.ok && refused.error.unwrap().contains("no call to answer"));
    let dnd = control.request(&Command::Dnd { on: true }).await.unwrap();
    assert!(!dnd.ok, "do not disturb needs FCP");

    // `hangup`: both calls end.
    assert!(
        control
            .request(&Command::Hangup { call: None })
            .await
            .unwrap()
            .ok
    );
    until(&bob, "Bob's call ends", |s| s.calls.is_empty()).await;
    until(&alice, "Alice's call ends", |s| s.calls.is_empty()).await;

    // The watcher saw the call appear, connect and end.
    let mut seen = Vec::new();
    while let Ok(Ok(Some(change))) =
        tokio::time::timeout(Duration::from_millis(300), watcher.next_change()).await
    {
        seen.push(change);
    }
    assert!(
        seen.iter()
            .any(|c| matches!(c, Change::Call { call: v } if v.state == CallState::Connected)),
        "{seen:?}"
    );
    assert!(
        seen.iter()
            .any(|c| matches!(c, Change::CallEnded { id, .. } if *id == call)),
        "{seen:?}"
    );

    server.abort();
    alice.shutdown().await;
    bob.shutdown().await;
}

/// Quiet audio with a choice of two microphones and one speaker; what was
/// chosen is shared with the test.
struct Devices(Arc<parking_lot::Mutex<(Option<String>, Option<String>)>>);

impl AudioHost for Devices {
    fn make_capture(&self, cfg: AudioFormat) -> Result<Box<dyn AudioSource>, AnvilError> {
        Quiet.make_capture(cfg)
    }
    fn make_playback(&self, cfg: AudioFormat) -> Result<Box<dyn AudioSink>, AnvilError> {
        Quiet.make_playback(cfg)
    }
    fn devices(&self) -> Vec<DeviceInfo> {
        let device = |id: &str, is_input, is_default| DeviceInfo {
            id: id.into(),
            name: id.into(),
            is_input,
            is_default,
        };
        vec![
            device("Built-in Microphone", true, true),
            device("USB Headset", true, false),
            device("Speakers", false, true),
        ]
    }
    fn choose_devices(&self, input: Option<&str>, output: Option<&str>) -> Result<(), AnvilError> {
        for id in [input, output].into_iter().flatten() {
            if !self.devices().iter().any(|d| d.id == id) {
                return Err(AnvilError::AudioDevice(format!("no device {id:?}")));
            }
        }
        *self.0.lock() = (input.map(String::from), output.map(String::from));
        Ok(())
    }
    fn chosen_devices(&self) -> (Option<String>, Option<String>) {
        self.0.lock().clone()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_audio_devices_chosen_are_kept_for_the_next_start() {
    log();
    let dir = std::env::temp_dir().join(format!("anvil-app-audio-{}", std::process::id()));
    let settings = dir.join("settings.json");
    let _ = std::fs::remove_dir_all(&dir);

    let chosen = Arc::new(parking_lot::Mutex::new((None, None)));
    let alice = phone_with(
        "alice",
        "127.0.0.32",
        Box::new(Devices(Arc::clone(&chosen))),
        Some(settings.clone()),
    )
    .await;
    let audio = alice.state().audio;
    assert_eq!(audio.inputs.len(), 2, "{audio:?}");
    assert_eq!(audio.outputs.len(), 1, "{audio:?}");
    assert_eq!((audio.input, audio.output), (None, None));

    // The headset, then a device that is not there.
    let used = alice
        .execute(Command::Audio {
            kind: AudioKind::Input,
            device: Some("USB Headset".into()),
        })
        .await
        .unwrap();
    assert!(used.ok, "{used:?}");
    assert_eq!(alice.state().audio.input.as_deref(), Some("USB Headset"));
    assert!(alice
        .execute(Command::Audio {
            kind: AudioKind::Output,
            device: Some("Nowhere".into()),
        })
        .await
        .is_err());
    assert_eq!(
        chosen.lock().clone(),
        (Some("USB Headset".to_string()), None),
        "a refused device changes nothing"
    );
    alice.shutdown().await;

    // Started again with the same settings: the headset is chosen at once.
    let again = Arc::new(parking_lot::Mutex::new((None, None)));
    let alice = phone_with(
        "alice",
        "127.0.0.32",
        Box::new(Devices(Arc::clone(&again))),
        Some(settings.clone()),
    )
    .await;
    assert_eq!(again.lock().0.as_deref(), Some("USB Headset"));
    assert_eq!(alice.state().audio.input.as_deref(), Some("USB Headset"));

    // Back to the default.
    assert!(
        alice
            .execute(Command::Audio {
                kind: AudioKind::Input,
                device: None,
            })
            .await
            .unwrap()
            .ok
    );
    assert_eq!(again.lock().0, None);
    alice.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// Quiet audio that cleans the microphone as it is told; what it was told
/// is shared with the test.
struct Cleaning(Arc<parking_lot::Mutex<Processing>>);

impl AudioHost for Cleaning {
    fn make_capture(&self, cfg: AudioFormat) -> Result<Box<dyn AudioSource>, AnvilError> {
        Quiet.make_capture(cfg)
    }
    fn make_playback(&self, cfg: AudioFormat) -> Result<Box<dyn AudioSink>, AnvilError> {
        Quiet.make_playback(cfg)
    }
    fn devices(&self) -> Vec<DeviceInfo> {
        Vec::new()
    }
    fn set_processing(&self, processing: Processing) -> Result<(), AnvilError> {
        *self.0.lock() = processing;
        Ok(())
    }
    fn processing(&self) -> Option<Processing> {
        Some(*self.0.lock())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn echo_cancellation_and_noise_suppression_are_kept_for_the_next_start() {
    log();
    let dir = std::env::temp_dir().join(format!("anvil-app-cleaning-{}", std::process::id()));
    let settings = dir.join("settings.json");
    let _ = std::fs::remove_dir_all(&dir);
    let on = AudioProcessing {
        echo_cancellation: true,
        noise_suppression: true,
    };

    let told = Arc::new(parking_lot::Mutex::new(Processing::default()));
    let alice = phone_with(
        "alice",
        "127.0.0.39",
        Box::new(Cleaning(Arc::clone(&told))),
        Some(settings.clone()),
    )
    .await;
    assert_eq!(
        alice.state().audio.processing,
        Some(on),
        "on unless turned off"
    );

    let off = alice
        .execute(Command::parse_line("audio echo off").unwrap())
        .await
        .unwrap();
    assert!(off.ok, "{off:?}");
    assert_eq!(
        *told.lock(),
        Processing {
            echo_cancellation: false,
            noise_suppression: true,
        }
    );
    assert_eq!(
        alice.state().audio.processing,
        Some(AudioProcessing {
            echo_cancellation: false,
            noise_suppression: true,
        })
    );
    alice.shutdown().await;

    // Started again: the host is told before any call.
    let again = Arc::new(parking_lot::Mutex::new(Processing::default()));
    let alice = phone_with(
        "alice",
        "127.0.0.39",
        Box::new(Cleaning(Arc::clone(&again))),
        Some(settings.clone()),
    )
    .await;
    assert!(!again.lock().echo_cancellation);
    assert!(again.lock().noise_suppression);
    assert!(
        alice
            .execute(Command::parse_line("audio echo on").unwrap())
            .await
            .unwrap()
            .ok
    );
    assert_eq!(*again.lock(), Processing::default());
    assert_eq!(alice.state().audio.processing, Some(on));
    alice.shutdown().await;

    // A host that does none of its own: nothing to show, and refused.
    let alice = phone_with("alice", "127.0.0.39", Box::new(Quiet), Some(settings)).await;
    assert_eq!(alice.state().audio.processing, None);
    assert!(alice
        .execute(Command::parse_line("audio noise off").unwrap())
        .await
        .is_err());
    alice.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// A host with volumes of its own, keeping what it is told.
struct Loudness(Arc<parking_lot::Mutex<Volume>>);

impl AudioHost for Loudness {
    fn make_capture(&self, cfg: AudioFormat) -> Result<Box<dyn AudioSource>, AnvilError> {
        Quiet.make_capture(cfg)
    }
    fn make_playback(&self, cfg: AudioFormat) -> Result<Box<dyn AudioSink>, AnvilError> {
        Quiet.make_playback(cfg)
    }
    fn devices(&self) -> Vec<DeviceInfo> {
        Vec::new()
    }
    fn set_volume(&self, volume: Volume) -> Result<(), AnvilError> {
        *self.0.lock() = volume;
        Ok(())
    }
    fn volume(&self) -> Option<Volume> {
        Some(*self.0.lock())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_volumes_are_kept_for_the_next_start() {
    log();
    let dir = std::env::temp_dir().join(format!("anvil-app-volume-{}", std::process::id()));
    let settings = dir.join("settings.json");
    let _ = std::fs::remove_dir_all(&dir);

    let told = Arc::new(parking_lot::Mutex::new(Volume::default()));
    let alice = phone_with(
        "alice",
        "127.0.0.40",
        Box::new(Loudness(Arc::clone(&told))),
        Some(settings.clone()),
    )
    .await;
    assert_eq!(
        alice.state().audio.volume,
        Some(AudioVolume {
            microphone: 100,
            speaker: 100
        }),
        "the sound as it is unless set"
    );
    for line in ["volume mic 70", "volume speaker 140"] {
        let done = alice
            .execute(Command::parse_line(line).unwrap())
            .await
            .unwrap();
        assert!(done.ok, "{line}: {done:?}");
    }
    let set = Volume {
        microphone: 70,
        speaker: 140,
    };
    assert_eq!(*told.lock(), set);
    assert_eq!(
        alice.state().audio.volume,
        Some(AudioVolume {
            microphone: 70,
            speaker: 140
        })
    );
    alice.shutdown().await;

    // Started again: the host is told before any call.
    let again = Arc::new(parking_lot::Mutex::new(Volume::default()));
    let alice = phone_with(
        "alice",
        "127.0.0.40",
        Box::new(Loudness(Arc::clone(&again))),
        Some(settings.clone()),
    )
    .await;
    assert_eq!(*again.lock(), set);
    alice.shutdown().await;

    // A host whose volume the system sets: nothing to show, and refused.
    let alice = phone_with("alice", "127.0.0.40", Box::new(Quiet), Some(settings)).await;
    assert_eq!(alice.state().audio.volume, None);
    assert!(alice
        .execute(Command::parse_line("volume mic 50").unwrap())
        .await
        .is_err());
    alice.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn answering_a_second_call_holds_the_first_and_resuming_swaps() {
    log();
    let alice = phone("alice", "127.0.0.33").await;
    let bob = phone("bob", "127.0.0.34").await;
    let carol = phone("carol", "127.0.0.35").await;

    // Alice and Bob talking.
    let first = alice
        .execute(Command::Call {
            target: format!("sip:bob@{}", bob.sip_address()),
        })
        .await
        .unwrap()
        .call
        .unwrap();
    until(&bob, "Bob rings", |s| {
        s.calls.iter().any(|c| c.state == CallState::Ringing)
    })
    .await;
    assert!(
        bob.execute(Command::Answer { call: None })
            .await
            .unwrap()
            .ok
    );
    until(&alice, "Alice and Bob talk", |s| {
        s.call_state(first) == Some(CallState::Connected)
    })
    .await;
    let view = alice
        .state()
        .calls
        .into_iter()
        .find(|c| c.id == first)
        .unwrap();
    assert!(!view.encrypted, "a plain call: {view:?}");

    // Carol calls Alice, who answers: Bob is held, Carol is on.
    carol
        .execute(Command::Call {
            target: format!("sip:alice@{}", alice.sip_address()),
        })
        .await
        .unwrap();
    until(&alice, "Carol rings at Alice", |s| {
        s.calls.iter().any(|c| c.state == CallState::Ringing)
    })
    .await;
    assert!(
        alice
            .execute(Command::Answer { call: None })
            .await
            .unwrap()
            .ok
    );
    until(&alice, "Bob held, Carol on", |s| {
        s.calls.len() == 2
            && s.calls.iter().all(|c| c.state == CallState::Connected)
            && s.calls.iter().any(|c| c.id == first && c.held)
            && s.calls.iter().any(|c| c.id != first && !c.held)
    })
    .await;
    assert!(
        alice.state().calls.iter().all(|c| c.connected_at.is_some()),
        "a connected call says since when"
    );

    // Back to Bob: Carol is held in turn, a swap.
    assert!(
        alice
            .execute(Command::Resume { call: Some(first) })
            .await
            .unwrap()
            .ok
    );
    until(&alice, "Bob on, Carol held", |s| {
        s.calls.iter().any(|c| c.id == first && !c.held)
            && s.calls.iter().any(|c| c.id != first && c.held)
    })
    .await;

    // Without FCP, what needs it says so.
    let refused = alice.execute(Command::Heard { id: "m1".into() }).await;
    assert!(
        matches!(&refused, Err(e) if e.to_string().contains("FCP")),
        "{:?}",
        refused.map(|r| r.ok)
    );

    alice.shutdown().await;
    bob.shutdown().await;
    carol.shutdown().await;
}

/// What a ringing phone was heard to play: loud frames on
/// the speaker it rings on and on the one calls use, and the speaker
/// chosen to ring on.
#[derive(Default)]
struct Heard {
    ringing: std::sync::atomic::AtomicUsize,
    in_calls: std::sync::atomic::AtomicUsize,
    ring_device: parking_lot::Mutex<Option<String>>,
}

impl Heard {
    fn ringing(&self) -> usize {
        self.ringing.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn in_calls(&self) -> usize {
        self.in_calls.load(std::sync::atomic::Ordering::SeqCst)
    }
}

struct Listening(Arc<Heard>);
struct CountingSink(Arc<Heard>, bool);

#[async_trait]
impl AudioSink for CountingSink {
    async fn write_frame(&mut self, frame: &AudioFrame) -> Result<(), AnvilError> {
        // Loud: a ringtone or a tone, not a quiet call's decoded silence.
        if frame.samples.iter().any(|&s| s.unsigned_abs() > 2_000) {
            let counter = if self.1 {
                &self.0.ringing
            } else {
                &self.0.in_calls
            };
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(())
    }
}

impl AudioHost for Listening {
    fn make_capture(&self, cfg: AudioFormat) -> Result<Box<dyn AudioSource>, AnvilError> {
        Quiet.make_capture(cfg)
    }
    fn make_playback(&self, _: AudioFormat) -> Result<Box<dyn AudioSink>, AnvilError> {
        Ok(Box::new(CountingSink(Arc::clone(&self.0), false)))
    }
    fn make_ringer(&self, _: AudioFormat) -> Result<Box<dyn AudioSink>, AnvilError> {
        Ok(Box::new(CountingSink(Arc::clone(&self.0), true)))
    }
    fn devices(&self) -> Vec<DeviceInfo> {
        vec![DeviceInfo {
            id: "Speakers".into(),
            name: "Speakers".into(),
            is_input: false,
            is_default: true,
        }]
    }
    fn choose_ring_device(&self, output: Option<&str>) -> Result<(), AnvilError> {
        if output.is_some_and(|o| o != "Speakers") {
            return Err(AnvilError::AudioDevice("no such speaker".into()));
        }
        *self.0.ring_device.lock() = output.map(String::from);
        Ok(())
    }
    fn chosen_ring_device(&self) -> Option<String> {
        self.0.ring_device.lock().clone()
    }
}

/// Whether `count` goes up within `within`.
async fn grows(count: impl Fn() -> usize, within: Duration) -> bool {
    let before = count();
    let deadline = tokio::time::Instant::now() + within;
    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
        if count() > before {
            return true;
        }
    }
    false
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_call_rings_until_answered_and_a_second_is_a_tone_in_the_call() {
    log();
    let dir = std::env::temp_dir().join(format!("anvil-app-ring-{}", std::process::id()));
    let settings = dir.join("settings.json");
    let _ = std::fs::remove_dir_all(&dir);
    let heard = Arc::new(Heard::default());
    let alice = start(
        "alice",
        "127.0.0.36",
        Box::new(Listening(Arc::clone(&heard))),
        Some(settings.clone()),
        true,
        None,
    )
    .await;
    let bob = phone("bob", "127.0.0.37").await;
    let carol = phone("carol", "127.0.0.38").await;

    // The ringtones: Anvil's two, the first ringing; one chosen is heard
    // once and kept, one that is not there refused.
    let audio = alice.state().audio;
    let ids: Vec<&str> = audio.ringtones.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["anvil", "classic"]);
    assert_eq!(audio.ringtone, "anvil");
    assert!(
        alice
            .execute(Command::Ringtone {
                id: Some("classic".into())
            })
            .await
            .unwrap()
            .ok
    );
    assert_eq!(alice.state().audio.ringtone, "classic");
    assert!(
        grows(|| heard.ringing(), SOON).await,
        "the ringtone previewed"
    );
    assert!(alice
        .execute(Command::Ringtone {
            id: Some("brand:Nope".into())
        })
        .await
        .is_err());
    assert!(
        alice
            .execute(Command::Audio {
                kind: AudioKind::Ring,
                device: Some("Speakers".into()),
            })
            .await
            .unwrap()
            .ok
    );
    assert_eq!(alice.state().audio.ring.as_deref(), Some("Speakers"));
    // Quiet once the preview is over.
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(!grows(|| heard.ringing(), Duration::from_millis(500)).await);

    // Bob calls: Alice rings on the ringing speaker, not in a call.
    let in_calls = heard.in_calls();
    bob.execute(Command::Call {
        target: format!("sip:alice@{}", alice.sip_address()),
    })
    .await
    .unwrap();
    until(&alice, "Bob rings at Alice", |s| {
        s.calls.iter().any(|c| c.state == CallState::Ringing)
    })
    .await;
    assert!(grows(|| heard.ringing(), SOON).await, "Alice rings");
    assert_eq!(heard.in_calls(), in_calls, "nothing in a call");

    // Answered: quiet.
    assert!(
        alice
            .execute(Command::Answer { call: None })
            .await
            .unwrap()
            .ok
    );
    until(&alice, "Alice and Bob talk", |s| {
        s.calls.iter().any(|c| c.state == CallState::Connected)
    })
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!grows(|| heard.ringing(), Duration::from_millis(500)).await);

    // Carol calls while Alice talks: a tone in the call, no ringing.
    carol
        .execute(Command::Call {
            target: format!("sip:alice@{}", alice.sip_address()),
        })
        .await
        .unwrap();
    until(&alice, "Carol rings at Alice", |s| {
        s.calls.iter().any(|c| c.state == CallState::Ringing)
    })
    .await;
    assert!(grows(|| heard.in_calls(), SOON).await, "the waiting tone");
    assert!(!grows(|| heard.ringing(), Duration::from_millis(500)).await);

    // Carol gives up: the tone stops.
    carol.execute(Command::Hangup { call: None }).await.unwrap();
    until(&alice, "Carol's call gone", |s| s.calls.len() == 1).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!grows(|| heard.in_calls(), Duration::from_millis(500)).await);

    alice.shutdown().await;
    bob.shutdown().await;
    carol.shutdown().await;

    // The ringtone and its speaker are kept for the next start.
    let kept = std::fs::read_to_string(&settings).unwrap();
    assert!(kept.contains("\"ringtone\": \"classic\""), "{kept}");
    assert!(kept.contains("\"ring_output\": \"Speakers\""), "{kept}");
    let _ = std::fs::remove_dir_all(dir);
}

/// A WAV file of `samples` of silence, 8 kHz mono 16-bit.
fn wav(samples: u32) -> Vec<u8> {
    let data = samples * 2;
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&8000u32.to_le_bytes());
    b.extend_from_slice(&16000u32.to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data.to_le_bytes());
    b.resize(b.len() + data as usize, 0);
    b
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_installed_profiles_brand_is_the_phones_while_fcp_has_none() {
    log();
    let dir = std::env::temp_dir().join(format!("anvil-app-profile-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("profile.json"),
        r##"{"brand": {"app_name": "Acme Voice", "colors": {"primary": "#AA2200"},
                       "ringtone": "acme.wav"}}"##,
    )
    .unwrap();
    std::fs::write(dir.join("acme.wav"), wav(800)).unwrap();
    let managed = anvil_app::managed::load_from(anvil_app::managed::Sources {
        profile_dir: Some(dir.clone()),
        ..Default::default()
    });
    let alice = start(
        "alice",
        "127.0.0.39",
        Box::new(Quiet),
        None,
        false,
        managed.brand,
    )
    .await;
    let state = alice.state();
    let brand = state.brand.expect("the profile's brand");
    assert_eq!(brand.app_name, "Acme Voice");
    assert_eq!(brand.primary.as_deref(), Some("#AA2200"));
    let ids: Vec<&str> = state
        .audio
        .ringtones
        .iter()
        .map(|r| r.id.as_str())
        .collect();
    assert!(ids.contains(&"brand:acme"), "{ids:?}");
    assert_eq!(state.audio.ringtone, "brand:acme", "the brand's rings");
    alice.shutdown().await;
    let _ = std::fs::remove_dir_all(dir);
}
