//! Pure-Rust capture-side audio processors.
//!
//! M8b ships one: a peak-following Automatic Gain Control. AEC and
//! noise suppression are deferred — real AEC needs platform-native
//! voice-processing audio units (iOS / Android) or
//! webrtc-audio-processing's render-stream API; both are larger lifts
//! than fits in this milestone. The `AudioProcessor` trait in
//! `anvil-core::audio` is the slot they plug into when implemented.

use anvil_core::audio::{AudioFrame, AudioProcessor};

/// Simple peak-following AGC.
///
/// Tracks the maximum absolute sample over a sliding window and scales
/// each frame so the running peak lands near `target_peak`. Limited to
/// `max_gain` so we don't blow up noise floor when the input is silent.
///
/// Compared to professional AGC: no compression curve, no attack /
/// release distinction, no look-ahead. Adequate for cleaning up a quiet
/// USB mic; not adequate for mastering. Good cheap default for a softphone.
pub struct SimpleAgc {
    /// Target peak amplitude as a fraction of i16::MAX (0.0..=1.0).
    /// 0.7 is a good "loud but not clipping" default.
    target_peak: f32,
    /// Hard cap on the multiplier. Without this, near-silent input
    /// becomes a noise amplifier.
    max_gain: f32,
    /// Smoothing factor for the running peak estimate, 0..1.
    /// Higher = slower response.
    decay: f32,
    /// Current smoothed peak, normalised to 0..1.
    smoothed_peak: f32,
    /// Current applied gain.
    gain: f32,
}

impl SimpleAgc {
    /// Default settings: target peak 0.7, max gain 8×, half-life of about
    /// half a second at 50 frames/s.
    pub fn new() -> Self {
        Self {
            target_peak: 0.7,
            max_gain: 8.0,
            decay: 0.97,
            smoothed_peak: 0.0,
            gain: 1.0,
        }
    }

    /// Override the target peak (0.0..=1.0). Below 0.5 leaves headroom
    /// for transients; above 0.8 risks µ-law / Opus quantisation noise.
    pub fn with_target(mut self, target_peak: f32) -> Self {
        self.target_peak = target_peak.clamp(0.05, 0.95);
        self
    }
}

impl Default for SimpleAgc {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioProcessor for SimpleAgc {
    fn process_capture(&mut self, frame: &mut AudioFrame) {
        // Find the frame's peak in [0, 1).
        let mut frame_peak = 0.0f32;
        for &s in &frame.samples {
            let n = (s as f32).abs() / i16::MAX as f32;
            if n > frame_peak {
                frame_peak = n;
            }
        }

        // Smooth: peak estimate decays toward the new measurement, slowly
        // when the current estimate is higher (so a single quiet frame
        // doesn't ramp up the gain) and fast when it's lower (so we react
        // quickly to a sudden loud sound).
        if frame_peak > self.smoothed_peak {
            self.smoothed_peak = frame_peak;
        } else {
            self.smoothed_peak = self.smoothed_peak * self.decay + frame_peak * (1.0 - self.decay);
        }

        // Compute target gain. Floor the smoothed peak so we don't divide
        // by ~0 during silence and shoot to max_gain instantly.
        let effective_peak = self.smoothed_peak.max(0.01);
        let target_gain = (self.target_peak / effective_peak).min(self.max_gain);

        // Smooth the gain itself with a slower attack to avoid pumping.
        self.gain = self.gain * 0.9 + target_gain * 0.1;

        // Apply.
        let g = self.gain;
        for s in frame.samples.iter_mut() {
            let scaled = (*s as f32) * g;
            *s = scaled.clamp(i16::MIN as f32, i16::MAX as f32) as i16;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anvil_core::audio::AudioFormat;

    fn fmt() -> AudioFormat {
        AudioFormat {
            sample_rate: 8000,
            channels: 1,
            frame_ms: 20,
        }
    }

    fn frame(samples: Vec<i16>) -> AudioFrame {
        AudioFrame {
            samples,
            format: fmt(),
        }
    }

    fn rms(s: &[i16]) -> f32 {
        let n = s.len().max(1) as f32;
        let sum_sq: f32 = s.iter().map(|&x| (x as f32) * (x as f32)).sum();
        (sum_sq / n).sqrt()
    }

    #[test]
    fn quiet_signal_gets_amplified_over_time() {
        let mut agc = SimpleAgc::new();
        // 100-Hz sine, 3% amplitude — a deliberately quiet signal.
        let n = 160usize;
        let scale = 0.03 * i16::MAX as f32;
        let mk = |phase: f32| -> AudioFrame {
            let mut samples = Vec::with_capacity(n);
            for i in 0..n {
                let t = (i as f32) / 8000.0;
                let s = (phase + 2.0 * std::f32::consts::PI * 100.0 * t).sin() * scale;
                samples.push(s as i16);
            }
            frame(samples)
        };

        let initial = rms(&mk(0.0).samples);
        let mut last = initial;
        for i in 0..50 {
            let mut f = mk(i as f32 * 0.1);
            agc.process_capture(&mut f);
            last = rms(&f.samples);
        }
        // After ~1 s of frames the AGC should have lifted the signal
        // materially closer to the target.
        assert!(
            last > initial * 3.0,
            "AGC didn't amplify: initial={initial:.0} last={last:.0}"
        );
    }

    #[test]
    fn loud_signal_is_not_clipped_to_zero() {
        let mut agc = SimpleAgc::new();
        // 1 kHz sine, 90% amplitude — already loud.
        let n = 160usize;
        let scale = 0.9 * i16::MAX as f32;
        let mut f = AudioFrame {
            samples: (0..n)
                .map(|i| {
                    let t = (i as f32) / 8000.0;
                    let s = (2.0 * std::f32::consts::PI * 1000.0 * t).sin() * scale;
                    s as i16
                })
                .collect(),
            format: fmt(),
        };
        let before = rms(&f.samples);
        agc.process_capture(&mut f);
        let after = rms(&f.samples);
        // Gain on a loud input should be near 1.0; allow ±20%.
        assert!(
            after > before * 0.8 && after < before * 1.2,
            "AGC scaled loud input too aggressively: before={before:.0} after={after:.0}"
        );
    }
}
