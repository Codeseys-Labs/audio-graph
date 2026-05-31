//! Bundled default configuration loaded from `config/default.toml`.
//!
//! This module parses the bundled TOML document. To avoid dead-config drift
//! (backlog B02), it intentionally models ONLY the keys that are actually
//! consumed at runtime: `audio.sample_rate`, `audio.channels`, and
//! `asr.model_path`. Any future setting must be added here *and* wired to a
//! real consumer at the same time, not parked as an unread field.

use serde::Deserialize;
use std::path::Path;
use std::sync::OnceLock;

const BUNDLED_DEFAULT_CONFIG_TOML: &str = include_str!("../config/default.toml");

#[derive(Debug, Clone, Default, Deserialize)]
pub struct DefaultConfig {
    #[serde(default)]
    pub audio: AudioConfig,
    #[serde(default)]
    pub asr: AsrConfig,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AudioConfig {
    pub sample_rate: Option<u32>,
    pub channels: Option<u16>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AsrConfig {
    pub model_path: Option<String>,
}

impl DefaultConfig {
    /// Return the configured ASR model filename, stripping any directory
    /// prefix from `asr.model_path`. Runtime model resolution already joins
    /// this filename to the app's models directory.
    pub fn whisper_model_filename(&self) -> Option<String> {
        let model_path = self.asr.model_path.as_deref()?;
        Path::new(model_path)
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.trim().is_empty())
            .map(str::to_string)
    }
}

pub fn load_default_config() -> &'static DefaultConfig {
    static CONFIG: OnceLock<DefaultConfig> = OnceLock::new();
    CONFIG.get_or_init(
        || match toml::from_str::<DefaultConfig>(BUNDLED_DEFAULT_CONFIG_TOML) {
            Ok(config) => config,
            Err(e) => {
                log::warn!(
                    "Failed to parse bundled config/default.toml; using hardcoded defaults: {}",
                    e
                );
                DefaultConfig::default()
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_default_config_parses_expected_values() {
        let config = load_default_config();

        assert_eq!(config.audio.sample_rate, Some(48_000));
        assert_eq!(config.audio.channels, Some(2));
    }

    #[test]
    fn asr_model_path_is_reduced_to_filename() {
        let config = load_default_config();
        assert_eq!(
            config.whisper_model_filename().as_deref(),
            Some("ggml-small.en.bin")
        );
    }

    #[test]
    fn unknown_config_keys_are_ignored_not_fatal() {
        // Forward-compat / dead-key safety: extra keys must not break parsing.
        let toml = "[audio]\nsample_rate = 16000\nchannels = 1\nbuffer_size = 99\n\n[graph]\nmax_nodes = 5\n";
        let parsed: DefaultConfig = toml::from_str(toml).expect("extra keys ignored");
        assert_eq!(parsed.audio.sample_rate, Some(16_000));
        assert_eq!(parsed.audio.channels, Some(1));
    }
}
