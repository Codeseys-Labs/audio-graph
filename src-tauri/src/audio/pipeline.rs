//! Audio processing pipeline — resampling and chunk accumulation.
//!
//! Receives raw, device-native interleaved `AudioChunk`s from capture threads,
//! downmixes/resamples them to the processed 16 kHz mono bus, and emits
//! fixed-size `ProcessedAudioChunk`s suitable for downstream ASR processing.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use audioadapter_buffers::direct::SequentialSliceOfVecs;
use crossbeam_channel::{Receiver, Sender};
use rubato::{
    Async, FixedAsync, Resampler, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};

use super::capture::{AudioChunk, CaptureBufferDrops, CaptureTimestampOrigin};

/// Ordered input to the process-lifetime audio pipeline. Reset travels on the
/// same channel as raw audio, so its acknowledgement proves every older chunk
/// has been consumed and per-source partial buffers have been discarded.
pub enum AudioPipelineInput {
    Chunk(AudioChunk),
    ResetSession {
        completion: Sender<Result<(), String>>,
    },
}

/// Ordered output consumed by the dispatcher. Forwarding the reset barrier
/// through this channel proves every older processed chunk was dispatched
/// before the caller rotates writers or starts a new capture generation.
pub enum ProcessedPipelineMessage {
    Chunk(ProcessedAudioChunk),
    ResetSession {
        completion: Sender<Result<(), String>>,
    },
}

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
    /// Position of the first represented frame in the capture timestamp domain.
    pub timestamp: Option<Duration>,
    pub metadata: ProcessedAudioMetadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioDiscontinuity {
    Gap {
        missing_duration: Duration,
    },
    ClockReset {
        previous_expected: Duration,
        observed: Duration,
    },
    RateChange {
        previous_rate_hz: u32,
        new_rate_hz: u32,
    },
    StreamGenerationChange {
        previous: u64,
        current: u64,
    },
    TimestampOriginChange {
        previous: CaptureTimestampOrigin,
        current: CaptureTimestampOrigin,
    },
    TimestampEstablished,
    DroppedBuffers(CaptureBufferDrops),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioSegmentBoundary {
    pub clock_generation: u64,
    pub timestamp: Option<Duration>,
    pub discontinuities: Vec<AudioDiscontinuity>,
    pub previous_input_rate_hz: Option<u32>,
    /// Native input frames from the previous segment that produced no PCM.
    pub discarded_input_frames: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcessedAudioMetadata {
    /// Local to a source and reset with the session. Advances when its clock,
    /// timestamp domain, capture generation, or sample rate changes.
    pub clock_generation: u64,
    pub stream_generation: u64,
    /// Timestamp domain for chunks that carry a timestamp.
    pub timestamp_origin: CaptureTimestampOrigin,
    /// Boundaries preceding this PCM, delivered once, in input order.
    pub boundaries: Vec<AudioSegmentBoundary>,
}

/// Canonical processed-audio sample rate emitted by the backend bus.
pub const PROCESSED_AUDIO_SAMPLE_RATE_HZ: u32 = 16_000;

/// Canonical processed-audio channel count. Multi-source audio remains
/// per-source unless explicitly mixed by `audio::mixer`; each bus chunk is mono.
pub const PROCESSED_AUDIO_CHANNELS: u16 = 1;

/// Target chunk size in frames (~32ms at 16kHz, suitable for streaming ASR).
pub const PROCESSED_AUDIO_CHUNK_FRAMES: usize = 512;

/// Duration of one full canonical processed-audio chunk.
pub const PROCESSED_AUDIO_CHUNK_DURATION_MS: u64 = 32;

impl ProcessedAudioChunk {
    /// `true` when the chunk obeys the processed-audio bus shape.
    ///
    /// The final flush chunk may contain fewer than
    /// [`PROCESSED_AUDIO_CHUNK_FRAMES`] frames, so this checks data integrity
    /// and format invariants rather than requiring a full-sized packet.
    pub fn matches_processed_audio_contract(&self) -> bool {
        self.sample_rate == PROCESSED_AUDIO_SAMPLE_RATE_HZ
            && self.num_frames == self.data.len()
            && self.data.iter().all(|sample| sample.is_finite())
    }

    pub fn duration(&self) -> Duration {
        processed_audio_duration(self.num_frames)
    }
}

pub fn processed_audio_duration(num_frames: usize) -> Duration {
    Duration::from_secs_f64(num_frames as f64 / PROCESSED_AUDIO_SAMPLE_RATE_HZ as f64)
}

/// Back-compat alias inside this module while older code and comments use the
/// target terminology.
const TARGET_SAMPLE_RATE: u32 = PROCESSED_AUDIO_SAMPLE_RATE_HZ;
const TARGET_CHUNK_FRAMES: usize = PROCESSED_AUDIO_CHUNK_FRAMES;

/// Resampler processing block size (input frames per rubato call).
const RESAMPLER_CHUNK_SIZE: usize = 1024;

/// Ignore small timestamp quantization and scheduling jitter between buffers.
const TIMESTAMP_JITTER: Duration = Duration::from_millis(2);

struct SourcePipelineState {
    /// rubato resampler (created lazily on first chunk).
    resampler: Option<Async<f32>>,
    /// Input rate of this segment, including the passthrough case.
    input_rate: Option<u32>,
    /// Startup delay and frame totals for the current resampler segment.
    resampler_delay: usize,
    segment_input_frames: usize,
    produced_output_frames: usize,
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
    /// Source timestamp anchoring the current segment.
    segment_timestamp: Option<Duration>,
    /// Timestamp of the next processed output frame.
    current_timestamp: Option<Duration>,
    expected_input_timestamp: Option<Duration>,
    metadata: ProcessedAudioMetadata,
}

impl SourcePipelineState {
    fn new() -> Self {
        Self {
            resampler: None,
            input_rate: None,
            resampler_delay: 0,
            segment_input_frames: 0,
            produced_output_frames: 0,
            resampler_input_buffer: Vec::with_capacity(RESAMPLER_CHUNK_SIZE * 2),
            accumulation_buffer: Vec::with_capacity(TARGET_CHUNK_FRAMES * 4),
            resampler_scratch: vec![Vec::with_capacity(RESAMPLER_CHUNK_SIZE)],
            segment_timestamp: None,
            current_timestamp: None,
            expected_input_timestamp: None,
            metadata: ProcessedAudioMetadata::default(),
        }
    }

    fn output_frames_for_input(&self) -> usize {
        (self.segment_input_frames as u128 * TARGET_SAMPLE_RATE as u128
            / self.input_rate.expect("resampling requires an input rate") as u128) as usize
    }

    fn discarded_input_frames(&self) -> usize {
        if self.resampler.is_none() {
            return 0;
        }
        let covered_input_frames = (self.produced_output_frames as u128
            * self.input_rate.expect("resampling requires an input rate") as u128)
            .div_ceil(TARGET_SAMPLE_RATE as u128) as usize;
        self.segment_input_frames
            .saturating_sub(covered_input_frames)
    }
}

/// Audio pipeline that resamples 48kHz stereo → 16kHz mono and emits fixed-size chunks.
pub struct AudioPipeline {
    /// Receives raw AudioChunks from capture threads.
    audio_rx: Receiver<AudioPipelineInput>,
    /// Sends processed chunks downstream (ASR, Gemini, etc.).
    output_tx: Sender<ProcessedPipelineMessage>,
    /// Independent resample/accumulation state per capture source, keyed by the
    /// chunk's `Arc<str>` source id so per-chunk lookups refcount-bump the key
    /// rather than re-allocating a `String` (FA-4b).
    source_states: HashMap<Arc<str>, SourcePipelineState>,
}

impl AudioPipeline {
    /// Create a new audio pipeline.
    pub fn new(
        audio_rx: Receiver<AudioPipelineInput>,
        output_tx: Sender<ProcessedPipelineMessage>,
    ) -> Self {
        Self {
            audio_rx,
            output_tx,
            source_states: HashMap::new(),
        }
    }

    /// Run the pipeline processing loop (blocking — spawn in a dedicated thread).
    pub fn run(&mut self) {
        log::info!("AudioPipeline: starting processing loop");
        while let Ok(input) = self.audio_rx.recv() {
            match input {
                AudioPipelineInput::Chunk(chunk) => self.process_chunk(chunk),
                AudioPipelineInput::ResetSession { completion } => {
                    // Partial audio is session-scoped and must never be flushed
                    // into the next generation.
                    self.source_states.clear();
                    let barrier = ProcessedPipelineMessage::ResetSession { completion };
                    if let Err(error) = self.output_tx.send(barrier) {
                        if let ProcessedPipelineMessage::ResetSession { completion } = error.0 {
                            let _ = completion
                                .send(Err("audio dispatcher is unavailable during session reset"
                                    .to_string()));
                        }
                        break;
                    }
                }
            }
        }
        self.flush();
        log::info!("AudioPipeline: processing loop ended (channel closed)");
    }

    /// Process a single audio chunk: mixdown → resample → accumulate → emit.
    fn process_chunk(&mut self, chunk: AudioChunk) {
        if chunk.sample_rate == 0 {
            log::warn!(
                "AudioPipeline: ignoring zero sample rate for {}",
                chunk.source_id
            );
            return;
        }
        // Step 1: Stereo (or multi-channel) → mono mixdown
        let mono = Self::stereo_to_mono(&chunk.data, chunk.channels);
        if mono.is_empty() {
            return;
        }
        let source_id = &chunk.source_id;
        let state = self
            .source_states
            .entry(Arc::clone(source_id))
            .or_insert_with(SourcePipelineState::new);

        Self::prepare_segment(&self.output_tx, state, &chunk);

        // Step 2: Resample if needed
        if chunk.sample_rate == TARGET_SAMPLE_RATE {
            // No resampling needed — push directly to accumulation
            state.accumulation_buffer.extend_from_slice(&mono);
        } else {
            if state.resampler.is_none() {
                match Self::create_resampler(chunk.sample_rate) {
                    Ok(r) => {
                        state.resampler_delay = r.output_delay();
                        state.resampler = Some(r);
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

            state.resampler_input_buffer.extend_from_slice(&mono);

            // Feed resampler in exact input_frames_next() batches
            Self::drain_resampler(state);
        }

        // Step 3: Emit complete chunks from accumulation buffer.
        // Pass `&self.output_tx` directly: it is a distinct field from
        // `self.source_states` (which backs `state`), so NLL permits the
        // disjoint borrows without cloning the Sender per chunk.
        Self::emit_chunks(&self.output_tx, source_id, state, false);
    }

    fn prepare_segment(
        output_tx: &Sender<ProcessedPipelineMessage>,
        state: &mut SourcePipelineState,
        chunk: &AudioChunk,
    ) {
        let mut boundaries = Vec::new();
        let mut clock_changed = false;
        if let Some(previous_rate_hz) = state.input_rate {
            if previous_rate_hz != chunk.sample_rate {
                boundaries.push(AudioDiscontinuity::RateChange {
                    previous_rate_hz,
                    new_rate_hz: chunk.sample_rate,
                });
                clock_changed = true;
            }
            if state.metadata.stream_generation != chunk.metadata.stream_generation {
                boundaries.push(AudioDiscontinuity::StreamGenerationChange {
                    previous: state.metadata.stream_generation,
                    current: chunk.metadata.stream_generation,
                });
                clock_changed = true;
            }
            if state.metadata.timestamp_origin != chunk.metadata.timestamp_origin {
                boundaries.push(AudioDiscontinuity::TimestampOriginChange {
                    previous: state.metadata.timestamp_origin,
                    current: chunk.metadata.timestamp_origin,
                });
                clock_changed = true;
            } else if let Some(observed) = chunk.timestamp {
                match state.expected_input_timestamp {
                    Some(expected) if observed.saturating_sub(expected) > TIMESTAMP_JITTER => {
                        boundaries.push(AudioDiscontinuity::Gap {
                            missing_duration: observed - expected,
                        });
                    }
                    Some(expected) if expected.saturating_sub(observed) > TIMESTAMP_JITTER => {
                        boundaries.push(AudioDiscontinuity::ClockReset {
                            previous_expected: expected,
                            observed,
                        });
                        clock_changed = true;
                    }
                    None => boundaries.push(AudioDiscontinuity::TimestampEstablished),
                    _ => {}
                }
            }
        }
        if chunk.metadata.drops != CaptureBufferDrops::default() {
            boundaries.push(AudioDiscontinuity::DroppedBuffers(chunk.metadata.drops));
        }

        let input_timestamp = if clock_changed {
            chunk.timestamp
        } else {
            chunk.timestamp.or(state.expected_input_timestamp)
        };
        if state.input_rate.is_none() || !boundaries.is_empty() {
            // Drain the old filter on its own timeline and account for any
            // native tail shorter than a target frame (or a failed drain).
            Self::drain_resampler_tail(state);
            Self::emit_chunks(output_tx, &chunk.source_id, state, true);
            let discarded_input_frames = state.discarded_input_frames();
            let previous_input_rate_hz = state.input_rate;
            if previous_input_rate_hz == Some(chunk.sample_rate) {
                if let Some(resampler) = state.resampler.as_mut() {
                    resampler.reset();
                    state.resampler_delay = resampler.output_delay();
                }
            } else {
                state.resampler = None;
            }
            state.resampler_input_buffer.clear();
            state.segment_input_frames = 0;
            state.produced_output_frames = 0;
            state.segment_timestamp = input_timestamp;
            state.current_timestamp = input_timestamp;
            state.input_rate = Some(chunk.sample_rate);
            state.metadata.stream_generation = chunk.metadata.stream_generation;
            state.metadata.timestamp_origin = chunk.metadata.timestamp_origin;
            if clock_changed {
                state.metadata.clock_generation += 1;
            }
            if !boundaries.is_empty() {
                state.metadata.boundaries.push(AudioSegmentBoundary {
                    clock_generation: state.metadata.clock_generation,
                    timestamp: state.current_timestamp,
                    discontinuities: boundaries,
                    previous_input_rate_hz,
                    discarded_input_frames,
                });
            }
        }
        // Derive the expectation from the segment's total frame count. This
        // prevents per-buffer timestamp rounding from accumulating as drift.
        state.segment_input_frames += chunk.num_frames;
        let duration =
            Duration::from_secs_f64(state.segment_input_frames as f64 / chunk.sample_rate as f64);
        state.expected_input_timestamp = state
            .segment_timestamp
            .map(|at| at.saturating_add(duration));
    }

    /// Feed the resampler with buffered input in exact chunk sizes.
    fn drain_resampler(state: &mut SourcePipelineState) -> bool {
        let max_output_frames = state.output_frames_for_input();
        // Borrow the relevant fields disjointly so the reusable scratch can be
        // mutated while `resampler` is held mutably (NLL splits the borrows).
        let SourcePipelineState {
            resampler,
            resampler_input_buffer,
            resampler_scratch,
            accumulation_buffer,
            resampler_delay,
            produced_output_frames,
            ..
        } = state;

        let resampler = match resampler.as_mut() {
            Some(r) => r,
            None => return true,
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
                    return false;
                }
            };

            match resampler.process(&input_adapter, 0, None) {
                Ok(interleaved_out) => {
                    // For mono, interleaved data is just the samples directly
                    let resampled = interleaved_out.take_data();
                    // Trim Rubato's startup delay so the first emitted sample
                    // represents the segment anchor. Cap output at the real
                    // input duration, excluding any shutdown padding.
                    let delay = (*resampler_delay).min(resampled.len());
                    *resampler_delay -= delay;
                    let available = max_output_frames - *produced_output_frames;
                    let count = available.min(resampled.len() - delay);
                    accumulation_buffer.extend_from_slice(&resampled[delay..delay + count]);
                    *produced_output_frames += count;
                }
                Err(e) => {
                    log::error!("AudioPipeline: resampling error: {}", e);
                    return false;
                }
            }
        }
        true
    }

    /// Emit full chunks, plus any short remainder when closing a segment.
    fn emit_chunks(
        output_tx: &Sender<ProcessedPipelineMessage>,
        source_id: &Arc<str>,
        state: &mut SourcePipelineState,
        flush_remainder: bool,
    ) {
        while !state.accumulation_buffer.is_empty() {
            let frames = state.accumulation_buffer.len().min(TARGET_CHUNK_FRAMES);
            if frames < TARGET_CHUNK_FRAMES && !flush_remainder {
                break;
            }
            let chunk_data: Vec<f32> = state.accumulation_buffer.drain(..frames).collect();

            let processed = ProcessedAudioChunk {
                source_id: Arc::clone(source_id),
                num_frames: chunk_data.len(),
                data: chunk_data,
                sample_rate: TARGET_SAMPLE_RATE,
                timestamp: Self::next_processed_timestamp(state, frames),
                metadata: ProcessedAudioMetadata {
                    boundaries: std::mem::take(&mut state.metadata.boundaries),
                    ..state.metadata
                },
            };

            if let Err(e) = output_tx.send(ProcessedPipelineMessage::Chunk(processed)) {
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
            Self::drain_resampler_tail(state);
            Self::emit_chunks(output_tx, source_id, state, true);
        }

        log::info!("AudioPipeline: flushed remaining audio");
    }

    fn drain_resampler_tail(state: &mut SourcePipelineState) {
        let Some(resampler) = state.resampler.as_ref() else {
            return;
        };
        // Fixed-input Rubato needs a full block to drain the filter. The
        // output cap in drain_resampler excludes any time beyond real input.
        let needed = resampler.input_frames_next();
        let output_frames = state.output_frames_for_input();
        while state.produced_output_frames < output_frames {
            state.resampler_input_buffer.resize(needed, 0.0);
            if !Self::drain_resampler(state) {
                break;
            }
        }
    }

    fn next_processed_timestamp(
        state: &mut SourcePipelineState,
        num_frames: usize,
    ) -> Option<Duration> {
        let timestamp = state.current_timestamp;
        if let Some(current) = timestamp {
            state.current_timestamp =
                Some(current.saturating_add(processed_audio_duration(num_frames)));
        }
        timestamp
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

    fn expect_chunk(message: ProcessedPipelineMessage) -> ProcessedAudioChunk {
        match message {
            ProcessedPipelineMessage::Chunk(chunk) => chunk,
            ProcessedPipelineMessage::ResetSession { .. } => {
                panic!("expected processed audio, received reset barrier")
            }
        }
    }

    fn collect_chunks(
        receiver: &crossbeam_channel::Receiver<ProcessedPipelineMessage>,
    ) -> Vec<ProcessedAudioChunk> {
        receiver
            .try_iter()
            .filter_map(|message| match message {
                ProcessedPipelineMessage::Chunk(chunk) => Some(chunk),
                ProcessedPipelineMessage::ResetSession { .. } => None,
            })
            .collect()
    }

    fn timed_input(
        source_id: &str,
        sample_rate: u32,
        frames: usize,
        timestamp: Duration,
        sample: f32,
    ) -> AudioChunk {
        AudioChunk {
            source_id: source_id.into(),
            data: vec![sample; frames],
            sample_rate,
            channels: 1,
            num_frames: frames,
            timestamp: Some(timestamp),
            metadata: Default::default(),
        }
    }

    fn process_inputs(inputs: impl IntoIterator<Item = AudioChunk>) -> Vec<ProcessedAudioChunk> {
        let (in_tx, in_rx) = crossbeam_channel::unbounded();
        let (out_tx, out_rx) = crossbeam_channel::unbounded();
        for chunk in inputs {
            in_tx.send(AudioPipelineInput::Chunk(chunk)).unwrap();
        }
        drop(in_tx);
        AudioPipeline::new(in_rx, out_tx).run();
        collect_chunks(&out_rx)
    }

    #[test]
    fn pipeline_preserves_two_second_gap_at_16k_and_48k() {
        for rate in [16_000, 48_000] {
            let frames = rate as usize / 10;
            let resumed_at = Duration::from_secs(2);
            let chunks = process_inputs([
                timed_input("mic", rate, frames, Duration::ZERO, 0.75),
                timed_input("mic", rate, frames, resumed_at, 0.0),
            ]);
            let first_new_index = chunks
                .iter()
                .position(|chunk| chunk.timestamp == Some(resumed_at))
                .expect("resumed segment starts at its source timestamp");
            let (old, new) = chunks.split_at(first_new_index);
            assert_eq!(old[0].timestamp, Some(Duration::ZERO));
            assert!(old.iter().all(|chunk| chunk.metadata.boundaries.is_empty()));
            let boundary = &new[0].metadata.boundaries[0];
            assert_eq!(boundary.timestamp, Some(resumed_at));
            assert_eq!(boundary.clock_generation, 0);
            assert_eq!(
                boundary.discontinuities,
                [AudioDiscontinuity::Gap {
                    missing_duration: Duration::from_millis(1900),
                }]
            );
            let old_frames = old.iter().map(|chunk| chunk.num_frames).sum::<usize>();
            assert_eq!(
                old_frames * (rate as usize / 16_000) + boundary.discarded_input_frames,
                frames,
                "all old input was represented or accounted for"
            );
            assert_eq!(
                new.iter().map(|chunk| chunk.num_frames).sum::<usize>(),
                1600
            );
            assert!(
                new[1..]
                    .iter()
                    .all(|chunk| chunk.metadata.boundaries.is_empty())
            );
            assert!(
                new.iter()
                    .flat_map(|chunk| &chunk.data)
                    .all(|sample| *sample == 0.0)
            );
            assert!(
                chunks
                    .iter()
                    .all(ProcessedAudioChunk::matches_processed_audio_contract)
            );
        }
    }

    #[test]
    fn backward_clock_jump_reanchors_and_increments_generation() {
        let chunks = process_inputs([
            timed_input("mic", 16_000, 512, Duration::from_secs(1), 0.25),
            timed_input("mic", 16_000, 1024, Duration::from_millis(5), 0.75),
        ]);
        assert_eq!(chunks[0].metadata.clock_generation, 0);
        assert_eq!(chunks[1].timestamp, Some(Duration::from_millis(5)));
        assert_eq!(chunks[1].metadata.clock_generation, 1);
        assert_eq!(
            chunks[1].metadata.boundaries[0].discontinuities,
            [AudioDiscontinuity::ClockReset {
                previous_expected: Duration::from_millis(1032),
                observed: Duration::from_millis(5),
            }]
        );
        assert_eq!(chunks[2].timestamp, Some(Duration::from_millis(37)));
        assert_eq!(chunks[2].metadata.clock_generation, 1);
        assert!(chunks[2].metadata.boundaries.is_empty());
    }

    #[test]
    fn rate_changes_flush_old_pcm_and_account_for_subframe_tail() {
        let first_transition = Duration::from_millis(10);
        let second_transition = Duration::from_millis(20);
        let chunks = process_inputs([
            timed_input("mic", 48_000, 481, Duration::ZERO, 0.75),
            timed_input("mic", 44_100, 442, first_transition, 0.5),
            timed_input("mic", 16_000, 512, second_transition, 0.0),
        ]);

        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].timestamp, Some(Duration::ZERO));
        assert_eq!(chunks[0].num_frames, 160);
        assert!(chunks[0].metadata.boundaries.is_empty());
        assert_eq!(chunks[1].timestamp, Some(first_transition));
        assert_eq!(chunks[1].num_frames, 160);
        assert_eq!(
            chunks[1].metadata.boundaries,
            [AudioSegmentBoundary {
                clock_generation: 1,
                timestamp: Some(first_transition),
                discontinuities: vec![AudioDiscontinuity::RateChange {
                    previous_rate_hz: 48_000,
                    new_rate_hz: 44_100,
                }],
                previous_input_rate_hz: Some(48_000),
                discarded_input_frames: 1,
            }]
        );
        let chunk = &chunks[2];
        assert_eq!(chunk.timestamp, Some(second_transition));
        assert_eq!(chunk.metadata.clock_generation, 2);
        assert!(chunk.data.iter().all(|sample| *sample == 0.0));
        assert_eq!(
            chunk.metadata.boundaries,
            [AudioSegmentBoundary {
                clock_generation: 2,
                timestamp: Some(second_transition),
                discontinuities: vec![AudioDiscontinuity::RateChange {
                    previous_rate_hz: 44_100,
                    new_rate_hz: 16_000,
                }],
                previous_input_rate_hz: Some(44_100),
                discarded_input_frames: 1,
            }]
        );
    }

    #[test]
    fn capture_drop_deltas_are_forwarded_once_even_with_contiguous_timestamps() {
        let drops = CaptureBufferDrops {
            ring_buffers: 2,
            subscriber_buffers: 3,
            pipeline_buffers: 1,
            pipeline_input_frames: 256,
        };
        let mut after_drops = timed_input("mic", 16_000, 1024, Duration::from_millis(16), 0.5);
        after_drops.metadata.drops = drops;
        let chunks = process_inputs([
            timed_input("mic", 16_000, 256, Duration::ZERO, 0.25),
            after_drops,
        ]);

        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].data, vec![0.25; 256]);
        assert!(chunks[0].metadata.boundaries.is_empty());
        assert_eq!(chunks[1].timestamp, Some(Duration::from_millis(16)));
        assert_eq!(chunks[1].data, vec![0.5; 512]);
        assert_eq!(
            chunks[1].metadata.boundaries[0].discontinuities,
            [AudioDiscontinuity::DroppedBuffers(drops)]
        );
        assert!(chunks[2].metadata.boundaries.is_empty());
    }

    #[test]
    fn device_recovery_marks_new_stream_generation_on_rsac_timeline() {
        let mut recovered = timed_input("mic", 16_000, 512, Duration::from_millis(32), 0.0);
        recovered.metadata.stream_generation = 3;
        let chunks = process_inputs([
            timed_input("mic", 16_000, 512, Duration::ZERO, 0.75),
            recovered,
        ]);

        assert_eq!(chunks[1].timestamp, Some(Duration::from_millis(32)));
        assert_eq!(chunks[1].metadata.clock_generation, 1);
        assert_eq!(chunks[1].metadata.stream_generation, 3);
        assert_eq!(
            chunks[1].metadata.boundaries[0].discontinuities,
            [AudioDiscontinuity::StreamGenerationChange {
                previous: 0,
                current: 3,
            }]
        );
    }

    #[test]
    fn fallback_and_rsac_timestamps_are_not_compared_as_the_same_clock() {
        let mut fallback = timed_input("mic", 16_000, 512, Duration::from_secs(1), 0.0);
        fallback.metadata.timestamp_origin = CaptureTimestampOrigin::ElapsedFallback;
        let chunks = process_inputs([
            fallback,
            timed_input("mic", 16_000, 512, Duration::from_millis(10), 0.0),
        ]);

        assert_eq!(
            chunks[0].metadata.timestamp_origin,
            CaptureTimestampOrigin::ElapsedFallback
        );
        assert_eq!(chunks[1].timestamp, Some(Duration::from_millis(10)));
        assert_eq!(
            chunks[1].metadata.timestamp_origin,
            CaptureTimestampOrigin::Rsac
        );
        assert_eq!(
            chunks[1].metadata.boundaries[0].discontinuities,
            [AudioDiscontinuity::TimestampOriginChange {
                previous: CaptureTimestampOrigin::ElapsedFallback,
                current: CaptureTimestampOrigin::Rsac,
            }]
        );
    }

    #[test]
    fn source_gap_does_not_break_an_interleaved_contiguous_source() {
        let chunks = process_inputs([
            timed_input("mic", 16_000, 512, Duration::ZERO, 0.25),
            timed_input("loopback", 16_000, 512, Duration::from_millis(50), 0.5),
            timed_input("mic", 16_000, 512, Duration::from_secs(2), 0.25),
            timed_input("loopback", 16_000, 512, Duration::from_millis(82), 0.5),
        ]);

        assert_eq!(chunks.len(), 4);
        assert_eq!(&*chunks[2].source_id, "mic");
        assert_eq!(chunks[2].timestamp, Some(Duration::from_secs(2)));
        assert_eq!(
            chunks[2].metadata.boundaries[0].discontinuities,
            [AudioDiscontinuity::Gap {
                missing_duration: Duration::from_millis(1968),
            }]
        );
        assert_eq!(&*chunks[3].source_id, "loopback");
        assert_eq!(chunks[3].timestamp, Some(Duration::from_millis(82)));
        assert_eq!(chunks[3].metadata, ProcessedAudioMetadata::default());
        assert_eq!(chunks[3].data, vec![0.5; 512]);
    }

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
    fn processed_audio_contract_constants_are_consistent() {
        assert_eq!(PROCESSED_AUDIO_SAMPLE_RATE_HZ, 16_000);
        assert_eq!(PROCESSED_AUDIO_CHANNELS, 1);
        assert_eq!(PROCESSED_AUDIO_CHUNK_FRAMES, 512);
        assert_eq!(
            processed_audio_duration(PROCESSED_AUDIO_CHUNK_FRAMES),
            Duration::from_millis(PROCESSED_AUDIO_CHUNK_DURATION_MS)
        );
    }

    #[test]
    fn processed_audio_chunk_contract_allows_final_remainder_but_rejects_bad_shape() {
        let good = ProcessedAudioChunk {
            source_id: "source".into(),
            data: vec![0.0; PROCESSED_AUDIO_CHUNK_FRAMES],
            sample_rate: PROCESSED_AUDIO_SAMPLE_RATE_HZ,
            num_frames: PROCESSED_AUDIO_CHUNK_FRAMES,
            timestamp: Some(Duration::from_millis(123)),
            metadata: Default::default(),
        };
        assert!(good.matches_processed_audio_contract());
        assert_eq!(good.duration(), Duration::from_millis(32));

        let remainder = ProcessedAudioChunk {
            num_frames: 128,
            data: vec![0.0; 128],
            ..good.clone()
        };
        assert!(remainder.matches_processed_audio_contract());
        assert_eq!(remainder.duration(), Duration::from_millis(8));

        let wrong_rate = ProcessedAudioChunk {
            sample_rate: 24_000,
            ..good.clone()
        };
        assert!(!wrong_rate.matches_processed_audio_contract());

        let mismatched_frames = ProcessedAudioChunk {
            num_frames: 10,
            ..good.clone()
        };
        assert!(!mismatched_frames.matches_processed_audio_contract());

        let non_finite = ProcessedAudioChunk {
            data: vec![f32::NAN],
            num_frames: 1,
            ..good
        };
        assert!(!non_finite.matches_processed_audio_contract());
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
            timestamp: Some(Duration::from_millis(10)),
            metadata: Default::default(),
        };
        in_tx.send(AudioPipelineInput::Chunk(chunk)).unwrap();
        drop(in_tx); // close channel so run() exits

        pipeline.run();

        // Should have emitted exactly 2 chunks of 512 frames
        let c1 = expect_chunk(out_rx.recv().unwrap());
        assert_eq!(c1.num_frames, 512);
        assert_eq!(c1.sample_rate, 16000);
        assert_eq!(&*c1.source_id, "test");
        assert_eq!(c1.timestamp, Some(Duration::from_millis(10)));
        assert!(c1.matches_processed_audio_contract());

        let c2 = expect_chunk(out_rx.recv().unwrap());
        assert_eq!(c2.num_frames, 512);
        assert_eq!(
            c2.timestamp,
            Some(Duration::from_millis(10) + Duration::from_millis(32))
        );
        assert!(c2.matches_processed_audio_contract());
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
        for index in 0..6 {
            in_tx
                .send(AudioPipelineInput::Chunk(AudioChunk {
                    source_id: "rs".into(),
                    data: vec![0.0; 8000], // 6 * 8000 = 48000 frames = 1s @ 48kHz
                    sample_rate: 48000,
                    channels: 1,
                    num_frames: 8000,
                    // Whole milliseconds simulate quantization of 1/6-second steps.
                    timestamp: Some(Duration::from_millis(index * 1000 / 6)),
                    metadata: Default::default(),
                }))
                .unwrap();
        }
        drop(in_tx);

        pipeline.run();

        let chunks = collect_chunks(&out_rx);
        // 48000 input frames downsampled to ~16000 -> at least 30 chunks of 512
        // (the exact count depends on resampler edge handling + flush padding).
        assert!(
            chunks.len() >= 30,
            "expected >=30 resampled chunks, got {}",
            chunks.len()
        );
        for c in &chunks {
            assert!(c.metadata.boundaries.is_empty());
            assert_eq!(c.sample_rate, 16000);
            assert_eq!(&*c.source_id, "rs");
            assert!(c.timestamp.is_some());
            assert!(c.matches_processed_audio_contract());
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
        for pair in chunks.windows(2) {
            assert_eq!(
                pair[1].timestamp,
                pair[0]
                    .timestamp
                    .map(|timestamp| timestamp + pair[0].duration())
            );
        }
    }

    #[test]
    fn pipeline_keeps_interleaved_sources_separate() {
        let (in_tx, in_rx) = crossbeam_channel::unbounded();
        let (out_tx, out_rx) = crossbeam_channel::unbounded();

        let mut pipeline = AudioPipeline::new(in_rx, out_tx);

        in_tx
            .send(AudioPipelineInput::Chunk(AudioChunk {
                source_id: "source-a".into(),
                data: vec![0.25; 256],
                sample_rate: 16000,
                channels: 1,
                num_frames: 256,
                timestamp: Some(Duration::from_millis(10)),
                metadata: Default::default(),
            }))
            .unwrap();
        in_tx
            .send(AudioPipelineInput::Chunk(AudioChunk {
                source_id: "source-b".into(),
                data: vec![0.75; 512],
                sample_rate: 16000,
                channels: 1,
                num_frames: 512,
                timestamp: Some(Duration::from_millis(20)),
                metadata: Default::default(),
            }))
            .unwrap();
        in_tx
            .send(AudioPipelineInput::Chunk(AudioChunk {
                source_id: "source-a".into(),
                data: vec![0.25; 256],
                sample_rate: 16000,
                channels: 1,
                num_frames: 256,
                timestamp: Some(Duration::from_millis(26)),
                metadata: Default::default(),
            }))
            .unwrap();
        drop(in_tx);

        pipeline.run();

        let chunks = collect_chunks(&out_rx);
        assert_eq!(chunks.len(), 2);

        assert_eq!(&*chunks[0].source_id, "source-b");
        assert_eq!(chunks[0].num_frames, 512);
        assert_eq!(chunks[0].timestamp, Some(Duration::from_millis(20)));
        assert!(chunks[0].matches_processed_audio_contract());
        assert!(
            chunks[0]
                .data
                .iter()
                .all(|sample| (*sample - 0.75).abs() < 1e-6)
        );

        assert_eq!(&*chunks[1].source_id, "source-a");
        assert_eq!(chunks[1].num_frames, 512);
        assert_eq!(chunks[1].timestamp, Some(Duration::from_millis(10)));
        assert!(chunks[1].matches_processed_audio_contract());
        assert!(
            chunks[1]
                .data
                .iter()
                .all(|sample| (*sample - 0.25).abs() < 1e-6)
        );
    }

    #[test]
    fn session_reset_discards_partial_source_state_before_same_source_restarts() {
        let (input_tx, input_rx) = crossbeam_channel::unbounded();
        let (output_tx, output_rx) = crossbeam_channel::unbounded();
        let mut pipeline = AudioPipeline::new(input_rx, output_tx);

        input_tx
            .send(AudioPipelineInput::Chunk(AudioChunk {
                source_id: "same-source".into(),
                data: vec![0.25; 256],
                sample_rate: 16_000,
                channels: 1,
                num_frames: 256,
                timestamp: Some(Duration::from_millis(10)),
                metadata: Default::default(),
            }))
            .unwrap();
        let (completion, _completion_rx) = crossbeam_channel::bounded(1);
        input_tx
            .send(AudioPipelineInput::ResetSession { completion })
            .unwrap();
        input_tx
            .send(AudioPipelineInput::Chunk(AudioChunk {
                source_id: "same-source".into(),
                data: vec![0.75; 512],
                sample_rate: 16_000,
                channels: 1,
                num_frames: 512,
                timestamp: Some(Duration::from_millis(100)),
                metadata: Default::default(),
            }))
            .unwrap();
        drop(input_tx);

        pipeline.run();

        let messages = output_rx.try_iter().collect::<Vec<_>>();
        assert!(matches!(
            messages.first(),
            Some(ProcessedPipelineMessage::ResetSession { .. })
        ));
        let chunks = messages
            .into_iter()
            .filter_map(|message| match message {
                ProcessedPipelineMessage::Chunk(chunk) => Some(chunk),
                ProcessedPipelineMessage::ResetSession { .. } => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].timestamp, Some(Duration::from_millis(100)));
        assert!(
            chunks[0]
                .data
                .iter()
                .all(|sample| (*sample - 0.75).abs() < 1e-6),
            "old-session partial audio must not mix into the restarted source"
        );
    }

    #[test]
    fn session_reset_reports_disconnected_dispatcher() {
        let (input_tx, input_rx) = crossbeam_channel::unbounded();
        let (output_tx, output_rx) = crossbeam_channel::unbounded();
        drop(output_rx);
        let (completion, completion_rx) = crossbeam_channel::bounded(1);
        input_tx
            .send(AudioPipelineInput::ResetSession { completion })
            .unwrap();
        drop(input_tx);

        let mut pipeline = AudioPipeline::new(input_rx, output_tx);
        pipeline.run();

        assert!(
            completion_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("reset failure acknowledgement")
                .is_err(),
            "a missing dispatcher must fail the boundary instead of timing out silently"
        );
    }

    #[test]
    fn session_reset_discards_partial_48k_resampler_input() {
        let (input_tx, input_rx) = crossbeam_channel::unbounded();
        let (output_tx, output_rx) = crossbeam_channel::unbounded();
        let mut pipeline = AudioPipeline::new(input_rx, output_tx);

        // Half of the resampler's normal input block from the old generation.
        input_tx
            .send(AudioPipelineInput::Chunk(AudioChunk {
                source_id: "same-48k-source".into(),
                data: vec![1.0; 512],
                sample_rate: 48_000,
                channels: 1,
                num_frames: 512,
                timestamp: Some(Duration::ZERO),
                metadata: Default::default(),
            }))
            .unwrap();
        let (completion, _completion_rx) = crossbeam_channel::bounded(1);
        input_tx
            .send(AudioPipelineInput::ResetSession { completion })
            .unwrap();
        input_tx
            .send(AudioPipelineInput::Chunk(AudioChunk {
                source_id: "same-48k-source".into(),
                data: vec![0.0; 4_800],
                sample_rate: 48_000,
                channels: 1,
                num_frames: 4_800,
                timestamp: Some(Duration::from_secs(1)),
                metadata: Default::default(),
            }))
            .unwrap();
        drop(input_tx);

        pipeline.run();

        let chunks = collect_chunks(&output_rx);
        assert!(!chunks.is_empty());
        assert!(
            chunks
                .iter()
                .flat_map(|chunk| chunk.data.iter())
                .all(|sample| sample.abs() < 1e-3),
            "old non-zero resampler input must not bleed into new-generation silence"
        );
        assert_eq!(chunks[0].timestamp, Some(Duration::from_secs(1)));
    }
}
