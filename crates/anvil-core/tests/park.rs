//! FCP's park code (`*70`) answers, says the orbit and hangs up; its
//! answer names the orbit (`X-FCP-Parked`) so the call's end can say where
//! the call went, not that the other side hung up. Against a scripted
//! server on loopback UDP.

mod common;

use anvil_core::{EndReason, Event};
use common::*;
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

/// The URI inside a name-addr's angle brackets.
fn uri_of(name_addr: &str) -> String {
    let start = name_addr.find('<').map_or(0, |i| i + 1);
    let end = name_addr[start..]
        .find('>')
        .map_or(name_addr.len(), |i| start + i);
    name_addr[start..end].to_string()
}

/// Place a call to the park code at a server that answers it with
/// `extra` headers, then sends BYE once ACKed: how the call ended.
async fn park_code_answered_with(extra: &str) -> EndReason {
    let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let here = server.local_addr().unwrap();
    let (anvil, mut events) = start(account(
        "sip:bob@127.0.0.1",
        &format!("sip:{here}"),
        "bob",
        "",
        "127.0.0.1",
        3600,
    ))
    .await;
    let call = anvil
        .place_call(&format!("sip:*70@{here}"))
        .await
        .expect("the park code is dialled");

    let mut buf = vec![0u8; 65536];
    let (n, from) = server.recv_from(&mut buf).await.unwrap();
    let invite = String::from_utf8_lossy(&buf[..n]).to_string();
    assert!(invite.starts_with("INVITE "), "{invite}");
    let sdp = "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\n\
               m=audio 40002 RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\na=sendrecv\r\n";
    let to = format!("{};tag=park", header(&invite, "To"));
    let ok = format!(
        "SIP/2.0 200 OK\r\n\
         Via: {via}\r\n\
         From: {from_h}\r\n\
         To: {to}\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: {cseq}\r\n\
         Contact: <sip:*70@{here}>\r\n\
         {extra}\
         Content-Type: application/sdp\r\n\
         Content-Length: {len}\r\n\r\n{sdp}",
        via = header(&invite, "Via"),
        from_h = header(&invite, "From"),
        call_id = header(&invite, "Call-ID"),
        cseq = header(&invite, "CSeq"),
        len = sdp.len(),
    );
    server.send_to(ok.as_bytes(), from).await.unwrap();
    loop {
        let (n, _) = server.recv_from(&mut buf).await.unwrap();
        if buf[..n].starts_with(b"ACK ") {
            break;
        }
    }
    expect(&mut events, SOON, |e| match e {
        Event::CallEstablished { call: c, .. } if *c == call => Some(()),
        _ => None,
    })
    .await
    .expect("the park code answers");

    // The orbit said, the server hangs up.
    let bye = format!(
        "BYE {target} SIP/2.0\r\n\
         Via: SIP/2.0/UDP {here};branch=z9hG4bK-park-bye\r\n\
         Max-Forwards: 70\r\n\
         From: {to}\r\n\
         To: {from_h}\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: 1 BYE\r\n\
         Content-Length: 0\r\n\r\n",
        target = uri_of(&header(&invite, "Contact")),
        from_h = header(&invite, "From"),
        call_id = header(&invite, "Call-ID"),
    );
    server.send_to(bye.as_bytes(), from).await.unwrap();
    let reason = expect(&mut events, SOON, |e| match e {
        Event::CallEnded { call: c, reason } if *c == call => Some(reason.clone()),
        _ => None,
    })
    .await
    .expect("the park code's call ends");
    let _ = anvil.shutdown().await;
    reason
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_park_codes_call_ends_saying_the_orbit() {
    assert_eq!(
        park_code_answered_with("X-FCP-Parked: 701\r\n").await,
        EndReason::Parked("701".into())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_answer_naming_no_orbit_ends_as_a_hangup() {
    assert_eq!(park_code_answered_with("").await, EndReason::RemoteHangup);
}
