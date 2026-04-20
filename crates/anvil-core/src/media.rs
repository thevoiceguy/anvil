//! Per-call RTP send/receive pipeline.
//!
//! Phase 1 M3 scope: one audio stream per call, G.711 µ-law (PT 0) at 8 kHz,
//! 20 ms frames (160 samples per packet), full-duplex. No jitter buffer yet —
//! received packets feed the audio sink in arrival order; Phase 2 adds a
//! proper reorder / loss-conceal buffer.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use bytes::Bytes;
use forge_codecs::g711;
use forge_codecs::g722::{G722BitRate, G722Decoder, G722Encoder};
use forge_rtp::rtp::RtpPacket;
use parking_lot::Mutex;
use rand::{thread_rng, RngCore};
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;

use crate::audio::{AudioFormat, AudioFrame, AudioSink, AudioSource};
use crate::config::Codec;
use crate::error::AnvilError;

/// Handles to the three tasks driving a single call's media (send, receive,
/// stats sampler) plus the atomic counters they share. Dropped (and aborted)
/// when the call ends.
pub(crate) struct MediaPipeline {
    send_task: JoinHandle<()>,
    recv_task: JoinHandle<()>,
    stats_task: JoinHandle<()>,
    pub(crate) metrics: Arc<MediaMetrics>,
}

impl MediaPipeline {
    pub(crate) fn abort(&self) {
        self.send_task.abort();
        self.recv_task.abort();
        self.stats_task.abort();
    }
}

impl Drop for MediaPipeline {
    fn drop(&mut self) {
        self.abort();
    }
}

/// Per-call counters shared between send task, receive task, and the
/// periodic stats sampler. Everything is atomic or behind a small mutex so
/// the RTP hot paths never block each other.
pub(crate) struct MediaMetrics {
    pub packets_rx: AtomicU64,
    pub packets_tx: AtomicU64,
    pub bytes_rx: AtomicU64,
    pub bytes_tx: AtomicU64,
    /// RFC 3550 §6.4.1 jitter accumulator plus the last packet's transit
    /// time, in RTP timestamp units. The `Mutex` is only touched on RTP
    /// receive; consumption reads a snapshot under the same lock.
    jitter: Mutex<JitterState>,
    /// Sequence-number bookkeeping per RFC 3550 §A.1, used to derive a
    /// packet-loss count.
    seq: Mutex<SeqState>,
    /// Audio clock, used to convert the jitter accumulator from RTP units
    /// to milliseconds when reporting.
    rtp_clock_rate: u32,
}

impl MediaMetrics {
    fn new(rtp_clock_rate: u32) -> Arc<Self> {
        Arc::new(Self {
            packets_rx: AtomicU64::new(0),
            packets_tx: AtomicU64::new(0),
            bytes_rx: AtomicU64::new(0),
            bytes_tx: AtomicU64::new(0),
            jitter: Mutex::new(JitterState::default()),
            seq: Mutex::new(SeqState::default()),
            rtp_clock_rate,
        })
    }

    fn on_rtp_rx(&self, seq: u16, rtp_ts: u32, bytes: usize, arrival: Instant) {
        self.packets_rx.fetch_add(1, Ordering::Relaxed);
        self.bytes_rx.fetch_add(bytes as u64, Ordering::Relaxed);

        // Jitter. RFC 3550: D(i,j) = (Rj-Ri) - (Sj-Si), then
        //   J(i) = J(i-1) + (|D(i-1,i)| - J(i-1)) / 16.
        // We work in RTP timestamp units; arrival time is converted from
        // Instant by scaling elapsed seconds by rtp_clock_rate. `arrival_ts`
        // can be any monotonic value with rtp_clock_rate ticks / s.
        let arrival_ticks = self.arrival_ticks(arrival);
        let transit = arrival_ticks.wrapping_sub(rtp_ts as i64);
        let mut j = self.jitter.lock();
        if let Some(last) = j.last_transit {
            let d = transit.wrapping_sub(last).unsigned_abs() as f64;
            j.current += (d - j.current) / 16.0;
        }
        j.last_transit = Some(transit);
        drop(j);

        // Sequence tracking (RFC 3550 §A.1, simplified).
        let mut s = self.seq.lock();
        s.received = s.received.saturating_add(1);
        if s.first.is_none() {
            s.first = Some(seq);
            s.highest = seq;
            return;
        }
        // Signed difference handles 16-bit wraparound.
        let delta = (seq as i32).wrapping_sub(s.highest as i32);
        if delta > 0 && delta < 32_768 {
            s.highest = seq;
        } else if delta < -32_768 {
            // Forward wrap.
            s.cycles = s.cycles.saturating_add(1);
            s.highest = seq;
        }
        // Out-of-order packets don't advance `highest`; they're still
        // counted via `received` above so the expected - received delta
        // works out.
    }

    fn on_rtp_tx(&self, bytes: usize) {
        self.packets_tx.fetch_add(1, Ordering::Relaxed);
        self.bytes_tx.fetch_add(bytes as u64, Ordering::Relaxed);
    }

    fn arrival_ticks(&self, arrival: Instant) -> i64 {
        // Anchor to the first call (doesn't matter what time zero is, as long
        // as it's monotonic). Cache the anchor via a OnceCell-ish trick: use
        // the `SeqState`'s Instant-on-first-packet... For now just use the
        // absolute nanoseconds since an epoch we don't need to be stable
        // across runs. `elapsed_since_unix` isn't needed — we only compare
        // deltas, so use an arbitrary fixed reference.
        static ANCHOR: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        let anchor = *ANCHOR.get_or_init(Instant::now);
        let elapsed = arrival.saturating_duration_since(anchor).as_nanos() as i64;
        // Convert to RTP clock units: ticks = elapsed_ns * clock_rate / 1e9.
        elapsed.saturating_mul(self.rtp_clock_rate as i64) / 1_000_000_000
    }
}

#[derive(Default)]
struct JitterState {
    last_transit: Option<i64>,
    current: f64,
}

#[derive(Default)]
struct SeqState {
    first: Option<u16>,
    highest: u16,
    cycles: u32,
    /// Total packets received (including out-of-order, including duplicates).
    received: u32,
}

/// 20 ms frame duration across every codec we support today.
const FRAME_MS: u32 = 20;

/// Static info about a codec: RTP payload type and clock, PCM sample rate.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CodecSpec {
    pub codec: Codec,
    /// RTP payload type as it appears in the m= line and in RTP packets.
    pub payload_type: u8,
    /// RTP timestamp clock rate (Hz). Equals PCM `sample_rate` for every codec
    /// except G.722, which per RFC 3551 §4.5.2 uses an 8 kHz RTP clock despite
    /// a 16 kHz sample rate.
    pub rtp_clock_rate: u32,
    /// Sample rate the audio pipeline feeds the codec at.
    pub sample_rate: u32,
}

impl CodecSpec {
    pub(crate) fn for_codec(codec: Codec) -> Self {
        match codec {
            Codec::Pcmu => Self { codec, payload_type: 0, rtp_clock_rate: 8000, sample_rate: 8000 },
            Codec::Pcma => Self { codec, payload_type: 8, rtp_clock_rate: 8000, sample_rate: 8000 },
            Codec::G722 => Self { codec, payload_type: 9, rtp_clock_rate: 8000, sample_rate: 16000 },
            // Opus arrives with P2-M3. Slot reserved so sdp_offer_codecs
            // below compiles exhaustively, but start_pipeline will reject it
            // until the driver exists.
            Codec::Opus => Self { codec, payload_type: 111, rtp_clock_rate: 48000, sample_rate: 48000 },
        }
    }

    /// PCM samples per 20 ms frame at `sample_rate`.
    fn samples_per_frame(&self) -> usize {
        (self.sample_rate * FRAME_MS / 1000) as usize
    }

    /// RTP timestamp increment between successive 20 ms packets.
    fn rtp_timestamp_per_frame(&self) -> u32 {
        self.rtp_clock_rate * FRAME_MS / 1000
    }

    /// Audio-host format matching this codec.
    pub(crate) fn audio_format(&self) -> AudioFormat {
        AudioFormat {
            sample_rate: self.sample_rate,
            channels: 1,
            frame_ms: FRAME_MS,
        }
    }
}

/// Kept for backward compatibility; returns the G.711 µ-law format.
pub(crate) fn g711_format() -> AudioFormat {
    CodecSpec::for_codec(Codec::Pcmu).audio_format()
}

/// Codec plugin — encodes PCM-16 frames to the RTP payload format, decodes
/// them back. G.711 implementations are stateless; G.722 is stateful.
trait CodecDriver: Send {
    fn spec(&self) -> CodecSpec;
    fn encode(&mut self, pcm: &[i16]) -> Vec<u8>;
    fn decode(&mut self, bytes: &[u8]) -> Vec<i16>;
}

struct PcmuDriver;
impl CodecDriver for PcmuDriver {
    fn spec(&self) -> CodecSpec { CodecSpec::for_codec(Codec::Pcmu) }
    fn encode(&mut self, pcm: &[i16]) -> Vec<u8> {
        pcm.iter().map(|&s| g711::encode_ulaw(s)).collect()
    }
    fn decode(&mut self, bytes: &[u8]) -> Vec<i16> {
        bytes.iter().map(|&b| g711::decode_ulaw(b)).collect()
    }
}

struct PcmaDriver;
impl CodecDriver for PcmaDriver {
    fn spec(&self) -> CodecSpec { CodecSpec::for_codec(Codec::Pcma) }
    fn encode(&mut self, pcm: &[i16]) -> Vec<u8> {
        pcm.iter().map(|&s| g711::encode_alaw(s)).collect()
    }
    fn decode(&mut self, bytes: &[u8]) -> Vec<i16> {
        bytes.iter().map(|&b| g711::decode_alaw(b)).collect()
    }
}

struct OpusDriver {
    encoder: opus::Encoder,
    decoder: opus::Decoder,
    /// Samples expected per 20 ms frame, i.e. sample_rate * 20 / 1000.
    frame_samples: usize,
}

impl OpusDriver {
    fn new() -> anyhow::Result<Self> {
        // Voip mode optimises for speech: aggressive VAD / DTX, lower
        // bit-rate bias, bandlimited to fullband.
        let spec = CodecSpec::for_codec(Codec::Opus);
        let encoder = opus::Encoder::new(
            spec.sample_rate,
            opus::Channels::Mono,
            opus::Application::Voip,
        )?;
        let decoder = opus::Decoder::new(spec.sample_rate, opus::Channels::Mono)?;
        Ok(Self {
            encoder,
            decoder,
            frame_samples: spec.samples_per_frame(),
        })
    }
}

impl CodecDriver for OpusDriver {
    fn spec(&self) -> CodecSpec { CodecSpec::for_codec(Codec::Opus) }

    fn encode(&mut self, pcm: &[i16]) -> Vec<u8> {
        // 1 275 bytes is libopus's hard max for a 20 ms frame at 48 kHz.
        // In practice voice payloads at reasonable bitrates stay well under
        // 200 bytes.
        self.encoder.encode_vec(pcm, 1275).unwrap_or_default()
    }

    fn decode(&mut self, bytes: &[u8]) -> Vec<i16> {
        let mut out = vec![0i16; self.frame_samples];
        match self.decoder.decode(bytes, &mut out, false) {
            Ok(n) => {
                out.truncate(n);
                out
            }
            Err(e) => {
                tracing::debug!(%e, "opus decode failed");
                Vec::new()
            }
        }
    }
}

struct G722DriverImpl {
    encoder: G722Encoder,
    decoder: G722Decoder,
}

impl G722DriverImpl {
    fn new() -> Self {
        Self {
            encoder: G722Encoder::new(G722BitRate::Rate64k),
            decoder: G722Decoder::new(G722BitRate::Rate64k),
        }
    }
}

impl CodecDriver for G722DriverImpl {
    fn spec(&self) -> CodecSpec { CodecSpec::for_codec(Codec::G722) }
    fn encode(&mut self, pcm: &[i16]) -> Vec<u8> {
        self.encoder.encode(pcm).unwrap_or_default()
    }
    fn decode(&mut self, bytes: &[u8]) -> Vec<i16> {
        self.decoder.decode(bytes)
    }
}

/// Build a codec driver. Opus construction is fallible (libopus init); other
/// codecs are infallible.
fn make_driver(codec: Codec) -> Option<Box<dyn CodecDriver>> {
    Some(match codec {
        Codec::Pcmu => Box::new(PcmuDriver),
        Codec::Pcma => Box::new(PcmaDriver),
        Codec::G722 => Box::new(G722DriverImpl::new()),
        Codec::Opus => match OpusDriver::new() {
            Ok(d) => Box::new(d),
            Err(e) => {
                tracing::warn!(%e, "opus driver init failed");
                return None;
            }
        },
    })
}

/// Codecs we support today, in the order we prefer to offer them.
pub(crate) fn supported_codecs() -> &'static [Codec] {
    // Opus first (48 kHz fullband, voice-optimised) → G.722 (16 kHz wideband)
    // → PCMU / PCMA (8 kHz narrowband, mandatory per RFC 3551).
    &[Codec::Opus, Codec::G722, Codec::Pcmu, Codec::Pcma]
}

fn rtpmap_name(codec: Codec) -> &'static str {
    match codec {
        Codec::Pcmu => "PCMU",
        Codec::Pcma => "PCMA",
        Codec::G722 => "G722",
        Codec::Opus => "opus",
    }
}

/// Build an SDP session for exactly `codecs` (in that order) plus
/// telephone-event. `username` is used for the o= line; media endpoint is
/// `addr:rtp_port`.
///
/// Built directly from `SessionDescription::builder` rather than
/// `MediaProfileBuilder::audio_only()` because the latter pre-populates
/// PCMU + PCMA and only supports appending, which made single-codec
/// answers carry spurious extra formats.
pub(crate) fn build_sdp(
    codecs: &[Codec],
    username: &str,
    addr: &str,
    rtp_port: u16,
) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let session_id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string());

    let mut media = sip_sdp::MediaDescription::audio(rtp_port);
    for codec in codecs {
        let spec = CodecSpec::for_codec(*codec);
        media = media
            .add_format(spec.payload_type)
            .expect("valid format")
            .add_rtpmap(spec.payload_type, rtpmap_name(*codec), spec.rtp_clock_rate, None)
            .expect("valid rtpmap");
    }
    // RFC 2833 DTMF. Phase 2 M4 wires up send/receive; advertising it
    // today is harmless and keeps CallKit / PBX DTMF UIs happy.
    media = media
        .add_format(101)
        .expect("valid format")
        .add_rtpmap(101, "telephone-event", 8000, None)
        .expect("valid rtpmap")
        .add_attribute("fmtp", "101 0-16")
        .expect("valid attribute");

    sip_sdp::SessionDescription::builder()
        .origin(username, &session_id, addr)
        .expect("valid origin")
        .session_name("anvil")
        .expect("valid session name")
        .connection(addr)
        .expect("valid connection")
        .time(0, 0)
        .media(media)
        .expect("valid media")
        .build()
        .to_string()
}

/// Given an SDP offer body and our preference list, return the codec we want
/// to answer with (highest-preference one that both sides support).
pub(crate) fn negotiate_answer_codec(offer: &[u8], prefer: &[Codec]) -> Option<Codec> {
    let text = std::str::from_utf8(offer).ok()?;
    let sdp = sip_sdp::parse::parse_sdp(text).ok()?;
    let media = sdp
        .media
        .iter()
        .find(|m| m.media_type == sip_sdp::MediaType::Audio)?;

    // Collect the codec set the peer offered, indexed by our Codec enum.
    let mut offered: std::collections::BTreeSet<Codec> = std::collections::BTreeSet::new();
    for f in &media.formats {
        let pt: u8 = match f.parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        if let Some(c) = codec_from_pt(pt, &media.rtpmaps) {
            offered.insert(c);
        }
    }

    // Walk our preference list in order, return the first one the peer offered.
    prefer.iter().copied().find(|c| offered.contains(c))
}

fn codec_from_pt(pt: u8, rtpmaps: &std::collections::HashMap<u8, sip_sdp::RtpMap>) -> Option<Codec> {
    match pt {
        0 => Some(Codec::Pcmu),
        8 => Some(Codec::Pcma),
        9 => Some(Codec::G722),
        other => {
            let name = rtpmaps.get(&other)?.encoding_name.as_str();
            match name.to_ascii_lowercase().as_str() {
                "opus" => Some(Codec::Opus),
                "pcmu" => Some(Codec::Pcmu),
                "pcma" => Some(Codec::Pcma),
                "g722" => Some(Codec::G722),
                _ => None,
            }
        }
    }
}

/// Start the media pipeline for an established call using the negotiated codec.
///
/// `stats` is the channel the per-call stats sampler emits `MediaStats`
/// events to, tagged with `call_id`. `stats_interval` controls cadence
/// (use `std::time::Duration::ZERO` to disable sampling for tests that
/// don't care about it).
#[allow(clippy::too_many_arguments)]
pub(crate) fn start_pipeline(
    codec: Codec,
    rtp_socket: Arc<UdpSocket>,
    remote_rtp: SocketAddr,
    mut capture: Box<dyn AudioSource>,
    mut playback: Box<dyn AudioSink>,
    stats_sink: Option<StatsSink>,
) -> Result<MediaPipeline, AnvilError> {
    let send_driver =
        make_driver(codec).ok_or_else(|| AnvilError::Codec(format!("no driver for {codec:?}")))?;
    let recv_driver =
        make_driver(codec).ok_or_else(|| AnvilError::Codec(format!("no driver for {codec:?}")))?;

    let mut rng = thread_rng();
    let ssrc: u32 = rng.next_u32();
    let initial_seq: u16 = rng.next_u32() as u16;
    let initial_ts: u32 = rng.next_u32();

    let send_spec = send_driver.spec();
    let recv_spec = recv_driver.spec();
    let samples_per_frame = send_spec.samples_per_frame();
    let ts_per_frame = send_spec.rtp_timestamp_per_frame();
    let payload_type = send_spec.payload_type;

    let metrics = MediaMetrics::new(send_spec.rtp_clock_rate);

    // Send task: pull frames from the mic, encode, RTP-wrap, send.
    let sock_send = Arc::clone(&rtp_socket);
    let metrics_send = Arc::clone(&metrics);
    let send_task = tokio::spawn(async move {
        let mut driver = send_driver;
        let mut seq = initial_seq;
        let mut ts = initial_ts;
        let mut sent_any = false;

        while let Some(frame) = capture.next_frame().await {
            if frame.samples.len() != samples_per_frame {
                tracing::warn!(
                    got = frame.samples.len(),
                    want = samples_per_frame,
                    codec = ?codec,
                    "capture frame size mismatch; skipping"
                );
                continue;
            }

            let payload = driver.encode(&frame.samples);

            // RFC 3551: marker bit on first packet of a talkspurt. We don't do
            // VAD yet, so set marker only on the very first packet.
            let marker = !sent_any;
            let packet = RtpPacket::build(
                payload_type,
                seq,
                ts,
                ssrc,
                Bytes::from(payload),
                marker,
            );
            let bytes = packet.to_bytes();

            match sock_send.send_to(&bytes, remote_rtp).await {
                Ok(_) => metrics_send.on_rtp_tx(bytes.len()),
                Err(e) => tracing::warn!(%e, %remote_rtp, "rtp send failed"),
            }

            seq = seq.wrapping_add(1);
            ts = ts.wrapping_add(ts_per_frame);
            sent_any = true;
        }

        tracing::debug!(packets_sent = sent_any, "send task exiting");
    });

    // Receive task: read UDP, parse RTP, decode, push to speaker.
    let sock_recv = Arc::clone(&rtp_socket);
    let recv_pt = recv_spec.payload_type;
    let recv_fmt = recv_spec.audio_format();
    let metrics_recv = Arc::clone(&metrics);
    let recv_task = tokio::spawn(async move {
        let mut driver = recv_driver;
        let mut buf = vec![0u8; 2048];
        loop {
            let (len, _peer) = match sock_recv.recv_from(&mut buf).await {
                Ok(x) => x,
                Err(e) => {
                    tracing::warn!(%e, "rtp recv failed");
                    break;
                }
            };
            let arrival = Instant::now();

            let data = Bytes::copy_from_slice(&buf[..len]);
            let packet = match RtpPacket::parse(data) {
                Ok(p) => p,
                Err(e) => {
                    tracing::debug!(%e, "rtp parse failed; ignoring");
                    continue;
                }
            };

            if packet.header.payload_type() != recv_pt {
                tracing::debug!(
                    pt = packet.header.payload_type(),
                    want = recv_pt,
                    "unexpected payload type; ignoring"
                );
                continue;
            }

            metrics_recv.on_rtp_rx(
                packet.header.sequence_number,
                packet.header.timestamp,
                len,
                arrival,
            );

            let samples = driver.decode(&packet.payload);
            let frame = AudioFrame {
                samples,
                format: recv_fmt,
            };

            if let Err(e) = playback.write_frame(&frame).await {
                tracing::warn!(%e, "playback write failed");
                break;
            }
        }
    });

    // Stats task: periodically snapshot the counters and emit MediaStats.
    let stats_task = {
        let metrics = Arc::clone(&metrics);
        let clock = send_spec.rtp_clock_rate;
        tokio::spawn(async move {
            let Some(sink) = stats_sink else { return };
            let mut ticker = tokio::time::interval(sink.interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            // First tick fires immediately — skip it so we report on real
            // elapsed intervals.
            ticker.tick().await;

            let mut prev_bytes_tx = 0u64;
            let mut prev_bytes_rx = 0u64;
            let mut prev_wall = Instant::now();

            loop {
                ticker.tick().await;
                let now = Instant::now();
                let elapsed = now.saturating_duration_since(prev_wall).as_secs_f32().max(0.001);
                prev_wall = now;

                let bytes_tx = metrics.bytes_tx.load(Ordering::Relaxed);
                let bytes_rx = metrics.bytes_rx.load(Ordering::Relaxed);
                let send_kbps = (bytes_tx.saturating_sub(prev_bytes_tx) as f32 * 8.0)
                    / elapsed
                    / 1000.0;
                let recv_kbps = (bytes_rx.saturating_sub(prev_bytes_rx) as f32 * 8.0)
                    / elapsed
                    / 1000.0;
                prev_bytes_tx = bytes_tx;
                prev_bytes_rx = bytes_rx;

                let jitter_ticks = metrics.jitter.lock().current;
                let jitter_ms = jitter_ticks as f32 * 1000.0 / clock as f32;

                let (expected, received) = {
                    let s = metrics.seq.lock();
                    let expected = match s.first {
                        Some(first) => {
                            (s.cycles as u64) * 65_536
                                + (s.highest as u64).wrapping_sub(first as u64)
                                + 1
                        }
                        None => 0,
                    };
                    (expected, s.received as u64)
                };
                let packet_loss_pct = if expected == 0 {
                    0.0
                } else {
                    let lost = expected.saturating_sub(received);
                    (lost as f32 * 100.0) / expected as f32
                };

                let stats = crate::event::MediaStats {
                    codec: sink.codec,
                    jitter_ms,
                    packet_loss_pct,
                    rtt_ms: None, // RTT needs RTCP — Phase 2 later milestone.
                    recv_kbps,
                    send_kbps,
                };
                // Drop silently if the event stream is full; stats are
                // strictly advisory.
                let _ = sink
                    .events
                    .send(crate::event::Event::MediaStats { call: sink.call, stats })
                    .await;
            }
        })
    };

    Ok(MediaPipeline {
        send_task,
        recv_task,
        stats_task,
        metrics,
    })
}

/// Sink for periodic `MediaStats` events.
pub(crate) struct StatsSink {
    pub call: crate::CallId,
    pub codec: Codec,
    pub interval: std::time::Duration,
    pub events: tokio::sync::mpsc::Sender<crate::event::Event>,
}

/// Parse the SDP answer body and extract the remote `(host, port)` of the
/// audio m= line. Phase 1 M3 accepts IPv4 and IPv6; higher-level address
/// hygiene (STUN-observed reflexive candidates, ICE, NAT traversal) is a
/// Phase 2 concern.
pub(crate) fn extract_remote_rtp_addr(body: &[u8]) -> Option<SocketAddr> {
    let text = std::str::from_utf8(body).ok()?;
    let sdp = sip_sdp::parse::parse_sdp(text).ok()?;
    let media = sdp
        .media
        .iter()
        .find(|m| m.media_type == sip_sdp::MediaType::Audio)?;

    let port = media.port;

    // Prefer media-level c= if present, otherwise session-level.
    let conn = media
        .connection
        .as_ref()
        .or(sdp.connection.as_ref())?;
    let ip: std::net::IpAddr = conn.connection_address.parse().ok()?;

    Some(SocketAddr::new(ip, port))
}

/// Best-effort codec detection from an SDP offer body. Same rules as the
/// answer path: first m= format byte, fall back to rtpmap lookup.
pub(crate) fn extract_codec_from_offer(body: &[u8]) -> Option<crate::config::Codec> {
    use crate::config::Codec;
    let text = std::str::from_utf8(body).ok()?;
    let sdp = sip_sdp::parse::parse_sdp(text).ok()?;
    let media = sdp
        .media
        .iter()
        .find(|m| m.media_type == sip_sdp::MediaType::Audio)?;
    let first_pt: u8 = media.formats.first()?.parse().ok()?;
    match first_pt {
        0 => Some(Codec::Pcmu),
        8 => Some(Codec::Pcma),
        9 => Some(Codec::G722),
        _ => {
            let name = media.rtpmaps.get(&first_pt)?.encoding_name.as_str();
            match name.to_ascii_lowercase().as_str() {
                "opus" => Some(Codec::Opus),
                "pcmu" => Some(Codec::Pcmu),
                "pcma" => Some(Codec::Pcma),
                "g722" => Some(Codec::G722),
                _ => None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! End-to-end validation of the RTP pipeline: two sockets, two
    //! synthetic tone sources, each instance sends to the other, both
    //! verify that audio survives the µ-law round trip.
    //!
    //! This is the M3 milestone exit criterion minus the SIP signaling
    //! (which M1 + M2 already covered). Incoming INVITE support is M4.
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use async_trait::async_trait;
    use tokio::net::UdpSocket;

    use super::*;
    use crate::audio::{AudioFormat, AudioFrame, AudioSink, AudioSource};
    use crate::error::AnvilError;

    const FRAME_MS: u32 = 20;
    const SAMPLE_RATE: u32 = 8000;

    struct ToneSource {
        sample_rate: u32,
        samples_per_frame: usize,
        phase: f32,
        phase_step: f32,
        next_tick: Option<tokio::time::Instant>,
    }

    impl ToneSource {
        fn new_at(freq_hz: f32, sample_rate: u32) -> Self {
            let samples_per_frame = (sample_rate * FRAME_MS / 1000) as usize;
            Self {
                sample_rate,
                samples_per_frame,
                phase: 0.0,
                phase_step: std::f32::consts::TAU * freq_hz / sample_rate as f32,
                next_tick: None,
            }
        }
    }

    #[async_trait]
    impl AudioSource for ToneSource {
        async fn next_frame(&mut self) -> Option<AudioFrame> {
            let period = Duration::from_millis(FRAME_MS as u64);
            let target = self.next_tick.unwrap_or_else(tokio::time::Instant::now);
            let now = tokio::time::Instant::now();
            if target > now {
                tokio::time::sleep(target - now).await;
            }
            self.next_tick = Some(target + period);

            let mut samples = Vec::with_capacity(self.samples_per_frame);
            let scale = 0.3 * i16::MAX as f32;
            for _ in 0..self.samples_per_frame {
                let s = (self.phase.sin() * scale) as i16;
                samples.push(s);
                self.phase += self.phase_step;
                if self.phase > std::f32::consts::TAU {
                    self.phase -= std::f32::consts::TAU;
                }
            }
            Some(AudioFrame {
                samples,
                format: AudioFormat {
                    sample_rate: self.sample_rate,
                    channels: 1,
                    frame_ms: FRAME_MS,
                },
            })
        }
    }

    struct RmsSink {
        frames: Arc<AtomicU64>,
        peak_rms_q: Arc<AtomicU64>,
    }

    #[async_trait]
    impl AudioSink for RmsSink {
        async fn write_frame(&mut self, frame: &AudioFrame) -> Result<(), AnvilError> {
            self.frames.fetch_add(1, Ordering::Relaxed);
            let n = frame.samples.len().max(1) as f64;
            let sum_sq: f64 = frame
                .samples
                .iter()
                .map(|&s| (s as f64) * (s as f64))
                .sum();
            let rms = (sum_sq / n).sqrt() as u64;
            self.peak_rms_q.fetch_max(rms, Ordering::Relaxed);
            Ok(())
        }
    }

    async fn round_trip_for(codec: Codec, sample_rate: u32) {
        let loopback = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let sock_a = Arc::new(UdpSocket::bind(SocketAddr::new(loopback, 0)).await.unwrap());
        let sock_b = Arc::new(UdpSocket::bind(SocketAddr::new(loopback, 0)).await.unwrap());
        let addr_a = sock_a.local_addr().unwrap();
        let addr_b = sock_b.local_addr().unwrap();

        let frames_a = Arc::new(AtomicU64::new(0));
        let rms_a = Arc::new(AtomicU64::new(0));
        let frames_b = Arc::new(AtomicU64::new(0));
        let rms_b = Arc::new(AtomicU64::new(0));

        let pipe_a = start_pipeline(
            codec,
            Arc::clone(&sock_a),
            addr_b,
            Box::new(ToneSource::new_at(440.0, sample_rate)),
            Box::new(RmsSink {
                frames: Arc::clone(&frames_a),
                peak_rms_q: Arc::clone(&rms_a),
            }),
            None,
        )
        .expect("pipeline A");

        let pipe_b = start_pipeline(
            codec,
            Arc::clone(&sock_b),
            addr_a,
            Box::new(ToneSource::new_at(660.0, sample_rate)),
            Box::new(RmsSink {
                frames: Arc::clone(&frames_b),
                peak_rms_q: Arc::clone(&rms_b),
            }),
            None,
        )
        .expect("pipeline B");

        tokio::time::sleep(Duration::from_millis(1000)).await;

        drop(pipe_a);
        drop(pipe_b);

        let frames_a = frames_a.load(Ordering::Relaxed);
        let frames_b = frames_b.load(Ordering::Relaxed);
        let rms_a = rms_a.load(Ordering::Relaxed);
        let rms_b = rms_b.load(Ordering::Relaxed);

        println!("{codec:?}: A={frames_a} fr, rms {rms_a}; B={frames_b} fr, rms {rms_b}");
        assert!(frames_a >= 30, "{codec:?}: A frames={frames_a}");
        assert!(frames_b >= 30, "{codec:?}: B frames={frames_b}");
        assert!(rms_a > 3000, "{codec:?}: A rms={rms_a} (silent?)");
        assert!(rms_b > 3000, "{codec:?}: B rms={rms_b} (silent?)");
    }

    /// Two RTP pipelines trade µ-law audio over loopback for one second.
    /// Exit criterion: each side's receive-side RMS is materially above noise.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn rtp_round_trip_preserves_audio() {
        round_trip_for(Codec::Pcmu, 8000).await;
    }

    /// G.722 round-trip at 16 kHz sample rate, 8 kHz RTP clock.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn rtp_round_trip_g722() {
        round_trip_for(Codec::G722, 16000).await;
    }

    /// PCMA round-trip — same shape as PCMU but different encoding table.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn rtp_round_trip_pcma() {
        round_trip_for(Codec::Pcma, 8000).await;
    }

    /// Opus round-trip at 48 kHz fullband.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn rtp_round_trip_opus() {
        round_trip_for(Codec::Opus, 48000).await;
    }

}

