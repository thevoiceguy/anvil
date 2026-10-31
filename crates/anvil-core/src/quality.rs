//! A call's audio quality as a listener would score it: the ITU-T G.107
//! E-model, narrowband, from the codec, packet loss, jitter and the round
//! trip, mapped to a mean opinion score (MOS) of 1 to 4.5.
//!
//! The E-model's own defaults stand in for what a phone cannot measure
//! (room noise, echo, the handset). Anvil plays packets as they arrive,
//! with no jitter buffer and no loss concealment, so a codec's packet-loss
//! robustness is G.113's figure without concealment, and jitter counts as
//! delay a buffer would have added.

use crate::config::Codec;

/// The E-model's R without impairments: G.107's default 93.2.
const R0: f32 = 93.2;

/// What a call's audio is estimated to score.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mos {
    /// The transmission rating, 0 to 100.
    pub r: f32,
    /// The mean opinion score, 1 to 4.5.
    pub score: f32,
}

/// Good, fair or poor, as FCP's `call.quality_poor` judges a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rating {
    Poor,
    Fair,
    Good,
}

impl Mos {
    /// 4.0 and above is good; under 3.1 is poor (FCP's threshold for
    /// `call.quality_poor`, `docs/CALL_DIAGNOSTICS.md`).
    pub fn rating(&self) -> Rating {
        if self.score >= 4.0 {
            Rating::Good
        } else if self.score >= 3.1 {
            Rating::Fair
        } else {
            Rating::Poor
        }
    }
}

/// The equipment impairment `Ie` and packet-loss robustness `Bpl` of a
/// codec played without concealment (G.113 Appendix I; Opus and G.722 at
/// their usual rates scored as G.711, which the E-model's narrowband scale
/// cannot better).
fn impairment(codec: Codec) -> (f32, f32) {
    match codec {
        Codec::Pcmu | Codec::Pcma | Codec::G722 => (0.0, 4.3),
        // Opus's own decoder smooths a lost frame a little even unasked.
        Codec::Opus => (0.0, 10.0),
    }
}

/// The codec's own delay: its frame plus what its encoder looks ahead.
fn codec_delay_ms(codec: Codec, ptime_ms: u32) -> f32 {
    let lookahead = match codec {
        Codec::Pcmu | Codec::Pcma => 0.0,
        Codec::G722 => 1.5,
        Codec::Opus => 6.5,
    };
    ptime_ms as f32 + lookahead
}

/// Estimate the score of audio in one direction: `loss_pct` of its packets
/// lost, `jitter_ms` on their arrival, `rtt_ms` the round trip when RTCP
/// has measured it (else the network's delay is taken as nothing).
pub fn estimate(
    codec: Codec,
    ptime_ms: u32,
    loss_pct: f32,
    jitter_ms: f32,
    rtt_ms: Option<f32>,
) -> Mos {
    let one_way = rtt_ms.unwrap_or(0.0).max(0.0) / 2.0
        + codec_delay_ms(codec, ptime_ms)
        + 2.0 * jitter_ms.max(0.0);
    let id = 0.024 * one_way
        + if one_way > 177.3 {
            0.11 * (one_way - 177.3)
        } else {
            0.0
        };
    let (ie, bpl) = impairment(codec);
    let ppl = loss_pct.clamp(0.0, 100.0);
    let ie_eff = ie + (95.0 - ie) * ppl / (ppl + bpl);
    let r = (R0 - id - ie_eff).clamp(0.0, 100.0);
    Mos { r, score: score(r) }
}

/// G.107 Annex B: R to MOS.
fn score(r: f32) -> f32 {
    if r <= 0.0 {
        1.0
    } else if r >= 100.0 {
        4.5
    } else {
        1.0 + 0.035 * r + r * (r - 60.0) * (100.0 - r) * 7.0e-6
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clean_local_call_is_good() {
        let m = estimate(Codec::Pcmu, 20, 0.0, 1.0, Some(20.0));
        assert!(m.score > 4.3, "{m:?}");
        assert_eq!(m.rating(), Rating::Good);
    }

    #[test]
    fn loss_and_delay_bring_it_down() {
        // G.711 without concealment: 1 % loss costs about a third of a point.
        let one = estimate(Codec::Pcmu, 20, 1.0, 1.0, Some(20.0));
        assert!((3.6..4.1).contains(&one.score), "{one:?}");
        let five = estimate(Codec::Pcmu, 20, 5.0, 1.0, Some(20.0));
        assert_eq!(five.rating(), Rating::Poor, "{five:?}");
        // Past 177 ms one way, delay costs fast.
        let far = estimate(Codec::Pcmu, 20, 0.0, 1.0, Some(600.0));
        assert!(far.score < 4.0, "{far:?}");
        assert!(far.score > estimate(Codec::Pcmu, 20, 0.0, 1.0, Some(900.0)).score);
        // Jitter counts as delay.
        assert!(
            estimate(Codec::Pcmu, 20, 0.0, 80.0, Some(200.0)).score
                < estimate(Codec::Pcmu, 20, 0.0, 1.0, Some(200.0)).score
        );
    }

    #[test]
    fn opus_takes_loss_better_than_g711() {
        let g711 = estimate(Codec::Pcmu, 20, 2.0, 1.0, None);
        let opus = estimate(Codec::Opus, 20, 2.0, 1.0, None);
        assert!(opus.score > g711.score);
    }

    #[test]
    fn the_scale_is_bounded() {
        assert_eq!(
            estimate(Codec::Pcmu, 20, 100.0, 500.0, Some(5000.0)).score,
            1.0
        );
        assert!(estimate(Codec::Pcmu, 20, 0.0, 0.0, Some(0.0)).score <= 4.5);
    }
}
