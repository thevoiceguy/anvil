//! Two Anvils in a video call, each with the desktop host filming a test
//! pattern: real pictures, through real codecs, over the core's RTP,
//! shown in each other's tiles. VP8 always; H.264 too when
//! `ANVIL_OPENH264_DIR` holds Cisco's OpenH264.

use std::path::PathBuf;
use std::time::Duration;

use anvil_audio::ToneHost;
use anvil_core::config::{AccountConfig, AnvilConfig, BrandConfig, MediaConfig, Transport};
use anvil_core::video::host::{VideoCodec, VideoHost};
use anvil_core::{Anvil, CallId, CallMedia, Event, EventStream};
use anvil_video::{DesktopVideoHost, PatternCamera, Picture, TileKey, Tiles};

const SOON: Duration = Duration::from_secs(10);

fn log() {
    if let Ok(filter) = std::env::var("ANVIL_LOG") {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
            .with_test_writer()
            .try_init();
    }
}

/// Where OpenH264 is, or a directory without it.
fn openh264_dir(h264: bool) -> PathBuf {
    match std::env::var_os("ANVIL_OPENH264_DIR") {
        Some(dir) if h264 => PathBuf::from(dir),
        _ => std::env::temp_dir().join("anvil-video-test-no-openh264"),
    }
}

async fn phone(user: &str, host: DesktopVideoHost) -> (Anvil, EventStream) {
    log();
    Anvil::start(AnvilConfig {
        account: AccountConfig {
            aor: format!("sip:{user}@127.0.0.1"),
            registrar: "sip:127.0.0.1:9".into(),
            username: user.into(),
            password: String::new(),
            transport: Transport::Udp,
            outbound_proxy: None,
            stun: None,
            register_expires: Duration::from_secs(3600),
            user_agent: "anvil-video-test".into(),
            bind_addr: Some("127.0.0.1:0".into()),
            tls_extra_ca_pem: None,
            provisioning_url: None,
            instance_id: None,
            push: None,
        },
        media: MediaConfig::default(),
        audio: Box::new(ToneHost::new()),
        video: Some(Box::new(host)),
        brand: BrandConfig::default(),
    })
    .await
    .expect("anvil starts")
}

async fn expect<T>(
    events: &mut EventStream,
    mut pick: impl FnMut(&Event) -> Option<T>,
) -> Option<T> {
    let deadline = tokio::time::Instant::now() + SOON;
    loop {
        let event = tokio::time::timeout_at(deadline, events.recv())
            .await
            .ok()??;
        if let Some(v) = pick(&event) {
            return Some(v);
        }
    }
}

/// The incoming stream's tile on `tiles`, once it has shown `n` pictures.
async fn shows(tiles: &Tiles, n: u64) -> TileKey {
    let deadline = tokio::time::Instant::now() + SOON;
    loop {
        let key = tiles
            .keys()
            .into_iter()
            .find(|k| matches!(k, TileKey::Stream(_)));
        if let Some(key) = key {
            if tiles.stats(key).is_some_and(|s| s.presented >= n) {
                return key;
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no {n} pictures: {:?}",
            key.and_then(|k| tiles.stats(k))
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// How close `picture` is to the nearest frame of the test pattern, dB
/// (the first 20 s of it: the pictures checked are a few seconds in).
fn likeness(picture: &Picture) -> f64 {
    (0..600)
        .map(|n| picture.psnr(&Picture::pattern(picture.width, picture.height, n)))
        .fold(0.0, f64::max)
}

async fn a_call_shows_each_camera_to_the_other(h264: bool) -> VideoCodec {
    let alice_host = DesktopVideoHost::with_cameras(PatternCamera::default(), &openh264_dir(h264));
    let bob_host = DesktopVideoHost::with_cameras(PatternCamera::default(), &openh264_dir(h264));
    let (alice_tiles, bob_tiles) = (alice_host.tiles(), bob_host.tiles());
    let (alice, mut alice_events) = phone("alice", alice_host).await;
    // A phone's events are read, or its calls wait on them.
    tokio::spawn(async move { while alice_events.recv().await.is_some() {} });
    let (bob, mut bob_events) = phone("bob", bob_host).await;

    let out = alice
        .place_call_with_media(&format!("sip:bob@{}", bob.sip_address()), CallMedia::VIDEO)
        .await
        .expect("the call is placed");
    let incoming: CallId = expect(&mut bob_events, |e| match e {
        Event::IncomingCall { call, .. } => Some(*call),
        _ => None,
    })
    .await
    .expect("bob rings");
    bob.answer_with_media(incoming, CallMedia::VIDEO)
        .await
        .expect("bob answers");
    let codec = expect(&mut bob_events, |e| match e {
        Event::VideoStarted { codec, .. } => Some(*codec),
        _ => None,
    })
    .await
    .expect("bob's video starts");
    tokio::spawn(async move { while bob_events.recv().await.is_some() {} });

    for (who, tiles) in [("bob", &bob_tiles), ("alice", &alice_tiles)] {
        let key = shows(tiles, 30).await;
        let stats = tiles.stats(key).unwrap();
        assert_eq!((stats.width, stats.height), (640, 360), "{who}");
        let last = tiles.last(key).unwrap();
        let psnr = likeness(&last);
        assert!(psnr > 28.0, "{who} sees the pattern: {psnr:.1} dB");
        assert!(
            tiles.stats(TileKey::SelfView).is_some(),
            "{who}'s self-view"
        );
    }

    alice.hangup(out).await.expect("hang up");
    // Both cameras close: the self-views go, and so do the streams' tiles.
    let deadline = tokio::time::Instant::now() + SOON;
    while !(alice_tiles.keys().is_empty() && bob_tiles.keys().is_empty()) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "tiles left: {:?} {:?}",
            alice_tiles.keys(),
            bob_tiles.keys()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let _ = alice.shutdown().await;
    let _ = bob.shutdown().await;
    codec
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_vp8_call_shows_each_camera_to_the_other() {
    assert_eq!(
        a_call_shows_each_camera_to_the_other(false).await,
        VideoCodec::Vp8
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_h264_call_shows_each_camera_to_the_other() {
    let host = DesktopVideoHost::with_cameras(PatternCamera::default(), &openh264_dir(true));
    if !host.codecs().iter().any(|c| c.codec == VideoCodec::H264) {
        assert!(
            std::env::var_os("ANVIL_OPENH264_DIR").is_none(),
            "no OpenH264 in ANVIL_OPENH264_DIR"
        );
        eprintln!("skipped: no ANVIL_OPENH264_DIR");
        return;
    }
    assert_eq!(
        a_call_shows_each_camera_to_the_other(true).await,
        VideoCodec::H264
    );
}
