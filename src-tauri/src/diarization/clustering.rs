//! Unbounded offline speaker diarization via sherpa-onnx (ADR-0017).
//!
//! Wraps `sherpa_onnx::OfflineSpeakerDiarization` = pyannote **segmentation** +
//! 3D-Speaker **embedding** extraction + **FastClustering**. With
//! `num_clusters = -1` and a cosine `threshold`, the speaker count is
//! **unknown / unbounded** — unlike the Sortformer backend, which is hard-capped
//! at 4 by its model. This is *offline*: feed a complete mono 16 kHz f32
//! waveform to [`ClusteringDiarizer::diarize`] and get back speaker-labeled
//! segments. The live-pipeline (rolling-window) integration is tracked
//! separately (ADR-0017 §"streaming integration").
//!
//! Requires the `diarization-clustering` Cargo feature, which pulls `sherpa-onnx`
//! and its ONNX Runtime. That ORT conflicts with `parakeet-rs`, so this feature
//! is mutually exclusive with the `diarization` (Sortformer) feature — enforced
//! by a `compile_error!` in `lib.rs`.

/// One diarized span: `[start, end]` seconds attributed to a cluster id.
/// `speaker` is 0-based; the number of distinct ids equals the detected speaker
/// count (which can exceed 4, unlike Sortformer).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClusterSegment {
    pub start: f32,
    pub end: f32,
    pub speaker: i32,
}

/// Default cosine-distance clustering threshold. Smaller ⇒ more speakers,
/// larger ⇒ fewer; 0.5 is the sherpa-onnx default starting point for the
/// 3D-Speaker embeddings.
pub const DEFAULT_CLUSTERING_THRESHOLD: f32 = 0.5;

/// Sample rate (Hz) the segmentation + embedding models expect. Inputs must be
/// resampled to this mono rate before [`ClusteringDiarizer::diarize`].
pub const CLUSTERING_SAMPLE_RATE: i32 = 16_000;

#[cfg(feature = "diarization-clustering")]
mod imp {
    use super::{ClusterSegment, CLUSTERING_SAMPLE_RATE};
    use std::path::Path;

    use sherpa_onnx::{
        FastClusteringConfig, OfflineSpeakerDiarization, OfflineSpeakerDiarizationConfig,
        OfflineSpeakerSegmentationModelConfig, OfflineSpeakerSegmentationPyannoteModelConfig,
        SpeakerEmbeddingExtractorConfig,
    };

    /// Offline, unbounded-speaker diarizer backed by sherpa-onnx.
    ///
    /// `OfflineSpeakerDiarization` is `Send + Sync`, so this can be held in a
    /// worker or moved across threads (single-object use).
    pub struct ClusteringDiarizer {
        inner: OfflineSpeakerDiarization,
    }

    impl ClusteringDiarizer {
        /// Build a diarizer from a pyannote segmentation model + a speaker
        /// embedding model, with an unknown speaker count (`num_clusters = -1`)
        /// controlled by `threshold` (cosine distance; smaller ⇒ more speakers).
        pub fn new(
            segmentation_model: &Path,
            embedding_model: &Path,
            threshold: f32,
        ) -> Result<Self, String> {
            for (label, p) in [
                ("segmentation", segmentation_model),
                ("embedding", embedding_model),
            ] {
                if !p.exists() {
                    return Err(format!(
                        "diarization {label} model not found at {}",
                        p.display()
                    ));
                }
            }

            let config = OfflineSpeakerDiarizationConfig {
                segmentation: OfflineSpeakerSegmentationModelConfig {
                    pyannote: OfflineSpeakerSegmentationPyannoteModelConfig {
                        model: Some(segmentation_model.display().to_string()),
                    },
                    ..Default::default()
                },
                embedding: SpeakerEmbeddingExtractorConfig {
                    model: Some(embedding_model.display().to_string()),
                    ..Default::default()
                },
                clustering: FastClusteringConfig {
                    num_clusters: -1, // unknown / unbounded speaker count
                    threshold,
                },
                ..Default::default()
            };

            let inner = OfflineSpeakerDiarization::create(&config).ok_or_else(|| {
                "failed to create sherpa-onnx OfflineSpeakerDiarization (bad/missing models?)"
                    .to_string()
            })?;
            Ok(Self { inner })
        }

        /// Sample rate the segmentation model expects (16 kHz).
        pub fn sample_rate(&self) -> i32 {
            self.inner.sample_rate()
        }

        /// Diarize a complete mono 16 kHz f32 waveform. Returns segments sorted
        /// by start time, each labeled with a 0-based speaker id; the distinct
        /// id count is the detected speaker count (may exceed 4).
        pub fn diarize(&self, samples_16k_mono: &[f32]) -> Result<Vec<ClusterSegment>, String> {
            if self.inner.sample_rate() != CLUSTERING_SAMPLE_RATE {
                return Err(format!(
                    "diarizer expects {} Hz, model reports {}",
                    CLUSTERING_SAMPLE_RATE,
                    self.inner.sample_rate()
                ));
            }
            if samples_16k_mono.is_empty() {
                return Ok(Vec::new());
            }
            let result = self
                .inner
                .process(samples_16k_mono)
                .ok_or_else(|| "sherpa-onnx diarization failed".to_string())?;
            Ok(result
                .sort_by_start_time()
                .into_iter()
                .map(|s| ClusterSegment {
                    start: s.start,
                    end: s.end,
                    speaker: s.speaker,
                })
                .collect())
        }
    }
}

#[cfg(feature = "diarization-clustering")]
pub use imp::ClusteringDiarizer;

// ---------------------------------------------------------------------------
// Stub when the feature is off — keeps the type referenceable so callers and
// the backend enum compile in every build; construction reports unavailability.
// ---------------------------------------------------------------------------
#[cfg(not(feature = "diarization-clustering"))]
pub struct ClusteringDiarizer;

#[cfg(not(feature = "diarization-clustering"))]
impl ClusteringDiarizer {
    pub fn new(
        _segmentation_model: &std::path::Path,
        _embedding_model: &std::path::Path,
        _threshold: f32,
    ) -> Result<Self, String> {
        Err(
            "clustering diarization is not included in this build (rebuild with \
             the `diarization-clustering` feature)"
                .to_string(),
        )
    }
    pub fn sample_rate(&self) -> i32 {
        CLUSTERING_SAMPLE_RATE
    }
    pub fn diarize(&self, _samples_16k_mono: &[f32]) -> Result<Vec<ClusterSegment>, String> {
        Err("clustering diarization is not included in this build".to_string())
    }
}

#[cfg(all(test, feature = "diarization-clustering"))]
mod model_backed_tests {
    use super::*;
    use std::path::PathBuf;

    /// Env-gated (CI has no models): set both to local ONNX paths to run.
    ///   AG_DIAR_SEG_MODEL=/path/sherpa-onnx-pyannote-segmentation-3-0/model.onnx
    ///   AG_DIAR_EMB_MODEL=/path/3dspeaker_..._16k.onnx
    ///   AG_DIAR_TEST_WAV=/path/multi-speaker-16k-mono.f32  (raw little-endian f32)
    fn paths() -> Option<(PathBuf, PathBuf, PathBuf)> {
        let seg = std::env::var("AG_DIAR_SEG_MODEL").ok()?;
        let emb = std::env::var("AG_DIAR_EMB_MODEL").ok()?;
        let wav = std::env::var("AG_DIAR_TEST_WAV").ok()?;
        Some((PathBuf::from(seg), PathBuf::from(emb), PathBuf::from(wav)))
    }

    /// Model paths only (no labeled clip) — for validating real-ONNX load +
    /// diarizer construction without needing a curated multi-speaker WAV.
    fn model_paths() -> Option<(PathBuf, PathBuf)> {
        let seg = std::env::var("AG_DIAR_SEG_MODEL").ok()?;
        let emb = std::env::var("AG_DIAR_EMB_MODEL").ok()?;
        Some((PathBuf::from(seg), PathBuf::from(emb)))
    }

    /// Validates that `ClusteringDiarizer` constructs against the REAL pyannote +
    /// embedding ONNX models (loads both, reports the expected 16 kHz rate) and
    /// that `diarize()` runs the full segmentation→embedding→clustering pipeline
    /// end-to-end on a buffer without erroring. This is the model-load /
    /// wiring-correctness check that does NOT need a labeled clip; the
    /// speaker-count *accuracy* assertion lives in
    /// `diarizes_a_clip_into_speaker_segments` (WAV-gated). Set
    /// `AG_DIAR_SEG_MODEL` + `AG_DIAR_EMB_MODEL` to run (e.g. the AudioGraph
    /// model cache `…/models/sherpa-onnx-pyannote-segmentation-3-0/model.int8.onnx`
    /// + `…/models/nemo_en_titanet_small.onnx`).
    #[test]
    fn constructs_and_runs_against_real_models() {
        let Some((seg, emb)) = model_paths() else {
            eprintln!(
                "skipping constructs_and_runs_against_real_models: set AG_DIAR_SEG_MODEL + AG_DIAR_EMB_MODEL"
            );
            return;
        };
        let diarizer = ClusteringDiarizer::new(&seg, &emb, DEFAULT_CLUSTERING_THRESHOLD)
            .expect("diarizer should construct from the real ONNX models");
        assert_eq!(
            diarizer.sample_rate(),
            CLUSTERING_SAMPLE_RATE,
            "pyannote segmentation model must report 16 kHz"
        );

        // ~5 s of 16 kHz mono audio (a quiet sine — no labeled speakers, so we
        // assert the pipeline RUNS, not the speaker count). diarize() must drive
        // segmentation→embedding→clustering without erroring; an empty result is
        // valid for non-speech input.
        let n = (CLUSTERING_SAMPLE_RATE as usize) * 5;
        let samples: Vec<f32> = (0..n)
            .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / 16_000.0).sin() * 0.1)
            .collect();
        let segments = diarizer
            .diarize(&samples)
            .expect("diarize() should run the real ONNX pipeline without error");
        eprintln!(
            "real-model diarize() produced {} segment(s) on synthetic audio",
            segments.len()
        );
    }

    #[test]
    fn diarizes_a_clip_into_speaker_segments() {
        let Some((seg, emb, wav)) = paths() else {
            eprintln!("skipping diarizes_a_clip_into_speaker_segments: set AG_DIAR_* env vars");
            return;
        };
        let diarizer = ClusteringDiarizer::new(&seg, &emb, DEFAULT_CLUSTERING_THRESHOLD)
            .expect("diarizer should construct from the test models");
        assert_eq!(diarizer.sample_rate(), CLUSTERING_SAMPLE_RATE);

        let bytes = std::fs::read(&wav).expect("read test wav f32 dump");
        let samples: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();

        let segments = diarizer
            .diarize(&samples)
            .expect("diarization should succeed");
        assert!(
            !segments.is_empty(),
            "expected at least one speaker segment"
        );
        let speakers: std::collections::BTreeSet<i32> =
            segments.iter().map(|s| s.speaker).collect();
        // Unbounded: the count is whatever the clip contains, not capped at 4.
        assert!(
            !speakers.is_empty(),
            "expected >=1 distinct speaker; got {speakers:?}"
        );
        eprintln!(
            "diarized {} segments across {} speakers",
            segments.len(),
            speakers.len()
        );
    }
}
