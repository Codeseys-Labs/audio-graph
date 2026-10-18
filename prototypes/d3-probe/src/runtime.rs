//! A loaded d3 checkpoint: tokenizer, encoder and the 255-code readout.

use crate::config::{self, DecisionConfig, TextConfig};
use crate::format::{self, Question};
use crate::model::Encoder;
use anyhow::{bail, Context, Result};
use candle_core::{DType, Device, Tensor};
use serde_json::Value;
use std::path::Path;
use tokenizers::Tokenizer;

pub struct D3 {
    pub text: TextConfig,
    pub decision: DecisionConfig,
    tokenizer: Tokenizer,
    encoder: Encoder,
    readout: Tensor, // (255, hidden) FP32
    device: Device,
}

impl D3 {
    pub fn load(root: &Path, dtype: DType, device: &Device) -> Result<Self> {
        let (text, decision) = config::load(root)?;
        if decision.attention_mode != "noncausal_full_attention" {
            bail!("attention_mode {:?}: this probe implements noncausal_full_attention only", decision.attention_mode);
        }
        let tokenizer = Tokenizer::from_file(root.join("tokenizer.json")).map_err(anyhow::Error::msg)?;
        for (code, id) in decision.codes.iter().zip(&decision.token_ids) {
            let enc = tokenizer.encode(code.as_str(), false).map_err(anyhow::Error::msg)?;
            if enc.get_ids() != [*id] {
                bail!("answer code {code:?} does not tokenize to its declared id {id}");
            }
        }
        let encoder = Encoder::load(root, &text, dtype, device)?;
        let readout = candle_core::safetensors::load(root.join("readout.safetensors"), device)?
            .remove("weight")
            .context("readout.safetensors has no `weight`")?
            .to_dtype(DType::F32)?;
        if readout.dims() != [format::MAX_OPTIONS, text.hidden_size] {
            bail!("readout weight has shape {:?}", readout.dims());
        }
        Ok(Self { text, decision, tokenizer, encoder, readout, device: device.clone() })
    }

    pub fn prompt(&self, state: &Value, question: &Question) -> Result<String> {
        format::render(state, question, &self.decision.codes)
    }

    pub fn tokenize(&self, prompt: &str) -> Result<Vec<u32>> {
        Ok(self.tokenizer.encode(prompt, false).map_err(anyhow::Error::msg)?.get_ids().to_vec())
    }

    /// Option probabilities (FP32 softmax over the question's first `count` codes at temperature T).
    pub fn probabilities(&self, ids: &[u32], count: usize) -> Result<Vec<f32>> {
        let hidden = self.encoder.last_hidden(ids)?; // (H,)
        let logits = self.readout.narrow(0, 0, count)?.matmul(&hidden.unsqueeze(1)?)?.squeeze(1)?;
        let scaled = (logits / self.decision.temperature)?;
        let probs = candle_nn::ops::softmax_last_dim(&scaled.unsqueeze(0)?)?.squeeze(0)?;
        Ok(probs.to_device(&Device::Cpu)?.to_vec1::<f32>()?)
    }

    pub fn device(&self) -> &Device {
        &self.device
    }
}
