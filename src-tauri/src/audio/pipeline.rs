//! Audio processing pipeline — resampling and chunk accumulation.
//!
//! Receives raw AudioChunks from capture threads (48kHz stereo),
//! resamples to 16kHz mono, and emits fixed-size ProcessedAudioChunks
//! suitable for downstream ASR processing.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use audioadapter_buffers::direct::SequentialSliceOfVecs;
use crossbeam_channel::{Receiver, Sender};
use rubato::{
    Async, FixedAsync, Resampler, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};

use super::capture::AudioChunk;

/// Resampled, mono audio chunk ready for downstream processing (ASR).
#[derive(Debug, Clone)]
pub struct ProcessedAudioChunk {
    /// Identifier of the capture source that produced this chunk.
    ///
    /// `Arc<str>` (not `String`): emitted once per `TARGET_CHUNK_FRAMES` window,
    /// so sharing one refcounted id across all chunks of a source avoids a heap
    /// alloc + copy on each emit (FA-4b).
    pub source_id: Arc<str>,
    pub data: Vec<f32>,
    pub sample_rate: u32,
    pub num_frames: usize,
    pub timestamp: Option<Duration>,
}

/// Target output sample rate for ASR.
const TARGET_SAMPLE_RATE: u32 = 16000;

/// Target chunk size in frames (~32ms at 16kHz, suitable for streaming ASR).
const TARGET_CHUNK_FRAMES: usize = 512;

/// Resampler processing block size (input frames per rubato call).
const RESAMPLER_CHUNK_SIZE: usize = 1024;

struct SourcePipelineState {
    /// rubato resampler (created lazily on first chunk).
    resampler: Option<Async<f32>>,
    /// Input sample rate the resampler was created for.
    resampler_input_rate: u32,
    /// Buffer accumulating mono samples waiting for the resampler.
    /// rubato requires exactly `input_frames_next()` samples per call.
    resampler_input_buffer: Vec<f32>,
    /// Buffer accumulating resampled output, drained in TARGET_CHUNK_FRAMES-sized pieces.
    accumulation_buffer: Vec<f32>,
    /// Reusable scratch for the resampler input frame, wrapped in a single-channel
    /// `Vec<Vec<f32>>` so it can back a `SequentialSliceOfVecs` adapter without
    /// allocating a fresh Vec on every `drain_resampler` inner-loop iteration.
    /// (Precedent: diarization worker's reused `scratch` buffer.)
    resampler_scratch: Vec<Vec<f32>>,
    /// Timestamp of the current accumulation start.
    current_timestamp: Option<Duration>,
}

impl SourcePipelineState {
    fn new() -> Self {
        Self {
            resampler: None,
            resampler_input_rate: 0,
            resampler_input_buffer: Vec::with_capacity(RESAMPLER_CHUNK_SIZE * 2),
            accumulation_buffer: Vec::with_capacity(TARGET_CHUNK_FRAMES * 4),
            resampler_scratch: vec![Vec::with_capacity(RESAMPLER_CHUNK_SIZE)],
            current_timestamp: None,
        }
    }
}

/// Audio pipeline that resamples 48kHz stereo → 16kHz mono and emits fixed-size chunks.
pub struct AudioPipeline {
    /// Receives raw AudioChunks from capture threads.
    audio_rx: Receiver<AudioChunk>,
    /// Sends processed chunks downstream (ASR, Gemini, etc.).
    output_tx: Sender<ProcessedAudioChunk>,
    /// Independent resample/accumulation state per capture source, keyed by the
    /// chunk's `Arc<str>` source id so per-chunk lookups refcount-bump the key
    /// rather than re-allocating a `String` (FA-4b).
    source_states: HashMap<Arc<str>, SourcePipelineState>,
}

impl AudioPipeline {
    /// Create a new audio pipeline.
    pub fn new(audio_rx: Receiver<AudioChunk>, output_tx: Sender<ProcessedAudioChunk>) -> Self {
        Self {
            audio_rx,
            output_tx,
            source_states: HashMap::new(),
        }
    }

    /// Run the pipeline processing loop (blocking — spawn in a dedicated thread).
    pub fn run(&mut self) {
        log::info!("AudioPipeline: starting processing loop");
        while let Ok(chunk) = self.audio_rx.recv() {
            self.process_chunk(chunk);
        }
        self.flush();
        log::info!("AudioPipeline: processing loop ended (channel closed)");
    }

    /// Process a single audio chunk: mixdown → resample → accumulate → emit.
    fn process_chunk(&mut self, chunk: AudioChunk) {
        // Step 1: Stereo (or multi-channel) → mono mixdown
        let mono = Self::stereo_to_mono(&chunk.data, chunk.channels);
        let source_id = chunk.source_id;
        let state = self
            .source_states
            .entry(Arc::clone(&source_id))
            .or_insert_with(SourcePipelineState::new);

        if state.current_timestamp.is_none() {
            state.current_timestamp = chunk.timestamp;
        }

        // Step 2: Resample if needed
        if chunk.sample_rate == TARGET_SAMPLE_RATE {
            // No resampling needed — push directly to accumulation
            state.accumulation_buffer.extend_from_slice(&mono);
        } else {
            // Ensure resampler exists and matches input rate
            if state.resampler.is_none() || state.resampler_input_rate != chunk.sample_rate {
                match Self::create_resampler(chunk.sample_rate) {
                    Ok(r) => {
                        state.resampler = Some(r);
                        state.resampler_input_rate = chunk.sample_rate;
                        state.resampler_input_buffer.clear();
                        log::info!(
                            "AudioPipeline: created resampler for {}: {}Hz → {}Hz",
                            source_id,
                            chunk.sample_rate,
                            TARGET_SAMPLE_RATE
                        );
                    }
                    Err(e) => {
                        log::error!("AudioPipeline: failed to create resampler: {}", e);
                        return;
                    }
                }
            }

            // Add mono samples to resampler input buffer
            state.resampler_input_buffer.extend_from_slice(&mono);

            // Feed resampler in exact input_frames_next() batches
            Self::drain_resampler(state);
        }

        // Step 3: Emit complete chunks from accumulation buffer.
        // Pass `&self.output_tx` directly: it is a distinct field from
        // `self.source_states` (which backs `state`), so NLL permits the
        // disjoint borrows without cloning the Sender per chunk.
        Self::emit_chunks(&self.output_tx, &source_id, state);
    }

    /// Feed the resampler with buffered input in exact chunk sizes.
    fn drain_resampler(state: &mut SourcePipelineState) {
        // Borrow the relevant fields disjointly so the reusable scratch can be
        // mutated while `resampler` is held mutably (NLL splits the borrows).
        let SourcePipelineState {
            resampler,
            resampler_input_buffer,
            resampler_scratch,
            accumulation_buffer,
            ..
        } = state;

        let resampler = match resampler.as_mut() {
            Some(r) => r,
            None => return,
        };

        // Single reusable inner buffer (created in the ctor as a one-element Vec).
        let scratch_input = &mut resampler_scratch[0];

        loop {
            let needed = resampler.input_frames_next();
            if resampler_input_buffer.len() < needed {
                break;
            }

            // Reuse the scratch buffer: clear + drain exactly `needed` samples into
            // it, instead of allocating a fresh Vec every iteration. Same bytes,
            // same order as the previous `drain(..needed).collect()`.
            scratch_input.clear();
            scratch_input.extend(resampler_input_buffer.drain(..needed));

            // Wrap in an audioadapter SequentialSliceOfVecs — rubato's
            // adapter-based process() API (audioadapter since rubato 1.0; current
            // dep is rubato 3.0 + audioadapter-buffers 3.0). The adapter only
            // borrows for the `process` call, so reusing the backing Vec is safe.
            let waves_in = std::slice::from_ref(scratch_input);
            let input_adapter = match SequentialSliceOfVecs::new(waves_in, 1, needed) {
                Ok(a) => a,
                Err(e) => {
                    log::error!("AudioPipeline: failed to create input adapter: {}", e);
                    break;
                }
            };

            match resampler.process(&input_adapter, 0, None) {
                Ok(interleaved_out) => {
                    // For mono, interleaved data is just the samples directly
                    let resampled = interleaved_out.take_data();
                    accumulation_buffer.extend_from_slice(&resampled);
                }
                Err(e) => {
                    log::error!("AudioPipeline: resampling error: {}", e);
                    break;
                }
            }
        }
    }

    /// Emit TARGET_CHUNK_FRAMES-sized chunks from the accumulation buffer.
    fn emit_chunks(
        output_tx: &Sender<ProcessedAudioChunk>,
        source_id: &Arc<str>,
        state: &mut SourcePipelineState,
    ) {
        while state.accumulation_buffer.len() >= TARGET_CHUNK_FRAMES {
            let chunk_data: Vec<f32> = state
                .accumulation_buffer
                .drain(..TARGET_CHUNK_FRAMES)
                .collect();

            let processed = ProcessedAudioChunk {
                source_id: Arc::clone(source_id),
                num_frames: chunk_data.len(),
                data: chunk_data,
                sample_rate: TARGET_SAMPLE_RATE,
                timestamp: state.current_timestamp,
            };

            if let Err(e) = output_tx.send(processed) {
                log::warn!("AudioPipeline: downstream channel closed: {}", e);
                return;
            }
        }
    }

    /// Flush remaining buffered audio on shutdown.
    fn flush(&mut self) {
        // `self.source_states` and `self.output_tx` are distinct fields, so the
        // iter_mut() borrow and `&self.output_tx` coexist under NLL — no Sender
        // clone needed (one-shot on shutdown, but kept consistent with the
        // process_chunk hot path).
        let output_tx = &self.output_tx;
        for (source_id, state) in self.source_states.iter_mut() {
            // Try to flush remaining resampler input by zero-padding
            if let Some(resampler) = state.resampler.as_mut() {
                let needed = resampler.input_frames_next();
                let current = state.resampler_input_buffer.len();
                if current > 0 && current < needed {
                    state.resampler_input_buffer.resize(needed, 0.0);
                    // drain_resampler will process this padded chunk
                }
            }
            Self::drain_resampler(state);

            // Emit any remaining accumulated samples as a final (possibly undersized) chunk
            if !state.accumulation_buffer.is_empty() {
                let remaining: Vec<f32> = state.accumulation_buffer.drain(..).collect();
                let processed = ProcessedAudioChunk {
                    source_id: source_id.clone(),
                    num_frames: remaining.len(),
                    data: remaining,
                    sample_rate: TARGET_SAMPLE_RATE,
                    timestamp: state.current_timestamp,
                };

                if let Err(e) = output_tx.send(processed) {
                    log::warn!("AudioPipeline: could not send final flush chunk: {}", e);
                }
            }
        }

        log::info!("AudioPipeline: flushed remaining audio");
    }

    /// Create a rubato sinc resampler for the given input sample rate → 16kHz.
    fn create_resampler(input_rate: u32) -> Result<Async<f32>, String> {
        let ratio = TARGET_SAMPLE_RATE as f64 / input_rate as f64;

        let params = SincInterpolationParameters {
            sinc_len: 256,
            f_cutoff: 0.95,
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: 256,
            window: WindowFunction::BlackmanHarris2,
        };

        Async::<f32>::new_sinc(
            ratio,
            2.0, // max_resample_ratio_relative
            &params,
            RESAMPLER_CHUNK_SIZE,
            1, // mono
            FixedAsync::Input,
        )
        .map_err(|e| format!("Failed to create resampler: {}", e))
    }

    /// Convert interleaved multi-channel audio to mono by averaging all channels per frame.
    fn stereo_to_mono(interleaved: &[f32], channels: u16) -> Vec<f32> {
        if channels <= 1 {
            return interleaved.to_vec();
        }

        let ch = channels as usize;
        let num_frames = interleaved.len() / ch;
        let mut mono = Vec::with_capacity(num_frames);

        for frame in 0..num_frames {
            let offset = frame * ch;
            let mut sum = 0.0_f32;
            for c in 0..ch {
                sum += interleaved[offset + c];
            }
            mono.push(sum / channels as f32);
        }

        mono
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_to_mono_basic() {
        let stereo = vec![1.0, 0.0, 0.5, 0.5, 0.0, 1.0];
        let mono = AudioPipeline::stereo_to_mono(&stereo, 2);
        assert_eq!(mono.len(), 3);
        assert!((mono[0] - 0.5).abs() < 1e-6);
        assert!((mono[1] - 0.5).abs() < 1e-6);
        assert!((mono[2] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn stereo_to_mono_passthrough() {
        let mono_in = vec![0.1, 0.2, 0.3];
        let mono_out = AudioPipeline::stereo_to_mono(&mono_in, 1);
        assert_eq!(mono_out, mono_in);
    }

    #[test]
    fn create_resampler_48k() {
        let r = AudioPipeline::create_resampler(48000);
        assert!(r.is_ok(), "Failed to create 48kHz resampler: {:?}", r.err());
    }

    #[test]
    fn create_resampler_44k() {
        let r = AudioPipeline::create_resampler(44100);
        assert!(
            r.is_ok(),
            "Failed to create 44.1kHz resampler: {:?}",
            r.err()
        );
    }

    #[test]
    fn pipeline_emits_chunks() {
        let (in_tx, in_rx) = crossbeam_channel::unbounded();
        let (out_tx, out_rx) = crossbeam_channel::unbounded();

        let mut pipeline = AudioPipeline::new(in_rx, out_tx);

        // Send a chunk of silence at 16kHz mono (no resampling needed)
        // 1024 frames should produce 2 chunks of 512
        let chunk = AudioChunk {
            source_id: "test".into(),
            data: vec![0.0; 1024],
            sample_rate: 16000,
            channels: 1,
            num_frames: 1024,
            timestamp: None,
        };
        in_tx.send(chunk).unwrap();
        drop(in_tx); // close channel so run() exits

        pipeline.run();

        // Should have emitted exactly 2 chunks of 512 frames
        let c1 = out_rx.recv().unwrap();
        assert_eq!(c1.num_frames, 512);
        assert_eq!(c1.sample_rate, 16000);
        assert_eq!(&*c1.source_id, "test");

        let c2 = out_rx.recv().unwrap();
        assert_eq!(c2.num_frames, 512);
    }

    #[test]
    fn pipeline_resamples_48k_to_16k() {
        // Exercises drain_resampler's reused scratch buffer across many inner-loop
        // iterations: feeds enough 48kHz input that the resampler runs repeatedly,
        // proving the scratch reuse is behavior-preserving (correct rate + chunking).
        let (in_tx, in_rx) = crossbeam_channel::unbounded();
        let (out_tx, out_rx) = crossbeam_channel::unbounded();

        let mut pipeline = AudioPipeline::new(in_rx, out_tx);

        // 1 second of 48kHz mono silence -> ~16000 frames at 16kHz -> many 512 chunks.
        // Split across several input chunks so multiple resampler batches run.
        for _ in 0..6 {
            in_tx
                .send(AudioChunk {
                    source_id: "rs".into(),
                    data: vec![0.0; 8000], // 6 * 8000 = 48000 frames = 1s @ 48kHz
                    sample_rate: 48000,
                    channels: 1,
                    num_frames: 8000,
                    timestamp: Some(Duration::from_millis(0)),
                })
                .unwrap();
        }
        drop(in_tx);

        pipeline.run();

        let chunks: Vec<ProcessedAudioChunk> = out_rx.try_iter().collect();
        // 48000 input frames downsampled to ~16000 -> at least 30 chunks of 512
        // (the exact count depends on resampler edge handling + flush padding).
        assert!(
            chunks.len() >= 30,
            "expected >=30 resampled chunks, got {}",
            chunks.len()
        );
        for c in &chunks {
            assert_eq!(c.sample_rate, 16000);
            assert_eq!(&*c.source_id, "rs");
            // Resampled silence stays ~silent (no NaN/garbage from buffer reuse).
            assert!(
                c.data.iter().all(|s| s.abs() < 1e-3 && s.is_finite()),
                "resampled silence produced non-silent/garbage samples"
            );
        }
        // All full chunks (except possibly the final flush remainder) are 512 frames.
        for c in &chunks[..chunks.len() - 1] {
            assert_eq!(c.num_frames, 512, "non-final chunk must be 512 frames");
        }
    }

    #[test]
    fn pipeline_keeps_interleaved_sources_separate() {
        let (in_tx, in_rx) = crossbeam_channel::unbounded();
        let (out_tx, out_rx) = crossbeam_channel::unbounded();

        let mut pipeline = AudioPipeline::new(in_rx, out_tx);

        in_tx
            .send(AudioChunk {
                source_id: "source-a".into(),
                data: vec![0.25; 256],
                sample_rate: 16000,
                channels: 1,
                num_frames: 256,
                timestamp: Some(Duration::from_millis(10)),
            })
            .unwrap();
        in_tx
            .send(AudioChunk {
                source_id: "source-b".into(),
                data: vec![0.75; 512],
                sample_rate: 16000,
                channels: 1,
                num_frames: 512,
                timestamp: Some(Duration::from_millis(20)),
            })
            .unwrap();
        in_tx
            .send(AudioChunk {
                source_id: "source-a".into(),
                data: vec![0.25; 256],
                sample_rate: 16000,
                channels: 1,
                num_frames: 256,
                timestamp: Some(Duration::from_millis(30)),
            })
            .unwrap();
        drop(in_tx);

        pipeline.run();

        let chunks: Vec<ProcessedAudioChunk> = out_rx.try_iter().collect();
        assert_eq!(chunks.len(), 2);

        assert_eq!(&*chunks[0].source_id, "source-b");
        assert_eq!(chunks[0].num_frames, 512);
        assert!(
            chunks[0]
                .data
                .iter()
                .all(|sample| (*sample - 0.75).abs() < 1e-6)
        );

        assert_eq!(&*chunks[1].source_id, "source-a");
        assert_eq!(chunks[1].num_frames, 512);
        assert!(
            chunks[1]
                .data
                .iter()
                .all(|sample| (*sample - 0.25).abs() < 1e-6)
        );
    }
}
