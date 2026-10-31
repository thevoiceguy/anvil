//! Two Anvils calling each other directly, with no server between them:
//! answered and hung up both ways, cancelled while ringing, declined, and
//! over TCP. Runs everywhere: no network beyond the loopback.

mod common;

use anvil_core::EndReason;
use common::*;

async fn pair(
    a: &str,
    b: &str,
) -> (
    (anvil_core::Anvil, anvil_core::EventStream),
    (anvil_core::Anvil, anvil_core::EventStream),
) {
    // No registrar: the accounts are never registered.
    let alice = start(account(
        "sip:alice@127.0.0.1",
        "sip:127.0.0.1:9",
        "alice",
        "",
        a,
        3600,
    ))
    .await;
    let bob = start(account(
        "sip:bob@127.0.0.1",
        "sip:127.0.0.1:9",
        "bob",
        "",
        b,
        3600,
    ))
    .await;
    (alice, bob)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_anvils_call_hang_up_cancel_and_decline() {
    let ((alice, mut alice_events), (bob, mut bob_events)) = pair("127.0.0.2", "127.0.0.3").await;
    let bob_uri = format!("sip:bob@{}", bob.sip_address());
    let alice_uri = format!("sip:alice@{}", alice.sip_address());

    // Alice calls Bob; Alice hangs up.
    let (out, incoming) =
        answered_call(&alice, &mut alice_events, &bob, &mut bob_events, &bob_uri).await;
    alice.hangup(out).await.expect("BYE");
    hung_up_by_the_other_side(&mut bob_events, incoming).await;

    // Bob calls Alice; Alice, the callee, hangs up.
    let (out, incoming) =
        answered_call(&bob, &mut bob_events, &alice, &mut alice_events, &alice_uri).await;
    alice.hangup(incoming).await.expect("BYE from the callee");
    hung_up_by_the_other_side(&mut bob_events, out).await;

    // Alice calls Bob and gives up while it rings: a CANCEL, reported once.
    let out = alice.place_call(&bob_uri).await.unwrap();
    let incoming = rings(&mut bob_events).await;
    alice.hangup(out).await.expect("CANCEL");
    ends_once(&mut alice_events, out, |r| {
        matches!(r, EndReason::LocalHangup)
    })
    .await;
    ends_once(&mut bob_events, incoming, |_| true).await;

    // Bob declines a call still ringing: Alice hears 603.
    let out = alice.place_call(&bob_uri).await.unwrap();
    let incoming = rings(&mut bob_events).await;
    bob.hangup(incoming).await.expect("603");
    ends_once(&mut alice_events, out, |r| {
        matches!(r, EndReason::Rejected(603))
    })
    .await;

    alice.shutdown().await.unwrap();
    bob.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_call_over_tcp_is_answered_on_its_connection() {
    let ((alice, mut alice_events), (bob, mut bob_events)) = pair("127.0.0.4", "127.0.0.5").await;
    let tcp_uri = format!("sip:bob@{};transport=tcp", bob.sip_address());
    let (out, incoming) =
        answered_call(&alice, &mut alice_events, &bob, &mut bob_events, &tcp_uri).await;
    bob.hangup(incoming).await.expect("BYE over TCP");
    hung_up_by_the_other_side(&mut alice_events, out).await;
    alice.shutdown().await.unwrap();
    bob.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn either_side_holds_and_resumes() {
    let ((alice, mut alice_events), (bob, mut bob_events)) = pair("127.0.0.6", "127.0.0.7").await;
    let bob_uri = format!("sip:bob@{}", bob.sip_address());
    let (out, incoming) =
        answered_call(&alice, &mut alice_events, &bob, &mut bob_events, &bob_uri).await;

    // The caller holds and resumes; then the callee, whose re-INVITE the
    // caller must find in the dialog of a call it placed.
    let soon = std::time::Duration::from_secs(5);
    for (who, call) in [(&alice, out), (&bob, incoming)] {
        for on in [true, false] {
            tokio::time::timeout(soon, who.hold(call, on))
                .await
                .expect("the re-INVITE is answered at once")
                .expect("the re-INVITE is accepted");
        }
    }

    alice.hangup(out).await.expect("BYE");
    hung_up_by_the_other_side(&mut bob_events, incoming).await;
    alice.shutdown().await.unwrap();
    bob.shutdown().await.unwrap();
}

/// RTCP both ways (FCP's `docs/CALL_DIAGNOSTICS.md` phase 1): each side's
/// reports reach the other, which reads the far end's view of its stream
/// and the round trip from them; plain and over SRTP.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_side_reads_the_others_reports_and_the_round_trip() {
    use anvil_core::config::{MediaConfig, SrtpMode};
    for (srtp, a, b) in [
        (SrtpMode::Off, "127.0.0.8", "127.0.0.9"),
        (SrtpMode::Required, "127.0.0.10", "127.0.0.11"),
    ] {
        let media = || MediaConfig {
            srtp,
            ..MediaConfig::default()
        };
        let (alice, mut alice_events) = start_with(
            account(
                "sip:alice@127.0.0.1",
                "sip:127.0.0.1:9",
                "alice",
                "",
                a,
                3600,
            ),
            media(),
        )
        .await;
        let (bob, mut bob_events) = start_with(
            account("sip:bob@127.0.0.1", "sip:127.0.0.1:9", "bob", "", b, 3600),
            media(),
        )
        .await;
        let bob_uri = format!("sip:bob@{}", bob.sip_address());
        let (out, incoming) =
            answered_call(&alice, &mut alice_events, &bob, &mut bob_events, &bob_uri).await;

        // A report a second in, then every five: two each way within eight.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        for (who, anvil, call) in [("Alice", &alice, out), ("Bob", &bob, incoming)] {
            loop {
                let stats = anvil.call_stats(call).expect("the call has media");
                assert!(stats.rtcp, "{who} has an RTCP socket");
                if let (true, Some(remote), Some(rtt)) =
                    (stats.rtcp_received >= 2, stats.remote, stats.rtt_ms)
                {
                    assert_eq!(stats.srtp, srtp != SrtpMode::Off, "{who}: {stats:?}");
                    assert!(
                        stats.packets_sent > 0 && stats.packets_received > 0,
                        "{stats:?}"
                    );
                    assert_eq!(remote.cumulative_lost, 0, "{who} over loopback: {stats:?}");
                    assert!(rtt < 200.0, "{who}: {stats:?}");
                    break;
                }
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "{who} never read the other's reports ({srtp:?}): {stats:?}"
                );
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        }

        // Each second's stats carry the rates and a score each way (phase 2):
        // a clean loopback call is good both ways at 50 packets a second.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        let stats = loop {
            let event = tokio::time::timeout_at(deadline, alice_events.recv())
                .await
                .expect("stats with the far end's score within five seconds")
                .expect("the stream is open");
            if let anvil_core::Event::MediaStats { call, stats } = event {
                if call == out && stats.far_mos.is_some() {
                    break stats;
                }
            }
        };
        assert_eq!(stats.recent_loss_pct, 0.0, "{stats:?}");
        assert!((40.0..60.0).contains(&stats.send_pps), "{stats:?}");
        assert!((40.0..60.0).contains(&stats.recv_pps), "{stats:?}");
        assert!(stats.send_kbps > 10.0, "{stats:?}");
        assert_eq!(
            stats.worst_mos().rating(),
            anvil_core::quality::Rating::Good,
            "{stats:?}"
        );

        alice.hangup(out).await.expect("BYE");
        hung_up_by_the_other_side(&mut bob_events, incoming).await;
        assert!(
            alice.call_stats(out).is_none(),
            "no stats once the call is gone"
        );
        alice.shutdown().await.unwrap();
        bob.shutdown().await.unwrap();
    }
}
