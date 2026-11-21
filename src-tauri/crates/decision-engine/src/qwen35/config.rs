//! The `text_config` of a Qwen3.5 checkpoint's `config.json`.

use candle_core::{Error, Result, bail};
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

pub(crate) fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = std::fs::read(path)?;
    serde_json::from_slice(&bytes).map_err(|e| Error::Msg(format!("{}: {e}", path.display())))
}

/// Reads and validates `<root>/config.json` (`model_type` `qwen3_5`).
pub fn load(root: &Path) -> Result<TextConfig> {
    let model: ModelConfig = read_json(&root.join("config.json"))?;
    if model.model_type != "qwen3_5" {
        bail!(
            "unsupported backbone {:?}; expected qwen3_5",
            model.model_type
        );
    }
    let text = model.text_config;
    if text.layer_types.len() < text.num_hidden_layers {
        bail!(
            "layer_types lists {} layers, expected {}",
            text.layer_types.len(),
            text.num_hidden_layers
        );
    }
    if text.hidden_act != "silu" || !text.attn_output_gate {
        bail!(
            "unsupported Qwen3.5 variant (hidden_act {}, attn_output_gate {})",
            text.hidden_act,
            text.attn_output_gate
        );
    }
    if text
        .rope_parameters
        .rope_type
        .as_deref()
        .is_some_and(|t| t != "default")
    {
        bail!("unsupported rope_type {:?}", text.rope_parameters.rope_type);
    }
    if !text
        .linear_num_value_heads
        .is_multiple_of(text.linear_num_key_heads)
    {
        bail!("linear value heads must be a multiple of key heads");
    }
    Ok(text)
}
