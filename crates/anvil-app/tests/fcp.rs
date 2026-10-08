//! The running phone against FCP (`ANVIL_FCP_ADMIN`, `ANVIL_FCP_TOKEN`,
//! `ANVIL_FCP_SIP`; skipped without them): two signed-in phones registered
//! with FCP, a call placed through one's control socket and answered, do
//! not disturb set through FCP and shown in the state, and the user's data
//! kept — the directory, calling settings, recent calls, favourites, and,
//! with `ANVIL_FCP_DATABASE_URL`, a voicemail message heard and deleted.

use std::path::PathBuf;
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

/// The tenant the Anvil tests use.
async fn anvil_tenant(admin: &str, token: &str) -> String {
    let tenants: serde_json::Value = reqwest::Client::new()
        .get(format!("{admin}/api/v1/tenants"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    tenants["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t.to_string().contains("anvil"))
        .map(|t| t["id"].as_str().unwrap().to_string())
        .expect("the anvil tenant")
}

/// A user made through FCP's admin API, signed in as an app.
async fn signed_in(admin: &str, token: &str, who: &str) -> (Arc<FcpClient>, String) {
    use rand::Rng;
    let http = reqwest::Client::new();
    let tenant = anvil_tenant(admin, token).await;
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
    phone_with(fcp, ip, None).await
}

async fn phone_with(fcp: Arc<FcpClient>, ip: &str, settings_path: Option<PathBuf>) -> Phone {
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
        settings_path,
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

/// A mailbox for `username` (a number, its line, the mailbox), through the
/// admin API; its id.
async fn mailbox(admin: &str, token: &str, username: &str) -> String {
    use rand::Rng;
    let http = reqwest::Client::new();
    let tenant = anvil_tenant(admin, token).await;
    let call = |method: reqwest::Method, path: String, body: serde_json::Value| {
        let req = http
            .request(method, format!("{admin}/api/v1{path}"))
            .bearer_auth(token)
            .header("X-FCP-Tenant-Id", &tenant)
            .json(&body);
        async move {
            let res = req.send().await.unwrap();
            let status = res.status();
            let json: serde_json::Value = res.json().await.unwrap_or_default();
            assert!(status.is_success(), "{path}: {status} {json}");
            json
        }
    };
    let user = call(
        reqwest::Method::GET,
        format!("/users/{username}"),
        serde_json::Value::Null,
    )
    .await;
    let n: u32 = rand::thread_rng().gen_range(1_000_000..9_999_999);
    let number = call(
        reqwest::Method::POST,
        "/phone-numbers".into(),
        serde_json::json!({"e164": format!("+1737{n}"), "tenant_id": tenant}),
    )
    .await;
    let line = call(
        reqwest::Method::POST,
        "/line-bindings".into(),
        serde_json::json!({"phone_number_id": number["id"], "owner_user_id": user["id"],
               "binding_type": "user", "tenant_id": tenant}),
    )
    .await;
    let made = call(
        reqwest::Method::POST,
        "/voicemail/mailboxes".into(),
        serde_json::json!({"tenant_id": tenant, "user_id": user["id"],
               "line_binding_id": line["id"], "pin": "4321", "enabled": true}),
    )
    .await;
    made["id"].as_str().expect("the mailbox's id").to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_phone_keeps_the_users_data() {
    let (Ok(admin), Ok(token)) = (
        std::env::var("ANVIL_FCP_ADMIN"),
        std::env::var("ANVIL_FCP_TOKEN"),
    ) else {
        eprintln!("ANVIL_FCP_ADMIN and ANVIL_FCP_TOKEN not set; skipping");
        return;
    };
    let dir = std::env::temp_dir().join(format!("anvil-app-data-{}", std::process::id()));
    let settings = dir.join("settings.json");
    let _ = std::fs::remove_dir_all(&dir);

    let (alice_fcp, alice) = signed_in(&admin, &token, "data-a").await;
    let (bob_fcp, bob) = signed_in(&admin, &token, "data-b").await;
    let alice_phone = phone(alice_fcp, "127.0.0.122").await;
    let bob_phone = phone_with(Arc::clone(&bob_fcp), "127.0.0.123", Some(settings.clone())).await;
    for (p, who) in [(&alice_phone, "Alice"), (&bob_phone, "Bob")] {
        until(p, &format!("{who} registers"), |s| {
            s.registration == Registration::Registered
        })
        .await;
    }

    // The directory, read at start: Alice is in it.
    until(&bob_phone, "Alice in Bob's directory", |s| {
        s.people.iter().any(|p| p.key == alice)
    })
    .await;

    // Calling settings: read at start, a forward set and cleared.
    assert!(bob_phone.state().calling.is_some(), "read at start");
    let set = bob_phone
        .execute(Command::parse_line("forward busy 1999").unwrap())
        .await
        .unwrap();
    assert!(set.ok, "{set:?}");
    let busy = bob_phone.state().calling.unwrap().forward_busy;
    assert!(
        busy.as_deref().is_some_and(|f| f.contains("1999")),
        "{busy:?}"
    );
    assert!(
        bob_phone
            .execute(Command::parse_line("forward busy off").unwrap())
            .await
            .unwrap()
            .ok
    );
    assert_eq!(bob_phone.state().calling.unwrap().forward_busy, None);

    // A call from Alice, answered and ended: in Bob's recents, incoming.
    let call = alice_phone
        .execute(Command::Call {
            target: bob.clone(),
        })
        .await
        .unwrap()
        .call
        .unwrap();
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
    until(&alice_phone, "the call is up", |s| {
        s.call_state(call) == Some(CallState::Connected)
    })
    .await;
    // Alice's lamp, from FCP's live events.
    until(&bob_phone, "Alice's lamp lights", |s| {
        s.people.iter().any(|p| p.key == alice && p.on_call)
    })
    .await;
    assert!(
        alice_phone
            .execute(Command::Hangup { call: None })
            .await
            .unwrap()
            .ok
    );
    until(&bob_phone, "Alice's lamp goes out", |s| {
        s.people.iter().any(|p| p.key == alice && !p.on_call)
    })
    .await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let recents = bob_phone.state().recents;
        if let Some(r) = recents.iter().find(|r| r.remote.contains(&alice)) {
            assert_eq!(r.direction, anvil_app::Direction::Incoming, "{r:?}");
            assert!(!r.missed, "{r:?}");
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the call in Bob's recents: {recents:?}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // `park`: Bob parks Alice's next call with FCP's park code; Bob's leg
    // ends, Alice waits in the park.
    let parked = alice_phone
        .execute(Command::Call {
            target: bob.clone(),
        })
        .await
        .unwrap()
        .call
        .unwrap();
    until(&bob_phone, "Bob rings again", |s| {
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
    until(&alice_phone, "the second call is up", |s| {
        s.call_state(parked) == Some(CallState::Connected)
    })
    .await;
    let park = bob_phone
        .execute(Command::Park { call: None })
        .await
        .unwrap();
    assert!(park.ok, "{park:?}");
    until(&bob_phone, "Bob's leg ends once parked", |s| {
        s.calls.is_empty()
    })
    .await;
    assert_eq!(
        alice_phone.state().call_state(parked),
        Some(CallState::Connected),
        "Alice waits in the park"
    );
    assert!(
        alice_phone
            .execute(Command::Hangup { call: None })
            .await
            .unwrap()
            .ok
    );

    // Alice a favourite: in the state, and kept on this device.
    assert!(
        bob_phone
            .execute(Command::Favourite {
                who: alice.clone(),
                on: true,
            })
            .await
            .unwrap()
            .ok
    );
    assert!(bob_phone
        .state()
        .people
        .iter()
        .any(|p| p.key == alice && p.favourite));
    let kept = std::fs::read_to_string(&settings).expect("the settings are saved");
    assert!(kept.contains(&alice), "{kept}");

    // Voicemail, when the test may write a message into FCP's database.
    if let Ok(url) = std::env::var("ANVIL_FCP_DATABASE_URL") {
        let mailbox_id = mailbox(&admin, &token, &bob).await;
        let (db, connection) = tokio_postgres::connect(&url, tokio_postgres::NoTls)
            .await
            .expect("FCP's database");
        tokio::spawn(connection);
        let id = format!("msg-anvil-{}", std::process::id());
        db.execute(
            "INSERT INTO voicemail_messages \
             (id, mailbox_id, caller, caller_name, status, priority, duration, \
              recording_path, transcription) \
             VALUES ($1, $2, '+15125550100', 'Carol', 'new', 'urgent', 7, \
                     '/nonexistent/anvil.wav', 'Call me back')",
            &[&id, &mailbox_id],
        )
        .await
        .expect("a message left");

        assert!(bob_phone.execute(Command::Refresh).await.unwrap().ok);
        let message = bob_phone
            .state()
            .voicemail
            .into_iter()
            .find(|m| m.id == id)
            .expect("the message in the state");
        assert!(message.new && message.urgent, "{message:?}");
        assert_eq!(message.transcription.as_deref(), Some("Call me back"));
        assert_eq!(message.duration_secs, 7);

        assert!(
            bob_phone
                .execute(Command::Heard { id: id.clone() })
                .await
                .unwrap()
                .ok
        );
        assert!(bob_phone
            .state()
            .voicemail
            .iter()
            .any(|m| m.id == id && !m.new));
        let page = bob_fcp.voicemail(None, None).await.unwrap();
        assert!(
            page.data.iter().any(|m| m.id == id && m.status == "heard"),
            "{page:?}"
        );

        assert!(
            bob_phone
                .execute(Command::DeleteVoicemail { id: id.clone() })
                .await
                .unwrap()
                .ok
        );
        assert!(bob_phone.state().voicemail.iter().all(|m| m.id != id));
        let page = bob_fcp.voicemail(None, None).await.unwrap();
        assert!(page.data.iter().all(|m| m.id != id), "{page:?}");
    } else {
        eprintln!("ANVIL_FCP_DATABASE_URL not set; voicemail not tried");
    }

    alice_phone.shutdown().await;
    bob_phone.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
}
