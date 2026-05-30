//! Bundled default configuration loaded from `config/default.toml`.
//!
//! This module owns parsing the full TOML document. Runtime code should only
//! consume fields that have a clear owner; unsupported fields are still parsed
//! here so future wiring can be explicit and typed instead of ad hoc.

use serde::Deserialize;
use std::path::Path;
use std::sync::OnceLock;

const BUNDLED_DEFAULT_CONFIG_TOML: &str = include_str!("../config/default.toml");

#[derive(Debug, Clone, Default, Deserialize)]
pub struct DefaultConfig {
    #[serde(default)]
    pub audio: AudioConfig,
    #[serde(default)]
    pub pipeline: PipelineConfig,
    #[serde(default)]
    pub asr: AsrConfig,
    #[serde(default)]
    pub diarization: DiarizationConfig,
    #[serde(default)]
    pub graph: GraphConfig,
    #[serde(default)]
    pub ui: UiConfig,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AudioConfig {
    pub sample_rate: Option<u32>,
    pub channels: Option<u16>,
    pub buffer_size: Option<usize>,
    pub ring_buffer_capacity: Option<usize>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PipelineConfig {
    pub segment_duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AsrConfig {
    pub model_path: Option<String>,
    pub language: Option<String>,
    pub beam_size: Option<u32>,
    pub temperature: Option<f32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct DiarizationConfig {
    pub segmentation_model: Option<String>,
    pub embedding_model: Option<String>,
    pub speaker_similarity_threshold: Option<f32>,
    pub max_speakers: Option<usize>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct GraphConfig {
    pub entity_similarity_threshold: Option<f32>,
    pub max_nodes: Option<usize>,
    pub max_edges: Option<usize>,
    pub snapshot_interval_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct UiConfig {
    pub theme: Option<String>,
    pub graph_dimension: Option<String>,
    pub max_transcript_entries: Option<usize>,
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
        assert_eq!(config.audio.buffer_size, Some(480));
        assert_eq!(config.audio.ring_buffer_capacity, Some(65_536));
        assert_eq!(config.pipeline.segment_duration_ms, Some(2_000));
        assert_eq!(config.asr.beam_size, Some(5));
        assert_eq!(config.asr.temperature, Some(0.0));
        assert_eq!(config.graph.max_nodes, Some(1_000));
        assert_eq!(config.graph.max_edges, Some(5_000));
        assert_eq!(config.ui.max_transcript_entries, Some(500));
    }

    #[test]
    fn asr_model_path_is_reduced_to_filename() {
        let config = load_default_config();
        assert_eq!(
            config.whisper_model_filename().as_deref(),
            Some("ggml-small.en.bin")
        );
    }
}
