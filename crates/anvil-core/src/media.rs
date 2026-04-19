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
use forge_rtp::rtp::RtpPacket;
use rand::{thread_rng, RngCore};
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;

use crate::audio::{AudioFormat, AudioFrame, AudioSink, AudioSource};
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

/// 20 ms frame size at 8 kHz mono: 160 samples.
const FRAME_SAMPLES: usize = 160;

/// Only G.711 µ-law for Phase 1 M3.
const PAYLOAD_TYPE_PCMU: u8 = 0;

/// Standard G.711 audio format.
pub(crate) fn g711_format() -> AudioFormat {
    AudioFormat {
        sample_rate: 8000,
        channels: 1,
        frame_ms: 20,
    }
}

/// Start the media pipeline for an established call.
pub(crate) fn start_pipeline(
    rtp_socket: Arc<UdpSocket>,
    remote_rtp: SocketAddr,
    mut capture: Box<dyn AudioSource>,
    mut playback: Box<dyn AudioSink>,
) -> Result<MediaPipeline, AnvilError> {
    let mut rng = thread_rng();
    let ssrc: u32 = rng.next_u32();
    let initial_seq: u16 = rng.next_u32() as u16;
    let initial_ts: u32 = rng.next_u32();

    // Send task: pull frames from the mic, encode, RTP-wrap, send.
    let sock_send = Arc::clone(&rtp_socket);
    let send_task = tokio::spawn(async move {
        let mut seq = initial_seq;
        let mut ts = initial_ts;
        let mut sent_any = false;

        while let Some(frame) = capture.next_frame().await {
            if frame.samples.len() != FRAME_SAMPLES {
                tracing::warn!(
                    got = frame.samples.len(),
                    want = FRAME_SAMPLES,
                    "capture frame size mismatch; skipping"
                );
                continue;
            }

            let mut payload = Vec::with_capacity(FRAME_SAMPLES);
            for &sample in &frame.samples {
                payload.push(g711::encode_ulaw(sample));
            }

            // RFC 3551: marker bit on first packet of a talkspurt. We don't do
            // VAD in M3, so set marker only on the very first packet.
            let marker = !sent_any;
            let packet = RtpPacket::build(
                PAYLOAD_TYPE_PCMU,
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
            ts = ts.wrapping_add(FRAME_SAMPLES as u32);
            sent_any = true;
        }

        tracing::debug!(packets_sent = sent_any, "send task exiting");
    });

    // Receive task: read UDP, parse RTP, decode, push to speaker.
    let sock_recv = Arc::clone(&rtp_socket);
    let recv_task = tokio::spawn(async move {
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

            if packet.header.payload_type() != PAYLOAD_TYPE_PCMU {
                tracing::debug!(
                    pt = packet.header.payload_type(),
                    "unexpected payload type; ignoring"
                );
                continue;
            }

            let samples: Vec<i16> = packet
                .payload
                .iter()
                .map(|&b| g711::decode_ulaw(b))
                .collect();

            let frame = AudioFrame {
                samples,
                format: g711_format(),
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
        samples_per_frame: usize,
        phase: f32,
        phase_step: f32,
        next_tick: Option<tokio::time::Instant>,
    }

    impl ToneSource {
        fn new(freq_hz: f32) -> Self {
            let samples_per_frame = (SAMPLE_RATE * FRAME_MS / 1000) as usize;
            Self {
                samples_per_frame,
                phase: 0.0,
                phase_step: std::f32::consts::TAU * freq_hz / SAMPLE_RATE as f32,
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
                    sample_rate: SAMPLE_RATE,
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

    /// Two RTP pipelines trade µ-law audio over loopback for one second.
    /// Exit criterion: each side's receive-side RMS is materially above noise.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn rtp_round_trip_preserves_audio() {
        let loopback = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let sock_a = Arc::new(UdpSocket::bind(SocketAddr::new(loopback, 0)).await.unwrap());
        let sock_b = Arc::new(UdpSocket::bind(SocketAddr::new(loopback, 0)).await.unwrap());
        let addr_a = sock_a.local_addr().unwrap();
        let addr_b = sock_b.local_addr().unwrap();

        let frames_a = Arc::new(AtomicU64::new(0));
        let rms_a = Arc::new(AtomicU64::new(0));
        let frames_b = Arc::new(AtomicU64::new(0));
        let rms_b = Arc::new(AtomicU64::new(0));

        // A ⇆ B, different tone frequencies so a sanity-check on the receiver
        // side could tell them apart if needed.
        let pipe_a = start_pipeline(
            Arc::clone(&sock_a),
            addr_b,
            Box::new(ToneSource::new(440.0)),
            Box::new(RmsSink {
                frames: Arc::clone(&frames_a),
                peak_rms_q: Arc::clone(&rms_a),
            }),
        )
        .expect("pipeline A");

        let pipe_b = start_pipeline(
            Arc::clone(&sock_b),
            addr_a,
            Box::new(ToneSource::new(660.0)),
            Box::new(RmsSink {
                frames: Arc::clone(&frames_b),
                peak_rms_q: Arc::clone(&rms_b),
            }),
        )
        .expect("pipeline B");

        // Hold the call for 1 s (≈50 frames per side).
        tokio::time::sleep(Duration::from_millis(1000)).await;

        drop(pipe_a);
        drop(pipe_b);

        let frames_a = frames_a.load(Ordering::Relaxed);
        let frames_b = frames_b.load(Ordering::Relaxed);
        let rms_a = rms_a.load(Ordering::Relaxed);
        let rms_b = rms_b.load(Ordering::Relaxed);

        println!("A received {frames_a} frames, peak RMS={rms_a}");
        println!("B received {frames_b} frames, peak RMS={rms_b}");

        // Expect ≥ 30 frames per side (out of ~50 sent; loopback shouldn't
        // lose anywhere near half).
        assert!(frames_a >= 30, "A got too few frames: {frames_a}");
        assert!(frames_b >= 30, "B got too few frames: {frames_b}");

        // RMS of a 0.3·i16 amplitude sine wave is ≈ 0.3 · 32768 / √2 ≈ 6951.
        // µ-law quantisation halves dynamic range in rough terms; require a
        // floor well above silence.
        assert!(rms_a > 3000, "A RMS too low — silent path? {rms_a}");
        assert!(rms_b > 3000, "B RMS too low — silent path? {rms_b}");
    }
}

