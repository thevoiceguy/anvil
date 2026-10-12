//! Echo cancellation and noise suppression (W5): WebRTC's audio processing
//! (AEC3, its noise suppressor and high-pass filter) as `sonora` ports it to
//! Rust, so it builds wherever the app does.
//!
//! The canceller has to hear what the speaker plays. Every call's speaker
//! stream queues what it is given in the host's [`Reference`]; the
//! microphone's [`Canceller`] takes the same length of it for each frame it
//! cleans, so the reference arrives at the microphone's pace however the
//! network delivered it. The echo comes back later than the reference
//! (through the speaker's buffer, the room and the microphone's), which
//! AEC3's delay estimator finds; it never comes earlier.

use std::collections::VecDeque;
use std::sync::Arc;

use anvil_core::audio::{AudioFrame, AudioProcessor, Processing};
use parking_lot::Mutex;
use sonora::config::{EchoCanceller, HighPassFilter, NoiseSuppression};
use sonora::{AudioProcessing, Config, StreamConfig};

/// The longest a speaker stream's queue gets before its oldest is dropped:
/// beyond it the microphone is not taking (no call, or a stalled one).
const MAX_QUEUED_MS: usize = 300;

/// What the speakers play, for the canceller.
#[derive(Default)]
pub(crate) struct Reference {
    inner: Mutex<Queues>,
}

#[derive(Default)]
struct Queues {
    next: u64,
    /// Each speaker stream's queue, by id.
    speakers: Vec<(u64, Queue)>,
    /// The cancellers by id, oldest first. Only the newest takes: two calls
    /// up are one heard and one held, and the held one sends nothing.
    cancellers: Vec<u64>,
}

struct Queue {
    rate: u32,
    samples: VecDeque<i16>,
}

impl Reference {
    pub(crate) fn new() -> Arc<Self> {
        Arc::default()
    }

    fn id(&self) -> u64 {
        let mut q = self.inner.lock();
        q.next += 1;
        q.next
    }

    /// A speaker stream at `rate`, queuing what it plays until dropped.
    pub(crate) fn tap(self: &Arc<Self>, rate: u32) -> Tap {
        let id = self.id();
        self.inner.lock().speakers.push((
            id,
            Queue {
                rate,
                samples: VecDeque::new(),
            },
        ));
        Tap {
            id,
            reference: Arc::clone(self),
        }
    }

    /// `n` samples at `rate` of what the speakers played, mixed, for
    /// canceller `id`; `None` when another, newer canceller is taking.
    fn take(&self, id: u64, rate: u32, n: usize) -> Option<Vec<i16>> {
        let mut q = self.inner.lock();
        if q.cancellers.last() != Some(&id) {
            return None;
        }
        let mut mixed = vec![0i32; n];
        for (_, queue) in &mut q.speakers {
            let want = (n as u64 * queue.rate as u64).div_ceil(rate as u64) as usize;
            let got: Vec<i16> = {
                let k = want.min(queue.samples.len());
                queue.samples.drain(..k).collect()
            };
            if got.is_empty() {
                continue;
            }
            for (i, m) in mixed.iter_mut().enumerate() {
                // Nearest sample: a reference needs the echo's shape, not
                // fidelity, and a ringtone at another rate is the only case.
                let at = i * want / n;
                *m += i32::from(*got.get(at).unwrap_or(&0));
            }
        }
        Some(
            mixed
                .into_iter()
                .map(|s| s.clamp(i16::MIN as i32, i16::MAX as i32) as i16)
                .collect(),
        )
    }

    fn leave(&self, id: u64) {
        let mut q = self.inner.lock();
        q.speakers.retain(|(s, _)| *s != id);
        q.cancellers.retain(|c| *c != id);
    }
}

/// A speaker stream's place in the [`Reference`].
pub(crate) struct Tap {
    id: u64,
    reference: Arc<Reference>,
}

impl Tap {
    /// The speaker is given `frame` (mono or the first channel of it).
    pub(crate) fn played(&self, frame: &AudioFrame) {
        let channels = frame.format.channels.max(1) as usize;
        let mut q = self.reference.inner.lock();
        let Some((_, queue)) = q.speakers.iter_mut().find(|(s, _)| *s == self.id) else {
            return;
        };
        queue
            .samples
            .extend(frame.samples.iter().step_by(channels).copied());
        let max = queue.rate as usize * MAX_QUEUED_MS / 1000;
        if queue.samples.len() > max {
            let over = queue.samples.len() - max;
            queue.samples.drain(..over);
        }
    }
}

impl Drop for Tap {
    fn drop(&mut self) {
        self.reference.leave(self.id);
    }
}

/// The microphone's cleaner: the host's [`Processing`] as it is set (read
/// each frame, so a change reaches the call that is up), then `then`.
pub(crate) struct Canceller {
    id: u64,
    reference: Arc<Reference>,
    settings: Arc<Mutex<Processing>>,
    applied: Option<Processing>,
    apm: Option<(u32, AudioProcessing)>,
    then: Box<dyn AudioProcessor>,
    warned: bool,
}

impl Canceller {
    pub(crate) fn new(
        reference: &Arc<Reference>,
        settings: Arc<Mutex<Processing>>,
        then: Box<dyn AudioProcessor>,
    ) -> Self {
        let id = reference.id();
        reference.inner.lock().cancellers.push(id);
        Self {
            id,
            reference: Arc::clone(reference),
            settings,
            applied: None,
            apm: None,
            then,
            warned: false,
        }
    }

    fn clean(&mut self, frame: &mut AudioFrame) {
        let wanted = *self.settings.lock();
        let rate = frame.format.sample_rate;
        let chunk = rate as usize / 100;
        if frame.format.channels != 1 || chunk == 0 || !frame.samples.len().is_multiple_of(chunk) {
            if !self.warned {
                self.warned = true;
                tracing::warn!(
                    channels = frame.format.channels,
                    rate,
                    len = frame.samples.len(),
                    "this microphone frame cannot be cleaned"
                );
            }
            return;
        }
        if !wanted.echo_cancellation && !wanted.noise_suppression {
            // Keep the reference moving so turning it on starts fresh.
            let _ = self.reference.take(self.id, rate, frame.samples.len());
            self.applied = Some(wanted);
            return;
        }
        if self.apm.as_ref().map(|(r, _)| *r) != Some(rate) {
            let stream = StreamConfig::new(rate, 1);
            let apm = AudioProcessing::builder()
                .config(config(wanted))
                .capture_config(stream)
                .render_config(stream)
                .build();
            self.apm = Some((rate, apm));
            self.applied = Some(wanted);
            tracing::debug!(rate, ?wanted, "microphone cleaning");
        }
        let (_, apm) = self.apm.as_mut().expect("made above");
        if self.applied != Some(wanted) {
            apm.apply_config(config(wanted));
            self.applied = Some(wanted);
            tracing::debug!(?wanted, "microphone cleaning changed");
        }
        let mut out = vec![0i16; chunk];
        let mut sink = vec![0i16; chunk];
        for piece in frame.samples.chunks_mut(chunk) {
            let render = self
                .reference
                .take(self.id, rate, chunk)
                .unwrap_or_else(|| vec![0; chunk]);
            let _ = apm.process_render_i16(&render, &mut sink);
            // AEC3 finds the delay itself; the hint only has to be set.
            let _ = apm.set_stream_delay_ms(0);
            if apm.process_capture_i16(piece, &mut out).is_ok() {
                piece.copy_from_slice(&out);
            }
        }
    }
}

impl AudioProcessor for Canceller {
    fn process_capture(&mut self, frame: &mut AudioFrame) {
        self.clean(frame);
        self.then.process_capture(frame);
    }
}

impl Drop for Canceller {
    fn drop(&mut self) {
        self.reference.leave(self.id);
    }
}

fn config(p: Processing) -> Config {
    Config {
        high_pass_filter: Some(HighPassFilter::default()),
        echo_canceller: p.echo_cancellation.then(EchoCanceller::default),
        noise_suppression: p.noise_suppression.then(NoiseSuppression::default),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anvil_core::audio::{AudioFormat, NullProcessor};

    const RATE: u32 = 16_000;
    const FRAME: usize = RATE as usize / 50;

    fn format() -> AudioFormat {
        AudioFormat {
            sample_rate: RATE,
            channels: 1,
            frame_ms: 20,
        }
    }

    /// Speech-like far end: noise shaped by a slow on/off envelope, from a
    /// fixed seed.
    fn far_end(frames: usize) -> Vec<Vec<i16>> {
        let mut x: u32 = 0x1234_5678;
        (0..frames)
            .map(|f| {
                let talking = (f / 25) % 4 != 3;
                (0..FRAME)
                    .map(|_| {
                        x ^= x << 13;
                        x ^= x >> 17;
                        x ^= x << 5;
                        let n = (x as i32 >> 16) as f32 / 32768.0;
                        if talking {
                            (n * 8000.0) as i16
                        } else {
                            0
                        }
                    })
                    .collect()
            })
            .collect()
    }

    fn energy(frames: &[Vec<i16>]) -> f64 {
        frames
            .iter()
            .flatten()
            .map(|&s| (s as f64) * (s as f64))
            .sum::<f64>()
            / (frames.len() * FRAME) as f64
    }

    /// The room: what the speaker played `delay` frames ago, quieter, plus
    /// `near` (the person at the microphone).
    fn run(
        settings: Processing,
        delay: usize,
        far: &[Vec<i16>],
        near: impl Fn(usize) -> Vec<i16>,
    ) -> (Vec<Vec<i16>>, Vec<Vec<i16>>) {
        let reference = Reference::new();
        let tap = reference.tap(RATE);
        let mut mic = Canceller::new(
            &reference,
            Arc::new(Mutex::new(settings)),
            Box::new(NullProcessor),
        );
        let silent = vec![0i16; FRAME];
        let (mut heard, mut sent) = (Vec::new(), Vec::new());
        for f in 0..far.len() {
            tap.played(&AudioFrame {
                samples: far[f].clone(),
                format: format(),
            });
            let echo = if f >= delay { &far[f - delay] } else { &silent };
            let person = near(f);
            let samples: Vec<i16> = echo
                .iter()
                .zip(&person)
                .map(|(&e, &p)| (e / 2).saturating_add(p))
                .collect();
            heard.push(samples.clone());
            let mut frame = AudioFrame {
                samples,
                format: format(),
            };
            mic.process_capture(&mut frame);
            sent.push(frame.samples);
        }
        (heard, sent)
    }

    fn db(a: f64, b: f64) -> f64 {
        10.0 * (a / b.max(1.0)).log10()
    }

    #[test]
    fn the_far_end_heard_back_through_the_microphone_is_taken_out() {
        let far = far_end(500);
        let quiet = |_| vec![0i16; FRAME];
        // 60 ms between the speaker and the microphone; the echo canceller
        // alone (the noise suppressor would take some of it too).
        let echo_only = Processing {
            echo_cancellation: true,
            noise_suppression: false,
        };
        let (heard, sent) = run(echo_only, 3, &far, quiet);
        // After four seconds to converge, the last second.
        let reduced = db(energy(&heard[400..]), energy(&sent[400..]));
        assert!(reduced > 20.0, "echo down only {reduced:.1} dB");

        let off = Processing {
            echo_cancellation: false,
            noise_suppression: false,
        };
        let (heard, sent) = run(off, 3, &far, quiet);
        assert_eq!(heard, sent, "off is untouched");
    }

    #[test]
    fn the_person_at_the_microphone_is_still_heard_over_the_echo() {
        let far = far_end(500);
        // The person speaks in the far end's pauses: a 300 Hz tone.
        let tone = |f: usize| -> Vec<i16> {
            if (f / 25) % 4 == 3 {
                (0..FRAME)
                    .map(|i| {
                        let t = (f * FRAME + i) as f32 / RATE as f32;
                        ((t * 300.0 * std::f32::consts::TAU).sin() * 6000.0) as i16
                    })
                    .collect()
            } else {
                vec![0; FRAME]
            }
        };
        let (_, sent) = run(Processing::default(), 3, &far, tone);
        let pauses: Vec<usize> = (400..500).filter(|f| (f / 25) % 4 == 3).collect();
        let kept: Vec<Vec<i16>> = pauses.iter().map(|&f| sent[f].clone()).collect();
        let spoken: Vec<Vec<i16>> = pauses.iter().map(|&f| tone(f)).collect();
        let lost = db(energy(&spoken), energy(&kept));
        assert!(lost < 6.0, "the person lost {lost:.1} dB");
    }

    #[test]
    fn steady_noise_is_turned_down() {
        let mut x: u32 = 0xdead_beef;
        let hiss = move |_| -> Vec<i16> {
            (0..FRAME)
                .map(|_| {
                    x = x.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                    ((x >> 16) as i16 as i32 / 16) as i16
                })
                .collect()
        };
        let hiss = std::cell::RefCell::new(hiss);
        let silent: Vec<Vec<i16>> = vec![vec![0; FRAME]; 250];
        let noise_only = Processing {
            echo_cancellation: false,
            noise_suppression: true,
        };
        let (heard, sent) = run(noise_only, 0, &silent, |f| (hiss.borrow_mut())(f));
        let reduced = db(energy(&heard[150..]), energy(&sent[150..]));
        assert!(reduced > 6.0, "noise down only {reduced:.1} dB");
    }

    #[test]
    fn a_change_reaches_the_call_that_is_up() {
        let reference = Reference::new();
        let settings = Arc::new(Mutex::new(Processing::default()));
        let mut mic = Canceller::new(&reference, Arc::clone(&settings), Box::new(NullProcessor));
        let mut frame = AudioFrame {
            samples: vec![1000; FRAME],
            format: format(),
        };
        mic.process_capture(&mut frame);
        assert_eq!(mic.applied, Some(Processing::default()));
        let off = Processing {
            echo_cancellation: false,
            noise_suppression: false,
        };
        *settings.lock() = off;
        let mut frame = AudioFrame {
            samples: vec![1000; FRAME],
            format: format(),
        };
        mic.process_capture(&mut frame);
        assert_eq!(mic.applied, Some(off));
        assert_eq!(frame.samples, vec![1000; FRAME]);
    }

    #[test]
    fn only_the_newest_call_takes_the_reference_and_a_queue_is_bounded() {
        let reference = Reference::new();
        let tap = reference.tap(RATE);
        let settings = Arc::new(Mutex::new(Processing::default()));
        let held = Canceller::new(&reference, Arc::clone(&settings), Box::new(NullProcessor));
        let up = Canceller::new(&reference, Arc::clone(&settings), Box::new(NullProcessor));
        for _ in 0..50 {
            tap.played(&AudioFrame {
                samples: vec![7; FRAME],
                format: format(),
            });
        }
        let queued = |r: &Reference| r.inner.lock().speakers[0].1.samples.len();
        assert_eq!(queued(&reference), RATE as usize * MAX_QUEUED_MS / 1000);
        assert_eq!(reference.take(held.id, RATE, 160), None);
        assert_eq!(reference.take(up.id, RATE, 160), Some(vec![7; 160]));
        drop(up);
        assert!(reference.take(held.id, RATE, 160).is_some());
        // At another rate, the same length of time.
        let before = queued(&reference);
        let _ = reference.take(held.id, 8_000, 80);
        assert_eq!(before - queued(&reference), 160);
        drop(tap);
        assert_eq!(reference.take(held.id, RATE, 160), Some(vec![0; 160]));
    }
}
