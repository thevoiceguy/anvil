//! What the network between us and the far end will carry (§2.3,
//! "bandwidth both ways").
//!
//! The estimators live in forge-media's `forge-bwe`, one Google Congestion
//! Control core shared with FCP's rooms and gateway (FCP's
//! `docs/TRANSPORT_CC.md`):
//! - [`RembEstimator`], the receive side we have always run, whose
//!   estimate goes to the far end as REMB;
//! - [`FeedbackWriter`], which reports what arrived as transport-cc
//!   feedback, for a far end that estimates itself;
//! - [`Uplink`], our own send-side estimate of what we send, from the far
//!   end's transport-cc feedback.

use bytes::Bytes;
use forge_bwe::SendSideEstimator;
pub(crate) use forge_bwe::{BandwidthEstimator, FeedbackWriter, RembEstimator, MIN_BPS};
use forge_rtp::transport_cc::{stamp, TransportCcFeedback};
use forge_rtp::RtpPacket;
use std::time::Instant;

/// What we send, numbered for transport-cc, and what the far end says of
/// it.
pub(crate) struct Uplink {
    ext: u8,
    seq: u16,
    estimator: SendSideEstimator,
}

impl Uplink {
    /// Numbered under `ext`, the extension's agreed id.
    pub(crate) fn new(ext: u8) -> Self {
        Self {
            ext,
            seq: rand::random(),
            estimator: SendSideEstimator::new(),
        }
    }

    /// `packet` numbered as it leaves, and noted as sent. A packet that
    /// does not parse goes as it is.
    pub(crate) fn stamp(&mut self, packet: Bytes) -> Bytes {
        let Ok(mut p) = RtpPacket::parse(packet.clone()) else {
            return packet;
        };
        let seq = self.seq;
        self.seq = seq.wrapping_add(1);
        stamp(&mut p, self.ext, seq);
        let out = p.to_bytes().freeze();
        self.estimator.on_sent(seq, out.len(), Instant::now(), None);
        out
    }

    /// The far end's feedback: the estimate, in bits a second.
    pub(crate) fn on_feedback(&mut self, fb: &TransportCcFeedback, now: Instant) -> Option<u64> {
        self.estimator.on_feedback(fb, now);
        self.estimator.estimate()
    }

    /// Whether the far end's feedback is arriving: while it is, its
    /// estimate is what we send by, not a REMB.
    pub(crate) fn fresh(&self, now: Instant) -> bool {
        self.estimator.fresh(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_rtp::transport_cc::transport_sequence;

    #[test]
    fn what_we_send_is_numbered_and_the_far_end_s_feedback_is_an_estimate() {
        let mut up = Uplink::new(5);
        let mut writer = FeedbackWriter::new();
        let mut last = None;
        for i in 0..10u16 {
            let p = RtpPacket::build(96, i, 0, 1, Bytes::from_static(&[1, 2, 3]), false);
            let out = up.stamp(p.to_bytes().freeze());
            let seq = transport_sequence(&RtpPacket::parse(out).unwrap(), 5).unwrap();
            if let Some(l) = last {
                assert_eq!(seq, u16::wrapping_add(l, 1));
            }
            last = Some(seq);
            writer.on_packet(seq, Instant::now());
        }
        let now = Instant::now();
        assert!(!up.fresh(now));
        let fb = writer.flush(now).unwrap();
        assert!(up.on_feedback(&fb, now).is_some());
        assert!(up.fresh(now));
    }
}
