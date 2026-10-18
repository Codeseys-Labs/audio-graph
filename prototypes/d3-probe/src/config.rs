//! `config.json` (Qwen3.5 text config) and `decision_config.json` of a d3 checkpoint.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct RopeParameters {
    pub rope_theta: f64,
    #[serde(default = "one")]
    pub partial_rotary_factor: f64,
    #[serde(default)]
    pub rope_type: Option<String>,
}

fn one() -> f64 {
    1.0
}

#[derive(Debug, Clone, Deserialize)]
pub struct TextConfig {
    pub hidden_size: usize,
    pub intermediate_size: usize,
    pub num_hidden_layers: usize,
    pub num_attention_heads: usize,
    pub num_key_value_heads: usize,
    pub head_dim: usize,
    pub rms_norm_eps: f64,
    pub layer_types: Vec<String>,
    pub linear_conv_kernel_dim: usize,
    pub linear_key_head_dim: usize,
    pub linear_value_head_dim: usize,
    pub linear_num_key_heads: usize,
    pub linear_num_value_heads: usize,
    pub vocab_size: usize,
    pub hidden_act: String,
    pub attn_output_gate: bool,
    pub rope_parameters: RopeParameters,
}

impl TextConfig {
    pub fn rotary_dim(&self) -> usize {
        (self.head_dim as f64 * self.rope_parameters.partial_rotary_factor) as usize
    }
}

#[derive(Debug, Deserialize)]
struct ModelConfig {
    model_type: String,
    text_config: TextConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DecisionConfig {
    pub format_id: String,
    pub prompt: String,
    pub codes: Vec<String>,
    pub token_ids: Vec<u32>,
    pub temperature: f64,
    pub attention_mode: String,
    pub pooling: String,
    pub max_length: Option<usize>,
    pub readout_dtype: String,
}

pub fn load(root: &Path) -> Result<(TextConfig, DecisionConfig)> {
    let model: ModelConfig = serde_json::from_slice(
        &std::fs::read(root.join("config.json")).context("reading config.json")?,
    )
    .context("parsing config.json")?;
    if model.model_type != "qwen3_5" {
        bail!("unsupported backbone {:?}; d3 native runtime takes qwen3_5", model.model_type);
    }
    let text = model.text_config;
    if text.layer_types.len() < text.num_hidden_layers {
        bail!("layer_types lists {} layers, expected {}", text.layer_types.len(), text.num_hidden_layers);
    }
    if text.hidden_act != "silu" || !text.attn_output_gate {
        bail!("unsupported Qwen3.5 variant (hidden_act {}, attn_output_gate {})", text.hidden_act, text.attn_output_gate);
    }
    if text.rope_parameters.rope_type.as_deref().is_some_and(|t| t != "default") {
        bail!("unsupported rope_type {:?}", text.rope_parameters.rope_type);
    }
    let decision: DecisionConfig = serde_json::from_slice(
        &std::fs::read(root.join("decision_config.json")).context("reading decision_config.json")?,
    )
    .context("parsing decision_config.json")?;
    if decision.format_id != "d3-code-readout-v1" || decision.prompt != "d3" || decision.pooling != "last" {
        bail!("unsupported decision format {} / prompt {} / pooling {}", decision.format_id, decision.prompt, decision.pooling);
    }
    if decision.codes.len() != crate::format::MAX_OPTIONS || decision.token_ids.len() != decision.codes.len() {
        bail!("decision_config.json must list {} codes and token ids", crate::format::MAX_OPTIONS);
    }
    if !(decision.temperature.is_finite() && decision.temperature > 0.0) {
        bail!("temperature must be finite and positive, got {}", decision.temperature);
    }
    Ok((text, decision))
}
