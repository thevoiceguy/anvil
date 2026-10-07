//! Per-call RTP send/receive pipeline.
//!
//! Phase 1 M3 scope: one audio stream per call, G.711 µ-law (PT 0) at 8 kHz,
//! 20 ms frames (160 samples per packet), full-duplex. No jitter buffer yet —
//! received packets feed the audio sink in arrival order; Phase 2 adds a
//! proper reorder / loss-conceal buffer.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use bytes::Bytes;
use forge_codecs::g711;
use forge_codecs::g722::{G722BitRate, G722Decoder, G722Encoder};
use forge_dtmf::dedup::DtmfDeduplicator;
use forge_dtmf::rfc2833::{Rfc2833Detector, Rfc2833Generator};
use forge_dtmf::DtmfDigit;
use forge_rtp::rtp::RtpPacket;
use parking_lot::Mutex;
use rand::{thread_rng, RngCore};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::audio::{AudioFormat, AudioFrame, AudioSink, AudioSource};
use crate::config::Codec;
use crate::error::AnvilError;

/// Handles to the three tasks driving a single call's media (send, receive,
/// stats sampler). Dropped (and aborted)
/// when the call ends.
pub(crate) struct MediaPipeline {
    send_task: JoinHandle<()>,
    recv_task: JoinHandle<()>,
    stats_task: JoinHandle<()>,
    /// Queue for outgoing DTMF. `Anvil::send_dtmf` pushes; the send task
    /// interleaves them into the RTP stream as RFC 2833 event packets.
    pub(crate) dtmf_tx: mpsc::Sender<char>,
    /// Hold / direction state. Flipped by `Anvil::hold` (local hold) and
    /// by the dispatcher's re-INVITE path (remote hold).
    pub(crate) hold: Arc<HoldState>,
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
        let forward = if delta < -32_768 {
            delta + 65_536
        } else if delta > 32_768 {
            delta - 65_536
        } else {
            delta
        };
        if !(-MAX_MISORDER..=MAX_DROPOUT).contains(&forward) {
            // A jump no loss or reordering explains: the far end is a new
            // stream (a transfer, a re-anchored leg). Count afresh
            // (RFC 3550 §A.1).
            *s = SeqState {
                first: Some(seq),
                highest: seq,
                cycles: 0,
                received: 1,
            };
        } else if forward > 0 {
            if seq < s.highest {
                // Forward wrap.
                s.cycles = s.cycles.saturating_add(1);
            }
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

/// The largest forward jump in sequence numbers still one stream, and the
/// largest backward one still reordering (RFC 3550 §A.1).
const MAX_DROPOUT: i32 = 3000;
const MAX_MISORDER: i32 = 100;

#[derive(Default)]
struct SeqState {
    first: Option<u16>,
    highest: u16,
    cycles: u32,
    /// Total packets received (including out-of-order, including duplicates).
    received: u32,
}

/// A packet ready for the wire: encrypted under our key when the call is
/// SRTP. `None` when it cannot be (logged; the packet is not sent).
fn protect(
    srtp: Option<&Arc<parking_lot::Mutex<forge_rtp::srtp::SrtpContext>>>,
    packet: impl Into<Bytes>,
) -> Option<Bytes> {
    let packet: Bytes = packet.into();
    match srtp {
        None => Some(packet),
        Some(ctx) => match ctx.lock().protect_rtp(&packet) {
            Ok(sealed) => Some(Bytes::from(sealed)),
            Err(e) => {
                tracing::warn!(%e, "srtp protect failed");
                None
            }
        },
    }
}

/// 20 ms frame duration across every codec we support today.
const FRAME_MS: u32 = 20;

/// RFC 4566 media direction. Controls whether the send / receive tasks
/// push frames during a call. `Sendrecv` is the default; a hold re-INVITE
/// flips to `Sendonly` on the holder or `Recvonly` on the peer being held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MediaDirection {
    Sendrecv,
    Sendonly,
    Recvonly,
    Inactive,
}

impl MediaDirection {
    fn as_sdp_attr(self) -> &'static str {
        match self {
            Self::Sendrecv => "sendrecv",
            Self::Sendonly => "sendonly",
            Self::Recvonly => "recvonly",
            Self::Inactive => "inactive",
        }
    }

    /// Mirror the peer's direction onto ours for the answer side. If the
    /// peer said `sendonly`, they'll send but expect us not to — i.e. we
    /// answer `recvonly`. Symmetric for the other cases.
    pub(crate) fn mirror(self) -> Self {
        match self {
            Self::Sendrecv => Self::Sendrecv,
            Self::Sendonly => Self::Recvonly,
            Self::Recvonly => Self::Sendonly,
            Self::Inactive => Self::Inactive,
        }
    }

    pub(crate) fn sends(self) -> bool {
        matches!(self, Self::Sendrecv | Self::Sendonly)
    }

    pub(crate) fn receives(self) -> bool {
        matches!(self, Self::Sendrecv | Self::Recvonly)
    }
}

/// Two bits of hold state shared between the send and receive tasks: is
/// audio transmission allowed, is audio reception allowed. Flipped by
/// `Anvil::hold` and by inbound re-INVITE handling.
pub(crate) struct HoldState {
    pub(crate) send_enabled: AtomicBool,
    pub(crate) recv_enabled: AtomicBool,
}

impl HoldState {
    fn new(direction: MediaDirection) -> Arc<Self> {
        Arc::new(Self {
            send_enabled: AtomicBool::new(direction.sends()),
            recv_enabled: AtomicBool::new(direction.receives()),
        })
    }

    pub(crate) fn apply(&self, direction: MediaDirection) {
        self.send_enabled
            .store(direction.sends(), Ordering::Relaxed);
        self.recv_enabled
            .store(direction.receives(), Ordering::Relaxed);
    }
}

/// Parse the `a=sendrecv` / `a=sendonly` / `a=recvonly` / `a=inactive`
/// attribute from the first audio media description. Defaults to
/// `Sendrecv` if no direction attribute is present.
///
/// Hand-parses rather than going through `sip_sdp::parse::parse_sdp`: that
/// parser has a bug where property attributes (a= without a value) that
/// appear before a later value attribute (a=name:value) get merged into
/// the next attribute's name because its `take_till` doesn't stop on
/// newlines. Line-by-line scanning is tiny here and sidesteps the issue.
pub(crate) fn extract_direction(body: &[u8]) -> MediaDirection {
    let Ok(text) = std::str::from_utf8(body) else {
        return MediaDirection::Sendrecv;
    };

    let mut in_audio_media = false;
    for line in text.lines() {
        let line = line.trim_end();
        if let Some(rest) = line.strip_prefix("m=") {
            in_audio_media = rest.starts_with("audio ");
            continue;
        }
        if !in_audio_media {
            continue;
        }
        // RFC 4566 direction attributes are Property form: `a=<name>` with
        // no `:<value>`.
        let Some(attr) = line.strip_prefix("a=") else {
            continue;
        };
        match attr.trim() {
            "sendrecv" => return MediaDirection::Sendrecv,
            "sendonly" => return MediaDirection::Sendonly,
            "recvonly" => return MediaDirection::Recvonly,
            "inactive" => return MediaDirection::Inactive,
            _ => {}
        }
    }
    MediaDirection::Sendrecv
}

/// Dynamic RTP payload type for RFC 2833 telephone-event. Negotiated
/// implicitly via the fmtp:101 line in our SDP; we don't parse alternatives
/// from the peer's answer because every SIP endpoint in the last 15 years
/// uses 101.
const DTMF_PAYLOAD_TYPE: u8 = 101;

/// RTP clock rate used for the DTMF rtpmap regardless of audio codec
/// (RFC 4733 §2.1 mandates 8 kHz for telephone-event).
const DTMF_CLOCK_RATE: u32 = 8000;

/// Static info about a codec: RTP payload type and clock, PCM sample rate.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CodecSpec {
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
            Codec::Pcmu => Self {
                payload_type: 0,
                rtp_clock_rate: 8000,
                sample_rate: 8000,
            },
            Codec::Pcma => Self {
                payload_type: 8,
                rtp_clock_rate: 8000,
                sample_rate: 8000,
            },
            Codec::G722 => Self {
                payload_type: 9,
                rtp_clock_rate: 8000,
                sample_rate: 16000,
            },
            // Opus arrives with P2-M3. Slot reserved so sdp_offer_codecs
            // below compiles exhaustively, but start_pipeline will reject it
            // until the driver exists.
            Codec::Opus => Self {
                payload_type: 111,
                rtp_clock_rate: 48000,
                sample_rate: 48000,
            },
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

/// Codec plugin — encodes PCM-16 frames to the RTP payload format, decodes
/// them back. G.711 implementations are stateless; G.722 is stateful.
trait CodecDriver: Send {
    fn spec(&self) -> CodecSpec;
    fn encode(&mut self, pcm: &[i16]) -> Vec<u8>;
    fn decode(&mut self, bytes: &[u8]) -> Vec<i16>;
}

struct PcmuDriver;
impl CodecDriver for PcmuDriver {
    fn spec(&self) -> CodecSpec {
        CodecSpec::for_codec(Codec::Pcmu)
    }
    fn encode(&mut self, pcm: &[i16]) -> Vec<u8> {
        pcm.iter().map(|&s| g711::encode_ulaw(s)).collect()
    }
    fn decode(&mut self, bytes: &[u8]) -> Vec<i16> {
        bytes.iter().map(|&b| g711::decode_ulaw(b)).collect()
    }
}

struct PcmaDriver;
impl CodecDriver for PcmaDriver {
    fn spec(&self) -> CodecSpec {
        CodecSpec::for_codec(Codec::Pcma)
    }
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
    fn spec(&self) -> CodecSpec {
        CodecSpec::for_codec(Codec::Opus)
    }

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
    fn spec(&self) -> CodecSpec {
        CodecSpec::for_codec(Codec::G722)
    }
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
    direction: MediaDirection,
    crypto: Option<&crate::srtp::Crypto>,
) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let session_id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string());

    let mut media = sip_sdp::MediaDescription::audio(rtp_port)
        .with_direction(direction.as_sdp_attr())
        .expect("valid direction");
    for codec in codecs {
        let spec = CodecSpec::for_codec(*codec);
        media = media
            .add_format(spec.payload_type)
            .expect("valid format")
            .add_rtpmap(
                spec.payload_type,
                rtpmap_name(*codec),
                spec.rtp_clock_rate,
                None,
            )
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
    // SDES-SRTP (RFC 4568): the secure profile and our key.
    if let Some(crypto) = crypto {
        media.protocol = sip_sdp::Protocol::RtpSavp;
        media = media
            .add_attribute("crypto", &crypto.attribute())
            .expect("valid attribute");
    }

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

fn codec_from_pt(
    pt: u8,
    rtpmaps: &std::collections::HashMap<u8, sip_sdp::RtpMap>,
) -> Option<Codec> {
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
    srtp: Option<forge_rtp::srtp::SrtpContext>,
) -> Result<MediaPipeline, AnvilError> {
    // One context for both directions: it holds our key for what we send
    // and theirs for what we receive.
    let srtp = srtp.map(|ctx| Arc::new(parking_lot::Mutex::new(ctx)));
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
    let hold = HoldState::new(MediaDirection::Sendrecv);

    // Channel for outbound DTMF requests. Keep a modest buffer — humans type
    // slowly, but an IVR script might emit a burst. 16 is plenty.
    let (dtmf_tx, mut dtmf_rx) = mpsc::channel::<char>(16);

    // Send task: pull frames from the mic, encode, RTP-wrap, send.
    // Also interleaves RFC 2833 DTMF packets when the app queues a digit.
    let sock_send = Arc::clone(&rtp_socket);
    let metrics_send = Arc::clone(&metrics);
    let hold_send = Arc::clone(&hold);
    let srtp_send = srtp.clone();
    let send_task = tokio::spawn(async move {
        let mut driver = send_driver;
        let mut seq = initial_seq;
        let mut ts = initial_ts;
        let mut sent_any = false;

        loop {
            tokio::select! {
                // Prefer pending DTMF over a captured frame so a queued digit
                // goes out inside the same 20 ms tick as it was requested.
                biased;

                Some(ch) = dtmf_rx.recv() => {
                    match send_dtmf_digit(
                        ch,
                        &sock_send,
                        remote_rtp,
                        &metrics_send,
                        &mut seq,
                        ts,
                        ssrc,
                        send_spec.rtp_clock_rate,
                        srtp_send.as_ref(),
                    ).await {
                        Ok(advanced) => {
                            ts = ts.wrapping_add(advanced);
                        }
                        Err(e) => tracing::warn!(%e, "DTMF send failed"),
                    }
                    sent_any = true;
                }

                maybe_frame = capture.next_frame() => {
                    let Some(frame) = maybe_frame else { break };
                    if frame.samples.len() != samples_per_frame {
                        tracing::warn!(
                            got = frame.samples.len(),
                            want = samples_per_frame,
                            codec = ?codec,
                            "capture frame size mismatch; skipping"
                        );
                        continue;
                    }

                    // While on hold, still drain capture (keeps the audio
                    // thread from backing up) but don't transmit. RTP seq
                    // and timestamp continue advancing so the peer's
                    // jitter / loss calc doesn't blow up when we resume.
                    if !hold_send.send_enabled.load(Ordering::Relaxed) {
                        seq = seq.wrapping_add(1);
                        ts = ts.wrapping_add(ts_per_frame);
                        continue;
                    }

                    let payload = driver.encode(&frame.samples);
                    // RFC 3551: marker bit on first packet of a talkspurt.
                    let marker = !sent_any;
                    let packet = RtpPacket::build(
                        payload_type,
                        seq,
                        ts,
                        ssrc,
                        Bytes::from(payload),
                        marker,
                    );
                    let Some(bytes) = protect(srtp_send.as_ref(), packet.to_bytes()) else {
                        seq = seq.wrapping_add(1);
                        ts = ts.wrapping_add(ts_per_frame);
                        continue;
                    };
                    match sock_send.send_to(&bytes, remote_rtp).await {
                        Ok(_) => metrics_send.on_rtp_tx(bytes.len()),
                        Err(e) => tracing::warn!(%e, %remote_rtp, "rtp send failed"),
                    }

                    seq = seq.wrapping_add(1);
                    ts = ts.wrapping_add(ts_per_frame);
                    sent_any = true;
                }
            }
        }

        tracing::debug!(packets_sent = sent_any, "send task exiting");
    });

    // Receive task: read UDP, parse RTP, decode, push to speaker.
    // Also picks RFC 2833 (PT 101) packets out of the stream and publishes
    // DtmfReceived events on the end-of-digit transition.
    let sock_recv = Arc::clone(&rtp_socket);
    let recv_pt = recv_spec.payload_type;
    let recv_fmt = recv_spec.audio_format();
    let metrics_recv = Arc::clone(&metrics);
    let hold_recv = Arc::clone(&hold);
    let srtp_recv = srtp.clone();
    let dtmf_events = stats_sink.as_ref().map(|s| (s.call, s.events.clone()));
    let recv_task = tokio::spawn(async move {
        let mut driver = recv_driver;
        let mut dtmf_detector = Rfc2833Detector::new(DTMF_CLOCK_RATE);
        let mut dtmf_dedup = DtmfDeduplicator::new();
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

            let data = match &srtp_recv {
                // Anything that does not authenticate under their key is
                // dropped (RFC 3711 §3.3).
                Some(ctx) => match ctx.lock().unprotect_rtp(&buf[..len]) {
                    Ok(plain) => Bytes::from(plain),
                    Err(e) => {
                        tracing::debug!(%e, "srtp unprotect failed; dropping");
                        continue;
                    }
                },
                None => Bytes::copy_from_slice(&buf[..len]),
            };
            let packet = match RtpPacket::parse(data) {
                Ok(p) => p,
                Err(e) => {
                    tracing::debug!(%e, "rtp parse failed; ignoring");
                    continue;
                }
            };

            let pt = packet.header.payload_type();
            if pt == DTMF_PAYLOAD_TYPE {
                handle_inbound_dtmf(
                    &mut dtmf_detector,
                    &mut dtmf_dedup,
                    &packet,
                    dtmf_events.as_ref(),
                )
                .await;
                continue;
            }

            if pt != recv_pt {
                tracing::debug!(pt, want = recv_pt, "unexpected payload type; ignoring");
                continue;
            }

            metrics_recv.on_rtp_rx(
                packet.header.sequence_number,
                packet.header.timestamp,
                len,
                arrival,
            );

            // When held, still decode (keeps the codec decoder state
            // valid for when we resume) but swallow the frame instead of
            // pushing to playback. Metrics were already updated above so
            // a "held" call still shows accurate recv_kbps.
            let samples = driver.decode(&packet.payload);
            if !hold_recv.recv_enabled.load(Ordering::Relaxed) {
                continue;
            }

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
                let elapsed = now
                    .saturating_duration_since(prev_wall)
                    .as_secs_f32()
                    .max(0.001);
                prev_wall = now;

                let bytes_tx = metrics.bytes_tx.load(Ordering::Relaxed);
                let bytes_rx = metrics.bytes_rx.load(Ordering::Relaxed);
                let send_kbps =
                    (bytes_tx.saturating_sub(prev_bytes_tx) as f32 * 8.0) / elapsed / 1000.0;
                let recv_kbps =
                    (bytes_rx.saturating_sub(prev_bytes_rx) as f32 * 8.0) / elapsed / 1000.0;
                prev_bytes_tx = bytes_tx;
                prev_bytes_rx = bytes_rx;

                let jitter_ticks = metrics.jitter.lock().current;
                let jitter_ms = jitter_ticks as f32 * 1000.0 / clock as f32;

                let (expected, received) = {
                    let s = metrics.seq.lock();
                    let expected = match s.first {
                        Some(first) => {
                            let extended = (s.cycles as i64) * 65_536 + s.highest as i64;
                            (extended - first as i64 + 1).max(0) as u64
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
                    .send(crate::event::Event::MediaStats {
                        call: sink.call,
                        stats,
                    })
                    .await;
            }
        })
    };

    Ok(MediaPipeline {
        send_task,
        recv_task,
        stats_task,
        dtmf_tx,
        hold,
    })
}

/// Emit one DTMF digit as an RFC 2833 event burst. Returns the number of
/// RTP timestamp ticks the audio clock should advance past to account for
/// the gap this digit consumed.
///
/// Structure:
/// - 1 start packet (marker bit set, duration 0)
/// - 3 continuation packets (duration accumulates)
/// - 3 identical end packets (end bit set, final duration)
///
/// The 7 packets at 20 ms spacing = 140 ms total per digit, which is well
/// within RFC 4733's recommendation (≥ 40 ms).
#[allow(clippy::too_many_arguments)] // one RFC 2833 event's whole context
async fn send_dtmf_digit(
    ch: char,
    socket: &UdpSocket,
    remote: SocketAddr,
    metrics: &MediaMetrics,
    seq: &mut u16,
    start_ts: u32,
    ssrc: u32,
    rtp_clock_rate: u32,
    srtp: Option<&Arc<parking_lot::Mutex<forge_rtp::srtp::SrtpContext>>>,
) -> anyhow::Result<u32> {
    let digit = DtmfDigit::from_char(ch)
        .map_err(|e| anyhow::anyhow!("invalid DTMF digit {:?}: {:?}", ch, e))?;

    // Generator uses a generic sample rate in samples/sec; the "packet
    // interval" advances the duration field by `rate * 20 / 1000` ticks per
    // call. Spec pins the rtpmap rate to 8 kHz for telephone-event, so we
    // use that uniformly.
    let mut gen = Rfc2833Generator::new(DTMF_CLOCK_RATE, FRAME_MS);

    let mut events: Vec<forge_dtmf::rfc2833::Rfc2833Event> = Vec::new();
    events.push(gen.start_digit(digit));
    for _ in 0..3 {
        if let Some(e) = gen.continue_digit() {
            events.push(e);
        }
    }
    if let Some(end_set) = gen.end_digit() {
        events.extend(end_set);
    }

    let frame_period = std::time::Duration::from_millis(FRAME_MS as u64);
    for (i, event) in events.iter().enumerate() {
        let is_first = i == 0;
        // End packets are sent back-to-back, not on the 20 ms beat.
        let is_end_burst = event.is_end();

        let payload = Bytes::from(event.to_bytes());
        let packet = RtpPacket::build(DTMF_PAYLOAD_TYPE, *seq, start_ts, ssrc, payload, is_first);
        let Some(bytes) = protect(srtp, packet.to_bytes()) else {
            *seq = seq.wrapping_add(1);
            continue;
        };
        match socket.send_to(&bytes, remote).await {
            Ok(_) => metrics.on_rtp_tx(bytes.len()),
            Err(e) => tracing::warn!(%e, "DTMF send_to failed"),
        }
        *seq = seq.wrapping_add(1);

        if !is_end_burst && i + 1 < events.len() {
            tokio::time::sleep(frame_period).await;
        }
    }

    // Account for the time the audio clock would have advanced during the
    // digit's playout. `continues + start = 4 frames` at the audio rate.
    // (End packets share the same timestamp, so they don't count.)
    let audio_frames_consumed = 4u32;
    Ok(rtp_clock_rate * FRAME_MS / 1000 * audio_frames_consumed)
}

async fn handle_inbound_dtmf(
    detector: &mut Rfc2833Detector,
    dedup: &mut DtmfDeduplicator,
    packet: &RtpPacket,
    dtmf_events: Option<&(crate::CallId, mpsc::Sender<crate::event::Event>)>,
) {
    let events = match detector.process_with_timestamp(&packet.payload, packet.header.timestamp) {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!(%e, "RFC 2833 parse failed");
            return;
        }
    };
    let Some((call, tx)) = dtmf_events else {
        return;
    };
    for event in events {
        // Surface only the first End event per digit. RFC 2833 §3.3
        // recommends three identical end packets for reliability, and we
        // re-parse each as a fresh End. DtmfDeduplicator suppresses the
        // repeats within its 100 ms window.
        if event.event_type != forge_dtmf::DtmfEventType::End {
            continue;
        }
        if !dedup.should_publish(&event) {
            continue;
        }
        let digit_ch = event.digit.to_string().chars().next().unwrap_or('?');
        let _ = tx
            .send(crate::event::Event::DtmfReceived {
                call: *call,
                digit: digit_ch,
            })
            .await;
    }
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
    let conn = media.connection.as_ref().or(sdp.connection.as_ref())?;
    let ip: std::net::IpAddr = conn.connection_address.parse().ok()?;

    Some(SocketAddr::new(ip, port))
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

    fn received(metrics: &MediaMetrics, seqs: &[u16]) {
        for &seq in seqs {
            metrics.on_rtp_rx(seq, 0, 160, Instant::now());
        }
    }

    #[test]
    fn a_new_stream_restarts_the_count() {
        let m = MediaMetrics::new(8000);
        received(&m, &[40_000, 40_001, 40_002]);
        // A transfer: the far end's numbering starts somewhere else.
        received(&m, &[100, 101]);
        let s = m.seq.lock();
        assert_eq!(
            (s.first, s.highest, s.cycles, s.received),
            (Some(100), 101, 0, 2)
        );
    }

    #[test]
    fn wraparound_and_reordering_are_one_stream() {
        let m = MediaMetrics::new(8000);
        received(&m, &[65_534, 65_535, 0, 65_533, 1]);
        let s = m.seq.lock();
        assert_eq!(
            (s.first, s.highest, s.cycles, s.received),
            (Some(65_534), 1, 1, 5)
        );
    }

    use crate::audio::{AudioFormat, AudioFrame, AudioSink, AudioSource};
    use crate::error::AnvilError;

    const FRAME_MS: u32 = 20;

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
            let sum_sq: f64 = frame.samples.iter().map(|&s| (s as f64) * (s as f64)).sum();
            let rms = (sum_sq / n).sqrt() as u64;
            self.peak_rms_q.fetch_max(rms, Ordering::Relaxed);
            Ok(())
        }
    }

    async fn round_trip_for(codec: Codec, sample_rate: u32) {
        let (frames_a, rms_a, frames_b, rms_b) = round_trip_keyed(codec, sample_rate, None).await;
        assert!(frames_a >= 30, "{codec:?}: A frames={frames_a}");
        assert!(frames_b >= 30, "{codec:?}: B frames={frames_b}");
        assert!(rms_a > 3000, "{codec:?}: A rms={rms_a} (silent?)");
        assert!(rms_b > 3000, "{codec:?}: B rms={rms_b} (silent?)");
    }

    /// Two pipelines trading a tone for a second, each with its SRTP
    /// context when given; the peak RMS each heard.
    async fn round_trip_keyed(
        codec: Codec,
        sample_rate: u32,
        srtp: Option<(forge_rtp::srtp::SrtpContext, forge_rtp::srtp::SrtpContext)>,
    ) -> (u64, u64, u64, u64) {
        let (srtp_a, srtp_b) = match srtp {
            Some((a, b)) => (Some(a), Some(b)),
            None => (None, None),
        };
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
            srtp_a,
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
            srtp_b,
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
        (frames_a, rms_a, frames_b, rms_b)
    }

    /// Keys from an SDES offer and its answer: the caller's and callee's
    /// contexts.
    fn sdes_pair() -> (crate::srtp::Crypto, crate::srtp::Crypto) {
        let offer = crate::srtp::Crypto::generate(1, crate::srtp::Suite::AesCm128HmacSha1_80);
        let (answer, _) = crate::srtp::answer_to(std::slice::from_ref(&offer)).unwrap();
        (offer, answer)
    }

    /// SDES-SRTP both ways: the audio survives encryption.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn rtp_round_trip_srtp() {
        let (offer, answer) = sdes_pair();
        let a = crate::srtp::context(&offer, &answer).unwrap();
        let b = crate::srtp::context(&answer, &offer).unwrap();
        let (frames_a, rms_a, frames_b, rms_b) =
            round_trip_keyed(Codec::Pcmu, 8000, Some((a, b))).await;
        assert!(
            frames_a >= 30 && frames_b >= 30,
            "A {frames_a}, B {frames_b} frames"
        );
        assert!(rms_a > 3000 && rms_b > 3000, "A {rms_a}, B {rms_b}");
    }

    /// A side keyed wrong hears nothing: every packet fails to authenticate
    /// and is dropped, rather than played as noise.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn srtp_under_the_wrong_key_is_silence() {
        let (offer, answer) = sdes_pair();
        let (_, stranger) = sdes_pair();
        let a = crate::srtp::context(&offer, &answer).unwrap();
        let b = crate::srtp::context(&answer, &stranger).unwrap();
        let (frames_a, rms_a, frames_b, _) =
            round_trip_keyed(Codec::Pcmu, 8000, Some((a, b))).await;
        assert!(
            frames_a >= 30 && rms_a > 3000,
            "A still hears B: {frames_a} frames, {rms_a}"
        );
        assert_eq!(frames_b, 0, "B played packets it could not authenticate");
    }

    #[test]
    fn an_encrypted_offer_is_savp_with_our_key() {
        let (offer, _) = sdes_pair();
        let body = super::build_sdp(
            &[Codec::Pcmu],
            "alice",
            "127.0.0.1",
            5000,
            MediaDirection::Sendrecv,
            Some(&offer),
        );
        assert!(body.contains("m=audio 5000 RTP/SAVP 0 101"), "{body}");
        assert!(
            body.contains(&format!("a=crypto:{}", offer.attribute())),
            "{body}"
        );
        assert_eq!(crate::srtp::offered(body.as_bytes()), vec![offer]);
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

    #[test]
    fn extract_direction_finds_sendonly() {
        let sdp = b"v=0\r\n\
                    o=alice 1 0 IN IP4 127.0.0.1\r\n\
                    s=anvil\r\n\
                    c=IN IP4 127.0.0.1\r\n\
                    t=0 0\r\n\
                    m=audio 5000 RTP/AVP 9\r\n\
                    a=sendonly\r\n\
                    a=rtpmap:9 G722/8000\r\n";
        assert_eq!(super::extract_direction(sdp), MediaDirection::Sendonly);
    }

    #[test]
    fn extract_direction_defaults_sendrecv() {
        let sdp = b"v=0\r\n\
                    o=alice 1 0 IN IP4 127.0.0.1\r\n\
                    s=anvil\r\n\
                    c=IN IP4 127.0.0.1\r\n\
                    t=0 0\r\n\
                    m=audio 5000 RTP/AVP 0\r\n\
                    a=rtpmap:0 PCMU/8000\r\n";
        assert_eq!(super::extract_direction(sdp), MediaDirection::Sendrecv);
    }

    #[test]
    fn extract_direction_roundtrips_through_build_sdp() {
        for dir in [
            MediaDirection::Sendrecv,
            MediaDirection::Sendonly,
            MediaDirection::Recvonly,
            MediaDirection::Inactive,
        ] {
            let body = super::build_sdp(&[Codec::G722], "alice", "127.0.0.1", 5000, dir, None);
            assert_eq!(super::extract_direction(body.as_bytes()), dir, "{dir:?}");
        }
    }

    /// Opus round-trip at 48 kHz fullband.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn rtp_round_trip_opus() {
        round_trip_for(Codec::Opus, 48000).await;
    }
}
