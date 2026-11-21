//! `decision_config.json` of a d3 checkpoint (the backbone's `config.json` is `qwen35::config`).

use crate::qwen35::{Attention, config::read_json};
use candle_core::{Result, bail};
use serde::Deserialize;
use std::path::Path;

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

impl DecisionConfig {
    /// `d3_runtime.ATTENTION_MODES`: `noncausal_full_attention` (every released size) or `causal`.
    pub fn attention(&self) -> Result<Attention> {
        match self.attention_mode.as_str() {
            "noncausal_full_attention" => Ok(Attention::Bidirectional),
            "causal" => Ok(Attention::Causal),
            other => bail!("unsupported attention_mode {other:?}"),
        }
    }
}

pub fn load(root: &Path) -> Result<DecisionConfig> {
    let decision: DecisionConfig = read_json(&root.join("decision_config.json"))?;
    if decision.format_id != "d3-code-readout-v1"
        || decision.prompt != "d3"
        || decision.pooling != "last"
    {
        bail!(
            "unsupported decision format {} / prompt {} / pooling {}",
            decision.format_id,
            decision.prompt,
            decision.pooling
        );
    }
    decision.attention()?;
    if decision.readout_dtype != "float32" {
        bail!(
            "readout_dtype {:?}: only float32 is implemented",
            decision.readout_dtype
        );
    }
    if decision.codes.len() != super::format::MAX_OPTIONS
        || decision.token_ids.len() != decision.codes.len()
    {
        bail!(
            "decision_config.json must list {} codes and token ids",
            super::format::MAX_OPTIONS
        );
    }
    if !(decision.temperature.is_finite() && decision.temperature > 0.0) {
        bail!(
            "temperature must be finite and positive, got {}",
            decision.temperature
        );
    }
    Ok(decision)
}
