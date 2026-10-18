//! Clef-flash (`Cloudflare/clef-flash`) on candle: the shared Qwen3.5 encoder (`crate::qwen35`, causal
//! attention as in the stock forward) under the checkpoint's joint schema head. One forward pass scores
//! every option of every question of a request (see `docs/designs/clef-flash-native-2026-10-18.md`).
//!
//! The release ships the rank-256 LoRA already merged into the backbone weights, so the encoder loads
//! them as plain Qwen3.5 weights.

pub mod format;
pub mod head;

use crate::qwen35::{self, Attention, Encoder, LoadOptions};
use audio_graph_decision_contract::{DecisionState, OrderedMap, Question};
use candle_core::{DType, Device, Result, Tensor, bail};
use candle_nn::VarBuilder;
use format::Encoded;
use head::JointSchemaHead;
use std::path::Path;
use tokenizers::Tokenizer;

/// Tensor name prefix of the text model in `Qwen3_5ForConditionalGeneration` checkpoints.
const TEXT_PREFIX: &str = "model.language_model";

/// A loaded checkpoint: tokenizer, encoder, head and the output-embedding table the head's lexical
/// option vectors read (kept on the host in its stored dtype; only option-token rows are gathered).
pub struct ClefModel {
    tokenizer: Tokenizer,
    encoder: Encoder,
    head: JointSchemaHead,
    lm_head: Tensor,
    device: Device,
}

/// Option probabilities of every question (prompt order, as `Encoded::questions`).
pub struct Scored {
    pub encoded: Encoded,
    pub probabilities: Vec<Vec<f32>>,
}

impl ClefModel {
    pub fn load(root: &Path, dtype: DType, device: &Device) -> Result<Self> {
        let text = qwen35::config::load(root)?;
        let tokenizer = Tokenizer::from_file(root.join("tokenizer.json"))
            .map_err(|e| candle_core::Error::Msg(format!("tokenizer.json: {e}")))?;
        let options = LoadOptions {
            prefix: TEXT_PREFIX,
            attention: Attention::Causal,
            dtype,
            // The release stores BF16; F32 compute upcasts per matmul instead of holding a 28 GB copy.
            weights: DType::BF16,
        };
        let encoder = Encoder::load(root, &text, &options, device)?;
        let files = qwen35::weight_files(root)?;
        // SAFETY: the checkpoint files are not modified while mapped.
        let host =
            unsafe { VarBuilder::from_mmaped_safetensors(&files, DType::BF16, &Device::Cpu)? };
        let lm_head = host.get((text.vocab_size, text.hidden_size), "lm_head.weight")?;
        let head = JointSchemaHead::load(root, dtype, device)?;
        Ok(Self {
            tokenizer,
            encoder,
            head,
            lm_head,
            device: device.clone(),
        })
    }

    pub fn tokenizer(&self) -> &Tokenizer {
        &self.tokenizer
    }

    /// Mean output-embedding row of each option's tokens, per question, on the device in FP32.
    fn lexical(&self, encoded: &Encoded) -> Result<Vec<Tensor>> {
        encoded
            .questions
            .iter()
            .map(|q| {
                let rows = q
                    .option_spans
                    .iter()
                    .map(|&(start, end)| {
                        let ids = Tensor::new(&encoded.input_ids[start..end], &Device::Cpu)?;
                        self.lm_head
                            .index_select(&ids, 0)?
                            .to_dtype(DType::F32)?
                            .mean_keepdim(0)
                    })
                    .collect::<Result<Vec<_>>>()?;
                Tensor::cat(&rows, 0)?.to_device(&self.device)
            })
            .collect()
    }

    /// One forward pass over an encoded request.
    pub fn score_encoded(&self, encoded: Encoded) -> Result<Scored> {
        if encoded.input_ids.is_empty() {
            bail!("empty prompt");
        }
        let hidden = self.encoder.hidden_states(&encoded.input_ids)?;
        let lexical = self.lexical(&encoded)?;
        let logits = self.head.logits(&hidden, &encoded, &lexical)?;
        let probabilities = logits.iter().map(|l| head::softmax(l)).collect();
        Ok(Scored {
            encoded,
            probabilities,
        })
    }

    pub fn score(
        &self,
        state: &DecisionState,
        questions: &OrderedMap<String, Question>,
    ) -> Result<Scored> {
        let encoded = format::encode(&self.tokenizer, state, questions)
            .map_err(|e| candle_core::Error::Msg(format!("encode: {e}")))?;
        self.score_encoded(encoded)
    }
}
