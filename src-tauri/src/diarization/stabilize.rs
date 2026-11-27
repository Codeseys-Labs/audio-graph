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
//! 3. maximum-weight one-to-one assignment at or above `sim_threshold` (cannot-link);
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
/// Id returned for an inactive local or one whose embedding was unusable and so
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
    /// with no active speech or an unusable embedding receive [`UNKNOWN_SPEAKER`]
    /// without reserving a global id or updating a centroid.
    pub fn assign(&mut self, locals: &[LocalCluster]) -> Vec<u32> {
        // Pre-normalize local embeddings once.
        let mut normalized: Vec<_> = locals
            .iter()
            .map(|l| {
                if l.duration_secs > 0.0 {
                    l2_normalize(&l.embedding)
                } else {
                    None
                }
            })
            .collect();

        // Lock the embedding dimension from the first usable vector we ever see.
        if self.dim == 0
            && let Some(d) = normalized.iter().flatten().map(Vec::len).find(|&d| d > 0)
        {
            self.dim = d;
        }

        let n_local = locals.len();
        let n_global = self.speakers.len();

        // Build the local × global cosine matrix. Both sides are normalized, so
        // cosine == dot product. NaN marks missing evidence, not a low cosine
        // that could accidentally pass a permissive threshold.
        let mut sim = vec![vec![f32::NAN; n_global]; n_local];
        for (li, lvec) in normalized.iter().enumerate() {
            let Some(lvec) = lvec else { continue };
            if self.dim != 0 && lvec.len() != self.dim {
                continue;
            }
            for (gi, g) in self.speakers.iter().enumerate() {
                sim[li][gi] = dot(lvec, &g.centroid);
            }
        }

        let matches = optimal_assignment(&sim, self.sim_threshold);

        let mut out = vec![0u32; n_local];
        for li in 0..n_local {
            match matches[li] {
                Some(gi) => {
                    let gid = self.speakers[gi].id;
                    out[li] = gid;
                    // Duration-gated centroid update for confident matches.
                    if locals[li].duration_secs >= self.min_update_secs
                        && let Some(lvec) = &normalized[li]
                    {
                        update_centroid(&mut self.speakers[gi], lvec);
                    }
                }
                None => {
                    // Unmatched. Mint a new global ONLY for a usable embedding;
                    // a degenerate/empty (or wrong-dim) one gets the reserved
                    // UNKNOWN_SPEAKER id so it never becomes a permanent dead
                    // global that can never re-match (unbounded-growth guard).
                    match normalized[li].take() {
                        Some(v) if self.dim == 0 || v.len() == self.dim => {
                            let id = self.next_id;
                            self.next_id += 1;
                            self.speakers.push(GlobalSpeaker {
                                id,
                                centroid: v,
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
    if !norm.is_finite() || norm <= f32::EPSILON {
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

/// Rectangular Hungarian assignment. At positive thresholds a valid edge weighs
/// its cosine; a dummy (new speaker) weighs zero. Maximize total evidence, not
/// match count: two marginal matches need not displace one much stronger match.
/// For a nonpositive floor, shift all eligible cosines by 2 so even −1 beats a
/// dummy. In either case, below-floor edges are forbidden, not just low weight.
fn optimal_assignment(sim: &[Vec<f32>], threshold: f32) -> Vec<Option<usize>> {
    let n_local = sim.len();
    let n_global = sim.first().map(Vec::len).unwrap_or(0);
    let mut result = vec![None; n_local];
    if n_global == 0 {
        return result;
    }

    // One dummy per local makes a complete row assignment feasible even if
    // no real edge qualifies. Columns are implicit; no padded matrix is needed.
    // Indices are 1-based; column 0 roots each augmenting path at the new row.
    let cols = n_global + n_local;
    let shift = if threshold <= 0.0 { 2.0 } else { 0.0 };
    let mut row_price = vec![0.0; n_local + 1];
    let mut col_price = vec![0.0; cols + 1];
    let mut owner = vec![0; cols + 1];
    let mut previous = vec![0; cols + 1];
    let mut slack = vec![f64::INFINITY; cols + 1];
    let mut visited = vec![false; cols + 1];

    for row in 1..=n_local {
        owner[0] = row;
        let mut col = 0;
        slack.fill(f64::INFINITY);
        visited.fill(false);
        loop {
            visited[col] = true;
            let i = owner[col];
            let scores = &sim[i - 1];
            let mut delta = f64::INFINITY;
            let mut next = 0;
            // Shortest augmenting path on reduced costs (negated weights).
            // Ascending scans and strict comparisons keep the first equal path.
            for j in 1..=cols {
                if visited[j] {
                    continue;
                }
                let cost = match scores.get(j - 1) {
                    Some(&s) if s >= threshold => -(f64::from(s) + shift),
                    Some(_) => f64::INFINITY,
                    None => 0.0, // dummy
                };
                let reduced = cost - row_price[i] - col_price[j];
                if reduced < slack[j] {
                    slack[j] = reduced;
                    previous[j] = col;
                }
                if slack[j] < delta {
                    delta = slack[j];
                    next = j;
                }
            }
            // Make the next edge tight while keeping matched edges tight.
            for j in 0..=cols {
                if visited[j] {
                    row_price[owner[j]] += delta;
                    col_price[j] -= delta;
                } else {
                    slack[j] -= delta;
                }
            }
            col = next;
            if owner[col] == 0 {
                break;
            }
        }
        // Flip the tight path ending at the free column.
        while col != 0 {
            let parent = previous[col];
            owner[col] = owner[parent];
            col = parent;
        }
    }
    for (global, &row) in owner[1..=n_global].iter().enumerate() {
        if row != 0 {
            result[row - 1] = Some(global);
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
    fn optimal_assignment_avoids_greedy_dead_end() {
        // Greedy takes .90, leaving only the forbidden .40. Reassigning the
        // first local gives .80 + .85 and avoids minting a third speaker.
        let sim = vec![vec![0.90, 0.80], vec![0.85, 0.40]];
        assert_eq!(optimal_assignment(&sim, 0.6), [Some(1), Some(0)]);
    }

    #[test]
    fn optimal_assignment_no_double_booking() {
        // Both locals most resemble global0; only the stronger one wins it, the
        // other stays unmatched (cannot-link) rather than double-booking.
        let sim = vec![vec![0.95, 0.1], vec![0.9, 0.05]];
        let m = optimal_assignment(&sim, 0.6);
        assert_eq!(m, vec![Some(0), None]);
    }

    #[test]
    fn optimal_assignment_maximizes_similarity_not_match_count() {
        let sim = vec![vec![0.95, 0.3], vec![0.3, 0.0]];
        assert_eq!(optimal_assignment(&sim, 0.3), [Some(0), None]);
    }

    #[test]
    fn optimal_assignment_breaks_ties_by_scan_order() {
        let sim = vec![vec![0.6; 2]; 3];
        assert_eq!(optimal_assignment(&sim, 0.6), [Some(0), Some(1), None]);
    }

    #[test]
    fn optimal_assignment_accepts_the_floor_but_not_missing_evidence() {
        let sim = vec![vec![-1.0, f32::NAN], vec![f32::NAN, f32::NAN]];
        assert_eq!(optimal_assignment(&sim, -1.0), [Some(0), None]);
        assert_eq!(optimal_assignment(&sim, -0.99), [None, None]);
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
    fn registry_reassigns_contested_global_instead_of_minting() {
        let local = |x, y| LocalCluster {
            embedding: vec![x, y],
            duration_secs: 2.0,
        };
        let mut reg = SpeakerRegistry::with_defaults();
        assert_eq!(reg.assign(&[local(1.0, 0.0), local(0.0, 1.0)]), [0, 1]);
        // First row (.8, .6); second row (~.71, ~−.71) can only reuse id 0.
        assert_eq!(reg.assign(&[local(4.0, 3.0), local(1.0, -1.0)]), [1, 0]);
        assert_eq!(reg.len(), 2);
    }

    #[test]
    fn registry_empty_and_inactive_windows_leave_identity_untouched() {
        let mut reg = SpeakerRegistry::with_defaults();
        let mut local = LocalCluster {
            embedding: vec![1.0],
            duration_secs: 0.0,
        };
        assert!(reg.assign(&[]).is_empty());
        assert_eq!(reg.assign(std::slice::from_ref(&local)), [UNKNOWN_SPEAKER]);
        assert!(reg.is_empty());
        local.embedding = vec![1.0, 0.0];
        local.duration_secs = 2.0;
        assert_eq!(reg.assign(std::slice::from_ref(&local)), [0]);
        assert!(reg.assign(&[]).is_empty());
        local.duration_secs = 0.0;
        assert_eq!(reg.assign(&[local]), [UNKNOWN_SPEAKER]);
        assert_eq!(reg.len(), 1);
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
        assert_eq!(reg.speakers[0].centroid, [1.0, 0.0]);
        assert_eq!(reg.speakers[0].count, 1);
        // Still recognizes the original clean vector strongly.
        let w3 = reg.assign(&[LocalCluster {
            embedding: vec![1.0, 0.0],
            duration_secs: 3.0,
        }]);
        assert_eq!(w3[0], w1[0]);
        assert_eq!(reg.speakers[0].count, 2);
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
