//! Video calls through the phone, two phones and no server, with the
//! core's synthetic video host: a video call answered with audio only,
//! the camera turned on later, the camera chosen, and H.264 allowed and
//! not, and the camera stopped while a phone's app is in the background
//! (FCP's `docs/SOFTPHONE_VIDEO.md` §2.5).

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anvil_app::{CallState, Command, OpenH264, Phone, PhoneConfig};
use anvil_core::audio::{AudioFormat, AudioFrame, AudioHost, AudioSink, AudioSource, DeviceInfo};
use anvil_core::config::{AccountConfig, MediaConfig};
use anvil_core::video::host::Resolution;
use anvil_core::video::synthetic::SyntheticVideoHost;
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

/// OpenH264 as a desktop fetches it, without the fetching.
#[derive(Default)]
struct FakeOpenH264 {
    ready: AtomicBool,
    enabled: AtomicU32,
    disabled: AtomicU32,
}

impl OpenH264 for FakeOpenH264 {
    fn notice(&self) -> String {
        "OpenH264 Video Codec provided by Cisco Systems, Inc.".into()
    }
    fn ready(&self) -> bool {
        self.ready.load(Ordering::Relaxed)
    }
    fn enable(&self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> {
        self.ready.store(true, Ordering::Relaxed);
        self.enabled.fetch_add(1, Ordering::Relaxed);
        Box::pin(async { Ok(()) })
    }
    fn disable(&self) {
        self.ready.store(false, Ordering::Relaxed);
        self.disabled.fetch_add(1, Ordering::Relaxed);
    }
}

async fn phone(
    user: &str,
    ip: &str,
    video: Option<SyntheticVideoHost>,
    openh264: Option<Arc<FakeOpenH264>>,
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
            audio: Box::new(Quiet),
            video: video.map(|v| Box::new(v) as _),
            brand: BrandConfig::default(),
        },
        fcp: None,
        register: false,
        settings_path: None,
        ring: false,
        fallback_brand: None,
        openh264: openh264.map(|o| o as Arc<dyn OpenH264>),
    })
    .await
    .expect("the phone starts")
}

fn log() {
    let filter =
        std::env::var("ANVIL_LOG").unwrap_or_else(|_| "anvil_core=debug,anvil_app=debug".into());
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

fn call(phone: &Phone) -> anvil_app::CallView {
    phone.state().calls.into_iter().next().expect("a call")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_video_call_answered_with_audio_gets_video_when_the_camera_is_turned_on() {
    log();
    let alice_host = SyntheticVideoHost::default();
    let h264 = Arc::new(FakeOpenH264::default());
    let alice = phone(
        "alice",
        "127.0.0.60",
        Some(alice_host.clone()),
        Some(Arc::clone(&h264)),
    )
    .await;
    let bob = phone(
        "bob",
        "127.0.0.61",
        Some(SyntheticVideoHost::default()),
        None,
    )
    .await;

    // What there is: video, a camera, H.264 fetched as it is allowed.
    until(&alice, "OpenH264 is ready", |s| {
        s.video.h264.as_ref().is_some_and(|h| h.ready)
    })
    .await;
    let video = alice.state().video;
    assert!(video.available);
    assert_eq!(video.cameras.len(), 1);
    assert_eq!(video.camera, None);
    let h = video.h264.unwrap();
    assert!(h.allowed);
    assert!(h.notice.contains("Cisco"));
    assert_eq!(h264.enabled.load(Ordering::Relaxed), 1);
    assert!(
        bob.state().video.h264.is_none(),
        "Bob does not fetch OpenH264"
    );

    // Alice places a video call; Bob sees it offers video and answers
    // with audio only.
    alice
        .execute(Command::parse_line(&format!("vcall sip:bob@{}", bob.sip_address())).unwrap())
        .await
        .unwrap();
    assert!(call(&alice).video.camera, "Alice's camera is on");
    until(&bob, "Bob rings with video", |s| {
        s.calls
            .iter()
            .any(|c| c.state == CallState::Ringing && c.video.offered)
    })
    .await;
    bob.execute(Command::parse_line("answer audio").unwrap())
        .await
        .unwrap();
    until(&alice, "the call is up", |s| {
        s.calls.iter().any(|c| c.state == CallState::Connected)
    })
    .await;
    until(&bob, "the call is up", |s| {
        s.calls.iter().any(|c| c.state == CallState::Connected)
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!call(&bob).video.camera);
    assert!(call(&bob).video.streams.is_empty(), "{:?}", call(&bob));
    assert!(call(&alice).video.streams.is_empty(), "{:?}", call(&alice));

    // Alice's camera went with the video Bob declined.
    assert!(!call(&alice).video.camera, "{:?}", call(&alice));

    // Bob turns his camera on: Alice sees him, and is asked for hers.
    bob.execute(Command::parse_line("video on").unwrap())
        .await
        .unwrap();
    until(&bob, "Bob sends", |s| {
        let c = &s.calls[0];
        c.video.camera && c.video.sending.is_some() && c.video.streams.is_empty()
    })
    .await;
    until(&alice, "Alice sees Bob and is asked", |s| {
        let c = &s.calls[0];
        c.video.requested && !c.video.camera && c.video.streams.len() == 1
    })
    .await;

    // Alice turns hers on: each sees the other.
    alice
        .execute(Command::parse_line("video on").unwrap())
        .await
        .unwrap();
    until(&alice, "Alice sends", |s| {
        let c = &s.calls[0];
        c.video.camera && !c.video.requested && c.video.sending.is_some()
    })
    .await;
    until(&bob, "Bob sees Alice", |s| {
        s.calls[0].video.streams.len() == 1
    })
    .await;

    // The camera chosen: one there is; one that is not is refused.
    alice
        .execute(Command::parse_line("camera synthetic").unwrap())
        .await
        .unwrap();
    assert_eq!(alice.state().video.camera.as_deref(), Some("synthetic"));
    assert_eq!(alice_host.record().camera.as_deref(), Some("synthetic"));
    assert!(alice
        .execute(Command::parse_line("camera nowhere").unwrap())
        .await
        .is_err());
    alice
        .execute(Command::parse_line("camera default").unwrap())
        .await
        .unwrap();
    assert_eq!(alice_host.record().camera, None);

    // Alice turns her camera off: Bob's tile of her goes.
    alice
        .execute(Command::parse_line("video off").unwrap())
        .await
        .unwrap();
    assert!(!call(&alice).video.camera);
    until(&bob, "Alice's picture goes", |s| {
        s.calls[0].video.streams.is_empty()
    })
    .await;

    // H.264 not allowed, then allowed again.
    alice
        .execute(Command::parse_line("h264 off").unwrap())
        .await
        .unwrap();
    let h = alice.state().video.h264.unwrap();
    assert!(!h.allowed && !h.ready);
    assert_eq!(h264.disabled.load(Ordering::Relaxed), 1);
    alice
        .execute(Command::parse_line("h264 on").unwrap())
        .await
        .unwrap();
    assert!(alice.state().video.h264.unwrap().ready);
    assert!(bob
        .execute(Command::parse_line("h264 on").unwrap())
        .await
        .is_err());

    alice
        .execute(Command::parse_line("hangup").unwrap())
        .await
        .unwrap();
    until(&bob, "the call ends", |s| s.calls.is_empty()).await;
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_a_video_host_a_video_call_is_an_audio_call() {
    log();
    let alice = phone("alice", "127.0.0.62", None, None).await;
    let bob = phone(
        "bob",
        "127.0.0.63",
        Some(SyntheticVideoHost::default()),
        None,
    )
    .await;
    assert!(!alice.state().video.available);
    alice
        .execute(Command::parse_line(&format!("vcall sip:bob@{}", bob.sip_address())).unwrap())
        .await
        .unwrap();
    assert!(!call(&alice).video.camera);
    until(&bob, "Bob rings, audio only", |s| {
        s.calls
            .iter()
            .any(|c| c.state == CallState::Ringing && !c.video.offered)
    })
    .await;
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_phone_in_the_background_stops_its_camera_until_it_comes_back() {
    log();
    let alice = phone(
        "alice",
        "127.0.0.64",
        Some(SyntheticVideoHost::default()),
        None,
    )
    .await;
    let bob = phone(
        "bob",
        "127.0.0.65",
        Some(SyntheticVideoHost::default()),
        None,
    )
    .await;
    let run = |p: &Phone, line: &str| {
        let (p, command) = (p.clone(), Command::parse_line(line).unwrap());
        async move { p.execute(command).await.unwrap() }
    };

    // Bob's app is in the background when Alice's video call rings (it is
    // answered at the lock screen): answered without his camera.
    run(&bob, "background on").await;
    run(&alice, &format!("vcall sip:bob@{}", bob.sip_address())).await;
    until(&bob, "Bob rings with video", |s| {
        s.calls
            .iter()
            .any(|c| c.state == CallState::Ringing && c.video.offered)
    })
    .await;
    run(&bob, "answer").await;
    until(&bob, "the call is up", |s| {
        s.calls.iter().any(|c| c.state == CallState::Connected)
    })
    .await;
    assert!(!call(&bob).video.camera, "{:?}", call(&bob));

    // Bob comes back: his camera starts, and each sees the other once
    // Alice turns hers on again.
    run(&bob, "background off").await;
    until(&bob, "Bob sends", |s| {
        let c = &s.calls[0];
        c.video.camera && c.video.sending.is_some()
    })
    .await;
    until(&alice, "Alice sees Bob", |s| {
        s.calls[0].video.streams.len() == 1
    })
    .await;
    run(&alice, "video on").await;
    until(&bob, "Bob sees Alice", |s| {
        s.calls[0].video.streams.len() == 1
    })
    .await;

    // Alice's app goes to the background: her camera stops, Bob's tile of
    // her goes; back, it starts again.
    run(&alice, "background on").await;
    assert!(!call(&alice).video.camera);
    until(&bob, "Alice's picture goes", |s| {
        s.calls[0].video.streams.is_empty()
    })
    .await;
    run(&alice, "background off").await;
    until(&alice, "Alice sends again", |s| {
        s.calls[0].video.camera && s.calls[0].video.sending.is_some()
    })
    .await;
    until(&bob, "Bob sees Alice again", |s| {
        s.calls[0].video.streams.len() == 1
    })
    .await;

    run(&alice, "hangup").await;
    until(&bob, "the call ends", |s| s.calls.is_empty()).await;
    alice.shutdown().await;
    bob.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn on_cellular_a_phone_sends_at_most_360p_and_with_wifi_only_none_until_wifi() {
    log();
    let alice_host = SyntheticVideoHost::default();
    let alice = phone("alice", "127.0.0.140", Some(alice_host.clone()), None).await;
    let bob = phone(
        "bob",
        "127.0.0.141",
        Some(SyntheticVideoHost::default()),
        None,
    )
    .await;
    let run = |p: &Phone, line: &str| {
        let (p, command) = (p.clone(), Command::parse_line(line).unwrap());
        async move { p.execute(command).await.unwrap() }
    };

    // Alice is on cellular and sends video only on Wi-Fi: she answers Bob's
    // video call seeing him, sending nothing.
    run(&alice, "network cellular").await;
    run(&alice, "wifi-only on").await;
    let video = alice.state().video;
    assert!(video.cellular && video.wifi_only, "{video:?}");
    run(&bob, &format!("vcall sip:alice@{}", alice.sip_address())).await;
    until(&alice, "Alice rings with video", |s| {
        s.calls
            .iter()
            .any(|c| c.state == CallState::Ringing && c.video.offered)
    })
    .await;
    run(&alice, "answer").await;
    until(&alice, "Alice sees Bob", |s| {
        s.calls
            .first()
            .is_some_and(|c| c.state == CallState::Connected && c.video.streams.len() == 1)
    })
    .await;
    assert!(!call(&alice).video.camera, "{:?}", call(&alice));
    assert!(alice_host.record().sent.is_empty(), "nothing sent");

    // On Wi-Fi: her camera starts, at the size the call allows.
    run(&alice, "network wifi").await;
    until(&bob, "Bob sees Alice", |s| {
        s.calls[0].video.streams.len() == 1
    })
    .await;
    assert_eq!(alice_host.record().sent[0].config.max, Resolution::P720);

    // Back on cellular with Wi-Fi only off: she keeps sending, at most
    // 360p from her next frame.
    run(&alice, "wifi-only off").await;
    run(&alice, "network cellular").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let sent = &alice_host.record().sent[0];
    assert!(!sent.stopped);
    let (_, max, _) = *sent.targets.last().expect("a target set");
    assert_eq!((max.width, max.height), (640, 360), "{:?}", sent.targets);
    assert!(call(&alice).video.camera);

    run(&alice, "hangup").await;
    until(&bob, "the call ends", |s| s.calls.is_empty()).await;
    alice.shutdown().await;
    bob.shutdown().await;
}
