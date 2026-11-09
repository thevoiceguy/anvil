//! When each incoming frame is shown (§2.3, "timing and lip sync").
//!
//! A frame is shown at its place on the sender's clock (its RTP
//! timestamp), carried across on the quickest any recent frame made the
//! trip, plus a short delay that absorbs the rest: 60 ms on a steady path,
//! growing with jitter to 150 ms. A frame later than that is shown on
//! arrival.
//!
//! **Lip sync.** Once the far end's sender reports have tied this stream's
//! RTP clock and its audio's to one NTP clock (RFC 3550 §6.4.1), a frame
//! captured at NTP time *t* is shown when audio captured at *t* is heard:
//! the audio's own trip from capture to arrival, plus the playback
//! device's latency. Video waits for audio (up to [`MAX_SYNC_WAIT`]); it
//! cannot hurry, so when it arrives later than the audio it is shown as
//! soon as it can be, and the offset says by how much.

use std::time::{Duration, Instant};

/// The shortest delay, on a path with no jitter.
pub(crate) const MIN_DELAY: Duration = Duration::from_millis(60);
/// The longest: past this, video would lag the conversation.
pub(crate) const MAX_DELAY: Duration = Duration::from_millis(150);
/// The most video is held back to meet its audio.
pub(crate) const MAX_SYNC_WAIT: Duration = Duration::from_millis(500);
/// How long the quickest trip is remembered: long enough to cover a
/// burst, short enough to follow a path that changed.
const TRANSIT_WINDOW: Duration = Duration::from_secs(5);

/// What audio does from capture to the ear, in the far end's NTP time
/// against ours (both sides of a call share no clock, so this includes
/// the offset between them; video's own trip includes the same offset,
/// which cancels).
pub(crate) trait AudioClock: Send + Sync {
    /// From an audio sample's capture (the far end's NTP clock, µs) to
    /// its arrival here (ours, µs), smoothed; `None` before the far end's
    /// first sender report.
    fn transit_us(&self) -> Option<i64>;
    /// The playback device's latency: from arrival to the ear.
    fn output_latency(&self) -> Duration;
    /// The CNAME our audio reports under, so our video's reports pair
    /// with it at the far end (RFC 3550 §6.5.1).
    fn cname(&self) -> String;
}

/// One stream's playout clock.
pub(crate) struct Playout {
    clock: u32,
    /// When the first frame arrived: every frame is placed relative to
    /// it (its timestamp is media time zero).
    origin: Option<(Instant, u32)>,
    /// The last timestamp, unwrapped.
    last: Option<(u32, i64)>,
    /// The quickest trip in the current window and the one before, µs.
    transit: [Option<i64>; 2],
    window_start: Option<Instant>,
    /// The far end's last sender report on this stream: (NTP µs, RTP).
    report: Option<(i64, u32)>,
    delay: Duration,
    last_shown: Option<Instant>,
    /// The last frame's offset from its audio, in ms (+: video late).
    offset_ms: Option<f32>,
}

impl Playout {
    pub(crate) fn new(clock: u32) -> Self {
        Self {
            clock: clock.max(1),
            origin: None,
            last: None,
            transit: [None, None],
            window_start: None,
            report: None,
            delay: MIN_DELAY,
            last_shown: None,
            offset_ms: None,
        }
    }

    /// Start again (back from hold): the old trips say nothing now.
    pub(crate) fn reset(&mut self) {
        let report = self.report;
        *self = Self::new(self.clock);
        self.report = report;
    }

    /// The far end's sender report on this stream.
    pub(crate) fn on_sender_report(&mut self, ntp_us: i64, rtp: u32) {
        self.report = Some((ntp_us, rtp));
    }

    /// The delay from jitter (ms, RFC 3550's estimate).
    pub(crate) fn set_jitter(&mut self, jitter_ms: f64) {
        let ms = (2.0 * jitter_ms + 40.0).clamp(
            MIN_DELAY.as_secs_f64() * 1000.0,
            MAX_DELAY.as_secs_f64() * 1000.0,
        );
        self.delay = Duration::from_secs_f64(ms / 1000.0);
    }

    pub(crate) fn delay(&self) -> Duration {
        self.delay
    }

    pub(crate) fn offset_ms(&self) -> Option<f32> {
        self.offset_ms
    }

    /// Microseconds of media time since the origin, for timestamp `ts`.
    fn media_us(&mut self, ts: u32) -> i64 {
        let ext = match self.last {
            None => 0,
            Some((last_ts, last_ext)) => last_ext + i64::from(ts.wrapping_sub(last_ts) as i32),
        };
        if self.last.is_none_or(|(_, e)| ext > e) {
            self.last = Some((ts, ext));
        }
        ext * 1_000_000 / i64::from(self.clock)
    }

    /// A frame with timestamp `ts`, complete at `arrival`, and the call's
    /// audio clock (`now_ntp_us` is our NTP time at `now`).
    pub(crate) fn place(
        &mut self,
        ts: u32,
        arrival: Instant,
        audio: Option<&dyn AudioClock>,
        now: Instant,
        now_ntp_us: i64,
    ) -> Instant {
        let origin = self.origin.get_or_insert((arrival, ts)).0;
        let media = self.media_us(ts);
        let arrived = signed_us(arrival, origin);
        let transit = arrived - media;

        // The quickest trip over the last two windows.
        let start = *self.window_start.get_or_insert(arrival);
        if arrival.saturating_duration_since(start) >= TRANSIT_WINDOW {
            self.transit = [None, self.transit[0]];
            self.window_start = Some(arrival);
        }
        self.transit[0] = Some(self.transit[0].map_or(transit, |t| t.min(transit)));
        let quickest = self
            .transit
            .iter()
            .flatten()
            .min()
            .copied()
            .unwrap_or(transit);

        let natural = at_us(origin, media + quickest) + self.delay;
        let mut show = natural.max(arrival);

        // When the frame was captured, on the far end's NTP clock, and
        // when its audio is heard here (the clocks' offset is in the
        // audio's transit).
        let captured_ntp = self.report.map(|(ntp, rtp)| {
            ntp + i64::from(ts.wrapping_sub(rtp) as i32) * 1_000_000 / i64::from(self.clock)
        });
        let ntp_to_instant = |ntp: i64| at_us(now, ntp - now_ntp_us);
        self.offset_ms = None;
        if let (Some(cap), Some(audio)) = (captured_ntp, audio) {
            if let Some(audio_transit) = audio.transit_us() {
                let heard = ntp_to_instant(cap + audio_transit) + audio.output_latency();
                if heard > show {
                    show = heard.min(show + MAX_SYNC_WAIT);
                }
                self.offset_ms = Some(signed_us(show, heard) as f32 / 1000.0);
            }
        }

        // Never before the frame shown last.
        if let Some(last) = self.last_shown {
            show = show.max(last);
        }
        self.last_shown = Some(show);
        show
    }
}

/// `a - b` in µs, either way round.
fn signed_us(a: Instant, b: Instant) -> i64 {
    if a >= b {
        a.duration_since(b).as_micros() as i64
    } else {
        -(b.duration_since(a).as_micros() as i64)
    }
}

/// `base` moved by `us`, either way.
fn at_us(base: Instant, us: i64) -> Instant {
    if us >= 0 {
        base + Duration::from_micros(us as u64)
    } else {
        base.checked_sub(Duration::from_micros(us.unsigned_abs()))
            .unwrap_or(base)
    }
}

/// Our NTP time now, in µs.
pub(crate) fn ntp_now_us() -> i64 {
    ntp_us(forge_rtp::rtcp::ntp::now())
}

/// A 64-bit NTP timestamp in µs.
pub(crate) fn ntp_us(ntp: u64) -> i64 {
    let secs = (ntp >> 32) as i64;
    let frac = ((ntp & 0xFFFF_FFFF) * 1_000_000) >> 32;
    secs * 1_000_000 + frac as i64
}

/// `at`, on our NTP clock in µs, given our NTP time `now_ntp_us` at `now`.
pub(crate) fn instant_ntp_us(at: Instant, now: Instant, now_ntp_us: i64) -> i64 {
    now_ntp_us + signed_us(at, now)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Audio {
        transit_us: Option<i64>,
    }

    impl AudioClock for Audio {
        fn transit_us(&self) -> Option<i64> {
            self.transit_us
        }
        fn output_latency(&self) -> Duration {
            Duration::from_millis(40)
        }
        fn cname(&self) -> String {
            "a@anvil".into()
        }
    }

    fn ms(d: Duration) -> f64 {
        d.as_secs_f64() * 1000.0
    }

    #[test]
    fn frames_are_paced_by_their_timestamps_not_their_arrival() {
        let mut p = Playout::new(90_000);
        let t0 = Instant::now();
        let ntp = ntp_now_us();
        // 30 fps; frame 2 arrives 25 ms late, frame 3 on time.
        let arrivals = [0.0, 33.3, 91.6, 100.0];
        let shows: Vec<Instant> = arrivals
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let at = t0 + Duration::from_secs_f64(a / 1000.0);
                p.place(i as u32 * 3000, at, None, t0, ntp)
            })
            .collect();
        for (i, s) in shows.iter().enumerate() {
            let expected = i as f64 * 33.333 + 60.0;
            assert!(
                (ms(*s - t0) - expected).abs() < 1.0,
                "frame {i} at {} not {expected}",
                ms(*s - t0)
            );
        }
    }

    #[test]
    fn jitter_grows_the_delay_within_its_bounds() {
        let mut p = Playout::new(90_000);
        p.set_jitter(0.0);
        assert_eq!(p.delay(), MIN_DELAY);
        p.set_jitter(30.0);
        assert!((ms(p.delay()) - 100.0).abs() < 0.5);
        p.set_jitter(500.0);
        assert_eq!(p.delay(), MAX_DELAY);
    }

    #[test]
    fn video_waits_for_its_audio_once_sender_reports_tie_the_clocks() {
        let mut p = Playout::new(90_000);
        let now = Instant::now();
        let now_ntp = ntp_now_us();
        // The far end's clock runs 7 s ahead of ours; its report says RTP
        // 0 was captured at its NTP time `cap`, 10 ms before now.
        let cap = now_ntp + 7_000_000 - 10_000;
        p.on_sender_report(cap, 0);
        // Its audio takes 300 ms (and the 7 s offset) to arrive.
        let audio = Audio {
            transit_us: Some(-7_000_000 + 300_000),
        };
        let show = p.place(0, now, Some(&audio), now, now_ntp);
        // Heard at capture + 300 ms + 40 ms of playback: 330 ms from now.
        let wait = ms(show - now);
        assert!((wait - 330.0).abs() < 1.0, "{wait}");
        assert!(p.offset_ms().unwrap().abs() < 1.0);
    }

    #[test]
    fn video_later_than_its_audio_is_shown_at_once_and_says_by_how_much() {
        let mut p = Playout::new(90_000);
        let now = Instant::now();
        let now_ntp = ntp_now_us();
        // Captured 400 ms ago; audio is heard 40 ms after capture.
        p.on_sender_report(now_ntp - 400_000, 0);
        let audio = Audio {
            transit_us: Some(0),
        };
        let show = p.place(0, now, Some(&audio), now, now_ntp);
        assert!(ms(show - now) <= 61.0, "not held back further");
        let off = p.offset_ms().unwrap();
        assert!((off - (60.0 + 360.0)).abs() < 2.0, "video late by {off}");
    }

    #[test]
    fn sync_never_holds_a_frame_past_its_limit() {
        let mut p = Playout::new(90_000);
        let now = Instant::now();
        let now_ntp = ntp_now_us();
        p.on_sender_report(now_ntp, 0);
        let audio = Audio {
            transit_us: Some(5_000_000),
        };
        let show = p.place(0, now, Some(&audio), now, now_ntp);
        assert!(ms(show - now) <= ms(MIN_DELAY + MAX_SYNC_WAIT) + 1.0);
    }
}
