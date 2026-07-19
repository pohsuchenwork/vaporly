//! Whisper Mode voicing detector: is the mic hearing PITCHED (voiced) speech
//! or unvoiced sound (a whisper, breath, hiss)?
//!
//! Physics: a whisper has no vocal-fold vibration, so it has no fundamental
//! frequency. A normal or loud voice always does. That makes "is it pitched?"
//! a mic-independent cue: a voiced talker can be rejected in whisper mode no
//! matter how quietly their voice reaches the microphone, which is exactly
//! the case a pure loudness ceiling cannot catch (round 20; ported from the
//! vaporly-lab testbed where it separated tones from whisper-noise cleanly).
//!
//! Method: normalized autocorrelation over a 60 ms sliding window (two 30 ms
//! frames), searching lags for f0 in 60..400 Hz, fused with the zero-crossing
//! rate (whisper noise crosses zero far more often than pitched speech).
//! Cost: ~180k multiply-adds per 30 ms frame (well under 0.1% of one core),
//! no allocation after the first window fills.

const SAMPLE_RATE: usize = 16_000;
const WINDOW: usize = 960; // 60 ms at 16 kHz
const F0_MIN_HZ: f32 = 60.0;
const F0_MAX_HZ: f32 = 400.0;
/// Below this window rms the frame is silence: report unvoiced.
const ENERGY_FLOOR: f32 = 0.003;
/// EMA smoothing for the reported probability (~5 frames).
const SMOOTH_ALPHA: f32 = 0.35;

#[derive(Debug, Clone)]
pub struct VoicingDetector {
    buf: Vec<f32>,
    smoothed: f32,
}

impl VoicingDetector {
    pub fn new() -> Self {
        Self {
            buf: Vec::with_capacity(WINDOW),
            smoothed: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.buf.clear();
        self.smoothed = 0.0;
    }

    /// Feed one 30 ms frame; returns the smoothed voiced probability (0..1)
    /// for the current 60 ms analysis window. High = pitched speech.
    pub fn step(&mut self, frame: &[f32]) -> f32 {
        self.buf.extend_from_slice(frame);
        let excess = self.buf.len().saturating_sub(WINDOW);
        if excess > 0 {
            self.buf.drain(..excess);
        }
        let w = &self.buf;
        let n = w.len();
        if n < WINDOW / 2 {
            return self.smoothed;
        }

        let energy: f32 = w.iter().map(|s| s * s).sum::<f32>() / n as f32;
        let rms = energy.sqrt();

        let mut crossings = 0usize;
        for i in 1..n {
            if (w[i - 1] >= 0.0) != (w[i] >= 0.0) {
                crossings += 1;
            }
        }
        let zcr = crossings as f32 / n as f32;

        if rms < ENERGY_FLOOR {
            self.smoothed += (0.0 - self.smoothed) * SMOOTH_ALPHA;
            return self.smoothed;
        }

        // Normalized autocorrelation peak over the pitch lag range.
        let lag_min = (SAMPLE_RATE as f32 / F0_MAX_HZ) as usize; // 40
        let lag_max = ((SAMPLE_RATE as f32 / F0_MIN_HZ) as usize).min(n - 1); // ~266
        let mut best_r = 0.0f32;
        for lag in lag_min..=lag_max {
            let m = n - lag;
            let mut num = 0.0f32;
            let mut e0 = 0.0f32;
            let mut e1 = 0.0f32;
            for i in 0..m {
                num += w[i] * w[i + lag];
                e0 += w[i] * w[i];
                e1 += w[i + lag] * w[i + lag];
            }
            let denom = (e0 * e1).sqrt().max(1e-9);
            let r = num / denom;
            if r > best_r {
                best_r = r;
            }
        }

        // Fuse: a strong periodic peak says voiced; a high zero-crossing rate
        // says hissy/unvoiced. Map r 0.4..0.9 -> 0..1, penalize high zcr.
        let periodicity = ((best_r - 0.4) / 0.5).clamp(0.0, 1.0);
        let zcr_penalty = ((zcr - 0.18) / 0.25).clamp(0.0, 1.0);
        let raw = (periodicity * (1.0 - 0.7 * zcr_penalty)).clamp(0.0, 1.0);

        self.smoothed += (raw - self.smoothed) * SMOOTH_ALPHA;
        self.smoothed
    }
}

impl Default for VoicingDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    const FRAME: usize = 480; // 30 ms at 16 kHz

    fn run(det: &mut VoicingDetector, samples: &[f32]) -> Vec<f32> {
        samples.chunks(FRAME).map(|f| det.step(f)).collect()
    }

    #[test]
    fn pure_tone_reads_voiced() {
        let n = SAMPLE_RATE * 2;
        let tone: Vec<f32> = (0..n)
            .map(|i| 0.2 * (TAU * 150.0 * i as f32 / SAMPLE_RATE as f32).sin())
            .collect();
        let mut det = VoicingDetector::new();
        let results = run(&mut det, &tone);
        let tail = &results[results.len() / 2..];
        let mean: f32 = tail.iter().sum::<f32>() / tail.len() as f32;
        assert!(mean > 0.7, "tone should be clearly voiced, got {mean}");
    }

    #[test]
    fn whisper_like_noise_reads_unvoiced() {
        // Deterministic LCG noise, high-passed like a whisper.
        let mut state = 12345u64;
        let mut prev = 0.0f32;
        let n = SAMPLE_RATE * 2;
        let noise: Vec<f32> = (0..n)
            .map(|_| {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let white = ((state >> 33) as f32 / u32::MAX as f32) * 2.0 - 1.0;
                let hp = white - prev;
                prev = white;
                0.08 * hp
            })
            .collect();
        let mut det = VoicingDetector::new();
        let results = run(&mut det, &noise);
        let tail = &results[results.len() / 2..];
        let mean: f32 = tail.iter().sum::<f32>() / tail.len() as f32;
        assert!(mean < 0.3, "noise should be unvoiced, got {mean}");
    }

    #[test]
    fn silence_reads_unvoiced_zero() {
        let silence = vec![0.0f32; SAMPLE_RATE];
        let mut det = VoicingDetector::new();
        let results = run(&mut det, &silence);
        assert!(*results.last().unwrap() < 0.05);
    }
}
