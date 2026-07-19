//! Whisper Mode dryness score: does the sound arrive DIRECT (near the mic) or
//! smeared with room reverberation (far away)? This is the single-microphone
//! proxy for PROXIMITY (round 20; ported from the vaporly-lab testbed).
//!
//! It approximates the direct-to-reverberant ratio, the feature the acoustics
//! literature uses for source distance. Robust DRR methods need two mics, so
//! this stays deliberately the SOFTEST veto signal: the shipping threshold is
//! conservative and the per-mic calibration is what earns it trust. Two cues,
//! computed on the per-frame rms envelope over a rolling ~1.2 s window:
//!
//! 1. Modulation depth: close speech has deep valleys between syllables; a
//!    room's reverb tail fills those valleys in, flattening the envelope.
//! 2. Decay rate: when a syllable ends, direct sound stops fast; reverberant
//!    sound rings down slowly.
//!
//! Output: 0..1 where 1 = bone dry (very close), 0 = washy (far). Neutral 0.5
//! when there is not enough signal to judge. Cost per frame is a sort of 40
//! floats plus envelope stats: microseconds.

use std::collections::VecDeque;

const WINDOW_FRAMES: usize = 40; // ~1.2 s of 30 ms frames
/// Envelope activity threshold as a multiple of the tracked noise floor.
const ACTIVE_FLOOR_MULT: f32 = 3.0;
/// Weight of modulation depth vs decay rate in the fused score.
const DEPTH_WEIGHT: f32 = 0.6;

#[derive(Debug, Clone)]
pub struct DrynessAnalyzer {
    env: VecDeque<f32>,
    noise_floor: f32,
    score: f32,
}

impl DrynessAnalyzer {
    pub fn new() -> Self {
        Self {
            env: VecDeque::with_capacity(WINDOW_FRAMES),
            noise_floor: 1e-4,
            score: 0.5,
        }
    }

    pub fn reset(&mut self) {
        self.env.clear();
        self.noise_floor = 1e-4;
        self.score = 0.5;
    }

    /// Feed one frame's raw rms; returns the current dryness score (0..1).
    pub fn step(&mut self, rms: f32) -> f32 {
        // Track the noise floor: fast to fall, very slow to rise.
        if rms < self.noise_floor {
            self.noise_floor += (rms - self.noise_floor) * 0.3;
        } else {
            self.noise_floor *= 1.005;
        }
        self.noise_floor = self.noise_floor.max(1e-5);

        self.env.push_back(rms);
        if self.env.len() > WINDOW_FRAMES {
            self.env.pop_front();
        }
        if self.env.len() < WINDOW_FRAMES / 2 {
            return self.score;
        }

        let mut sorted: Vec<f32> = self.env.iter().copied().collect();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let p10 = sorted[sorted.len() / 10];
        let p90 = sorted[sorted.len() * 9 / 10];

        // Only judge when the window actually contains signal.
        if p90 < self.noise_floor * ACTIVE_FLOOR_MULT {
            // Drift back toward neutral while idle.
            self.score += (0.5 - self.score) * 0.05;
            return self.score;
        }

        // Cue 1: modulation depth. Dry speech: deep valleys => p10 << p90.
        let depth = (1.0 - p10 / p90.max(1e-6)).clamp(0.0, 1.0);
        // Reverb keeps depth high-ish too, so stretch the useful range:
        // typical dry speech ~0.9+, reverberant ~0.6-0.85.
        let depth_score = ((depth - 0.55) / 0.40).clamp(0.0, 1.0);

        // Cue 2: decay rate. Mean relative fall between consecutive frames on
        // falling segments; direct sound stops faster than a reverb tail.
        let mut falls = 0.0f32;
        let mut fall_n = 0usize;
        let frames: Vec<f32> = self.env.iter().copied().collect();
        for i in 1..frames.len() {
            let prev = frames[i - 1];
            let cur = frames[i];
            if prev > self.noise_floor * ACTIVE_FLOOR_MULT && cur < prev {
                falls += (prev - cur) / prev;
                fall_n += 1;
            }
        }
        let mean_fall = if fall_n > 0 {
            falls / fall_n as f32
        } else {
            0.0
        };
        // Map: reverberant tails fall ~5-15% per 30 ms, dry stops ~25-60%.
        let decay_score = ((mean_fall - 0.10) / 0.30).clamp(0.0, 1.0);

        let fused = DEPTH_WEIGHT * depth_score + (1.0 - DEPTH_WEIGHT) * decay_score;
        self.score += (fused - self.score) * 0.15;
        self.score
    }
}

impl Default for DrynessAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mean dryness over the active frames of a synthetic rms envelope.
    fn mean_active_dryness(envelope: &[f32]) -> f32 {
        let mut d = DrynessAnalyzer::new();
        let mut scores = Vec::new();
        for &rms in envelope {
            let s = d.step(rms);
            if rms > 0.01 {
                scores.push(s);
            }
        }
        assert!(!scores.is_empty(), "no active frames in the envelope");
        scores.iter().sum::<f32>() / scores.len() as f32
    }

    #[test]
    fn dry_speech_scores_drier_than_reverberant_speech() {
        // Dry: syllable bursts with sharp offsets into near-silence gaps.
        let mut dry = Vec::new();
        // Wet: the same bursts, but each offset rings down slowly (~8% fall
        // per 30 ms frame, a room tail) instead of stopping.
        let mut wet = Vec::new();
        for _ in 0..40 {
            for _ in 0..8 {
                dry.push(0.05);
                wet.push(0.05);
            }
            let mut tail = 0.05f32;
            for _ in 0..8 {
                dry.push(0.001);
                tail *= 0.92;
                wet.push(tail);
            }
        }
        let dry_score = mean_active_dryness(&dry);
        let wet_score = mean_active_dryness(&wet);
        assert!(
            dry_score > wet_score + 0.05,
            "dry ({dry_score:.3}) should clearly exceed reverberant ({wet_score:.3})"
        );
    }

    #[test]
    fn silence_stays_neutral() {
        let mut d = DrynessAnalyzer::new();
        let mut last = 0.5;
        for _ in 0..200 {
            last = d.step(0.0002);
        }
        assert!(
            (last - 0.5).abs() < 0.1,
            "idle input should hover near neutral, got {last}"
        );
    }
}
