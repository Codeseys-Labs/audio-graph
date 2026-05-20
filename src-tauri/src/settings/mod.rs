//! Application settings — persistence layer for user configuration.
//!
//! Settings are stored as JSON in the app data directory and loaded
//! at startup. If the file is missing or unparseable, defaults are used.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tauri::Manager;

// ---------------------------------------------------------------------------
// Helper default functions
// ---------------------------------------------------------------------------

const FALLBACK_SAMPLE_RATE: u32 = 48000;
const FALLBACK_CHANNELS: u16 = 1;
const FALLBACK_WHISPER_MODEL: &str = "ggml-small.en.bin";

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
fn default_true() -> bool {
    true
}
fn default_sherpa_model() -> String {
    "streaming-zipformer-en-20M".to_string()
}

// ---------------------------------------------------------------------------
// AWS credential source
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
        access_key: String,
    },
}

// ---------------------------------------------------------------------------
// ASR provider
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AsrProvider {
    #[serde(rename = "local_whisper")]
    #[default]
    LocalWhisper,
    #[serde(rename = "api")]
    Api {
        endpoint: String,
        #[serde(default, skip_serializing)]
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
    },
    #[serde(rename = "assemblyai")]
    AssemblyAI {
        #[serde(default, skip_serializing)]
        api_key: String,
        #[serde(default = "default_true")]
        enable_diarization: bool,
    },
    #[serde(rename = "sherpa_onnx")]
    SherpaOnnx {
        #[serde(default = "default_sherpa_model")]
        model_dir: String,
        #[serde(default = "default_true")]
        enable_endpoint_detection: bool,
    },
}

// ---------------------------------------------------------------------------
// LLM API config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmApiConfig {
    pub endpoint: String,
    #[serde(default)]
    #[serde(skip_serializing)]
    pub api_key: Option<String>,
    pub model: String,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default = "default_temperature")]
    pub temperature: f32,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum LlmProvider {
    #[serde(rename = "local_llama")]
    LocalLlama,
    #[serde(rename = "api")]
    Api {
        endpoint: String,
        #[serde(default, skip_serializing)]
        api_key: String,
        model: String,
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
/// telephony values so a hand-edited `settings.json` can't coax us into
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum GeminiAuthMode {
    #[serde(rename = "api_key")]
    ApiKey {
        #[serde(default, skip_serializing)]
        api_key: String,
    },
    #[serde(rename = "vertex_ai")]
    VertexAI {
        project_id: String,
        location: String,
        #[serde(default)]
        service_account_path: Option<String>,
    },
}

impl Default for GeminiAuthMode {
    fn default() -> Self {
        Self::ApiKey {
            api_key: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiSettings {
    #[serde(default)]
    pub auth: GeminiAuthMode,
    #[serde(default = "default_gemini_model")]
    pub model: String,
}

fn default_gemini_model() -> String {
    "gemini-3.1-flash-live-preview".to_string()
}

impl Default for GeminiSettings {
    fn default() -> Self {
        Self {
            auth: GeminiAuthMode::default(),
            model: default_gemini_model(),
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

// ---------------------------------------------------------------------------
// Top-level settings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(default)]
    pub asr_provider: AsrProvider,
    #[serde(default = "default_whisper_model")]
    pub whisper_model: String,
    #[serde(default)]
    pub llm_provider: LlmProvider,
    #[serde(default)]
    pub llm_api_config: Option<LlmApiConfig>,
    #[serde(default)]
    pub audio_settings: AudioSettings,
    #[serde(default)]
    pub gemini: GeminiSettings,
    /// Selected TTS provider. Default `None` keeps the chat reply path
    /// text-only and avoids introducing a backend dependency on cloud TTS
    /// for users who don't want it. See plan A1 + ADR-0004.
    #[serde(default)]
    pub tts_provider: TtsProvider,
    /// Runtime log-verbosity preference: one of
    /// "off" | "error" | "warn" | "info" | "debug" | "trace".
    ///
    /// `None` means "not set — fall back to the default (info) unless
    /// the user set RUST_LOG at startup". `skip_serializing_if` keeps the
    /// field out of written YAML/JSON when unset, so older settings files
    /// stay byte-identical after a round-trip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_level: Option<String>,
    /// Demo mode — set once on first launch when no cloud credentials are
    /// present. `None` means "not yet decided" (the setup hook will make
    /// the call on the next launch); `Some(true)` means the app is wired
    /// for local-only providers and should show the demo banner until
    /// local models are downloaded; `Some(false)` means the user has
    /// configured something real (either via ExpressSetup or directly).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub demo_mode: Option<bool>,
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
            llm_api_config: None,
            audio_settings: AudioSettings::default(),
            gemini: GeminiSettings::default(),
            tts_provider: TtsProvider::default(),
            log_level: Some("info".to_string()),
            demo_mode: None,
        }
    }
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

/// Pick the credential slot for OpenAI-compatible HTTP providers.
///
/// Settings only store routing details such as endpoint/model. Secrets live in
/// `credentials.yaml`; for OpenAI-compatible providers the endpoint is the
/// stable provider discriminator we have available at runtime.
pub fn credential_key_for_endpoint(endpoint: &str) -> &'static str {
    let lower = endpoint.to_ascii_lowercase();
    if lower.contains("generativelanguage.googleapis.com") || lower.contains("gemini") {
        "gemini_api_key"
    } else if lower.contains("groq") {
        "groq_api_key"
    } else if lower.contains("together") {
        "together_api_key"
    } else if lower.contains("fireworks") {
        "fireworks_api_key"
    } else {
        // OpenAI, OpenRouter, Anthropic-compatible shims, vLLM with auth, and
        // unknown OpenAI-compatible endpoints share the generic bearer slot.
        "openai_api_key"
    }
}

fn credential_value_for_endpoint<'a>(
    endpoint: &str,
    store: &'a crate::credentials::CredentialStore,
) -> Option<&'a str> {
    match credential_key_for_endpoint(endpoint) {
        "gemini_api_key" => option_non_empty_secret(&store.gemini_api_key),
        "groq_api_key" => option_non_empty_secret(&store.groq_api_key),
        "together_api_key" => option_non_empty_secret(&store.together_api_key),
        "fireworks_api_key" => option_non_empty_secret(&store.fireworks_api_key),
        _ => option_non_empty_secret(&store.openai_api_key),
    }
}

fn save_secret_if_present(key: &str, value: &str) -> Result<(), String> {
    if let Some(secret) = non_empty_secret(value) {
        crate::credentials::set_credential(key, secret)
            .map_err(|e| format!("Failed to save {key}: {e}"))?;
    }
    Ok(())
}

/// Persist any legacy inline settings secrets into `credentials.yaml`.
///
/// This is intentionally tolerant of empty fields: empty values mean "no new
/// secret supplied" and must not wipe an existing credential.
pub fn persist_inline_credentials(settings: &AppSettings) -> Result<(), String> {
    match &settings.asr_provider {
        AsrProvider::Api {
            endpoint, api_key, ..
        } => save_secret_if_present(credential_key_for_endpoint(endpoint), api_key)?,
        AsrProvider::DeepgramStreaming { api_key, .. } => {
            save_secret_if_present("deepgram_api_key", api_key)?
        }
        AsrProvider::AssemblyAI { api_key, .. } => {
            save_secret_if_present("assemblyai_api_key", api_key)?
        }
        AsrProvider::AwsTranscribe {
            credential_source, ..
        } => {
            if let AwsCredentialSource::AccessKeys { access_key } = credential_source {
                save_secret_if_present("aws_access_key", access_key)?;
            }
        }
        AsrProvider::LocalWhisper | AsrProvider::SherpaOnnx { .. } => {}
    }

    match &settings.llm_provider {
        LlmProvider::Api {
            endpoint, api_key, ..
        } => save_secret_if_present(credential_key_for_endpoint(endpoint), api_key)?,
        LlmProvider::AwsBedrock {
            credential_source, ..
        } => {
            if let AwsCredentialSource::AccessKeys { access_key } = credential_source {
                save_secret_if_present("aws_access_key", access_key)?;
            }
        }
        LlmProvider::LocalLlama | LlmProvider::MistralRs { .. } => {}
    }

    if let Some(config) = &settings.llm_api_config {
        if let Some(api_key) = option_non_empty_secret(&config.api_key) {
            save_secret_if_present(credential_key_for_endpoint(&config.endpoint), api_key)?;
        }
    }

    match &settings.gemini.auth {
        GeminiAuthMode::ApiKey { api_key } => save_secret_if_present("gemini_api_key", api_key)?,
        GeminiAuthMode::VertexAI { .. } => {}
    }

    Ok(())
}

pub fn has_inline_credentials(settings: &AppSettings) -> bool {
    let asr_has_secret = match &settings.asr_provider {
        AsrProvider::Api { api_key, .. }
        | AsrProvider::DeepgramStreaming { api_key, .. }
        | AsrProvider::AssemblyAI { api_key, .. } => non_empty_secret(api_key).is_some(),
        AsrProvider::AwsTranscribe {
            credential_source, ..
        } => matches!(
            credential_source,
            AwsCredentialSource::AccessKeys { access_key }
                if non_empty_secret(access_key).is_some()
        ),
        AsrProvider::LocalWhisper | AsrProvider::SherpaOnnx { .. } => false,
    };

    let llm_has_secret = match &settings.llm_provider {
        LlmProvider::Api { api_key, .. } => non_empty_secret(api_key).is_some(),
        LlmProvider::AwsBedrock {
            credential_source, ..
        } => matches!(
            credential_source,
            AwsCredentialSource::AccessKeys { access_key }
                if non_empty_secret(access_key).is_some()
        ),
        LlmProvider::LocalLlama | LlmProvider::MistralRs { .. } => false,
    };

    let llm_config_has_secret = settings
        .llm_api_config
        .as_ref()
        .and_then(|config| option_non_empty_secret(&config.api_key))
        .is_some();

    let gemini_has_secret = match &settings.gemini.auth {
        GeminiAuthMode::ApiKey { api_key } => non_empty_secret(api_key).is_some(),
        GeminiAuthMode::VertexAI { .. } => false,
    };

    asr_has_secret || llm_has_secret || llm_config_has_secret || gemini_has_secret
}

/// Return a copy that is safe to write to `settings.json` or return over IPC.
pub fn redacted_settings(settings: &AppSettings) -> AppSettings {
    let mut redacted = settings.clone();

    match &mut redacted.asr_provider {
        AsrProvider::Api { api_key, .. }
        | AsrProvider::DeepgramStreaming { api_key, .. }
        | AsrProvider::AssemblyAI { api_key, .. } => api_key.clear(),
        AsrProvider::AwsTranscribe {
            credential_source, ..
        } => {
            if let AwsCredentialSource::AccessKeys { access_key } = credential_source {
                access_key.clear();
            }
        }
        AsrProvider::LocalWhisper | AsrProvider::SherpaOnnx { .. } => {}
    }

    match &mut redacted.llm_provider {
        LlmProvider::Api { api_key, .. } => api_key.clear(),
        LlmProvider::AwsBedrock {
            credential_source, ..
        } => {
            if let AwsCredentialSource::AccessKeys { access_key } = credential_source {
                access_key.clear();
            }
        }
        LlmProvider::LocalLlama | LlmProvider::MistralRs { .. } => {}
    }

    if let Some(config) = &mut redacted.llm_api_config {
        config.api_key = None;
    }

    if let GeminiAuthMode::ApiKey { api_key } = &mut redacted.gemini.auth {
        api_key.clear();
    }

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
        AsrProvider::AwsTranscribe {
            credential_source, ..
        } => {
            if let AwsCredentialSource::AccessKeys { access_key } = credential_source {
                if let Some(secret) = option_non_empty_secret(&store.aws_access_key) {
                    *access_key = secret.to_string();
                }
            }
        }
        AsrProvider::LocalWhisper | AsrProvider::SherpaOnnx { .. } => {}
    }

    match &mut hydrated.llm_provider {
        LlmProvider::Api {
            endpoint, api_key, ..
        } => {
            if let Some(secret) = credential_value_for_endpoint(endpoint, store) {
                *api_key = secret.to_string();
            }
        }
        LlmProvider::AwsBedrock {
            credential_source, ..
        } => {
            if let AwsCredentialSource::AccessKeys { access_key } = credential_source {
                if let Some(secret) = option_non_empty_secret(&store.aws_access_key) {
                    *access_key = secret.to_string();
                }
            }
        }
        LlmProvider::LocalLlama | LlmProvider::MistralRs { .. } => {}
    }

    if let Some(config) = &mut hydrated.llm_api_config {
        config.api_key =
            credential_value_for_endpoint(&config.endpoint, store).map(|secret| secret.to_string());
    }

    if let GeminiAuthMode::ApiKey { api_key } = &mut hydrated.gemini.auth {
        if let Some(secret) = option_non_empty_secret(&store.gemini_api_key) {
            *api_key = secret.to_string();
        }
    }

    hydrated
}

// ---------------------------------------------------------------------------
// Path resolution
// ---------------------------------------------------------------------------

pub fn get_settings_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data directory: {}", e))?;
    Ok(data_dir.join("settings.json"))
}

// ---------------------------------------------------------------------------
// Load / Save
// ---------------------------------------------------------------------------

pub fn load_settings(app: &tauri::AppHandle) -> AppSettings {
    match get_settings_path(app) {
        Ok(path) => {
            if path.exists() {
                match fs::read_to_string(&path) {
                    Ok(contents) => match serde_json::from_str::<AppSettings>(&contents) {
                        Ok(settings) => {
                            log::info!("Loaded settings from {}", path.display());
                            settings
                        }
                        Err(e) => {
                            log::warn!("Failed to parse settings file, using defaults: {}", e);
                            AppSettings::default()
                        }
                    },
                    Err(e) => {
                        log::warn!("Failed to read settings file, using defaults: {}", e);
                        AppSettings::default()
                    }
                }
            } else {
                log::info!("No settings file found, using defaults");
                AppSettings::default()
            }
        }
        Err(e) => {
            log::warn!("Failed to determine settings path, using defaults: {}", e);
            AppSettings::default()
        }
    }
}

/// Canonical cloud-provider credential keys checked for first-launch demo
/// detection. If every one of these slots is empty in credentials.yaml AND
/// the user hasn't yet chosen `demo_mode` in settings, the app auto-enters
/// demo mode (local ASR + local LLM) so it can still be used without keys.
///
/// IMPORTANT: keep in sync with `FIRST_TIME_CREDENTIAL_KEYS` in `src/App.tsx`.
pub const DEMO_CREDENTIAL_KEYS: &[&str] = &[
    "openai_api_key",
    "gemini_api_key",
    "deepgram_api_key",
    "assemblyai_api_key",
    "groq_api_key",
    "aws_access_key",
];

/// Return `true` if the credential store has no cloud-provider key populated.
/// "Populated" means `Some(s)` where `s.trim()` is non-empty — whitespace
/// doesn't count (it would never authenticate against a real provider).
pub fn all_demo_credentials_empty(store: &crate::credentials::CredentialStore) -> bool {
    let probe = |v: &Option<String>| v.as_deref().map(|s| s.trim()).unwrap_or("").is_empty();
    probe(&store.openai_api_key)
        && probe(&store.gemini_api_key)
        && probe(&store.deepgram_api_key)
        && probe(&store.assemblyai_api_key)
        && probe(&store.groq_api_key)
        && probe(&store.aws_access_key)
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

pub fn save_settings(app: &tauri::AppHandle, settings: &AppSettings) -> Result<(), String> {
    let path = get_settings_path(app)?;
    persist_inline_credentials(settings)?;
    let settings_for_disk = redacted_settings(settings);

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create settings directory: {}", e))?;
    }

    let json = serde_json::to_string_pretty(&settings_for_disk)
        .map_err(|e| format!("Failed to serialize settings: {}", e))?;

    let tmp_path = path.with_extension("json.tmp");
    fs::write(&tmp_path, &json).map_err(|e| format!("Failed to write settings file: {}", e))?;

    // Lock down perms before rename so the file is never world-readable, even briefly.
    crate::fs_util::set_owner_only(&tmp_path);

    fs::rename(&tmp_path, &path).map_err(|e| format!("Failed to finalize settings file: {}", e))?;

    // Re-apply after rename in case rename semantics differ across platforms.
    crate::fs_util::set_owner_only(&path);

    log::info!("Settings saved to {}", path.display());
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

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
            },
            ..AppSettings::default()
        };

        assert!(has_inline_credentials(&settings));
        let redacted = redacted_settings(&settings);
        let json = serde_json::to_string(&redacted).unwrap();

        for secret in ["dg-secret", "sk-secret", "cfg-secret", "gemini-secret"] {
            assert!(
                !json.contains(secret),
                "settings JSON must not contain inline secret {secret}"
            );
        }
        assert!(!has_inline_credentials(&redacted));
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
}
