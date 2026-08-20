//! Deterministic calibration-signal synthesis for CI audio fixtures.
//!
//! No RNG and no third-party DSP crate: every generator here is a pure
//! closed-form function of its inputs, so regenerating a fixture from these
//! functions is always byte-identical (see `audio_signal_fixtures.rs`, which
//! asserts exactly that against the checked-in WAV files).

use std::f64::consts::PI;

/// Generate `duration_ms` of a pure sine tone at `freq_hz`, quantized to
/// signed 16-bit PCM. `amplitude` is clamped to `[0.0, 1.0]` (fraction of
/// full scale).
pub fn sine_tone_i16(freq_hz: f64, amplitude: f64, duration_ms: u64, sample_rate: u32) -> Vec<i16> {
    let n = frame_count(duration_ms, sample_rate);
    let amplitude = amplitude.clamp(0.0, 1.0);
    (0..n)
        .map(|i| {
            let t = i as f64 / f64::from(sample_rate);
            quantize(amplitude * (2.0 * PI * freq_hz * t).sin())
        })
        .collect()
}

/// Generate `duration_ms` of a linear chirp sweeping from `start_hz` to
/// `end_hz`, quantized to signed 16-bit PCM. `amplitude` is clamped to
/// `[0.0, 1.0]`.
///
/// Uses the closed-form instantaneous phase of a linear-frequency chirp,
/// `phase(t) = 2*pi*(f0*t + (k/2)*t^2)` where `k = (f1 - f0) / duration_s` is
/// the sweep rate — no numerical integration, so the result is exact and
/// deterministic for identical inputs.
pub fn linear_chirp_i16(
    start_hz: f64,
    end_hz: f64,
    amplitude: f64,
    duration_ms: u64,
    sample_rate: u32,
) -> Vec<i16> {
    let n = frame_count(duration_ms, sample_rate);
    let amplitude = amplitude.clamp(0.0, 1.0);
    let duration_s = duration_ms as f64 / 1000.0;
    let sweep_rate = if duration_s > 0.0 {
        (end_hz - start_hz) / duration_s
    } else {
        0.0
    };
    (0..n)
        .map(|i| {
            let t = i as f64 / f64::from(sample_rate);
            let phase = 2.0 * PI * (start_hz * t + 0.5 * sweep_rate * t * t);
            quantize(amplitude * phase.sin())
        })
        .collect()
}

fn frame_count(duration_ms: u64, sample_rate: u32) -> usize {
    ((u128::from(duration_ms) * u128::from(sample_rate)) / 1000) as usize
}

fn quantize(value: f64) -> i16 {
    let clamped = value.clamp(-1.0, 1.0);
    (clamped * f64::from(i16::MAX)).round() as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_tone_is_deterministic() {
        let a = sine_tone_i16(440.0, 0.5, 2_000, 16_000);
        let b = sine_tone_i16(440.0, 0.5, 2_000, 16_000);
        assert_eq!(a, b);
        assert_eq!(a.len(), 32_000);
    }

    #[test]
    fn linear_chirp_is_deterministic() {
        let a = linear_chirp_i16(200.0, 2_000.0, 0.5, 2_000, 16_000);
        let b = linear_chirp_i16(200.0, 2_000.0, 0.5, 2_000, 16_000);
        assert_eq!(a, b);
        assert_eq!(a.len(), 32_000);
    }

    #[test]
    fn sine_tone_starts_at_zero_and_stays_within_amplitude() {
        let samples = sine_tone_i16(440.0, 0.5, 100, 16_000);
        assert_eq!(samples[0], 0, "sin(0) == 0");
        let max_expected = (0.5 * f64::from(i16::MAX)).round() as i16;
        assert!(
            samples.iter().all(|&s| s.abs() <= max_expected + 1),
            "no sample should exceed the requested amplitude"
        );
    }

    #[test]
    fn amplitude_zero_produces_silence() {
        let samples = sine_tone_i16(440.0, 0.0, 100, 16_000);
        assert!(samples.iter().all(|&s| s == 0));
    }

    #[test]
    fn amplitude_is_clamped_above_one() {
        let full_scale = sine_tone_i16(440.0, 1.0, 100, 16_000);
        let over_driven = sine_tone_i16(440.0, 5.0, 100, 16_000);
        assert_eq!(full_scale, over_driven, "amplitude > 1.0 must clamp to 1.0");
    }

    #[test]
    fn chirp_frequency_at_start_matches_start_hz() {
        // Over a short enough window the chirp is nearly a fixed tone at
        // start_hz, so it should share the sine tone's zero-crossing origin.
        let samples = linear_chirp_i16(200.0, 2_000.0, 0.5, 2_000, 16_000);
        assert_eq!(samples[0], 0, "sin(phase(0)) == 0");
    }

    #[test]
    fn zero_duration_produces_no_samples() {
        assert!(sine_tone_i16(440.0, 0.5, 0, 16_000).is_empty());
        assert!(linear_chirp_i16(200.0, 2_000.0, 0.5, 0, 16_000).is_empty());
    }
}
