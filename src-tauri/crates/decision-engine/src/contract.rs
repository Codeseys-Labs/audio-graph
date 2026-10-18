use async_trait::async_trait;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

pub type QuestionId = String;
pub type OrderedMap<K, V> = IndexMap<K, V>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DecisionState {
    Text(String),
    Json(serde_json::Value),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionRequest {
    pub state: DecisionState,
    pub questions: OrderedMap<QuestionId, Question>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoulCriteria {
    #[serde(rename = "true", skip_serializing_if = "Option::is_none")]
    pub true_desc: Option<String>,
    #[serde(rename = "false", skip_serializing_if = "Option::is_none")]
    pub false_desc: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Question {
    Choice {
        instructions: String,
        criteria: OrderedMap<String, Option<String>>,
    },
    Noul {
        instructions: String,
        criteria: Option<NoulCriteria>,
    },
    Score {
        instructions: String,
        criteria: Vec<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Answer {
    Choice {
        choice: String,
        probabilities: OrderedMap<String, f64>,
        confidence: f64,
    },
    Noul {
        noul: f64,
        probabilities: Option<OrderedMap<String, f64>>,
        confidence: Option<f64>,
    },
    Score {
        score: f64,
        probabilities: Vec<f64>,
        confidence: f64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionResponse {
    pub answers: OrderedMap<QuestionId, Answer>,
    pub usage: Usage,
    pub engine_id: String,
    pub model: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Egress {
    Local,
    Cloud,
}

#[derive(Debug, Clone, Copy)]
pub struct EngineLimits {
    pub max_questions: usize,
    pub max_choice_options: usize,
    pub max_score_levels: usize,
    pub max_state_tokens: Option<u32>,
}

#[derive(Debug, thiserror::Error)]
pub enum DecisionError {
    #[error("unsupported question")]
    UnsupportedQuestion,
    #[error("question {question} exceeds limit {limit}")]
    CardinalityLimit { question: String, limit: usize },
    #[error("state too long")]
    StateTooLong,
    #[error("decision transport failed: {0}")]
    Transport(String),
    #[error("decision credential unavailable")]
    Credential,
    #[error("decision engine unavailable; enable {feature}")]
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
