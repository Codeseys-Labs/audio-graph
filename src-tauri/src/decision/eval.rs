//! Read-only session/fixture replay. Agreement is with heuristics, not human truth.

use super::{CloudflareModel, EndpointKind, SystemOneHttpEngine};
use crate::events::{AgentProposalKind, AgentProposalPayload, SignalGrade};
use crate::persistence::{FileMemoryRepository, LocalMemoryRepository};
use crate::projection_eval::{OfflineProjectionReplayFixture, OfflineProjectionReplayStep};
use crate::state::TranscriptSegment;
use audio_graph_decision_contract::{
    Answer, DecisionEngine, DecisionRequest, DecisionState, OrderedMap, Question, Usage,
};
use serde::{Deserialize, Serialize};
use std::{path::Path, time::Instant};

pub const KIND_LABELS: [&str; 4] = ["question", "graph_suggestion", "note", "none"];
pub const GRADE_LABELS: [&str; 3] = ["fragment", "weak", "strong"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplaySpan {
    pub id: String,
    pub text: String,
    pub confidence: f32,
}

impl From<TranscriptSegment> for ReplaySpan {
    fn from(segment: TranscriptSegment) -> Self {
        Self {
            id: segment.id,
            text: segment.text,
            confidence: segment.confidence,
        }
    }
}

pub struct ReplayCorpus {
    pub spans: Vec<ReplaySpan>,
    pub persisted_cards: usize,
}

/// Canonical transcript revisions take precedence over legacy segments, exactly
/// as session loading does. Card heads are counted, not used as reference labels.
pub fn load_session(session_id: &str) -> Result<ReplayCorpus, String> {
    let segments = crate::persistence::load_transcript_segments_preferring_ledger(session_id)?;
    let cards = FileMemoryRepository::user_data().load_live_assist_cards(session_id)?;
    Ok(ReplayCorpus {
        spans: segments.into_iter().map(ReplaySpan::from).collect(),
        persisted_cards: cards.len(),
    })
}

/// Uses the existing projection fixture schema and canonical ledger collapse;
/// partials/revisions cannot accidentally count as extra evaluation samples.
pub fn load_fixture(path: &Path) -> Result<ReplayCorpus, String> {
    let json =
        std::fs::read_to_string(path).map_err(|_| "Could not read replay fixture".to_owned())?;
    let fixture: OfflineProjectionReplayFixture =
        serde_json::from_str(&json).map_err(|_| "Invalid projection replay fixture".to_owned())?;
    let events = fixture
        .steps
        .into_iter()
        .filter_map(|step| match step {
            OfflineProjectionReplayStep::Transcript { event } => Some(event),
            _ => None,
        })
        .collect::<Vec<_>>();
    let ledger = crate::projections::TranscriptLedger::replay(&fixture.session_id, events)
        .map_err(|_| "Transcript fixture replay failed".to_owned())?;
    let segments = crate::projections::derive_legacy_transcript_segments(&ledger);
    Ok(ReplayCorpus {
        spans: segments.into_iter().map(ReplaySpan::from).collect(),
        persisted_cards: 0,
    })
}

fn heuristic(span: &ReplaySpan) -> (usize, Option<usize>) {
    let kind = crate::speech::agent_proposal_kind(span.text.trim());
    let index = match kind {
        Some(AgentProposalKind::Question) => 0,
        Some(AgentProposalKind::GraphSuggestion) => 1,
        Some(AgentProposalKind::Note) => 2,
        None => 3,
    };
    if index != 0 {
        return (index, None);
    }
    let confidence = if span.confidence.is_finite() {
        span.confidence.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let payload = AgentProposalPayload {
        id: span.id.clone(),
        source_segment_id: span.id.clone(),
        source_id: "replay".to_owned(),
        speaker_label: None,
        kind: AgentProposalKind::Question,
        title: String::new(),
        body: crate::speech::agent_proposal_body(&AgentProposalKind::Question, span.text.trim()),
        confidence,
        created_at_ms: 0,
        signal: None,
    };
    let grade = match crate::speech::agent_signal_grade(&payload) {
        SignalGrade::Fragment => 0,
        SignalGrade::Weak => 1,
        SignalGrade::Strong => 2,
    };
    (index, Some(grade))
}

pub fn questions(span: &ReplaySpan) -> DecisionRequest {
    let criteria = [
        ("question", "A question asking for information or clarification."),
        ("graph_suggestion", "An action item, follow-up, to-do, decision, or relationship worth reviewing for the graph."),
        ("note", "Important context to remember or keep as a note."),
        ("none", "No question, action, decision, or important context to retain."),
    ].into_iter().map(|(name, description)| (name.to_owned(), Some(description.to_owned()))).collect();
    DecisionRequest {
        state: DecisionState::Json(serde_json::json!({ "text": span.text, "asr_confidence": span.confidence })),
        questions: OrderedMap::from([
            ("agent_proposal_kind".to_owned(), Question::Choice {
                instructions: "Classify the speech span in `text` for a meeting assistant. Pick its primary intent; questions take precedence over graph suggestions, which take precedence over notes. Use none for ordinary statements without actionable or important context.".to_owned(), criteria,
            }),
            ("agent_signal_grade".to_owned(), Question::Score {
                instructions: "If this span is a question, grade whether it is safe to spend an automatic answer on it. Consider `asr_confidence` and whether `text` is a complete, intelligible question. This speculative result is ignored by code unless kind is question.".to_owned(),
                criteria: vec!["fragment: incomplete, too short, or dangling clause".to_owned(), "weak: ASR confidence below 0.5".to_owned(), "strong: complete, actionable question with ASR confidence at least 0.5".to_owned()],
            }),
        ]),
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CalibrationBin {
    pub count: usize,
    pub probability_sum: f64,
    pub correct: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Calibration {
    pub bins: [CalibrationBin; 10],
    pub ece: Option<f64>,
}

impl Default for Calibration {
    fn default() -> Self {
        Self {
            bins: std::array::from_fn(|_| CalibrationBin::default()),
            ece: None,
        }
    }
}
impl Calibration {
    fn observe(&mut self, probability: f64, correct: bool) {
        let index = ((probability * 10.0).floor() as usize).min(9);
        let bin = &mut self.bins[index];
        bin.count += 1;
        bin.probability_sum += probability;
        bin.correct += usize::from(correct);
    }
    fn finish(&mut self) {
        let count: usize = self.bins.iter().map(|b| b.count).sum();
        if count != 0 {
            self.ece = Some(
                self.bins
                    .iter()
                    .map(|b| (b.correct as f64 - b.probability_sum).abs())
                    .sum::<f64>()
                    / count as f64,
            );
        }
    }
}

#[derive(Debug, Serialize)]
pub struct EvalReport {
    pub engine_id: String,
    pub models: Vec<String>,
    pub spans: usize,
    pub persisted_cards: usize,
    pub kind_labels: [&'static str; 4],
    pub grade_labels: [&'static str; 3],
    /// Rows: heuristic; columns: model. Not an accuracy claim.
    pub kind_confusion: [[usize; 4]; 4],
    pub kind_agreement: f64,
    /// Only the intersection of heuristic-question and model-question spans.
    pub grade_confusion: [[usize; 3]; 3],
    pub grade_compared: usize,
    pub grade_agreement: Option<f64>,
    pub heuristic_questions: usize,
    pub model_questions: usize,
    pub kind_calibration: Calibration,
    pub grade_calibration: Calibration,
    pub latency_ms_p50: f64,
    pub latency_ms_p95: f64,
    pub usage: Usage,
    pub usd_per_million_input_tokens: f64,
    pub estimated_cost_usd: f64,
}

fn percentile(sorted: &[f64], percent: f64) -> f64 {
    sorted[((sorted.len() as f64 * percent).ceil() as usize).saturating_sub(1)]
}

pub fn input_price(kind: &EndpointKind) -> f64 {
    match kind {
        EndpointKind::TypeSafe
        | EndpointKind::Cloudflare {
            model: CloudflareModel::Jev,
            ..
        } => 0.042,
        EndpointKind::Cloudflare {
            model: CloudflareModel::ClefFlash,
            ..
        } => 0.038,
        EndpointKind::Ollama { .. } | EndpointKind::Vllm { .. } => 0.0,
    }
}

pub async fn run(
    engine: &SystemOneHttpEngine,
    corpus: &ReplayCorpus,
    price: f64,
) -> Result<EvalReport, String> {
    if corpus.spans.is_empty() {
        return Err("No final speech spans to evaluate".to_owned());
    }
    if !price.is_finite() || price < 0.0 {
        return Err("Input price must be finite and nonnegative".to_owned());
    }
    let mut report = EvalReport {
        engine_id: engine.id().to_owned(),
        models: Vec::new(),
        spans: corpus.spans.len(),
        persisted_cards: corpus.persisted_cards,
        kind_labels: KIND_LABELS,
        grade_labels: GRADE_LABELS,
        kind_confusion: [[0; 4]; 4],
        kind_agreement: 0.0,
        grade_confusion: [[0; 3]; 3],
        grade_compared: 0,
        grade_agreement: None,
        heuristic_questions: 0,
        model_questions: 0,
        kind_calibration: Calibration::default(),
        grade_calibration: Calibration::default(),
        latency_ms_p50: 0.0,
        latency_ms_p95: 0.0,
        usage: Usage::default(),
        usd_per_million_input_tokens: price,
        estimated_cost_usd: 0.0,
    };
    let mut latency = Vec::with_capacity(corpus.spans.len());
    for span in &corpus.spans {
        let reference = heuristic(span);
        let request = questions(span);
        let start = Instant::now();
        let response = engine
            .system_one(&request)
            .await
            .map_err(|error| error.to_string())?;
        latency.push(start.elapsed().as_secs_f64() * 1000.0);
        if !report.models.contains(&response.model) {
            report.models.push(response.model.clone());
        }
        report.usage.input_tokens += response.usage.input_tokens;
        report.usage.output_tokens += response.usage.output_tokens;
        let Answer::Choice {
            choice,
            probabilities,
            ..
        } = &response.answers["agent_proposal_kind"]
        else {
            return Err("Expected kind Choice".to_owned());
        };
        let predicted = KIND_LABELS
            .iter()
            .position(|label| label == choice)
            .ok_or("Unknown kind answer")?;
        report.kind_confusion[reference.0][predicted] += 1;
        report
            .kind_calibration
            .observe(probabilities[choice], reference.0 == predicted);
        report.heuristic_questions += usize::from(reference.0 == 0);
        report.model_questions += usize::from(predicted == 0);
        // Never apply a grade to a model non-question. Separately report misses
        // in kind_confusion so this conditional denominator cannot hide them.
        if let (0, Some(reference_grade)) = (predicted, reference.1) {
            let Answer::Score { probabilities, .. } = &response.answers["agent_signal_grade"]
            else {
                return Err("Expected grade Score".to_owned());
            };
            // A score is an expectation; use argmax level for confusion/ECE.
            // Strict > preserves the lower index on tied levels.
            let predicted_grade = probabilities.iter().enumerate().fold(0, |best, (i, p)| {
                if *p > probabilities[best] { i } else { best }
            });
            report.grade_confusion[reference_grade][predicted_grade] += 1;
            report.grade_compared += 1;
            report.grade_calibration.observe(
                probabilities[predicted_grade],
                reference_grade == predicted_grade,
            );
        }
    }
    report.kind_agreement =
        (0..4).map(|i| report.kind_confusion[i][i]).sum::<usize>() as f64 / report.spans as f64;
    if report.grade_compared > 0 {
        report.grade_agreement = Some(
            (0..3).map(|i| report.grade_confusion[i][i]).sum::<usize>() as f64
                / report.grade_compared as f64,
        );
    }
    report.kind_calibration.finish();
    report.grade_calibration.finish();
    latency.sort_by(f64::total_cmp);
    report.latency_ms_p50 = percentile(&latency, 0.5);
    report.latency_ms_p95 = percentile(&latency, 0.95);
    report.estimated_cost_usd = report.usage.input_tokens as f64 * price / 1_000_000.0;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ece_uses_weighted_bins_including_probability_one() {
        let mut calibration = Calibration::default();
        calibration.observe(0.2, false);
        calibration.observe(0.8, true);
        calibration.observe(1.0, false);
        calibration.finish();
        assert!((calibration.ece.unwrap() - 1.4 / 3.0).abs() < 1e-12);
        assert_eq!(calibration.bins[9].count, 1);
        assert_eq!(Calibration::default().ece, None);
    }
    #[test]
    fn fixture_loader_collapses_superseded_revisions() {
        let mut fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../fixtures/projection_eval/two_span_repair.json"
        ))
        .unwrap();
        let steps = fixture["steps"].as_array_mut().unwrap();
        let mut partial = steps[0].clone();
        partial["event"]["text"] = "obsolete partial".into();
        partial["event"]["is_final"] = false.into();
        partial["event"]["stability"] = "partial".into();
        steps[0]["event"]["revision_number"] = 2.into();
        steps[0]["event"]["received_at_ms"] = 1001.into();
        steps.insert(0, partial);
        let path =
            std::env::temp_dir().join(format!("decision-replay-{}.json", uuid::Uuid::new_v4()));
        std::fs::write(&path, serde_json::to_vec(&fixture).unwrap()).unwrap();
        let corpus = load_fixture(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(
            corpus
                .spans
                .iter()
                .map(|s| s.text.as_str())
                .collect::<Vec<_>>(),
            ["first span", "second span"]
        );
    }
}
