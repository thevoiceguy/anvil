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
    Anvil, AnvilConfig, AnvilError, BrandConfig, CallId, EndReason, Event, EventStream,
    MediaConfig, RegState,
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

/// A PNG's signature and header, and bytes of its own.
fn tiny_png(salt: u8) -> Vec<u8> {
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    png.extend_from_slice(&64u32.to_be_bytes());
    png.extend_from_slice(&64u32.to_be_bytes());
    png.extend_from_slice(&[8, 6, 0, 0, 0, salt]);
    png
}

impl Fcp {
    /// Upload an asset to the tenant's brand: its hash.
    async fn brand_asset(&self, bytes: Vec<u8>) -> String {
        let res = self
            .http
            .post(format!("{}/api/v1/branding/assets", self.admin))
            .bearer_auth(&self.token)
            .header("X-FCP-Tenant-Id", &self.tenant)
            .body(bytes)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status().as_u16(), 201);
        res.json::<Value>().await.unwrap()["sha256"]
            .as_str()
            .unwrap()
            .to_string()
    }

    async fn branding(&self, method: reqwest::Method, body: Option<Value>) -> u16 {
        let mut req = self
            .http
            .request(method, format!("{}/api/v1/branding", self.admin))
            .bearer_auth(&self.token)
            .header("X-FCP-Tenant-Id", &self.tenant);
        if let Some(body) = body {
            req = req.json(&body);
        }
        req.send().await.unwrap().status().as_u16()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn anvil_takes_on_the_tenants_brand() {
    let fcp = fcp_or_skip!();
    let logo = tiny_png(7);
    let ring: Vec<u8> = b"OggS\0\x02anvil-ring".to_vec();
    let (logo_hash, ring_hash) = (
        fcp.brand_asset(logo.clone()).await,
        fcp.brand_asset(ring.clone()).await,
    );
    let brand = |name: &str| {
        json!({
            "app_name": name,
            "colors": {"primary": "#1A73E8"},
            "logo": logo_hash,
            "ringtones": [{"name": "Acme", "default": true, "sha256": ring_hash}],
            "links": {"support": "https://acme.example/support"},
        })
    };
    assert_eq!(
        fcp.branding(reqwest::Method::PUT, Some(brand("Acme Voice")))
            .await,
        200
    );

    let server = discover(&fcp.admin).await.unwrap();
    let (name, pw) = fcp.user("brand").await;
    let session = password_sign_in(
        server,
        AppClient::this_machine(AppClient::new_install_id()),
        &name,
        &pw,
        None,
    )
    .await
    .unwrap();
    let client = Arc::new(FcpClient::new(session, None));
    let cache = std::env::temp_dir().join(format!("anvil-brand-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cache);
    let config = client.softphone().await.unwrap();
    let start = |client: Arc<FcpClient>| {
        let cache = cache.clone();
        let config = config.clone();
        async move {
            let mut account = client.account_config(&config).await.unwrap();
            account.bind_addr = Some("127.0.0.41:0".into());
            Anvil::start(AnvilConfig {
                account,
                media: MediaConfig::default(),
                audio: Box::new(Quiet),
                brand: anvil_fcp::brand_config(client, Some(cache)).unwrap(),
            })
            .await
            .unwrap()
        }
    };
    let brand_of = |e: &Event| match e {
        Event::BrandUpdated { profile } => Some((**profile).clone()),
        _ => None,
    };

    // The first launch fetches it, files and all.
    let (anvil, mut events) = start(Arc::clone(&client)).await;
    let profile = expect(&mut events, SOON, brand_of)
        .await
        .expect("the brand");
    assert_eq!(profile.app_name, "Acme Voice");
    assert_eq!(
        profile.colors.primary.map(|c| c.to_hex()).as_deref(),
        Some("#1A73E8")
    );
    assert_eq!(profile.logo.as_ref().unwrap().bytes, logo);
    assert_eq!(profile.ringtones[0].asset.bytes, ring);
    assert_eq!(
        profile.links.support.as_deref(),
        Some("https://acme.example/support")
    );
    let _ = anvil.shutdown().await;

    // The next shows the cached brand at once and fetches nothing new.
    let (anvil, mut events) = start(Arc::clone(&client)).await;
    let cached = expect(&mut events, SOON, brand_of)
        .await
        .expect("the cached brand");
    assert_eq!(cached.app_name, "Acme Voice");
    assert!(
        cached.logo.as_ref().unwrap().local_path.is_some(),
        "from the cache"
    );
    assert!(
        expect(&mut events, Duration::from_secs(2), brand_of)
            .await
            .is_none(),
        "unchanged: 304, no second event"
    );

    // Changed at FCP: a refresh brings it.
    assert_eq!(
        fcp.branding(reqwest::Method::PUT, Some(brand("Acme Calls")))
            .await,
        200
    );
    anvil.refresh_brand().await.unwrap();
    let changed = expect(&mut events, SOON, brand_of)
        .await
        .expect("the new brand");
    assert_eq!(changed.app_name, "Acme Calls");

    // Gone at FCP: the app's own theme again.
    assert_eq!(fcp.branding(reqwest::Method::DELETE, None).await, 204);
    anvil.refresh_brand().await.unwrap();
    expect(&mut events, SOON, |e| {
        matches!(e, Event::BrandCleared).then_some(())
    })
    .await
    .expect("the brand cleared");
    let _ = anvil.shutdown().await;
    let _ = std::fs::remove_dir_all(&cache);
}

impl Fcp {
    /// A mailbox for `username`: a number, its line, the mailbox.
    async fn mailbox(&self, username: &str) {
        use rand::Rng;
        let call = |method: reqwest::Method, path: String, body: Value| {
            let req = self
                .http
                .request(method, format!("{}/api/v1{path}", self.admin))
                .bearer_auth(&self.token)
                .header("X-FCP-Tenant-Id", &self.tenant)
                .json(&body);
            async move {
                let res = req.send().await.unwrap();
                let status = res.status();
                let json: Value = res.json().await.unwrap_or_default();
                assert!(status.is_success(), "{path}: {status} {json}");
                json
            }
        };
        let user = self
            .http
            .get(format!("{}/api/v1/users/{username}", self.admin))
            .bearer_auth(&self.token)
            .header("X-FCP-Tenant-Id", &self.tenant)
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        let n: u32 = rand::thread_rng().gen_range(1_000_000..9_999_999);
        let number = call(
            reqwest::Method::POST,
            "/phone-numbers".into(),
            json!({"e164": format!("+1512{n}"), "tenant_id": self.tenant}),
        )
        .await;
        let line = call(
            reqwest::Method::POST,
            "/line-bindings".into(),
            json!({"phone_number_id": number["id"], "owner_user_id": user["id"],
                   "binding_type": "user", "tenant_id": self.tenant}),
        )
        .await;
        call(
            reqwest::Method::POST,
            "/voicemail/mailboxes".into(),
            json!({"tenant_id": self.tenant, "user_id": user["id"],
                   "line_binding_id": line["id"], "pin": "4321", "enabled": true}),
        )
        .await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn anvil_is_told_its_messages() {
    let fcp = fcp_or_skip!();
    let server = discover(&fcp.admin).await.unwrap();
    let (name, pw) = fcp.user("mwi").await;
    fcp.mailbox(&name).await;
    let session = password_sign_in(
        server,
        AppClient::this_machine(AppClient::new_install_id()),
        &name,
        &pw,
        None,
    )
    .await
    .unwrap();
    let client = FcpClient::new(session, None);
    let (anvil, mut events) = softphone(&client, "127.0.0.42").await;
    anvil
        .subscribe_mwi()
        .await
        .expect("FCP takes the message-summary subscription");
    let summary = expect(&mut events, SOON, |e| match e {
        Event::MessageWaiting { summary } => Some(*summary),
        _ => None,
    })
    .await
    .expect("the mailbox's counts at once");
    assert!(!summary.waiting, "{summary:?}");
    assert_eq!((summary.new, summary.old), (0, 0));
    let _ = anvil.shutdown().await;
}

/// A signed-in Anvil, by name.
async fn signed_in(fcp: &Fcp, who: &str) -> (FcpClient, String) {
    let server = discover(&fcp.admin).await.unwrap();
    let (name, pw) = fcp.user(who).await;
    let session = password_sign_in(
        server,
        AppClient::this_machine(AppClient::new_install_id()),
        &name,
        &pw,
        None,
    )
    .await
    .unwrap();
    (FcpClient::new(session, None), name)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn anvil_watches_a_colleagues_lamp() {
    use anvil_core::watch::{LineState, WatchKind};
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
    let fcp = fcp_or_skip!();
    let (alice, _) = signed_in(&fcp, "lamp-alice").await;
    let (bob, bob_name) = signed_in(&fcp, "lamp-bob").await;
    let (carol, _) = signed_in(&fcp, "lamp-carol").await;
    let domain = alice.softphone().await.unwrap().account.domain;
    let (alice_phone, _alice_events) = softphone(&alice, "127.0.0.43").await;
    let (bob_phone, mut bob_events) = softphone(&bob, "127.0.0.44").await;
    let (carol_phone, mut carol_events) = softphone(&carol, "127.0.0.45").await;
    let bob_aor = format!("sip:{bob_name}@{domain}");

    // Carol watches Bob's lamp: idle at once.
    carol_phone
        .watch(&bob_aor, WatchKind::Dialog)
        .await
        .expect("FCP takes the dialog subscription");
    let lamp = |e: &Event| match e {
        Event::LineStateChanged { state, .. } => Some(*state),
        _ => None,
    };
    assert_eq!(
        expect(&mut carol_events, SOON, lamp).await,
        Some(LineState::Idle)
    );

    // Alice calls Bob: his lamp lights, and goes out when the call ends.
    let out = alice_phone.place_call(&bob_aor).await.unwrap();
    let incoming = expect(&mut bob_events, SOON, |e| match e {
        Event::IncomingCall { call, .. } => Some(*call),
        _ => None,
    })
    .await
    .expect("Bob rings");
    let lit = expect(&mut carol_events, SOON, |e| match lamp(e) {
        Some(LineState::Idle) | None => None,
        Some(s) => Some(s),
    })
    .await;
    assert!(lit.is_some(), "Carol sees Bob's lamp light");
    bob_phone.answer(incoming).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    alice_phone.hangup(out).await.unwrap();
    assert_eq!(
        expect(&mut carol_events, SOON, |e| match lamp(e) {
            Some(LineState::Idle) => Some(LineState::Idle),
            _ => None,
        })
        .await,
        Some(LineState::Idle),
        "and go out"
    );

    // Stopped watching: nothing more.
    carol_phone.unwatch(&bob_aor, WatchKind::Dialog);
    for phone in [alice_phone, bob_phone, carol_phone] {
        let _ = phone.shutdown().await;
    }
}

/// Three signed-in softphones, registered, at three addresses.
async fn three(fcp: &Fcp, tag: &str, base: u8) -> Vec<(FcpClient, String, Anvil, EventStream)> {
    let mut out = Vec::new();
    for (i, who) in ["alice", "bob", "carol"].iter().enumerate() {
        let (client, name) = signed_in(fcp, &format!("{tag}-{who}")).await;
        let (anvil, events) = softphone(&client, &format!("127.0.0.{}", base + i as u8)).await;
        out.push((client, name, anvil, events));
    }
    out
}

async fn established(events: &mut EventStream, call: Option<CallId>) -> CallId {
    expect(events, SOON, |e| match e {
        Event::CallEstablished { call: c, .. } if call.is_none_or(|x| x == *c) => Some(*c),
        _ => None,
    })
    .await
    .expect("the call is up")
}

async fn rings(events: &mut EventStream) -> CallId {
    expect(events, SOON, |e| match e {
        Event::IncomingCall { call, .. } => Some(*call),
        _ => None,
    })
    .await
    .expect("the phone rings")
}

async fn ended(events: &mut EventStream, call: CallId) -> EndReason {
    expect(events, SOON, |e| match e {
        Event::CallEnded { call: c, reason } if *c == call => Some(reason.clone()),
        _ => None,
    })
    .await
    .expect("the call ends")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn anvil_transfers_a_call_blind() {
    let fcp = fcp_or_skip!();
    let mut p = three(&fcp, "blind", 50).await;
    let domain = p[0].0.softphone().await.unwrap().account.domain;
    let aor = |name: &str| format!("sip:{name}@{domain}");
    let (bob_aor, carol_aor) = (aor(&p[1].1), aor(&p[2].1));

    // Alice calls Bob; Bob answers.
    let out = p[0].2.place_call(&bob_aor).await.unwrap();
    let at_bob = rings(&mut p[1].3).await;
    p[1].2.answer(at_bob).await.unwrap();
    established(&mut p[0].3, Some(out)).await;

    // Alice sends Bob to Carol: Carol rings and answers, Alice is told and
    // let go, Bob stays on the line.
    p[0].2
        .transfer(out, &carol_aor)
        .await
        .expect("FCP takes the REFER");
    let at_carol = rings(&mut p[2].3).await;
    p[2].2.answer(at_carol).await.unwrap();
    let done = expect(&mut p[0].3, SOON, |e| match e {
        Event::TransferProgress { call, code, .. } if *call == out && *code >= 200 => Some(*code),
        _ => None,
    })
    .await;
    assert_eq!(done, Some(200), "the transfer succeeded");
    assert!(matches!(
        ended(&mut p[0].3, out).await,
        EndReason::RemoteHangup
    ));
    assert!(
        expect(&mut p[1].3, Duration::from_secs(1), |e| matches!(
            e,
            Event::CallEnded { call, .. } if *call == at_bob
        )
        .then_some(()))
        .await
        .is_none(),
        "Bob is still on the line, now with Carol"
    );
    for (_, _, anvil, _) in p {
        let _ = anvil.shutdown().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn anvil_transfers_a_call_attended() {
    let fcp = fcp_or_skip!();
    let mut p = three(&fcp, "attended", 60).await;
    let domain = p[0].0.softphone().await.unwrap().account.domain;
    let aor = |name: &str| format!("sip:{name}@{domain}");
    let (bob_aor, carol_aor) = (aor(&p[1].1), aor(&p[2].1));

    // Alice talks to Bob, holds him, consults Carol.
    let to_bob = p[0].2.place_call(&bob_aor).await.unwrap();
    let at_bob = rings(&mut p[1].3).await;
    p[1].2.answer(at_bob).await.unwrap();
    established(&mut p[0].3, Some(to_bob)).await;
    p[0].2.hold(to_bob, true).await.expect("Bob held");
    let to_carol = p[0].2.place_call(&carol_aor).await.unwrap();
    let at_carol = rings(&mut p[2].3).await;
    p[2].2.answer(at_carol).await.unwrap();
    established(&mut p[0].3, Some(to_carol)).await;

    // Alice joins them and leaves: both her calls end, theirs goes on.
    p[0].2
        .transfer_attended(to_bob, to_carol)
        .await
        .expect("FCP takes the REFER with Replaces");
    // Both of Alice's calls end, in either order.
    let mut open = vec![to_bob, to_carol];
    while !open.is_empty() {
        let gone = expect(&mut p[0].3, SOON, |e| match e {
            Event::CallEnded { call, .. } if open.contains(call) => Some(*call),
            _ => None,
        })
        .await
        .unwrap_or_else(|| panic!("Alice's calls {open:?} did not end"));
        open.retain(|c| *c != gone);
    }
    // FCP joins Bob's leg to Carol's: both stay in the calls they had.
    for (i, who, call) in [(1, "Bob", at_bob), (2, "Carol", at_carol)] {
        let gone = expect(&mut p[i].3, Duration::from_secs(1), |e| match e {
            Event::CallEnded { call: c, .. } if *c == call => Some(()),
            _ => None,
        })
        .await;
        assert!(gone.is_none(), "{who}'s call ended with the transfer");
    }
    for (_, _, anvil, _) in p {
        let _ = anvil.shutdown().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn anvil_sets_do_not_disturb_and_hears_a_second_call() {
    let fcp = fcp_or_skip!();
    let mut p = three(&fcp, "dnd", 70).await;
    let domain = p[0].0.softphone().await.unwrap().account.domain;
    let bob_aor = format!("sip:{}@{domain}", p[1].1);

    // Call waiting: Bob on a call with Alice, Carol's call rings him too.
    let out = p[0].2.place_call(&bob_aor).await.unwrap();
    let at_bob = rings(&mut p[1].3).await;
    p[1].2.answer(at_bob).await.unwrap();
    established(&mut p[0].3, Some(out)).await;
    let from_carol = p[2].2.place_call(&bob_aor).await.unwrap();
    let waiting = rings(&mut p[1].3).await;
    assert_ne!(waiting, at_bob, "a second call, while the first is up");
    p[1].2.reject(waiting, 486).await.unwrap();
    assert!(matches!(
        ended(&mut p[2].3, from_carol).await,
        EndReason::Rejected(_)
    ));
    p[0].2.hangup(out).await.unwrap();

    // Do not disturb, set from the app: Alice's call is refused unrung.
    let set = p[1]
        .0
        .set_calling(&anvil_fcp::CallingUpdate {
            dnd: Some(true),
            ..Default::default()
        })
        .await
        .expect("/me/calling");
    assert!(set.dnd);
    let refused = p[0].2.place_call(&bob_aor).await.unwrap();
    assert!(matches!(
        ended(&mut p[0].3, refused).await,
        EndReason::Rejected(_)
    ));
    assert!(
        expect(&mut p[1].3, Duration::from_millis(500), |e| matches!(
            e,
            Event::IncomingCall { .. }
        )
        .then_some(()))
        .await
        .is_none(),
        "Bob's app did not ring"
    );
    let cleared = p[1]
        .0
        .set_calling(&anvil_fcp::CallingUpdate {
            dnd: Some(false),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(!cleared.dnd);
    for (_, _, anvil, _) in p {
        let _ = anvil.shutdown().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn anvil_reads_the_users_history_directory_voicemail_and_events() {
    let fcp = fcp_or_skip!();
    let (alice_client, alice) = signed_in(&fcp, "data-alice").await;
    let (bob_client, bob) = signed_in(&fcp, "data-bob").await;
    let (alice_phone, mut alice_events) = softphone(&alice_client, "127.0.0.80").await;
    let (bob_phone, mut bob_events) = softphone(&bob_client, "127.0.0.81").await;
    let domain = bob_client.softphone().await.unwrap().account.domain;

    // Bob's live events, open before the call.
    let mut live = bob_client.events().await.expect("/me/events");

    // Alice calls Bob; Bob answers; Alice hangs up.
    let out = alice_phone
        .place_call(&format!("sip:{bob}@{domain}"))
        .await
        .unwrap();
    let at_bob = expect(&mut bob_events, SOON, |e| match e {
        Event::IncomingCall { call, .. } => Some(*call),
        _ => None,
    })
    .await
    .expect("Bob rings");
    bob_phone.answer(at_bob).await.unwrap();
    expect(&mut alice_events, SOON, |e| match e {
        Event::CallEstablished { call, .. } if *call == out => Some(()),
        _ => None,
    })
    .await
    .expect("answered");
    alice_phone.hangup(out).await.unwrap();

    // Bob's stream follows the call from its start to its end.
    let mut seen = Vec::new();
    let deadline = tokio::time::Instant::now() + SOON;
    while tokio::time::Instant::now() < deadline && !seen.iter().any(|n: &String| n == "call.ended")
    {
        match tokio::time::timeout(Duration::from_millis(500), live.next()).await {
            Ok(Some(event)) => seen.push(event.name),
            Ok(None) => break,
            Err(_) => {}
        }
    }
    for name in ["call.initiated", "call.answered", "call.ended"] {
        assert!(seen.iter().any(|n| n == name), "{name} in {seen:?}");
    }
    live.close().await;

    // The call in Bob's history, with Alice.
    let mut found = None;
    for _ in 0..40 {
        let page = bob_client
            .calls(&anvil_fcp::CallQuery::default())
            .await
            .expect("/me/calls");
        found = page.data.into_iter().find(|c| {
            c.other_party.contains(&alice) || c.other_party_name.as_deref() == Some(&alice)
        });
        if found.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let record = found.expect("the call is in Bob's history");
    assert_eq!(record.direction, "incoming", "{record:?}");
    assert!(!record.missed, "{record:?}");

    // Alice in the directory, found by her name.
    let directory = bob_client
        .directory(Some(&alice), None, None)
        .await
        .expect("/me/directory");
    assert!(
        directory
            .data
            .iter()
            .any(|e| e.username.as_deref() == Some(alice.as_str())),
        "{directory:?}"
    );

    // Bob's voicemail: none yet.
    match bob_client.voicemail(None, None).await {
        Ok(page) => assert!(page.data.is_empty(), "{page:?}"),
        Err(FcpError::Refused { status: 404, .. }) => {}
        Err(e) => panic!("/me/voicemail/messages: {e}"),
    }

    let _ = alice_phone.shutdown().await;
    let _ = bob_phone.shutdown().await;
}
