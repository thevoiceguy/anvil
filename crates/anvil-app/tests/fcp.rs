//! The running phone against FCP (`ANVIL_FCP_ADMIN`, `ANVIL_FCP_TOKEN`,
//! `ANVIL_FCP_SIP`; skipped without them): two signed-in phones registered
//! with FCP, a call placed through one's control socket and answered, and do
//! not disturb set through FCP and shown in the state.

use std::sync::Arc;
use std::time::Duration;

use anvil_app::control::{Client, Endpoint};
use anvil_app::{CallState, Command, Phone, PhoneConfig, Registration};
use anvil_core::audio::{AudioFormat, AudioFrame, AudioHost, AudioSink, AudioSource, DeviceInfo};
use anvil_core::{AnvilConfig, AnvilError, BrandConfig};
use anvil_fcp::{discover, password_sign_in, AppClient, FcpClient};
use async_trait::async_trait;

const SOON: Duration = Duration::from_secs(8);

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

/// A user made through FCP's admin API, signed in as an app.
async fn signed_in(admin: &str, token: &str, who: &str) -> (Arc<FcpClient>, String) {
    use rand::Rng;
    let http = reqwest::Client::new();
    let tenants: serde_json::Value = http
        .get(format!("{admin}/api/v1/tenants"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let tenant = tenants["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t.to_string().contains("anvil"))
        .map(|t| t["id"].as_str().unwrap().to_string())
        .expect("the anvil tenant");
    let n: u32 = rand::thread_rng().gen_range(100_000..999_999);
    let (user, pass) = (format!("anvil-app-{who}-{n}"), format!("Anvil-{n}-pw!"));
    let made = http
        .post(format!("{admin}/api/v1/users"))
        .bearer_auth(token)
        .header("X-FCP-Tenant-Id", &tenant)
        .json(&serde_json::json!({"username": user, "password": pass, "tenant_id": tenant}))
        .send()
        .await
        .unwrap();
    assert!(made.status().is_success());
    let session = password_sign_in(
        discover(admin).await.unwrap(),
        AppClient::this_machine(AppClient::new_install_id()),
        &user,
        &pass,
        None,
    )
    .await
    .unwrap();
    (Arc::new(FcpClient::new(session, None)), user)
}

async fn phone(fcp: Arc<FcpClient>, ip: &str) -> Phone {
    let config = fcp.softphone().await.unwrap();
    let mut account = fcp.account_config(&config).await.unwrap();
    account.bind_addr = Some(format!("{ip}:0"));
    Phone::start(PhoneConfig {
        anvil: AnvilConfig {
            account,
            media: config.media_config(),
            audio: Box::new(Quiet),
            brand: BrandConfig::default(),
        },
        fcp: Some(fcp),
        register: true,
    })
    .await
    .expect("the phone starts and registers")
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_running_phone_calls_through_fcp_and_sets_do_not_disturb() {
    let (Ok(admin), Ok(token)) = (
        std::env::var("ANVIL_FCP_ADMIN"),
        std::env::var("ANVIL_FCP_TOKEN"),
    ) else {
        eprintln!("ANVIL_FCP_ADMIN and ANVIL_FCP_TOKEN not set; skipping");
        return;
    };
    let (alice_fcp, _) = signed_in(&admin, &token, "alice").await;
    let (bob_fcp, bob) = signed_in(&admin, &token, "bob").await;
    let alice = phone(alice_fcp, "127.0.0.120").await;
    let bob_phone = phone(bob_fcp, "127.0.0.121").await;
    until(&alice, "Alice registers", |s| {
        s.registration == Registration::Registered
    })
    .await;
    until(&bob_phone, "Bob registers", |s| {
        s.registration == Registration::Registered
    })
    .await;
    assert_eq!(
        alice.state().dnd,
        Some(false),
        "do not disturb read from FCP"
    );

    let at = if cfg!(windows) {
        Endpoint::Pipe(format!(r"\\.\pipe\anvil-fcp-test-{}", std::process::id()))
    } else {
        Endpoint::Unix(
            std::env::temp_dir().join(format!("anvil-fcp-test-{}.sock", std::process::id())),
        )
    };
    let server = tokio::spawn(anvil_app::control::serve(alice.clone(), at.clone()));
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut control = Client::connect(&at).await.unwrap();

    // `call bob` by his name: dialled in the account's domain.
    let placed = control
        .request(&Command::Call {
            target: bob.clone(),
        })
        .await
        .unwrap();
    assert!(placed.ok, "{placed:?}");
    let call = placed.call.unwrap();
    until(&bob_phone, "Bob rings", |s| {
        s.calls.iter().any(|c| c.state == CallState::Ringing)
    })
    .await;
    assert!(
        bob_phone
            .execute(Command::Answer { call: None })
            .await
            .unwrap()
            .ok
    );
    until(&alice, "the call is up", |s| {
        s.call_state(call) == Some(CallState::Connected)
    })
    .await;
    assert!(
        control
            .request(&Command::Hangup { call: None })
            .await
            .unwrap()
            .ok
    );
    until(&bob_phone, "Bob's call ends", |s| s.calls.is_empty()).await;

    // `dnd on` through FCP: the state follows.
    let set = control.request(&Command::Dnd { on: true }).await.unwrap();
    assert!(set.ok, "{set:?}");
    assert_eq!(alice.state().dnd, Some(true));
    assert!(
        control
            .request(&Command::Dnd { on: false })
            .await
            .unwrap()
            .ok
    );

    server.abort();
    alice.shutdown().await;
    bob_phone.shutdown().await;
}
