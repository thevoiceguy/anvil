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

/// A host that speaks a tone and remembers the loudest frame it heard.
struct Tone {
    hz: f32,
    heard: Arc<std::sync::atomic::AtomicU64>,
}
struct ToneSource {
    format: AudioFormat,
    tick: tokio::time::Interval,
    hz: f32,
    phase: f32,
}
struct LoudestSink(Arc<std::sync::atomic::AtomicU64>);

#[async_trait]
impl AudioSource for ToneSource {
    async fn next_frame(&mut self) -> Option<AudioFrame> {
        self.tick.tick().await;
        let rate = self.format.sample_rate as f32;
        let n = (self.format.sample_rate * self.format.frame_ms / 1000) as usize;
        let samples = (0..n)
            .map(|_| {
                self.phase = (self.phase + self.hz / rate) % 1.0;
                ((self.phase * std::f32::consts::TAU).sin() * 12_000.0) as i16
            })
            .collect();
        Some(AudioFrame {
            samples,
            format: self.format,
        })
    }
}

#[async_trait]
impl AudioSink for LoudestSink {
    async fn write_frame(&mut self, frame: &AudioFrame) -> Result<(), AnvilError> {
        let n = frame.samples.len().max(1) as f64;
        let sum: f64 = frame
            .samples
            .iter()
            .map(|&s| f64::from(s) * f64::from(s))
            .sum();
        self.0.fetch_max(
            (sum / n).sqrt() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
        Ok(())
    }
}

impl AudioHost for Tone {
    fn make_capture(&self, cfg: AudioFormat) -> Result<Box<dyn AudioSource>, AnvilError> {
        Ok(Box::new(ToneSource {
            format: cfg,
            tick: tokio::time::interval(Duration::from_millis(u64::from(cfg.frame_ms))),
            hz: self.hz,
            phase: 0.0,
        }))
    }
    fn make_playback(&self, _cfg: AudioFormat) -> Result<Box<dyn AudioSink>, AnvilError> {
        Ok(Box::new(LoudestSink(Arc::clone(&self.heard))))
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
    softphone_with(client, ip, MediaConfig::default(), Box::new(Quiet)).await
}

async fn softphone_with(
    client: &FcpClient,
    ip: &str,
    media: MediaConfig,
    audio: Box<dyn AudioHost>,
) -> (Anvil, EventStream) {
    softphone_with_video(client, ip, media, audio, None).await
}

async fn softphone_with_video(
    client: &FcpClient,
    ip: &str,
    media: MediaConfig,
    audio: Box<dyn AudioHost>,
    video: Option<Box<dyn anvil_core::video::host::VideoHost>>,
) -> (Anvil, EventStream) {
    let config = client.softphone().await.expect("/me/softphone");
    let mut account = client.account_config(&config).await.expect("an account");
    account.bind_addr = Some(format!("{ip}:0"));
    let (anvil, mut events) = Anvil::start(AnvilConfig {
        account,
        media,
        audio,
        video,
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
                video: None,
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn anvil_encrypts_its_media_and_fcp_answers_in_srtp() {
    use std::sync::atomic::{AtomicU64, Ordering};
    let fcp = fcp_or_skip!();
    let (alice_client, _) = signed_in(&fcp, "srtp-alice").await;
    let (bob_client, bob) = signed_in(&fcp, "srtp-bob").await;
    let (alice_heard, bob_heard) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)));
    // Alice requires SRTP; Bob's phone speaks RTP in the clear.
    let (alice_phone, mut alice_events) = softphone_with(
        &alice_client,
        "127.0.0.90",
        MediaConfig {
            srtp: anvil_core::config::SrtpMode::Required,
            ..MediaConfig::default()
        },
        Box::new(Tone {
            hz: 440.0,
            heard: Arc::clone(&alice_heard),
        }),
    )
    .await;
    let (bob_phone, mut bob_events) = softphone_with(
        &bob_client,
        "127.0.0.91",
        MediaConfig::default(),
        Box::new(Tone {
            hz: 660.0,
            heard: Arc::clone(&bob_heard),
        }),
    )
    .await;
    let domain = bob_client.softphone().await.unwrap().account.domain;

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
    // FCP answered Alice's encrypted offer with a key of its own, or her
    // phone, requiring SRTP, would have ended the call.
    expect(&mut alice_events, SOON, |e| match e {
        Event::CallEstablished { call, .. } if *call == out => Some(()),
        Event::CallEnded { call, reason } if *call == out => panic!("the call ended: {reason:?}"),
        _ => None,
    })
    .await
    .expect("Alice's call is up");

    // Each hears the other: FCP decrypts what Alice sends and encrypts what
    // it sends her.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let (a, b) = (
        alice_heard.load(Ordering::Relaxed),
        bob_heard.load(Ordering::Relaxed),
    );
    assert!(a > 2000, "Alice hears Bob through SRTP: {a}");
    assert!(b > 2000, "Bob hears Alice through FCP: {b}");

    alice_phone.hangup(out).await.unwrap();
    let _ = alice_phone.shutdown().await;
    let _ = bob_phone.shutdown().await;
}

/// A softphone registered over TLS is called encrypted: FCP offers it
/// SDES-SRTP (FCP's A5b). Bob requires SRTP, so a plain offer would be
/// refused; Alice, over UDP, is plain. Needs `ANVIL_FCP_SIPS` (FCP's TLS
/// listener) and `ANVIL_FCP_CA` (the CA its certificate is from).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn anvil_over_tls_is_called_encrypted() {
    use std::sync::atomic::{AtomicU64, Ordering};
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
    let fcp = fcp_or_skip!();
    let (Ok(sips), Ok(ca)) = (
        std::env::var("ANVIL_FCP_SIPS"),
        std::env::var("ANVIL_FCP_CA"),
    ) else {
        eprintln!("ANVIL_FCP_SIPS and ANVIL_FCP_CA not set; skipping");
        return;
    };
    let (alice_client, _) = signed_in(&fcp, "tls-alice").await;
    let (bob_client, bob) = signed_in(&fcp, "tls-bob").await;
    let (alice_heard, bob_heard) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)));

    // Bob's softphone over TLS, requiring SRTP.
    let config = bob_client.softphone().await.unwrap();
    let mut account = bob_client.account_config(&config).await.unwrap();
    account.transport = anvil_core::Transport::Tls;
    // As FCP names a TLS server in `/me/softphone`: the registrar is the
    // domain, the outbound proxy the server with `;transport=tls`.
    account.outbound_proxy = Some(format!("sip:{sips};transport=tls"));
    account.tls_extra_ca_pem = Some(std::fs::read(&ca).expect("the CA"));
    account.bind_addr = Some("127.0.0.92:0".into());
    let (bob_phone, mut bob_events) = Anvil::start(AnvilConfig {
        account,
        media: MediaConfig {
            srtp: anvil_core::config::SrtpMode::Required,
            ..MediaConfig::default()
        },
        audio: Box::new(Tone {
            hz: 660.0,
            heard: Arc::clone(&bob_heard),
        }),
        video: None,
        brand: BrandConfig::default(),
    })
    .await
    .expect("anvil starts");
    bob_phone.register().await.expect("registers over TLS");
    expect(&mut bob_events, SOON, |e| match e {
        Event::RegistrationChanged {
            state: RegState::Registered,
            ..
        } => Some(()),
        _ => None,
    })
    .await
    .expect("registered over TLS");

    let (alice_phone, mut alice_events) = softphone_with(
        &alice_client,
        "127.0.0.93",
        MediaConfig::default(),
        Box::new(Tone {
            hz: 440.0,
            heard: Arc::clone(&alice_heard),
        }),
    )
    .await;
    let out = alice_phone
        .place_call(&format!("sip:{bob}@{}", config.account.domain))
        .await
        .unwrap();
    // A plain offer would have been refused 488 before ringing.
    let at_bob = expect(&mut bob_events, SOON, |e| match e {
        Event::IncomingCall { call, .. } => Some(*call),
        _ => None,
    })
    .await
    .expect("Bob's softphone rings: the offer was encrypted");
    bob_phone.answer(at_bob).await.unwrap();
    expect(&mut alice_events, SOON, |e| match e {
        Event::CallEstablished { call, .. } if *call == out => Some(()),
        Event::CallEnded { call, reason } if *call == out => panic!("the call ended: {reason:?}"),
        _ => None,
    })
    .await
    .expect("Alice's call is up");

    tokio::time::sleep(Duration::from_secs(2)).await;
    let (a, b) = (
        alice_heard.load(Ordering::Relaxed),
        bob_heard.load(Ordering::Relaxed),
    );
    assert!(a > 2000, "Alice hears Bob, who speaks SRTP: {a}");
    assert!(b > 2000, "Bob hears Alice through SRTP: {b}");

    alice_phone.hangup(out).await.unwrap();
    let _ = alice_phone.shutdown().await;
    let _ = bob_phone.shutdown().await;
}

impl Fcp {
    /// A meeting with video at a fresh number, owned by `owner`: its id
    /// and number.
    async fn meeting(&self, owner: &str) -> (String, String) {
        self.meeting_with(owner, json!({})).await
    }

    /// [`meeting`](Self::meeting) with `extra` fields (a forwarding room's
    /// `video_mode`).
    async fn meeting_with(&self, owner: &str, extra: Value) -> (String, String) {
        use rand::Rng;
        let room = format!("77{}", rand::thread_rng().gen_range(100_000..999_999));
        let made = self
            .http
            .post(format!("{}/api/v1/conferences/meetings", self.admin))
            .bearer_auth(&self.token)
            .header("X-FCP-Tenant-Id", &self.tenant)
            .json(&{
                let mut body = json!({
                    "room_number": room,
                    "name": format!("Anvil room {room}"),
                    "owner_id": owner,
                    "video_enabled": true,
                    "play_entry_tone": false,
                    "play_exit_tone": false,
                });
                if let (Some(b), Some(e)) = (body.as_object_mut(), extra.as_object()) {
                    b.extend(e.clone());
                }
                body
            })
            .send()
            .await
            .unwrap();
        let status = made.status();
        let body: Value = made.json().await.unwrap_or_default();
        assert!(status.is_success(), "{status} {body}");
        (body["id"].as_str().expect("a meeting id").to_string(), room)
    }
}

/// The next roster `call` is told that `ok` takes.
async fn roster_where(
    events: &mut EventStream,
    call: CallId,
    what: &str,
    ok: impl Fn(&[anvil_core::RoomMember]) -> bool,
) -> Vec<anvil_core::RoomMember> {
    let mut last = Vec::new();
    let found = expect(events, Duration::from_secs(15), |e| match e {
        Event::RoomRoster {
            call: c, members, ..
        } if *c == call => {
            last = members.clone();
            ok(members).then(|| members.clone())
        }
        _ => None,
    })
    .await;
    found.unwrap_or_else(|| panic!("no roster where {what}; last: {last:?}"))
}

/// The room's picture reaching `call`.
async fn room_picture(events: &mut EventStream, call: CallId) {
    use anvil_core::video::host::VideoKind;
    use anvil_core::video::VideoStreamDirection;
    expect(events, Duration::from_secs(15), |e| match e {
        Event::VideoStarted {
            call: c,
            direction: VideoStreamDirection::Incoming,
            kind: VideoKind::Camera,
            ..
        } if *c == call => Some(()),
        _ => None,
    })
    .await
    .expect("the room's picture arrives");
}

/// Two signed-in Anvils meet in a room on FCP (FCP's
/// `docs/SOFTPHONE_VIDEO.md` §2.6): its owner dials its number, a guest
/// follows the join link the owner made, each sees the room's picture and
/// a roster with both; the owner mutes the guest as the room's host and
/// the roster says so; the guest may not do the same.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_anvils_meet_in_a_room_on_fcp() {
    use anvil_core::video::synthetic::SyntheticVideoHost;
    use anvil_core::{CallMedia, RoomAudio, RoomJoin};
    use anvil_fcp::JoinRole;
    let fcp = fcp_or_skip!();
    let (alice_client, alice_name) = signed_in(&fcp, "room-alice").await;
    let (bob_client, _) = signed_in(&fcp, "room-bob").await;
    let video = || {
        Some(Box::new(SyntheticVideoHost::default()) as Box<dyn anvil_core::video::host::VideoHost>)
    };
    let (alice, mut alice_events) = softphone_with_video(
        &alice_client,
        "127.0.0.70",
        MediaConfig::default(),
        Box::new(Quiet),
        video(),
    )
    .await;
    let (bob, mut bob_events) = softphone_with_video(
        &bob_client,
        "127.0.0.71",
        MediaConfig::default(),
        Box::new(Quiet),
        video(),
    )
    .await;
    let domain = alice_client.softphone().await.unwrap().account.domain;
    let (meeting, room) = fcp.meeting(&alice_name).await;

    // The owner's meetings, and a link for a guest.
    let mine = alice_client.meetings().await.expect("/me/conferences");
    assert!(
        mine.iter()
            .any(|m| m.id == meeting && m.room_number == room),
        "{mine:?}"
    );
    let link = alice_client
        .join_link(&meeting, JoinRole::Guest, Some("Bob"), None)
        .await
        .expect("a join link");
    assert_eq!(link.room_number, room);

    // The owner dials in, the guest follows the link.
    let target = format!("sip:{room}@{domain}");
    let a = alice
        .join_room(&target, &RoomJoin::default(), CallMedia::VIDEO)
        .await
        .expect("dials the room");
    let a = established(&mut alice_events, Some(a)).await;
    let join = RoomJoin::from_link(&link.link()).expect("the link's token");
    let b = bob
        .join_room(&target, &join, CallMedia::VIDEO)
        .await
        .expect("follows the link");
    let b = established(&mut bob_events, Some(b)).await;
    room_picture(&mut alice_events, a).await;
    room_picture(&mut bob_events, b).await;

    let both = |m: &[anvil_core::RoomMember]| m.len() == 2 && m.iter().any(|m| m.name == "Bob");
    let members = roster_where(&mut alice_events, a, "both are in", both).await;
    roster_where(&mut bob_events, b, "both are in", both).await;
    let bob_id = members
        .iter()
        .find(|m| m.name == "Bob")
        .map(|m| m.id.clone())
        .unwrap();

    // The owner, as host, mutes the guest; the guest may not.
    let refused = bob_client
        .room_host(&room, None)
        .mute(&bob_id, true)
        .await
        .expect_err("a guest is not the host");
    assert!(
        matches!(refused, FcpError::Refused { status: 403, .. }),
        "{refused:?}"
    );
    let host = alice_client.room_host(&room, None);
    let live = host.room().await.expect("the room as it is");
    assert!(live.to_string().contains(&bob_id), "{live}");
    host.mute(&bob_id, true)
        .await
        .expect("the host mutes the guest");
    roster_where(&mut alice_events, a, "the guest is muted", |m| {
        m.iter()
            .any(|m| m.id == bob_id && m.audio == RoomAudio::Muted)
    })
    .await;
    host.mute(&bob_id, false).await.expect("and unmutes them");
    roster_where(&mut bob_events, b, "the guest is heard again", |m| {
        m.iter()
            .any(|m| m.id == bob_id && m.audio == RoomAudio::Sending)
    })
    .await;

    // The guest leaves: the owner's roster lists the owner alone.
    bob.hangup(b).await.expect("the guest leaves");
    roster_where(&mut alice_events, a, "the guest left", |m| m.len() == 1).await;
    alice.hangup(a).await.expect("the owner leaves");
    let _ = alice.shutdown().await;
    let _ = bob.shutdown().await;
}

/// The next floor notice in `call` that `ok` accepts.
async fn floor_where(
    events: &mut EventStream,
    call: CallId,
    within: Duration,
    what: &str,
    ok: impl Fn(&anvil_core::FloorNotice) -> bool,
) -> anvil_core::FloorNotice {
    let mut seen = Vec::new();
    let found = expect(events, within, |e| match e {
        Event::RoomFloor { call: c, notice } if *c == call => {
            seen.push(notice.clone());
            ok(notice).then(|| notice.clone())
        }
        _ => None,
    })
    .await;
    found.unwrap_or_else(|| panic!("{what}: the floor said {seen:?}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_screen_shared_in_a_room_on_fcp_takes_the_floor_and_frees_it_on_stop() {
    use anvil_core::video::host::Resolution;
    use anvil_core::{CallMedia, FloorState, RoomJoin};
    use anvil_video::{DesktopVideoHost, PatternCamera, PatternScreen, SoftwareCodecs};
    let fcp = fcp_or_skip!();
    let (client, name) = signed_in(&fcp, "share-alice").await;
    // Real VP8 both ways: the room decodes the shared screen.
    let host = DesktopVideoHost::new(
        PatternCamera::new(Resolution {
            width: 320,
            height: 180,
        }),
        SoftwareCodecs::default(),
    )
    .with_screens(PatternScreen::new(Resolution {
        width: 640,
        height: 360,
    }));
    let (alice, mut events) = softphone_with_video(
        &client,
        "127.0.0.72",
        MediaConfig::default(),
        Box::new(Quiet),
        Some(Box::new(host)),
    )
    .await;
    let domain = client.softphone().await.unwrap().account.domain;
    let (_, room) = fcp.meeting(&name).await;
    let a = alice
        .join_room(
            &format!("sip:{room}@{domain}"),
            &RoomJoin::default(),
            CallMedia::VIDEO,
        )
        .await
        .expect("dials the room");
    let a = established(&mut events, Some(a)).await;
    room_picture(&mut events, a).await;
    assert_eq!(alice.screens().len(), 1);

    // She shares: the floor is hers, and the roster says she presents.
    alice.share_screen(a, true).await.expect("the share");
    let granted = floor_where(&mut events, a, Duration::from_secs(15), "granted", |n| {
        n.state == FloorState::Granted && n.mine
    })
    .await;
    assert!(granted.presenter.is_some(), "{granted:?}");
    roster_where(&mut events, a, "she presents", |m| {
        m.iter().any(|m| m.presenting)
    })
    .await;

    // She stops: the room frees the floor on the re-INVITE, well before
    // its idle timeout (30 s).
    let stopped = std::time::Instant::now();
    alice.share_screen(a, false).await.expect("the stop");
    let released = floor_where(&mut events, a, Duration::from_secs(10), "released", |n| {
        n.state == FloorState::Released && n.mine
    })
    .await;
    assert_eq!(released.reason.as_deref(), Some("ended"));
    assert!(stopped.elapsed() < Duration::from_secs(10));
    roster_where(&mut events, a, "nobody presents", |m| {
        m.iter().all(|m| !m.presenting)
    })
    .await;

    // A second share is a new one: the floor again.
    alice.share_screen(a, true).await.expect("shares again");
    floor_where(
        &mut events,
        a,
        Duration::from_secs(15),
        "granted again",
        |n| n.state == FloorState::Granted && n.mine,
    )
    .await;

    // The owner, as host, stops the share: she is told, her screen
    // stops, and once the room hears her stop she may share again.
    client
        .room_host(&room, None)
        .stop_content()
        .await
        .expect("a host stops the share");
    let stopped = floor_where(
        &mut events,
        a,
        Duration::from_secs(10),
        "a host's stop",
        |n| n.state == FloorState::Released && n.mine,
    )
    .await;
    assert_eq!(stopped.reason.as_deref(), Some("host"));
    alice.share_screen(a, false).await.expect("the stop told");
    alice
        .share_screen(a, true)
        .await
        .expect("shares after the stop");
    floor_where(
        &mut events,
        a,
        Duration::from_secs(15),
        "granted after a host's stop",
        |n| n.state == FloorState::Granted && n.mine,
    )
    .await;
    alice.hangup(a).await.expect("leaves");
    let _ = alice.shutdown().await;
}

/// The streams `call` is sent that `ok` takes, by the members they show.
async fn streams_where(
    events: &mut EventStream,
    call: CallId,
    within: Duration,
    what: &str,
    ok: impl Fn(&[anvil_core::RoomStream]) -> bool,
) -> Vec<anvil_core::RoomStream> {
    let mut last = Vec::new();
    let found = expect(events, within, |e| match e {
        Event::RoomStreams { call: c, streams } if *c == call => {
            last = streams.clone();
            ok(streams).then(|| streams.clone())
        }
        _ => None,
    })
    .await;
    found.unwrap_or_else(|| panic!("no streams where {what}; last: {last:?}"))
}

/// The members `streams` show, apart from the speaker stream.
fn shown(streams: &[anvil_core::RoomStream]) -> std::collections::BTreeSet<String> {
    streams
        .iter()
        .filter(|s| !s.speaker)
        .filter_map(|s| s.member.clone())
        .collect()
}

/// Three Anvils in a room that forwards (FCP's `docs/FORWARDING_ROOMS.md`,
/// a meeting with `video_mode: auto`): each is sent the other two's own
/// streams, named by the roster, and none of its own; a view asking for
/// one of them leaves only that one coming.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_anvils_in_a_forwarding_room_are_each_sent_the_others() {
    use anvil_core::video::host::Resolution;
    use anvil_core::{CallMedia, RoomJoin};
    use anvil_fcp::JoinRole;
    use anvil_video::{DesktopVideoHost, PatternCamera, PatternScreen, SoftwareCodecs};
    let fcp = fcp_or_skip!();
    let (owner_client, owner) = signed_in(&fcp, "fwd-alice").await;
    let (meeting, room) = fcp
        .meeting_with(&owner, json!({ "video_mode": "auto" }))
        .await;
    let link = owner_client
        .join_link(&meeting, JoinRole::Guest, None, None)
        .await
        .expect("a join link");
    let domain = owner_client.softphone().await.unwrap().account.domain;
    let target = format!("sip:{room}@{domain}");
    let clients = [
        owner_client,
        signed_in(&fcp, "fwd-bob").await.0,
        signed_in(&fcp, "fwd-carol").await.0,
    ];
    let mut people = Vec::new();
    for (i, (who, client)) in ["alice", "bob", "carol"].iter().zip(&clients).enumerate() {
        // Real VP8: the room forwards from a keyframe it can read.
        let host = DesktopVideoHost::new(
            PatternCamera::new(Resolution {
                width: 320,
                height: 180,
            }),
            SoftwareCodecs::default(),
        )
        .with_screens(PatternScreen::new(Resolution {
            width: 320,
            height: 180,
        }));
        let (anvil, mut events) = softphone_with_video(
            client,
            &format!("127.0.0.{}", 76 + i),
            MediaConfig::default(),
            Box::new(Quiet),
            Some(Box::new(host)),
        )
        .await;
        let join = if i == 0 {
            RoomJoin::default()
        } else {
            RoomJoin::from_link(&link.link()).expect("the link's token")
        };
        let call = anvil
            .join_room(&target, &join, CallMedia::VIDEO)
            .await
            .expect("joins");
        let call = established(&mut events, Some(call)).await;
        assert!(anvil.room_forwards(call), "{who}: the room forwards");
        people.push((anvil, events, call));
    }

    // Each is sent two people's streams, named; together they name all
    // three, each leaving out one (themselves).
    let mut everyone = std::collections::BTreeSet::new();
    let mut seen = Vec::new();
    for (_, events, call) in people.iter_mut() {
        let streams = streams_where(events, *call, Duration::from_secs(20), "two named", |s| {
            shown(s).len() == 2
        })
        .await;
        let members = shown(&streams);
        everyone.extend(members.iter().cloned());
        seen.push(members);
    }
    assert_eq!(everyone.len(), 3, "{seen:?}");
    assert!(seen[0] != seen[1] && seen[1] != seen[2], "{seen:?}");

    // Alice asks for one of them alone: the other's stream stops coming.
    let (alice, alice_events, a) = &mut people[0];
    let keep = seen[0].iter().next().unwrap().clone();
    let view = anvil_core::RoomView {
        streams: Some(vec![anvil_core::ViewStream {
            target: anvil_core::ViewTarget::Member(keep.clone()),
            max_height: Some(360),
            priority: None,
        }]),
        max_streams: Some(1),
    };
    alice.set_room_view(*a, &view).await.expect("the view");
    streams_where(
        alice_events,
        *a,
        Duration::from_secs(20),
        "only the one asked for",
        |s| shown(s) == std::collections::BTreeSet::from([keep.clone()]),
    )
    .await;
    // The roster stops naming the other at once; the room stops sending it,
    // so its stream falls silent and ends.
    streams_where(
        alice_events,
        *a,
        Duration::from_secs(20),
        "the other's stream ended",
        |s| s.len() == 1 && s[0].member.as_deref() == Some(keep.as_str()),
    )
    .await;

    for (anvil, _, call) in &people {
        let _ = anvil.hangup(*call).await;
    }
    for (anvil, _, _) in people {
        let _ = anvil.shutdown().await;
    }
}

/// A forwarding room sends each viewer the simulcast layer it asks for
/// (FCP's `docs/FORWARDING_ROOMS.md` §2.4–2.5): two Anvils send their
/// 640x360 camera as three encodings, and a third, asking for one of them
/// at 360 and the other at 90, is shown each at that size; asked the other
/// way round, the sizes change places.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_simulcast_camera_reaches_each_viewer_at_the_size_it_asks_for() {
    use anvil_core::video::host::Resolution;
    use anvil_core::{CallMedia, RoomJoin, RoomView, ViewStream, ViewTarget};
    use anvil_fcp::JoinRole;
    use anvil_video::{DesktopVideoHost, PatternCamera, SoftwareCodecs, TileKey};
    let fcp = fcp_or_skip!();
    let (owner_client, owner) = signed_in(&fcp, "sim-alice").await;
    let (meeting, room) = fcp
        .meeting_with(&owner, json!({ "video_mode": "auto" }))
        .await;
    let link = owner_client
        .join_link(&meeting, JoinRole::Guest, None, None)
        .await
        .expect("a join link");
    let domain = owner_client.softphone().await.unwrap().account.domain;
    let target = format!("sip:{room}@{domain}");
    let clients = [
        owner_client,
        signed_in(&fcp, "sim-bob").await.0,
        signed_in(&fcp, "sim-carol").await.0,
    ];
    let mut people = Vec::new();
    let mut tiles = Vec::new();
    for (i, (who, client)) in ["alice", "bob", "carol"].iter().zip(&clients).enumerate() {
        let host = DesktopVideoHost::new(
            PatternCamera::new(Resolution {
                width: 640,
                height: 360,
            }),
            SoftwareCodecs::default(),
        );
        tiles.push(host.tiles());
        let (anvil, mut events) = softphone_with_video(
            client,
            &format!("127.0.0.{}", 79 + i),
            MediaConfig::default(),
            Box::new(Quiet),
            Some(Box::new(host)),
        )
        .await;
        let join = if i == 0 {
            RoomJoin::default()
        } else {
            RoomJoin::from_link(&link.link()).expect("the link's token")
        };
        let call = anvil
            .join_room(&target, &join, CallMedia::VIDEO)
            .await
            .expect("joins");
        let call = established(&mut events, Some(call)).await;
        assert!(anvil.room_forwards(call), "{who}: the room forwards");
        people.push((anvil, events, call));
    }

    let carol_tiles = tiles.pop().unwrap();
    let (carol, carol_events, c) = &mut people[2];
    let streams = streams_where(
        carol_events,
        *c,
        Duration::from_secs(20),
        "two named",
        |s| shown(s).len() == 2,
    )
    .await;
    let named: Vec<(String, anvil_core::video::host::StreamId)> = streams
        .iter()
        .filter(|s| !s.speaker)
        .filter_map(|s| Some((s.member.clone()?, s.stream)))
        .collect();
    let (one, two) = (named[0].clone(), named[1].clone());
    let ask = |big: &str, small: &str| RoomView {
        streams: Some(vec![
            ViewStream {
                target: ViewTarget::Member(big.to_string()),
                max_height: Some(360),
                priority: Some(1),
            },
            ViewStream {
                target: ViewTarget::Member(small.to_string()),
                max_height: Some(90),
                priority: None,
            },
        ]),
        max_streams: Some(2),
    };
    // The heights `one` and `two` are shown at, once both are as asked.
    let shown_at = async |want: (u32, u32), what: &str| {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        let mut last = (None, None);
        while std::time::Instant::now() < deadline {
            let h = |id| carol_tiles.stats(TileKey::Stream(id)).map(|t| t.height);
            last = (h(one.1), h(two.1));
            if last == (Some(want.0), Some(want.1)) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("{what}: shown at {last:?}, not {want:?}");
    };

    carol
        .set_room_view(*c, &ask(&one.0, &two.0))
        .await
        .expect("the view");
    shown_at((360, 90), "one large, the other small").await;
    carol
        .set_room_view(*c, &ask(&two.0, &one.0))
        .await
        .expect("the view");
    shown_at((90, 360), "the other way round").await;

    for (anvil, _, call) in &people {
        let _ = anvil.hangup(*call).await;
    }
    for (anvil, _, _) in people {
        let _ = anvil.shutdown().await;
    }
}

/// The next of `call`'s events that `pick` takes, or a panic naming `what`.
async fn told<T>(
    events: &mut EventStream,
    call: CallId,
    what: &str,
    pick: impl Fn(&Event) -> Option<T>,
) -> T {
    expect(events, Duration::from_secs(15), |e| {
        let mine = match e {
            Event::RoomWebinar { call: c, .. }
            | Event::RoomHand { call: c, .. }
            | Event::RoomStage { call: c, .. }
            | Event::RoomQuestions { call: c, .. }
            | Event::RoomChat { call: c, .. }
            | Event::RoomChatMuted { call: c, .. } => *c == call,
            _ => false,
        };
        if mine {
            pick(e)
        } else {
            None
        }
    })
    .await
    .unwrap_or_else(|| panic!("never told {what}"))
}

/// A webinar on FCP run from two Anvils (FCP's `docs/WEBINARS.md` A7a and
/// A7b). The owner joins by a host's link, and a guest follows a join link
/// and is an attendee in the lobby. The host takes it live through
/// `/me`, and the attendee is let in and told. The attendee raises a hand
/// in the call, and the host sees it on the roster and in the audience.
/// The host brings them to the stage and they are told. The attendee
/// chats, and the host is told. The host mutes their chat, and they are
/// told. The host ends it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_webinar_on_fcp_is_run_by_its_host_from_anvil() {
    use anvil_core::{CallMedia, ChatRequest, ChatTo, QaRequest, RoomJoin, RoomRole, WebinarPhase};
    use anvil_fcp::JoinRole;
    let fcp = fcp_or_skip!();
    let (alice_client, alice_name) = signed_in(&fcp, "webinar-alice").await;
    let (bob_client, _) = signed_in(&fcp, "webinar-bob").await;
    let (alice, mut alice_events) = softphone(&alice_client, "127.0.0.74").await;
    let (bob, mut bob_events) = softphone(&bob_client, "127.0.0.75").await;
    let domain = alice_client.softphone().await.unwrap().account.domain;
    let (meeting, room) = fcp
        .meeting_with(
            &alice_name,
            json!({ "kind": "webinar", "video_enabled": false }),
        )
        .await;
    let link = alice_client
        .join_link(&meeting, JoinRole::Guest, Some("Bob"), None)
        .await
        .expect("a join link");

    // In a webinar the owner is its host by a host's link: dialed by number
    // with no PIN, anyone is an attendee.
    let host_link = alice_client
        .join_link(&meeting, JoinRole::Host, Some("Alice"), None)
        .await
        .expect("a host's link");
    let target = format!("sip:{room}@{domain}");
    let a = alice
        .join_room(
            &target,
            &RoomJoin::from_link(&host_link.link()).expect("the host link's token"),
            CallMedia::AUDIO,
        )
        .await
        .expect("the owner joins as host");
    let a = established(&mut alice_events, Some(a)).await;
    let state = told(&mut alice_events, a, "the host the webinar", |e| match e {
        Event::RoomWebinar { state, .. } => Some(*state),
        _ => None,
    })
    .await;
    assert_eq!(state.phase, WebinarPhase::Practice);
    assert!(
        matches!(state.role, None | Some(RoomRole::Host)),
        "{state:?}"
    );

    let join = RoomJoin::from_link(&link.link()).expect("the link's token");
    let b = bob
        .join_room(&target, &join, CallMedia::AUDIO)
        .await
        .expect("the guest follows the link");
    let b = established(&mut bob_events, Some(b)).await;
    let state = told(&mut bob_events, b, "the attendee the lobby", |e| match e {
        Event::RoomWebinar { state, .. } => Some(*state),
        _ => None,
    })
    .await;
    assert!(
        state.lobby && state.phase == WebinarPhase::Practice,
        "{state:?}"
    );
    assert!(
        matches!(state.role, None | Some(RoomRole::Attendee)),
        "{state:?}"
    );

    // Live: the attendee is let in.
    let host = alice_client.room_host(&room, None);
    bob_client
        .room_host(&room, None)
        .go_live()
        .await
        .expect_err("an attendee does not take it live");
    host.go_live().await.expect("the host takes it live");
    told(&mut bob_events, b, "the webinar live", |e| match e {
        Event::RoomWebinar { state, .. } if state.phase == WebinarPhase::Live && !state.lobby => {
            Some(())
        }
        _ => None,
    })
    .await;

    // A hand, in the call: on the host's roster and in the audience.
    bob.room_qa(b, &QaRequest::Hand { raised: true })
        .await
        .expect("the hand goes up");
    let members = roster_where(&mut alice_events, a, "the attendee's hand", |m| {
        m.iter().any(|m| m.attendee && m.hand.is_some())
    })
    .await;
    let bob_id = members
        .iter()
        .find(|m| m.attendee)
        .map(|m| m.id.clone())
        .unwrap();
    let page = host.audience(None, 50).await.expect("the audience");
    assert!(
        page.data
            .iter()
            .any(|m| m.participant_id == bob_id && m.hand.is_some()),
        "{page:?}"
    );

    // To the stage, and told.
    host.stage(&bob_id, true)
        .await
        .expect("brought to the stage");
    told(&mut bob_events, b, "the stage", |e| match e {
        Event::RoomStage { on: true, .. } => Some(()),
        _ => None,
    })
    .await;

    // Chat: posted, seen by the host; then muted, and told.
    bob.room_chat(
        b,
        // A webinar's chat is to the hosts unless its meeting says.
        &ChatRequest::Post {
            text: "Hello from the audience".into(),
            to: ChatTo::Hosts,
        },
    )
    .await
    .expect("posted");
    told(
        &mut alice_events,
        a,
        "the attendee's message",
        |e| match e {
            Event::RoomChat { messages, .. }
                if messages.iter().any(|m| m.text == "Hello from the audience") =>
            {
                Some(())
            }
            _ => None,
        },
    )
    .await;
    host.mute_chat(&bob_id, true).await.expect("muted");
    told(&mut bob_events, b, "the chat muted", |e| match e {
        Event::RoomChatMuted { on: true, .. } => Some(()),
        _ => None,
    })
    .await;

    host.end_webinar().await.expect("ended");
    told(&mut bob_events, b, "the webinar ended", |e| match e {
        Event::RoomWebinar { state, .. } if state.phase == WebinarPhase::Ended => Some(()),
        _ => None,
    })
    .await;
    let _ = alice.hangup(a).await;
    let _ = bob.hangup(b).await;
    let _ = alice.shutdown().await;
    let _ = bob.shutdown().await;
}
