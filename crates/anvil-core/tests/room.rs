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
        Some("fcp-content"),
        "without video, no streams of anyone's are asked for"
    );
    expect(&mut events, SOON, |e| match e {
        Event::CallEstablished { call: c, .. } if *c == call => Some(()),
        _ => None,
    })
    .await
    .expect("in the room");
    assert!(!alice.room_forwards(call));
    assert!(
        alice
            .set_room_view(call, &anvil_core::RoomView::default())
            .await
            .is_err(),
        "a view is not asked of a room that does not forward"
    );

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
            ..
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
            ..
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
    assert_eq!(
        header(&invite, "Recv-Info").as_deref(),
        Some("fcp-content, fcp-view"),
        "with video, each person's stream is asked for"
    );
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

/// Whether an RTP packet of payload type `pt` reaches `socket` within
/// `within`. Anything else is noted and passed over.
async fn arrives(socket: &UdpSocket, pt: u8, within: std::time::Duration) -> bool {
    let mut buf = [0u8; 2048];
    let end = tokio::time::Instant::now() + within;
    loop {
        match tokio::time::timeout_at(end, socket.recv_from(&mut buf)).await {
            Err(_) => return false,
            Ok(Ok((n, _))) if n >= 12 && buf[0] >> 6 == 2 && buf[1] & 0x7f == pt => return true,
            Ok(Ok((n, from))) => eprintln!(
                "passed over: {n} bytes from {from}: {:02x?}",
                &buf[..n.min(4)]
            ),
            Ok(Err(e)) => eprintln!("passed over: {e}"),
        }
    }
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
    let rtp_pt: u8 = pt.parse().unwrap();
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
        !arrives(&screen, rtp_pt, Duration::from_millis(300)).await,
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
    assert!(
        arrives(&screen, rtp_pt, SOON).await,
        "her screen reaches the room"
    );

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
    // What queued in the socket meanwhile is read until 200 ms pass
    // without a packet (a screen still sent at 50 fps never goes quiet);
    // a short wait is not enough on Windows, where a ready socket is
    // told through a poll that can lose the race to a 10 ms timer.
    let drained = tokio::time::timeout(Duration::from_secs(3), async {
        while arrives(&screen, rtp_pt, Duration::from_millis(200)).await {}
    })
    .await;
    assert!(drained.is_ok(), "her screen never went quiet");
    assert!(
        !arrives(&screen, rtp_pt, Duration::from_millis(500)).await,
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

/// A member of a forwarding room as FCP lists them to one subscriber: their
/// own stream to us on `src`, and with `speaker` the speaker stream showing
/// them.
fn forwarded_user(id: &str, name: &str, src: u32, speaker: Option<u32>) -> String {
    let speaker = speaker
        .map(|s| {
            format!(
                "<media id=\"speaker\"><type>video</type><label>speaker</label>\
                 <src-id>{s}</src-id><status>sendonly</status></media>"
            )
        })
        .unwrap_or_default();
    format!(
        "<user entity=\"sip:{id}@room\" state=\"full\"><display-text>{name}</display-text>\
         <roles><entry>participant</entry></roles>\
         <endpoint entity=\"sip:{id}@room\" state=\"full\"><status>connected</status>\
         <media id=\"audio\"><type>audio</type><status>sendrecv</status></media>\
         <media id=\"video\"><type>video</type><src-id>{src}</src-id><status>sendrecv</status>\
         <fcp:camera>on</fcp:camera></media>{speaker}\
         <fcp:speaking>false</fcp:speaking><fcp:presenting>false</fcp:presenting>\
         </endpoint></user>\n"
    )
}

/// The port of the `m=` line in `section`.
fn section_port(section: &str) -> u16 {
    section
        .split_whitespace()
        .nth(1)
        .and_then(|p| p.parse().ok())
        .expect("a port")
}

/// A VP8 packet of `ssrc`: what a forwarding room sends each stream as.
fn vp8_packet(pt: u8, ssrc: u32, seq: u16) -> Vec<u8> {
    let mut p = vec![0x80, 0x80 | pt];
    p.extend_from_slice(&seq.to_be_bytes());
    p.extend_from_slice(&(u32::from(seq) * 3000).to_be_bytes());
    p.extend_from_slice(&ssrc.to_be_bytes());
    // The VP8 descriptor (start of a partition), then a keyframe's start.
    p.extend_from_slice(&[
        0x10, 0x50, 0x01, 0x00, 0x9d, 0x01, 0x2a, 0x40, 0x01, 0xf0, 0x00,
    ]);
    p
}

/// A room that forwards (FCP's `docs/FORWARDING_ROOMS.md` §2.2–2.3): its
/// answer says `Recv-Info: fcp-view`, each person's stream comes down the
/// camera section under an SSRC of its own, the roster's `<src-id>`s say
/// whose each is (whichever arrives first), and a view is asked of it in
/// the call as an `fcp-view` INFO.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_forwarding_room_names_each_stream_and_takes_a_view() {
    use anvil_core::video::host::VideoCodec;
    use anvil_core::{RoomStream, RoomView, ViewStream, ViewTarget};
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
        Some(SyntheticVideoHost::new(&[VideoCodec::Vp8])),
    )
    .await;
    let alice = std::sync::Arc::new(alice);
    let mut room = Room::bind("127.0.0.10").await;
    let camera = UdpSocket::bind("127.0.0.10:0").await.unwrap();
    let screen = UdpSocket::bind("127.0.0.10:0").await.unwrap();
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
    assert_eq!(
        header(&invite, "Recv-Info").as_deref(),
        Some("fcp-content, fcp-view")
    );
    let offered = video_sections(body(&invite));
    let pt = vp8_pt(&offered[0]);
    let rtp_pt: u8 = pt.parse().unwrap();
    let alice_camera = SocketAddr::from(([127, 0, 0, 1], section_port(&offered[0])));
    assert!(
        !offered[0].contains("a=simulcast"),
        "a host that cannot simulcast offers one encoding"
    );
    let answer = video_answer(
        "127.0.0.10",
        &pt,
        camera.local_addr().unwrap().port(),
        screen.local_addr().unwrap().port(),
        "sendonly",
    );
    room.answer(
        &invite,
        from,
        "200 OK",
        "Recv-Info: fcp-content, fcp-view\r\n",
        "application/sdp",
        &answer,
    )
    .await;
    room.next("ACK ").await;
    expect(&mut events, SOON, |e| match e {
        Event::CallEstablished { call: c, .. } if *c == call => Some(()),
        _ => None,
    })
    .await
    .expect("in the room");
    assert!(alice.room_forwards(call));

    // Bob's stream arrives before the roster names anyone.
    for seq in 0..3 {
        camera
            .send_to(&vp8_packet(rtp_pt, 2222, seq), alice_camera)
            .await
            .unwrap();
    }
    let unnamed = expect(&mut events, SOON, |e| match e {
        Event::RoomStreams { call: c, streams } if *c == call => Some(streams.clone()),
        _ => None,
    })
    .await
    .expect("the first stream is told");
    assert_eq!(unnamed.len(), 1);
    assert_eq!(unnamed[0].member, None);
    let bob_stream = unnamed[0].stream;

    let (subscribe, from) = room.next("SUBSCRIBE ").await;
    room.answer(&subscribe, from, "200 OK", "Expires: 600\r\n", "", "")
        .await;
    let whole = roster(
        "full",
        1,
        &(forwarded_user("p1", "Carol", 1111, Some(3333))
            + &forwarded_user("p2", "Bob", 2222, None)),
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
    expect(&mut events, SOON, |e| match e {
        Event::RoomStreams { call: c, streams }
            if *c == call
                && streams
                    == &vec![RoomStream {
                        stream: bob_stream,
                        member: Some("p2".into()),
                        speaker: false,
                    }] =>
        {
            Some(())
        }
        _ => None,
    })
    .await
    .expect("the roster names Bob's stream");

    // Carol's own stream and the speaker stream showing her, after it.
    for seq in 0..3 {
        for ssrc in [1111, 3333] {
            camera
                .send_to(&vp8_packet(rtp_pt, ssrc, seq), alice_camera)
                .await
                .unwrap();
        }
    }
    let named = expect(&mut events, SOON, |e| match e {
        Event::RoomStreams { call: c, streams } if *c == call && streams.len() == 3 => {
            Some(streams.clone())
        }
        _ => None,
    })
    .await
    .expect("every stream is named");
    let carol: Vec<&RoomStream> = named
        .iter()
        .filter(|s| s.member.as_deref() == Some("p1"))
        .collect();
    assert_eq!(carol.len(), 2, "{named:?}");
    assert_eq!(carol.iter().filter(|s| s.speaker).count(), 1, "{named:?}");

    // A view: Carol large, the speaker small.
    let view = RoomView {
        streams: Some(vec![
            ViewStream {
                target: ViewTarget::Member("p1".into()),
                max_height: Some(720),
                priority: Some(1),
            },
            ViewStream {
                target: ViewTarget::Speaker,
                max_height: Some(180),
                priority: None,
            },
        ]),
        max_streams: Some(4),
    };
    let asked = {
        let alice = std::sync::Arc::clone(&alice);
        let view = view.clone();
        tokio::spawn(async move { alice.set_room_view(call, &view).await })
    };
    let (info, from) = room.next("INFO ").await;
    assert_eq!(header(&info, "Info-Package").as_deref(), Some("fcp-view"));
    assert_eq!(
        header(&info, "Content-Type").as_deref(),
        Some("application/vnd.fcp.view+json")
    );
    assert_eq!(header(&info, "Call-ID"), header(&room.invite, "Call-ID"));
    let sent: serde_json::Value = serde_json::from_str(body(&info)).unwrap();
    assert_eq!(
        sent,
        serde_json::json!({
            "streams": [
                { "participant": "p1", "max_height": 720, "priority": 1 },
                { "speaker": true, "max_height": 180 }
            ],
            "max_streams": 4
        })
    );
    room.answer(&info, from, "200 OK", "", "", "").await;
    asked.await.unwrap().expect("the view is taken");

    // One the room refuses is an error.
    let refused = {
        let alice = std::sync::Arc::clone(&alice);
        tokio::spawn(async move { alice.set_room_view(call, &view).await })
    };
    let (info, from) = room.next("INFO ").await;
    room.answer(&info, from, "400 Bad Request", "", "", "")
        .await;
    let err = refused.await.unwrap().expect_err("refused");
    assert!(err.to_string().contains("400"), "{err}");
}

/// A camera's RTP packets reaching `socket` within `within`: each SSRC
/// with the rid its packets name under extension `ext`.
async fn layers_arriving(
    socket: &UdpSocket,
    ext: u8,
    within: std::time::Duration,
) -> std::collections::BTreeMap<u32, String> {
    let mut out = std::collections::BTreeMap::new();
    let mut buf = [0u8; 2048];
    let end = tokio::time::Instant::now() + within;
    while let Ok(Ok((n, _))) = tokio::time::timeout_at(end, socket.recv_from(&mut buf)).await {
        let p = &buf[..n];
        // RTP (not RTCP: 200–206) with a one-byte header extension
        // (RFC 8285).
        if n < 16 || p[0] >> 6 != 2 || p[0] & 0x10 == 0 || (200..=206).contains(&p[1]) {
            continue;
        }
        let ssrc = u32::from_be_bytes([p[8], p[9], p[10], p[11]]);
        let at = 12 + 4 * usize::from(p[0] & 0x0f);
        if p[at..at + 2] != [0xBE, 0xDE] {
            continue;
        }
        let words = usize::from(u16::from_be_bytes([p[at + 2], p[at + 3]]));
        let mut i = at + 4;
        let stop = (i + 4 * words).min(n);
        while i < stop {
            let (id, len) = (p[i] >> 4, usize::from(p[i] & 0x0f) + 1);
            if p[i] == 0 {
                i += 1;
                continue;
            }
            if id == ext {
                let rid = String::from_utf8_lossy(&p[i + 1..i + 1 + len]).to_string();
                out.insert(ssrc, rid);
            }
            i += 1 + len;
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_room_s_camera_is_sent_as_the_simulcast_the_room_takes() {
    use anvil_core::video::host::VideoCodec;
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
        Some(SyntheticVideoHost::new(&[VideoCodec::Vp8]).with_simulcast(3)),
    )
    .await;
    let mut room = Room::bind("127.0.0.11").await;
    let camera = UdpSocket::bind("127.0.0.11:0").await.unwrap();
    let screen = UdpSocket::bind("127.0.0.11:0").await.unwrap();
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
    for line in [
        "a=rid:h send",
        "a=rid:m send",
        "a=rid:l send",
        "a=simulcast:send h;m;l",
        "urn:ietf:params:rtp-hdrext:sdes:rtp-stream-id",
    ] {
        assert!(offered[0].contains(line), "{line} in {}", offered[0]);
    }
    assert!(
        !offered[1].contains("a=simulcast"),
        "the shared screen is not simulcast"
    );
    // The room takes two of them, under an extension id of its own.
    let pt = vp8_pt(&offered[0]);
    let answer = video_answer(
        "127.0.0.11",
        &pt,
        camera.local_addr().unwrap().port(),
        screen.local_addr().unwrap().port(),
        "sendonly",
    )
    .replacen(
        "a=sendrecv\r\n",
        "a=extmap:9 urn:ietf:params:rtp-hdrext:sdes:rtp-stream-id\r\n\
         a=rid:h recv\r\na=rid:l recv\r\na=simulcast:recv h;l\r\na=sendrecv\r\n",
        1,
    );
    room.answer(
        &invite,
        from,
        "200 OK",
        "Recv-Info: fcp-content, fcp-view\r\n",
        "application/sdp",
        &answer,
    )
    .await;
    room.next("ACK ").await;
    expect(&mut events, SOON, |e| match e {
        Event::CallEstablished { call: c, .. } if *c == call => Some(()),
        _ => None,
    })
    .await
    .expect("in the room");

    let layers = layers_arriving(&camera, 9, std::time::Duration::from_secs(2)).await;
    let mut rids: Vec<&String> = layers.values().collect();
    rids.sort();
    assert_eq!(
        rids,
        ["h", "l"],
        "the two it took, each under an SSRC of its own: {layers:?}"
    );
}
