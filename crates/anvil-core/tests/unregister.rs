//! A phone that stops takes its registration back (Expires: 0), with the
//! Contact it registered: a binding left behind kept ringing, and stayed on
//! FCP's Devices page, until it lapsed (FCP's D8). Against a scripted
//! registrar on loopback UDP.

mod common;

use std::sync::Arc;

use common::*;
use parking_lot::Mutex;
use tokio::net::UdpSocket;

/// The header named `name` in a SIP message, first value.
fn header(message: &str, name: &str) -> String {
    message
        .lines()
        .find_map(|l| {
            let (h, v) = l.split_once(':')?;
            h.trim()
                .eq_ignore_ascii_case(name)
                .then(|| v.trim().to_string())
        })
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_phone_that_stops_unregisters_its_contact() {
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let here = socket.local_addr().unwrap();
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
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
            kept.lock().push(request.clone());
            let response = format!(
                "SIP/2.0 200 OK\r\nVia: {}\r\nFrom: {}\r\nTo: {};tag=reg1\r\n\
                 Call-ID: {}\r\nCSeq: {}\r\nContact: {}\r\nContent-Length: 0\r\n\r\n",
                header(&request, "Via"),
                header(&request, "From"),
                header(&request, "To"),
                header(&request, "Call-ID"),
                header(&request, "CSeq"),
                header(&request, "Contact"),
            );
            let _ = socket.send_to(response.as_bytes(), from).await;
        }
    });

    let (anvil, mut events) = start(account(
        "sip:app@127.0.0.1",
        &format!("sip:{here}"),
        "app",
        "",
        "127.0.0.1",
        3600,
    ))
    .await;
    register(&anvil, &mut events).await;
    anvil.shutdown().await.unwrap();

    let seen = seen.lock().clone();
    assert_eq!(seen.len(), 2, "a REGISTER and its un-REGISTER: {seen:?}");
    let contact = |r: &str| {
        let c = header(r, "Contact");
        c[..c.find('>').map_or(c.len(), |i| i + 1)].to_string()
    };
    assert_eq!(contact(&seen[1]), contact(&seen[0]), "the same Contact");
    assert!(
        header(&seen[1], "Expires") == "0" || header(&seen[1], "Contact").contains("expires=0"),
        "Expires: 0\n{}",
        seen[1]
    );
}
