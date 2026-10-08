//! Two phones calling each other with no server: one driven only through its
//! control socket, as `anvil-cli call …` drives a running phone.

use std::time::Duration;

use anvil_app::control::{Client, Endpoint};
use anvil_app::{CallState, Change, Command, Phone, PhoneConfig};
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
            audio: Box::new(Quiet),
            brand: BrandConfig::default(),
        },
        fcp: None,
        register: false,
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
