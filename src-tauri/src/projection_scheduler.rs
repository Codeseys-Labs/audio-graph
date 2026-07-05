//! TTFT-aware projection scheduling primitives.
//!
//! This module intentionally stops before provider I/O. It owns the deterministic
//! queue semantics the runtime will need: start a basis-bound job when the
//! transcript ledger changes, coalesce newer ledger state while an LLM call is
//! in flight, and reject stale completions before notes/graph materializers see
//! them.

use crate::projections::{
    ProjectionBasis, ProjectionBasisStaleness, ProjectionJob, ProjectionKind, ProjectionPriority,
    TranscriptLedger,
};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ProjectionSchedulerConfig {
    /// Current first-token latency estimate for the selected LLM/model.
    pub ttft_estimate_ms: u64,
    /// Coalescing pressure threshold based on the current pending basis size.
    pub coalesce_span_threshold: usize,
}

impl Default for ProjectionSchedulerConfig {
    fn default() -> Self {
        Self {
            ttft_estimate_ms: 1_200,
            coalesce_span_threshold: 2,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionTtftEstimateSource {
    Default,
    Configured,
    ObservedGeneration,
}

#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ProjectionSchedulerMetrics {
    pub jobs_started: u64,
    pub completed_jobs: u64,
    pub failed_jobs: u64,
    pub generation_failures: u64,
    pub coalesced_updates: u64,
    pub coalesced_span_count: u64,
    pub stale_discards: u64,
    pub repair_jobs_started: u64,
    pub follow_up_jobs_started: u64,
    pub accepted_patches: u64,
    pub apply_failures: u64,
    pub tokens_used: u64,
    pub last_job_lag_ms: u64,
    pub max_job_lag_ms: u64,
    pub last_generation_latency_ms: u64,
    pub max_generation_latency_ms: u64,
    pub last_apply_latency_ms: u64,
    pub max_apply_latency_ms: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ProjectionSchedulerTelemetry {
    pub kind: ProjectionKind,
    pub ttft_estimate_ms: u64,
    pub ttft_estimate_source: ProjectionTtftEstimateSource,
    pub in_flight_job_id: Option<String>,
    pub in_flight_age_ms: u64,
    pub in_flight_span_count: usize,
    pub pending_span_count: usize,
    pub metrics: ProjectionSchedulerMetrics,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ProjectionSchedulersTelemetry {
    pub notes: ProjectionSchedulerTelemetry,
    pub graph: ProjectionSchedulerTelemetry,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct ProjectionSchedulersObservation {
    pub notes: ProjectionSchedulerDecision,
    pub graph: ProjectionSchedulerDecision,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionCoalescingReason {
    PendingSpanThreshold,
    InFlightAgeThreshold,
    TtftWindow,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProjectionSchedulerDecision {
    Idle,
    StartJob {
        job: ProjectionJob,
    },
    Coalesced {
        in_flight_job_id: String,
        queued_span_count: usize,
        coalesced_span_delta: usize,
        ttft_estimate_ms: u64,
        in_flight_age_ms: u64,
        reason: ProjectionCoalescingReason,
    },
    CompletedCurrent {
        completed_job_id: String,
    },
    CompletedAndStartedFollowUp {
        completed_job_id: String,
        job: ProjectionJob,
    },
    DiscardedStaleAndStartedRepair {
        discarded_job_id: String,
        staleness: ProjectionBasisStaleness,
        job: ProjectionJob,
    },
    DiscardedStaleNoCurrentBasis {
        discarded_job_id: String,
        staleness: ProjectionBasisStaleness,
    },
    FailedCurrent {
        failed_job_id: String,
    },
    FailedStaleAndStartedRepair {
        failed_job_id: String,
        staleness: ProjectionBasisStaleness,
        job: ProjectionJob,
    },
    FailedStaleNoCurrentBasis {
        failed_job_id: String,
        staleness: ProjectionBasisStaleness,
    },
}

#[derive(Debug, Clone)]
pub struct ProjectionScheduler {
    session_id: String,
    kind: ProjectionKind,
    config: ProjectionSchedulerConfig,
    ttft_estimate_source: ProjectionTtftEstimateSource,
    next_job_index: u64,
    in_flight: Option<ProjectionJob>,
    pending_basis: Option<ProjectionBasis>,
    last_completed_basis: Option<ProjectionBasis>,
    last_failed_basis: Option<ProjectionBasis>,
    metrics: ProjectionSchedulerMetrics,
}

impl ProjectionScheduler {
    pub fn new(session_id: impl Into<String>, kind: ProjectionKind) -> Self {
        Self::with_config_and_source(
            session_id,
            kind,
            ProjectionSchedulerConfig::default(),
            ProjectionTtftEstimateSource::Default,
        )
    }

    pub fn with_config(
        session_id: impl Into<String>,
        kind: ProjectionKind,
        config: ProjectionSchedulerConfig,
    ) -> Self {
        Self::with_config_and_source(
            session_id,
            kind,
            config,
            ProjectionTtftEstimateSource::Configured,
        )
    }

    fn with_config_and_source(
        session_id: impl Into<String>,
        kind: ProjectionKind,
        config: ProjectionSchedulerConfig,
        ttft_estimate_source: ProjectionTtftEstimateSource,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            kind,
            config,
            ttft_estimate_source,
            next_job_index: 0,
            in_flight: None,
            pending_basis: None,
            last_completed_basis: None,
            last_failed_basis: None,
            metrics: ProjectionSchedulerMetrics::default(),
        }
    }

    pub fn in_flight_job(&self) -> Option<&ProjectionJob> {
        self.in_flight.as_ref()
    }

    pub fn metrics(&self) -> &ProjectionSchedulerMetrics {
        &self.metrics
    }

    pub fn record_generation_result(&mut self, latency_ms: u64, tokens_used: u32, success: bool) {
        self.metrics.last_generation_latency_ms = latency_ms;
        self.metrics.max_generation_latency_ms =
            self.metrics.max_generation_latency_ms.max(latency_ms);
        if success && latency_ms > 0 {
            self.config.ttft_estimate_ms = latency_ms;
            self.ttft_estimate_source = ProjectionTtftEstimateSource::ObservedGeneration;
        }
        self.metrics.tokens_used = self
            .metrics
            .tokens_used
            .saturating_add(u64::from(tokens_used));
        if !success {
            self.metrics.generation_failures = self.metrics.generation_failures.saturating_add(1);
        }
    }

    pub fn set_configured_ttft_estimate(&mut self, estimate_ms: u64) {
        if estimate_ms == 0 {
            return;
        }
        self.config.ttft_estimate_ms = estimate_ms;
        self.ttft_estimate_source = ProjectionTtftEstimateSource::Configured;
    }

    pub fn record_apply_result(&mut self, latency_ms: u64, accepted: bool) {
        self.metrics.last_apply_latency_ms = latency_ms;
        self.metrics.max_apply_latency_ms = self.metrics.max_apply_latency_ms.max(latency_ms);
        if accepted {
            self.metrics.accepted_patches = self.metrics.accepted_patches.saturating_add(1);
        } else {
            self.metrics.apply_failures = self.metrics.apply_failures.saturating_add(1);
        }
    }

    pub fn telemetry(&self) -> ProjectionSchedulerTelemetry {
        self.telemetry_at(0)
    }

    pub fn telemetry_at(&self, now_ms: u64) -> ProjectionSchedulerTelemetry {
        ProjectionSchedulerTelemetry {
            kind: self.kind.clone(),
            ttft_estimate_ms: self.config.ttft_estimate_ms,
            ttft_estimate_source: self.ttft_estimate_source.clone(),
            in_flight_job_id: self.in_flight.as_ref().map(|job| job.id.clone()),
            in_flight_age_ms: self
                .in_flight
                .as_ref()
                .map(|job| now_ms.saturating_sub(job.queued_at_ms))
                .unwrap_or(0),
            in_flight_span_count: self
                .in_flight
                .as_ref()
                .map(|job| job.basis.span_revisions.len())
                .unwrap_or(0),
            pending_span_count: self
                .pending_basis
                .as_ref()
                .map(|basis| basis.span_revisions.len())
                .unwrap_or(0),
            metrics: self.metrics.clone(),
        }
    }

    pub fn observe_ledger(
        &mut self,
        ledger: &TranscriptLedger,
        now_ms: u64,
    ) -> ProjectionSchedulerDecision {
        let basis = ledger.current_basis();
        if basis.span_revisions.is_empty() {
            return ProjectionSchedulerDecision::Idle;
        }

        if let Some(in_flight) = self.in_flight.as_ref() {
            let in_flight_age_ms = now_ms.saturating_sub(in_flight.queued_at_ms);
            let queued_span_count = basis.span_revisions.len();
            let reason = self.coalescing_reason(in_flight_age_ms, queued_span_count);
            let previous_pending_basis = self.pending_basis.as_ref().unwrap_or(&in_flight.basis);
            let coalesced_span_delta = basis_revision_delta_count(previous_pending_basis, &basis);
            if self.pending_basis.as_ref() != Some(&basis) {
                self.pending_basis = Some(basis.clone());
                self.metrics.coalesced_updates += 1;
                self.metrics.coalesced_span_count = self
                    .metrics
                    .coalesced_span_count
                    .saturating_add(coalesced_span_delta as u64);
            }
            return ProjectionSchedulerDecision::Coalesced {
                in_flight_job_id: in_flight.id.clone(),
                queued_span_count,
                coalesced_span_delta,
                ttft_estimate_ms: self.config.ttft_estimate_ms,
                in_flight_age_ms,
                reason,
            };
        }

        if self.last_completed_basis.as_ref() == Some(&basis) {
            return ProjectionSchedulerDecision::Idle;
        }
        if self.last_failed_basis.as_ref() == Some(&basis) {
            return ProjectionSchedulerDecision::Idle;
        }

        let job = self.start_job(basis, ProjectionPriority::Realtime, now_ms);
        ProjectionSchedulerDecision::StartJob { job }
    }

    pub fn complete_in_flight(
        &mut self,
        ledger: &TranscriptLedger,
        now_ms: u64,
    ) -> ProjectionSchedulerDecision {
        let Some(completed) = self.in_flight.take() else {
            return ProjectionSchedulerDecision::Idle;
        };

        match ledger.validate_basis(&completed.basis) {
            Ok(()) => {
                self.record_job_lag(&completed, now_ms);
                self.metrics.completed_jobs += 1;
                self.last_completed_basis = Some(completed.basis);
                let current_basis = ledger.current_basis();
                self.pending_basis = None;
                if current_basis.span_revisions.is_empty()
                    || self.last_completed_basis.as_ref() == Some(&current_basis)
                {
                    ProjectionSchedulerDecision::CompletedCurrent {
                        completed_job_id: completed.id,
                    }
                } else {
                    self.metrics.follow_up_jobs_started += 1;
                    let job = self.start_job(current_basis, ProjectionPriority::Background, now_ms);
                    ProjectionSchedulerDecision::CompletedAndStartedFollowUp {
                        completed_job_id: completed.id,
                        job,
                    }
                }
            }
            Err(staleness) => {
                self.record_job_lag(&completed, now_ms);
                self.metrics.stale_discards += 1;
                self.pending_basis = None;
                let current_basis = ledger.current_basis();
                if current_basis.span_revisions.is_empty() {
                    ProjectionSchedulerDecision::DiscardedStaleNoCurrentBasis {
                        discarded_job_id: completed.id,
                        staleness,
                    }
                } else {
                    self.metrics.repair_jobs_started += 1;
                    let job = self.start_job(current_basis, ProjectionPriority::Replay, now_ms);
                    ProjectionSchedulerDecision::DiscardedStaleAndStartedRepair {
                        discarded_job_id: completed.id,
                        staleness,
                        job,
                    }
                }
            }
        }
    }

    pub fn fail_in_flight(
        &mut self,
        ledger: &TranscriptLedger,
        now_ms: u64,
    ) -> ProjectionSchedulerDecision {
        let Some(failed) = self.in_flight.take() else {
            return ProjectionSchedulerDecision::Idle;
        };

        self.record_job_lag(&failed, now_ms);
        self.metrics.failed_jobs += 1;
        self.pending_basis = None;

        match ledger.validate_basis(&failed.basis) {
            Ok(()) => {
                self.last_failed_basis = Some(failed.basis);
                ProjectionSchedulerDecision::FailedCurrent {
                    failed_job_id: failed.id,
                }
            }
            Err(staleness) => {
                self.metrics.stale_discards += 1;
                let current_basis = ledger.current_basis();
                if current_basis.span_revisions.is_empty() {
                    ProjectionSchedulerDecision::FailedStaleNoCurrentBasis {
                        failed_job_id: failed.id,
                        staleness,
                    }
                } else {
                    self.metrics.repair_jobs_started += 1;
                    let job = self.start_job(current_basis, ProjectionPriority::Replay, now_ms);
                    ProjectionSchedulerDecision::FailedStaleAndStartedRepair {
                        failed_job_id: failed.id,
                        staleness,
                        job,
                    }
                }
            }
        }
    }

    fn start_job(
        &mut self,
        basis: ProjectionBasis,
        priority: ProjectionPriority,
        now_ms: u64,
    ) -> ProjectionJob {
        self.next_job_index += 1;
        let job = ProjectionJob {
            id: format!(
                "projection:{}:{}:{}",
                self.session_id,
                projection_kind_key(&self.kind),
                self.next_job_index
            ),
            session_id: self.session_id.clone(),
            kind: self.kind.clone(),
            basis,
            priority,
            queued_at_ms: now_ms,
        };
        self.metrics.jobs_started += 1;
        self.last_failed_basis = None;
        // The new job's basis is the current ledger basis, which subsumes any
        // queued pending work (e.g. a basis demoted from a persisted
        // in-flight job by `restore_from_snapshot`). Clear it so the
        // coalescing baseline restarts from this job.
        self.pending_basis = None;
        self.in_flight = Some(job.clone());
        job
    }

    fn record_job_lag(&mut self, job: &ProjectionJob, now_ms: u64) {
        let lag = now_ms.saturating_sub(job.queued_at_ms);
        self.metrics.last_job_lag_ms = lag;
        self.metrics.max_job_lag_ms = self.metrics.max_job_lag_ms.max(lag);
    }

    fn coalescing_reason(
        &self,
        in_flight_age_ms: u64,
        queued_span_count: usize,
    ) -> ProjectionCoalescingReason {
        if queued_span_count >= self.config.coalesce_span_threshold.max(1) {
            ProjectionCoalescingReason::PendingSpanThreshold
        } else if in_flight_age_ms >= self.config.ttft_estimate_ms {
            ProjectionCoalescingReason::InFlightAgeThreshold
        } else {
            ProjectionCoalescingReason::TtftWindow
        }
    }
}

fn basis_revision_delta_count(previous: &ProjectionBasis, next: &ProjectionBasis) -> usize {
    let transcript_delta = next
        .span_revisions
        .iter()
        .filter(|candidate| {
            !previous
                .span_revisions
                .iter()
                .any(|current| current == *candidate)
        })
        .count();
    let diarization_delta = next
        .diarization_span_revisions
        .iter()
        .filter(|candidate| {
            !previous
                .diarization_span_revisions
                .iter()
                .any(|current| current == *candidate)
        })
        .count();
    transcript_delta + diarization_delta
}

/// Persistent snapshot of the durable parts of a [`ProjectionSchedulers`]
/// instance: `pending_basis` and `in_flight` for both notes and graph.
/// Written to disk whenever the queue mutates; rehydrated by `load_session`.
///
/// The `*_in_flight` jobs are persisted for diagnostics only — after a
/// process restart there is no running task backing them, so
/// [`ProjectionSchedulers::restore_from_snapshot`] never resurrects them as
/// in-flight. It demotes a persisted in-flight job's basis into
/// `pending_basis` (unless a newer coalesced pending basis superseded it),
/// letting the next `observe_ledger` start a real job for that work.
///
/// Metrics and ttft_estimate are intentionally NOT persisted — they are
/// per-session runtime counters that start fresh on every restart, and the
/// ttft estimate is re-learned quickly from the first successful generation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct SchedulerQueueState {
    pub notes_pending_basis: Option<crate::projections::ProjectionBasis>,
    pub notes_in_flight: Option<crate::projections::ProjectionJob>,
    pub graph_pending_basis: Option<crate::projections::ProjectionBasis>,
    pub graph_in_flight: Option<crate::projections::ProjectionJob>,
}

#[derive(Debug, Clone)]
pub struct ProjectionSchedulers {
    notes: ProjectionScheduler,
    graph: ProjectionScheduler,
}

impl ProjectionSchedulers {
    pub fn new(session_id: impl Into<String>) -> Self {
        let session_id = session_id.into();
        Self {
            notes: ProjectionScheduler::new(session_id.clone(), ProjectionKind::Notes),
            graph: ProjectionScheduler::new(session_id, ProjectionKind::Graph),
        }
    }

    pub fn reset(&mut self, session_id: impl Into<String>) {
        *self = Self::new(session_id);
    }

    pub fn observe_ledger(
        &mut self,
        ledger: &TranscriptLedger,
        now_ms: u64,
    ) -> ProjectionSchedulersObservation {
        ProjectionSchedulersObservation {
            notes: self.notes.observe_ledger(ledger, now_ms),
            graph: self.graph.observe_ledger(ledger, now_ms),
        }
    }

    pub fn complete_notes_in_flight(
        &mut self,
        ledger: &TranscriptLedger,
        now_ms: u64,
    ) -> ProjectionSchedulerDecision {
        self.notes.complete_in_flight(ledger, now_ms)
    }

    pub fn complete_graph_in_flight(
        &mut self,
        ledger: &TranscriptLedger,
        now_ms: u64,
    ) -> ProjectionSchedulerDecision {
        self.graph.complete_in_flight(ledger, now_ms)
    }

    pub fn fail_notes_in_flight(
        &mut self,
        ledger: &TranscriptLedger,
        now_ms: u64,
    ) -> ProjectionSchedulerDecision {
        self.notes.fail_in_flight(ledger, now_ms)
    }

    pub fn fail_graph_in_flight(
        &mut self,
        ledger: &TranscriptLedger,
        now_ms: u64,
    ) -> ProjectionSchedulerDecision {
        self.graph.fail_in_flight(ledger, now_ms)
    }

    pub fn notes(&self) -> &ProjectionScheduler {
        &self.notes
    }

    pub fn graph(&self) -> &ProjectionScheduler {
        &self.graph
    }

    pub fn record_generation_result(
        &mut self,
        kind: &ProjectionKind,
        latency_ms: u64,
        tokens_used: u32,
        success: bool,
    ) {
        match kind {
            ProjectionKind::Notes => {
                self.notes
                    .record_generation_result(latency_ms, tokens_used, success)
            }
            ProjectionKind::Graph => {
                self.graph
                    .record_generation_result(latency_ms, tokens_used, success)
            }
        }
    }

    pub fn set_configured_ttft_estimate(&mut self, kind: &ProjectionKind, estimate_ms: u64) {
        match kind {
            ProjectionKind::Notes => self.notes.set_configured_ttft_estimate(estimate_ms),
            ProjectionKind::Graph => self.graph.set_configured_ttft_estimate(estimate_ms),
        }
    }

    pub fn record_apply_result(&mut self, kind: &ProjectionKind, latency_ms: u64, accepted: bool) {
        match kind {
            ProjectionKind::Notes => self.notes.record_apply_result(latency_ms, accepted),
            ProjectionKind::Graph => self.graph.record_apply_result(latency_ms, accepted),
        }
    }

    pub fn telemetry(&self) -> ProjectionSchedulersTelemetry {
        self.telemetry_at(0)
    }

    pub fn telemetry_at(&self, now_ms: u64) -> ProjectionSchedulersTelemetry {
        ProjectionSchedulersTelemetry {
            notes: self.notes.telemetry_at(now_ms),
            graph: self.graph.telemetry_at(now_ms),
        }
    }

    /// Snapshot the durable queue state for persistence.
    ///
    /// The `in_flight` jobs are captured for diagnostics only:
    /// [`Self::restore_from_snapshot`] demotes them to `pending_basis` rather
    /// than resurrecting a phantom in-flight job with no backing task.
    pub fn snapshot_queue(&self) -> SchedulerQueueState {
        SchedulerQueueState {
            notes_pending_basis: self.notes.pending_basis.clone(),
            notes_in_flight: self.notes.in_flight.clone(),
            graph_pending_basis: self.graph.pending_basis.clone(),
            graph_in_flight: self.graph.in_flight.clone(),
        }
    }

    /// Restore queue state from a persisted snapshot.
    ///
    /// Only applies when the scheduler is idle (i.e. it was just created via
    /// `new` / `reset`). This is safe to call unconditionally after `reset()`
    /// — if the scheduler already has live work, the snapshot is silently
    /// ignored.
    ///
    /// A persisted `in_flight` job is never restored as in-flight: after a
    /// restart there is no running task backing it, so rehydrating it would
    /// leave the scheduler waiting forever on a phantom job — every new
    /// ledger change would coalesce behind it and no real job would ever
    /// start. Instead the job is demoted: its basis folds into
    /// `pending_basis` so the next `observe_ledger` starts a real job for
    /// that work. When the snapshot carries both an `in_flight` job and a
    /// `pending_basis`, the pending basis is newer (it coalesced after the
    /// job started) and wins; the superseded in-flight basis is dropped.
    pub fn restore_from_snapshot(&mut self, snapshot: SchedulerQueueState) {
        if self.notes.in_flight.is_none() && self.notes.pending_basis.is_none() {
            self.notes.pending_basis = snapshot
                .notes_pending_basis
                .or_else(|| snapshot.notes_in_flight.map(|job| job.basis));
        }
        if self.graph.in_flight.is_none() && self.graph.pending_basis.is_none() {
            self.graph.pending_basis = snapshot
                .graph_pending_basis
                .or_else(|| snapshot.graph_in_flight.map(|job| job.basis));
        }
    }
}

fn projection_kind_key(kind: &ProjectionKind) -> &'static str {
    match kind {
        ProjectionKind::Notes => "notes",
        ProjectionKind::Graph => "graph",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projections::{TranscriptEvent, TranscriptEventStability};

    fn event(span_id: &str, revision_number: u64, text: &str) -> TranscriptEvent {
        TranscriptEvent {
            span_id: span_id.to_string(),
            provider: "test".to_string(),
            source_id: "source-1".to_string(),
            provider_item_id: Some(span_id.to_string()),
            transcript_segment_id: None,
            speaker_id: None,
            speaker_label: None,
            channel: None,
            text: text.to_string(),
            start_time: revision_number as f64,
            end_time: revision_number as f64 + 1.0,
            confidence: 0.9,
            is_final: true,
            stability: TranscriptEventStability::Final,
            revision_number,
            supersedes: None,
            turn_id: None,
            end_of_turn: true,
            raw_event_ref: None,
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms: 1_700_000_000_000 + revision_number,
        }
    }

    #[test]
    fn scheduler_starts_coalesces_and_repairs_stale_in_flight_job() {
        let mut ledger = TranscriptLedger::new("session-1");
        ledger
            .apply_event(event("span-1", 1, "first"))
            .expect("first event");
        let mut scheduler = ProjectionScheduler::with_config(
            "session-1",
            ProjectionKind::Notes,
            ProjectionSchedulerConfig {
                ttft_estimate_ms: 900,
                coalesce_span_threshold: 2,
            },
        );

        let first = scheduler.observe_ledger(&ledger, 10);
        let first_job_id = match first {
            ProjectionSchedulerDecision::StartJob { job } => {
                assert_eq!(job.priority, ProjectionPriority::Realtime);
                assert_eq!(job.basis.span_revisions.len(), 1);
                job.id
            }
            other => panic!("expected start job, got {other:?}"),
        };
        let telemetry = scheduler.telemetry_at(1_510);
        assert_eq!(
            telemetry.in_flight_job_id.as_deref(),
            Some(first_job_id.as_str())
        );
        assert_eq!(
            telemetry.ttft_estimate_source,
            ProjectionTtftEstimateSource::Configured
        );
        assert_eq!(telemetry.in_flight_age_ms, 1_500);
        assert_eq!(telemetry.in_flight_span_count, 1);

        ledger
            .apply_event(event("span-2", 1, "second"))
            .expect("second event");
        assert_eq!(
            scheduler.observe_ledger(&ledger, 20),
            ProjectionSchedulerDecision::Coalesced {
                in_flight_job_id: first_job_id.clone(),
                queued_span_count: 2,
                coalesced_span_delta: 1,
                ttft_estimate_ms: 900,
                in_flight_age_ms: 10,
                reason: ProjectionCoalescingReason::PendingSpanThreshold,
            }
        );

        let repair = scheduler.complete_in_flight(&ledger, 30);
        match repair {
            ProjectionSchedulerDecision::DiscardedStaleAndStartedRepair {
                discarded_job_id,
                staleness,
                job,
            } => {
                assert_eq!(discarded_job_id, first_job_id);
                assert_eq!(
                    staleness,
                    ProjectionBasisStaleness::MissingCurrentSpan {
                        span_id: "span-2".to_string(),
                        current_revision: 1,
                    }
                );
                assert_eq!(job.priority, ProjectionPriority::Replay);
                assert_eq!(job.basis.span_revisions.len(), 2);
            }
            other => panic!("expected stale repair, got {other:?}"),
        }

        assert_eq!(scheduler.metrics().jobs_started, 2);
        assert_eq!(scheduler.metrics().coalesced_updates, 1);
        assert_eq!(scheduler.metrics().coalesced_span_count, 1);
        assert_eq!(scheduler.metrics().stale_discards, 1);
        assert_eq!(scheduler.metrics().repair_jobs_started, 1);
        assert_eq!(scheduler.metrics().completed_jobs, 0);
        assert_eq!(scheduler.metrics().last_job_lag_ms, 20);
        assert_eq!(scheduler.metrics().max_job_lag_ms, 20);

        let telemetry = scheduler.telemetry();
        assert_eq!(telemetry.kind, ProjectionKind::Notes);
        assert_eq!(telemetry.ttft_estimate_ms, 900);
        assert_eq!(
            telemetry.ttft_estimate_source,
            ProjectionTtftEstimateSource::Configured
        );
        assert!(telemetry.in_flight_job_id.is_some());
        assert_eq!(telemetry.in_flight_span_count, 2);
        assert_eq!(telemetry.pending_span_count, 0);
    }

    #[test]
    fn scheduler_updates_ttft_estimate_from_successful_generation_latency() {
        let mut scheduler = ProjectionScheduler::new("session-1", ProjectionKind::Graph);
        let telemetry = scheduler.telemetry();
        assert_eq!(telemetry.ttft_estimate_ms, 1_200);
        assert_eq!(
            telemetry.ttft_estimate_source,
            ProjectionTtftEstimateSource::Default
        );

        scheduler.record_generation_result(640, 24, true);
        let telemetry = scheduler.telemetry();
        assert_eq!(telemetry.ttft_estimate_ms, 640);
        assert_eq!(
            telemetry.ttft_estimate_source,
            ProjectionTtftEstimateSource::ObservedGeneration
        );
        assert_eq!(telemetry.metrics.tokens_used, 24);
        assert_eq!(telemetry.metrics.last_generation_latency_ms, 640);

        scheduler.record_generation_result(80, 0, false);
        let telemetry = scheduler.telemetry();
        assert_eq!(
            telemetry.ttft_estimate_ms, 640,
            "failed generations must not poison the next TTFT estimate",
        );
        assert_eq!(
            telemetry.ttft_estimate_source,
            ProjectionTtftEstimateSource::ObservedGeneration
        );
        assert_eq!(telemetry.metrics.generation_failures, 1);
        assert_eq!(telemetry.metrics.last_generation_latency_ms, 80);

        scheduler.set_configured_ttft_estimate(720);
        let telemetry = scheduler.telemetry();
        assert_eq!(telemetry.ttft_estimate_ms, 720);
        assert_eq!(
            telemetry.ttft_estimate_source,
            ProjectionTtftEstimateSource::Configured
        );
    }

    #[test]
    fn scheduler_classifies_coalescing_pressure_and_counts_span_deltas() {
        let mut ledger = TranscriptLedger::new("session-1");
        ledger
            .apply_event(event("span-1", 1, "first"))
            .expect("first event");
        let mut scheduler = ProjectionScheduler::with_config(
            "session-1",
            ProjectionKind::Notes,
            ProjectionSchedulerConfig {
                ttft_estimate_ms: 100,
                coalesce_span_threshold: 10,
            },
        );

        let first_job_id = match scheduler.observe_ledger(&ledger, 10) {
            ProjectionSchedulerDecision::StartJob { job } => job.id,
            other => panic!("expected start job, got {other:?}"),
        };

        ledger
            .apply_event(event("span-2", 1, "second"))
            .expect("second event");
        assert_eq!(
            scheduler.observe_ledger(&ledger, 40),
            ProjectionSchedulerDecision::Coalesced {
                in_flight_job_id: first_job_id.clone(),
                queued_span_count: 2,
                coalesced_span_delta: 1,
                ttft_estimate_ms: 100,
                in_flight_age_ms: 30,
                reason: ProjectionCoalescingReason::TtftWindow,
            }
        );
        assert_eq!(scheduler.metrics().coalesced_updates, 1);
        assert_eq!(scheduler.metrics().coalesced_span_count, 1);

        assert_eq!(
            scheduler.observe_ledger(&ledger, 150),
            ProjectionSchedulerDecision::Coalesced {
                in_flight_job_id: first_job_id.clone(),
                queued_span_count: 2,
                coalesced_span_delta: 0,
                ttft_estimate_ms: 100,
                in_flight_age_ms: 140,
                reason: ProjectionCoalescingReason::InFlightAgeThreshold,
            }
        );
        assert_eq!(
            scheduler.metrics().coalesced_updates,
            1,
            "re-observing the same pending basis must not double-count updates",
        );
        assert_eq!(scheduler.metrics().coalesced_span_count, 1);

        ledger
            .apply_event(event("span-3", 1, "third"))
            .expect("third event");
        assert_eq!(
            scheduler.observe_ledger(&ledger, 160),
            ProjectionSchedulerDecision::Coalesced {
                in_flight_job_id: first_job_id,
                queued_span_count: 3,
                coalesced_span_delta: 1,
                ttft_estimate_ms: 100,
                in_flight_age_ms: 150,
                reason: ProjectionCoalescingReason::InFlightAgeThreshold,
            }
        );
        assert_eq!(scheduler.metrics().coalesced_updates, 2);
        assert_eq!(scheduler.metrics().coalesced_span_count, 2);
    }

    #[test]
    fn scheduler_marks_current_completion_and_idles_until_basis_changes() {
        let mut ledger = TranscriptLedger::new("session-1");
        ledger
            .apply_event(event("span-1", 1, "first"))
            .expect("first event");
        let mut scheduler = ProjectionScheduler::new("session-1", ProjectionKind::Graph);

        let started = scheduler.observe_ledger(&ledger, 10);
        let job_id = match started {
            ProjectionSchedulerDecision::StartJob { job } => job.id,
            other => panic!("expected start job, got {other:?}"),
        };
        assert_eq!(
            scheduler.complete_in_flight(&ledger, 20),
            ProjectionSchedulerDecision::CompletedCurrent {
                completed_job_id: job_id,
            }
        );
        assert_eq!(scheduler.metrics().completed_jobs, 1);
        assert_eq!(scheduler.metrics().last_job_lag_ms, 10);
        assert_eq!(scheduler.metrics().max_job_lag_ms, 10);
        assert!(scheduler.telemetry().in_flight_job_id.is_none());
        assert_eq!(
            scheduler.observe_ledger(&ledger, 30),
            ProjectionSchedulerDecision::Idle
        );

        ledger
            .apply_event(event("span-2", 1, "second"))
            .expect("second event");
        assert!(matches!(
            scheduler.observe_ledger(&ledger, 40),
            ProjectionSchedulerDecision::StartJob { .. }
        ));
    }

    #[test]
    fn scheduler_failure_clears_in_flight_and_idles_until_basis_changes() {
        let mut ledger = TranscriptLedger::new("session-1");
        ledger
            .apply_event(event("span-1", 1, "first"))
            .expect("first event");
        let mut scheduler = ProjectionScheduler::new("session-1", ProjectionKind::Notes);

        let started = scheduler.observe_ledger(&ledger, 10);
        let job_id = match started {
            ProjectionSchedulerDecision::StartJob { job } => job.id,
            other => panic!("expected start job, got {other:?}"),
        };
        assert_eq!(
            scheduler.fail_in_flight(&ledger, 25),
            ProjectionSchedulerDecision::FailedCurrent {
                failed_job_id: job_id,
            }
        );
        assert_eq!(scheduler.metrics().failed_jobs, 1);
        assert_eq!(scheduler.metrics().last_job_lag_ms, 15);
        assert_eq!(scheduler.metrics().max_job_lag_ms, 15);
        assert!(scheduler.in_flight_job().is_none());
        assert_eq!(
            scheduler.observe_ledger(&ledger, 30),
            ProjectionSchedulerDecision::Idle,
            "unchanged failed basis must not retry forever"
        );

        ledger
            .apply_event(event("span-2", 1, "second"))
            .expect("second event");
        assert!(matches!(
            scheduler.observe_ledger(&ledger, 40),
            ProjectionSchedulerDecision::StartJob { .. }
        ));
        assert_eq!(scheduler.metrics().jobs_started, 2);
    }

    #[test]
    fn scheduler_failure_on_stale_job_starts_repair_for_current_basis() {
        let mut ledger = TranscriptLedger::new("session-1");
        ledger
            .apply_event(event("span-1", 1, "first"))
            .expect("first event");
        let mut scheduler = ProjectionScheduler::new("session-1", ProjectionKind::Graph);

        let started = scheduler.observe_ledger(&ledger, 10);
        let job_id = match started {
            ProjectionSchedulerDecision::StartJob { job } => job.id,
            other => panic!("expected start job, got {other:?}"),
        };
        ledger
            .apply_event(event("span-2", 1, "second"))
            .expect("second event");
        assert!(matches!(
            scheduler.observe_ledger(&ledger, 20),
            ProjectionSchedulerDecision::Coalesced { .. }
        ));

        let repair = scheduler.fail_in_flight(&ledger, 35);
        match repair {
            ProjectionSchedulerDecision::FailedStaleAndStartedRepair {
                failed_job_id,
                staleness,
                job,
            } => {
                assert_eq!(failed_job_id, job_id);
                assert_eq!(
                    staleness,
                    ProjectionBasisStaleness::MissingCurrentSpan {
                        span_id: "span-2".to_string(),
                        current_revision: 1,
                    }
                );
                assert_eq!(job.priority, ProjectionPriority::Replay);
                assert_eq!(job.basis.span_revisions.len(), 2);
            }
            other => panic!("expected stale failure repair, got {other:?}"),
        }
        assert_eq!(scheduler.metrics().failed_jobs, 1);
        assert_eq!(scheduler.metrics().stale_discards, 1);
        assert_eq!(scheduler.metrics().repair_jobs_started, 1);
        assert!(scheduler.in_flight_job().is_some());
    }

    #[test]
    fn scheduler_queue_snapshot_restore_demotes_in_flight_to_pending() {
        let session_id = "test-queue-persist-abc123";
        let mut schedulers = ProjectionSchedulers::new(session_id);

        // Build a ledger with one span so observe_ledger queues a job.
        let mut ledger = TranscriptLedger::new(session_id);
        ledger
            .apply_event(event("span-1", 1, "hello"))
            .expect("apply");
        let obs = schedulers.observe_ledger(&ledger, 100);
        assert!(
            matches!(obs.notes, ProjectionSchedulerDecision::StartJob { .. }),
            "notes job started"
        );
        assert!(
            matches!(obs.graph, ProjectionSchedulerDecision::StartJob { .. }),
            "graph job started"
        );

        // Snapshot captures the in-flight jobs (for diagnostics)...
        let snapshot = schedulers.snapshot_queue();
        assert!(
            snapshot.notes_in_flight.is_some(),
            "notes in_flight captured"
        );
        assert!(
            snapshot.graph_in_flight.is_some(),
            "graph in_flight captured"
        );
        let expected_basis = snapshot
            .notes_in_flight
            .as_ref()
            .expect("notes in_flight captured")
            .basis
            .clone();

        // ...but restoring into a fresh scheduler (simulating a restart) must
        // NOT resurrect them: no running task backs a persisted in-flight
        // job, so rehydrating it would deadlock the queue behind a phantom.
        let mut fresh = ProjectionSchedulers::new(session_id);
        fresh.restore_from_snapshot(snapshot);
        assert!(
            fresh.notes().in_flight_job().is_none(),
            "persisted notes in_flight must be demoted, not restored"
        );
        assert!(
            fresh.graph().in_flight_job().is_none(),
            "persisted graph in_flight must be demoted, not restored"
        );
        let fresh_snap = fresh.snapshot_queue();
        assert_eq!(
            fresh_snap.notes_pending_basis.as_ref(),
            Some(&expected_basis),
            "demoted notes in_flight basis lands in pending"
        );
        assert!(
            fresh_snap.graph_pending_basis.is_some(),
            "demoted graph in_flight basis lands in pending"
        );

        // The next observe starts a REAL job covering the demoted basis — the
        // work is re-dispatched to a live task, not lost and not phantom.
        let obs = fresh.observe_ledger(&ledger, 200);
        match obs.notes {
            ProjectionSchedulerDecision::StartJob { job } => {
                assert_eq!(
                    job.basis, expected_basis,
                    "restarted notes job covers the demoted basis"
                );
            }
            other => panic!("expected fresh notes job after restore, got {other:?}"),
        }
        assert!(
            matches!(obs.graph, ProjectionSchedulerDecision::StartJob { .. }),
            "graph restarts a real job after restore"
        );
    }

    #[test]
    fn scheduler_queue_restore_prefers_newer_pending_basis_over_in_flight() {
        let session_id = "test-queue-pending-wins";
        let mut schedulers = ProjectionSchedulers::new(session_id);

        let mut ledger = TranscriptLedger::new(session_id);
        ledger
            .apply_event(event("span-1", 1, "first"))
            .expect("first event");
        let obs = schedulers.observe_ledger(&ledger, 100);
        assert!(
            matches!(obs.notes, ProjectionSchedulerDecision::StartJob { .. }),
            "notes job started"
        );

        // A second span arrives while the job is in flight → coalesces into
        // pending_basis, which is now newer than the in-flight basis.
        ledger
            .apply_event(event("span-2", 1, "second"))
            .expect("second event");
        let obs = schedulers.observe_ledger(&ledger, 110);
        assert!(
            matches!(obs.notes, ProjectionSchedulerDecision::Coalesced { .. }),
            "second span coalesces behind the in-flight job"
        );

        let snapshot = schedulers.snapshot_queue();
        let pending = snapshot
            .notes_pending_basis
            .clone()
            .expect("coalesced pending basis captured");
        let in_flight_basis = snapshot
            .notes_in_flight
            .as_ref()
            .expect("in-flight job captured")
            .basis
            .clone();
        assert_ne!(pending, in_flight_basis, "pending superseded in-flight");

        // Restore: the newer pending basis wins; the superseded in-flight
        // basis is dropped (its work is contained within the pending basis).
        let mut fresh = ProjectionSchedulers::new(session_id);
        fresh.restore_from_snapshot(snapshot);
        assert!(fresh.notes().in_flight_job().is_none());
        assert_eq!(
            fresh.snapshot_queue().notes_pending_basis.as_ref(),
            Some(&pending),
            "restore keeps the newer coalesced pending basis"
        );
    }

    /// RAII guard that points `AUDIOGRAPH_DATA_DIR` at an isolated tempdir and
    /// restores the previous value on drop. Mutating process env requires the
    /// `crate::sessions::TEST_HOME_LOCK` to be held by the caller.
    struct DataDirGuard {
        prev: Option<std::ffi::OsString>,
    }

    impl DataDirGuard {
        #[allow(unsafe_code)]
        fn set(path: &std::path::Path) -> Self {
            let prev = std::env::var_os(crate::user_data::DATA_DIR_ENV);
            // SAFETY: serialized by crate::sessions::TEST_HOME_LOCK.
            unsafe {
                std::env::set_var(crate::user_data::DATA_DIR_ENV, path);
            }
            Self { prev }
        }
    }

    impl Drop for DataDirGuard {
        #[allow(unsafe_code)]
        fn drop(&mut self) {
            // SAFETY: serialized by crate::sessions::TEST_HOME_LOCK.
            unsafe {
                match &self.prev {
                    Some(value) => std::env::set_var(crate::user_data::DATA_DIR_ENV, value),
                    None => std::env::remove_var(crate::user_data::DATA_DIR_ENV),
                }
            }
        }
    }

    #[test]
    fn scheduler_queue_round_trips_through_disk_persistence() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "audio-graph-scheduler-queue-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("create tempdir");
        let _guard = DataDirGuard::set(&dir);

        let session_id = "queue-disk-roundtrip";
        let mut schedulers = ProjectionSchedulers::new(session_id);
        let mut ledger = TranscriptLedger::new(session_id);
        ledger
            .apply_event(event("span-1", 1, "hello"))
            .expect("apply");
        let obs = schedulers.observe_ledger(&ledger, 100);
        assert!(
            matches!(obs.notes, ProjectionSchedulerDecision::StartJob { .. }),
            "notes job started"
        );

        let snapshot = schedulers.snapshot_queue();
        let expected_basis = snapshot
            .notes_in_flight
            .as_ref()
            .expect("notes in_flight captured")
            .basis
            .clone();
        crate::persistence::save_scheduler_queue_state(session_id, &snapshot);

        let loaded = crate::persistence::load_scheduler_queue_state(session_id)
            .expect("snapshot loads back from disk");
        assert_eq!(loaded, snapshot, "disk round-trip preserves the snapshot");

        // Restoring the disk-loaded snapshot demotes in-flight, same as the
        // in-memory path load_session exercises.
        let mut fresh = ProjectionSchedulers::new(session_id);
        fresh.restore_from_snapshot(loaded);
        assert!(
            fresh.notes().in_flight_job().is_none(),
            "disk-restored in_flight must be demoted"
        );
        assert_eq!(
            fresh.snapshot_queue().notes_pending_basis.as_ref(),
            Some(&expected_basis),
            "demoted basis lands in pending after disk round-trip"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
