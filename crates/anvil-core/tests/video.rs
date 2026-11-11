//! Video between two Anvils calling each other directly (FCP's
//! `docs/SOFTPHONE_VIDEO.md` §4): offered and answered, in the clear and
//! over SRTP; answered with audio only; declined by a softphone without
//! video; stopped by hold and back on resume. Each side's host is the
//! synthetic one, so the frames are numbered and checked.

mod common;

use std::time::Duration;

use anvil_core::config::{MediaConfig, SrtpMode};
use anvil_core::video::host::{Resolution, StreamId, VideoKind};
use anvil_core::video::synthetic::{ReceivedStream, SyntheticRecord, SyntheticVideoHost};
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

/// An audio call from `alice` to `bob`, answered.
async fn audio_call(
    alice: &Anvil,
    alice_events: &mut EventStream,
    bob: &Anvil,
    bob_events: &mut EventStream,
) -> (CallId, CallId) {
    let target = format!("sip:bob@{}", bob.sip_address());
    let out = alice
        .place_call_with_media(&target, CallMedia::AUDIO)
        .await
        .expect("the call is placed");
    let incoming = rings(bob_events).await;
    bob.answer(incoming).await.expect("bob answers");
    expect(alice_events, SOON, |e| match e {
        Event::CallEstablished { call, .. } if *call == out => Some(()),
        _ => None,
    })
    .await
    .expect("alice hears the answer");
    (out, incoming)
}

/// Waits until `ok` holds of `host`'s record.
async fn until(what: &str, host: &SyntheticVideoHost, ok: impl Fn(&SyntheticRecord) -> bool) {
    let deadline = tokio::time::Instant::now() + SOON;
    loop {
        let record = host.record();
        if ok(&record) {
            return;
        }
        assert!(tokio::time::Instant::now() < deadline, "{what}: {record:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn live_frames(r: &SyntheticRecord) -> usize {
    r.received
        .values()
        .filter(|s| !s.stopped)
        .map(|s| s.frames.len())
        .sum()
}

fn cameras_open(r: &SyntheticRecord) -> usize {
    r.sent.iter().filter(|s| !s.stopped).count()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_audio_call_gains_video_one_camera_at_a_time_and_loses_it() {
    for (srtp, a_ip, b_ip) in [
        (SrtpMode::Off, "127.0.1.13", "127.0.1.14"),
        (SrtpMode::Required, "127.0.1.15", "127.0.1.16"),
    ] {
        let (a_host, b_host) = (host(), host());
        let (alice, mut alice_events) = phone("alice", a_ip, srtp, Some(a_host.clone())).await;
        let (bob, mut bob_events) = phone("bob", b_ip, srtp, Some(b_host.clone())).await;
        let (out, incoming) = audio_call(&alice, &mut alice_events, &bob, &mut bob_events).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(a_host.record().sent.is_empty() && b_host.record().sent.is_empty());

        // Alice turns her camera on: Bob is asked, and sees her; his own
        // camera stays shut until he says.
        alice.set_video(out, true).await.expect("alice's camera on");
        expect(&mut bob_events, SOON, |e| match e {
            Event::VideoRequested { call } if *call == incoming => Some(()),
            _ => None,
        })
        .await
        .unwrap_or_else(|| panic!("{srtp:?}: bob is asked"));
        in_order(&receives(&b_host, 30).await);
        assert_eq!(
            cameras_open(&b_host.record()),
            0,
            "bob's camera is his to open"
        );
        assert!(
            a_host.record().received.is_empty(),
            "alice sees nothing yet"
        );

        // Bob joins: both ways.
        bob.set_video(incoming, true)
            .await
            .expect("bob's camera on");
        in_order(&receives(&a_host, 30).await);
        assert_eq!(cameras_open(&b_host.record()), 1);

        // Alice turns hers off: her camera closes and Bob's tile of her
        // goes; she still sees him.
        alice
            .set_video(out, false)
            .await
            .expect("alice's camera off");
        until("alice's camera closes", &a_host, |r| cameras_open(r) == 0).await;
        until("bob's tile of alice goes", &b_host, |r| {
            r.received.values().all(|s| s.stopped)
        })
        .await;
        let seen = live_frames(&a_host.record());
        until("alice still sees bob", &a_host, |r| {
            live_frames(r) > seen + 20
        })
        .await;

        // Bob turns his off too: the video is gone both ways.
        bob.set_video(incoming, false)
            .await
            .expect("bob's camera off");
        until("bob's camera closes", &b_host, |r| cameras_open(r) == 0).await;
        until("alice's tile of bob goes", &a_host, |r| {
            r.received.values().all(|s| s.stopped)
        })
        .await;

        // And Alice can start it again, asking Bob anew.
        alice
            .set_video(out, true)
            .await
            .expect("alice's camera on again");
        expect(&mut bob_events, SOON, |e| match e {
            Event::VideoRequested { call } if *call == incoming => Some(()),
            _ => None,
        })
        .await
        .expect("bob is asked again");
        until("bob sees alice again", &b_host, |r| live_frames(r) > 10).await;

        // The call itself carried on throughout.
        alice.hangup(out).await.expect("BYE");
        hung_up_by_the_other_side(&mut bob_events, incoming).await;
        alice.shutdown().await.unwrap();
        bob.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_held_call_keeps_its_camera_choice_and_a_softphone_without_video_declines_one() {
    let (a_host, b_host) = (host(), host());
    let (alice, mut alice_events) =
        phone("alice", "127.0.1.17", SrtpMode::Off, Some(a_host.clone())).await;
    let (bob, mut bob_events) =
        phone("bob", "127.0.1.18", SrtpMode::Off, Some(b_host.clone())).await;
    let (out, incoming) = audio_call(&alice, &mut alice_events, &bob, &mut bob_events).await;
    alice.set_video(out, true).await.expect("alice's camera on");
    receives(&b_host, 10).await;

    // Bob holds and resumes: his camera, never turned on, stays shut.
    bob.hold(incoming, true).await.expect("hold");
    bob.hold(incoming, false).await.expect("resume");
    let shown = receives(&b_host, 0).await.frames.len();
    until("alice's picture is back", &b_host, |r| {
        live_frames(r) > shown + 10
    })
    .await;
    assert_eq!(
        cameras_open(&b_host.record()),
        0,
        "a resume opens no camera"
    );
    alice.hangup(out).await.expect("BYE");
    hung_up_by_the_other_side(&mut bob_events, incoming).await;

    // A softphone without video declines the upgrade, and the call goes on.
    let (carol, mut carol_events) = phone("carol", "127.0.1.19", SrtpMode::Off, None).await;
    let target = format!("sip:carol@{}", carol.sip_address());
    let out = alice
        .place_call_with_media(&target, CallMedia::AUDIO)
        .await
        .unwrap();
    let incoming = rings(&mut carol_events).await;
    carol.answer(incoming).await.unwrap();
    expect(&mut alice_events, SOON, |e| match e {
        Event::CallEstablished { call, .. } if *call == out => Some(()),
        _ => None,
    })
    .await
    .unwrap();
    alice
        .set_video(out, true)
        .await
        .expect("the offer is answered");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        cameras_open(&a_host.record()),
        0,
        "declined: alice's camera closes again"
    );
    assert!(
        carol.set_video(incoming, true).await.is_err(),
        "carol has none"
    );
    alice.hangup(out).await.expect("BYE");
    hung_up_by_the_other_side(&mut carol_events, incoming).await;
    for p in [alice, bob, carol] {
        p.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn both_cameras_turned_on_at_once_both_get_through() {
    let (a_host, b_host) = (host(), host());
    let (alice, mut alice_events) =
        phone("alice", "127.0.1.20", SrtpMode::Off, Some(a_host.clone())).await;
    let (bob, mut bob_events) =
        phone("bob", "127.0.1.21", SrtpMode::Off, Some(b_host.clone())).await;
    let (out, incoming) = audio_call(&alice, &mut alice_events, &bob, &mut bob_events).await;

    // The two offers cross: one side answers 491 and the other tries again
    // (RFC 3261 §14), and the call ends up with both cameras.
    let (a, b) = tokio::join!(alice.set_video(out, true), bob.set_video(incoming, true));
    a.expect("alice's camera on");
    b.expect("bob's camera on");
    in_order(&receives(&a_host, 20).await);
    in_order(&receives(&b_host, 20).await);
    assert_eq!(cameras_open(&a_host.record()), 1);
    assert_eq!(cameras_open(&b_host.record()), 1);

    alice.hangup(out).await.expect("BYE");
    hung_up_by_the_other_side(&mut bob_events, incoming).await;
    alice.shutdown().await.unwrap();
    bob.shutdown().await.unwrap();
}

/// The content source `host` started, once it has sent `frames`.
async fn content_sent(
    host: &SyntheticVideoHost,
    frames: u32,
) -> anvil_core::video::synthetic::SentSource {
    let deadline = tokio::time::Instant::now() + SOON;
    loop {
        let record = host.record();
        if let Some(s) = record
            .sent
            .iter()
            .find(|s| s.config.kind == VideoKind::Content && !s.stopped && s.frames >= frames)
        {
            return s.clone();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no shared screen of {frames} frames: {record:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The content streams `host` receives that are still live.
fn content_received(r: &SyntheticRecord) -> Vec<ReceivedStream> {
    r.received
        .values()
        .filter(|s| s.config.kind == VideoKind::Content && !s.stopped)
        .cloned()
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_screen_shared_in_a_room_is_sent_and_stopped_by_re_invite() {
    let (a_host, b_host) = (host(), host());
    let (alice, mut alice_events) =
        phone("alice", "127.0.2.1", SrtpMode::Off, Some(a_host.clone())).await;
    let (bob, mut bob_events) =
        phone("bob", "127.0.2.2", SrtpMode::Off, Some(b_host.clone())).await;

    // Bob stands in for the room: Alice joins with video, so her offer
    // has a shared-screen section she only receives on.
    let out = alice
        .join_room(
            &format!("sip:bob@{}", bob.sip_address()),
            &anvil_core::RoomJoin::default(),
            CallMedia::VIDEO,
        )
        .await
        .expect("the room is dialed");
    let incoming = expect(&mut bob_events, SOON, |e| match e {
        Event::IncomingCall { call, .. } => Some(*call),
        _ => None,
    })
    .await
    .expect("the room rings");
    bob.answer_with_media(incoming, CallMedia::VIDEO)
        .await
        .expect("the room answers");
    expect(&mut alice_events, SOON, |e| match e {
        Event::CallEstablished { call, .. } if *call == out => Some(()),
        _ => None,
    })
    .await
    .expect("alice is in");
    assert!(!alice.sharing(out));
    assert!(
        !a_host
            .record()
            .sent
            .iter()
            .any(|s| s.config.kind == VideoKind::Content),
        "no screen before a share"
    );
    assert_eq!(alice.screens().len(), 1);
    alice.set_screen(Some("synthetic-screen"));
    assert_eq!(a_host.record().screen.as_deref(), Some("synthetic-screen"));

    // She shares: a re-INVITE, then her screen reaches the room, sharper
    // and slower than a camera.
    alice
        .share_screen(out, true)
        .await
        .expect("the share is offered");
    assert!(alice.sharing(out));
    let sent = content_sent(&a_host, 10).await;
    assert_eq!(sent.config.max, Resolution::P1080);
    assert_eq!(sent.config.fps, 10);
    let deadline = tokio::time::Instant::now() + SOON;
    while !content_received(&b_host.record())
        .iter()
        .any(|s| s.frames.len() >= 5)
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the room never saw her screen: {:?}",
            b_host.record()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // Asking again changes nothing.
    alice.share_screen(out, true).await.unwrap();

    // She stops: her screen at once, the section kept (receiving only),
    // and nothing more of it reaches the room.
    alice
        .share_screen(out, false)
        .await
        .expect("the stop is offered");
    assert!(!alice.sharing(out));
    let deadline = tokio::time::Instant::now() + SOON;
    while !a_host
        .record()
        .sent
        .iter()
        .filter(|s| s.config.kind == VideoKind::Content)
        .all(|s| s.stopped)
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "her screen did not stop: {:?}",
            a_host.record()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let shown = |r: &SyntheticRecord| -> usize {
        r.received
            .values()
            .filter(|s| s.config.kind == VideoKind::Content)
            .map(|s| s.frames.len())
            .sum()
    };
    // What was in flight lands; then the count stands still.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let before = shown(&b_host.record());
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(shown(&b_host.record()), before, "her screen still arrives");

    // A call that is not a room has no screen to share.
    let (carol, mut carol_events) = phone("carol", "127.0.2.3", SrtpMode::Off, Some(host())).await;
    let (dave, mut dave_events) = phone("dave", "127.0.2.4", SrtpMode::Off, Some(host())).await;
    let (plain, _, _, _) = video_call(
        &carol,
        &mut carol_events,
        &dave,
        &mut dave_events,
        CallMedia::VIDEO,
    )
    .await;
    assert!(matches!(
        carol.share_screen(plain, true).await,
        Err(anvil_core::AnvilError::Config(_))
    ));
}
