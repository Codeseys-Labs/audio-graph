//! AEC/VAD fixture harness scaffold (seed audio-graph-098b).
//!
//! This module is the **buildable evidence slice** the parent VAD/AEC crate
//! bakeoff seed (`audio-graph-0bdc`) is blocked on. It deliberately does NOT
//! pull a real acoustic-echo-cancellation candidate (no `sonora`, no
//! `webrtc-audio-processing`) and does NOT wire a runtime. Selecting and wiring
//! a real candidate is the runtime decision `0bdc` owns — and that decision
//! pulls a heavy native dependency under its own guardrail.
//!
//! ## Where the AEC stage conceptually sits
//!
//! ```text
//!   playback::AudioPlayer ── render-reference twin ──┐
//!                                                     ▼
//!   capture (mic / system) ──► [ AEC stage ] ──► downmix/resample ──►
//!                                                  ProcessedAudioChunk bus
//!                                                  (16 kHz mono, pipeline.rs)
//! ```
//!
//! The AEC stage runs **before** the canonical 16 kHz
//! [`ProcessedAudioChunk`](crate::audio::pipeline::ProcessedAudioChunk) bus
//! (the processed-audio bus rate, 16_000 Hz). It consumes an aligned
//! mic/system-capture frame plus the
//! assistant render-reference frame and produces cleaned frames. The seed
//! guardrail: the AEC stage MUST NOT mutate already-emitted ASR
//! `ProcessedAudioChunk`s to *simulate* echo cancellation — it is an upstream
//! producer of clean audio, not a mutator of the downstream bus.
//!
//! Alignment between the capture track and the render-reference track reuses the
//! existing `ProcessedAudioChunk.timestamp` primitive (a wall-clock-relative
//! [`Duration`]); the harness models that as an explicit per-frame `timestamp`
//! so the two streams can be lined up sample-for-sample.

use std::time::Duration;

/// Canonical fixture sample rate (16 kHz mono pcm_s16le). Matches the processed
/// bus rate so harness fixtures need no resample before scoring.
pub const FIXTURE_SAMPLE_RATE_HZ: u32 = 16_000;

/// One aligned frame fed into the AEC stage.
///
/// `capture` is the device-side signal (mic and/or system loopback, already
/// summed for the harness) and `render_reference` is the assistant playback
/// twin — the signal the [`crate::playback::AudioPlayer`] emitted. The AEC
/// stage subtracts the echo of `render_reference` out of `capture`.
///
/// `timestamp` is the alignment anchor. In production this is the
/// `ProcessedAudioChunk.timestamp` of the capture chunk; the render-reference
/// twin is aligned to the same clock.
#[derive(Debug, Clone)]
pub struct AlignedFrame {
    /// Capture-side samples (mic / system), normalized f32 in [-1.0, 1.0].
    pub capture: Vec<f32>,
    /// Assistant render-reference samples, normalized f32 in [-1.0, 1.0].
    /// Empty when the assistant was not rendering (quiet room, keyboard noise).
    pub render_reference: Vec<f32>,
    /// Alignment anchor shared with `ProcessedAudioChunk.timestamp`.
    pub timestamp: Duration,
}

impl AlignedFrame {
    /// Number of capture samples in this frame.
    pub fn len(&self) -> usize {
        self.capture.len()
    }

    /// `true` when the frame carries no capture samples.
    pub fn is_empty(&self) -> bool {
        self.capture.is_empty()
    }

    /// `true` when an assistant render-reference is present and the same length
    /// as the capture track (i.e. the two are sample-aligned).
    pub fn has_aligned_reference(&self) -> bool {
        !self.render_reference.is_empty() && self.render_reference.len() == self.capture.len()
    }
}

/// One cleaned frame produced by an [`AecAdapter`].
#[derive(Debug, Clone)]
pub struct CleanedFrame {
    /// Echo-suppressed capture samples, normalized f32.
    pub samples: Vec<f32>,
    /// Alignment anchor, carried through from the input [`AlignedFrame`].
    pub timestamp: Duration,
}

/// An acoustic-echo-cancellation adapter.
///
/// Implementors take an aligned mic/system-capture frame plus the assistant
/// render-reference frame and return a cleaned frame. The trait is intentionally
/// minimal so a real candidate (selected by seed `0bdc`) and the
/// [`FakeAecAdapter`] can both satisfy it without the harness depending on any
/// native AEC library.
pub trait AecAdapter {
    /// Stable identifier for the adapter (used by the metrics reporter).
    fn name(&self) -> &str;

    /// Process one aligned frame, subtracting the render-reference echo out of
    /// the capture track.
    fn process(&mut self, frame: &AlignedFrame) -> CleanedFrame;

    /// Reset any internal adaptive state between fixtures.
    fn reset(&mut self) {}
}

/// A deterministic, dependency-free AEC adapter used to exercise the harness.
///
/// It models the *shape* of echo cancellation without claiming acoustic
/// fidelity: it subtracts a fixed-gain, fixed-delay copy of the render reference
/// from the capture track. With no render reference it is a pass-through. This
/// is enough to score the harness metrics end-to-end and to prove the fixtures,
/// trait, and reporter all wire together — it is NOT a real AEC.
#[derive(Debug, Clone)]
pub struct FakeAecAdapter {
    name: String,
    /// Echo gain applied to the render reference before subtraction.
    echo_gain: f32,
    /// Render-reference delay in samples (models loudspeaker→mic path latency).
    delay_samples: usize,
    /// Carry-over tail from the previous frame's delayed reference.
    reference_tail: Vec<f32>,
}

impl Default for FakeAecAdapter {
    fn default() -> Self {
        Self::new(0.6, 8)
    }
}

impl FakeAecAdapter {
    /// Create a fake adapter with an explicit echo gain and reference delay.
    pub fn new(echo_gain: f32, delay_samples: usize) -> Self {
        Self {
            name: "fake-aec".to_string(),
            echo_gain,
            delay_samples,
            reference_tail: Vec::new(),
        }
    }
}

impl AecAdapter for FakeAecAdapter {
    fn name(&self) -> &str {
        &self.name
    }

    fn process(&mut self, frame: &AlignedFrame) -> CleanedFrame {
        let mut samples = frame.capture.clone();

        if !frame.render_reference.is_empty() {
            // Build the delayed reference: tail carried from the previous frame,
            // then this frame's reference shifted by `delay_samples`.
            let mut delayed = Vec::with_capacity(self.delay_samples + frame.render_reference.len());
            delayed.extend_from_slice(&self.reference_tail);
            delayed.extend_from_slice(&frame.render_reference);

            for (out, echo) in samples.iter_mut().zip(delayed.iter()) {
                *out -= self.echo_gain * *echo;
                *out = out.clamp(-1.0, 1.0);
            }

            // Stash the tail of this frame's reference for the next frame's
            // delay window so the model stays continuous across frames.
            let ref_len = frame.render_reference.len();
            let tail_start = ref_len.saturating_sub(self.delay_samples);
            self.reference_tail = frame.render_reference[tail_start..].to_vec();
        } else {
            self.reference_tail.clear();
        }

        CleanedFrame {
            samples,
            timestamp: frame.timestamp,
        }
    }

    fn reset(&mut self) {
        self.reference_tail.clear();
    }
}

/// Metrics an AEC candidate is scored on over the fixture set.
///
/// These mirror the seed's required reporting axes. They are populated by the
/// `compute_*` helpers below over the harness fixtures; for the
/// [`FakeAecAdapter`] the numbers are illustrative, not acceptance gates (a real
/// candidate selected by seed `0bdc` produces the real numbers).
#[derive(Debug, Clone, PartialEq)]
pub struct AecMetricsReport {
    /// Fixture class these metrics were computed over.
    pub fixture_class: String,
    /// Adapter that produced the cleaned audio.
    pub adapter: String,
    /// Residual echo energy ratio (cleaned echo energy / reference echo
    /// energy), 0.0 = perfect cancellation, 1.0 = no cancellation. Lower better.
    pub echo_leak: f32,
    /// Fraction of frames where the VAD/barge-in logic would fire a turn-start
    /// on residual echo alone (a false barge-in). Lower better.
    pub false_start_rate: f32,
    /// Fraction of genuine user barge-in frames the cleaned signal would let
    /// the VAD miss (suppressed real speech). Lower better.
    pub missed_barge_in_rate: f32,
    /// Added processing latency per frame attributable to the AEC stage.
    pub latency: Duration,
    /// Real-time factor: processing time / audio duration. < 1.0 is real-time.
    pub rtf: f32,
    /// Estimated CPU cost as a fraction of one core while streaming. 0.0..=1.0.
    pub cpu_fraction: f32,
    /// On-disk footprint of the adapter's binary contribution, in bytes.
    pub binary_footprint_bytes: u64,
    /// On-disk footprint of any model weights the adapter ships, in bytes.
    pub model_footprint_bytes: u64,
}

/// Computes [`AecMetricsReport`]s for an adapter over harness fixtures.
///
/// The compute functions are pure over (reference echo, cleaned output, VAD
/// ground truth) so they can be unit-tested without audio devices and without a
/// real AEC dependency.
#[derive(Debug, Default)]
pub struct AecMetricsReporter;

impl AecMetricsReporter {
    pub fn new() -> Self {
        Self
    }

    /// Echo-leak ratio: residual echo energy in the cleaned signal relative to
    /// the echo energy that was present before cancellation.
    ///
    /// `reference_echo` is the echo component that was mixed into the capture
    /// track; `cleaned` is the adapter's output. Returns 0.0 when there was no
    /// echo to cancel.
    pub fn compute_echo_leak(reference_echo: &[f32], cleaned: &[f32]) -> f32 {
        let echo_energy = energy(reference_echo);
        if echo_energy <= f32::EPSILON {
            return 0.0;
        }
        let residual_energy = energy(cleaned);
        (residual_energy / echo_energy).clamp(0.0, 1.0)
    }

    /// False-start rate: fraction of frames the VAD would mark as speech where
    /// the ground truth says only echo/noise was present.
    pub fn compute_false_start_rate(vad_active: &[bool], ground_truth_speech: &[bool]) -> f32 {
        rate(vad_active, ground_truth_speech, |pred, truth| {
            pred && !truth
        })
    }

    /// Missed-barge-in rate: fraction of genuine-speech frames the VAD missed.
    pub fn compute_missed_barge_in_rate(vad_active: &[bool], ground_truth_speech: &[bool]) -> f32 {
        let total_speech = ground_truth_speech.iter().filter(|&&t| t).count();
        if total_speech == 0 {
            return 0.0;
        }
        let missed = vad_active
            .iter()
            .zip(ground_truth_speech.iter())
            .filter(|&(&pred, &truth)| truth && !pred)
            .count();
        missed as f32 / total_speech as f32
    }

    /// Real-time factor: processing wall time over the audio duration it
    /// covered. Below 1.0 means the adapter keeps up with the stream.
    pub fn compute_rtf(processing: Duration, audio: Duration) -> f32 {
        let audio_s = audio.as_secs_f32();
        if audio_s <= f32::EPSILON {
            return 0.0;
        }
        processing.as_secs_f32() / audio_s
    }

    /// Score one fixture class end-to-end. `reference_echo`/`cleaned` are the
    /// concatenated per-frame echo and cleaned samples; the `vad_*` slices are
    /// per-frame VAD decision vs ground truth; the footprints describe the
    /// adapter binary/model size.
    #[allow(clippy::too_many_arguments)]
    pub fn score_fixture(
        &self,
        fixture_class: impl Into<String>,
        adapter: &dyn AecAdapter,
        reference_echo: &[f32],
        cleaned: &[f32],
        vad_active: &[bool],
        ground_truth_speech: &[bool],
        processing: Duration,
        audio: Duration,
        cpu_fraction: f32,
        binary_footprint_bytes: u64,
        model_footprint_bytes: u64,
    ) -> AecMetricsReport {
        AecMetricsReport {
            fixture_class: fixture_class.into(),
            adapter: adapter.name().to_string(),
            echo_leak: Self::compute_echo_leak(reference_echo, cleaned),
            false_start_rate: Self::compute_false_start_rate(vad_active, ground_truth_speech),
            missed_barge_in_rate: Self::compute_missed_barge_in_rate(
                vad_active,
                ground_truth_speech,
            ),
            latency: processing,
            rtf: Self::compute_rtf(processing, audio),
            cpu_fraction: cpu_fraction.clamp(0.0, 1.0),
            binary_footprint_bytes,
            model_footprint_bytes,
        }
    }
}

fn energy(samples: &[f32]) -> f32 {
    samples.iter().map(|s| s * s).sum()
}

fn rate(pred: &[bool], truth: &[bool], hit: impl Fn(bool, bool) -> bool) -> f32 {
    let n = pred.len().min(truth.len());
    if n == 0 {
        return 0.0;
    }
    let count = (0..n).filter(|&i| hit(pred[i], truth[i])).count();
    count as f32 / n as f32
}

// ---------------------------------------------------------------------------
// Deterministic WAV synthesis helpers.
//
// The harness fixtures are *synthesized* (tones + deterministic pseudo-noise),
// not recorded — they carry no secrets and are byte-for-byte reproducible from a
// seed. The validator regenerates them when missing and parses the checked-in
// headers, so the synthesis lives in non-test code and is reused by both the
// generator and the test. WAVs are written by hand (no `hound` dependency),
// mirroring the hand-rolled parser in `source_separation_fixtures.rs`.
// ---------------------------------------------------------------------------

/// Render an f32 mono signal (in [-1.0, 1.0]) into a 16 kHz pcm_s16le WAV byte
/// buffer with a canonical 44-byte header. No external WAV crate is used.
pub fn encode_wav_pcm_s16le(samples: &[f32], sample_rate: u32) -> Vec<u8> {
    let channels: u16 = 1;
    let bits_per_sample: u16 = 16;
    let byte_rate = sample_rate * u32::from(channels) * u32::from(bits_per_sample / 8);
    let block_align = channels * (bits_per_sample / 8);
    let data_len = (samples.len() * 2) as u32;
    let riff_len = 36 + data_len;

    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_len.to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits_per_sample.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for &s in samples {
        let clamped = s.clamp(-1.0, 1.0);
        let v = (clamped * i16::MAX as f32).round() as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// Deterministic, seedable pseudo-random number generator (xorshift64*) so
/// noise tracks are byte-for-byte reproducible without an `rand` dependency.
struct DeterministicRng(u64);

impl DeterministicRng {
    fn new(seed: u64) -> Self {
        // Avoid the all-zero fixed point of xorshift.
        Self(seed | 1)
    }

    fn next_f32(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        let v = x.wrapping_mul(0x2545_F491_4F6C_DD1D);
        // Map the top 24 bits to [-1.0, 1.0).
        let unit = ((v >> 40) as f32) / (1u32 << 24) as f32;
        unit * 2.0 - 1.0
    }
}

fn samples_for_ms(ms: u64, sample_rate: u32) -> usize {
    (ms as usize * sample_rate as usize) / 1000
}

/// Synthesize a sine tone of `freq_hz` at `amplitude` for `ms` milliseconds.
fn tone(freq_hz: f32, amplitude: f32, ms: u64, sample_rate: u32) -> Vec<f32> {
    let n = samples_for_ms(ms, sample_rate);
    (0..n)
        .map(|i| {
            let t = i as f32 / sample_rate as f32;
            amplitude * (2.0 * std::f32::consts::PI * freq_hz * t).sin()
        })
        .collect()
}

/// Synthesize deterministic band-limited noise for `ms` milliseconds.
fn noise(amplitude: f32, ms: u64, sample_rate: u32, seed: u64) -> Vec<f32> {
    let n = samples_for_ms(ms, sample_rate);
    let mut rng = DeterministicRng::new(seed);
    (0..n).map(|_| amplitude * rng.next_f32()).collect()
}

fn add_in_place(dst: &mut [f32], src: &[f32], offset_samples: usize) {
    for (i, &s) in src.iter().enumerate() {
        let idx = offset_samples + i;
        if idx < dst.len() {
            dst[idx] += s;
        }
    }
}

/// A synthesized fixture: a capture track and the aligned assistant
/// render-reference track. Both are mono f32 at [`FIXTURE_SAMPLE_RATE_HZ`].
#[derive(Debug, Clone)]
pub struct SynthesizedFixture {
    /// Mic/system capture track (what the device hears).
    pub capture: Vec<f32>,
    /// Assistant render-reference twin (what playback emitted). Same length as
    /// `capture` so the two are sample-aligned; silent where the assistant was
    /// not speaking.
    pub render_reference: Vec<f32>,
}

/// Synthesize the five canonical AEC/VAD fixture classes deterministically.
///
/// Every class returns capture+render-reference tracks of equal length so the
/// validator can assert sample-for-sample time alignment. The signals are
/// intentionally simple (tones for "speech", deterministic noise for keyboard /
/// room noise) — the harness scores *shape*, not acoustic realism.
pub fn synthesize_fixture(class: &str, sample_rate: u32) -> SynthesizedFixture {
    let total_ms: u64 = 2000;
    let total = samples_for_ms(total_ms, sample_rate);

    // The assistant "voice" the loudspeaker plays (and which echoes into the mic).
    let assistant = tone(220.0, 0.5, total_ms, sample_rate);
    // The user's "voice" (a distinct fundamental so VAD can tell them apart).
    let user = tone(440.0, 0.45, 1000, sample_rate);

    match class {
        // Assistant speaking, echo of the assistant in the mic, no user speech.
        "echo-only" => {
            let mut capture = vec![0.0f32; total];
            // Echo: delayed, attenuated copy of the assistant render.
            let delay = samples_for_ms(20, sample_rate);
            add_in_place(&mut capture, &scale(&assistant, 0.6), delay);
            SynthesizedFixture {
                capture,
                render_reference: assistant,
            }
        }
        // User barges in over the assistant: assistant echo + user speech overlap.
        "user-barge-in-over-assistant" => {
            let mut capture = vec![0.0f32; total];
            let delay = samples_for_ms(20, sample_rate);
            add_in_place(&mut capture, &scale(&assistant, 0.6), delay);
            // User starts at 600 ms, while the assistant is still rendering.
            add_in_place(&mut capture, &user, samples_for_ms(600, sample_rate));
            SynthesizedFixture {
                capture,
                render_reference: assistant,
            }
        }
        // Keyboard / background noise, no assistant render reference.
        "keyboard-noise" => {
            let capture = noise(0.3, total_ms, sample_rate, 0x5EED_1234);
            SynthesizedFixture {
                capture,
                render_reference: vec![0.0f32; total],
            }
        }
        // Quiet room: a tiny noise floor, no assistant, no user.
        "quiet-room" => {
            let capture = noise(0.01, total_ms, sample_rate, 0x0FFE_E000);
            SynthesizedFixture {
                capture,
                render_reference: vec![0.0f32; total],
            }
        }
        // Two users overlapping (no assistant render reference): both voices
        // active at the same time.
        "overlapped-speech" => {
            let mut capture = vec![0.0f32; total];
            let user_a = tone(330.0, 0.4, 1200, sample_rate);
            let user_b = tone(550.0, 0.4, 1200, sample_rate);
            // A starts at 200 ms, B starts at 700 ms — they overlap 700..1400 ms.
            add_in_place(&mut capture, &user_a, samples_for_ms(200, sample_rate));
            add_in_place(&mut capture, &user_b, samples_for_ms(700, sample_rate));
            SynthesizedFixture {
                capture,
                render_reference: vec![0.0f32; total],
            }
        }
        other => panic!("unknown fixture class: {other}"),
    }
}

fn scale(samples: &[f32], factor: f32) -> Vec<f32> {
    samples.iter().map(|s| s * factor).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_adapter_is_pass_through_without_reference() {
        let mut adapter = FakeAecAdapter::default();
        let frame = AlignedFrame {
            capture: vec![0.1, -0.2, 0.3],
            render_reference: Vec::new(),
            timestamp: Duration::from_millis(0),
        };
        let cleaned = adapter.process(&frame);
        assert_eq!(cleaned.samples, frame.capture);
        assert_eq!(cleaned.timestamp, frame.timestamp);
    }

    #[test]
    fn fake_adapter_reduces_echo_energy() {
        let mut adapter = FakeAecAdapter::new(1.0, 0);
        // Capture is exactly the echo (gain 1, zero delay) so it should cancel
        // to near silence.
        let reference: Vec<f32> = (0..256).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        let frame = AlignedFrame {
            capture: reference.clone(),
            render_reference: reference.clone(),
            timestamp: Duration::from_millis(10),
        };
        let cleaned = adapter.process(&frame);
        let before = energy(&reference);
        let after = energy(&cleaned.samples);
        assert!(
            after < before * 0.01,
            "expected near-total cancellation, before={before} after={after}"
        );
    }

    #[test]
    fn aligned_frame_reports_alignment() {
        let frame = AlignedFrame {
            capture: vec![0.0; 8],
            render_reference: vec![0.0; 8],
            timestamp: Duration::ZERO,
        };
        assert!(frame.has_aligned_reference());
        assert_eq!(frame.len(), 8);
        assert!(!frame.is_empty());
    }

    #[test]
    fn echo_leak_is_zero_without_echo() {
        assert_eq!(AecMetricsReporter::compute_echo_leak(&[], &[0.1, 0.2]), 0.0);
    }

    #[test]
    fn false_start_and_missed_rates() {
        let vad = vec![true, false, true, false];
        let truth = vec![false, false, true, true];
        // frame 0: vad true, truth false -> false start (1 of 4).
        assert_eq!(
            AecMetricsReporter::compute_false_start_rate(&vad, &truth),
            0.25
        );
        // truth speech frames: 2 and 3; vad missed frame 3 -> 1 of 2.
        assert_eq!(
            AecMetricsReporter::compute_missed_barge_in_rate(&vad, &truth),
            0.5
        );
    }

    #[test]
    fn rtf_under_one_is_realtime() {
        let rtf =
            AecMetricsReporter::compute_rtf(Duration::from_millis(10), Duration::from_millis(100));
        assert!((rtf - 0.1).abs() < 1e-6);
    }

    #[test]
    fn synthesized_classes_are_aligned_and_deterministic() {
        for class in [
            "echo-only",
            "user-barge-in-over-assistant",
            "keyboard-noise",
            "quiet-room",
            "overlapped-speech",
        ] {
            let a = synthesize_fixture(class, FIXTURE_SAMPLE_RATE_HZ);
            let b = synthesize_fixture(class, FIXTURE_SAMPLE_RATE_HZ);
            assert_eq!(
                a.capture, b.capture,
                "{class} capture must be deterministic"
            );
            assert_eq!(
                a.capture.len(),
                a.render_reference.len(),
                "{class} capture and render reference must be sample-aligned"
            );
        }
    }
}
