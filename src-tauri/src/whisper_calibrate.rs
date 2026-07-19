//! Whisper Mode per-mic calibration math (round 20; ported from the
//! vaporly-lab testbed). The wizard measures THIS mic in THIS room instead of
//! trusting fixed ceilings - fixed constants are exactly what forced the
//! R16-R19 recalibrations, and the acoustics literature is blunt that
//! single-mic distance cues only become trustworthy after calibrating on the
//! speaker's own voice.
//!
//! Pure functions only: the ~8 s three-phase capture (ambient, normal voice,
//! whisper) lives in `commands::audio`; this module turns the three measured
//! mean levels into the stored [`WhisperCalibration`].

use crate::settings::WhisperCalibration;

/// Per-frame rms statistics for one wizard phase: (mean, 75th percentile).
///
/// Phases are user-terminated (wizard v2), so a recording usually carries
/// trailing silence while the user reaches for the Continue button. A plain
/// mean gets dragged down by those gap frames (ceilings land too low and the
/// gate then eats real whispers); the 75th percentile sits inside the speech
/// frames and barely moves when silence is appended. The wizard uses the
/// MEAN for the ambient phase (uniform silence) and the P75 for the voice
/// phases.
pub fn phase_stats(samples: &[f32]) -> (f32, f32) {
    const FRAME: usize = 480; // 30 ms at 16 kHz, matching the gate
    let mut frames: Vec<f32> = samples
        .chunks(FRAME)
        .filter(|f| !f.is_empty())
        .map(|f| (f.iter().map(|s| s * s).sum::<f32>() / f.len() as f32).sqrt())
        .collect();
    if frames.is_empty() {
        return (0.0, 0.0);
    }
    let mean = (frames.iter().map(|&r| f64::from(r)).sum::<f64>() / frames.len() as f64) as f32;
    frames.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p75 = frames[(frames.len() * 3 / 4).min(frames.len() - 1)];
    (mean, p75)
}

/// Turn the three measured phase levels into per-strength ceilings, a veto
/// energy floor, and a separation verdict.
pub fn recommend(
    device_name: String,
    ambient: f32,
    normal: f32,
    whisper: f32,
) -> WhisperCalibration {
    let ambient = ambient.max(1e-5);
    let normal = normal.max(1e-5);
    let whisper = whisper.max(1e-5);

    // How cleanly this mic separates a whisper from a normal voice.
    let ratio = normal / whisper;
    let separation = if ratio >= 3.0 {
        "good"
    } else if ratio >= 2.0 {
        "workable"
    } else {
        "poor"
    };

    // High accepts only whispers: sit just above the measured whisper level.
    let high = whisper * 1.6;
    // Medium accepts quiet talk, rejects a normal voice: the geometric mean
    // splits the gap evenly in log space (kept above High so the ladder holds).
    let medium = (whisper * normal).sqrt().max(high * 1.1);
    // Light rejects only clearly-loud sources.
    let light = (normal * 1.8).max(medium * 1.1);

    // The veto energy floor sits just above the room: twice the ambient,
    // never above half the whisper level (a floor that eats whispers would
    // defeat the mode), never below an absolute minimum.
    let energy_floor = (ambient * 2.0).clamp(0.002, (whisper * 0.5).max(0.002));

    WhisperCalibration {
        device_name,
        ambient_rms: ambient,
        normal_rms: normal,
        whisper_rms: whisper,
        light_ceiling: light,
        medium_ceiling: medium,
        high_ceiling: high,
        energy_floor,
        separation: separation.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn healthy_mic_gets_good_separation_and_ordered_ceilings() {
        // Numbers shaped like the owner's real R17 measurements.
        let r = recommend("default".into(), 0.002, 0.045, 0.012);
        assert_eq!(r.separation, "good");
        assert!(r.high_ceiling > r.whisper_rms);
        assert!(r.medium_ceiling > r.high_ceiling);
        assert!(r.light_ceiling > r.medium_ceiling);
        assert!(
            r.medium_ceiling < r.normal_rms,
            "medium must reject a normal voice"
        );
        // The floor sits above the room but well under the whisper.
        assert!(r.energy_floor > r.ambient_rms);
        assert!(r.energy_floor <= r.whisper_rms * 0.5);
    }

    #[test]
    fn weak_mic_flags_poor_separation() {
        let r = recommend("default".into(), 0.010, 0.020, 0.013);
        assert_eq!(r.separation, "poor");
    }

    #[test]
    fn p75_ignores_trailing_gaps_that_drag_the_mean() {
        // A bursty phase: 8 loud frames then 8 near-silent frames, repeated,
        // plus a long trailing fumble-gap before the user presses Continue.
        let mut samples = Vec::new();
        for _ in 0..10 {
            samples.extend(std::iter::repeat(0.05f32).take(480 * 8));
            samples.extend(std::iter::repeat(0.001f32).take(480 * 8));
        }
        samples.extend(std::iter::repeat(0.001f32).take(480 * 40)); // trailing gap
        let (mean, p75) = phase_stats(&samples);
        // The mean is dragged toward silence; p75 stays at the speech level.
        assert!(mean < 0.03, "mean should sink with the gaps, got {mean}");
        assert!(
            (p75 - 0.05).abs() < 0.005,
            "p75 should sit at the burst level, got {p75}"
        );
    }

    #[test]
    fn floor_never_eats_the_whisper() {
        // A loud room: ambient*2 would exceed half the whisper level, so the
        // floor caps at whisper/2 instead of gating the whisper itself.
        let r = recommend("default".into(), 0.008, 0.05, 0.012);
        assert!(r.energy_floor <= 0.006 + 1e-6);
        // And a silent room still yields a nonzero floor.
        let r = recommend("default".into(), 0.0001, 0.05, 0.012);
        assert!(r.energy_floor >= 0.002);
    }
}
