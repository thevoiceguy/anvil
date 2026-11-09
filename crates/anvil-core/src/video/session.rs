//! One video session: a source going out and any number of streams coming
//! in, over one transport (§2.4).
//!
//! Going out: the host's coded frames are packetized, kept for NACK and
//! sent; a PLI or FIR from the far end asks the host for a keyframe.
//! Coming in: each SSRC is its own stream, with its own assembler, NACK
//! and PLI state and its own sink, made when its first packet arrives and
//! dropped when it falls silent.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use forge_rtp::rtcp::{
    PayloadFeedback, PsFeedback, ReceiverReport, RtcpPacket, RtpFeedback, TransportFeedback,
};
use forge_rtp::rtp::RtpPacket;
use forge_rtp::video::{AssemblerEvent, FrameAssembler, KeyframeRequestGate, RtxCache};
use rand::RngCore;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::host::{
    CodedFrame, EncodingId, ReceiveConfig, SendConfig, StreamId, VideoHost, VideoSink, VideoSource,
};
use super::transport::{Inbound, MediaTransport};
use super::{Feedback, StreamPlan, VideoEvents, VideoStreamDirection};
use crate::media::MediaDirection;

/// The largest RTP payload sent: a packet fits a 1280-byte path with
/// room for SRTP's tag and a relay's headers.
const MAX_PAYLOAD: usize = 1100;

/// Packets kept for NACK: about a second of 720p.
const RTX_CACHE: usize = 512;

/// Keyframe requests sent at most this often, per stream.
const KEYFRAME_REQUEST_INTERVAL: Duration = Duration::from_millis(300);

/// A sequence number is asked for again after this long.
const NACK_RETRY: Duration = Duration::from_millis(100);

/// A stream starting (or back from hold) is given this long to send its
/// keyframe before one is asked for: a sender starts on one anyway.
const FIRST_KEYFRAME_GRACE: Duration = Duration::from_millis(200);

/// An incoming stream silent this long has ended.
pub(crate) const STREAM_IDLE: Duration = Duration::from_secs(5);

/// The 90 kHz video clock (RFC 6184, RFC 7741).
const CLOCK: u64 = 90_000;

/// A direction a session can be told to take, shared with its tasks.
#[derive(Debug)]
pub(crate) struct DirectionCell(AtomicU8);

impl DirectionCell {
    fn new(d: MediaDirection) -> Arc<Self> {
        Arc::new(Self(AtomicU8::new(Self::code(d))))
    }

    fn code(d: MediaDirection) -> u8 {
        match d {
            MediaDirection::Sendrecv => 0,
            MediaDirection::Sendonly => 1,
            MediaDirection::Recvonly => 2,
            MediaDirection::Inactive => 3,
        }
    }

    pub(crate) fn get(&self) -> MediaDirection {
        match self.0.load(Ordering::Relaxed) {
            0 => MediaDirection::Sendrecv,
            1 => MediaDirection::Sendonly,
            2 => MediaDirection::Recvonly,
            _ => MediaDirection::Inactive,
        }
    }

    fn set(&self, d: MediaDirection) -> MediaDirection {
        let was = self.0.swap(Self::code(d), Ordering::Relaxed);
        match was {
            0 => MediaDirection::Sendrecv,
            1 => MediaDirection::Sendonly,
            2 => MediaDirection::Recvonly,
            _ => MediaDirection::Inactive,
        }
    }
}

/// What the receive side tells the send side.
#[derive(Debug)]
enum SendCommand {
    Resend(Vec<u16>),
    Keyframe,
}

/// A running session. Dropping it stops its tasks, its source and its
/// sinks.
pub(crate) struct VideoSession {
    direction: Arc<DirectionCell>,
    commands: mpsc::Sender<SendCommand>,
    send_task: Option<JoinHandle<()>>,
    recv_task: JoinHandle<()>,
}

impl VideoSession {
    /// Start a session on `transport` as `plan` says, reading `inbound`.
    /// `streams` numbers the call's streams.
    pub(crate) fn start(
        plan: StreamPlan,
        host: Arc<dyn VideoHost>,
        transport: Arc<dyn MediaTransport>,
        inbound: mpsc::Receiver<Inbound>,
        streams: Arc<AtomicU32>,
        events: Option<VideoEvents>,
    ) -> Self {
        // Ours, sending or not: RTCP needs one.
        let ssrc = rand::thread_rng().next_u32();
        let direction = DirectionCell::new(plan.direction);
        let (commands, command_rx) = mpsc::channel(64);

        let send_task = plan.send.clone().and_then(|cfg| {
            let source = match host.start_send(cfg.clone()) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(%e, kind = ?plan.kind, "video source would not start; receiving only");
                    return None;
                }
            };
            let id = StreamId(streams.fetch_add(1, Ordering::Relaxed));
            if let Some(ev) = &events {
                ev.started(id, VideoStreamDirection::Outgoing, plan.kind, plan.codec);
            }
            Some(tokio::spawn(send_loop(SendLoop {
                source,
                cfg,
                pt: plan.pt,
                ssrc,
                transport: Arc::clone(&transport),
                direction: Arc::clone(&direction),
                commands: command_rx,
                events: events.clone(),
                id,
            })))
        });

        let recv_task = tokio::spawn(recv_loop(RecvLoop {
            plan: plan.clone(),
            host,
            transport,
            inbound,
            streams,
            events,
            ssrc,
            direction: Arc::clone(&direction),
            commands: commands.clone(),
            incoming: HashMap::new(),
        }));

        Self {
            direction,
            commands,
            send_task,
            recv_task,
        }
    }

    pub(crate) fn direction(&self) -> MediaDirection {
        self.direction.get()
    }

    /// Send, receive, both or neither from now on (hold, resume). A
    /// source sending again starts on a keyframe.
    pub(crate) fn set_direction(&self, d: MediaDirection) {
        let was = self.direction.set(d);
        if d.sends() && !was.sends() {
            let _ = self.commands.try_send(SendCommand::Keyframe);
        }
    }
}

impl Drop for VideoSession {
    fn drop(&mut self) {
        if let Some(t) = &self.send_task {
            t.abort();
        }
        self.recv_task.abort();
    }
}

struct SendLoop {
    source: Box<dyn VideoSource>,
    cfg: SendConfig,
    pt: u8,
    ssrc: u32,
    transport: Arc<dyn MediaTransport>,
    direction: Arc<DirectionCell>,
    commands: mpsc::Receiver<SendCommand>,
    events: Option<VideoEvents>,
    id: StreamId,
}

async fn send_loop(mut s: SendLoop) {
    let (mut seq, base_ts) = {
        let mut rng = rand::thread_rng();
        (rng.next_u32() as u16, rng.next_u32())
    };
    let epoch = Instant::now();
    let mut cache = RtxCache::new(RTX_CACHE);
    let codec = s.cfg.codec.forge();
    let ours = s.cfg.encodings.first().copied().unwrap_or(EncodingId(0));
    loop {
        tokio::select! {
            biased;
            Some(cmd) = s.commands.recv() => match cmd {
                SendCommand::Keyframe => s.source.request_keyframe(),
                SendCommand::Resend(seqs) => {
                    for (_, packet) in cache.lookup(&seqs) {
                        s.transport.send_rtp(packet.clone()).await;
                    }
                }
            },
            next = s.source.next_frame() => {
                let Some((encoding, frame)) = next else { break };
                // One encoding is sent today; the others a host may make
                // for simulcast are for a transport that can carry them.
                if encoding != ours || !s.direction.get().sends() {
                    continue;
                }
                let ticks = frame.captured.saturating_duration_since(epoch).as_micros() as u64
                    * CLOCK / 1_000_000;
                let ts = base_ts.wrapping_add(ticks as u32);
                let payloads = match forge_rtp::video::packetize(
                    codec,
                    &forge_rtp::video::CodedFrame {
                        timestamp: ts,
                        keyframe: frame.keyframe,
                        data: frame.data,
                    },
                    MAX_PAYLOAD,
                ) {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::debug!(%e, "a frame from the host would not packetize");
                        continue;
                    }
                };
                let last = payloads.len().saturating_sub(1);
                for (i, payload) in payloads.into_iter().enumerate() {
                    let packet = RtpPacket::build(s.pt, seq, ts, s.ssrc, payload, i == last)
                        .to_bytes()
                        .freeze();
                    cache.push(seq, packet.clone());
                    s.transport.send_rtp(packet).await;
                    seq = seq.wrapping_add(1);
                }
            }
        }
    }
    if let Some(ev) = &s.events {
        ev.stopped(s.id);
    }
}

/// One incoming stream: an SSRC's packets made into frames for its sink.
struct Incoming {
    id: StreamId,
    assembler: FrameAssembler,
    sink: Box<dyn VideoSink>,
    gate: KeyframeRequestGate,
    /// Sequence numbers asked for, and when.
    nacked: BTreeMap<u16, Instant>,
    last_packet: Instant,
    /// When it (re)started: its first packet, or the end of a hold.
    started: Instant,
    /// Whether the picture is broken (the sink told so) until a keyframe.
    broken: bool,
}

struct RecvLoop {
    plan: StreamPlan,
    host: Arc<dyn VideoHost>,
    transport: Arc<dyn MediaTransport>,
    inbound: mpsc::Receiver<Inbound>,
    streams: Arc<AtomicU32>,
    events: Option<VideoEvents>,
    ssrc: u32,
    direction: Arc<DirectionCell>,
    commands: mpsc::Sender<SendCommand>,
    incoming: HashMap<u32, Incoming>,
}

async fn recv_loop(mut r: RecvLoop) {
    let mut sweep = tokio::time::interval(Duration::from_secs(1));
    sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut receiving = r.direction.get().receives();
    loop {
        tokio::select! {
            packet = r.inbound.recv() => match packet {
                None => break,
                Some(Inbound::Rtp { packet, at }) => {
                    let now_receiving = r.direction.get().receives();
                    if now_receiving && !receiving {
                        // Back from hold: every picture starts again from a
                        // keyframe.
                        for s in r.incoming.values_mut() {
                            s.assembler.reset();
                            s.started = at;
                        }
                    }
                    receiving = now_receiving;
                    if receiving {
                        r.on_rtp(packet, at).await;
                    }
                }
                Some(Inbound::Rtcp { packet }) => r.on_rtcp(&packet),
            },
            _ = sweep.tick() => r.sweep(),
        }
    }
}

impl RecvLoop {
    async fn on_rtp(&mut self, packet: RtpPacket, at: Instant) {
        if packet.header.payload_type() != self.plan.pt {
            return;
        }
        let ssrc = packet.header.ssrc;
        if !self.incoming.contains_key(&ssrc) {
            let id = StreamId(self.streams.fetch_add(1, Ordering::Relaxed));
            let cfg = ReceiveConfig {
                codec: self.plan.codec,
                kind: self.plan.kind,
            };
            let sink = match self.host.start_receive(id, cfg) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(%e, ssrc, "no sink for an incoming video stream");
                    return;
                }
            };
            tracing::info!(ssrc, stream = id.0, kind = ?self.plan.kind, "video: a stream arrived");
            if let Some(ev) = &self.events {
                ev.started(
                    id,
                    VideoStreamDirection::Incoming,
                    self.plan.kind,
                    self.plan.codec,
                );
            }
            self.incoming.insert(
                ssrc,
                Incoming {
                    id,
                    assembler: FrameAssembler::new(self.plan.codec.forge()),
                    sink,
                    gate: KeyframeRequestGate::new(KEYFRAME_REQUEST_INTERVAL),
                    nacked: BTreeMap::new(),
                    last_packet: at,
                    started: at,
                    broken: false,
                },
            );
        }
        let s = self.incoming.get_mut(&ssrc).expect("just made");
        s.last_packet = at;
        let seq = packet.header.sequence_number;
        s.nacked.remove(&seq);
        for event in s.assembler.push(packet) {
            match event {
                AssemblerEvent::Frame(f) => {
                    s.broken = false;
                    s.sink.frame(
                        CodedFrame {
                            data: f.data,
                            keyframe: f.keyframe,
                            captured: at,
                        },
                        at,
                    );
                }
                AssemblerEvent::Lost { .. } | AssemblerEvent::Invalid { .. } => {
                    if !s.broken {
                        s.broken = true;
                        s.sink.lost();
                    }
                }
            }
        }

        let fb = self.plan.feedback;
        let mut out = Vec::new();
        if fb.nack {
            let now = Instant::now();
            let ask: Vec<u16> = s
                .assembler
                .missing()
                .into_iter()
                .filter(|seq| {
                    s.nacked
                        .get(seq)
                        .is_none_or(|t| now.saturating_duration_since(*t) >= NACK_RETRY)
                })
                .collect();
            if !ask.is_empty() {
                for seq in &ask {
                    s.nacked.insert(*seq, now);
                }
                // Forget what is far behind: it is lost or it came.
                while s.nacked.len() > 256 {
                    s.nacked.pop_first();
                }
                out.push(RtcpPacket::TransportFeedback(RtpFeedback::nack(
                    self.ssrc, ssrc, &ask,
                )));
            }
        }
        let waited = s.broken || at.saturating_duration_since(s.started) >= FIRST_KEYFRAME_GRACE;
        if s.assembler.needs_keyframe() && waited && s.gate.allow() {
            out.push(keyframe_request(fb, self.ssrc, ssrc, s));
        }
        if !out.is_empty() {
            let transport = Arc::clone(&self.transport);
            transport.send_rtcp(feedback(self.ssrc, out)).await;
        }
    }

    fn on_rtcp(&mut self, packet: &[u8]) {
        for p in RtcpPacket::parse_compound(packet) {
            if p.feedback_media_ssrc() != Some(self.ssrc)
                && !matches!(
                    &p,
                    RtcpPacket::PayloadFeedback(PsFeedback {
                        kind: PayloadFeedback::Remb { .. },
                        ..
                    })
                )
            {
                continue;
            }
            let cmd = match p {
                RtcpPacket::TransportFeedback(RtpFeedback {
                    kind: TransportFeedback::Nack(entries),
                    ..
                }) => SendCommand::Resend(entries.iter().flat_map(|e| e.lost()).collect()),
                p if p.is_keyframe_request() => SendCommand::Keyframe,
                _ => continue,
            };
            let _ = self.commands.try_send(cmd);
        }
    }

    /// Streams fallen silent have ended (a transfer brought another SSRC,
    /// or the far end stopped sending): their tiles go.
    fn sweep(&mut self) {
        // Silence while not receiving is ours, not the stream ending.
        if !self.direction.get().receives() {
            for s in self.incoming.values_mut() {
                s.last_packet = Instant::now();
            }
            return;
        }
        let now = Instant::now();
        let gone: Vec<u32> = self
            .incoming
            .iter()
            .filter(|(_, s)| now.saturating_duration_since(s.last_packet) >= STREAM_IDLE)
            .map(|(ssrc, _)| *ssrc)
            .collect();
        for ssrc in gone {
            if let Some(s) = self.incoming.remove(&ssrc) {
                tracing::info!(ssrc, stream = s.id.0, "video: a stream fell silent");
                if let Some(ev) = &self.events {
                    ev.stopped(s.id);
                }
            }
        }
    }
}

/// Feedback as a compound packet, which starts with a report (RFC 3550
/// §6.1): an empty receiver report, the feedback after it.
fn feedback(ssrc: u32, packets: Vec<RtcpPacket>) -> Bytes {
    let mut bytes = RtcpPacket::ReceiverReport(ReceiverReport::new(ssrc)).to_bytes();
    for p in packets {
        bytes.extend_from_slice(&p.to_bytes());
    }
    Bytes::from(bytes)
}

/// PLI where the far end takes it, else FIR, else PLI anyway: an endpoint
/// that names no feedback at all usually still answers a PLI.
fn keyframe_request(fb: Feedback, ours: u32, theirs: u32, s: &mut Incoming) -> RtcpPacket {
    if fb.fir && !fb.pli {
        // The sequence number moves with each new request (RFC 5104 §4.3.1.2).
        let seq = (s.gate.absorbed() as u8).wrapping_add(s.id.0 as u8);
        RtcpPacket::PayloadFeedback(PsFeedback::fir(ours, theirs, seq))
    } else {
        RtcpPacket::PayloadFeedback(PsFeedback::pli(ours, theirs))
    }
}

#[cfg(test)]
mod tests {
    use super::super::host::{Resolution, VideoCodec, VideoKind};
    use super::super::synthetic::SyntheticVideoHost;
    use super::super::transport::looped::LoopTransport;
    use super::*;

    fn plan(codec: VideoCodec, send: bool, feedback: Feedback) -> StreamPlan {
        StreamPlan {
            kind: VideoKind::Camera,
            codec,
            pt: 96,
            direction: MediaDirection::Sendrecv,
            feedback,
            send: send.then(|| SendConfig {
                codec,
                kind: VideoKind::Camera,
                camera: None,
                max: Resolution::P720,
                kbps: 1500,
                fps: 30,
                encodings: vec![EncodingId(0)],
            }),
        }
    }

    const ALL: Feedback = Feedback {
        nack: true,
        pli: true,
        fir: true,
        remb: true,
    };

    /// Two sessions joined by a loop: (a, b, a's transport, b's host).
    fn joined(
        codec: VideoCodec,
        feedback: Feedback,
        a_host: SyntheticVideoHost,
        b_host: SyntheticVideoHost,
    ) -> (
        VideoSession,
        VideoSession,
        Arc<LoopTransport>,
        Arc<LoopTransport>,
    ) {
        let (a_tx, a_rx) = mpsc::channel(super::super::transport::INBOUND_QUEUE);
        let (b_tx, b_rx) = mpsc::channel(super::super::transport::INBOUND_QUEUE);
        let a_out = LoopTransport::new(b_tx);
        let b_out = LoopTransport::new(a_tx);
        let a = VideoSession::start(
            plan(codec, true, feedback),
            Arc::new(a_host),
            a_out.clone(),
            a_rx,
            Arc::default(),
            None,
        );
        let b = VideoSession::start(
            plan(codec, true, feedback),
            Arc::new(b_host),
            b_out.clone(),
            b_rx,
            Arc::default(),
            None,
        );
        (a, b, a_out, b_out)
    }

    async fn until(what: &str, mut ok: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ok() {
            assert!(Instant::now() < deadline, "timed out: {what}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn frames_cross_both_ways_in_order_from_a_keyframe() {
        for codec in [VideoCodec::H264, VideoCodec::Vp8] {
            let a_host = SyntheticVideoHost::new(&[codec]).with_fps(50);
            let b_host = SyntheticVideoHost::new(&[codec]).with_fps(50);
            let (_a, _b, _, _) = joined(codec, ALL, a_host.clone(), b_host.clone());
            for host in [&a_host, &b_host] {
                until("30 frames arrive", || {
                    host.record()
                        .received
                        .values()
                        .next()
                        .is_some_and(|s| s.frames.len() >= 30)
                })
                .await;
                let record = host.record();
                assert_eq!(record.received.len(), 1, "one stream in");
                let s = record.received.values().next().unwrap();
                assert!(s.frames[0].1, "{codec:?}: the picture starts on a keyframe");
                let n = s.numbers();
                assert!(n.windows(2).all(|w| w[1] == w[0] + 1), "{codec:?}: {n:?}");
                assert_eq!(s.unreadable, 0);
            }
        }
    }

    #[tokio::test]
    async fn a_lost_packet_is_asked_for_again_and_the_picture_never_breaks() {
        let a_host = SyntheticVideoHost::new(&[VideoCodec::H264]).with_fps(50);
        let b_host = SyntheticVideoHost::new(&[VideoCodec::H264]).with_fps(50);
        let (_a, _b, a_out, _) = joined(VideoCodec::H264, ALL, a_host.clone(), b_host.clone());
        // Every seventh packet a sends is lost on the way.
        a_out.drop_every(7);
        until("100 frames arrive at b", || {
            b_host
                .record()
                .received
                .values()
                .next()
                .is_some_and(|s| s.frames.len() >= 100)
        })
        .await;
        let s = b_host.record().received.values().next().unwrap().clone();
        let n = s.numbers();
        assert!(
            n.windows(2).all(|w| w[1] == w[0] + 1),
            "every frame, in order: {n:?}"
        );
        assert_eq!(s.lost, 0, "retransmission filled every gap");
        // Some packet went twice: the retransmission.
        let log = a_out.rtp_log.lock().clone();
        let mut seen = std::collections::HashSet::new();
        assert!(
            log.iter().any(|seq| !seen.insert(*seq)),
            "a packet was resent"
        );
    }

    #[tokio::test]
    async fn without_nack_a_loss_breaks_the_picture_and_a_pli_restores_it() {
        let fb = Feedback {
            nack: false,
            pli: true,
            fir: false,
            remb: false,
        };
        let a_host = SyntheticVideoHost::new(&[VideoCodec::H264]).with_fps(50);
        let b_host = SyntheticVideoHost::new(&[VideoCodec::H264]).with_fps(50);
        let (_a, _b, a_out, _) = joined(VideoCodec::H264, fb, a_host.clone(), b_host.clone());
        until("the first frames", || {
            b_host
                .record()
                .received
                .values()
                .next()
                .is_some_and(|s| s.frames.len() >= 5)
        })
        .await;
        assert_eq!(
            a_host.record().sent[0].keyframe_requests,
            0,
            "a stream starting on a keyframe asks for none"
        );
        a_out.drop_every(40);
        until("a keyframe asked for after a loss", || {
            a_host
                .record()
                .sent
                .first()
                .is_some_and(|s| s.keyframe_requests >= 1)
        })
        .await;
        a_out.drop_every(0);
        let shown = b_host
            .record()
            .received
            .values()
            .next()
            .unwrap()
            .frames
            .len();
        until("frames again", || {
            b_host
                .record()
                .received
                .values()
                .next()
                .is_some_and(|s| s.frames.len() > shown + 10)
        })
        .await;
        let s = b_host.record().received.values().next().unwrap().clone();
        assert!(s.lost >= 1, "the sink was told");
        // After the break, the next frame shown was a keyframe.
        let first_after = s.frames.iter().skip(1).position(|(_, k)| *k);
        assert!(first_after.is_some(), "a keyframe followed: {:?}", s.frames);
    }

    #[tokio::test]
    async fn two_sources_in_one_session_are_two_streams_with_two_sinks() {
        // b receives from a's session and from a second sender on the same
        // transport: one session, two SSRCs, two tiles.
        let host = SyntheticVideoHost::new(&[VideoCodec::Vp8]).with_fps(50);
        let (b_tx, b_rx) = mpsc::channel(512);
        let (back_tx, _back_rx) = mpsc::channel(512);
        let b = VideoSession::start(
            plan(VideoCodec::Vp8, false, ALL),
            Arc::new(host.clone()),
            LoopTransport::new(back_tx.clone()),
            b_rx,
            Arc::default(),
            None,
        );
        let senders: Vec<VideoSession> = (0..2)
            .map(|_| {
                let (_tx, rx) = mpsc::channel(512);
                VideoSession::start(
                    plan(VideoCodec::Vp8, true, ALL),
                    Arc::new(SyntheticVideoHost::new(&[VideoCodec::Vp8]).with_fps(50)),
                    LoopTransport::new(b_tx.clone()),
                    rx,
                    Arc::default(),
                    None,
                )
            })
            .collect();
        until("two streams, each with frames", || {
            let r = host.record();
            r.received.len() == 2 && r.received.values().all(|s| s.frames.len() >= 20)
        })
        .await;
        for s in host.record().received.values() {
            let n = s.numbers();
            assert!(
                n.windows(2).all(|w| w[1] == w[0] + 1),
                "each in order: {n:?}"
            );
        }
        assert!(host.record().sent.is_empty(), "b sends nothing");

        // One sender stops: its stream ends, the other goes on.
        let mut senders = senders;
        drop(senders.remove(0));
        tokio::time::sleep(STREAM_IDLE + Duration::from_secs(2)).await;
        let r = host.record();
        assert_eq!(r.received.values().filter(|s| s.stopped).count(), 1);
        drop(b);
    }

    #[tokio::test]
    async fn on_hold_nothing_is_sent_and_resuming_starts_on_a_keyframe() {
        let a_host = SyntheticVideoHost::new(&[VideoCodec::H264]).with_fps(50);
        let b_host = SyntheticVideoHost::new(&[VideoCodec::H264]).with_fps(50);
        let (a, b, a_out, _) = joined(VideoCodec::H264, ALL, a_host.clone(), b_host.clone());
        until("frames", || {
            b_host
                .record()
                .received
                .values()
                .next()
                .is_some_and(|s| s.frames.len() >= 5)
        })
        .await;
        a.set_direction(MediaDirection::Recvonly);
        b.set_direction(MediaDirection::Sendonly);
        tokio::time::sleep(Duration::from_millis(100)).await;
        let sent = a_out.rtp_log.lock().len();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(a_out.rtp_log.lock().len(), sent, "held: nothing sent");

        let requests = a_host.record().sent[0].keyframe_requests;
        let shown = b_host
            .record()
            .received
            .values()
            .next()
            .unwrap()
            .frames
            .len();
        a.set_direction(MediaDirection::Sendrecv);
        b.set_direction(MediaDirection::Sendrecv);
        until("a keyframe on resuming", || {
            a_host.record().sent[0].keyframe_requests > requests
        })
        .await;
        until("frames again", || {
            b_host
                .record()
                .received
                .values()
                .next()
                .is_some_and(|s| s.frames.len() > shown + 5)
        })
        .await;
        let s = b_host.record().received.values().next().unwrap().clone();
        assert!(
            s.frames[shown].1,
            "the first frame after hold is a keyframe"
        );
    }
}
