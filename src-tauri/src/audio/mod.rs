//! Audio capture and processing pipeline.
//!
//! This module manages audio capture via rsac and the pre-processing pipeline
//! (resampling, chunk accumulation) before passing audio to ASR.

pub mod backpressure;
pub mod capture;
pub mod consumer;
// Logic behind the standalone `fixture-player` binary (src/bin/fixture_player.rs).
// Lives here, not in the bin, so argument parsing + WAV reading are
// unit-testable via `cargo test --lib` without a real audio device.
pub mod fixture_player;
pub mod mix_math;
pub mod mixer;
pub mod pcm;
pub mod pipeline;
// Pure signal-assertion helpers (RMS, clipping rate, single-bin tone energy,
// monotonic timestamps) for the strict live-audio-smoke pass. Not
// feature-gated: its own unit tests prove silence and clipping both fail
// without needing a device or the `live-audio-smoke` feature.
pub mod signal_assertions;
// Deterministic calibration-signal synthesis (tone/chirp) for CI audio
// fixtures (seed audio-graph-f166 / wave1-fixture-player). Pure functions —
// no device, no feature gate.
pub mod signal_synth;
// Minimal WAV PCM16 reader/writer shared by the fixture generators, the
// fixture-player binary, and the fixture validator tests. No `hound`
// dependency (see module docs for why).
pub mod wav_io;

// Live-audio e2e smoke (seed 0d66). Compiled only under `--features
// live-audio-smoke` AND in a test build (`cargo test`), since the module is
// entirely a `#[test]` plus its helpers — gating on `test` too keeps a
// non-test feature build free of dead-code warnings under `-D warnings`.
#[cfg(all(test, feature = "live-audio-smoke"))]
mod live_audio_smoke;

pub use capture::{AudioCaptureManager, AudioChunk};
pub use consumer::ProcessedAudioConsumerRegistry;
pub use pipeline::ProcessedAudioChunk;
