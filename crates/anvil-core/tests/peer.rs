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
