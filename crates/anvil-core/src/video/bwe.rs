//! What the network between us and the far end will carry of what it
//! sends us (§2.3, "bandwidth both ways"), told to it by REMB.
//!
//! Behind [`BandwidthEstimator`], because REMB from the receiver is one
//! way to do it: forwarding servers use send-side estimation
//! (transport-cc), which plugs in beside this one later (§2.4).
//!
//! [`RembEstimator`] is a receive-side estimator in the shape of Google
//! Congestion Control (draft-ietf-rmcat-gcc-02):
//! - **delay**: each frame's arrival spacing against its RTP spacing; a
//!   queue building somewhere shows as a rising trend, which is overuse;
//! - **loss**: packets missing on first arrival (a retransmission does
//!   not hide a loss from it);
//! - **rate**: AIMD — up by 8% a second while neither says otherwise, and
//!   never far above what actually arrives; on overuse, down to 85% of
//!   what arrives; under heavy loss, by half the loss.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

/// Watches what arrives and says what the far end may send.
pub(crate) trait BandwidthEstimator: Send {
    /// An RTP packet arrived: its stream, sequence number, RTP timestamp
    /// (on a 90 kHz clock), size on the wire and arrival.
    fn on_packet(&mut self, ssrc: u32, seq: u16, timestamp: u32, size: usize, at: Instant);
    /// A stream ended or restarted (hold): its history is no guide.
    fn forget(&mut self, ssrc: u32);
    /// What the far end may send, in bits a second, as of `now`; `None`
    /// until there is enough to say.
    fn estimate(&mut self, now: Instant) -> Option<u64>;
}

/// The first estimate, before anything is known: above what a camera
/// sends by default, so it limits nothing until the network says so.
pub(crate) const START_BPS: u64 = 2_500_000;
/// Never asked below this: a picture of some kind gets through.
pub(crate) const MIN_BPS: u64 = 100_000;

/// What arrives is measured over this long.
const RATE_WINDOW: Duration = Duration::from_millis(1000);
/// The trend is a regression over this many frame deltas.
const TREND_WINDOW: usize = 20;
const TREND_SMOOTHING: f64 = 0.9;
const TREND_GAIN: f64 = 4.0;
/// The overuse threshold adapts between these, in ms (GCC §5.4).
const THRESHOLD_START: f64 = 12.5;
const THRESHOLD_MIN: f64 = 6.0;
const THRESHOLD_MAX: f64 = 600.0;
const K_UP: f64 = 0.0087;
const K_DOWN: f64 = 0.039;
/// Overuse must last this long before the rate comes down.
const OVERUSE_TIME: Duration = Duration::from_millis(10);
/// Each second without trouble, the rate may rise this much.
const INCREASE_PER_SECOND: f64 = 1.08;
/// On overuse, the rate is set to this share of what arrives.
const DECREASE: f64 = 0.85;
/// The rate stays within this multiple of what arrives (GCC's 1.5),
/// plus a little so a quiet sender can still climb.
const HEADROOM: f64 = 1.5;
const HEADROOM_BPS: f64 = 50_000.0;
/// Loss above this brings the rate down; below the lower one it no longer
/// holds it back.
const LOSS_HIGH: f64 = 0.10;
const LOSS_LOW: f64 = 0.02;
/// Loss is counted over this long.
const LOSS_WINDOW: Duration = Duration::from_millis(1000);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Usage {
    Normal,
    Over,
    Under,
}

/// One stream's delay trend.
struct DelayDetector {
    /// The frame being received: its RTP timestamp and last arrival.
    current: Option<(u32, Instant)>,
    /// The last complete frame.
    previous: Option<(u32, Instant)>,
    first_arrival: Option<Instant>,
    accumulated: f64,
    smoothed: f64,
    /// (arrival in ms since the first, smoothed delay in ms)
    history: VecDeque<(f64, f64)>,
    deltas: usize,
    threshold: f64,
    last_threshold_update: Option<Instant>,
    over_since: Option<Instant>,
    previous_trend: f64,
    usage: Usage,
    /// The highest sequence number seen, for loss.
    highest: Option<u16>,
}

impl DelayDetector {
    fn new() -> Self {
        Self {
            current: None,
            previous: None,
            first_arrival: None,
            accumulated: 0.0,
            smoothed: 0.0,
            history: VecDeque::with_capacity(TREND_WINDOW + 1),
            deltas: 0,
            threshold: THRESHOLD_START,
            last_threshold_update: None,
            over_since: None,
            previous_trend: 0.0,
            usage: Usage::Normal,
            highest: None,
        }
    }

    /// A packet of frame `ts` arrived at `at`.
    fn on_packet(&mut self, ts: u32, at: Instant) {
        match self.current {
            Some((cur, _)) if cur == ts => self.current = Some((ts, at)),
            // An older frame's straggler (a retransmission): no delta.
            Some((cur, _)) if (ts.wrapping_sub(cur) as i32) < 0 => {}
            Some(done) => {
                if let Some(prev) = self.previous {
                    self.on_delta(prev, done);
                }
                self.previous = Some(done);
                self.current = Some((ts, at));
            }
            None => self.current = Some((ts, at)),
        }
    }

    /// Two whole frames: how much later the second arrived than it was
    /// sent, feeding the trend.
    fn on_delta(&mut self, (ts0, at0): (u32, Instant), (ts1, at1): (u32, Instant)) {
        let send_ms = f64::from(ts1.wrapping_sub(ts0)) / 90.0;
        let arrive_ms = at1.saturating_duration_since(at0).as_secs_f64() * 1000.0;
        // A jump (a pause, a new stream) says nothing about a queue.
        if send_ms > 1000.0 {
            return;
        }
        let first = *self.first_arrival.get_or_insert(at1);
        self.accumulated += arrive_ms - send_ms;
        self.smoothed =
            TREND_SMOOTHING * self.smoothed + (1.0 - TREND_SMOOTHING) * self.accumulated;
        let x = at1.saturating_duration_since(first).as_secs_f64() * 1000.0;
        self.history.push_back((x, self.smoothed));
        if self.history.len() > TREND_WINDOW {
            self.history.pop_front();
        }
        self.deltas = (self.deltas + 1).min(60);
        if self.history.len() < TREND_WINDOW {
            return;
        }
        let trend = slope(&self.history).unwrap_or(0.0) * self.deltas as f64 * TREND_GAIN;
        self.detect(trend, at1);
    }

    fn detect(&mut self, trend: f64, now: Instant) {
        if trend > self.threshold {
            let since = *self.over_since.get_or_insert(now);
            if now.saturating_duration_since(since) >= OVERUSE_TIME && trend >= self.previous_trend
            {
                self.usage = Usage::Over;
            }
        } else if trend < -self.threshold {
            self.over_since = None;
            self.usage = Usage::Under;
        } else {
            self.over_since = None;
            self.usage = Usage::Normal;
        }
        self.previous_trend = trend;
        // The threshold follows the trend, slowly up and faster down, so
        // a competing flow does not starve us (GCC §5.4).
        let dt = self
            .last_threshold_update
            .map_or(0.0, |t| {
                now.saturating_duration_since(t).as_secs_f64() * 1000.0
            })
            .min(100.0);
        self.last_threshold_update = Some(now);
        if trend.abs() < self.threshold + 15.0 {
            let k = if trend.abs() < self.threshold {
                K_DOWN
            } else {
                K_UP
            };
            self.threshold += k * (trend.abs() - self.threshold) * dt;
            self.threshold = self.threshold.clamp(THRESHOLD_MIN, THRESHOLD_MAX);
        }
    }
}

/// The least-squares slope of `points`.
fn slope(points: &VecDeque<(f64, f64)>) -> Option<f64> {
    let n = points.len() as f64;
    let (sx, sy) = points
        .iter()
        .fold((0.0, 0.0), |(sx, sy), (x, y)| (sx + x, sy + y));
    let (mx, my) = (sx / n, sy / n);
    let (num, den) = points.iter().fold((0.0, 0.0), |(num, den), (x, y)| {
        (num + (x - mx) * (y - my), den + (x - mx) * (x - mx))
    });
    (den > 0.0).then(|| num / den)
}

/// The receive-side estimator REMB carries.
pub(crate) struct RembEstimator {
    streams: HashMap<u32, DelayDetector>,
    /// (arrival, bytes) over the last [`RATE_WINDOW`].
    arrivals: VecDeque<(Instant, usize)>,
    arrived_bytes: usize,
    /// (when, expected, lost) per packet slot, over [`LOSS_WINDOW`].
    losses: VecDeque<(Instant, u32, u32)>,
    rate: f64,
    loss_cap: Option<f64>,
    last_update: Option<Instant>,
    /// The rate came down: hold until overuse ends.
    holding: bool,
    first_packet: Option<Instant>,
}

impl Default for RembEstimator {
    fn default() -> Self {
        Self {
            streams: HashMap::new(),
            arrivals: VecDeque::new(),
            arrived_bytes: 0,
            losses: VecDeque::new(),
            rate: START_BPS as f64,
            loss_cap: None,
            last_update: None,
            holding: false,
            first_packet: None,
        }
    }
}

impl RembEstimator {
    /// What arrived over the last second, in bits a second.
    fn incoming_bps(&self) -> f64 {
        self.arrived_bytes as f64 * 8.0 / RATE_WINDOW.as_secs_f64()
    }

    /// Of the packets due over the last second, the share that never came.
    pub(crate) fn loss(&self) -> f64 {
        let (expected, lost) = self
            .losses
            .iter()
            .fold((0u32, 0u32), |(e, l), (_, de, dl)| (e + de, l + dl));
        if expected == 0 {
            0.0
        } else {
            f64::from(lost) / f64::from(expected)
        }
    }

    fn usage(&self) -> Usage {
        if self.streams.values().any(|d| d.usage == Usage::Over) {
            Usage::Over
        } else if self.streams.values().any(|d| d.usage == Usage::Under) {
            Usage::Under
        } else {
            Usage::Normal
        }
    }

    fn expire(&mut self, now: Instant) {
        while let Some(&(t, bytes)) = self.arrivals.front() {
            if now.saturating_duration_since(t) <= RATE_WINDOW {
                break;
            }
            self.arrived_bytes -= bytes;
            self.arrivals.pop_front();
        }
        while let Some(&(t, _, _)) = self.losses.front() {
            if now.saturating_duration_since(t) <= LOSS_WINDOW {
                break;
            }
            self.losses.pop_front();
        }
    }
}

impl BandwidthEstimator for RembEstimator {
    fn on_packet(&mut self, ssrc: u32, seq: u16, timestamp: u32, size: usize, at: Instant) {
        self.first_packet.get_or_insert(at);
        self.arrivals.push_back((at, size));
        self.arrived_bytes += size;
        let d = self.streams.entry(ssrc).or_insert_with(DelayDetector::new);
        // Loss: a jump forward leaves a gap; anything at or behind the
        // highest is a retransmission or a reordering, and counts for
        // nothing either way.
        match d.highest {
            None => {
                d.highest = Some(seq);
                self.losses.push_back((at, 1, 0));
            }
            Some(h) => {
                let ahead = seq.wrapping_sub(h);
                if ahead != 0 && ahead < 0x8000 {
                    d.highest = Some(seq);
                    // A jump of thousands is a restart, not a loss.
                    let gap = if ahead > 1000 {
                        0
                    } else {
                        u32::from(ahead) - 1
                    };
                    self.losses.push_back((at, gap + 1, gap));
                }
            }
        }
        d.on_packet(timestamp, at);
        self.expire(at);
    }

    fn forget(&mut self, ssrc: u32) {
        self.streams.remove(&ssrc);
    }

    fn estimate(&mut self, now: Instant) -> Option<u64> {
        self.expire(now);
        let first = self.first_packet?;
        let dt = self
            .last_update
            .map_or(0.0, |t| now.saturating_duration_since(t).as_secs_f64())
            .min(1.0);
        self.last_update = Some(now);
        // A second of arrivals before anything is said of them.
        if now.saturating_duration_since(first) < RATE_WINDOW {
            return None;
        }
        let incoming = self.incoming_bps();
        match self.usage() {
            Usage::Over => {
                if !self.holding {
                    self.rate = (DECREASE * incoming).min(self.rate);
                    self.holding = true;
                }
            }
            Usage::Under => self.holding = true,
            Usage::Normal => {
                self.holding = false;
                self.rate *= INCREASE_PER_SECOND.powf(dt);
            }
        }
        self.rate = self.rate.min(HEADROOM * incoming + HEADROOM_BPS);

        let loss = self.loss();
        if loss > LOSS_HIGH {
            let cap = self.loss_cap.unwrap_or(self.rate).min(incoming.max(1.0));
            // Once per update period of loss, as GCC's sender does on
            // each report.
            self.loss_cap = Some(cap * (1.0 - 0.5 * loss * dt.max(0.2)));
        } else if loss < LOSS_LOW {
            self.loss_cap = self
                .loss_cap
                .map(|c| c * INCREASE_PER_SECOND.powf(dt))
                .filter(|c| *c < self.rate);
        }
        let rate = self.loss_cap.map_or(self.rate, |c| c.min(self.rate));
        self.rate = self.rate.max(MIN_BPS as f64);
        Some((rate as u64).max(MIN_BPS))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `seconds` of a 30 fps stream at `kbps`, each frame's last packet
    /// arriving `extra(i)` ms after its time.
    fn feed(
        e: &mut RembEstimator,
        start: Instant,
        frames: std::ops::Range<u32>,
        kbps: u32,
        mut extra: impl FnMut(u32) -> f64,
        mut drop: impl FnMut(u16) -> bool,
        seq: &mut u16,
    ) -> Vec<u64> {
        let per_frame = (kbps as usize * 1000 / 8 / 30).max(200);
        let packets = per_frame.div_ceil(1100);
        let mut estimates = Vec::new();
        for i in frames {
            let ts = i * 3000;
            let at = start
                + Duration::from_micros(u64::from(i) * 33_333)
                + Duration::from_secs_f64(extra(i).max(0.0) / 1000.0);
            for _ in 0..packets {
                if !drop(*seq) {
                    e.on_packet(1, *seq, ts, per_frame / packets, at);
                }
                *seq = seq.wrapping_add(1);
            }
            if i % 6 == 0 {
                if let Some(x) = e.estimate(at) {
                    estimates.push(x);
                }
            }
        }
        estimates
    }

    #[test]
    fn a_clear_path_limits_nothing_a_sender_needs() {
        let mut e = RembEstimator::default();
        let mut seq = 0;
        let est = feed(
            &mut e,
            Instant::now(),
            0..300,
            1500,
            |_| 0.0,
            |_| false,
            &mut seq,
        );
        let last = *est.last().unwrap();
        assert!(last >= 1_500_000, "a clean 1.5 Mb/s is allowed: {last}");
    }

    #[test]
    fn a_growing_queue_is_overuse_and_brings_the_estimate_under_what_arrives() {
        let mut e = RembEstimator::default();
        let mut seq = 0;
        let start = Instant::now();
        feed(&mut e, start, 0..90, 1500, |_| 0.0, |_| false, &mut seq);
        // From frame 90, each frame waits 4 ms longer than the last: a
        // bottleneck filling.
        let est = feed(
            &mut e,
            start,
            90..150,
            1500,
            |i| f64::from(i - 90) * 4.0,
            |_| false,
            &mut seq,
        );
        let last = *est.last().unwrap();
        assert!(last < 1_500_000, "overuse lowers it: {est:?}");
    }

    #[test]
    fn heavy_loss_brings_it_down_and_light_loss_does_not() {
        let mut e = RembEstimator::default();
        let mut seq = 0;
        let start = Instant::now();
        let est = feed(
            &mut e,
            start,
            0..300,
            1500,
            |_| 0.0,
            |s| s % 50 == 0,
            &mut seq,
        );
        assert!(*est.last().unwrap() >= 1_500_000, "2% loss: {est:?}");
        let est = feed(
            &mut e,
            start,
            300..450,
            1500,
            |_| 0.0,
            |s| s % 5 == 0,
            &mut seq,
        );
        assert!(*est.last().unwrap() < 1_200_000, "20% loss: {est:?}");
        assert!(e.loss() > 0.1);
    }

    #[test]
    fn a_retransmission_does_not_hide_a_loss() {
        let mut e = RembEstimator::default();
        let now = Instant::now();
        for seq in [1u16, 2, 4, 5] {
            e.on_packet(1, seq, 0, 1000, now);
        }
        e.on_packet(1, 3, 0, 1000, now);
        assert!((e.loss() - 0.2).abs() < 1e-9, "{}", e.loss());
    }
}
