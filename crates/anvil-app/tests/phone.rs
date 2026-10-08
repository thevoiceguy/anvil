//! Two phones calling each other with no server: one driven only through its
//! control socket, as `anvil-cli call …` drives a running phone.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anvil_app::control::{Client, Endpoint};
use anvil_app::{AudioKind, CallState, Change, Command, Phone, PhoneConfig};
use anvil_core::audio::{AudioFormat, AudioFrame, AudioHost, AudioSink, AudioSource, DeviceInfo};
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
            },
            media: MediaConfig::default(),
            audio,
            brand: BrandConfig::default(),
        },
        fcp: None,
        register: false,
        settings_path,
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn answering_a_second_call_holds_the_first() {
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
