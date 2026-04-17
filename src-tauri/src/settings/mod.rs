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

fn default_aws_region() -> String {
    "us-east-1".to_string()
}
fn default_language_code() -> String {
    "en-US".to_string()
}
fn default_deepgram_model() -> String {
    "nova-3".to_string()
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AwsCredentialSource {
    #[serde(rename = "default_chain")]
    DefaultChain,
    #[serde(rename = "profile")]
    Profile { name: String },
    #[serde(rename = "access_keys")]
    AccessKeys { access_key: String },
}

impl Default for AwsCredentialSource {
    fn default() -> Self {
        Self::DefaultChain
    }
}

// ---------------------------------------------------------------------------
// ASR provider
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AsrProvider {
    #[serde(rename = "local_whisper")]
    LocalWhisper,
    #[serde(rename = "api")]
    Api {
        endpoint: String,
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
        api_key: String,
        #[serde(default = "default_deepgram_model")]
        model: String,
        #[serde(default = "default_true")]
        enable_diarization: bool,
    },
    #[serde(rename = "assemblyai")]
    AssemblyAI {
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

impl Default for AsrProvider {
    fn default() -> Self {
        Self::LocalWhisper
    }
}

// ---------------------------------------------------------------------------
// LLM API config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmApiConfig {
    pub endpoint: String,
    #[serde(default)]
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
    "ggml-small-extract.gguf".to_string()
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
    16000
}
fn default_channels() -> u16 {
    1
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
    matches!(hz, 16000 | 22050 | 44100 | 48000 | 88200 | 96000)
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
    ApiKey { api_key: String },
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
    /// Runtime log-verbosity preference: one of
    /// "off" | "error" | "warn" | "info" | "debug" | "trace".
    ///
    /// `None` means "not set — fall back to the default (info) unless
    /// the user set RUST_LOG at startup". `skip_serializing_if` keeps the
    /// field out of written YAML/JSON when unset, so older settings files
    /// stay byte-identical after a round-trip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_level: Option<String>,
}

fn default_whisper_model() -> String {
    "ggml-small.en.bin".to_string()
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
            log_level: Some("info".to_string()),
        }
    }
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

pub fn save_settings(app: &tauri::AppHandle, settings: &AppSettings) -> Result<(), String> {
    let path = get_settings_path(app)?;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create settings directory: {}", e))?;
    }

    let json = serde_json::to_string_pretty(settings)
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
        for hz in [16000u32, 22050, 44100, 48000, 88200, 96000] {
            assert!(
                sample_rate_is_valid(hz),
                "{} Hz should be accepted by the whitelist",
                hz
            );
        }
        // Out-of-set values we explicitly don't support. 8000 (telephony)
        // and 192000 (studio) are left out on purpose — they're not worth
        // testing against until rsac is verified on them.
        for hz in [0u32, 1, 8000, 11025, 32000, 192000, u32::MAX] {
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
}
