//! Anvil signing in to FCP and calling with what FCP tells it: discovery,
//! the browser sign-in (the browser played by HTTP requests) and the
//! password sign-in, the install's device and its SIP account,
//! `/me/softphone` into an account that registers through the outbound
//! proxy, a call between two signed-in Anvils, refresh, a new password,
//! signing out.
//!
//! Skipped unless `ANVIL_FCP_ADMIN`, `ANVIL_FCP_TOKEN` (one that may create
//! users and tenants) and `ANVIL_FCP_SIP` name a running FCP; see
//! anvil-core's tests/fcp.rs. `ANVIL_FCP_DOMAIN` (default `anvil.test`) is
//! the domain the users are made in.

use std::sync::Arc;
use std::time::Duration;

use anvil_core::audio::{AudioFormat, AudioFrame, AudioHost, AudioSink, AudioSource, DeviceInfo};
use anvil_core::{
    Anvil, AnvilConfig, AnvilError, BrandConfig, Event, EventStream, MediaConfig, RegState,
};
use anvil_fcp::{discover, password_sign_in, AppClient, BrowserSignIn, FcpClient, FcpError};
use async_trait::async_trait;
use serde_json::{json, Value};

const SOON: Duration = Duration::from_secs(5);

struct Quiet;
struct QuietSource(AudioFormat, tokio::time::Interval);
struct QuietSink;

#[async_trait]
impl AudioSource for QuietSource {
    async fn next_frame(&mut self) -> Option<AudioFrame> {
        self.1.tick().await;
        let n =
            (self.0.sample_rate * self.0.frame_ms / 1000) as usize * usize::from(self.0.channels);
        Some(AudioFrame {
            samples: vec![0; n],
            format: self.0,
        })
    }
}

#[async_trait]
impl AudioSink for QuietSink {
    async fn write_frame(&mut self, _frame: &AudioFrame) -> Result<(), AnvilError> {
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
    fn make_playback(&self, _cfg: AudioFormat) -> Result<Box<dyn AudioSink>, AnvilError> {
        Ok(Box::new(QuietSink))
    }
    fn devices(&self) -> Vec<DeviceInfo> {
        Vec::new()
    }
}

/// The FCP a test runs against, and an administrator's way into it.
struct Fcp {
    admin: String,
    token: String,
    domain: String,
    tenant: String,
    http: reqwest::Client,
}

impl Fcp {
    async fn from_env() -> Option<Self> {
        let mut fcp = Self {
            admin: std::env::var("ANVIL_FCP_ADMIN").ok()?,
            token: std::env::var("ANVIL_FCP_TOKEN").ok()?,
            domain: std::env::var("ANVIL_FCP_DOMAIN").unwrap_or_else(|_| "anvil.test".into()),
            tenant: String::new(),
            http: reqwest::Client::new(),
        };
        std::env::var("ANVIL_FCP_SIP").ok()?;
        fcp.tenant = fcp.tenant_for_domain().await;
        Some(fcp)
    }

    async fn tenant_for_domain(&self) -> String {
        for _ in 0..3 {
            let tenants: Value = self
                .http
                .get(format!("{}/api/v1/tenants?limit=500", self.admin))
                .bearer_auth(&self.token)
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            if let Some(t) = tenants["data"].as_array().and_then(|l| {
                l.iter().find(|t| {
                    t["sip_domains"]
                        .as_array()
                        .is_some_and(|d| d.iter().any(|d| d == self.domain.as_str()))
                })
            }) {
                return t["id"].as_str().unwrap().to_string();
            }
            let _ = self
                .http
                .post(format!("{}/api/v1/tenants", self.admin))
                .bearer_auth(&self.token)
                .json(&json!({"name": "anvil-tests", "sip_domains": [self.domain]}))
                .send()
                .await;
        }
        panic!("no tenant for {}", self.domain);
    }

    /// A user with a web password, made for the test.
    async fn user(&self, name: &str) -> (String, String) {
        use rand::Rng;
        let n: u32 = rand::thread_rng().gen_range(100_000..999_999);
        let (username, password) = (format!("anvil-{name}-{n}"), format!("Anvil-{n}-pw!"));
        let made = self
            .http
            .post(format!("{}/api/v1/users", self.admin))
            .bearer_auth(&self.token)
            .header("X-FCP-Tenant-Id", &self.tenant)
            .json(&json!({"username": username, "password": password, "tenant_id": self.tenant}))
            .send()
            .await
            .unwrap();
        assert!(made.status().is_success(), "{:?}", made.text().await);
        (username, password)
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

async fn expect<T>(
    events: &mut EventStream,
    within: Duration,
    mut pick: impl FnMut(&Event) -> Option<T>,
) -> Option<T> {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let event = tokio::time::timeout_at(deadline, events.recv())
            .await
            .ok()??;
        if let Some(v) = pick(&event) {
            return Some(v);
        }
    }
}

/// An Anvil started from what FCP tells the signed-in app, bound to `ip`.
async fn softphone(client: &FcpClient, ip: &str) -> (Anvil, EventStream) {
    let config = client.softphone().await.expect("/me/softphone");
    let mut account = client.account_config(&config).await.expect("an account");
    account.bind_addr = Some(format!("{ip}:0"));
    let (anvil, mut events) = Anvil::start(AnvilConfig {
        account,
        media: MediaConfig::default(),
        audio: Box::new(Quiet),
        brand: BrandConfig::default(),
    })
    .await
    .expect("anvil starts");
    anvil
        .register()
        .await
        .expect("registers with FCP's settings");
    expect(&mut events, SOON, |e| match e {
        Event::RegistrationChanged {
            state: RegState::Registered,
            ..
        } => Some(()),
        _ => None,
    })
    .await
    .expect("registered");
    (anvil, events)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn anvil_signs_in_to_fcp_and_calls_with_what_it_is_told() {
    let fcp = fcp_or_skip!();
    let server = discover(&fcp.admin).await.expect("the well-known document");
    assert!(
        server.token_url.ends_with("/api/v1/auth/app/token"),
        "{server:?}"
    );

    // Two people sign in without a browser: each install is a device with a
    // SIP account of its own.
    let (alice_name, alice_pw) = fcp.user("alice").await;
    let (bob_name, bob_pw) = fcp.user("bob").await;
    let alice = password_sign_in(
        server.clone(),
        AppClient::this_machine(AppClient::new_install_id()),
        &alice_name,
        &alice_pw,
        None,
    )
    .await
    .expect("Alice signs in");
    let device = alice.device.clone().expect("the install's device");
    assert!(device.sip_username.starts_with("app-"), "{device:?}");
    let bob = password_sign_in(
        server.clone(),
        AppClient::this_machine(AppClient::new_install_id()),
        &bob_name,
        &bob_pw,
        None,
    )
    .await
    .expect("Bob signs in");
    let (alice, bob) = (FcpClient::new(alice, None), FcpClient::new(bob, None));

    // What they are told registers them through the outbound proxy; Alice
    // calls Bob in the domain.
    let config = alice.softphone().await.unwrap();
    assert!(
        config
            .account
            .aor
            .starts_with(&format!("sip:{alice_name}@")),
        "{config:?}"
    );
    assert!(config.account.outbound_proxy.is_some(), "{config:?}");
    let (alice_phone, mut alice_events) = softphone(&alice, "127.0.0.30").await;
    let (bob_phone, mut bob_events) = softphone(&bob, "127.0.0.31").await;
    let out = alice_phone
        .place_call(&format!("sip:{bob_name}@{}", config.account.domain))
        .await
        .unwrap();
    let incoming = expect(&mut bob_events, SOON, |e| match e {
        Event::IncomingCall { call, .. } => Some(*call),
        _ => None,
    })
    .await
    .expect("Bob's app rings");
    bob_phone.answer(incoming).await.unwrap();
    expect(&mut alice_events, SOON, |e| match e {
        Event::CallEstablished { call, .. } if *call == out => Some(()),
        _ => None,
    })
    .await
    .expect("answered");
    alice_phone.hangup(out).await.unwrap();

    // The session refreshes; a new SIP password registers.
    let before = alice.session().await.refresh_token;
    alice.refresh().await.expect("refreshes");
    assert_ne!(alice.session().await.refresh_token, before);
    alice.rotate_password().await.expect("a new SIP password");
    alice_phone.shutdown().await.unwrap();
    let (alice_phone, _events) = softphone(&alice, "127.0.0.32").await;
    alice_phone.shutdown().await.unwrap();
    bob_phone.shutdown().await.unwrap();

    // Signed out: FCP no longer takes the session.
    alice.sign_out().await.unwrap();
    assert!(matches!(alice.softphone().await, Err(FcpError::SignedOut)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn anvil_signs_in_through_the_browser() {
    let fcp = fcp_or_skip!();
    let (name, password) = fcp.user("carol").await;
    let server = discover(&fcp.admin).await.unwrap();
    let signin = BrowserSignIn::start(server.clone(), AppClient::this_machine("carol-laptop"))
        .await
        .unwrap();

    // The browser: opens the address, lands on the sign-in page, signs in,
    // is given the way back to the app, and follows it.
    let browser = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let authorize = browser.get(signin.authorize_url()).send().await.unwrap();
    assert_eq!(authorize.status().as_u16(), 303);
    let location = authorize.headers()["location"]
        .to_str()
        .unwrap()
        .to_string();
    let request = location
        .split_once("/login?app=")
        .map(|(_, id)| id.to_string())
        .unwrap_or_else(|| panic!("the sign-in page: {location}"));
    let web: Value = browser
        .post(format!("{}/auth/login", server.api_url))
        .json(&json!({"username": name, "password": password}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let approved: Value = browser
        .post(format!("{}/auth/app/approve", server.api_url))
        .bearer_auth(web["token"].as_str().unwrap())
        .json(&json!({"request": request}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let back = approved["redirect_url"].as_str().unwrap().to_string();
    let finished = tokio::spawn(signin.finish(SOON));
    let page = browser
        .get(&back)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(page.contains("Signed in"), "{page}");

    let session = finished.await.unwrap().expect("the app's session");
    assert_eq!(session.username, name);
    assert!(session.device.is_some(), "the install's device");
    let client = Arc::new(FcpClient::new(session, None));
    let config = client.softphone().await.expect("/me/softphone");
    assert!(config.account.aor.contains(&name));
    client.sign_out().await.unwrap();
}
