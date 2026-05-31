//! Cross-window speaker-label stabilization for live (rolling-window) clustering
//! diarization (ADR-0017 §"streaming integration").
//!
//! `ClusteringDiarizer` (and any `OfflineSpeakerDiarization` backend) re-runs a
//! fresh clustering on every window, so the *local* speaker ids it returns are
//! permutation-arbitrary: local speaker `0` in window N has no relationship to
//! local speaker `0` in window N+1. To present a stable speaker identity over a
//! live stream we maintain a **global speaker registry** of L2-normalized
//! embedding centroids and map each window's local clusters onto it by cosine
//! similarity, with a "cannot-link" constraint (two locals in the same window —
//! already separated by the segmenter — never collapse onto one global).
//!
//! This module is **pure** (std-only, no ONNX, no audio I/O) so it is available
//! and unit-tested in every build regardless of the `diarization-clustering`
//! feature. The feature-gated worker that owns the diarizer + audio ring buffer
//! computes one embedding per local cluster and feeds them here.
//!
//! Algorithm (per the diart / Coria et al. 2021 incremental-clustering scheme):
//! 1. one L2-normalized embedding + active duration per local cluster;
//! 2. cosine similarity matrix local × global;
//! 3. greedy one-to-one assignment above `sim_threshold` (cannot-link);
//! 4. unmatched locals mint new global speakers;
//! 5. confident, long-enough matches update the global centroid (running mean,
//!    re-normalized) — short segments don't corrupt centroids.

/// A local cluster produced for one window: its mean speaker embedding and how
/// much speech (seconds) it accounts for within the window.
#[derive(Debug, Clone)]
pub struct LocalCluster {
    /// Raw (un-normalized is fine) embedding vector for this local speaker.
    pub embedding: Vec<f32>,
    /// Total active speech duration (s) for this local speaker in the window.
    /// Gates centroid updates so brief, noisy segments don't pollute identity.
    pub duration_secs: f32,
}

#[derive(Debug, Clone)]
struct GlobalSpeaker {
    id: u32,
    /// L2-normalized running-mean centroid.
    centroid: Vec<f32>,
    /// Number of confident contributions folded into the centroid.
    count: u32,
}

/// Maintains stable global speaker ids across windows by matching per-window
/// embeddings to retained centroids.
#[derive(Debug, Clone)]
pub struct SpeakerRegistry {
    speakers: Vec<GlobalSpeaker>,
    next_id: u32,
    /// Embedding dimension; locked on first insert (0 = not yet locked).
    dim: usize,
    /// Cosine-similarity floor to reuse an existing global (else mint a new one).
    sim_threshold: f32,
    /// Minimum active duration (s) for a match to update its centroid.
    min_update_secs: f32,
}

/// Default cross-window cosine-similarity floor for reusing a global speaker.
/// Separate from the within-window `clustering.threshold`. Higher ⇒ stricter
/// (more new speakers); 0.55–0.70 is a sane range for ERes2Net/TitaNet cosine.
pub const DEFAULT_SIM_THRESHOLD: f32 = 0.6;
/// Default minimum active duration (s) before a match updates its centroid.
pub const DEFAULT_MIN_UPDATE_SECS: f32 = 1.5;
/// Id returned for a local cluster whose embedding was empty/degenerate and so
/// could neither be matched nor registered. Reserved so such clusters never
/// mint permanent, never-matching "dead" globals that would grow the registry
/// unboundedly over a long live session.
pub const UNKNOWN_SPEAKER: u32 = u32::MAX;

impl SpeakerRegistry {
    pub fn new(sim_threshold: f32, min_update_secs: f32) -> Self {
        Self {
            speakers: Vec::new(),
            next_id: 0,
            dim: 0,
            sim_threshold,
            min_update_secs,
        }
    }

    pub fn with_defaults() -> Self {
        Self::new(DEFAULT_SIM_THRESHOLD, DEFAULT_MIN_UPDATE_SECS)
    }

    /// Number of distinct global speakers seen so far.
    pub fn len(&self) -> usize {
        self.speakers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.speakers.is_empty()
    }

    /// Assign global speaker ids to a window's local clusters.
    ///
    /// Returns one global id per input local cluster, in the same order. Locals
    /// with an empty or wrong-dimension embedding are still assigned (a fresh
    /// global is minted) so the caller never loses a segment, but they do not
    /// update any centroid.
    pub fn assign(&mut self, locals: &[LocalCluster]) -> Vec<u32> {
        // Pre-normalize local embeddings once.
        let normalized: Vec<Option<Vec<f32>>> =
            locals.iter().map(|l| l2_normalize(&l.embedding)).collect();

        // Lock the embedding dimension from the first usable vector we ever see.
        if self.dim == 0 {
            if let Some(d) = normalized.iter().flatten().map(Vec::len).find(|&d| d > 0) {
                self.dim = d;
            }
        }

        let n_local = locals.len();
        let n_global = self.speakers.len();

        // Build the local × global cosine matrix. Both sides are normalized, so
        // cosine == dot product. Unusable locals get an all-`-1` row (no match).
        let mut sim = vec![vec![-1.0f32; n_global]; n_local];
        for (li, lvec) in normalized.iter().enumerate() {
            let Some(lvec) = lvec else { continue };
            if self.dim != 0 && lvec.len() != self.dim {
                continue;
            }
            for (gi, g) in self.speakers.iter().enumerate() {
                sim[li][gi] = dot(lvec, &g.centroid);
            }
        }

        let matches = greedy_assign(&sim, self.sim_threshold);

        let mut out = vec![0u32; n_local];
        for li in 0..n_local {
            match matches[li] {
                Some(gi) => {
                    let gid = self.speakers[gi].id;
                    out[li] = gid;
                    // Duration-gated centroid update for confident matches.
                    if locals[li].duration_secs >= self.min_update_secs {
                        if let Some(lvec) = &normalized[li] {
                            update_centroid(&mut self.speakers[gi], lvec);
                        }
                    }
                }
                None => {
                    // Unmatched. Mint a new global ONLY for a usable embedding;
                    // a degenerate/empty (or wrong-dim) one gets the reserved
                    // UNKNOWN_SPEAKER id so it never becomes a permanent dead
                    // global that can never re-match (unbounded-growth guard).
                    match &normalized[li] {
                        Some(v) if self.dim == 0 || v.len() == self.dim => {
                            let id = self.next_id;
                            self.next_id += 1;
                            self.speakers.push(GlobalSpeaker {
                                id,
                                centroid: v.clone(),
                                count: 1,
                            });
                            out[li] = id;
                        }
                        _ => out[li] = UNKNOWN_SPEAKER,
                    }
                }
            }
        }
        out
    }
}

/// L2-normalize a vector. Returns `None` for an empty or zero-magnitude vector
/// (cosine is undefined / would divide by zero).
pub fn l2_normalize(v: &[f32]) -> Option<Vec<f32>> {
    if v.is_empty() {
        return None;
    }
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm <= f32::EPSILON {
        return None;
    }
    Some(v.iter().map(|x| x / norm).collect())
}

/// Dot product of two equal-length slices (cosine similarity for L2-normalized
/// inputs). Returns 0.0 for length mismatch (treated as "no similarity").
pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Greedy one-to-one assignment with a cannot-link constraint: sort every
/// (local, global) pair by descending similarity and accept a pair only if
/// neither side is already taken and the similarity meets `threshold`. This is
/// the cheap, deterministic alternative to Hungarian assignment and is adequate
/// at the handful-of-speakers scale of a single window.
///
/// Returns, for each local row index, the matched global column index (or
/// `None` if it should become a new speaker).
pub fn greedy_assign(sim: &[Vec<f32>], threshold: f32) -> Vec<Option<usize>> {
    let n_local = sim.len();
    let n_global = sim.first().map(Vec::len).unwrap_or(0);
    let mut result = vec![None; n_local];
    if n_global == 0 {
        return result;
    }

    let mut triples: Vec<(f32, usize, usize)> = Vec::with_capacity(n_local * n_global);
    for (li, row) in sim.iter().enumerate() {
        for (gi, &s) in row.iter().enumerate() {
            if s >= threshold {
                triples.push((s, li, gi));
            }
        }
    }
    // Descending similarity; ties broken by (local, global) for determinism.
    triples.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.cmp(&b.1))
            .then(a.2.cmp(&b.2))
    });

    let mut local_taken = vec![false; n_local];
    let mut global_taken = vec![false; n_global];
    for (_, li, gi) in triples {
        if !local_taken[li] && !global_taken[gi] {
            result[li] = Some(gi);
            local_taken[li] = true;
            global_taken[gi] = true;
        }
    }
    result
}

/// Fold a new normalized embedding into a global speaker's centroid as a running
/// mean and re-normalize, so the centroid stays a unit vector. Contributions are
/// count-weighted, NOT duration-weighted — `duration_secs` only gates *whether*
/// to update (in `assign`), so a 2s and a 30s confident segment move the centroid
/// equally. Centroids are always non-empty + dim-locked here (degenerate inputs
/// never reach this path), so a length mismatch can only be a dim change → skip.
fn update_centroid(speaker: &mut GlobalSpeaker, normalized: &[f32]) {
    if speaker.centroid.len() != normalized.len() {
        return;
    }
    let n = speaker.count as f32;
    for (c, x) in speaker.centroid.iter_mut().zip(normalized) {
        *c = (*c * n + *x) / (n + 1.0);
    }
    if let Some(renorm) = l2_normalize(&speaker.centroid) {
        speaker.centroid = renorm;
    }
    speaker.count += 1;
}

/// Rolling-window scheduler for applying an offline diarizer to a live stream.
///
/// Pure bookkeeping over **sample counts** (no audio buffer here — the worker
/// owns the ring buffer): tracks total samples ingested and decides when the
/// next window is due (`window` samples available, advancing by `hop`). Kept
/// separate + pure so the cadence logic is deterministically testable.
#[derive(Debug, Clone)]
pub struct WindowSchedule {
    window: u64,
    hop: u64,
    total: u64,
    next_run_at: u64,
}

impl WindowSchedule {
    /// `window_secs` of context per run, advancing `hop_secs` each run, not
    /// running until `min_start_secs` of audio exists. All converted to samples
    /// via `sample_rate`, which callers MUST pass as a validated, non-zero rate
    /// (a 0 rate degenerates to 1-sample windows rather than panicking).
    pub fn new(sample_rate: u32, window_secs: f32, hop_secs: f32, min_start_secs: f32) -> Self {
        debug_assert!(
            sample_rate > 0,
            "WindowSchedule needs a non-zero sample_rate"
        );
        let sr = sample_rate as f32;
        let window = (window_secs * sr).max(1.0) as u64;
        let hop = (hop_secs * sr).max(1.0) as u64;
        let min_start = ((min_start_secs * sr).max(0.0) as u64).max(1);
        Self {
            window,
            hop,
            total: 0,
            next_run_at: min_start,
        }
    }

    /// Record `n` newly ingested samples.
    pub fn ingest(&mut self, n: u64) {
        self.total = self.total.saturating_add(n);
    }

    /// If a window is due, advance the schedule and return the number of
    /// trailing samples the worker should pass to `diarize` (`min(window, total)`).
    pub fn poll(&mut self) -> Option<u64> {
        if self.total >= self.next_run_at {
            let take = self.window.min(self.total);
            // Advance by whole hops past the current total so we don't backlog.
            while self.next_run_at <= self.total {
                self.next_run_at = self.next_run_at.saturating_add(self.hop);
            }
            Some(take)
        } else {
            None
        }
    }

    pub fn window_samples(&self) -> u64 {
        self.window
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn l2_normalize_unit_and_edges() {
        let v = l2_normalize(&[3.0, 4.0]).unwrap();
        assert!(approx(v[0], 0.6) && approx(v[1], 0.8));
        assert!(l2_normalize(&[]).is_none());
        assert!(l2_normalize(&[0.0, 0.0]).is_none());
    }

    #[test]
    fn dot_matches_cosine_for_normalized() {
        let a = l2_normalize(&[1.0, 0.0]).unwrap();
        let b = l2_normalize(&[1.0, 0.0]).unwrap();
        assert!(approx(dot(&a, &b), 1.0));
        let c = l2_normalize(&[0.0, 1.0]).unwrap();
        assert!(approx(dot(&a, &c), 0.0));
        assert_eq!(dot(&[1.0], &[1.0, 2.0]), 0.0); // length mismatch
    }

    #[test]
    fn greedy_assign_respects_cannot_link() {
        // 2 locals, 2 globals. local0 ~ global1, local1 ~ global0.
        let sim = vec![vec![0.1, 0.9], vec![0.8, 0.2]];
        let m = greedy_assign(&sim, 0.6);
        assert_eq!(m, vec![Some(1), Some(0)]);
    }

    #[test]
    fn greedy_assign_no_double_booking() {
        // Both locals most resemble global0; only the stronger one wins it, the
        // other stays unmatched (cannot-link) rather than double-booking.
        let sim = vec![vec![0.95, 0.1], vec![0.9, 0.05]];
        let m = greedy_assign(&sim, 0.6);
        assert_eq!(m, vec![Some(0), None]);
    }

    #[test]
    fn greedy_assign_threshold_filters() {
        let sim = vec![vec![0.4, 0.3]];
        assert_eq!(greedy_assign(&sim, 0.6), vec![None]);
    }

    #[test]
    fn registry_first_window_mints_speakers() {
        let mut reg = SpeakerRegistry::with_defaults();
        let ids = reg.assign(&[
            LocalCluster {
                embedding: vec![1.0, 0.0],
                duration_secs: 3.0,
            },
            LocalCluster {
                embedding: vec![0.0, 1.0],
                duration_secs: 3.0,
            },
        ]);
        assert_eq!(ids, vec![0, 1]);
        assert_eq!(reg.len(), 2);
    }

    #[test]
    fn registry_matches_same_speaker_across_windows() {
        let mut reg = SpeakerRegistry::with_defaults();
        let w1 = reg.assign(&[
            LocalCluster {
                embedding: vec![1.0, 0.0],
                duration_secs: 3.0,
            },
            LocalCluster {
                embedding: vec![0.0, 1.0],
                duration_secs: 3.0,
            },
        ]);
        // Next window: same two speakers but local order swapped + slight noise.
        let w2 = reg.assign(&[
            LocalCluster {
                embedding: vec![0.05, 0.99],
                duration_secs: 3.0,
            },
            LocalCluster {
                embedding: vec![0.99, 0.05],
                duration_secs: 3.0,
            },
        ]);
        // Stable global ids: w2[0] is the speaker that was w1[1]; w2[1] was w1[0].
        assert_eq!(w2[0], w1[1]);
        assert_eq!(w2[1], w1[0]);
        assert_eq!(reg.len(), 2, "no spurious new speakers");
    }

    #[test]
    fn registry_adds_new_speaker_when_dissimilar() {
        let mut reg = SpeakerRegistry::with_defaults();
        reg.assign(&[LocalCluster {
            embedding: vec![1.0, 0.0, 0.0],
            duration_secs: 3.0,
        }]);
        let w2 = reg.assign(&[
            LocalCluster {
                embedding: vec![1.0, 0.0, 0.0],
                duration_secs: 3.0,
            }, // same
            LocalCluster {
                embedding: vec![0.0, 0.0, 1.0],
                duration_secs: 3.0,
            }, // new
        ]);
        assert_eq!(w2[0], 0);
        assert_eq!(w2[1], 1);
        assert_eq!(reg.len(), 2);
    }

    #[test]
    fn registry_short_segments_do_not_update_centroid() {
        let mut reg = SpeakerRegistry::new(0.6, 1.5);
        let w1 = reg.assign(&[LocalCluster {
            embedding: vec![1.0, 0.0],
            duration_secs: 3.0,
        }]);
        // A brief, noisy observation matches but must not drag the centroid.
        let w2 = reg.assign(&[LocalCluster {
            embedding: vec![0.7, 0.71],
            duration_secs: 0.2,
        }]);
        assert_eq!(w2[0], w1[0]);
        // Still recognizes the original clean vector strongly.
        let w3 = reg.assign(&[LocalCluster {
            embedding: vec![1.0, 0.0],
            duration_secs: 3.0,
        }]);
        assert_eq!(w3[0], w1[0]);
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn registry_handles_empty_embedding_without_panic() {
        let mut reg = SpeakerRegistry::with_defaults();
        let ids = reg.assign(&[LocalCluster {
            embedding: vec![],
            duration_secs: 1.0,
        }]);
        // Degenerate embedding → reserved id, and NO permanent dead global minted.
        assert_eq!(ids, vec![UNKNOWN_SPEAKER]);
        assert!(reg.is_empty());
    }

    #[test]
    fn window_schedule_waits_then_fires_on_hop() {
        // 16kHz, 10s window, 3s hop, min 6s.
        let mut s = WindowSchedule::new(16_000, 10.0, 3.0, 6.0);
        s.ingest(5 * 16_000);
        assert_eq!(s.poll(), None, "below min_start");
        s.ingest(2 * 16_000); // total 7s >= 6s min
        assert_eq!(s.poll(), Some(7 * 16_000), "fires, takes min(window,total)");
        assert_eq!(s.poll(), None, "no new audio since last run");
        s.ingest(3 * 16_000); // total 10s, crossed next hop (9s)
        assert_eq!(s.poll(), Some(10 * 16_000));
        s.ingest(20 * 16_000); // total 30s, window caps the take
        assert_eq!(s.poll(), Some(10 * 16_000));
    }
}
