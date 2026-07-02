//! Application settings — persistence layer for user configuration.
//!
//! Settings are stored as YAML in the app config directory and loaded at startup.
//! Legacy `settings.json` files in the app data directory are imported once when
//! `config.yaml` does not exist. If the active config is missing or unparseable,
//! defaults are used.

use serde::{Deserialize, Serialize};
use std::fs;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use tauri::Manager;

use crate::llm::openrouter::OpenRouterRoutingPolicy;

// ---------------------------------------------------------------------------
// Helper default functions
// ---------------------------------------------------------------------------

const FALLBACK_SAMPLE_RATE: u32 = 48000;
// Match the shipped `default.toml` (`audio.channels = 2`). This only takes
// effect if `default.toml` fails to parse; keeping it equal to the bundled
// value means a parse failure degrades to the same channel count the app
// otherwise ships with, instead of silently halving capture to mono.
const FALLBACK_CHANNELS: u16 = 2;
const FALLBACK_WHISPER_MODEL: &str = "ggml-small.en.bin";

/// Process-wide lock serializing settings-file read+write sequences.
///
/// `save_settings` (full-blob writes from `save_settings_cmd` + startup demo
/// saves) and `set_logging_config`'s load→patch→save both persist
/// `config.yaml`. The atomic rename in `save_settings` prevents on-disk
/// corruption, but without serialization a logging commit's load→save could
/// interleave with a full save and silently revert the other's just-written
/// fields (last-writer-wins on stale data). Holding this lock across each
/// read+write sequence makes the two mutually exclusive.
static SETTINGS_IO_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// Acquire the process-wide settings I/O lock. Recovers from poisoning: a
/// panic mid-write can't corrupt the `()` payload, and refusing to ever save
/// settings again after one panic would be worse than proceeding.
pub fn lock_settings_io() -> MutexGuard<'static, ()> {
    SETTINGS_IO_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn configured_sample_rate() -> Option<u32> {
    let hz = crate::config::load_default_config().audio.sample_rate?;
    sample_rate_is_valid(hz).then_some(hz)
}

fn configured_channels() -> Option<u16> {
    let channels = crate::config::load_default_config().audio.channels?;
    channels_is_valid(channels).then_some(channels)
}

fn configured_whisper_model() -> Option<String> {
    crate::config::load_default_config().whisper_model_filename()
}

fn default_aws_region() -> String {
    "us-east-1".to_string()
}
fn default_language_code() -> String {
    "en-US".to_string()
}
fn default_deepgram_model() -> String {
    "nova-3".to_string()
}
fn default_deepgram_endpointing_ms() -> u32 {
    300
}
fn default_deepgram_utterance_end_ms() -> u32 {
    1000
}
fn default_deepgram_eot_threshold() -> f32 {
    0.5
}
fn default_deepgram_eager_eot_threshold() -> f32 {
    0.0
}
fn default_deepgram_eot_timeout_ms() -> u32 {
    0
}
fn default_max_speakers() -> u32 {
    // Default to 0 = NO cap: surface as many speakers as Deepgram actually
    // detects. Speaker attribution is a headline feature, so the safe default is
    // not to suppress it — capping to 2 by default silently hid real speakers
    // (BUG-4: "stuck on 2 speakers"). Users who know they have a 1:1 / interview
    // can opt INTO a small cap to tame Deepgram's occasional over-segmentation.
    0
}
fn default_true() -> bool {
    true
}
fn default_sherpa_model() -> String {
    "streaming-zipformer-en-20M".to_string()
}
fn default_moonshine_model() -> String {
    "moonshine-small-streaming-en".to_string()
}
fn default_openai_realtime_model() -> String {
    crate::asr::openai_realtime::DEFAULT_MODEL.to_string()
}
fn default_soniox_model() -> String {
    crate::asr::soniox::DEFAULT_MODEL.to_string()
}

// ---------------------------------------------------------------------------
// AWS credential source
// ---------------------------------------------------------------------------

#[derive(Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type")]
pub enum AwsCredentialSource {
    #[serde(rename = "default_chain")]
    #[default]
    DefaultChain,
    #[serde(rename = "profile")]
    Profile { name: String },
    #[serde(rename = "access_keys")]
    AccessKeys {
        #[serde(default, skip_serializing)]
        #[schemars(skip)]
        access_key: String,
        /// Legacy inline AWS secret material accepted only for one-time import
        /// into the credential backend. Never serialize this into config.yaml.
        #[serde(default, skip_serializing)]
        #[schemars(skip)]
        secret_key: Option<String>,
        /// Legacy inline AWS STS session token accepted only for one-time
        /// import into the credential backend. Never serialize this into config.yaml.
        #[serde(default, skip_serializing)]
        #[schemars(skip)]
        session_token: Option<String>,
    },
}

impl std::fmt::Debug for AwsCredentialSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DefaultChain => f.write_str("DefaultChain"),
            Self::Profile { name } => f.debug_struct("Profile").field("name", name).finish(),
            Self::AccessKeys {
                access_key,
                secret_key,
                session_token,
            } => f
                .debug_struct("AccessKeys")
                .field(
                    "access_key",
                    &crate::credentials::redacted_secret_presence(Some(access_key)),
                )
                .field(
                    "secret_key",
                    &crate::credentials::redacted_secret_presence(secret_key.as_deref()),
                )
                .field(
                    "session_token",
                    &crate::credentials::redacted_secret_presence(session_token.as_deref()),
                )
                .finish(),
        }
    }
}

// ---------------------------------------------------------------------------
// ASR provider
// ---------------------------------------------------------------------------

#[derive(Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type")]
pub enum AsrProvider {
    #[serde(rename = "local_whisper")]
    #[default]
    LocalWhisper,
    #[serde(rename = "api")]
    Api {
        endpoint: String,
        #[serde(default, skip_serializing)]
        #[schemars(skip)]
        api_key: String,
        model: String,
    },
    #[serde(rename = "aws_transcribe")]
    AwsTranscribe {
        #[serde(default = "default_aws_region")]
        region: String,
        #[serde(default = "default_language_code")]
        language_code: String,
        #[serde(default)]
        credential_source: AwsCredentialSource,
        #[serde(default = "default_true")]
        enable_diarization: bool,
    },
    #[serde(rename = "deepgram")]
    DeepgramStreaming {
        #[serde(default, skip_serializing)]
        #[schemars(skip)]
        api_key: String,
        #[serde(default = "default_deepgram_model")]
        model: String,
        #[serde(default = "default_true")]
        enable_diarization: bool,
        #[serde(default = "default_deepgram_endpointing_ms")]
        endpointing_ms: u32,
        #[serde(default = "default_deepgram_utterance_end_ms")]
        utterance_end_ms: u32,
        #[serde(default = "default_true")]
        vad_events: bool,
        #[serde(default = "default_deepgram_eot_threshold")]
        eot_threshold: f32,
        #[serde(default = "default_deepgram_eager_eot_threshold")]
        eager_eot_threshold: f32,
        #[serde(default = "default_deepgram_eot_timeout_ms")]
        eot_timeout_ms: u32,
        /// Cap on distinct speaker labels. Deepgram streaming diarization can
        /// over-segment (label a 2-person chat as 3+ speakers); when set,
        /// speaker ids beyond this many distinct speakers are remapped to the
        /// most-recently-seen in-range speaker. `0` = no cap (raw Deepgram).
        #[serde(default = "default_max_speakers")]
        max_speakers: u32,
    },
    #[serde(rename = "assemblyai")]
    AssemblyAI {
        #[serde(default, skip_serializing)]
        #[schemars(skip)]
        api_key: String,
        #[serde(default = "default_true")]
        enable_diarization: bool,
    },
    #[serde(rename = "soniox")]
    Soniox {
        #[serde(default, skip_serializing)]
        #[schemars(skip)]
        api_key: String,
        #[serde(default = "default_soniox_model")]
        model: String,
        #[serde(default = "default_true")]
        enable_diarization: bool,
        #[serde(default = "default_true")]
        enable_language_identification: bool,
        #[serde(default)]
        language_hints: Vec<String>,
        #[serde(default = "default_max_speakers")]
        max_speakers: u32,
    },
    #[serde(rename = "sherpa_onnx")]
    SherpaOnnx {
        #[serde(default = "default_sherpa_model")]
        model_dir: String,
        #[serde(default = "default_true")]
        enable_endpoint_detection: bool,
    },
    #[serde(rename = "moonshine")]
    Moonshine {
        #[serde(default = "default_moonshine_model")]
        model_dir: String,
        #[serde(default = "default_true")]
        enable_speaker_hints: bool,
    },
    /// OpenAI Realtime streaming transcription (ADR-0002 Wave A —
    /// `gpt-realtime-whisper`). The native speech-to-speech voice agent is a
    /// separate provider (B18) and is not selectable here.
    ///
    /// The Bearer token reuses the existing `openai_api_key` credential slot
    /// (shared with the OpenAI-compatible HTTP provider); it stays empty in
    /// `config.yaml` and is hydrated at runtime by
    /// [`hydrate_runtime_credentials`].
    #[serde(rename = "openai_realtime")]
    OpenAiRealtimeTranscription {
        #[serde(default, skip_serializing)]
        #[schemars(skip)]
        api_key: String,
        #[serde(default = "default_openai_realtime_model")]
        model: String,
        #[serde(default)]
        language: Option<String>,
    },
}

impl std::fmt::Debug for AsrProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LocalWhisper => f.write_str("LocalWhisper"),
            Self::Api {
                endpoint,
                api_key,
                model,
            } => f
                .debug_struct("Api")
                .field("endpoint", endpoint)
                .field(
                    "api_key",
                    &crate::credentials::redacted_secret_presence(Some(api_key)),
                )
                .field("model", model)
                .finish(),
            Self::AwsTranscribe {
                region,
                language_code,
                credential_source,
                enable_diarization,
            } => f
                .debug_struct("AwsTranscribe")
                .field("region", region)
                .field("language_code", language_code)
                .field("credential_source", credential_source)
                .field("enable_diarization", enable_diarization)
                .finish(),
            Self::DeepgramStreaming {
                api_key,
                model,
                enable_diarization,
                endpointing_ms,
                utterance_end_ms,
                vad_events,
                eot_threshold,
                eager_eot_threshold,
                eot_timeout_ms,
                max_speakers,
            } => f
                .debug_struct("DeepgramStreaming")
                .field(
                    "api_key",
                    &crate::credentials::redacted_secret_presence(Some(api_key)),
                )
                .field("model", model)
                .field("enable_diarization", enable_diarization)
                .field("endpointing_ms", endpointing_ms)
                .field("utterance_end_ms", utterance_end_ms)
                .field("vad_events", vad_events)
                .field("eot_threshold", eot_threshold)
                .field("eager_eot_threshold", eager_eot_threshold)
                .field("eot_timeout_ms", eot_timeout_ms)
                .field("max_speakers", max_speakers)
                .finish(),
            Self::AssemblyAI {
                api_key,
                enable_diarization,
            } => f
                .debug_struct("AssemblyAI")
                .field(
                    "api_key",
                    &crate::credentials::redacted_secret_presence(Some(api_key)),
                )
                .field("enable_diarization", enable_diarization)
                .finish(),
            Self::Soniox {
                api_key,
                model,
                enable_diarization,
                enable_language_identification,
                language_hints,
                max_speakers,
            } => f
                .debug_struct("Soniox")
                .field(
                    "api_key",
                    &crate::credentials::redacted_secret_presence(Some(api_key)),
                )
                .field("model", model)
                .field("enable_diarization", enable_diarization)
                .field(
                    "enable_language_identification",
                    enable_language_identification,
                )
                .field("language_hints", language_hints)
                .field("max_speakers", max_speakers)
                .finish(),
            Self::SherpaOnnx {
                model_dir,
                enable_endpoint_detection,
            } => f
                .debug_struct("SherpaOnnx")
                .field("model_dir", model_dir)
                .field("enable_endpoint_detection", enable_endpoint_detection)
                .finish(),
            Self::Moonshine {
                model_dir,
                enable_speaker_hints,
            } => f
                .debug_struct("Moonshine")
                .field("model_dir", model_dir)
                .field("enable_speaker_hints", enable_speaker_hints)
                .finish(),
            Self::OpenAiRealtimeTranscription {
                api_key,
                model,
                language,
            } => f
                .debug_struct("OpenAiRealtimeTranscription")
                .field(
                    "api_key",
                    &crate::credentials::redacted_secret_presence(Some(api_key)),
                )
                .field("model", model)
                .field("language", language)
                .finish(),
        }
    }
}

pub fn endpoint_is_loopback(endpoint: &str) -> bool {
    let Ok(parsed) = url::Url::parse(endpoint.trim()) else {
        return false;
    };
    parsed.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    })
}

impl AsrProvider {
    pub fn runtime_provider_id(&self) -> &'static str {
        match self {
            AsrProvider::LocalWhisper => "asr.local_whisper",
            AsrProvider::Api { .. } => "asr.api",
            AsrProvider::AwsTranscribe { .. } => "asr.aws_transcribe",
            AsrProvider::DeepgramStreaming { .. } => "asr.deepgram",
            AsrProvider::AssemblyAI { .. } => "asr.assemblyai",
            AsrProvider::Soniox { .. } => "asr.soniox",
            AsrProvider::SherpaOnnx { .. } => "asr.sherpa_onnx",
            AsrProvider::Moonshine { .. } => "asr.moonshine",
            AsrProvider::OpenAiRealtimeTranscription { .. } => "asr.openai_realtime",
        }
    }

    pub fn requires_cloud_content_transfer(&self) -> bool {
        match self {
            AsrProvider::LocalWhisper
            | AsrProvider::SherpaOnnx { .. }
            | AsrProvider::Moonshine { .. } => false,
            AsrProvider::Api { endpoint, .. } => !endpoint_is_loopback(endpoint),
            AsrProvider::AwsTranscribe { .. }
            | AsrProvider::DeepgramStreaming { .. }
            | AsrProvider::AssemblyAI { .. }
            | AsrProvider::Soniox { .. }
            | AsrProvider::OpenAiRealtimeTranscription { .. } => true,
        }
    }
}

// ---------------------------------------------------------------------------
// LLM API config
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LlmApiConfig {
    pub endpoint: String,
    #[serde(default)]
    #[serde(skip_serializing)]
    #[schemars(skip)]
    pub api_key: Option<String>,
    pub model: String,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default = "default_temperature")]
    pub temperature: f32,
}

impl std::fmt::Debug for LlmApiConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlmApiConfig")
            .field("endpoint", &self.endpoint)
            .field(
                "api_key",
                &crate::credentials::redacted_secret_presence(self.api_key.as_deref()),
            )
            .field("model", &self.model)
            .field("max_tokens", &self.max_tokens)
            .field("temperature", &self.temperature)
            .finish()
    }
}

fn default_max_tokens() -> u32 {
    2048
}
fn default_temperature() -> f32 {
    0.7
}

// ---------------------------------------------------------------------------
// LLM provider
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type")]
pub enum LlmProvider {
    #[serde(rename = "local_llama")]
    LocalLlama,
    #[serde(rename = "api")]
    Api {
        endpoint: String,
        #[serde(default, skip_serializing)]
        #[schemars(skip)]
        api_key: String,
        model: String,
    },
    /// First-class OpenRouter provider (ADR-0005). Distinct from the generic
    /// `Api` variant so the UI surfaces an "OpenRouter" entry, the credentials
    /// allowlist gets a dedicated `openrouter_api_key` slot, and the
    /// test-connection command can validate against `/api/v1/models` without
    /// firing a chat completion. Streaming chat is plan A3 / ADR-0006.
    #[serde(rename = "openrouter")]
    OpenRouter {
        /// OpenRouter model slug, e.g. `"anthropic/claude-sonnet-4.5"`. Empty
        /// until the user picks one in the settings model picker; the UI must
        /// enforce non-empty before save.
        #[serde(default)]
        model: String,
        #[serde(default = "default_openrouter_base_url")]
        base_url: String,
        #[serde(default)]
        provider_order: Option<Vec<String>>,
        #[serde(default = "default_true")]
        include_usage_in_stream: bool,
        /// Bearer token. Persisted in `credentials.yaml` under
        /// `openrouter_api_key`; this field stays empty in `config.yaml` and
        /// is hydrated at runtime by [`hydrate_runtime_credentials`].
        #[serde(default, skip_serializing)]
        #[schemars(skip)]
        api_key: String,
    },
    #[serde(rename = "aws_bedrock")]
    AwsBedrock {
        #[serde(default = "default_aws_region")]
        region: String,
        model_id: String,
        #[serde(default)]
        credential_source: AwsCredentialSource,
    },
    #[serde(rename = "mistralrs")]
    MistralRs {
        #[serde(default = "default_mistralrs_model")]
        model_id: String,
    },
}

impl std::fmt::Debug for LlmProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LocalLlama => f.write_str("LocalLlama"),
            Self::Api {
                endpoint,
                api_key,
                model,
            } => f
                .debug_struct("Api")
                .field("endpoint", endpoint)
                .field(
                    "api_key",
                    &crate::credentials::redacted_secret_presence(Some(api_key)),
                )
                .field("model", model)
                .finish(),
            Self::OpenRouter {
                model,
                base_url,
                provider_order,
                include_usage_in_stream,
                api_key,
            } => f
                .debug_struct("OpenRouter")
                .field("model", model)
                .field("base_url", base_url)
                .field("provider_order", provider_order)
                .field("include_usage_in_stream", include_usage_in_stream)
                .field(
                    "api_key",
                    &crate::credentials::redacted_secret_presence(Some(api_key)),
                )
                .finish(),
            Self::AwsBedrock {
                region,
                model_id,
                credential_source,
            } => f
                .debug_struct("AwsBedrock")
                .field("region", region)
                .field("model_id", model_id)
                .field("credential_source", credential_source)
                .finish(),
            Self::MistralRs { model_id } => f
                .debug_struct("MistralRs")
                .field("model_id", model_id)
                .finish(),
        }
    }
}

impl LlmProvider {
    /// Whether this backend can honor the [`AppSettings::streaming_prefill`]
    /// setting. Only the in-process llama.cpp engine exposes the prefill/decode
    /// control required to warm the KV cache from streaming transcript and defer
    /// decode to the turn boundary (see ADR-0012). mistral.rs's public API is
    /// atomic (prompt in → completion out) and remote/OpenAI-compatible
    /// endpoints expose no prefill hook, so both ignore the setting.
    pub fn supports_streaming_prefill(&self) -> bool {
        matches!(self, LlmProvider::LocalLlama)
    }

    pub fn runtime_provider_id(&self) -> &'static str {
        match self {
            LlmProvider::LocalLlama => "llm.local_llama",
            LlmProvider::Api { .. } => "llm.api",
            LlmProvider::OpenRouter { .. } => "llm.openrouter",
            LlmProvider::AwsBedrock { .. } => "llm.aws_bedrock",
            LlmProvider::MistralRs { .. } => "llm.mistralrs",
        }
    }

    pub fn requires_cloud_content_transfer(&self) -> bool {
        match self {
            LlmProvider::LocalLlama | LlmProvider::MistralRs { .. } => false,
            LlmProvider::Api { endpoint, .. } => !endpoint_is_loopback(endpoint),
            LlmProvider::OpenRouter { .. } | LlmProvider::AwsBedrock { .. } => true,
        }
    }
}

fn default_openrouter_base_url() -> String {
    crate::llm::openrouter::DEFAULT_BASE_URL.to_string()
}

fn default_mistralrs_model() -> String {
    crate::models::LLM_MODEL_FILENAME.to_string()
}

impl Default for LlmProvider {
    fn default() -> Self {
        Self::Api {
            endpoint: "http://localhost:11434/v1".to_string(),
            api_key: String::new(),
            model: "llama3.2".to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// TTS provider
// ---------------------------------------------------------------------------

/// Cloud / local TTS provider selection.
///
/// Mirrors the `AsrProvider` / `LlmProvider` shape so the frontend can render
/// a settings dropdown with the same conventions. Per ADR-0004 the v1 ship-
/// list is `None` (TTS off) and `DeepgramAura`; local engines (Kokoro, Piper,
/// Coqui) are explicitly out of scope for plan A1 and will land as new
/// variants in their own plans.
///
/// The Deepgram API key for `DeepgramAura` reuses the `deepgram_api_key`
/// credential slot already used by `AsrProvider::DeepgramStreaming` -- the
/// same key works for both STT and TTS, so we don't introduce a separate
/// `deepgram_tts_api_key`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type")]
pub enum TtsProvider {
    /// TTS disabled. The chat reply path stays text-only.
    #[serde(rename = "none")]
    #[default]
    None,
    /// Deepgram Aura streaming TTS (cloud). Voices are a fixed list per the
    /// Aura docs; the frontend exposes them as a TS constant rather than
    /// fetching them dynamically.
    #[serde(rename = "deepgram_aura")]
    DeepgramAura {
        /// Aura voice id, e.g. `aura-asteria-en` or `aura-2-thalia-en`.
        #[serde(default = "default_aura_voice")]
        voice: String,
        /// PCM sample rate in Hz. Aura streaming default is 24000.
        #[serde(default = "default_aura_sample_rate")]
        sample_rate: u32,
        /// Speed multiplier (Aura accepts 0.7..=1.5). Persisted unclamped;
        /// the runtime clamps before sending to the wire.
        #[serde(default = "default_aura_speed")]
        speed: f32,
    },
}

impl TtsProvider {
    pub fn runtime_provider_id(&self) -> &'static str {
        match self {
            TtsProvider::None => "tts.none",
            TtsProvider::DeepgramAura { .. } => "tts.deepgram_aura",
        }
    }

    pub fn requires_cloud_content_transfer(&self) -> bool {
        match self {
            TtsProvider::None => false,
            TtsProvider::DeepgramAura { .. } => true,
        }
    }
}

fn default_aura_voice() -> String {
    "aura-asteria-en".to_string()
}
fn default_aura_sample_rate() -> u32 {
    24_000
}
fn default_aura_speed() -> f32 {
    1.0
}

// ---------------------------------------------------------------------------
// Audio settings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AudioSettings {
    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,
    #[serde(default = "default_channels")]
    pub channels: u16,
}

fn default_sample_rate() -> u32 {
    configured_sample_rate().unwrap_or(FALLBACK_SAMPLE_RATE)
}
fn default_channels() -> u16 {
    configured_channels().unwrap_or(FALLBACK_CHANNELS)
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            sample_rate: default_sample_rate(),
            channels: default_channels(),
        }
    }
}

/// Whitelist of sample rates the capture backend (rsac) is willing to request.
///
/// rsac can resample to most rates, but we cap the UI at common studio /
/// telephony values so a hand-edited `config.yaml` can't coax us into
/// trying something absurd (e.g. `4` Hz or `u32::MAX`). If the persisted
/// value falls outside this set we log a warn and fall back to the default
/// rather than panicking mid-capture — the worst case is the user sees their
/// custom rate ignored until they revisit Settings.
pub fn sample_rate_is_valid(hz: u32) -> bool {
    matches!(hz, 22050 | 32000 | 44100 | 48000 | 88200 | 96000)
}

/// Whitelist of channel counts. Pipeline downmixes to mono regardless, so
/// only mono and stereo capture are meaningful; anything else would just
/// waste CPU on extra channels the ASR stage throws away.
pub fn channels_is_valid(ch: u16) -> bool {
    matches!(ch, 1 | 2)
}

/// Resolve the effective `(sample_rate, channels)` for capture, applying
/// the validation whitelist and falling back to defaults with a warn log
/// when a persisted value is out of range. Returns the fully-validated
/// pair so call sites don't need to think about fallback behavior.
pub fn resolve_audio_settings(settings: &AudioSettings) -> (u32, u16) {
    let sr = if sample_rate_is_valid(settings.sample_rate) {
        settings.sample_rate
    } else {
        log::warn!(
            "Invalid persisted sample_rate {} Hz — falling back to default {} Hz. \
             Edit Settings → Audio to pick a supported rate.",
            settings.sample_rate,
            default_sample_rate()
        );
        default_sample_rate()
    };
    let ch = if channels_is_valid(settings.channels) {
        settings.channels
    } else {
        log::warn!(
            "Invalid persisted channels count {} — falling back to default {}. \
             Edit Settings → Audio to pick 1 (mono) or 2 (stereo).",
            settings.channels,
            default_channels()
        );
        default_channels()
    };
    (sr, ch)
}

// ---------------------------------------------------------------------------
// Gemini auth mode + settings
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type")]
pub enum GeminiAuthMode {
    #[serde(rename = "api_key")]
    ApiKey {
        #[serde(default, skip_serializing)]
        #[schemars(skip)]
        api_key: String,
    },
    #[serde(rename = "vertex_ai")]
    VertexAI {
        project_id: String,
        location: String,
        #[serde(default, skip_serializing)]
        #[schemars(skip)]
        service_account_path: Option<String>,
    },
}

impl std::fmt::Debug for GeminiAuthMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ApiKey { api_key } => f
                .debug_struct("ApiKey")
                .field(
                    "api_key",
                    &crate::credentials::redacted_secret_presence(Some(api_key)),
                )
                .finish(),
            Self::VertexAI {
                project_id,
                location,
                service_account_path,
            } => f
                .debug_struct("VertexAI")
                .field("project_id", project_id)
                .field("location", location)
                .field(
                    "service_account_path",
                    &crate::credentials::redacted_secret_presence(service_account_path.as_deref()),
                )
                .finish(),
        }
    }
}

impl Default for GeminiAuthMode {
    fn default() -> Self {
        Self::ApiKey {
            api_key: String::new(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GeminiSettings {
    #[serde(default)]
    pub auth: GeminiAuthMode,
    #[serde(default = "default_gemini_model")]
    pub model: String,
    /// Prebuilt voice for converse-mode AUDIO sessions (B18 / ADR-0018). Empty
    /// falls back to the engine default (`gemini::DEFAULT_GEMINI_VOICE`).
    /// Ignored by the notes/graph TEXT pipeline.
    #[serde(default)]
    pub voice: String,
}

impl std::fmt::Debug for GeminiSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeminiSettings")
            .field("auth", &self.auth)
            .field("model", &self.model)
            .field("voice", &self.voice)
            .finish()
    }
}

fn default_gemini_model() -> String {
    "gemini-2.0-flash-live-001".to_string()
}

impl Default for GeminiSettings {
    fn default() -> Self {
        Self {
            auth: GeminiAuthMode::default(),
            model: default_gemini_model(),
            voice: String::new(),
        }
    }
}

impl GeminiSettings {
    /// Extract the API key from auth mode (convenience for backward compat).
    pub fn api_key(&self) -> String {
        match &self.auth {
            GeminiAuthMode::ApiKey { api_key } => api_key.clone(),
            GeminiAuthMode::VertexAI { .. } => String::new(),
        }
    }
}

/// Authentication mode for the OpenAI Realtime **voice agent** (S2S) provider.
///
/// OpenAI Realtime authenticates with a single Bearer API key
/// (`openai_api_key`). Modeled as a tagged enum to mirror [`GeminiAuthMode`]'s
/// shape (so the settings UI + readiness plumbing can branch uniformly) while
/// keeping room for a future ephemeral-token mode without a breaking change.
#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type")]
pub enum OpenAiRealtimeAgentAuthMode {
    #[serde(rename = "api_key")]
    ApiKey {
        #[serde(default, skip_serializing)]
        #[schemars(skip)]
        api_key: String,
    },
}

impl std::fmt::Debug for OpenAiRealtimeAgentAuthMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ApiKey { api_key } => f
                .debug_struct("ApiKey")
                .field(
                    "api_key",
                    &crate::credentials::redacted_secret_presence(Some(api_key)),
                )
                .finish(),
        }
    }
}

impl Default for OpenAiRealtimeAgentAuthMode {
    fn default() -> Self {
        Self::ApiKey {
            api_key: String::new(),
        }
    }
}

fn default_openai_realtime_agent_model() -> String {
    "gpt-realtime-2".to_string()
}

/// Settings for the OpenAI Realtime cloud-native S2S voice agent
/// (`realtime_agent.openai_realtime`). Mirrors [`GeminiSettings`] (auth, model,
/// voice) so the converse-mode native-engine selector can offer OpenAI as a
/// sibling of Gemini Live. The `api_key` lives only at runtime (hydrated from
/// `credentials.yaml`), never persisted in settings.
#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct OpenAiRealtimeAgentSettings {
    #[serde(default)]
    pub auth: OpenAiRealtimeAgentAuthMode,
    #[serde(default = "default_openai_realtime_agent_model")]
    pub model: String,
    /// Prebuilt voice for the S2S session (empty falls back to the engine
    /// default, `openai_realtime::DEFAULT_VOICE`).
    #[serde(default)]
    pub voice: String,
}

impl std::fmt::Debug for OpenAiRealtimeAgentSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiRealtimeAgentSettings")
            .field("auth", &self.auth)
            .field("model", &self.model)
            .field("voice", &self.voice)
            .finish()
    }
}

impl Default for OpenAiRealtimeAgentSettings {
    fn default() -> Self {
        Self {
            auth: OpenAiRealtimeAgentAuthMode::default(),
            model: default_openai_realtime_agent_model(),
            voice: String::new(),
        }
    }
}

impl OpenAiRealtimeAgentSettings {
    /// Extract the API key from auth mode.
    pub fn api_key(&self) -> String {
        match &self.auth {
            OpenAiRealtimeAgentAuthMode::ApiKey { api_key } => api_key.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum DiarizationMode {
    Off,
    #[default]
    Provider,
    Local,
    Hybrid,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum DiarizationSpeakerCount {
    #[default]
    Auto,
    Fixed,
    Unbounded,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq, Default)]
pub struct DiarizationSettings {
    #[serde(default)]
    pub mode: DiarizationMode,
    #[serde(default)]
    pub speaker_count: DiarizationSpeakerCount,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_speakers: Option<u32>,
}

impl DiarizationSettings {
    /// Whether provider-native diarization should run for a provider-specific
    /// `enable_diarization` flag. The provider flag remains the provider's
    /// detailed control, while the global policy is the top-level gate.
    pub fn provider_diarization_enabled(&self, provider_requested: bool) -> bool {
        provider_requested
            && matches!(
                self.mode,
                DiarizationMode::Provider | DiarizationMode::Hybrid
            )
    }

    /// Effective provider max-speaker cap. Providers use `0` as uncapped where
    /// supported; fixed mode needs a positive cap even if a hand-written config
    /// omitted `max_speakers`.
    pub fn provider_max_speakers(&self, provider_max_speakers: u32) -> u32 {
        match self.speaker_count {
            DiarizationSpeakerCount::Auto => provider_max_speakers,
            DiarizationSpeakerCount::Unbounded => 0,
            DiarizationSpeakerCount::Fixed => self.max_speakers.unwrap_or(1).max(1),
        }
    }
}

impl AsrProvider {
    /// Apply the global diarization policy before runtime startup. This keeps
    /// backend behavior aligned with `config.yaml` even if stale provider-level
    /// booleans survive from older settings files or manual edits.
    pub fn apply_diarization_settings(&mut self, policy: &DiarizationSettings) {
        match self {
            AsrProvider::AwsTranscribe {
                enable_diarization, ..
            }
            | AsrProvider::AssemblyAI {
                enable_diarization, ..
            } => {
                *enable_diarization = policy.provider_diarization_enabled(*enable_diarization);
            }
            AsrProvider::Soniox {
                enable_diarization,
                max_speakers,
                ..
            } => {
                *enable_diarization = policy.provider_diarization_enabled(*enable_diarization);
                *max_speakers = policy.provider_max_speakers(*max_speakers);
            }
            AsrProvider::DeepgramStreaming {
                enable_diarization,
                max_speakers,
                ..
            } => {
                *enable_diarization = policy.provider_diarization_enabled(*enable_diarization);
                *max_speakers = policy.provider_max_speakers(*max_speakers);
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Runtime privacy mode
// ---------------------------------------------------------------------------

/// Cross-cutting privacy gate controlling whether session content (audio,
/// transcript text, prompts) may leave the machine.
///
/// This is the single most security-relevant setting: every content-egress
/// provider derives a [`crate::asr::ProviderContentEgressPolicy`] from it (via
/// [`ProviderContentEgressPolicy::from_privacy_mode`](crate::asr::ProviderContentEgressPolicy::from_privacy_mode)),
/// and only [`Self::ByokCloud`] permits content transfer
/// ([`allows_session_cloud_content_transfer`](Self::allows_session_cloud_content_transfer)).
/// Readiness/connectivity probes that send no content are always allowed
/// ([`allows_no_content_provider_probe`](Self::allows_no_content_provider_probe)).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum PrivacyMode {
    /// No session content may leave the machine — cloud providers are blocked
    /// from receiving audio/text/prompts. Local engines only.
    LocalOnly,
    /// Bring-your-own-key cloud: the **only** mode that permits session content
    /// egress to cloud providers. The default — content transfer is allowed
    /// because the user supplied their own provider keys.
    #[default]
    ByokCloud,
    /// Cloud providers may be configured and readiness-probed, but no session
    /// content is transferred (content egress blocked, no-content probes allowed).
    CloudDisabledReadinessOnly,
    /// Org-managed promotion mode. Treated like a non-`ByokCloud` mode for the
    /// content gate: session content egress is blocked.
    OrgPromotion,
}

impl PrivacyMode {
    pub fn as_str(self) -> &'static str {
        match self {
            PrivacyMode::LocalOnly => "local_only",
            PrivacyMode::ByokCloud => "byok_cloud",
            PrivacyMode::CloudDisabledReadinessOnly => "cloud_disabled_readiness_only",
            PrivacyMode::OrgPromotion => "org_promotion",
        }
    }

    pub fn allows_session_cloud_content_transfer(self) -> bool {
        matches!(self, PrivacyMode::ByokCloud)
    }

    pub fn allows_no_content_provider_probe(self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// Top-level settings
// ---------------------------------------------------------------------------

/// Persisted user configuration — the root of the `config.yaml` schema.
///
/// Loaded at startup ([`load_settings`]) and cached in `AppState` hydrated with
/// runtime-only credentials ([`hydrate_runtime_credentials`]); written back via
/// [`save_settings`] with inline secrets redacted ([`redacted_settings`]). The
/// public JSON Schema for this type is generated by
/// [`public_app_settings_schema_json`]. Secret fields (provider `api_key`s) are
/// `#[serde(skip_serializing)]` so they never land in `config.yaml`; they live
/// in the credential backend ([`crate::credentials`]) instead.
///
/// Diagnostics are governed by two **independent** opt-in/opt-out toggles:
/// [`analytics_enabled`](Self::analytics_enabled) (anonymous Sentry, opt-in,
/// default off) and [`file_logging`](Self::file_logging) (local log tee,
/// default on). The [`privacy_mode`](Self::privacy_mode) field is the
/// cross-cutting cloud-content-egress gate — see [`PrivacyMode`].
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AppSettings {
    #[serde(default)]
    pub asr_provider: AsrProvider,
    #[serde(default = "default_whisper_model")]
    pub whisper_model: String,
    #[serde(default)]
    pub llm_provider: LlmProvider,
    /// Rich OpenRouter provider-routing policy. This is separate from the
    /// legacy `LlmProvider::OpenRouter.provider_order` compatibility field;
    /// runtime OpenRouter request construction prefers this policy when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openrouter_routing_policy: Option<OpenRouterRoutingPolicy>,
    #[serde(default)]
    pub llm_api_config: Option<LlmApiConfig>,
    #[serde(default)]
    pub audio_settings: AudioSettings,
    #[serde(default)]
    pub gemini: GeminiSettings,
    /// OpenAI Realtime cloud-native S2S voice agent settings
    /// (`realtime_agent.openai_realtime`). Matches the registry descriptor's
    /// `settings_variant: "openai_realtime_agent"`. Sibling of `gemini` for the
    /// converse-mode native engine.
    #[serde(default)]
    pub openai_realtime_agent: OpenAiRealtimeAgentSettings,
    #[serde(default)]
    pub diarization: DiarizationSettings,
    #[serde(default)]
    pub privacy_mode: PrivacyMode,
    /// Selected TTS provider. Default `None` keeps the chat reply path
    /// text-only and avoids introducing a backend dependency on cloud TTS
    /// for users who don't want it. See plan A1 + ADR-0004.
    #[serde(default)]
    pub tts_provider: TtsProvider,
    /// Speak chat replies aloud through the configured TTS provider.
    /// Default `false` — the speak-aloud loop is opt-in. When true and
    /// `tts_provider` is not `None`, each streaming chat reply is also
    /// piped through the TTS provider (clause-boundary flushing) and out
    /// the audio playback subsystem. Has no effect when `tts_provider`
    /// is `None`. (Wave C / audio-graph-92c7.)
    #[serde(default)]
    pub speak_aloud: bool,
    /// Enable streaming / incremental prefill on **supported local LLM
    /// backends** (currently llama.cpp only — see
    /// [`LlmProvider::supports_streaming_prefill`]). When true and the active
    /// provider supports it, the local extraction engine warms the KV cache
    /// with transcript as it streams in and defers decode until the turn
    /// boundary, lowering post-turn latency (ADR-0012). Default `false`
    /// (opt-in). No effect for providers that don't support it (mistral.rs,
    /// remote/API) — the flag is simply ignored there.
    #[serde(default)]
    pub streaming_prefill: bool,
    /// Runtime log-verbosity preference: one of
    /// "off" | "error" | "warn" | "info" | "debug" | "trace".
    ///
    /// `None` means "not set — fall back to the default (info) unless
    /// the user set RUST_LOG at startup". `skip_serializing_if` keeps the
    /// field out of written YAML/JSON when unset, so older settings files
    /// stay byte-identical after a round-trip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_level: Option<String>,
    /// Write logs to a rotating file under the app config dir's `logs/`.
    /// `None` means "use the default" (enabled). When enabled, the logger
    /// also tees every `log::*` record to the file so test sessions can be
    /// attached as feedback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_logging: Option<bool>,
    /// How the log file is initialized at startup: "archive" (default —
    /// rename the previous log to a timestamped file, then append to a fresh
    /// one) or "overwrite" (truncate the single log file each launch).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_file_mode: Option<String>,
    /// Demo mode — set once on first launch when no cloud credentials are
    /// present. `None` means "not yet decided" (the setup hook will make
    /// the call on the next launch); `Some(true)` means the app is wired
    /// for local-only providers and should show the demo banner until
    /// local models are downloaded; `Some(false)` means the user has
    /// configured something real (either via ExpressSetup or directly).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub demo_mode: Option<bool>,
    /// OPT-IN anonymous diagnostics analytics (Sentry). `Some(false)` /
    /// `None` means OFF — the feature stays disabled until the user explicitly
    /// turns it on. When `Some(true)`, the app initializes the anonymous,
    /// PII-stripped analytics channel ([`crate::analytics`]) at startup. This
    /// is fully independent of `file_logging` (local logs) and of the local
    /// crash handler — any combination is valid. No transcripts, audio,
    /// credentials, or IPs are ever sent (see `crate::analytics` privacy gate).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analytics_enabled: Option<bool>,
}

fn default_whisper_model() -> String {
    configured_whisper_model().unwrap_or_else(|| FALLBACK_WHISPER_MODEL.to_string())
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            asr_provider: AsrProvider::default(),
            whisper_model: default_whisper_model(),
            llm_provider: LlmProvider::default(),
            openrouter_routing_policy: None,
            llm_api_config: None,
            audio_settings: AudioSettings::default(),
            gemini: GeminiSettings::default(),
            openai_realtime_agent: OpenAiRealtimeAgentSettings::default(),
            diarization: DiarizationSettings::default(),
            privacy_mode: PrivacyMode::default(),
            tts_provider: TtsProvider::default(),
            speak_aloud: false,
            streaming_prefill: false,
            log_level: Some("info".to_string()),
            file_logging: Some(true),
            log_file_mode: Some("archive".to_string()),
            demo_mode: None,
            analytics_enabled: Some(false),
        }
    }
}

/// Generate the public settings JSON Schema.
///
/// This is the inspectable contract for `config.yaml` and redacted settings
/// IPC. Runtime credential fields are intentionally omitted via
/// `#[schemars(skip)]`; the credential backend has its own private schema.
pub fn public_app_settings_schema_json() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(AppSettings))
        .expect("AppSettings JSON Schema must serialize")
}

fn non_empty_secret(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn option_non_empty_secret(value: &Option<String>) -> Option<&str> {
    value.as_deref().and_then(non_empty_secret)
}

pub const CEREBRAS_BASE_URL: &str = "https://api.cerebras.ai/v1";

pub fn is_cerebras_endpoint(endpoint: &str) -> bool {
    let normalized = endpoint.trim().trim_end_matches('/').to_ascii_lowercase();
    normalized == CEREBRAS_BASE_URL
}

/// Pick the credential slot for OpenAI-compatible HTTP providers.
///
/// Settings only store routing details such as endpoint/model. Secrets live in
/// `credentials.yaml`; for OpenAI-compatible providers the endpoint is the
/// stable provider discriminator we have available at runtime.
pub fn credential_key_for_endpoint(endpoint: &str) -> &'static str {
    let lower = endpoint.to_ascii_lowercase();
    if is_cerebras_endpoint(endpoint) {
        "cerebras_api_key"
    } else if lower.contains("openrouter") {
        "openrouter_api_key"
    } else if lower.contains("generativelanguage.googleapis.com") || lower.contains("gemini") {
        "gemini_api_key"
    } else if lower.contains("groq") {
        "groq_api_key"
    } else if lower.contains("together") {
        "together_api_key"
    } else if lower.contains("fireworks") {
        "fireworks_api_key"
    } else {
        // OpenAI, Anthropic-compatible shims, vLLM with auth, and unknown
        // OpenAI-compatible endpoints share the generic bearer slot.
        "openai_api_key"
    }
}

fn credential_value_for_endpoint<'a>(
    endpoint: &str,
    store: &'a crate::credentials::CredentialStore,
) -> Option<&'a str> {
    match credential_key_for_endpoint(endpoint) {
        "cerebras_api_key" => option_non_empty_secret(&store.cerebras_api_key),
        "openrouter_api_key" => option_non_empty_secret(&store.openrouter_api_key),
        "gemini_api_key" => option_non_empty_secret(&store.gemini_api_key),
        "groq_api_key" => option_non_empty_secret(&store.groq_api_key),
        "together_api_key" => option_non_empty_secret(&store.together_api_key),
        "fireworks_api_key" => option_non_empty_secret(&store.fireworks_api_key),
        _ => option_non_empty_secret(&store.openai_api_key),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InlineCredentialUpdate {
    key: &'static str,
    value: String,
}

fn push_secret_if_present(
    updates: &mut Vec<InlineCredentialUpdate>,
    key: &'static str,
    value: &str,
) {
    if let Some(secret) = non_empty_secret(value) {
        updates.push(InlineCredentialUpdate {
            key,
            value: secret.to_string(),
        });
    }
}

fn push_secret_option_if_present(
    updates: &mut Vec<InlineCredentialUpdate>,
    key: &'static str,
    value: &Option<String>,
) {
    if let Some(secret) = option_non_empty_secret(value) {
        push_secret_if_present(updates, key, secret);
    }
}

fn push_aws_access_key_credentials(
    updates: &mut Vec<InlineCredentialUpdate>,
    access_key: &str,
    secret_key: &Option<String>,
    session_token: &Option<String>,
) {
    push_secret_if_present(updates, "aws_access_key", access_key);
    push_secret_option_if_present(updates, "aws_secret_key", secret_key);
    push_secret_option_if_present(updates, "aws_session_token", session_token);
}

fn aws_access_key_credentials_have_inline_secret(
    access_key: &str,
    secret_key: &Option<String>,
    session_token: &Option<String>,
) -> bool {
    non_empty_secret(access_key).is_some()
        || option_non_empty_secret(secret_key).is_some()
        || option_non_empty_secret(session_token).is_some()
}

fn redact_aws_access_key_credentials(credential_source: &mut AwsCredentialSource) {
    if let AwsCredentialSource::AccessKeys {
        access_key,
        secret_key,
        session_token,
    } = credential_source
    {
        access_key.clear();
        *secret_key = None;
        *session_token = None;
    }
}

fn inline_credential_updates(settings: &AppSettings) -> Vec<InlineCredentialUpdate> {
    let mut updates = Vec::new();

    match &settings.asr_provider {
        AsrProvider::Api {
            endpoint, api_key, ..
        } => push_secret_if_present(&mut updates, credential_key_for_endpoint(endpoint), api_key),
        AsrProvider::DeepgramStreaming { api_key, .. } => {
            push_secret_if_present(&mut updates, "deepgram_api_key", api_key)
        }
        AsrProvider::AssemblyAI { api_key, .. } => {
            push_secret_if_present(&mut updates, "assemblyai_api_key", api_key)
        }
        AsrProvider::Soniox { api_key, .. } => {
            push_secret_if_present(&mut updates, "soniox_api_key", api_key)
        }
        AsrProvider::OpenAiRealtimeTranscription { api_key, .. } => {
            push_secret_if_present(&mut updates, "openai_api_key", api_key)
        }
        AsrProvider::AwsTranscribe {
            credential_source, ..
        } => {
            if let AwsCredentialSource::AccessKeys {
                access_key,
                secret_key,
                session_token,
            } = credential_source
            {
                push_aws_access_key_credentials(
                    &mut updates,
                    access_key,
                    secret_key,
                    session_token,
                );
            }
        }
        AsrProvider::LocalWhisper
        | AsrProvider::SherpaOnnx { .. }
        | AsrProvider::Moonshine { .. } => {}
    }

    match &settings.llm_provider {
        LlmProvider::Api {
            endpoint, api_key, ..
        } => push_secret_if_present(&mut updates, credential_key_for_endpoint(endpoint), api_key),
        LlmProvider::OpenRouter { api_key, .. } => {
            push_secret_if_present(&mut updates, "openrouter_api_key", api_key);
        }
        LlmProvider::AwsBedrock {
            credential_source, ..
        } => {
            if let AwsCredentialSource::AccessKeys {
                access_key,
                secret_key,
                session_token,
            } = credential_source
            {
                push_aws_access_key_credentials(
                    &mut updates,
                    access_key,
                    secret_key,
                    session_token,
                );
            }
        }
        LlmProvider::LocalLlama | LlmProvider::MistralRs { .. } => {}
    }

    if let Some(config) = &settings.llm_api_config
        && let Some(api_key) = option_non_empty_secret(&config.api_key)
    {
        push_secret_if_present(
            &mut updates,
            credential_key_for_endpoint(&config.endpoint),
            api_key,
        );
    }

    match &settings.gemini.auth {
        GeminiAuthMode::ApiKey { api_key } => {
            push_secret_if_present(&mut updates, "gemini_api_key", api_key);
        }
        GeminiAuthMode::VertexAI {
            service_account_path,
            ..
        } => push_secret_option_if_present(
            &mut updates,
            "google_service_account_path",
            service_account_path,
        ),
    }

    updates
}

/// Persist any legacy inline settings secrets into `credentials.yaml`.
///
/// This is intentionally tolerant of empty fields: empty values mean "no new
/// secret supplied" and must not wipe an existing credential.
pub fn persist_inline_credentials(settings: &AppSettings) -> Result<(), String> {
    for update in inline_credential_updates(settings) {
        crate::credentials::set_credential(update.key, &update.value)
            .map_err(|e| format!("Failed to save {}: {e}", update.key))?;
    }
    Ok(())
}

pub fn has_inline_credentials(settings: &AppSettings) -> bool {
    let asr_has_secret = match &settings.asr_provider {
        AsrProvider::Api { api_key, .. }
        | AsrProvider::DeepgramStreaming { api_key, .. }
        | AsrProvider::AssemblyAI { api_key, .. }
        | AsrProvider::Soniox { api_key, .. }
        | AsrProvider::OpenAiRealtimeTranscription { api_key, .. } => {
            non_empty_secret(api_key).is_some()
        }
        AsrProvider::AwsTranscribe {
            credential_source, ..
        } => match credential_source {
            AwsCredentialSource::AccessKeys {
                access_key,
                secret_key,
                session_token,
            } => {
                aws_access_key_credentials_have_inline_secret(access_key, secret_key, session_token)
            }
            AwsCredentialSource::DefaultChain | AwsCredentialSource::Profile { .. } => false,
        },
        AsrProvider::LocalWhisper
        | AsrProvider::SherpaOnnx { .. }
        | AsrProvider::Moonshine { .. } => false,
    };

    let llm_has_secret = match &settings.llm_provider {
        LlmProvider::Api { api_key, .. } | LlmProvider::OpenRouter { api_key, .. } => {
            non_empty_secret(api_key).is_some()
        }
        LlmProvider::AwsBedrock {
            credential_source, ..
        } => match credential_source {
            AwsCredentialSource::AccessKeys {
                access_key,
                secret_key,
                session_token,
            } => {
                aws_access_key_credentials_have_inline_secret(access_key, secret_key, session_token)
            }
            AwsCredentialSource::DefaultChain | AwsCredentialSource::Profile { .. } => false,
        },
        LlmProvider::LocalLlama | LlmProvider::MistralRs { .. } => false,
    };

    let llm_config_has_secret = settings
        .llm_api_config
        .as_ref()
        .and_then(|config| option_non_empty_secret(&config.api_key))
        .is_some();

    let gemini_has_secret = match &settings.gemini.auth {
        GeminiAuthMode::ApiKey { api_key } => non_empty_secret(api_key).is_some(),
        GeminiAuthMode::VertexAI {
            service_account_path,
            ..
        } => option_non_empty_secret(service_account_path).is_some(),
    };

    let openai_realtime_agent_has_secret = match &settings.openai_realtime_agent.auth {
        OpenAiRealtimeAgentAuthMode::ApiKey { api_key } => non_empty_secret(api_key).is_some(),
    };

    asr_has_secret
        || llm_has_secret
        || llm_config_has_secret
        || gemini_has_secret
        || openai_realtime_agent_has_secret
}

/// Return a copy that is safe to write to `config.yaml` or return over IPC.
pub fn redacted_settings(settings: &AppSettings) -> AppSettings {
    let mut redacted = settings.clone();

    match &mut redacted.asr_provider {
        AsrProvider::Api { api_key, .. }
        | AsrProvider::DeepgramStreaming { api_key, .. }
        | AsrProvider::AssemblyAI { api_key, .. }
        | AsrProvider::Soniox { api_key, .. }
        | AsrProvider::OpenAiRealtimeTranscription { api_key, .. } => api_key.clear(),
        AsrProvider::AwsTranscribe {
            credential_source, ..
        } => {
            redact_aws_access_key_credentials(credential_source);
        }
        AsrProvider::LocalWhisper
        | AsrProvider::SherpaOnnx { .. }
        | AsrProvider::Moonshine { .. } => {}
    }

    match &mut redacted.llm_provider {
        LlmProvider::Api { api_key, .. } | LlmProvider::OpenRouter { api_key, .. } => {
            api_key.clear()
        }
        LlmProvider::AwsBedrock {
            credential_source, ..
        } => {
            redact_aws_access_key_credentials(credential_source);
        }
        LlmProvider::LocalLlama | LlmProvider::MistralRs { .. } => {}
    }

    if let Some(config) = &mut redacted.llm_api_config {
        config.api_key = None;
    }

    if let GeminiAuthMode::ApiKey { api_key } = &mut redacted.gemini.auth {
        api_key.clear();
    } else if let GeminiAuthMode::VertexAI {
        service_account_path,
        ..
    } = &mut redacted.gemini.auth
    {
        *service_account_path = None;
    }

    let OpenAiRealtimeAgentAuthMode::ApiKey { api_key } = &mut redacted.openai_realtime_agent.auth;
    api_key.clear();

    redacted
}

/// Return a runtime-only copy with secrets filled from `credentials.yaml`.
///
/// The returned value must not be serialized. It is stored in memory so the
/// capture/transcription/LLM paths can use existing provider structs without
/// reaching into the credential store at every call site.
pub fn hydrate_runtime_credentials(
    settings: &AppSettings,
    store: &crate::credentials::CredentialStore,
) -> AppSettings {
    let mut hydrated = redacted_settings(settings);

    match &mut hydrated.asr_provider {
        AsrProvider::Api {
            endpoint, api_key, ..
        } => {
            if let Some(secret) = credential_value_for_endpoint(endpoint, store) {
                *api_key = secret.to_string();
            }
        }
        AsrProvider::DeepgramStreaming { api_key, .. } => {
            if let Some(secret) = option_non_empty_secret(&store.deepgram_api_key) {
                *api_key = secret.to_string();
            }
        }
        AsrProvider::AssemblyAI { api_key, .. } => {
            if let Some(secret) = option_non_empty_secret(&store.assemblyai_api_key) {
                *api_key = secret.to_string();
            }
        }
        AsrProvider::Soniox { api_key, .. } => {
            if let Some(secret) = option_non_empty_secret(&store.soniox_api_key) {
                *api_key = secret.to_string();
            }
        }
        AsrProvider::OpenAiRealtimeTranscription { api_key, .. } => {
            if let Some(secret) = option_non_empty_secret(&store.openai_api_key) {
                *api_key = secret.to_string();
            }
        }
        AsrProvider::AwsTranscribe {
            credential_source, ..
        } => {
            if let AwsCredentialSource::AccessKeys { access_key, .. } = credential_source
                && let Some(secret) = option_non_empty_secret(&store.aws_access_key)
            {
                *access_key = secret.to_string();
            }
        }
        AsrProvider::LocalWhisper
        | AsrProvider::SherpaOnnx { .. }
        | AsrProvider::Moonshine { .. } => {}
    }

    match &mut hydrated.llm_provider {
        LlmProvider::Api {
            endpoint, api_key, ..
        } => {
            if let Some(secret) = credential_value_for_endpoint(endpoint, store) {
                *api_key = secret.to_string();
            }
        }
        LlmProvider::OpenRouter { api_key, .. } => {
            if let Some(secret) = option_non_empty_secret(&store.openrouter_api_key) {
                *api_key = secret.to_string();
            }
        }
        LlmProvider::AwsBedrock {
            credential_source, ..
        } => {
            if let AwsCredentialSource::AccessKeys { access_key, .. } = credential_source
                && let Some(secret) = option_non_empty_secret(&store.aws_access_key)
            {
                *access_key = secret.to_string();
            }
        }
        LlmProvider::LocalLlama | LlmProvider::MistralRs { .. } => {}
    }

    if let Some(config) = &mut hydrated.llm_api_config {
        config.api_key =
            credential_value_for_endpoint(&config.endpoint, store).map(|secret| secret.to_string());
    }

    if let GeminiAuthMode::ApiKey { api_key } = &mut hydrated.gemini.auth
        && let Some(secret) = option_non_empty_secret(&store.gemini_api_key)
    {
        *api_key = secret.to_string();
    } else if let GeminiAuthMode::VertexAI {
        service_account_path,
        ..
    } = &mut hydrated.gemini.auth
        && let Some(path) = option_non_empty_secret(&store.google_service_account_path)
    {
        *service_account_path = Some(path.to_string());
    }

    // OpenAI Realtime S2S voice agent shares the `openai_api_key` credential
    // (same key as the OpenAI Realtime STT transcription provider) — see the
    // credential mapping at commands.rs `realtime_agent.openai_realtime`.
    let OpenAiRealtimeAgentAuthMode::ApiKey { api_key } = &mut hydrated.openai_realtime_agent.auth;
    if let Some(secret) = option_non_empty_secret(&store.openai_api_key) {
        *api_key = secret.to_string();
    }

    hydrated
}

// ---------------------------------------------------------------------------
// Path resolution
// ---------------------------------------------------------------------------

pub fn get_settings_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("Failed to get app config directory: {}", e))?;
    Ok(config_dir.join("config.yaml"))
}

pub fn get_legacy_settings_json_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data directory: {}", e))?;
    Ok(data_dir.join("settings.json"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsLoadStatus {
    CanonicalOk,
    CanonicalErrorDefaulted,
    LegacyImported,
    LegacyErrorDefaulted,
    DefaultsMissing,
    PathErrorDefaulted,
}

impl SettingsLoadStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CanonicalOk => "canonical_ok",
            Self::CanonicalErrorDefaulted => "canonical_error_defaulted",
            Self::LegacyImported => "legacy_imported",
            Self::LegacyErrorDefaulted => "legacy_error_defaulted",
            Self::DefaultsMissing => "defaults_missing",
            Self::PathErrorDefaulted => "path_error_defaulted",
        }
    }

    pub fn allows_automatic_writeback(self) -> bool {
        !matches!(
            self,
            Self::CanonicalErrorDefaulted | Self::LegacyErrorDefaulted | Self::PathErrorDefaulted
        )
    }
}

#[derive(Debug, Clone)]
pub struct LoadedSettings {
    pub settings: AppSettings,
    pub status: SettingsLoadStatus,
}

impl LoadedSettings {
    fn new(settings: AppSettings, status: SettingsLoadStatus) -> Self {
        Self { settings, status }
    }
}

pub fn allow_automatic_settings_writeback(status: SettingsLoadStatus, reason: &str) -> bool {
    if status.allows_automatic_writeback() {
        return true;
    }

    log::warn!(
        "Skipped {reason} because settings loaded with status {}; leaving recoverable settings file unchanged",
        status.as_str()
    );
    false
}

trait ConfigCodec {
    fn parse_config_yaml(&self, contents: &str) -> Result<AppSettings, String>;
    fn parse_legacy_json(&self, contents: &str) -> Result<AppSettings, String>;
    fn serialize_config_yaml(&self, settings: &AppSettings) -> Result<String, String>;
}

#[derive(Debug, Clone, Copy)]
struct SerdeConfigCodec;

static CONFIG_CODEC: SerdeConfigCodec = SerdeConfigCodec;

fn config_codec() -> &'static SerdeConfigCodec {
    &CONFIG_CODEC
}

impl ConfigCodec for SerdeConfigCodec {
    fn parse_config_yaml(&self, contents: &str) -> Result<AppSettings, String> {
        serde_yaml::from_str::<AppSettings>(contents)
            .map_err(|e| format!("Failed to parse config.yaml: {}", e))
    }

    fn parse_legacy_json(&self, contents: &str) -> Result<AppSettings, String> {
        serde_json::from_str::<AppSettings>(contents)
            .map_err(|e| format!("Failed to parse legacy settings.json: {}", e))
    }

    fn serialize_config_yaml(&self, settings: &AppSettings) -> Result<String, String> {
        let settings_for_disk = redacted_settings(settings);
        serde_yaml::to_string(&settings_for_disk)
            .map_err(|e| format!("Failed to serialize settings: {}", e))
    }
}

fn parse_settings_yaml(contents: &str) -> Result<AppSettings, String> {
    config_codec().parse_config_yaml(contents)
}

fn parse_settings_json(contents: &str) -> Result<AppSettings, String> {
    config_codec().parse_legacy_json(contents)
}

fn read_settings_file<F>(path: &Path, parser: F) -> Result<AppSettings, String>
where
    F: FnOnce(&str) -> Result<AppSettings, String>,
{
    let contents = fs::read_to_string(path)
        .map_err(|e| format!("Failed to read settings file {}: {}", path.display(), e))?;
    parser(&contents)
}

/// Fill in owner-managed fields that the incoming payload omitted (`None`)
/// from the value currently on disk, so a whole-struct write from a caller
/// that doesn't carry them can't silently drop them.
///
/// Currently this guards `analytics_enabled`, whose sole owner is the
/// `set_analytics_enabled` command (load → patch → save). Any other writer
/// (e.g. the Settings footer "Save") that arrives with `None` adopts the
/// on-disk value here; an explicit `Some(_)` always wins, so the toggle stays
/// authoritative in both directions. Returns a borrow when nothing needs
/// patching (the common case) to avoid a clone.
fn preserve_owned_fields_from_disk<'a>(
    path: &Path,
    settings: &'a AppSettings,
) -> std::borrow::Cow<'a, AppSettings> {
    // Only read the disk when there is actually a gap to fill.
    if settings.analytics_enabled.is_some() {
        return std::borrow::Cow::Borrowed(settings);
    }

    // Best-effort read of the current on-disk value. If the file is missing or
    // unparseable we simply leave the field as-is (`None`); we never fail a
    // save just because we couldn't consult the previous value.
    let on_disk = fs::read_to_string(path)
        .ok()
        .and_then(|contents| parse_settings_yaml(&contents).ok());

    let Some(existing_analytics) = on_disk.and_then(|s| s.analytics_enabled) else {
        return std::borrow::Cow::Borrowed(settings);
    };

    let mut patched = settings.clone();
    patched.analytics_enabled = Some(existing_analytics);
    std::borrow::Cow::Owned(patched)
}

fn save_settings_to_path(path: &Path, settings: &AppSettings) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create settings directory: {}", e))?;
    }

    // Preserve-on-None: the analytics toggle (`set_analytics_enabled`) is the
    // SOLE owner of `analytics_enabled`, persisting an explicit `Some(true)` /
    // `Some(false)`. Other writers — most notably the Settings footer "Save",
    // which re-serializes the whole struct from a frontend store that may not
    // carry this field — arrive with `None`. Because the field is
    // `skip_serializing_if = "Option::is_none"`, a blind whole-struct write
    // would DROP the key and clobber a previously-persisted `true`. So when the
    // incoming payload omits it, we adopt whatever is currently on disk before
    // serializing. An explicit `Some(_)` always wins (no reverse clobber).
    let settings = preserve_owned_fields_from_disk(path, settings);

    let yaml = config_codec().serialize_config_yaml(settings.as_ref())?;

    let tmp_path = path.with_extension("yaml.tmp");
    fs::write(&tmp_path, &yaml).map_err(|e| format!("Failed to write settings file: {}", e))?;

    // Lock down perms before rename so the file is never world-readable, even briefly.
    crate::fs_util::set_owner_only(&tmp_path);

    fs::rename(&tmp_path, path).map_err(|e| format!("Failed to finalize settings file: {}", e))?;

    // Re-apply after rename in case rename semantics differ across platforms.
    crate::fs_util::set_owner_only(path);

    log::info!("Settings saved to {}", path.display());
    Ok(())
}

fn persist_settings_to_path(path: &Path, settings: &AppSettings) -> Result<(), String> {
    persist_inline_credentials(settings)?;
    save_settings_to_path(path, settings)
}

fn ensure_existing_config_is_parseable_for_write(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }

    let contents = fs::read_to_string(path).map_err(|e| {
        format!("Refusing to overwrite existing config.yaml because it cannot be read: {e}")
    })?;
    config_codec()
        .parse_config_yaml(&contents)
        .map(|_| ())
        .map_err(|_| {
            "Refusing to overwrite existing config.yaml because it cannot be parsed; \
         recover or remove the file explicitly before saving new settings"
                .to_string()
        })
}

fn load_settings_from_paths_with_status<F>(
    config_path: &Path,
    legacy_path: Option<&Path>,
    persist_import: F,
) -> LoadedSettings
where
    F: FnOnce(&AppSettings) -> Result<(), String>,
{
    if config_path.exists() {
        match read_settings_file(config_path, parse_settings_yaml) {
            Ok(settings) => {
                log::info!("Loaded settings from {}", config_path.display());
                return LoadedSettings::new(settings, SettingsLoadStatus::CanonicalOk);
            }
            Err(_e) => {
                log::warn!(
                    "Failed to load config.yaml, using defaults; leaving existing config.yaml unchanged"
                );
                return LoadedSettings::new(
                    AppSettings::default(),
                    SettingsLoadStatus::CanonicalErrorDefaulted,
                );
            }
        }
    }

    let Some(legacy_path) = legacy_path else {
        log::info!("No settings file found, using defaults");
        return LoadedSettings::new(AppSettings::default(), SettingsLoadStatus::DefaultsMissing);
    };

    if !legacy_path.exists() {
        log::info!("No settings file found, using defaults");
        return LoadedSettings::new(AppSettings::default(), SettingsLoadStatus::DefaultsMissing);
    }

    match read_settings_file(legacy_path, parse_settings_json) {
        Ok(settings) => {
            log::info!(
                "Imported legacy settings from {}; writing canonical {}",
                legacy_path.display(),
                config_path.display()
            );
            if let Err(e) = persist_import(&settings) {
                log::warn!("Failed to write imported config.yaml: {}", e);
            }
            LoadedSettings::new(settings, SettingsLoadStatus::LegacyImported)
        }
        Err(_e) => {
            log::warn!(
                "Failed to import legacy settings.json, using defaults; leaving legacy settings file unchanged"
            );
            LoadedSettings::new(
                AppSettings::default(),
                SettingsLoadStatus::LegacyErrorDefaulted,
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Load / Save
// ---------------------------------------------------------------------------

pub fn load_settings_with_status(app: &tauri::AppHandle) -> LoadedSettings {
    let config_path = match get_settings_path(app) {
        Ok(path) => path,
        Err(e) => {
            log::warn!("Failed to determine settings path, using defaults: {}", e);
            return LoadedSettings::new(
                AppSettings::default(),
                SettingsLoadStatus::PathErrorDefaulted,
            );
        }
    };

    if config_path.exists() {
        return load_settings_from_paths_with_status(&config_path, None, |_| Ok(()));
    }

    let legacy_path = match get_legacy_settings_json_path(app) {
        Ok(path) => path,
        Err(e) => {
            log::debug!("Failed to determine legacy settings path: {}", e);
            return load_settings_from_paths_with_status(&config_path, None, |_| Ok(()));
        }
    };

    load_settings_from_paths_with_status(&config_path, Some(&legacy_path), |settings| {
        persist_settings_to_path(&config_path, settings)
    })
}

pub fn load_settings(app: &tauri::AppHandle) -> AppSettings {
    load_settings_with_status(app).settings
}

/// Canonical cloud-provider credential keys checked for first-launch demo
/// detection. If every one of these slots is empty in credentials.yaml AND
/// the user hasn't yet chosen `demo_mode` in settings, the app auto-enters
/// demo mode (local ASR + local LLM) so it can still be used without keys.
///
/// IMPORTANT: keep in sync with `FIRST_TIME_CREDENTIAL_KEYS` in `src/App.tsx`.
pub const DEMO_CREDENTIAL_KEYS: &[&str] = &[
    "openai_api_key",
    "cerebras_api_key",
    "openrouter_api_key",
    "gemini_api_key",
    "deepgram_api_key",
    "assemblyai_api_key",
    "soniox_api_key",
    "gladia_api_key",
    "speechmatics_api_key",
    "elevenlabs_api_key",
    "revai_api_key",
    "groq_api_key",
    "aws_access_key",
];

/// Read the credential slot named `key` out of `store`. Returns `None` for
/// keys not in [`DEMO_CREDENTIAL_KEYS`] — this accessor exists solely to drive
/// [`all_demo_credentials_empty`] from the key list, so the list stays the
/// single source of truth and can't drift from a hand-written field probe.
fn demo_credential_slot<'a>(
    store: &'a crate::credentials::CredentialStore,
    key: &str,
) -> Option<&'a Option<String>> {
    match key {
        "openai_api_key" => Some(&store.openai_api_key),
        "cerebras_api_key" => Some(&store.cerebras_api_key),
        "openrouter_api_key" => Some(&store.openrouter_api_key),
        "gemini_api_key" => Some(&store.gemini_api_key),
        "deepgram_api_key" => Some(&store.deepgram_api_key),
        "assemblyai_api_key" => Some(&store.assemblyai_api_key),
        "soniox_api_key" => Some(&store.soniox_api_key),
        "gladia_api_key" => Some(&store.gladia_api_key),
        "speechmatics_api_key" => Some(&store.speechmatics_api_key),
        "elevenlabs_api_key" => Some(&store.elevenlabs_api_key),
        "revai_api_key" => Some(&store.revai_api_key),
        "groq_api_key" => Some(&store.groq_api_key),
        "aws_access_key" => Some(&store.aws_access_key),
        _ => None,
    }
}

/// Return `true` if the credential store has no cloud-provider key populated.
/// "Populated" means `Some(s)` where `s.trim()` is non-empty — whitespace
/// doesn't count (it would never authenticate against a real provider).
///
/// Driven by [`DEMO_CREDENTIAL_KEYS`] so adding a key there automatically
/// extends this check (no parallel hand-written field probe to forget).
pub fn all_demo_credentials_empty(store: &crate::credentials::CredentialStore) -> bool {
    let is_empty = |v: &Option<String>| v.as_deref().map(|s| s.trim()).unwrap_or("").is_empty();
    DEMO_CREDENTIAL_KEYS.iter().all(|key| {
        // Every entry in DEMO_CREDENTIAL_KEYS must resolve to a real slot;
        // an unmapped key (drift) counts as "not empty" so we never silently
        // skip a credential that should block demo mode.
        demo_credential_slot(store, key).is_some_and(is_empty)
    })
}

/// If `settings.demo_mode` is `None` (first launch) and every canonical
/// cloud credential is empty, mutate `settings` into the demo configuration
/// (ASR=LocalWhisper, LLM=LocalLlama, demo_mode=Some(true)) and return
/// `true` so the caller can persist. If `demo_mode` is already set, or any
/// credential exists, flip `demo_mode` to `Some(false)` (decision made) and
/// return `false`. Callers should only persist when this returns `true`.
pub fn apply_first_launch_demo_mode(
    settings: &mut AppSettings,
    store: &crate::credentials::CredentialStore,
) -> bool {
    if settings.demo_mode.is_some() {
        return false;
    }
    if all_demo_credentials_empty(store) {
        settings.asr_provider = AsrProvider::LocalWhisper;
        settings.llm_provider = LlmProvider::LocalLlama;
        settings.demo_mode = Some(true);
        log::info!(
            "First launch with no cloud credentials — entering demo mode \
             (local Whisper + local Llama). Download models via Settings to proceed."
        );
        true
    } else {
        settings.demo_mode = Some(false);
        true
    }
}

/// Persist `settings` to disk, serializing against concurrent saves.
///
/// Acquires the process-wide `SETTINGS_IO_LOCK` for the duration of the
/// write. Callers that already hold that lock (e.g. a load→patch→save
/// sequence) must call [`save_settings_locked`] instead to avoid deadlock.
pub fn save_settings(app: &tauri::AppHandle, settings: &AppSettings) -> Result<(), String> {
    let _guard = lock_settings_io();
    save_settings_locked(app, settings)
}

/// Write `settings` to disk **without** taking `SETTINGS_IO_LOCK`.
///
/// Pre-condition: the caller already holds the lock (via [`lock_settings_io`])
/// so the full read+write sequence is atomic with respect to other writers.
pub fn save_settings_locked(app: &tauri::AppHandle, settings: &AppSettings) -> Result<(), String> {
    let path = get_settings_path(app)?;
    ensure_existing_config_is_parseable_for_write(&path)?;
    persist_settings_to_path(&path, settings)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn unique_tempdir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "audio-graph-settings-{label}-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("create temp settings dir");
        dir
    }

    fn assert_debug_redacts(value: &impl std::fmt::Debug, secrets: &[&str], expected: &[&str]) {
        let debug = format!("{value:?}");
        for secret in secrets {
            assert!(
                !debug.contains(secret),
                "debug output leaked {secret:?}: {debug}"
            );
        }
        for expected in expected {
            assert!(
                debug.contains(expected),
                "debug output should retain {expected:?}: {debug}"
            );
        }
        assert!(
            debug.contains("<present>"),
            "debug output should show redacted presence: {debug}"
        );
    }

    #[derive(Debug, Clone, Copy)]
    struct SaphyrCandidateConfigCodec;

    impl ConfigCodec for SaphyrCandidateConfigCodec {
        fn parse_config_yaml(&self, contents: &str) -> Result<AppSettings, String> {
            serde_saphyr::from_str::<AppSettings>(contents)
                .map_err(|e| format!("Failed to parse config.yaml with serde-saphyr: {}", e))
        }

        fn parse_legacy_json(&self, contents: &str) -> Result<AppSettings, String> {
            serde_json::from_str::<AppSettings>(contents)
                .map_err(|e| format!("Failed to parse legacy settings.json: {}", e))
        }

        fn serialize_config_yaml(&self, settings: &AppSettings) -> Result<String, String> {
            let settings_for_disk = redacted_settings(settings);
            serde_saphyr::to_string(&settings_for_disk)
                .map_err(|e| format!("Failed to serialize settings with serde-saphyr: {}", e))
        }
    }

    fn redacted_json_value(settings: &AppSettings) -> serde_json::Value {
        serde_json::to_value(redacted_settings(settings)).expect("settings serialize to JSON value")
    }

    fn assert_semantically_equal_settings(left: &AppSettings, right: &AppSettings) {
        assert_eq!(redacted_json_value(left), redacted_json_value(right));
    }

    fn assert_yaml_has_no_inline_secrets(yaml: &str, secrets: &[&str]) {
        for secret in secrets {
            assert!(
                !yaml.contains(secret),
                "config.yaml must not contain inline secret {secret}"
            );
        }
        for secret_field in [
            "api_key:",
            "access_key:",
            "secret_key:",
            "session_token:",
            "service_account_path:",
        ] {
            assert!(
                !yaml.contains(secret_field),
                "config.yaml must not serialize inline credential field {secret_field}: {yaml}"
            );
        }
    }

    fn json_contains_object_key(value: &serde_json::Value, needle: &str) -> bool {
        match value {
            serde_json::Value::Object(map) => {
                map.contains_key(needle)
                    || map
                        .values()
                        .any(|nested| json_contains_object_key(nested, needle))
            }
            serde_json::Value::Array(items) => items
                .iter()
                .any(|nested| json_contains_object_key(nested, needle)),
            _ => false,
        }
    }

    #[test]
    fn sample_rate_whitelist_accepts_supported_values_and_rejects_others() {
        // Supported set — every entry in the Audio settings dropdown.
        for hz in [22050u32, 32000, 44100, 48000, 88200, 96000] {
            assert!(
                sample_rate_is_valid(hz),
                "{} Hz should be accepted by the whitelist",
                hz
            );
        }
        // Out-of-set values we explicitly don't support. 16 kHz is the
        // downstream ASR format but rsac capture rejects it, so capture stays
        // on rates the OS backend accepts and the app resamples internally.
        for hz in [0u32, 1, 8000, 11025, 16000, 192000, u32::MAX] {
            assert!(
                !sample_rate_is_valid(hz),
                "{} Hz must be rejected — not in the UI whitelist",
                hz
            );
        }
    }

    #[test]
    fn channels_whitelist_accepts_mono_stereo_only_and_resolve_falls_back() {
        assert!(channels_is_valid(1));
        assert!(channels_is_valid(2));
        assert!(!channels_is_valid(0));
        assert!(!channels_is_valid(3));
        assert!(!channels_is_valid(u16::MAX));

        // resolve_audio_settings must fall back to defaults for invalid
        // persisted values rather than bubble the junk into capture.
        let bad = AudioSettings {
            sample_rate: 12345,
            channels: 7,
        };
        let (sr, ch) = resolve_audio_settings(&bad);
        assert_eq!(sr, default_sample_rate());
        assert_eq!(ch, default_channels());

        // Valid values must round-trip unchanged.
        let good = AudioSettings {
            sample_rate: 48000,
            channels: 2,
        };
        assert_eq!(resolve_audio_settings(&good), (48000, 2));
    }

    #[test]
    fn app_settings_default_uses_bundled_config_audio_defaults() {
        let settings = AppSettings::default();

        assert_eq!(settings.audio_settings.sample_rate, 48_000);
        assert_eq!(settings.audio_settings.channels, 2);
    }

    #[test]
    fn streaming_prefill_defaults_off_and_only_local_llama_supports_it() {
        // Opt-in: a fresh install must not silently change extraction behavior.
        assert!(!AppSettings::default().streaming_prefill);

        // Only the in-process llama.cpp engine exposes prefill/decode control
        // (ADR-0012). Everything else ignores the flag.
        assert!(LlmProvider::LocalLlama.supports_streaming_prefill());
        assert!(
            !LlmProvider::MistralRs {
                model_id: "m.gguf".into()
            }
            .supports_streaming_prefill()
        );
        assert!(
            !LlmProvider::Api {
                endpoint: "https://api.openai.com/v1".into(),
                api_key: String::new(),
                model: "gpt-4o-mini".into(),
            }
            .supports_streaming_prefill()
        );
        assert!(!LlmProvider::default().supports_streaming_prefill());
    }

    #[test]
    fn privacy_mode_defaults_to_byok_and_allows_readiness_probes() {
        let settings = AppSettings::default();
        assert_eq!(settings.privacy_mode, PrivacyMode::ByokCloud);
        assert!(PrivacyMode::LocalOnly.allows_no_content_provider_probe());
        assert!(PrivacyMode::CloudDisabledReadinessOnly.allows_no_content_provider_probe());
        assert!(!PrivacyMode::LocalOnly.allows_session_cloud_content_transfer());
        assert!(!PrivacyMode::OrgPromotion.allows_session_cloud_content_transfer());
        assert!(PrivacyMode::ByokCloud.allows_session_cloud_content_transfer());
    }

    #[test]
    fn provider_content_transfer_classification_treats_loopback_as_local() {
        assert!(
            !AsrProvider::Api {
                endpoint: "http://127.0.0.1:8080/v1".into(),
                api_key: String::new(),
                model: "local-asr".into(),
            }
            .requires_cloud_content_transfer()
        );
        assert!(
            AsrProvider::DeepgramStreaming {
                api_key: String::new(),
                model: "nova-3".into(),
                enable_diarization: true,
                endpointing_ms: 300,
                utterance_end_ms: 1000,
                vad_events: true,
                eot_threshold: 0.5,
                eager_eot_threshold: 0.0,
                eot_timeout_ms: 0,
                max_speakers: 0,
            }
            .requires_cloud_content_transfer()
        );
        assert!(
            !LlmProvider::Api {
                endpoint: "http://localhost:11434/v1".into(),
                api_key: String::new(),
                model: "llama3.2".into(),
            }
            .requires_cloud_content_transfer()
        );
        assert!(
            LlmProvider::OpenRouter {
                model: "openai/gpt-5.2".into(),
                base_url: default_openrouter_base_url(),
                provider_order: None,
                include_usage_in_stream: true,
                api_key: String::new(),
            }
            .requires_cloud_content_transfer()
        );
        assert!(
            TtsProvider::DeepgramAura {
                voice: default_aura_voice(),
                sample_rate: default_aura_sample_rate(),
                speed: default_aura_speed(),
            }
            .requires_cloud_content_transfer()
        );
    }

    #[test]
    fn streaming_prefill_round_trips_through_serialization() {
        let settings = AppSettings {
            streaming_prefill: true,
            ..AppSettings::default()
        };
        let json = serde_json::to_string(&redacted_settings(&settings)).unwrap();
        let back: AppSettings = serde_json::from_str(&json).unwrap();
        assert!(back.streaming_prefill);

        // Missing field (older settings.json) must default to false, not error.
        let legacy = serde_json::json!({}).to_string();
        let parsed: AppSettings = serde_json::from_str(&legacy).unwrap();
        assert!(!parsed.streaming_prefill);
    }

    #[test]
    fn diarization_policy_gates_provider_runtime_flags() {
        let mut aws = AsrProvider::AwsTranscribe {
            region: "us-east-1".into(),
            language_code: "en-US".into(),
            credential_source: AwsCredentialSource::DefaultChain,
            enable_diarization: true,
        };
        aws.apply_diarization_settings(&DiarizationSettings {
            mode: DiarizationMode::Off,
            ..DiarizationSettings::default()
        });
        assert!(matches!(
            aws,
            AsrProvider::AwsTranscribe {
                enable_diarization: false,
                ..
            }
        ));

        let mut assemblyai = AsrProvider::AssemblyAI {
            api_key: "key".into(),
            enable_diarization: true,
        };
        assemblyai.apply_diarization_settings(&DiarizationSettings {
            mode: DiarizationMode::Local,
            ..DiarizationSettings::default()
        });
        assert!(matches!(
            assemblyai,
            AsrProvider::AssemblyAI {
                enable_diarization: false,
                ..
            }
        ));

        let mut provider_enabled = AsrProvider::AssemblyAI {
            api_key: "key".into(),
            enable_diarization: true,
        };
        provider_enabled.apply_diarization_settings(&DiarizationSettings {
            mode: DiarizationMode::Provider,
            ..DiarizationSettings::default()
        });
        assert!(matches!(
            provider_enabled,
            AsrProvider::AssemblyAI {
                enable_diarization: true,
                ..
            }
        ));

        let mut provider_disabled = AsrProvider::AssemblyAI {
            api_key: "key".into(),
            enable_diarization: false,
        };
        provider_disabled.apply_diarization_settings(&DiarizationSettings {
            mode: DiarizationMode::Hybrid,
            ..DiarizationSettings::default()
        });
        assert!(matches!(
            provider_disabled,
            AsrProvider::AssemblyAI {
                enable_diarization: false,
                ..
            }
        ));

        let mut deepgram = AsrProvider::DeepgramStreaming {
            api_key: "key".into(),
            model: "nova-3".into(),
            enable_diarization: true,
            endpointing_ms: 300,
            utterance_end_ms: 1000,
            vad_events: true,
            eot_threshold: 0.5,
            eager_eot_threshold: 0.0,
            eot_timeout_ms: 0,
            max_speakers: 0,
        };
        deepgram.apply_diarization_settings(&DiarizationSettings {
            mode: DiarizationMode::Hybrid,
            speaker_count: DiarizationSpeakerCount::Fixed,
            max_speakers: Some(4),
        });
        assert!(matches!(
            deepgram,
            AsrProvider::DeepgramStreaming {
                enable_diarization: true,
                max_speakers: 4,
                ..
            }
        ));
    }

    #[test]
    fn diarization_policy_speaker_count_maps_to_provider_caps() {
        let auto = DiarizationSettings {
            speaker_count: DiarizationSpeakerCount::Auto,
            max_speakers: Some(7),
            ..DiarizationSettings::default()
        };
        assert_eq!(auto.provider_max_speakers(3), 3);

        let unbounded = DiarizationSettings {
            speaker_count: DiarizationSpeakerCount::Unbounded,
            max_speakers: Some(7),
            ..DiarizationSettings::default()
        };
        assert_eq!(unbounded.provider_max_speakers(3), 0);

        let fixed = DiarizationSettings {
            speaker_count: DiarizationSpeakerCount::Fixed,
            max_speakers: Some(6),
            ..DiarizationSettings::default()
        };
        assert_eq!(fixed.provider_max_speakers(3), 6);

        let malformed_fixed = DiarizationSettings {
            speaker_count: DiarizationSpeakerCount::Fixed,
            max_speakers: Some(0),
            ..DiarizationSettings::default()
        };
        assert_eq!(malformed_fixed.provider_max_speakers(3), 1);
    }

    #[test]
    fn diarization_settings_default_and_round_trip_without_secrets() {
        let settings = AppSettings {
            diarization: DiarizationSettings {
                mode: DiarizationMode::Hybrid,
                speaker_count: DiarizationSpeakerCount::Fixed,
                max_speakers: Some(6),
            },
            ..AppSettings::default()
        };

        let yaml = config_codec().serialize_config_yaml(&settings).unwrap();
        assert!(yaml.contains("diarization:"));
        assert!(yaml.contains("mode: hybrid"));
        assert!(yaml.contains("speaker_count: fixed"));
        assert!(yaml.contains("max_speakers: 6"));

        let back = config_codec().parse_config_yaml(&yaml).unwrap();
        assert_eq!(back.diarization.mode, DiarizationMode::Hybrid);
        assert_eq!(
            back.diarization.speaker_count,
            DiarizationSpeakerCount::Fixed
        );
        assert_eq!(back.diarization.max_speakers, Some(6));

        let legacy: AppSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(legacy.diarization, DiarizationSettings::default());
    }

    #[test]
    fn credential_bearing_settings_debug_redacts_secrets() {
        assert_debug_redacts(
            &AwsCredentialSource::AccessKeys {
                access_key: "AKIA-DEBUG-SECRET".into(),
                secret_key: Some("AWS-DEBUG-SECRET".into()),
                session_token: Some("AWS-DEBUG-TOKEN".into()),
            },
            &["AKIA-DEBUG-SECRET", "AWS-DEBUG-SECRET", "AWS-DEBUG-TOKEN"],
            &["AccessKeys", "access_key"],
        );

        assert_debug_redacts(
            &AsrProvider::Api {
                endpoint: "https://api.openai.com/v1".into(),
                api_key: "sk-asr-api-secret".into(),
                model: "whisper-1".into(),
            },
            &["sk-asr-api-secret"],
            &["https://api.openai.com/v1", "whisper-1"],
        );
        assert_debug_redacts(
            &AsrProvider::DeepgramStreaming {
                api_key: "dg-asr-secret".into(),
                model: "nova-3".into(),
                enable_diarization: true,
                endpointing_ms: 300,
                utterance_end_ms: 1000,
                vad_events: true,
                eot_threshold: 0.7,
                eager_eot_threshold: 0.5,
                eot_timeout_ms: 1200,
                max_speakers: 0,
            },
            &["dg-asr-secret"],
            &["DeepgramStreaming", "nova-3", "max_speakers"],
        );
        assert_debug_redacts(
            &AsrProvider::AssemblyAI {
                api_key: "aai-asr-secret".into(),
                enable_diarization: true,
            },
            &["aai-asr-secret"],
            &["AssemblyAI", "enable_diarization"],
        );
        assert_debug_redacts(
            &AsrProvider::OpenAiRealtimeTranscription {
                api_key: "sk-realtime-secret".into(),
                model: "gpt-realtime-transcribe".into(),
                language: Some("en".into()),
            },
            &["sk-realtime-secret"],
            &[
                "OpenAiRealtimeTranscription",
                "gpt-realtime-transcribe",
                "en",
            ],
        );
        assert_debug_redacts(
            &AsrProvider::AwsTranscribe {
                region: "us-east-1".into(),
                language_code: "en-US".into(),
                credential_source: AwsCredentialSource::AccessKeys {
                    access_key: "AKIA-ASR-SECRET".into(),
                    secret_key: None,
                    session_token: None,
                },
                enable_diarization: true,
            },
            &["AKIA-ASR-SECRET"],
            &["AwsTranscribe", "us-east-1", "en-US"],
        );

        assert_debug_redacts(
            &LlmApiConfig {
                endpoint: "https://api.openai.com/v1".into(),
                api_key: Some("sk-llm-api-config-secret".into()),
                model: "gpt-5-mini".into(),
                max_tokens: 512,
                temperature: 0.2,
            },
            &["sk-llm-api-config-secret"],
            &["LlmApiConfig", "gpt-5-mini"],
        );
        assert_debug_redacts(
            &LlmProvider::Api {
                endpoint: "https://api.groq.com/openai/v1".into(),
                api_key: "gsk-llm-secret".into(),
                model: "llama-3.3".into(),
            },
            &["gsk-llm-secret"],
            &["https://api.groq.com/openai/v1", "llama-3.3"],
        );
        assert_debug_redacts(
            &LlmProvider::OpenRouter {
                model: "anthropic/claude-sonnet-4.5".into(),
                base_url: "https://openrouter.ai/api/v1".into(),
                provider_order: Some(vec!["anthropic".into()]),
                include_usage_in_stream: true,
                api_key: "sk-or-llm-secret".into(),
            },
            &["sk-or-llm-secret"],
            &["OpenRouter", "anthropic/claude-sonnet-4.5"],
        );
        assert_debug_redacts(
            &LlmProvider::AwsBedrock {
                region: "us-west-2".into(),
                model_id: "anthropic.claude".into(),
                credential_source: AwsCredentialSource::AccessKeys {
                    access_key: "AKIA-LLM-SECRET".into(),
                    secret_key: None,
                    session_token: None,
                },
            },
            &["AKIA-LLM-SECRET"],
            &["AwsBedrock", "us-west-2", "anthropic.claude"],
        );

        assert_debug_redacts(
            &GeminiAuthMode::ApiKey {
                api_key: "AIza-gemini-secret".into(),
            },
            &["AIza-gemini-secret"],
            &["ApiKey", "api_key"],
        );
        assert_debug_redacts(
            &GeminiAuthMode::VertexAI {
                project_id: "audio-graph-prod".into(),
                location: "us-central1".into(),
                service_account_path: Some("/secret/service-account.json".into()),
            },
            &["/secret/service-account.json"],
            &["VertexAI", "audio-graph-prod", "us-central1"],
        );

        let settings = AppSettings {
            asr_provider: AsrProvider::Api {
                endpoint: "https://api.openai.com/v1".into(),
                api_key: "sk-app-asr-secret".into(),
                model: "whisper-1".into(),
            },
            llm_provider: LlmProvider::OpenRouter {
                model: "openai/gpt-5.2".into(),
                base_url: "https://openrouter.ai/api/v1".into(),
                provider_order: None,
                include_usage_in_stream: true,
                api_key: "sk-app-or-secret".into(),
            },
            llm_api_config: Some(LlmApiConfig {
                endpoint: "https://api.openai.com/v1".into(),
                api_key: Some("sk-app-api-config-secret".into()),
                model: "gpt-5-mini".into(),
                max_tokens: 128,
                temperature: 0.1,
            }),
            gemini: GeminiSettings {
                auth: GeminiAuthMode::ApiKey {
                    api_key: "AIza-app-gemini-secret".into(),
                },
                model: "gemini-live".into(),
                voice: "Kore".into(),
            },
            ..AppSettings::default()
        };
        assert_debug_redacts(
            &settings,
            &[
                "sk-app-asr-secret",
                "sk-app-or-secret",
                "sk-app-api-config-secret",
                "AIza-app-gemini-secret",
            ],
            &["AppSettings", "whisper-1", "openai/gpt-5.2", "gemini-live"],
        );
    }

    #[test]
    fn app_settings_default_strips_bundled_whisper_model_path() {
        let expected = crate::config::load_default_config()
            .whisper_model_filename()
            .expect("bundled config should include an ASR model path");

        assert_eq!(expected, "ggml-small.en.bin");
        assert_eq!(AppSettings::default().whisper_model, expected);
    }

    #[test]
    fn demo_credentials_empty_treats_missing_and_whitespace_as_empty() {
        let mut store = crate::credentials::CredentialStore::default();
        assert!(all_demo_credentials_empty(&store));

        // Whitespace-only values are not real credentials — still empty.
        store.openai_api_key = Some("   ".to_string());
        assert!(all_demo_credentials_empty(&store));

        // Any non-empty key flips the result.
        store.openai_api_key = Some("sk-real".to_string());
        assert!(!all_demo_credentials_empty(&store));
    }

    #[test]
    fn all_demo_credentials_empty_tracks_every_listed_key() {
        // Single source of truth: each key in DEMO_CREDENTIAL_KEYS must, when
        // populated, flip all_demo_credentials_empty to false. If someone adds
        // a key to the list but forgets to map it in demo_credential_slot
        // (drift), this fails — the unmapped key would be skipped silently.
        //
        // We mutate the store via the public fields keyed by the same string,
        // so this also fails if a listed key has no matching field at all.
        let set_slot = |store: &mut crate::credentials::CredentialStore, key: &str| match key {
            "openai_api_key" => store.openai_api_key = Some("real-secret".to_string()),
            "cerebras_api_key" => store.cerebras_api_key = Some("real-secret".to_string()),
            "openrouter_api_key" => store.openrouter_api_key = Some("real-secret".to_string()),
            "gemini_api_key" => store.gemini_api_key = Some("real-secret".to_string()),
            "deepgram_api_key" => store.deepgram_api_key = Some("real-secret".to_string()),
            "assemblyai_api_key" => store.assemblyai_api_key = Some("real-secret".to_string()),
            "soniox_api_key" => store.soniox_api_key = Some("real-secret".to_string()),
            "gladia_api_key" => store.gladia_api_key = Some("real-secret".to_string()),
            "speechmatics_api_key" => store.speechmatics_api_key = Some("real-secret".to_string()),
            "elevenlabs_api_key" => store.elevenlabs_api_key = Some("real-secret".to_string()),
            "revai_api_key" => store.revai_api_key = Some("real-secret".to_string()),
            "groq_api_key" => store.groq_api_key = Some("real-secret".to_string()),
            "aws_access_key" => store.aws_access_key = Some("real-secret".to_string()),
            other => panic!("DEMO_CREDENTIAL_KEYS entry {other} has no field in this test"),
        };

        for &key in DEMO_CREDENTIAL_KEYS {
            let mut store = crate::credentials::CredentialStore::default();
            assert!(
                all_demo_credentials_empty(&store),
                "default store should be empty before setting {key}"
            );
            assert!(
                demo_credential_slot(&store, key).is_some(),
                "DEMO_CREDENTIAL_KEYS entry {key} has no demo_credential_slot mapping (drift)"
            );

            set_slot(&mut store, key);
            assert!(
                !all_demo_credentials_empty(&store),
                "setting {key} must make all_demo_credentials_empty return false"
            );
        }
    }

    #[test]
    fn fallback_channels_matches_shipped_default_toml() {
        // FALLBACK_CHANNELS only fires when default.toml fails to parse; it must
        // equal the shipped `audio.channels` so a parse failure degrades to the
        // same channel count rather than silently halving to mono.
        let shipped = crate::config::load_default_config()
            .audio
            .channels
            .expect("bundled config should specify audio.channels");
        assert_eq!(shipped, 2, "default.toml ships audio.channels = 2");
        assert_eq!(
            FALLBACK_CHANNELS, shipped,
            "FALLBACK_CHANNELS must match the shipped default.toml channel count"
        );
    }

    #[test]
    fn first_launch_demo_mode_enables_local_providers_when_no_creds() {
        let mut settings = AppSettings {
            demo_mode: None,
            // Simulate a non-default LLM choice to prove we overwrite it.
            llm_provider: LlmProvider::Api {
                endpoint: "https://api.openai.com/v1".into(),
                api_key: "".into(),
                model: "gpt-4o".into(),
            },
            ..AppSettings::default()
        };
        let store = crate::credentials::CredentialStore::default();

        let changed = apply_first_launch_demo_mode(&mut settings, &store);
        assert!(
            changed,
            "first-launch with no creds must persist a decision"
        );
        assert_eq!(settings.demo_mode, Some(true));
        assert!(matches!(settings.asr_provider, AsrProvider::LocalWhisper));
        assert!(matches!(settings.llm_provider, LlmProvider::LocalLlama));
    }

    #[test]
    fn first_launch_demo_mode_skips_when_any_cred_present() {
        let mut settings = AppSettings {
            demo_mode: None,
            ..AppSettings::default()
        };
        let mut store = crate::credentials::CredentialStore::default();
        store.gemini_api_key = Some("AIza...".to_string());

        let changed = apply_first_launch_demo_mode(&mut settings, &store);
        assert!(changed, "decision must be recorded even when demo is off");
        assert_eq!(settings.demo_mode, Some(false));
    }

    #[test]
    fn first_launch_demo_mode_noop_when_already_decided() {
        // If the user has already seen the banner once (or ExpressSetup set
        // demo_mode=false), the setup hook must not re-stomp provider
        // settings on a later launch — even if credentials are missing.
        let mut settings = AppSettings {
            demo_mode: Some(false),
            llm_provider: LlmProvider::Api {
                endpoint: "https://api.openai.com/v1".into(),
                api_key: "".into(),
                model: "gpt-4o".into(),
            },
            ..AppSettings::default()
        };
        let store = crate::credentials::CredentialStore::default();

        let changed = apply_first_launch_demo_mode(&mut settings, &store);
        assert!(!changed);
        // LLM choice preserved.
        assert!(matches!(settings.llm_provider, LlmProvider::Api { .. }));
    }

    #[test]
    fn settings_serialization_redacts_inline_credentials() {
        let settings = AppSettings {
            asr_provider: AsrProvider::DeepgramStreaming {
                api_key: "dg-secret".into(),
                model: "nova-3".into(),
                enable_diarization: true,
                endpointing_ms: 300,
                utterance_end_ms: 1000,
                vad_events: true,
                eot_threshold: 0.5,
                eager_eot_threshold: 0.0,
                eot_timeout_ms: 0,
                max_speakers: 2,
            },
            llm_provider: LlmProvider::Api {
                endpoint: "https://api.openai.com/v1".into(),
                api_key: "sk-secret".into(),
                model: "gpt-4o-mini".into(),
            },
            llm_api_config: Some(LlmApiConfig {
                endpoint: "https://api.openai.com/v1".into(),
                api_key: Some("cfg-secret".into()),
                model: "gpt-4o-mini".into(),
                max_tokens: 2048,
                temperature: 0.7,
            }),
            gemini: GeminiSettings {
                auth: GeminiAuthMode::ApiKey {
                    api_key: "gemini-secret".into(),
                },
                model: default_gemini_model(),
                voice: String::new(),
            },
            ..AppSettings::default()
        };

        assert!(has_inline_credentials(&settings));
        let redacted = redacted_settings(&settings);
        let json = serde_json::to_string(&redacted).unwrap();
        let yaml = config_codec().serialize_config_yaml(&settings).unwrap();

        for secret in ["dg-secret", "sk-secret", "cfg-secret", "gemini-secret"] {
            assert!(
                !json.contains(secret),
                "settings JSON must not contain inline secret {secret}"
            );
            assert!(
                !yaml.contains(secret),
                "config.yaml must not contain inline secret {secret}"
            );
        }
        assert!(!has_inline_credentials(&redacted));
        let parsed = config_codec()
            .parse_config_yaml(&yaml)
            .expect("redacted config.yaml still parses");
        assert!(!has_inline_credentials(&parsed));
    }

    #[test]
    fn public_app_settings_schema_omits_credential_material() {
        let schema = public_app_settings_schema_json();
        let schema_text =
            serde_json::to_string(&schema).expect("public settings schema serializes");

        for secret_field in [
            "api_key",
            "access_key",
            "secret_key",
            "session_token",
            "service_account_path",
        ] {
            assert!(
                !json_contains_object_key(&schema, secret_field),
                "public settings schema must not expose credential field {secret_field}: {schema_text}"
            );
        }

        for non_secret_field in [
            "\"asr_provider\"",
            "\"llm_provider\"",
            "\"gemini\"",
            "\"diarization\"",
            "\"audio_settings\"",
            "\"vertex_ai\"",
            "\"aws_transcribe\"",
        ] {
            assert!(
                schema_text.contains(non_secret_field),
                "public settings schema should expose non-secret field {non_secret_field}: {schema_text}"
            );
        }
    }

    #[test]
    fn vertex_service_account_path_is_credential_material_not_config() {
        let settings = AppSettings {
            gemini: GeminiSettings {
                auth: GeminiAuthMode::VertexAI {
                    project_id: "audio-graph-prod".into(),
                    location: "us-central1".into(),
                    service_account_path: Some("/secure/audio-graph-sa.json".into()),
                },
                model: default_gemini_model(),
                voice: String::new(),
            },
            ..AppSettings::default()
        };

        assert!(has_inline_credentials(&settings));
        let updates = inline_credential_updates(&settings);
        assert_eq!(
            updates,
            vec![InlineCredentialUpdate {
                key: "google_service_account_path",
                value: "/secure/audio-graph-sa.json".into(),
            }]
        );

        let raw_json = serde_json::to_string(&settings).unwrap();
        assert!(
            !raw_json.contains("/secure/audio-graph-sa.json"),
            "raw settings serialization must skip service account paths"
        );

        let yaml = config_codec().serialize_config_yaml(&settings).unwrap();
        assert_yaml_has_no_inline_secrets(&yaml, &["/secure/audio-graph-sa.json"]);
        let parsed = config_codec()
            .parse_config_yaml(&yaml)
            .expect("redacted Vertex AI YAML parses");
        assert!(!has_inline_credentials(&parsed));
        match &parsed.gemini.auth {
            GeminiAuthMode::VertexAI {
                service_account_path,
                ..
            } => assert!(service_account_path.is_none()),
            other => panic!("unexpected Gemini auth mode: {:?}", other),
        }

        let mut store = crate::credentials::CredentialStore::default();
        store.google_service_account_path = Some("/secure/audio-graph-sa.json".into());
        let hydrated = hydrate_runtime_credentials(&parsed, &store);
        match hydrated.gemini.auth {
            GeminiAuthMode::VertexAI {
                service_account_path,
                ..
            } => assert_eq!(
                service_account_path.as_deref(),
                Some("/secure/audio-graph-sa.json")
            ),
            other => panic!("unexpected Gemini auth mode: {:?}", other),
        }
    }

    #[test]
    fn legacy_aws_inline_multifield_credentials_are_imported_and_redacted() {
        let settings = AppSettings {
            asr_provider: AsrProvider::AwsTranscribe {
                region: "us-east-1".into(),
                language_code: "en-US".into(),
                credential_source: AwsCredentialSource::AccessKeys {
                    access_key: "AKIA_LEGACY".into(),
                    secret_key: Some("AWS_LEGACY_SECRET".into()),
                    session_token: Some("AWS_LEGACY_SESSION".into()),
                },
                enable_diarization: true,
            },
            ..AppSettings::default()
        };

        assert!(has_inline_credentials(&settings));
        let updates = inline_credential_updates(&settings);
        assert_eq!(
            updates,
            vec![
                InlineCredentialUpdate {
                    key: "aws_access_key",
                    value: "AKIA_LEGACY".into(),
                },
                InlineCredentialUpdate {
                    key: "aws_secret_key",
                    value: "AWS_LEGACY_SECRET".into(),
                },
                InlineCredentialUpdate {
                    key: "aws_session_token",
                    value: "AWS_LEGACY_SESSION".into(),
                },
            ]
        );

        let raw_json = serde_json::to_string(&settings).unwrap();
        for secret in ["AKIA_LEGACY", "AWS_LEGACY_SECRET", "AWS_LEGACY_SESSION"] {
            assert!(
                !raw_json.contains(secret),
                "raw settings serialization must skip legacy AWS credential material"
            );
        }

        let yaml = config_codec().serialize_config_yaml(&settings).unwrap();
        assert_yaml_has_no_inline_secrets(
            &yaml,
            &["AKIA_LEGACY", "AWS_LEGACY_SECRET", "AWS_LEGACY_SESSION"],
        );
        let parsed = config_codec()
            .parse_config_yaml(&yaml)
            .expect("redacted AWS YAML parses");
        assert!(!has_inline_credentials(&parsed));
        match &parsed.asr_provider {
            AsrProvider::AwsTranscribe {
                credential_source:
                    AwsCredentialSource::AccessKeys {
                        access_key,
                        secret_key,
                        session_token,
                    },
                ..
            } => {
                assert!(access_key.is_empty());
                assert!(secret_key.is_none());
                assert!(session_token.is_none());
            }
            other => panic!("unexpected ASR provider: {:?}", other),
        }

        let mut store = crate::credentials::CredentialStore::default();
        store.aws_access_key = Some("AKIA_LEGACY".into());
        store.aws_secret_key = Some("AWS_LEGACY_SECRET".into());
        store.aws_session_token = Some("AWS_LEGACY_SESSION".into());
        let hydrated = hydrate_runtime_credentials(&parsed, &store);
        match hydrated.asr_provider {
            AsrProvider::AwsTranscribe {
                credential_source: AwsCredentialSource::AccessKeys { access_key, .. },
                ..
            } => assert_eq!(access_key, "AKIA_LEGACY"),
            other => panic!("unexpected ASR provider: {:?}", other),
        }
    }

    #[test]
    fn config_yaml_round_trips_redacted_settings() {
        let settings = AppSettings {
            demo_mode: Some(false),
            audio_settings: AudioSettings {
                sample_rate: 44_100,
                channels: 2,
            },
            ..AppSettings::default()
        };
        let yaml = config_codec().serialize_config_yaml(&settings).unwrap();
        let parsed = config_codec()
            .parse_config_yaml(&yaml)
            .expect("valid config.yaml");

        assert_eq!(parsed.demo_mode, Some(false));
        assert_eq!(parsed.audio_settings.sample_rate, 44_100);
        assert_eq!(parsed.audio_settings.channels, 2);
    }

    #[test]
    fn legacy_settings_json_still_parses_for_import() {
        let settings = AppSettings {
            demo_mode: Some(true),
            audio_settings: AudioSettings {
                sample_rate: 48_000,
                channels: 1,
            },
            ..AppSettings::default()
        };
        let json = serde_json::to_string(&redacted_settings(&settings)).unwrap();
        let parsed = config_codec()
            .parse_legacy_json(&json)
            .expect("valid legacy settings.json");

        assert_eq!(parsed.demo_mode, Some(true));
        assert_eq!(parsed.audio_settings.sample_rate, 48_000);
        assert_eq!(parsed.audio_settings.channels, 1);
    }

    #[test]
    fn corrupt_config_yaml_is_rejected_before_default_fallback() {
        let err = config_codec()
            .parse_config_yaml("asr_provider: [not valid")
            .expect_err("invalid yaml");

        assert!(err.contains("Failed to parse config.yaml"));
    }

    #[test]
    fn config_codec_parses_current_yaml_fixture() {
        let parsed = config_codec()
            .parse_config_yaml(include_str!("../../fixtures/settings/current-config.yaml"))
            .expect("current config fixture parses");

        assert_eq!(parsed.demo_mode, Some(false));
        assert!(parsed.speak_aloud);
        assert!(parsed.streaming_prefill);
        assert_eq!(parsed.audio_settings.sample_rate, 44_100);
        assert_eq!(parsed.audio_settings.channels, 2);
        assert_eq!(parsed.diarization.mode, DiarizationMode::Hybrid);
        assert_eq!(
            parsed.diarization.speaker_count,
            DiarizationSpeakerCount::Fixed
        );
        assert_eq!(parsed.diarization.max_speakers, Some(6));
        assert_eq!(parsed.gemini.voice, "Kore");

        match parsed.asr_provider {
            AsrProvider::DeepgramStreaming {
                api_key,
                model,
                endpointing_ms,
                utterance_end_ms,
                max_speakers,
                ..
            } => {
                assert!(api_key.is_empty());
                assert_eq!(model, "nova-3");
                assert_eq!(endpointing_ms, 250);
                assert_eq!(utterance_end_ms, 900);
                assert_eq!(max_speakers, 4);
            }
            other => panic!("unexpected ASR provider: {:?}", other),
        }
        match parsed.llm_provider {
            LlmProvider::OpenRouter {
                api_key,
                model,
                base_url,
                provider_order,
                include_usage_in_stream,
            } => {
                assert!(api_key.is_empty());
                assert_eq!(model, "openai/gpt-5.2");
                assert_eq!(base_url, "https://openrouter.ai/api/v1");
                assert_eq!(provider_order, Some(vec!["openai".to_string()]));
                assert!(include_usage_in_stream);
            }
            other => panic!("unexpected LLM provider: {:?}", other),
        }
        let llm_api = parsed.llm_api_config.expect("fixture includes API config");
        assert!(llm_api.api_key.is_none());
        assert_eq!(llm_api.max_tokens, 1024);
        assert!((llm_api.temperature - 0.3).abs() < f32::EPSILON);

        match parsed.tts_provider {
            TtsProvider::DeepgramAura {
                voice,
                sample_rate,
                speed,
            } => {
                assert_eq!(voice, "aura-2-thalia-en");
                assert_eq!(sample_rate, 24_000);
                assert!((speed - 1.1).abs() < f32::EPSILON);
            }
            other => panic!("unexpected TTS provider: {:?}", other),
        }
    }

    #[test]
    fn config_codec_parses_legacy_json_fixture() {
        let parsed = config_codec()
            .parse_legacy_json(include_str!("../../fixtures/settings/legacy-settings.json"))
            .expect("legacy JSON fixture parses");

        assert_eq!(parsed.demo_mode, Some(true));
        assert!(parsed.streaming_prefill);
        assert_eq!(parsed.audio_settings.sample_rate, 48_000);
        assert_eq!(parsed.audio_settings.channels, 1);
        assert_eq!(parsed.diarization.mode, DiarizationMode::Local);
        assert_eq!(
            parsed.diarization.speaker_count,
            DiarizationSpeakerCount::Unbounded
        );
        match parsed.asr_provider {
            AsrProvider::Api {
                endpoint,
                api_key,
                model,
            } => {
                assert_eq!(endpoint, "https://api.groq.com/openai/v1");
                assert!(api_key.is_empty());
                assert_eq!(model, "whisper-large-v3");
            }
            other => panic!("unexpected ASR provider: {:?}", other),
        }
    }

    #[test]
    fn config_codec_preserves_unknown_field_tolerance() {
        let parsed = config_codec()
            .parse_config_yaml(include_str!(
                "../../fixtures/settings/unknown-fields-config.yaml"
            ))
            .expect("unknown fields stay tolerated for forward compatibility");

        assert_eq!(parsed.demo_mode, Some(false));
        assert_eq!(parsed.audio_settings.sample_rate, 48_000);
        assert_eq!(parsed.audio_settings.channels, 2);
        assert!(matches!(parsed.asr_provider, AsrProvider::LocalWhisper));
    }

    #[test]
    fn config_codec_parses_edge_compatible_yaml_fixture_and_rewrites_known_schema() {
        let fixture = include_str!("../../fixtures/settings/edge-compatible-config.yaml");
        let parsed = config_codec()
            .parse_config_yaml(fixture)
            .expect("edge-compatible YAML fixture parses");

        assert!(matches!(parsed.asr_provider, AsrProvider::LocalWhisper));
        assert_eq!(parsed.audio_settings.sample_rate, 48_000);
        assert_eq!(parsed.audio_settings.channels, 2);
        assert_eq!(parsed.diarization.max_speakers, None);
        assert_eq!(parsed.file_logging, Some(true));
        assert!(!parsed.speak_aloud);
        assert!(parsed.streaming_prefill);

        let rewritten = config_codec()
            .serialize_config_yaml(&parsed)
            .expect("edge-compatible fixture rewrites");
        for syntax in [
            "#",
            "&audio_defaults",
            "*local_provider",
            "<<:",
            "future_alias_budget",
        ] {
            assert!(
                !rewritten.contains(syntax),
                "known-schema writeback should normalize comments, anchors, merge keys, aliases, and unknown fields: {rewritten}"
            );
        }

        let candidate = SaphyrCandidateConfigCodec
            .parse_config_yaml(fixture)
            .expect("serde-saphyr candidate parses the accepted edge fixture");
        assert_semantically_equal_settings(&parsed, &candidate);
    }

    #[test]
    fn config_codec_parses_tagged_scalar_fixture_but_rewrites_without_tags() {
        let fixture = include_str!("../../fixtures/settings/tagged-config.yaml");
        let parsed = config_codec()
            .parse_config_yaml(fixture)
            .expect("serde_yaml accepts explicit tags on compatible scalar values");

        assert_eq!(parsed.demo_mode, Some(false));
        let rewritten = config_codec()
            .serialize_config_yaml(&parsed)
            .expect("tagged scalar fixture rewrites");
        assert!(
            !rewritten.contains("!audio-graph/tagged"),
            "known-schema writeback should not preserve explicit tags: {rewritten}"
        );

        let candidate = SaphyrCandidateConfigCodec
            .parse_config_yaml(fixture)
            .expect("serde-saphyr candidate matches tagged scalar acceptance");
        assert_semantically_equal_settings(&parsed, &candidate);
    }

    #[test]
    fn config_codec_defaults_missing_yaml_fields() {
        let parsed = config_codec()
            .parse_config_yaml("{}")
            .expect("empty YAML maps to default settings");

        assert_eq!(
            parsed.audio_settings.sample_rate,
            AppSettings::default().audio_settings.sample_rate
        );
        assert_eq!(parsed.diarization, DiarizationSettings::default());
        assert!(!parsed.speak_aloud);
        assert!(!parsed.streaming_prefill);
        assert_eq!(parsed.demo_mode, None);
    }

    #[test]
    fn config_codec_rejects_corrupt_yaml_fixture() {
        let err = config_codec()
            .parse_config_yaml(include_str!("../../fixtures/settings/corrupt-config.yaml"))
            .expect_err("corrupt config fixture fails");

        assert!(err.contains("Failed to parse config.yaml"));
    }

    #[test]
    fn config_codec_rejects_unknown_provider_type() {
        let err = config_codec()
            .parse_config_yaml(include_str!(
                "../../fixtures/settings/unknown-provider-config.yaml"
            ))
            .expect_err("unknown enum provider type must fail");

        assert!(err.contains("Failed to parse config.yaml"));
        assert!(
            err.contains("future_provider"),
            "error should keep the unknown non-secret provider id visible: {err}"
        );
    }

    #[test]
    fn config_codec_rejects_documented_yaml_edge_breakers() {
        for (name, fixture) in [
            (
                "duplicate keys",
                include_str!("../../fixtures/settings/duplicate-key-config.yaml"),
            ),
            (
                "multi-document YAML",
                include_str!("../../fixtures/settings/multi-document-config.yaml"),
            ),
        ] {
            let current = config_codec().parse_config_yaml(fixture);
            assert!(
                current.is_err(),
                "serde_yaml oracle should reject {name} fixture"
            );

            let candidate = SaphyrCandidateConfigCodec.parse_config_yaml(fixture);
            assert!(
                candidate.is_err(),
                "serde-saphyr candidate should match serde_yaml rejection for {name}; any future delta must be documented before parser migration"
            );
        }
    }

    #[test]
    fn config_codec_documents_saphyr_yaml11_boolean_delta() {
        let fixture = include_str!("../../fixtures/settings/yaml11-boolean-config.yaml");
        let current = config_codec().parse_config_yaml(fixture);
        assert!(
            current.is_err(),
            "serde_yaml oracle currently rejects YAML 1.1 boolean spellings"
        );

        let candidate = SaphyrCandidateConfigCodec
            .parse_config_yaml(fixture)
            .expect("serde-saphyr accepts YAML 1.1 boolean spellings");
        assert_eq!(candidate.file_logging, Some(true));
        assert!(!candidate.speak_aloud);
        assert!(candidate.streaming_prefill);
    }

    #[test]
    fn config_codec_candidate_matches_serde_yaml_fixture_semantics() {
        let current = include_str!("../../fixtures/settings/current-config.yaml");
        let unknown_fields = include_str!("../../fixtures/settings/unknown-fields-config.yaml");
        let edge_compatible = include_str!("../../fixtures/settings/edge-compatible-config.yaml");
        let tagged = include_str!("../../fixtures/settings/tagged-config.yaml");

        for fixture in [current, unknown_fields, edge_compatible, tagged, "{}"] {
            let serde_yaml_settings = config_codec()
                .parse_config_yaml(fixture)
                .expect("serde_yaml fixture parse");
            let candidate_settings = SaphyrCandidateConfigCodec
                .parse_config_yaml(fixture)
                .expect("serde-saphyr fixture parse");

            assert_semantically_equal_settings(&serde_yaml_settings, &candidate_settings);
        }

        let legacy = include_str!("../../fixtures/settings/legacy-settings.json");
        let serde_yaml_legacy = config_codec()
            .parse_legacy_json(legacy)
            .expect("serde_json legacy parse");
        let candidate_legacy = SaphyrCandidateConfigCodec
            .parse_legacy_json(legacy)
            .expect("candidate legacy parse");
        assert_semantically_equal_settings(&serde_yaml_legacy, &candidate_legacy);
    }

    #[test]
    fn config_codec_candidate_rejects_same_breaking_fixtures() {
        for fixture in [
            include_str!("../../fixtures/settings/corrupt-config.yaml"),
            include_str!("../../fixtures/settings/unknown-provider-config.yaml"),
            include_str!("../../fixtures/settings/duplicate-key-config.yaml"),
            include_str!("../../fixtures/settings/multi-document-config.yaml"),
        ] {
            assert!(
                config_codec().parse_config_yaml(fixture).is_err(),
                "current codec must reject breaking fixture"
            );
            assert!(
                SaphyrCandidateConfigCodec
                    .parse_config_yaml(fixture)
                    .is_err(),
                "candidate codec must reject breaking fixture"
            );
        }
    }

    #[test]
    fn config_codec_candidate_writeback_is_redacted_and_current_readable() {
        let settings = AppSettings {
            asr_provider: AsrProvider::Api {
                endpoint: "https://api.groq.com/openai/v1".into(),
                api_key: "candidate-asr-secret".into(),
                model: "whisper-large-v3".into(),
            },
            llm_provider: LlmProvider::OpenRouter {
                model: "openai/gpt-5.2".into(),
                base_url: "https://openrouter.ai/api/v1".into(),
                provider_order: Some(vec!["openai".into()]),
                include_usage_in_stream: true,
                api_key: "candidate-openrouter-secret".into(),
            },
            llm_api_config: Some(LlmApiConfig {
                endpoint: "https://api.fireworks.ai/inference/v1".into(),
                api_key: Some("candidate-llm-config-secret".into()),
                model: "accounts/fireworks/models/kimi-k2-instruct".into(),
                max_tokens: 1024,
                temperature: 0.2,
            }),
            gemini: GeminiSettings {
                auth: GeminiAuthMode::ApiKey {
                    api_key: "candidate-gemini-secret".into(),
                },
                model: default_gemini_model(),
                voice: String::new(),
            },
            ..AppSettings::default()
        };
        let secrets = [
            "candidate-asr-secret",
            "candidate-openrouter-secret",
            "candidate-llm-config-secret",
            "candidate-gemini-secret",
        ];

        let current_yaml = config_codec()
            .serialize_config_yaml(&settings)
            .expect("current codec serializes");
        let candidate_yaml = SaphyrCandidateConfigCodec
            .serialize_config_yaml(&settings)
            .expect("candidate codec serializes");

        assert_yaml_has_no_inline_secrets(&current_yaml, &secrets);
        assert_yaml_has_no_inline_secrets(&candidate_yaml, &secrets);

        let current_from_candidate = config_codec()
            .parse_config_yaml(&candidate_yaml)
            .expect("current codec parses candidate writeback");
        let candidate_from_current = SaphyrCandidateConfigCodec
            .parse_config_yaml(&current_yaml)
            .expect("candidate codec parses current writeback");

        assert!(!has_inline_credentials(&current_from_candidate));
        assert!(!has_inline_credentials(&candidate_from_current));
        assert_semantically_equal_settings(&current_from_candidate, &candidate_from_current);
    }

    #[test]
    fn config_codec_unknown_fields_are_tolerated_but_not_preserved_on_writeback() {
        let parsed = config_codec()
            .parse_config_yaml(include_str!(
                "../../fixtures/settings/unknown-fields-config.yaml"
            ))
            .expect("unknown fields are tolerated");

        let rewritten = config_codec()
            .serialize_config_yaml(&parsed)
            .expect("known-schema writeback succeeds");

        assert!(
            !rewritten.contains("future_"),
            "current writeback is a known-schema rewrite, not a comment/unknown-field preserving edit"
        );
    }

    #[test]
    fn load_settings_from_paths_prefers_canonical_config_yaml() {
        let dir = unique_tempdir("canonical-wins");
        let config_path = dir.join("config").join("config.yaml");
        let legacy_path = dir.join("data").join("settings.json");

        let settings = AppSettings {
            demo_mode: Some(false),
            audio_settings: AudioSettings {
                sample_rate: 44_100,
                channels: 2,
            },
            ..AppSettings::default()
        };
        save_settings_to_path(&config_path, &settings).expect("write canonical config.yaml");

        let legacy = AppSettings {
            demo_mode: Some(true),
            audio_settings: AudioSettings {
                sample_rate: 48_000,
                channels: 1,
            },
            ..AppSettings::default()
        };
        fs::create_dir_all(legacy_path.parent().expect("legacy parent")).expect("legacy dir");
        fs::write(
            &legacy_path,
            serde_json::to_string(&redacted_settings(&legacy)).expect("legacy json"),
        )
        .expect("write legacy settings.json");

        let imported = Cell::new(false);
        let loaded = load_settings_from_paths_with_status(&config_path, Some(&legacy_path), |_| {
            imported.set(true);
            Ok(())
        });

        assert!(
            !imported.get(),
            "legacy import must not run when YAML exists"
        );
        assert_eq!(loaded.status, SettingsLoadStatus::CanonicalOk);
        let loaded = loaded.settings;
        assert_eq!(loaded.demo_mode, Some(false));
        assert_eq!(loaded.audio_settings.sample_rate, 44_100);
        assert_eq!(loaded.audio_settings.channels, 2);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_settings_reports_defaults_missing_status() {
        let dir = unique_tempdir("missing-settings");
        let config_path = dir.join("config").join("config.yaml");

        let loaded = load_settings_from_paths_with_status(&config_path, None, |_| {
            panic!("missing settings must not persist import")
        });

        assert_eq!(loaded.status, SettingsLoadStatus::DefaultsMissing);
        assert!(loaded.status.allows_automatic_writeback());
        assert_eq!(loaded.settings.demo_mode, AppSettings::default().demo_mode);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_settings_json_import_writes_canonical_config_yaml() {
        let dir = unique_tempdir("legacy-import");
        let config_path = dir.join("config").join("config.yaml");
        let legacy_path = dir.join("data").join("settings.json");

        let legacy = AppSettings {
            demo_mode: Some(true),
            audio_settings: AudioSettings {
                sample_rate: 48_000,
                channels: 1,
            },
            ..AppSettings::default()
        };
        fs::create_dir_all(legacy_path.parent().expect("legacy parent")).expect("legacy dir");
        fs::write(
            &legacy_path,
            serde_json::to_string(&redacted_settings(&legacy)).expect("legacy json"),
        )
        .expect("write legacy settings.json");

        let loaded =
            load_settings_from_paths_with_status(&config_path, Some(&legacy_path), |settings| {
                save_settings_to_path(&config_path, settings)
            });

        assert_eq!(loaded.status, SettingsLoadStatus::LegacyImported);
        let loaded = loaded.settings;
        assert_eq!(loaded.demo_mode, Some(true));
        assert_eq!(loaded.audio_settings.sample_rate, 48_000);
        assert_eq!(loaded.audio_settings.channels, 1);

        let written = fs::read_to_string(&config_path).expect("canonical config.yaml exists");
        let parsed = config_codec()
            .parse_config_yaml(&written)
            .expect("imported YAML parses");
        assert_eq!(parsed.demo_mode, Some(true));
        assert_eq!(parsed.audio_settings.sample_rate, 48_000);
        assert_eq!(parsed.audio_settings.channels, 1);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_config_yaml_falls_back_without_importing_legacy_json() {
        let dir = unique_tempdir("corrupt-config");
        let config_path = dir.join("config").join("config.yaml");
        let legacy_path = dir.join("data").join("settings.json");

        fs::create_dir_all(config_path.parent().expect("config parent")).expect("config dir");
        fs::write(&config_path, "asr_provider: [not valid").expect("write corrupt config");

        let legacy = AppSettings {
            demo_mode: Some(true),
            audio_settings: AudioSettings {
                sample_rate: 44_100,
                channels: 1,
            },
            ..AppSettings::default()
        };
        fs::create_dir_all(legacy_path.parent().expect("legacy parent")).expect("legacy dir");
        fs::write(
            &legacy_path,
            serde_json::to_string(&redacted_settings(&legacy)).expect("legacy json"),
        )
        .expect("write legacy settings.json");

        let imported = Cell::new(false);
        let loaded = load_settings_from_paths_with_status(&config_path, Some(&legacy_path), |_| {
            imported.set(true);
            Ok(())
        });

        assert!(
            !imported.get(),
            "corrupt canonical YAML must not be replaced by legacy JSON"
        );
        assert_eq!(loaded.status, SettingsLoadStatus::CanonicalErrorDefaulted);
        assert!(!loaded.status.allows_automatic_writeback());
        let loaded = loaded.settings;
        assert_eq!(loaded.demo_mode, AppSettings::default().demo_mode);
        assert_eq!(
            loaded.audio_settings.sample_rate,
            AppSettings::default().audio_settings.sample_rate
        );
        assert_eq!(
            fs::read_to_string(&config_path).expect("config remains on disk"),
            "asr_provider: [not valid"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_config_status_blocks_startup_demo_mode_writeback() {
        let dir = unique_tempdir("corrupt-demo-writeback");
        let config_path = dir.join("config").join("config.yaml");
        fs::create_dir_all(config_path.parent().expect("config parent")).expect("config dir");
        fs::write(&config_path, "asr_provider: [not valid").expect("write corrupt config");

        let loaded = load_settings_from_paths_with_status(&config_path, None, |_| Ok(()));
        assert_eq!(loaded.status, SettingsLoadStatus::CanonicalErrorDefaulted);

        let mut settings = loaded.settings;
        let store = crate::credentials::CredentialStore::default();
        assert!(
            apply_first_launch_demo_mode(&mut settings, &store),
            "demo mode would normally persist a first-launch decision"
        );
        if loaded.status.allows_automatic_writeback() {
            save_settings_to_path(&config_path, &settings).expect("writeback");
        }

        assert_eq!(
            fs::read_to_string(&config_path).expect("config remains on disk"),
            "asr_provider: [not valid"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_config_status_blocks_startup_demo_mode_writeback_with_credentials_present() {
        let dir = unique_tempdir("corrupt-demo-credential-writeback");
        let config_path = dir.join("config").join("config.yaml");
        fs::create_dir_all(config_path.parent().expect("config parent")).expect("config dir");
        fs::write(&config_path, "asr_provider: [not valid").expect("write corrupt config");

        let loaded = load_settings_from_paths_with_status(&config_path, None, |_| Ok(()));
        assert_eq!(loaded.status, SettingsLoadStatus::CanonicalErrorDefaulted);

        let mut settings = loaded.settings;
        let mut store = crate::credentials::CredentialStore::default();
        store.openai_api_key = Some("present-test-credential".to_string());
        assert!(
            apply_first_launch_demo_mode(&mut settings, &store),
            "demo mode records a non-demo decision when credentials exist"
        );
        assert_eq!(settings.demo_mode, Some(false));
        if loaded.status.allows_automatic_writeback() {
            save_settings_to_path(&config_path, &settings).expect("writeback");
        }

        assert_eq!(
            fs::read_to_string(&config_path).expect("config remains on disk"),
            "asr_provider: [not valid"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_config_status_blocks_logging_patch_writeback() {
        let dir = unique_tempdir("corrupt-logging-writeback");
        let config_path = dir.join("config").join("config.yaml");
        fs::create_dir_all(config_path.parent().expect("config parent")).expect("config dir");
        fs::write(&config_path, "asr_provider: [not valid").expect("write corrupt config");

        let loaded = load_settings_from_paths_with_status(&config_path, None, |_| Ok(()));
        assert_eq!(loaded.status, SettingsLoadStatus::CanonicalErrorDefaulted);

        if loaded.status.allows_automatic_writeback() {
            let mut on_disk = loaded.settings;
            on_disk.file_logging = Some(false);
            on_disk.log_file_mode = Some("overwrite".to_string());
            on_disk.log_level = Some("debug".to_string());
            save_settings_to_path(&config_path, &on_disk).expect("writeback");
        }

        assert_eq!(
            fs::read_to_string(&config_path).expect("config remains on disk"),
            "asr_provider: [not valid"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn normal_save_refuses_to_overwrite_corrupt_config_yaml() {
        let dir = unique_tempdir("corrupt-normal-save");
        let config_path = dir.join("config").join("config.yaml");
        fs::create_dir_all(config_path.parent().expect("config parent")).expect("config dir");
        fs::write(&config_path, "asr_provider: [not valid").expect("write corrupt config");

        let err = ensure_existing_config_is_parseable_for_write(&config_path)
            .expect_err("corrupt config should require explicit recovery");
        assert!(err.contains("Refusing to overwrite existing config.yaml"));

        assert_eq!(
            fs::read_to_string(&config_path).expect("config remains on disk"),
            "asr_provider: [not valid"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_settings_to_path_writes_redacted_yaml() {
        let dir = unique_tempdir("redacted-write");
        let config_path = dir.join("config").join("config.yaml");
        let settings = AppSettings {
            asr_provider: AsrProvider::DeepgramStreaming {
                api_key: "dg-file-secret".into(),
                model: "nova-3".into(),
                enable_diarization: true,
                endpointing_ms: 300,
                utterance_end_ms: 1000,
                vad_events: true,
                eot_threshold: 0.5,
                eager_eot_threshold: 0.0,
                eot_timeout_ms: 0,
                max_speakers: 0,
            },
            ..AppSettings::default()
        };

        save_settings_to_path(&config_path, &settings).expect("write config.yaml");
        let written = fs::read_to_string(&config_path).expect("read config.yaml");

        assert!(
            !written.contains("dg-file-secret"),
            "config.yaml must not persist inline secrets"
        );
        let parsed = config_codec()
            .parse_config_yaml(&written)
            .expect("redacted YAML parses");
        assert!(!has_inline_credentials(&parsed));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn openrouter_routing_policy_round_trips_without_serializing_api_key() {
        let settings = AppSettings {
            llm_provider: LlmProvider::OpenRouter {
                model: "openai/gpt-5.2".into(),
                base_url: "https://openrouter.ai/api/v1".into(),
                provider_order: Some(vec!["legacy-provider".into()]),
                include_usage_in_stream: true,
                api_key: "sk-openrouter-settings-secret".into(),
            },
            openrouter_routing_policy: Some(OpenRouterRoutingPolicy {
                order: vec!["cerebras".into(), "groq".into()],
                only: vec!["cerebras".into(), "groq".into()],
                allow_fallbacks: Some(false),
                data_collection: Some(crate::llm::openrouter::OpenRouterDataCollectionPolicy::Deny),
                zdr: Some(true),
                ..OpenRouterRoutingPolicy::default()
            }),
            ..AppSettings::default()
        };

        let raw_json = serde_json::to_string(&settings).expect("settings serialize to JSON");
        assert!(
            !raw_json.contains("sk-openrouter-settings-secret"),
            "OpenRouter api_key must never be serialized: {raw_json}"
        );
        assert!(raw_json.contains("openrouter_routing_policy"));
        assert!(raw_json.contains("\"allow_fallbacks\":false"));

        let yaml = config_codec()
            .serialize_config_yaml(&settings)
            .expect("settings serialize to YAML");
        assert_yaml_has_no_inline_secrets(&yaml, &["sk-openrouter-settings-secret"]);
        assert!(yaml.contains("openrouter_routing_policy:"));
        assert!(yaml.contains("allow_fallbacks: false"));
        assert!(yaml.contains("data_collection: deny"));

        let parsed = config_codec()
            .parse_config_yaml(&yaml)
            .expect("serialized settings parse");
        assert!(!has_inline_credentials(&parsed));
        assert_eq!(
            parsed.openrouter_routing_policy,
            settings.openrouter_routing_policy
        );
        match parsed.llm_provider {
            LlmProvider::OpenRouter {
                api_key,
                provider_order,
                ..
            } => {
                assert!(api_key.is_empty());
                assert_eq!(provider_order, Some(vec!["legacy-provider".to_string()]));
            }
            other => panic!("unexpected LLM provider: {:?}", other),
        }
    }

    #[test]
    fn openai_realtime_api_key_is_never_serialized_and_redacts() {
        let settings = AppSettings {
            asr_provider: AsrProvider::OpenAiRealtimeTranscription {
                api_key: "sk-openai-secret".into(),
                model: "gpt-realtime-whisper".into(),
                language: Some("en".into()),
            },
            ..AppSettings::default()
        };

        // has_inline_credentials must see the inline key.
        assert!(has_inline_credentials(&settings));

        // Serializing even the *unredacted* settings must not leak the key,
        // because the field is `skip_serializing`.
        let raw_json = serde_json::to_string(&settings).unwrap();
        assert!(
            !raw_json.contains("sk-openai-secret"),
            "api_key must never be serialized (skip_serializing): {raw_json}"
        );
        // The non-secret routing fields are still present.
        assert!(raw_json.contains("openai_realtime"));
        assert!(raw_json.contains("gpt-realtime-whisper"));

        // Redaction clears the in-memory copy too.
        let redacted = redacted_settings(&settings);
        assert!(!has_inline_credentials(&redacted));
        match redacted.asr_provider {
            AsrProvider::OpenAiRealtimeTranscription { api_key, .. } => assert!(api_key.is_empty()),
            other => panic!("unexpected ASR provider: {:?}", other),
        }
    }

    #[test]
    fn openai_realtime_hydrates_from_openai_api_key_slot() {
        let settings = AppSettings {
            asr_provider: AsrProvider::OpenAiRealtimeTranscription {
                api_key: String::new(),
                model: "gpt-realtime-whisper".into(),
                language: None,
            },
            ..AppSettings::default()
        };
        let mut store = crate::credentials::CredentialStore::default();
        // Reuses the shared `openai_api_key` slot.
        store.openai_api_key = Some("sk-from-store".into());

        let hydrated = hydrate_runtime_credentials(&settings, &store);
        match hydrated.asr_provider {
            AsrProvider::OpenAiRealtimeTranscription { api_key, .. } => {
                assert_eq!(api_key, "sk-from-store")
            }
            other => panic!("unexpected ASR provider: {:?}", other),
        }
    }

    #[test]
    fn soniox_api_key_uses_dedicated_credential_slot_and_redacts() {
        let settings = AppSettings {
            asr_provider: AsrProvider::Soniox {
                api_key: "sx-inline-secret".into(),
                model: crate::asr::soniox::DEFAULT_MODEL.into(),
                enable_diarization: true,
                enable_language_identification: true,
                language_hints: vec!["en".into()],
                max_speakers: 3,
            },
            ..AppSettings::default()
        };

        assert!(has_inline_credentials(&settings));
        let raw_json = serde_json::to_string(&settings).unwrap();
        assert!(
            !raw_json.contains("sx-inline-secret"),
            "soniox api_key must never be serialized: {raw_json}"
        );
        assert!(raw_json.contains("soniox"));
        assert!(raw_json.contains(crate::asr::soniox::DEFAULT_MODEL));

        let updates = inline_credential_updates(&settings);
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].key, "soniox_api_key");
        assert_eq!(updates[0].value, "sx-inline-secret");

        let redacted = redacted_settings(&settings);
        assert!(!has_inline_credentials(&redacted));
        match &redacted.asr_provider {
            AsrProvider::Soniox { api_key, .. } => assert!(api_key.is_empty()),
            other => panic!("unexpected ASR provider: {:?}", other),
        }

        let mut store = crate::credentials::CredentialStore::default();
        store.soniox_api_key = Some("sx-from-store".into());
        let hydrated = hydrate_runtime_credentials(&redacted, &store);
        match hydrated.asr_provider {
            AsrProvider::Soniox {
                api_key,
                model,
                enable_diarization,
                enable_language_identification,
                language_hints,
                max_speakers,
            } => {
                assert_eq!(api_key, "sx-from-store");
                assert_eq!(model, crate::asr::soniox::DEFAULT_MODEL);
                assert!(enable_diarization);
                assert!(enable_language_identification);
                assert_eq!(language_hints, vec!["en"]);
                assert_eq!(max_speakers, 3);
            }
            other => panic!("unexpected ASR provider: {:?}", other),
        }
    }

    #[test]
    fn endpoint_credential_routing_covers_known_openai_compatible_hosts() {
        for (endpoint, key) in [
            ("https://api.openai.com/v1", "openai_api_key"),
            (CEREBRAS_BASE_URL, "cerebras_api_key"),
            ("https://api.cerebras.ai/v1/", "cerebras_api_key"),
            ("https://openrouter.ai/api/v1", "openrouter_api_key"),
            ("https://api.groq.com/openai/v1", "groq_api_key"),
            ("https://api.together.xyz/v1", "together_api_key"),
            ("https://api.fireworks.ai/inference/v1", "fireworks_api_key"),
            (
                "https://generativelanguage.googleapis.com/v1beta/openai",
                "gemini_api_key",
            ),
        ] {
            assert_eq!(
                credential_key_for_endpoint(endpoint),
                key,
                "{endpoint} should use {key}"
            );
        }
    }

    #[test]
    fn generic_openrouter_api_provider_hydrates_from_openrouter_slot() {
        let settings = AppSettings {
            llm_provider: LlmProvider::Api {
                endpoint: "https://openrouter.ai/api/v1".into(),
                api_key: String::new(),
                model: "openai/gpt-4o-mini".into(),
            },
            ..AppSettings::default()
        };
        let mut store = crate::credentials::CredentialStore::default();
        store.openai_api_key = Some("sk-openai".into());
        store.openrouter_api_key = Some("sk-or-store".into());

        let hydrated = hydrate_runtime_credentials(&settings, &store);
        match hydrated.llm_provider {
            LlmProvider::Api { api_key, .. } => assert_eq!(api_key, "sk-or-store"),
            other => panic!("unexpected LLM provider: {:?}", other),
        }
    }

    #[test]
    fn runtime_credentials_hydrate_from_store_without_affecting_redacted_copy() {
        let settings = AppSettings {
            asr_provider: AsrProvider::DeepgramStreaming {
                api_key: String::new(),
                model: "nova-3".into(),
                enable_diarization: true,
                endpointing_ms: 300,
                utterance_end_ms: 1000,
                vad_events: true,
                eot_threshold: 0.5,
                eager_eot_threshold: 0.0,
                eot_timeout_ms: 0,
                max_speakers: 2,
            },
            llm_provider: LlmProvider::Api {
                endpoint: "https://api.groq.com/openai/v1".into(),
                api_key: String::new(),
                model: "llama-3.1-8b-instant".into(),
            },
            gemini: GeminiSettings {
                auth: GeminiAuthMode::ApiKey {
                    api_key: String::new(),
                },
                model: default_gemini_model(),
                voice: String::new(),
            },
            ..AppSettings::default()
        };
        let mut store = crate::credentials::CredentialStore::default();
        store.deepgram_api_key = Some("dg-store".into());
        store.groq_api_key = Some("gsk-store".into());
        store.gemini_api_key = Some("AIza-store".into());

        let hydrated = hydrate_runtime_credentials(&settings, &store);
        match hydrated.asr_provider {
            AsrProvider::DeepgramStreaming { api_key, .. } => assert_eq!(api_key, "dg-store"),
            other => panic!("unexpected ASR provider: {:?}", other),
        }
        match hydrated.llm_provider {
            LlmProvider::Api { api_key, .. } => assert_eq!(api_key, "gsk-store"),
            other => panic!("unexpected LLM provider: {:?}", other),
        }
        match hydrated.gemini.auth {
            GeminiAuthMode::ApiKey { api_key } => assert_eq!(api_key, "AIza-store"),
            other => panic!("unexpected Gemini auth mode: {:?}", other),
        }

        assert!(!has_inline_credentials(&settings));
    }

    /// Regression: the Settings footer "Save" re-serializes the whole
    /// `AppSettings` from a frontend store object that historically never
    /// carried `analytics_enabled`, so the field arrived as `None`. Because
    /// `analytics_enabled` is `#[serde(skip_serializing_if = "Option::is_none")]`,
    /// a blind whole-struct write silently DROPPED the key from `config.yaml`,
    /// clobbering the `analytics_enabled: true` that the separate
    /// `set_analytics_enabled` command had written. Startup then read `None ->
    /// false` and Sentry never initialized.
    ///
    /// The disk-write path must preserve the on-disk `analytics_enabled` when
    /// the incoming payload omits it (`None`), making the analytics toggle the
    /// sole owner of that field and defending against ANY caller that drops it.
    #[test]
    fn save_settings_preserves_on_disk_analytics_when_payload_omits_it() {
        let dir = unique_tempdir("analytics-preserve");
        let config_path = dir.join("config.yaml");

        // 1. The analytics toggle (`set_analytics_enabled`) persists true.
        let with_analytics = AppSettings {
            analytics_enabled: Some(true),
            ..AppSettings::default()
        };
        save_settings_to_path(&config_path, &with_analytics).expect("write analytics=true");

        // The on-disk YAML must actually contain the enabled flag.
        let yaml_after_toggle =
            fs::read_to_string(&config_path).expect("read config.yaml after toggle");
        assert!(
            yaml_after_toggle.contains("analytics_enabled: true"),
            "config.yaml must persist analytics_enabled: true after the toggle; got:\n{yaml_after_toggle}"
        );

        // 2. The footer "Save" re-serializes the whole struct WITHOUT the
        //    analytics field (the store never carried it -> None).
        let footer_save_payload = AppSettings {
            analytics_enabled: None,
            // A user edit to some unrelated field the footer legitimately owns.
            log_level: Some("debug".to_string()),
            ..AppSettings::default()
        };
        save_settings_to_path(&config_path, &footer_save_payload)
            .expect("write footer save payload");

        // 3. The on-disk true MUST be preserved, not dropped.
        let yaml_after_footer_save =
            fs::read_to_string(&config_path).expect("read config.yaml after footer save");
        assert!(
            yaml_after_footer_save.contains("analytics_enabled: true"),
            "footer Save with analytics_enabled=None must NOT drop the on-disk true; got:\n{yaml_after_footer_save}"
        );

        // 4. Startup read (load) sees the preserved value.
        let loaded = load_settings_from_paths_with_status(&config_path, None, |_| Ok(()));
        assert_eq!(
            loaded.settings.analytics_enabled,
            Some(true),
            "startup must read the preserved analytics_enabled: true"
        );

        // 5. The unrelated footer-owned edit still lands (no over-preservation).
        assert_eq!(loaded.settings.log_level.as_deref(), Some("debug"));

        // 6. The toggle stays authoritative in BOTH directions: an explicit
        //    Some(false) (analytics turned OFF) must overwrite the on-disk true
        //    (no reverse clobber where preservation pins it ON forever).
        let toggle_off = AppSettings {
            analytics_enabled: Some(false),
            ..AppSettings::default()
        };
        save_settings_to_path(&config_path, &toggle_off).expect("write analytics=false");
        let after_off = load_settings_from_paths_with_status(&config_path, None, |_| Ok(()));
        assert_eq!(
            after_off.settings.analytics_enabled,
            Some(false),
            "an explicit toggle-off must overwrite the on-disk true"
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
