//! Per-call RTP send/receive pipeline.
//!
//! Phase 1 M3 scope: one audio stream per call, G.711 µ-law (PT 0) at 8 kHz,
//! 20 ms frames (160 samples per packet), full-duplex. No jitter buffer yet —
//! received packets feed the audio sink in arrival order; Phase 2 adds a
//! proper reorder / loss-conceal buffer.

use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use forge_codecs::g711;
use forge_codecs::g722::{G722BitRate, G722Decoder, G722Encoder};
use forge_rtp::rtp::RtpPacket;
use rand::{thread_rng, RngCore};
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;

use crate::audio::{AudioFormat, AudioFrame, AudioSink, AudioSource};
use crate::config::Codec;
use crate::error::AnvilError;

/// Handles to the two tasks driving a single call's media. Dropped (and
/// aborted) when the call ends.
pub(crate) struct MediaPipeline {
    send_task: JoinHandle<()>,
    recv_task: JoinHandle<()>,
}

impl MediaPipeline {
    pub(crate) fn abort(&self) {
        self.send_task.abort();
        self.recv_task.abort();
    }
}

impl Drop for MediaPipeline {
    fn drop(&mut self) {
        self.abort();
    }
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

/// Build a codec driver by name. Returns `None` for codecs we have no
/// implementation for yet (Opus).
fn make_driver(codec: Codec) -> Option<Box<dyn CodecDriver>> {
    Some(match codec {
        Codec::Pcmu => Box::new(PcmuDriver),
        Codec::Pcma => Box::new(PcmaDriver),
        Codec::G722 => Box::new(G722DriverImpl::new()),
        Codec::Opus => return None,
    })
}

/// Codecs we support today, in the order we prefer to offer them. Opus is
/// listed but rejected by `make_driver` until the P2-M3 milestone wires it up.
pub(crate) fn supported_codecs() -> &'static [Codec] {
    // G.722 first — wideband improves quality noticeably for voice and costs
    // roughly the same bandwidth as PCMU. PCMU / PCMA follow as mandatory
    // fallbacks per RFC 3551.
    &[Codec::G722, Codec::Pcmu, Codec::Pcma]
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
pub(crate) fn start_pipeline(
    codec: Codec,
    rtp_socket: Arc<UdpSocket>,
    remote_rtp: SocketAddr,
    mut capture: Box<dyn AudioSource>,
    mut playback: Box<dyn AudioSink>,
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

    // Send task: pull frames from the mic, encode, RTP-wrap, send.
    let sock_send = Arc::clone(&rtp_socket);
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

            if let Err(e) = sock_send.send_to(&bytes, remote_rtp).await {
                tracing::warn!(%e, %remote_rtp, "rtp send failed");
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

    Ok(MediaPipeline {
        send_task,
        recv_task,
    })
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

}

