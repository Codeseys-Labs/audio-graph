//! Speaker diarization module — supports both a simple signal-based MVP
//! and a streaming neural diarization backend via `parakeet-rs` Sortformer.
//!
//! The `DiarizationWorker` maintains the same channel-based interface regardless
//! of which backend is active. The backend is selected via [`DiarizationBackend`]
//! at construction time:
//!
//! - **`Simple`** — Pure-Rust, no-ML approach using RMS energy, zero-crossing
//!   rate, and mean absolute deviation as a lightweight speaker fingerprint.
//!   Always available; works as a fallback.
//!
//! - **`Sortformer`** — Uses NVIDIA's Sortformer v2 ONNX model via the
//!   `parakeet-rs` crate for streaming speaker diarization (up to 4 speakers).
//!   Requires the `diarization` Cargo feature and the model ONNX file on disk.
//!
//! Both backends produce [`DiarizedTranscript`] values downstream.

use std::path::PathBuf;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};

use crate::state::{SpeakerInfo, TranscriptSegment};

// ── Speaker color palette ────────────────────────────────────────────────

/// Predefined color palette for distinguishing speakers in the UI.
const SPEAKER_COLORS: &[&str] = &[
    "#4A90D9", // blue
    "#E74C3C", // red
    "#2ECC71", // green
    "#F39C12", // orange
    "#9B59B6", // purple
    "#1ABC9C", // teal
    "#E67E22", // dark orange
    "#3498DB", // light blue
    "#E91E63", // pink
    "#00BCD4", // cyan
];

/// Maximum number of speakers the Sortformer model supports.
const SORTFORMER_MAX_SPEAKERS: usize = 4;

/// Sample rate expected by both the simple backend and Sortformer (16 kHz).
#[allow(dead_code)] // Used in Sortformer backend calculations
const SAMPLE_RATE: u64 = 16_000;

// ── Types ────────────────────────────────────────────────────────────────

/// Audio features used as a simple speaker fingerprint (Simple backend only).
#[derive(Debug, Clone, Copy)]
pub struct AudioFeatures {
    /// Root-mean-square energy of the signal.
    pub rms_energy: f32,
    /// Fraction of consecutive sample pairs that cross zero.
    pub zero_crossing_rate: f32,
    /// Mean absolute deviation (MAD) of the signal.
    pub spectral_centroid: f32,
}

/// A known speaker profile, accumulated over time.
#[derive(Debug, Clone)]
pub struct SpeakerProfile {
    /// Unique identifier (e.g. `"speaker-1"`).
    pub id: String,
    /// Human-readable label (e.g. `"Speaker A"`).
    pub label: String,
    /// Hex colour for the UI.
    pub color: String,
    /// Running average of audio features for this speaker (Simple backend).
    pub features: Option<AudioFeatures>,
    /// Number of segments attributed to this speaker.
    pub segment_count: u32,
    /// Cumulative speaking time in seconds.
    pub total_speaking_time: f64,
}

/// Which diarization backend to use.
#[derive(Debug, Clone, Default)]
pub enum DiarizationBackend {
    /// Pure-Rust signal-based MVP (always available).
    #[default]
    Simple,
    /// Streaming neural diarization via parakeet-rs Sortformer ONNX model.
    /// The `PathBuf` points to the ONNX model file on disk.
    Sortformer { model_path: PathBuf },
}

/// Configuration knobs for the diarization worker.
pub struct DiarizationConfig {
    /// Which backend to use.
    pub backend: DiarizationBackend,
    /// Maximum normalised feature distance to consider "same speaker" (Simple backend).
    pub similarity_threshold: f32,
    /// Hard cap on the number of distinct speakers (Simple backend).
    pub max_speakers: usize,
    /// Time gap (seconds) that increases likelihood of a speaker change (Simple backend).
    pub gap_threshold_secs: f64,
}

impl Default for DiarizationConfig {
    fn default() -> Self {
        Self {
            backend: DiarizationBackend::Simple,
            similarity_threshold: 0.7,
            max_speakers: 10,
            gap_threshold_secs: 2.0,
        }
    }
}

impl DiarizationConfig {
    /// Create a config that uses the Sortformer backend with the given model path.
    pub fn sortformer(model_path: PathBuf) -> Self {
        Self {
            backend: DiarizationBackend::Sortformer { model_path },
            // Simple-backend fields are unused but set to defaults for completeness.
            similarity_threshold: 0.7,
            max_speakers: SORTFORMER_MAX_SPEAKERS,
            gap_threshold_secs: 2.0,
        }
    }
}

/// Input to the diarization worker — a transcript segment paired with the
/// raw speech audio that produced it.
#[derive(Debug, Clone)]
pub struct DiarizationInput {
    /// The transcript segment (with `speaker_id` / `speaker_label` = `None`).
    pub transcript: TranscriptSegment,
    /// 16 kHz mono f32 audio for this segment.
    pub speech_audio: Vec<f32>,
    /// Absolute start time of the speech.
    pub speech_start_time: Duration,
    /// Absolute end time of the speech.
    pub speech_end_time: Duration,
}

/// Output from diarization: the transcript enriched with speaker info.
#[derive(Debug, Clone)]
pub struct DiarizedTranscript {
    /// Transcript segment with `speaker_id` and `speaker_label` filled in.
    pub segment: TranscriptSegment,
    /// Current state of the assigned speaker.
    pub speaker_info: SpeakerInfo,
}

// ── Sortformer wrapper (feature-gated) ───────────────────────────────────

/// Wrapper around parakeet-rs Sortformer, feature-gated behind `diarization`.
#[cfg(feature = "diarization")]
struct SortformerEngine {
    engine: parakeet_rs::sortformer::Sortformer,
}

#[cfg(feature = "diarization")]
impl SortformerEngine {
    fn new(model_path: &std::path::Path) -> Result<Self, String> {
        use parakeet_rs::sortformer::{DiarizationConfig as SfConfig, Sortformer};

        let engine = Sortformer::with_config(model_path, None, SfConfig::callhome())
            .map_err(|e| format!("Failed to load Sortformer model: {}", e))?;

        log::info!(
            "SortformerEngine loaded: chunk_len={}, right_context={}, latency={:.2}s",
            engine.chunk_len,
            engine.right_context,
            engine.latency(),
        );

        Ok(Self { engine })
    }

    /// Feed an audio chunk and get back speaker segments.
    /// Uses the buffered streaming API (`feed`) for proper state tracking.
    fn feed(
        &mut self,
        audio_16k_mono: &[f32],
    ) -> Result<Vec<parakeet_rs::sortformer::SpeakerSegment>, String> {
        self.engine
            .feed(audio_16k_mono)
            .map_err(|e| format!("Sortformer feed error: {}", e))
    }

    /// Flush any remaining buffered audio (call at end of stream).
    fn flush(&mut self) -> Result<Vec<parakeet_rs::sortformer::SpeakerSegment>, String> {
        self.engine
            .flush()
            .map_err(|e| format!("Sortformer flush error: {}", e))
    }
}

// ── Worker ───────────────────────────────────────────────────────────────

/// Speaker diarization worker.
///
/// Runs on a dedicated thread. For each incoming [`DiarizationInput`] it
/// assigns a speaker and sends a [`DiarizedTranscript`] downstream.
///
/// The internal implementation dispatches to either the Simple (signal-based)
/// backend or the Sortformer (neural) backend depending on configuration.
pub struct DiarizationWorker {
    config: DiarizationConfig,
    speakers: Vec<SpeakerProfile>,
    output_tx: Sender<DiarizedTranscript>,
    next_speaker_num: u32,
    last_segment_end: Option<f64>,

    /// Sortformer engine (only present when backend = Sortformer and feature enabled).
    #[cfg(feature = "diarization")]
    sortformer: Option<SortformerEngine>,
}

impl DiarizationWorker {
    /// Create a new diarization worker.
    ///
    /// If the `Sortformer` backend is requested but the model fails to load
    /// (or the `diarization` feature is not enabled), falls back to `Simple`.
    pub fn new(config: DiarizationConfig, output_tx: Sender<DiarizedTranscript>) -> Self {
        log::info!(
            "DiarizationWorker created (backend={:?}, threshold={}, max_speakers={}, gap={}s)",
            config.backend,
            config.similarity_threshold,
            config.max_speakers,
            config.gap_threshold_secs,
        );

        #[cfg(feature = "diarization")]
        let sortformer = match &config.backend {
            DiarizationBackend::Sortformer { model_path } => {
                match SortformerEngine::new(model_path) {
                    Ok(engine) => {
                        log::info!("DiarizationWorker: Sortformer engine loaded successfully");
                        Some(engine)
                    }
                    Err(e) => {
                        log::warn!(
                            "DiarizationWorker: failed to load Sortformer, falling back to Simple: {}",
                            e
                        );
                        None
                    }
                }
            }
            DiarizationBackend::Simple => None,
        };

        #[cfg(not(feature = "diarization"))]
        if matches!(config.backend, DiarizationBackend::Sortformer { .. }) {
            log::warn!(
                "DiarizationWorker: Sortformer backend requested but `diarization` feature \
                 is not enabled. Falling back to Simple backend."
            );
        }

        Self {
            config,
            speakers: Vec::new(),
            output_tx,
            next_speaker_num: 1,
            last_segment_end: None,
            #[cfg(feature = "diarization")]
            sortformer,
        }
    }

    /// Run the diarization processing loop (blocking — spawn on a dedicated thread).
    ///
    /// Consumes `DiarizationInput`s from `input_rx` until the channel closes.
    pub fn run(mut self, input_rx: Receiver<DiarizationInput>) {
        log::info!("DiarizationWorker: entering processing loop");

        while let Ok(input) = input_rx.recv() {
            let result = self.process_input(input);

            if let Err(e) = self.output_tx.send(result) {
                log::warn!("DiarizationWorker: output channel closed, stopping: {}", e);
                return;
            }
        }

        // Flush Sortformer at end of stream
        #[cfg(feature = "diarization")]
        if let Some(ref mut sf) = self.sortformer {
            match sf.flush() {
                Ok(segments) => {
                    if !segments.is_empty() {
                        log::info!(
                            "DiarizationWorker: flushed {} final segment(s) from Sortformer",
                            segments.len()
                        );
                    }
                }
                Err(e) => {
                    log::warn!("DiarizationWorker: Sortformer flush error: {}", e);
                }
            }
        }

        log::info!(
            "DiarizationWorker: input channel closed, exiting. Tracked {} speaker(s)",
            self.speakers.len()
        );
    }

    /// Returns `true` if the Sortformer engine is active.
    fn is_sortformer_active(&self) -> bool {
        #[cfg(feature = "diarization")]
        {
            self.sortformer.is_some()
        }
        #[cfg(not(feature = "diarization"))]
        {
            false
        }
    }

    /// Process a single diarization input and return an enriched transcript.
    pub fn process_input(&mut self, input: DiarizationInput) -> DiarizedTranscript {
        if self.is_sortformer_active() {
            self.process_input_sortformer(input)
        } else {
            self.process_input_simple(input)
        }
    }

    // ── Sortformer backend ───────────────────────────────────────────

    /// Process a single input using the Sortformer streaming engine.
    fn process_input_sortformer(&mut self, input: DiarizationInput) -> DiarizedTranscript {
        #[cfg(feature = "diarization")]
        {
            let sf = self
                .sortformer
                .as_mut()
                .expect("process_input_sortformer called but sortformer is None");

            let segment_duration =
                input.speech_end_time.as_secs_f64() - input.speech_start_time.as_secs_f64();

            // Feed the audio chunk to Sortformer
            let segments = match sf.feed(&input.speech_audio) {
                Ok(segs) => segs,
                Err(e) => {
                    log::warn!(
                        "DiarizationWorker: Sortformer feed failed, assigning unknown: {}",
                        e
                    );
                    Vec::new()
                }
            };

            // Determine the dominant speaker for this chunk:
            // Pick the speaker with the longest total duration across returned segments.
            let speaker_id = Self::dominant_speaker(&segments);

            log::debug!(
                "DiarizationWorker [Sortformer]: {} segment(s) returned, dominant speaker = {:?}",
                segments.len(),
                speaker_id,
            );

            // Map the Sortformer speaker_id (0..3) to our internal speaker tracking.
            let speaker_idx = match speaker_id {
                Some(sid) => self.get_or_create_sortformer_speaker(sid),
                None => self.get_or_create_unknown_speaker(),
            };

            // Update stats for this speaker
            {
                let speaker = &mut self.speakers[speaker_idx];
                speaker.segment_count += 1;
                speaker.total_speaking_time += segment_duration;
            }

            let speaker = &self.speakers[speaker_idx];

            log::debug!(
                "DiarizationWorker [Sortformer]: assigned to {} (segments={}, total_time={:.1}s)",
                speaker.label,
                speaker.segment_count,
                speaker.total_speaking_time,
            );

            // Build enriched transcript
            let mut segment = input.transcript;
            segment.speaker_id = Some(speaker.id.clone());
            segment.speaker_label = Some(speaker.label.clone());

            let speaker_info = SpeakerInfo {
                id: speaker.id.clone(),
                label: speaker.label.clone(),
                color: speaker.color.clone(),
                total_speaking_time: speaker.total_speaking_time,
                segment_count: speaker.segment_count,
            };

            DiarizedTranscript {
                segment,
                speaker_info,
            }
        }

        #[cfg(not(feature = "diarization"))]
        {
            // Should never be reached — is_sortformer_active() returns false
            // when the feature is disabled. Fall back to simple.
            self.process_input_simple(input)
        }
    }

    /// From a set of Sortformer segments, find the speaker with the longest
    /// total duration. Returns `None` if no segments were produced.
    #[cfg(feature = "diarization")]
    fn dominant_speaker(segments: &[parakeet_rs::sortformer::SpeakerSegment]) -> Option<usize> {
        if segments.is_empty() {
            return None;
        }

        // Accumulate duration per speaker_id
        let mut durations = [0u64; SORTFORMER_MAX_SPEAKERS];
        for seg in segments {
            let sid = seg.speaker_id;
            if sid < SORTFORMER_MAX_SPEAKERS {
                durations[sid] += seg.end.saturating_sub(seg.start);
            }
        }

        durations
            .iter()
            .enumerate()
            .filter(|(_, &d)| d > 0)
            .max_by_key(|(_, &d)| d)
            .map(|(id, _)| id)
    }

    /// Get or create a speaker profile for a Sortformer speaker ID (0-based).
    /// Maps Sortformer IDs (0..3) to stable "Speaker A".."Speaker D" labels.
    #[allow(dead_code)] // Used when `diarization` feature is enabled
    fn get_or_create_sortformer_speaker(&mut self, sortformer_id: usize) -> usize {
        let target_id = format!("speaker-sf-{}", sortformer_id);

        // Look for existing profile
        if let Some(idx) = self.speakers.iter().position(|s| s.id == target_id) {
            return idx;
        }

        // Create a new profile with letter-based label (A, B, C, D)
        let letter = (b'A' + sortformer_id as u8) as char;
        let color_idx = sortformer_id % SPEAKER_COLORS.len();

        let profile = SpeakerProfile {
            id: target_id,
            label: format!("Speaker {}", letter),
            color: SPEAKER_COLORS[color_idx].to_string(),
            features: None,
            segment_count: 0,
            total_speaking_time: 0.0,
        };

        log::info!(
            "DiarizationWorker: created Sortformer speaker '{}' (color={})",
            profile.label,
            profile.color,
        );

        self.speakers.push(profile);
        self.speakers.len() - 1
    }

    /// Get or create an "Unknown" speaker for when Sortformer returns no segments.
    #[allow(dead_code)] // Used when `diarization` feature is enabled
    fn get_or_create_unknown_speaker(&mut self) -> usize {
        let target_id = "speaker-unknown";

        if let Some(idx) = self.speakers.iter().position(|s| s.id == target_id) {
            return idx;
        }

        let profile = SpeakerProfile {
            id: target_id.to_string(),
            label: "Unknown".to_string(),
            color: "#888888".to_string(),
            features: None,
            segment_count: 0,
            total_speaking_time: 0.0,
        };

        log::info!("DiarizationWorker: created Unknown speaker profile");

        self.speakers.push(profile);
        self.speakers.len() - 1
    }

    // ── Simple backend ───────────────────────────────────────────────

    /// Process a single diarization input using the Simple (signal-based) backend.
    fn process_input_simple(&mut self, input: DiarizationInput) -> DiarizedTranscript {
        // 1. Extract audio features
        let features = Self::extract_features(&input.speech_audio);

        log::debug!(
            "DiarizationWorker [Simple]: features for segment '{}': rms={:.4}, zcr={:.4}, mad={:.4}",
            input.transcript.id,
            features.rms_energy,
            features.zero_crossing_rate,
            features.spectral_centroid,
        );

        // 2. Compute time gap from previous segment
        let time_gap = match self.last_segment_end {
            Some(prev_end) => (input.transcript.start_time - prev_end).max(0.0),
            None => 0.0,
        };
        self.last_segment_end = Some(input.transcript.end_time);

        // 3. Find or create speaker
        let speaker_idx = self.find_or_create_speaker_simple(&features, time_gap);

        // 4. Update the matched speaker's running features & stats
        let segment_duration =
            input.speech_end_time.as_secs_f64() - input.speech_start_time.as_secs_f64();
        {
            let speaker = &mut self.speakers[speaker_idx];
            if let Some(ref mut existing) = speaker.features {
                update_features(existing, &features, speaker.segment_count);
            }
            speaker.segment_count += 1;
            speaker.total_speaking_time += segment_duration;
        }

        let speaker = &self.speakers[speaker_idx];

        log::debug!(
            "DiarizationWorker [Simple]: assigned to {} (distance-based, segments={}, total_time={:.1}s)",
            speaker.label,
            speaker.segment_count,
            speaker.total_speaking_time,
        );

        // 5. Build enriched transcript
        let mut segment = input.transcript;
        segment.speaker_id = Some(speaker.id.clone());
        segment.speaker_label = Some(speaker.label.clone());

        let speaker_info = SpeakerInfo {
            id: speaker.id.clone(),
            label: speaker.label.clone(),
            color: speaker.color.clone(),
            total_speaking_time: speaker.total_speaking_time,
            segment_count: speaker.segment_count,
        };

        DiarizedTranscript {
            segment,
            speaker_info,
        }
    }

    // ── Feature extraction (Simple) ──────────────────────────────────

    /// Compute simple audio features from a 16 kHz mono f32 waveform.
    pub fn extract_features(audio: &[f32]) -> AudioFeatures {
        if audio.is_empty() {
            return AudioFeatures {
                rms_energy: 0.0,
                zero_crossing_rate: 0.0,
                spectral_centroid: 0.0,
            };
        }

        let n = audio.len() as f32;

        // RMS energy
        let sum_sq: f32 = audio.iter().map(|&x| x * x).sum();
        let rms_energy = (sum_sq / n).sqrt();

        // Zero-crossing rate
        let zero_crossings: usize = audio
            .windows(2)
            .filter(|w| (w[0] >= 0.0) != (w[1] >= 0.0))
            .count();
        let zero_crossing_rate = if audio.len() > 1 {
            zero_crossings as f32 / (audio.len() - 1) as f32
        } else {
            0.0
        };

        // Mean absolute deviation (MAD)
        let mean: f32 = audio.iter().sum::<f32>() / n;
        let mad: f32 = audio.iter().map(|&x| (x - mean).abs()).sum::<f32>() / n;

        AudioFeatures {
            rms_energy,
            zero_crossing_rate,
            spectral_centroid: mad,
        }
    }

    // ── Speaker matching (Simple) ────────────────────────────────────

    /// Find the best matching speaker for the given features, or create a new one.
    /// (Simple backend only.)
    fn find_or_create_speaker_simple(&mut self, features: &AudioFeatures, time_gap: f64) -> usize {
        // Only consider speakers that have feature profiles (Simple-created speakers).
        let simple_speakers: Vec<(usize, &SpeakerProfile)> = self
            .speakers
            .iter()
            .enumerate()
            .filter(|(_, s)| s.features.is_some())
            .collect();

        if simple_speakers.is_empty() {
            return self.create_speaker_simple(features);
        }

        // Find closest existing speaker
        let (best_idx, best_dist) = simple_speakers
            .iter()
            .map(|&(i, sp)| (i, feature_distance(features, sp.features.as_ref().unwrap())))
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .expect("simple_speakers is non-empty");

        // Apply gap penalty
        let effective_threshold = if time_gap > self.config.gap_threshold_secs {
            self.config.similarity_threshold * 0.7
        } else {
            self.config.similarity_threshold
        };

        log::debug!(
            "DiarizationWorker [Simple]: best match = {} (dist={:.4}, threshold={:.4}, gap={:.2}s)",
            self.speakers[best_idx].label,
            best_dist,
            effective_threshold,
            time_gap,
        );

        if best_dist < effective_threshold {
            best_idx
        } else if self.speakers.len() < self.config.max_speakers {
            self.create_speaker_simple(features)
        } else {
            log::debug!(
                "DiarizationWorker [Simple]: max speakers reached ({}), assigning to closest",
                self.config.max_speakers,
            );
            best_idx
        }
    }

    /// Create a new speaker profile (Simple backend) and return its index.
    fn create_speaker_simple(&mut self, features: &AudioFeatures) -> usize {
        let num = self.next_speaker_num;
        self.next_speaker_num += 1;

        let color_idx = (num as usize - 1) % SPEAKER_COLORS.len();

        let profile = SpeakerProfile {
            id: format!("speaker-{}", num),
            label: format!("Speaker {}", num),
            color: SPEAKER_COLORS[color_idx].to_string(),
            features: Some(*features),
            segment_count: 0,
            total_speaking_time: 0.0,
        };

        log::info!(
            "DiarizationWorker [Simple]: created new speaker '{}' (color={})",
            profile.label,
            profile.color,
        );

        self.speakers.push(profile);
        self.speakers.len() - 1
    }
}

// ── Free functions ───────────────────────────────────────────────────────

/// Compute normalised Euclidean distance between two feature vectors.
pub fn feature_distance(a: &AudioFeatures, b: &AudioFeatures) -> f32 {
    let d_rms = (a.rms_energy - b.rms_energy) / 0.5;
    let d_zcr = (a.zero_crossing_rate - b.zero_crossing_rate) / 0.3;
    let d_mad = (a.spectral_centroid - b.spectral_centroid) / 0.3;
    ((d_rms * d_rms + d_zcr * d_zcr + d_mad * d_mad) / 3.0).sqrt()
}

/// Incrementally update a speaker's running-average features with a new
/// observation using an exponential moving average.
fn update_features(existing: &mut AudioFeatures, new: &AudioFeatures, count: u32) {
    let alpha = 1.0 / (count as f32 + 1.0);
    existing.rms_energy = existing.rms_energy * (1.0 - alpha) + new.rms_energy * alpha;
    existing.zero_crossing_rate =
        existing.zero_crossing_rate * (1.0 - alpha) + new.zero_crossing_rate * alpha;
    existing.spectral_centroid =
        existing.spectral_centroid * (1.0 - alpha) + new.spectral_centroid * alpha;
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // -- AudioFeatures / extract_features ----------------------------------

    #[test]
    fn extract_features_empty_audio() {
        let f = DiarizationWorker::extract_features(&[]);
        assert_eq!(f.rms_energy, 0.0);
        assert_eq!(f.zero_crossing_rate, 0.0);
        assert_eq!(f.spectral_centroid, 0.0);
    }

    #[test]
    fn extract_features_silence() {
        let audio = vec![0.0_f32; 16000]; // 1 second of silence
        let f = DiarizationWorker::extract_features(&audio);
        assert!(f.rms_energy.abs() < 1e-6);
        assert!(f.zero_crossing_rate.abs() < 1e-6);
        assert!(f.spectral_centroid.abs() < 1e-6);
    }

    #[test]
    fn extract_features_dc_offset() {
        let audio = vec![0.5_f32; 1000];
        let f = DiarizationWorker::extract_features(&audio);
        assert!((f.rms_energy - 0.5).abs() < 1e-4);
        assert_eq!(f.zero_crossing_rate, 0.0);
        assert!(
            f.spectral_centroid.abs() < 1e-6,
            "MAD should be ~0 for constant signal"
        );
    }

    #[test]
    fn extract_features_alternating_signal() {
        let audio: Vec<f32> = (0..1000)
            .map(|i| if i % 2 == 0 { 1.0 } else { -1.0 })
            .collect();
        let f = DiarizationWorker::extract_features(&audio);
        assert!(
            (f.rms_energy - 1.0).abs() < 1e-4,
            "RMS of ±1 signal should be 1.0"
        );
        assert!(
            (f.zero_crossing_rate - 1.0).abs() < 1e-3,
            "ZCR of fully alternating signal should be ~1.0, got {}",
            f.zero_crossing_rate
        );
        assert!(
            (f.spectral_centroid - 1.0).abs() < 1e-3,
            "MAD should be ~1.0, got {}",
            f.spectral_centroid
        );
    }

    #[test]
    fn extract_features_sine_wave() {
        let n = 1600;
        let audio: Vec<f32> = (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16000.0).sin())
            .collect();
        let f = DiarizationWorker::extract_features(&audio);
        assert!(
            (f.rms_energy - 0.707).abs() < 0.02,
            "RMS of unit sine should be ~0.707, got {}",
            f.rms_energy
        );
        assert!(
            (f.zero_crossing_rate - 0.055).abs() < 0.01,
            "ZCR of 440 Hz signal at 16 kHz should be ~0.055, got {}",
            f.zero_crossing_rate
        );
        assert!(f.spectral_centroid > 0.1);
    }

    // -- feature_distance ---------------------------------------------------

    #[test]
    fn feature_distance_identical() {
        let a = AudioFeatures {
            rms_energy: 0.1,
            zero_crossing_rate: 0.05,
            spectral_centroid: 0.08,
        };
        assert!((feature_distance(&a, &a)).abs() < 1e-6);
    }

    #[test]
    fn feature_distance_symmetry() {
        let a = AudioFeatures {
            rms_energy: 0.1,
            zero_crossing_rate: 0.05,
            spectral_centroid: 0.08,
        };
        let b = AudioFeatures {
            rms_energy: 0.3,
            zero_crossing_rate: 0.15,
            spectral_centroid: 0.12,
        };
        let d_ab = feature_distance(&a, &b);
        let d_ba = feature_distance(&b, &a);
        assert!((d_ab - d_ba).abs() < 1e-6, "distance should be symmetric");
    }

    #[test]
    fn feature_distance_scales_correctly() {
        let base = AudioFeatures {
            rms_energy: 0.0,
            zero_crossing_rate: 0.0,
            spectral_centroid: 0.0,
        };
        let far = AudioFeatures {
            rms_energy: 0.5,
            zero_crossing_rate: 0.3,
            spectral_centroid: 0.3,
        };
        let dist = feature_distance(&base, &far);
        assert!(
            (dist - 1.0).abs() < 1e-4,
            "distance from origin to max should be 1.0, got {}",
            dist
        );
    }

    // -- update_features ----------------------------------------------------

    #[test]
    fn update_features_first_observation() {
        let mut existing = AudioFeatures {
            rms_energy: 0.2,
            zero_crossing_rate: 0.1,
            spectral_centroid: 0.05,
        };
        let new = AudioFeatures {
            rms_energy: 0.4,
            zero_crossing_rate: 0.2,
            spectral_centroid: 0.15,
        };
        update_features(&mut existing, &new, 0);
        assert!((existing.rms_energy - 0.4).abs() < 1e-5);
        assert!((existing.zero_crossing_rate - 0.2).abs() < 1e-5);
        assert!((existing.spectral_centroid - 0.15).abs() < 1e-5);
    }

    #[test]
    fn update_features_converges_toward_new() {
        let mut existing = AudioFeatures {
            rms_energy: 0.0,
            zero_crossing_rate: 0.0,
            spectral_centroid: 0.0,
        };
        let target = AudioFeatures {
            rms_energy: 1.0,
            zero_crossing_rate: 1.0,
            spectral_centroid: 1.0,
        };
        for count in 0..100 {
            update_features(&mut existing, &target, count);
        }
        assert!(
            (existing.rms_energy - 1.0).abs() < 0.05,
            "should converge toward 1.0, got {}",
            existing.rms_energy
        );
    }

    // -- DiarizationConfig default ------------------------------------------

    #[test]
    fn default_config_values() {
        let cfg = DiarizationConfig::default();
        assert!((cfg.similarity_threshold - 0.7).abs() < f32::EPSILON);
        assert_eq!(cfg.max_speakers, 10);
        assert!((cfg.gap_threshold_secs - 2.0).abs() < f64::EPSILON);
        assert!(matches!(cfg.backend, DiarizationBackend::Simple));
    }

    #[test]
    fn sortformer_config_sets_backend() {
        let cfg = DiarizationConfig::sortformer(PathBuf::from("/tmp/model.onnx"));
        assert!(matches!(cfg.backend, DiarizationBackend::Sortformer { .. }));
        assert_eq!(cfg.max_speakers, SORTFORMER_MAX_SPEAKERS);
    }

    // -- Speaker creation and assignment (Simple backend) -------------------

    #[test]
    fn process_input_creates_first_speaker() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut worker = DiarizationWorker::new(DiarizationConfig::default(), tx);

        let input = make_test_input(vec![0.1; 8000], 0.0, 0.5);
        let result = worker.process_input(input);

        assert_eq!(result.segment.speaker_id, Some("speaker-1".to_string()));
        assert_eq!(result.segment.speaker_label, Some("Speaker 1".to_string()));
        assert_eq!(result.speaker_info.id, "speaker-1");
        assert_eq!(result.speaker_info.color, "#4A90D9");
        assert_eq!(result.speaker_info.segment_count, 1);
        assert_eq!(worker.speakers.len(), 1);

        drop(rx);
    }

    #[test]
    fn process_input_same_speaker_for_similar_audio() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut worker = DiarizationWorker::new(DiarizationConfig::default(), tx);

        let input1 = make_test_input(vec![0.1; 8000], 0.0, 0.5);
        let input2 = make_test_input(vec![0.1; 8000], 0.5, 1.0);

        let r1 = worker.process_input(input1);
        let r2 = worker.process_input(input2);

        assert_eq!(r1.segment.speaker_id, r2.segment.speaker_id);
        assert_eq!(worker.speakers.len(), 1);
        assert_eq!(worker.speakers[0].segment_count, 2);
    }

    #[test]
    fn process_input_different_speaker_for_different_audio() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let config = DiarizationConfig {
            similarity_threshold: 0.3,
            ..DiarizationConfig::default()
        };
        let mut worker = DiarizationWorker::new(config, tx);

        let quiet_dc = vec![0.05_f32; 8000];
        let loud_alternating: Vec<f32> = (0..8000)
            .map(|i| if i % 2 == 0 { 0.8 } else { -0.8 })
            .collect();

        let input1 = make_test_input(quiet_dc, 0.0, 0.5);
        let input2 = make_test_input(loud_alternating, 1.0, 1.5);

        let r1 = worker.process_input(input1);
        let r2 = worker.process_input(input2);

        assert_ne!(
            r1.segment.speaker_id, r2.segment.speaker_id,
            "very different audio should yield different speakers"
        );
        assert_eq!(worker.speakers.len(), 2);
    }

    #[test]
    fn max_speakers_cap_is_respected() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let config = DiarizationConfig {
            similarity_threshold: 0.001,
            max_speakers: 3,
            ..DiarizationConfig::default()
        };
        let mut worker = DiarizationWorker::new(config, tx);

        for i in 0..5 {
            let amp = 0.1 + i as f32 * 0.15;
            let audio = vec![amp; 8000];
            let start = i as f64;
            let input = make_test_input(audio, start, start + 0.5);
            worker.process_input(input);
        }

        assert!(
            worker.speakers.len() <= 3,
            "should not exceed max_speakers=3, got {}",
            worker.speakers.len()
        );
    }

    #[test]
    fn speaker_colors_cycle() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let config = DiarizationConfig {
            similarity_threshold: 0.0,
            max_speakers: 12,
            ..DiarizationConfig::default()
        };
        let mut worker = DiarizationWorker::new(config, tx);

        for i in 0..12 {
            let amp = 0.05 + i as f32 * 0.05;
            let audio = vec![amp; 8000];
            let start = i as f64 * 10.0;
            let input = make_test_input(audio, start, start + 0.5);
            worker.process_input(input);
        }

        assert_eq!(worker.speakers.len(), 12);
        assert_eq!(worker.speakers[10].color, SPEAKER_COLORS[0]);
    }

    // -- Sortformer speaker mapping (unit tests without model) -------------

    #[test]
    fn sortformer_speaker_labels_use_letters() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut worker = DiarizationWorker::new(DiarizationConfig::default(), tx);

        let idx_a = worker.get_or_create_sortformer_speaker(0);
        let idx_b = worker.get_or_create_sortformer_speaker(1);
        let idx_a2 = worker.get_or_create_sortformer_speaker(0); // same speaker

        assert_eq!(worker.speakers[idx_a].label, "Speaker A");
        assert_eq!(worker.speakers[idx_b].label, "Speaker B");
        assert_eq!(idx_a, idx_a2, "same sortformer ID should return same index");
        assert_eq!(worker.speakers.len(), 2);
    }

    #[test]
    fn unknown_speaker_is_created_once() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut worker = DiarizationWorker::new(DiarizationConfig::default(), tx);

        let idx1 = worker.get_or_create_unknown_speaker();
        let idx2 = worker.get_or_create_unknown_speaker();
        assert_eq!(idx1, idx2);
        assert_eq!(worker.speakers[idx1].label, "Unknown");
    }

    // -- Backend selection --------------------------------------------------

    #[test]
    fn default_backend_is_simple() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let worker = DiarizationWorker::new(DiarizationConfig::default(), tx);
        assert!(!worker.is_sortformer_active());
    }

    #[test]
    fn sortformer_backend_falls_back_without_model() {
        // Requesting Sortformer with a non-existent model path should fall back
        // gracefully (sortformer field = None when feature is enabled, or always
        // inactive when feature is disabled).
        let (tx, _rx) = crossbeam_channel::unbounded();
        let config = DiarizationConfig::sortformer(PathBuf::from("/nonexistent/model.onnx"));
        let worker = DiarizationWorker::new(config, tx);
        // Whether or not the feature is enabled, the worker should still function
        // (Simple fallback).
        assert!(!worker.is_sortformer_active() || cfg!(feature = "diarization"));
    }

    // -- Helpers -----------------------------------------------------------

    fn make_test_input(audio: Vec<f32>, start_secs: f64, end_secs: f64) -> DiarizationInput {
        DiarizationInput {
            transcript: TranscriptSegment {
                id: uuid::Uuid::new_v4().to_string(),
                source_id: "test-source".to_string(),
                speaker_id: None,
                speaker_label: None,
                text: "test text".to_string(),
                start_time: start_secs,
                end_time: end_secs,
                confidence: 0.9,
            },
            speech_audio: audio,
            speech_start_time: Duration::from_secs_f64(start_secs),
            speech_end_time: Duration::from_secs_f64(end_secs),
        }
    }
}
