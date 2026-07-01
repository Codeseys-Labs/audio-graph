//! Speech processing orchestrator.
//!
//! Contains the speech processor logic (ASR + diarization + entity extraction)
//! extracted from `commands.rs` to keep command handlers thin.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

mod context;
pub(crate) use context::{ExtractionDeps, SpeechChannels, SpeechConfig, SpeechShared};

/// Bounded thread pool for fire-and-forget entity extraction tasks.
///
/// Previously, each transcript segment spawned a new `std::thread` — a 10-hour
/// session at 2 segments/sec creates 72,000 threads, exhausting OS thread
/// limits (typically 1024-4096 per process). Using rayon's work-stealing pool
/// with a fixed worker count (4) eliminates this issue while still giving
/// extraction tasks their own thread budget separate from the ASR critical path.
fn extraction_pool() -> &'static rayon::ThreadPool {
    static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(4)
            .thread_name(|i| format!("extraction-{}", i))
            .build()
            .expect("Failed to build extraction thread pool")
    })
}

/// Small pool for deterministic agent/react event production.
///
/// Keep this separate from the extraction pool: background LLM extraction can
/// block on provider I/O, but proposal/status events should keep flowing so
/// the UI can react to fresh transcript segments.
fn agent_pool() -> &'static rayon::ThreadPool {
    static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(2)
            .thread_name(|i| format!("agent-react-{}", i))
            .build()
            .expect("Failed to build agent/react thread pool")
    })
}

use crossbeam_channel::Receiver;
use tauri::{AppHandle, Emitter};

use crate::asr::AsrConfig;
#[cfg(feature = "asr-whisper")]
use crate::asr::AsrWorker;
use crate::asr::cloud::CloudAsrConfig;
use crate::asr::moonshine::{
    MoonshineSpanRevision, MoonshineStreamingAdapter, MoonshineStreamingWorker,
    MoonshineWorkerError,
};
use crate::asr::soniox::SonioxParsedRevision;
use crate::audio::pipeline::ProcessedAudioChunk;
use crate::diarization::{
    DiarizationConfig, DiarizationInput, DiarizationWorker, DiarizedTranscript,
};
use crate::events::{self, PipelineStatus, StageStatus};
use crate::graph::entities::{GraphDelta, GraphSnapshot};
use crate::graph::extraction::RuleBasedExtractor;
use crate::graph::temporal::TemporalKnowledgeGraph;
use crate::llm::{
    ApiClient, LlmEngine, LlmExecutor, LlmPriority, MistralRsEngine, ProjectionPatchOutcome,
};
use crate::models::SORTFORMER_MODEL_FILENAME;
use crate::persistence::{FileMemoryRepository, LocalMemoryRepository};
use crate::projection_scheduler::{ProjectionSchedulerDecision, ProjectionSchedulersObservation};
use crate::projections::{
    DiarizationSpanRevision, MaterializedGraph, MaterializedNotes, ProjectionApplyError,
    ProjectionJob, ProjectionKind, ProjectionPatch, SpeakerTimeline, TranscriptLedger,
};
use crate::settings::{AsrProvider, LlmProvider};
use crate::state::{
    ProjectionRuntimeApplyError, ProjectionRuntimeHandle, SpeakerInfo, TranscriptSegment,
};

const MAX_PENDING_AGENT_PROPOSALS: usize = 200;
const MOONSHINE_RECV_TIMEOUT: Duration = Duration::from_millis(50);

/// Emit a single pipeline latency sample. Best-effort: telemetry must never
/// block or fail the speech pipeline.
fn emit_stage_latency(
    app_handle: &AppHandle,
    stage: &str,
    source_id: Option<&str>,
    segment_id: Option<&str>,
    elapsed: Duration,
) {
    let timestamp_ms = current_unix_millis();
    events::emit_or_log(
        app_handle,
        events::PIPELINE_LATENCY,
        events::PipelineLatencyPayload {
            stage: stage.to_string(),
            source_id: source_id.map(str::to_string),
            segment_id: segment_id.map(str::to_string),
            latency_ms: elapsed.as_secs_f64() * 1000.0,
            timestamp_ms,
        },
    );
}

fn current_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

trait DiarizationEventSink {
    fn emit_diarization_span_revision(&self, payload: &events::DiarizationSpanRevisionPayload);
    fn emit_graph_delta(&self, delta: &GraphDelta);
    fn emit_graph_update(&self, snapshot: &GraphSnapshot);
}

struct TauriDiarizationEventSink<'a> {
    app_handle: &'a AppHandle,
}

impl DiarizationEventSink for TauriDiarizationEventSink<'_> {
    fn emit_diarization_span_revision(&self, payload: &events::DiarizationSpanRevisionPayload) {
        events::emit_or_log(
            self.app_handle,
            events::DIARIZATION_SPAN_REVISION,
            payload.clone(),
        );
    }

    fn emit_graph_delta(&self, delta: &GraphDelta) {
        events::emit_or_log(self.app_handle, events::GRAPH_DELTA, delta);
    }

    fn emit_graph_update(&self, snapshot: &GraphSnapshot) {
        events::emit_or_log(self.app_handle, events::GRAPH_UPDATE, snapshot);
    }
}

struct DiarizationDispatchContext<'a, E: DiarizationEventSink + ?Sized> {
    event_sink: &'a E,
    speaker_timeline: &'a Arc<Mutex<SpeakerTimeline>>,
    knowledge_graph: &'a Arc<Mutex<TemporalKnowledgeGraph>>,
    graph_snapshot: &'a Arc<RwLock<GraphSnapshot>>,
}

fn millis_from_secs(value: f64) -> i64 {
    if value.is_finite() {
        (value * 1000.0).round() as i64
    } else {
        0
    }
}

fn time_based_span_id(provider: &str, source_id: &str, start_time: f64, end_time: f64) -> String {
    format!(
        "{}:{}:{}-{}",
        provider,
        source_id,
        millis_from_secs(start_time),
        millis_from_secs(end_time)
    )
}

fn provider_item_span_id(provider: &str, source_id: &str, provider_item_id: &str) -> String {
    format!("{}:{}:{}", provider, source_id, provider_item_id)
}

fn provider_start_span_id(provider: &str, source_id: &str, start_time: f64) -> String {
    format!(
        "{}:{}:start-{}",
        provider,
        source_id,
        millis_from_secs(start_time)
    )
}

fn provider_sequence_span_id(
    provider: &str,
    source_id: &str,
    sequence_label: &str,
    sequence: u64,
) -> String {
    format!("{provider}:{source_id}:{sequence_label}-{sequence}")
}

fn final_only_provider_item_id(start_time: f64, end_time: f64) -> String {
    format!(
        "final-{}-{}",
        millis_from_secs(start_time),
        millis_from_secs(end_time)
    )
}

fn final_only_revision_meta(
    provider: &str,
    source_id: &str,
    start_time: f64,
    end_time: f64,
) -> AsrRevisionMeta {
    let provider_item_id = final_only_provider_item_id(start_time, end_time);
    AsrRevisionMeta {
        span_id: Some(provider_item_span_id(
            provider,
            source_id,
            &provider_item_id,
        )),
        provider_item_id: Some(provider_item_id),
        revision_number: Some(1),
        ..AsrRevisionMeta::default()
    }
}

fn revision_ref(span_id: &str, revision_number: u64) -> String {
    format!("{span_id}@rev{revision_number}")
}

fn next_span_revision(
    revision_numbers_by_span: &mut HashMap<String, u64>,
    span_id: &str,
) -> (u64, Option<String>) {
    let revision_number = revision_numbers_by_span
        .entry(span_id.to_string())
        .or_insert(0);
    *revision_number += 1;
    let supersedes = (*revision_number > 1).then(|| revision_ref(span_id, *revision_number - 1));
    (*revision_number, supersedes)
}

fn final_span_revision(
    revision_numbers_by_span: &mut HashMap<String, u64>,
    span_id: &str,
) -> (u64, Option<String>) {
    let revision_number = revision_numbers_by_span.remove(span_id).unwrap_or(0) + 1;
    let supersedes = (revision_number > 1).then(|| revision_ref(span_id, revision_number - 1));
    (revision_number, supersedes)
}

fn diarization_span_revision_id(
    provider: &str,
    timeline_id: &str,
    start_time: f64,
    end_time: f64,
    speaker_id: Option<&str>,
) -> String {
    format!(
        "{}:{}:{}-{}:{}",
        provider,
        timeline_id,
        millis_from_secs(start_time),
        millis_from_secs(end_time),
        speaker_id.unwrap_or("unknown")
    )
}

fn transcript_speaker_key(segment: &TranscriptSegment) -> Option<&str> {
    segment
        .speaker_id
        .as_deref()
        .or(segment.speaker_label.as_deref())
        .filter(|value| !value.trim().is_empty())
}

fn diarization_span_revision_for_transcript(
    provider: &str,
    segment: &TranscriptSegment,
    basis_asr_span_id: &str,
    channel: Option<String>,
    raw_event_ref: Option<String>,
    received_at_ms: u64,
) -> Option<events::DiarizationSpanRevisionPayload> {
    let speaker_key = transcript_speaker_key(segment)?;
    Some(events::DiarizationSpanRevisionPayload {
        span_id: diarization_span_revision_id(
            provider,
            &segment.source_id,
            segment.start_time,
            segment.end_time,
            Some(speaker_key),
        ),
        provider: provider.to_string(),
        timeline_id: segment.source_id.clone(),
        source_id: Some(segment.source_id.clone()),
        speaker_id: segment.speaker_id.clone(),
        speaker_label: segment.speaker_label.clone(),
        channel,
        start_time: segment.start_time,
        end_time: segment.end_time,
        confidence: segment.confidence.is_finite().then_some(segment.confidence),
        is_final: true,
        stability: events::DiarizationSpanStability::Final,
        revision_number: 1,
        supersedes: None,
        basis_asr_span_ids: vec![basis_asr_span_id.to_string()],
        basis_transcript_segment_ids: vec![segment.id.clone()],
        raw_event_ref,
        capture_latency_ms: None,
        asr_latency_ms: None,
        received_at_ms,
    })
}

fn emit_diarization_span_revision_for_transcript<E: DiarizationEventSink + ?Sized>(
    dispatch_ctx: &DiarizationDispatchContext<'_, E>,
    provider: &str,
    segment: &TranscriptSegment,
    basis_asr_span_id: &str,
    channel: Option<String>,
    raw_event_ref: Option<String>,
) {
    if let Some(payload) = diarization_span_revision_for_transcript(
        provider,
        segment,
        basis_asr_span_id,
        channel,
        raw_event_ref,
        current_unix_millis(),
    ) {
        emit_and_dispatch_diarization_span_revision(dispatch_ctx, payload);
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct DiarizationRevisionOutcome {
    pub accepted: bool,
    pub retcon_fired: bool,
    pub edges_retconned: usize,
}

fn dispatch_diarization_span_revision(
    timeline: &mut SpeakerTimeline,
    graph: &mut TemporalKnowledgeGraph,
    revision: DiarizationSpanRevision,
    timestamp: f64,
) -> DiarizationRevisionOutcome {
    let remap = match timeline.apply_event(revision) {
        Ok(remap) => remap,
        Err(error) => {
            log::warn!("Diarization revision rejected by speaker timeline: {error:?}");
            return DiarizationRevisionOutcome {
                accepted: false,
                retcon_fired: false,
                edges_retconned: 0,
            };
        }
    };

    let Some(remap) = remap else {
        return DiarizationRevisionOutcome {
            accepted: true,
            retcon_fired: false,
            edges_retconned: 0,
        };
    };

    let invalidated = graph.supersede_entity(
        &remap.superseded_label,
        &remap.canonical_label,
        timestamp,
        1.0,
    );
    DiarizationRevisionOutcome {
        accepted: true,
        retcon_fired: invalidated > 0,
        edges_retconned: invalidated,
    }
}

fn emit_and_dispatch_diarization_span_revision<E: DiarizationEventSink + ?Sized>(
    dispatch_ctx: &DiarizationDispatchContext<'_, E>,
    payload: events::DiarizationSpanRevisionPayload,
) -> DiarizationRevisionOutcome {
    dispatch_ctx
        .event_sink
        .emit_diarization_span_revision(&payload);

    let (outcome, delta, snapshot) = {
        let mut timeline = match dispatch_ctx.speaker_timeline.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                log::warn!("Speaker timeline mutex poisoned; recovering");
                poisoned.into_inner()
            }
        };
        let mut graph = match dispatch_ctx.knowledge_graph.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                log::warn!("Knowledge graph mutex poisoned, recovering");
                poisoned.into_inner()
            }
        };
        let outcome = dispatch_diarization_span_revision(
            &mut timeline,
            &mut graph,
            DiarizationSpanRevision::from(payload),
            current_unix_millis() as f64 / 1000.0,
        );
        if outcome.retcon_fired {
            let delta = graph.has_delta().then(|| graph.take_delta());
            let snapshot = graph.snapshot();
            (outcome, delta, Some(snapshot))
        } else {
            (outcome, None, None)
        }
    };

    if let Some(delta) = delta {
        dispatch_ctx.event_sink.emit_graph_delta(&delta);
    }
    if let Some(snapshot) = snapshot {
        if let Ok(mut cached) = dispatch_ctx.graph_snapshot.write() {
            *cached = snapshot.clone();
        }
        dispatch_ctx.event_sink.emit_graph_update(&snapshot);
    }

    outcome
}

#[derive(Default)]
struct AsrRevisionMeta {
    span_id: Option<String>,
    provider_item_id: Option<String>,
    speaker_id: Option<String>,
    speaker_label: Option<String>,
    channel: Option<String>,
    revision_number: Option<u64>,
    supersedes: Option<String>,
    turn_id: Option<String>,
    raw_event_ref: Option<String>,
    capture_latency_ms: Option<u64>,
    asr_latency_ms: Option<u64>,
    received_at_ms: Option<u64>,
}

fn metadata_or_dash(value: Option<&str>) -> &str {
    value.unwrap_or("-")
}

fn revision_or_dash(revision_number: Option<u64>) -> String {
    revision_number
        .map(|revision_number| revision_number.to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn log_final_transcript_metadata(
    context: &str,
    provider: &str,
    count: u64,
    segment: &TranscriptSegment,
    meta: &AsrRevisionMeta,
) {
    log::debug!(
        "{}: emitted transcript metadata provider={} count={} segment_id={} span_id={} provider_item_id={} revision={} text_len={} confidence={:.3} speaker_present={}",
        context,
        provider,
        count,
        segment.id,
        metadata_or_dash(meta.span_id.as_deref()),
        metadata_or_dash(meta.provider_item_id.as_deref()),
        revision_or_dash(meta.revision_number),
        segment.text.chars().count(),
        segment.confidence,
        transcript_speaker_key(segment).is_some(),
    );
}

fn speech_error_diagnostic(provider: &str, category: &str, code: &str, message: &str) -> String {
    format!(
        "provider={} error_category={} error_code={} message_len={}",
        provider,
        category,
        code,
        message.chars().count()
    )
}

fn cloud_error_code(message: &str) -> String {
    let Some(status_start) = message.find("status=") else {
        return "cloud_asr_error".to_string();
    };
    let status = message[status_start + "status=".len()..]
        .split_whitespace()
        .next()
        .unwrap_or_default();
    if !status.is_empty() && status.chars().all(|ch| ch.is_ascii_digit()) {
        status.to_string()
    } else {
        "cloud_asr_error".to_string()
    }
}

fn aws_error_category_and_code(
    error: &crate::aws_util::UiAwsError,
) -> (&'static str, &'static str) {
    match error {
        crate::aws_util::UiAwsError::InvalidAccessKey => {
            ("invalid_access_key", "invalid_access_key")
        }
        crate::aws_util::UiAwsError::SignatureMismatch => {
            ("signature_mismatch", "signature_mismatch")
        }
        crate::aws_util::UiAwsError::ExpiredToken => ("expired_token", "expired_token"),
        crate::aws_util::UiAwsError::AccessDenied { .. } => ("access_denied", "access_denied"),
        crate::aws_util::UiAwsError::RegionNotSupported { .. } => {
            ("region_not_supported", "region_not_supported")
        }
        crate::aws_util::UiAwsError::NetworkUnreachable => {
            ("network_unreachable", "network_unreachable")
        }
        crate::aws_util::UiAwsError::Unknown { .. } => ("unknown", "unknown"),
    }
}

fn aws_error_diagnostic(error: &crate::aws_util::UiAwsError, raw_message: &str) -> String {
    let (category, code) = aws_error_category_and_code(error);
    speech_error_diagnostic("aws-transcribe", category, code, raw_message)
}

fn safe_aws_permission(permission: Option<String>) -> Option<String> {
    permission.filter(|permission| {
        !permission.is_empty()
            && permission.len() <= 128
            && permission
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, ':' | '*' | '-' | '_' | '.'))
    })
}

fn aws_error_for_diagnostic_event(
    error: crate::aws_util::UiAwsError,
    diagnostic: &str,
) -> crate::aws_util::UiAwsError {
    match error {
        crate::aws_util::UiAwsError::Unknown { .. } => crate::aws_util::UiAwsError::Unknown {
            message: diagnostic.to_string(),
        },
        crate::aws_util::UiAwsError::AccessDenied { permission } => {
            crate::aws_util::UiAwsError::AccessDenied {
                permission: safe_aws_permission(permission),
            }
        }
        other => other,
    }
}

fn emit_asr_span_revision(app_handle: &AppHandle, payload: events::AsrSpanRevisionPayload) {
    events::emit_or_log(app_handle, events::ASR_SPAN_REVISION, payload);
}

/// Write the ASR stage status, recovering from a poisoned lock the same way
/// the extraction path does (`mod.rs:679`). **Pure** (no Tauri) — testable.
///
/// FA-1: a poisoned `pipeline_status` lock must not silently swallow an error
/// status — recover the inner guard and write through it so the failure is
/// still recorded (and then emitted by the caller).
fn set_asr_status(pipeline_status: &Arc<RwLock<PipelineStatus>>, asr: StageStatus) {
    let mut status = pipeline_status.write().unwrap_or_else(|e| e.into_inner());
    status.asr = asr;
}

/// Emit the current pipeline status to the UI. Best-effort — recovers from a
/// poisoned lock so an error status is never *doubly* lost (FA-1). Mirrors the
/// read+emit pattern at the end of `process_extraction_and_emit`.
fn emit_pipeline_status(app_handle: &AppHandle, pipeline_status: &Arc<RwLock<PipelineStatus>>) {
    let status = pipeline_status.read().unwrap_or_else(|e| e.into_inner());
    let _ = app_handle.emit(events::PIPELINE_STATUS_EVENT, &*status);
}

/// Set the ASR stage status **and** emit the updated pipeline status to the UI
/// (FA-1). Cloud/streaming providers that go to `Error`/`Reconnecting` (or back
/// to `Running` on reconnect) must push the new state to the frontend, else the
/// UI keeps showing the last `Running` snapshot while the provider is dead.
fn set_asr_status_and_emit(
    app_handle: &AppHandle,
    pipeline_status: &Arc<RwLock<PipelineStatus>>,
    asr: StageStatus,
) {
    set_asr_status(pipeline_status, asr);
    emit_pipeline_status(app_handle, pipeline_status);
}

fn source_hint_or_fallback(source_id_hint: &Arc<RwLock<Option<String>>>, fallback: &str) -> String {
    source_id_hint
        .read()
        .ok()
        .and_then(|hint| hint.clone())
        .unwrap_or_else(|| fallback.to_string())
}

#[allow(clippy::too_many_arguments)]
fn emit_asr_partial_with_meta(
    ctx: &TranscriptProcessingContext,
    provider: &str,
    source_id: impl Into<String>,
    text: impl Into<String>,
    start_time: f64,
    end_time: f64,
    confidence: f32,
    meta: AsrRevisionMeta,
) {
    let text = text.into();
    if text.trim().is_empty() {
        return;
    }

    let source_id = source_id.into();
    let span_id = meta
        .span_id
        .unwrap_or_else(|| time_based_span_id(provider, &source_id, start_time, end_time));
    let received_at_ms = current_unix_millis();
    let asr_payload = events::AsrSpanRevisionPayload {
        span_id,
        provider: provider.to_string(),
        source_id: source_id.clone(),
        provider_item_id: meta.provider_item_id,
        transcript_segment_id: None,
        speaker_id: None,
        speaker_label: None,
        channel: meta.channel,
        text: text.clone(),
        start_time,
        end_time,
        confidence,
        is_final: false,
        stability: events::AsrSpanStability::Partial,
        revision_number: meta.revision_number.unwrap_or(1),
        supersedes: meta.supersedes,
        turn_id: meta.turn_id,
        end_of_turn: false,
        raw_event_ref: meta.raw_event_ref,
        capture_latency_ms: meta.capture_latency_ms,
        asr_latency_ms: meta.asr_latency_ms,
        received_at_ms,
    };
    if !record_asr_span_revision_event_and_observe_projection(
        &ctx.transcript_ledger,
        &ctx.transcript_event_writer,
        &ctx.projection_schedulers,
        Some(&ctx.projection_dispatch_context()),
        &asr_payload,
    ) {
        return;
    }
    emit_asr_span_revision(&ctx.app_handle, asr_payload);

    events::emit_or_log(
        &ctx.app_handle,
        events::ASR_PARTIAL,
        events::AsrPartialPayload {
            provider: provider.to_string(),
            source_id,
            text,
            start_time,
            end_time,
            confidence,
            timestamp_ms: received_at_ms,
        },
    );
}

#[derive(Debug, Clone)]
struct TurnEventInput {
    provider: &'static str,
    source_id: String,
    kind: events::TurnEventKind,
    text: Option<String>,
    start_time: Option<f64>,
    end_time: Option<f64>,
    confidence: Option<f32>,
    turn_index: Option<u64>,
}

/// Emit a provider-neutral speech turn lifecycle event.
fn emit_turn_event(app_handle: &AppHandle, input: TurnEventInput) {
    events::emit_or_log(
        app_handle,
        events::TURN_EVENT,
        events::TurnEventPayload {
            provider: input.provider.to_string(),
            source_id: input.source_id,
            kind: input.kind,
            text: input.text,
            start_time: input.start_time,
            end_time: input.end_time,
            confidence: input.confidence,
            turn_index: input.turn_index,
            timestamp_ms: current_unix_millis(),
        },
    );
}

fn emit_agent_status(
    app_handle: &AppHandle,
    state: events::AgentStatusState,
    source_segment_id: Option<&str>,
    message: Option<&str>,
) {
    events::emit_or_log(
        app_handle,
        events::AGENT_STATUS,
        events::AgentStatusPayload {
            state,
            source_segment_id: source_segment_id.map(str::to_string),
            message: message.map(str::to_string),
            timestamp_ms: current_unix_millis(),
        },
    );
}

fn agent_proposal_kind(text: &str) -> Option<events::AgentProposalKind> {
    let lower = text.to_lowercase();
    if text.trim_end().ends_with('?')
        || lower.starts_with("who ")
        || lower.starts_with("what ")
        || lower.starts_with("when ")
        || lower.starts_with("where ")
        || lower.starts_with("why ")
        || lower.starts_with("how ")
    {
        return Some(events::AgentProposalKind::Question);
    }
    if lower.contains("follow up")
        || lower.contains("action item")
        || lower.contains("todo")
        || lower.contains("decide")
        || lower.contains("decision")
    {
        return Some(events::AgentProposalKind::GraphSuggestion);
    }
    if lower.contains("note that") || lower.contains("remember") || lower.contains("important") {
        return Some(events::AgentProposalKind::Note);
    }
    None
}

fn agent_proposal_title(kind: &events::AgentProposalKind, speaker: &str) -> String {
    match kind {
        events::AgentProposalKind::Question => format!("Question from {}", speaker),
        events::AgentProposalKind::GraphSuggestion => "Possible graph update".to_string(),
        events::AgentProposalKind::Note => format!("Context from {}", speaker),
    }
}

fn agent_proposal_body(kind: &events::AgentProposalKind, text: &str) -> String {
    match kind {
        events::AgentProposalKind::Question => {
            format!("Consider answering or linking this question: {}", text)
        }
        events::AgentProposalKind::GraphSuggestion => {
            format!(
                "Review this for an action item, decision, or relationship: {}",
                text
            )
        }
        events::AgentProposalKind::Note => format!("Keep this context available: {}", text),
    }
}

fn prune_pending_agent_proposals(pending: &mut HashMap<String, events::AgentProposalPayload>) {
    if pending.len() <= MAX_PENDING_AGENT_PROPOSALS {
        return;
    }

    let mut ids_by_age: Vec<(String, u64)> = pending
        .iter()
        .map(|(id, proposal)| (id.clone(), proposal.created_at_ms))
        .collect();
    ids_by_age.sort_by_key(|(_, created_at_ms)| *created_at_ms);
    let remove_count = pending.len().saturating_sub(MAX_PENDING_AGENT_PROPOSALS);
    for (id, _) in ids_by_age.into_iter().take(remove_count) {
        pending.remove(&id);
    }
}

fn spawn_agent_proposal_task(
    segment: TranscriptSegment,
    session_id: String,
    source_span_id: String,
    app_handle: AppHandle,
    pending_agent_proposals: Arc<Mutex<HashMap<String, events::AgentProposalPayload>>>,
) {
    let text = segment.text.trim().to_string();
    if text.is_empty() || text == "[speech]" {
        return;
    }

    agent_pool().spawn(move || {
        let start = Instant::now();
        emit_agent_status(
            &app_handle,
            events::AgentStatusState::Running,
            Some(&segment.id),
            Some("Reviewing transcript segment"),
        );

        let speaker = segment.speaker_label.as_deref().unwrap_or("Unknown");
        let Some(kind) = agent_proposal_kind(&text) else {
            emit_stage_latency(
                &app_handle,
                "agent",
                Some(&segment.source_id),
                Some(&segment.id),
                start.elapsed(),
            );
            emit_agent_status(
                &app_handle,
                events::AgentStatusState::Idle,
                Some(&segment.id),
                None,
            );
            return;
        };
        let confidence = if segment.confidence.is_finite() {
            segment.confidence.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let proposal = events::AgentProposalPayload {
            id: uuid::Uuid::new_v4().to_string(),
            source_segment_id: segment.id.clone(),
            source_id: segment.source_id.clone(),
            speaker_label: segment.speaker_label.clone(),
            title: agent_proposal_title(&kind, speaker),
            body: agent_proposal_body(&kind, &text),
            kind,
            confidence,
            created_at_ms: current_unix_millis(),
        };

        match pending_agent_proposals.lock() {
            Ok(mut pending) => {
                pending.insert(proposal.id.clone(), proposal.clone());
                prune_pending_agent_proposals(&mut pending);
            }
            Err(err) => {
                log::warn!("Failed to store pending agent proposal: {}", err);
                emit_agent_status(
                    &app_handle,
                    events::AgentStatusState::Error,
                    Some(&segment.id),
                    Some("Could not store agent proposal"),
                );
                return;
            }
        }

        let live_card = events::LiveAssistCardRecord {
            session_id: session_id.clone(),
            proposal: proposal.clone(),
            status: events::LiveAssistCardStatus::Pending,
            source_span_ids: vec![source_span_id],
            graph_context_ids: Vec::new(),
            outcome: None,
            projection_patch_sequence: None,
            created_at_ms: proposal.created_at_ms,
            updated_at_ms: proposal.created_at_ms,
        };
        if let Err(err) =
            FileMemoryRepository::user_data().upsert_live_assist_card(&session_id, &live_card)
        {
            log::warn!(
                "Failed to persist live assist card {}: {}",
                proposal.id,
                err
            );
        }

        events::emit_or_log(&app_handle, events::AGENT_PROPOSAL, proposal);
        emit_stage_latency(
            &app_handle,
            "agent",
            Some(&segment.source_id),
            Some(&segment.id),
            start.elapsed(),
        );
        emit_agent_status(
            &app_handle,
            events::AgentStatusState::Idle,
            Some(&segment.id),
            None,
        );
    });
}

// ---------------------------------------------------------------------------
// Accumulated speech segment (replaces the old VAD-produced SpeechSegment)
// ---------------------------------------------------------------------------

/// A segment of speech audio accumulated from the processed audio pipeline.
///
/// The speech processor accumulates `ProcessedAudioChunk`s into ~2 second
/// segments for better Whisper transcription quality (individual 32ms chunks
/// are too short for coherent speech recognition).
#[derive(Debug, Clone)]
pub(crate) struct AccumulatedSegment {
    /// Identifier of the audio source that produced this segment.
    pub source_id: String,
    /// 16kHz mono f32 audio data for the segment.
    pub audio: Vec<f32>,
    /// Start time relative to stream start.
    pub start_time: Duration,
    /// End time relative to stream start.
    pub end_time: Duration,
    /// Number of audio frames (equal to `audio.len()`).
    pub num_frames: usize,
}

/// Target number of frames per accumulated segment (~2 seconds at 16kHz).
const TARGET_FRAMES: usize = 16_000 * 2;

/// Number of frames to retain as overlap between consecutive segments (~0.5s at 16kHz).
/// This ensures words at segment boundaries are captured in both adjacent segments.
const OVERLAP_FRAMES: usize = 16_000 / 2;

// ---------------------------------------------------------------------------
// Diarization config helper
// ---------------------------------------------------------------------------

/// Build the best available `DiarizationConfig` for the given models directory.
///
/// Backend selection (highest available first):
/// 1. **Clustering** (sherpa-onnx, unbounded) when the `diarization-clustering`
///    feature is compiled in *and* both the pyannote segmentation + embedding
///    ONNX models exist on disk (ADR-0017 / B16). The live engine is
///    `diarization::worker::LiveDiarizationWorker`, spawned + fed separately —
///    see [`maybe_spawn_clustering_diarization`].
/// 2. **Sortformer** (parakeet-rs, ≤4 speakers) when the `diarization` feature
///    is compiled in and the Sortformer ONNX file exists.
/// 3. **Simple** signal-based fallback otherwise.
///
/// Clustering and Sortformer are mutually exclusive at build time (ORT link
/// conflict, enforced in `lib.rs`), so at most one neural branch is reachable.
fn make_diarization_config(models_dir: &std::path::Path) -> DiarizationConfig {
    #[cfg(feature = "diarization-clustering")]
    {
        let seg = models_dir
            .join(crate::models::DIAR_SEG_PYANNOTE_DIR)
            .join(crate::models::DIAR_SEG_PYANNOTE_FILE);
        let emb = models_dir.join(crate::models::DIAR_EMB_TITANET_FILENAME);
        if seg.exists() && emb.exists() {
            log::info!(
                "Clustering diarization models found (seg='{}', emb='{}') — using unbounded \
                 sherpa-onnx clustering backend (ADR-0017).",
                seg.display(),
                emb.display()
            );
            return DiarizationConfig::clustering(
                seg,
                emb,
                crate::diarization::clustering::DEFAULT_CLUSTERING_THRESHOLD,
            );
        }
        log::info!(
            "Clustering diarization models not found (seg='{}', emb='{}') — falling back. \
             Download via Settings → Models for unbounded speaker identification.",
            seg.display(),
            emb.display()
        );
    }

    let sortformer_path = models_dir.join(SORTFORMER_MODEL_FILENAME);

    if sortformer_path.exists() {
        log::info!(
            "Sortformer model found at '{}' — using neural diarization backend",
            sortformer_path.display()
        );
        DiarizationConfig::sortformer(sortformer_path)
    } else {
        log::info!(
            "Sortformer model not found at '{}' — using Simple diarization backend. \
             Download via Settings → Models for improved speaker identification.",
            sortformer_path.display()
        );
        DiarizationConfig::default()
    }
}

// ---------------------------------------------------------------------------
// Live clustering diarization wiring (ADR-0017 / B16-pipe)
// ---------------------------------------------------------------------------
//
// The clustering backend's live engine is `LiveDiarizationWorker`: an offline
// re-diarizer run on a rolling window on a dedicated thread, fed a 16 kHz mono
// audio tap via an SPSC ring (the producer side, `DiarizationFeed`). Unlike the
// per-utterance Simple/Sortformer `DiarizationWorker`, it owns its own thread +
// emission, so the accumulator/ASR loops just push their already-16 kHz-mono
// audio into the feed. The worker emits window-local `StableSegment`s; the
// consumer thread lifts them to absolute session time, maps them onto transcript
// times by overlap, and emits `SPEAKER_DETECTED` (mirroring speech/mod.rs:597).
//
// `buffer_start_abs` (session seconds at the rolling buffer's leading edge) is
// tracked from the cumulative count of samples ever fed minus the live worker's
// bounded window, so window-local times convert to session times exactly as the
// research "rolling window" note prescribes (`abs = buffer_start_abs + local`).

/// Sample rate of the audio fed to the live clustering diarizer (16 kHz mono).
#[cfg(feature = "diarization-clustering")]
const CLUSTERING_FEED_SR: u32 = 16_000;
/// How many recent session speaker-spans to retain for transcript overlap
/// labeling (bounds memory over a long session; a transcript segment only ever
/// overlaps very recent spans).
#[cfg(feature = "diarization-clustering")]
const CLUSTERING_SPAN_HISTORY: usize = 512;

/// Handle bundling a spawned live clustering diarizer: the audio feed (push
/// 16 kHz mono into it), the cooperative stop flag, the shared session-time span
/// registry (for transcript overlap-labeling), and the worker/consumer join
/// handles. Held by the speech processor for the session's duration.
#[cfg(feature = "diarization-clustering")]
pub(crate) struct ClusteringDiarizationHandle {
    feed: crate::diarization::worker::DiarizationFeed,
    /// Session-time speaker spans, kept fresh by the consumer thread; read here
    /// to label transcript segments by time overlap.
    spans: Arc<RwLock<VecDeque<crate::diarization::SessionSpeakerSpan>>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    _worker: std::thread::JoinHandle<()>,
    _consumer: std::thread::JoinHandle<()>,
}

#[cfg(feature = "diarization-clustering")]
impl ClusteringDiarizationHandle {
    /// Push a chunk of 16 kHz mono f32 audio (the same data the ASR path sees)
    /// into the diarization ring. Never blocks (the worker drops + counts on a
    /// full ring). The worker stamps each emitted span's absolute window-start
    /// sample itself (B16-offset), so no fed-sample bookkeeping is needed here.
    pub(crate) fn push(&mut self, samples: &[f32]) {
        self.feed.push(samples);
    }

    /// Look up the best-overlapping global speaker for a transcript segment
    /// (absolute session seconds) and return its `(speaker_id, speaker_label)`,
    /// or `None` when no diarization span overlaps yet (the offline diarizer lags
    /// live audio by up to a window). Pure overlap-mapping via
    /// [`crate::diarization::overlap_speaker_for_segment`].
    pub(crate) fn label_segment(&self, start_time: f64, end_time: f64) -> Option<(String, String)> {
        let spans = self.spans.read().ok()?;
        let slice: Vec<_> = spans.iter().copied().collect();
        drop(spans);
        let gid = crate::diarization::overlap_speaker_for_segment(start_time, end_time, &slice)?;
        Some((
            crate::diarization::clustering_speaker_id(gid),
            crate::diarization::clustering_speaker_label(gid),
        ))
    }

    /// Signal the worker + consumer to drain once more and exit.
    pub(crate) fn stop(&self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// If the configured backend is `Clustering`, build + spawn the live
/// [`LiveDiarizationWorker`] and its `SPEAKER_DETECTED`-emitting consumer thread,
/// returning a handle the caller feeds 16 kHz mono audio into. Returns `None`
/// for any other backend (the per-utterance `DiarizationWorker` handles those)
/// or if the worker fails to construct (logged; the Simple path still runs).
#[cfg(feature = "diarization-clustering")]
pub(crate) fn maybe_spawn_clustering_diarization(
    diarization_config: &DiarizationConfig,
    app_handle: AppHandle,
    speaker_timeline: Arc<Mutex<SpeakerTimeline>>,
    knowledge_graph: Arc<Mutex<TemporalKnowledgeGraph>>,
    graph_snapshot: Arc<RwLock<GraphSnapshot>>,
) -> Option<ClusteringDiarizationHandle> {
    use crate::diarization::DiarizationBackend;
    use crate::diarization::worker::{
        DEFAULT_HOP_SECS, DEFAULT_MIN_START_SECS, DEFAULT_WINDOW_SECS, LiveDiarizationWorker,
        StableSegment,
    };

    let (segmentation_model, embedding_model, threshold) = match &diarization_config.backend {
        DiarizationBackend::Clustering {
            segmentation_model,
            embedding_model,
            threshold,
        } => (segmentation_model, embedding_model, *threshold),
        _ => return None,
    };

    let (worker, feed) = match LiveDiarizationWorker::new(
        segmentation_model,
        embedding_model,
        threshold,
        DEFAULT_WINDOW_SECS,
        DEFAULT_HOP_SECS,
        DEFAULT_MIN_START_SECS,
    ) {
        Ok(pair) => pair,
        Err(e) => {
            log::warn!(
                "Clustering diarization: failed to build live worker ({e}); \
                 speaker labels disabled for this session."
            );
            return None;
        }
    };

    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let spans = Arc::new(RwLock::new(VecDeque::<
        crate::diarization::SessionSpeakerSpan,
    >::new()));
    let (seg_tx, seg_rx) = crossbeam_channel::unbounded::<StableSegment>();
    let worker_handle = worker.spawn(seg_tx, stop.clone());

    // Consumer thread: lift each StableSegment to absolute session time (using the
    // worker-stamped window_start_sample — exact, no fed-sample reconstruction),
    // record it in the shared span registry for transcript overlap-labeling, and
    // emit SPEAKER_DETECTED with running per-speaker stats (mirrors speech/mod.rs:597).
    let consumer_handle = match std::thread::Builder::new()
        .name("diarization-clustering-emit".to_string())
        .spawn({
            let spans = spans.clone();
            move || {
                run_clustering_emit_loop(
                    seg_rx,
                    app_handle,
                    speaker_timeline,
                    knowledge_graph,
                    graph_snapshot,
                    spans,
                );
            }
        }) {
        Ok(handle) => handle,
        Err(e) => {
            // Spawn failed (e.g. OS thread-limit exhaustion). Don't abort the
            // whole session — disable speaker labels gracefully. Signal the
            // already-spawned live worker to stop so it doesn't run headless,
            // then return None (the Simple per-utterance path still runs).
            log::warn!(
                "Clustering diarization: failed to spawn emit consumer thread ({e}); \
                 speaker labels disabled for this session."
            );
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
            // `seg_rx` was moved into the failed spawn closure and is now dropped,
            // disconnecting the worker's segment channel as a second stop signal.
            let _ = worker_handle.join();
            return None;
        }
    };

    log::info!(
        "Clustering diarization: live worker + emit consumer spawned (window={DEFAULT_WINDOW_SECS}s, \
         hop={DEFAULT_HOP_SECS}s, threshold={threshold})."
    );

    Some(ClusteringDiarizationHandle {
        feed,
        spans,
        stop,
        _worker: worker_handle,
        _consumer: consumer_handle,
    })
}

/// Consume stabilized window-local diarization spans: lift to absolute session
/// time, record for transcript overlap-labeling, and emit `SPEAKER_DETECTED`.
///
/// `StableSegment.start`/`end` are **window-local** seconds, but each segment now
/// carries `window_start_sample` — the worker's own absolute ingested-sample index
/// of the window's first sample, stamped at diarize time (B16-offset). So
/// `buffer_start_abs = window_start_sample / sr` is **exact** (precise even under
/// backpressure), and (research "rolling window") `abs = buffer_start_abs + local`
/// via [`crate::diarization::window_local_to_session_span`]. Spans are pushed into
/// the shared registry (bounded to `CLUSTERING_SPAN_HISTORY`) so the ASR loop can
/// map transcript times onto them by overlap.
#[cfg(feature = "diarization-clustering")]
fn run_clustering_emit_loop(
    seg_rx: crossbeam_channel::Receiver<crate::diarization::worker::StableSegment>,
    app_handle: AppHandle,
    speaker_timeline: Arc<Mutex<SpeakerTimeline>>,
    knowledge_graph: Arc<Mutex<TemporalKnowledgeGraph>>,
    graph_snapshot: Arc<RwLock<GraphSnapshot>>,
    spans: Arc<RwLock<VecDeque<crate::diarization::SessionSpeakerSpan>>>,
) {
    let mut stats = crate::diarization::ClusteringSpeakerStats::new();
    let event_sink = TauriDiarizationEventSink {
        app_handle: &app_handle,
    };
    let diarization_dispatch = DiarizationDispatchContext {
        event_sink: &event_sink,
        speaker_timeline: &speaker_timeline,
        knowledge_graph: &knowledge_graph,
        graph_snapshot: &graph_snapshot,
    };
    log::info!("Clustering diarization emit loop: entering");
    while let Ok(seg) = seg_rx.recv() {
        // Exact absolute session time of the window's leading edge, stamped by the
        // worker at diarize time (no producer-side reconstruction → no backpressure
        // skew). The worker guarantees window_start_sample aligns with seg.start=0.
        let buffer_start_abs = seg.window_start_sample as f64 / CLUSTERING_FEED_SR as f64;

        let session_span = crate::diarization::window_local_to_session_span(
            seg.start,
            seg.end,
            buffer_start_abs,
            seg.global_speaker,
        );
        let (speaker_id, speaker_label) =
            if seg.global_speaker == crate::diarization::stabilize::UNKNOWN_SPEAKER {
                (None, None)
            } else {
                (
                    Some(crate::diarization::clustering_speaker_id(
                        seg.global_speaker,
                    )),
                    Some(crate::diarization::clustering_speaker_label(
                        seg.global_speaker,
                    )),
                )
            };

        if let Ok(mut q) = spans.write() {
            q.push_back(session_span);
            while q.len() > CLUSTERING_SPAN_HISTORY {
                q.pop_front();
            }
        }

        emit_and_dispatch_diarization_span_revision(
            &diarization_dispatch,
            events::DiarizationSpanRevisionPayload {
                span_id: diarization_span_revision_id(
                    "local_clustering",
                    "session",
                    session_span.start,
                    session_span.end,
                    speaker_id.as_deref(),
                ),
                provider: "local_clustering".to_string(),
                timeline_id: "session".to_string(),
                source_id: None,
                speaker_id: speaker_id.clone(),
                speaker_label: speaker_label.clone(),
                channel: None,
                start_time: session_span.start,
                end_time: session_span.end,
                confidence: None,
                is_final: false,
                stability: events::DiarizationSpanStability::Provisional,
                revision_number: 1,
                supersedes: None,
                basis_asr_span_ids: Vec::new(),
                basis_transcript_segment_ids: Vec::new(),
                raw_event_ref: Some(format!("window_start_sample:{}", seg.window_start_sample)),
                capture_latency_ms: None,
                asr_latency_ms: None,
                received_at_ms: current_unix_millis(),
            },
        );

        let duration = (seg.end - seg.start).max(0.0) as f64;
        if let Some(info) = stats.record(seg.global_speaker, duration) {
            let _ = app_handle.emit(events::SPEAKER_DETECTED, &info);
            log::debug!(
                "Clustering diarization: SPEAKER_DETECTED {} (segments={}, total={:.1}s)",
                info.label,
                info.segment_count,
                info.total_speaking_time,
            );
        }
    }
    log::info!(
        "Clustering diarization emit loop: channel closed, exiting ({} speaker(s) seen)",
        stats.len()
    );
}

// ---------------------------------------------------------------------------
// Helper: extraction + graph update + event emission (I1: deduplicated)
// ---------------------------------------------------------------------------

/// Perform entity extraction, update the knowledge graph, and emit events.
///
/// Shared by both the full (ASR + diarization) and diarization-only speech
/// processor loops. LLM-backed extraction runs through the priority executor,
/// with rule-based extraction as the final fallback.
#[allow(clippy::too_many_arguments)]
pub(crate) fn process_extraction_and_emit(
    text: &str,
    speaker: &str,
    context: &str,
    segment_id: &str,
    timestamp: f64,
    deps: &ExtractionDeps<'_>,
    extraction_count: &mut u64,
    graph_update_count: &mut u64,
) {
    let extraction_result = deps
        .llm_executor
        .extract_entities_with_policy(
            text.to_string(),
            speaker.to_string(),
            context.to_string(),
            (*deps.llm_provider).clone(),
            LlmPriority::Background,
            deps.llm_allow_cloud_fallbacks,
        )
        .unwrap_or_else(|| deps.graph_extractor.extract(speaker, text));

    *extraction_count += 1;

    // Feed extraction into the knowledge graph
    if !extraction_result.entities.is_empty() {
        let graph_start = Instant::now();
        {
            let mut graph = deps.knowledge_graph.lock().unwrap_or_else(|e| {
                log::warn!("Knowledge graph mutex poisoned, recovering: {}", e);
                e.into_inner()
            });
            graph.process_extraction(&extraction_result, timestamp, speaker, segment_id);

            *graph_update_count += 1;

            // Emit delta update (every extraction cycle — lightweight)
            if graph.has_delta() {
                let delta = graph.take_delta();
                let _ = deps.app_handle.emit(crate::events::GRAPH_DELTA, &delta);
                log::debug!(
                    "Graph delta emitted: +{} nodes, ~{} updated, +{} edges, -{} nodes, -{} edges",
                    delta.added_nodes.len(),
                    delta.updated_nodes.len(),
                    delta.added_edges.len(),
                    delta.removed_node_ids.len(),
                    delta.removed_edge_ids.len(),
                );
            }

            // Emit full snapshot less frequently (every 10th update)
            if (*graph_update_count).is_multiple_of(10) {
                let snapshot = graph.snapshot();
                if let Ok(mut gs) = deps.graph_snapshot.write() {
                    *gs = snapshot.clone();
                }
                let _ = deps.app_handle.emit(crate::events::GRAPH_UPDATE, &snapshot);
                log::debug!(
                    "Graph full snapshot emitted: {} nodes, {} edges (update #{})",
                    snapshot.stats.total_nodes,
                    snapshot.stats.total_edges,
                    graph_update_count,
                );
            } else {
                // Still update the cached snapshot (for Tauri commands that read it)
                let snapshot = graph.snapshot();
                if let Ok(mut gs) = deps.graph_snapshot.write() {
                    *gs = snapshot;
                }
            }
        }
        emit_stage_latency(
            deps.app_handle,
            "graph",
            None,
            Some(segment_id),
            graph_start.elapsed(),
        );
    }

    // Update entity_extraction and graph status, then emit pipeline status
    if let Ok(mut status) = deps.pipeline_status.write() {
        status.entity_extraction = StageStatus::Running {
            processed_count: *extraction_count,
        };
        status.graph = StageStatus::Running {
            processed_count: *graph_update_count,
        };
    }
    if let Ok(status) = deps.pipeline_status.read() {
        let _ = deps
            .app_handle
            .emit(events::PIPELINE_STATUS_EVENT, &*status);
    }
}

// ---------------------------------------------------------------------------
// Shared post-transcription tail pipeline
// ---------------------------------------------------------------------------

/// Shared dependencies for post-transcription processing across all ASR workers.
///
/// Every ASR worker — local Whisper, cloud batch, Deepgram/AssemblyAI/AWS
/// streaming, sherpa-onnx streaming — runs an identical tail once it has a
/// final `TranscriptSegment`: buffer + persist + emit + status + extract.
/// Collecting these dependencies in one struct lets that tail live in
/// [`emit_transcript_and_extract_with_meta`] instead of being copied six times.
#[derive(Clone)]
pub(crate) struct TranscriptProcessingContext {
    pub asr_provider: &'static str,
    pub transcript_buffer: Arc<RwLock<VecDeque<TranscriptSegment>>>,
    pub transcript_writer: Arc<Mutex<Option<crate::persistence::TranscriptWriter>>>,
    pub transcript_event_writer: Arc<Mutex<Option<crate::persistence::TranscriptEventWriter>>>,
    pub transcript_ledger: Arc<Mutex<crate::projections::TranscriptLedger>>,
    pub speaker_timeline: Arc<Mutex<SpeakerTimeline>>,
    pub projection_schedulers: Arc<Mutex<crate::projection_scheduler::ProjectionSchedulers>>,
    pub projection_runtime: ProjectionRuntimeHandle,
    pub pipeline_status: Arc<RwLock<PipelineStatus>>,
    pub app_handle: AppHandle,
    pub llm_engine: Arc<Mutex<Option<LlmEngine>>>,
    pub api_client: Arc<Mutex<Option<ApiClient>>>,
    pub mistralrs_engine: Arc<Mutex<Option<MistralRsEngine>>>,
    pub llm_executor: LlmExecutor,
    pub llm_provider: LlmProvider,
    pub llm_allow_cloud_fallbacks: bool,
    pub graph_extractor: Arc<RuleBasedExtractor>,
    pub knowledge_graph: Arc<Mutex<TemporalKnowledgeGraph>>,
    pub graph_snapshot: Arc<RwLock<GraphSnapshot>>,
    pub pending_agent_proposals: Arc<Mutex<HashMap<String, events::AgentProposalPayload>>>,
    /// Coalescing buffer: consecutive same-speaker segments accumulate here and
    /// are flushed to extraction as one batch (see `coalesce_submit`).
    pub pending_extraction: Arc<Mutex<Option<PendingBatch>>>,
}

#[derive(Clone)]
struct ProjectionDispatchContext {
    transcript_ledger: Arc<Mutex<crate::projections::TranscriptLedger>>,
    projection_schedulers: Arc<Mutex<crate::projection_scheduler::ProjectionSchedulers>>,
    projection_runtime: ProjectionRuntimeHandle,
    event_sink: Arc<dyn ProjectionRuntimeEventSink>,
    patch_generator: Arc<dyn ProjectionPatchGenerator>,
}

trait ProjectionPatchGenerator: Send + Sync {
    fn generate_projection_patch(
        &self,
        job: ProjectionJob,
        ledger: TranscriptLedger,
        sequence: u64,
        created_at_ms: u64,
    ) -> Result<ProjectionPatchOutcome, String>;
}

trait ProjectionRuntimeEventSink: Send + Sync {
    fn emit_projection_patch(&self, patch: &ProjectionPatch);
    fn emit_materialized_notes(&self, notes: &MaterializedNotes);
    fn emit_materialized_graph(&self, graph: &MaterializedGraph);
}

struct TauriProjectionRuntimeEventSink {
    app_handle: AppHandle,
}

impl ProjectionRuntimeEventSink for TauriProjectionRuntimeEventSink {
    fn emit_projection_patch(&self, patch: &ProjectionPatch) {
        events::emit_or_log(&self.app_handle, events::PROJECTION_PATCH, patch.clone());
    }

    fn emit_materialized_notes(&self, notes: &MaterializedNotes) {
        events::emit_or_log(
            &self.app_handle,
            events::MATERIALIZED_NOTES_UPDATE,
            notes.clone(),
        );
    }

    fn emit_materialized_graph(&self, graph: &MaterializedGraph) {
        events::emit_or_log(
            &self.app_handle,
            events::MATERIALIZED_GRAPH_UPDATE,
            graph.clone(),
        );
    }
}

struct ExecutorProjectionPatchGenerator {
    llm_executor: LlmExecutor,
    llm_provider: LlmProvider,
    allow_cloud_fallbacks: bool,
}

impl ProjectionPatchGenerator for ExecutorProjectionPatchGenerator {
    fn generate_projection_patch(
        &self,
        job: ProjectionJob,
        ledger: TranscriptLedger,
        sequence: u64,
        created_at_ms: u64,
    ) -> Result<ProjectionPatchOutcome, String> {
        self.llm_executor.generate_projection_patch_with_policy(
            job,
            ledger,
            self.llm_provider.clone(),
            sequence,
            created_at_ms,
            self.allow_cloud_fallbacks,
        )
    }
}

impl TranscriptProcessingContext {
    fn projection_dispatch_context(&self) -> ProjectionDispatchContext {
        ProjectionDispatchContext {
            transcript_ledger: self.transcript_ledger.clone(),
            projection_schedulers: self.projection_schedulers.clone(),
            projection_runtime: self.projection_runtime.clone(),
            event_sink: Arc::new(TauriProjectionRuntimeEventSink {
                app_handle: self.app_handle.clone(),
            }),
            patch_generator: Arc::new(ExecutorProjectionPatchGenerator {
                llm_executor: self.llm_executor.clone(),
                llm_provider: self.llm_provider.clone(),
                allow_cloud_fallbacks: self.llm_allow_cloud_fallbacks,
            }),
        }
    }
}

/// Build a `TranscriptProcessingContext` from the shared state + LLM provider.
/// Every downstream worker consumes this to drive the buffer/persist/emit/
/// extract tail — converting here keeps the workers free of the 11-field
/// struct literal.
fn shared_to_transcript_context(
    shared: SpeechShared,
    llm_provider: LlmProvider,
    llm_allow_cloud_fallbacks: bool,
    asr_provider: &'static str,
) -> TranscriptProcessingContext {
    TranscriptProcessingContext {
        asr_provider,
        transcript_buffer: shared.transcript_buffer,
        transcript_writer: shared.transcript_writer,
        transcript_event_writer: shared.transcript_event_writer,
        transcript_ledger: shared.transcript_ledger,
        speaker_timeline: shared.speaker_timeline,
        projection_schedulers: shared.projection_schedulers,
        projection_runtime: shared.projection_runtime,
        pipeline_status: shared.pipeline_status,
        app_handle: shared.app_handle,
        llm_engine: shared.llm_engine,
        api_client: shared.api_client,
        mistralrs_engine: shared.mistralrs_engine,
        llm_executor: shared.llm_executor,
        llm_provider,
        llm_allow_cloud_fallbacks,
        graph_extractor: shared.graph_extractor,
        knowledge_graph: shared.knowledge_graph,
        graph_snapshot: shared.graph_snapshot,
        pending_agent_proposals: shared.pending_agent_proposals,
        pending_extraction: Arc::new(Mutex::new(None)),
    }
}

fn record_asr_span_revision_event(
    transcript_ledger: &Arc<Mutex<crate::projections::TranscriptLedger>>,
    transcript_event_writer: &Arc<Mutex<Option<crate::persistence::TranscriptEventWriter>>>,
    payload: &events::AsrSpanRevisionPayload,
) -> bool {
    let transcript_event = crate::projections::TranscriptEvent::from(payload.clone());
    let mut ledger = match transcript_ledger.lock() {
        Ok(ledger) => ledger,
        Err(poisoned) => {
            log::warn!("Transcript ledger lock poisoned; recovering");
            poisoned.into_inner()
        }
    };
    let mut next_ledger = ledger.clone();
    match next_ledger.apply_event(transcript_event.clone()) {
        Ok(()) => {}
        Err(e) => {
            log::warn!(
                "Transcript ledger rejected span revision span_id={} revision={} error={:?}",
                transcript_event.span_id,
                transcript_event.revision_number,
                e
            );
            return false;
        }
    }
    match transcript_event_writer.lock() {
        Ok(writer_guard) => {
            if let Some(ref writer) = *writer_guard
                && !writer.append(&transcript_event)
            {
                log::warn!(
                    "Transcript event writer rejected span revision span_id={} revision={}; ledger was not advanced",
                    transcript_event.span_id,
                    transcript_event.revision_number
                );
                return false;
            }
        }
        Err(poisoned) => {
            log::warn!("Transcript event writer lock poisoned; recovering before ledger advance");
            let writer_guard = poisoned.into_inner();
            match writer_guard.as_ref() {
                Some(writer) => {
                    if !writer.append(&transcript_event) {
                        log::warn!(
                            "Transcript event writer rejected span revision span_id={} revision={} after poisoned-lock recovery; ledger was not advanced",
                            transcript_event.span_id,
                            transcript_event.revision_number
                        );
                        return false;
                    }
                }
                None => {
                    log::warn!(
                        "Transcript event writer lock poisoned with no recoverable writer for span_id={} revision={}; ledger was not advanced",
                        transcript_event.span_id,
                        transcript_event.revision_number
                    );
                    return false;
                }
            }
        }
    }
    *ledger = next_ledger;
    true
}

fn record_asr_span_revision_event_and_observe_projection(
    transcript_ledger: &Arc<Mutex<crate::projections::TranscriptLedger>>,
    transcript_event_writer: &Arc<Mutex<Option<crate::persistence::TranscriptEventWriter>>>,
    projection_schedulers: &Arc<Mutex<crate::projection_scheduler::ProjectionSchedulers>>,
    projection_dispatch: Option<&ProjectionDispatchContext>,
    payload: &events::AsrSpanRevisionPayload,
) -> bool {
    if !record_asr_span_revision_event(transcript_ledger, transcript_event_writer, payload) {
        return false;
    }
    observe_projection_schedulers_for_asr_revision(
        transcript_ledger,
        projection_schedulers,
        projection_dispatch,
        payload,
    );
    true
}

fn observe_projection_schedulers_for_asr_revision(
    transcript_ledger: &Arc<Mutex<crate::projections::TranscriptLedger>>,
    projection_schedulers: &Arc<Mutex<crate::projection_scheduler::ProjectionSchedulers>>,
    projection_dispatch: Option<&ProjectionDispatchContext>,
    payload: &events::AsrSpanRevisionPayload,
) {
    if !(payload.is_final
        || payload.end_of_turn
        || matches!(payload.stability, events::AsrSpanStability::Final))
    {
        return;
    }

    let observation = {
        let ledger = match transcript_ledger.lock() {
            Ok(ledger) => ledger,
            Err(poisoned) => {
                log::warn!(
                    "Transcript ledger lock poisoned during projection scheduling; recovering"
                );
                poisoned.into_inner()
            }
        };
        let mut schedulers = match projection_schedulers.lock() {
            Ok(schedulers) => schedulers,
            Err(poisoned) => {
                log::warn!("Projection scheduler lock poisoned; recovering");
                poisoned.into_inner()
            }
        };
        let observation = schedulers.observe_ledger(&ledger, current_unix_millis());
        log::debug!(
            "projection_schedulers.observe_asr_revision span_id={} revision={} notes={:?} graph={:?}",
            payload.span_id,
            payload.revision_number,
            observation.notes,
            observation.graph
        );
        observation
    };
    if let Some(dispatch) = projection_dispatch {
        dispatch_projection_observation(dispatch.clone(), observation);
    }
}

fn dispatch_projection_observation(
    dispatch: ProjectionDispatchContext,
    observation: ProjectionSchedulersObservation,
) {
    dispatch_projection_decision(dispatch.clone(), observation.notes);
    dispatch_projection_decision(dispatch, observation.graph);
}

fn dispatch_projection_decision(
    dispatch: ProjectionDispatchContext,
    decision: ProjectionSchedulerDecision,
) {
    match decision {
        ProjectionSchedulerDecision::StartJob { job }
        | ProjectionSchedulerDecision::CompletedAndStartedFollowUp { job, .. }
        | ProjectionSchedulerDecision::DiscardedStaleAndStartedRepair { job, .. }
        | ProjectionSchedulerDecision::FailedStaleAndStartedRepair { job, .. } => {
            spawn_projection_job(dispatch, job);
        }
        ProjectionSchedulerDecision::Idle
        | ProjectionSchedulerDecision::Coalesced { .. }
        | ProjectionSchedulerDecision::CompletedCurrent { .. }
        | ProjectionSchedulerDecision::DiscardedStaleNoCurrentBasis { .. }
        | ProjectionSchedulerDecision::FailedCurrent { .. }
        | ProjectionSchedulerDecision::FailedStaleNoCurrentBasis { .. } => {}
    }
}

#[derive(Debug, Clone, Copy)]
enum ProjectionJobCompletion {
    Completed,
    Failed,
}

fn spawn_projection_job(dispatch: ProjectionDispatchContext, job: ProjectionJob) {
    let failure_dispatch = dispatch.clone();
    let failure_kind = job.kind.clone();
    let job_id = job.id.clone();
    let thread_name = format!("projection-{}", projection_kind_key(&job.kind));
    match std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || run_projection_job(dispatch, job))
    {
        Ok(_) => {}
        Err(error) => {
            log::error!(
                "Failed to spawn projection job thread job_id={} error={}",
                job_id,
                error
            );
            finish_projection_scheduler_job(
                failure_dispatch,
                failure_kind,
                ProjectionJobCompletion::Failed,
            );
        }
    }
}

fn run_projection_job(dispatch: ProjectionDispatchContext, job: ProjectionJob) {
    let sequence = dispatch
        .projection_runtime
        .next_projection_sequence(&job.kind);
    let created_at_ms = current_unix_millis();
    let ledger = dispatch.projection_runtime.transcript_ledger_snapshot();
    let generation_started_ms = current_unix_millis();

    match dispatch.patch_generator.generate_projection_patch(
        job.clone(),
        ledger,
        sequence,
        created_at_ms,
    ) {
        Ok(outcome) => {
            let generation_latency_ms = current_unix_millis().saturating_sub(generation_started_ms);
            record_projection_generation_result(
                &dispatch,
                &job.kind,
                generation_latency_ms,
                outcome.tokens_used,
                true,
            );
            let mut patch = outcome.patch;
            patch.queued_at_ms.get_or_insert(job.queued_at_ms);
            patch
                .generation_latency_ms
                .get_or_insert(generation_latency_ms);
            let emitted_patch = patch.clone();
            let apply_started_ms = current_unix_millis();
            match dispatch.projection_runtime.apply_runtime_projection_patch(
                &job.session_id,
                &job.basis,
                patch,
            ) {
                Ok(result) => {
                    record_projection_apply_result(
                        &dispatch,
                        &job.kind,
                        current_unix_millis().saturating_sub(apply_started_ms),
                        true,
                    );
                    log::debug!(
                        "Projection job applied job_id={} kind={:?} outcome={:?}",
                        job.id,
                        job.kind,
                        result.outcome
                    );
                    emit_projection_runtime_events(&dispatch, &emitted_patch);
                    finish_projection_scheduler_job(
                        dispatch,
                        job.kind,
                        ProjectionJobCompletion::Completed,
                    );
                }
                Err(error) => {
                    record_projection_apply_result(
                        &dispatch,
                        &job.kind,
                        current_unix_millis().saturating_sub(apply_started_ms),
                        false,
                    );
                    let stale_apply = matches!(
                        &error,
                        ProjectionRuntimeApplyError::Apply {
                            error: ProjectionApplyError::StaleBasis { .. }
                        }
                    );
                    log::warn!(
                        "Projection job apply failed job_id={} kind={:?} stale_apply={} error={:?}",
                        job.id,
                        job.kind,
                        stale_apply,
                        error
                    );
                    finish_projection_scheduler_job(
                        dispatch,
                        job.kind,
                        if stale_apply {
                            ProjectionJobCompletion::Completed
                        } else {
                            ProjectionJobCompletion::Failed
                        },
                    );
                }
            }
        }
        Err(error) => {
            record_projection_generation_result(
                &dispatch,
                &job.kind,
                current_unix_millis().saturating_sub(generation_started_ms),
                0,
                false,
            );
            log::warn!(
                "Projection job generation failed job_id={} kind={:?} error={}",
                job.id,
                job.kind,
                error
            );
            finish_projection_scheduler_job(dispatch, job.kind, ProjectionJobCompletion::Failed);
        }
    }
}

fn record_projection_generation_result(
    dispatch: &ProjectionDispatchContext,
    kind: &ProjectionKind,
    latency_ms: u64,
    tokens_used: u32,
    success: bool,
) {
    let mut schedulers = match dispatch.projection_schedulers.lock() {
        Ok(schedulers) => schedulers,
        Err(poisoned) => {
            log::warn!(
                "Projection scheduler lock poisoned during generation telemetry; recovering"
            );
            poisoned.into_inner()
        }
    };
    schedulers.record_generation_result(kind, latency_ms, tokens_used, success);
}

fn record_projection_apply_result(
    dispatch: &ProjectionDispatchContext,
    kind: &ProjectionKind,
    latency_ms: u64,
    accepted: bool,
) {
    let mut schedulers = match dispatch.projection_schedulers.lock() {
        Ok(schedulers) => schedulers,
        Err(poisoned) => {
            log::warn!("Projection scheduler lock poisoned during apply telemetry; recovering");
            poisoned.into_inner()
        }
    };
    schedulers.record_apply_result(kind, latency_ms, accepted);
}

fn emit_projection_runtime_events(dispatch: &ProjectionDispatchContext, patch: &ProjectionPatch) {
    dispatch.event_sink.emit_projection_patch(patch);
    let materialized = dispatch
        .projection_runtime
        .materialized_projection_snapshot();
    match patch.kind {
        ProjectionKind::Notes => dispatch
            .event_sink
            .emit_materialized_notes(&materialized.notes),
        ProjectionKind::Graph => dispatch
            .event_sink
            .emit_materialized_graph(&materialized.graph),
    }
}

fn finish_projection_scheduler_job(
    dispatch: ProjectionDispatchContext,
    kind: ProjectionKind,
    completion: ProjectionJobCompletion,
) {
    let ledger = match dispatch.transcript_ledger.lock() {
        Ok(ledger) => ledger.clone(),
        Err(poisoned) => {
            log::warn!("Transcript ledger lock poisoned during projection completion; recovering");
            poisoned.into_inner().clone()
        }
    };
    let decision = {
        let mut schedulers = match dispatch.projection_schedulers.lock() {
            Ok(schedulers) => schedulers,
            Err(poisoned) => {
                log::warn!(
                    "Projection scheduler lock poisoned during projection completion; recovering"
                );
                poisoned.into_inner()
            }
        };
        let now_ms = current_unix_millis();
        match (&kind, completion) {
            (ProjectionKind::Notes, ProjectionJobCompletion::Completed) => {
                schedulers.complete_notes_in_flight(&ledger, now_ms)
            }
            (ProjectionKind::Graph, ProjectionJobCompletion::Completed) => {
                schedulers.complete_graph_in_flight(&ledger, now_ms)
            }
            (ProjectionKind::Notes, ProjectionJobCompletion::Failed) => {
                schedulers.fail_notes_in_flight(&ledger, now_ms)
            }
            (ProjectionKind::Graph, ProjectionJobCompletion::Failed) => {
                schedulers.fail_graph_in_flight(&ledger, now_ms)
            }
        }
    };
    log::debug!(
        "Projection scheduler completion kind={:?} completion={:?} decision={:?}",
        kind,
        completion,
        decision
    );
    dispatch_projection_decision(dispatch, decision);
}

fn projection_kind_key(kind: &ProjectionKind) -> &'static str {
    match kind {
        ProjectionKind::Notes => "notes",
        ProjectionKind::Graph => "graph",
    }
}

/// Store, emit, update status, and spawn extraction for a final transcript
/// segment. Shared by every ASR worker implementation to eliminate the
/// ~60-line tail that used to be copied inline at each call site.
///
/// Behaviour preserved from the original inline copies:
/// - Append to the 500-item ring buffer, persist to disk, emit
///   `TRANSCRIPT_UPDATE`, write pipeline status, fire extraction.
/// - `speaker_info` controls the `SPEAKER_DETECTED` event: pass `Some(info)`
///   for the diarized-in-place workers (local/cloud/AWS) where speaker_info
///   was previously emitted here; pass `None` for the streaming receivers
///   (Deepgram/AssemblyAI/sherpa) where `SPEAKER_DETECTED` is already emitted
///   earlier, inside the diarization branch.
#[allow(clippy::too_many_arguments)]
fn emit_transcript_and_extract_with_meta(
    segment: TranscriptSegment,
    speaker_info: Option<SpeakerInfo>,
    ctx: &TranscriptProcessingContext,
    asr_count: u64,
    diarization_count: u64,
    extraction_count: &Arc<AtomicU64>,
    graph_update_count: &Arc<AtomicU64>,
    asr_meta: AsrRevisionMeta,
) -> bool {
    let span_id = asr_meta.span_id.unwrap_or_else(|| segment.id.clone());
    let provider_item_id = asr_meta.provider_item_id;
    let speaker_id = asr_meta
        .speaker_id
        .clone()
        .or_else(|| segment.speaker_id.clone());
    let speaker_label = asr_meta
        .speaker_label
        .clone()
        .or_else(|| segment.speaker_label.clone());
    let channel = asr_meta.channel;
    let revision_number = asr_meta.revision_number.unwrap_or(1);
    let supersedes = asr_meta.supersedes;
    let turn_id = asr_meta.turn_id;
    let raw_event_ref = asr_meta.raw_event_ref;
    let capture_latency_ms = asr_meta.capture_latency_ms;
    let asr_latency_ms = asr_meta.asr_latency_ms;
    let received_at_ms = asr_meta.received_at_ms.unwrap_or_else(current_unix_millis);
    let asr_payload = events::AsrSpanRevisionPayload {
        span_id: span_id.clone(),
        provider: ctx.asr_provider.to_string(),
        source_id: segment.source_id.clone(),
        provider_item_id,
        transcript_segment_id: Some(segment.id.clone()),
        speaker_id,
        speaker_label,
        channel: channel.clone(),
        text: segment.text.clone(),
        start_time: segment.start_time,
        end_time: segment.end_time,
        confidence: segment.confidence,
        is_final: true,
        stability: events::AsrSpanStability::Final,
        revision_number,
        supersedes,
        turn_id,
        end_of_turn: true,
        raw_event_ref: raw_event_ref.clone(),
        capture_latency_ms,
        asr_latency_ms,
        received_at_ms,
    };
    if !record_asr_span_revision_event_and_observe_projection(
        &ctx.transcript_ledger,
        &ctx.transcript_event_writer,
        &ctx.projection_schedulers,
        Some(&ctx.projection_dispatch_context()),
        &asr_payload,
    ) {
        return false;
    }

    // 1. Store in transcript buffer (ring-buffered at 500 items).
    if let Ok(mut buffer) = ctx.transcript_buffer.write() {
        buffer.push_back(segment.clone());
        if buffer.len() > 500 {
            buffer.pop_front();
        }
    }
    // 2. Persist transcript segment.
    if let Ok(writer_guard) = ctx.transcript_writer.lock()
        && let Some(ref writer) = *writer_guard
    {
        writer.append(&segment);
    }

    // 3. Emit Tauri events.
    emit_asr_span_revision(&ctx.app_handle, asr_payload);
    let event_sink = TauriDiarizationEventSink {
        app_handle: &ctx.app_handle,
    };
    let diarization_dispatch = DiarizationDispatchContext {
        event_sink: &event_sink,
        speaker_timeline: &ctx.speaker_timeline,
        knowledge_graph: &ctx.knowledge_graph,
        graph_snapshot: &ctx.graph_snapshot,
    };
    emit_diarization_span_revision_for_transcript(
        &diarization_dispatch,
        ctx.asr_provider,
        &segment,
        &span_id,
        channel,
        raw_event_ref,
    );
    let _ = ctx.app_handle.emit(events::TRANSCRIPT_UPDATE, &segment);
    if let Some(info) = speaker_info.as_ref() {
        let _ = ctx.app_handle.emit(events::SPEAKER_DETECTED, info);
    }
    spawn_agent_proposal_task(
        segment.clone(),
        ctx.projection_runtime.current_session_id(),
        span_id,
        ctx.app_handle.clone(),
        ctx.pending_agent_proposals.clone(),
    );

    // 4. Update pipeline status counts.
    if let Ok(mut status) = ctx.pipeline_status.write() {
        status.asr = StageStatus::Running {
            processed_count: asr_count,
        };
        status.diarization = StageStatus::Running {
            processed_count: diarization_count,
        };
    }

    // 5. Knowledge Graph Extraction — fire-and-forget, COALESCED. Consecutive
    // same-speaker segments are batched (see coalesce_submit) to cut redundant
    // LLM calls and graph churn; the idle/age flush comes from the receiver
    // loop heartbeat (flush_pending_if_due) and shutdown (flush_pending_now).
    // Build a sliding window of recent transcript as context so the extractor
    // can resolve references and connect this segment to the conversation.
    let context = {
        const CONTEXT_WINDOW: usize = 6;
        match ctx.transcript_buffer.read() {
            Ok(buffer) => {
                let n = buffer.len();
                // Take the CONTEXT_WINDOW segments BEFORE the current one (the
                // current segment was just pushed at the tail in step 1).
                let start = n.saturating_sub(CONTEXT_WINDOW + 1);
                let end = n.saturating_sub(1);
                buffer
                    .iter()
                    .take(end)
                    .skip(start)
                    .map(|s| {
                        format!(
                            "[{}]: {}",
                            s.speaker_label.as_deref().unwrap_or("Unknown"),
                            s.text
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            }
            Err(_) => String::new(),
        }
    };
    coalesce_submit(
        ctx,
        segment.text.clone(),
        segment
            .speaker_label
            .clone()
            .unwrap_or_else(|| "Unknown".to_string()),
        context,
        segment.id.clone(),
        segment.start_time,
        extraction_count,
        graph_update_count,
    );
    true
}

#[allow(dead_code)]
fn emit_moonshine_span_revision(
    revision: MoonshineSpanRevision,
    ctx: &TranscriptProcessingContext,
    asr_count: u64,
    extraction_count: &Arc<AtomicU64>,
    graph_update_count: &Arc<AtomicU64>,
) -> bool {
    if let Some(latency_ms) = revision.latency_ms {
        emit_stage_latency(
            &ctx.app_handle,
            "asr.moonshine",
            Some(&revision.payload.source_id),
            Some(&revision.payload.span_id),
            Duration::from_millis(latency_ms),
        );
    }

    if revision.payload.is_final {
        let Some(segment) = moonshine_final_transcript_segment(&revision) else {
            return false;
        };
        return emit_transcript_and_extract_with_meta(
            segment,
            None,
            ctx,
            asr_count,
            0,
            extraction_count,
            graph_update_count,
            moonshine_revision_meta(&revision),
        );
    }

    if !record_asr_span_revision_event_and_observe_projection(
        &ctx.transcript_ledger,
        &ctx.transcript_event_writer,
        &ctx.projection_schedulers,
        Some(&ctx.projection_dispatch_context()),
        &revision.payload,
    ) {
        return false;
    }
    emit_asr_span_revision(&ctx.app_handle, revision.payload.clone());
    events::emit_or_log(
        &ctx.app_handle,
        events::ASR_PARTIAL,
        events::AsrPartialPayload {
            provider: revision.payload.provider.clone(),
            source_id: revision.payload.source_id.clone(),
            text: revision.payload.text.clone(),
            start_time: revision.payload.start_time,
            end_time: revision.payload.end_time,
            confidence: revision.payload.confidence,
            timestamp_ms: revision.payload.received_at_ms,
        },
    );
    true
}

fn moonshine_final_transcript_segment(
    revision: &MoonshineSpanRevision,
) -> Option<TranscriptSegment> {
    let payload = &revision.payload;
    if !payload.is_final || payload.text.trim().is_empty() {
        return None;
    }

    Some(TranscriptSegment {
        id: payload
            .transcript_segment_id
            .clone()
            .unwrap_or_else(|| format!("{}@final", payload.span_id)),
        source_id: payload.source_id.clone(),
        // Moonshine speaker values are provider hints until SpeakerTimeline
        // can reconcile them with local/provider diarization revisions.
        speaker_id: None,
        speaker_label: None,
        text: payload.text.clone(),
        start_time: payload.start_time,
        end_time: payload.end_time,
        confidence: payload.confidence,
    })
}

fn moonshine_revision_meta(revision: &MoonshineSpanRevision) -> AsrRevisionMeta {
    let payload = &revision.payload;
    AsrRevisionMeta {
        span_id: Some(payload.span_id.clone()),
        provider_item_id: payload.provider_item_id.clone(),
        speaker_id: payload.speaker_id.clone(),
        speaker_label: payload.speaker_label.clone(),
        channel: payload.channel.clone(),
        revision_number: Some(payload.revision_number),
        supersedes: payload.supersedes.clone(),
        turn_id: payload.turn_id.clone(),
        raw_event_ref: payload.raw_event_ref.clone(),
        capture_latency_ms: payload.capture_latency_ms,
        asr_latency_ms: payload.asr_latency_ms.or(revision.latency_ms),
        received_at_ms: Some(payload.received_at_ms),
    }
}

fn emit_provider_span_revision_payload(
    payload: events::AsrSpanRevisionPayload,
    ctx: &TranscriptProcessingContext,
    asr_count: u64,
    extraction_count: &Arc<AtomicU64>,
    graph_update_count: &Arc<AtomicU64>,
) -> bool {
    if payload.is_final {
        let Some(segment) = final_transcript_segment_from_asr_payload(&payload) else {
            return false;
        };
        return emit_transcript_and_extract_with_meta(
            segment,
            None,
            ctx,
            asr_count,
            0,
            extraction_count,
            graph_update_count,
            asr_payload_revision_meta(&payload),
        );
    }

    if !record_asr_span_revision_event_and_observe_projection(
        &ctx.transcript_ledger,
        &ctx.transcript_event_writer,
        &ctx.projection_schedulers,
        Some(&ctx.projection_dispatch_context()),
        &payload,
    ) {
        return false;
    }
    emit_asr_span_revision(&ctx.app_handle, payload.clone());
    events::emit_or_log(
        &ctx.app_handle,
        events::ASR_PARTIAL,
        events::AsrPartialPayload {
            provider: payload.provider.clone(),
            source_id: payload.source_id.clone(),
            text: payload.text.clone(),
            start_time: payload.start_time,
            end_time: payload.end_time,
            confidence: payload.confidence,
            timestamp_ms: payload.received_at_ms,
        },
    );
    true
}

fn final_transcript_segment_from_asr_payload(
    payload: &events::AsrSpanRevisionPayload,
) -> Option<TranscriptSegment> {
    if !payload.is_final || payload.text.trim().is_empty() {
        return None;
    }

    Some(TranscriptSegment {
        id: payload
            .transcript_segment_id
            .clone()
            .unwrap_or_else(|| format!("{}@final", payload.span_id)),
        source_id: payload.source_id.clone(),
        speaker_id: payload.speaker_id.clone(),
        speaker_label: payload.speaker_label.clone(),
        text: payload.text.clone(),
        start_time: payload.start_time,
        end_time: payload.end_time,
        confidence: payload.confidence,
    })
}

fn asr_payload_revision_meta(payload: &events::AsrSpanRevisionPayload) -> AsrRevisionMeta {
    AsrRevisionMeta {
        span_id: Some(payload.span_id.clone()),
        provider_item_id: payload.provider_item_id.clone(),
        speaker_id: payload.speaker_id.clone(),
        speaker_label: payload.speaker_label.clone(),
        channel: payload.channel.clone(),
        revision_number: Some(payload.revision_number),
        supersedes: payload.supersedes.clone(),
        turn_id: payload.turn_id.clone(),
        raw_event_ref: payload.raw_event_ref.clone(),
        capture_latency_ms: payload.capture_latency_ms,
        asr_latency_ms: payload.asr_latency_ms,
        received_at_ms: Some(payload.received_at_ms),
    }
}

fn normalize_assemblyai_v3_revision_for_side_effects(
    revision: &mut crate::asr::assemblyai::AssemblyAiV3ParsedRevision,
) {
    // AssemblyAI may emit an unformatted final turn and then a formatted final
    // turn for the same turn_order. Keep the unformatted event as a span
    // revision, but do not append a durable transcript row, start projection
    // jobs, or spawn live-assist proposals until the formatted final arrives.
    if revision.payload.is_final && !revision.turn_is_formatted {
        revision.payload.is_final = false;
        revision.payload.transcript_segment_id = None;
        revision.payload.stability = events::AsrSpanStability::Partial;
        revision.payload.end_of_turn = false;
    }
}

fn emit_soniox_span_revision(
    revision: SonioxParsedRevision,
    ctx: &TranscriptProcessingContext,
    asr_count: u64,
    extraction_count: &Arc<AtomicU64>,
    graph_update_count: &Arc<AtomicU64>,
) -> bool {
    if let Some(latency_ms) = revision
        .final_audio_proc_ms
        .or(revision.total_audio_proc_ms)
    {
        emit_stage_latency(
            &ctx.app_handle,
            "asr.soniox",
            Some(&revision.payload.source_id),
            Some(&revision.payload.span_id),
            Duration::from_millis(latency_ms),
        );
    }
    emit_provider_span_revision_payload(
        revision.payload,
        ctx,
        asr_count,
        extraction_count,
        graph_update_count,
    )
}

fn assemblyai_source_id_from_span_id(span_id: &str) -> String {
    span_id
        .strip_prefix("assemblyai:")
        .and_then(|rest| rest.rsplit_once(":turn-").map(|(source, _)| source))
        .filter(|source| !source.trim().is_empty())
        .unwrap_or("assemblyai-stream")
        .to_string()
}

fn emit_assemblyai_speaker_revision(
    revision: &crate::asr::assemblyai::AssemblyAiV3SpeakerRevision,
    ctx: &TranscriptProcessingContext,
    speaker_revision_numbers_by_span: &mut HashMap<String, u64>,
    received_at_ms: u64,
) -> DiarizationRevisionOutcome {
    let event_sink = TauriDiarizationEventSink {
        app_handle: &ctx.app_handle,
    };
    let diarization_dispatch = DiarizationDispatchContext {
        event_sink: &event_sink,
        speaker_timeline: &ctx.speaker_timeline,
        knowledge_graph: &ctx.knowledge_graph,
        graph_snapshot: &ctx.graph_snapshot,
    };
    emit_assemblyai_speaker_revision_with_dispatch(
        revision,
        &diarization_dispatch,
        speaker_revision_numbers_by_span,
        received_at_ms,
    )
}

fn emit_assemblyai_speaker_revision_with_dispatch<E: DiarizationEventSink + ?Sized>(
    revision: &crate::asr::assemblyai::AssemblyAiV3SpeakerRevision,
    dispatch_ctx: &DiarizationDispatchContext<'_, E>,
    speaker_revision_numbers_by_span: &mut HashMap<String, u64>,
    received_at_ms: u64,
) -> DiarizationRevisionOutcome {
    let source_id = assemblyai_source_id_from_span_id(&revision.span_id);
    let start_time = revision
        .words
        .iter()
        .filter_map(|word| word.start_time)
        .min_by(f64::total_cmp)
        .unwrap_or(0.0);
    let end_time = revision
        .words
        .iter()
        .filter_map(|word| word.end_time)
        .max_by(f64::total_cmp)
        .unwrap_or(start_time);
    let span_id = format!(
        "assemblyai:{source_id}:turn-{}:speaker",
        revision.turn_order
    );
    let (revision_number, supersedes) =
        next_span_revision(speaker_revision_numbers_by_span, &span_id);

    emit_and_dispatch_diarization_span_revision(
        dispatch_ctx,
        events::DiarizationSpanRevisionPayload {
            span_id,
            provider: "assemblyai".to_string(),
            timeline_id: source_id.clone(),
            source_id: Some(source_id),
            speaker_id: revision.speaker_id.clone(),
            speaker_label: revision.speaker_label.clone(),
            channel: None,
            start_time,
            end_time,
            confidence: None,
            is_final: true,
            stability: events::DiarizationSpanStability::Final,
            revision_number,
            supersedes,
            basis_asr_span_ids: vec![revision.span_id.clone()],
            basis_transcript_segment_ids: vec![format!("{}@final", revision.provider_item_id)],
            raw_event_ref: Some(format!(
                "assemblyai.v3.speaker_revision.turn-{}",
                revision.turn_order
            )),
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms,
        },
    )
}

// ---------------------------------------------------------------------------
// Fire-and-forget extraction task
// ---------------------------------------------------------------------------

// Spawn entity extraction on a separate thread so it doesn't block the
// ASR processing loop. Falls back to inline execution if thread spawn fails.
// ---------------------------------------------------------------------------
// Extraction coalescing
// ---------------------------------------------------------------------------
//
// Firing an LLM extraction per transcript segment is wasteful under fast speech
// (many short finals): redundant calls, graph churn, quota burn, and queue
// pressure. We coalesce consecutive SAME-speaker segments into one extraction,
// flushing when the batch is "done": a speaker change, a size/segment cap, an
// idle gap after the last segment, or a max age. Larger batches also extract
// more accurately (more context per call) than tiny fragments. The graph still
// updates within a couple seconds — fine for a background surface.

/// Flush a coalesced batch after this idle gap since the last segment (ms).
const COALESCE_IDLE_MS: u64 = 1000;
/// Hard cap on how long a batch may accumulate before flushing (ms).
const COALESCE_MAX_AGE_MS: u64 = 3500;
/// Flush once a batch reaches this many segments…
const COALESCE_MAX_SEGS: usize = 3;
/// …or this many characters of combined text.
const COALESCE_MAX_CHARS: usize = 500;

/// A batch of consecutive same-speaker segments awaiting extraction.
pub(crate) struct PendingBatch {
    speaker: String,
    text: String,
    /// Sliding-window context captured from the FIRST segment of the batch.
    context: String,
    last_segment_id: String,
    first_ts: f64,
    seg_count: usize,
    last_push: Instant,
    batch_start: Instant,
}

/// Build `ExtractionDeps` from the context and submit a (possibly coalesced)
/// extraction batch to the rayon pool.
fn flush_batch(
    ctx: &TranscriptProcessingContext,
    batch: PendingBatch,
    extraction_count: &Arc<AtomicU64>,
    graph_update_count: &Arc<AtomicU64>,
) {
    if batch.text.trim().is_empty() {
        return;
    }
    let deps = ExtractionDeps {
        llm_engine: &ctx.llm_engine,
        api_client: &ctx.api_client,
        mistralrs_engine: &ctx.mistralrs_engine,
        llm_executor: &ctx.llm_executor,
        llm_provider: &ctx.llm_provider,
        llm_allow_cloud_fallbacks: ctx.llm_allow_cloud_fallbacks,
        graph_extractor: &ctx.graph_extractor,
        knowledge_graph: &ctx.knowledge_graph,
        graph_snapshot: &ctx.graph_snapshot,
        pipeline_status: &ctx.pipeline_status,
        app_handle: &ctx.app_handle,
    };
    spawn_extraction_task(
        batch.text,
        batch.speaker,
        batch.context,
        batch.last_segment_id,
        batch.first_ts,
        &deps,
        extraction_count,
        graph_update_count,
    );
}

/// Add a segment to the coalescing buffer, flushing the previous batch when the
/// speaker changes or a size/segment cap is hit.
#[allow(clippy::too_many_arguments)]
fn coalesce_submit(
    ctx: &TranscriptProcessingContext,
    text: String,
    speaker: String,
    context: String,
    segment_id: String,
    timestamp: f64,
    extraction_count: &Arc<AtomicU64>,
    graph_update_count: &Arc<AtomicU64>,
) {
    let now = Instant::now();
    let trimmed = text.trim();
    let mut to_flush: Option<PendingBatch> = None;
    {
        let mut guard = ctx
            .pending_extraction
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match guard.as_mut() {
            Some(batch) if batch.speaker == speaker => {
                if !trimmed.is_empty() {
                    if !batch.text.is_empty() {
                        batch.text.push(' ');
                    }
                    batch.text.push_str(trimmed);
                }
                batch.seg_count += 1;
                batch.last_segment_id = segment_id;
                batch.last_push = now;
                if batch.seg_count >= COALESCE_MAX_SEGS || batch.text.len() >= COALESCE_MAX_CHARS {
                    to_flush = guard.take();
                }
            }
            _ => {
                // Speaker changed (or nothing pending): flush the old batch and
                // start a fresh one for this speaker.
                to_flush = guard.take();
                *guard = Some(PendingBatch {
                    speaker,
                    text: trimmed.to_string(),
                    context,
                    last_segment_id: segment_id,
                    first_ts: timestamp,
                    seg_count: 1,
                    last_push: now,
                    batch_start: now,
                });
            }
        }
    }
    if let Some(batch) = to_flush {
        flush_batch(ctx, batch, extraction_count, graph_update_count);
    }
}

/// Flush the pending batch if it has gone idle or hit its max age. Called from
/// the receiver loops' recv-timeout heartbeat (~every 500 ms), so a batch is
/// extracted shortly after speech pauses without a dedicated timer thread.
pub(crate) fn flush_pending_if_due(
    ctx: &TranscriptProcessingContext,
    extraction_count: &Arc<AtomicU64>,
    graph_update_count: &Arc<AtomicU64>,
) {
    let now = Instant::now();
    let mut to_flush: Option<PendingBatch> = None;
    {
        let mut guard = ctx
            .pending_extraction
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_ref() {
            let idle = now.duration_since(batch.last_push).as_millis() as u64 >= COALESCE_IDLE_MS;
            let aged =
                now.duration_since(batch.batch_start).as_millis() as u64 >= COALESCE_MAX_AGE_MS;
            if idle || aged {
                to_flush = guard.take();
            }
        }
    }
    if let Some(batch) = to_flush {
        flush_batch(ctx, batch, extraction_count, graph_update_count);
    }
}

/// Flush any pending batch immediately (call on shutdown so the last utterance
/// before stop still reaches the graph).
pub(crate) fn flush_pending_now(
    ctx: &TranscriptProcessingContext,
    extraction_count: &Arc<AtomicU64>,
    graph_update_count: &Arc<AtomicU64>,
) {
    let batch = ctx
        .pending_extraction
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    if let Some(batch) = batch {
        flush_batch(ctx, batch, extraction_count, graph_update_count);
    }
}

/// Submit a fire-and-forget entity-extraction task to the shared bounded rayon
/// pool (4 workers). Used by the speech path and by the Gemini event receiver
/// so neither blocks its own critical path on LLM extraction I/O.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_extraction_task(
    text: String,
    speaker: String,
    context: String,
    segment_id: String,
    timestamp: f64,
    deps: &ExtractionDeps<'_>,
    extraction_count: &Arc<std::sync::atomic::AtomicU64>,
    graph_update_count: &Arc<std::sync::atomic::AtomicU64>,
) {
    let llm_engine = deps.llm_engine.clone();
    let api_client = deps.api_client.clone();
    let mistralrs_engine = deps.mistralrs_engine.clone();
    let llm_executor = deps.llm_executor.clone();
    let llm_provider = deps.llm_provider.clone();
    let graph_extractor = deps.graph_extractor.clone();
    let knowledge_graph = deps.knowledge_graph.clone();
    let graph_snapshot = deps.graph_snapshot.clone();
    let pipeline_status = deps.pipeline_status.clone();
    let app_handle = deps.app_handle.clone();
    let llm_allow_cloud_fallbacks = deps.llm_allow_cloud_fallbacks;
    let extraction_count = extraction_count.clone();
    let graph_update_count = graph_update_count.clone();

    let run_extraction = move || {
        let extraction_start = Instant::now();
        let mut local_extraction = extraction_count.load(Ordering::Relaxed);
        let mut local_graph = graph_update_count.load(Ordering::Relaxed);
        let owned_deps = ExtractionDeps {
            llm_engine: &llm_engine,
            api_client: &api_client,
            mistralrs_engine: &mistralrs_engine,
            llm_executor: &llm_executor,
            llm_provider: &llm_provider,
            llm_allow_cloud_fallbacks,
            graph_extractor: &graph_extractor,
            knowledge_graph: &knowledge_graph,
            graph_snapshot: &graph_snapshot,
            pipeline_status: &pipeline_status,
            app_handle: &app_handle,
        };
        process_extraction_and_emit(
            &text,
            &speaker,
            &context,
            &segment_id,
            timestamp,
            &owned_deps,
            &mut local_extraction,
            &mut local_graph,
        );
        extraction_count.store(local_extraction, Ordering::Relaxed);
        graph_update_count.store(local_graph, Ordering::Relaxed);
        emit_stage_latency(
            &app_handle,
            "entity_extraction",
            None,
            Some(&segment_id),
            extraction_start.elapsed(),
        );
    };

    // Submit to the bounded rayon thread pool (4 workers). Unlike
    // `std::thread::spawn`, `rayon::ThreadPool::spawn` cannot fail — work is
    // queued on an existing worker. This prevents OS thread exhaustion during
    // long sessions (previously 72K+ threads in 10hrs at 2 segments/sec).
    extraction_pool().spawn(run_extraction);
}

// ---------------------------------------------------------------------------
// Audio accumulation helper
// ---------------------------------------------------------------------------

/// Accumulator that collects `ProcessedAudioChunk`s into `AccumulatedSegment`s
/// of approximately `TARGET_FRAMES` length.
struct AudioAccumulator {
    audio: Vec<f32>,
    source_id: String,
    segment_start: Option<Duration>,
    segment_end: Duration,
}

impl AudioAccumulator {
    fn new() -> Self {
        Self {
            audio: Vec::with_capacity(TARGET_FRAMES),
            source_id: String::new(),
            segment_start: None,
            segment_end: Duration::ZERO,
        }
    }

    /// Feed a chunk. Returns `Some(AccumulatedSegment)` if the accumulator
    /// has reached the target size, otherwise `None`.
    fn feed(&mut self, chunk: &ProcessedAudioChunk) -> Option<AccumulatedSegment> {
        if self.source_id.is_empty() {
            // Boundary: AccumulatedSegment.source_id is a persisted/serialized
            // String, so materialize the chunk's Arc<str> id here (FA-4b).
            self.source_id = chunk.source_id.to_string();
        }
        if self.segment_start.is_none() {
            self.segment_start = chunk.timestamp;
        }
        self.segment_end = chunk.timestamp.unwrap_or(Duration::ZERO);
        self.audio.extend_from_slice(&chunk.data);

        if self.audio.len() >= TARGET_FRAMES {
            Some(self.take())
        } else {
            None
        }
    }

    /// Take the current accumulated audio as a segment, retaining the last
    /// `OVERLAP_FRAMES` samples for continuity with the next segment.
    fn take(&mut self) -> AccumulatedSegment {
        let full_audio = std::mem::replace(&mut self.audio, Vec::with_capacity(TARGET_FRAMES));
        let num_frames = full_audio.len();
        let seg_start = self.segment_start.unwrap_or(Duration::ZERO);
        let seg_end = self.segment_end;

        // Retain the last OVERLAP_FRAMES samples for the next segment
        let overlap_start = num_frames.saturating_sub(OVERLAP_FRAMES);
        self.audio.extend_from_slice(&full_audio[overlap_start..]);

        // Compute overlap duration so the next segment's start time is set correctly
        let overlap_duration =
            Duration::from_secs_f64((num_frames - overlap_start) as f64 / 16_000.0);
        // The next segment starts at (end_time - overlap_duration)
        self.segment_start = Some(seg_end.saturating_sub(overlap_duration));

        AccumulatedSegment {
            source_id: self.source_id.clone(),
            audio: full_audio,
            start_time: seg_start,
            end_time: seg_end,
            num_frames,
        }
    }

    /// Flush any remaining audio as a final segment. Returns `None` if empty.
    fn flush(mut self) -> Option<AccumulatedSegment> {
        if self.audio.is_empty() {
            None
        } else {
            Some(self.take())
        }
    }
}

fn feed_source_accumulator(
    accumulators: &mut HashMap<String, AudioAccumulator>,
    chunk: &ProcessedAudioChunk,
) -> Option<AccumulatedSegment> {
    accumulators
        .entry(chunk.source_id.to_string())
        .or_insert_with(AudioAccumulator::new)
        .feed(chunk)
}

fn flush_source_accumulators(
    accumulators: HashMap<String, AudioAccumulator>,
) -> Vec<AccumulatedSegment> {
    accumulators
        .into_values()
        .filter_map(AudioAccumulator::flush)
        .collect()
}

// ---------------------------------------------------------------------------
// Speech processor threads (2-thread model)
// ---------------------------------------------------------------------------

/// Speech processor orchestrator — 2-thread architecture:
///
/// 1. **Accumulator thread** (this function): Receives `ProcessedAudioChunk`s,
///    accumulates them into ~2s segments, and sends them to the ASR worker.
///    Always consuming from the channel, never blocked by inference.
///
/// 2. **ASR worker thread** (spawned internally): Receives accumulated segments,
///    runs Whisper transcription, diarization, and fires off extraction.
///
/// Returns a `JoinHandle` for the spawned ASR worker thread so the caller
/// can track it for clean shutdown.
pub(crate) fn run_speech_processor(
    channels: SpeechChannels,
    shared: SpeechShared,
    config: SpeechConfig,
    asr_provider: AsrProvider,
    whisper_model: String,
) {
    let SpeechChannels {
        processed_rx,
        is_transcribing,
    } = channels;

    // Macro to reduce duplication: each fallback site calls
    // run_speech_processor_diarization_only with the same arguments
    // and then returns.  Only one branch is ever taken at runtime, so
    // the compiler accepts the conditional moves.
    macro_rules! fallback_diarization_only {
        () => {
            run_speech_processor_diarization_only(
                SpeechChannels {
                    processed_rx,
                    is_transcribing,
                },
                shared,
                config,
            );
            return;
        };
    }

    // Register the AppHandle with the persistence module so its background
    // writer threads (transcript appender, graph autosave) can emit
    // `CAPTURE_STORAGE_FULL` on ENOSPC. First caller wins; subsequent
    // speech-processor invocations are no-ops.
    crate::persistence::register_app_handle(shared.app_handle.clone());

    // Log LLM provider for diagnostics
    match &config.llm_provider {
        LlmProvider::LocalLlama => {
            log::info!(
                "Speech processor: LLM provider is LocalLlama — will prefer native LLM engine for entity extraction."
            );
        }
        LlmProvider::Api {
            endpoint, model, ..
        } => {
            log::info!(
                "Speech processor: LLM provider is API (endpoint={}, model={}) — will prefer API client for entity extraction.",
                endpoint,
                model
            );
        }
        LlmProvider::OpenRouter {
            model, base_url, ..
        } => {
            log::info!(
                "Speech processor: LLM provider is OpenRouter (base_url={}, model={}) — will prefer OpenRouter client for entity extraction.",
                base_url,
                model
            );
        }
        LlmProvider::AwsBedrock {
            region, model_id, ..
        } => {
            log::info!(
                "Speech processor: LLM provider is AWS Bedrock (region={}, model={}) — will prefer API client for entity extraction.",
                region,
                model_id
            );
        }
        LlmProvider::MistralRs { model_id } => {
            log::info!(
                "Speech processor: LLM provider is mistral.rs (model={}).",
                model_id
            );
        }
    }

    // ── Respect AsrProvider setting ──────────────────────────────────────
    // If the user has selected a cloud API provider for ASR, launch the
    // cloud ASR worker instead of loading local Whisper.
    if let AsrProvider::Api {
        ref endpoint,
        ref api_key,
        ref model,
    } = asr_provider
    {
        log::info!(
            "Speech processor: ASR provider is cloud API (endpoint={}, model={}) — \
             launching cloud ASR worker.",
            endpoint,
            model
        );
        let cloud_config = CloudAsrConfig {
            endpoint: endpoint.clone(),
            api_key: api_key.clone(),
            model: model.clone(),
            language: "en".to_string(),
        };
        run_cloud_asr_speech_processor(
            SpeechChannels {
                processed_rx,
                is_transcribing,
            },
            shared,
            config,
            cloud_config,
        );
        return;
    }

    // If the user selected Deepgram streaming ASR, launch the streaming
    // WebSocket worker instead of loading local Whisper.
    if let AsrProvider::DeepgramStreaming {
        ref api_key,
        ref model,
        enable_diarization,
        endpointing_ms,
        utterance_end_ms,
        vad_events,
        eot_threshold,
        eager_eot_threshold,
        eot_timeout_ms,
        max_speakers,
    } = asr_provider
    {
        log::info!(
            "Speech processor: ASR provider is Deepgram streaming (model={}) — \
             launching Deepgram streaming worker.",
            model
        );
        let effective_eager_eot = (eager_eot_threshold > 0.0
            && eager_eot_threshold <= eot_threshold)
            .then_some(eager_eot_threshold);
        let deepgram_config = crate::asr::deepgram::DeepgramConfig {
            api_key: api_key.clone(),
            model: model.clone(),
            enable_diarization,
            endpointing_ms: (endpointing_ms > 0).then_some(endpointing_ms),
            utterance_end_ms: (utterance_end_ms > 0).then_some(utterance_end_ms),
            vad_events,
            eot_threshold: (eot_threshold > 0.0).then_some(eot_threshold),
            eager_eot_threshold: effective_eager_eot,
            eot_timeout_ms: (eot_timeout_ms > 0).then_some(eot_timeout_ms),
            content_egress_policy: config.provider_content_egress_policy,
        };
        run_deepgram_speech_processor(
            SpeechChannels {
                // Mix all selected sources into one stream so Deepgram's single
                // WebSocket gets coherent audio instead of interleaved sources.
                // Transparent for a single source (pass-through).
                processed_rx: crate::audio::mixer::spawn_mixer(
                    processed_rx,
                    is_transcribing.clone(),
                ),
                is_transcribing,
            },
            shared,
            config,
            deepgram_config,
            max_speakers,
        );
        return;
    }

    // If the user selected AssemblyAI streaming ASR, launch the streaming
    // WebSocket worker instead of loading local Whisper.
    if let AsrProvider::AssemblyAI {
        ref api_key,
        enable_diarization,
    } = asr_provider
    {
        log::info!(
            "Speech processor: ASR provider is AssemblyAI streaming — \
             launching AssemblyAI streaming worker."
        );
        let assemblyai_config = crate::asr::assemblyai::AssemblyAIConfig {
            api_key: api_key.clone(),
            enable_diarization,
            content_egress_policy: config.provider_content_egress_policy,
        };
        run_assemblyai_speech_processor(
            SpeechChannels {
                processed_rx,
                is_transcribing,
            },
            shared,
            config,
            assemblyai_config,
        );
        return;
    }

    // If the user selected Soniox realtime ASR, launch the streaming WebSocket
    // worker. Soniox consumes one PCM stream per socket, so selected sources
    // are mixed into the backend-owned synthetic `mixed` source.
    if let AsrProvider::Soniox {
        ref api_key,
        ref model,
        enable_diarization,
        enable_language_identification,
        ref language_hints,
        max_speakers,
    } = asr_provider
    {
        log::info!(
            "Speech processor: ASR provider is Soniox realtime (model={}) — \
             launching Soniox streaming worker.",
            model
        );
        let soniox_config = crate::asr::soniox::SonioxConfig {
            api_key: api_key.clone(),
            model: model.clone(),
            source_id: crate::audio::mixer::MIXED_SOURCE_ID.to_string(),
            enable_diarization,
            enable_language_identification,
            language_hints: language_hints.clone(),
            content_egress_policy: config.provider_content_egress_policy,
        };
        run_soniox_speech_processor(
            SpeechChannels {
                processed_rx: crate::audio::mixer::spawn_mixer(
                    processed_rx,
                    is_transcribing.clone(),
                ),
                is_transcribing,
            },
            shared,
            config,
            soniox_config,
            max_speakers,
        );
        return;
    }

    // If the user selected OpenAI Realtime streaming transcription, launch the
    // streaming WebSocket worker instead of loading local Whisper.
    if let AsrProvider::OpenAiRealtimeTranscription {
        ref api_key,
        ref model,
        ref language,
    } = asr_provider
    {
        log::info!(
            "Speech processor: ASR provider is OpenAI Realtime transcription (model={}) — \
             launching OpenAI Realtime streaming worker.",
            model
        );
        let openai_config = crate::asr::openai_realtime::OpenAiRealtimeConfig {
            api_key: api_key.clone(),
            model: model.clone(),
            language: language.clone(),
            sample_rate: crate::asr::openai_realtime::REALTIME_SAMPLE_RATE,
            content_egress_policy: config.provider_content_egress_policy,
        };
        run_openai_realtime_speech_processor(
            SpeechChannels {
                // Mix all selected sources into one stream so the single
                // WebSocket gets coherent audio (transparent for one source),
                // mirroring the Deepgram path.
                processed_rx: crate::audio::mixer::spawn_mixer(
                    processed_rx,
                    is_transcribing.clone(),
                ),
                is_transcribing,
            },
            shared,
            config,
            openai_config,
        );
        return;
    }

    if let AsrProvider::AwsTranscribe {
        ref region,
        ref language_code,
        ref credential_source,
        enable_diarization,
    } = asr_provider
    {
        log::info!(
            "Speech processor: ASR provider is AWS Transcribe (region={}) — \
             launching streaming session.",
            region
        );
        let aws_config = crate::asr::aws_transcribe::AwsTranscribeConfig {
            region: region.clone(),
            language_code: language_code.clone(),
            credential_source: credential_source.clone(),
            enable_diarization,
        };
        run_aws_transcribe_speech_processor(
            SpeechChannels {
                processed_rx,
                is_transcribing,
            },
            shared,
            config,
            aws_config,
        );
        return;
    }

    // If the user selected sherpa-onnx streaming ASR, launch the streaming
    // worker that processes every audio chunk frame-by-frame.
    #[cfg(feature = "sherpa-streaming")]
    if let AsrProvider::SherpaOnnx {
        ref model_dir,
        enable_endpoint_detection,
    } = asr_provider
    {
        log::info!(
            "Speech processor: ASR provider is sherpa-onnx streaming (model_dir={}) — \
             launching streaming worker.",
            model_dir
        );
        let sherpa_config = crate::asr::sherpa_streaming::SherpaStreamingConfig {
            model_dir: config.models_dir.join(model_dir),
            enable_endpoint_detection,
        };
        run_sherpa_onnx_speech_processor(
            SpeechChannels {
                processed_rx,
                is_transcribing,
            },
            shared,
            config,
            sherpa_config,
        );
        return;
    }

    #[cfg(not(feature = "sherpa-streaming"))]
    if matches!(asr_provider, AsrProvider::SherpaOnnx { .. }) {
        log::error!(
            "Speech processor: sherpa-onnx ASR provider selected but the \
             'sherpa-streaming' feature is not enabled. Falling back to \
             diarization-only mode."
        );
        fallback_diarization_only!();
    }

    if matches!(asr_provider, AsrProvider::Moonshine { .. }) {
        log::error!(
            "Speech processor: Moonshine ASR provider selected before the \
             native runtime worker is implemented. Falling back to \
             diarization-only mode."
        );
        fallback_diarization_only!();
    }

    log::info!("Speech processor: loading Whisper model...");

    let asr_config = AsrConfig::with_models_dir_and_model(&config.models_dir, &whisper_model);
    let model_path_str = asr_config.model_path.display().to_string();

    // ── Pre-validate model file ─────────────────────────────────────────
    {
        let model_path = &asr_config.model_path;
        if !model_path.exists() {
            log::warn!(
                "Speech processor: Whisper model not found at '{}'. \
                 ASR disabled — running diarization-only mode. \
                 Download the model via Settings → Models.",
                model_path_str
            );
            fallback_diarization_only!();
        }

        match std::fs::metadata(model_path) {
            Ok(meta) => {
                const MIN_MODEL_SIZE: u64 = 1_000_000;
                if meta.len() < MIN_MODEL_SIZE {
                    log::warn!(
                        "Speech processor: Whisper model at '{}' appears corrupted \
                         (size: {} bytes, expected >= {} bytes). \
                         ASR disabled — running diarization-only mode. \
                         Re-download the model via Settings → Models.",
                        model_path_str,
                        meta.len(),
                        MIN_MODEL_SIZE
                    );
                    fallback_diarization_only!();
                }
                log::info!(
                    "Speech processor: model file validated — {} ({:.1} MB)",
                    model_path_str,
                    meta.len() as f64 / 1_048_576.0
                );
            }
            Err(e) => {
                log::warn!(
                    "Speech processor: cannot read model file metadata at '{}': {}. \
                     ASR disabled — running diarization-only mode.",
                    model_path_str,
                    e
                );
                fallback_diarization_only!();
            }
        }
    }

    // ── Create internal channel: accumulator → ASR worker ───────────────
    // Capacity 4 = up to 8s of buffered segments; prevents unbounded growth
    // while giving the ASR worker headroom for inference latency.
    let (asr_seg_tx, asr_seg_rx) = crossbeam_channel::bounded::<AccumulatedSegment>(4);

    // ── Spawn ASR + processing worker thread ────────────────────────────
    let is_transcribing_asr = is_transcribing.clone();
    let asr_worker_handle = std::thread::Builder::new()
        .name("asr-worker".to_string())
        .spawn({
            let shared_for_asr = shared.clone();
            let config_for_asr = config.clone();
            let model_path_str = model_path_str.clone();
            let asr_config =
                AsrConfig::with_models_dir_and_model(&config_for_asr.models_dir, &whisper_model);

            move || {
                run_asr_worker(
                    asr_seg_rx,
                    is_transcribing_asr,
                    shared_for_asr,
                    config_for_asr,
                    model_path_str,
                    asr_config,
                );
            }
        });

    match asr_worker_handle {
        Ok(_handle) => {
            // Store handle if needed for shutdown; currently the thread exits
            // when asr_seg_tx is dropped (channel disconnect) or the stop flag.
            log::info!("ASR worker thread spawned successfully");
            // We intentionally don't join here — the accumulator thread runs
            // independently. The handle is dropped, but the thread lives on
            // until the channel disconnects.
            // Note: the caller in commands.rs can store the asr-worker thread
            // handle separately if needed.
        }
        Err(e) => {
            log::error!("Failed to spawn ASR worker thread: {}", e);
            // Fall back to diarization-only on the current thread
            fallback_diarization_only!();
        }
    }

    // ── Accumulator loop (this thread) ──────────────────────────────────
    // Lightweight: just receives chunks, accumulates, and sends segments.
    // Never blocked by ASR inference.
    log::info!("Speech processor: entering accumulator loop");
    let mut accumulators: HashMap<String, AudioAccumulator> = HashMap::new();

    loop {
        let chunk = match processed_rx.recv_timeout(Duration::from_millis(500)) {
            Ok(chunk) => chunk,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                if !is_transcribing.load(Ordering::Relaxed) {
                    log::info!(
                        "Speech processor (accumulator): is_transcribing flag cleared, exiting"
                    );
                    break;
                }
                continue;
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        };

        if !is_transcribing.load(Ordering::Relaxed) {
            log::info!("Speech processor (accumulator): is_transcribing flag cleared, exiting");
            break;
        }

        // Accumulate chunks into ~2s segments
        if let Some(segment) = feed_source_accumulator(&mut accumulators, &chunk) {
            // Send to ASR worker; if channel full, log and drop (ASR can't keep up)
            if let Err(crossbeam_channel::TrySendError::Full(seg)) = asr_seg_tx.try_send(segment) {
                log::warn!(
                    "Speech processor: ASR segment channel full, dropping {:.2}s segment \
                     (ASR inference slower than real-time)",
                    seg.num_frames as f64 / 16_000.0
                );
            }
            // Disconnected case: ASR worker died, we'll detect on next iteration
        }
    }

    // Flush remaining audio. Bounded blocking send (not try_send) so the final
    // accumulated segment isn't dropped if the ASR channel is briefly full
    // exactly when the user stops (critique H3).
    for segment in flush_source_accumulators(accumulators) {
        let _ = asr_seg_tx.send_timeout(segment, std::time::Duration::from_secs(1));
    }

    // Drop the sender to signal the ASR worker to exit
    drop(asr_seg_tx);

    log::info!("Speech processor (accumulator): exiting");
}

fn handle_moonshine_worker_error(
    ctx: &TranscriptProcessingContext,
    phase: &str,
    err: MoonshineWorkerError,
) {
    log::warn!("Moonshine streaming: {phase} failed: {err}");
    set_asr_status_and_emit(
        &ctx.app_handle,
        &ctx.pipeline_status,
        StageStatus::Error {
            message: format!("Moonshine {phase} failed: {err}"),
        },
    );
}

fn emit_moonshine_revisions(
    revisions: Vec<MoonshineSpanRevision>,
    ctx: &TranscriptProcessingContext,
    asr_count: &mut u64,
    extraction_count: &Arc<AtomicU64>,
    graph_update_count: &Arc<AtomicU64>,
) {
    for revision in revisions {
        if revision.payload.is_final {
            *asr_count += 1;
        }
        let _ = emit_moonshine_span_revision(
            revision,
            ctx,
            *asr_count,
            extraction_count,
            graph_update_count,
        );
    }
}

fn poll_moonshine_pending<A: MoonshineStreamingAdapter>(
    worker: &mut MoonshineStreamingWorker<A>,
    source_id: &str,
    ctx: &TranscriptProcessingContext,
    asr_count: &mut u64,
    extraction_count: &Arc<AtomicU64>,
    graph_update_count: &Arc<AtomicU64>,
) -> Result<(), MoonshineWorkerError> {
    let now_ms = current_unix_millis();
    let revisions = worker.poll_pending_at(source_id, now_ms, now_ms)?;
    emit_moonshine_revisions(
        revisions,
        ctx,
        asr_count,
        extraction_count,
        graph_update_count,
    );
    Ok(())
}

#[allow(dead_code)]
pub(crate) fn run_moonshine_speech_processor_with_worker<A: MoonshineStreamingAdapter>(
    channels: SpeechChannels,
    shared: SpeechShared,
    config: SpeechConfig,
    mut worker: MoonshineStreamingWorker<A>,
) {
    let SpeechChannels {
        processed_rx,
        is_transcribing,
    } = channels;

    let mut asr_count: u64 = 0;
    let extraction_count = Arc::new(AtomicU64::new(0));
    let graph_update_count = Arc::new(AtomicU64::new(0));
    let mut chunks_processed: u64 = 0;
    let mut last_source_id: Option<String> = None;

    set_asr_status(
        &shared.pipeline_status,
        StageStatus::Running { processed_count: 0 },
    );

    let ctx = shared_to_transcript_context(
        shared,
        config.llm_provider,
        config.llm_allow_cloud_fallbacks,
        "moonshine",
    );

    log::info!("Moonshine streaming: entering processed-audio loop");

    loop {
        match processed_rx.recv_timeout(MOONSHINE_RECV_TIMEOUT) {
            Ok(chunk) => {
                if !is_transcribing.load(Ordering::Relaxed) {
                    log::info!("Moonshine streaming: is_transcribing flag cleared, exiting");
                    break;
                }

                let source_id = chunk.source_id.to_string();
                let now_ms = current_unix_millis();
                match worker.process_chunk_at(&source_id, &chunk.data, now_ms, now_ms) {
                    Ok(revisions) => {
                        last_source_id = Some(source_id);
                        chunks_processed += 1;
                        emit_moonshine_revisions(
                            revisions,
                            &ctx,
                            &mut asr_count,
                            &extraction_count,
                            &graph_update_count,
                        );
                    }
                    Err(err) => {
                        handle_moonshine_worker_error(&ctx, "process_chunk", err);
                        break;
                    }
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                if let Some(source_id) = last_source_id.as_deref()
                    && let Err(err) = poll_moonshine_pending(
                        &mut worker,
                        source_id,
                        &ctx,
                        &mut asr_count,
                        &extraction_count,
                        &graph_update_count,
                    )
                {
                    handle_moonshine_worker_error(&ctx, "poll_pending", err);
                    break;
                }

                flush_pending_if_due(&ctx, &extraction_count, &graph_update_count);

                if !is_transcribing.load(Ordering::Relaxed) {
                    log::info!("Moonshine streaming: is_transcribing flag cleared, exiting");
                    break;
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                if let Some(source_id) = last_source_id.as_deref()
                    && let Err(err) = poll_moonshine_pending(
                        &mut worker,
                        source_id,
                        &ctx,
                        &mut asr_count,
                        &extraction_count,
                        &graph_update_count,
                    )
                {
                    handle_moonshine_worker_error(&ctx, "poll_pending", err);
                }
                log::info!("Moonshine streaming: audio channel disconnected, exiting");
                break;
            }
        }
    }

    flush_pending_now(&ctx, &extraction_count, &graph_update_count);

    if let Err(err) = worker.stop() {
        set_asr_status_and_emit(
            &ctx.app_handle,
            &ctx.pipeline_status,
            StageStatus::Error {
                message: format!("Moonshine stop failed: {err}"),
            },
        );
    }

    log::info!(
        "Moonshine streaming: exiting. Chunks={}, ASR={}",
        chunks_processed,
        asr_count,
    );
}

// ---------------------------------------------------------------------------
// ASR + Processing worker (runs on dedicated thread)
// ---------------------------------------------------------------------------

/// ASR worker thread: receives accumulated segments, runs Whisper transcription,
/// diarization, stores results, emits events, and fires off extraction as
/// fire-and-forget tasks to avoid blocking the processing loop.
#[cfg(feature = "asr-whisper")]
fn run_asr_worker(
    asr_seg_rx: Receiver<AccumulatedSegment>,
    is_transcribing: Arc<std::sync::atomic::AtomicBool>,
    shared: SpeechShared,
    config: SpeechConfig,
    model_path_str: String,
    asr_config: AsrConfig,
) {
    use whisper_rs::{WhisperContext, WhisperContextParameters};

    // ── Load Whisper model on this thread ────────────────────────────────
    let ctx =
        match WhisperContext::new_with_params(&model_path_str, WhisperContextParameters::default())
        {
            Ok(ctx) => {
                log::info!("ASR worker: Whisper model loaded from {}", model_path_str);
                ctx
            }
            Err(e) => {
                log::error!(
                    "ASR worker: failed to load Whisper model from {}: {}. Exiting.",
                    model_path_str,
                    e
                );
                return;
            }
        };

    let mut whisper_state = match ctx.create_state() {
        Ok(s) => s,
        Err(e) => {
            log::error!("ASR worker: failed to create Whisper state: {}", e);
            return;
        }
    };

    let mut asr_worker = AsrWorker::new(asr_config);

    let diarization_config = make_diarization_config(&config.models_dir);

    // ADR-0017 / B16: when the unbounded clustering backend is selected, spawn
    // its live rolling-window worker fed by the same 16 kHz mono segment audio
    // (the per-utterance DiarizationWorker falls back to Simple for this
    // backend — it doesn't own the clustering engine). The feed is pushed below.
    #[cfg(feature = "diarization-clustering")]
    let mut clustering = maybe_spawn_clustering_diarization(
        &diarization_config,
        shared.app_handle.clone(),
        shared.speaker_timeline.clone(),
        shared.knowledge_graph.clone(),
        shared.graph_snapshot.clone(),
    );

    let (dummy_diar_tx, _dummy_diar_rx) = crossbeam_channel::unbounded::<DiarizedTranscript>();
    let mut diarization_worker = DiarizationWorker::new(diarization_config, dummy_diar_tx);

    let mut asr_count: u64 = 0;
    let mut diarization_count: u64 = 0;
    // Extraction counts are tracked via Arc<AtomicU64> shared with fire-and-forget threads
    let extraction_count = Arc::new(AtomicU64::new(0));
    let graph_update_count = Arc::new(AtomicU64::new(0));

    let ctx = shared_to_transcript_context(
        shared,
        config.llm_provider,
        config.llm_allow_cloud_fallbacks,
        "local_whisper",
    );

    log::info!("ASR worker: entering processing loop");

    loop {
        // `mut` is required by the FA-5 zero-clone path below (`mem::take` of
        // `segment.audio` on the last transcript); harmless otherwise.
        let mut segment = match asr_seg_rx.recv_timeout(Duration::from_millis(500)) {
            Ok(seg) => seg,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                if !is_transcribing.load(Ordering::Relaxed) {
                    log::info!("ASR worker: is_transcribing flag cleared, exiting");
                    break;
                }
                flush_pending_if_due(&ctx, &extraction_count, &graph_update_count);
                continue;
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                log::info!("ASR worker: segment channel disconnected, exiting");
                break;
            }
        };

        if !is_transcribing.load(Ordering::Relaxed) {
            log::info!("ASR worker: is_transcribing flag cleared, exiting");
            break;
        }

        // Feed the live clustering diarizer (if active) the same 16 kHz mono
        // audio. Non-blocking (drops + counts on a full ring). The accumulator
        // retains an OVERLAP_FRAMES tail across segments, so we'd double-feed
        // that overlap; the rolling-window diarizer is overlap-tolerant (it
        // re-diarizes a trailing window and emits only the fresh hop), so this
        // is acceptable for B16's first wiring. Exact de-dup is a follow-up.
        #[cfg(feature = "diarization-clustering")]
        if let Some(handle) = clustering.as_mut() {
            handle.push(&segment.audio);
        }

        // 1. Run ASR transcription
        let speech_segment = AccumulatedSegment::to_asr_segment(&segment);
        let asr_start = Instant::now();
        let transcribe_result = asr_worker.transcribe_segment(&mut whisper_state, &speech_segment);
        emit_stage_latency(
            &ctx.app_handle,
            "asr",
            Some(&segment.source_id),
            None,
            asr_start.elapsed(),
        );

        match transcribe_result {
            Ok(transcripts) => {
                // FA-5: the same ~2 s (~64 KB) `segment.audio` feeds every
                // transcript's diarization input, but the worker only *borrows*
                // it (RMS/ZCR/MAD; the clustering ring was fed above). Move it
                // into the last input (the common single-transcript case ⇒ zero
                // clones) and clone only for earlier transcripts.
                let last_idx = transcripts.len().saturating_sub(1);
                for (i, transcript) in transcripts.into_iter().enumerate() {
                    asr_count += 1;

                    let speech_audio = if i == last_idx {
                        std::mem::take(&mut segment.audio)
                    } else {
                        segment.audio.clone()
                    };

                    // 2. Run diarization
                    let input = DiarizationInput {
                        transcript,
                        speech_audio,
                        speech_start_time: segment.start_time,
                        speech_end_time: segment.end_time,
                    };
                    let diarization_start = Instant::now();
                    let diarized = diarization_worker.process_input(input);
                    emit_stage_latency(
                        &ctx.app_handle,
                        "diarization",
                        Some(&segment.source_id),
                        Some(&diarized.segment.id),
                        diarization_start.elapsed(),
                    );
                    diarization_count += 1;

                    // `mut` is only exercised under the clustering feature.
                    #[cfg_attr(not(feature = "diarization-clustering"), allow(unused_mut))]
                    let mut final_segment = diarized.segment;

                    // ADR-0017 / B16: when the clustering backend is live, map
                    // this transcript onto the stabilized diarization spans by
                    // time overlap and override the Simple-fallback label. When
                    // it relabels, the consumer thread already owns clustering's
                    // SPEAKER_DETECTED, so suppress the Simple `speaker_info` to
                    // avoid clobbering the UI's speaker stats.
                    #[cfg(feature = "diarization-clustering")]
                    let speaker_info_to_emit = match clustering.as_ref() {
                        Some(handle) => {
                            match handle
                                .label_segment(final_segment.start_time, final_segment.end_time)
                            {
                                Some((id, label)) => {
                                    final_segment.speaker_id = Some(id);
                                    final_segment.speaker_label = Some(label);
                                    None
                                }
                                // Diarizer hasn't covered this time yet — keep the
                                // Simple fallback label + its speaker_info.
                                None => Some(diarized.speaker_info),
                            }
                        }
                        None => Some(diarized.speaker_info),
                    };
                    #[cfg(not(feature = "diarization-clustering"))]
                    let speaker_info_to_emit = Some(diarized.speaker_info);

                    let final_meta = final_only_revision_meta(
                        "local_whisper",
                        &final_segment.source_id,
                        final_segment.start_time,
                        final_segment.end_time,
                    );
                    log_final_transcript_metadata(
                        "ASR worker",
                        "local_whisper",
                        asr_count,
                        &final_segment,
                        &final_meta,
                    );

                    emit_turn_event(
                        &ctx.app_handle,
                        TurnEventInput {
                            provider: "local_whisper",
                            source_id: final_segment.source_id.clone(),
                            kind: events::TurnEventKind::LocalWindow,
                            text: Some(final_segment.text.clone()),
                            start_time: Some(final_segment.start_time),
                            end_time: Some(final_segment.end_time),
                            confidence: Some(final_segment.confidence),
                            turn_index: Some(asr_count),
                        },
                    );

                    // 3–6. Buffer, persist, emit, update status, and spawn
                    //      extraction in the shared tail helper.
                    emit_transcript_and_extract_with_meta(
                        final_segment,
                        speaker_info_to_emit,
                        &ctx,
                        asr_count,
                        diarization_count,
                        &extraction_count,
                        &graph_update_count,
                        final_meta,
                    );
                }
            }
            Err(e) => {
                log::warn!(
                    "ASR worker: transcription failed metadata {}",
                    speech_error_diagnostic(
                        "local_whisper",
                        "transcription_failed",
                        "local_whisper_transcription_failed",
                        &e,
                    )
                );
            }
        }
    }

    flush_pending_now(&ctx, &extraction_count, &graph_update_count);

    // Stop the live clustering diarizer (drains once more, emits, exits).
    #[cfg(feature = "diarization-clustering")]
    if let Some(handle) = clustering.as_ref() {
        handle.stop();
    }

    log::info!(
        "ASR worker: exiting. ASR segments={}, diarized={}",
        asr_count,
        diarization_count,
    );
}

/// Stub when local Whisper is not compiled in (cloud-only build). Drains the
/// segment channel so the accumulator doesn't block on the bounded queue, and
/// logs that Local Whisper is unavailable. (A cloud-only build should select a
/// cloud/streaming ASR provider; this only guards the Local-Whisper selection.)
#[cfg(not(feature = "asr-whisper"))]
fn run_asr_worker(
    asr_seg_rx: Receiver<AccumulatedSegment>,
    is_transcribing: Arc<std::sync::atomic::AtomicBool>,
    _shared: SpeechShared,
    _config: SpeechConfig,
    _model_path_str: String,
    _asr_config: AsrConfig,
) {
    log::error!(
        "Local Whisper ASR is not included in this build (cloud-only). Select a \
         cloud/streaming ASR provider (e.g. Deepgram), or rebuild with the \
         `local-ml` / `asr-whisper` feature."
    );
    // Drain + discard so the accumulator's sends don't back up the bounded channel.
    while is_transcribing.load(Ordering::Relaxed) {
        match asr_seg_rx.recv_timeout(Duration::from_millis(500)) {
            Ok(_) => {}
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// Fallback speech processor — diarization only (no ASR).
///
/// Used when the Whisper model fails to load. Generates placeholder transcript
/// segments with `[speech]` text and still performs speaker attribution.
pub(crate) fn run_speech_processor_diarization_only(
    channels: SpeechChannels,
    shared: SpeechShared,
    config: SpeechConfig,
) {
    let SpeechChannels {
        processed_rx,
        is_transcribing,
    } = channels;
    let projection_dispatch = ProjectionDispatchContext {
        transcript_ledger: shared.transcript_ledger.clone(),
        projection_schedulers: shared.projection_schedulers.clone(),
        projection_runtime: shared.projection_runtime.clone(),
        event_sink: Arc::new(TauriProjectionRuntimeEventSink {
            app_handle: shared.app_handle.clone(),
        }),
        patch_generator: Arc::new(ExecutorProjectionPatchGenerator {
            llm_executor: shared.llm_executor.clone(),
            llm_provider: config.llm_provider.clone(),
            allow_cloud_fallbacks: config.llm_allow_cloud_fallbacks,
        }),
    };

    // Register the AppHandle with the persistence module (see note in
    // `run_speech_processor`). Diarization-only may be entered directly when
    // Whisper model load fails, so we register here too.
    crate::persistence::register_app_handle(shared.app_handle.clone());

    // Auto-detect Sortformer / clustering models; falls back to Simple if none.
    let diarization_config = make_diarization_config(&config.models_dir);

    // ADR-0017 / B16: spawn the live clustering worker when that backend is
    // selected, fed the same 16 kHz mono segment audio used for the placeholder
    // transcript below.
    #[cfg(feature = "diarization-clustering")]
    let mut clustering = maybe_spawn_clustering_diarization(
        &diarization_config,
        shared.app_handle.clone(),
        shared.speaker_timeline.clone(),
        shared.knowledge_graph.clone(),
        shared.graph_snapshot.clone(),
    );

    // Same dummy-channel pattern as in `run_speech_processor` — see M2
    // comment there for rationale.
    let (dummy_diar_tx, _dummy_diar_rx) = crossbeam_channel::unbounded::<DiarizedTranscript>();
    let mut diarization_worker = DiarizationWorker::new(diarization_config, dummy_diar_tx);

    let mut count: u64 = 0;
    let extraction_count = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let graph_update_count = Arc::new(std::sync::atomic::AtomicU64::new(0));

    // Mark ASR as errored since the model didn't load. Preserve a more specific
    // error a caller already recorded (e.g. the sherpa-init path) rather than
    // clobbering it with the generic Whisper message. FA-1 follow-up: EMIT so
    // every fallback caller leaves the UI's "Running" state instead of looking
    // healthy while no ASR is running.
    {
        let mut status = shared
            .pipeline_status
            .write()
            .unwrap_or_else(|e| e.into_inner());
        if !matches!(status.asr, StageStatus::Error { .. }) {
            status.asr = StageStatus::Error {
                message: "Whisper model not loaded".to_string(),
            };
        }
        status.entity_extraction = StageStatus::Running { processed_count: 0 };
        status.graph = StageStatus::Running { processed_count: 0 };
    }
    emit_pipeline_status(&shared.app_handle, &shared.pipeline_status);

    log::info!("Speech processor (diarization-only): entering processing loop");

    let mut accumulators: HashMap<String, AudioAccumulator> = HashMap::new();

    loop {
        // Bug 2 fix: use recv_timeout so we periodically check the stop flag
        let chunk = match processed_rx.recv_timeout(Duration::from_millis(500)) {
            Ok(chunk) => chunk,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                if !is_transcribing.load(Ordering::Relaxed) {
                    log::info!(
                        "Speech processor (diarization-only): is_transcribing flag cleared, exiting"
                    );
                    break;
                }
                continue;
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        };

        // Also check flag on each chunk for faster exit
        if !is_transcribing.load(Ordering::Relaxed) {
            log::info!(
                "Speech processor (diarization-only): is_transcribing flag cleared, exiting"
            );
            break;
        }

        let segment = match feed_source_accumulator(&mut accumulators, &chunk) {
            Some(seg) => seg,
            None => continue,
        };

        // Feed the live clustering diarizer (if active) the 16 kHz mono audio.
        #[cfg(feature = "diarization-clustering")]
        if let Some(handle) = clustering.as_mut() {
            handle.push(&segment.audio);
        }

        count += 1;

        // Create a placeholder transcript segment (no ASR)
        let placeholder_transcript = TranscriptSegment {
            id: uuid::Uuid::new_v4().to_string(),
            source_id: segment.source_id.clone(),
            speaker_id: None,
            speaker_label: None,
            text: "[speech]".to_string(),
            start_time: segment.start_time.as_secs_f64(),
            end_time: segment.end_time.as_secs_f64(),
            confidence: 0.0,
        };

        // FA-5: move the audio in (no clone). The Simple backend only computes
        // RMS/ZCR/MAD from it (never retains it) and the clustering path was
        // already fed `&segment.audio` above; `segment.audio` is unused after
        // this, so the per-segment ~64 KB clone was pure waste.
        let input = DiarizationInput {
            transcript: placeholder_transcript,
            speech_audio: segment.audio,
            speech_start_time: segment.start_time,
            speech_end_time: segment.end_time,
        };
        let diarization_start = Instant::now();
        let diarized = diarization_worker.process_input(input);
        emit_stage_latency(
            &shared.app_handle,
            "diarization",
            Some(&segment.source_id),
            Some(&diarized.segment.id),
            diarization_start.elapsed(),
        );

        // `mut` is only exercised under the clustering feature (the relabel
        // branch); other builds rebind it unchanged.
        #[cfg_attr(not(feature = "diarization-clustering"), allow(unused_mut))]
        let mut final_segment = diarized.segment;

        // ADR-0017 / B16: override the Simple-fallback label with the clustering
        // backend's overlap-mapped speaker when the live diarizer has covered
        // this time. The clustering consumer thread owns the SPEAKER_DETECTED
        // emission for relabeled segments, so suppress the Simple one then.
        #[cfg(feature = "diarization-clustering")]
        let emit_simple_speaker = match clustering.as_ref() {
            Some(handle) => {
                match handle.label_segment(final_segment.start_time, final_segment.end_time) {
                    Some((id, label)) => {
                        final_segment.speaker_id = Some(id);
                        final_segment.speaker_label = Some(label);
                        false
                    }
                    None => true,
                }
            }
            None => true,
        };
        #[cfg(not(feature = "diarization-clustering"))]
        let emit_simple_speaker = true;

        emit_turn_event(
            &shared.app_handle,
            TurnEventInput {
                provider: "local_diarization",
                source_id: final_segment.source_id.clone(),
                kind: events::TurnEventKind::LocalWindow,
                text: Some(final_segment.text.clone()),
                start_time: Some(final_segment.start_time),
                end_time: Some(final_segment.end_time),
                confidence: Some(final_segment.confidence),
                turn_index: Some(count),
            },
        );

        if let Ok(mut buffer) = shared.transcript_buffer.write() {
            buffer.push_back(final_segment.clone());
            if buffer.len() > 500 {
                buffer.pop_front();
            }
        }
        // Persist transcript segment asynchronously
        if let Ok(writer_guard) = shared.transcript_writer.lock()
            && let Some(ref writer) = *writer_guard
        {
            writer.append(&final_segment);
        }

        let final_meta = final_only_revision_meta(
            "local_diarization",
            &final_segment.source_id,
            final_segment.start_time,
            final_segment.end_time,
        );
        let final_span_id = final_meta
            .span_id
            .unwrap_or_else(|| final_segment.id.clone());
        let final_provider_item_id = final_meta.provider_item_id;
        let asr_payload = events::AsrSpanRevisionPayload {
            span_id: final_span_id.clone(),
            provider: "local_diarization".to_string(),
            source_id: final_segment.source_id.clone(),
            provider_item_id: final_provider_item_id,
            transcript_segment_id: Some(final_segment.id.clone()),
            speaker_id: final_segment.speaker_id.clone(),
            speaker_label: final_segment.speaker_label.clone(),
            channel: None,
            text: final_segment.text.clone(),
            start_time: final_segment.start_time,
            end_time: final_segment.end_time,
            confidence: final_segment.confidence,
            is_final: true,
            stability: events::AsrSpanStability::Final,
            revision_number: 1,
            supersedes: None,
            turn_id: None,
            end_of_turn: true,
            raw_event_ref: None,
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms: current_unix_millis(),
        };
        if record_asr_span_revision_event_and_observe_projection(
            &shared.transcript_ledger,
            &shared.transcript_event_writer,
            &shared.projection_schedulers,
            Some(&projection_dispatch),
            &asr_payload,
        ) {
            emit_asr_span_revision(&shared.app_handle, asr_payload);
        }
        let event_sink = TauriDiarizationEventSink {
            app_handle: &shared.app_handle,
        };
        let diarization_dispatch = DiarizationDispatchContext {
            event_sink: &event_sink,
            speaker_timeline: &shared.speaker_timeline,
            knowledge_graph: &shared.knowledge_graph,
            graph_snapshot: &shared.graph_snapshot,
        };
        emit_diarization_span_revision_for_transcript(
            &diarization_dispatch,
            "local_diarization",
            &final_segment,
            &final_span_id,
            None,
            None,
        );
        let _ = shared
            .app_handle
            .emit(events::TRANSCRIPT_UPDATE, &final_segment);
        if emit_simple_speaker {
            let _ = shared
                .app_handle
                .emit(events::SPEAKER_DETECTED, &diarized.speaker_info);
        }
        spawn_agent_proposal_task(
            final_segment.clone(),
            shared.projection_runtime.current_session_id(),
            final_segment.id.clone(),
            shared.app_handle.clone(),
            shared.pending_agent_proposals.clone(),
        );

        if let Ok(mut status) = shared.pipeline_status.write() {
            status.diarization = StageStatus::Running {
                processed_count: count,
            };
        }

        // Knowledge Graph Extraction — fire-and-forget
        spawn_extraction_task(
            final_segment.text.clone(),
            final_segment
                .speaker_label
                .clone()
                .unwrap_or_else(|| "Unknown".to_string()),
            String::new(),
            final_segment.id.clone(),
            final_segment.start_time,
            &ExtractionDeps {
                llm_engine: &shared.llm_engine,
                api_client: &shared.api_client,
                mistralrs_engine: &shared.mistralrs_engine,
                llm_executor: &shared.llm_executor,
                llm_provider: &config.llm_provider,
                llm_allow_cloud_fallbacks: config.llm_allow_cloud_fallbacks,
                graph_extractor: &shared.graph_extractor,
                knowledge_graph: &shared.knowledge_graph,
                graph_snapshot: &shared.graph_snapshot,
                pipeline_status: &shared.pipeline_status,
                app_handle: &shared.app_handle,
            },
            &extraction_count,
            &graph_update_count,
        );
    }

    // Stop the live clustering diarizer (drains once more, emits, exits).
    #[cfg(feature = "diarization-clustering")]
    if let Some(handle) = clustering.as_ref() {
        handle.stop();
    }

    log::info!(
        "Speech processor (diarization-only): exiting. Segments processed={}",
        count,
    );
}

// ---------------------------------------------------------------------------
// Cloud ASR speech processor (batch HTTP API)
// ---------------------------------------------------------------------------

/// Cloud ASR speech processor — same 2-thread architecture as the local
/// Whisper path, but the ASR worker sends accumulated segments to a cloud
/// STT API (OpenAI-compatible: Groq, OpenAI, Deepgram REST, etc.)
/// instead of running local inference.
pub(crate) fn run_cloud_asr_speech_processor(
    channels: SpeechChannels,
    shared: SpeechShared,
    config: SpeechConfig,
    cloud_config: CloudAsrConfig,
) {
    let SpeechChannels {
        processed_rx,
        is_transcribing,
    } = channels;

    // Capacity 32 = up to ~64s of buffered 2s segments. Cloud ASR HTTP calls
    // can take 1–5s per segment; a short 4-slot queue overflows during
    // latency spikes and drops real audio. 32 slots give the accumulator
    // meaningful headroom while still bounding memory (~32 × 2s × 16kHz × 4B
    // ≈ 4 MB worst case).
    let (asr_seg_tx, asr_seg_rx) = crossbeam_channel::bounded::<AccumulatedSegment>(32);

    let is_transcribing_asr = is_transcribing.clone();
    let pipeline_status_for_status_update = shared.pipeline_status.clone();
    let _asr_worker_handle = std::thread::Builder::new()
        .name("cloud-asr-worker".to_string())
        .spawn({
            let shared_for_worker = shared.clone();
            let config_for_worker = config.clone();

            move || {
                run_cloud_asr_worker(
                    asr_seg_rx,
                    is_transcribing_asr,
                    shared_for_worker,
                    config_for_worker,
                    cloud_config,
                );
            }
        });

    if let Ok(mut status) = pipeline_status_for_status_update.write() {
        status.asr = StageStatus::Running { processed_count: 0 };
    }

    log::info!("Cloud ASR speech processor: entering accumulator loop");
    let mut accumulators: HashMap<String, AudioAccumulator> = HashMap::new();

    loop {
        let chunk = match processed_rx.recv_timeout(Duration::from_millis(500)) {
            Ok(chunk) => chunk,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                if !is_transcribing.load(Ordering::Relaxed) {
                    break;
                }
                continue;
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        };

        if !is_transcribing.load(Ordering::Relaxed) {
            break;
        }

        if let Some(segment) = feed_source_accumulator(&mut accumulators, &chunk)
            && let Err(crossbeam_channel::TrySendError::Full(seg)) = asr_seg_tx.try_send(segment)
        {
            log::warn!(
                "Cloud ASR: segment channel full, dropping {:.2}s segment (API slower than real-time)",
                seg.num_frames as f64 / 16_000.0
            );
        }
    }

    for final_seg in flush_source_accumulators(accumulators) {
        // Bounded blocking send so the last segment isn't dropped on stop (H3).
        let _ = asr_seg_tx.send_timeout(final_seg, std::time::Duration::from_secs(1));
    }
    drop(asr_seg_tx);

    log::info!("Cloud ASR speech processor: accumulator loop exited");
}

/// Cloud ASR worker thread — receives accumulated segments, transcribes via
/// HTTP API, then runs the same diarization + extraction pipeline as local.
fn run_cloud_asr_worker(
    asr_seg_rx: Receiver<AccumulatedSegment>,
    is_transcribing: Arc<std::sync::atomic::AtomicBool>,
    shared: SpeechShared,
    config: SpeechConfig,
    cloud_config: CloudAsrConfig,
) {
    let provider_content_egress_policy = config.provider_content_egress_policy;
    let diarization_config = make_diarization_config(&config.models_dir);
    let (dummy_diar_tx, _dummy_diar_rx) = crossbeam_channel::unbounded::<DiarizedTranscript>();
    let mut diarization_worker = DiarizationWorker::new(diarization_config, dummy_diar_tx);

    let mut asr_count: u64 = 0;
    let mut diarization_count: u64 = 0;
    let extraction_count = Arc::new(AtomicU64::new(0));
    let graph_update_count = Arc::new(AtomicU64::new(0));

    let ctx = shared_to_transcript_context(
        shared,
        config.llm_provider,
        config.llm_allow_cloud_fallbacks,
        "cloud_api",
    );

    log::info!(
        "Cloud ASR worker: entering processing loop (endpoint={}, model={})",
        cloud_config.endpoint,
        cloud_config.model
    );
    let cloud_config = cloud_config.with_content_egress_policy(provider_content_egress_policy);

    loop {
        // `mut` is required by the FA-5 zero-clone path below (`mem::take` of
        // `segment.audio` on the last transcript).
        let mut segment = match asr_seg_rx.recv_timeout(Duration::from_millis(500)) {
            Ok(seg) => seg,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                if !is_transcribing.load(Ordering::Relaxed) {
                    break;
                }
                flush_pending_if_due(&ctx, &extraction_count, &graph_update_count);
                continue;
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        };

        if !is_transcribing.load(Ordering::Relaxed) {
            break;
        }

        let speech_segment = AccumulatedSegment::to_asr_segment(&segment);
        let asr_start = Instant::now();
        let transcribe_result =
            crate::asr::cloud::transcribe_segment(&cloud_config, &speech_segment);
        emit_stage_latency(
            &ctx.app_handle,
            "asr",
            Some(&segment.source_id),
            None,
            asr_start.elapsed(),
        );
        match transcribe_result {
            Ok(transcripts) => {
                // FA-5: move the shared per-segment audio into the last
                // transcript's diarization input (common case: one transcript ⇒
                // zero clones); the worker only borrows it.
                let last_idx = transcripts.len().saturating_sub(1);
                for (i, transcript) in transcripts.into_iter().enumerate() {
                    asr_count += 1;

                    let speech_audio = if i == last_idx {
                        std::mem::take(&mut segment.audio)
                    } else {
                        segment.audio.clone()
                    };

                    let input = DiarizationInput {
                        transcript,
                        speech_audio,
                        speech_start_time: segment.start_time,
                        speech_end_time: segment.end_time,
                    };
                    let diarization_start = Instant::now();
                    let diarized = diarization_worker.process_input(input);
                    emit_stage_latency(
                        &ctx.app_handle,
                        "diarization",
                        Some(&segment.source_id),
                        Some(&diarized.segment.id),
                        diarization_start.elapsed(),
                    );
                    diarization_count += 1;

                    let final_segment = diarized.segment;
                    let final_meta = final_only_revision_meta(
                        "cloud_api",
                        &final_segment.source_id,
                        final_segment.start_time,
                        final_segment.end_time,
                    );
                    log_final_transcript_metadata(
                        "Cloud ASR worker",
                        "cloud_api",
                        asr_count,
                        &final_segment,
                        &final_meta,
                    );
                    emit_transcript_and_extract_with_meta(
                        final_segment,
                        Some(diarized.speaker_info),
                        &ctx,
                        asr_count,
                        diarization_count,
                        &extraction_count,
                        &graph_update_count,
                        final_meta,
                    );
                }
            }
            Err(e) => {
                let error_code = cloud_error_code(&e);
                let diagnostic =
                    speech_error_diagnostic("cloud_api", "transcription_failed", &error_code, &e);
                log::warn!(
                    "Cloud ASR worker: transcription failed metadata {}",
                    diagnostic
                );
                // FA-1: emit so the UI reflects the error instead of the last
                // "Running" snapshot.
                set_asr_status_and_emit(
                    &ctx.app_handle,
                    &ctx.pipeline_status,
                    StageStatus::Error {
                        message: format!("Cloud ASR error {diagnostic}"),
                    },
                );
            }
        }
    }

    flush_pending_now(&ctx, &extraction_count, &graph_update_count);

    log::info!(
        "Cloud ASR worker: exiting. ASR segments={}, diarized={}",
        asr_count,
        diarization_count,
    );
}

// ---------------------------------------------------------------------------
// Deepgram Streaming ASR speech processor
// ---------------------------------------------------------------------------

/// Deepgram streaming speech processor — no accumulation needed.
///
/// Unlike batch ASR (local Whisper or cloud HTTP), Deepgram streaming receives
/// audio chunks directly and returns transcript results over the WebSocket.
/// This function:
/// 1. Creates a `DeepgramStreamingClient` and connects.
/// 2. Reads `ProcessedAudioChunk`s directly from the processed channel.
/// 3. Sends raw audio to Deepgram via `send_audio()`.
/// 4. Spawns a receiver thread that consumes Deepgram events, wraps final
///    transcripts as `TranscriptSegment`s, and feeds them through the
///    diarization + storage + events + extraction pipeline.
pub(crate) fn run_deepgram_speech_processor(
    channels: SpeechChannels,
    shared: SpeechShared,
    config: SpeechConfig,
    deepgram_config: crate::asr::deepgram::DeepgramConfig,
    max_speakers: u32,
) {
    use crate::asr::deepgram::DeepgramStreamingClient;

    let SpeechChannels {
        processed_rx,
        is_transcribing,
    } = channels;

    // Create and connect the Deepgram client.
    let mut client = DeepgramStreamingClient::new(deepgram_config);
    match client.connect() {
        Ok(()) => {
            log::info!("Deepgram streaming: connected successfully");
        }
        Err(e) => {
            log::error!("Deepgram streaming: failed to connect: {e}");
            // FA-1: emit so the UI leaves the "Running" state for this dead
            // provider instead of looking healthy. This returns immediately
            // after, so the receiver thread never runs to report it.
            set_asr_status_and_emit(
                &shared.app_handle,
                &shared.pipeline_status,
                StageStatus::Error {
                    message: format!("Deepgram connect failed: {e}"),
                },
            );
            return;
        }
    }

    let event_rx = client.event_rx();
    let source_id_hint = Arc::new(RwLock::new(None::<String>));

    // Spawn the Deepgram event receiver thread (processes transcript results).
    let is_transcribing_rx = is_transcribing.clone();
    let pipeline_status_for_status_update = shared.pipeline_status.clone();
    let _receiver_handle = std::thread::Builder::new()
        .name("deepgram-event-rx".to_string())
        .spawn({
            let shared_for_receiver = shared.clone();
            let config_for_receiver = config.clone();
            let source_id_hint_for_receiver = Arc::clone(&source_id_hint);

            move || {
                run_deepgram_event_receiver(
                    event_rx,
                    is_transcribing_rx,
                    shared_for_receiver,
                    config_for_receiver,
                    source_id_hint_for_receiver,
                    max_speakers,
                );
            }
        });

    // Mark ASR as running.
    if let Ok(mut status) = pipeline_status_for_status_update.write() {
        status.asr = StageStatus::Running { processed_count: 0 };
    }

    // Audio sender loop: reads chunks and forwards to Deepgram.
    log::info!("Deepgram streaming: entering audio sender loop");
    let mut chunks_sent: u64 = 0;

    loop {
        let chunk = match processed_rx.recv_timeout(Duration::from_millis(500)) {
            Ok(chunk) => chunk,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                if !is_transcribing.load(Ordering::Relaxed) {
                    log::info!("Deepgram streaming: is_transcribing flag cleared, exiting sender");
                    break;
                }
                continue;
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                log::info!("Deepgram streaming: audio channel disconnected, exiting sender");
                break;
            }
        };

        if !is_transcribing.load(Ordering::Relaxed) {
            log::info!("Deepgram streaming: is_transcribing flag cleared, exiting sender");
            break;
        }

        // NOTE: intentionally no longer checks `client.is_connected()` here.
        // The client's internal session task handles transient reconnects
        // with exponential backoff, and `send_audio` buffers into the
        // unbounded audio channel during the reconnect window. The channel
        // is only closed when the session task permanently exits (reconnect
        // budget exhausted or user-initiated disconnect), at which point the
        // `send_audio` call below will return "Audio channel closed" and we
        // fall through to the `break`.

        // Send audio directly to Deepgram (no accumulation needed).
        if let Ok(mut hint) = source_id_hint.write() {
            *hint = Some(chunk.source_id.to_string());
        }
        if let Err(e) = client.send_audio(&chunk.data) {
            log::warn!("Deepgram streaming: failed to send audio: {e}");
            break;
        }

        chunks_sent += 1;
        if chunks_sent.is_multiple_of(100) {
            log::debug!("Deepgram streaming: sent {} audio chunks", chunks_sent);
        }
    }

    // Disconnect the client.
    client.disconnect();

    log::info!(
        "Deepgram streaming: audio sender exiting. Chunks sent={}",
        chunks_sent
    );
}

/// Remap a raw Deepgram speaker id to a capped 0-based speaker index.
///
/// Deepgram streaming diarization sometimes over-segments (labels a 2-person
/// conversation as 3+ speakers). When `max_speakers > 0` we keep the first
/// `max_speakers` distinct ids in first-seen order and collapse any further id
/// onto `last_speaker` (the most-recently-seen in-range speaker) — the cheapest
/// correct behaviour for a streaming context where a global re-cluster isn't
/// available. `max_speakers == 0` passes ids through unchanged.
fn remap_deepgram_speaker(
    raw: u32,
    max_speakers: u32,
    speaker_map: &mut std::collections::HashMap<u32, u32>,
    last_speaker: &mut u32,
) -> u32 {
    if max_speakers == 0 {
        return raw;
    }
    if let Some(&mapped) = speaker_map.get(&raw) {
        *last_speaker = mapped;
        return mapped;
    }
    if (speaker_map.len() as u32) < max_speakers {
        let next = speaker_map.len() as u32; // 0-based, dense
        speaker_map.insert(raw, next);
        *last_speaker = next;
        return next;
    }
    // Over the cap: collapse onto the most-recently-seen allowed speaker.
    *last_speaker
}

/// Deepgram event receiver thread — processes transcript events from the
/// Deepgram WebSocket and feeds them into the diarization + storage + events
/// + extraction pipeline (same downstream path as cloud ASR).
fn run_deepgram_event_receiver(
    event_rx: crossbeam_channel::Receiver<crate::asr::deepgram::DeepgramEvent>,
    is_transcribing: Arc<std::sync::atomic::AtomicBool>,
    shared: SpeechShared,
    config: SpeechConfig,
    source_id_hint: Arc<RwLock<Option<String>>>,
    max_speakers: u32,
) {
    use crate::asr::deepgram::{DeepgramEvent, DeepgramTurnKind};
    use crate::diarization::{DiarizationInput, DiarizationWorker, DiarizedTranscript};

    let diarization_config = make_diarization_config(&config.models_dir);
    let (dummy_diar_tx, _dummy_diar_rx) = crossbeam_channel::unbounded::<DiarizedTranscript>();
    let mut diarization_worker = DiarizationWorker::new(diarization_config, dummy_diar_tx);

    let mut asr_count: u64 = 0;
    let mut diarization_count: u64 = 0;
    // Speaker-cap state: maps raw Deepgram speaker ids -> clamped 0-based index,
    // and remembers the last in-range speaker so over-segmented ids collapse
    // onto the most-recently-seen allowed speaker. See `remap_deepgram_speaker`.
    let mut speaker_map: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
    let mut last_speaker: u32 = 0;
    let mut revision_numbers_by_span: HashMap<String, u64> = HashMap::new();
    let extraction_count = Arc::new(AtomicU64::new(0));
    let graph_update_count = Arc::new(AtomicU64::new(0));

    let ctx = shared_to_transcript_context(
        shared,
        config.llm_provider,
        config.llm_allow_cloud_fallbacks,
        "deepgram",
    );

    log::info!("Deepgram event receiver: entering processing loop");

    loop {
        let event = match event_rx.recv_timeout(Duration::from_millis(500)) {
            Ok(ev) => ev,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                if !is_transcribing.load(Ordering::Relaxed) {
                    log::info!("Deepgram event receiver: is_transcribing flag cleared, exiting");
                    break;
                }
                // Heartbeat: flush a coalesced extraction batch once speech has
                // paused (idle/age), without waiting for the next segment.
                flush_pending_if_due(&ctx, &extraction_count, &graph_update_count);
                continue;
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                log::info!("Deepgram event receiver: event channel disconnected, exiting");
                break;
            }
        };

        if !is_transcribing.load(Ordering::Relaxed) {
            log::info!("Deepgram event receiver: is_transcribing flag cleared, exiting");
            break;
        }

        match event {
            DeepgramEvent::Transcript {
                text,
                confidence,
                is_final,
                speech_final: _,
                start,
                duration,
                words,
            } => {
                let source_id = source_hint_or_fallback(&source_id_hint, "deepgram-stream");
                let end_time = start + duration;
                let span_id = provider_start_span_id("deepgram", &source_id, start);

                // Only process final transcripts to avoid duplicates.
                if !is_final {
                    let (revision_number, supersedes) =
                        next_span_revision(&mut revision_numbers_by_span, &span_id);
                    log::debug!(
                        "Deepgram: interim transcript metadata provider=deepgram span_id={} revision={} text_len={} confidence={:.3} speaker_present={}",
                        span_id,
                        revision_number,
                        text.chars().count(),
                        confidence,
                        words.iter().any(|word| word.speaker.is_some())
                    );
                    emit_asr_partial_with_meta(
                        &ctx,
                        "deepgram",
                        source_id,
                        text,
                        start,
                        end_time,
                        confidence,
                        AsrRevisionMeta {
                            span_id: Some(span_id),
                            revision_number: Some(revision_number),
                            supersedes,
                            raw_event_ref: Some("deepgram.results.interim".to_string()),
                            ..AsrRevisionMeta::default()
                        },
                    );
                    continue;
                }

                asr_count += 1;
                let (final_revision_number, supersedes) =
                    final_span_revision(&mut revision_numbers_by_span, &span_id);

                // Determine speaker from word-level diarization if available.
                let speaker_from_deepgram = words.first().and_then(|w| w.speaker).map(|raw| {
                    let id = remap_deepgram_speaker(
                        raw,
                        max_speakers,
                        &mut speaker_map,
                        &mut last_speaker,
                    );
                    format!("Speaker {}", id)
                });

                let segment = TranscriptSegment {
                    id: uuid::Uuid::new_v4().to_string(),
                    source_id,
                    speaker_id: speaker_from_deepgram.clone(),
                    speaker_label: speaker_from_deepgram,
                    text: text.clone(),
                    start_time: start,
                    end_time,
                    confidence,
                };

                // If Deepgram provides speaker labels, use them directly.
                // Otherwise, run through local diarization (needs audio, which
                // we don't have in the event path — so we skip diarization
                // and use the segment as-is).
                let final_segment = if segment.speaker_label.is_some() {
                    // Deepgram diarization provided speaker labels.
                    diarization_count += 1;
                    segment.clone()
                } else {
                    // No speaker from Deepgram; create a minimal diarization input
                    // with empty audio (the Simple diarization backend will
                    // assign a speaker based on signal heuristics, but with
                    // empty audio it will just assign a default speaker).
                    let input = DiarizationInput {
                        transcript: segment.clone(),
                        speech_audio: vec![],
                        speech_start_time: Duration::from_secs_f64(start),
                        speech_end_time: Duration::from_secs_f64(end_time),
                    };
                    let diarized = diarization_worker.process_input(input);
                    diarization_count += 1;

                    let _ = ctx
                        .app_handle
                        .emit(events::SPEAKER_DETECTED, &diarized.speaker_info);
                    diarized.segment
                };

                log::debug!(
                    "Deepgram event receiver: emitted transcript metadata provider=deepgram count={} span_id={} revision={} text_len={} confidence={:.3} speaker_present={}",
                    asr_count,
                    span_id,
                    final_revision_number,
                    final_segment.text.chars().count(),
                    final_segment.confidence,
                    final_segment.speaker_label.is_some(),
                );

                // SPEAKER_DETECTED was already emitted above (if needed) — pass
                // `None` here so the shared helper doesn't double-emit.
                emit_transcript_and_extract_with_meta(
                    final_segment,
                    None,
                    &ctx,
                    asr_count,
                    diarization_count,
                    &extraction_count,
                    &graph_update_count,
                    AsrRevisionMeta {
                        span_id: Some(span_id),
                        revision_number: Some(final_revision_number),
                        supersedes,
                        raw_event_ref: Some("deepgram.results.final".to_string()),
                        ..AsrRevisionMeta::default()
                    },
                );
            }
            DeepgramEvent::Turn {
                kind,
                text,
                start,
                end,
                confidence,
                turn_index,
            } => {
                let normalized_kind = match kind {
                    DeepgramTurnKind::SpeechStarted | DeepgramTurnKind::StartOfTurn => {
                        events::TurnEventKind::SpeechStarted
                    }
                    DeepgramTurnKind::SpeechFinal => events::TurnEventKind::SpeechFinal,
                    DeepgramTurnKind::UtteranceEnd => events::TurnEventKind::UtteranceEnd,
                    DeepgramTurnKind::EagerEndOfTurn => events::TurnEventKind::EagerEndOfTurn,
                    DeepgramTurnKind::EndOfTurn => events::TurnEventKind::EndOfTurn,
                    DeepgramTurnKind::TurnResumed => events::TurnEventKind::TurnResumed,
                };
                let source_id = source_hint_or_fallback(&source_id_hint, "deepgram-stream");
                emit_turn_event(
                    &ctx.app_handle,
                    TurnEventInput {
                        provider: "deepgram",
                        source_id,
                        kind: normalized_kind,
                        text,
                        start_time: start,
                        end_time: end,
                        confidence,
                        turn_index,
                    },
                );
            }
            DeepgramEvent::Error { message } => {
                log::warn!("Deepgram event receiver: error: {message}");
                // FA-1: emit so the UI reflects the error instead of the last
                // "Running" snapshot.
                set_asr_status_and_emit(
                    &ctx.app_handle,
                    &ctx.pipeline_status,
                    StageStatus::Error {
                        message: format!("Deepgram error: {message}"),
                    },
                );
            }
            DeepgramEvent::Disconnected => {
                log::info!("Deepgram event receiver: disconnected; waiting for reconnect or stop");
                set_asr_status_and_emit(
                    &ctx.app_handle,
                    &ctx.pipeline_status,
                    StageStatus::Error {
                        message: "Deepgram disconnected; waiting for reconnect".to_string(),
                    },
                );
            }
            DeepgramEvent::Connected => {
                log::debug!("Deepgram event receiver: connected event received");
            }
            DeepgramEvent::Reconnecting {
                attempt,
                backoff_secs,
            } => {
                // Auto-reconnect in flight — surface through pipeline status
                // so the UI can show a "reconnecting…" hint instead of
                // leaving the stage looking healthy.
                log::info!(
                    "Deepgram event receiver: reconnecting attempt={attempt} backoff={backoff_secs}s"
                );
                set_asr_status_and_emit(
                    &ctx.app_handle,
                    &ctx.pipeline_status,
                    StageStatus::Error {
                        message: format!(
                            "Deepgram reconnecting (attempt {attempt}, retry in {backoff_secs}s)"
                        ),
                    },
                );
            }
            DeepgramEvent::Reconnected => {
                log::info!("Deepgram event receiver: reconnected");
                // Preserve the running count across reconnects so the UI
                // doesn't flash back to 0 transcripts.
                set_asr_status_and_emit(
                    &ctx.app_handle,
                    &ctx.pipeline_status,
                    StageStatus::Running {
                        processed_count: asr_count,
                    },
                );
            }
        }
    }

    // Flush any coalesced batch so the final utterance before stop reaches the graph.
    flush_pending_now(&ctx, &extraction_count, &graph_update_count);

    log::info!(
        "Deepgram event receiver: exiting. ASR segments={}, diarized={}",
        asr_count,
        diarization_count,
    );
}

// ---------------------------------------------------------------------------
// AssemblyAI streaming speech processor
// ---------------------------------------------------------------------------

/// AssemblyAI streaming speech processor — connects to the AssemblyAI real-time
/// WebSocket API, streams audio, and processes transcript events through the
/// same downstream pipeline (diarization, storage, events, extraction).
pub(crate) fn run_assemblyai_speech_processor(
    channels: SpeechChannels,
    shared: SpeechShared,
    config: SpeechConfig,
    assemblyai_config: crate::asr::assemblyai::AssemblyAIConfig,
) {
    use crate::asr::assemblyai::AssemblyAIClient;

    let SpeechChannels {
        processed_rx,
        is_transcribing,
    } = channels;

    // Create and connect the AssemblyAI client.
    let mut client = AssemblyAIClient::new(assemblyai_config);
    match client.connect() {
        Ok(()) => {
            log::info!("AssemblyAI streaming: connected successfully");
        }
        Err(e) => {
            log::error!("AssemblyAI streaming: failed to connect: {e}");
            // FA-1: emit so the UI leaves the "Running" state for this dead
            // provider instead of looking healthy. This returns immediately
            // after, so the receiver thread never runs to report it.
            set_asr_status_and_emit(
                &shared.app_handle,
                &shared.pipeline_status,
                StageStatus::Error {
                    message: format!("AssemblyAI connect failed: {e}"),
                },
            );
            return;
        }
    }

    let event_rx = client.event_rx();
    let source_id_hint = Arc::new(RwLock::new(None::<String>));

    // Spawn the AssemblyAI event receiver thread (processes transcript results).
    let is_transcribing_rx = is_transcribing.clone();
    let pipeline_status_for_status_update = shared.pipeline_status.clone();
    let _receiver_handle = std::thread::Builder::new()
        .name("assemblyai-event-rx".to_string())
        .spawn({
            let shared_for_receiver = shared.clone();
            let config_for_receiver = config.clone();
            let source_id_hint_for_receiver = source_id_hint.clone();

            move || {
                run_assemblyai_event_receiver(
                    event_rx,
                    is_transcribing_rx,
                    shared_for_receiver,
                    config_for_receiver,
                    source_id_hint_for_receiver,
                );
            }
        });

    // Mark ASR as running.
    if let Ok(mut status) = pipeline_status_for_status_update.write() {
        status.asr = StageStatus::Running { processed_count: 0 };
    }

    // Audio sender loop: reads chunks and forwards to AssemblyAI.
    log::info!("AssemblyAI streaming: entering audio sender loop");
    let mut chunks_sent: u64 = 0;

    loop {
        let chunk = match processed_rx.recv_timeout(Duration::from_millis(500)) {
            Ok(chunk) => chunk,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                if !is_transcribing.load(Ordering::Relaxed) {
                    log::info!(
                        "AssemblyAI streaming: is_transcribing flag cleared, exiting sender"
                    );
                    break;
                }
                continue;
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                log::info!("AssemblyAI streaming: audio channel disconnected, exiting sender");
                break;
            }
        };

        if !is_transcribing.load(Ordering::Relaxed) {
            log::info!("AssemblyAI streaming: is_transcribing flag cleared, exiting sender");
            break;
        }

        if let Ok(mut hint) = source_id_hint.write() {
            *hint = Some(chunk.source_id.to_string());
        }

        // NOTE: intentionally no longer checks `client.is_connected()` — the
        // client's session task handles transient reconnects internally and
        // `send_audio` buffers during the reconnect window. A truly dead
        // client surfaces via `send_audio` returning "Audio channel closed".

        // Send audio directly to AssemblyAI (no accumulation needed).
        if let Err(e) = client.send_audio(&chunk.data) {
            log::warn!("AssemblyAI streaming: failed to send audio: {e}");
            break;
        }

        chunks_sent += 1;
        if chunks_sent.is_multiple_of(100) {
            log::debug!("AssemblyAI streaming: sent {} audio chunks", chunks_sent);
        }
    }

    // Disconnect the client.
    client.disconnect();

    log::info!(
        "AssemblyAI streaming: audio sender exiting. Chunks sent={}",
        chunks_sent
    );
}

/// AssemblyAI event receiver thread — processes transcript events from the
/// AssemblyAI WebSocket and feeds them into the diarization + storage + events
/// + extraction pipeline (same downstream path as Deepgram).
fn run_assemblyai_event_receiver(
    event_rx: crossbeam_channel::Receiver<crate::asr::assemblyai::AssemblyAIEvent>,
    is_transcribing: Arc<std::sync::atomic::AtomicBool>,
    shared: SpeechShared,
    config: SpeechConfig,
    source_id_hint: Arc<RwLock<Option<String>>>,
) {
    use crate::asr::assemblyai::AssemblyAIEvent;
    use crate::diarization::{DiarizationInput, DiarizationWorker, DiarizedTranscript};

    let diarization_config = make_diarization_config(&config.models_dir);
    let (dummy_diar_tx, _dummy_diar_rx) = crossbeam_channel::unbounded::<DiarizedTranscript>();
    let mut diarization_worker = DiarizationWorker::new(diarization_config, dummy_diar_tx);

    let mut asr_count: u64 = 0;
    let mut diarization_count: u64 = 0;
    let extraction_count = Arc::new(AtomicU64::new(0));
    let graph_update_count = Arc::new(AtomicU64::new(0));

    // Track cumulative time offset for segments (AssemblyAI does not provide
    // absolute timestamps in the same way Deepgram does).
    let session_start = std::time::Instant::now();
    let mut assemblyai_turn_index: u64 = 0;
    let mut active_assemblyai_span: Option<(String, String, u64)> = None;
    let mut revision_numbers_by_span: HashMap<String, u64> = HashMap::new();
    let mut v3_parser: Option<crate::asr::assemblyai::AssemblyAiV3Parser> = None;
    let mut speaker_revision_numbers_by_span: HashMap<String, u64> = HashMap::new();

    let ctx = shared_to_transcript_context(
        shared,
        config.llm_provider,
        config.llm_allow_cloud_fallbacks,
        "assemblyai",
    );

    log::info!("AssemblyAI event receiver: entering processing loop");

    loop {
        let event = match event_rx.recv_timeout(Duration::from_millis(500)) {
            Ok(ev) => ev,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                if !is_transcribing.load(Ordering::Relaxed) {
                    log::info!("AssemblyAI event receiver: is_transcribing flag cleared, exiting");
                    break;
                }
                flush_pending_if_due(&ctx, &extraction_count, &graph_update_count);
                continue;
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                log::info!("AssemblyAI event receiver: event channel disconnected, exiting");
                break;
            }
        };

        if !is_transcribing.load(Ordering::Relaxed) {
            log::info!("AssemblyAI event receiver: is_transcribing flag cleared, exiting");
            break;
        }

        match event {
            AssemblyAIEvent::ServerMessage {
                frame,
                received_at_ms,
            } => {
                let source_id = source_hint_or_fallback(&source_id_hint, "assemblyai-stream");
                let parser = v3_parser.get_or_insert_with(|| {
                    crate::asr::assemblyai::AssemblyAiV3Parser::new(source_id.clone())
                });
                parser.set_source_id_if_no_turns(source_id);

                let parsed = match parser.parse_message(frame.as_str(), received_at_ms) {
                    Ok(parsed) => parsed,
                    Err(error) => {
                        log::warn!("AssemblyAI v3 parser error: {error:?}");
                        set_asr_status_and_emit(
                            &ctx.app_handle,
                            &ctx.pipeline_status,
                            StageStatus::Error {
                                message: format!("AssemblyAI parser error: {error:?}"),
                            },
                        );
                        continue;
                    }
                };

                if let Some(session_id) = parsed.session_id {
                    log::info!(
                        "AssemblyAI v3 session started session_id_present={} session_id_len={}",
                        !session_id.is_empty(),
                        session_id.chars().count()
                    );
                }

                if let Some(error) = parsed.error {
                    log::warn!(
                        "AssemblyAI event receiver: provider error: {}",
                        error.message
                    );
                    set_asr_status_and_emit(
                        &ctx.app_handle,
                        &ctx.pipeline_status,
                        StageStatus::Error {
                            message: format!("AssemblyAI error: {}", error.message),
                        },
                    );
                    continue;
                }

                for speaker_revision in parsed.speaker_revisions {
                    emit_assemblyai_speaker_revision(
                        &speaker_revision,
                        &ctx,
                        &mut speaker_revision_numbers_by_span,
                        received_at_ms,
                    );
                }

                for mut revision in parsed.revisions {
                    if revision.payload.end_of_turn {
                        emit_turn_event(
                            &ctx.app_handle,
                            TurnEventInput {
                                provider: "assemblyai",
                                source_id: revision.payload.source_id.clone(),
                                kind: events::TurnEventKind::EndOfTurn,
                                text: Some(revision.payload.text.clone()),
                                start_time: Some(revision.payload.start_time),
                                end_time: Some(revision.payload.end_time),
                                confidence: Some(revision.payload.confidence),
                                turn_index: revision
                                    .payload
                                    .turn_id
                                    .as_deref()
                                    .and_then(|turn_id| turn_id.strip_prefix("turn-"))
                                    .and_then(|turn| turn.parse::<u64>().ok()),
                            },
                        );
                    }

                    normalize_assemblyai_v3_revision_for_side_effects(&mut revision);

                    if revision.payload.is_final {
                        asr_count += 1;
                    }
                    let _ = emit_provider_span_revision_payload(
                        revision.payload,
                        &ctx,
                        asr_count,
                        &extraction_count,
                        &graph_update_count,
                    );
                }

                if parsed.terminated {
                    log::info!("AssemblyAI event receiver: v3 session terminated");
                    break;
                }
            }
            AssemblyAIEvent::FinalTranscript { text, confidence } => {
                asr_count += 1;

                let now_secs = session_start.elapsed().as_secs_f64();
                // Approximate segment timing from session clock.
                let start_time = now_secs;
                let end_time = now_secs;
                let (span_id, source_id, turn_index) =
                    active_assemblyai_span.take().unwrap_or_else(|| {
                        assemblyai_turn_index += 1;
                        let source_id =
                            source_hint_or_fallback(&source_id_hint, "assemblyai-stream");
                        let span_id = provider_sequence_span_id(
                            "assemblyai",
                            &source_id,
                            "turn",
                            assemblyai_turn_index,
                        );
                        (span_id, source_id, assemblyai_turn_index)
                    });
                let (final_revision_number, supersedes) =
                    final_span_revision(&mut revision_numbers_by_span, &span_id);

                let segment = TranscriptSegment {
                    id: uuid::Uuid::new_v4().to_string(),
                    source_id,
                    speaker_id: None,
                    speaker_label: None,
                    text: text.clone(),
                    start_time,
                    end_time,
                    confidence: confidence as f32,
                };

                // Run through local diarization with empty audio (assigns
                // a default speaker when no audio signal is available).
                let input = DiarizationInput {
                    transcript: segment.clone(),
                    speech_audio: vec![],
                    speech_start_time: Duration::from_secs_f64(start_time),
                    speech_end_time: Duration::from_secs_f64(end_time),
                };
                let diarized = diarization_worker.process_input(input);
                diarization_count += 1;

                let _ = ctx
                    .app_handle
                    .emit(events::SPEAKER_DETECTED, &diarized.speaker_info);
                let final_segment = diarized.segment;

                log::debug!(
                    "AssemblyAI event receiver: emitted transcript metadata provider=assemblyai count={} span_id={} revision={} text_len={} confidence={:.3} speaker_present={}",
                    asr_count,
                    span_id,
                    final_revision_number,
                    final_segment.text.chars().count(),
                    final_segment.confidence,
                    final_segment.speaker_label.is_some(),
                );

                // SPEAKER_DETECTED was already emitted above — pass `None`
                // so the shared helper doesn't double-emit.
                emit_transcript_and_extract_with_meta(
                    final_segment,
                    None,
                    &ctx,
                    asr_count,
                    diarization_count,
                    &extraction_count,
                    &graph_update_count,
                    AsrRevisionMeta {
                        span_id: Some(span_id),
                        revision_number: Some(final_revision_number),
                        supersedes,
                        turn_id: Some(format!("assemblyai-turn-{turn_index}")),
                        raw_event_ref: Some("assemblyai.final_transcript".to_string()),
                        ..AsrRevisionMeta::default()
                    },
                );
            }
            AssemblyAIEvent::PartialTranscript { text } => {
                let now_secs = session_start.elapsed().as_secs_f64();
                let (span_id, source_id, turn_index) =
                    active_assemblyai_span.clone().unwrap_or_else(|| {
                        assemblyai_turn_index += 1;
                        let source_id =
                            source_hint_or_fallback(&source_id_hint, "assemblyai-stream");
                        let span_id = provider_sequence_span_id(
                            "assemblyai",
                            &source_id,
                            "turn",
                            assemblyai_turn_index,
                        );
                        let state = (span_id, source_id, assemblyai_turn_index);
                        active_assemblyai_span = Some(state.clone());
                        state
                    });
                let (revision_number, supersedes) =
                    next_span_revision(&mut revision_numbers_by_span, &span_id);
                log::debug!(
                    "AssemblyAI: interim transcript metadata provider=assemblyai span_id={} revision={} text_len={} confidence={:.3} speaker_present={}",
                    span_id,
                    revision_number,
                    text.chars().count(),
                    0.0,
                    false
                );
                emit_asr_partial_with_meta(
                    &ctx,
                    "assemblyai",
                    source_id,
                    text,
                    now_secs,
                    now_secs,
                    0.0,
                    AsrRevisionMeta {
                        span_id: Some(span_id),
                        revision_number: Some(revision_number),
                        supersedes,
                        turn_id: Some(format!("assemblyai-turn-{turn_index}")),
                        raw_event_ref: Some("assemblyai.partial_transcript".to_string()),
                        ..AsrRevisionMeta::default()
                    },
                );
            }
            AssemblyAIEvent::Error { message } => {
                log::warn!("AssemblyAI event receiver: error: {message}");
                // FA-1: emit so the UI reflects the error instead of the last
                // "Running" snapshot.
                set_asr_status_and_emit(
                    &ctx.app_handle,
                    &ctx.pipeline_status,
                    StageStatus::Error {
                        message: format!("AssemblyAI error: {message}"),
                    },
                );
            }
            AssemblyAIEvent::SessionTerminated => {
                log::info!("AssemblyAI event receiver: session terminated");
                break;
            }
            AssemblyAIEvent::Reconnecting {
                attempt,
                backoff_secs,
            } => {
                log::info!(
                    "AssemblyAI event receiver: reconnecting attempt={attempt} backoff={backoff_secs}s"
                );
                set_asr_status_and_emit(
                    &ctx.app_handle,
                    &ctx.pipeline_status,
                    StageStatus::Error {
                        message: format!(
                            "AssemblyAI reconnecting (attempt {attempt}, retry in {backoff_secs}s)"
                        ),
                    },
                );
            }
            AssemblyAIEvent::Reconnected => {
                log::info!("AssemblyAI event receiver: reconnected");
                // Preserve the running count across reconnects so the UI
                // doesn't flash back to 0 transcripts.
                set_asr_status_and_emit(
                    &ctx.app_handle,
                    &ctx.pipeline_status,
                    StageStatus::Running {
                        processed_count: asr_count,
                    },
                );
            }
        }
    }

    flush_pending_now(&ctx, &extraction_count, &graph_update_count);

    log::info!(
        "AssemblyAI event receiver: exiting. ASR segments={}, diarized={}",
        asr_count,
        diarization_count,
    );
}

// ---------------------------------------------------------------------------
// Soniox realtime streaming ASR speech processor
// ---------------------------------------------------------------------------

pub(crate) fn run_soniox_speech_processor(
    channels: SpeechChannels,
    shared: SpeechShared,
    config: SpeechConfig,
    soniox_config: crate::asr::soniox::SonioxConfig,
    max_speakers: u32,
) {
    use crate::asr::soniox::SonioxClient;

    let SpeechChannels {
        processed_rx,
        is_transcribing,
    } = channels;

    let mut client = SonioxClient::new(soniox_config);
    match client.connect() {
        Ok(()) => {
            log::info!("Soniox streaming: connected successfully");
        }
        Err(e) => {
            log::error!("Soniox streaming: failed to connect: {e}");
            set_asr_status_and_emit(
                &shared.app_handle,
                &shared.pipeline_status,
                StageStatus::Error {
                    message: format!("Soniox connect failed: {e}"),
                },
            );
            return;
        }
    }

    let event_rx = client.event_rx();
    let is_transcribing_rx = is_transcribing.clone();
    let pipeline_status_for_status_update = shared.pipeline_status.clone();
    let _receiver_handle = std::thread::Builder::new()
        .name("soniox-event-rx".to_string())
        .spawn({
            let shared_for_receiver = shared.clone();
            let config_for_receiver = config.clone();

            move || {
                run_soniox_event_receiver(
                    event_rx,
                    is_transcribing_rx,
                    shared_for_receiver,
                    config_for_receiver,
                    max_speakers,
                );
            }
        });

    if let Ok(mut status) = pipeline_status_for_status_update.write() {
        status.asr = StageStatus::Running { processed_count: 0 };
    }

    log::info!("Soniox streaming: entering audio sender loop");
    let mut chunks_sent: u64 = 0;

    loop {
        let chunk = match processed_rx.recv_timeout(Duration::from_millis(500)) {
            Ok(chunk) => chunk,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                if !is_transcribing.load(Ordering::Relaxed) {
                    log::info!("Soniox streaming: is_transcribing flag cleared, exiting sender");
                    break;
                }
                continue;
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                log::info!("Soniox streaming: audio channel disconnected, exiting sender");
                break;
            }
        };

        if !is_transcribing.load(Ordering::Relaxed) {
            log::info!("Soniox streaming: is_transcribing flag cleared, exiting sender");
            break;
        }

        if let Err(e) = client.send_audio(&chunk.data) {
            log::warn!("Soniox streaming: failed to send audio: {e}");
            break;
        }

        chunks_sent += 1;
        if chunks_sent.is_multiple_of(100) {
            log::debug!("Soniox streaming: sent {} audio chunks", chunks_sent);
        }
    }

    client.disconnect();

    log::info!(
        "Soniox streaming: audio sender exiting. Chunks sent={}",
        chunks_sent
    );
}

fn remap_string_speaker(
    raw: &str,
    max_speakers: u32,
    speaker_map: &mut HashMap<String, String>,
    last_speaker: &mut Option<String>,
) -> String {
    if max_speakers == 0 {
        return raw.to_string();
    }
    if let Some(mapped) = speaker_map.get(raw) {
        *last_speaker = Some(mapped.clone());
        return mapped.clone();
    }
    if (speaker_map.len() as u32) < max_speakers {
        let mapped = raw.to_string();
        speaker_map.insert(raw.to_string(), mapped.clone());
        *last_speaker = Some(mapped.clone());
        return mapped;
    }
    last_speaker.clone().unwrap_or_else(|| raw.to_string())
}

fn run_soniox_event_receiver(
    event_rx: crossbeam_channel::Receiver<crate::asr::soniox::SonioxEvent>,
    is_transcribing: Arc<std::sync::atomic::AtomicBool>,
    shared: SpeechShared,
    config: SpeechConfig,
    max_speakers: u32,
) {
    use crate::asr::soniox::SonioxEvent;

    let mut asr_count: u64 = 0;
    let extraction_count = Arc::new(AtomicU64::new(0));
    let graph_update_count = Arc::new(AtomicU64::new(0));
    let mut speaker_map: HashMap<String, String> = HashMap::new();
    let mut last_speaker: Option<String> = None;

    let ctx = shared_to_transcript_context(
        shared,
        config.llm_provider,
        config.llm_allow_cloud_fallbacks,
        "soniox",
    );

    log::info!("Soniox event receiver: entering processing loop");

    loop {
        let event = match event_rx.recv_timeout(Duration::from_millis(500)) {
            Ok(ev) => ev,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                if !is_transcribing.load(Ordering::Relaxed) {
                    log::info!("Soniox event receiver: is_transcribing flag cleared, exiting");
                    break;
                }
                flush_pending_if_due(&ctx, &extraction_count, &graph_update_count);
                continue;
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                log::info!("Soniox event receiver: event channel disconnected, exiting");
                break;
            }
        };

        if !is_transcribing.load(Ordering::Relaxed) {
            log::info!("Soniox event receiver: is_transcribing flag cleared, exiting");
            break;
        }

        match event {
            SonioxEvent::Revision(mut revision) => {
                if let Some(raw_speaker) = revision.payload.speaker_id.clone() {
                    let speaker_id = remap_string_speaker(
                        &raw_speaker,
                        max_speakers,
                        &mut speaker_map,
                        &mut last_speaker,
                    );
                    revision.payload.speaker_id = Some(speaker_id.clone());
                    revision.payload.speaker_label = Some(format!("Speaker {speaker_id}"));
                }

                if revision.payload.end_of_turn {
                    emit_turn_event(
                        &ctx.app_handle,
                        TurnEventInput {
                            provider: "soniox",
                            source_id: revision.payload.source_id.clone(),
                            kind: events::TurnEventKind::EndOfTurn,
                            text: Some(revision.payload.text.clone()),
                            start_time: Some(revision.payload.start_time),
                            end_time: Some(revision.payload.end_time),
                            confidence: Some(revision.payload.confidence),
                            turn_index: revision
                                .payload
                                .turn_id
                                .as_deref()
                                .and_then(|turn_id| turn_id.strip_prefix("turn-"))
                                .and_then(|turn| turn.parse::<u64>().ok()),
                        },
                    );
                }

                if revision.payload.is_final {
                    asr_count += 1;
                }
                let _ = emit_soniox_span_revision(
                    revision,
                    &ctx,
                    asr_count,
                    &extraction_count,
                    &graph_update_count,
                );
            }
            SonioxEvent::Finished => {
                log::info!("Soniox event receiver: session finished");
                break;
            }
            SonioxEvent::Error { message } => {
                log::warn!("Soniox event receiver: error: {message}");
                set_asr_status_and_emit(
                    &ctx.app_handle,
                    &ctx.pipeline_status,
                    StageStatus::Error {
                        message: format!("Soniox error: {message}"),
                    },
                );
            }
            SonioxEvent::Disconnected => {
                log::info!("Soniox event receiver: disconnected; waiting for reconnect or stop");
                set_asr_status_and_emit(
                    &ctx.app_handle,
                    &ctx.pipeline_status,
                    StageStatus::Error {
                        message: "Soniox disconnected; waiting for reconnect".to_string(),
                    },
                );
            }
            SonioxEvent::Connected => {
                log::debug!("Soniox event receiver: connected event received");
            }
            SonioxEvent::Reconnecting {
                attempt,
                backoff_secs,
            } => {
                log::info!(
                    "Soniox event receiver: reconnecting attempt={attempt} backoff={backoff_secs}s"
                );
                set_asr_status_and_emit(
                    &ctx.app_handle,
                    &ctx.pipeline_status,
                    StageStatus::Error {
                        message: format!(
                            "Soniox reconnecting (attempt {attempt}, retry in {backoff_secs}s)"
                        ),
                    },
                );
            }
            SonioxEvent::Reconnected => {
                log::info!("Soniox event receiver: reconnected");
                set_asr_status_and_emit(
                    &ctx.app_handle,
                    &ctx.pipeline_status,
                    StageStatus::Running {
                        processed_count: asr_count,
                    },
                );
            }
        }
    }

    flush_pending_now(&ctx, &extraction_count, &graph_update_count);

    log::info!("Soniox event receiver: exiting. ASR segments={}", asr_count,);
}

// ---------------------------------------------------------------------------
// OpenAI Realtime streaming transcription speech processor (ADR-0002 Wave A)
// ---------------------------------------------------------------------------

/// OpenAI Realtime transcription speech processor — connects to the OpenAI
/// Realtime API (`gpt-realtime-whisper`), streams the mixed mono audio tap, and
/// processes transcript events through the same downstream pipeline
/// (diarization, storage, events, extraction) as the other streaming providers.
///
/// `gpt-realtime-whisper` has no server VAD, so each ~32ms audio chunk is
/// followed by a `commit()` to flush the buffer for incremental transcription —
/// the cheapest way to get streaming deltas without the speech processor having
/// to detect utterance boundaries itself. The OpenAI client correlates the
/// resulting `delta`/`completed` events by `item_id` internally.
pub(crate) fn run_openai_realtime_speech_processor(
    channels: SpeechChannels,
    shared: SpeechShared,
    config: SpeechConfig,
    openai_config: crate::asr::openai_realtime::OpenAiRealtimeConfig,
) {
    use crate::asr::openai_realtime::OpenAiRealtimeClient;

    let SpeechChannels {
        processed_rx,
        is_transcribing,
    } = channels;

    // Create and connect the OpenAI Realtime client.
    let mut client = OpenAiRealtimeClient::new(openai_config);
    match client.connect() {
        Ok(()) => {
            log::info!("OpenAI Realtime streaming: connected successfully");
        }
        Err(e) => {
            log::error!("OpenAI Realtime streaming: failed to connect: {e}");
            // FA-1: emit so the UI leaves the "Running" state for this dead
            // provider instead of looking healthy. This returns immediately
            // after, so the receiver thread never runs to report it.
            set_asr_status_and_emit(
                &shared.app_handle,
                &shared.pipeline_status,
                StageStatus::Error {
                    message: format!("OpenAI Realtime connect failed: {e}"),
                },
            );
            return;
        }
    }

    let event_rx = client.event_rx();
    let source_id_hint = Arc::new(RwLock::new(None::<String>));

    // Spawn the OpenAI Realtime event receiver thread.
    let is_transcribing_rx = is_transcribing.clone();
    let pipeline_status_for_status_update = shared.pipeline_status.clone();
    let _receiver_handle = std::thread::Builder::new()
        .name("openai-realtime-event-rx".to_string())
        .spawn({
            let shared_for_receiver = shared.clone();
            let config_for_receiver = config.clone();
            let source_id_hint_for_receiver = source_id_hint.clone();

            move || {
                run_openai_realtime_event_receiver(
                    event_rx,
                    is_transcribing_rx,
                    shared_for_receiver,
                    config_for_receiver,
                    source_id_hint_for_receiver,
                );
            }
        });

    // Mark ASR as running.
    if let Ok(mut status) = pipeline_status_for_status_update.write() {
        status.asr = StageStatus::Running { processed_count: 0 };
    }

    // Audio sender loop: reads chunks, forwards to OpenAI, then commits on an
    // utterance-scale cadence to trigger incremental transcription.
    //
    // gpt-realtime-whisper has no server VAD, so the client must drive turns by
    // committing the input buffer. Committing on EVERY ~32 ms chunk (the naive
    // approach) fragments the transcript into one tiny item per chunk and
    // multiplies request volume / 429 risk (see B33 / the converse-cadence
    // review). Instead we accumulate roughly `COMMIT_INTERVAL` of audio between
    // commits — utterance-scale, not frame-scale — which keeps incremental
    // deltas flowing without per-chunk fragmentation. The exact interval is a
    // latency/granularity trade-off best tuned against a live key (runtime-gated);
    // 0.5 s mirrors the cloud-batch path's segment granularity as a safe default.
    const COMMIT_INTERVAL: Duration = Duration::from_millis(500);
    log::info!("OpenAI Realtime streaming: entering audio sender loop");
    let mut chunks_sent: u64 = 0;
    let mut last_commit = std::time::Instant::now();
    let mut uncommitted_since_last = false;

    loop {
        let chunk = match processed_rx.recv_timeout(Duration::from_millis(500)) {
            Ok(chunk) => chunk,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                if !is_transcribing.load(Ordering::Relaxed) {
                    log::info!(
                        "OpenAI Realtime streaming: is_transcribing flag cleared, exiting sender"
                    );
                    break;
                }
                // Idle: speech stopped before COMMIT_INTERVAL elapsed. Flush the
                // buffered audio so a short utterance finalizes promptly instead
                // of waiting for the next chunk or teardown (CodeRabbit
                // speech/mod.rs:3240). The cadence commit only ran after
                // send_audio(), so without this the tail can sit uncommitted.
                if uncommitted_since_last && last_commit.elapsed() >= COMMIT_INTERVAL {
                    if let Err(e) = client.commit() {
                        log::warn!("OpenAI Realtime streaming: idle commit failed: {e}");
                        break;
                    }
                    last_commit = std::time::Instant::now();
                    uncommitted_since_last = false;
                }
                continue;
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                log::info!("OpenAI Realtime streaming: audio channel disconnected, exiting sender");
                break;
            }
        };

        if !is_transcribing.load(Ordering::Relaxed) {
            log::info!("OpenAI Realtime streaming: is_transcribing flag cleared, exiting sender");
            break;
        }

        if let Ok(mut hint) = source_id_hint.write() {
            *hint = Some(chunk.source_id.to_string());
        }

        // NOTE: like the Deepgram/AssemblyAI paths, this intentionally does not
        // check `client.is_connected()` — the client's session task handles
        // transient reconnects internally and `send_audio` buffers during the
        // reconnect window. A truly dead client surfaces via `send_audio`
        // returning "Audio channel closed".
        if let Err(e) = client.send_audio(&chunk.data) {
            log::warn!("OpenAI Realtime streaming: failed to send audio: {e}");
            break;
        }
        uncommitted_since_last = true;
        // Commit on an utterance-scale cadence rather than per chunk (B33): once
        // ~COMMIT_INTERVAL of audio has accumulated, flush the buffer so whisper
        // transcribes a meaningful span instead of a single 32 ms frame. A commit
        // on an empty/uncommitted buffer is a server-side no-op, so this is safe
        // even across silence.
        if last_commit.elapsed() >= COMMIT_INTERVAL {
            if let Err(e) = client.commit() {
                log::warn!("OpenAI Realtime streaming: failed to commit audio: {e}");
                break;
            }
            last_commit = std::time::Instant::now();
            uncommitted_since_last = false;
        }

        chunks_sent += 1;
        if chunks_sent.is_multiple_of(100) {
            log::debug!(
                "OpenAI Realtime streaming: sent {} audio chunks",
                chunks_sent
            );
        }
    }

    // Flush any audio buffered since the last cadence commit so the final
    // partial utterance is transcribed rather than dropped on teardown.
    if uncommitted_since_last && let Err(e) = client.commit() {
        log::debug!("OpenAI Realtime streaming: final flush commit failed: {e}");
    }

    // Disconnect the client.
    client.disconnect();

    log::info!(
        "OpenAI Realtime streaming: audio sender exiting. Chunks sent={}",
        chunks_sent
    );
}

/// OpenAI Realtime event receiver thread — processes transcript events from the
/// OpenAI Realtime WebSocket and feeds them into the diarization + storage +
/// events + extraction pipeline (same downstream path as AssemblyAI: text-only,
/// no provider speaker labels, so it runs through local diarization).
fn run_openai_realtime_event_receiver(
    event_rx: crossbeam_channel::Receiver<crate::asr::openai_realtime::OpenAiRealtimeEvent>,
    is_transcribing: Arc<std::sync::atomic::AtomicBool>,
    shared: SpeechShared,
    config: SpeechConfig,
    source_id_hint: Arc<RwLock<Option<String>>>,
) {
    use crate::asr::openai_realtime::OpenAiRealtimeEvent;
    use crate::diarization::{DiarizationInput, DiarizationWorker, DiarizedTranscript};

    let diarization_config = make_diarization_config(&config.models_dir);
    let (dummy_diar_tx, _dummy_diar_rx) = crossbeam_channel::unbounded::<DiarizedTranscript>();
    let mut diarization_worker = DiarizationWorker::new(diarization_config, dummy_diar_tx);

    let mut asr_count: u64 = 0;
    let mut diarization_count: u64 = 0;
    let extraction_count = Arc::new(AtomicU64::new(0));
    let graph_update_count = Arc::new(AtomicU64::new(0));

    // OpenAI Realtime transcription events do not carry absolute timestamps, so
    // — like AssemblyAI — approximate segment timing from a session clock.
    let session_start = std::time::Instant::now();
    let mut revision_numbers_by_item: HashMap<String, u64> = HashMap::new();

    let ctx = shared_to_transcript_context(
        shared,
        config.llm_provider,
        config.llm_allow_cloud_fallbacks,
        "openai_realtime",
    );

    log::info!("OpenAI Realtime event receiver: entering processing loop");

    loop {
        let event = match event_rx.recv_timeout(Duration::from_millis(500)) {
            Ok(ev) => ev,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                if !is_transcribing.load(Ordering::Relaxed) {
                    log::info!(
                        "OpenAI Realtime event receiver: is_transcribing flag cleared, exiting"
                    );
                    break;
                }
                flush_pending_if_due(&ctx, &extraction_count, &graph_update_count);
                continue;
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                log::info!("OpenAI Realtime event receiver: event channel disconnected, exiting");
                break;
            }
        };

        if !is_transcribing.load(Ordering::Relaxed) {
            log::info!("OpenAI Realtime event receiver: is_transcribing flag cleared, exiting");
            break;
        }

        match event {
            OpenAiRealtimeEvent::Transcript {
                text,
                item_id,
                is_final,
            } => {
                // Interim accumulated deltas -> partial; final completed -> a
                // durable transcript segment (mirrors the Deepgram is_final
                // gating).
                let source_id = source_hint_or_fallback(&source_id_hint, "openai-realtime-stream");
                let span_id = provider_item_span_id("openai_realtime", &source_id, &item_id);
                if !is_final {
                    let now_secs = session_start.elapsed().as_secs_f64();
                    let revision_number =
                        revision_numbers_by_item.entry(item_id.clone()).or_insert(0);
                    *revision_number += 1;
                    let supersedes = (*revision_number > 1)
                        .then(|| revision_ref(&span_id, *revision_number - 1));
                    log::debug!(
                        "OpenAI Realtime: interim transcript metadata provider=openai_realtime span_id={} revision={} text_len={} confidence={:.3} speaker_present={}",
                        span_id,
                        *revision_number,
                        text.chars().count(),
                        0.0,
                        false
                    );
                    emit_asr_partial_with_meta(
                        &ctx,
                        "openai_realtime",
                        source_id,
                        text,
                        now_secs,
                        now_secs,
                        0.0,
                        AsrRevisionMeta {
                            span_id: Some(span_id),
                            provider_item_id: Some(item_id),
                            revision_number: Some(*revision_number),
                            supersedes,
                            raw_event_ref: Some(
                                "conversation.item.input_audio_transcription.delta".to_string(),
                            ),
                            ..AsrRevisionMeta::default()
                        },
                    );
                    continue;
                }

                asr_count += 1;
                let now_secs = session_start.elapsed().as_secs_f64();
                let final_revision_number =
                    revision_numbers_by_item.remove(&item_id).unwrap_or(0) + 1;
                let supersedes = (final_revision_number > 1)
                    .then(|| revision_ref(&span_id, final_revision_number - 1));

                let segment = TranscriptSegment {
                    id: uuid::Uuid::new_v4().to_string(),
                    source_id,
                    speaker_id: None,
                    speaker_label: None,
                    text: text.clone(),
                    start_time: now_secs,
                    end_time: now_secs,
                    // OpenAI Realtime transcription does not surface a per-item
                    // confidence on the STT path; report 1.0 (parity with the
                    // local-Whisper "no confidence" default).
                    confidence: 1.0,
                };

                // Run through local diarization with empty audio (assigns a
                // default speaker when no audio signal is available) — same as
                // the AssemblyAI path.
                let input = DiarizationInput {
                    transcript: segment.clone(),
                    speech_audio: vec![],
                    speech_start_time: Duration::from_secs_f64(now_secs),
                    speech_end_time: Duration::from_secs_f64(now_secs),
                };
                let diarized = diarization_worker.process_input(input);
                diarization_count += 1;

                let _ = ctx
                    .app_handle
                    .emit(events::SPEAKER_DETECTED, &diarized.speaker_info);
                let final_segment = diarized.segment;

                log::debug!(
                    "OpenAI Realtime event receiver: emitted transcript metadata provider=openai_realtime count={} span_id={} revision={} text_len={} confidence={:.3} speaker_present={}",
                    asr_count,
                    span_id,
                    final_revision_number,
                    final_segment.text.chars().count(),
                    final_segment.confidence,
                    final_segment.speaker_label.is_some(),
                );

                // SPEAKER_DETECTED was already emitted above — pass `None` so
                // the shared helper doesn't double-emit.
                emit_transcript_and_extract_with_meta(
                    final_segment,
                    None,
                    &ctx,
                    asr_count,
                    diarization_count,
                    &extraction_count,
                    &graph_update_count,
                    AsrRevisionMeta {
                        span_id: Some(span_id),
                        provider_item_id: Some(item_id),
                        revision_number: Some(final_revision_number),
                        supersedes,
                        raw_event_ref: Some(
                            "conversation.item.input_audio_transcription.completed".to_string(),
                        ),
                        ..AsrRevisionMeta::default()
                    },
                );
            }
            OpenAiRealtimeEvent::Error { message } => {
                log::warn!("OpenAI Realtime event receiver: error: {message}");
                // FA-1: emit so the UI reflects the error instead of the last
                // "Running" snapshot.
                set_asr_status_and_emit(
                    &ctx.app_handle,
                    &ctx.pipeline_status,
                    StageStatus::Error {
                        message: format!("OpenAI Realtime error: {message}"),
                    },
                );
            }
            OpenAiRealtimeEvent::Connected => {
                log::debug!("OpenAI Realtime event receiver: connected event received");
            }
            OpenAiRealtimeEvent::Disconnected => {
                log::info!(
                    "OpenAI Realtime event receiver: disconnected; waiting for reconnect or stop"
                );
                set_asr_status_and_emit(
                    &ctx.app_handle,
                    &ctx.pipeline_status,
                    StageStatus::Error {
                        message: "OpenAI Realtime disconnected; waiting for reconnect".to_string(),
                    },
                );
            }
            OpenAiRealtimeEvent::Reconnecting {
                attempt,
                backoff_secs,
            } => {
                log::info!(
                    "OpenAI Realtime event receiver: reconnecting attempt={attempt} backoff={backoff_secs}s"
                );
                set_asr_status_and_emit(
                    &ctx.app_handle,
                    &ctx.pipeline_status,
                    StageStatus::Error {
                        message: format!(
                            "OpenAI Realtime reconnecting (attempt {attempt}, retry in {backoff_secs}s)"
                        ),
                    },
                );
            }
            OpenAiRealtimeEvent::Reconnected => {
                log::info!("OpenAI Realtime event receiver: reconnected");
                // Preserve the running count across reconnects so the UI
                // doesn't flash back to 0 transcripts.
                set_asr_status_and_emit(
                    &ctx.app_handle,
                    &ctx.pipeline_status,
                    StageStatus::Running {
                        processed_count: asr_count,
                    },
                );
            }
        }
    }

    flush_pending_now(&ctx, &extraction_count, &graph_update_count);

    log::info!(
        "OpenAI Realtime event receiver: exiting. ASR segments={}, diarized={}",
        asr_count,
        diarization_count,
    );
}

// ---------------------------------------------------------------------------
// AWS Transcribe streaming speech processor
// ---------------------------------------------------------------------------

pub(crate) fn run_aws_transcribe_speech_processor(
    channels: SpeechChannels,
    shared: SpeechShared,
    config: SpeechConfig,
    aws_config: crate::asr::aws_transcribe::AwsTranscribeConfig,
) {
    let SpeechChannels {
        processed_rx,
        is_transcribing,
    } = channels;

    let diarization_config = make_diarization_config(&config.models_dir);
    let (dummy_diar_tx, _dummy_diar_rx) = crossbeam_channel::unbounded::<DiarizedTranscript>();
    let mut diarization_worker = DiarizationWorker::new(diarization_config, dummy_diar_tx);

    let mut asr_count: u64 = 0;
    let mut diarization_count: u64 = 0;
    let extraction_count = Arc::new(AtomicU64::new(0));
    let graph_update_count = Arc::new(AtomicU64::new(0));

    if let Ok(mut status) = shared.pipeline_status.write() {
        status.asr = StageStatus::Running { processed_count: 0 };
    }

    log::info!("AWS Transcribe speech processor: starting streaming session");

    let pipeline_status_err = shared.pipeline_status.clone();
    let provider_content_egress_policy = config.provider_content_egress_policy;

    // Built from clones so the callback can move `ctx` while the outer
    // `pipeline_status_err` stays usable for error reporting after the
    // session returns.
    let ctx = shared_to_transcript_context(
        shared,
        config.llm_provider,
        config.llm_allow_cloud_fallbacks,
        "aws-transcribe",
    );

    // Capture the region before `aws_config` is moved into the session —
    // the classifier needs it to distinguish "wrong region" from "DNS dead".
    let aws_region_for_classification = aws_config.region.clone();
    let app_handle_for_err = ctx.app_handle.clone();
    let revision_numbers_by_span = Arc::new(Mutex::new(HashMap::<String, u64>::new()));
    let revisions_for_transcript = revision_numbers_by_span.clone();
    let revisions_for_partial = revision_numbers_by_span;
    let ctx_for_transcript = ctx.clone();
    let ctx_for_partial = ctx.clone();
    let aws_config = aws_config.with_content_egress_policy(provider_content_egress_policy);

    let result = crate::asr::aws_transcribe::run_aws_transcribe_session(
        processed_rx,
        is_transcribing,
        aws_config,
        move |transcript| {
            asr_count += 1;
            let source_id = transcript.segment.source_id.clone();
            let provider_item_id = transcript.provider_item_id.clone();
            let span_id = provider_item_id
                .as_deref()
                .map(|provider_item_id| {
                    provider_item_span_id("aws-transcribe", &source_id, provider_item_id)
                })
                .unwrap_or_else(|| {
                    provider_start_span_id(
                        "aws-transcribe",
                        &source_id,
                        transcript.segment.start_time,
                    )
                });
            let (final_revision_number, supersedes) = revisions_for_transcript
                .lock()
                .map(|mut revisions| final_span_revision(&mut revisions, &span_id))
                .unwrap_or_else(|poisoned| {
                    log::warn!("AWS Transcribe revision map poisoned; recovering");
                    let mut revisions = poisoned.into_inner();
                    final_span_revision(&mut revisions, &span_id)
                });

            let input = DiarizationInput {
                transcript: transcript.segment,
                speech_audio: vec![],
                speech_start_time: Duration::ZERO,
                speech_end_time: Duration::ZERO,
            };
            let diarized = diarization_worker.process_input(input);
            diarization_count += 1;

            emit_transcript_and_extract_with_meta(
                diarized.segment,
                Some(diarized.speaker_info),
                &ctx_for_transcript,
                asr_count,
                diarization_count,
                &extraction_count,
                &graph_update_count,
                AsrRevisionMeta {
                    span_id: Some(span_id),
                    provider_item_id,
                    revision_number: Some(final_revision_number),
                    supersedes,
                    raw_event_ref: Some("aws.transcribe.result.final".to_string()),
                    ..AsrRevisionMeta::default()
                },
            );
        },
        move |partial| {
            let source_id = partial.source_id;
            let provider_item_id = partial.provider_item_id;
            let span_id = provider_item_id
                .as_deref()
                .map(|provider_item_id| {
                    provider_item_span_id("aws-transcribe", &source_id, provider_item_id)
                })
                .unwrap_or_else(|| {
                    provider_start_span_id("aws-transcribe", &source_id, partial.start_time)
                });
            let (revision_number, supersedes) = revisions_for_partial
                .lock()
                .map(|mut revisions| next_span_revision(&mut revisions, &span_id))
                .unwrap_or_else(|poisoned| {
                    log::warn!("AWS Transcribe revision map poisoned; recovering");
                    let mut revisions = poisoned.into_inner();
                    next_span_revision(&mut revisions, &span_id)
                });
            emit_asr_partial_with_meta(
                &ctx_for_partial,
                "aws-transcribe",
                source_id,
                partial.text,
                partial.start_time,
                partial.end_time,
                partial.confidence,
                AsrRevisionMeta {
                    span_id: Some(span_id),
                    provider_item_id,
                    revision_number: Some(revision_number),
                    supersedes,
                    raw_event_ref: Some("aws.transcribe.result.partial".to_string()),
                    ..AsrRevisionMeta::default()
                },
            );
        },
    );

    if let Err(e) = result {
        // ag#13: translate the raw aws-sdk string into a UiAwsError and emit
        // a structured event so the frontend can show a localized, actionable
        // toast instead of a cryptic SDK display string.
        let classified =
            crate::aws_util::classify_aws_error(&e, Some(aws_region_for_classification.as_str()));
        let diagnostic = aws_error_diagnostic(&classified, &e);
        let event_error = aws_error_for_diagnostic_event(classified, &diagnostic);
        log::error!("AWS Transcribe session error metadata {}", diagnostic);
        crate::events::emit_or_log(
            &app_handle_for_err,
            crate::events::AWS_ERROR,
            crate::events::AwsErrorPayload {
                error: event_error,
                raw_message: diagnostic.clone(),
            },
        );
        // FA-1 follow-up: also push the stage status to the UI status bar (the
        // AWS_ERROR toast above is separate from the per-stage status dots).
        set_asr_status_and_emit(
            &app_handle_for_err,
            &pipeline_status_err,
            StageStatus::Error {
                message: format!("AWS Transcribe error {diagnostic}"),
            },
        );
    }

    log::info!("AWS Transcribe speech processor: session ended");
}

// ---------------------------------------------------------------------------
// AccumulatedSegment → ASR bridge
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Sherpa-onnx streaming ASR speech processor
// ---------------------------------------------------------------------------

#[cfg(feature = "sherpa-streaming")]
pub(crate) fn run_sherpa_onnx_speech_processor(
    channels: SpeechChannels,
    shared: SpeechShared,
    config: SpeechConfig,
    sherpa_config: crate::asr::sherpa_streaming::SherpaStreamingConfig,
) {
    use crate::asr::sherpa_streaming::SherpaStreamingWorker;
    use crate::diarization::{DiarizationInput, DiarizationWorker, DiarizedTranscript};

    let mut worker = match SherpaStreamingWorker::new(&sherpa_config) {
        Ok(w) => w,
        Err(e) => {
            let diagnostic = speech_error_diagnostic(
                "sherpa-onnx",
                "worker_init_failed",
                "sherpa_onnx_init_failed",
                &e,
            );
            log::error!(
                "Sherpa-onnx streaming: failed to create worker metadata {}",
                diagnostic
            );
            // FA-1 follow-up: record the specific init error; the diarization-only
            // fallback preserves it (it only writes the generic message when asr
            // is not already Error) and emits the pipeline status to the UI.
            set_asr_status(
                &shared.pipeline_status,
                StageStatus::Error {
                    message: format!("Sherpa-onnx init failed {diagnostic}"),
                },
            );
            run_speech_processor_diarization_only(channels, shared, config);
            return;
        }
    };

    let SpeechChannels {
        processed_rx,
        is_transcribing,
    } = channels;

    let diarization_config = make_diarization_config(&config.models_dir);
    let (dummy_diar_tx, _dummy_diar_rx) = crossbeam_channel::unbounded::<DiarizedTranscript>();
    let mut diarization_worker = DiarizationWorker::new(diarization_config, dummy_diar_tx);

    let mut asr_count: u64 = 0;
    let mut diarization_count: u64 = 0;
    let extraction_count = Arc::new(AtomicU64::new(0));
    let graph_update_count = Arc::new(AtomicU64::new(0));
    let session_start = std::time::Instant::now();
    let mut utterance_start = std::time::Instant::now();
    let mut sherpa_utterance_index: u64 = 0;
    let mut active_sherpa_span: Option<(String, String, String, u64)> = None;
    let mut revision_numbers_by_span: HashMap<String, u64> = HashMap::new();

    if let Ok(mut status) = shared.pipeline_status.write() {
        status.asr = StageStatus::Running { processed_count: 0 };
    }

    let ctx = shared_to_transcript_context(
        shared,
        config.llm_provider,
        config.llm_allow_cloud_fallbacks,
        "sherpa-onnx",
    );

    log::info!("Sherpa-onnx streaming: entering processing loop");
    let mut chunks_processed: u64 = 0;

    loop {
        let chunk = match processed_rx.recv_timeout(Duration::from_millis(500)) {
            Ok(chunk) => chunk,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                if !is_transcribing.load(Ordering::Relaxed) {
                    log::info!("Sherpa-onnx streaming: is_transcribing flag cleared, exiting");
                    break;
                }
                flush_pending_if_due(&ctx, &extraction_count, &graph_update_count);
                continue;
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                log::info!("Sherpa-onnx streaming: audio channel disconnected, exiting");
                break;
            }
        };

        if !is_transcribing.load(Ordering::Relaxed) {
            log::info!("Sherpa-onnx streaming: is_transcribing flag cleared, exiting");
            break;
        }

        chunks_processed += 1;

        if let Some((text, is_endpoint)) = worker.process_chunk(&chunk.data) {
            if is_endpoint {
                asr_count += 1;
                let end_time = session_start.elapsed().as_secs_f64();
                let start_time = end_time - utterance_start.elapsed().as_secs_f64();
                utterance_start = std::time::Instant::now();
                let (span_id, source_id, provider_item_id, utterance_index) =
                    active_sherpa_span.take().unwrap_or_else(|| {
                        sherpa_utterance_index += 1;
                        let source_id = chunk.source_id.to_string();
                        let provider_item_id = format!("utterance-{}", sherpa_utterance_index);
                        let span_id = provider_sequence_span_id(
                            "sherpa-onnx",
                            &source_id,
                            "utterance",
                            sherpa_utterance_index,
                        );
                        (span_id, source_id, provider_item_id, sherpa_utterance_index)
                    });
                let (final_revision_number, supersedes) =
                    final_span_revision(&mut revision_numbers_by_span, &span_id);

                let segment = TranscriptSegment {
                    id: uuid::Uuid::new_v4().to_string(),
                    source_id,
                    speaker_id: None,
                    speaker_label: None,
                    text: text.clone(),
                    start_time,
                    end_time,
                    confidence: 0.9,
                };

                let input = DiarizationInput {
                    transcript: segment,
                    speech_audio: vec![],
                    speech_start_time: Duration::from_secs_f64(start_time),
                    speech_end_time: Duration::from_secs_f64(end_time),
                };
                let diarized = diarization_worker.process_input(input);
                diarization_count += 1;

                let _ = ctx
                    .app_handle
                    .emit(events::SPEAKER_DETECTED, &diarized.speaker_info);
                let final_segment = diarized.segment;
                let final_meta = AsrRevisionMeta {
                    span_id: Some(span_id),
                    provider_item_id: Some(provider_item_id),
                    revision_number: Some(final_revision_number),
                    supersedes,
                    turn_id: Some(format!("sherpa-onnx-utterance-{utterance_index}")),
                    raw_event_ref: Some("sherpa_onnx.endpoint".to_string()),
                    ..AsrRevisionMeta::default()
                };
                log_final_transcript_metadata(
                    "Sherpa-onnx streaming",
                    "sherpa-onnx",
                    asr_count,
                    &final_segment,
                    &final_meta,
                );

                // SPEAKER_DETECTED was already emitted above — pass `None`
                // so the shared helper doesn't double-emit.
                emit_transcript_and_extract_with_meta(
                    final_segment,
                    None,
                    &ctx,
                    asr_count,
                    diarization_count,
                    &extraction_count,
                    &graph_update_count,
                    final_meta,
                );
            } else {
                let end_time = session_start.elapsed().as_secs_f64();
                let start_time = end_time - utterance_start.elapsed().as_secs_f64();
                let (span_id, source_id, provider_item_id, utterance_index) =
                    active_sherpa_span.clone().unwrap_or_else(|| {
                        sherpa_utterance_index += 1;
                        let source_id = chunk.source_id.to_string();
                        let provider_item_id = format!("utterance-{}", sherpa_utterance_index);
                        let span_id = provider_sequence_span_id(
                            "sherpa-onnx",
                            &source_id,
                            "utterance",
                            sherpa_utterance_index,
                        );
                        let state = (span_id, source_id, provider_item_id, sherpa_utterance_index);
                        active_sherpa_span = Some(state.clone());
                        state
                    });
                let (revision_number, supersedes) =
                    next_span_revision(&mut revision_numbers_by_span, &span_id);
                emit_asr_partial_with_meta(
                    &ctx,
                    "sherpa-onnx",
                    source_id,
                    text,
                    start_time,
                    end_time,
                    0.9,
                    AsrRevisionMeta {
                        span_id: Some(span_id),
                        provider_item_id: Some(provider_item_id),
                        revision_number: Some(revision_number),
                        supersedes,
                        turn_id: Some(format!("sherpa-onnx-utterance-{utterance_index}")),
                        raw_event_ref: Some("sherpa_onnx.partial".to_string()),
                        ..AsrRevisionMeta::default()
                    },
                );
            }
        }

        if chunks_processed.is_multiple_of(500) {
            log::debug!(
                "Sherpa-onnx streaming: processed {} chunks, {} transcripts",
                chunks_processed,
                asr_count
            );
        }
    }

    flush_pending_now(&ctx, &extraction_count, &graph_update_count);

    log::info!(
        "Sherpa-onnx streaming: exiting. Chunks={}, ASR={}, diarized={}",
        chunks_processed,
        asr_count,
        diarization_count,
    );
}

impl AccumulatedSegment {
    /// Convert an `AccumulatedSegment` into the `SpeechSegment` type expected
    /// by the ASR worker.
    fn to_asr_segment(seg: &AccumulatedSegment) -> crate::asr::SpeechSegment {
        crate::asr::SpeechSegment {
            source_id: seg.source_id.clone(),
            audio: seg.audio.clone(),
            start_time: seg.start_time,
            end_time: seg.end_time,
            num_frames: seg.num_frames,
        }
    }
}

// ponytail: ONE gtk app per process. tao acquires the gtk main context on first
// `tauri::Builder::build()` and never releases it, so a 2nd build on another
// test thread panics ("main context already acquired by another thread"), which
// poisoned process-global test locks and cascaded ~40 failures. We build the app
// exactly once, leak it for 'static, and share its AppHandle — every test that
// only needs to .emit()/.listen_any() reuses the handle. (seed audio-graph-65f0)
#[cfg(test)]
pub(crate) fn shared_test_app_handle() -> tauri::AppHandle {
    static SHARED: std::sync::OnceLock<tauri::AppHandle> = std::sync::OnceLock::new();
    SHARED
        .get_or_init(|| {
            #[cfg(not(target_os = "macos"))]
            let builder = tauri::Builder::default().any_thread();
            #[cfg(target_os = "macos")]
            let builder = tauri::Builder::default();
            let app = builder
                .build(tauri::test::mock_context(tauri::test::noop_assets()))
                .expect("shared test app should build");
            let handle = app.handle().clone();
            // Keep the App alive for the whole process so the handle stays valid.
            Box::leak(Box::new(app));
            handle
        })
        .clone()
}

// Integration tests (Task #81 — loop 10 HIGH #3): narrow-scope tests proving
// the diarization → extraction → graph plumbing works end-to-end without
// requiring a mocked `tauri::AppHandle`.
#[cfg(test)]
mod tests_integration;

// Unit tests for AudioAccumulator (loop-15 A3 — closes loop-12 HIGH #2's
// open test gap on the segment-batching helper).
#[cfg(test)]
mod tests_audio_accumulator;

// Unit tests for the FA-1 pipeline-status helper: a poisoned `pipeline_status`
// lock must still record the ASR error status (poison recovery), not silently
// swallow it. The emit half is Tauri-bound and exercised at the integration
// layer; the pure write half is tested here.
#[cfg(test)]
mod tests_status {
    use super::{
        DiarizationDispatchContext, DiarizationEventSink, PipelineStatus,
        ProjectionDispatchContext, ProjectionPatchGenerator, ProjectionPatchOutcome,
        ProjectionRuntimeEventSink, SpeechChannels, SpeechConfig, SpeechShared, StageStatus,
        aws_error_diagnostic, aws_error_for_diagnostic_event, cloud_error_code,
        diarization_span_revision_for_transcript, emit_assemblyai_speaker_revision_with_dispatch,
        final_only_revision_meta, final_span_revision, moonshine_final_transcript_segment,
        moonshine_revision_meta, next_span_revision, provider_item_span_id,
        provider_sequence_span_id, provider_start_span_id, record_asr_span_revision_event,
        record_asr_span_revision_event_and_observe_projection, revision_ref,
        run_moonshine_speech_processor_with_worker, run_projection_job, set_asr_status,
        speech_error_diagnostic,
    };
    use crate::asr::moonshine::{
        MoonshineAdapterError, MoonshineRuntimeConfig, MoonshineSpanMapper,
        MoonshineStreamingAdapter, MoonshineStreamingWorker, MoonshineTranscriptLine,
    };
    use crate::audio::pipeline::{PROCESSED_AUDIO_SAMPLE_RATE_HZ, ProcessedAudioChunk};
    use crate::events::{self, AsrSpanRevisionPayload, AsrSpanStability, DiarizationSpanStability};
    use crate::graph::entities::{GraphDelta, GraphSnapshot};
    use crate::persistence::{
        FileMemoryRepository, LocalMemoryRepository, TranscriptEventWriter,
        load_materialized_graph, load_materialized_notes, load_projection_events,
        load_transcript_events,
    };
    use crate::projection_scheduler::{ProjectionSchedulerDecision, ProjectionSchedulers};
    use crate::projections::{
        ProjectionJob, ProjectionKind, ProjectionOperation, ProjectionPatch, ProjectionProvenance,
        TranscriptLedger,
    };
    use crate::settings::LlmProvider;
    use crate::state::{AppState, TranscriptSegment};
    use std::collections::{HashMap, VecDeque};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, RwLock};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use tauri::Listener;

    fn unique_tempdir(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "audio-graph-speech-{}-{}-{}-{}",
            label,
            std::process::id(),
            nanos,
            n
        ));
        std::fs::create_dir_all(&dir).expect("create tempdir");
        dir
    }

    fn poison_transcript_event_writer_lock(writer: Arc<Mutex<Option<TranscriptEventWriter>>>) {
        let result = std::thread::spawn(move || {
            let _guard = writer.lock().unwrap();
            panic!("intentional transcript event writer lock poison");
        })
        .join();
        assert!(
            result.is_err(),
            "poison helper should panic while holding writer lock"
        );
    }

    struct DataDirGuard {
        prev_data_dir: Option<std::ffi::OsString>,
        prev_home: Option<std::ffi::OsString>,
        prev_userprofile: Option<std::ffi::OsString>,
    }

    impl DataDirGuard {
        #[allow(unsafe_code)]
        fn set(dir: &Path) -> Self {
            let prev_data_dir = std::env::var_os(crate::user_data::DATA_DIR_ENV);
            let prev_home = std::env::var_os("HOME");
            let prev_userprofile = std::env::var_os("USERPROFILE");
            unsafe {
                std::env::set_var(crate::user_data::DATA_DIR_ENV, dir);
                std::env::set_var("HOME", dir);
                std::env::set_var("USERPROFILE", dir);
            }
            Self {
                prev_data_dir,
                prev_home,
                prev_userprofile,
            }
        }
    }

    impl Drop for DataDirGuard {
        #[allow(unsafe_code)]
        fn drop(&mut self) {
            unsafe {
                match &self.prev_data_dir {
                    Some(value) => std::env::set_var(crate::user_data::DATA_DIR_ENV, value),
                    None => std::env::remove_var(crate::user_data::DATA_DIR_ENV),
                }
                match &self.prev_home {
                    Some(value) => std::env::set_var("HOME", value),
                    None => std::env::remove_var("HOME"),
                }
                match &self.prev_userprofile {
                    Some(value) => std::env::set_var("USERPROFILE", value),
                    None => std::env::remove_var("USERPROFILE"),
                }
            }
        }
    }

    #[derive(Clone)]
    struct FnProjectionPatchGenerator {
        calls: Arc<AtomicUsize>,
        #[allow(clippy::type_complexity)]
        generate: Arc<
            dyn Fn(
                    ProjectionJob,
                    TranscriptLedger,
                    u64,
                    u64,
                ) -> Result<ProjectionPatchOutcome, String>
                + Send
                + Sync,
        >,
    }

    impl FnProjectionPatchGenerator {
        fn new(
            generate: impl Fn(
                ProjectionJob,
                TranscriptLedger,
                u64,
                u64,
            ) -> Result<ProjectionPatchOutcome, String>
            + Send
            + Sync
            + 'static,
        ) -> (Self, Arc<AtomicUsize>) {
            let calls = Arc::new(AtomicUsize::new(0));
            (
                Self {
                    calls: calls.clone(),
                    generate: Arc::new(generate),
                },
                calls,
            )
        }
    }

    impl ProjectionPatchGenerator for FnProjectionPatchGenerator {
        fn generate_projection_patch(
            &self,
            job: ProjectionJob,
            ledger: TranscriptLedger,
            sequence: u64,
            created_at_ms: u64,
        ) -> Result<ProjectionPatchOutcome, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            (self.generate)(job, ledger, sequence, created_at_ms)
        }
    }

    #[derive(Clone, Default)]
    struct RecordingProjectionRuntimeEventSink {
        patches: Arc<Mutex<Vec<ProjectionPatch>>>,
        notes: Arc<Mutex<Vec<crate::projections::MaterializedNotes>>>,
        graphs: Arc<Mutex<Vec<crate::projections::MaterializedGraph>>>,
    }

    impl RecordingProjectionRuntimeEventSink {
        fn patch_count(&self) -> usize {
            self.patches.lock().unwrap_or_else(|p| p.into_inner()).len()
        }

        fn notes_count(&self) -> usize {
            self.notes.lock().unwrap_or_else(|p| p.into_inner()).len()
        }

        fn graph_count(&self) -> usize {
            self.graphs.lock().unwrap_or_else(|p| p.into_inner()).len()
        }
    }

    impl ProjectionRuntimeEventSink for RecordingProjectionRuntimeEventSink {
        fn emit_projection_patch(&self, patch: &ProjectionPatch) {
            self.patches
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(patch.clone());
        }

        fn emit_materialized_notes(&self, notes: &crate::projections::MaterializedNotes) {
            self.notes
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(notes.clone());
        }

        fn emit_materialized_graph(&self, graph: &crate::projections::MaterializedGraph) {
            self.graphs
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(graph.clone());
        }
    }

    #[derive(Clone, Default)]
    struct RecordingDiarizationEventSink {
        revisions: Arc<AtomicUsize>,
        graph_deltas: Arc<AtomicUsize>,
        graph_updates: Arc<AtomicUsize>,
    }

    impl RecordingDiarizationEventSink {
        fn revision_count(&self) -> usize {
            self.revisions.load(Ordering::SeqCst)
        }

        fn graph_delta_count(&self) -> usize {
            self.graph_deltas.load(Ordering::SeqCst)
        }

        fn graph_update_count(&self) -> usize {
            self.graph_updates.load(Ordering::SeqCst)
        }
    }

    impl DiarizationEventSink for RecordingDiarizationEventSink {
        fn emit_diarization_span_revision(
            &self,
            _payload: &events::DiarizationSpanRevisionPayload,
        ) {
            self.revisions.fetch_add(1, Ordering::SeqCst);
        }

        fn emit_graph_delta(&self, _delta: &GraphDelta) {
            self.graph_deltas.fetch_add(1, Ordering::SeqCst);
        }

        fn emit_graph_update(&self, _snapshot: &GraphSnapshot) {
            self.graph_updates.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn projection_dispatch_for_app(
        app: &AppState,
        generator: FnProjectionPatchGenerator,
    ) -> (
        ProjectionDispatchContext,
        RecordingProjectionRuntimeEventSink,
    ) {
        let event_sink = RecordingProjectionRuntimeEventSink::default();
        (
            ProjectionDispatchContext {
                transcript_ledger: app.transcript_ledger.clone(),
                projection_schedulers: app.projection_schedulers.clone(),
                projection_runtime: app.projection_runtime_handle(),
                event_sink: Arc::new(event_sink.clone()),
                patch_generator: Arc::new(generator),
            },
            event_sink,
        )
    }

    fn test_projection_patch(
        job: &ProjectionJob,
        sequence: u64,
        created_at_ms: u64,
    ) -> ProjectionPatch {
        let operations = match job.kind {
            ProjectionKind::Notes => vec![ProjectionOperation::UpsertNote {
                id: format!("note-{}", job.basis.transcript_hash),
                title: "Projection note".to_string(),
                body: format!(
                    "Projected {} transcript span(s).",
                    job.basis.span_revisions.len()
                ),
                tags: vec!["test".to_string()],
            }],
            ProjectionKind::Graph => vec![ProjectionOperation::UpsertGraphNode {
                id: format!("node-{}", job.basis.transcript_hash),
                name: "Projection Node".to_string(),
                entity_type: "concept".to_string(),
                description: Some(format!(
                    "Projected {} transcript span(s).",
                    job.basis.span_revisions.len()
                )),
            }],
        };
        ProjectionPatch {
            sequence,
            kind: job.kind.clone(),
            llm_request_id: format!("fake:{}:{}", job.id, sequence),
            basis: job.basis.clone(),
            operations,
            confidence: 1.0,
            provenance: ProjectionProvenance {
                provider: "fake".to_string(),
                model: "projection-dispatch-test".to_string(),
                prompt_id: "projection_patch_v1_test".to_string(),
            },
            queued_at_ms: None,
            generation_latency_ms: None,
            apply_latency_ms: None,
            created_at_ms,
        }
    }

    fn drain_app_writers(app: &AppState) {
        let timeout = std::time::Duration::from_secs(3);
        if let Some(writer) = app
            .transcript_writer
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            assert!(writer.shutdown_with_timeout(timeout));
        }
        if let Some(writer) = app
            .transcript_event_writer
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            assert!(writer.shutdown_with_timeout(timeout));
        }
        if let Some(writer) = app
            .projection_event_writer
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            assert!(writer.shutdown_with_timeout(timeout));
        }
    }

    fn wait_until(label: &str, mut done: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            if done() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("timed out waiting for {label}");
    }

    fn moonshine_test_app() -> (tauri::AppHandle, tauri::AppHandle) {
        // Returns (kept-alive handle, handle) to preserve the prior 2-tuple shape
        // at call sites; both are the shared process-wide handle now.
        let handle = super::shared_test_app_handle();
        (handle.clone(), handle)
    }

    fn moonshine_shared_for_app(app: &AppState, app_handle: tauri::AppHandle) -> SpeechShared {
        SpeechShared {
            transcript_buffer: app.transcript_buffer.clone(),
            transcript_writer: app.transcript_writer.clone(),
            transcript_event_writer: app.transcript_event_writer.clone(),
            transcript_ledger: app.transcript_ledger.clone(),
            speaker_timeline: app.speaker_timeline.clone(),
            projection_schedulers: app.projection_schedulers.clone(),
            projection_runtime: app.projection_runtime_handle(),
            pipeline_status: app.pipeline_status.clone(),
            app_handle,
            knowledge_graph: app.knowledge_graph.clone(),
            graph_snapshot: app.graph_snapshot.clone(),
            graph_extractor: app.graph_extractor.clone(),
            llm_engine: app.llm_engine.clone(),
            api_client: app.api_client.clone(),
            mistralrs_engine: app.mistralrs_engine.clone(),
            llm_executor: app.llm_executor.clone(),
            pending_agent_proposals: app.pending_agent_proposals.clone(),
        }
    }

    fn moonshine_speech_config(models_dir: PathBuf) -> SpeechConfig {
        SpeechConfig {
            models_dir,
            llm_provider: LlmProvider::default(),
            llm_allow_cloud_fallbacks: true,
            provider_content_egress_policy: crate::asr::ProviderContentEgressPolicy::allow(),
        }
    }

    fn processed_audio_chunk(source_id: &str, sample: f32) -> ProcessedAudioChunk {
        ProcessedAudioChunk {
            source_id: source_id.into(),
            data: vec![sample; 512],
            sample_rate: PROCESSED_AUDIO_SAMPLE_RATE_HZ,
            num_frames: 512,
            timestamp: Some(Duration::from_millis(0)),
        }
    }

    fn moonshine_worker(
        adapter: FakeMoonshineSpeechAdapter,
    ) -> MoonshineStreamingWorker<FakeMoonshineSpeechAdapter> {
        let mut config = MoonshineRuntimeConfig::new(PathBuf::from("moonshine-test-model"));
        config.poll_interval = Duration::from_millis(0);
        MoonshineStreamingWorker::new_with_config(adapter, config).expect("moonshine worker")
    }

    fn run_moonshine_helper_once(
        app: &AppState,
        app_handle: tauri::AppHandle,
        adapter: FakeMoonshineSpeechAdapter,
        chunks: Vec<ProcessedAudioChunk>,
        models_dir: PathBuf,
    ) {
        let (processed_tx, processed_rx) = crossbeam_channel::unbounded();
        for chunk in chunks {
            processed_tx.send(chunk).expect("send processed audio");
        }
        drop(processed_tx);
        run_moonshine_speech_processor_with_worker(
            SpeechChannels {
                processed_rx,
                is_transcribing: Arc::new(AtomicBool::new(true)),
            },
            moonshine_shared_for_app(app, app_handle),
            moonshine_speech_config(models_dir),
            moonshine_worker(adapter),
        );
    }

    #[derive(Default)]
    struct FakeMoonshineSpeechAdapter {
        polls: VecDeque<Result<Vec<MoonshineTranscriptLine>, MoonshineAdapterError>>,
        accepted_sample_rates: Arc<Mutex<Vec<u32>>>,
        started: bool,
        stopped: bool,
    }

    impl FakeMoonshineSpeechAdapter {
        fn new() -> (Self, Arc<Mutex<Vec<u32>>>) {
            let accepted_sample_rates = Arc::new(Mutex::new(Vec::new()));
            (
                Self {
                    accepted_sample_rates: accepted_sample_rates.clone(),
                    ..Self::default()
                },
                accepted_sample_rates,
            )
        }

        fn push_batch(&mut self, batch: Vec<MoonshineTranscriptLine>) {
            self.polls.push_back(Ok(batch));
        }

        fn push_error(&mut self, message: &str) {
            self.polls
                .push_back(Err(MoonshineAdapterError::new(message)));
        }
    }

    impl MoonshineStreamingAdapter for FakeMoonshineSpeechAdapter {
        fn start(&mut self) -> Result<(), MoonshineAdapterError> {
            self.started = true;
            Ok(())
        }

        fn accept_pcm(
            &mut self,
            sample_rate_hz: u32,
            samples: &[f32],
        ) -> Result<(), MoonshineAdapterError> {
            if !self.started {
                return Err(MoonshineAdapterError::new("adapter not started"));
            }
            assert!(!samples.is_empty(), "speech helper should forward PCM");
            self.accepted_sample_rates
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(sample_rate_hz);
            Ok(())
        }

        fn poll_updates(&mut self) -> Result<Vec<MoonshineTranscriptLine>, MoonshineAdapterError> {
            self.polls.pop_front().unwrap_or_else(|| Ok(Vec::new()))
        }

        fn stop(&mut self) -> Result<(), MoonshineAdapterError> {
            self.stopped = true;
            Ok(())
        }
    }

    fn projection_asr_payload(
        span_id: &str,
        revision_number: u64,
        text: &str,
        final_revision: bool,
    ) -> AsrSpanRevisionPayload {
        AsrSpanRevisionPayload {
            span_id: span_id.to_string(),
            provider: "projection-test".to_string(),
            source_id: "system".to_string(),
            provider_item_id: Some(span_id.to_string()),
            transcript_segment_id: final_revision.then(|| format!("segment-{span_id}")),
            speaker_id: Some("speaker-1".to_string()),
            speaker_label: Some("Speaker 1".to_string()),
            channel: None,
            text: text.to_string(),
            start_time: revision_number as f64,
            end_time: revision_number as f64 + 1.0,
            confidence: 0.95,
            is_final: final_revision,
            stability: if final_revision {
                AsrSpanStability::Final
            } else {
                AsrSpanStability::Partial
            },
            revision_number,
            supersedes: (revision_number > 1)
                .then(|| format!("{span_id}@rev{}", revision_number - 1)),
            turn_id: None,
            end_of_turn: final_revision,
            raw_event_ref: Some(format!("projection-test[{revision_number}]")),
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms: 1_700_000_000_000 + revision_number,
        }
    }

    fn assemblyai_speaker_revision(
        speaker_id: &str,
        speaker_label: &str,
    ) -> crate::asr::assemblyai::AssemblyAiV3SpeakerRevision {
        crate::asr::assemblyai::AssemblyAiV3SpeakerRevision {
            turn_order: 7,
            span_id: "assemblyai:source-retcon:turn-7".to_string(),
            provider_item_id: "turn-7".to_string(),
            speaker_id: Some(speaker_id.to_string()),
            speaker_label: Some(speaker_label.to_string()),
            words: vec![crate::asr::assemblyai::AssemblyAiV3SpeakerRevisionWord {
                text: "hello".to_string(),
                speaker_id: Some(speaker_id.to_string()),
                start_time: Some(1.0),
                end_time: Some(1.4),
            }],
        }
    }

    #[test]
    fn assemblyai_speaker_revision_emission_retcons_graph_on_label_remap() {
        use crate::graph::entities::{ExtractedEntity, ExtractedRelation, ExtractionResult};

        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("assemblyai-diarization-retcon");
        let _guard = DataDirGuard::set(&dir);

        let app = AppState::new();
        let event_sink = RecordingDiarizationEventSink::default();
        let diarization_dispatch = DiarizationDispatchContext {
            event_sink: &event_sink,
            speaker_timeline: &app.speaker_timeline,
            knowledge_graph: &app.knowledge_graph,
            graph_snapshot: &app.graph_snapshot,
        };

        {
            let mut graph = app
                .knowledge_graph
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            graph.process_extraction(
                &ExtractionResult {
                    entities: vec![
                        ExtractedEntity {
                            name: "Speaker 2".to_string(),
                            entity_type: "Person".to_string(),
                            description: None,
                        },
                        ExtractedEntity {
                            name: "Alice".to_string(),
                            entity_type: "Person".to_string(),
                            description: None,
                        },
                        ExtractedEntity {
                            name: "Bob".to_string(),
                            entity_type: "Person".to_string(),
                            description: None,
                        },
                    ],
                    relations: vec![ExtractedRelation {
                        source: "Speaker 2".to_string(),
                        target: "Bob".to_string(),
                        relation_type: "knows".to_string(),
                        detail: None,
                    }],
                },
                1.0,
                "Speaker 2",
                "seg-1",
            );
            let _ = graph.take_delta();
        }

        let mut revision_numbers_by_span = HashMap::new();

        let first_outcome = emit_assemblyai_speaker_revision_with_dispatch(
            &assemblyai_speaker_revision("speaker-2", "Speaker 2"),
            &diarization_dispatch,
            &mut revision_numbers_by_span,
            1_700_000_000_001,
        );
        assert!(first_outcome.accepted);
        assert!(!first_outcome.retcon_fired);
        assert_eq!(first_outcome.edges_retconned, 0);
        assert_eq!(event_sink.revision_count(), 1);
        assert_eq!(
            event_sink.graph_delta_count(),
            0,
            "first-seen provisional label should not retcon"
        );

        let second_outcome = emit_assemblyai_speaker_revision_with_dispatch(
            &assemblyai_speaker_revision("speaker-alice", "Alice"),
            &diarization_dispatch,
            &mut revision_numbers_by_span,
            1_700_000_000_002,
        );
        assert!(second_outcome.accepted);
        assert!(second_outcome.retcon_fired);
        assert_eq!(second_outcome.edges_retconned, 1);
        assert_eq!(event_sink.revision_count(), 2);
        assert_eq!(event_sink.graph_delta_count(), 1);
        assert_eq!(event_sink.graph_update_count(), 1);

        {
            let timeline = app
                .speaker_timeline
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            assert_eq!(timeline.accepted_event_count, 2);
            assert_eq!(timeline.latest_spans.len(), 1);
            assert_eq!(
                timeline.latest_spans[0].speaker_label.as_deref(),
                Some("Alice")
            );
            assert_eq!(timeline.latest_spans[0].revision_number, 2);
        }

        let snapshot = app
            .graph_snapshot
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let alice_id = snapshot
            .nodes
            .iter()
            .find(|node| node.name == "Alice")
            .expect("canonical speaker node")
            .id
            .clone();
        let live_knows: Vec<_> = snapshot
            .links
            .iter()
            .filter(|link| link.relation_type == "knows")
            .collect();
        assert_eq!(live_knows.len(), 1);
        assert_eq!(
            live_knows[0].source, alice_id,
            "speaker-label remap should re-point the live edge to Alice"
        );

        drain_app_writers(&app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn provider_item_revision_helpers_are_stable() {
        let span_id = provider_item_span_id("openai_realtime", "system-default", "item_003");
        assert_eq!(span_id, "openai_realtime:system-default:item_003");
        assert_eq!(
            revision_ref(&span_id, 2),
            "openai_realtime:system-default:item_003@rev2"
        );
    }

    #[test]
    fn provider_start_revision_helpers_chain_partial_to_final() {
        let span_id = provider_start_span_id("deepgram", "system-default", 1.2344);
        assert_eq!(span_id, "deepgram:system-default:start-1234");

        let mut revisions = HashMap::new();
        assert_eq!(
            next_span_revision(&mut revisions, &span_id),
            (1, None),
            "first partial starts the span revision chain"
        );
        assert_eq!(
            next_span_revision(&mut revisions, &span_id),
            (
                2,
                Some("deepgram:system-default:start-1234@rev1".to_string())
            ),
            "second partial supersedes the first"
        );
        assert_eq!(
            final_span_revision(&mut revisions, &span_id),
            (
                3,
                Some("deepgram:system-default:start-1234@rev2".to_string())
            ),
            "final transcript supersedes the latest partial"
        );
        assert!(
            revisions.is_empty(),
            "finalized spans should not retain revision state"
        );
    }

    #[test]
    fn provider_sequence_revision_helpers_chain_partial_to_final() {
        let span_id = provider_sequence_span_id("assemblyai", "system-default", "turn", 7);
        assert_eq!(span_id, "assemblyai:system-default:turn-7");
        assert_eq!(
            provider_sequence_span_id("sherpa-onnx", "mic-1", "utterance", 3),
            "sherpa-onnx:mic-1:utterance-3"
        );

        let mut revisions = HashMap::new();
        assert_eq!(next_span_revision(&mut revisions, &span_id), (1, None));
        assert_eq!(
            final_span_revision(&mut revisions, &span_id),
            (2, Some("assemblyai:system-default:turn-7@rev1".to_string()))
        );
    }

    #[test]
    fn final_only_revision_meta_is_stable_for_non_streaming_asr_paths() {
        for provider in ["local_whisper", "cloud_api", "local_diarization"] {
            let meta = final_only_revision_meta(provider, "system-default", 1.2344, 2.9996);
            let expected_span = format!("{provider}:system-default:final-1234-3000");
            assert_eq!(meta.span_id.as_deref(), Some(expected_span.as_str()));
            assert_eq!(meta.provider_item_id.as_deref(), Some("final-1234-3000"));
            assert_eq!(meta.revision_number, Some(1));
            assert_eq!(meta.supersedes, None);
        }
    }

    #[test]
    fn speech_error_diagnostic_omits_raw_message_text() {
        let raw = "provider returned verbatim content body";
        let diagnostic = speech_error_diagnostic("cloud_api", "transcription_failed", "401", raw);

        assert_eq!(
            diagnostic,
            format!(
                "provider=cloud_api error_category=transcription_failed error_code=401 message_len={}",
                raw.chars().count()
            )
        );
        assert!(!diagnostic.contains("verbatim content body"));
        assert_eq!(
            cloud_error_code(
                "Cloud ASR API error: provider=cloud_asr status=429 Too Many Requests body_bytes=9 body_chars=9"
            ),
            "429"
        );
        assert_eq!(cloud_error_code("network failure"), "cloud_asr_error");
    }

    #[test]
    fn aws_error_diagnostic_event_omits_unknown_raw_message_text() {
        let raw = "unexpected provider status contained content body";
        let classified = crate::aws_util::UiAwsError::Unknown {
            message: raw.to_string(),
        };
        let diagnostic = aws_error_diagnostic(&classified, raw);
        let event_error = aws_error_for_diagnostic_event(classified, &diagnostic);

        assert_eq!(
            diagnostic,
            format!(
                "provider=aws-transcribe error_category=unknown error_code=unknown message_len={}",
                raw.chars().count()
            )
        );
        assert!(!diagnostic.contains("content body"));
        match event_error {
            crate::aws_util::UiAwsError::Unknown { message } => {
                assert_eq!(message, diagnostic);
                assert!(!message.contains("content body"));
            }
            other => panic!("expected redacted Unknown error, got {other:?}"),
        }
    }

    #[test]
    fn moonshine_final_bridge_keeps_speaker_hints_out_of_legacy_transcript() {
        let mut mapper = MoonshineSpanMapper::default();
        let mut line = MoonshineTranscriptLine::final_line("line-9", "hello from moonshine");
        line.start_time = 2.0;
        line.end_time = 3.5;
        line.confidence = Some(0.82);
        line.speaker_id = Some("moonshine-speaker-1".to_string());
        line.speaker_label = Some("Moonshine speaker 1".to_string());
        line.channel = Some("mixed".to_string());

        let revision = mapper
            .map_line_update_at("mic", &line, 1_700_000_000_100)
            .expect("mapping")
            .expect("revision");

        let segment =
            moonshine_final_transcript_segment(&revision).expect("final transcript segment");
        assert_eq!(segment.id, "moonshine:mic:line-9@final");
        assert_eq!(segment.source_id, "mic");
        assert_eq!(segment.text, "hello from moonshine");
        assert_eq!(segment.speaker_id, None);
        assert_eq!(segment.speaker_label, None);

        let meta = moonshine_revision_meta(&revision);
        assert_eq!(meta.span_id.as_deref(), Some("moonshine:mic:line-9"));
        assert_eq!(meta.provider_item_id.as_deref(), Some("line-9"));
        assert_eq!(meta.speaker_id.as_deref(), Some("moonshine-speaker-1"));
        assert_eq!(meta.speaker_label.as_deref(), Some("Moonshine speaker 1"));
        assert_eq!(meta.channel.as_deref(), Some("mixed"));
        assert_eq!(meta.revision_number, Some(1));
        assert_eq!(meta.raw_event_ref.as_deref(), Some("moonshine.line.final"));
        assert_eq!(meta.received_at_ms, Some(1_700_000_000_100));
    }

    #[test]
    fn moonshine_partial_bridge_does_not_create_legacy_transcript_segment() {
        let mut mapper = MoonshineSpanMapper::default();
        let partial = mapper
            .map_line_update_at(
                "loopback",
                &MoonshineTranscriptLine::partial("line-partial", "still forming"),
                1_700_000_000_200,
            )
            .expect("mapping")
            .expect("revision");

        assert!(
            moonshine_final_transcript_segment(&partial).is_none(),
            "partials must stay in the transcript ledger and ASR events only"
        );
        let meta = moonshine_revision_meta(&partial);
        assert_eq!(
            meta.span_id.as_deref(),
            Some("moonshine:loopback:line-partial")
        );
        assert_eq!(meta.revision_number, Some(1));
        assert_eq!(
            meta.raw_event_ref.as_deref(),
            Some("moonshine.line.partial")
        );
    }

    #[test]
    #[cfg_attr(
        target_os = "macos",
        ignore = "Tauri/Tao AppHandle construction must run on the macOS main thread"
    )]
    fn moonshine_speech_helper_wires_fake_adapter_runtime() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let data_dir = unique_tempdir("moonshine-helper-runtime");
        let _guard = DataDirGuard::set(&data_dir);
        let (_tauri_app, app_handle) = moonshine_test_app();

        {
            let app = AppState::new();
            let mut adapter = FakeMoonshineSpeechAdapter::default();
            adapter.push_batch(vec![MoonshineTranscriptLine::partial(
                "line-partial",
                "forming",
            )]);

            run_moonshine_helper_once(
                &app,
                app_handle.clone(),
                adapter,
                vec![processed_audio_chunk("mic-1", 0.2)],
                data_dir.join("models-partial"),
            );

            assert!(
                app.transcript_buffer.read().unwrap().is_empty(),
                "partial Moonshine revisions must not create legacy transcript rows"
            );
            let ledger = app.transcript_ledger.lock().unwrap();
            assert_eq!(ledger.accepted_event_count, 1);
            assert_eq!(ledger.latest_spans.len(), 1);
            assert_eq!(ledger.latest_spans[0].provider, "moonshine");
            assert_eq!(ledger.latest_spans[0].text, "forming");
            assert!(!ledger.latest_spans[0].is_final);
            drop(ledger);
            drain_app_writers(&app);
        }

        {
            let app = AppState::new();
            let session_id = app.current_session_id();
            let (mut adapter, accepted_sample_rates) = FakeMoonshineSpeechAdapter::new();
            adapter.push_batch(vec![
                MoonshineTranscriptLine::partial("line-final", "almost"),
                MoonshineTranscriptLine::final_line("line-final", "final text"),
            ]);

            run_moonshine_helper_once(
                &app,
                app_handle.clone(),
                adapter,
                vec![processed_audio_chunk("mic-1", 0.3)],
                data_dir.join("models-final"),
            );

            assert_eq!(
                *accepted_sample_rates.lock().unwrap(),
                vec![PROCESSED_AUDIO_SAMPLE_RATE_HZ],
                "Moonshine helper must feed the worker processed 16 kHz PCM"
            );
            {
                let buffer = app.transcript_buffer.read().unwrap();
                assert_eq!(buffer.len(), 1);
                let segment = buffer.front().expect("final transcript segment");
                assert_eq!(segment.source_id, "mic-1");
                assert_eq!(segment.text, "final text");
                assert_eq!(segment.id, "moonshine:mic-1:line-final@final");
            }
            {
                let status = app.pipeline_status.read().unwrap();
                assert!(matches!(
                    status.asr,
                    StageStatus::Running { processed_count: 1 }
                ));
            }

            drain_app_writers(&app);
            let transcript_path =
                crate::user_data::transcript_path(&session_id).expect("transcript path");
            let rows = std::fs::read_to_string(&transcript_path).expect("transcript file");
            let rows: Vec<&str> = rows
                .lines()
                .filter(|line| !line.trim().is_empty())
                .collect();
            assert_eq!(rows.len(), 1);
            let persisted: TranscriptSegment =
                serde_json::from_str(rows[0]).expect("persisted transcript row");
            assert_eq!(persisted.text, "final text");
        }

        {
            let app = AppState::new();
            let (mut adapter, accepted_sample_rates) = FakeMoonshineSpeechAdapter::new();
            adapter.push_batch(Vec::new());
            adapter.push_batch(vec![MoonshineTranscriptLine::partial(
                "line-pending",
                "pending update",
            )]);
            let (processed_tx, processed_rx) = crossbeam_channel::unbounded();
            processed_tx
                .send(processed_audio_chunk("loopback", 0.1))
                .expect("send initial processed audio");
            let is_transcribing = Arc::new(AtomicBool::new(true));
            let helper_is_transcribing = is_transcribing.clone();
            let shared = moonshine_shared_for_app(&app, app_handle.clone());
            let config = moonshine_speech_config(data_dir.join("models-pending"));
            let worker = moonshine_worker(adapter);

            let helper = std::thread::spawn(move || {
                run_moonshine_speech_processor_with_worker(
                    SpeechChannels {
                        processed_rx,
                        is_transcribing: helper_is_transcribing,
                    },
                    shared,
                    config,
                    worker,
                );
            });

            wait_until("Moonshine pending poll revision", || {
                app.transcript_ledger.lock().unwrap().accepted_event_count == 1
            });
            is_transcribing.store(false, Ordering::Relaxed);
            drop(processed_tx);
            helper.join().expect("moonshine helper thread");

            assert_eq!(
                *accepted_sample_rates.lock().unwrap(),
                vec![PROCESSED_AUDIO_SAMPLE_RATE_HZ]
            );
            assert!(
                app.transcript_buffer.read().unwrap().is_empty(),
                "pending partial should not create a transcript segment"
            );
            let ledger = app.transcript_ledger.lock().unwrap();
            assert_eq!(ledger.latest_spans[0].source_id, "loopback");
            assert_eq!(ledger.latest_spans[0].text, "pending update");
            drop(ledger);
            drain_app_writers(&app);
        }

        {
            let app = AppState::new();
            let (latency_tx, latency_rx) = std::sync::mpsc::channel();
            let listener_id = app_handle.listen_any(events::PIPELINE_LATENCY, move |event| {
                if let Ok(payload) =
                    serde_json::from_str::<events::PipelineLatencyPayload>(event.payload())
                {
                    let _ = latency_tx.send(payload);
                }
            });
            let mut adapter = FakeMoonshineSpeechAdapter::default();
            let mut final_line = MoonshineTranscriptLine::final_line("line-latency", "timed final");
            final_line.latency_ms = Some(37);
            adapter.push_batch(vec![final_line]);

            run_moonshine_helper_once(
                &app,
                app_handle.clone(),
                adapter,
                vec![processed_audio_chunk("mic-latency", 0.4)],
                data_dir.join("models-latency"),
            );

            let payload = latency_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("Moonshine latency event");
            assert_eq!(payload.stage, "asr.moonshine");
            assert_eq!(payload.source_id.as_deref(), Some("mic-latency"));
            assert_eq!(
                payload.segment_id.as_deref(),
                Some("moonshine:mic-latency:line-latency")
            );
            assert_eq!(payload.latency_ms, 37.0);
            app_handle.unlisten(listener_id);
            drain_app_writers(&app);
        }

        {
            let app = AppState::new();
            let mut adapter = FakeMoonshineSpeechAdapter::default();
            adapter.push_error("simulated adapter failure");

            run_moonshine_helper_once(
                &app,
                app_handle,
                adapter,
                vec![processed_audio_chunk("mic-error", 0.5)],
                data_dir.join("models-error"),
            );

            let status = app.pipeline_status.read().unwrap();
            match &status.asr {
                StageStatus::Error { message } => {
                    assert!(message.contains("Moonshine process_chunk failed"));
                    assert!(message.contains("simulated adapter failure"));
                }
                other => panic!("expected Moonshine ASR error, got {other:?}"),
            }
            drop(status);
            drain_app_writers(&app);
        }

        let _ = std::fs::remove_dir_all(&data_dir);
    }

    #[test]
    fn asr_partial_revision_recording_updates_ledger_without_legacy_segment() {
        let ledger = Arc::new(Mutex::new(TranscriptLedger::new("session-1")));
        let writer = Arc::new(Mutex::new(None));
        let partial = AsrSpanRevisionPayload {
            span_id: "deepgram:system:start-1000".to_string(),
            provider: "deepgram".to_string(),
            source_id: "system".to_string(),
            provider_item_id: None,
            transcript_segment_id: None,
            speaker_id: None,
            speaker_label: None,
            channel: None,
            text: "hello wor".to_string(),
            start_time: 1.0,
            end_time: 1.7,
            confidence: 0.7,
            is_final: false,
            stability: AsrSpanStability::Partial,
            revision_number: 1,
            supersedes: None,
            turn_id: None,
            end_of_turn: false,
            raw_event_ref: Some("deepgram.results.interim".to_string()),
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms: 1_700_000_000_001,
        };
        let final_revision = AsrSpanRevisionPayload {
            text: "hello world".to_string(),
            is_final: true,
            stability: AsrSpanStability::Final,
            revision_number: 2,
            supersedes: Some("deepgram:system:start-1000@rev1".to_string()),
            end_of_turn: true,
            transcript_segment_id: Some("segment-1".to_string()),
            raw_event_ref: Some("deepgram.results.final".to_string()),
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms: 1_700_000_000_002,
            ..partial.clone()
        };

        assert!(record_asr_span_revision_event(&ledger, &writer, &partial));
        {
            let guard = ledger.lock().unwrap();
            assert_eq!(guard.accepted_event_count, 1);
            assert_eq!(guard.latest_spans.len(), 1);
            assert_eq!(guard.latest_spans[0].text, "hello wor");
            assert!(!guard.latest_spans[0].is_final);
            assert_eq!(guard.latest_spans[0].transcript_segment_id, None);
        }

        assert!(record_asr_span_revision_event(
            &ledger,
            &writer,
            &final_revision
        ));
        let guard = ledger.lock().unwrap();
        assert_eq!(guard.accepted_event_count, 2);
        assert_eq!(guard.latest_spans.len(), 1);
        assert_eq!(guard.latest_spans[0].text, "hello world");
        assert!(guard.latest_spans[0].is_final);
        assert_eq!(
            guard.latest_spans[0].transcript_segment_id.as_deref(),
            Some("segment-1")
        );
    }

    #[test]
    fn asr_partial_revision_queue_full_does_not_advance_ledger() {
        let session_id = "session-asr-queue-full";
        let ledger = Arc::new(Mutex::new(TranscriptLedger::new(session_id)));
        let payload = AsrSpanRevisionPayload {
            span_id: "deepgram:system:start-1000".to_string(),
            provider: "deepgram".to_string(),
            source_id: "system".to_string(),
            provider_item_id: None,
            transcript_segment_id: None,
            speaker_id: None,
            speaker_label: None,
            channel: None,
            text: "queue full should not advance".to_string(),
            start_time: 1.0,
            end_time: 1.7,
            confidence: 0.72,
            is_final: false,
            stability: AsrSpanStability::Partial,
            revision_number: 1,
            supersedes: None,
            turn_id: None,
            end_of_turn: false,
            raw_event_ref: Some("deepgram.results.interim".to_string()),
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms: 1_700_000_000_001,
        };
        let saturated_writer = TranscriptEventWriter::saturated_for_tests(
            crate::projections::TranscriptEvent::from(payload.clone()),
        );
        let writer = Arc::new(Mutex::new(Some(saturated_writer)));

        assert!(
            !record_asr_span_revision_event(&ledger, &writer, &payload),
            "full transcript event queue should reject the ASR revision"
        );

        let guard = ledger.lock().unwrap();
        assert_eq!(guard.accepted_event_count, 0);
        assert!(
            guard.latest_spans.is_empty(),
            "ledger must not advance when accepted event cannot be enqueued"
        );
    }

    #[test]
    fn asr_partial_revision_poisoned_writer_lock_recovers_and_persists() {
        let data_dir = unique_tempdir("asr-poisoned-writer");
        let repo = Arc::new(FileMemoryRepository::with_data_root(&data_dir));
        let repository: Arc<dyn LocalMemoryRepository> = repo.clone();
        let session_id = "session-asr-poisoned-writer";
        let ledger = Arc::new(Mutex::new(TranscriptLedger::new(session_id)));
        let writer = Arc::new(Mutex::new(TranscriptEventWriter::repository(
            session_id, repository,
        )));
        assert!(
            writer.lock().unwrap().is_some(),
            "repository transcript event writer should spawn"
        );
        poison_transcript_event_writer_lock(writer.clone());
        assert!(
            writer.lock().is_err(),
            "precondition: writer lock should be poisoned"
        );

        let payload = projection_asr_payload(
            "projection-poisoned-recovery-span",
            1,
            "poisoned writer recovery persists this revision",
            false,
        );
        assert!(record_asr_span_revision_event(&ledger, &writer, &payload));

        {
            let guard = ledger.lock().unwrap();
            assert_eq!(guard.accepted_event_count, 1);
            assert_eq!(guard.latest_spans.len(), 1);
            assert_eq!(guard.latest_spans[0].span_id, payload.span_id);
        }

        let writer_handle = writer
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
            .expect("repository transcript event writer handle");
        assert!(
            writer_handle.shutdown_with_timeout(Duration::from_secs(2)),
            "repository transcript event writer should drain accepted event"
        );

        let loaded = repo
            .load_transcript_events(session_id)
            .expect("load repository transcript events");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].span_id, payload.span_id);
        assert_eq!(loaded[0].text, payload.text);

        let _ = std::fs::remove_dir_all(&data_dir);
    }

    #[test]
    fn asr_partial_revision_poisoned_missing_writer_does_not_advance_ledger() {
        let session_id = "session-asr-poisoned-missing-writer";
        let ledger = Arc::new(Mutex::new(TranscriptLedger::new(session_id)));
        let writer = Arc::new(Mutex::new(None));
        poison_transcript_event_writer_lock(writer.clone());
        assert!(
            writer.lock().is_err(),
            "precondition: writer lock should be poisoned"
        );

        let payload = projection_asr_payload(
            "projection-poisoned-missing-writer-span",
            1,
            "poisoned missing writer must not advance",
            false,
        );
        assert!(
            !record_asr_span_revision_event(&ledger, &writer, &payload),
            "poisoned writer lock without a recoverable writer cannot prove append acceptance"
        );

        let guard = ledger.lock().unwrap();
        assert_eq!(guard.accepted_event_count, 0);
        assert!(
            guard.latest_spans.is_empty(),
            "ledger must not advance when poisoned writer recovery cannot prove persistence"
        );
    }

    #[test]
    fn asr_partial_revision_recording_rejects_stale_revisions() {
        let ledger = Arc::new(Mutex::new(TranscriptLedger::new("session-1")));
        let writer = Arc::new(Mutex::new(None));
        let revision_two = AsrSpanRevisionPayload {
            span_id: "deepgram:system:start-1000".to_string(),
            provider: "deepgram".to_string(),
            source_id: "system".to_string(),
            provider_item_id: None,
            transcript_segment_id: None,
            speaker_id: None,
            speaker_label: None,
            channel: None,
            text: "hello world".to_string(),
            start_time: 1.0,
            end_time: 1.7,
            confidence: 0.72,
            is_final: false,
            stability: AsrSpanStability::Partial,
            revision_number: 2,
            supersedes: Some("deepgram:system:start-1000@rev1".to_string()),
            turn_id: None,
            end_of_turn: false,
            raw_event_ref: Some("deepgram.results.interim[2]".to_string()),
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms: 1_700_000_000_002,
        };
        let stale_revision = AsrSpanRevisionPayload {
            text: "hello wor".to_string(),
            confidence: 0.7,
            revision_number: 1,
            supersedes: None,
            raw_event_ref: Some("deepgram.results.interim[1]".to_string()),
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms: 1_700_000_000_001,
            ..revision_two.clone()
        };

        assert!(record_asr_span_revision_event(
            &ledger,
            &writer,
            &revision_two
        ));
        assert!(!record_asr_span_revision_event(
            &ledger,
            &writer,
            &stale_revision
        ));

        let guard = ledger.lock().unwrap();
        assert_eq!(guard.accepted_event_count, 1);
        assert_eq!(guard.latest_spans.len(), 1);
        assert_eq!(guard.latest_spans[0].text, "hello world");
        assert_eq!(guard.latest_spans[0].revision_number, 2);
    }

    #[test]
    fn asr_partial_revision_recording_persists_accepted_events_only() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let data_dir = unique_tempdir("asr-events");
        let _guard = DataDirGuard::set(&data_dir);
        let session_id = "session-asr-events";
        let ledger = Arc::new(Mutex::new(TranscriptLedger::new(session_id)));
        let writer = Arc::new(Mutex::new(TranscriptEventWriter::spawn(session_id)));
        assert!(
            writer.lock().unwrap().is_some(),
            "transcript event writer should spawn under isolated data dir"
        );

        let partial = AsrSpanRevisionPayload {
            span_id: "deepgram:system:start-1000".to_string(),
            provider: "deepgram".to_string(),
            source_id: "system".to_string(),
            provider_item_id: None,
            transcript_segment_id: None,
            speaker_id: None,
            speaker_label: None,
            channel: None,
            text: "hello wor".to_string(),
            start_time: 1.0,
            end_time: 1.7,
            confidence: 0.7,
            is_final: false,
            stability: AsrSpanStability::Partial,
            revision_number: 1,
            supersedes: None,
            turn_id: None,
            end_of_turn: false,
            raw_event_ref: Some("deepgram.results.interim[1]".to_string()),
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms: 1_700_000_000_001,
        };
        let final_revision = AsrSpanRevisionPayload {
            text: "hello world".to_string(),
            confidence: 0.92,
            is_final: true,
            stability: AsrSpanStability::Final,
            revision_number: 2,
            supersedes: Some("deepgram:system:start-1000@rev1".to_string()),
            transcript_segment_id: Some("segment-1".to_string()),
            end_of_turn: true,
            raw_event_ref: Some("deepgram.results.final".to_string()),
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms: 1_700_000_000_002,
            ..partial.clone()
        };
        let stale_revision = AsrSpanRevisionPayload {
            text: "stale hello".to_string(),
            raw_event_ref: Some("deepgram.results.interim[stale]".to_string()),
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms: 1_700_000_000_003,
            ..partial.clone()
        };

        assert!(record_asr_span_revision_event(&ledger, &writer, &partial));
        assert!(record_asr_span_revision_event(
            &ledger,
            &writer,
            &final_revision
        ));
        assert!(!record_asr_span_revision_event(
            &ledger,
            &writer,
            &stale_revision
        ));

        let writer_handle = writer
            .lock()
            .unwrap()
            .take()
            .expect("transcript event writer handle");
        assert!(
            writer_handle.shutdown_with_timeout(std::time::Duration::from_secs(2)),
            "transcript event writer should drain accepted events"
        );

        let loaded = load_transcript_events(session_id).expect("load transcript events");
        assert_eq!(loaded.len(), 2, "stale rejection must not append JSONL row");
        assert_eq!(loaded[0].text, "hello wor");
        assert!(!loaded[0].is_final);
        assert_eq!(loaded[0].transcript_segment_id, None);
        assert_eq!(loaded[1].text, "hello world");
        assert!(loaded[1].is_final);
        assert_eq!(loaded[1].revision_number, 2);
        assert_eq!(
            loaded[1].transcript_segment_id.as_deref(),
            Some("segment-1")
        );

        let legacy_transcript_path =
            crate::user_data::transcript_path(session_id).expect("legacy transcript path");
        assert!(
            !legacy_transcript_path.exists(),
            "ASR span revision persistence must not create legacy transcript rows"
        );
    }

    #[test]
    fn asr_partial_revision_recording_can_persist_through_repository_writer() {
        let data_dir = unique_tempdir("asr-repository-events");
        let repo = Arc::new(FileMemoryRepository::with_data_root(&data_dir));
        let repository: Arc<dyn LocalMemoryRepository> = repo.clone();
        let session_id = "session-asr-repository-events";
        let ledger = Arc::new(Mutex::new(TranscriptLedger::new(session_id)));
        let writer = Arc::new(Mutex::new(TranscriptEventWriter::repository(
            session_id, repository,
        )));
        assert!(
            writer.lock().unwrap().is_some(),
            "repository transcript event writer should spawn"
        );

        let partial = AsrSpanRevisionPayload {
            span_id: "deepgram:system:start-1000".to_string(),
            provider: "deepgram".to_string(),
            source_id: "system".to_string(),
            provider_item_id: None,
            transcript_segment_id: None,
            speaker_id: None,
            speaker_label: None,
            channel: None,
            text: "repo hello wor".to_string(),
            start_time: 1.0,
            end_time: 1.7,
            confidence: 0.7,
            is_final: false,
            stability: AsrSpanStability::Partial,
            revision_number: 1,
            supersedes: None,
            turn_id: None,
            end_of_turn: false,
            raw_event_ref: Some("deepgram.results.interim[1]".to_string()),
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms: 1_700_000_000_001,
        };
        let final_revision = AsrSpanRevisionPayload {
            text: "repo hello world".to_string(),
            confidence: 0.92,
            is_final: true,
            stability: AsrSpanStability::Final,
            revision_number: 2,
            supersedes: Some("deepgram:system:start-1000@rev1".to_string()),
            transcript_segment_id: Some("segment-1".to_string()),
            end_of_turn: true,
            raw_event_ref: Some("deepgram.results.final".to_string()),
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms: 1_700_000_000_002,
            ..partial.clone()
        };
        let stale_revision = AsrSpanRevisionPayload {
            text: "repo stale hello".to_string(),
            raw_event_ref: Some("deepgram.results.interim[stale]".to_string()),
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms: 1_700_000_000_003,
            ..partial.clone()
        };

        assert!(record_asr_span_revision_event(&ledger, &writer, &partial));
        assert!(record_asr_span_revision_event(
            &ledger,
            &writer,
            &final_revision
        ));
        assert!(!record_asr_span_revision_event(
            &ledger,
            &writer,
            &stale_revision
        ));

        let writer_handle = writer
            .lock()
            .unwrap()
            .take()
            .expect("repository transcript event writer handle");
        assert!(
            writer_handle.shutdown_with_timeout(std::time::Duration::from_secs(2)),
            "repository transcript event writer should drain accepted events"
        );

        let loaded = repo
            .load_transcript_events(session_id)
            .expect("load repository transcript events");
        assert_eq!(loaded.len(), 2, "stale rejection must not append row");
        assert_eq!(loaded[0].text, "repo hello wor");
        assert!(!loaded[0].is_final);
        assert_eq!(loaded[1].text, "repo hello world");
        assert!(loaded[1].is_final);
        assert_eq!(loaded[1].revision_number, 2);

        let _ = std::fs::remove_dir_all(&data_dir);
    }

    #[test]
    fn runtime_projection_dispatch_applies_fake_notes_and_graph_patches() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("projection-dispatch-success");
        let _guard = DataDirGuard::set(&dir);

        let app = AppState::new();
        let session_id = app.current_session_id();
        let (generator, calls) =
            FnProjectionPatchGenerator::new(|job, _ledger, sequence, created_at_ms| {
                Ok(ProjectionPatchOutcome {
                    patch: test_projection_patch(&job, sequence, created_at_ms),
                    tokens_used: 37,
                })
            });
        let (dispatch, event_sink) = projection_dispatch_for_app(&app, generator);
        let writer = app.transcript_event_writer.clone();
        let final_revision =
            projection_asr_payload("projection-success-span", 1, "Alice met Bob.", true);

        assert!(record_asr_span_revision_event_and_observe_projection(
            &app.transcript_ledger,
            &writer,
            &app.projection_schedulers,
            Some(&dispatch),
            &final_revision
        ));

        wait_until("notes and graph projection dispatch success", || {
            let materialized = app
                .materialized_projection_state
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let schedulers = app
                .projection_schedulers
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            materialized.notes.notes.len() == 1
                && materialized.graph.nodes.len() == 1
                && schedulers.notes().in_flight_job().is_none()
                && schedulers.graph().in_flight_job().is_none()
        });

        assert_eq!(calls.load(Ordering::SeqCst), 2);
        wait_until("projection runtime events emitted", || {
            event_sink.patch_count() == 2
                && event_sink.notes_count() == 1
                && event_sink.graph_count() == 1
        });
        {
            let schedulers = app
                .projection_schedulers
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            assert_eq!(schedulers.notes().metrics().completed_jobs, 1);
            assert_eq!(schedulers.graph().metrics().completed_jobs, 1);
            assert_eq!(schedulers.notes().metrics().failed_jobs, 0);
            assert_eq!(schedulers.graph().metrics().failed_jobs, 0);
            assert_eq!(schedulers.notes().metrics().accepted_patches, 1);
            assert_eq!(schedulers.graph().metrics().accepted_patches, 1);
            assert_eq!(schedulers.notes().metrics().tokens_used, 37);
            assert_eq!(schedulers.graph().metrics().tokens_used, 37);
            assert_eq!(schedulers.notes().metrics().apply_failures, 0);
            assert_eq!(schedulers.graph().metrics().apply_failures, 0);
        }

        drain_app_writers(&app);

        let notes = load_materialized_notes(&session_id)
            .expect("load notes")
            .expect("notes artifact");
        assert_eq!(notes.notes.len(), 1);
        let graph = load_materialized_graph(&session_id)
            .expect("load graph")
            .expect("graph artifact");
        assert_eq!(graph.nodes.len(), 1);
        let events = load_projection_events(&session_id).expect("load projection events");
        assert_eq!(events.len(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn runtime_projection_dispatch_clears_scheduler_on_generation_failure() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("projection-dispatch-failure");
        let _guard = DataDirGuard::set(&dir);

        let app = AppState::new();
        let (generator, calls) =
            FnProjectionPatchGenerator::new(|job, _ledger, _sequence, _created_at_ms| {
                Err(format!("fake generation failure for {:?}", job.kind))
            });
        let (dispatch, event_sink) = projection_dispatch_for_app(&app, generator);
        let writer = app.transcript_event_writer.clone();
        let final_revision =
            projection_asr_payload("projection-failure-span", 1, "No backend works.", true);

        assert!(record_asr_span_revision_event_and_observe_projection(
            &app.transcript_ledger,
            &writer,
            &app.projection_schedulers,
            Some(&dispatch),
            &final_revision
        ));

        wait_until("projection generation failure clears schedulers", || {
            let schedulers = app
                .projection_schedulers
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            schedulers.notes().metrics().failed_jobs == 1
                && schedulers.graph().metrics().failed_jobs == 1
                && schedulers.notes().in_flight_job().is_none()
                && schedulers.graph().in_flight_job().is_none()
        });

        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(event_sink.patch_count(), 0);
        assert_eq!(event_sink.notes_count(), 0);
        assert_eq!(event_sink.graph_count(), 0);
        {
            let materialized = app
                .materialized_projection_state
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            assert!(materialized.notes.notes.is_empty());
            assert!(materialized.graph.nodes.is_empty());
        }

        drain_app_writers(&app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn runtime_projection_dispatch_repairs_stale_apply_with_current_basis() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("projection-dispatch-stale-repair");
        let _guard = DataDirGuard::set(&dir);

        let app = AppState::new();
        let mutated = Arc::new(AtomicBool::new(false));
        let ledger_for_mutation = app.transcript_ledger.clone();
        let mutated_for_generator = mutated.clone();
        let (generator, calls) =
            FnProjectionPatchGenerator::new(move |job, _ledger, sequence, created_at_ms| {
                if job.kind == ProjectionKind::Notes
                    && !mutated_for_generator.swap(true, Ordering::SeqCst)
                {
                    let mut ledger = ledger_for_mutation
                        .lock()
                        .unwrap_or_else(|p| p.into_inner());
                    ledger
                        .apply_event(crate::projections::TranscriptEvent {
                            span_id: "projection-repair-new-span".to_string(),
                            provider: "projection-test".to_string(),
                            source_id: "system".to_string(),
                            provider_item_id: Some("projection-repair-new-span".to_string()),
                            transcript_segment_id: Some(
                                "segment-projection-repair-new-span".to_string(),
                            ),
                            speaker_id: Some("speaker-1".to_string()),
                            speaker_label: Some("Speaker 1".to_string()),
                            channel: None,
                            text: "Newer context arrived before apply.".to_string(),
                            start_time: 2.0,
                            end_time: 3.0,
                            confidence: 0.94,
                            is_final: true,
                            stability: crate::projections::TranscriptEventStability::Final,
                            revision_number: 1,
                            supersedes: None,
                            turn_id: None,
                            end_of_turn: true,
                            raw_event_ref: Some("projection-test[repair]".to_string()),
                            capture_latency_ms: None,
                            asr_latency_ms: None,
                            received_at_ms: 1_700_000_000_010,
                        })
                        .expect("mutate ledger with newer context");
                }
                Ok(ProjectionPatchOutcome {
                    patch: test_projection_patch(&job, sequence, created_at_ms),
                    tokens_used: 41,
                })
            });
        let (dispatch, event_sink) = projection_dispatch_for_app(&app, generator);

        {
            let mut ledger = app
                .transcript_ledger
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            ledger
                .apply_event(crate::projections::TranscriptEvent::from(
                    projection_asr_payload(
                        "projection-repair-old-span",
                        1,
                        "Original context.",
                        true,
                    ),
                ))
                .expect("seed old basis");
            let mut schedulers = app
                .projection_schedulers
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let observation = schedulers.observe_ledger(&ledger, 10);
            let notes_job = match observation.notes {
                ProjectionSchedulerDecision::StartJob { job } => job,
                other => panic!("expected notes start job, got {other:?}"),
            };
            drop(schedulers);
            drop(ledger);
            run_projection_job(dispatch.clone(), notes_job);
        }

        wait_until("stale projection apply repair completes", || {
            let materialized = app
                .materialized_projection_state
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let schedulers = app
                .projection_schedulers
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            materialized.notes.notes.len() == 1
                && materialized.notes.notes[0].basis.span_revisions.len() == 2
                && schedulers.notes().metrics().stale_discards == 1
                && schedulers.notes().metrics().repair_jobs_started == 1
                && schedulers.notes().metrics().completed_jobs == 1
                && schedulers.notes().in_flight_job().is_none()
        });

        assert!(
            calls.load(Ordering::SeqCst) >= 2,
            "stale apply should generate original and repair patches"
        );
        assert_eq!(event_sink.patch_count(), 1);
        assert_eq!(event_sink.notes_count(), 1);
        assert_eq!(event_sink.graph_count(), 0);

        drain_app_writers(&app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn runtime_projection_dispatch_ignores_partials_even_with_generator() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("projection-dispatch-partial");
        let _guard = DataDirGuard::set(&dir);

        let app = AppState::new();
        let (generator, calls) =
            FnProjectionPatchGenerator::new(|job, _ledger, sequence, created_at_ms| {
                Ok(ProjectionPatchOutcome {
                    patch: test_projection_patch(&job, sequence, created_at_ms),
                    tokens_used: 37,
                })
            });
        let (dispatch, event_sink) = projection_dispatch_for_app(&app, generator);
        let writer = Arc::new(Mutex::new(None));
        let partial = projection_asr_payload("projection-partial-span", 1, "still partial", false);

        assert!(record_asr_span_revision_event_and_observe_projection(
            &app.transcript_ledger,
            &writer,
            &app.projection_schedulers,
            Some(&dispatch),
            &partial
        ));

        let schedulers = app
            .projection_schedulers
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        assert_eq!(schedulers.notes().metrics().jobs_started, 0);
        assert_eq!(schedulers.graph().metrics().jobs_started, 0);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(event_sink.patch_count(), 0);
        assert_eq!(event_sink.notes_count(), 0);
        assert_eq!(event_sink.graph_count(), 0);
        drop(schedulers);

        drain_app_writers(&app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn runtime_projection_scheduler_observes_finals_without_partial_job_churn() {
        let session_id = "session-runtime-scheduler";
        let ledger = Arc::new(Mutex::new(TranscriptLedger::new(session_id)));
        let writer = Arc::new(Mutex::new(None));
        let schedulers = Arc::new(Mutex::new(ProjectionSchedulers::new(session_id)));

        let first_partial = AsrSpanRevisionPayload {
            span_id: "deepgram:system:start-1000".to_string(),
            provider: "deepgram".to_string(),
            source_id: "system".to_string(),
            provider_item_id: None,
            transcript_segment_id: None,
            speaker_id: None,
            speaker_label: None,
            channel: None,
            text: "hello wor".to_string(),
            start_time: 1.0,
            end_time: 1.7,
            confidence: 0.7,
            is_final: false,
            stability: AsrSpanStability::Partial,
            revision_number: 1,
            supersedes: None,
            turn_id: None,
            end_of_turn: false,
            raw_event_ref: Some("deepgram.results.interim[1]".to_string()),
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms: 1_700_000_000_001,
        };
        let second_partial = AsrSpanRevisionPayload {
            text: "hello worl".to_string(),
            revision_number: 2,
            supersedes: Some("deepgram:system:start-1000@rev1".to_string()),
            raw_event_ref: Some("deepgram.results.interim[2]".to_string()),
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms: 1_700_000_000_002,
            ..first_partial.clone()
        };
        let final_revision = AsrSpanRevisionPayload {
            text: "hello world".to_string(),
            confidence: 0.92,
            is_final: true,
            stability: AsrSpanStability::Final,
            revision_number: 3,
            supersedes: Some("deepgram:system:start-1000@rev2".to_string()),
            transcript_segment_id: Some("segment-1".to_string()),
            end_of_turn: true,
            raw_event_ref: Some("deepgram.results.final".to_string()),
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms: 1_700_000_000_003,
            ..first_partial.clone()
        };
        let stale_final_revision = AsrSpanRevisionPayload {
            text: "stale final".to_string(),
            confidence: 0.91,
            is_final: true,
            stability: AsrSpanStability::Final,
            revision_number: 2,
            supersedes: Some("deepgram:system:start-1000@rev1".to_string()),
            transcript_segment_id: Some("segment-stale".to_string()),
            end_of_turn: true,
            raw_event_ref: Some("deepgram.results.final[stale]".to_string()),
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms: 1_700_000_000_004,
            ..first_partial.clone()
        };
        let next_final = AsrSpanRevisionPayload {
            span_id: "deepgram:system:start-2000".to_string(),
            text: "next turn".to_string(),
            start_time: 2.0,
            end_time: 2.8,
            confidence: 0.9,
            is_final: true,
            stability: AsrSpanStability::Final,
            revision_number: 1,
            supersedes: None,
            transcript_segment_id: Some("segment-2".to_string()),
            end_of_turn: true,
            raw_event_ref: Some("deepgram.results.final[2]".to_string()),
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms: 1_700_000_000_005,
            ..first_partial.clone()
        };

        assert!(record_asr_span_revision_event_and_observe_projection(
            &ledger,
            &writer,
            &schedulers,
            None,
            &first_partial
        ));
        assert!(record_asr_span_revision_event_and_observe_projection(
            &ledger,
            &writer,
            &schedulers,
            None,
            &second_partial
        ));
        {
            let guard = schedulers.lock().unwrap();
            assert_eq!(guard.notes().metrics().jobs_started, 0);
            assert_eq!(guard.graph().metrics().jobs_started, 0);
        }

        assert!(record_asr_span_revision_event_and_observe_projection(
            &ledger,
            &writer,
            &schedulers,
            None,
            &final_revision
        ));
        {
            let guard = schedulers.lock().unwrap();
            assert_eq!(guard.notes().metrics().jobs_started, 1);
            assert_eq!(guard.graph().metrics().jobs_started, 1);
            assert!(guard.notes().in_flight_job().is_some());
            assert!(guard.graph().in_flight_job().is_some());
        }

        assert!(!record_asr_span_revision_event_and_observe_projection(
            &ledger,
            &writer,
            &schedulers,
            None,
            &stale_final_revision
        ));
        {
            let guard = schedulers.lock().unwrap();
            assert_eq!(
                guard.notes().metrics().coalesced_updates,
                0,
                "stale rejected final must not be observed by notes scheduler"
            );
            assert_eq!(
                guard.graph().metrics().coalesced_updates,
                0,
                "stale rejected final must not be observed by graph scheduler"
            );
        }

        assert!(record_asr_span_revision_event_and_observe_projection(
            &ledger,
            &writer,
            &schedulers,
            None,
            &next_final
        ));
        let guard = schedulers.lock().unwrap();
        assert_eq!(
            guard.notes().metrics().jobs_started,
            1,
            "eligible revisions should coalesce while notes job is in flight"
        );
        assert_eq!(
            guard.graph().metrics().jobs_started,
            1,
            "eligible revisions should coalesce while graph job is in flight"
        );
        assert_eq!(guard.notes().metrics().coalesced_updates, 1);
        assert_eq!(guard.graph().metrics().coalesced_updates, 1);
    }

    #[test]
    fn transcript_diarization_revision_uses_source_timeline_and_basis() {
        let segment = TranscriptSegment {
            id: "segment-1".to_string(),
            source_id: "system-default".to_string(),
            speaker_id: Some("spk_0".to_string()),
            speaker_label: Some("Speaker 0".to_string()),
            text: "hello".to_string(),
            start_time: 1.0,
            end_time: 2.25,
            confidence: 0.82,
        };

        let payload = diarization_span_revision_for_transcript(
            "aws_transcribe",
            &segment,
            "aws_transcribe:system-default:item-1",
            Some("channel-0".to_string()),
            Some("aws.results[0]".to_string()),
            1_700_000_000_000,
        )
        .expect("speaker-labeled transcript should produce a diarization revision");

        assert_eq!(
            payload.span_id,
            "aws_transcribe:system-default:1000-2250:spk_0"
        );
        assert_eq!(payload.provider, "aws_transcribe");
        assert_eq!(payload.timeline_id, "system-default");
        assert_eq!(payload.source_id.as_deref(), Some("system-default"));
        assert_eq!(payload.speaker_id.as_deref(), Some("spk_0"));
        assert_eq!(payload.speaker_label.as_deref(), Some("Speaker 0"));
        assert_eq!(payload.channel.as_deref(), Some("channel-0"));
        assert_eq!(payload.confidence, Some(0.82));
        assert_eq!(payload.stability, DiarizationSpanStability::Final);
        assert_eq!(
            payload.basis_asr_span_ids,
            vec!["aws_transcribe:system-default:item-1".to_string()]
        );
        assert_eq!(
            payload.basis_transcript_segment_ids,
            vec!["segment-1".to_string()]
        );
        assert_eq!(payload.raw_event_ref.as_deref(), Some("aws.results[0]"));
        assert_eq!(payload.received_at_ms, 1_700_000_000_000);
    }

    #[test]
    fn transcript_without_speaker_does_not_emit_diarization_revision() {
        let segment = TranscriptSegment {
            id: "segment-1".to_string(),
            source_id: "system-default".to_string(),
            speaker_id: None,
            speaker_label: None,
            text: "hello".to_string(),
            start_time: 1.0,
            end_time: 2.0,
            confidence: 0.82,
        };

        assert!(
            diarization_span_revision_for_transcript(
                "deepgram",
                &segment,
                "deepgram:system-default:1000-2000",
                None,
                None,
                1_700_000_000_000,
            )
            .is_none()
        );
    }

    #[test]
    fn set_asr_status_writes_through() {
        let ps = Arc::new(RwLock::new(PipelineStatus::default()));
        set_asr_status(
            &ps,
            StageStatus::Error {
                message: "boom".to_string(),
            },
        );
        let guard = ps.read().unwrap();
        match &guard.asr {
            StageStatus::Error { message } => assert_eq!(message, "boom"),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn set_asr_status_recovers_from_poisoned_lock() {
        let ps = Arc::new(RwLock::new(PipelineStatus::default()));

        // Poison the lock by panicking while holding the write guard.
        let ps_clone = Arc::clone(&ps);
        let _ = std::thread::spawn(move || {
            let _g = ps_clone.write().unwrap();
            panic!("intentional panic to poison the lock");
        })
        .join();
        assert!(ps.write().is_err(), "precondition: lock is poisoned");

        // The error status must still be recorded despite the poison — FA-1's
        // whole point is that a poisoned lock cannot silently lose the failure.
        set_asr_status(
            &ps,
            StageStatus::Error {
                message: "after-poison".to_string(),
            },
        );

        let guard = ps.read().unwrap_or_else(|e| e.into_inner());
        match &guard.asr {
            StageStatus::Error { message } => assert_eq!(message, "after-poison"),
            other => panic!("expected Error after poison recovery, got {other:?}"),
        }
    }
}
