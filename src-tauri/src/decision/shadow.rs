//! Shadow observations never replace heuristic kind or signal, including user-origin questions.
use super::route::{self, DecisionBasis, DecisionOutcome, DecisionRecord, DispatchContext};
use crate::{
    events,
    persistence::FileMemoryRepository,
    settings::{DecisionRouteSettings, PrivacyMode},
};
use audio_graph_decision_contract::{DecisionRequest, DecisionState, OrderedMap, Question};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

pub const LIVE_ASSIST_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgreementCounter {
    pub evaluated: u64,
    pub agreed: u64,
    pub unavailable: u64,
}
#[derive(Default)]
struct SessionCounters {
    session_id: String,
    questions: BTreeMap<String, AgreementCounter>,
}
pub struct ShadowRuntime {
    counters: Mutex<SessionCounters>,
    engines: route::EngineCache,
}
impl ShadowRuntime {
    #[cfg(test)]
    pub(crate) fn set_fixture_engine(
        &self,
        engine: Arc<dyn audio_graph_decision_contract::DecisionEngine>,
    ) {
        self.engines.set_fixture(engine);
    }
    pub fn new(session_id: &str) -> Self {
        Self {
            engines: route::EngineCache::default(),
            counters: Mutex::new(SessionCounters {
                session_id: session_id.into(),
                questions: BTreeMap::new(),
            }),
        }
    }
    pub fn reset(&self, session_id: &str) {
        *self.counters.lock().unwrap_or_else(|e| e.into_inner()) = SessionCounters {
            session_id: session_id.into(),
            questions: BTreeMap::new(),
        };
    }
    pub fn record(&self, session_id: &str, records: &[DecisionRecord]) {
        let mut counters = self.counters.lock().unwrap_or_else(|e| e.into_inner());
        if counters.session_id != session_id {
            return;
        }
        for record in records {
            let counter = counters
                .questions
                .entry(record.question_id.clone())
                .or_default();
            if record.outcome == DecisionOutcome::Unavailable {
                counter.unavailable += 1;
            } else {
                counter.evaluated += 1;
                if record.route_answer.as_deref() == Some(&record.heuristic_answer) {
                    counter.agreed += 1;
                }
            }
        }
    }
    pub fn agreement(&self, session_id: &str) -> BTreeMap<String, AgreementCounter> {
        let counters = self.counters.lock().unwrap_or_else(|e| e.into_inner());
        if counters.session_id == session_id {
            counters.questions.clone()
        } else {
            BTreeMap::new()
        }
    }
}

#[derive(Clone)]
pub(crate) struct ShadowWork {
    pub settings: DecisionRouteSettings,
    pub privacy: PrivacyMode,
    pub models_dir: PathBuf,
    pub runtime: Arc<ShadowRuntime>,
}
impl ShadowWork {
    pub fn from_state(state: &crate::state::AppState, app: &tauri::AppHandle) -> Option<Self> {
        let settings = state.app_settings.read().ok()?;
        if !settings.decision_routes.shadow_enabled {
            return None;
        }
        Some(Self {
            settings: settings.decision_routes.clone(),
            privacy: settings.privacy_mode,
            models_dir: crate::models::get_models_dir(app),
            runtime: state.decision_shadow.clone(),
        })
    }
    pub async fn evaluate(
        &self,
        session_id: &str,
        text: &str,
        confidence: f32,
        kind: Option<&events::AgentProposalKind>,
        signal: events::SignalGrade,
        basis: &[DecisionBasis],
    ) -> Vec<DecisionRecord> {
        if !self.settings.shadow_enabled {
            return Vec::new();
        }
        let request = request(
            text,
            confidence,
            kind == Some(&events::AgentProposalKind::Question),
        );
        let heuristic_kind = kind_key(kind);
        let heuristic_signal = signal_key(signal, confidence);
        let heuristics = [
            ("agent_proposal_kind", heuristic_kind),
            ("answerable_question", heuristic_signal),
        ];
        let repository = FileMemoryRepository::user_data();
        let context = DispatchContext {
            session_id,
            settings: &self.settings,
            privacy: self.privacy,
            models_dir: &self.models_dir,
            basis,
            repository: &repository,
            engines: &self.runtime.engines,
        };
        let selected = route::select_shadow_route(&self.settings, &self.models_dir);
        route::dispatch(selected, &request, &context, &heuristics).await
    }
}

pub(crate) fn kind_key(kind: Option<&events::AgentProposalKind>) -> &'static str {
    match kind {
        Some(events::AgentProposalKind::Question) => "question",
        Some(events::AgentProposalKind::GraphSuggestion) => "graph_suggestion",
        Some(events::AgentProposalKind::Note) => "note",
        None => "none",
    }
}
fn signal_key(signal: events::SignalGrade, confidence: f32) -> &'static str {
    if signal == events::SignalGrade::Strong && confidence >= 0.5 {
        "true"
    } else {
        "false"
    }
}
fn request(text: &str, confidence: f32, question: bool) -> DecisionRequest {
    let criteria = ["question", "graph_suggestion", "note", "none"]
        .into_iter()
        .map(|key| (key.to_owned(), None))
        .collect();
    let mut questions = OrderedMap::from([("agent_proposal_kind".into(), Question::Choice { instructions: "Classify the utterance as a question, graph suggestion (action item or decision), note, or none.".into(), criteria })]);
    if question {
        questions.insert(
            "answerable_question".into(),
            Question::Noul {
                instructions: "Is this a complete, intelligible question safe to auto-answer?"
                    .into(),
                criteria: None,
            },
        );
    }
    DecisionRequest {
        state: DecisionState::Json(serde_json::json!({"text": text, "asr_confidence": confidence})),
        questions,
    }
}

pub(crate) fn stamp(proposal: &mut events::AgentProposalPayload, records: Vec<DecisionRecord>) {
    if !records.is_empty() {
        proposal.decision_provenance = Some(records);
    }
}

#[cfg(test)]
pub(crate) struct DisagreeingEngine;
#[cfg(test)]
#[audio_graph_decision_contract::async_trait]
impl audio_graph_decision_contract::DecisionEngine for DisagreeingEngine {
    fn id(&self) -> &str {
        "d3-edge"
    }
    fn egress(&self) -> audio_graph_decision_contract::Egress {
        audio_graph_decision_contract::Egress::Local
    }
    fn limits(&self) -> audio_graph_decision_contract::EngineLimits {
        route::route_by_id("route.decision.d3_edge").unwrap().limits
    }
    async fn system_one(
        &self,
        request: &DecisionRequest,
    ) -> Result<
        audio_graph_decision_contract::DecisionResponse,
        audio_graph_decision_contract::DecisionError,
    > {
        use audio_graph_decision_contract::{Answer, DecisionResponse, Usage};
        let answers = request
            .questions
            .iter()
            .map(|(id, question)| {
                let answer = match question {
                    Question::Choice { criteria, .. } => Answer::Choice {
                        choice: "none".into(),
                        probabilities: criteria
                            .keys()
                            .map(|key| (key.clone(), if key == "none" { 1.0 } else { 0.0 }))
                            .collect(),
                        confidence: 1.0,
                    },
                    Question::Noul { .. } => Answer::Noul {
                        noul: 0.0,
                        probabilities: None,
                        confidence: Some(1.0),
                    },
                    _ => unreachable!(),
                };
                (id.clone(), answer)
            })
            .collect();
        Ok(DecisionResponse {
            answers,
            usage: Usage {
                input_tokens: 8,
                output_tokens: 0,
            },
            engine_id: self.id().into(),
            model: "vllm-sr/d3-edge".into(),
        })
    }
}
