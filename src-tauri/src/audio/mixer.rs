//! Audio mixer — sums multiple per-source 16 kHz-mono streams into ONE stream
//! so a single-WebSocket streaming ASR (Deepgram/AssemblyAI/…) can consume
//! several selected sources at once.
//!
//! Placement: it runs *after* the pipeline's resample/downmix stage, where
//! every source is already a common format (16 kHz mono f32), so the mixer only
//! does time-alignment + summing — no resampling, no channel logic. It sits in
//! front of a streaming ASR worker, replacing the per-source receiver with a
//! single "mixed" stream.
//!
//! Strategy: per-source ring buffers absorb arrival jitter; on a fixed frame
//! cadence we pull `FRAME` samples from each active source (silence-padding any
//! laggard), sum them, scale by 1/sqrt(active) to preserve loudness without
//! letting a dominant source vanish, then hard-clamp to [-1, 1]. Sources that
//! go quiet for `SILENCE_EVICT` are dropped so they stop contributing.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError};

use super::mix_math::{FRAME, mix_frame, take_frame};
use super::pipeline::ProcessedAudioChunk;

/// Mixed-stream synthetic source id (attribution collapses to one stream).
pub const MIXED_SOURCE_ID: &str = "mixed";

const TARGET_SAMPLE_RATE: u32 = 16000;
/// Drop a source that hasn't produced audio for this long.
const SILENCE_EVICT: Duration = Duration::from_secs(2);
/// Cap per-source buffering so a runaway source can't grow unbounded (~2 s).
const MAX_BUFFERED: usize = TARGET_SAMPLE_RATE as usize * 2;

struct SourceBuffer {
    samples: VecDeque<f32>,
    last_seen: Instant,
}

struct AudioMixer {
    sources: HashMap<String, SourceBuffer>,
}

impl AudioMixer {
    fn new() -> Self {
        Self {
            sources: HashMap::new(),
        }
    }

    fn ingest(&mut self, chunk: ProcessedAudioChunk) {
        let entry = self
            .sources
            .entry(chunk.source_id)
            .or_insert_with(|| SourceBuffer {
                samples: VecDeque::new(),
                last_seen: Instant::now(),
            });
        entry.last_seen = Instant::now();
        entry.samples.extend(chunk.data.iter().copied());
        // Bound memory: drop oldest if a source outruns the consumer.
        while entry.samples.len() > MAX_BUFFERED {
            entry.samples.pop_front();
        }
    }

    fn evict_stale(&mut self) {
        let now = Instant::now();
        self.sources.retain(|_, b| {
            now.duration_since(b.last_seen) < SILENCE_EVICT || !b.samples.is_empty()
        });
    }

    /// The largest number of buffered samples across active sources.
    fn max_buffered(&self) -> usize {
        self.sources
            .values()
            .map(|b| b.samples.len())
            .max()
            .unwrap_or(0)
    }

    /// True when every active source has at least a full frame buffered, so we
    /// can pull one frame from each and actually SUM them (rather than emitting
    /// a single source's frame alone, which would just re-interleave sources).
    fn aligned_for_mix(&self) -> bool {
        !self.sources.is_empty() && self.sources.values().all(|b| b.samples.len() >= FRAME)
    }

    /// Emit one mixed `FRAME` by taking a frame from each source that has data.
    /// Returns `None` if no source had any samples.
    fn pull_mixed_frame(&mut self) -> Option<Vec<f32>> {
        let mut frames: Vec<Vec<f32>> = Vec::new();
        for buf in self.sources.values_mut() {
            if let Some(f) = take_frame(&mut buf.samples) {
                frames.push(f);
            }
        }
        if frames.is_empty() {
            None
        } else {
            Some(mix_frame(&frames))
        }
    }
}

/// Spawn the mixer thread. Consumes the per-source `input_rx` and returns a
/// receiver of a single mixed 16 kHz-mono stream (all chunks tagged
/// [`MIXED_SOURCE_ID`]). Exits when transcription stops or the input closes.
pub fn spawn_mixer(
    input_rx: Receiver<ProcessedAudioChunk>,
    is_transcribing: Arc<AtomicBool>,
) -> Receiver<ProcessedAudioChunk> {
    let (out_tx, out_rx) = crossbeam_channel::bounded::<ProcessedAudioChunk>(1024);
    let _ = std::thread::Builder::new()
        .name("audio-mixer".to_string())
        .spawn(move || {
            let mut mixer = AudioMixer::new();
            // Anti-stall: if one source backs up while another lags, flush a
            // (silence-padded) mixed frame rather than waiting forever.
            const FLUSH_AFTER: Duration = Duration::from_millis(80);
            let mut last_emit = Instant::now();
            log::info!("Audio mixer: started");
            loop {
                match input_rx.recv_timeout(Duration::from_millis(20)) {
                    Ok(chunk) => mixer.ingest(chunk),
                    Err(RecvTimeoutError::Timeout) => {
                        if !is_transcribing.load(Ordering::Relaxed) {
                            break;
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                }
                // Drain everything immediately available so all sources are
                // caught up before we decide whether they're aligned to mix.
                while let Ok(chunk) = input_rx.try_recv() {
                    mixer.ingest(chunk);
                }
                mixer.evict_stale();

                let emit = |data: Vec<f32>| -> bool {
                    let chunk = ProcessedAudioChunk {
                        source_id: MIXED_SOURCE_ID.to_string(),
                        num_frames: data.len(),
                        data,
                        sample_rate: TARGET_SAMPLE_RATE,
                        timestamp: None,
                    };
                    out_tx.send(chunk).is_ok()
                };

                // Normal path: every source has a full frame → sum aligned frames.
                while mixer.aligned_for_mix() {
                    let Some(data) = mixer.pull_mixed_frame() else {
                        break;
                    };
                    if !emit(data) {
                        log::info!("Audio mixer: output closed, exiting");
                        return;
                    }
                    last_emit = Instant::now();
                }
                // Anti-stall path: a source is backing up but others lag — flush
                // with silence-fill so we don't fall behind real time.
                if mixer.max_buffered() >= FRAME
                    && Instant::now().duration_since(last_emit) > FLUSH_AFTER
                    && let Some(data) = mixer.pull_mixed_frame()
                {
                    if !emit(data) {
                        return;
                    }
                    last_emit = Instant::now();
                }
            }
            log::info!("Audio mixer: stopped");
        });
    out_rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixer_evicts_stale_sources() {
        let mut m = AudioMixer::new();
        m.ingest(ProcessedAudioChunk {
            source_id: "a".into(),
            data: vec![0.1; FRAME],
            sample_rate: TARGET_SAMPLE_RATE,
            num_frames: FRAME,
            timestamp: None,
        });
        assert_eq!(m.sources.len(), 1);
        // Force last_seen into the past and empty the buffer → evicted.
        if let Some(b) = m.sources.get_mut("a") {
            b.samples.clear();
            b.last_seen = Instant::now() - Duration::from_secs(5);
        }
        m.evict_stale();
        assert_eq!(m.sources.len(), 0);
    }
}
