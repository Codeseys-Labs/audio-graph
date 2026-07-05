//! No-network projection replay/evaluation harness.
//!
//! This module feeds deterministic transcript fixtures through the same ledger,
//! scheduler, and materializer contracts used by live projection dispatch. Tests
//! and future CLI/CI hooks can provide a local patch generator instead of making
//! paid provider calls.

use crate::projection_scheduler::{ProjectionSchedulerDecision, ProjectionSchedulers};
use crate::projections::{
    DiarizationSpanRevision, MaterializedProjectionState, ProjectionApplyError, ProjectionJob,
    ProjectionKind, ProjectionPatch, SpeakerTimeline, TranscriptEvent, TranscriptLedger,
    TranscriptLedgerError,
};

const TWO_SPAN_REPAIR_FIXTURE_JSON: &str =
    include_str!("../fixtures/projection_eval/two_span_repair.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OfflineProjectionReplayFixtureCatalogEntry {
    pub id: &'static str,
    pub description: &'static str,
    pub json: &'static str,
}

impl OfflineProjectionReplayFixtureCatalogEntry {
    pub fn fixture(&self) -> Result<OfflineProjectionReplayFixture, serde_json::Error> {
        serde_json::from_str(self.json)
    }
}

pub const OFFLINE_PROJECTION_REPLAY_FIXTURE_CATALOG:
    &[OfflineProjectionReplayFixtureCatalogEntry] = &[OfflineProjectionReplayFixtureCatalogEntry {
    id: "two_span_repair",
    description: "Two transcript spans coalesce while notes/graph jobs are in flight, forcing stale first completions and replay repair jobs with deterministic token and latency costs.",
    json: TWO_SPAN_REPAIR_FIXTURE_JSON,
}];

pub fn offline_projection_replay_fixture_catalog()
-> &'static [OfflineProjectionReplayFixtureCatalogEntry] {
    OFFLINE_PROJECTION_REPLAY_FIXTURE_CATALOG
}

// Serialized replay-step enum: boxing the large `Transcript` variant would
// ripple through every construction and match site for negligible benefit.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OfflineProjectionReplayStep {
    Transcript { event: TranscriptEvent },
    Diarization { revision: DiarizationSpanRevision },
    CompleteNotes { now_ms: u64 },
    CompleteGraph { now_ms: u64 },
    CompleteAll { now_ms: u64 },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct OfflineProjectionPatchOutcome {
    pub patch: ProjectionPatch,
    pub tokens_used: u32,
    pub generation_latency_ms: u64,
    pub apply_latency_ms: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq, Default)]
pub struct OfflineProjectionReplayMetrics {
    pub accepted_transcript_event_count: usize,
    pub rejected_transcript_event_count: usize,
    pub accepted_diarization_revision_count: usize,
    pub rejected_diarization_revision_count: usize,
    pub generated_patch_count: usize,
    pub applied_patch_count: usize,
    pub generation_failure_count: usize,
    pub apply_failure_count: usize,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct OfflineProjectionReplayReport {
    pub session_id: String,
    pub metrics: OfflineProjectionReplayMetrics,
    pub latency: OfflineProjectionLatencyBreakdown,
    pub schedulers: crate::projection_scheduler::ProjectionSchedulersTelemetry,
    pub materialized: MaterializedProjectionState,
    /// Provider-neutral speaker timeline replayed in parallel with the
    /// transcript ledger. Empty when the fixture has no diarization steps.
    pub speaker_timeline: SpeakerTimeline,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq, Default)]
pub struct OfflineProjectionLatencyBreakdown {
    pub completed_job_count: usize,
    pub accepted_job_count: usize,
    pub rejected_job_count: usize,
    pub max_asr_event_to_job_queued_ms: u64,
    pub max_projection_queue_lag_ms: u64,
    pub total_generation_latency_ms: u64,
    pub total_apply_latency_ms: u64,
    pub max_generation_latency_ms: u64,
    pub max_apply_latency_ms: u64,
    pub notes: OfflineProjectionKindLatencyBreakdown,
    pub graph: OfflineProjectionKindLatencyBreakdown,
    pub last_job: Option<OfflineProjectionJobLatency>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq, Default)]
pub struct OfflineProjectionKindLatencyBreakdown {
    pub completed_job_count: usize,
    pub accepted_job_count: usize,
    pub rejected_job_count: usize,
    pub tokens_used: u64,
    pub total_generation_latency_ms: u64,
    pub total_apply_latency_ms: u64,
    pub max_generation_latency_ms: u64,
    pub max_apply_latency_ms: u64,
    pub max_projection_queue_lag_ms: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct OfflineProjectionJobLatency {
    pub kind: ProjectionKind,
    pub job_id: String,
    pub accepted: bool,
    pub basis_span_count: usize,
    pub basis_latest_received_at_ms: Option<u64>,
    pub queued_at_ms: u64,
    pub completed_at_ms: u64,
    pub asr_event_to_job_queued_ms: u64,
    pub projection_queue_lag_ms: u64,
    pub generation_latency_ms: u64,
    pub apply_latency_ms: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct OfflineProjectionReplayFixture {
    pub session_id: String,
    pub steps: Vec<OfflineProjectionReplayStep>,
    pub generated_patches: Vec<OfflineProjectionFixturePatch>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct OfflineProjectionFixturePatch {
    pub kind: ProjectionKind,
    pub operations: Vec<crate::projections::ProjectionOperation>,
    #[serde(default = "default_fixture_confidence")]
    pub confidence: f32,
    #[serde(default)]
    pub tokens_used: u32,
    #[serde(default)]
    pub generation_latency_ms: u64,
    #[serde(default)]
    pub apply_latency_ms: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq, Default)]
pub struct OfflineProjectionFixtureCostSummary {
    pub patch_plan_count: usize,
    pub notes_patch_plan_count: usize,
    pub graph_patch_plan_count: usize,
    pub total_tokens_used: u64,
    pub notes_tokens_used: u64,
    pub graph_tokens_used: u64,
    pub total_generation_latency_ms: u64,
    pub total_apply_latency_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OfflineProjectionReplayError {
    Transcript {
        event_index: usize,
        error: TranscriptLedgerError,
    },
    Generate {
        job_id: String,
        kind: ProjectionKind,
        error: String,
    },
    Apply {
        job_id: String,
        kind: ProjectionKind,
        error: ProjectionApplyError,
    },
}

pub fn run_offline_projection_fixture(
    fixture: OfflineProjectionReplayFixture,
) -> OfflineProjectionReplayReport {
    let mut generated_patches = fixture.generated_patches.into_iter();
    run_offline_projection_replay(
        fixture.session_id,
        fixture.steps,
        move |job, _ledger, sequence, created_at_ms| {
            let Some(plan) = generated_patches.next() else {
                return Err(format!(
                    "fixture did not provide a generated {:?} patch for job {}",
                    job.kind, job.id
                ));
            };

            if plan.kind != job.kind {
                return Err(format!(
                    "fixture generated patch kind mismatch for job {}: expected {:?}, got {:?}",
                    job.id, job.kind, plan.kind
                ));
            }

            Ok(OfflineProjectionPatchOutcome {
                patch: ProjectionPatch {
                    sequence,
                    kind: job.kind.clone(),
                    llm_request_id: format!("offline-fixture:{}:{sequence}", job.id),
                    basis: job.basis.clone(),
                    operations: plan.operations,
                    confidence: plan.confidence,
                    provenance: default_fixture_provenance(),
                    queued_at_ms: None,
                    generation_latency_ms: None,
                    apply_latency_ms: None,
                    created_at_ms,
                },
                tokens_used: plan.tokens_used,
                generation_latency_ms: plan.generation_latency_ms,
                apply_latency_ms: plan.apply_latency_ms,
            })
        },
    )
}

pub fn summarize_offline_projection_fixture_costs(
    fixture: &OfflineProjectionReplayFixture,
) -> OfflineProjectionFixtureCostSummary {
    let mut summary = OfflineProjectionFixtureCostSummary::default();
    for plan in &fixture.generated_patches {
        let tokens_used = u64::from(plan.tokens_used);
        summary.patch_plan_count += 1;
        summary.total_tokens_used = summary.total_tokens_used.saturating_add(tokens_used);
        summary.total_generation_latency_ms = summary
            .total_generation_latency_ms
            .saturating_add(plan.generation_latency_ms);
        summary.total_apply_latency_ms = summary
            .total_apply_latency_ms
            .saturating_add(plan.apply_latency_ms);
        match plan.kind {
            ProjectionKind::Notes => {
                summary.notes_patch_plan_count += 1;
                summary.notes_tokens_used = summary.notes_tokens_used.saturating_add(tokens_used);
            }
            ProjectionKind::Graph => {
                summary.graph_patch_plan_count += 1;
                summary.graph_tokens_used = summary.graph_tokens_used.saturating_add(tokens_used);
            }
        }
    }
    summary
}

pub fn run_offline_projection_replay<G>(
    session_id: impl Into<String>,
    steps: impl IntoIterator<Item = OfflineProjectionReplayStep>,
    mut generator: G,
) -> OfflineProjectionReplayReport
where
    G: FnMut(
        &ProjectionJob,
        &TranscriptLedger,
        u64,
        u64,
    ) -> Result<OfflineProjectionPatchOutcome, String>,
{
    let session_id = session_id.into();
    let mut ledger = TranscriptLedger::new(&session_id);
    let mut speaker_timeline = SpeakerTimeline::new(&session_id);
    let mut schedulers = ProjectionSchedulers::new(&session_id);
    let mut materialized = MaterializedProjectionState::new(&session_id);
    let mut metrics = OfflineProjectionReplayMetrics::default();
    let mut latency = OfflineProjectionLatencyBreakdown::default();
    let mut note_sequence = 0;
    let mut graph_sequence = 0;

    for (event_index, step) in steps.into_iter().enumerate() {
        match step {
            OfflineProjectionReplayStep::Transcript { event } => {
                let now_ms = event.received_at_ms;
                match ledger.apply_event(event) {
                    Ok(()) => {
                        metrics.accepted_transcript_event_count += 1;
                        schedulers.observe_ledger(&ledger, now_ms);
                    }
                    Err(error) => {
                        metrics.rejected_transcript_event_count += 1;
                        log::warn!(
                            "Offline projection replay rejected transcript event index={event_index}: {error:?}"
                        );
                    }
                }
            }
            OfflineProjectionReplayStep::Diarization { revision } => {
                // Parallel to the transcript arm: speaker retcons revise the
                // durable speaker timeline (provisional -> stable supersedes,
                // stale/conflicting rejection) without touching the transcript
                // ledger or the projection schedulers.
                match speaker_timeline.apply_event(revision) {
                    Ok(_) => {
                        metrics.accepted_diarization_revision_count += 1;
                    }
                    Err(error) => {
                        metrics.rejected_diarization_revision_count += 1;
                        log::warn!(
                            "Offline projection replay rejected diarization revision index={event_index}: {error:?}"
                        );
                    }
                }
            }
            OfflineProjectionReplayStep::CompleteNotes { now_ms } => {
                complete_kind(
                    ProjectionKind::Notes,
                    now_ms,
                    &ledger,
                    &speaker_timeline,
                    &mut schedulers,
                    &mut materialized,
                    &mut note_sequence,
                    &mut metrics,
                    &mut latency,
                    &mut generator,
                );
            }
            OfflineProjectionReplayStep::CompleteGraph { now_ms } => {
                complete_kind(
                    ProjectionKind::Graph,
                    now_ms,
                    &ledger,
                    &speaker_timeline,
                    &mut schedulers,
                    &mut materialized,
                    &mut graph_sequence,
                    &mut metrics,
                    &mut latency,
                    &mut generator,
                );
            }
            OfflineProjectionReplayStep::CompleteAll { now_ms } => {
                complete_kind(
                    ProjectionKind::Notes,
                    now_ms,
                    &ledger,
                    &speaker_timeline,
                    &mut schedulers,
                    &mut materialized,
                    &mut note_sequence,
                    &mut metrics,
                    &mut latency,
                    &mut generator,
                );
                complete_kind(
                    ProjectionKind::Graph,
                    now_ms,
                    &ledger,
                    &speaker_timeline,
                    &mut schedulers,
                    &mut materialized,
                    &mut graph_sequence,
                    &mut metrics,
                    &mut latency,
                    &mut generator,
                );
            }
        }
    }

    OfflineProjectionReplayReport {
        session_id,
        metrics,
        latency,
        schedulers: schedulers.telemetry(),
        materialized,
        speaker_timeline,
    }
}

fn default_fixture_confidence() -> f32 {
    1.0
}

fn default_fixture_provenance() -> crate::projections::ProjectionProvenance {
    crate::projections::ProjectionProvenance {
        provider: "offline-fixture".to_string(),
        model: "deterministic".to_string(),
        prompt_id: "offline-projection-eval-v1".to_string(),
    }
}

#[allow(clippy::too_many_arguments)]
fn complete_kind<G>(
    kind: ProjectionKind,
    now_ms: u64,
    ledger: &TranscriptLedger,
    speaker_timeline: &SpeakerTimeline,
    schedulers: &mut ProjectionSchedulers,
    materialized: &mut MaterializedProjectionState,
    sequence: &mut u64,
    metrics: &mut OfflineProjectionReplayMetrics,
    latency: &mut OfflineProjectionLatencyBreakdown,
    generator: &mut G,
) where
    G: FnMut(
        &ProjectionJob,
        &TranscriptLedger,
        u64,
        u64,
    ) -> Result<OfflineProjectionPatchOutcome, String>,
{
    let job = match kind {
        ProjectionKind::Notes => schedulers.notes().in_flight_job().cloned(),
        ProjectionKind::Graph => schedulers.graph().in_flight_job().cloned(),
    };
    let Some(job) = job else {
        return;
    };

    *sequence = sequence.saturating_add(1);
    let generated = generator(&job, ledger, *sequence, now_ms);
    match generated {
        Ok(outcome) => {
            metrics.generated_patch_count += 1;
            schedulers.record_generation_result(
                &kind,
                outcome.generation_latency_ms,
                outcome.tokens_used,
                true,
            );
            let apply_result = materialized.apply_validated_patch_with_speaker_timeline(
                ledger,
                speaker_timeline,
                &outcome.patch,
            );
            match apply_result {
                Ok(_) => {
                    record_latency(
                        &job,
                        ledger,
                        now_ms,
                        outcome.generation_latency_ms,
                        outcome.apply_latency_ms,
                        outcome.tokens_used,
                        true,
                        latency,
                    );
                    metrics.applied_patch_count += 1;
                    schedulers.record_apply_result(&kind, outcome.apply_latency_ms, true);
                    complete_scheduler_kind(&kind, schedulers, ledger, now_ms);
                }
                Err(error) => {
                    record_latency(
                        &job,
                        ledger,
                        now_ms,
                        outcome.generation_latency_ms,
                        outcome.apply_latency_ms,
                        outcome.tokens_used,
                        false,
                        latency,
                    );
                    metrics.apply_failure_count += 1;
                    schedulers.record_apply_result(&kind, outcome.apply_latency_ms, false);
                    let stale_apply = matches!(error, ProjectionApplyError::StaleBasis { .. });
                    log::debug!(
                        "Offline projection replay apply failed job_id={} kind={kind:?} stale_apply={stale_apply}: {error:?}",
                        job.id
                    );
                    if stale_apply {
                        complete_scheduler_kind(&kind, schedulers, ledger, now_ms);
                    } else {
                        fail_scheduler_kind(&kind, schedulers, ledger, now_ms);
                    }
                }
            }
        }
        Err(error) => {
            record_latency(&job, ledger, now_ms, 0, 0, 0, false, latency);
            metrics.generation_failure_count += 1;
            schedulers.record_generation_result(&kind, 0, 0, false);
            log::debug!(
                "Offline projection replay generation failed job_id={} kind={kind:?}: {error}",
                job.id
            );
            fail_scheduler_kind(&kind, schedulers, ledger, now_ms);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn record_latency(
    job: &ProjectionJob,
    ledger: &TranscriptLedger,
    completed_at_ms: u64,
    generation_latency_ms: u64,
    apply_latency_ms: u64,
    tokens_used: u32,
    accepted: bool,
    latency: &mut OfflineProjectionLatencyBreakdown,
) {
    let basis_latest_received_at_ms = basis_latest_received_at_ms(job, ledger);
    let asr_event_to_job_queued_ms = basis_latest_received_at_ms
        .map(|received_at_ms| job.queued_at_ms.saturating_sub(received_at_ms))
        .unwrap_or(0);
    let projection_queue_lag_ms = completed_at_ms.saturating_sub(job.queued_at_ms);
    let job_latency = OfflineProjectionJobLatency {
        kind: job.kind.clone(),
        job_id: job.id.clone(),
        accepted,
        basis_span_count: job.basis.span_revisions.len(),
        basis_latest_received_at_ms,
        queued_at_ms: job.queued_at_ms,
        completed_at_ms,
        asr_event_to_job_queued_ms,
        projection_queue_lag_ms,
        generation_latency_ms,
        apply_latency_ms,
    };

    latency.completed_job_count += 1;
    if accepted {
        latency.accepted_job_count += 1;
    } else {
        latency.rejected_job_count += 1;
    }
    latency.max_asr_event_to_job_queued_ms = latency
        .max_asr_event_to_job_queued_ms
        .max(asr_event_to_job_queued_ms);
    latency.max_projection_queue_lag_ms = latency
        .max_projection_queue_lag_ms
        .max(projection_queue_lag_ms);
    latency.total_generation_latency_ms = latency
        .total_generation_latency_ms
        .saturating_add(generation_latency_ms);
    latency.total_apply_latency_ms = latency
        .total_apply_latency_ms
        .saturating_add(apply_latency_ms);
    latency.max_generation_latency_ms =
        latency.max_generation_latency_ms.max(generation_latency_ms);
    latency.max_apply_latency_ms = latency.max_apply_latency_ms.max(apply_latency_ms);
    record_kind_latency(
        &job.kind,
        projection_queue_lag_ms,
        generation_latency_ms,
        apply_latency_ms,
        tokens_used,
        accepted,
        latency,
    );
    latency.last_job = Some(job_latency);
}

fn record_kind_latency(
    kind: &ProjectionKind,
    projection_queue_lag_ms: u64,
    generation_latency_ms: u64,
    apply_latency_ms: u64,
    tokens_used: u32,
    accepted: bool,
    latency: &mut OfflineProjectionLatencyBreakdown,
) {
    let breakdown = match kind {
        ProjectionKind::Notes => &mut latency.notes,
        ProjectionKind::Graph => &mut latency.graph,
    };
    breakdown.completed_job_count += 1;
    if accepted {
        breakdown.accepted_job_count += 1;
    } else {
        breakdown.rejected_job_count += 1;
    }
    breakdown.tokens_used = breakdown.tokens_used.saturating_add(u64::from(tokens_used));
    breakdown.total_generation_latency_ms = breakdown
        .total_generation_latency_ms
        .saturating_add(generation_latency_ms);
    breakdown.total_apply_latency_ms = breakdown
        .total_apply_latency_ms
        .saturating_add(apply_latency_ms);
    breakdown.max_generation_latency_ms = breakdown
        .max_generation_latency_ms
        .max(generation_latency_ms);
    breakdown.max_apply_latency_ms = breakdown.max_apply_latency_ms.max(apply_latency_ms);
    breakdown.max_projection_queue_lag_ms = breakdown
        .max_projection_queue_lag_ms
        .max(projection_queue_lag_ms);
}

fn basis_latest_received_at_ms(job: &ProjectionJob, ledger: &TranscriptLedger) -> Option<u64> {
    job.basis
        .span_revisions
        .iter()
        .filter_map(|basis_span| {
            ledger
                .latest_spans
                .iter()
                .find(|event| {
                    event.span_id == basis_span.span_id
                        && event.revision_number == basis_span.revision_number
                })
                .map(|event| event.received_at_ms)
        })
        .max()
}

fn complete_scheduler_kind(
    kind: &ProjectionKind,
    schedulers: &mut ProjectionSchedulers,
    ledger: &TranscriptLedger,
    now_ms: u64,
) -> ProjectionSchedulerDecision {
    match kind {
        ProjectionKind::Notes => schedulers.complete_notes_in_flight(ledger, now_ms),
        ProjectionKind::Graph => schedulers.complete_graph_in_flight(ledger, now_ms),
    }
}

fn fail_scheduler_kind(
    kind: &ProjectionKind,
    schedulers: &mut ProjectionSchedulers,
    ledger: &TranscriptLedger,
    now_ms: u64,
) -> ProjectionSchedulerDecision {
    match kind {
        ProjectionKind::Notes => schedulers.fail_notes_in_flight(ledger, now_ms),
        ProjectionKind::Graph => schedulers.fail_graph_in_flight(ledger, now_ms),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{LlmExecutor, OpenRouterClient, OpenRouterConfig};
    use crate::projections::{
        DiarizationEventStability, ProjectionBasis, ProjectionBasisSpan, ProjectionOperation,
        ProjectionPriority, ProjectionProvenance, TranscriptEventStability,
    };
    use crate::settings::LlmProvider;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    const PROVIDER_PROJECTION_SMOKE_ENV: &str = "AUDIOGRAPH_PROVIDER_PROJECTION_SMOKE";
    const PROVIDER_PROJECTION_SMOKE_MODEL_ENV: &str =
        "AUDIOGRAPH_PROVIDER_PROJECTION_SMOKE_OPENROUTER_MODEL";
    const DEFAULT_PROVIDER_PROJECTION_SMOKE_MODEL: &str = "anthropic/claude-sonnet-4.5";

    fn event(
        span_id: &str,
        revision_number: u64,
        text: &str,
        received_at_ms: u64,
    ) -> TranscriptEvent {
        TranscriptEvent {
            span_id: span_id.to_string(),
            provider: "fixture".to_string(),
            source_id: "system".to_string(),
            provider_item_id: Some(span_id.to_string()),
            transcript_segment_id: Some(format!("segment-{span_id}")),
            speaker_id: Some("speaker-1".to_string()),
            speaker_label: Some("Speaker 1".to_string()),
            channel: None,
            text: text.to_string(),
            start_time: revision_number as f64,
            end_time: revision_number as f64 + 0.5,
            confidence: 0.9,
            is_final: true,
            stability: TranscriptEventStability::Final,
            revision_number,
            supersedes: None,
            turn_id: Some("turn-1".to_string()),
            end_of_turn: true,
            raw_event_ref: Some("offline.fixture".to_string()),
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms,
        }
    }

    #[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
    struct ProviderProjectionSmokeReport {
        provider: &'static str,
        model: String,
        generated_patch_count: usize,
        applied_patch_count: usize,
        materialized_note_count: usize,
        accepted_job_count: usize,
        rejected_job_count: usize,
        total_tokens_used: u64,
        max_asr_event_to_job_queued_ms: u64,
        max_projection_queue_lag_ms: u64,
        max_generation_latency_ms: u64,
        max_apply_latency_ms: u64,
    }

    fn truthy_env(value: Option<&str>) -> bool {
        matches!(
            value.map(str::trim).map(str::to_ascii_lowercase).as_deref(),
            Some("1" | "true" | "yes" | "on")
        )
    }

    fn non_empty_env(name: &str) -> Option<String> {
        std::env::var(name)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    }

    fn openrouter_smoke_key_from_env_or_store() -> Option<String> {
        non_empty_env("OPENROUTER_API_KEY").or_else(|| {
            crate::credentials::load_credentials()
                .openrouter_api_key
                .clone()
                .and_then(|key| {
                    let trimmed = key.trim().to_string();
                    if trimmed.is_empty() {
                        None
                    } else {
                        Some(trimmed)
                    }
                })
        })
    }

    fn provider_projection_smoke_config() -> Option<OpenRouterConfig> {
        if !truthy_env(std::env::var(PROVIDER_PROJECTION_SMOKE_ENV).ok().as_deref()) {
            eprintln!(
                "skipping provider-backed projection smoke: set {PROVIDER_PROJECTION_SMOKE_ENV}=1"
            );
            return None;
        }

        let Some(api_key) = openrouter_smoke_key_from_env_or_store() else {
            eprintln!(
                "skipping provider-backed projection smoke: missing OPENROUTER_API_KEY or saved openrouter_api_key"
            );
            return None;
        };

        let model = non_empty_env(PROVIDER_PROJECTION_SMOKE_MODEL_ENV)
            .unwrap_or_else(|| DEFAULT_PROVIDER_PROJECTION_SMOKE_MODEL.to_string());
        let mut config = OpenRouterConfig::with_defaults(api_key, model);
        config.max_tokens = 384;
        config.temperature = 0.0;
        Some(config)
    }

    fn run_openrouter_provider_projection_smoke(
        config: OpenRouterConfig,
        transcript_text: &str,
    ) -> Result<ProviderProjectionSmokeReport, String> {
        let model = config.model.clone();
        let provider = LlmProvider::OpenRouter {
            model: model.clone(),
            base_url: config.base_url.clone(),
            provider_order: config.provider_order.clone(),
            include_usage_in_stream: config.include_usage_in_stream,
            api_key: String::new(),
        };
        let executor = LlmExecutor::new(
            Arc::new(Mutex::new(None)),
            Arc::new(Mutex::new(None)),
            Arc::new(Mutex::new(Some(
                OpenRouterClient::new(config)
                    .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::allow()),
            ))),
            Arc::new(Mutex::new(None)),
        );

        let report = run_offline_projection_replay(
            "provider-projection-smoke",
            [
                OfflineProjectionReplayStep::Transcript {
                    event: event("smoke-span-1", 1, transcript_text, 1_000),
                },
                OfflineProjectionReplayStep::CompleteNotes { now_ms: 1_200 },
            ],
            |job, ledger, sequence, created_at_ms| {
                let started = Instant::now();
                let outcome = executor.generate_projection_patch(
                    job.clone(),
                    ledger.clone(),
                    provider.clone(),
                    sequence,
                    created_at_ms,
                )?;
                let generation_latency_ms =
                    started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);

                Ok(OfflineProjectionPatchOutcome {
                    patch: outcome.patch,
                    tokens_used: outcome.tokens_used,
                    generation_latency_ms,
                    apply_latency_ms: 1,
                })
            },
        );

        if report.metrics.generation_failure_count > 0 || report.metrics.apply_failure_count > 0 {
            return Err(format!(
                "provider projection smoke failed: generation_failures={} apply_failures={}",
                report.metrics.generation_failure_count, report.metrics.apply_failure_count
            ));
        }

        let materialized_note_count = report.materialized.notes.notes.len();
        if materialized_note_count == 0 {
            return Err("provider projection smoke produced no materialized notes".to_string());
        }

        Ok(ProviderProjectionSmokeReport {
            provider: "openrouter",
            model,
            generated_patch_count: report.metrics.generated_patch_count,
            applied_patch_count: report.metrics.applied_patch_count,
            materialized_note_count,
            accepted_job_count: report.latency.accepted_job_count,
            rejected_job_count: report.latency.rejected_job_count,
            total_tokens_used: report.latency.notes.tokens_used,
            max_asr_event_to_job_queued_ms: report.latency.max_asr_event_to_job_queued_ms,
            max_projection_queue_lag_ms: report.latency.max_projection_queue_lag_ms,
            max_generation_latency_ms: report.latency.max_generation_latency_ms,
            max_apply_latency_ms: report.latency.max_apply_latency_ms,
        })
    }

    fn patch_for_job(job: &ProjectionJob, sequence: u64, created_at_ms: u64) -> ProjectionPatch {
        let operations = match job.kind {
            ProjectionKind::Notes => vec![ProjectionOperation::UpsertNote {
                id: "note:summary".to_string(),
                title: "Summary".to_string(),
                body: format!("{} span(s)", job.basis.span_revisions.len()),
                tags: vec!["offline".to_string()],
            }],
            ProjectionKind::Graph => vec![ProjectionOperation::UpsertGraphNode {
                id: "topic:offline-replay".to_string(),
                name: "Offline replay".to_string(),
                entity_type: "topic".to_string(),
                description: Some(format!("{} span basis", job.basis.span_revisions.len())),
            }],
        };
        ProjectionPatch {
            sequence,
            kind: job.kind.clone(),
            llm_request_id: format!("offline:{}:{sequence}", job.id),
            basis: ProjectionBasis {
                span_revisions: job.basis.span_revisions.clone(),
                diarization_span_revisions: Vec::new(),
                transcript_hash: job.basis.transcript_hash.clone(),
                summarized_through_revision: None,
            },
            operations,
            confidence: 0.9,
            provenance: ProjectionProvenance {
                provider: "offline-fixture".to_string(),
                model: "deterministic".to_string(),
                prompt_id: "offline-projection-eval-v1".to_string(),
            },
            queued_at_ms: None,
            generation_latency_ms: None,
            apply_latency_ms: None,
            created_at_ms,
        }
    }

    #[test]
    fn provider_backed_projection_smoke_is_env_gated_and_sanitized() {
        let Some(config) = provider_projection_smoke_config() else {
            return;
        };
        let submitted_key = config.api_key.clone();
        let transcript_text =
            "Decision: build the provider projection smoke. Action: verify timing telemetry.";

        let report = run_openrouter_provider_projection_smoke(config, transcript_text)
            .expect("provider-backed projection smoke should produce a notes patch");

        assert_eq!(report.provider, "openrouter");
        assert_eq!(report.generated_patch_count, 1);
        assert_eq!(report.applied_patch_count, 1);
        assert_eq!(report.accepted_job_count, 1);
        assert_eq!(report.rejected_job_count, 0);
        assert!(report.materialized_note_count >= 1);
        assert!(report.max_asr_event_to_job_queued_ms > 0);
        assert!(report.max_projection_queue_lag_ms > 0);
        assert!(report.max_generation_latency_ms > 0);
        assert!(report.max_apply_latency_ms > 0);

        let serialized =
            serde_json::to_string(&report).expect("sanitized smoke report serializes to JSON");
        assert!(
            !serialized.contains(&submitted_key),
            "sanitized smoke report must not contain the submitted OpenRouter key"
        );
        assert!(
            !serialized.contains(transcript_text),
            "sanitized smoke report must not contain transcript text"
        );
        assert!(
            !serialized.contains("Decision:"),
            "sanitized smoke report must not contain prompt/transcript fragments"
        );
        assert!(
            !serialized.contains("verify timing telemetry"),
            "sanitized smoke report must not contain note bodies"
        );
    }

    #[test]
    fn offline_replay_runner_exercises_scheduler_coalescing_and_stale_repair() {
        let span_one = event("span-1", 1, "first span", 1_000);
        let span_two = event("span-2", 1, "second span", 1_100);
        let mut calls = Vec::new();

        let report = run_offline_projection_replay(
            "session-offline",
            [
                OfflineProjectionReplayStep::Transcript { event: span_one },
                OfflineProjectionReplayStep::Transcript { event: span_two },
                OfflineProjectionReplayStep::CompleteAll { now_ms: 1_200 },
                OfflineProjectionReplayStep::CompleteAll { now_ms: 1_400 },
            ],
            |job, _ledger, sequence, created_at_ms| {
                calls.push((
                    job.kind.clone(),
                    job.priority.clone(),
                    job.basis.span_revisions.len(),
                ));
                Ok(OfflineProjectionPatchOutcome {
                    patch: patch_for_job(job, sequence, created_at_ms),
                    tokens_used: match job.kind {
                        ProjectionKind::Notes => 11,
                        ProjectionKind::Graph => 17,
                    },
                    generation_latency_ms: match job.kind {
                        ProjectionKind::Notes => 30,
                        ProjectionKind::Graph => 45,
                    },
                    apply_latency_ms: 5,
                })
            },
        );

        assert_eq!(report.metrics.accepted_transcript_event_count, 2);
        assert_eq!(report.metrics.rejected_transcript_event_count, 0);
        assert_eq!(report.metrics.generated_patch_count, 4);
        assert_eq!(report.metrics.applied_patch_count, 2);
        assert_eq!(report.metrics.apply_failure_count, 2);

        assert_eq!(report.latency.completed_job_count, 4);
        assert_eq!(report.latency.accepted_job_count, 2);
        assert_eq!(report.latency.rejected_job_count, 2);
        assert_eq!(report.latency.max_asr_event_to_job_queued_ms, 100);
        assert_eq!(report.latency.max_projection_queue_lag_ms, 200);
        assert_eq!(report.latency.total_generation_latency_ms, 150);
        assert_eq!(report.latency.total_apply_latency_ms, 20);
        assert_eq!(report.latency.notes.completed_job_count, 2);
        assert_eq!(report.latency.notes.accepted_job_count, 1);
        assert_eq!(report.latency.notes.rejected_job_count, 1);
        assert_eq!(report.latency.notes.tokens_used, 22);
        assert_eq!(report.latency.notes.total_generation_latency_ms, 60);
        assert_eq!(report.latency.notes.total_apply_latency_ms, 10);
        assert_eq!(report.latency.notes.max_projection_queue_lag_ms, 200);
        assert_eq!(report.latency.graph.completed_job_count, 2);
        assert_eq!(report.latency.graph.accepted_job_count, 1);
        assert_eq!(report.latency.graph.rejected_job_count, 1);
        assert_eq!(report.latency.graph.tokens_used, 34);
        assert_eq!(report.latency.graph.total_generation_latency_ms, 90);
        assert_eq!(report.latency.graph.total_apply_latency_ms, 10);
        assert_eq!(report.latency.graph.max_projection_queue_lag_ms, 200);
        assert_eq!(
            report.latency.last_job,
            Some(OfflineProjectionJobLatency {
                kind: ProjectionKind::Graph,
                job_id: "projection:session-offline:graph:2".to_string(),
                accepted: true,
                basis_span_count: 2,
                basis_latest_received_at_ms: Some(1_100),
                queued_at_ms: 1_200,
                completed_at_ms: 1_400,
                asr_event_to_job_queued_ms: 100,
                projection_queue_lag_ms: 200,
                generation_latency_ms: 45,
                apply_latency_ms: 5,
            })
        );

        assert_eq!(report.materialized.notes.notes.len(), 1);
        assert_eq!(report.materialized.notes.notes[0].body, "2 span(s)");
        assert_eq!(report.materialized.graph.nodes.len(), 1);
        assert_eq!(
            report.materialized.graph.nodes[0].description.as_deref(),
            Some("2 span basis")
        );

        assert_eq!(report.schedulers.notes.metrics.jobs_started, 2);
        assert_eq!(report.schedulers.graph.metrics.jobs_started, 2);
        assert_eq!(report.schedulers.notes.metrics.coalesced_updates, 1);
        assert_eq!(report.schedulers.graph.metrics.coalesced_updates, 1);
        assert_eq!(report.schedulers.notes.metrics.stale_discards, 1);
        assert_eq!(report.schedulers.graph.metrics.stale_discards, 1);
        assert_eq!(report.schedulers.notes.metrics.repair_jobs_started, 1);
        assert_eq!(report.schedulers.graph.metrics.repair_jobs_started, 1);
        assert_eq!(report.schedulers.notes.metrics.accepted_patches, 1);
        assert_eq!(report.schedulers.graph.metrics.accepted_patches, 1);
        assert_eq!(report.schedulers.notes.metrics.apply_failures, 1);
        assert_eq!(report.schedulers.graph.metrics.apply_failures, 1);
        assert_eq!(report.schedulers.notes.metrics.tokens_used, 22);
        assert_eq!(report.schedulers.graph.metrics.tokens_used, 34);
        assert_eq!(
            report.schedulers.notes.metrics.max_generation_latency_ms,
            30
        );
        assert_eq!(
            report.schedulers.graph.metrics.max_generation_latency_ms,
            45
        );
        assert!(report.schedulers.notes.in_flight_job_id.is_none());
        assert!(report.schedulers.graph.in_flight_job_id.is_none());

        assert_eq!(
            calls,
            vec![
                (ProjectionKind::Notes, ProjectionPriority::Realtime, 1),
                (ProjectionKind::Graph, ProjectionPriority::Realtime, 1),
                (ProjectionKind::Notes, ProjectionPriority::Replay, 2),
                (ProjectionKind::Graph, ProjectionPriority::Replay, 2),
            ]
        );
    }

    #[test]
    fn serializable_fixture_runner_deserializes_and_stamps_runtime_patch_metadata() {
        let fixture: OfflineProjectionReplayFixture = serde_json::from_str(
            r#"{
                "session_id": "session-fixture",
                "steps": [
                    {
                        "type": "transcript",
                        "event": {
                            "span_id": "span-1",
                            "provider": "fixture",
                            "source_id": "system",
                            "text": "first span",
                            "start_time": 0.0,
                            "end_time": 0.5,
                            "confidence": 0.91,
                            "is_final": true,
                            "stability": "final",
                            "revision_number": 1,
                            "end_of_turn": true,
                            "received_at_ms": 1000
                        }
                    },
                    {
                        "type": "transcript",
                        "event": {
                            "span_id": "span-2",
                            "provider": "fixture",
                            "source_id": "system",
                            "text": "second span",
                            "start_time": 0.5,
                            "end_time": 1.0,
                            "confidence": 0.92,
                            "is_final": true,
                            "stability": "final",
                            "revision_number": 1,
                            "end_of_turn": true,
                            "received_at_ms": 1100
                        }
                    },
                    { "type": "complete_all", "now_ms": 1200 },
                    { "type": "complete_all", "now_ms": 1400 }
                ],
                "generated_patches": [
                    {
                        "kind": "notes",
                        "tokens_used": 10,
                        "generation_latency_ms": 31,
                        "apply_latency_ms": 4,
                        "operations": [
                            {
                                "type": "upsert_note",
                                "id": "note:summary",
                                "title": "Summary",
                                "body": "first basis should be stale",
                                "tags": ["fixture"]
                            }
                        ]
                    },
                    {
                        "kind": "graph",
                        "tokens_used": 11,
                        "generation_latency_ms": 41,
                        "apply_latency_ms": 5,
                        "operations": [
                            {
                                "type": "upsert_graph_node",
                                "id": "topic:fixture",
                                "name": "Fixture",
                                "entity_type": "topic",
                                "description": "first basis should be stale"
                            }
                        ]
                    },
                    {
                        "kind": "notes",
                        "tokens_used": 20,
                        "generation_latency_ms": 32,
                        "apply_latency_ms": 6,
                        "operations": [
                            {
                                "type": "upsert_note",
                                "id": "note:summary",
                                "title": "Summary",
                                "body": "two-span fixture note",
                                "tags": ["fixture", "accepted"]
                            }
                        ]
                    },
                    {
                        "kind": "graph",
                        "tokens_used": 21,
                        "generation_latency_ms": 42,
                        "apply_latency_ms": 7,
                        "operations": [
                            {
                                "type": "upsert_graph_node",
                                "id": "topic:fixture",
                                "name": "Fixture",
                                "entity_type": "topic",
                                "description": "two-span fixture graph"
                            }
                        ]
                    }
                ]
            }"#,
        )
        .expect("fixture JSON should deserialize");

        assert_eq!(fixture.generated_patches[0].confidence, 1.0);

        let report = run_offline_projection_fixture(fixture);

        assert_eq!(report.session_id, "session-fixture");
        assert_eq!(report.metrics.accepted_transcript_event_count, 2);
        assert_eq!(report.metrics.generated_patch_count, 4);
        assert_eq!(report.metrics.applied_patch_count, 2);
        assert_eq!(report.metrics.apply_failure_count, 2);

        assert_eq!(report.latency.completed_job_count, 4);
        assert_eq!(report.latency.accepted_job_count, 2);
        assert_eq!(report.latency.rejected_job_count, 2);
        assert_eq!(report.latency.max_asr_event_to_job_queued_ms, 100);
        assert_eq!(report.latency.max_projection_queue_lag_ms, 200);
        assert_eq!(report.latency.total_generation_latency_ms, 146);
        assert_eq!(report.latency.total_apply_latency_ms, 22);
        assert_eq!(report.latency.max_generation_latency_ms, 42);
        assert_eq!(report.latency.max_apply_latency_ms, 7);
        assert_eq!(report.latency.notes.tokens_used, 30);
        assert_eq!(report.latency.notes.total_generation_latency_ms, 63);
        assert_eq!(report.latency.notes.total_apply_latency_ms, 10);
        assert_eq!(report.latency.graph.tokens_used, 32);
        assert_eq!(report.latency.graph.total_generation_latency_ms, 83);
        assert_eq!(report.latency.graph.total_apply_latency_ms, 12);

        assert_eq!(report.schedulers.notes.metrics.tokens_used, 30);
        assert_eq!(report.schedulers.graph.metrics.tokens_used, 32);
        assert_eq!(report.schedulers.notes.metrics.repair_jobs_started, 1);
        assert_eq!(report.schedulers.graph.metrics.repair_jobs_started, 1);
        assert!(report.schedulers.notes.in_flight_job_id.is_none());
        assert!(report.schedulers.graph.in_flight_job_id.is_none());

        let note = &report.materialized.notes.notes[0];
        assert_eq!(note.body, "two-span fixture note");
        assert_eq!(note.updated_by_sequence, 2);
        assert_eq!(note.updated_at_ms, 1_400);
        assert_eq!(note.basis.span_revisions.len(), 2);
        assert_eq!(note.provenance.provider, "offline-fixture");
        assert_eq!(note.provenance.model, "deterministic");

        let graph_node = &report.materialized.graph.nodes[0];
        assert_eq!(
            graph_node.description.as_deref(),
            Some("two-span fixture graph")
        );
        assert_eq!(graph_node.updated_by_sequence, 2);
        assert_eq!(graph_node.updated_at_ms, 1_400);
        assert_eq!(graph_node.basis.span_revisions.len(), 2);
        assert_eq!(
            graph_node.provenance.prompt_id,
            "offline-projection-eval-v1"
        );
        assert_eq!(graph_node.confidence, 1.0);
    }

    #[test]
    fn fixture_catalog_deserializes_summarizes_costs_and_runs_replay() {
        let catalog = offline_projection_replay_fixture_catalog();
        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog[0].id, "two_span_repair");

        let fixture = catalog[0].fixture().expect("catalog fixture should parse");
        let cost = summarize_offline_projection_fixture_costs(&fixture);
        assert_eq!(
            cost,
            OfflineProjectionFixtureCostSummary {
                patch_plan_count: 4,
                notes_patch_plan_count: 2,
                graph_patch_plan_count: 2,
                total_tokens_used: 62,
                notes_tokens_used: 30,
                graph_tokens_used: 32,
                total_generation_latency_ms: 146,
                total_apply_latency_ms: 22,
            }
        );

        let report = run_offline_projection_fixture(fixture);

        assert_eq!(report.metrics.generated_patch_count, 4);
        assert_eq!(report.metrics.applied_patch_count, 2);
        assert_eq!(report.metrics.apply_failure_count, 2);
        assert_eq!(report.latency.total_generation_latency_ms, 146);
        assert_eq!(report.latency.total_apply_latency_ms, 22);
        assert_eq!(report.latency.notes.tokens_used, 30);
        assert_eq!(report.latency.notes.total_generation_latency_ms, 63);
        assert_eq!(report.latency.notes.total_apply_latency_ms, 10);
        assert_eq!(report.latency.graph.tokens_used, 32);
        assert_eq!(report.latency.graph.total_generation_latency_ms, 83);
        assert_eq!(report.latency.graph.total_apply_latency_ms, 12);
        assert_eq!(report.schedulers.notes.metrics.tokens_used, 30);
        assert_eq!(report.schedulers.graph.metrics.tokens_used, 32);
        assert_eq!(
            report.materialized.notes.notes[0].body,
            "two-span catalog note"
        );
        assert_eq!(
            report.materialized.graph.nodes[0].description.as_deref(),
            Some("two-span catalog graph")
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn diarization_revision(
        span_id: &str,
        revision_number: u64,
        speaker_id: &str,
        start_time: f64,
        end_time: f64,
        stability: DiarizationEventStability,
        source_id: Option<&str>,
        channel: Option<&str>,
        received_at_ms: u64,
    ) -> DiarizationSpanRevision {
        DiarizationSpanRevision {
            span_id: span_id.to_string(),
            provider: "deepgram".to_string(),
            timeline_id: source_id
                .map(str::to_string)
                .unwrap_or_else(|| "session".to_string()),
            source_id: source_id.map(str::to_string),
            speaker_id: Some(speaker_id.to_string()),
            speaker_label: Some(format!("Speaker {speaker_id}")),
            provider_speaker_id: None,
            channel: channel.map(str::to_string),
            start_time,
            end_time,
            confidence: Some(0.85),
            is_final: matches!(stability, DiarizationEventStability::Final),
            stability,
            revision_number,
            supersedes: (revision_number > 1)
                .then(|| format!("{span_id}@rev{}", revision_number - 1)),
            basis_asr_span_ids: vec![format!("{span_id}-asr")],
            basis_transcript_segment_ids: Vec::new(),
            raw_event_ref: Some("deepgram.diarization".to_string()),
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms,
        }
    }

    /// Speaker remaps revise existing spans in the offline harness in parallel
    /// with the transcript ledger: provisional -> stable supersedes collapse by
    /// span id, stale and conflicting-same-revision revisions are rejected,
    /// overlapping mono spans both survive, the speaker count grows, and channel
    /// labels only ride along when source/channel provenance is present.
    #[test]
    fn offline_replay_revises_speaker_timeline_in_parallel_with_transcript() {
        let report = run_offline_projection_replay(
            "session-diarization",
            [
                // span-1: provisional mono attribution, no source/channel.
                OfflineProjectionReplayStep::Diarization {
                    revision: diarization_revision(
                        "span-1",
                        1,
                        "spk-a",
                        0.0,
                        2.0,
                        DiarizationEventStability::Provisional,
                        None,
                        None,
                        1_000,
                    ),
                },
                // span-1 remapped to a stable, different speaker (supersede).
                OfflineProjectionReplayStep::Diarization {
                    revision: diarization_revision(
                        "span-1",
                        2,
                        "spk-b",
                        0.0,
                        2.0,
                        DiarizationEventStability::Stable,
                        None,
                        None,
                        1_100,
                    ),
                },
                // Stale revision for span-1 (rev 1 < current rev 2) is rejected.
                OfflineProjectionReplayStep::Diarization {
                    revision: diarization_revision(
                        "span-1",
                        1,
                        "spk-stale",
                        0.0,
                        2.0,
                        DiarizationEventStability::Provisional,
                        None,
                        None,
                        1_150,
                    ),
                },
                // span-2: overlapping mono speech (overlaps span-1's window) with
                // source + channel provenance; channel label rides along.
                OfflineProjectionReplayStep::Diarization {
                    revision: diarization_revision(
                        "span-2",
                        1,
                        "spk-c",
                        1.0,
                        3.0,
                        DiarizationEventStability::Stable,
                        Some("mic-1"),
                        Some("left"),
                        1_200,
                    ),
                },
                // Conflicting same-revision for span-2 (rev 1, different speaker)
                // is rejected.
                OfflineProjectionReplayStep::Diarization {
                    revision: diarization_revision(
                        "span-2",
                        1,
                        "spk-conflict",
                        1.0,
                        3.0,
                        DiarizationEventStability::Stable,
                        Some("mic-1"),
                        Some("left"),
                        1_250,
                    ),
                },
                // A transcript event still flows through the (separate) ledger.
                OfflineProjectionReplayStep::Transcript {
                    event: event("t-span-1", 1, "hello there", 1_300),
                },
                OfflineProjectionReplayStep::CompleteNotes { now_ms: 1_400 },
            ],
            |job, _ledger, sequence, created_at_ms| {
                Ok(OfflineProjectionPatchOutcome {
                    patch: patch_for_job(job, sequence, created_at_ms),
                    tokens_used: 5,
                    generation_latency_ms: 10,
                    apply_latency_ms: 2,
                })
            },
        );

        assert_eq!(report.metrics.accepted_diarization_revision_count, 3);
        assert_eq!(report.metrics.rejected_diarization_revision_count, 2);
        // Transcript ledger is untouched by diarization rejections.
        assert_eq!(report.metrics.accepted_transcript_event_count, 1);
        assert_eq!(report.metrics.rejected_transcript_event_count, 0);

        let timeline = &report.speaker_timeline;
        assert_eq!(
            timeline.latest_spans.len(),
            2,
            "overlapping mono spans coexist"
        );

        let span_one = timeline
            .latest_spans
            .iter()
            .find(|span| span.span_id == "span-1")
            .expect("span-1 survives supersede");
        assert_eq!(span_one.revision_number, 2);
        assert_eq!(span_one.speaker_id.as_deref(), Some("spk-b"));
        assert_eq!(
            span_one.stability,
            DiarizationEventStability::Stable,
            "provisional was superseded by the stable remap"
        );
        // No source/channel provenance: the channel label is absent.
        assert_eq!(span_one.source_id, None);
        assert_eq!(span_one.channel, None);

        let span_two = timeline
            .latest_spans
            .iter()
            .find(|span| span.span_id == "span-2")
            .expect("span-2 present");
        assert_eq!(span_two.speaker_id.as_deref(), Some("spk-c"));
        // Source/channel provenance present: the channel label rides along.
        assert_eq!(span_two.source_id.as_deref(), Some("mic-1"));
        assert_eq!(span_two.channel.as_deref(), Some("left"));
        // The conflicting same-revision payload did not overwrite spk-c.
        assert_ne!(span_two.speaker_id.as_deref(), Some("spk-conflict"));

        // Speaker-count growth: span-1's spk-a was remapped to spk-b, span-2 is
        // spk-c, so the timeline ends with two distinct resolved speakers.
        assert_eq!(timeline.speaker_count(), 2);

        // The patch still applied against the transcript ledger + empty
        // diarization basis (the offline patch generator carries no diarization
        // spans), proving the diarization arm runs alongside projection apply.
        assert_eq!(report.metrics.applied_patch_count, 1);
        assert_eq!(report.materialized.notes.notes.len(), 1);
    }

    /// A projection patch whose basis cites speaker-timeline spans is accepted
    /// only while those revisions are current, and rejected once a remap makes
    /// them stale — mirroring the transcript stale-basis path.
    #[test]
    fn offline_replay_rejects_patch_with_stale_diarization_basis() {
        let mut timeline = SpeakerTimeline::new("session-diar-basis");
        timeline
            .apply_event(diarization_revision(
                "d-span-1",
                1,
                "spk-a",
                0.0,
                2.0,
                DiarizationEventStability::Provisional,
                None,
                None,
                1_000,
            ))
            .expect("provisional diarization span");

        let transcript = event("t-span-1", 1, "hello", 1_000);
        let ledger =
            TranscriptLedger::replay("session-diar-basis", [transcript.clone()]).expect("ledger");
        let mut materialized = MaterializedProjectionState::new("session-diar-basis");

        let basis = ProjectionBasis::from_transcript_events_and_speaker_spans(
            std::slice::from_ref(&transcript),
            &[ProjectionBasisSpan {
                span_id: "d-span-1".to_string(),
                revision_number: 1,
            }],
        );
        let patch = ProjectionPatch {
            sequence: 1,
            kind: ProjectionKind::Notes,
            llm_request_id: "diar-basis-1".to_string(),
            basis,
            operations: vec![ProjectionOperation::UpsertNote {
                id: "note:diar".to_string(),
                title: "Speaker note".to_string(),
                body: "cites the provisional speaker span".to_string(),
                tags: Vec::new(),
            }],
            confidence: 0.9,
            provenance: default_fixture_provenance(),
            queued_at_ms: None,
            generation_latency_ms: None,
            apply_latency_ms: None,
            created_at_ms: 1_100,
        };

        // While the provisional rev is current, the patch applies.
        assert!(
            materialized
                .apply_validated_patch_with_speaker_timeline(&ledger, &timeline, &patch)
                .is_ok()
        );

        // Remap the span (provisional -> stable, rev 2). The earlier basis is
        // now stale.
        timeline
            .apply_event(diarization_revision(
                "d-span-1",
                2,
                "spk-b",
                0.0,
                2.0,
                DiarizationEventStability::Stable,
                None,
                None,
                1_200,
            ))
            .expect("stable remap");

        let mut stale_state = MaterializedProjectionState::new("session-diar-basis");
        let result =
            stale_state.apply_validated_patch_with_speaker_timeline(&ledger, &timeline, &patch);
        assert!(matches!(
            result,
            Err(ProjectionApplyError::StaleBasis {
                staleness: crate::projections::ProjectionBasisStaleness::StaleDiarizationSpanRevision {
                    ref span_id,
                    current_revision: 2,
                    basis_revision: 1,
                },
            }) if span_id == "d-span-1"
        ));
        assert!(stale_state.notes.notes.is_empty());
    }

    #[test]
    fn offline_diarization_step_round_trips_through_fixture_json() {
        let fixture: OfflineProjectionReplayFixture = serde_json::from_str(
            r#"{
                "session_id": "session-diar-json",
                "steps": [
                    {
                        "type": "diarization",
                        "revision": {
                            "span_id": "span-1",
                            "provider": "deepgram",
                            "timeline_id": "session",
                            "speaker_id": "spk-a",
                            "start_time": 0.0,
                            "end_time": 2.0,
                            "is_final": false,
                            "stability": "provisional",
                            "revision_number": 1,
                            "basis_asr_span_ids": ["span-1-asr"],
                            "basis_transcript_segment_ids": [],
                            "received_at_ms": 1000
                        }
                    },
                    {
                        "type": "diarization",
                        "revision": {
                            "span_id": "span-1",
                            "provider": "deepgram",
                            "timeline_id": "session",
                            "speaker_id": "spk-b",
                            "start_time": 0.0,
                            "end_time": 2.0,
                            "is_final": false,
                            "stability": "stable",
                            "revision_number": 2,
                            "basis_asr_span_ids": ["span-1-asr"],
                            "basis_transcript_segment_ids": [],
                            "received_at_ms": 1100
                        }
                    }
                ],
                "generated_patches": []
            }"#,
        )
        .expect("diarization fixture JSON should deserialize");

        let report = run_offline_projection_fixture(fixture);
        assert_eq!(report.metrics.accepted_diarization_revision_count, 2);
        assert_eq!(report.speaker_timeline.latest_spans.len(), 1);
        assert_eq!(
            report.speaker_timeline.latest_spans[0]
                .speaker_id
                .as_deref(),
            Some("spk-b")
        );
    }
}
