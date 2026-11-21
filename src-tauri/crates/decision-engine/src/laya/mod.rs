mod sequence;

use crate::{
    DecisionEngine, DecisionError, DecisionRequest, DecisionResponse, DecisionState, Egress,
    EngineConfig, EngineLimits, ExecutionProvider, LAYA_ENGLISH, LAYA_MULTILINGUAL, OrderedMap,
    Question, Usage,
};
use async_trait::async_trait;
use ort::{
    session::{
        Session,
        builder::{GraphOptimizationLevel, SessionBuilder},
    },
    value::Tensor,
};
use sequence::{Config, SpecialIds};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tokenizers::Tokenizer;
use unicode_script::{Script, UnicodeScript};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Checkpoint {
    English,
    Multilingual,
}

pub fn checkpoint_for(text: &str) -> Checkpoint {
    if text.chars().any(|ch| {
        !matches!(
            ch.script(),
            Script::Latin | Script::Common | Script::Inherited
        )
    }) {
        Checkpoint::Multilingual
    } else {
        Checkpoint::English
    }
}

pub fn checkpoint_for_request(request: &DecisionRequest) -> Checkpoint {
    let non_latin = |text: &str| checkpoint_for(text) == Checkpoint::Multilingual;
    fn json_has_non_latin(value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::String(text) => checkpoint_for(text) == Checkpoint::Multilingual,
            serde_json::Value::Array(values) => values.iter().any(json_has_non_latin),
            serde_json::Value::Object(values) => values.iter().any(|(key, value)| {
                checkpoint_for(key) == Checkpoint::Multilingual || json_has_non_latin(value)
            }),
            _ => false,
        }
    }
    let state_non_latin = match &request.state {
        DecisionState::Text(text) => non_latin(text),
        DecisionState::Json(value) => json_has_non_latin(value),
    };
    if state_non_latin {
        return Checkpoint::Multilingual;
    }
    for question in request.questions.values() {
        let (instructions, criteria_non_latin) = match question {
            Question::Choice {
                instructions,
                criteria,
            } => (
                instructions,
                criteria
                    .iter()
                    .any(|(key, value)| non_latin(key) || value.as_deref().is_some_and(non_latin)),
            ),
            Question::Score {
                instructions,
                criteria,
            } => (instructions, criteria.iter().any(|value| non_latin(value))),
            Question::Noul {
                instructions,
                criteria,
            } => (
                instructions,
                criteria.as_ref().is_some_and(|c| {
                    c.true_desc.as_deref().is_some_and(non_latin)
                        || c.false_desc.as_deref().is_some_and(non_latin)
                }),
            ),
        };
        if non_latin(instructions) || criteria_non_latin {
            return Checkpoint::Multilingual;
        }
    }
    Checkpoint::English
}

struct Model {
    session: Session,
    tok: Tokenizer,
    config: Config,
    special: SpecialIds,
}

fn internal<T, E>(result: Result<T, E>) -> Result<T, DecisionError> {
    result.map_err(|_| DecisionError::Internal)
}

fn provider_builder(provider: ExecutionProvider) -> Result<SessionBuilder, DecisionError> {
    if !provider.is_compiled() {
        return Err(DecisionError::EngineUnavailable {
            feature: provider.feature().unwrap_or("decision-laya"),
        });
    }
    let library = std::env::var_os("ORT_DYLIB_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(format!(
                "{}onnxruntime{}",
                std::env::consts::DLL_PREFIX,
                std::env::consts::DLL_SUFFIX
            ))
        });
    let _ = internal(ort::init_from(library))?
        .with_telemetry(false)
        .commit();
    let builder = internal(Session::builder())?;
    let builder = internal(builder.with_optimization_level(GraphOptimizationLevel::Level3))?;
    match provider {
        ExecutionProvider::Cpu => Ok(builder),
        #[cfg(all(feature = "cuda", any(target_os = "windows", target_os = "linux")))]
        ExecutionProvider::Cuda => internal(
            builder.with_execution_providers([ort::ep::CUDA::default()
                .with_tf32(false)
                .build()
                .error_on_failure()]),
        ),
        #[cfg(all(feature = "vulkan", target_os = "windows"))]
        ExecutionProvider::DirectMl => {
            let builder = internal(builder.with_parallel_execution(false))?;
            let builder = internal(builder.with_memory_pattern(false))?;
            internal(builder.with_execution_providers([
                ort::ep::DirectML::default().build().error_on_failure(),
            ]))
        }
        #[cfg(all(feature = "metal", target_os = "macos"))]
        ExecutionProvider::CoreMl => internal(
            builder
                .with_execution_providers([ort::ep::CoreML::default().build().error_on_failure()]),
        ),
        #[allow(unreachable_patterns)]
        _ => Err(DecisionError::EngineUnavailable {
            feature: provider.feature().unwrap_or("decision-laya"),
        }),
    }
}

impl Model {
    fn load(directory: &Path, provider: ExecutionProvider) -> Result<Self, DecisionError> {
        let (graph_file, config_file) = if directory.join("model-fp32.onnx").is_file() {
            ("model-fp32.onnx", "rl_agent_config.json")
        } else {
            ("laya.onnx", "laya_config.json")
        };
        let config: Config = serde_json::from_slice(
            &std::fs::read(directory.join(config_file)).map_err(|_| DecisionError::Internal)?,
        )
        .map_err(|_| DecisionError::Internal)?;
        if config.max_len < 16 || config.max_len > 1024 || config.head_max_len < 16 {
            return Err(DecisionError::Internal);
        }
        let mut tok = Tokenizer::from_file(directory.join("tokenizer/tokenizer.json"))
            .map_err(|_| DecisionError::Internal)?;
        tok.with_truncation(None)
            .map_err(|_| DecisionError::Internal)?;
        tok.with_padding(None);
        let special_config: serde_json::Value = serde_json::from_slice(
            &std::fs::read(directory.join("tokenizer/tokenizer_config.json"))
                .map_err(|_| DecisionError::Internal)?,
        )
        .map_err(|_| DecisionError::Internal)?;
        let token = |name: &str, fallback: &str| -> Result<(String, i64), DecisionError> {
            let value = special_config
                .get(name)
                .and_then(|v| {
                    v.as_str()
                        .or_else(|| v.get("content").and_then(|v| v.as_str()))
                })
                .unwrap_or(fallback);
            Ok((
                value.into(),
                i64::from(tok.token_to_id(value).ok_or(DecisionError::Internal)?),
            ))
        };
        let (mask_text, mask) = token("mask_token", "[MASK]")?;
        let special = SpecialIds {
            cls: token("cls_token", "[CLS]")?.1,
            sep: token("sep_token", "[SEP]")?.1,
            pad: token("pad_token", "[PAD]")?.1,
            mask,
            mask_text,
        };
        let session =
            internal(provider_builder(provider)?.commit_from_file(directory.join(graph_file)))?;
        let required_inputs = [
            "input_ids",
            "attention_mask",
            "marker_pos",
            "marker_mask",
            "qtype",
        ];
        if session.inputs().len() != required_inputs.len()
            || !required_inputs
                .iter()
                .all(|name| session.inputs().iter().any(|input| input.name() == *name))
            || !session
                .outputs()
                .iter()
                .any(|output| output.name() == "logits")
        {
            return Err(DecisionError::Internal);
        }
        Ok(Self {
            session,
            tok,
            config,
            special,
        })
    }

    fn run(
        &mut self,
        request: &DecisionRequest,
        state: &str,
        allow_large: bool,
        model: &str,
    ) -> Result<DecisionResponse, DecisionError> {
        if request.questions.is_empty() {
            return Err(DecisionError::UnsupportedQuestion);
        }
        let newest_first = matches!(
            &request.state,
            DecisionState::Json(serde_json::Value::Array(_))
        );
        let state_ids = sequence::encode(&self.tok, &self.special, state)?;
        let sequences = request
            .questions
            .iter()
            .map(|(id, q)| {
                sequence::build(
                    &self.tok,
                    &self.special,
                    &self.config,
                    sequence::StateWindow {
                        tokens: &state_ids,
                        keep_newest: newest_first,
                    },
                    id,
                    q,
                    allow_large,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let batch = sequences.len();
        let length = sequences
            .iter()
            .map(|s| s.ids.len())
            .max()
            .ok_or(DecisionError::Internal)?;
        let width = sequences
            .iter()
            .map(|s| s.markers.len())
            .max()
            .unwrap_or(2)
            .max(2);
        let mut ids = vec![self.special.pad; batch * length];
        let mut attention = vec![0_i64; batch * length];
        let mut markers = vec![0_i64; batch * width];
        let mut mask = vec![false; batch * width];
        let mut kinds = Vec::with_capacity(batch);
        let mut input_tokens = 0;
        for (row, seq) in sequences.iter().enumerate() {
            ids[row * length..row * length + seq.ids.len()].copy_from_slice(&seq.ids);
            attention[row * length..row * length + seq.ids.len()].fill(1);
            markers[row * width..row * width + seq.markers.len()].copy_from_slice(&seq.markers);
            mask[row * width..row * width + seq.markers.len()].fill(true);
            kinds.push(seq.kind as i64);
            input_tokens += seq.ids.len() as u64;
        }
        let outputs = internal(self.session.run(ort::inputs![
            "input_ids" => internal(Tensor::from_array(([batch, length], ids)))?,
            "attention_mask" => internal(Tensor::from_array(([batch, length], attention)))?,
            "marker_pos" => internal(Tensor::from_array(([batch, width], markers)))?,
            "marker_mask" => internal(Tensor::from_array(([batch, width], mask)))?,
            "qtype" => internal(Tensor::from_array(([batch], kinds)))?,
        ]))?;
        let logits_value = outputs.get("logits").ok_or(DecisionError::Internal)?;
        let (shape, logits) = internal(logits_value.try_extract_tensor::<f32>())?;
        if shape.as_ref() != [batch as i64, width as i64] {
            return Err(DecisionError::Internal);
        }
        let mut answers = OrderedMap::with_capacity(batch);
        for (row, ((id, question), seq)) in request.questions.iter().zip(&sequences).enumerate() {
            let k = seq.markers.len();
            answers.insert(
                id.clone(),
                sequence::answer(
                    question,
                    &logits[row * width..row * width + k],
                    self.config.temperature(seq.kind, k),
                )?,
            );
        }
        Ok(DecisionResponse {
            answers,
            usage: Usage {
                input_tokens,
                output_tokens: 0,
            },
            engine_id: "laya".into(),
            model: model.into(),
        })
    }
}

/// One lock per checkpoint: loading one model must not stall requests routed to the other.
#[derive(Default)]
struct LoadedModels {
    english: Mutex<Option<Model>>,
    multilingual: Mutex<Option<Model>>,
}

pub struct LayaEngine {
    directory: PathBuf,
    provider: ExecutionProvider,
    allow_large: bool,
    models: Arc<LoadedModels>,
}

impl LayaEngine {
    pub fn load(config: &EngineConfig) -> Result<Self, DecisionError> {
        if !config.execution_provider.is_compiled() {
            return Err(DecisionError::EngineUnavailable {
                feature: config
                    .execution_provider
                    .feature()
                    .unwrap_or("decision-laya"),
            });
        }
        Ok(Self {
            directory: config.models_dir.clone(),
            provider: config.execution_provider,
            allow_large: config.allow_large_choice_sets,
            models: Arc::default(),
        })
    }
}

#[async_trait]
impl DecisionEngine for LayaEngine {
    fn id(&self) -> &str {
        "laya"
    }
    fn egress(&self) -> Egress {
        Egress::Local
    }
    fn limits(&self) -> EngineLimits {
        EngineLimits {
            max_questions: usize::MAX,
            max_choice_options: if self.allow_large { 44 } else { 20 },
            max_score_levels: 44,
            max_state_tokens: Some(1024),
        }
    }
    async fn system_one(
        &self,
        request: &DecisionRequest,
    ) -> Result<DecisionResponse, DecisionError> {
        if request.questions.is_empty() {
            return Err(DecisionError::UnsupportedQuestion);
        }
        for (id, question) in &request.questions {
            let (count, limit) = match question {
                Question::Choice { criteria, .. } => {
                    (criteria.len(), self.limits().max_choice_options)
                }
                Question::Score { criteria, .. } => {
                    (criteria.len(), self.limits().max_score_levels)
                }
                Question::Noul { .. } => (2, 2),
            };
            if count == 0 {
                return Err(DecisionError::UnsupportedQuestion);
            }
            if count > limit {
                return Err(DecisionError::CardinalityLimit {
                    question: id.clone(),
                    limit,
                });
            }
        }
        let checkpoint = checkpoint_for_request(request);
        let request = request.clone();
        let models = Arc::clone(&self.models);
        let directory = self.directory.clone();
        let provider = self.provider;
        let allow_large = self.allow_large;
        tokio::task::spawn_blocking(move || {
            let state = sequence::state_text(&request.state);
            let (slot, folder) = match checkpoint {
                Checkpoint::English => (&models.english, LAYA_ENGLISH),
                Checkpoint::Multilingual => (&models.multilingual, LAYA_MULTILINGUAL),
            };
            let mut slot = slot.lock().map_err(|_| DecisionError::Internal)?;
            if slot.is_none() {
                *slot = Some(Model::load(&directory.join(folder), provider)?);
            }
            slot.as_mut()
                .ok_or(DecisionError::Internal)?
                .run(&request, &state, allow_large, folder)
        })
        .await
        .map_err(|_| DecisionError::Internal)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_script_routes_without_discarding_mixed_language_text() {
        assert_eq!(checkpoint_for("English, café, 123!"), Checkpoint::English);
        for text in ["English 中文", "مرحبا", "Привет", "नमस्ते", "日本語"]
        {
            assert_eq!(checkpoint_for(text), Checkpoint::Multilingual);
        }
    }

    #[tokio::test]
    async fn invalid_answer_spaces_fail_before_loading_model_files() {
        let engine = LayaEngine::load(&EngineConfig::new(PathBuf::from("absent-models"))).unwrap();
        let mut request = DecisionRequest {
            state: DecisionState::Text(String::new()),
            questions: OrderedMap::new(),
        };
        assert!(matches!(
            engine.system_one(&request).await,
            Err(DecisionError::UnsupportedQuestion)
        ));
        request.questions.insert(
            "oversized".into(),
            Question::Choice {
                instructions: "choose".into(),
                criteria: (0..21).map(|i| (i.to_string(), None)).collect(),
            },
        );
        assert!(
            matches!(engine.system_one(&request).await, Err(DecisionError::CardinalityLimit { question, limit: 20 }) if question == "oversized")
        );
    }
}
