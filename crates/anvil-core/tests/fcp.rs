//! Anvil against FCP, the server it is built for: users made through FCP's
//! admin API register with digest, call each other through FCP's B2BUA both
//! ways, hang up from either side, decline, stay registered past the expiry
//! FCP granted, and meet FCP's minimum expiry.
//!
//! Skipped unless these name a running FCP (an admin and a call manager):
//!
//! - `ANVIL_FCP_ADMIN`: the admin's base URL, e.g. `http://127.0.0.1:8080`;
//! - `ANVIL_FCP_TOKEN`: a token that may create users (the static token);
//! - `ANVIL_FCP_SIP`: the call manager's SIP address, e.g. `127.0.0.1:5060`;
//! - `ANVIL_FCP_DOMAIN`: a SIP domain the users are made in (default
//!   `anvil.test`); a tenant `anvil-tests` is made for it if no tenant
//!   has it;
//! - `ANVIL_FCP_PUSH_FCM_PROJECT`, optionally: an FCM project the call
//!   manager's `[push]` lists, for the test of a push address it takes.
//!
//! CI runs it against FCP's compose stack from the nightly images.

mod common;

use std::time::Duration;

use anvil_core::{EndReason, Event, PushAddress, PushProvider, PushState, RegState};
use common::*;
use rand::Rng;
use serde_json::{json, Value};

struct Fcp {
    admin: String,
    token: String,
    sip: String,
    domain: String,
    tenant: String,
    http: reqwest::Client,
}

/// A user made for one test, removed when the test ends.
struct User {
    username: String,
    password: String,
    extension: String,
}

impl Fcp {
    async fn from_env() -> Option<Self> {
        let mut fcp = Self {
            admin: std::env::var("ANVIL_FCP_ADMIN").ok()?,
            token: std::env::var("ANVIL_FCP_TOKEN").ok()?,
            sip: std::env::var("ANVIL_FCP_SIP").ok()?,
            domain: std::env::var("ANVIL_FCP_DOMAIN").unwrap_or_else(|_| "anvil.test".into()),
            tenant: String::new(),
            http: reqwest::Client::new(),
        };
        fcp.tenant = fcp.tenant_for_domain().await;
        Some(fcp)
    }

    /// The tenant that has the domain, made if none has.
    async fn tenant_for_domain(&self) -> String {
        let tenants: Value = self
            .http
            .get(format!("{}/api/v1/tenants?limit=500", self.admin))
            .bearer_auth(&self.token)
            .send()
            .await
            .expect("the admin answers")
            .json()
            .await
            .expect("a tenant list");
        let has_domain = |t: &&Value| {
            t["sip_domains"]
                .as_array()
                .is_some_and(|d| d.iter().any(|d| d == self.domain.as_str()))
        };
        if let Some(t) = tenants["data"]
            .as_array()
            .and_then(|l| l.iter().find(has_domain))
        {
            return t["id"].as_str().expect("a tenant id").to_string();
        }
        let response = self
            .http
            .post(format!("{}/api/v1/tenants", self.admin))
            .bearer_auth(&self.token)
            .json(&json!({"name": "anvil-tests", "sip_domains": [self.domain]}))
            .send()
            .await
            .expect("the admin answers");
        let status = response.status();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        if status == reqwest::StatusCode::CONFLICT {
            // Another test made it first.
            return Box::pin(self.tenant_for_domain()).await;
        }
        assert!(status.is_success(), "creating the tenant: {status} {body}");
        body["id"].as_str().expect("a tenant id").to_string()
    }

    /// A user with a fresh name, extension and password.
    async fn user(&self, name: &str) -> User {
        let n: u32 = rand::thread_rng().gen_range(100_000..999_999);
        let user = User {
            username: format!("anvil-{name}-{n}"),
            password: format!("Anvil-{n}-pw!"),
            extension: format!("7{n}"),
        };
        let response = self
            .http
            .post(format!("{}/api/v1/users", self.admin))
            .bearer_auth(&self.token)
            .header("X-FCP-Tenant-Id", &self.tenant)
            .json(&json!({
                "username": user.username,
                "password": user.password,
                "sip_provisioning_password": user.password,
                "extension": user.extension,
                "tenant_id": self.tenant,
            }))
            .send()
            .await
            .expect("the admin answers");
        let status = response.status();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        assert!(
            status.is_success(),
            "creating {}: {status} {body}",
            user.username
        );
        user
    }

    async fn remove(&self, user: &User) {
        let _ = self
            .http
            .delete(format!("{}/api/v1/users/{}", self.admin, user.username))
            .bearer_auth(&self.token)
            .header("X-FCP-Tenant-Id", &self.tenant)
            .send()
            .await;
    }

    /// An Anvil for `user`, bound to loopback address `ip`, asking for
    /// `expires` seconds.
    async fn anvil(
        &self,
        user: &User,
        ip: &str,
        expires: u64,
    ) -> (anvil_core::Anvil, anvil_core::EventStream) {
        self.anvil_with(user, ip, expires, |_| {}).await
    }

    /// [`anvil`](Self::anvil), its account changed by `change` first.
    async fn anvil_with(
        &self,
        user: &User,
        ip: &str,
        expires: u64,
        change: impl FnOnce(&mut anvil_core::AccountConfig),
    ) -> (anvil_core::Anvil, anvil_core::EventStream) {
        // Registered in the tenant's domain, through the call manager as
        // the outbound proxy, as FCP's provisioning tells a phone to.
        let mut account = account(
            &format!("sip:{}@{}", user.username, self.domain),
            &format!("sip:{}", self.domain),
            &user.username,
            &user.password,
            ip,
            expires,
        );
        account.outbound_proxy = Some(format!("sip:{}", self.sip));
        change(&mut account);
        start(account).await
    }

    /// Where to call `user`: their name in the tenant's domain.
    fn uri(&self, user: &User) -> String {
        format!("sip:{}@{}", user.username, self.domain)
    }
}

macro_rules! fcp_or_skip {
    () => {
        match Fcp::from_env().await {
            Some(fcp) => fcp,
            None => {
                eprintln!("ANVIL_FCP_ADMIN, ANVIL_FCP_TOKEN and ANVIL_FCP_SIP not set; skipping");
                return;
            }
        }
    };
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_anvils_call_each_other_through_fcp() {
    let fcp = fcp_or_skip!();
    let (alice_user, bob_user) = (fcp.user("alice").await, fcp.user("bob").await);
    let (alice, mut alice_events) = fcp.anvil(&alice_user, "127.0.0.20", 3600).await;
    let (bob, mut bob_events) = fcp.anvil(&bob_user, "127.0.0.21", 3600).await;
    register(&alice, &mut alice_events).await;
    register(&bob, &mut bob_events).await;

    // Alice calls Bob; Alice hangs up.
    let (out, incoming) = answered_call(
        &alice,
        &mut alice_events,
        &bob,
        &mut bob_events,
        &fcp.uri(&bob_user),
    )
    .await;
    alice.hangup(out).await.expect("BYE");
    hung_up_by_the_other_side(&mut bob_events, incoming).await;

    // Bob calls Alice; Alice, the callee, hangs up.
    let (out, incoming) = answered_call(
        &bob,
        &mut bob_events,
        &alice,
        &mut alice_events,
        &fcp.uri(&alice_user),
    )
    .await;
    alice.hangup(incoming).await.expect("BYE from the callee");
    hung_up_by_the_other_side(&mut bob_events, out).await;

    // Alice gives up while Bob's phone rings: FCP cancels Bob's leg.
    let out = alice.place_call(&fcp.uri(&bob_user)).await.unwrap();
    let incoming = rings(&mut bob_events).await;
    alice.hangup(out).await.expect("CANCEL");
    ends_once(&mut alice_events, out, |r| {
        matches!(r, EndReason::LocalHangup)
    })
    .await;
    ends_once(&mut bob_events, incoming, |_| true).await;

    // Bob declines: Alice's call is refused.
    let out = alice.place_call(&fcp.uri(&bob_user)).await.unwrap();
    let incoming = rings(&mut bob_events).await;
    bob.hangup(incoming).await.expect("603");
    ends_once(&mut alice_events, out, |r| {
        matches!(r, EndReason::Rejected(_))
    })
    .await;

    alice.unregister().await.expect("unregisters");
    bob.unregister().await.expect("unregisters");
    alice.shutdown().await.unwrap();
    bob.shutdown().await.unwrap();
    fcp.remove(&alice_user).await;
    fcp.remove(&bob_user).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_registration_outlives_the_expiry_fcp_granted() {
    let fcp = fcp_or_skip!();
    let (alice_user, bob_user) = (fcp.user("alice").await, fcp.user("bob").await);
    // A minute, FCP's least by default: refreshed at 54 seconds.
    let (alice, mut alice_events) = fcp.anvil(&alice_user, "127.0.0.22", 60).await;
    let (bob, mut bob_events) = fcp.anvil(&bob_user, "127.0.0.23", 3600).await;
    register(&alice, &mut alice_events).await;
    register(&bob, &mut bob_events).await;

    // Past the grant, Bob still reaches Alice: she refreshed.
    tokio::time::sleep(Duration::from_secs(66)).await;
    let (out, incoming) = answered_call(
        &bob,
        &mut bob_events,
        &alice,
        &mut alice_events,
        &fcp.uri(&alice_user),
    )
    .await;
    alice.hangup(incoming).await.unwrap();
    hung_up_by_the_other_side(&mut bob_events, out).await;
    let failed = expect(&mut alice_events, Duration::from_millis(200), |e| match e {
        Event::RegistrationChanged {
            state: RegState::Failed,
            ..
        } => Some(()),
        _ => None,
    })
    .await;
    assert!(failed.is_none(), "a refresh failed");

    alice.shutdown().await.unwrap();
    bob.shutdown().await.unwrap();
    fcp.remove(&alice_user).await;
    fcp.remove(&bob_user).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fcps_minimum_expiry_is_met() {
    let fcp = fcp_or_skip!();
    let alice_user = fcp.user("alice").await;
    // Thirty seconds is under FCP's minimum: 423, then its Min-Expires.
    let (alice, mut alice_events) = fcp.anvil(&alice_user, "127.0.0.24", 30).await;
    register(&alice, &mut alice_events).await;
    alice.shutdown().await.unwrap();
    fcp.remove(&alice_user).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wrong_password_is_refused() {
    let fcp = fcp_or_skip!();
    let mut alice_user = fcp.user("alice").await;
    alice_user.password = "not-the-password".into();
    let (alice, mut alice_events) = fcp.anvil(&alice_user, "127.0.0.25", 3600).await;
    assert!(
        alice.register().await.is_err(),
        "registered with a wrong password"
    );
    let failed = expect(&mut alice_events, SOON, |e| match e {
        Event::RegistrationChanged {
            state: RegState::Failed,
            reason,
        } => Some(reason.clone()),
        _ => None,
    })
    .await;
    assert!(failed.is_some(), "no failure reported");
    alice.shutdown().await.unwrap();
    fcp.remove(&alice_user).await;
}

/// An app's push address at an FCM project, with a fresh token.
fn fcm_address(project: &str) -> PushAddress {
    let n: u64 = rand::thread_rng().gen();
    PushAddress {
        provider: PushProvider::Fcm,
        param: project.to_string(),
        prid: format!("anvil-token-{n:016x}"),
    }
}

fn instance() -> String {
    let n: u128 = rand::thread_rng().gen();
    let h = format!("{n:032x}");
    format!(
        "<urn:uuid:{}-{}-{}-{}-{}>",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

async fn push_state(events: &mut anvil_core::EventStream) -> Option<PushState> {
    expect(events, SOON, |e| match e {
        Event::PushChanged { state } => Some(state.clone()),
        _ => None,
    })
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_fcp_cannot_wake_registers_without_push_and_still_rings() {
    let fcp = fcp_or_skip!();
    let (alice_user, bob_user) = (fcp.user("alice").await, fcp.user("bob").await);
    // A project no call manager pushes for: 555, and Alice's app registers
    // again without it.
    let (alice, mut alice_events) = fcp
        .anvil_with(&alice_user, "127.0.0.26", 3600, |account| {
            account.instance_id = Some(instance());
            account.push = Some(fcm_address("anvil-no-such-project"));
        })
        .await;
    let (bob, mut bob_events) = fcp.anvil(&bob_user, "127.0.0.27", 3600).await;
    alice.register().await.expect("registers without push");
    let refused = push_state(&mut alice_events).await;
    assert!(
        matches!(refused, Some(PushState::Refused { .. })),
        "FCP should refuse the push address: {refused:?}"
    );
    register(&bob, &mut bob_events).await;

    // Awake, Alice's app rings as any phone does.
    let (out, incoming) = answered_call(
        &bob,
        &mut bob_events,
        &alice,
        &mut alice_events,
        &fcp.uri(&alice_user),
    )
    .await;
    alice.hangup(incoming).await.unwrap();
    hung_up_by_the_other_side(&mut bob_events, out).await;

    alice.shutdown().await.unwrap();
    bob.shutdown().await.unwrap();
    fcp.remove(&alice_user).await;
    fcp.remove(&bob_user).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fcp_takes_the_push_address_of_an_app_it_can_wake() {
    let fcp = fcp_or_skip!();
    let Ok(project) = std::env::var("ANVIL_FCP_PUSH_FCM_PROJECT") else {
        eprintln!("ANVIL_FCP_PUSH_FCM_PROJECT not set; skipping");
        return;
    };
    let alice_user = fcp.user("alice").await;
    let (alice, mut alice_events) = fcp
        .anvil_with(&alice_user, "127.0.0.28", 3600, |account| {
            account.instance_id = Some(instance());
            account.push = Some(fcm_address(&project));
        })
        .await;
    alice.register().await.expect("registers");
    assert_eq!(
        push_state(&mut alice_events).await,
        Some(PushState::Accepted)
    );

    // A renewed token, then push off: each registered at once.
    alice
        .set_push(Some(fcm_address(&project)))
        .await
        .expect("the new token");
    alice.set_push(None).await.expect("push off");
    assert_eq!(push_state(&mut alice_events).await, Some(PushState::Off));

    alice.shutdown().await.unwrap();
    fcp.remove(&alice_user).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fcp_names_its_call_in_the_invite() {
    // As a push names it (FCP's `docs/PUSH.md` §2.5): a woken app knows
    // which INVITE is the call it shows. FCP before v0.25.382 names
    // nothing, and the test says so.
    let fcp = fcp_or_skip!();
    let (alice_user, bob_user) = (fcp.user("alice").await, fcp.user("bob").await);
    let (alice, mut alice_events) = fcp.anvil(&alice_user, "127.0.0.29", 3600).await;
    let (bob, mut bob_events) = fcp.anvil(&bob_user, "127.0.0.30", 3600).await;
    register(&alice, &mut alice_events).await;
    register(&bob, &mut bob_events).await;

    let out = alice.place_call(&fcp.uri(&bob_user)).await.unwrap();
    let (incoming, named) = expect(&mut bob_events, SOON, |e| match e {
        Event::IncomingCall {
            call, server_call, ..
        } => Some((*call, server_call.clone())),
        _ => None,
    })
    .await
    .expect("Bob's phone rings");
    match named {
        Some(id) => assert!(
            id.len() == 36 && id.chars().filter(|c| *c == '-').count() == 4,
            "FCP's id for the call: {id:?}"
        ),
        None => eprintln!("this FCP does not name its calls (before v0.25.382)"),
    }
    alice.hangup(out).await.expect("CANCEL");
    ends_once(&mut bob_events, incoming, |_| true).await;

    alice.shutdown().await.unwrap();
    bob.shutdown().await.unwrap();
    fcp.remove(&alice_user).await;
    fcp.remove(&bob_user).await;
}

/// RTCP through FCP's media relay (FCP's `docs/CALL_DIAGNOSTICS.md` phase
/// 1): the relay passes each phone's reports to the other, so each reads
/// the far phone's view of its stream and the round trip end to end.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_phone_reads_the_others_reports_through_fcp() {
    let fcp = fcp_or_skip!();
    let (alice_user, bob_user) = (fcp.user("alice").await, fcp.user("bob").await);
    let (alice, mut alice_events) = fcp.anvil(&alice_user, "127.0.0.22", 3600).await;
    let (bob, mut bob_events) = fcp.anvil(&bob_user, "127.0.0.23", 3600).await;
    register(&alice, &mut alice_events).await;
    register(&bob, &mut bob_events).await;
    let (out, incoming) = answered_call(
        &alice,
        &mut alice_events,
        &bob,
        &mut bob_events,
        &fcp.uri(&bob_user),
    )
    .await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    for (who, anvil, call) in [("Alice", &alice, out), ("Bob", &bob, incoming)] {
        loop {
            let stats = anvil.call_stats(call).expect("the call has media");
            if let (true, Some(_), Some(rtt)) =
                (stats.rtcp_received >= 2, stats.remote, stats.rtt_ms)
            {
                assert!(rtt < 500.0, "{who} through the relay: {stats:?}");
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "{who} never read the other phone's reports through FCP: {stats:?}"
            );
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    alice.hangup(out).await.expect("BYE");
    hung_up_by_the_other_side(&mut bob_events, incoming).await;
    alice.unregister().await.expect("unregisters");
    bob.unregister().await.expect("unregisters");
    alice.shutdown().await.unwrap();
    bob.shutdown().await.unwrap();
    fcp.remove(&alice_user).await;
    fcp.remove(&bob_user).await;
}
