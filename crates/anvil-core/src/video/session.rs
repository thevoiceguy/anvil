//! One video session: a source going out and any number of streams coming
//! in, over one transport (§2.4).
//!
//! Going out: the host's coded frames are packetized, kept for NACK and
//! sent; a PLI or FIR from the far end asks the host for a keyframe, and
//! its REMB sets what the encoder aims for.
//! Coming in: each SSRC is its own stream, with its own assembler, NACK
//! and PLI state, its own playout and its own sink, made when its first
//! packet arrives and dropped when it falls silent. What arrives feeds
//! the session's bandwidth estimate, sent back as REMB.
//! Every second: a sender or receiver report with a block per incoming
//! stream, our audio's CNAME (so the far end pairs our video with it for
//! lip sync), the estimate; and each stream's stats to the app.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use forge_rtp::rtcp::{
    ntp, PayloadFeedback, PsFeedback, ReceiverReport, ReceptionReportBlock, RtcpPacket,
    RtpFeedback, SdesItem, SenderReport, SourceDescription, TransportFeedback,
};
use forge_rtp::rtp::RtpPacket;
use forge_rtp::video::{AssemblerEvent, FrameAssembler, KeyframeRequestGate, RtxCache};
use forge_rtp::{RttTracker, SourceStats};
use parking_lot::Mutex;
use rand::RngCore;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::bwe::{BandwidthEstimator, RembEstimator, MIN_BPS};
use super::host::{
    CodedFrame, EncodingId, ReceiveConfig, Resolution, SendConfig, StreamId, VideoHost, VideoSink,
    VideoSource,
};
use super::playout::{self, AudioClock, Playout};
use super::transport::{Inbound, MediaTransport};
use super::{Feedback, StreamPlan, VideoEvents, VideoStreamDirection, VideoStreamStats};
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

/// A packet asked for this long ago and still missing is given up: the
/// picture waits for a keyframe instead (the frames after a loss cannot
/// be decoded without one). Room for a few retries on a long path; both
/// this and the retry stretch with the round trip, up to the most a
/// picture is let freeze for one packet.
const NACK_GIVE_UP: Duration = Duration::from_millis(400);
const NACK_GIVE_UP_MAX: Duration = Duration::from_millis(1500);

/// A packet is not sent again sooner than this after its last resend (or
/// a round trip, when longer).
const RESEND_GAP: Duration = Duration::from_millis(100);
/// Resends are remembered this long.
const RESEND_FORGET: Duration = Duration::from_secs(2);
/// Resends take at most this share of the target, so a lossy path is not
/// filled with them (the loss is often the path being full).
const RESEND_SHARE: f64 = 0.5;

/// What resends may still spend: [`RESEND_SHARE`] of the target a
/// second, up to a second's worth.
struct ResendBudget {
    bytes: f64,
    at: Option<Instant>,
    cap: f64,
}

impl ResendBudget {
    fn new() -> Self {
        Self {
            bytes: 0.0,
            at: None,
            cap: 0.0,
        }
    }

    fn refill(&mut self, now: Instant, target_kbps: u32) {
        let per_second = f64::from(target_kbps) * 125.0 * RESEND_SHARE;
        self.cap = per_second;
        let secs = self
            .at
            .map_or(1.0, |t| now.saturating_duration_since(t).as_secs_f64());
        self.at = Some(now);
        self.bytes = (self.bytes + per_second * secs).min(self.cap);
    }

    fn take(&mut self, bytes: usize) -> bool {
        if self.bytes < bytes as f64 {
            return false;
        }
        self.bytes -= bytes as f64;
        true
    }
}

/// A sender makes at most one keyframe asked for in this long; requests
/// within it are served by the next one after it. Each is several frames'
/// worth of bytes, and on a congested path a keyframe for every request
/// keeps the path congested.
const KEYFRAME_MIN_INTERVAL: Duration = Duration::from_millis(500);

/// Packets held out of order while a gap is asked for: more than the
/// sender's cache keeps, so the gap is given up by time, not by count.
/// (forge-rtp's default of 16 is ~100 ms at 1.5 Mb/s, under one round
/// trip on many paths.)
const REORDER_WINDOW: usize = 1024;
/// A gap wider than this is a burst no retransmission will mend in
/// time, and asking for it all would add a burst of resends to whatever
/// caused it: a keyframe is asked for instead.
const MAX_NACK_LIST: usize = 64;
/// Each missing packet is asked for at most this many times.
const MAX_NACK_TRIES: u8 = 3;

/// Without NACK, how long a gap waits for a reordered packet.
const REORDER_WAIT: Duration = Duration::from_millis(50);
const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;

/// A stream starting (or back from hold) is given this long to send its
/// keyframe before one is asked for: a sender starts on one anyway.
const FIRST_KEYFRAME_GRACE: Duration = Duration::from_millis(200);

/// An incoming stream silent this long has ended.
pub(crate) const STREAM_IDLE: Duration = Duration::from_secs(5);

/// The 90 kHz video clock (RFC 6184, RFC 7741).
const CLOCK: u64 = 90_000;

/// How often the estimate is looked at; a fall of more than
/// [`REMB_DROP`] is sent at once rather than with the next report.
const ESTIMATE_TICK: Duration = Duration::from_millis(200);
const REMB_DROP: f64 = 0.97;

/// Reports and stats every this many estimate ticks: a second. Video's
/// RTCP is AVPF's (RFC 4585 §3.5), so it need not wait RFC 3550's five.
const TICKS_PER_REPORT: u32 = 5;

/// The encoder's target moves only on a change larger than this share.
const TARGET_STEP: f64 = 0.05;

/// SDES item type CNAME (RFC 3550 §6.5.1).
const SDES_CNAME: u8 = 1;

/// A direction a session can be told to take, shared with its tasks,
/// and whether incoming tiles stay while it does not receive (a hold
/// freezes them; the far end's camera turned off removes them).
#[derive(Debug)]
pub(crate) struct DirectionCell(AtomicU8, std::sync::atomic::AtomicBool);

impl DirectionCell {
    fn new(d: MediaDirection) -> Arc<Self> {
        Arc::new(Self(
            AtomicU8::new(Self::code(d)),
            std::sync::atomic::AtomicBool::new(true),
        ))
    }

    fn keeps_tiles(&self) -> bool {
        self.1.load(Ordering::Relaxed)
    }

    fn code(d: MediaDirection) -> u8 {
        match d {
            MediaDirection::Sendrecv => 0,
            MediaDirection::Sendonly => 1,
            MediaDirection::Recvonly => 2,
            MediaDirection::Inactive => 3,
        }
    }

    fn decode(code: u8) -> MediaDirection {
        match code {
            0 => MediaDirection::Sendrecv,
            1 => MediaDirection::Sendonly,
            2 => MediaDirection::Recvonly,
            _ => MediaDirection::Inactive,
        }
    }

    pub(crate) fn get(&self) -> MediaDirection {
        Self::decode(self.0.load(Ordering::Relaxed))
    }

    fn set(&self, d: MediaDirection) -> MediaDirection {
        Self::decode(self.0.swap(Self::code(d), Ordering::Relaxed))
    }
}

/// What the receive side tells the send side.
#[derive(Debug)]
enum SendCommand {
    Resend(Vec<u16>),
    Keyframe,
    /// The far end's REMB, in bits a second.
    Target(u64),
    /// The direction changed: open or close the camera.
    Wake,
}

/// What a session's streams share with it besides the transport.
#[derive(Clone, Default)]
pub(crate) struct SessionLinks {
    /// Numbers the call's streams.
    pub streams: Arc<AtomicU32>,
    pub events: Option<VideoEvents>,
    /// The call's audio, for lip sync and for the CNAME our reports carry.
    pub audio: Option<Arc<dyn AudioClock>>,
}

/// The send side as the reports and stats see it.
struct Outgoing {
    id: Option<StreamId>,
    ssrc: u32,
    /// Our RTP clock: `base_ts` at `epoch`, so a report's timestamp is
    /// the one a frame captured at that moment carries.
    epoch: Instant,
    base_ts: u32,
    packets: u32,
    octets: u32,
    sent_at_last_report: u32,
    rtt: RttTracker,
    /// The far end's last report on ours: the share it lost, in percent.
    remote_loss_pct: Option<f32>,
    target_kbps: u32,
    /// Since the last stats.
    frames: u32,
    bytes: u64,
    nacks: u32,
    keyframe_requests: u32,
}

impl Outgoing {
    fn rtp_at(&self, now: Instant) -> u32 {
        let ticks =
            now.saturating_duration_since(self.epoch).as_micros() as u64 * CLOCK / 1_000_000;
        self.base_ts.wrapping_add(ticks as u32)
    }
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
    pub(crate) fn start(
        plan: StreamPlan,
        host: Arc<dyn VideoHost>,
        transport: Arc<dyn MediaTransport>,
        inbound: mpsc::Receiver<Inbound>,
        links: SessionLinks,
    ) -> Self {
        // Ours, sending or not: RTCP needs one.
        let (ssrc, seq, base_ts) = {
            let mut rng = rand::thread_rng();
            (rng.next_u32(), rng.next_u32() as u16, rng.next_u32())
        };
        let direction = DirectionCell::new(plan.direction);
        let (commands, command_rx) = mpsc::channel(64);
        let outgoing = Arc::new(Mutex::new(Outgoing {
            id: None,
            ssrc,
            epoch: Instant::now(),
            base_ts,
            packets: 0,
            octets: 0,
            sent_at_last_report: 0,
            rtt: RttTracker::new(Duration::from_secs(30)),
            remote_loss_pct: None,
            target_kbps: plan.send.as_ref().map_or(0, |c| c.kbps),
            frames: 0,
            bytes: 0,
            nacks: 0,
            keyframe_requests: 0,
        }));

        let send_task = plan.send.clone().map(|cfg| {
            tokio::spawn(send_loop(SendLoop {
                host: Arc::clone(&host),
                cfg,
                kind: plan.kind,
                pt: plan.pt,
                seq,
                transport: Arc::clone(&transport),
                direction: Arc::clone(&direction),
                commands: command_rx,
                links: links.clone(),
                outgoing: Arc::clone(&outgoing),
            }))
        });

        let recv_task = tokio::spawn(recv_loop(RecvLoop {
            plan: plan.clone(),
            host,
            transport,
            inbound,
            links,
            ssrc,
            direction: Arc::clone(&direction),
            commands: commands.clone(),
            incoming: HashMap::new(),
            outgoing,
            estimator: Box::new(RembEstimator::default()),
            estimate: None,
            last_remb: None,
            rtt: None,
            stats_at: Instant::now(),
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

    /// Send, receive, both or neither from now on (hold, resume, the
    /// camera on or off). The camera is open only while sending; opened
    /// again, it starts on a keyframe.
    #[cfg(test)]
    pub(crate) fn set_direction(&self, d: MediaDirection) {
        self.set_direction_keeping(d, true);
    }

    /// As [`set_direction`](Self::set_direction); not receiving with
    /// `keep_tiles` false (the far end stopped sending, not a hold), the
    /// incoming streams end.
    pub(crate) fn set_direction_keeping(&self, d: MediaDirection, keep_tiles: bool) {
        self.direction.1.store(keep_tiles, Ordering::Relaxed);
        if self.direction.set(d) != d {
            let _ = self.commands.try_send(SendCommand::Wake);
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

/// A target this far above what the encoder sends means the sender is
/// application-limited (a still picture codes small).
const APP_LIMITED: f64 = 1.3;

/// The rate the ladder steps by. A far end's estimate tracks what it
/// receives (at most 1.5× it, as GCC caps it), so while the encoder sends
/// well under the target a falling estimate says only that the picture is
/// cheap, not that the network is short: the picture keeps its size, and
/// may still grow. Once the target falls to near what is sent, the
/// network is the limit and the ladder follows it down.
pub(crate) fn ladder_kbps(target: u32, sent_kbps: Option<u32>, previous: u32) -> u32 {
    match sent_kbps {
        Some(sent) if f64::from(target) >= APP_LIMITED * f64::from(sent) => previous.max(target),
        _ => target,
    }
}

/// The picture an encoder aims for at `kbps`: 720p from a megabit, 360p
/// from 450 kb/s, 180p below; never above `max`. Under 150 kb/s, at most
/// 15 frames a second.
pub(crate) fn ladder(kbps: u32, max: Resolution, fps: u32) -> (Resolution, u32) {
    let rung = if kbps >= 1000 {
        Resolution::P720
    } else if kbps >= 450 {
        Resolution {
            width: 640,
            height: 360,
        }
    } else {
        Resolution {
            width: 320,
            height: 180,
        }
    };
    let res = if rung.width * rung.height > max.width * max.height {
        max
    } else {
        rung
    };
    (res, if kbps < 150 { fps.min(15) } else { fps })
}

struct SendLoop {
    host: Arc<dyn VideoHost>,
    cfg: SendConfig,
    kind: super::host::VideoKind,
    pt: u8,
    seq: u16,
    transport: Arc<dyn MediaTransport>,
    direction: Arc<DirectionCell>,
    commands: mpsc::Receiver<SendCommand>,
    links: SessionLinks,
    outgoing: Arc<Mutex<Outgoing>>,
}

/// The next frame of `source`, or never when there is none.
async fn next_frame(source: &mut Option<Box<dyn VideoSource>>) -> Option<(EncodingId, CodedFrame)> {
    match source {
        Some(s) => s.next_frame().await,
        None => std::future::pending().await,
    }
}

async fn send_loop(mut s: SendLoop) {
    let mut seq = s.seq;
    let mut cache = RtxCache::new(RTX_CACHE);
    let codec = s.cfg.codec.forge();
    let ours = s.cfg.encodings.first().copied().unwrap_or(EncodingId(0));
    let mut target = s.cfg.kbps;
    let mut target_changed = false;
    // The rate the picture's size was chosen for, and what the encoder
    // actually sent over the last second.
    let mut rung_kbps = s.cfg.kbps;
    let mut sent_kbps: Option<u32> = None;
    let mut sent_window = (Instant::now(), 0u64);
    let mut last_forced: Option<Instant> = None;
    let mut resent: HashMap<u16, Instant> = HashMap::new();
    let mut budget = ResendBudget::new();
    let mut keyframe_due = false;
    let ssrc = s.outgoing.lock().ssrc;
    // The camera is open only while the session sends: not before the
    // user agreed to send (an upgrade taken receive-only), not on hold.
    let mut source: Option<Box<dyn VideoSource>> = None;
    // It would not open: not again until the session stops sending.
    let mut refused = false;
    let mut id: Option<StreamId> = None;
    loop {
        let sends = s.direction.get().sends();
        if !sends {
            if source.take().is_some() {
                tracing::debug!(kind = ?s.kind, "video: the camera closes (not sending)");
            }
            refused = false;
        } else if source.is_none() && !refused {
            match s.host.start_send(s.cfg.clone()) {
                Ok(src) => {
                    // A new source starts on a keyframe, as encoders do.
                    if target != s.cfg.kbps {
                        let (max, fps) = ladder(rung_kbps, s.cfg.max, s.cfg.fps);
                        src.set_target(target, max, fps);
                    }
                    source = Some(src);
                    keyframe_due = false;
                    target_changed = false;
                    if id.is_none() {
                        let new = StreamId(s.links.streams.fetch_add(1, Ordering::Relaxed));
                        id = Some(new);
                        s.outgoing.lock().id = Some(new);
                        if let Some(ev) = &s.links.events {
                            ev.started(new, VideoStreamDirection::Outgoing, s.kind, s.cfg.codec);
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(%e, kind = ?s.kind, "video source would not start; receiving only");
                    refused = true;
                }
            }
        }
        if let Some(src) = &source {
            // A keyframe asked for, once the last one made is old enough.
            if keyframe_due && last_forced.is_none_or(|t| t.elapsed() >= KEYFRAME_MIN_INTERVAL) {
                keyframe_due = false;
                last_forced = Some(Instant::now());
                src.request_keyframe();
            }
            if target_changed {
                target_changed = false;
                rung_kbps = ladder_kbps(target, sent_kbps, rung_kbps);
                let (max, fps) = ladder(rung_kbps, s.cfg.max, s.cfg.fps);
                tracing::debug!(
                    kbps = target,
                    sent_kbps,
                    ?max,
                    fps,
                    "video: the far end's estimate moved the target"
                );
                src.set_target(target, max, fps);
            }
        }
        tokio::select! {
            biased;
            cmd = s.commands.recv() => match cmd {
                None => break,
                Some(SendCommand::Wake) => {}
                Some(SendCommand::Keyframe) => {
                    s.outgoing.lock().keyframe_requests += 1;
                    keyframe_due = true;
                }
                Some(SendCommand::Resend(seqs)) => {
                    let rtt = {
                        let mut o = s.outgoing.lock();
                        o.nacks += seqs.len() as u32;
                        o.rtt.mean_ms()
                    };
                    let now = Instant::now();
                    // Not again within a round trip: the last one may be
                    // on its way still.
                    let gap = RESEND_GAP.max(Duration::from_secs_f32(rtt.unwrap_or(0.0).max(0.0) / 1000.0));
                    resent.retain(|_, t| now.saturating_duration_since(*t) < RESEND_FORGET);
                    budget.refill(now, target);
                    for (seq, packet) in cache.lookup(&seqs) {
                        if resent.get(&seq).is_some_and(|t| now.saturating_duration_since(*t) < gap) {
                            continue;
                        }
                        if !budget.take(packet.len()) {
                            break;
                        }
                        resent.insert(seq, now);
                        s.transport.send_rtp(packet.clone()).await;
                    }
                }
                Some(SendCommand::Target(bps)) => {
                    // What the far end allows, within what we were set up
                    // to send.
                    let kbps = ((bps / 1000) as u32).clamp((MIN_BPS / 1000) as u32, s.cfg.kbps.max(1));
                    if f64::from(kbps.abs_diff(target)) > f64::from(target) * TARGET_STEP
                        || (kbps == s.cfg.kbps && target != kbps)
                    {
                        target = kbps;
                        target_changed = true;
                        s.outgoing.lock().target_kbps = kbps;
                    }
                }
            },
            next = next_frame(&mut source) => {
                let Some((encoding, frame)) = next else {
                    tracing::info!(kind = ?s.kind, "video: the source ended");
                    break;
                };
                // One encoding is sent today; the others a host may make
                // for simulcast are for a transport that can carry them.
                if encoding != ours || !s.direction.get().sends() {
                    continue;
                }
                let ts = s.outgoing.lock().rtp_at(frame.captured);
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
                let mut sent = (0u32, 0usize, 0usize);
                for (i, payload) in payloads.into_iter().enumerate() {
                    let len = payload.len();
                    let packet = RtpPacket::build(s.pt, seq, ts, ssrc, payload, i == last)
                        .to_bytes()
                        .freeze();
                    sent = (sent.0 + 1, sent.1 + len, sent.2 + packet.len());
                    cache.push(seq, packet.clone());
                    s.transport.send_rtp(packet).await;
                    seq = seq.wrapping_add(1);
                }
                sent_window.1 += sent.2 as u64;
                let window = sent_window.0.elapsed();
                if window >= Duration::from_secs(1) {
                    sent_kbps = Some((sent_window.1 * 8 / window.as_millis().max(1) as u64) as u32);
                    sent_window = (Instant::now(), 0);
                }
                let mut o = s.outgoing.lock();
                o.packets = o.packets.wrapping_add(sent.0);
                o.octets = o.octets.wrapping_add(sent.1 as u32);
                o.bytes += sent.2 as u64;
                o.frames += 1;
            }
        }
    }
    drop(source);
    if let (Some(ev), Some(id)) = (&s.links.events, id) {
        ev.stopped(id);
    }
}

/// One incoming stream: an SSRC's packets made into frames for its sink.
struct Incoming {
    id: StreamId,
    assembler: FrameAssembler,
    sink: Box<dyn VideoSink>,
    gate: KeyframeRequestGate,
    /// Sequence numbers missing: when first seen, when last asked for,
    /// and how often.
    nacked: BTreeMap<u16, (Instant, Instant, u8)>,
    last_packet: Instant,
    /// When it (re)started: its first packet, or the end of a hold.
    started: Instant,
    /// Whether the picture is broken (the sink told so) until a keyframe.
    broken: bool,
    /// A keyframe has arrived since the stream (re)started: until one
    /// does, nothing reaches the sink (a decoder makes garbage of a
    /// picture without its keyframe, as when a relay drops a stream's
    /// first packets before it has latched).
    keyed: bool,
    stats: SourceStats,
    playout: Playout,
    /// The highest sequence number, for loss on first arrival.
    highest: Option<u16>,
    /// Since the last stats.
    frames: u32,
    bytes: u64,
    expected: u32,
    lost: u32,
    nacks: u32,
    keyframe_requests: u32,
}

impl Incoming {
    /// Counts loss as the network made it: a jump forward leaves a gap
    /// that a retransmission later fills does not close.
    fn count(&mut self, seq: u16) {
        match self.highest {
            None => {
                self.highest = Some(seq);
                self.expected += 1;
            }
            Some(h) => {
                let ahead = seq.wrapping_sub(h);
                if ahead != 0 && ahead < 0x8000 {
                    self.highest = Some(seq);
                    let ahead = if ahead > 1000 { 1 } else { u32::from(ahead) };
                    self.expected += ahead;
                    self.lost += ahead - 1;
                }
            }
        }
    }
}

struct RecvLoop {
    plan: StreamPlan,
    host: Arc<dyn VideoHost>,
    transport: Arc<dyn MediaTransport>,
    inbound: mpsc::Receiver<Inbound>,
    links: SessionLinks,
    ssrc: u32,
    direction: Arc<DirectionCell>,
    commands: mpsc::Sender<SendCommand>,
    incoming: HashMap<u32, Incoming>,
    outgoing: Arc<Mutex<Outgoing>>,
    estimator: Box<dyn BandwidthEstimator>,
    /// The latest estimate, in bits a second.
    estimate: Option<u64>,
    /// The last REMB sent: its value and when.
    last_remb: Option<(u64, Instant)>,
    /// The round trip, from our reports: what a retransmission takes.
    rtt: Option<Duration>,
    stats_at: Instant,
}

async fn recv_loop(mut r: RecvLoop) {
    let mut tick = tokio::time::interval(ESTIMATE_TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut ticks = 0u32;
    let mut receiving = r.direction.get().receives();
    loop {
        tokio::select! {
            packet = r.inbound.recv() => match packet {
                None => break,
                Some(Inbound::Rtp { packet, at }) => {
                    let now_receiving = r.direction.get().receives();
                    if now_receiving && !receiving {
                        // Back from hold: every picture starts again from a
                        // keyframe, and its timing and the estimate's
                        // history start again too.
                        for (ssrc, s) in r.incoming.iter_mut() {
                            s.assembler.reset();
                            s.playout.reset();
                            s.highest = None;
                            s.started = at;
                            s.keyed = false;
                            r.estimator.forget(*ssrc);
                        }
                    }
                    receiving = now_receiving;
                    if receiving {
                        r.on_rtp(packet, at).await;
                    }
                }
                Some(Inbound::Rtcp { packet }) => r.on_rtcp(&packet),
            },
            _ = tick.tick() => {
                ticks += 1;
                let report = ticks.is_multiple_of(TICKS_PER_REPORT);
                r.on_tick(report).await;
            }
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
            let id = StreamId(self.links.streams.fetch_add(1, Ordering::Relaxed));
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
            if let Some(ev) = &self.links.events {
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
                    assembler: FrameAssembler::with_limits(
                        self.plan.codec.forge(),
                        REORDER_WINDOW,
                        MAX_FRAME_BYTES,
                    ),
                    sink,
                    gate: KeyframeRequestGate::new(KEYFRAME_REQUEST_INTERVAL),
                    nacked: BTreeMap::new(),
                    last_packet: at,
                    started: at,
                    broken: false,
                    keyed: false,
                    stats: SourceStats::new(ssrc, CLOCK as u32),
                    playout: Playout::new(CLOCK as u32),
                    highest: None,
                    frames: 0,
                    bytes: 0,
                    expected: 0,
                    lost: 0,
                    nacks: 0,
                    keyframe_requests: 0,
                },
            );
        }
        let s = self.incoming.get_mut(&ssrc).expect("just made");
        s.last_packet = at;
        let seq = packet.header.sequence_number;
        let ts = packet.header.timestamp;
        // The payload and a 12-byte header, as the network carried it.
        let size = packet.payload.len() + 12;
        s.nacked.remove(&seq);
        s.count(seq);
        s.stats.on_packet(seq, ts, at);
        s.bytes += size as u64;
        self.estimator.on_packet(ssrc, seq, ts, size, at);

        let audio = self.links.audio.as_deref();
        let (now, now_ntp) = (Instant::now(), playout::ntp_now_us());
        for event in s.assembler.push(packet) {
            match event {
                AssemblerEvent::Frame(f) => {
                    if !(f.keyframe || s.keyed) {
                        continue;
                    }
                    s.keyed = true;
                    s.broken = false;
                    s.frames += 1;
                    s.playout.set_jitter(s.stats.jitter_ms());
                    let show = s.playout.place(f.timestamp, at, audio, now, now_ntp);
                    s.sink.frame(
                        CodedFrame {
                            data: f.data,
                            keyframe: f.keyframe,
                            captured: at,
                        },
                        show,
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
        let missing = s.assembler.missing();
        // Each gap from when it was first seen; without NACK only
        // reordering can fill it, which takes moments.
        let rtt = self.rtt.unwrap_or_default();
        let (retry, give_up) = if fb.nack {
            (
                NACK_RETRY.max(rtt.mul_f32(1.25)),
                NACK_GIVE_UP.max(rtt * 3 + NACK_RETRY).min(NACK_GIVE_UP_MAX),
            )
        } else {
            (NACK_RETRY, REORDER_WAIT)
        };
        let given_up = missing.iter().any(|seq| {
            s.nacked
                .get(seq)
                .is_some_and(|(first, _, _)| now.saturating_duration_since(*first) >= give_up)
        });
        if given_up || missing.len() > MAX_NACK_LIST {
            // Let it go, and the picture with it until a keyframe.
            s.assembler.reset();
            s.nacked.clear();
            if !s.broken {
                s.broken = true;
                s.sink.lost();
            }
        } else {
            let ask: Vec<u16> = missing
                .into_iter()
                .filter(|seq| {
                    s.nacked.get(seq).is_none_or(|(_, t, tries)| {
                        *tries < MAX_NACK_TRIES && now.saturating_duration_since(*t) >= retry
                    })
                })
                .collect();
            if !ask.is_empty() {
                for seq in &ask {
                    let (first, tries) = s.nacked.get(seq).map_or((now, 0), |(f, _, n)| (*f, *n));
                    s.nacked.insert(*seq, (first, now, tries + 1));
                }
                // Forget what is far behind: it is lost or it came.
                while s.nacked.len() > 256 {
                    s.nacked.pop_first();
                }
                if fb.nack {
                    s.nacks += ask.len() as u32;
                    out.push(RtcpPacket::TransportFeedback(RtpFeedback::nack(
                        self.ssrc, ssrc, &ask,
                    )));
                }
            }
        }
        let waited = s.broken || at.saturating_duration_since(s.started) >= FIRST_KEYFRAME_GRACE;
        if (s.assembler.needs_keyframe() || !s.keyed) && waited && s.gate.allow() {
            s.keyframe_requests += 1;
            out.push(keyframe_request(fb, self.ssrc, ssrc, s));
        }
        if !out.is_empty() {
            let transport = Arc::clone(&self.transport);
            transport.send_rtcp(feedback(self.ssrc, out)).await;
        }
    }

    fn on_rtcp(&mut self, packet: &[u8]) {
        let now = Instant::now();
        for p in RtcpPacket::parse_compound(packet) {
            let blocks = match p {
                RtcpPacket::SenderReport(sr) => {
                    if let Some(s) = self.incoming.get_mut(&sr.ssrc) {
                        s.stats
                            .on_sender_report(sr.ntp_timestamp_msw, sr.ntp_timestamp_lsw, now);
                        s.playout.on_sender_report(
                            playout::ntp_us(
                                (u64::from(sr.ntp_timestamp_msw) << 32)
                                    | u64::from(sr.ntp_timestamp_lsw),
                            ),
                            sr.rtp_timestamp,
                        );
                    }
                    sr.report_blocks
                }
                RtcpPacket::ReceiverReport(rr) => rr.report_blocks,
                RtcpPacket::PayloadFeedback(PsFeedback {
                    kind: PayloadFeedback::Remb { bitrate_bps, .. },
                    ..
                }) => {
                    // A REMB is for whatever we send in this section,
                    // whichever SSRCs a relay lists.
                    let _ = self.commands.try_send(SendCommand::Target(bitrate_bps));
                    continue;
                }
                p if p.feedback_media_ssrc() != Some(self.ssrc) => continue,
                RtcpPacket::TransportFeedback(RtpFeedback {
                    kind: TransportFeedback::Nack(entries),
                    ..
                }) => {
                    let lost = entries.iter().flat_map(|e| e.lost()).collect();
                    let _ = self.commands.try_send(SendCommand::Resend(lost));
                    continue;
                }
                p if p.is_keyframe_request() => {
                    let _ = self.commands.try_send(SendCommand::Keyframe);
                    continue;
                }
                _ => continue,
            };
            // The far end's report on ours: by our SSRC, else the only one
            // (a relay that renumbers still reports on what we sent).
            let ours = blocks
                .iter()
                .find(|b| b.ssrc == self.ssrc)
                .or(match blocks.as_slice() {
                    [only] if self.incoming.is_empty() => Some(only),
                    _ => None,
                });
            if let Some(b) = ours {
                let mut o = self.outgoing.lock();
                o.remote_loss_pct = Some(f32::from(b.fraction_lost) * 100.0 / 256.0);
                o.rtt
                    .observe_incoming_rr(b.last_sr, b.delay_since_last_sr, ntp::now());
            }
        }
    }

    /// Every [`ESTIMATE_TICK`]: the estimate, sent at once if it fell;
    /// every second (`report`), the report, the stats and the sweep.
    async fn on_tick(&mut self, report: bool) {
        let now = Instant::now();
        let receiving = self.direction.get().receives();
        if receiving && !self.incoming.is_empty() {
            self.estimate = self.estimator.estimate(now).or(self.estimate);
        }
        let remb = self
            .estimate
            .filter(|_| self.plan.feedback.remb && receiving && !self.incoming.is_empty());
        let fell = remb.is_some_and(|e| {
            self.last_remb
                .is_some_and(|(sent, _)| (e as f64) < sent as f64 * REMB_DROP)
        });
        if report {
            self.rtt = self
                .outgoing
                .lock()
                .rtt
                .mean_ms()
                .map(|ms| Duration::from_secs_f32(ms.max(0.0) / 1000.0));
            let packet = self.report(remb, now);
            self.transport.send_rtcp(packet).await;
            self.stats(now);
            self.sweep();
        } else if fell {
            let e = remb.expect("fell");
            self.last_remb = Some((e, now));
            let p = RtcpPacket::PayloadFeedback(PsFeedback::remb(
                self.ssrc,
                e,
                self.incoming.keys().copied().collect(),
            ));
            self.transport.send_rtcp(feedback(self.ssrc, vec![p])).await;
        }
    }

    /// The compound report: SR when we sent since the last, else RR; a
    /// block on each incoming stream; our audio's CNAME; the estimate.
    fn report(&mut self, remb: Option<u64>, now: Instant) -> Bytes {
        let blocks: Vec<ReceptionReportBlock> = self
            .incoming
            .values_mut()
            .filter(|s| s.stats.is_started())
            .map(|s| s.stats.report_block(now))
            .collect();
        let mut packet = {
            let mut o = self.outgoing.lock();
            let sending = o.packets != o.sent_at_last_report;
            o.sent_at_last_report = o.packets;
            if sending {
                let mut sr =
                    SenderReport::with_current_time(self.ssrc, o.rtp_at(now), o.packets, o.octets);
                o.rtt.record_outgoing_sr(
                    (u64::from(sr.ntp_timestamp_msw) << 32) | u64::from(sr.ntp_timestamp_lsw),
                );
                for b in blocks {
                    sr.add_report_block(b);
                }
                RtcpPacket::SenderReport(sr).to_bytes()
            } else {
                let mut rr = ReceiverReport::new(self.ssrc);
                for b in blocks {
                    rr.add_report_block(b);
                }
                RtcpPacket::ReceiverReport(rr).to_bytes()
            }
        };
        let cname = self
            .links
            .audio
            .as_ref()
            .map_or_else(|| format!("{:08x}@anvil", self.ssrc), |a| a.cname());
        let mut sdes = SourceDescription::new();
        sdes.add_chunk(self.ssrc, vec![SdesItem::new(SDES_CNAME, cname)]);
        packet.extend_from_slice(&RtcpPacket::SourceDescription(sdes).to_bytes());
        if let Some(e) = remb {
            self.last_remb = Some((e, now));
            packet.extend_from_slice(
                &RtcpPacket::PayloadFeedback(PsFeedback::remb(
                    self.ssrc,
                    e,
                    self.incoming.keys().copied().collect(),
                ))
                .to_bytes(),
            );
        }
        Bytes::from(packet)
    }

    /// Each stream's last second, to the app.
    fn stats(&mut self, now: Instant) {
        let secs = now
            .saturating_duration_since(self.stats_at)
            .as_secs_f32()
            .max(0.001);
        self.stats_at = now;
        let Some(ev) = self.links.events.clone() else {
            // Nobody to tell; the counts still start again.
            for s in self.incoming.values_mut() {
                (
                    s.frames,
                    s.bytes,
                    s.expected,
                    s.lost,
                    s.nacks,
                    s.keyframe_requests,
                ) = (0, 0, 0, 0, 0, 0);
            }
            let mut o = self.outgoing.lock();
            (o.frames, o.bytes, o.nacks, o.keyframe_requests) = (0, 0, 0, 0);
            return;
        };
        let (kind, codec) = (self.plan.kind, self.plan.codec);
        let sending = self.direction.get().sends();
        {
            let mut o = self.outgoing.lock();
            if let (Some(id), true) = (o.id, sending) {
                let stats = VideoStreamStats {
                    direction: VideoStreamDirection::Outgoing,
                    kind,
                    codec,
                    kbps: o.bytes as f32 * 8.0 / 1000.0 / secs,
                    fps: o.frames as f32 / secs,
                    loss_pct: o.remote_loss_pct,
                    nacks: o.nacks,
                    keyframe_requests: o.keyframe_requests,
                    target_kbps: Some(o.target_kbps),
                    estimate_kbps: None,
                    jitter_ms: None,
                    playout_delay_ms: None,
                    av_offset_ms: None,
                    rtt_ms: o.rtt.mean_ms(),
                };
                ev.stats(id, stats);
            }
            (o.frames, o.bytes, o.nacks, o.keyframe_requests) = (0, 0, 0, 0);
        }
        let estimate_kbps = self.estimate.map(|e| (e / 1000) as u32);
        for s in self.incoming.values_mut() {
            let stats = VideoStreamStats {
                direction: VideoStreamDirection::Incoming,
                kind,
                codec,
                kbps: s.bytes as f32 * 8.0 / 1000.0 / secs,
                fps: s.frames as f32 / secs,
                loss_pct: (s.expected > 0).then(|| s.lost as f32 * 100.0 / s.expected as f32),
                nacks: s.nacks,
                keyframe_requests: s.keyframe_requests,
                target_kbps: None,
                estimate_kbps,
                jitter_ms: Some(s.stats.jitter_ms() as f32),
                playout_delay_ms: Some(s.playout.delay().as_secs_f32() * 1000.0),
                av_offset_ms: s.playout.offset_ms(),
                rtt_ms: None,
            };
            ev.stats(s.id, stats);
            (
                s.frames,
                s.bytes,
                s.expected,
                s.lost,
                s.nacks,
                s.keyframe_requests,
            ) = (0, 0, 0, 0, 0, 0);
        }
    }

    /// Streams fallen silent have ended (a transfer brought another SSRC,
    /// or the far end stopped sending): their tiles go.
    fn sweep(&mut self) {
        let now = Instant::now();
        let receiving = self.direction.get().receives();
        // Silence while held is ours, not the stream ending: its tile
        // freezes.
        if !receiving && self.direction.keeps_tiles() {
            for s in self.incoming.values_mut() {
                s.last_packet = now;
            }
            return;
        }
        let gone: Vec<u32> = self
            .incoming
            .iter()
            .filter(|(_, s)| {
                !receiving || now.saturating_duration_since(s.last_packet) >= STREAM_IDLE
            })
            .map(|(ssrc, _)| *ssrc)
            .collect();
        for ssrc in gone {
            if let Some(s) = self.incoming.remove(&ssrc) {
                tracing::info!(ssrc, stream = s.id.0, "video: a stream fell silent");
                self.estimator.forget(ssrc);
                if let Some(ev) = &self.links.events {
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
            SessionLinks::default(),
        );
        let b = VideoSession::start(
            plan(codec, true, feedback),
            Arc::new(b_host),
            b_out.clone(),
            b_rx,
            SessionLinks::default(),
        );
        (a, b, a_out, b_out)
    }

    async fn until(what: &str, ok: impl FnMut() -> bool) {
        until_within(Duration::from_secs(5), what, ok).await
    }

    async fn until_within(within: Duration, what: &str, mut ok: impl FnMut() -> bool) {
        let deadline = Instant::now() + within;
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
    async fn a_stream_whose_first_keyframe_is_lost_shows_nothing_until_it_asks_for_one() {
        // a's sessions start before b's receives: as when a relay drops a
        // stream's first packets, its first keyframe never arrives.
        let a_host = SyntheticVideoHost::new(&[VideoCodec::H264]).with_fps(50);
        let b_host = SyntheticVideoHost::new(&[VideoCodec::H264]).with_fps(50);
        let (a_tx, a_rx) = mpsc::channel(super::super::transport::INBOUND_QUEUE);
        let (b_tx, b_rx) = mpsc::channel(super::super::transport::INBOUND_QUEUE);
        let a_out = LoopTransport::new(b_tx);
        a_out.drop_first(12);
        let _a = VideoSession::start(
            plan(VideoCodec::H264, true, ALL),
            Arc::new(a_host.clone()),
            a_out.clone(),
            a_rx,
            SessionLinks::default(),
        );
        let _b = VideoSession::start(
            plan(VideoCodec::H264, false, ALL),
            Arc::new(b_host.clone()),
            LoopTransport::new(a_tx),
            b_rx,
            SessionLinks::default(),
        );
        until("frames arrive at b", || {
            b_host
                .record()
                .received
                .values()
                .next()
                .is_some_and(|s| s.frames.len() >= 20)
        })
        .await;
        let s = b_host.record().received.values().next().unwrap().clone();
        assert!(
            s.frames[0].1,
            "the picture starts on a keyframe: {:?}",
            s.frames
        );
        assert!(
            a_host.record().sent[0].keyframe_requests >= 1,
            "b asked for one"
        );
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
            SessionLinks::default(),
        );
        let senders: Vec<VideoSession> = (0..2)
            .map(|_| {
                let (_tx, rx) = mpsc::channel(512);
                VideoSession::start(
                    plan(VideoCodec::Vp8, true, ALL),
                    Arc::new(SyntheticVideoHost::new(&[VideoCodec::Vp8]).with_fps(50)),
                    LoopTransport::new(b_tx.clone()),
                    rx,
                    SessionLinks::default(),
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
    async fn on_hold_nothing_is_sent_the_camera_closes_and_resuming_starts_on_a_keyframe() {
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

        // The camera closed with the hold.
        assert!(a_host.record().sent[0].stopped, "held: the camera is off");
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
        until("the camera opens again", || a_host.record().sent.len() == 2).await;
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

    fn events(call: u64) -> (VideoEvents, mpsc::Receiver<crate::event::Event>) {
        let (tx, rx) = mpsc::channel(1024);
        (
            VideoEvents {
                call: crate::CallId(call),
                tx,
            },
            rx,
        )
    }

    type WithEvents = (VideoSession, mpsc::Receiver<crate::event::Event>);

    /// Like [`joined`], with each side's events and `audio` on b.
    fn joined_with(
        a_host: SyntheticVideoHost,
        b_host: SyntheticVideoHost,
        b_audio: Option<Arc<dyn AudioClock>>,
    ) -> (WithEvents, WithEvents, Arc<LoopTransport>) {
        let (a_tx, a_rx) = mpsc::channel(super::super::transport::INBOUND_QUEUE);
        let (b_tx, b_rx) = mpsc::channel(super::super::transport::INBOUND_QUEUE);
        let a_out = LoopTransport::new(b_tx);
        let b_out = LoopTransport::new(a_tx);
        let (a_ev, a_events) = events(1);
        let (b_ev, b_events) = events(2);
        let a = VideoSession::start(
            plan(VideoCodec::H264, true, ALL),
            Arc::new(a_host),
            a_out.clone(),
            a_rx,
            SessionLinks {
                events: Some(a_ev),
                ..SessionLinks::default()
            },
        );
        let b = VideoSession::start(
            plan(VideoCodec::H264, true, ALL),
            Arc::new(b_host),
            b_out,
            b_rx,
            SessionLinks {
                events: Some(b_ev),
                audio: b_audio,
                ..SessionLinks::default()
            },
        );
        ((a, a_events), (b, b_events), a_out)
    }

    /// The next stats of `direction` that `ok` accepts.
    async fn stats_until(
        events: &mut mpsc::Receiver<crate::event::Event>,
        direction: VideoStreamDirection,
        what: &str,
        mut ok: impl FnMut(&VideoStreamStats) -> bool,
    ) -> VideoStreamStats {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            let e = tokio::time::timeout_at(deadline, events.recv())
                .await
                .unwrap_or_else(|_| panic!("timed out: {what}"))
                .expect("events");
            if let crate::event::Event::VideoStats { stats, .. } = e {
                if stats.direction == direction && ok(&stats) {
                    return stats;
                }
            }
        }
    }

    fn last_target(host: &SyntheticVideoHost) -> Option<u32> {
        host.record().sent.first()?.targets.last().map(|t| t.0)
    }

    #[tokio::test]
    async fn reports_go_both_ways_and_give_each_side_its_stats() {
        let a_host = SyntheticVideoHost::new(&[VideoCodec::H264]).with_fps(30);
        let b_host = SyntheticVideoHost::new(&[VideoCodec::H264]).with_fps(30);
        let ((_a, mut a_events), (_b, mut b_events), a_out) =
            joined_with(a_host.clone(), b_host.clone(), None);
        let out = stats_until(
            &mut a_events,
            VideoStreamDirection::Outgoing,
            "a round trip",
            |s| s.rtt_ms.is_some(),
        )
        .await;
        assert_eq!(out.target_kbps, Some(1500));
        assert_eq!(out.loss_pct, Some(0.0));
        assert!(out.fps > 20.0, "{out:?}");
        let inc = stats_until(
            &mut b_events,
            VideoStreamDirection::Incoming,
            "frames and an estimate",
            |s| s.fps > 20.0 && s.estimate_kbps.is_some(),
        )
        .await;
        assert_eq!(inc.loss_pct, Some(0.0));
        assert!(inc.kbps > 1000.0, "the camera's 1.5 Mb/s: {inc:?}");
        assert!(inc.estimate_kbps.unwrap() >= 1500, "a clear path: {inc:?}");
        assert!(inc.playout_delay_ms.unwrap() >= 60.0);
        // b reports on a's stream with a sender report of its own, its
        // CNAME and its REMB in one compound packet.
        let rtcp = a_out.rtcp_log.lock().clone();
        let sr = rtcp
            .iter()
            .flat_map(|p| RtcpPacket::parse_compound(p))
            .find_map(|p| match p {
                RtcpPacket::SenderReport(sr) if !sr.report_blocks.is_empty() => Some(sr),
                _ => None,
            });
        assert!(sr.is_some(), "a sends SRs with a block on b's stream");
        assert!(rtcp.iter().any(|p| {
            let ps = RtcpPacket::parse_compound(p);
            matches!(ps.first(), Some(RtcpPacket::SenderReport(_)))
                && ps
                    .iter()
                    .any(|p| matches!(p, RtcpPacket::SourceDescription(_)))
                && ps.iter().any(|p| {
                    matches!(
                        p,
                        RtcpPacket::PayloadFeedback(PsFeedback {
                            kind: PayloadFeedback::Remb { .. },
                            ..
                        })
                    )
                })
        }));
    }

    #[tokio::test]
    async fn a_bottleneck_brings_the_estimate_down_and_the_sender_follows_it() {
        let a_host = SyntheticVideoHost::new(&[VideoCodec::H264]).with_fps(30);
        let b_host = SyntheticVideoHost::new(&[VideoCodec::H264]).with_fps(30);
        let ((_a, _), (_b, mut b_events), a_out) =
            joined_with(a_host.clone(), b_host.clone(), None);
        // A 600 kb/s path under a 1.5 Mb/s camera.
        a_out.limit_kbps(600);
        until("the target follows the path down", || {
            last_target(&a_host).is_some_and(|t| t <= 700)
        })
        .await;
        stats_until(
            &mut b_events,
            VideoStreamDirection::Incoming,
            "the estimate under the path",
            |s| s.estimate_kbps.is_some_and(|e| e <= 700),
        )
        .await;
        // The camera at its new target fits the path: the queue drains
        // and frames keep coming.
        let shown = b_host
            .record()
            .received
            .values()
            .next()
            .unwrap()
            .frames
            .len();
        until_within(
            Duration::from_secs(10),
            "frames through the bottleneck",
            || {
                b_host
                    .record()
                    .received
                    .values()
                    .next()
                    .unwrap()
                    .frames
                    .len()
                    > shown + 30
            },
        )
        .await;
        // And it settles: no storm of resends or keyframes keeps the path
        // full.
        let calm = stats_until(
            &mut b_events,
            VideoStreamDirection::Incoming,
            "a calm second",
            |s| s.fps > 20.0,
        )
        .await;
        assert!(calm.nacks < 50 && calm.keyframe_requests <= 1, "{calm:?}");
    }

    #[tokio::test]
    async fn heavy_loss_lowers_the_target_and_nack_still_mends_the_picture() {
        let a_host = SyntheticVideoHost::new(&[VideoCodec::H264]).with_fps(30);
        let b_host = SyntheticVideoHost::new(&[VideoCodec::H264]).with_fps(30);
        let ((_a, _), (_b, mut b_events), a_out) =
            joined_with(a_host.clone(), b_host.clone(), None);
        // One packet in six lost on the way.
        a_out.drop_every(6);
        let inc = stats_until(
            &mut b_events,
            VideoStreamDirection::Incoming,
            "loss seen",
            |s| s.loss_pct.is_some_and(|l| l > 10.0) && s.nacks > 0,
        )
        .await;
        assert!(inc.loss_pct.unwrap() < 25.0, "{inc:?}");
        // By half the loss a second, as GCC's loss controller does.
        until_within(Duration::from_secs(15), "the target comes down", || {
            last_target(&a_host).is_some_and(|t| t < 1300)
        })
        .await;
        let s = b_host.record().received.values().next().unwrap().clone();
        let n = s.numbers();
        assert!(
            n.windows(2).all(|w| w[1] == w[0] + 1),
            "every frame, in order: {n:?}"
        );
    }

    /// Audio that takes `transit` from capture to arrival.
    struct SlowAudio {
        transit: Duration,
    }

    impl AudioClock for SlowAudio {
        fn transit_us(&self) -> Option<i64> {
            Some(self.transit.as_micros() as i64)
        }
        fn output_latency(&self) -> Duration {
            Duration::from_millis(40)
        }
        fn cname(&self) -> String {
            "slow@anvil".into()
        }
    }

    #[tokio::test]
    async fn video_waits_for_slower_audio_once_a_sender_report_arrives() {
        let a_host = SyntheticVideoHost::new(&[VideoCodec::H264]).with_fps(30);
        let b_host = SyntheticVideoHost::new(&[VideoCodec::H264]).with_fps(30);
        // b's audio from a arrives 250 ms after it was captured; its
        // video, at once.
        let audio: Arc<dyn AudioClock> = Arc::new(SlowAudio {
            transit: Duration::from_millis(250),
        });
        let ((_a, _), (_b, mut b_events), _) =
            joined_with(a_host.clone(), b_host.clone(), Some(audio));
        let inc = stats_until(
            &mut b_events,
            VideoStreamDirection::Incoming,
            "in sync",
            |s| s.av_offset_ms.is_some() && s.fps > 20.0,
        )
        .await;
        assert!(inc.av_offset_ms.unwrap().abs() < 5.0, "{inc:?}");
        let waits = b_host
            .record()
            .received
            .values()
            .next()
            .unwrap()
            .waits_ms
            .clone();
        let last = *waits.last().unwrap();
        // 250 ms of audio transit and 40 of playback.
        assert!(
            (last - 290.0).abs() < 25.0,
            "frames wait for their audio: {last}"
        );
        // Before the first report, only the playout delay.
        assert!(waits[0] < 100.0, "{waits:?}");
    }

    #[test]
    fn the_ladder_steps_the_picture_down_with_the_rate() {
        assert_eq!(ladder(1500, Resolution::P720, 30), (Resolution::P720, 30));
        assert_eq!(ladder(600, Resolution::P720, 30).0.height, 360);
        assert_eq!(ladder(200, Resolution::P720, 30).0.height, 180);
        assert_eq!(ladder(100, Resolution::P720, 30).1, 15);
        let small = Resolution {
            width: 320,
            height: 240,
        };
        assert_eq!(ladder(1500, small, 30).0, small, "never above the max");
    }

    #[test]
    fn a_cheap_picture_keeps_its_size_and_a_short_network_takes_it_down() {
        // Sending 95 kb/s of a still picture; the far end allows 1.5× that.
        assert_eq!(ladder_kbps(176, Some(95), 1500), 1500);
        // Allowed more: it may grow from a lower rung.
        assert_eq!(ladder_kbps(1200, Some(300), 600), 1200);
        // The estimate under what is sent: the network is the limit.
        assert_eq!(ladder_kbps(400, Some(900), 1500), 400);
        assert_eq!(ladder_kbps(1000, Some(900), 1500), 1000);
        // Nothing measured yet: the estimate as it is.
        assert_eq!(ladder_kbps(300, None, 1500), 300);
    }
}
