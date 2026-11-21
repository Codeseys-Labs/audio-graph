//! Clef-flash (`Cloudflare/clef-flash`) on candle: the shared Qwen3.5 encoder (`crate::qwen35`, causal
//! attention as in the stock forward) under the checkpoint's joint schema head. One forward pass scores
//! every option of every question of a request (see `docs/designs/clef-flash-native-2026-10-18.md`).
//!
//! The release ships the rank-256 LoRA already merged into the backbone weights, so the encoder loads
//! them as plain Qwen3.5 weights.

pub mod format;
pub mod head;

use crate::qwen35::{self, Attention, Encoder, LoadOptions};
use crate::{EngineConfig, EngineRegistry, ExecutionProvider};
use async_trait::async_trait;
use audio_graph_decision_contract::{
    DecisionEngine, DecisionError, DecisionRequest, DecisionResponse, DecisionState, Egress,
    EngineLimits, OrderedMap, Question, Usage,
};
use candle_core::{DType, Device, Result, Tensor, bail};
use candle_nn::VarBuilder;
use format::Encoded;
use head::JointSchemaHead;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokenizers::Tokenizer;

/// Engine id; also the checkpoint's directory under `models_dir`.
pub const CLEF_FLASH: &str = "clef-flash";
const REPO: &str = "Cloudflare/clef-flash";
/// vLLM's `JointSchemaStrategy` limit; the reference itself is bounded only by the token budget.
const MAX_QUESTIONS: usize = 64;
/// Options per choice and levels per score. The reference has no fixed bound (the 16k budget is the real
/// one); this keeps requests in the range the model was evaluated on.
const MAX_OPTIONS: usize = 255;

/// Add the Clef-flash engine to `registry` through its constructor API.
pub fn register(registry: &mut EngineRegistry) {
    registry.register(CLEF_FLASH.into(), |config| {
        Ok(Arc::new(ClefEngine::load(config)?))
    });
}

/// Backbone and head compute dtype. Weights stay in their stored BF16 either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Precision {
    /// The published numerics (the release runs BF16).
    Bf16,
    /// FP32 compute on the BF16 weights: matches the FP32 reference to about 1e-6, slower.
    F32,
}

impl Precision {
    fn dtype(self) -> DType {
        match self {
            Self::Bf16 => DType::BF16,
            Self::F32 => DType::F32,
        }
    }
}

/// Clef-flash behind System One. One forward pass answers every question of a request; probabilities
/// match the checkpoint's `systemone()` (before its 4-decimal rounding). `confidence` is the native-engine
/// meaning shared with Laya and d3: the calibrated probability of the reported answer (top option for
/// choice and score, `max(p, 1 - p)` for noul). The state is truncated to the 16,384-token budget like the
/// reference and vLLM; `usage.input_tokens` counts the prompt actually read.
pub struct ClefEngine {
    directory: PathBuf,
    provider: ExecutionProvider,
    precision: Precision,
    model: Arc<Mutex<Option<ClefModel>>>,
}

impl ClefEngine {
    /// Checks the provider; the model files load on the first request.
    pub fn load(config: &EngineConfig) -> std::result::Result<Self, DecisionError> {
        if matches!(config.execution_provider, ExecutionProvider::DirectMl) {
            return Err(DecisionError::EngineUnavailable { feature: "cuda" });
        }
        if !config.execution_provider.is_compiled() {
            return Err(DecisionError::EngineUnavailable {
                feature: config
                    .execution_provider
                    .feature()
                    .unwrap_or("decision-clef"),
            });
        }
        Ok(Self {
            directory: config.models_dir.join(CLEF_FLASH),
            provider: config.execution_provider,
            // candle's CPU matmul has no BF16 kernel.
            precision: if config.execution_provider == ExecutionProvider::Cpu {
                Precision::F32
            } else {
                Precision::Bf16
            },
            model: Arc::new(Mutex::new(None)),
        })
    }

    /// Override the compute dtype on GPU (CPU stays FP32). Takes effect before the first request.
    pub fn with_precision(mut self, precision: Precision) -> Self {
        if self.provider != ExecutionProvider::Cpu {
            self.precision = precision;
        }
        self
    }

    pub fn precision(&self) -> Precision {
        self.precision
    }
}

#[async_trait]
impl DecisionEngine for ClefEngine {
    fn id(&self) -> &str {
        CLEF_FLASH
    }

    fn egress(&self) -> Egress {
        Egress::Local
    }

    fn limits(&self) -> EngineLimits {
        EngineLimits {
            max_questions: MAX_QUESTIONS,
            max_choice_options: MAX_OPTIONS,
            max_score_levels: MAX_OPTIONS,
            max_state_tokens: Some(format::MAX_LENGTH as u32),
        }
    }

    async fn system_one(
        &self,
        request: &DecisionRequest,
    ) -> std::result::Result<DecisionResponse, DecisionError> {
        // Cardinality is checked before any model file is read.
        self.limits().validate(request)?;
        let request = request.clone();
        let model = Arc::clone(&self.model);
        let directory = self.directory.clone();
        let (provider, precision) = (self.provider, self.precision);
        tokio::task::spawn_blocking(move || {
            let mut slot = model.lock().map_err(|_| DecisionError::Internal)?;
            if slot.is_none() {
                let device = qwen35::device_for(provider)?;
                *slot = Some(
                    ClefModel::load(&directory, precision.dtype(), &device)
                        .map_err(|_| DecisionError::Internal)?,
                );
            }
            let loaded = slot.as_ref().ok_or(DecisionError::Internal)?;
            let encoded = format::encode(loaded.tokenizer(), &request.state, &request.questions)?;
            let input_tokens = encoded.input_ids.len() as u64;
            let scored = loaded
                .score_encoded(encoded)
                .map_err(|_| DecisionError::Internal)?;
            let mut answers = OrderedMap::new();
            for (question, probabilities) in
                scored.encoded.questions.iter().zip(&scored.probabilities)
            {
                let typed = request
                    .questions
                    .get(&question.id)
                    .ok_or(DecisionError::Internal)?;
                answers.insert(
                    question.id.clone(),
                    format::answer(typed, question, probabilities)?,
                );
            }
            Ok(DecisionResponse {
                answers,
                usage: Usage {
                    input_tokens,
                    output_tokens: 0,
                },
                engine_id: CLEF_FLASH.to_owned(),
                model: REPO.to_owned(),
            })
        })
        .await
        .map_err(|_| DecisionError::Internal)?
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn registered_engine_refuses_invalid_requests_before_loading_model_files() {
        let config = EngineConfig::new(PathBuf::from("absent-models"));
        let mut registry = EngineRegistry::default();
        register(&mut registry);
        let engine = registry.create(CLEF_FLASH, &config).unwrap();
        assert_eq!(engine.id(), CLEF_FLASH);
        let question = |i: usize| {
            (
                format!("q{i}"),
                Question::Noul {
                    instructions: "x".into(),
                    criteria: None,
                },
            )
        };
        let too_many = DecisionRequest {
            state: DecisionState::Text("s".into()),
            questions: (0..=MAX_QUESTIONS).map(question).collect(),
        };
        assert!(matches!(
            engine.system_one(&too_many).await,
            Err(DecisionError::CardinalityLimit {
                limit: MAX_QUESTIONS,
                ..
            })
        ));
        let one_level = DecisionRequest {
            state: DecisionState::Text("s".into()),
            questions: [(
                "s".to_owned(),
                Question::Score {
                    instructions: "x".into(),
                    criteria: vec!["only".into()],
                },
            )]
            .into_iter()
            .collect(),
        };
        assert!(matches!(
            engine.system_one(&one_level).await,
            Err(DecisionError::CardinalityLimit { .. })
        ));
    }

    #[test]
    fn cpu_runs_fp32_and_directml_is_refused() {
        let mut config = EngineConfig::new(PathBuf::from("absent-models"));
        let engine = ClefEngine::load(&config)
            .unwrap()
            .with_precision(Precision::Bf16);
        assert_eq!(engine.precision(), Precision::F32);
        config.execution_provider = ExecutionProvider::DirectMl;
        assert!(matches!(
            ClefEngine::load(&config),
            Err(DecisionError::EngineUnavailable { .. })
        ));
    }
}
