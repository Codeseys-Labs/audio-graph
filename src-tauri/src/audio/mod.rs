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

pub use capture::{AudioCaptureManager, AudioChunk};
pub use consumer::ProcessedAudioConsumerRegistry;
pub use pipeline::ProcessedAudioChunk;
