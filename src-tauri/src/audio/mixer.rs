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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError};

use super::pipeline::ProcessedAudioChunk;

/// Mixed-stream synthetic source id (attribution collapses to one stream).
pub const MIXED_SOURCE_ID: &str = "mixed";

const TARGET_SAMPLE_RATE: u32 = 16000;
/// Frame size pulled per mix step (~32 ms at 16 kHz; matches the pipeline).
const FRAME: usize = 512;
/// Drop a source that hasn't produced audio for this long.
const SILENCE_EVICT: Duration = Duration::from_secs(2);
/// Cap per-source buffering so a runaway source can't grow unbounded (~2 s).
const MAX_BUFFERED: usize = TARGET_SAMPLE_RATE as usize * 2;

struct SourceBuffer {
    samples: VecDeque<f32>,
    last_seen: Instant,
}

/// Sum one `FRAME` from each source (each slice is `FRAME` long, silence-padded
/// by the caller), scale by 1/sqrt(active), and clamp. Pure + unit-tested.
fn mix_frame(frames: &[Vec<f32>]) -> Vec<f32> {
    let mut out = vec![0.0f32; FRAME];
    if frames.is_empty() {
        return out;
    }
    for frame in frames {
        for (o, &s) in out.iter_mut().zip(frame.iter()) {
            *o += s;
        }
    }
    let scale = 1.0 / (frames.len() as f32).sqrt();
    for o in out.iter_mut() {
        *o = (*o * scale).clamp(-1.0, 1.0);
    }
    out
}

/// Pull up to `FRAME` samples from a buffer, silence-padding the tail when the
/// source is short (jitter / just-stopped). Returns `None` when fully empty.
fn take_frame(buf: &mut VecDeque<f32>) -> Option<Vec<f32>> {
    if buf.is_empty() {
        return None;
    }
    let mut frame = Vec::with_capacity(FRAME);
    for _ in 0..FRAME {
        frame.push(buf.pop_front().unwrap_or(0.0));
    }
    Some(frame)
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
        self.sources
            .retain(|_, b| now.duration_since(b.last_seen) < SILENCE_EVICT || !b.samples.is_empty());
    }

    /// The largest number of buffered samples across active sources.
    fn max_buffered(&self) -> usize {
        self.sources.values().map(|b| b.samples.len()).max().unwrap_or(0)
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
                mixer.evict_stale();
                // Emit while we have at least a full frame buffered somewhere,
                // keeping latency low and absorbing per-source jitter.
                while mixer.max_buffered() >= FRAME {
                    let Some(data) = mixer.pull_mixed_frame() else {
                        break;
                    };
                    let chunk = ProcessedAudioChunk {
                        source_id: MIXED_SOURCE_ID.to_string(),
                        num_frames: data.len(),
                        data,
                        sample_rate: TARGET_SAMPLE_RATE,
                        timestamp: None,
                    };
                    if out_tx.send(chunk).is_err() {
                        log::info!("Audio mixer: output closed, exiting");
                        return;
                    }
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
    fn single_source_passes_through_scaled() {
        // One source, full of 0.5 → scale 1/sqrt(1)=1 → unchanged.
        let frames = vec![vec![0.5f32; FRAME]];
        let out = mix_frame(&frames);
        assert_eq!(out.len(), FRAME);
        assert!((out[0] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn two_sources_sum_and_scale() {
        // 0.5 + 0.5 = 1.0, scaled by 1/sqrt(2) ≈ 0.707.
        let frames = vec![vec![0.5f32; FRAME], vec![0.5f32; FRAME]];
        let out = mix_frame(&frames);
        assert!((out[0] - (1.0 / 2.0_f32.sqrt())).abs() < 1e-4);
    }

    #[test]
    fn loud_sum_is_clamped() {
        let frames = vec![vec![1.0f32; FRAME], vec![1.0f32; FRAME], vec![1.0f32; FRAME]];
        let out = mix_frame(&frames);
        assert!(out.iter().all(|&s| s <= 1.0 && s >= -1.0));
    }

    #[test]
    fn take_frame_silence_pads_short_source() {
        let mut buf: VecDeque<f32> = VecDeque::from(vec![1.0f32; 10]);
        let f = take_frame(&mut buf).unwrap();
        assert_eq!(f.len(), FRAME);
        assert_eq!(f[0], 1.0);
        assert_eq!(f[FRAME - 1], 0.0); // padded
        assert!(buf.is_empty());
    }

    #[test]
    fn empty_buffer_yields_no_frame() {
        let mut buf: VecDeque<f32> = VecDeque::new();
        assert!(take_frame(&mut buf).is_none());
    }

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
