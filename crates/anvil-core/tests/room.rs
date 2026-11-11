//! Anvil joining a video room as FCP's are joined (FCP's
//! `docs/SOFTPHONE_VIDEO.md` §2.6), against a room scripted here over UDP:
//! the INVITE carries the join link's token and asks for the floor's
//! notices, offers the shared screen its own section, and once answered
//! subscribes to the roster in the call; the roster's NOTIFYs are told in
//! order, a gap asks for the roster whole again, and the floor's INFOs
//! are told. Runs everywhere: no network beyond the loopback.

mod common;

use std::net::SocketAddr;

use anvil_core::video::synthetic::SyntheticVideoHost;
use anvil_core::{CallMedia, Event, FloorState, MediaConfig, RoomAudio, RoomJoin};
use common::*;
use tokio::net::UdpSocket;

/// A room on a UDP socket, one call at a time.
struct Room {
    socket: UdpSocket,
    addr: SocketAddr,
    /// The phone's INVITE, once received, and where it came from.
    invite: String,
    phone: SocketAddr,
    cseq: u32,
}

/// A header's value in `text`.
fn header(text: &str, name: &str) -> Option<String> {
    text.split("\r\n\r\n")
        .next()?
        .lines()
        .skip(1)
        .find_map(|l| {
            let (n, v) = l.split_once(':')?;
            n.trim()
                .eq_ignore_ascii_case(name)
                .then(|| v.trim().to_string())
        })
}

fn body(text: &str) -> &str {
    text.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("")
}

impl Room {
    async fn bind(ip: &str) -> Room {
        let socket = UdpSocket::bind(format!("{ip}:0")).await.unwrap();
        let addr = socket.local_addr().unwrap();
        Room {
            socket,
            addr,
            invite: String::new(),
            phone: addr,
            cseq: 1,
        }
    }

    /// The next message starting `start` (a method, or `SIP/2.0 <code>`).
    async fn next(&self, start: &str) -> (String, SocketAddr) {
        let mut buf = vec![0u8; 65536];
        loop {
            let (n, from) = tokio::time::timeout(SOON, self.socket.recv_from(&mut buf))
                .await
                .unwrap_or_else(|_| panic!("no {start}"))
                .unwrap();
            let text = String::from_utf8_lossy(&buf[..n]).to_string();
            if text.starts_with(start) {
                return (text, from);
            }
        }
    }

    /// `code` to `request`, with `extra` headers and `body` of `kind`.
    async fn answer(
        &self,
        request: &str,
        to: SocketAddr,
        code: &str,
        extra: &str,
        kind: &str,
        body: &str,
    ) {
        let mut out = format!("SIP/2.0 {code}\r\n");
        for line in request.split("\r\n\r\n").next().unwrap().lines().skip(1) {
            let name = line
                .split(':')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            match name.as_str() {
                "via" | "from" | "call-id" | "cseq" => {
                    out.push_str(line);
                    out.push_str("\r\n");
                }
                "to" => {
                    out.push_str(line);
                    if !line.contains("tag=") {
                        out.push_str(";tag=room");
                    }
                    out.push_str("\r\n");
                }
                _ => {}
            }
        }
        out.push_str(&format!("Contact: <sip:room@{}>\r\n{extra}", self.addr));
        if !kind.is_empty() {
            out.push_str(&format!("Content-Type: {kind}\r\n"));
        }
        out.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
        self.socket.send_to(out.as_bytes(), to).await.unwrap();
    }

    /// A request of ours in the call, as the room sends one: to the phone's
    /// Contact, from the room's side of the dialog.
    async fn send(&mut self, method: &str, extra: &str, kind: &str, body: &str) -> String {
        self.cseq += 1;
        let contact = header(&self.invite, "Contact").unwrap();
        let target = contact
            .trim_start_matches('<')
            .split('>')
            .next()
            .unwrap()
            .to_string();
        let from = header(&self.invite, "From").unwrap();
        let to = header(&self.invite, "To").unwrap();
        let call_id = header(&self.invite, "Call-ID").unwrap();
        let request = format!(
            "{method} {target} SIP/2.0\r\n\
             Via: SIP/2.0/UDP {addr};branch=z9hG4bKroom{cseq}\r\n\
             Max-Forwards: 70\r\n\
             From: {to};tag=room\r\n\
             To: {from}\r\n\
             Call-ID: {call_id}\r\n\
             CSeq: {cseq} {method}\r\n\
             Contact: <sip:room@{addr}>\r\n\
             {extra}Content-Type: {kind}\r\n\
             Content-Length: {len}\r\n\r\n{body}",
            addr = self.addr,
            cseq = self.cseq,
            len = body.len(),
        );
        self.socket
            .send_to(request.as_bytes(), self.phone)
            .await
            .unwrap();
        let (response, _) = self.next("SIP/2.0 ").await;
        response
    }

    /// Answer the phone's INVITE with `sdp` and take its ACK.
    async fn admit(&mut self, sdp: &str) {
        let (invite, from) = self.next("INVITE ").await;
        self.invite = invite.clone();
        self.phone = from;
        self.answer(&invite, from, "200 OK", "", "application/sdp", sdp)
            .await;
        self.next("ACK ").await;
    }
}

/// An audio-only room's answer.
fn audio_answer(ip: &str) -> String {
    format!(
        "v=0\r\no=- 1 1 IN IP4 {ip}\r\ns=-\r\nc=IN IP4 {ip}\r\nt=0 0\r\n\
         m=audio 40010 RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\n"
    )
}

fn roster(state: &str, version: u32, users: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <conference-info xmlns=\"urn:ietf:params:xml:ns:conference-info\" \
         xmlns:fcp=\"urn:fcp:params:xml:ns:conference-info\" \
         entity=\"sip:3000@room\" state=\"{state}\" version=\"{version}\">\n\
         <users>\n{users}</users>\n</conference-info>\n"
    )
}

fn user(id: &str, name: &str, audio: &str, speaking: bool) -> String {
    format!(
        "<user entity=\"sip:{id}@room\" state=\"full\"><display-text>{name}</display-text>\
         <roles><entry>participant</entry></roles>\
         <endpoint entity=\"sip:{id}@room\" state=\"full\"><status>connected</status>\
         <media id=\"audio\"><type>audio</type><status>{audio}</status></media>\
         <fcp:speaking>{speaking}</fcp:speaking><fcp:presenting>false</fcp:presenting>\
         </endpoint></user>\n"
    )
}

const ROSTER_TYPE: &str = "application/conference-info+xml";
const FLOOR_TYPE: &str = "application/vnd.fcp.content+json";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_room_joined_by_link_tells_its_roster_and_its_floor() {
    let (alice, mut events) = start(account(
        "sip:alice@127.0.0.1",
        "sip:127.0.0.1:9",
        "alice",
        "",
        "127.0.0.1",
        3600,
    ))
    .await;
    let mut room = Room::bind("127.0.0.7").await;
    let join = RoomJoin::from_link("fcp://join/tok.en").unwrap();
    let call = alice
        .join_room(&format!("sip:3000@{}", room.addr), &join, CallMedia::AUDIO)
        .await
        .expect("the room is dialed");

    room.admit(&audio_answer("127.0.0.7")).await;
    assert_eq!(
        header(&room.invite, "X-FCP-Join").as_deref(),
        Some("tok.en")
    );
    assert_eq!(
        header(&room.invite, "Recv-Info").as_deref(),
        Some("fcp-content")
    );
    expect(&mut events, SOON, |e| match e {
        Event::CallEstablished { call: c, .. } if *c == call => Some(()),
        _ => None,
    })
    .await
    .expect("in the room");

    // The roster, subscribed to in the call.
    let (subscribe, from) = room.next("SUBSCRIBE ").await;
    assert_eq!(header(&subscribe, "Event").as_deref(), Some("conference"));
    assert_eq!(header(&subscribe, "Accept").as_deref(), Some(ROSTER_TYPE));
    assert_eq!(
        header(&subscribe, "Call-ID"),
        header(&room.invite, "Call-ID")
    );
    assert!(
        header(&subscribe, "To").unwrap().contains("tag=room"),
        "in the call's dialog: {subscribe}"
    );
    room.answer(&subscribe, from, "200 OK", "Expires: 600\r\n", "", "")
        .await;

    let whole = roster(
        "full",
        1,
        &(user("p1", "Alice", "sendrecv", false) + &user("p2", "Bob", "recvonly", true)),
    );
    let ok = room
        .send(
            "NOTIFY",
            "Event: conference\r\nSubscription-State: active;expires=600\r\n",
            ROSTER_TYPE,
            &whole,
        )
        .await;
    assert!(ok.starts_with("SIP/2.0 200"), "{ok}");
    let (members, speaker) = expect(&mut events, SOON, |e| match e {
        Event::RoomRoster {
            call: c,
            members,
            speaker,
        } if *c == call => Some((members.clone(), speaker.clone())),
        _ => None,
    })
    .await
    .expect("the roster is told");
    assert_eq!(members.len(), 2);
    assert_eq!(members[1].name, "Bob");
    assert_eq!(members[1].audio, RoomAudio::Muted);
    assert_eq!(speaker.as_deref(), Some("p2"));

    // Version 2 was missed: the roster is asked for whole again.
    let skipped = roster("partial", 3, &user("p1", "Alice", "sendrecv", true));
    room.send(
        "NOTIFY",
        "Event: conference\r\nSubscription-State: active\r\n",
        ROSTER_TYPE,
        &skipped,
    )
    .await;
    let (again, from) = room.next("SUBSCRIBE ").await;
    room.answer(&again, from, "200 OK", "Expires: 600\r\n", "", "")
        .await;
    let whole = roster("full", 4, &user("p1", "Alice", "sendrecv", true));
    room.send(
        "NOTIFY",
        "Event: conference\r\nSubscription-State: active\r\n",
        ROSTER_TYPE,
        &whole,
    )
    .await;
    let members = expect(&mut events, SOON, |e| match e {
        Event::RoomRoster {
            call: c,
            members,
            speaker,
        } if *c == call => {
            assert_eq!(speaker.as_deref(), Some("p1"));
            Some(members.clone())
        }
        _ => None,
    })
    .await
    .expect("the whole roster again");
    assert_eq!(
        members.len(),
        1,
        "Bob left while the roster was out of step"
    );

    // Bob shares; then the floor's other notices are not taken.
    let granted =
        r#"{"floor":"granted","participant_id":"p2","presenter":"Bob","reason":null,"mine":false}"#;
    let ok = room
        .send("INFO", "Info-Package: fcp-content\r\n", FLOOR_TYPE, granted)
        .await;
    assert!(ok.starts_with("SIP/2.0 200"), "{ok}");
    let notice = expect(&mut events, SOON, |e| match e {
        Event::RoomFloor { call: c, notice } if *c == call => Some(notice.clone()),
        _ => None,
    })
    .await
    .expect("the floor is told");
    assert_eq!(notice.state, FloorState::Granted);
    assert_eq!(notice.presenter.as_deref(), Some("Bob"));
    let other = room
        .send(
            "INFO",
            "Info-Package: something-else\r\n",
            "application/json",
            "{}",
        )
        .await;
    assert!(other.starts_with("SIP/2.0 469"), "{other}");
    assert_eq!(header(&other, "Recv-Info").as_deref(), Some("fcp-content"));

    // Leaving.
    let alice = std::sync::Arc::new(alice);
    let hangup = {
        let alice = std::sync::Arc::clone(&alice);
        tokio::spawn(async move { alice.hangup(call).await })
    };
    let (bye, from) = room.next("BYE ").await;
    room.answer(&bye, from, "200 OK", "", "", "").await;
    hangup.await.unwrap().expect("the BYE is answered");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_room_dialed_with_video_is_offered_a_section_for_its_shared_screen() {
    let (alice, _events) = start_with_video(
        account(
            "sip:alice@127.0.0.1",
            "sip:127.0.0.1:9",
            "alice",
            "",
            "127.0.0.1",
            3600,
        ),
        MediaConfig::default(),
        Some(SyntheticVideoHost::default()),
    )
    .await;
    // An INVITE this large goes over TCP (RFC 3261 §18.1.1).
    let room = tokio::net::TcpListener::bind("127.0.0.8:0").await.unwrap();
    let addr = room.local_addr().unwrap();
    alice
        .join_room(
            &format!("sip:3000@{addr}"),
            &RoomJoin::default(),
            CallMedia::VIDEO,
        )
        .await
        .expect("the room is dialed");
    let invite = tokio::time::timeout(SOON, async {
        use tokio::io::AsyncReadExt;
        let (mut stream, _) = room.accept().await.unwrap();
        let mut text = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = stream.read(&mut buf).await.unwrap();
            assert!(n > 0, "the INVITE is cut short");
            text.extend_from_slice(&buf[..n]);
            let s = String::from_utf8_lossy(&text).to_string();
            if let Some((head, rest)) = s.split_once("\r\n\r\n") {
                let len: usize = header(&format!("{head}\r\n\r\n"), "Content-Length")
                    .and_then(|l| l.parse().ok())
                    .unwrap_or(0);
                if rest.len() >= len {
                    return s;
                }
            }
        }
    })
    .await
    .expect("an INVITE");
    assert_eq!(header(&invite, "X-FCP-Join"), None, "dialed by number");
    assert_eq!(header(&invite, "Recv-Info").as_deref(), Some("fcp-content"));
    let sdp = body(&invite);
    let video: Vec<&str> = sdp.split("m=").filter(|s| s.starts_with("video")).collect();
    assert_eq!(video.len(), 2, "a camera and the shared screen: {sdp}");
    assert!(
        video[0].contains("a=sendrecv") && !video[0].contains("a=content"),
        "{sdp}"
    );
    assert!(
        video[1].contains("a=content:slides") && video[1].contains("a=recvonly"),
        "{sdp}"
    );
}

/// The payload type `section` (an m= section's text) gives VP8.
fn vp8_pt(section: &str) -> String {
    section
        .lines()
        .find_map(|l| {
            let rest = l.trim().strip_prefix("a=rtpmap:")?;
            let (pt, codec) = rest.split_once(' ')?;
            codec.starts_with("VP8/").then(|| pt.to_string())
        })
        .expect("VP8 offered")
}

/// The video sections of an SDP, in order.
fn video_sections(sdp: &str) -> Vec<String> {
    sdp.split("\r\nm=")
        .filter(|s| s.starts_with("video"))
        .map(|s| format!("m={s}"))
        .collect()
}

/// A room's answer: audio, the camera and the shared screen, each video
/// section on its own port and as `content` says for the screen.
fn video_answer(ip: &str, pt: &str, camera: u16, screen: u16, content: &str) -> String {
    format!(
        "v=0\r\no=- 1 1 IN IP4 {ip}\r\ns=-\r\nc=IN IP4 {ip}\r\nt=0 0\r\n\
         m=audio 40010 RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\n\
         m=video {camera} RTP/AVPF {pt}\r\na=rtpmap:{pt} VP8/90000\r\na=sendrecv\r\n\
         m=video {screen} RTP/AVPF {pt}\r\na=rtpmap:{pt} VP8/90000\r\n\
         a=content:slides\r\na={content}\r\n"
    )
}

/// Whether a packet reaches `socket` within `within`.
async fn arrives(socket: &UdpSocket, within: std::time::Duration) -> bool {
    let mut buf = [0u8; 2048];
    tokio::time::timeout(within, socket.recv_from(&mut buf))
        .await
        .is_ok()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_share_the_room_refuses_stops_at_once_and_is_ended_by_re_invite() {
    use anvil_core::video::host::VideoCodec;
    use std::time::Duration;
    let (alice, mut events) = start_with_video(
        account(
            "sip:alice@127.0.0.1",
            "sip:127.0.0.1:9",
            "alice",
            "",
            "127.0.0.1",
            3600,
        ),
        MediaConfig::default(),
        // One codec keeps the INVITE within UDP's size.
        Some(SyntheticVideoHost::new(&[VideoCodec::Vp8]).with_fps(50)),
    )
    .await;
    let alice = std::sync::Arc::new(alice);
    let mut room = Room::bind("127.0.0.9").await;
    let camera = UdpSocket::bind("127.0.0.9:0").await.unwrap();
    let screen = UdpSocket::bind("127.0.0.9:0").await.unwrap();
    let (camera_port, screen_port) = (
        camera.local_addr().unwrap().port(),
        screen.local_addr().unwrap().port(),
    );
    let call = alice
        .join_room(
            &format!("sip:3000@{}", room.addr),
            &RoomJoin::default(),
            CallMedia::VIDEO,
        )
        .await
        .expect("the room is dialed");
    let (invite, from) = room.next("INVITE ").await;
    room.invite = invite.clone();
    room.phone = from;
    let offered = video_sections(body(&invite));
    assert_eq!(offered.len(), 2, "{invite}");
    assert!(offered[1].contains("a=recvonly"), "{}", offered[1]);
    let pt = vp8_pt(&offered[1]);
    let answer = video_answer("127.0.0.9", &pt, camera_port, screen_port, "sendrecv");
    room.answer(&invite, from, "200 OK", "", "application/sdp", &answer)
        .await;
    room.next("ACK ").await;
    expect(&mut events, SOON, |e| match e {
        Event::CallEstablished { call: c, .. } if *c == call => Some(()),
        _ => None,
    })
    .await
    .expect("in the room");
    assert!(
        !arrives(&screen, Duration::from_millis(300)).await,
        "no screen before a share"
    );

    // Alice shares: the re-INVITE sends on the screen's section.
    let share = {
        let alice = std::sync::Arc::clone(&alice);
        tokio::spawn(async move { alice.share_screen(call, true).await })
    };
    let (reinvite, from) = room.next("INVITE ").await;
    let offered = video_sections(body(&reinvite));
    assert!(
        offered[1].contains("a=content:slides") && offered[1].contains("a=sendrecv"),
        "{}",
        offered[1]
    );
    room.answer(&reinvite, from, "200 OK", "", "application/sdp", &answer)
        .await;
    room.next("ACK ").await;
    share.await.unwrap().expect("the share is answered");
    assert!(arrives(&screen, SOON).await, "her screen reaches the room");

    // The room refuses it (the floor is held): her screen stops at once,
    // before the stop is offered.
    let refused = r#"{"floor":"refused","participant_id":"p1","presenter":"Bob","reason":"held","mine":true}"#;
    let ok = room
        .send("INFO", "Info-Package: fcp-content\r\n", FLOOR_TYPE, refused)
        .await;
    assert!(ok.starts_with("SIP/2.0 200"), "{ok}");
    let notice = expect(&mut events, SOON, |e| match e {
        Event::RoomFloor { call: c, notice } if *c == call => Some(notice.clone()),
        _ => None,
    })
    .await
    .expect("the refusal is told");
    assert_eq!(notice.state, FloorState::Refused);
    assert!(notice.mine);
    // What was in flight drains; then nothing.
    tokio::time::sleep(Duration::from_millis(200)).await;
    while arrives(&screen, Duration::from_millis(10)).await {}
    assert!(
        !arrives(&screen, Duration::from_millis(500)).await,
        "her screen still claims the floor"
    );

    // Told, she stops: the section goes back to receiving only.
    let stop = {
        let alice = std::sync::Arc::clone(&alice);
        tokio::spawn(async move { alice.share_screen(call, false).await })
    };
    let (reinvite, from) = room.next("INVITE ").await;
    let offered = video_sections(body(&reinvite));
    assert!(
        offered[1].contains("a=content:slides") && offered[1].contains("a=recvonly"),
        "{}",
        offered[1]
    );
    let answer = video_answer("127.0.0.9", &pt, camera_port, screen_port, "sendonly");
    room.answer(&reinvite, from, "200 OK", "", "application/sdp", &answer)
        .await;
    room.next("ACK ").await;
    stop.await.unwrap().expect("the stop is answered");
    assert!(!alice.sharing(call));
}
