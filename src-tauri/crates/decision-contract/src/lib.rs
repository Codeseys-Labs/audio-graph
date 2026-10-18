//! Typed System One decisions. `WireRequest` and `WireResponse` retain the original
//! JSON bytes, including vendor extensions and field order; typed values are the
//! engine-neutral projection used by native engines.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::value::RawValue;

pub use async_trait::async_trait;
pub use indexmap::IndexMap as OrderedMap;
pub type QuestionId = String;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DecisionState {
    Text(String),
    Json(serde_json::Value),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionRequest {
    pub state: DecisionState,
    pub questions: OrderedMap<QuestionId, Question>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Question {
    Choice {
        instructions: String,
        criteria: OrderedMap<String, Option<String>>,
    },
    Noul {
        instructions: String,
        #[serde(default)]
        criteria: Option<NoulCriteria>,
    },
    Score {
        instructions: String,
        criteria: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NoulCriteria {
    #[serde(rename = "true", default, skip_serializing_if = "Option::is_none")]
    pub true_desc: Option<String>,
    #[serde(rename = "false", default, skip_serializing_if = "Option::is_none")]
    pub false_desc: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Answer {
    Choice {
        choice: String,
        probabilities: OrderedMap<String, f64>,
        confidence: f64,
    },
    Noul {
        noul: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        probabilities: Option<OrderedMap<String, f64>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        confidence: Option<f64>,
    },
    Score {
        score: f64,
        #[serde(with = "score_probabilities")]
        probabilities: Vec<f64>,
        confidence: f64,
    },
}

mod score_probabilities {
    use super::*;
    pub fn serialize<S: Serializer>(values: &[f64], serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(values.len()))?;
        for (index, value) in values.iter().enumerate() {
            map.serialize_entry(&index.to_string(), value)?;
        }
        map.end()
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<f64>, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Probabilities {
            List(Vec<f64>),
            Map(OrderedMap<String, f64>),
        }
        match Probabilities::deserialize(deserializer)? {
            Probabilities::List(values) => Ok(values),
            Probabilities::Map(map) => {
                let mut values = vec![0.0; map.len()];
                let mut seen = vec![false; map.len()];
                for (key, value) in map {
                    let index: usize = key.parse().map_err(serde::de::Error::custom)?;
                    if index >= values.len() || seen[index] || key != index.to_string() {
                        return Err(serde::de::Error::custom(
                            "score indices must be contiguous from zero",
                        ));
                    }
                    seen[index] = true;
                    values[index] = value;
                }
                Ok(values)
            }
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionResponse {
    pub answers: OrderedMap<QuestionId, Answer>,
    pub usage: Usage,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub engine_id: String,
    #[serde(default)]
    pub model: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Egress {
    Local,
    Cloud,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineLimits {
    pub max_questions: usize,
    pub max_choice_options: usize,
    pub max_score_levels: usize,
    pub max_state_tokens: Option<u32>,
}

impl EngineLimits {
    /// Token limits require the engine's actual tokenizer, never a byte heuristic.
    pub fn validate(&self, request: &DecisionRequest) -> Result<(), DecisionError> {
        if request.questions.is_empty() || request.questions.len() > self.max_questions {
            return Err(DecisionError::CardinalityLimit {
                question: String::new(),
                limit: self.max_questions,
            });
        }
        for (id, question) in &request.questions {
            if id.is_empty() {
                return Err(DecisionError::UnsupportedQuestion);
            }
            let (count, minimum, limit) = match question {
                Question::Choice { criteria, .. } => (criteria.len(), 1, self.max_choice_options),
                Question::Score { criteria, .. } => (criteria.len(), 2, self.max_score_levels),
                Question::Noul { .. } => continue,
            };
            if count < minimum || count > limit {
                return Err(DecisionError::CardinalityLimit {
                    question: id.clone(),
                    limit,
                });
            }
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DecisionError {
    #[error("unsupported decision question")]
    UnsupportedQuestion,
    #[error("question {question:?} exceeds cardinality limit {limit} (or is below the minimum)")]
    CardinalityLimit { question: QuestionId, limit: usize },
    #[error("decision state exceeds the engine context window")]
    StateTooLong,
    #[error("decision transport: {0}")]
    Transport(String),
    #[error("decision credential missing or rejected")]
    Credential,
    #[error("decision engine unavailable: enable feature {feature}")]
    EngineUnavailable { feature: &'static str },
    #[error("internal decision engine error")]
    Internal,
}

#[async_trait]
pub trait DecisionEngine: Send + Sync {
    fn id(&self) -> &str;
    fn egress(&self) -> Egress;
    fn limits(&self) -> EngineLimits;
    async fn system_one(&self, req: &DecisionRequest) -> Result<DecisionResponse, DecisionError>;
}

/// Immutable typed projection plus the original JSON, including unknown fields.
/// Use this at transport/archive boundaries when lossless replay matters.
#[derive(Debug, Clone)]
pub struct Wire<T> {
    typed: T,
    raw: Box<RawValue>,
}
pub type WireRequest = Wire<DecisionRequest>;
pub type WireResponse = Wire<DecisionResponse>;

impl<T> Wire<T> {
    pub fn typed(&self) -> &T {
        &self.typed
    }
    pub fn into_typed(self) -> T {
        self.typed
    }
    pub fn json(&self) -> &str {
        self.raw.get()
    }
}
impl<T: for<'de> Deserialize<'de>> Wire<T> {
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        let raw = RawValue::from_string(json.to_owned())?;
        let typed = serde_json::from_str(raw.get())?;
        Ok(Self { typed, raw })
    }
}
impl<T> Serialize for Wire<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.raw.serialize(serializer)
    }
}
impl<'de, T: for<'a> Deserialize<'a>> Deserialize<'de> for Wire<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = Box::<RawValue>::deserialize(deserializer)?;
        let typed = serde_json::from_str(raw.get()).map_err(serde::de::Error::custom)?;
        Ok(Self { typed, raw })
    }
}
