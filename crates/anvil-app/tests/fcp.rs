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
use anvil_app::{CallState, Command, Phone, PhoneConfig, Push, Registration};
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
        ring: false,
        fallback_brand: None,
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

    // A push token for an app FCP cannot wake: registered at once, refused
    // with 555, and the phone stays registered without it; then push off.
    let push = control
        .request(&Command::Push {
            address: Some(anvil_core::PushAddress {
                provider: anvil_core::PushProvider::Fcm,
                param: "anvil-no-such-project".into(),
                prid: "anvil-app-test-token".into(),
            }),
        })
        .await
        .unwrap();
    assert!(push.ok, "{push:?}");
    until(&alice, "FCP refuses the push address", |s| {
        matches!(s.push, Push::Refused { .. }) && s.registration == Registration::Registered
    })
    .await;
    assert!(
        control
            .request(&Command::Push { address: None })
            .await
            .unwrap()
            .ok
    );
    until(&alice, "push is off", |s| s.push == Push::Off).await;

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

    // `park`: Bob parks Alice's next call with FCP's park code, dialled as
    // a desk phone dials it: Bob hears the orbit on that call, then both
    // his calls end and Alice waits in the park.
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
    let told = park.call.expect("the park code dialled");
    until(&bob_phone, "the park code answers with the orbit", |s| {
        s.call_state(told) == Some(CallState::Connected)
    })
    .await;
    until(&bob_phone, "Bob's calls end once parked and told", |s| {
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
        // A recording FCP can read when it shares a directory with the test
        // (`ANVIL_FCP_VOICEMAIL_DIR`); else one it cannot.
        let recording = match std::env::var("ANVIL_FCP_VOICEMAIL_DIR") {
            Ok(dir) => {
                let path = PathBuf::from(dir).join(format!("{id}.wav"));
                std::fs::write(&path, half_a_second_wav()).expect("the recording written");
                Some(path)
            }
            Err(_) => None,
        };
        let recording_path = recording
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "/nonexistent/anvil.wav".into());
        db.execute(
            "INSERT INTO voicemail_messages \
             (id, mailbox_id, caller, caller_name, status, priority, duration, \
              recording_path, transcription) \
             VALUES ($1, $2, '+15125550100', 'Carol', 'new', 'urgent', 7, \
                     $3, 'Call me back')",
            &[&id, &mailbox_id, &recording_path],
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

        if recording.is_some() {
            // Played: playing while it plays, heard, then nothing playing.
            assert!(
                bob_phone
                    .execute(Command::Play { id: id.clone() })
                    .await
                    .unwrap()
                    .ok
            );
            let state = bob_phone.state();
            assert_eq!(state.playing.as_deref(), Some(id.as_str()));
            assert!(state.voicemail.iter().any(|m| m.id == id && !m.new));
            until(&bob_phone, "the message plays out", |s| s.playing.is_none()).await;
        } else {
            // A recording FCP cannot read is refused, and nothing plays.
            assert!(bob_phone
                .execute(Command::Play { id: id.clone() })
                .await
                .is_err());
            assert_eq!(bob_phone.state().playing, None);
            eprintln!("ANVIL_FCP_VOICEMAIL_DIR not set; playing tried only as a refusal");
        }

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
        if let Some(path) = recording {
            let _ = std::fs::remove_file(path);
        }
    } else {
        eprintln!("ANVIL_FCP_DATABASE_URL not set; voicemail not tried");
    }

    alice_phone.shutdown().await;
    bob_phone.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// Half a second of a 440 Hz tone: an 8 kHz, 16-bit mono WAV.
fn half_a_second_wav() -> Vec<u8> {
    let samples: Vec<i16> = (0..4000)
        .map(|i| ((i as f32 * 440.0 * std::f32::consts::TAU / 8000.0).sin() * 8000.0) as i16)
        .collect();
    let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&8000u32.to_le_bytes());
    wav.extend_from_slice(&16000u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
    wav.extend_from_slice(&data);
    let clip = anvil_app::decode_wav(&wav).expect("our own WAV decodes");
    assert_eq!((clip.samples.len(), clip.sample_rate), (4000, 8000));
    wav
}

/// A user FCP lets record (`record_on_demand`, set by an administrator)
/// records a call they placed: the server names the call in its answer,
/// `record on` and `record off` go to `/me/calls/{id}/recording`, and the
/// call shows it. Against an FCP before v0.25.466, which has no such
/// setting, it says so and passes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_user_records_a_call_they_placed_when_fcp_lets_them() {
    let (Ok(admin), Ok(token)) = (
        std::env::var("ANVIL_FCP_ADMIN"),
        std::env::var("ANVIL_FCP_TOKEN"),
    ) else {
        eprintln!("ANVIL_FCP_ADMIN and ANVIL_FCP_TOKEN not set; skipping");
        return;
    };
    let (alice_fcp, alice_name) = signed_in(&admin, &token, "rec-alice").await;
    let (bob_fcp, bob) = signed_in(&admin, &token, "rec-bob").await;
    let tenant = anvil_tenant(&admin, &token).await;
    let allowed = reqwest::Client::new()
        .put(format!("{admin}/api/v1/users/{alice_name}"))
        .bearer_auth(&token)
        .header("X-FCP-Tenant-Id", &tenant)
        .json(&serde_json::json!({"media_preferences": {"record_on_demand": true}}))
        .send()
        .await
        .unwrap();
    assert!(allowed.status().is_success(), "{allowed:?}");
    if !alice_fcp.softphone().await.unwrap().may_record {
        eprintln!("this FCP does not let users record (before v0.25.466); skipping");
        return;
    }

    let alice = phone(alice_fcp, "127.0.0.122").await;
    let bob_phone = phone(bob_fcp, "127.0.0.123").await;
    until(&alice, "Alice may record", |s| s.may_record).await;
    until(&bob_phone, "Bob registers", |s| {
        s.registration == Registration::Registered
    })
    .await;
    until(&alice, "Alice registers", |s| {
        s.registration == Registration::Registered
    })
    .await;

    let placed = alice.execute(Command::Call { target: bob }).await.unwrap();
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
    until(&alice, "the call is up and named", |s| {
        s.calls
            .iter()
            .any(|c| c.id == call && c.state == CallState::Connected && c.server_call.is_some())
    })
    .await;

    let on = alice
        .execute(Command::Record {
            call: None,
            on: true,
        })
        .await
        .unwrap();
    assert!(on.ok, "{on:?}");
    let recording = |phone: &Phone| {
        phone
            .state()
            .calls
            .iter()
            .any(|c| c.id == call && c.recording)
    };
    assert!(recording(&alice));
    let off = alice
        .execute(Command::Record {
            call: None,
            on: false,
        })
        .await
        .unwrap();
    assert!(off.ok, "{off:?}");
    assert!(!recording(&alice));
    assert!(
        alice
            .execute(Command::Hangup { call: None })
            .await
            .unwrap()
            .ok
    );
    until(&bob_phone, "Bob's call ends", |s| s.calls.is_empty()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn contacts_on_the_account_reach_every_app_the_user_signs_in_to() {
    let (Ok(admin), Ok(token)) = (
        std::env::var("ANVIL_FCP_ADMIN"),
        std::env::var("ANVIL_FCP_TOKEN"),
    ) else {
        eprintln!("ANVIL_FCP_ADMIN and ANVIL_FCP_TOKEN not set; skipping");
        return;
    };
    let (laptop_fcp, user) = signed_in(&admin, &token, "contacts").await;
    if let Err(e) = laptop_fcp.contacts(None, None, None).await {
        eprintln!("this FCP keeps no contacts (before v0.25.467: {e}); skipping");
        return;
    }
    // The same user on a second install: `signed_in` names the user and
    // the password by one number.
    let n = user.rsplit('-').next().unwrap();
    let mobile_fcp = Arc::new(FcpClient::new(
        password_sign_in(
            discover(&admin).await.unwrap(),
            AppClient::this_machine(AppClient::new_install_id()),
            &user,
            &format!("Anvil-{n}-pw!"),
            None,
        )
        .await
        .unwrap(),
        None,
    ));
    let dir = std::env::temp_dir().join(format!("anvil-contacts-{n}"));
    std::fs::create_dir_all(&dir).unwrap();
    let settings = dir.join("settings.json");
    let laptop = phone_with(laptop_fcp, "127.0.0.124", Some(settings.clone())).await;

    for line in [
        "contact add account 1003 Cy Clark",
        "contact add 1004 Dee Dunn",
    ] {
        let done = laptop
            .execute(Command::parse_line(line).unwrap())
            .await
            .unwrap();
        assert!(done.ok, "{line}: {done:?}");
    }
    let contacts = laptop.state().contacts;
    let placed: Vec<_> = contacts
        .iter()
        .map(|c| (c.name.as_str(), c.on_account))
        .collect();
    assert_eq!(placed, [("Cy Clark", true), ("Dee Dunn", false)]);
    assert!(contacts[0].id.starts_with("contact-"), "FCP named it");

    // The device's contact moved to the account: FCP has it, the device
    // does not.
    let mut dee = contacts[1].clone();
    dee.on_account = true;
    let moved = laptop
        .execute(Command::SaveContact { contact: dee })
        .await
        .unwrap();
    assert!(moved.ok, "{moved:?}");
    let contacts = laptop.state().contacts;
    assert!(contacts.iter().all(|c| c.on_account), "{contacts:?}");
    assert_eq!(contacts.len(), 2);
    assert!(
        !std::fs::read_to_string(&settings)
            .unwrap()
            .contains("Dee Dunn"),
        "the device's copy is gone"
    );
    // An account's contact stays there.
    let mut cy = contacts[0].clone();
    cy.on_account = false;
    assert!(laptop
        .execute(Command::SaveContact { contact: cy })
        .await
        .is_err());

    // The other install reads them when it starts, and changes one.
    let mobile = phone_with(mobile_fcp, "127.0.0.125", None).await;
    until(&mobile, "the account's contacts are read", |s| {
        s.contacts.len() == 2
    })
    .await;
    let mut cy = mobile.state().contacts[0].clone();
    assert_eq!(cy.name, "Cy Clark");
    cy.company = Some("Clark & Co".into());
    assert!(
        mobile
            .execute(Command::SaveContact {
                contact: cy.clone()
            })
            .await
            .unwrap()
            .ok
    );

    // The first sees the change when it reads again, and deletes it.
    assert!(laptop.execute(Command::Refresh).await.unwrap().ok);
    let seen = laptop.state().contacts;
    assert_eq!(seen[0].company.as_deref(), Some("Clark & Co"));
    assert!(
        laptop
            .execute(Command::DeleteContact { id: cy.id.clone() })
            .await
            .unwrap()
            .ok
    );
    assert!(mobile.execute(Command::Refresh).await.unwrap().ok);
    let left: Vec<_> = mobile
        .state()
        .contacts
        .into_iter()
        .map(|c| c.name)
        .collect();
    assert_eq!(left, ["Dee Dunn"]);
    laptop.shutdown().await;
    mobile.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
}
