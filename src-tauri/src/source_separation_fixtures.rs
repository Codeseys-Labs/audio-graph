use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct SourceSeparationFixtureManifest {
    schema_version: u32,
    id: String,
    format: AudioFormat,
    source_dataset: SourceDataset,
    speakers: Vec<Speaker>,
    source_components: Vec<SourceComponent>,
    fixtures: Vec<Fixture>,
    candidate_quality_thresholds: CandidateQualityThresholds,
}

#[derive(Debug, Deserialize)]
struct AudioFormat {
    container: String,
    encoding: String,
    sample_rate: u32,
    channels: u16,
    bit_depth: u16,
}

#[derive(Debug, Deserialize)]
struct SourceDataset {
    license: String,
    license_url: String,
    homepage_url: String,
    dataset_card_url: String,
    derived_from_url: String,
}

#[derive(Debug, Deserialize)]
struct Speaker {
    id: String,
    source_speaker_id: String,
}

#[derive(Debug, Deserialize)]
struct SourceComponent {
    id: String,
    speaker_id: String,
    audio_path: String,
    duration_ms: u64,
    text: String,
}

#[derive(Debug, Deserialize)]
struct Fixture {
    id: String,
    kind: FixtureKind,
    audio_path: String,
    duration_ms: u64,
    expected_speaker_count: u32,
    segments: Vec<Segment>,
    baseline: Baseline,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum FixtureKind {
    Overlap,
    TurnTaking,
}

#[derive(Debug, Deserialize)]
struct Segment {
    speaker_id: String,
    source_component_id: String,
    start_ms: u64,
    end_ms: u64,
    text: String,
}

#[derive(Debug, Deserialize)]
struct Baseline {
    mono_asr: BaselineStatus,
    diarization: DiarizationBaselineStatus,
}

#[derive(Debug, Deserialize)]
struct BaselineStatus {
    status: String,
    reference_transcript: String,
    required_before_close: bool,
}

#[derive(Debug, Deserialize)]
struct DiarizationBaselineStatus {
    status: String,
    expected_speaker_count: u32,
    expected_overlap_regions: Vec<ExpectedOverlapRegion>,
    required_before_close: bool,
}

#[derive(Debug, Deserialize)]
struct ExpectedOverlapRegion {
    start_ms: u64,
    end_ms: u64,
    speaker_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CandidateQualityThresholds {
    overlap_region_wer_must_improve_over_mono: bool,
    artifact_flag_requires_mono_fallback: bool,
    source_native_channel_claim_allowed: bool,
    generated_speaker_lane_selectable_without_baseline: bool,
}

#[derive(Debug)]
struct WavInfo {
    audio_format: u16,
    channels: u16,
    sample_rate: u32,
    bit_depth: u16,
    data_bytes: u32,
}

#[test]
fn source_separation_manifest_has_required_fixture_shapes() {
    let manifest = load_manifest();

    assert_eq!(manifest.schema_version, 1);
    assert_eq!(manifest.id, "librispeech-two-speaker-mini-v1");
    assert_eq!(manifest.format.container, "wav");
    assert_eq!(manifest.format.encoding, "pcm_s16le");
    assert_eq!(manifest.format.sample_rate, 16_000);
    assert_eq!(manifest.format.channels, 1);
    assert_eq!(manifest.format.bit_depth, 16);

    assert_eq!(manifest.source_dataset.license, "CC BY 4.0");
    assert!(manifest
        .source_dataset
        .license_url
        .starts_with("https://creativecommons.org/licenses/by/4.0/"));
    assert!(manifest.source_dataset.homepage_url.contains("openslr.org"));
    assert!(manifest
        .source_dataset
        .dataset_card_url
        .contains("librispeech_asr"));
    assert!(manifest
        .source_dataset
        .derived_from_url
        .contains("librivox"));

    let speaker_ids = manifest
        .speakers
        .iter()
        .map(|speaker| speaker.id.as_str())
        .collect::<HashSet<_>>();
    assert!(speaker_ids.contains("speaker_a"));
    assert!(speaker_ids.contains("speaker_b"));
    assert_eq!(speaker_ids.len(), 2);
    assert!(manifest
        .speakers
        .iter()
        .all(|speaker| !speaker.source_speaker_id.is_empty()));

    let component_ids = manifest
        .source_components
        .iter()
        .map(|component| component.id.as_str())
        .collect::<HashSet<_>>();
    assert!(component_ids.contains("source_a"));
    assert!(component_ids.contains("source_b"));
    for component in &manifest.source_components {
        assert!(
            speaker_ids.contains(component.speaker_id.as_str()),
            "{} references unknown speaker {}",
            component.id,
            component.speaker_id
        );
        assert!(!component.text.trim().is_empty());
        assert_eq!(component.duration_ms, 3000);
    }

    let overlap = manifest
        .fixtures
        .iter()
        .find(|fixture| fixture.kind == FixtureKind::Overlap)
        .expect("fixture set must include an overlap clip");
    assert!(
        has_cross_speaker_overlap(overlap),
        "overlap fixture must include an overlapping region"
    );

    let turn_taking = manifest
        .fixtures
        .iter()
        .find(|fixture| fixture.kind == FixtureKind::TurnTaking)
        .expect("fixture set must include a turn-taking clip");
    assert!(
        !has_cross_speaker_overlap(turn_taking),
        "turn-taking fixture should not overlap speakers"
    );

    for fixture in &manifest.fixtures {
        assert!(!fixture.id.trim().is_empty());
        assert_eq!(fixture.expected_speaker_count, 2);
        assert_eq!(fixture.baseline.mono_asr.status, "pending_real_run");
        assert_eq!(fixture.baseline.diarization.status, "pending_real_run");
        assert!(fixture.baseline.mono_asr.required_before_close);
        assert!(fixture.baseline.diarization.required_before_close);
        assert!(!fixture.baseline.mono_asr.reference_transcript.is_empty());
        assert_eq!(
            fixture.baseline.diarization.expected_speaker_count,
            fixture.expected_speaker_count
        );
        for region in &fixture.baseline.diarization.expected_overlap_regions {
            assert!(region.start_ms < region.end_ms);
            assert_eq!(region.speaker_ids.len(), 2);
        }
        for segment in &fixture.segments {
            assert!(speaker_ids.contains(segment.speaker_id.as_str()));
            assert!(component_ids.contains(segment.source_component_id.as_str()));
            assert!(segment.start_ms < segment.end_ms);
            assert!(segment.end_ms <= fixture.duration_ms);
            assert!(!segment.text.trim().is_empty());
        }
    }

    assert!(
        manifest
            .candidate_quality_thresholds
            .overlap_region_wer_must_improve_over_mono
    );
    assert!(
        manifest
            .candidate_quality_thresholds
            .artifact_flag_requires_mono_fallback
    );
    assert!(
        !manifest
            .candidate_quality_thresholds
            .source_native_channel_claim_allowed
    );
    assert!(
        !manifest
            .candidate_quality_thresholds
            .generated_speaker_lane_selectable_without_baseline
    );
}

#[test]
fn source_separation_wavs_match_manifest_format() {
    let manifest = load_manifest();
    let mut audio_paths = manifest
        .source_components
        .iter()
        .map(|component| (component.audio_path.as_str(), component.duration_ms))
        .collect::<Vec<_>>();
    audio_paths.extend(
        manifest
            .fixtures
            .iter()
            .map(|fixture| (fixture.audio_path.as_str(), fixture.duration_ms)),
    );

    for (relative_path, expected_duration_ms) in audio_paths {
        let path = fixture_root().join(relative_path);
        let info = parse_wav_info(&path);
        assert_eq!(info.audio_format, 1, "{} must be PCM", path.display());
        assert_eq!(
            info.sample_rate,
            manifest.format.sample_rate,
            "{} sample rate",
            path.display()
        );
        assert_eq!(
            info.channels,
            manifest.format.channels,
            "{} channels",
            path.display()
        );
        assert_eq!(
            info.bit_depth,
            manifest.format.bit_depth,
            "{} bit depth",
            path.display()
        );
        assert_eq!(
            wav_duration_ms(&info),
            expected_duration_ms,
            "{} duration",
            path.display()
        );
    }
}

fn load_manifest() -> SourceSeparationFixtureManifest {
    let path = fixture_root().join("manifest.json");
    let body = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    serde_json::from_str(&body)
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()))
}

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("source_separation")
}

fn has_cross_speaker_overlap(fixture: &Fixture) -> bool {
    fixture.segments.iter().enumerate().any(|(index, left)| {
        fixture.segments.iter().skip(index + 1).any(|right| {
            left.speaker_id != right.speaker_id
                && left.start_ms < right.end_ms
                && right.start_ms < left.end_ms
        })
    })
}

fn parse_wav_info(path: &Path) -> WavInfo {
    let data =
        fs::read(path).unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    assert!(data.len() >= 44, "{} is too small for WAV", path.display());
    assert_eq!(&data[0..4], b"RIFF", "{} RIFF header", path.display());
    assert_eq!(&data[8..12], b"WAVE", "{} WAVE header", path.display());

    let mut offset = 12usize;
    let mut audio_format = None;
    let mut channels = None;
    let mut sample_rate = None;
    let mut bit_depth = None;
    let mut data_bytes = None;

    while offset + 8 <= data.len() {
        let chunk_id = &data[offset..offset + 4];
        let chunk_size = u32::from_le_bytes(
            data[offset + 4..offset + 8]
                .try_into()
                .expect("chunk size slice"),
        ) as usize;
        let chunk_start = offset + 8;
        let chunk_end = chunk_start + chunk_size;
        assert!(
            chunk_end <= data.len(),
            "{} has truncated WAV chunk",
            path.display()
        );

        if chunk_id == b"fmt " {
            assert!(chunk_size >= 16, "{} has short fmt chunk", path.display());
            audio_format = Some(u16::from_le_bytes(
                data[chunk_start..chunk_start + 2]
                    .try_into()
                    .expect("audio format slice"),
            ));
            channels = Some(u16::from_le_bytes(
                data[chunk_start + 2..chunk_start + 4]
                    .try_into()
                    .expect("channel slice"),
            ));
            sample_rate = Some(u32::from_le_bytes(
                data[chunk_start + 4..chunk_start + 8]
                    .try_into()
                    .expect("sample rate slice"),
            ));
            bit_depth = Some(u16::from_le_bytes(
                data[chunk_start + 14..chunk_start + 16]
                    .try_into()
                    .expect("bit depth slice"),
            ));
        } else if chunk_id == b"data" {
            data_bytes = Some(chunk_size as u32);
        }

        offset = chunk_end + (chunk_size % 2);
    }

    WavInfo {
        audio_format: audio_format.expect("WAV fmt chunk missing"),
        channels: channels.expect("WAV channels missing"),
        sample_rate: sample_rate.expect("WAV sample rate missing"),
        bit_depth: bit_depth.expect("WAV bit depth missing"),
        data_bytes: data_bytes.expect("WAV data chunk missing"),
    }
}

fn wav_duration_ms(info: &WavInfo) -> u64 {
    let bytes_per_frame = u64::from(info.channels) * u64::from(info.bit_depth / 8);
    let frame_count = u64::from(info.data_bytes) / bytes_per_frame;
    (frame_count * 1000) / u64::from(info.sample_rate)
}

// ===========================================================================
// Offline baseline runner (seed audio-graph-c237, workstream D3).
//
// The fixture DATA + the two validators above are the prerequisite slice. This
// section adds the *runner*: it loads the checked-in WAVs, runs the mono-ASR and
// clustering-diarization baselines WHEN a real backend is available, computes
// WER + turn-fragmentation (mono-ASR) and speaker-count + overlap-region
// accuracy/churn (diarization) against the manifest's expectations, and writes
// the results to a sibling `baseline_results.json`.
//
// It does NOT modify the diarizer or the ASR engine — it only reads their public
// APIs (`asr::AsrWorker::transcribe_segment`, `diarization::clustering::
// ClusteringDiarizer::diarize`). Neither real backend ships in the offline CI
// gate (`--no-default-features --features cloud`): whisper is behind
// `asr-whisper` and the clustering diarizer behind `diarization-clustering`.
// So in CI the runner stays in "baseline still pending_real_run" mode — exactly
// what the manifest declares and the validators assert. The metric math and the
// manifest plumbing are exercised offline by unit tests, with no models and no
// network, so the gate actually executes this code.
//
// To record a REAL baseline locally, set the env vars below and run this module
// with `--ignored` (the real-run tests are `#[ignore]`d so CI never needs them):
//   AG_SS_ASR_MODEL=/path/ggml-base.en.bin            (whisper GGML model)
//   AG_DIAR_SEG_MODEL=/path/pyannote-segmentation.onnx
//   AG_DIAR_EMB_MODEL=/path/3dspeaker-embedding.onnx
//   cargo test --features asr-whisper,diarization-clustering \
//       --lib source_separation_fixtures::baseline -- --ignored --test-threads=1
// ===========================================================================
mod baseline {
    use super::{
        fixture_root, load_manifest, parse_wav_info, ExpectedOverlapRegion, Fixture, FixtureKind,
    };
    use serde::Serialize;
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::{Path, PathBuf};

    // ── WAV → mono f32 16 kHz samples ──────────────────────────────────────
    // Reuses the same RIFF walk the validator uses (no new dependency); returns
    // the PCM s16le `data` chunk decoded to mono f32 in [-1.0, 1.0].

    /// Decode a checked-in fixture WAV (PCM s16le, mono, 16 kHz per the manifest)
    /// into f32 samples. Asserts the format invariants the diarizer/ASR require.
    fn load_wav_samples_16k_mono(path: &Path) -> Vec<f32> {
        let info = parse_wav_info(path);
        assert_eq!(info.audio_format, 1, "{} must be PCM", path.display());
        assert_eq!(info.channels, 1, "{} must be mono", path.display());
        assert_eq!(
            info.sample_rate,
            16_000,
            "{} must be 16 kHz",
            path.display()
        );
        assert_eq!(info.bit_depth, 16, "{} must be 16-bit", path.display());

        let data = fs::read(path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));

        // Walk to the `data` chunk payload and decode i16 LE → f32.
        let mut offset = 12usize;
        while offset + 8 <= data.len() {
            let chunk_id = &data[offset..offset + 4];
            let chunk_size = u32::from_le_bytes(
                data[offset + 4..offset + 8]
                    .try_into()
                    .expect("chunk size slice"),
            ) as usize;
            let chunk_start = offset + 8;
            let chunk_end = chunk_start + chunk_size;
            if chunk_id == b"data" {
                let payload = &data[chunk_start..chunk_end.min(data.len())];
                return payload
                    .chunks_exact(2)
                    .map(|c| {
                        let sample = i16::from_le_bytes([c[0], c[1]]);
                        sample as f32 / 32_768.0
                    })
                    .collect();
            }
            offset = chunk_end + (chunk_size % 2);
        }
        panic!("{} has no data chunk", path.display());
    }

    // ── Text normalization + WER ───────────────────────────────────────────

    /// Lowercase, strip punctuation, collapse whitespace into word tokens — the
    /// standard normalization before scoring word error rate. Keeps apostrophes
    /// inside words (don't → dont) by dropping all non-alphanumeric chars.
    fn normalize_words(text: &str) -> Vec<String> {
        text.split_whitespace()
            .map(|word| {
                word.chars()
                    .filter(|c| c.is_alphanumeric())
                    .flat_map(|c| c.to_lowercase())
                    .collect::<String>()
            })
            .filter(|w| !w.is_empty())
            .collect()
    }

    /// Word-level edit operations (substitutions + deletions + insertions)
    /// between a reference and a hypothesis token stream — the numerator of WER.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
    pub struct EditCounts {
        pub substitutions: usize,
        pub deletions: usize,
        pub insertions: usize,
        pub reference_words: usize,
    }

    impl EditCounts {
        fn total_errors(&self) -> usize {
            self.substitutions + self.deletions + self.insertions
        }

        /// WER = (S + D + I) / N_ref. An empty reference yields 0.0 when the
        /// hypothesis is also empty, else 1.0-per-inserted-word (capped sane).
        fn wer(&self) -> f64 {
            if self.reference_words == 0 {
                return if self.insertions == 0 { 0.0 } else { 1.0 };
            }
            self.total_errors() as f64 / self.reference_words as f64
        }
    }

    /// Levenshtein alignment over word tokens, classifying each edit as a
    /// substitution / deletion / insertion (the components of WER).
    fn word_edit_counts(reference: &[String], hypothesis: &[String]) -> EditCounts {
        let n = reference.len();
        let m = hypothesis.len();
        // dp[i][j] = (cost, S, D, I) to align reference[..i] with hypothesis[..j].
        let mut dp = vec![vec![(0usize, 0usize, 0usize, 0usize); m + 1]; n + 1];
        // Column 0: i deletions of the first i reference words.
        for i in 0..=n {
            dp[i][0] = (i, 0, i, 0);
        }
        // Row 0: j insertions of the first j hypothesis words.
        for j in 0..=m {
            dp[0][j] = (j, 0, 0, j);
        }
        for i in 1..=n {
            for j in 1..=m {
                if reference[i - 1] == hypothesis[j - 1] {
                    dp[i][j] = dp[i - 1][j - 1];
                } else {
                    let sub = dp[i - 1][j - 1];
                    let del = dp[i - 1][j];
                    let ins = dp[i][j - 1];
                    // Choose the minimum-cost predecessor; tie-break sub<del<ins.
                    let sub_cost = sub.0 + 1;
                    let del_cost = del.0 + 1;
                    let ins_cost = ins.0 + 1;
                    if sub_cost <= del_cost && sub_cost <= ins_cost {
                        dp[i][j] = (sub_cost, sub.1 + 1, sub.2, sub.3);
                    } else if del_cost <= ins_cost {
                        dp[i][j] = (del_cost, del.1, del.2 + 1, del.3);
                    } else {
                        dp[i][j] = (ins_cost, ins.1, ins.2, ins.3 + 1);
                    }
                }
            }
        }
        let (_, s, d, i) = dp[n][m];
        EditCounts {
            substitutions: s,
            deletions: d,
            insertions: i,
            reference_words: n,
        }
    }

    /// Compute WER for a hypothesis transcript against a reference transcript.
    pub fn compute_wer(reference: &str, hypothesis: &str) -> (f64, EditCounts) {
        let counts = word_edit_counts(&normalize_words(reference), &normalize_words(hypothesis));
        (counts.wer(), counts)
    }

    // ── Mono-ASR baseline result ───────────────────────────────────────────

    /// A scored mono-ASR baseline for one fixture: WER vs the manifest reference
    /// plus a turn-fragmentation count (how many ASR output segments the engine
    /// produced for the clip). Real-run only fills `hypothesis`; the math is
    /// otherwise identical to the unit-tested path.
    #[derive(Debug, Clone, Serialize)]
    pub struct MonoAsrResult {
        pub fixture_id: String,
        pub reference_transcript: String,
        pub hypothesis_transcript: String,
        pub wer: f64,
        pub edits: EditCounts,
        /// Number of distinct ASR output segments — the turn-fragmentation
        /// proxy. A single utterance ideally yields one segment; a fragmented
        /// transcript splits it into many.
        pub asr_segment_count: usize,
        /// `asr_segment_count` minus the manifest's expected speaker-turn count.
        /// Positive ⇒ the ASR over-fragmented relative to the reference turns.
        pub turn_fragmentation_delta: i64,
    }

    /// Count distinct speaker turns in a fixture: a turn boundary occurs whenever
    /// consecutive (by start time) segments switch speaker.
    fn expected_turn_count(fixture: &Fixture) -> usize {
        let mut segs: Vec<&super::Segment> = fixture.segments.iter().collect();
        segs.sort_by_key(|s| s.start_ms);
        let mut turns = 0usize;
        let mut last: Option<&str> = None;
        for seg in segs {
            if last != Some(seg.speaker_id.as_str()) {
                turns += 1;
                last = Some(seg.speaker_id.as_str());
            }
        }
        turns
    }

    /// Score a mono-ASR hypothesis (and its segment count) against a fixture.
    pub fn score_mono_asr(
        fixture: &Fixture,
        hypothesis: &str,
        asr_segment_count: usize,
    ) -> MonoAsrResult {
        let reference = fixture.baseline.mono_asr.reference_transcript.clone();
        let (wer, edits) = compute_wer(&reference, hypothesis);
        let expected_turns = expected_turn_count(fixture);
        MonoAsrResult {
            fixture_id: fixture.id.clone(),
            reference_transcript: reference,
            hypothesis_transcript: hypothesis.to_string(),
            wer,
            edits,
            asr_segment_count,
            turn_fragmentation_delta: asr_segment_count as i64 - expected_turns as i64,
        }
    }

    // ── Diarization baseline result ────────────────────────────────────────

    /// One detected diarization segment in milliseconds (decoupled from the
    /// feature-gated `ClusterSegment` so the math is testable feature-free).
    #[derive(Debug, Clone, Copy)]
    pub struct DetectedSegment {
        pub start_ms: u64,
        pub end_ms: u64,
        pub speaker: i32,
    }

    /// Scored diarization baseline for one fixture: detected vs expected speaker
    /// count, overlap-region accuracy (recall of the manifest's expected overlap
    /// spans), and churn (segment count + speaker-switch count).
    #[derive(Debug, Clone, Serialize)]
    pub struct DiarizationResult {
        pub fixture_id: String,
        pub expected_speaker_count: u32,
        pub detected_speaker_count: u32,
        pub speaker_count_correct: bool,
        /// Fraction of the expected overlap-region duration that the diarizer
        /// actually covered with >=2 concurrent speakers. 1.0 = perfect recall;
        /// for turn-taking fixtures (no expected overlap) this is 1.0 iff the
        /// diarizer reported no spurious overlap.
        pub overlap_region_accuracy: f64,
        /// Total detected overlap duration (ms) the diarizer reported OUTSIDE
        /// any expected region — spurious concurrency, i.e. overlap "churn".
        pub spurious_overlap_ms: u64,
        /// Number of detected segments — raw churn proxy.
        pub detected_segment_count: usize,
        /// Number of speaker switches between time-ordered detected segments.
        pub speaker_switch_count: usize,
    }

    /// Build the half-open millisecond timeline of how many DISTINCT speakers are
    /// active, as a list of `(start, end, distinct_speaker_count)` spans. Used to
    /// measure overlap (>=2) regions.
    fn concurrency_spans(segments: &[DetectedSegment]) -> Vec<(u64, u64, usize)> {
        // Collect all boundary points.
        let mut points: BTreeSet<u64> = BTreeSet::new();
        for s in segments {
            points.insert(s.start_ms);
            points.insert(s.end_ms);
        }
        let points: Vec<u64> = points.into_iter().collect();
        let mut spans = Vec::new();
        for window in points.windows(2) {
            let (a, b) = (window[0], window[1]);
            if a >= b {
                continue;
            }
            let mid = a + (b - a) / 2;
            let speakers: BTreeSet<i32> = segments
                .iter()
                .filter(|s| s.start_ms <= mid && mid < s.end_ms)
                .map(|s| s.speaker)
                .collect();
            spans.push((a, b, speakers.len()));
        }
        spans
    }

    /// Total ms of `spans` (>=2 speakers) that intersect `[start, end)`.
    fn overlap_ms_within(spans: &[(u64, u64, usize)], start: u64, end: u64) -> u64 {
        spans
            .iter()
            .filter(|(_, _, n)| *n >= 2)
            .map(|(a, b, _)| {
                let lo = (*a).max(start);
                let hi = (*b).min(end);
                hi.saturating_sub(lo)
            })
            .sum()
    }

    /// Score detected diarization segments against a fixture's expectations.
    pub fn score_diarization(fixture: &Fixture, detected: &[DetectedSegment]) -> DiarizationResult {
        let detected_speakers: BTreeSet<i32> = detected.iter().map(|s| s.speaker).collect();
        let detected_speaker_count = detected_speakers.len() as u32;
        let expected_speaker_count = fixture.expected_speaker_count;

        let spans = concurrency_spans(detected);
        let expected: &[ExpectedOverlapRegion] =
            &fixture.baseline.diarization.expected_overlap_regions;

        let overlap_region_accuracy = if expected.is_empty() {
            // Turn-taking: perfect iff the diarizer reported NO overlap at all.
            let total_overlap: u64 = spans
                .iter()
                .filter(|(_, _, n)| *n >= 2)
                .map(|(a, b, _)| b - a)
                .sum();
            if total_overlap == 0 {
                1.0
            } else {
                0.0
            }
        } else {
            let mut expected_total = 0u64;
            let mut covered_total = 0u64;
            for region in expected {
                expected_total += region.end_ms - region.start_ms;
                covered_total += overlap_ms_within(&spans, region.start_ms, region.end_ms);
            }
            if expected_total == 0 {
                1.0
            } else {
                covered_total as f64 / expected_total as f64
            }
        };

        // Spurious overlap = total detected overlap ms NOT inside any expected
        // region.
        let total_detected_overlap: u64 = spans
            .iter()
            .filter(|(_, _, n)| *n >= 2)
            .map(|(a, b, _)| b - a)
            .sum();
        let inside_expected: u64 = expected
            .iter()
            .map(|r| overlap_ms_within(&spans, r.start_ms, r.end_ms))
            .sum();
        let spurious_overlap_ms = total_detected_overlap.saturating_sub(inside_expected);

        // Speaker switches between time-ordered detected segments.
        let mut ordered: Vec<&DetectedSegment> = detected.iter().collect();
        ordered.sort_by_key(|s| s.start_ms);
        let mut switches = 0usize;
        let mut last: Option<i32> = None;
        for seg in &ordered {
            if last != Some(seg.speaker) {
                if last.is_some() {
                    switches += 1;
                }
                last = Some(seg.speaker);
            }
        }

        DiarizationResult {
            fixture_id: fixture.id.clone(),
            expected_speaker_count,
            detected_speaker_count,
            speaker_count_correct: detected_speaker_count == expected_speaker_count,
            overlap_region_accuracy,
            spurious_overlap_ms,
            detected_segment_count: detected.len(),
            speaker_switch_count: switches,
        }
    }

    // ── Combined per-fixture + run report (serialized to baseline_results.json)

    #[derive(Debug, Clone, Serialize)]
    pub struct FixtureBaseline {
        pub fixture_id: String,
        pub kind: String,
        pub mono_asr: Option<MonoAsrResult>,
        pub diarization: Option<DiarizationResult>,
    }

    #[derive(Debug, Clone, Serialize)]
    pub struct BaselineRunReport {
        pub manifest_id: String,
        /// "pending_real_run" until a real ASR + diarization backend produced the
        /// numbers below; mirrors the manifest's per-fixture baseline status.
        pub mono_asr_status: String,
        pub diarization_status: String,
        pub fixtures: Vec<FixtureBaseline>,
        /// Human-readable note explaining why a status is still pending (no
        /// model / feature off / no env vars), for the results file.
        pub note: String,
    }

    fn results_path() -> PathBuf {
        fixture_root().join("baseline_results.json")
    }

    /// Write the run report next to the manifest. Returns the path written.
    fn write_report(report: &BaselineRunReport) -> PathBuf {
        let path = results_path();
        let body = serde_json::to_string_pretty(report).expect("serialize baseline report");
        fs::write(&path, body)
            .unwrap_or_else(|error| panic!("failed to write {}: {error}", path.display()));
        path
    }

    fn kind_str(kind: &FixtureKind) -> &'static str {
        match kind {
            FixtureKind::Overlap => "overlap",
            FixtureKind::TurnTaking => "turn_taking",
        }
    }

    // ── Real-backend drivers (env-gated; `#[ignore]`d so CI never runs them) ──

    /// Run mono-ASR over a fixture WAV with a real whisper model, returning the
    /// concatenated hypothesis + the ASR segment count. Only compiled with the
    /// `asr-whisper` feature.
    #[cfg(feature = "asr-whisper")]
    fn run_mono_asr_real(model_path: &Path, wav_path: &Path) -> Result<(String, usize), String> {
        use crate::asr::{AsrConfig, AsrWorker, SpeechSegment};
        use std::time::Duration;
        use whisper_rs::{WhisperContext, WhisperContextParameters};

        let samples = load_wav_samples_16k_mono(wav_path);
        let config = AsrConfig {
            model_path: model_path.to_path_buf(),
            language: "en".to_string(),
            n_threads: 4,
            temperature: 0.0,
            beam_size: 1,
        };
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut worker = AsrWorker::new(config, tx);

        let ctx = WhisperContext::new_with_params(
            &model_path.display().to_string(),
            WhisperContextParameters::default(),
        )
        .map_err(|e| format!("whisper context: {e}"))?;
        let mut state = ctx
            .create_state()
            .map_err(|e| format!("whisper state: {e}"))?;

        let segment = SpeechSegment {
            source_id: "fixture".to_string(),
            num_frames: samples.len(),
            audio: samples,
            start_time: Duration::ZERO,
            end_time: Duration::ZERO,
        };
        let transcripts = worker.transcribe_segment(&mut state, &segment)?;
        let count = transcripts.len();
        let hypothesis = transcripts
            .iter()
            .map(|t| t.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        Ok((hypothesis, count))
    }

    /// Run clustering diarization over a fixture WAV with real sherpa-onnx
    /// models. Only compiled with the `diarization-clustering` feature.
    #[cfg(feature = "diarization-clustering")]
    fn run_diarization_real(
        seg_model: &Path,
        emb_model: &Path,
        wav_path: &Path,
    ) -> Result<Vec<DetectedSegment>, String> {
        use crate::diarization::clustering::{ClusteringDiarizer, DEFAULT_CLUSTERING_THRESHOLD};
        let samples = load_wav_samples_16k_mono(wav_path);
        let diarizer = ClusteringDiarizer::new(seg_model, emb_model, DEFAULT_CLUSTERING_THRESHOLD)?;
        let segments = diarizer.diarize(&samples)?;
        Ok(segments
            .into_iter()
            .map(|s| DetectedSegment {
                start_ms: (s.start * 1000.0).round() as u64,
                end_ms: (s.end * 1000.0).round() as u64,
                speaker: s.speaker,
            })
            .collect())
    }

    // =======================================================================
    // Tests
    // =======================================================================
    #[cfg(test)]
    mod tests {
        use super::*;

        // ── Offline metric unit tests (no models, no network) ──────────────

        #[test]
        fn wer_is_zero_for_identical_transcripts() {
            let (wer, edits) = compute_wer("the quick brown fox", "The  QUICK, brown fox.");
            assert_eq!(
                wer, 0.0,
                "normalization should erase case/punctuation diffs"
            );
            assert_eq!(edits.total_errors(), 0);
            assert_eq!(edits.reference_words, 4);
        }

        #[test]
        fn wer_counts_substitution_deletion_insertion() {
            // ref: a b c d   hyp: a x c d e
            //  - b->x substitution, e insertion => 2 errors over 4 ref words.
            let (wer, edits) = compute_wer("a b c d", "a x c d e");
            assert_eq!(edits.substitutions, 1, "{edits:?}");
            assert_eq!(edits.insertions, 1, "{edits:?}");
            assert_eq!(edits.deletions, 0, "{edits:?}");
            assert!((wer - 0.5).abs() < 1e-9, "wer={wer}");
        }

        #[test]
        fn wer_pure_deletion() {
            // ref has 3 words, hyp drops the middle one => 1 deletion.
            let (wer, edits) = compute_wer("one two three", "one three");
            assert_eq!(edits.deletions, 1, "{edits:?}");
            assert_eq!(edits.substitutions, 0);
            assert_eq!(edits.insertions, 0);
            assert!((wer - (1.0 / 3.0)).abs() < 1e-9, "wer={wer}");
        }

        #[test]
        fn wer_empty_reference_empty_hypothesis_is_zero() {
            let (wer, _) = compute_wer("", "");
            assert_eq!(wer, 0.0);
        }

        #[test]
        fn concurrency_spans_detect_overlap_region() {
            // Speaker 0: [0, 3000), speaker 1: [800, 3800) -> overlap [800,3000).
            let segs = vec![
                DetectedSegment {
                    start_ms: 0,
                    end_ms: 3000,
                    speaker: 0,
                },
                DetectedSegment {
                    start_ms: 800,
                    end_ms: 3800,
                    speaker: 1,
                },
            ];
            let spans = concurrency_spans(&segs);
            let overlap = overlap_ms_within(&spans, 0, 3800);
            assert_eq!(overlap, 2200, "overlap should be [800,3000) = 2200ms");
        }

        #[test]
        fn diarization_scores_overlap_fixture_perfectly_when_segments_match() {
            let manifest = load_manifest();
            let overlap = manifest
                .fixtures
                .iter()
                .find(|f| f.kind == FixtureKind::Overlap)
                .expect("overlap fixture");
            // Hand the scorer the manifest's own ground-truth segments — the
            // scorer must recover speaker count 2 and 100% overlap recall.
            let detected: Vec<DetectedSegment> = overlap
                .segments
                .iter()
                .enumerate()
                .map(|(i, s)| DetectedSegment {
                    start_ms: s.start_ms,
                    end_ms: s.end_ms,
                    speaker: i as i32,
                })
                .collect();
            let result = score_diarization(overlap, &detected);
            assert_eq!(result.detected_speaker_count, 2);
            assert!(result.speaker_count_correct);
            assert!(
                (result.overlap_region_accuracy - 1.0).abs() < 1e-9,
                "expected full overlap recall, got {}",
                result.overlap_region_accuracy
            );
            assert_eq!(result.spurious_overlap_ms, 0);
        }

        #[test]
        fn diarization_turn_taking_perfect_when_no_overlap_reported() {
            let manifest = load_manifest();
            let turn = manifest
                .fixtures
                .iter()
                .find(|f| f.kind == FixtureKind::TurnTaking)
                .expect("turn-taking fixture");
            let detected: Vec<DetectedSegment> = turn
                .segments
                .iter()
                .enumerate()
                .map(|(i, s)| DetectedSegment {
                    start_ms: s.start_ms,
                    end_ms: s.end_ms,
                    speaker: i as i32,
                })
                .collect();
            let result = score_diarization(turn, &detected);
            assert!(result.speaker_count_correct);
            assert!(
                (result.overlap_region_accuracy - 1.0).abs() < 1e-9,
                "turn-taking with no overlap should score 1.0"
            );
            assert_eq!(result.spurious_overlap_ms, 0);
            assert_eq!(result.speaker_switch_count, 1, "A then B = one switch");
        }

        #[test]
        fn diarization_penalizes_spurious_overlap_in_turn_taking() {
            let manifest = load_manifest();
            let turn = manifest
                .fixtures
                .iter()
                .find(|f| f.kind == FixtureKind::TurnTaking)
                .expect("turn-taking fixture");
            // Diarizer wrongly makes the two speakers overlap.
            let detected = vec![
                DetectedSegment {
                    start_ms: 0,
                    end_ms: 4000,
                    speaker: 0,
                },
                DetectedSegment {
                    start_ms: 3000,
                    end_ms: 6400,
                    speaker: 1,
                },
            ];
            let result = score_diarization(turn, &detected);
            assert_eq!(
                result.overlap_region_accuracy, 0.0,
                "spurious overlap in a turn-taking clip must score 0.0"
            );
            assert!(result.spurious_overlap_ms > 0);
        }

        #[test]
        fn mono_asr_scoring_reports_wer_and_fragmentation() {
            let manifest = load_manifest();
            let overlap = manifest
                .fixtures
                .iter()
                .find(|f| f.kind == FixtureKind::Overlap)
                .expect("overlap fixture");
            // A perfect hypothesis but split into 3 ASR segments (over-fragmented
            // vs the 2 reference turns).
            let reference = overlap.baseline.mono_asr.reference_transcript.clone();
            let result = score_mono_asr(overlap, &reference, 3);
            assert_eq!(result.wer, 0.0, "identical transcript => 0 WER");
            assert_eq!(result.asr_segment_count, 3);
            assert_eq!(
                result.turn_fragmentation_delta, 1,
                "3 ASR segments vs 2 expected turns => +1 fragmentation"
            );
        }

        #[test]
        fn fixture_wavs_decode_to_expected_sample_counts() {
            let manifest = load_manifest();
            for fixture in &manifest.fixtures {
                let path = fixture_root().join(&fixture.audio_path);
                let samples = load_wav_samples_16k_mono(&path);
                // 16 kHz mono => duration_ms * 16 samples/ms (within one frame).
                let expected = (fixture.duration_ms * 16) as usize;
                assert!(
                    samples.len().abs_diff(expected) <= 16,
                    "{} decoded {} samples, expected ~{expected}",
                    fixture.audio_path,
                    samples.len()
                );
                // Real audio, not silence.
                assert!(
                    samples.iter().any(|s| s.abs() > 1e-4),
                    "{} appears to be silent",
                    fixture.audio_path
                );
            }
        }

        /// Offline driver: with no real ASR/diarization backend wired (the CI
        /// gate uses `--features cloud`), the runner records a report whose
        /// statuses remain `pending_real_run` and writes it next to the
        /// manifest. This is the always-on, network-free integration test that
        /// proves the runner plumbing end-to-end.
        #[test]
        fn baseline_run_records_pending_report_offline() {
            let manifest = load_manifest();

            let asr_model = std::env::var("AG_SS_ASR_MODEL").ok().map(PathBuf::from);
            let seg_model = std::env::var("AG_DIAR_SEG_MODEL").ok().map(PathBuf::from);
            let emb_model = std::env::var("AG_DIAR_EMB_MODEL").ok().map(PathBuf::from);

            let mut fixtures = Vec::new();
            let mut any_asr = false;
            let mut any_diar = false;

            for fixture in &manifest.fixtures {
                let wav = fixture_root().join(&fixture.audio_path);

                // Mono-ASR: only when a real whisper model + feature are present.
                let mono_asr = {
                    #[cfg(feature = "asr-whisper")]
                    {
                        if let Some(model) = &asr_model {
                            match run_mono_asr_real(model, &wav) {
                                Ok((hyp, count)) => {
                                    any_asr = true;
                                    Some(score_mono_asr(fixture, &hyp, count))
                                }
                                Err(e) => {
                                    eprintln!("mono-ASR baseline skipped ({}): {e}", fixture.id);
                                    None
                                }
                            }
                        } else {
                            None
                        }
                    }
                    #[cfg(not(feature = "asr-whisper"))]
                    {
                        let _ = &asr_model;
                        None
                    }
                };

                // Diarization: only with real models + the clustering feature.
                let diarization = {
                    #[cfg(feature = "diarization-clustering")]
                    {
                        match (&seg_model, &emb_model) {
                            (Some(seg), Some(emb)) => match run_diarization_real(seg, emb, &wav) {
                                Ok(detected) => {
                                    any_diar = true;
                                    Some(score_diarization(fixture, &detected))
                                }
                                Err(e) => {
                                    eprintln!("diarization baseline skipped ({}): {e}", fixture.id);
                                    None
                                }
                            },
                            _ => None,
                        }
                    }
                    #[cfg(not(feature = "diarization-clustering"))]
                    {
                        let _ = (&seg_model, &emb_model);
                        None
                    }
                };

                fixtures.push(FixtureBaseline {
                    fixture_id: fixture.id.clone(),
                    kind: kind_str(&fixture.kind).to_string(),
                    mono_asr,
                    diarization,
                });
            }

            let mono_asr_status = if any_asr {
                "complete"
            } else {
                "pending_real_run"
            };
            let diarization_status = if any_diar {
                "complete"
            } else {
                "pending_real_run"
            };
            let report = BaselineRunReport {
                manifest_id: manifest.id.clone(),
                mono_asr_status: mono_asr_status.to_string(),
                diarization_status: diarization_status.to_string(),
                fixtures,
                note: "Offline CI build lacks the asr-whisper / diarization-clustering \
                       features and the AG_SS_ASR_MODEL / AG_DIAR_* model paths, so the \
                       mono-ASR and diarization baselines remain pending_real_run. Run with \
                       those features + env vars to record real numbers."
                    .to_string(),
            };

            let path = write_report(&report);
            assert!(path.exists(), "report should have been written");

            // In the offline CI gate the runner must NOT have flipped either
            // status — the manifest still asserts pending_real_run.
            #[cfg(not(feature = "asr-whisper"))]
            assert_eq!(report.mono_asr_status, "pending_real_run");
            #[cfg(not(feature = "diarization-clustering"))]
            assert_eq!(report.diarization_status, "pending_real_run");
        }

        // ── Real-backend tests (env-gated, `#[ignore]`d so CI never runs) ──

        #[test]
        #[ignore = "requires AG_SS_ASR_MODEL + the asr-whisper feature"]
        #[cfg(feature = "asr-whisper")]
        fn real_mono_asr_baseline_scores_fixtures() {
            let model = match std::env::var("AG_SS_ASR_MODEL") {
                Ok(p) => PathBuf::from(p),
                Err(_) => {
                    eprintln!("set AG_SS_ASR_MODEL to a whisper GGML model to run");
                    return;
                }
            };
            let manifest = load_manifest();
            for fixture in &manifest.fixtures {
                let wav = fixture_root().join(&fixture.audio_path);
                let (hyp, count) =
                    run_mono_asr_real(&model, &wav).expect("real mono-ASR should succeed");
                let result = score_mono_asr(fixture, &hyp, count);
                eprintln!(
                    "[{}] WER={:.3} segments={} frag_delta={} hyp={:?}",
                    result.fixture_id,
                    result.wer,
                    result.asr_segment_count,
                    result.turn_fragmentation_delta,
                    result.hypothesis_transcript
                );
                assert!(result.wer <= 1.0);
            }
        }

        #[test]
        #[ignore = "requires AG_DIAR_SEG_MODEL + AG_DIAR_EMB_MODEL + the diarization-clustering feature"]
        #[cfg(feature = "diarization-clustering")]
        fn real_diarization_baseline_scores_fixtures() {
            let (seg, emb) = match (
                std::env::var("AG_DIAR_SEG_MODEL"),
                std::env::var("AG_DIAR_EMB_MODEL"),
            ) {
                (Ok(s), Ok(e)) => (PathBuf::from(s), PathBuf::from(e)),
                _ => {
                    eprintln!("set AG_DIAR_SEG_MODEL + AG_DIAR_EMB_MODEL to run");
                    return;
                }
            };
            let manifest = load_manifest();
            for fixture in &manifest.fixtures {
                let wav = fixture_root().join(&fixture.audio_path);
                let detected = run_diarization_real(&seg, &emb, &wav)
                    .expect("real diarization should succeed");
                let result = score_diarization(fixture, &detected);
                eprintln!(
                    "[{}] speakers={}/{} overlap_acc={:.3} spurious_ms={} switches={}",
                    result.fixture_id,
                    result.detected_speaker_count,
                    result.expected_speaker_count,
                    result.overlap_region_accuracy,
                    result.spurious_overlap_ms,
                    result.speaker_switch_count
                );
            }
        }
    }
}
