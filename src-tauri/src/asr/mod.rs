//! Automatic Speech Recognition (ASR) module.
//!
//! Uses whisper-rs to transcribe speech utterances into text segments.
//! The speech pipeline owns the Whisper state and calls
//! [`AsrWorker::transcribe_segment`] per `SpeechSegment`, producing
//! `TranscriptSegment`s.

use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(feature = "asr-whisper")]
use log::debug;
#[cfg(feature = "asr-whisper")]
use uuid::Uuid;
#[cfg(feature = "asr-whisper")]
use whisper_rs::{FullParams, SamplingStrategy};

pub mod assemblyai;
pub mod aws_transcribe;
pub mod cloud;
pub mod deepgram;
#[cfg(test)]
mod event_fixtures;
#[cfg(test)]
mod fixtures;
pub mod gladia;
pub mod moonshine;
pub mod openai_realtime;
// The reconnect ladder now lives at crate root (`crate::reconnect`) so the
// Gemini + Aura transports can share it too (review n2). Re-exported here so the
// ASR clients' existing `super::reconnect::*` paths keep resolving.
pub(crate) use crate::reconnect;
pub mod revai;
#[cfg(feature = "sherpa-streaming")]
pub mod sherpa_streaming;
pub mod soniox;
pub mod speechmatics;
mod transport;
#[cfg(test)]
mod ws_fixture;

/// Provider-client defense-in-depth guard for session content egress.
///
/// Command handlers still enforce the user-facing privacy mode before a
/// session starts. This value is threaded into long-lived provider clients so
/// direct lower-level calls cannot send content if a caller bypasses the
/// command/session gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderContentEgressPolicy {
    allows_content: bool,
    privacy_mode: &'static str,
}

impl ProviderContentEgressPolicy {
    pub const fn allow() -> Self {
        Self {
            allows_content: true,
            privacy_mode: "byok_cloud",
        }
    }

    pub const fn block(privacy_mode: &'static str) -> Self {
        Self {
            allows_content: false,
            privacy_mode,
        }
    }

    pub fn from_privacy_mode(mode: crate::settings::PrivacyMode) -> Self {
        if mode.allows_session_cloud_content_transfer() {
            Self::allow()
        } else {
            Self::block(mode.as_str())
        }
    }

    pub fn from_privacy_mode_and_transfer_requirement(
        mode: crate::settings::PrivacyMode,
        requires_cloud_content_transfer: bool,
    ) -> Self {
        if !requires_cloud_content_transfer || mode.allows_session_cloud_content_transfer() {
            Self::allow()
        } else {
            Self::block(mode.as_str())
        }
    }

    pub fn check_audio(self, provider: &str) -> Result<(), String> {
        self.check_content(provider, "audio")
    }

    pub fn check_text(self, provider: &str) -> Result<(), String> {
        self.check_content(provider, "text")
    }

    pub fn check_prompt(self, provider: &str) -> Result<(), String> {
        self.check_content(provider, "prompt")
    }

    pub fn check_json(self, provider: &str) -> Result<(), String> {
        self.check_content(provider, "json")
    }

    pub fn check_content(self, provider: &str, data_class: &str) -> Result<(), String> {
        if self.allows_content {
            return Ok(());
        }

        Err(format!(
            "Privacy policy blocked {data_class} egress to {provider} in mode {}",
            self.privacy_mode
        ))
    }
}

impl Default for ProviderContentEgressPolicy {
    fn default() -> Self {
        Self::block("explicit_policy_required")
    }
}

#[cfg(test)]
mod provider_content_egress_policy_tests {
    use super::ProviderContentEgressPolicy;

    #[test]
    fn provider_content_egress_policy_requires_explicit_allow_by_default() {
        let policy = ProviderContentEgressPolicy::default();

        for result in [
            policy.check_audio("asr.deepgram"),
            policy.check_text("tts.deepgram_aura"),
            policy.check_prompt("llm.openrouter"),
            policy.check_json("llm.api"),
        ] {
            let error = result.expect_err("default policy should require an explicit allow");
            assert!(error.contains("Privacy policy blocked"));
            assert!(error.contains("explicit_policy_required"));
        }
    }

    #[test]
    fn provider_content_egress_policy_blocks_each_content_class_without_payload() {
        let policy = ProviderContentEgressPolicy::block("local_only");

        for (data_class, result) in [
            ("audio", policy.check_audio("asr.deepgram")),
            ("text", policy.check_text("tts.deepgram_aura")),
            ("prompt", policy.check_prompt("llm.openrouter")),
            ("json", policy.check_json("llm.api")),
        ] {
            let error = result.expect_err("blocked policy should reject content");
            assert!(error.contains("Privacy policy blocked"));
            assert!(error.contains(data_class));
            assert!(error.contains("local_only"));
            assert!(
                !error.contains("patient said private diagnosis"),
                "policy error must not echo payload text: {error}"
            );
        }
    }

    #[test]
    fn provider_content_egress_policy_allows_local_transfer_requirement_in_local_only_mode() {
        let local_loopback =
            ProviderContentEgressPolicy::from_privacy_mode_and_transfer_requirement(
                crate::settings::PrivacyMode::LocalOnly,
                false,
            );
        assert!(
            local_loopback.check_prompt("llm.api").is_ok(),
            "local-only mode must still allow loopback/local providers"
        );

        let remote = ProviderContentEgressPolicy::from_privacy_mode_and_transfer_requirement(
            crate::settings::PrivacyMode::LocalOnly,
            true,
        );
        assert!(remote.check_prompt("llm.api").is_err());
    }
}

// Only the whisper-gated `transcribe_segment` returns `TranscriptSegment`s now
// that the vestigial `Sender<TranscriptSegment>` field is gone (FA-6b).
#[cfg(feature = "asr-whisper")]
use crate::state::TranscriptSegment;

/// A segment of speech audio ready for ASR transcription.
///
/// This is the ASR module's input type — it represents a contiguous chunk
/// of speech audio (typically ~2 seconds) accumulated from the pipeline.
#[derive(Debug, Clone)]
pub struct SpeechSegment {
    /// Identifier of the audio source that produced this segment.
    pub source_id: String,
    /// 16kHz mono f32 audio data for the speech segment.
    pub audio: Vec<f32>,
    /// Start time relative to stream start.
    pub start_time: Duration,
    /// End time relative to stream start.
    pub end_time: Duration,
    /// Number of audio frames (equal to `audio.len()`).
    pub num_frames: usize,
}

/// Configuration for the ASR worker.
pub struct AsrConfig {
    /// Path to the Whisper GGML model file (e.g. `models/ggml-small.en.bin`).
    pub model_path: PathBuf,
    /// Language code for transcription (e.g. `"en"`).
    pub language: String,
    /// Number of threads for whisper inference. Default: 4.
    pub n_threads: i32,
    /// Sampling temperature. 0.0 = greedy. Default: 0.0.
    pub temperature: f32,
    /// Beam size (only used with beam-search strategy). Default: 5.
    pub beam_size: i32,
}

impl AsrConfig {
    /// Create an `AsrConfig` with the model path resolved under the given
    /// models directory.
    pub fn with_models_dir(models_dir: &Path) -> Self {
        Self {
            model_path: models_dir.join("ggml-small.en.bin"),
            language: "en".to_string(),
            n_threads: 4,
            temperature: 0.0,
            beam_size: 5,
        }
    }

    pub fn with_models_dir_and_model(models_dir: &Path, model_filename: &str) -> Self {
        Self {
            model_path: models_dir.join(model_filename),
            language: "en".to_string(),
            n_threads: 4,
            temperature: 0.0,
            beam_size: 5,
        }
    }
}

impl Default for AsrConfig {
    fn default() -> Self {
        Self {
            model_path: PathBuf::from("models/ggml-small.en.bin"),
            language: "en".to_string(),
            n_threads: 4,
            temperature: 0.0,
            beam_size: 5,
        }
    }
}

/// ASR worker that processes speech segments into transcript segments.
///
/// The live entrypoint is [`AsrWorker::transcribe_segment`]: the speech
/// pipeline (`speech/mod.rs`) owns the Whisper state and drives transcription
/// segment-by-segment, interleaving diarization and downstream emission. There
/// is intentionally no self-owned receive loop — `transcribe_segment` is called
/// directly from the pipeline's own loop.
// `config` is only read by the whisper-gated transcribe_segment.
#[cfg_attr(not(feature = "asr-whisper"), allow(dead_code))]
pub struct AsrWorker {
    config: AsrConfig,
    segments_processed: u64,
}

impl AsrWorker {
    /// Create a new ASR worker with the given config. The pipeline
    /// (`speech/mod.rs`) drives transcription by calling
    /// [`Self::transcribe_segment`] directly and routes the returned segments
    /// itself — there is no self-owned output channel (FA-6b dropped the
    /// vestigial one left over from the removed `run()` loop).
    pub fn new(config: AsrConfig) -> Self {
        Self {
            config,
            segments_processed: 0,
        }
    }

    /// Transcribe a single speech segment into zero or more transcript segments.
    ///
    /// Configures Whisper parameters, runs inference, then extracts and filters
    /// the resulting segments. Whisper timestamps (in centiseconds) are converted
    /// to absolute seconds by adding the speech segment's `start_time` offset.
    #[cfg(feature = "asr-whisper")]
    pub fn transcribe_segment(
        &mut self,
        state: &mut whisper_rs::WhisperState,
        segment: &SpeechSegment,
    ) -> Result<Vec<TranscriptSegment>, String> {
        // ── Configure Whisper params ────────────────────────────────────
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some(&self.config.language));
        params.set_print_progress(false);
        params.set_print_timestamps(false);
        params.set_print_realtime(false);
        params.set_print_special(false);
        params.set_single_segment(false);
        params.set_no_context(true);
        params.set_n_threads(self.config.n_threads);
        params.set_temperature(self.config.temperature);

        // ── Run inference ───────────────────────────────────────────────
        state
            .full(params, &segment.audio)
            .map_err(|e| format!("Whisper inference failed: {}", e))?;

        // ── Extract results ─────────────────────────────────────────────
        let num_segments = state.full_n_segments();

        let mut transcripts = Vec::new();

        for i in 0..num_segments {
            let whisper_seg = match state.get_segment(i) {
                Some(s) => s,
                None => continue,
            };

            let text = whisper_seg
                .to_str()
                .map_err(|e| format!("Failed to get segment text: {}", e))?;

            let text = text.trim().to_string();
            if text.is_empty() {
                continue;
            }

            // Whisper returns timestamps in centiseconds (1/100th of a second)
            let t0 = whisper_seg.start_timestamp();
            let t1 = whisper_seg.end_timestamp();

            // Convert whisper timestamps (centiseconds) to absolute seconds
            // by adding the speech segment's start-time offset.
            let segment_start_secs = segment.start_time.as_secs_f64();
            let start_time = segment_start_secs + (t0 as f64 / 100.0);
            let end_time = segment_start_secs + (t1 as f64 / 100.0);

            // Use (1.0 - no_speech_probability) as a rough confidence proxy
            let confidence = 1.0 - whisper_seg.no_speech_probability();

            self.segments_processed += 1;

            let transcript = TranscriptSegment {
                id: Uuid::new_v4().to_string(),
                source_id: segment.source_id.clone(),
                speaker_id: None,    // filled by diarization later
                speaker_label: None, // filled by diarization later
                text: text.clone(),
                start_time,
                end_time,
                confidence,
            };

            debug!(
                "ASR segment metadata count={} source_id={} start={:.2}s end={:.2}s conf={:.2} text_len={}",
                self.segments_processed,
                segment.source_id,
                start_time,
                end_time,
                confidence,
                text.chars().count()
            );

            transcripts.push(transcript);
        }

        Ok(transcripts)
    }

    /// Returns the total number of transcript segments produced so far.
    pub fn segments_processed(&self) -> u64 {
        self.segments_processed
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_models_dir_joins_default_small_en_model() {
        let cfg = AsrConfig::with_models_dir(Path::new("/opt/models"));
        assert_eq!(
            cfg.model_path,
            PathBuf::from("/opt/models").join("ggml-small.en.bin")
        );
        assert_eq!(cfg.language, "en");
        assert_eq!(cfg.n_threads, 4);
        assert!((cfg.temperature - 0.0).abs() < f32::EPSILON);
        assert_eq!(cfg.beam_size, 5);
    }

    #[test]
    fn with_models_dir_and_model_joins_given_filename() {
        let cfg =
            AsrConfig::with_models_dir_and_model(Path::new("/opt/models"), "ggml-medium.en.bin");
        assert_eq!(
            cfg.model_path,
            PathBuf::from("/opt/models").join("ggml-medium.en.bin")
        );
        // Other fields keep their defaults.
        assert_eq!(cfg.language, "en");
        assert_eq!(cfg.n_threads, 4);
        assert_eq!(cfg.beam_size, 5);
    }

    #[test]
    fn default_config_matches_documented_values() {
        let cfg = AsrConfig::default();
        assert_eq!(cfg.model_path, PathBuf::from("models/ggml-small.en.bin"));
        assert_eq!(cfg.language, "en");
        assert_eq!(cfg.n_threads, 4);
        assert!((cfg.temperature - 0.0).abs() < f32::EPSILON);
        assert_eq!(cfg.beam_size, 5);
    }

    #[test]
    fn speech_segment_num_frames_equals_audio_len_invariant() {
        let audio = vec![0.0_f32; 32_000]; // 2s @ 16kHz
        let seg = SpeechSegment {
            source_id: "src-1".to_string(),
            audio: audio.clone(),
            start_time: Duration::from_secs(0),
            end_time: Duration::from_secs(2),
            num_frames: audio.len(),
        };
        // The documented invariant: num_frames == audio.len().
        assert_eq!(seg.num_frames, seg.audio.len());
    }

    #[test]
    fn new_worker_starts_with_zero_segments_processed() {
        let worker = AsrWorker::new(AsrConfig::default());
        assert_eq!(worker.segments_processed(), 0);
    }
}
