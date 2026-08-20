//! Pure signal-assertion helpers for the strict live-audio smoke
//! (`audio::live_audio_smoke`, feature `live-audio-smoke`).
//!
//! Deliberately NOT feature-gated and NOT device-dependent: every helper
//! here is a plain function over sample buffers, and the unit tests below
//! prove they actually FAIL on silence and on clipping. The strict smoke
//! imports these same functions rather than re-implementing the math behind
//! the feature flag — untested arithmetic hidden behind a device-only
//! feature is exactly how a "silence still passes" bug would go unnoticed
//! (seed audio-graph-f166: "the test must FAIL on silence").

use std::time::Duration;

/// Everything that can go wrong when a captured buffer is checked against
/// the expectations recorded in a fixture manifest.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SignalAssertionError {
    /// No capture buffers arrived at all.
    NoBuffers,
    /// Buffers arrived but carried zero frames total.
    NoFrames,
    /// RMS was below the fixture's declared floor — the classic "recorded
    /// silence instead of the fixture" failure.
    RmsBelowFloor { rms: f64, floor: f64 },
    /// RMS was above the fixture's declared ceiling — clipping, gain
    /// runaway, or capturing the wrong (much louder) source.
    RmsAboveCeiling { rms: f64, ceiling: f64 },
    /// Too large a fraction of samples sat at (or near) full scale.
    ClippingRateAboveMax { rate: f64, max_rate: f64 },
    /// A calibration tone's energy was not concentrated in its target bin —
    /// i.e. what was captured was not a clean tone at that frequency.
    ToneEnergyBelowMin { fraction: f64, min_fraction: f64 },
    /// Consecutive capture timestamps went backwards.
    NonMonotonicTimestamp { index: usize },
    /// Two consecutive capture timestamps were farther apart than allowed —
    /// a stall/dropout in the capture stream.
    TimestampGapTooLarge {
        index: usize,
        gap: Duration,
        max_gap: Duration,
    },
}

impl std::fmt::Display for SignalAssertionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoBuffers => write!(f, "no capture buffers arrived"),
            Self::NoFrames => write!(f, "capture buffers arrived but carried zero frames"),
            Self::RmsBelowFloor { rms, floor } => {
                write!(
                    f,
                    "RMS {rms} is below the required floor {floor} (looks like silence)"
                )
            }
            Self::RmsAboveCeiling { rms, ceiling } => {
                write!(f, "RMS {rms} is above the allowed ceiling {ceiling}")
            }
            Self::ClippingRateAboveMax { rate, max_rate } => {
                write!(
                    f,
                    "clipping rate {rate} exceeds the allowed maximum {max_rate}"
                )
            }
            Self::ToneEnergyBelowMin {
                fraction,
                min_fraction,
            } => write!(
                f,
                "single-bin tone energy fraction {fraction} is below the required minimum {min_fraction}"
            ),
            Self::NonMonotonicTimestamp { index } => {
                write!(
                    f,
                    "timestamp at index {index} is earlier than the previous one"
                )
            }
            Self::TimestampGapTooLarge {
                index,
                gap,
                max_gap,
            } => write!(
                f,
                "timestamp gap at index {index} ({gap:?}) exceeds the maximum allowed gap ({max_gap:?})"
            ),
        }
    }
}

impl std::error::Error for SignalAssertionError {}

/// Root-mean-square amplitude of `samples`. `0.0` for an empty slice.
pub fn rms(samples: &[i16]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f64 = samples.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    (sum_sq / samples.len() as f64).sqrt()
}

/// Fraction of `samples` whose absolute value is at or above
/// `i16::MAX - margin` — i.e. at or near full-scale. `0.0` for an empty
/// slice.
pub fn clipping_rate(samples: &[i16], margin: i16) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let threshold = i16::MAX.saturating_sub(margin.max(0));
    let clipped = samples
        .iter()
        .filter(|&&s| s.unsigned_abs() >= threshold.unsigned_abs())
        .count();
    clipped as f64 / samples.len() as f64
}

/// Fraction of `samples`' total spectral energy that sits in the DFT bin
/// nearest `target_hz` plus its two immediate neighbours, computed with the
/// Goertzel algorithm (an exact single-bin DFT term — no FFT crate needed)
/// run three times. By Parseval's theorem a real signal's total DFT energy
/// is `n * sum(x_i^2)`; a pure sinusoid whose frequency lands exactly on a
/// bin puts very close to half of that energy in this bin (the other half
/// in its mirror bin), so the fraction is bounded in `[0.0, 0.5]` for a
/// real-valued signal and near `0.0` for silence, broadband noise, or a
/// tone at the wrong frequency.
///
/// Summing the nearest bin's two neighbours matters because the analysed
/// buffer length is not chosen to make `target_hz` land exactly on a bin
/// (it is whatever number of samples a live capture happens to accumulate
/// before its deadline). When the true frequency sits between two bins
/// ("scalloping loss"), a single-bin read can lose most of a clean tone's
/// energy to the adjacent bin it leaked into — up to ~59% of it in the
/// worst case (frequency exactly halfway between two bins) — which can
/// spuriously fail a manifest's minimum-fraction threshold on a perfectly
/// healthy signal. Summing the two straddling bins recovers that spread
/// energy while still reading close to zero for broadband noise or an
/// unrelated frequency, where energy is not locally concentrated at all.
pub fn single_bin_energy_fraction(samples: &[i16], sample_rate: u32, target_hz: f64) -> f64 {
    let n = samples.len();
    if n == 0 || sample_rate == 0 {
        return 0.0;
    }
    let total_power: f64 = samples.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    if total_power <= 0.0 {
        // All-silence input: defined as zero energy anywhere, not NaN.
        return 0.0;
    }

    let center_k = (0.5 + (n as f64 * target_hz) / f64::from(sample_rate)).floor() as i64;
    let bin_power: f64 = [center_k - 1, center_k, center_k + 1]
        .into_iter()
        .filter(|&k| k >= 0)
        .map(|k| goertzel_bin_power(samples, k as usize, n))
        .sum();

    bin_power / (n as f64 * total_power)
}

/// Power of DFT bin `k` (out of `n`) via the Goertzel algorithm — an exact
/// single-bin DFT term computed in O(n) without an FFT crate.
fn goertzel_bin_power(samples: &[i16], k: usize, n: usize) -> f64 {
    let omega = 2.0 * std::f64::consts::PI * k as f64 / n as f64;
    let coeff = 2.0 * omega.cos();

    let (mut s_prev, mut s_prev2) = (0.0f64, 0.0f64);
    for &sample in samples {
        let s = f64::from(sample) + coeff * s_prev - s_prev2;
        s_prev2 = s_prev;
        s_prev = s;
    }
    let real = s_prev - s_prev2 * omega.cos();
    let imag = s_prev2 * omega.sin();
    real * real + imag * imag
}

/// At least one buffer and at least one frame arrived.
pub fn assert_nonzero_buffers_and_frames(
    buffers: usize,
    frames: usize,
) -> Result<(), SignalAssertionError> {
    if buffers == 0 {
        return Err(SignalAssertionError::NoBuffers);
    }
    if frames == 0 {
        return Err(SignalAssertionError::NoFrames);
    }
    Ok(())
}

/// RMS of `samples` falls within `[floor, ceiling]`. Returns the computed
/// RMS on success so callers can log it.
pub fn assert_rms_in_range(
    samples: &[i16],
    floor: f64,
    ceiling: f64,
) -> Result<f64, SignalAssertionError> {
    let value = rms(samples);
    if value < floor {
        return Err(SignalAssertionError::RmsBelowFloor { rms: value, floor });
    }
    if value > ceiling {
        return Err(SignalAssertionError::RmsAboveCeiling {
            rms: value,
            ceiling,
        });
    }
    Ok(value)
}

/// Clipping rate of `samples` (margin `128`, i.e. within ~0.4% of full
/// scale counts as clipped) does not exceed `max_rate`. Returns the
/// computed rate on success.
pub fn assert_clipping_rate_below(
    samples: &[i16],
    margin: i16,
    max_rate: f64,
) -> Result<f64, SignalAssertionError> {
    let rate = clipping_rate(samples, margin);
    if rate > max_rate {
        return Err(SignalAssertionError::ClippingRateAboveMax { rate, max_rate });
    }
    Ok(rate)
}

/// `single_bin_energy_fraction` at `target_hz` meets `min_fraction`. Returns
/// the computed fraction on success.
pub fn assert_single_bin_tone_energy(
    samples: &[i16],
    sample_rate: u32,
    target_hz: f64,
    min_fraction: f64,
) -> Result<f64, SignalAssertionError> {
    let fraction = single_bin_energy_fraction(samples, sample_rate, target_hz);
    if fraction < min_fraction {
        return Err(SignalAssertionError::ToneEnergyBelowMin {
            fraction,
            min_fraction,
        });
    }
    Ok(fraction)
}

/// `timestamps` is non-decreasing and no consecutive gap exceeds `max_gap`.
pub fn assert_monotonic_timestamps(
    timestamps: &[Duration],
    max_gap: Duration,
) -> Result<(), SignalAssertionError> {
    for index in 1..timestamps.len() {
        if timestamps[index] < timestamps[index - 1] {
            return Err(SignalAssertionError::NonMonotonicTimestamp { index });
        }
        let gap = timestamps[index] - timestamps[index - 1];
        if gap > max_gap {
            return Err(SignalAssertionError::TimestampGapTooLarge {
                index,
                gap,
                max_gap,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::signal_synth::sine_tone_i16;

    const SAMPLE_RATE_HZ: u32 = 16_000;

    fn silence(len: usize) -> Vec<i16> {
        vec![0; len]
    }

    fn full_scale_clip(len: usize) -> Vec<i16> {
        vec![i16::MAX; len]
    }

    // ---- "must FAIL on silence" (seed audio-graph-f166 acceptance) ----

    #[test]
    fn rms_of_silence_is_zero() {
        assert_eq!(rms(&silence(1_000)), 0.0);
    }

    #[test]
    fn silence_fails_the_rms_floor_assertion() {
        let samples = silence(16_000);
        let error = assert_rms_in_range(&samples, 4_000.0, 20_000.0)
            .expect_err("silence must fail an RMS floor check");
        assert_eq!(
            error,
            SignalAssertionError::RmsBelowFloor {
                rms: 0.0,
                floor: 4_000.0
            }
        );
    }

    #[test]
    fn silence_fails_the_single_bin_tone_energy_assertion() {
        let samples = silence(32_000);
        let error = assert_single_bin_tone_energy(&samples, SAMPLE_RATE_HZ, 440.0, 0.25)
            .expect_err("silence must fail the tone-energy check");
        assert_eq!(
            error,
            SignalAssertionError::ToneEnergyBelowMin {
                fraction: 0.0,
                min_fraction: 0.25
            }
        );
    }

    #[test]
    fn nonzero_buffers_and_frames_rejects_zero_of_either() {
        assert_eq!(
            assert_nonzero_buffers_and_frames(0, 10),
            Err(SignalAssertionError::NoBuffers)
        );
        assert_eq!(
            assert_nonzero_buffers_and_frames(10, 0),
            Err(SignalAssertionError::NoFrames)
        );
        assert_eq!(assert_nonzero_buffers_and_frames(1, 1), Ok(()));
    }

    // ---- "must FAIL on clipping" (seed audio-graph-f166 acceptance) ----

    #[test]
    fn clipping_rate_of_full_scale_buffer_is_one() {
        assert_eq!(clipping_rate(&full_scale_clip(500), 128), 1.0);
    }

    #[test]
    fn clipping_fails_the_clipping_rate_assertion() {
        let samples = full_scale_clip(500);
        let error = assert_clipping_rate_below(&samples, 128, 0.01)
            .expect_err("a fully clipped buffer must fail the clipping-rate check");
        assert_eq!(
            error,
            SignalAssertionError::ClippingRateAboveMax {
                rate: 1.0,
                max_rate: 0.01
            }
        );
    }

    #[test]
    fn clean_tone_has_a_negligible_clipping_rate() {
        let samples = sine_tone_i16(440.0, 0.5, 2_000, SAMPLE_RATE_HZ);
        let rate = assert_clipping_rate_below(&samples, 128, 0.01)
            .expect("a healthy 0.5-amplitude tone must not read as clipped");
        assert_eq!(rate, 0.0);
    }

    // ---- healthy-signal detection (the assertions must also PASS) ----

    #[test]
    fn healthy_tone_passes_the_rms_range_check() {
        let samples = sine_tone_i16(440.0, 0.5, 2_000, SAMPLE_RATE_HZ);
        let value =
            assert_rms_in_range(&samples, 4_000.0, 20_000.0).expect("healthy tone must pass");
        assert!(value > 4_000.0 && value < 20_000.0);
    }

    #[test]
    fn pure_tone_concentrates_energy_in_its_own_bin_far_above_threshold() {
        let samples = sine_tone_i16(440.0, 0.5, 2_000, SAMPLE_RATE_HZ);
        let fraction = assert_single_bin_tone_energy(&samples, SAMPLE_RATE_HZ, 440.0, 0.25)
            .expect("a clean 440 Hz tone must clear a 0.25 single-bin-energy threshold");
        // 440 Hz over 2000 ms at 16 kHz is an exact 880-cycle window (no
        // spectral leakage), so the fraction should sit close to the
        // real-signal theoretical maximum of 0.5.
        assert!(
            fraction > 0.4,
            "expected close to the 0.5 ideal, got {fraction}"
        );
    }

    #[test]
    fn scalloping_loss_at_an_off_bin_capture_length_still_clears_the_threshold() {
        // Regression for the "must FAIL on silence, must NOT spuriously fail
        // on a healthy tone" acceptance: at 16 kHz, a 1508ms 440 Hz tone
        // gives an exact-frequency bin index of 663.52 — 0.48 off-centre,
        // close to the worst-case 0.5 scalloping loss. The single-bin-only
        // algorithm reads a fraction of ~0.219 here (below the manifest's
        // 0.25 minimum) on a perfectly clean tone; summing the nearest
        // bin's two neighbours recovers the spread energy.
        let samples = sine_tone_i16(440.0, 0.5, 1_508, SAMPLE_RATE_HZ);
        let fraction = assert_single_bin_tone_energy(&samples, SAMPLE_RATE_HZ, 440.0, 0.25)
            .expect("a clean tone at an off-bin capture length must still clear the threshold");
        assert!(
            fraction > 0.25,
            "expected the 3-bin sum to clear the threshold, got {fraction}"
        );
    }

    #[test]
    fn pure_tone_does_not_concentrate_energy_in_an_unrelated_bin() {
        let samples = sine_tone_i16(440.0, 0.5, 2_000, SAMPLE_RATE_HZ);
        let fraction = single_bin_energy_fraction(&samples, SAMPLE_RATE_HZ, 3_000.0);
        assert!(
            fraction < 0.05,
            "a 440 Hz tone should show negligible energy at 3000 Hz, got {fraction}"
        );
    }

    #[test]
    fn white_noise_like_alternating_signal_does_not_pass_as_a_pure_tone() {
        // A cheap, deterministic noise-ish signal: not a fixture concern
        // (no RNG dependency here), just something broadband enough that no
        // single bin should dominate the way a pure tone does.
        let samples: Vec<i16> = (0..32_000u32)
            .map(|i| ((i.wrapping_mul(2_654_435_761) % 20_000) as i32 - 10_000) as i16)
            .collect();
        let fraction = single_bin_energy_fraction(&samples, SAMPLE_RATE_HZ, 440.0);
        assert!(
            fraction < 0.25,
            "a broadband signal should not clear the tone-energy bar, got {fraction}"
        );
    }

    // ---- timestamps ----

    #[test]
    fn monotonic_timestamps_within_gap_pass() {
        let timestamps = vec![
            Duration::from_millis(0),
            Duration::from_millis(20),
            Duration::from_millis(41),
            Duration::from_millis(60),
        ];
        assert_eq!(
            assert_monotonic_timestamps(&timestamps, Duration::from_millis(50)),
            Ok(())
        );
    }

    #[test]
    fn non_monotonic_timestamp_fails() {
        let timestamps = vec![
            Duration::from_millis(20),
            Duration::from_millis(10),
            Duration::from_millis(40),
        ];
        assert_eq!(
            assert_monotonic_timestamps(&timestamps, Duration::from_millis(50)),
            Err(SignalAssertionError::NonMonotonicTimestamp { index: 1 })
        );
    }

    #[test]
    fn a_large_gap_fails_even_though_monotonic() {
        let timestamps = vec![
            Duration::from_millis(0),
            Duration::from_millis(20),
            Duration::from_millis(500),
        ];
        assert_eq!(
            assert_monotonic_timestamps(&timestamps, Duration::from_millis(50)),
            Err(SignalAssertionError::TimestampGapTooLarge {
                index: 2,
                gap: Duration::from_millis(480),
                max_gap: Duration::from_millis(50),
            })
        );
    }

    #[test]
    fn empty_and_single_timestamp_lists_trivially_pass() {
        assert_eq!(
            assert_monotonic_timestamps(&[], Duration::from_millis(50)),
            Ok(())
        );
        assert_eq!(
            assert_monotonic_timestamps(&[Duration::from_millis(1)], Duration::from_millis(50)),
            Ok(())
        );
    }
}
