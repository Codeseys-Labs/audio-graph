//! Audio capture and processing pipeline.
//!
//! This module manages audio capture via rsac and the pre-processing pipeline
//! (resampling, chunk accumulation) before passing audio to ASR.

pub mod backpressure;
pub mod capture;
pub mod consumer;
pub mod mix_math;
pub mod mixer;
pub mod pcm;
pub mod pipeline;

// Live-audio e2e smoke (seed 0d66). Compiled only under `--features
// live-audio-smoke` AND in a test build (`cargo test`), since the module is
// entirely a `#[test]` plus its helpers — gating on `test` too keeps a
// non-test feature build free of dead-code warnings under `-D warnings`.
#[cfg(all(test, feature = "live-audio-smoke"))]
mod live_audio_smoke;

pub use capture::{AudioCaptureManager, AudioChunk};
pub use consumer::ProcessedAudioConsumerRegistry;
pub use pipeline::ProcessedAudioChunk;
