//! Audio capture and processing pipeline.
//!
//! This module manages audio capture via rsac and the pre-processing pipeline
//! (resampling, chunk accumulation) before passing audio to ASR.

pub mod capture;
pub mod pipeline;

pub use capture::{AudioCaptureManager, AudioChunk};
pub use pipeline::ProcessedAudioChunk;
