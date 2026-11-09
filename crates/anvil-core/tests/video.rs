//! Video between two Anvils calling each other directly (FCP's
//! `docs/SOFTPHONE_VIDEO.md` §4): offered and answered, in the clear and
//! over SRTP; answered with audio only; declined by a softphone without
//! video; stopped by hold and back on resume. Each side's host is the
//! synthetic one, so the frames are numbered and checked.

mod common;

use std::time::Duration;

use anvil_core::config::{MediaConfig, SrtpMode};
use anvil_core::video::host::{StreamId, VideoKind};
use anvil_core::video::synthetic::{ReceivedStream, SyntheticVideoHost};
use anvil_core::video::{VideoStreamDirection, VideoStreamStats};
use anvil_core::{Anvil, CallId, CallMedia, Event, EventStream};
use common::*;

async fn phone(
    user: &str,
    ip: &str,
    srtp: SrtpMode,
    video: Option<SyntheticVideoHost>,
) -> (Anvil, EventStream) {
    start_with_video(
        account(
            &format!("sip:{user}@127.0.0.1"),
            "sip:127.0.0.1:9",
            user,
            "",
            ip,
            3600,
        ),
        MediaConfig {
            srtp,
            ..MediaConfig::default()
        },
        video,
    )
    .await
}

fn host() -> SyntheticVideoHost {
    SyntheticVideoHost::default().with_fps(50)
}

/// The one stream `host` receives, once it has shown `frames`.
async fn receives(host: &SyntheticVideoHost, frames: usize) -> ReceivedStream {
    let deadline = tokio::time::Instant::now() + SOON;
    loop {
        let record = host.record();
        let live: Vec<_> = record.received.values().filter(|s| !s.stopped).collect();
        if let [s] = live.as_slice() {
            if s.frames.len() >= frames {
                return (*s).clone();
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no {frames} frames: {record:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn in_order(s: &ReceivedStream) {
    let n = s.numbers();
    assert!(s.frames[0].1, "the picture starts on a keyframe");
    assert!(n.windows(2).all(|w| w[1] == w[0] + 1), "in order: {n:?}");
    assert_eq!(s.unreadable, 0);
}

type Started = (StreamId, VideoStreamDirection, VideoKind);

/// The streams `call` started, until `n` of them have (`seen` already).
async fn started(
    events: &mut EventStream,
    call: CallId,
    n: usize,
    mut seen: Vec<Started>,
) -> Vec<Started> {
    while seen.len() < n {
        let s = expect(events, SOON, |e| match e {
            Event::VideoStarted {
                call: c,
                stream,
                direction,
                kind,
                ..
            } if *c == call => Some((*stream, *direction, *kind)),
            _ => None,
        })
        .await
        .unwrap_or_else(|| panic!("{n} streams start; saw {seen:?}"));
        seen.push(s);
    }
    seen
}

/// A video call from `alice` to `bob`, answered as `answer` says.
async fn video_call(
    alice: &Anvil,
    alice_events: &mut EventStream,
    bob: &Anvil,
    bob_events: &mut EventStream,
    answer: CallMedia,
) -> (CallId, CallId, bool, Vec<Started>) {
    let target = format!("sip:bob@{}", bob.sip_address());
    let out = alice
        .place_call_with_media(&target, CallMedia::VIDEO)
        .await
        .expect("the call is placed");
    let (incoming, offered) = expect(bob_events, SOON, |e| match e {
        Event::IncomingCall { call, video, .. } => Some((*call, *video)),
        _ => None,
    })
    .await
    .expect("bob rings");
    bob.answer_with_media(incoming, answer)
        .await
        .expect("bob answers");
    // Alice's camera starts as the answer arrives, before she is told.
    let mut early = Vec::new();
    expect(alice_events, SOON, |e| match e {
        Event::VideoStarted {
            call,
            stream,
            direction,
            kind,
            ..
        } if *call == out => {
            early.push((*stream, *direction, *kind));
            None
        }
        Event::CallEstablished { call, .. } if *call == out => Some(()),
        _ => None,
    })
    .await
    .expect("alice hears the answer");
    (out, incoming, offered, early)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_video_call_carries_each_camera_to_the_other_side() {
    for (srtp, a_ip, b_ip) in [
        (SrtpMode::Off, "127.0.1.1", "127.0.1.2"),
        (SrtpMode::Required, "127.0.1.3", "127.0.1.4"),
    ] {
        let (a_host, b_host) = (host(), host());
        let (alice, mut alice_events) = phone("alice", a_ip, srtp, Some(a_host.clone())).await;
        let (bob, mut bob_events) = phone("bob", b_ip, srtp, Some(b_host.clone())).await;
        assert!(alice.has_video() && bob.has_video());

        let (out, incoming, offered, early) = video_call(
            &alice,
            &mut alice_events,
            &bob,
            &mut bob_events,
            CallMedia::VIDEO,
        )
        .await;
        assert!(offered, "{srtp:?}: bob is told the call offers video");

        for (events, call, seen) in [
            (&mut alice_events, out, early),
            (&mut bob_events, incoming, Vec::new()),
        ] {
            let mut streams = started(events, call, 2, seen).await;
            streams.sort_by_key(|(_, d, _)| *d == VideoStreamDirection::Outgoing);
            assert_eq!(streams[0].1, VideoStreamDirection::Incoming);
            assert_eq!(streams[1].1, VideoStreamDirection::Outgoing);
            assert!(streams.iter().all(|(_, _, k)| *k == VideoKind::Camera));
        }
        for (who, h) in [("alice", &a_host), ("bob", &b_host)] {
            let s = receives(h, 40).await;
            in_order(&s);
            let sent = &h.record().sent;
            assert_eq!(sent.len(), 1, "{who}: one camera");
        }
        // H.264 first, as the core prefers it.
        assert_eq!(
            b_host.record().sent[0].config.codec,
            anvil_core::video::host::VideoCodec::H264
        );

        alice.hangup(out).await.expect("BYE");
        hung_up_by_the_other_side(&mut bob_events, incoming).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        for h in [&a_host, &b_host] {
            let r = h.record();
            assert!(
                r.sent.iter().all(|s| s.stopped),
                "cameras stop with the call"
            );
            assert!(
                r.received.values().all(|s| s.stopped),
                "tiles go with the call"
            );
        }
        alice.shutdown().await.unwrap();
        bob.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_video_call_answered_with_audio_only_has_no_video() {
    let (a_host, b_host) = (host(), host());
    let (alice, mut alice_events) =
        phone("alice", "127.0.1.5", SrtpMode::Off, Some(a_host.clone())).await;
    let (bob, mut bob_events) =
        phone("bob", "127.0.1.6", SrtpMode::Off, Some(b_host.clone())).await;
    let (out, incoming, offered, _) = video_call(
        &alice,
        &mut alice_events,
        &bob,
        &mut bob_events,
        CallMedia::AUDIO,
    )
    .await;
    assert!(offered);
    tokio::time::sleep(Duration::from_millis(500)).await;
    for h in [&a_host, &b_host] {
        let r = h.record();
        assert!(r.sent.is_empty(), "no camera: {r:?}");
        assert!(r.received.is_empty(), "no tile: {r:?}");
    }
    // The call itself is up, and holds as any call does.
    alice.hold(out, true).await.expect("hold");
    alice.hold(out, false).await.expect("resume");
    alice.hangup(out).await.expect("BYE");
    hung_up_by_the_other_side(&mut bob_events, incoming).await;
    alice.shutdown().await.unwrap();
    bob.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_softphone_without_video_answers_audio_and_offers_none() {
    let a_host = host();
    let (alice, mut alice_events) =
        phone("alice", "127.0.1.7", SrtpMode::Off, Some(a_host.clone())).await;
    let (bob, mut bob_events) = phone("bob", "127.0.1.8", SrtpMode::Off, None).await;
    assert!(!bob.has_video());

    // Alice offers video; Bob cannot take it, and is not told it is there.
    let (out, incoming, offered, _) = video_call(
        &alice,
        &mut alice_events,
        &bob,
        &mut bob_events,
        CallMedia::VIDEO,
    )
    .await;
    assert!(!offered, "bob has no video to answer with");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(a_host.record().sent.is_empty(), "declined: no camera");
    bob.hold(incoming, true)
        .await
        .expect("bob holds a call with a declined section");
    bob.hold(incoming, false).await.expect("and resumes it");
    alice.hangup(out).await.expect("BYE");
    hung_up_by_the_other_side(&mut bob_events, incoming).await;

    // Bob calls Alice asking for video: he has none, so none is offered.
    let target = format!("sip:alice@{}", alice.sip_address());
    let out = bob
        .place_call_with_media(&target, CallMedia::VIDEO)
        .await
        .unwrap();
    let offered = expect(&mut alice_events, SOON, |e| match e {
        Event::IncomingCall { video, .. } => Some(*video),
        _ => None,
    })
    .await
    .expect("alice rings");
    assert!(!offered);
    bob.hangup(out).await.expect("CANCEL");
    alice.shutdown().await.unwrap();
    bob.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hold_stops_video_both_ways_and_resuming_brings_it_back() {
    let (a_host, b_host) = (host(), host());
    let (alice, mut alice_events) = phone(
        "alice",
        "127.0.1.9",
        SrtpMode::Optional,
        Some(a_host.clone()),
    )
    .await;
    let (bob, mut bob_events) = phone(
        "bob",
        "127.0.1.10",
        SrtpMode::Optional,
        Some(b_host.clone()),
    )
    .await;
    let (out, incoming, _, _) = video_call(
        &alice,
        &mut alice_events,
        &bob,
        &mut bob_events,
        CallMedia::VIDEO,
    )
    .await;
    receives(&a_host, 10).await;
    receives(&b_host, 10).await;

    // Each side holds in turn: the caller, then the callee.
    for (who, call) in [(&alice, out), (&bob, incoming)] {
        who.hold(call, true).await.expect("hold");
        tokio::time::sleep(Duration::from_millis(200)).await;
        let before = (receives(&a_host, 0).await, receives(&b_host, 0).await);
        tokio::time::sleep(Duration::from_millis(500)).await;
        let during = (receives(&a_host, 0).await, receives(&b_host, 0).await);
        assert_eq!(
            before.0.frames.len(),
            during.0.frames.len(),
            "held: alice sees none"
        );
        assert_eq!(
            before.1.frames.len(),
            during.1.frames.len(),
            "held: bob sees none"
        );

        who.hold(call, false).await.expect("resume");
        for (h, shown) in [
            (&a_host, during.0.frames.len()),
            (&b_host, during.1.frames.len()),
        ] {
            let s = receives(h, shown + 10).await;
            assert!(s.frames[shown].1, "the picture comes back on a keyframe");
            assert_eq!(s.lost, 0, "a hold is not a loss");
        }
    }

    alice.hangup(out).await.expect("BYE");
    hung_up_by_the_other_side(&mut bob_events, incoming).await;
    alice.shutdown().await.unwrap();
    bob.shutdown().await.unwrap();
}

/// The next stats of `call` for a stream going `direction` that `ok`
/// accepts.
async fn stats_until(
    events: &mut EventStream,
    call: CallId,
    direction: VideoStreamDirection,
    ok: impl Fn(&VideoStreamStats) -> bool,
) -> VideoStreamStats {
    expect(events, Duration::from_secs(10), |e| match e {
        Event::VideoStats { call: c, stats, .. }
            if *c == call && stats.direction == direction && ok(stats) =>
        {
            Some(*stats)
        }
        _ => None,
    })
    .await
    .unwrap_or_else(|| panic!("no {direction:?} stats as wanted"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_video_call_reports_its_streams_and_keeps_the_picture_with_the_sound() {
    let (a_host, b_host) = (host(), host());
    let (alice, mut alice_events) = phone(
        "alice",
        "127.0.1.11",
        SrtpMode::Required,
        Some(a_host.clone()),
    )
    .await;
    let (bob, mut bob_events) = phone(
        "bob",
        "127.0.1.12",
        SrtpMode::Required,
        Some(b_host.clone()),
    )
    .await;
    let (out, incoming, _, _) = video_call(
        &alice,
        &mut alice_events,
        &bob,
        &mut bob_events,
        CallMedia::VIDEO,
    )
    .await;

    // Alice's camera, as she sends it and as Bob's reports say it went.
    let sent = stats_until(
        &mut alice_events,
        out,
        VideoStreamDirection::Outgoing,
        |s| s.rtt_ms.is_some(),
    )
    .await;
    assert_eq!(sent.loss_pct, Some(0.0), "{sent:?}");
    assert_eq!(sent.target_kbps, Some(1500), "a clear path: {sent:?}");
    assert!(sent.fps > 40.0, "{sent:?}");

    // Bob's view of it: its rate, the estimate he sends back, and the
    // picture against Alice's voice once her audio's and her video's
    // sender reports tie them to her clock.
    let seen = stats_until(
        &mut bob_events,
        incoming,
        VideoStreamDirection::Incoming,
        |s| s.av_offset_ms.is_some() && s.estimate_kbps.is_some(),
    )
    .await;
    assert!(seen.fps > 40.0 && seen.kbps > 1000.0, "{seen:?}");
    assert!(seen.estimate_kbps.unwrap() >= 1500, "{seen:?}");
    // The video's delay against the audio's playback: within a frame
    // or two.
    let offset = seen.av_offset_ms.unwrap();
    assert!(offset.abs() < 50.0, "in sync: {seen:?}");
    in_order(&receives(&b_host, 100).await);

    alice.hangup(out).await.expect("BYE");
    hung_up_by_the_other_side(&mut bob_events, incoming).await;
    alice.shutdown().await.unwrap();
    bob.shutdown().await.unwrap();
}
