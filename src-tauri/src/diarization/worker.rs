//! Live (rolling-window) clustering-diarization worker (ADR-0017 / B16).
//!
//! Drives the already-built pure core ([`super::clustering::ClusteringDiarizer`]
//! + [`super::stabilize`]) over a *live* 16 kHz mono stream. The diarizer is
//! offline (whole-window), so we re-diarize a bounded rolling window on a fixed
//! cadence ([`super::stabilize::WindowSchedule`]), embed each within-window
//! cluster, map the permutation-arbitrary local ids onto stable global speaker
//! ids ([`super::stabilize::SpeakerRegistry`]), and emit only the freshly-covered
//! trailing hop.
//!
//! ## Threading / data flow
//!
//! ```text
//!   capture/pipeline 16k-mono tap          dedicated std::thread (this module)
//!   ─────────────────────────────          ─────────────────────────────────
//!   prod.push_slice(&samples) ──▶ ringbuf::HeapRb<f32> ──▶ cons.pop_slice
//!                                 (SPSC, lock-free)          │
//!                                                            ▼ rolling Vec<f32>
//!                                          WindowSchedule.ingest / poll
//!                                                            │ Some(take)
//!                                                            ▼
//!                       ClusteringDiarizer.diarize(trailing take samples)
//!                                                            │ ClusterSegment[]
//!                                                            ▼ per local cluster
//!                       SpeakerEmbeddingExtractor (stream API) → Vec<f32>
//!                                                            ▼
//!                       SpeakerRegistry.assign(locals) → stable global ids
//!                                                            ▼ trailing-hop only
//!                       SPEAKER_DETECTED segments out a crossbeam channel
//! ```
//!
//! The SPSC ring + dedicated thread mirror [`crate::playback`]; the audio tap is
//! never blocked (a full ring drops samples — counted, never blocks). The ONNX
//! work runs on this worker thread, never in the audio callback.
//!
//! This module is feature-gated behind `diarization-clustering` for the runtime
//! engine, but the **pure glue** (sample-range slicing, the trailing-hop emit
//! filter, rolling-buffer trim) is compiled and unit-tested in every build — it
//! needs no models, audio, or ONNX Runtime.

use super::clustering::ClusterSegment;

/// One within-window local cluster reduced to what the worker needs to build its
/// embedding + register it: the concatenated sample ranges (window-local indices)
/// of every span attributed to this local speaker, and the total active speech
/// duration in seconds. `local_speaker` is the diarizer's permutation-arbitrary
/// id for *this* window only.
#[derive(Debug, Clone, PartialEq)]
pub struct ClusterSamples {
    /// Diarizer-local speaker id (only meaningful within the window it came from).
    pub local_speaker: i32,
    /// `[start, end)` sample-index ranges into the window buffer, in order, that
    /// belong to this local speaker. Concatenate these to form the embedding
    /// input. Clamped to `window_len` and never empty (degenerate spans dropped).
    pub ranges: Vec<(usize, usize)>,
    /// Total active speech duration (s) = sum of span durations for this cluster.
    /// Feeds [`super::stabilize::LocalCluster::duration_secs`] (centroid gating).
    pub duration_secs: f32,
}

/// Group a window's diarizer segments by local speaker into per-cluster sample
/// ranges, ready for embedding. **Pure** — no ONNX, no audio.
///
/// `segments` are `[start,end]`-in-seconds spans from
/// [`ClusteringDiarizer::diarize`]; `sample_rate` is the window buffer's rate
/// (16 kHz); `window_len` is the buffer length in samples (ranges are clamped to
/// it so a segment ending fractionally past the buffer can't index out of
/// bounds). Returns one [`ClusterSamples`] per distinct local speaker, ordered by
/// first appearance (`sort_by_start_time` order from the diarizer), so the
/// resulting `LocalCluster` order is deterministic.
///
/// Per the research (§1.5): for each cluster slice `samples[(start*sr) ..
/// (end*sr)]`, concatenate, feed once. Spans that resolve to an empty range
/// (rounding, or start >= end after clamping) are dropped; a cluster left with no
/// ranges is omitted entirely (it would produce an empty embedding the registry
/// can only map to UNKNOWN).
pub fn cluster_sample_ranges(
    segments: &[ClusterSegment],
    sample_rate: u32,
    window_len: usize,
) -> Vec<ClusterSamples> {
    let sr = sample_rate as f32;
    // Preserve first-appearance order of local speaker ids.
    let mut order: Vec<i32> = Vec::new();
    let mut by_speaker: std::collections::HashMap<i32, (Vec<(usize, usize)>, f32)> =
        std::collections::HashMap::new();

    for seg in segments {
        // Clamp to the window and to non-negative; convert seconds → samples.
        let start_s = seg.start.max(0.0);
        let end_s = seg.end.max(0.0);
        if end_s <= start_s {
            continue;
        }
        let start_idx = (start_s * sr) as usize;
        let end_idx = ((end_s * sr) as usize).min(window_len);
        if start_idx >= end_idx {
            continue;
        }
        let entry = by_speaker.entry(seg.speaker).or_insert_with(|| {
            order.push(seg.speaker);
            (Vec::new(), 0.0)
        });
        entry.0.push((start_idx, end_idx));
        // Duration from the *clamped sample* range so it matches what we embed.
        entry.1 += (end_idx - start_idx) as f32 / sr;
    }

    order
        .into_iter()
        .filter_map(|spk| {
            let (ranges, duration_secs) = by_speaker.remove(&spk)?;
            if ranges.is_empty() {
                return None;
            }
            Some(ClusterSamples {
                local_speaker: spk,
                ranges,
                duration_secs,
            })
        })
        .collect()
}

/// Concatenate a cluster's sample ranges out of the window buffer into one
/// contiguous `Vec<f32>` (the embedding input). **Pure.** Ranges are assumed
/// already clamped by [`cluster_sample_ranges`]; any that still overflow `buf`
/// are skipped defensively.
pub fn gather_cluster_samples(buf: &[f32], ranges: &[(usize, usize)]) -> Vec<f32> {
    let total: usize = ranges.iter().map(|&(s, e)| e.saturating_sub(s)).sum();
    let mut out = Vec::with_capacity(total);
    for &(s, e) in ranges {
        if s < e && e <= buf.len() {
            out.extend_from_slice(&buf[s..e]);
        }
    }
    out
}

/// Whether a diarizer segment overlaps the trailing hop of the window — i.e. the
/// newly-covered audio we should emit this run. **Pure.**
///
/// The window covers `[0, window_secs)` in window-local seconds; the trailing hop
/// is `[window_secs - hop_secs, window_secs)`. A segment is emitted iff it
/// overlaps that interval (`seg.end > hop_start` and `seg.start < window_secs`),
/// so each chunk of audio is emitted exactly once across successive overlapping
/// windows rather than re-emitting the whole (stable) context every run.
///
/// `window_secs` is `window_len / sample_rate` (the *actual* trailing length
/// passed to `diarize`, which may be < the configured window early in a session).
pub fn segment_in_trailing_hop(seg: &ClusterSegment, window_secs: f32, hop_secs: f32) -> bool {
    let hop_start = (window_secs - hop_secs).max(0.0);
    seg.end > hop_start && seg.start < window_secs
}

/// Filter a window's segments down to those overlapping the trailing hop, in
/// input order. **Pure.** Convenience over [`segment_in_trailing_hop`].
pub fn trailing_hop_segments(
    segments: &[ClusterSegment],
    window_secs: f32,
    hop_secs: f32,
) -> Vec<ClusterSegment> {
    segments
        .iter()
        .copied()
        .filter(|s| segment_in_trailing_hop(s, window_secs, hop_secs))
        .collect()
}

/// Number of leading samples to drop from a rolling buffer so it retains at most
/// `window_samples` trailing samples. **Pure.** Returns 0 when the buffer is
/// already within bound. Keeping the buffer bounded is what makes a long live
/// session O(window) rather than O(session).
pub fn rolling_trim_count(buf_len: usize, window_samples: u64) -> usize {
    let cap = window_samples as usize;
    buf_len.saturating_sub(cap)
}

/// Default rolling-window length (s) of context handed to `diarize` each run.
/// 10 s gives the offline clusterer enough material to separate a few speakers
/// while bounding per-run cost; the buffer is trimmed to this.
pub const DEFAULT_WINDOW_SECS: f32 = 10.0;
/// Default hop (s) — how much fresh audio between re-diarizations, and the
/// trailing slice actually emitted. 3 s balances label latency against CPU.
pub const DEFAULT_HOP_SECS: f32 = 3.0;
/// Default minimum audio (s) before the first window runs.
pub const DEFAULT_MIN_START_SECS: f32 = 4.0;
/// Ring-buffer capacity in samples (~4 s @ 16 kHz) — comfortably ≥ one hop plus
/// jitter, matching the research §2.4 sizing (`16000 * 4`).
pub const RING_CAPACITY_SAMPLES: usize = 16_000 * 4;

// ===========================================================================
// Runtime worker — feature-gated (needs sherpa-onnx + ONNX Runtime).
// ===========================================================================
#[cfg(feature = "diarization-clustering")]
mod imp {
    use super::{
        cluster_sample_ranges, gather_cluster_samples, rolling_trim_count, trailing_hop_segments,
        RING_CAPACITY_SAMPLES,
    };
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;

    use ringbuf::traits::{Consumer, Producer, Split};
    use ringbuf::{HeapCons, HeapProd, HeapRb};
    use sherpa_onnx::{SpeakerEmbeddingExtractor, SpeakerEmbeddingExtractorConfig};

    use super::super::clustering::{ClusterSegment, ClusteringDiarizer, CLUSTERING_SAMPLE_RATE};
    use super::super::stabilize::{LocalCluster, SpeakerRegistry, WindowSchedule};

    /// A relabeled, stabilized diarization span emitted by the live worker: a
    /// window-local `[start,end]` (seconds) carrying the *global* speaker id from
    /// the cross-window registry. `UNKNOWN_SPEAKER` (`u32::MAX`) means the cluster
    /// embedding was degenerate/too-short to identify (see `stabilize`).
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct StableSegment {
        pub start: f32,
        pub end: f32,
        pub global_speaker: u32,
    }

    /// Producer handle for the capture/pipeline 16 kHz-mono tap. Cheaply held by
    /// the audio side; `push` never blocks (drops + counts on a full ring).
    pub struct DiarizationFeed {
        prod: HeapProd<f32>,
        dropped: Arc<AtomicU64>,
    }

    impl DiarizationFeed {
        /// Push 16 kHz mono f32 samples into the worker's ring. Returns the count
        /// actually accepted (`< samples.len()` when the ring is full); the
        /// shortfall is added to the dropped-sample counter and never blocks the
        /// audio thread.
        pub fn push(&mut self, samples: &[f32]) -> usize {
            let wrote = self.prod.push_slice(samples);
            if wrote < samples.len() {
                self.dropped
                    .fetch_add((samples.len() - wrote) as u64, Ordering::Relaxed);
            }
            wrote
        }

        /// Total samples dropped so far because the ring was full (observability).
        pub fn dropped_samples(&self) -> u64 {
            self.dropped.load(Ordering::Relaxed)
        }
    }

    /// Live rolling-window clustering diarizer. Owns exactly one
    /// [`ClusteringDiarizer`], one [`SpeakerEmbeddingExtractor`] (pointed at the
    /// **same** embedding model), one [`SpeakerRegistry`], and one
    /// [`WindowSchedule`]; runs the diarization loop on a dedicated thread.
    pub struct LiveDiarizationWorker {
        diarizer: ClusteringDiarizer,
        embedder: SpeakerEmbeddingExtractor,
        registry: SpeakerRegistry,
        schedule: WindowSchedule,
        cons: HeapCons<f32>,
        dropped: Arc<AtomicU64>,
        rolling: Vec<f32>,
        sample_rate: u32,
        hop_secs: f32,
    }

    impl LiveDiarizationWorker {
        /// Build the worker + its audio feed.
        ///
        /// `segmentation_model` / `embedding_model` are the pyannote + embedding
        /// ONNX paths (`models::DIAR_*`); `threshold` is the within-window
        /// clustering cosine distance. `window_secs` / `hop_secs` /
        /// `min_start_secs` set the rolling cadence (use the `DEFAULT_*` consts).
        ///
        /// Returns `(worker, feed)`: keep `feed` on the capture side, move
        /// `worker` into [`LiveDiarizationWorker::spawn`].
        #[allow(clippy::too_many_arguments)]
        pub fn new(
            segmentation_model: &Path,
            embedding_model: &Path,
            threshold: f32,
            window_secs: f32,
            hop_secs: f32,
            min_start_secs: f32,
        ) -> Result<(Self, DiarizationFeed), String> {
            let diarizer = ClusteringDiarizer::new(segmentation_model, embedding_model, threshold)?;
            let sample_rate = diarizer.sample_rate();
            if sample_rate != CLUSTERING_SAMPLE_RATE {
                return Err(format!(
                    "diarizer expects {CLUSTERING_SAMPLE_RATE} Hz, model reports {sample_rate}"
                ));
            }

            // A second sherpa object on the SAME embedding model — the diarizer
            // doesn't expose per-cluster embeddings (research §1.5).
            let emb_config = SpeakerEmbeddingExtractorConfig {
                model: Some(embedding_model.display().to_string()),
                ..Default::default()
            };
            let embedder = SpeakerEmbeddingExtractor::create(&emb_config).ok_or_else(|| {
                "failed to create sherpa-onnx SpeakerEmbeddingExtractor (bad/missing model?)"
                    .to_string()
            })?;

            let rb = HeapRb::<f32>::new(RING_CAPACITY_SAMPLES);
            let (prod, cons) = rb.split();
            let dropped = Arc::new(AtomicU64::new(0));

            let sr = sample_rate as u32;
            let schedule = WindowSchedule::new(sr, window_secs, hop_secs, min_start_secs);

            let worker = Self {
                diarizer,
                embedder,
                registry: SpeakerRegistry::with_defaults(),
                schedule,
                cons,
                dropped: dropped.clone(),
                rolling: Vec::with_capacity(schedule_window_capacity(sr, window_secs)),
                sample_rate: sr,
                hop_secs,
            };
            let feed = DiarizationFeed { prod, dropped };
            Ok((worker, feed))
        }

        /// Spawn the worker on a dedicated `std::thread` (mirrors
        /// [`crate::playback`]). The thread runs until `stop` is set (then drains
        /// once more and exits) or the producer side is dropped and the ring
        /// empties. Each window emits its trailing-hop [`StableSegment`]s on
        /// `out`; the caller maps those to `SPEAKER_DETECTED` / transcript times.
        pub fn spawn(
            mut self,
            out: crossbeam_channel::Sender<StableSegment>,
            stop: Arc<AtomicBool>,
        ) -> std::thread::JoinHandle<()> {
            std::thread::Builder::new()
                .name("diarization-clustering".into())
                .spawn(move || {
                    let hop_sleep =
                        std::time::Duration::from_secs_f32((self.hop_secs / 4.0).max(0.05));
                    let mut scratch = vec![0.0f32; RING_CAPACITY_SAMPLES];
                    loop {
                        // Drain everything currently available into the rolling buf.
                        let mut got_any = false;
                        loop {
                            let n = self.cons.pop_slice(&mut scratch);
                            if n == 0 {
                                break;
                            }
                            got_any = true;
                            self.rolling.extend_from_slice(&scratch[..n]);
                            self.schedule.ingest(n as u64);
                        }

                        // Run as many due windows as the schedule reports.
                        while let Some(take) = self.schedule.poll() {
                            let take = take as usize;
                            let segs = self.run_window(take, &out);
                            if segs.is_err() {
                                // diarize failure already logged; keep the loop alive.
                            }
                        }

                        // Bound the rolling buffer to one window.
                        let trim =
                            rolling_trim_count(self.rolling.len(), self.schedule.window_samples());
                        if trim > 0 {
                            self.rolling.drain(0..trim);
                        }

                        if stop.load(Ordering::Relaxed) {
                            // Final drain pass already happened above; exit.
                            break;
                        }
                        if !got_any {
                            std::thread::sleep(hop_sleep);
                        }
                    }
                    let dropped = self.dropped.load(Ordering::Relaxed);
                    if dropped > 0 {
                        log::warn!(
                            "diarization-clustering: dropped {dropped} sample(s) on a full ring"
                        );
                    }
                    log::info!(
                        "diarization-clustering worker exiting (tracked {} global speaker(s))",
                        self.registry.len()
                    );
                })
                .expect("diarization-clustering worker thread spawn")
        }

        /// Run one window: diarize the trailing `take` samples, embed each local
        /// cluster, stabilize ids, emit only the trailing-hop segments. Returns
        /// the emitted segments (also sent on `out`) for testing/inspection.
        fn run_window(
            &mut self,
            take: usize,
            out: &crossbeam_channel::Sender<StableSegment>,
        ) -> Result<Vec<StableSegment>, String> {
            if take == 0 || self.rolling.is_empty() {
                return Ok(Vec::new());
            }
            let start = self.rolling.len().saturating_sub(take);
            let window: &[f32] = &self.rolling[start..];
            let window_len = window.len();
            let window_secs = window_len as f32 / self.sample_rate as f32;

            let segments: Vec<ClusterSegment> = match self.diarizer.diarize(window) {
                Ok(s) => s,
                Err(e) => {
                    log::warn!("diarization-clustering: diarize failed: {e}");
                    return Err(e);
                }
            };
            if segments.is_empty() {
                return Ok(Vec::new());
            }

            // Per local cluster: concatenated samples → embedding stream → Vec<f32>.
            let cluster_ranges = cluster_sample_ranges(&segments, self.sample_rate, window_len);
            let mut locals: Vec<LocalCluster> = Vec::with_capacity(cluster_ranges.len());
            let mut local_ids: Vec<i32> = Vec::with_capacity(cluster_ranges.len());
            for cluster in &cluster_ranges {
                let samples = gather_cluster_samples(window, &cluster.ranges);
                let embedding = self.embed(&samples).unwrap_or_default();
                locals.push(LocalCluster {
                    embedding,
                    duration_secs: cluster.duration_secs,
                });
                local_ids.push(cluster.local_speaker);
            }

            // Stabilize: local → global ids (same order as `locals`).
            let globals = self.registry.assign(&locals);
            let local_to_global: std::collections::HashMap<i32, u32> =
                local_ids.into_iter().zip(globals).collect();

            // Emit ONLY the freshly-covered trailing hop, relabeled to globals.
            let emit_segs = trailing_hop_segments(&segments, window_secs, self.hop_secs);
            let mut out_segs = Vec::with_capacity(emit_segs.len());
            for seg in emit_segs {
                // A local id with no embedding produced no `locals` entry only if
                // its ranges were empty; otherwise it always has a global. Fall
                // back to UNKNOWN for the (rare) empty-range case.
                let global = local_to_global
                    .get(&seg.speaker)
                    .copied()
                    .unwrap_or(super::super::stabilize::UNKNOWN_SPEAKER);
                let stable = StableSegment {
                    start: seg.start,
                    end: seg.end,
                    global_speaker: global,
                };
                out_segs.push(stable);
                if out.send(stable).is_err() {
                    // Consumer gone — stop bothering for this window.
                    break;
                }
            }
            Ok(out_segs)
        }

        /// Run the sherpa embedding stream over one cluster's concatenated
        /// samples. Returns `None` (skip — too short / not ready) per research
        /// §1.5; `compute()` is raw, un-normalized (stabilize normalizes).
        fn embed(&self, samples: &[f32]) -> Option<Vec<f32>> {
            if samples.is_empty() {
                return None;
            }
            let stream = self.embedder.create_stream()?;
            stream.accept_waveform(self.sample_rate as i32, samples);
            stream.input_finished();
            if !self.embedder.is_ready(&stream) {
                return None;
            }
            self.embedder.compute(&stream)
        }

        /// Number of global speakers identified so far.
        pub fn speaker_count(&self) -> usize {
            self.registry.len()
        }
    }

    /// Pre-size the rolling buffer to one window plus a hop of slack.
    fn schedule_window_capacity(sample_rate: u32, window_secs: f32) -> usize {
        ((window_secs + 1.0) * sample_rate as f32) as usize
    }
}

#[cfg(feature = "diarization-clustering")]
pub use imp::{DiarizationFeed, LiveDiarizationWorker, StableSegment};

// ===========================================================================
// Tests — PURE glue only (no models / ONNX / audio). The model-backed runtime
// path is exercised by the env-gated test in `clustering.rs` (`AG_DIAR_*`).
// ===========================================================================
#[cfg(test)]
mod tests {
    use super::*;

    fn seg(start: f32, end: f32, speaker: i32) -> ClusterSegment {
        ClusterSegment {
            start,
            end,
            speaker,
        }
    }

    // -- cluster_sample_ranges --------------------------------------------

    #[test]
    fn groups_segments_by_local_speaker_into_sample_ranges() {
        // 1 kHz so seconds map cleanly to samples (1 s = 1000 samples).
        let segs = vec![
            seg(0.0, 1.0, 0), // spk0: [0,1000)
            seg(1.0, 2.0, 1), // spk1: [1000,2000)
            seg(2.0, 3.0, 0), // spk0: [2000,3000)
        ];
        let clusters = cluster_sample_ranges(&segs, 1_000, 4_000);
        assert_eq!(clusters.len(), 2);
        // First-appearance order: speaker 0, then speaker 1.
        assert_eq!(clusters[0].local_speaker, 0);
        assert_eq!(clusters[0].ranges, vec![(0, 1000), (2000, 3000)]);
        assert!((clusters[0].duration_secs - 2.0).abs() < 1e-4);
        assert_eq!(clusters[1].local_speaker, 1);
        assert_eq!(clusters[1].ranges, vec![(1000, 2000)]);
        assert!((clusters[1].duration_secs - 1.0).abs() < 1e-4);
    }

    #[test]
    fn clamps_ranges_to_window_len_and_drops_empty() {
        // A segment that runs past the buffer end is clamped; a zero/negative
        // span is dropped entirely.
        let segs = vec![
            seg(0.0, 5.0, 0), // clamps to [0, 4000)
            seg(2.0, 2.0, 1), // zero-length → dropped
            seg(3.0, 1.0, 2), // end < start → dropped
        ];
        let clusters = cluster_sample_ranges(&segs, 1_000, 4_000);
        assert_eq!(clusters.len(), 1, "only speaker 0 survives");
        assert_eq!(clusters[0].ranges, vec![(0, 4000)]);
        assert!((clusters[0].duration_secs - 4.0).abs() < 1e-4);
    }

    #[test]
    fn negative_start_is_clamped_to_zero() {
        let segs = vec![seg(-1.0, 1.0, 0)];
        let clusters = cluster_sample_ranges(&segs, 1_000, 4_000);
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].ranges, vec![(0, 1000)]);
    }

    #[test]
    fn empty_segments_yield_no_clusters() {
        assert!(cluster_sample_ranges(&[], 16_000, 16_000).is_empty());
    }

    // -- gather_cluster_samples -------------------------------------------

    #[test]
    fn gathers_and_concatenates_ranges_in_order() {
        let buf: Vec<f32> = (0..10).map(|i| i as f32).collect();
        let out = gather_cluster_samples(&buf, &[(0, 3), (5, 7)]);
        assert_eq!(out, vec![0.0, 1.0, 2.0, 5.0, 6.0]);
    }

    #[test]
    fn gather_skips_out_of_bounds_ranges_defensively() {
        let buf: Vec<f32> = (0..5).map(|i| i as f32).collect();
        // (3,9) overflows; (4,2) is reversed → both skipped, (0,2) kept.
        let out = gather_cluster_samples(&buf, &[(0, 2), (3, 9), (4, 2)]);
        assert_eq!(out, vec![0.0, 1.0]);
    }

    // -- segment_in_trailing_hop / trailing_hop_segments ------------------

    #[test]
    fn trailing_hop_keeps_only_overlapping_segments() {
        // 10 s window, 3 s hop → hop interval [7, 10).
        let window = 10.0;
        let hop = 3.0;
        assert!(
            !segment_in_trailing_hop(&seg(0.0, 5.0, 0), window, hop),
            "fully before the hop → not emitted"
        );
        assert!(
            segment_in_trailing_hop(&seg(6.5, 7.5, 0), window, hop),
            "straddles hop_start=7 → emitted"
        );
        assert!(
            segment_in_trailing_hop(&seg(8.0, 9.5, 0), window, hop),
            "inside the hop → emitted"
        );
        assert!(
            !segment_in_trailing_hop(&seg(10.0, 11.0, 0), window, hop),
            "at/after window end → not emitted"
        );
    }

    #[test]
    fn trailing_hop_segments_filters_in_order() {
        let segs = vec![
            seg(0.0, 2.0, 0),  // before hop
            seg(7.5, 8.0, 1),  // in hop
            seg(6.0, 9.0, 2),  // straddles
            seg(9.5, 10.0, 0), // in hop
        ];
        let kept = trailing_hop_segments(&segs, 10.0, 3.0);
        assert_eq!(
            kept,
            vec![seg(7.5, 8.0, 1), seg(6.0, 9.0, 2), seg(9.5, 10.0, 0)]
        );
    }

    #[test]
    fn trailing_hop_handles_short_first_window() {
        // Early in a session the actual window (=take/sr) is < configured 10 s.
        // hop_start = max(window-hop, 0) clamps so a 2 s window with 3 s hop
        // emits everything (hop_start=0).
        let window = 2.0;
        let hop = 3.0;
        assert!(segment_in_trailing_hop(&seg(0.0, 1.0, 0), window, hop));
        assert!(segment_in_trailing_hop(&seg(1.5, 1.9, 1), window, hop));
    }

    // -- rolling_trim_count ------------------------------------------------

    #[test]
    fn rolling_trim_drops_only_the_overflow() {
        // Buffer 25 s @ 16 kHz, window 10 s → trim 15 s of leading samples.
        let sr = 16_000u64;
        assert_eq!(
            rolling_trim_count((25 * sr) as usize, 10 * sr),
            (15 * sr) as usize
        );
        // Already within bound → no trim.
        assert_eq!(rolling_trim_count((5 * sr) as usize, 10 * sr), 0);
        // Exactly at bound → no trim.
        assert_eq!(rolling_trim_count((10 * sr) as usize, 10 * sr), 0);
    }
}
