//! Decision 3.0 (`vllm-sr/d3-*`) on candle: the shared Qwen3.5 encoder (`crate::qwen35`) with
//! bidirectional full attention, plus the d3 prompt and 255-code readout. One forward pass per question;
//! answers match `model.system_one()` of the checkpoint's Python package (see
//! `docs/designs/d3-native-runtime-2026-10-18.md`).

mod config;
pub mod format;

use crate::qwen35::{self, Encoder, LoadOptions};
use crate::{EngineConfig, EngineConstructor, EngineRegistry, ExecutionProvider};
use async_trait::async_trait;
use audio_graph_decision_contract::{
    DecisionEngine, DecisionError, DecisionRequest, DecisionResponse, Egress, EngineLimits,
    OrderedMap, Usage,
};
use candle_core::{DType, Device, Tensor};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokenizers::Tokenizer;

/// One Decision 3.0 size: the engine id, which is also its directory under `models_dir`, and the
/// Hugging Face repository reported as the response `model`.
#[derive(Debug)]
pub struct Checkpoint {
    pub id: &'static str,
    pub repo: &'static str,
}

pub const CHECKPOINTS: &[Checkpoint] = &[
    Checkpoint {
        id: "d3-edge",
        repo: "vllm-sr/d3-edge",
    },
    Checkpoint {
        id: "d3-lite",
        repo: "vllm-sr/d3-lite",
    },
    Checkpoint {
        id: "d3-nano",
        repo: "vllm-sr/d3-nano",
    },
    Checkpoint {
        id: "d3-mini",
        repo: "vllm-sr/d3-mini",
    },
];

/// Add the four d3 engines to `registry` through its constructor API.
pub fn register(registry: &mut EngineRegistry) {
    let constructors: [EngineConstructor; 4] = [
        construct::<0>,
        construct::<1>,
        construct::<2>,
        construct::<3>,
    ];
    for (checkpoint, constructor) in CHECKPOINTS.iter().zip(constructors) {
        registry.register(checkpoint.id.into(), constructor);
    }
}

fn construct<const INDEX: usize>(
    config: &EngineConfig,
) -> Result<Arc<dyn DecisionEngine>, DecisionError> {
    Ok(Arc::new(D3Engine::load(config, &CHECKPOINTS[INDEX])?))
}

/// Backbone compute dtype. The readout and softmax always run in FP32, as in the reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Precision {
    /// Matches the reference's FP32 numerics (parity fixtures: at most a few 1e-6 per probability).
    F32,
    /// The reference's own BF16 numerics: within its batch-size spread, near-ties may flip.
    Bf16,
}

impl Precision {
    fn dtype(self) -> DType {
        match self {
            Self::F32 => DType::F32,
            Self::Bf16 => DType::BF16,
        }
    }
}

struct Loaded {
    tokenizer: Tokenizer,
    encoder: Encoder,
    readout: Tensor, // (255, hidden) FP32 on the device
    codes: Vec<String>,
    temperature: f64,
    max_length: Option<usize>,
}

fn internal<T, E>(result: Result<T, E>) -> Result<T, DecisionError> {
    result.map_err(|_| DecisionError::Internal)
}

impl Loaded {
    fn load(
        directory: &Path,
        device: &Device,
        precision: Precision,
    ) -> Result<Self, DecisionError> {
        let text = internal(qwen35::config::load(directory))?;
        let decision = internal(config::load(directory))?;
        let tokenizer = internal(Tokenizer::from_file(directory.join("tokenizer.json")))?;
        for (code, id) in decision.codes.iter().zip(&decision.token_ids) {
            let encoding = internal(tokenizer.encode(code.as_str(), false))?;
            if encoding.get_ids() != [*id] {
                return Err(DecisionError::Internal);
            }
        }
        let options = LoadOptions {
            prefix: "language_model",
            attention: internal(decision.attention())?,
            dtype: precision.dtype(),
            weights: precision.dtype(),
        };
        let encoder = internal(Encoder::load(directory, &text, &options, device))?;
        let readout = internal(candle_core::safetensors::load(
            directory.join("readout.safetensors"),
            device,
        ))?
        .remove("weight")
        .ok_or(DecisionError::Internal)?;
        let readout = internal(readout.to_dtype(DType::F32))?;
        if readout.dims() != [format::MAX_OPTIONS, text.hidden_size] {
            return Err(DecisionError::Internal);
        }
        Ok(Self {
            tokenizer,
            encoder,
            readout,
            codes: decision.codes,
            temperature: decision.temperature,
            max_length: decision.max_length,
        })
    }

    /// Softmax over the question's first `count` codes at the checkpoint temperature (FP32).
    fn probabilities(&self, ids: &[u32], count: usize) -> candle_core::Result<Vec<f32>> {
        let hidden = self.encoder.last_hidden(ids)?;
        let logits = self
            .readout
            .narrow(0, 0, count)?
            .matmul(&hidden.unsqueeze(1)?)?
            .squeeze(1)?;
        let probs = candle_nn::ops::softmax_last_dim(&(logits / self.temperature)?.unsqueeze(0)?)?;
        probs.squeeze(0)?.to_device(&Device::Cpu)?.to_vec1::<f32>()
    }
}

/// The candle device for an execution provider. candle has CUDA and Metal backends only; DirectML
/// requests are refused (Vulkan-only GPUs: audio-graph-26b4).
fn device_for(provider: ExecutionProvider) -> Result<Device, DecisionError> {
    match provider {
        ExecutionProvider::Cpu => Ok(Device::Cpu),
        #[cfg(all(feature = "cuda", any(target_os = "windows", target_os = "linux")))]
        ExecutionProvider::Cuda => {
            let device = Device::new_cuda(0)
                .map_err(|_| DecisionError::EngineUnavailable { feature: "cuda" })?;
            // One stream and no cross-stream sharing: candle's per-tensor event tracking is pure overhead.
            // SAFETY: every tensor of this engine lives on this device's single stream.
            unsafe { internal(device.as_cuda_device())?.disable_event_tracking() };
            Ok(device)
        }
        #[cfg(all(feature = "metal", target_os = "macos"))]
        ExecutionProvider::CoreMl => {
            Device::new_metal(0).map_err(|_| DecisionError::EngineUnavailable { feature: "metal" })
        }
        #[allow(unreachable_patterns)]
        _ => Err(DecisionError::EngineUnavailable { feature: "cuda" }),
    }
}

/// FP32 wherever it is cheap; BF16 for the two larger sizes on CUDA (halves memory, about half the
/// long-prompt time). candle's CPU matmul has no BF16 kernel, so CPU is always FP32.
fn default_precision(checkpoint: &Checkpoint, provider: ExecutionProvider) -> Precision {
    match (provider, checkpoint.id) {
        (ExecutionProvider::Cuda, "d3-nano" | "d3-mini") => Precision::Bf16,
        _ => Precision::F32,
    }
}

pub struct D3Engine {
    checkpoint: &'static Checkpoint,
    directory: PathBuf,
    provider: ExecutionProvider,
    precision: Precision,
    model: Arc<Mutex<Option<Loaded>>>,
}

impl D3Engine {
    /// Checks the provider; the model files load on the first request.
    pub fn load(
        config: &EngineConfig,
        checkpoint: &'static Checkpoint,
    ) -> Result<Self, DecisionError> {
        // candle has no DirectML/Vulkan backend: point at the GPU path that exists.
        if matches!(config.execution_provider, ExecutionProvider::DirectMl) {
            return Err(DecisionError::EngineUnavailable { feature: "cuda" });
        }
        if !config.execution_provider.is_compiled() {
            return Err(DecisionError::EngineUnavailable {
                feature: config.execution_provider.feature().unwrap_or("decision-d3"),
            });
        }
        Ok(Self {
            checkpoint,
            directory: config.models_dir.join(checkpoint.id),
            provider: config.execution_provider,
            precision: default_precision(checkpoint, config.execution_provider),
            model: Arc::new(Mutex::new(None)),
        })
    }

    /// Override the backbone dtype on GPU (CPU stays FP32). Takes effect before the first request.
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

/// System One over a d3 checkpoint. Option probabilities, picks, noul and score values match the
/// checkpoint's `model.system_one()`. `confidence` is the native-engine meaning shared with Laya: the
/// calibrated probability of the reported answer (the top option's probability for choice and score,
/// `max(p, 1 - p)` for noul), not the Python package's entropy-based confidence. Noul answers also carry
/// `probabilities` keyed `false` / `true`. Thresholds on `confidence` are per decision route.
#[async_trait]
impl DecisionEngine for D3Engine {
    fn id(&self) -> &str {
        self.checkpoint.id
    }

    fn egress(&self) -> Egress {
        Egress::Local
    }

    fn limits(&self) -> EngineLimits {
        EngineLimits {
            max_questions: usize::MAX,
            max_choice_options: format::MAX_OPTIONS,
            max_score_levels: format::MAX_SCORE_LEVELS,
            max_state_tokens: None,
        }
    }

    async fn system_one(
        &self,
        request: &DecisionRequest,
    ) -> Result<DecisionResponse, DecisionError> {
        if request.questions.is_empty() {
            return Err(DecisionError::UnsupportedQuestion);
        }
        // Validate every question before touching model files.
        let questions: Vec<(String, format::Prompted)> = request
            .questions
            .iter()
            .map(|(id, q)| Ok((id.clone(), format::normalize(id, q)?)))
            .collect::<Result<_, DecisionError>>()?;
        let state = format::state_text(&request.state);
        let model = Arc::clone(&self.model);
        let directory = self.directory.clone();
        let (provider, precision, checkpoint) = (self.provider, self.precision, self.checkpoint);
        tokio::task::spawn_blocking(move || {
            let mut slot = model.lock().map_err(|_| DecisionError::Internal)?;
            if slot.is_none() {
                let device = device_for(provider)?;
                *slot = Some(Loaded::load(&directory, &device, precision)?);
            }
            let loaded = slot.as_ref().ok_or(DecisionError::Internal)?;
            let mut answers = OrderedMap::new();
            let mut input_tokens = 0u64;
            for (id, question) in &questions {
                let prompt = format::render(&state, question, &loaded.codes);
                let encoding = internal(loaded.tokenizer.encode(prompt.as_str(), false))?;
                let ids = encoding.get_ids();
                if loaded.max_length.is_some_and(|limit| ids.len() > limit) {
                    return Err(DecisionError::StateTooLong);
                }
                input_tokens += ids.len() as u64;
                let probabilities = internal(loaded.probabilities(ids, question.keys.len()))?;
                answers.insert(id.clone(), format::answer(question, &probabilities)?);
            }
            Ok(DecisionResponse {
                answers,
                usage: Usage {
                    input_tokens,
                    output_tokens: 0,
                },
                engine_id: checkpoint.id.to_owned(),
                model: checkpoint.repo.to_owned(),
            })
        })
        .await
        .map_err(|_| DecisionError::Internal)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use audio_graph_decision_contract::{DecisionState, Question};

    #[tokio::test]
    async fn invalid_questions_fail_before_loading_model_files() {
        let config = EngineConfig::new(PathBuf::from("absent-models"));
        let engine = D3Engine::load(&config, &CHECKPOINTS[0]).unwrap();
        let mut request = DecisionRequest {
            state: DecisionState::Text("s".into()),
            questions: OrderedMap::new(),
        };
        assert!(matches!(
            engine.system_one(&request).await,
            Err(DecisionError::UnsupportedQuestion)
        ));
        request.questions.insert(
            "too_many".into(),
            Question::Choice {
                instructions: "pick".into(),
                criteria: (0..256).map(|i| (i.to_string(), None)).collect(),
            },
        );
        assert!(matches!(
            engine.system_one(&request).await,
            Err(DecisionError::CardinalityLimit { question, limit: 255 }) if question == "too_many"
        ));
    }

    #[test]
    fn directml_is_refused_and_cpu_is_always_fp32() {
        let mut config = EngineConfig::new(PathBuf::from("absent-models"));
        config.execution_provider = ExecutionProvider::DirectMl;
        assert!(matches!(
            D3Engine::load(&config, &CHECKPOINTS[0]),
            Err(DecisionError::EngineUnavailable { .. })
        ));
        config.execution_provider = ExecutionProvider::Cpu;
        let nano = CHECKPOINTS.iter().find(|c| c.id == "d3-nano").unwrap();
        let engine = D3Engine::load(&config, nano)
            .unwrap()
            .with_precision(Precision::Bf16);
        assert_eq!(engine.precision(), Precision::F32);
    }

    #[test]
    fn register_adds_the_four_sizes_without_loading_models() {
        let config = EngineConfig::new(PathBuf::from("absent-models"));
        let mut registry = EngineRegistry::default();
        register(&mut registry);
        for checkpoint in CHECKPOINTS {
            assert_eq!(
                registry.create(checkpoint.id, &config).unwrap().id(),
                checkpoint.id
            );
        }
    }
}
