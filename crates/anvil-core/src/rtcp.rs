//! RTCP for a call (RFC 3550 §6): a sender or receiver report every few
//! seconds with a reception report block on what arrives, an SDES CNAME,
//! SRTCP when the call is SRTP (RFC 3711 §3.4); the far end's reports read
//! for what it receives of ours and the round trip.
//!
//! RTCP runs on the port above RTP. FCP's media relay keeps the same pair
//! and passes each side's RTCP to the other (re-protected under that leg's
//! keys), so the reports are the far phone's and the round trip is end to
//! end through the relay.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use forge_rtp::rtcp::{
    ntp, ReceiverReport, ReceptionReportBlock, RtcpPacket, SdesItem, SourceDescription,
};
use forge_rtp::{RttTracker, SenderStats, SourceStats};
use parking_lot::Mutex;
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;

/// How often a report is sent (RFC 3550 §6.2's recommended minimum).
pub(crate) const REPORT_INTERVAL: Duration = Duration::from_secs(5);

/// The first report, soon after media starts, so a short call still gets
/// a round trip.
const FIRST_REPORT: Duration = Duration::from_secs(1);

/// How long round-trip samples are averaged over.
const RTT_WINDOW: Duration = Duration::from_secs(30);

/// SDES item type CNAME (RFC 3550 §6.5.1).
const SDES_CNAME: u8 = 1;

/// What the far end last reported receiving of ours.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RemoteReport {
    /// Of our packets in its last interval, the share lost, in percent.
    pub fraction_lost_pct: f32,
    /// Our packets it has lost since the call began.
    pub cumulative_lost: i32,
    /// Its jitter estimate on our stream, in milliseconds.
    pub jitter_ms: f32,
}

/// A call's RTCP state, fed by its send and receive paths.
pub(crate) struct RtcpState {
    ours: SenderStats,
    /// The far end's stream, made when its first packet arrives and anew
    /// when its SSRC changes (a transfer, a re-anchored leg).
    theirs: Option<SourceStats>,
    clock_rate: u32,
    rtt: RttTracker,
    remote: Option<RemoteReport>,
    cname: String,
    /// Sent packets at the last report: a report says SR only when we sent
    /// since (RFC 3550 §6.4).
    sent_at_last_report: u32,
    pub(crate) reports_sent: u64,
    pub(crate) reports_received: u64,
    /// Where the far end's RTCP comes from: where ours goes (symmetric,
    /// past a NAT that moved the port).
    latched: Option<SocketAddr>,
}

impl RtcpState {
    pub(crate) fn new(ssrc: u32, clock_rate: u32, cname: String) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            ours: SenderStats::new(ssrc, clock_rate),
            theirs: None,
            clock_rate: clock_rate.max(1),
            rtt: RttTracker::new(RTT_WINDOW),
            remote: None,
            cname,
            sent_at_last_report: 0,
            reports_sent: 0,
            reports_received: 0,
            latched: None,
        }))
    }

    /// One of our RTP packets sent, carrying `payload_len` octets.
    pub(crate) fn on_sent(&mut self, rtp_timestamp: u32, payload_len: usize, now: Instant) {
        self.ours.on_send(rtp_timestamp, payload_len, now);
    }

    /// One of the far end's RTP packets received.
    pub(crate) fn on_received(&mut self, ssrc: u32, seq: u16, rtp_timestamp: u32, now: Instant) {
        if self.theirs.as_ref().map(SourceStats::ssrc) != Some(ssrc) {
            self.theirs = Some(SourceStats::new(ssrc, self.clock_rate));
        }
        if let Some(theirs) = self.theirs.as_mut() {
            theirs.on_packet(seq, rtp_timestamp, now);
        }
    }

    /// What the far end last reported of ours.
    pub(crate) fn remote(&self) -> Option<RemoteReport> {
        self.remote
    }

    /// The round trip, averaged over the last half minute.
    pub(crate) fn rtt_ms(&mut self) -> Option<f32> {
        self.rtt.mean_ms()
    }

    /// The compound packet to send now: SR when we sent since the last
    /// report, else RR; a block on the far end's stream once it has sent;
    /// then SDES CNAME (RFC 3550 §6.1).
    pub(crate) fn report(&mut self, now: Instant) -> Vec<u8> {
        let block = self
            .theirs
            .as_mut()
            .filter(|t| t.is_started())
            .map(|t| t.report_block(now));
        let sending = self.ours.has_sent() && self.ours.packets_sent() != self.sent_at_last_report;
        self.sent_at_last_report = self.ours.packets_sent();
        let mut packet = if sending {
            let mut sr = self.ours.sender_report(now);
            self.rtt.record_outgoing_sr(
                (u64::from(sr.ntp_timestamp_msw) << 32) | u64::from(sr.ntp_timestamp_lsw),
            );
            if let Some(block) = block {
                sr.add_report_block(block);
            }
            RtcpPacket::SenderReport(sr).to_bytes()
        } else {
            let mut rr = ReceiverReport::new(self.ours.ssrc());
            if let Some(block) = block {
                rr.add_report_block(block);
            }
            RtcpPacket::ReceiverReport(rr).to_bytes()
        };
        let mut sdes = SourceDescription::new();
        sdes.add_chunk(
            self.ours.ssrc(),
            vec![SdesItem::new(SDES_CNAME, self.cname.clone())],
        );
        packet.extend_from_slice(&RtcpPacket::SourceDescription(sdes).to_bytes());
        self.reports_sent += 1;
        packet
    }

    /// A compound packet from the far end, decrypted.
    pub(crate) fn on_report(&mut self, packet: &[u8], now: Instant) {
        let packets = RtcpPacket::parse_compound(packet);
        if packets.is_empty() {
            return;
        }
        self.reports_received += 1;
        for p in packets {
            let blocks = match p {
                RtcpPacket::SenderReport(sr) => {
                    if let Some(theirs) = self.theirs.as_mut() {
                        theirs.on_sender_report(sr.ntp_timestamp_msw, sr.ntp_timestamp_lsw, now);
                    }
                    sr.report_blocks
                }
                RtcpPacket::ReceiverReport(rr) => rr.report_blocks,
                _ => continue,
            };
            if let Some(block) = self.block_on_ours(&blocks) {
                self.remote = Some(RemoteReport {
                    fraction_lost_pct: f32::from(block.fraction_lost) * 100.0 / 256.0,
                    cumulative_lost: block.cumulative_lost,
                    jitter_ms: block.jitter as f32 * 1000.0 / self.clock_rate as f32,
                });
                self.rtt
                    .observe_incoming_rr(block.last_sr, block.delay_since_last_sr, ntp::now());
            }
        }
    }

    /// The block about our stream: by our SSRC, else the only one (a relay
    /// that renumbers what it forwards still reports on what we sent).
    fn block_on_ours<'a>(
        &self,
        blocks: &'a [ReceptionReportBlock],
    ) -> Option<&'a ReceptionReportBlock> {
        blocks
            .iter()
            .find(|b| b.ssrc == self.ours.ssrc())
            .or(match blocks {
                [only] => Some(only),
                _ => None,
            })
    }
}

/// The far end's RTCP address: the port above its RTP (RFC 3550 §11),
/// until its first report says where it is.
pub(crate) fn remote_rtcp_addr(remote_rtp: SocketAddr) -> SocketAddr {
    SocketAddr::new(remote_rtp.ip(), remote_rtp.port().wrapping_add(1))
}

type Srtp = Option<Arc<Mutex<forge_rtp::srtp::SrtpContext>>>;

/// The two RTCP tasks of a call: reports sent every
/// [`REPORT_INTERVAL`], and the far end's read.
pub(crate) fn start(
    socket: Arc<UdpSocket>,
    remote: SocketAddr,
    state: Arc<Mutex<RtcpState>>,
    srtp: Srtp,
    call: Option<u64>,
) -> (JoinHandle<()>, JoinHandle<()>) {
    let send = {
        let (socket, state, srtp) = (Arc::clone(&socket), Arc::clone(&state), srtp.clone());
        tokio::spawn(async move {
            tokio::time::sleep(FIRST_REPORT).await;
            let mut ticker = tokio::time::interval(REPORT_INTERVAL);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                let (packet, to) = {
                    let mut s = state.lock();
                    (s.report(Instant::now()), s.latched.unwrap_or(remote))
                };
                let packet = match &srtp {
                    None => packet,
                    Some(ctx) => match ctx.lock().protect_rtcp(&packet) {
                        Ok(sealed) => sealed,
                        Err(e) => {
                            tracing::warn!(?call, %e, "srtcp protect failed");
                            continue;
                        }
                    },
                };
                if let Err(e) = socket.send_to(&packet, to).await {
                    tracing::debug!(?call, %e, %to, "rtcp send failed");
                }
            }
        })
    };
    let recv = tokio::spawn(async move {
        let mut buf = vec![0u8; 1500];
        let mut first = true;
        loop {
            let (len, from) = match socket.recv_from(&mut buf).await {
                Ok(x) => x,
                // A refused datagram of ours (Windows): not this socket failing.
                Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => continue,
                Err(e) => {
                    tracing::debug!(?call, %e, "rtcp recv failed");
                    break;
                }
            };
            let plain = match &srtp {
                None => buf[..len].to_vec(),
                Some(ctx) => match ctx.lock().unprotect_rtcp(&buf[..len]) {
                    Ok(plain) => plain,
                    Err(e) => {
                        tracing::debug!(?call, %e, %from, "srtcp unprotect failed; dropping");
                        continue;
                    }
                },
            };
            if first {
                tracing::info!(?call, %from, "rtcp: first report received");
                first = false;
            }
            let mut s = state.lock();
            s.latched = Some(from);
            s.on_report(&plain, Instant::now());
        }
    });
    (send, recv)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sending(state: &Arc<Mutex<RtcpState>>, packets: u32) {
        let now = Instant::now();
        let mut s = state.lock();
        for i in 0..packets {
            s.on_sent(i * 160, 160, now);
        }
    }

    #[test]
    fn a_sender_reports_with_its_cname_and_a_block_on_what_it_receives() {
        let a = RtcpState::new(0x1111, 8000, "a@anvil".into());
        sending(&a, 10);
        {
            let mut s = a.lock();
            for seq in [1u16, 2, 4] {
                s.on_received(0x2222, seq, u32::from(seq) * 160, Instant::now());
            }
        }
        let packet = a.lock().report(Instant::now());
        let parsed = RtcpPacket::parse_compound(&packet);
        let RtcpPacket::SenderReport(sr) = &parsed[0] else {
            panic!("an SR first: {parsed:?}");
        };
        assert_eq!(sr.ssrc, 0x1111);
        assert_eq!(sr.sender_packet_count, 10);
        assert_eq!(sr.report_blocks.len(), 1);
        assert_eq!(sr.report_blocks[0].ssrc, 0x2222);
        assert_eq!(sr.report_blocks[0].cumulative_lost, 1, "seq 3 missing");
        assert!(
            matches!(&parsed[1], RtcpPacket::SourceDescription(_)),
            "{parsed:?}"
        );
        // Nothing sent since: a receiver report.
        let again = a.lock().report(Instant::now());
        assert!(matches!(
            RtcpPacket::parse_compound(&again)[0],
            RtcpPacket::ReceiverReport(_)
        ));
    }

    #[test]
    fn the_far_ends_report_on_ours_gives_its_loss_jitter_and_the_round_trip() {
        let a = RtcpState::new(0x1111, 8000, "a@anvil".into());
        let b = RtcpState::new(0x2222, 8000, "b@anvil".into());
        sending(&a, 100);
        let sr = a.lock().report(Instant::now());
        // B hears A's stream with a gap, then hears A's SR.
        {
            let mut s = b.lock();
            for seq in (1u16..=100).filter(|s| s % 10 != 0) {
                s.on_received(0x1111, seq, u32::from(seq) * 160, Instant::now());
            }
            s.on_report(&sr, Instant::now());
        }
        std::thread::sleep(Duration::from_millis(20));
        let rr = b.lock().report(Instant::now());
        a.lock().on_report(&rr, Instant::now());

        let mut s = a.lock();
        let remote = s.remote().expect("B reported on A's stream");
        assert_eq!(remote.cumulative_lost, 9);
        assert!((remote.fraction_lost_pct - 9.0).abs() < 1.0, "{remote:?}");
        let rtt = s.rtt_ms().expect("a round trip from B's echo of the SR");
        assert!(rtt < 1000.0, "{rtt}");
        assert_eq!((s.reports_sent, s.reports_received), (1, 1));
    }

    #[test]
    fn a_relay_that_renumbers_still_reports_on_ours() {
        let a = RtcpState::new(0x1111, 8000, "a@anvil".into());
        let mut rr = ReceiverReport::new(0x9999);
        let mut block = ReceptionReportBlock::new(0x5555);
        block.cumulative_lost = 3;
        rr.add_report_block(block);
        a.lock()
            .on_report(&RtcpPacket::ReceiverReport(rr).to_bytes(), Instant::now());
        assert_eq!(a.lock().remote().map(|r| r.cumulative_lost), Some(3));
    }

    #[test]
    fn the_far_ends_rtcp_is_the_port_above_its_rtp() {
        let rtp: SocketAddr = "192.0.2.1:40000".parse().unwrap();
        assert_eq!(remote_rtcp_addr(rtp).port(), 40001);
    }
}
