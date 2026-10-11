//! Registering where the app is woken (RFC 8599, FCP's `docs/PUSH.md`
//! phase A1), against a scripted registrar: the push address rides inside
//! the REGISTER's Contact beside the install's instance (RFC 5626), a new
//! token or none is registered at once, and a 555 is asked again without
//! the address.

mod common;

use std::sync::Arc;
use std::time::Duration;

use anvil_core::{Event, PushAddress, PushProvider, PushState};
use common::*;
use parking_lot::Mutex;
use tokio::net::UdpSocket;

const INSTANCE: &str = "<urn:uuid:3f9c0000-0000-4000-8000-0000000000a1>";

/// A registrar on loopback UDP that answers every REGISTER as `answer`
/// says and keeps each one's Contact and Supported.
struct Registrar {
    addr: std::net::SocketAddr,
    seen: Arc<Mutex<Vec<(String, String)>>>,
}

impl Registrar {
    async fn start(answer: fn(&str) -> (u16, &'static str, Vec<String>)) -> Self {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&seen);
        tokio::spawn(async move {
            let mut buf = vec![0u8; 65536];
            loop {
                let Ok((n, from)) = socket.recv_from(&mut buf).await else {
                    return;
                };
                let request = String::from_utf8_lossy(&buf[..n]).to_string();
                if !request.starts_with("REGISTER ") {
                    continue;
                }
                let header = |name: &str| {
                    request
                        .lines()
                        .filter(|l| {
                            l.split_once(':')
                                .is_some_and(|(h, _)| h.trim().eq_ignore_ascii_case(name))
                        })
                        .map(|l| l.split_once(':').unwrap().1.trim().to_string())
                        .collect::<Vec<_>>()
                };
                let contact = header("Contact").join(", ");
                kept.lock()
                    .push((contact.clone(), header("Supported").join(", ")));
                let (code, reason, extra) = answer(&contact);
                let mut response = format!("SIP/2.0 {code} {reason}\r\n");
                for via in header("Via") {
                    response.push_str(&format!("Via: {via}\r\n"));
                }
                for name in ["From", "Call-ID", "CSeq"] {
                    response.push_str(&format!("{name}: {}\r\n", header(name)[0]));
                }
                response.push_str(&format!("To: {};tag=reg1\r\n", header("To")[0]));
                if code == 200 {
                    response.push_str("Expires: 3600\r\n");
                }
                for line in extra {
                    response.push_str(&line);
                    response.push_str("\r\n");
                }
                response.push_str("Content-Length: 0\r\n\r\n");
                let _ = socket.send_to(response.as_bytes(), from).await;
            }
        });
        Self { addr, seen }
    }

    fn contacts(&self) -> Vec<String> {
        self.seen.lock().iter().map(|(c, _)| c.clone()).collect()
    }

    async fn wait_for(&self, count: usize) -> Vec<String> {
        for _ in 0..100 {
            if self.seen.lock().len() >= count {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        self.contacts()
    }
}

fn fcm(token: &str) -> PushAddress {
    PushAddress {
        provider: PushProvider::Fcm,
        param: "anvil-12345".into(),
        prid: token.into(),
    }
}

async fn push_state(events: &mut anvil_core::EventStream) -> Option<PushState> {
    // Told before the registration is: the REGISTER's answer says it.
    expect(events, SOON, |e| match e {
        Event::PushChanged { state } => Some(state.clone()),
        _ => None,
    })
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_push_address_rides_on_every_register() {
    let registrar = Registrar::start(|_| (200, "OK", Vec::new())).await;
    let mut account = account(
        "sip:app@push.test",
        &format!("sip:{}", registrar.addr),
        "app",
        "secret",
        "127.0.0.1",
        3600,
    );
    account.instance_id = Some(INSTANCE.into());
    account.push = Some(fcm("token-1"));
    let (anvil, mut events) = start(account).await;
    anvil.register().await.expect("registers");
    assert_eq!(push_state(&mut events).await, Some(PushState::Accepted));

    let contact = &registrar.contacts()[0];
    assert!(
        contact.contains(";pn-provider=fcm;pn-param=anvil-12345;pn-prid=token-1>"),
        "the push address is the Contact URI's: {contact}"
    );
    assert!(
        contact.contains(&format!("+sip.instance=\"{INSTANCE}\"")) && contact.contains("reg-id=1"),
        "the install's instance: {contact}"
    );
    let supported = registrar.seen.lock()[0].1.clone();
    assert!(supported.contains("outbound"), "Supported: {supported}");

    // A renewed token is registered at once.
    anvil.set_push(Some(fcm("token-2"))).await.unwrap();
    let contacts = registrar.wait_for(2).await;
    assert!(
        contacts[1].contains("pn-prid=token-2>"),
        "the new token: {}",
        contacts[1]
    );

    // Push off: the next REGISTER carries no address.
    anvil.set_push(None).await.unwrap();
    assert_eq!(push_state(&mut events).await, Some(PushState::Off));
    let contacts = registrar.wait_for(3).await;
    assert!(
        !contacts[2].contains("pn-") && contacts[2].contains("+sip.instance"),
        "no push address, the same instance: {}",
        contacts[2]
    );
    let _ = anvil.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_555_registers_without_the_push_address() {
    // A registrar that pushes through APNs only.
    let registrar = Registrar::start(|contact| {
        if contact.contains("pn-provider") {
            (
                555,
                "Push Notification Service Not Supported",
                vec!["Feature-Caps: *;+sip.pns=\"apns\"".to_string()],
            )
        } else {
            (200, "OK", Vec::new())
        }
    })
    .await;
    let mut account = account(
        "sip:app@push.test",
        &format!("sip:{}", registrar.addr),
        "app",
        "secret",
        "127.0.0.1",
        3600,
    );
    account.instance_id = Some(INSTANCE.into());
    account.push = Some(fcm("token-1"));
    let (anvil, mut events) = start(account).await;
    anvil.register().await.expect("registers");
    assert_eq!(
        push_state(&mut events).await,
        Some(PushState::Refused {
            supported: vec!["apns".to_string()]
        })
    );
    let contacts = registrar.contacts();
    assert_eq!(contacts.len(), 2, "{contacts:?}");
    assert!(contacts[0].contains("pn-prid=token-1"));
    assert!(
        !contacts[1].contains("pn-") && contacts[1].contains("+sip.instance"),
        "asked again without the address: {}",
        contacts[1]
    );
    let _ = anvil.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_invite_names_the_call_a_push_did() {
    // FCP names its call in every INVITE (`X-FCP-Call-Id`), as the push
    // that woke the app did: the app knows which INVITE is the call it
    // already shows.
    let (anvil, mut events) = start(account(
        "sip:app@127.0.0.1",
        "sip:127.0.0.1:9",
        "app",
        "",
        "127.0.0.1",
        3600,
    ))
    .await;
    let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let here = server.local_addr().unwrap();
    let sdp = "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\n\
               m=audio 40000 RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\na=sendrecv\r\n";
    let invite = format!(
        "INVITE sip:app@{at} SIP/2.0\r\n\
         Via: SIP/2.0/UDP {here};branch=z9hG4bK-named-1\r\n\
         Max-Forwards: 70\r\n\
         From: \"Ann Lee\" <sip:1002@{here}>;tag=named\r\n\
         To: <sip:app@{at}>\r\n\
         Call-ID: named-1@127.0.0.1\r\n\
         CSeq: 1 INVITE\r\n\
         Contact: <sip:1002@{here}>\r\n\
         X-FCP-Call-Id: 6f1c2a9e-0b7d-4c3e-9a51-2d8f40c7b1aa\r\n\
         Content-Type: application/sdp\r\n\
         Content-Length: {len}\r\n\r\n{sdp}",
        at = anvil.sip_address(),
        len = sdp.len(),
    );
    server
        .send_to(invite.as_bytes(), anvil.sip_address())
        .await
        .unwrap();
    let named = expect(&mut events, SOON, |e| match e {
        Event::IncomingCall {
            display_name,
            server_call,
            ..
        } => Some((display_name.clone(), server_call.clone())),
        _ => None,
    })
    .await;
    assert_eq!(
        named,
        Some((
            Some("Ann Lee".to_string()),
            Some("6f1c2a9e-0b7d-4c3e-9a51-2d8f40c7b1aa".to_string())
        ))
    );
}
