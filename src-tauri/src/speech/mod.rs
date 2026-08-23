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
use crate::graph::entities::{ExtractionResult, GraphDelta, GraphSnapshot};
use crate::graph::extraction::RuleBasedExtractor;
use crate::graph::temporal::TemporalKnowledgeGraph;
use crate::llm::{
    ApiClient, LlmEngine, LlmExecutor, LlmPriority, MistralRsEngine, ProjectionPatchAttempt,
};
#[cfg(not(feature = "diarization-clustering"))]
use crate::models::SORTFORMER_MODEL_FILENAME;
use crate::persistence::{FileMemoryRepository, LocalMemoryRepository};
use crate::projection_scheduler::{ProjectionSchedulerDecision, ProjectionSchedulersObservation};
use crate::projections::{
    AppliedBasisCurrency, DiarizationSpanRevision, MaterializedGraph, MaterializedNotes,
    ProjectionApplyError, ProjectionJob, ProjectionKind, ProjectionPatch, SpeakerTimeline,
    TranscriptLedger,
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

/// Snapshot the backend-owned active session id without letting a poisoned
/// lock erase lifecycle ownership. Session ids are content-free identifiers,
/// so logging a rejected generation never exposes transcript data.
fn active_session_id_snapshot(active_session_id: &Arc<RwLock<String>>) -> String {
    match active_session_id.read() {
        Ok(session_id) => session_id.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

/// Lock the current transcript generation iff `expected_session_id` still
/// owns both the published active id and the canonical ledger.
///
/// The returned ledger guard is intentionally held across the subsequent
/// graph/proposal/status commit. Rotation resets the ledger before it clears
/// those aggregates, so this closes the otherwise unavoidable
/// check-then-mutate window: either the old task commits before rotation owns
/// the ledger, or it observes the new generation and becomes a no-op.
fn lock_current_session_generation<'a>(
    active_session_id: &Arc<RwLock<String>>,
    transcript_ledger: &'a Arc<Mutex<TranscriptLedger>>,
    expected_session_id: &str,
) -> Option<std::sync::MutexGuard<'a, TranscriptLedger>> {
    if active_session_id_snapshot(active_session_id) != expected_session_id {
        return None;
    }

    let ledger = match transcript_ledger.lock() {
        Ok(ledger) => ledger,
        Err(poisoned) => {
            log::warn!(
                "Transcript ledger lock poisoned during session-generation check; recovering"
            );
            poisoned.into_inner()
        }
    };
    if ledger.session_id != expected_session_id
        || active_session_id_snapshot(active_session_id) != expected_session_id
    {
        return None;
    }
    Some(ledger)
}

fn session_generation_is_current(
    active_session_id: &Arc<RwLock<String>>,
    transcript_ledger: &Arc<Mutex<TranscriptLedger>>,
    expected_session_id: &str,
) -> bool {
    lock_current_session_generation(active_session_id, transcript_ledger, expected_session_id)
        .is_some()
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

/// LOCK-ORDERING AUDIT (audio-graph-9b11): `emit_and_dispatch_diarization_span_revision`
/// (the sole consumer of `transcript_ledger` on this context) acquires
/// `transcript_ledger` alone, in its own scoped block, to derive a
/// session-relative timestamp — then drops it BEFORE acquiring
/// `speaker_timeline` and `knowledge_graph` (in that order, unchanged from the
/// pre-existing dispatch). The three locks are therefore never held
/// concurrently by this path: `transcript_ledger` -> (dropped) ->
/// `speaker_timeline` -> `knowledge_graph`. This mirrors the discipline
/// `commands.rs`'s `merge_graph_entities_impl` / `approve_agent_proposal_impl`
/// established for the manual-write call sites (audio-graph-4b52): compute the
/// ledger-derived timestamp in a scoped block, then take the graph lock
/// separately.
///
/// IMPORTANT — `lock_current_session_generation` is NOT an example of that
/// standalone/scoped-and-dropped discipline; do not extend this audit's
/// "every other ledger site drops it first" claim to it. It deliberately
/// *returns* the live `transcript_ledger` `MutexGuard` (see its own doc
/// comment), and `apply_extraction_result_if_current` binds that guard as
/// `_generation_guard` and holds it across a subsequent `knowledge_graph.lock()`
/// — a real, permanent `transcript_ledger` -> `knowledge_graph` nesting on the
/// extraction-commit path. That nesting happens to be the SAME direction as
/// this dispatch path (ledger acquired-and-released before graph), so it does
/// not create a cycle with this function. But it does mean the reverse order
/// — `knowledge_graph` or `speaker_timeline` acquired first, `transcript_ledger`
/// second — is permanently forbidden anywhere in this codebase, not merely
/// avoided on this dispatch path: taking it would deadlock against
/// `apply_extraction_result_if_current`'s held ledger guard. Verified by
/// grepping every `transcript_ledger` / `speaker_timeline` / `knowledge_graph`
/// lock site in `commands.rs`, `speech/mod.rs`, `state.rs`,
/// `persistence/mod.rs`, and `speech/tests_integration.rs`: no site anywhere
/// acquires `knowledge_graph`/`speaker_timeline` first and `transcript_ledger`
/// second, so this new order cannot invert an existing one.
struct DiarizationDispatchContext<'a, E: DiarizationEventSink + ?Sized> {
    event_sink: &'a E,
    speaker_timeline: &'a Arc<Mutex<SpeakerTimeline>>,
    knowledge_graph: &'a Arc<Mutex<TemporalKnowledgeGraph>>,
    graph_snapshot: &'a Arc<RwLock<GraphSnapshot>>,
    /// Anchor source for converting the live diarization retcon's wall-clock
    /// receipt time into the graph's session-relative-seconds clock, via
    /// `TranscriptLedger::session_relative_timestamp` (audio-graph-9b11, same
    /// bug class/fix as audio-graph-4b52's manual-write call sites). Locked
    /// standalone and dropped before `speaker_timeline`/`knowledge_graph` — see
    /// the lock-ordering audit above.
    transcript_ledger: &'a Arc<Mutex<TranscriptLedger>>,
    /// Session the accepted revision is durably appended under, so a live
    /// speaker relabel survives reload and can be replayed into a
    /// `SpeakerTimeline` (ADR-0025 §2b / ADR-0026 §3 cross-reload retcon).
    session_id: &'a str,
}

/// Best available anchor id for `TranscriptLedger::session_relative_timestamp`
/// from a diarization span revision's own basis references (audio-graph-9b11).
///
/// Prefers the first `basis_asr_span_ids` entry: it maps 1:1 onto the ledger's
/// `TranscriptEvent::span_id` (the field every recorded ASR span revision sets
/// unconditionally), so it is the more reliable exact-match candidate.
/// `basis_transcript_segment_ids` is the fallback — it maps onto
/// `TranscriptEvent::transcript_segment_id`, an `Option` that a partial-revision
/// producer can leave unset. When a revision carries neither (e.g. the
/// clustering backend's provisional spans, which have no ASR basis at all),
/// this returns `""`, which resolves through `session_relative_timestamp`'s own
/// any-span / `0.0` fallback tiers rather than forcing that fallback
/// unconditionally the way an always-empty anchor would.
fn diarization_revision_anchor_id(revision: &DiarizationSpanRevision) -> &str {
    revision
        .basis_asr_span_ids
        .first()
        .or_else(|| revision.basis_transcript_segment_ids.first())
        .map(String::as_str)
        .unwrap_or("")
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

// Only the sherpa-onnx streaming receiver (and the unit tests) still build a
// span id from a monotonic sequence counter; the cloud providers key spans off
// provider item ids or start timestamps. Allow it to sit unused when the
// `sherpa-streaming` feature is compiled out.
#[cfg_attr(not(feature = "sherpa-streaming"), allow(dead_code))]
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

    let revision = DiarizationSpanRevision::from(payload);

    // Convert the wall-clock receipt time into the graph's session-relative-
    // seconds clock (audio-graph-9b11, same bug class as audio-graph-4b52):
    // left as raw epoch seconds, a live retcon's re-pointed edge would always
    // be the graph's maximum `valid_from` and could never be evicted. Locked
    // standalone and dropped BEFORE `speaker_timeline`/`knowledge_graph` below
    // — see the lock-ordering audit on `DiarizationDispatchContext`.
    //
    // Deliberate trade-off: this locks `transcript_ledger` and runs
    // `session_relative_timestamp`'s scan over `latest_spans` on EVERY
    // revision, even the common case where no remap fires and `timestamp` is
    // never read (dispatch below only consumes it inside the `Some(remap)`
    // branch). Deferring the lookup until a remap is known would require
    // taking `transcript_ledger` *after* `speaker_timeline`/`knowledge_graph`
    // are already held — the exact reverse-order acquisition the audit above
    // documents as permanently forbidden (it would deadlock against
    // `apply_extraction_result_if_current`'s held ledger guard). So the ledger
    // lock stays up-front and unconditional; it is intentionally paid on
    // every revision to keep the lock order fixed, not an oversight.
    let timestamp = {
        let ledger = match dispatch_ctx.transcript_ledger.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                log::warn!("Transcript ledger mutex poisoned; recovering");
                poisoned.into_inner()
            }
        };
        ledger.session_relative_timestamp(
            diarization_revision_anchor_id(&revision),
            current_unix_millis(),
        )
    };

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
            revision.clone(),
            timestamp,
        );
        if outcome.retcon_fired {
            let delta = graph.has_delta().then(|| graph.take_delta());
            let snapshot = graph.snapshot();
            (outcome, delta, Some(snapshot))
        } else {
            (outcome, None, None)
        }
    };

    // Durably append the accepted revision to the session's speaker log so a
    // live speaker relabel survives reload and can be replayed into a
    // `SpeakerTimeline` (audio-graph-719d; ADR-0025 §2b / ADR-0026 §3). This is
    // deliberately done OUTSIDE the timeline/graph locks and after the in-memory
    // apply, mirroring the ASR ledger-write posture: best-effort, log on
    // failure, never break the live path. Only accepted revisions are persisted
    // so the durable log never diverges from the in-memory timeline with a
    // stale/rejected row.
    if outcome.accepted {
        persist_diarization_span_revision(dispatch_ctx.session_id, &revision);
    }

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

/// Best-effort durable append of an accepted diarization span revision to the
/// session's `<session>.speaker.jsonl` log (audio-graph-719d).
///
/// A failed write is logged and swallowed — persistence must never break the
/// live diarization/retcon path (the same posture as the ASR event writer). An
/// empty session id (no active session, e.g. some diarization-only test seams)
/// is skipped silently rather than logging a validation warning per revision.
fn persist_diarization_span_revision(session_id: &str, revision: &DiarizationSpanRevision) {
    if session_id.is_empty() {
        return;
    }
    if let Err(error) =
        FileMemoryRepository::user_data().append_diarization_span_revision(session_id, revision)
    {
        log::warn!(
            "Failed to persist diarization span revision span_id={} revision={} for session {}: {error}",
            revision.span_id,
            revision.revision_number,
            session_id
        );
    }
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

/// Apply a diarization backend selection's degradation (if any) to the shared
/// pipeline status and notify the UI, once, at worker startup — mirrors
/// `set_asr_status_and_emit` above (audio-graph-586b: `mode=provider` must
/// never be silently overridden by an unannounced Simple-backend fallback).
///
/// No-op when `reason` is `None` (the selected backend matched what `mode`
/// asked for, or diarization is off) — the caller's existing `Running{0}`
/// pre-set (from `start_transcribe`) is left as-is.
///
/// Per-segment status updates (`emit_transcript_and_extract_with_meta`,
/// `run_speech_processor_diarization_only`'s loop) must never clobber a
/// `Degraded` set here back to `Running` — both sites guard on
/// `!matches!(status.diarization, StageStatus::Degraded { .. })` before
/// overwriting, so this call's honest state persists for the whole session.
fn apply_diarization_degradation(
    app_handle: &AppHandle,
    pipeline_status: &Arc<RwLock<PipelineStatus>>,
    reason: Option<DiarizationDegradationReason>,
) {
    let Some(reason) = reason else {
        return;
    };
    log::warn!(
        "Diarization degraded ({}): {}",
        reason.as_wire_code(),
        reason.diagnostic_text()
    );
    {
        let mut status = pipeline_status.write().unwrap_or_else(|e| e.into_inner());
        status.diarization = StageStatus::Degraded {
            reason: reason.as_wire_code().to_string(),
        };
    }
    emit_pipeline_status(app_handle, pipeline_status);
}

/// Suppress a local-backend degradation reason when the ACTIVE provider is
/// already delivering native speaker labels for this session
/// (audio-graph-586b review follow-up).
///
/// `make_diarization_config` only knows about the local engine/asset — it has
/// no visibility into whether the selected cloud provider is diarizing on its
/// own. For providers like Deepgram, `mode=Provider` (the settings default)
/// enables provider-native `diarize=true`, and the receiver's per-segment
/// logic (`speaker_label.is_some()`) then never touches the local worker at
/// all — so reporting "this build doesn't include the neural diarization
/// engine" in that case is a false "basic mode" claim: no neural engine was
/// needed because the provider already labeled the speakers.
///
/// `provider_native_diarization_active` is true iff this session's provider
/// socket was actually opened with diarization requested (e.g.
/// `DeepgramConfig::enable_diarization`, captured by the caller before the
/// provider config moved into its client). When true, this always returns
/// `None` regardless of what the local engine/asset probe found. When false,
/// the local probe's reason (if any) passes through unchanged.
fn diarization_degradation_for_provider_labeled_session(
    provider_native_diarization_active: bool,
    local_engine_degradation: Option<DiarizationDegradationReason>,
) -> Option<DiarizationDegradationReason> {
    if provider_native_diarization_active {
        None
    } else {
        local_engine_degradation
    }
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
        speaker_id: meta.speaker_id,
        speaker_label: meta.speaker_label,
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
    expected_session_id: String,
    source_span_id: String,
    app_handle: AppHandle,
    pending_agent_proposals: Arc<Mutex<HashMap<String, events::AgentProposalPayload>>>,
    active_session_id: Arc<RwLock<String>>,
    transcript_ledger: Arc<Mutex<TranscriptLedger>>,
) {
    let text = segment.text.trim().to_string();
    if text.is_empty() || text == "[speech]" {
        return;
    }

    agent_pool().spawn(move || {
        let _ = run_agent_proposal_task(
            segment,
            text,
            expected_session_id,
            source_span_id,
            app_handle,
            pending_agent_proposals,
            active_session_id,
            transcript_ledger,
        );
    });
}

/// Execute one queued proposal task only while its submission-time session
/// still owns the live aggregate. The ledger guard spans proposal insertion,
/// durable live-card persistence, and UI/status emission, making the whole
/// commit atomic with respect to session rotation.
#[allow(clippy::too_many_arguments)]
fn run_agent_proposal_task(
    segment: TranscriptSegment,
    text: String,
    expected_session_id: String,
    source_span_id: String,
    app_handle: AppHandle,
    pending_agent_proposals: Arc<Mutex<HashMap<String, events::AgentProposalPayload>>>,
    active_session_id: Arc<RwLock<String>>,
    transcript_ledger: Arc<Mutex<TranscriptLedger>>,
) -> bool {
    let Some(_generation_guard) = lock_current_session_generation(
        &active_session_id,
        &transcript_ledger,
        &expected_session_id,
    ) else {
        log::debug!(
            "Discarding stale agent proposal task session_id={} segment_id={}",
            expected_session_id,
            segment.id
        );
        return false;
    };

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
        return true;
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
            return true;
        }
    }

    let live_card = events::LiveAssistCardRecord {
        session_id: expected_session_id.clone(),
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
        FileMemoryRepository::user_data().upsert_live_assist_card(&expected_session_id, &live_card)
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
    true
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

/// Stable, machine-readable classification of why `make_diarization_config`
/// fell back to the Simple heuristic instead of the backend `mode` asked for
/// (audio-graph-586b review follow-up).
///
/// Review finding: the original 586b fix composed hardcoded English prose in
/// Rust and shipped it verbatim to the UI, bypassing this crate's existing
/// typed + translated degradation vocabulary (`SttFidelityDegradation`,
/// `commands.rs`, rendered via
/// `t(\`settings.providerReadiness.fidelity.degradation.${value}\`)`). This
/// enum is that same pattern applied here: `as_wire_code` is the ONLY thing
/// that reaches `StageStatus::Degraded.reason` (never English prose), and the
/// frontend renders `pipeline.diarizationDegradedReason.<code>` — a real
/// translated string in every locale — keyed off it. `diagnostic_text` stays
/// Rust-side only, for `log::warn!`; it is never sent over the wire.
///
/// `#[allow(dead_code)]`: which variants are constructible is
/// build-feature-dependent (`diarization` vs. `diarization-clustering` vs.
/// neither are mutually exclusive build configurations — see
/// `make_diarization_config`), so exactly one of `ClusteringAssetsNotDownloaded`
/// vs. `{EngineNotCompiled, AssetNotDownloaded, AssetInvalid}` is genuinely
/// unconstructed in any single build. Per-variant `cfg_attr` would be more
/// precise but each build direction needs the OPPOSITE gate, so a single
/// blanket allow on the enum is clearer than either half being silently
/// right and the other silently wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum DiarizationDegradationReason {
    /// Neither neural diarization engine (`diarization` nor
    /// `diarization-clustering`) is compiled into this build — today's
    /// shipped configuration (`Cargo.toml`'s `default` features, `release.yml`).
    EngineNotCompiled,
    /// `diarization-clustering` IS compiled, but its two-file pyannote
    /// segmentation + embedding asset pair hasn't been downloaded — distinct
    /// from `EngineNotCompiled` because a neural engine genuinely is present
    /// here, just missing its own asset (review finding: the pre-fix
    /// fallthrough reported "doesn't include the neural diarization engine"
    /// for this case too, which is false under `--features diarization-clustering`).
    ClusteringAssetsNotDownloaded,
    /// `diarization` is compiled, but the Sortformer ONNX asset hasn't been
    /// downloaded (`models::ModelReadiness::NotDownloaded`).
    AssetNotDownloaded,
    /// `diarization` is compiled, and the Sortformer ONNX asset exists but
    /// failed size verification — truncated or corrupt
    /// (`models::ModelReadiness::Invalid`).
    AssetInvalid,
}

impl DiarizationDegradationReason {
    /// The ONLY form of this value that reaches the frontend
    /// (`StageStatus::Degraded.reason`) — a stable `snake_case` code, never
    /// prose. Must have a matching `pipeline.diarizationDegradedReason.<code>`
    /// key in EVERY locale (`i18n/locale-parity.test.ts` enforces parity
    /// across locales generally; this string is the join key).
    fn as_wire_code(self) -> &'static str {
        match self {
            Self::EngineNotCompiled => "engine_not_compiled",
            Self::ClusteringAssetsNotDownloaded => "clustering_assets_not_downloaded",
            Self::AssetNotDownloaded => "asset_not_downloaded",
            Self::AssetInvalid => "asset_invalid",
        }
    }

    /// Human-readable diagnostic text for `log::warn!` only — Rust-side, never
    /// serialized, never sent to the UI.
    fn diagnostic_text(self) -> &'static str {
        match self {
            Self::EngineNotCompiled => {
                "neither neural diarization engine is compiled into this build"
            }
            Self::ClusteringAssetsNotDownloaded => {
                "diarization-clustering is compiled but its pyannote asset pair isn't downloaded"
            }
            Self::AssetNotDownloaded => "the Sortformer ONNX asset isn't downloaded",
            Self::AssetInvalid => {
                "the Sortformer ONNX asset failed size verification (corrupt or incomplete)"
            }
        }
    }
}

/// Build the best available `DiarizationConfig` for the given models
/// directory and the user's global diarization policy.
///
/// Backend selection (highest available first):
/// 1. **Clustering** (sherpa-onnx, unbounded) when the `diarization-clustering`
///    feature is compiled in *and* both the pyannote segmentation + embedding
///    ONNX models exist on disk (ADR-0017 / B16). The live engine is
///    `diarization::worker::LiveDiarizationWorker`, spawned + fed separately —
///    see [`maybe_spawn_clustering_diarization`].
/// 2. **Sortformer** (parakeet-rs, ≤4 speakers) when the `diarization` feature
///    is compiled in and the Sortformer ONNX asset is `Ready` per
///    [`crate::models::sortformer_readiness`] — NOT a bare `Path::exists()`
///    (audio-graph-586b: a truncated download or an upstream-drifted file
///    used to pass `.exists()` and only fail once handed to the ONNX loader).
/// 3. **Simple** signal-based fallback otherwise.
///
/// Clustering and Sortformer are mutually exclusive at build time (ORT link
/// conflict, enforced in `lib.rs`), so at most one neural branch is reachable.
///
/// Returns the config to hand to `DiarizationWorker::new`, plus
/// `Some(DiarizationDegradationReason)` when the backend that's actually
/// about to run falls short of what `mode` asked for — e.g. the neural
/// engine isn't compiled in, or its model asset is missing/invalid, so the
/// crude Simple heuristic runs instead with no other signal anywhere that
/// this happened (audio-graph-586b — field evidence: phantom speakers /
/// mid-word speaker splits, entirely attributable to Simple running where
/// Sortformer was expected, silently). `None` when the selected backend
/// matches expectations, OR when `mode == Off` — a user who explicitly
/// disabled diarization gets no model-asset probing and no degradation report
/// for a backend they didn't ask to run well.
///
/// Two known scope boundaries (review follow-up, audio-graph-586b — evidence-
/// backed decisions to leave as-is rather than fix here):
///
/// 1. `mode == Off` returns the `Simple`-backend `DiarizationConfig::default()`
///    unconditionally, WITHOUT probing for a compiled+ready neural engine
///    first. In a non-default `diarization`/`diarization-clustering` build
///    (neither is in `Cargo.toml`'s `default` features) with the asset
///    already present, this means `Off` can select a plainer backend than
///    the build would otherwise use — a real, narrow (non-shipping-config)
///    behavior difference from a mode-unaware caller, but the ALTERNATIVE
///    (probe anyway, just suppress the report) contradicts this function's
///    own tested intent (`tests_status::diarization_mode_off_skips_asset_probing_and_never_degrades`,
///    whose doc comment is explicit: probing for an asset the user didn't
///    ask to use would itself be a dishonest claim about what they
///    configured). Reversing that design is a product decision, not a
///    review-fix.
/// 2. This function governs backend SELECTION only. It does not gate
///    whether `DiarizationWorker::process_input` runs at all — that call is
///    unconditional at every one of this function's 7 call sites, `mode`
///    or no `mode`, so a `Simple`-backend worker still writes a
///    `speaker_id`/`speaker_label` onto every segment even when the user
///    picked `Off`. This is pre-existing (verified: `DiarizationMode::Off`
///    has never gated worker invocation, before or after this ticket) and
///    would require touching call-site wiring at all 7 sites — a materially
///    larger change than this ticket's actual scope (honest DEGRADATION
///    REPORTING for a backend that already runs).
fn make_diarization_config(
    models_dir: &std::path::Path,
    mode: crate::settings::DiarizationMode,
) -> (DiarizationConfig, Option<DiarizationDegradationReason>) {
    if mode == crate::settings::DiarizationMode::Off {
        log::info!(
            "Diarization mode is Off — using Simple backend without probing neural model assets."
        );
        return (DiarizationConfig::default(), None);
    }

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
            return (
                DiarizationConfig::clustering(
                    seg,
                    emb,
                    crate::diarization::clustering::DEFAULT_CLUSTERING_THRESHOLD,
                ),
                None,
            );
        }
        log::info!(
            "Clustering diarization models not found (seg='{}', emb='{}') — falling back. \
             Download via Settings → Models for unbounded speaker identification.",
            seg.display(),
            emb.display()
        );
        // Returns directly here (review fix, audio-graph-586b: previously
        // fell through to the `diarization`-feature check below, which
        // reported `EngineNotCompiled` — false under this exact build
        // configuration, since we are inside a `#[cfg(feature =
        // "diarization-clustering")]` block: a neural engine IS compiled, only
        // its OWN two-file asset pair is missing).
        //
        // This is this block's (and, under `diarization-clustering`, this
        // FUNCTION's) tail expression, not a `return` — clippy's
        // `needless_return` correctly flags a `return` here as redundant:
        // the `diarization`-feature check + Sortformer match below are
        // gated `#[cfg(not(feature = "diarization-clustering"))]`, so under
        // THIS build this cfg-active block is the last thing in the
        // function body and its trailing expression already IS the
        // function's return value with no `return` needed. Kept as a
        // trailing expression rather than restructured further specifically
        // so an unconditional `return` here does not make the
        // `diarization`-feature check below provably-dead code under THIS
        // build (which `rustc`'s unreachable-code lint — run under
        // `-D warnings` — would otherwise flag). `diarization` and
        // `diarization-clustering` are mutually exclusive at build time (ORT
        // link conflict, `lib.rs`) regardless of this `cfg` split, so no
        // build's actual backend-selection behavior changes.
        (
            DiarizationConfig::default(),
            Some(DiarizationDegradationReason::ClusteringAssetsNotDownloaded),
        )
    }

    #[cfg(not(feature = "diarization-clustering"))]
    {
        if !cfg!(feature = "diarization") {
            log::info!(
                "Neural diarization engine (`diarization` feature) not compiled into this \
                 build — using Simple backend."
            );
            return (
                DiarizationConfig::default(),
                Some(DiarizationDegradationReason::EngineNotCompiled),
            );
        }

        let sortformer_path = models_dir.join(SORTFORMER_MODEL_FILENAME);
        // Trailing expression (not `return ...;`), same `needless_return`
        // reasoning as the clustering block above — this is the tail of
        // this `#[cfg(not(feature = "diarization-clustering"))]` block, and
        // under THIS build that block is the function's own tail position.
        match crate::models::sortformer_readiness(models_dir) {
            crate::models::ModelReadiness::Ready => {
                log::info!(
                    "Sortformer model ready at '{}' — using neural diarization backend",
                    sortformer_path.display()
                );
                (DiarizationConfig::sortformer(sortformer_path), None)
            }
            crate::models::ModelReadiness::NotDownloaded => {
                log::info!(
                    "Sortformer model not found at '{}' — using Simple diarization backend.",
                    sortformer_path.display()
                );
                (
                    DiarizationConfig::default(),
                    Some(DiarizationDegradationReason::AssetNotDownloaded),
                )
            }
            crate::models::ModelReadiness::Invalid => {
                log::warn!(
                    "Sortformer model at '{}' failed size verification (corrupt or incomplete \
                     download) — using Simple diarization backend.",
                    sortformer_path.display()
                );
                (
                    DiarizationConfig::default(),
                    Some(DiarizationDegradationReason::AssetInvalid),
                )
            }
        }
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
    transcript_ledger: Arc<Mutex<TranscriptLedger>>,
    session_id: String,
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
                    transcript_ledger,
                    spans,
                    session_id,
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
#[allow(clippy::too_many_arguments)]
fn run_clustering_emit_loop(
    seg_rx: crossbeam_channel::Receiver<crate::diarization::worker::StableSegment>,
    app_handle: AppHandle,
    speaker_timeline: Arc<Mutex<SpeakerTimeline>>,
    knowledge_graph: Arc<Mutex<TemporalKnowledgeGraph>>,
    graph_snapshot: Arc<RwLock<GraphSnapshot>>,
    transcript_ledger: Arc<Mutex<TranscriptLedger>>,
    spans: Arc<RwLock<VecDeque<crate::diarization::SessionSpeakerSpan>>>,
    session_id: String,
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
        transcript_ledger: &transcript_ledger,
        session_id: &session_id,
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
) -> bool {
    // Reject work that sat in the bounded pool until after a rotation before
    // spending provider/local extraction resources on it.
    if !session_generation_is_current(
        deps.active_session_id,
        deps.transcript_ledger,
        deps.expected_session_id,
    ) {
        log::debug!(
            "Discarding stale extraction before generation session_id={} segment_id={}",
            deps.expected_session_id,
            segment_id
        );
        return false;
    }

    let extraction_start = Instant::now();
    let extraction_result = deps
        .llm_executor
        .extract_entities(
            text.to_string(),
            speaker.to_string(),
            context.to_string(),
            (*deps.llm_provider).clone(),
            LlmPriority::Background,
        )
        .unwrap_or_else(|| deps.graph_extractor.extract(speaker, text));

    apply_extraction_result_if_current(
        extraction_result,
        speaker,
        segment_id,
        timestamp,
        extraction_start.elapsed(),
        deps,
        extraction_count,
        graph_update_count,
    )
}

/// Commit a completed extraction only if its submission-time session still
/// owns the live aggregate. This is the post-provider generation check; the
/// ledger guard stays held through graph/snapshot/status/UI mutation so
/// rotation cannot clear the new session between validation and commit.
#[allow(clippy::too_many_arguments)]
fn apply_extraction_result_if_current(
    extraction_result: ExtractionResult,
    speaker: &str,
    segment_id: &str,
    timestamp: f64,
    extraction_elapsed: Duration,
    deps: &ExtractionDeps<'_>,
    extraction_count: &mut u64,
    graph_update_count: &mut u64,
) -> bool {
    let Some(_generation_guard) = lock_current_session_generation(
        deps.active_session_id,
        deps.transcript_ledger,
        deps.expected_session_id,
    ) else {
        log::debug!(
            "Discarding stale extraction result session_id={} segment_id={}",
            deps.expected_session_id,
            segment_id
        );
        return false;
    };

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
    emit_stage_latency(
        deps.app_handle,
        "entity_extraction",
        None,
        Some(segment_id),
        extraction_elapsed,
    );
    true
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
    pub active_session_id: Arc<RwLock<String>>,
    pub transcript_buffer: Arc<RwLock<VecDeque<TranscriptSegment>>>,
    pub transcript_writer: Arc<Mutex<Option<crate::persistence::TranscriptWriter>>>,
    /// See `AppState::display_transcript_write_misses`.
    pub display_transcript_write_misses: Arc<AtomicU64>,
    pub transcript_event_writer: Arc<Mutex<Option<crate::persistence::TranscriptEventWriter>>>,
    pub transcript_ledger: Arc<Mutex<crate::projections::TranscriptLedger>>,
    pub speaker_timeline: Arc<Mutex<SpeakerTimeline>>,
    pub projection_schedulers: Arc<Mutex<crate::projection_scheduler::ProjectionSchedulers>>,
    pub projection_runtime: ProjectionRuntimeHandle,
    /// Live projection job thread registry (audio-graph-9cc1 / ADR-0045
    /// decision 4). See `AppState::projection_job_workers`.
    pub projection_job_workers: crate::state::ProjectionJobRegistry,
    /// Set at Stop before the registry above is drained. See
    /// `AppState::projection_lane_stopping`.
    pub projection_lane_stopping: Arc<std::sync::atomic::AtomicBool>,
    pub pipeline_status: Arc<RwLock<PipelineStatus>>,
    pub app_handle: AppHandle,
    pub llm_engine: Arc<Mutex<Option<LlmEngine>>>,
    pub api_client: Arc<Mutex<Option<ApiClient>>>,
    pub mistralrs_engine: Arc<Mutex<Option<MistralRsEngine>>>,
    pub llm_executor: LlmExecutor,
    pub llm_provider: LlmProvider,
    /// Whether the session's privacy mode allows session content to leave the
    /// device (`PrivacyMode::ByokCloud`).
    ///
    /// Since ADR-0038 this field authorizes NOTHING. It used to select the
    /// executor's fallback chain; automatic cross-provider fallback is gone, and a
    /// cloud route under a non-`ByokCloud` mode is now refused by the client's own
    /// content-egress policy instead of silently downgraded to a local model. What
    /// remains is a privacy-REPORT input: it is persisted in per-session
    /// data-movement events (`ProjectionMovementFacts::cloud_transfer_allowed`) and
    /// gates whether the remote summary/prefix movement is emitted at all (seed
    /// audio-graph-72d5). Removing it would be an ADR-0027 migration for no safety
    /// gain, so the name is kept and its reach is narrowed.
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
    /// Live projection job thread registry (audio-graph-9cc1 / ADR-0045
    /// decision 4, drain half). `spawn_projection_job` registers into it;
    /// `run_projection_job` self-deregisters on every exit path. See
    /// `AppState::projection_job_workers` for the full rationale.
    projection_job_workers: crate::state::ProjectionJobRegistry,
    /// Set at Stop before the registry above is drained. See
    /// `AppState::projection_lane_stopping`.
    ///
    /// Read in two places: the `projection-retry-<kind>` clock thread
    /// (`spawn_deferred_lane_observation`) polls it so it wakes and exits
    /// instead of firing after Stop has already begun tearing the session
    /// down, and `dispatch_projection_decision` checks it synchronously,
    /// before spawning either a projection job thread or a deferred-retry
    /// clock thread, so a job/clock that would only start after Stop began
    /// is discarded instead of outliving the drain unbounded. Cleared by
    /// `start_transcribe` BEFORE the speech thread that drives the
    /// schedulers spawns (audio-graph-1609) — see the ordering comment
    /// there — so no dispatch on a freshly (re)started lane can ever
    /// observe this still set from a prior Stop.
    projection_lane_stopping: Arc<std::sync::atomic::AtomicBool>,
    event_sink: Arc<dyn ProjectionRuntimeEventSink>,
    patch_generator: Arc<dyn ProjectionPatchGenerator>,
    /// Configured LLM provider for this session — the *intended* projection
    /// destination. Used to ledger the remote-LLM data flow (ADR-0025 §2g /
    /// seed audio-graph-72d5).
    llm_provider: LlmProvider,
    /// Whether the session policy allows session content to leave the device
    /// (derived from `PrivacyMode::ByokCloud`). Gates the whole
    /// context-efficiency remote path: a local-only session emits NO remote
    /// summary/prefix movement (seed audio-graph-72d5).
    ///
    /// Since ADR-0038 this authorizes no provider selection — it is a
    /// privacy-report input only.
    llm_allow_cloud_fallbacks: bool,
    /// Data-movement ledger emitter (content-free). `None` disables emission
    /// (tests that don't assert on the ledger).
    data_movement_sink: Arc<dyn ProjectionDataMovementSink>,
}

trait ProjectionPatchGenerator: Send + Sync {
    /// Returns the dispatch outcome PLUS the identity of the route it actually
    /// reached, so a failure can still ledger under that identity instead of
    /// the session-start snapshot (seeds audio-graph-862c / audio-graph-7da4).
    ///
    /// `notes` is the live Notes-kind snapshot (seed audio-graph-253c part 2)
    /// `run_projection_job` clones under `AppState`'s
    /// `materialized_projection_state` lock and hands to every implementer —
    /// this trait never reaches back into that lock itself. `Some` renders
    /// the "Current notes state" prompt block; `None` omits it (Graph-kind,
    /// or a Notes-kind snapshot whose session id did not match this job's).
    fn generate_projection_patch(
        &self,
        job: ProjectionJob,
        ledger: TranscriptLedger,
        notes: Option<MaterializedNotes>,
        sequence: u64,
        created_at_ms: u64,
    ) -> ProjectionPatchAttempt;
}

trait ProjectionRuntimeEventSink: Send + Sync {
    fn emit_projection_patch(&self, patch: &ProjectionPatch);
    fn emit_materialized_notes(&self, notes: &MaterializedNotes);
    fn emit_materialized_graph(&self, graph: &MaterializedGraph);
}

/// Durable sink for the projection path's content-free data-movement events
/// (ADR-0025 §2g / seed audio-graph-72d5). Abstracted so the runtime uses the
/// file-backed repository in production and tests can record without touching
/// disk.
trait ProjectionDataMovementSink: Send + Sync {
    fn record(&self, session_id: &str, event: &crate::persistence::DataMovementEvent);

    /// Test-observability hook (audio-graph-a6b5 W2 fix round): gives tests
    /// direct value-level visibility into the content-free
    /// `ProjectionMovementFacts` BEFORE it is folded into (or, for some
    /// fields, never folded into — see
    /// `projection_data_movement::ProjectionMovementFacts::no_op_filtered_count`'s
    /// doc comment) the persisted `DataMovementEvent`. `no_op_filtered_count`
    /// has no `MovementCounts` sink by design (an ipc-contract change this
    /// ticket deliberately stays out of), so without this hook nothing
    /// outside a source-text inspection test could ever observe whether the
    /// real value reached this struct. Default no-op: production's
    /// `FileProjectionDataMovementSink` never overrides it, so this call adds
    /// zero production behavior.
    fn record_movement_facts(
        &self,
        _facts: &crate::projection_data_movement::ProjectionMovementFacts,
    ) {
    }
}

/// Production sink: appends to the per-session data-movement JSONL via the
/// file-backed repository (the same seam ASR/artifact events use).
struct FileProjectionDataMovementSink;

impl ProjectionDataMovementSink for FileProjectionDataMovementSink {
    fn record(&self, session_id: &str, event: &crate::persistence::DataMovementEvent) {
        use crate::persistence::LocalMemoryRepository;
        if let Err(error) =
            FileMemoryRepository::user_data().append_data_movement_event(session_id, event)
        {
            log::warn!(
                "Failed to append projection data-movement event session_id={session_id} \
                 event_type={:?} error={error}",
                event.event_type
            );
        }
    }
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
}

impl ProjectionPatchGenerator for ExecutorProjectionPatchGenerator {
    fn generate_projection_patch(
        &self,
        job: ProjectionJob,
        ledger: TranscriptLedger,
        notes: Option<MaterializedNotes>,
        sequence: u64,
        created_at_ms: u64,
    ) -> ProjectionPatchAttempt {
        self.llm_executor.generate_projection_patch(
            job,
            ledger,
            notes,
            self.llm_provider.clone(),
            sequence,
            created_at_ms,
        )
    }
}

impl TranscriptProcessingContext {
    fn projection_dispatch_context(&self) -> ProjectionDispatchContext {
        ProjectionDispatchContext {
            transcript_ledger: self.transcript_ledger.clone(),
            projection_schedulers: self.projection_schedulers.clone(),
            projection_runtime: self.projection_runtime.clone(),
            projection_job_workers: self.projection_job_workers.clone(),
            projection_lane_stopping: self.projection_lane_stopping.clone(),
            event_sink: Arc::new(TauriProjectionRuntimeEventSink {
                app_handle: self.app_handle.clone(),
            }),
            patch_generator: Arc::new(ExecutorProjectionPatchGenerator {
                llm_executor: self.llm_executor.clone(),
                llm_provider: self.llm_provider.clone(),
            }),
            llm_provider: self.llm_provider.clone(),
            llm_allow_cloud_fallbacks: self.llm_allow_cloud_fallbacks,
            data_movement_sink: Arc::new(FileProjectionDataMovementSink),
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
        active_session_id: shared.active_session_id,
        transcript_buffer: shared.transcript_buffer,
        transcript_writer: shared.transcript_writer,
        display_transcript_write_misses: shared.display_transcript_write_misses,
        transcript_event_writer: shared.transcript_event_writer,
        transcript_ledger: shared.transcript_ledger,
        speaker_timeline: shared.speaker_timeline,
        projection_schedulers: shared.projection_schedulers,
        projection_runtime: shared.projection_runtime,
        projection_job_workers: shared.projection_job_workers,
        projection_lane_stopping: shared.projection_lane_stopping,
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
            let Some(writer) = writer_guard.as_ref() else {
                log::warn!(
                    "Transcript event writer unavailable for span_id={} revision={}; ledger was not advanced",
                    transcript_event.span_id,
                    transcript_event.revision_number
                );
                return false;
            };
            if !writer.append(&transcript_event) {
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
        log_attempt_budget_exhausted(
            &observation.notes,
            schedulers.notes().metrics().attempts_exhausted,
        );
        log_attempt_budget_exhausted(
            &observation.graph,
            schedulers.graph().metrics().attempts_exhausted,
        );
        observation
    };
    if let Some(dispatch) = projection_dispatch {
        dispatch_projection_observation(dispatch.clone(), observation);
    }
}

/// Stable, content-free retune signal for
/// [`ProjectionSchedulerDecision::AttemptBudgetExhausted`] — the log key
/// named in `PROJECTION_LANE_ATTEMPT_BUDGET`'s tuning-procedure doc comment.
/// Logs only `kind`, the attempt count, and the cumulative
/// `attempts_exhausted` metric — never the basis or transcript content — at
/// `info` level so it is observable under the default log filter without
/// needing `RUST_LOG=debug`.
fn log_attempt_budget_exhausted(decision: &ProjectionSchedulerDecision, attempts_exhausted: u64) {
    if let ProjectionSchedulerDecision::AttemptBudgetExhausted { kind, attempts, .. } = decision {
        log::info!(
            "projection_scheduler.attempt_budget_exhausted kind={:?} attempts={} attempts_exhausted={}",
            kind,
            attempts,
            attempts_exhausted
        );
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
        | ProjectionSchedulerDecision::FailedAndStartedFollowUp { job, .. }
        | ProjectionSchedulerDecision::FailedStaleAndStartedRepair { job, .. } => {
            // ADR-0045 decision 4 (audio-graph-9cc1, adversarial-review fix):
            // this is the ONLY call site of `spawn_projection_job`, reached
            // both from a fresh ASR-triggered dispatch AND from a completing
            // job's own tail (`finish_projection_scheduler_job` ->
            // `dispatch_projection_decision`, running ON the completing
            // job's thread). `stop_capture_impl` sets
            // `projection_lane_stopping` BEFORE it joins sp/asr and drains
            // this registry, so checking it here — synchronously, before the
            // thread that would carry the job is ever spawned — closes the
            // race where a job completing mid-drain chains a mandatory
            // follow-up (`CompletedAndStartedFollowUp` on an
            // `AppendOnlyStale` completion is unconditional) AFTER the
            // drain's registry snapshot has already been taken.
            //
            // audio-graph-1609 fix: the scheduler already recorded `job` as
            // its new in-flight job by this point (`start_job` runs before
            // the decision is returned) — leaving it un-abandoned here used
            // to be treated as intentional on the theory that
            // `rotate_session` resets scheduler in-flight state on the next
            // session regardless. That theory was false for the common
            // case: `rotate_session` only runs on an explicit New Session;
            // restarting capture+transcribe on the SAME session
            // (`start_capture_impl`/`start_transcribe`) never touches the
            // schedulers, so the un-abandoned entry became a PHANTOM
            // in-flight job — no thread will ever complete or fail it, so
            // every subsequent `observe_ledger` on this lane returned
            // `Coalesced` behind it forever, and the lane never projected
            // again for the rest of the session. `abandon_discarded_projection_job`
            // releases exactly that entry so the lane can start fresh work
            // on its very next observation.
            if dispatch.projection_lane_stopping.load(Ordering::SeqCst) {
                log::debug!(
                    "projection_lane_stopping set; discarding dispatch instead of spawning job_id={} kind={:?}",
                    job.id,
                    job.kind
                );
                abandon_discarded_projection_job(&dispatch, &job);
                return;
            }
            spawn_projection_job(dispatch, job);
        }
        // ADR-0045 decision 3 (audio-graph-bf5d): a same-basis `Current`
        // failure under budget armed exactly one deferred retry
        // (`deferred_retry_at_ms`). Spawn the single one-shot clock thread
        // that fires it even if no further final ASR revision ever arrives
        // — mirrors the stopping check above, at the analogous point (before
        // any thread exists), so a lane already stopping never gets a
        // pointless clock armed for it. `None` means this same failure
        // exhausted the budget: nothing to arm.
        //
        // audio-graph-1609 fold-in: discarding this spawn leaves
        // `deferred_retry_at_ms` armed with no clock thread ever registered
        // to fire it (the bf5d orphaned-deferral gap) — `abandon_discarded_deferred_retry`
        // clears it so the next same-basis observation retries immediately
        // instead of silently degrading to event-driven-only with no active
        // signal that it did.
        //
        // audio-graph-fa56 known gap (disclosed, not fixed here): this
        // branch is reachable WHILE `drain_projection_job_workers` is still
        // joining the projection job thread that hit this failure — a job
        // still in flight when Stop begins can finish and fail during the
        // drain. When that happens, `abandon_discarded_deferred_retry`
        // clears `deferred_retry_at_ms` back to `None` before
        // `stop_capture_impl`'s post-drain `log_abandoned_deferred_retries_
        // after_stop` (commands.rs) ever reads it, so this failure is
        // invisible to that WARN and to the diagnostics snapshot it
        // persists — the only signal is the `log::debug!` immediately below.
        // This is the exact same user-facing gap the WARN exists to surface
        // (a failed apply near Stop whose retry never runs); closing it
        // needs a signal emitted from THIS discard site (e.g. promoting
        // this log line, or a sibling one, to `log::warn!`), which is out
        // of scope for audio-graph-fa56's detection-at-Stop primitive.
        ProjectionSchedulerDecision::FailedCurrent {
            failed_job_id,
            kind,
            deferred_retry_at_ms: Some(deferred_retry_at_ms),
        } => {
            if dispatch.projection_lane_stopping.load(Ordering::SeqCst) {
                log::debug!(
                    "projection_lane_stopping set; not arming deferred retry clock for failed_job_id={failed_job_id} kind={kind:?}"
                );
                abandon_discarded_deferred_retry(&dispatch, &kind, deferred_retry_at_ms);
                return;
            }
            spawn_deferred_lane_observation(dispatch, kind, failed_job_id, deferred_retry_at_ms);
        }
        ProjectionSchedulerDecision::Idle
        | ProjectionSchedulerDecision::Coalesced { .. }
        | ProjectionSchedulerDecision::CompletedCurrent { .. }
        | ProjectionSchedulerDecision::DiscardedStaleNoCurrentBasis { .. }
        | ProjectionSchedulerDecision::FailedCurrent {
            deferred_retry_at_ms: None,
            ..
        }
        | ProjectionSchedulerDecision::FailedStaleNoCurrentBasis { .. }
        | ProjectionSchedulerDecision::IgnoredSupersededCompletion { .. }
        // Emit-only (audio-graph-ff10 / ADR-0045): no consumer is wired here.
        // Routing a stalled lane onward is ADR-0036 territory.
        | ProjectionSchedulerDecision::AttemptBudgetExhausted { .. } => {}
    }
}

/// audio-graph-1609: companion to `dispatch_projection_decision`'s
/// STOPPING discard branch for the job-spawn arm. Re-acquires the scheduler
/// lock — safe here because both of `dispatch_projection_decision`'s
/// callers (`observe_projection_schedulers_for_asr_revision` and
/// `finish_projection_scheduler_job`) release it before ever calling
/// `dispatch_projection_decision` — and releases exactly the `in_flight`
/// entry `start_job` recorded for `job`, so this lane is not wedged behind
/// `Coalesced` for a job that will never actually run. See
/// `ProjectionScheduler::abandon_in_flight` for the full contract (does not
/// touch `failed_attempts` or `pending_since_ms`).
fn abandon_discarded_projection_job(dispatch: &ProjectionDispatchContext, job: &ProjectionJob) {
    let mut schedulers = dispatch
        .projection_schedulers
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    schedulers.abandon_in_flight(&job.kind, &job.id, &job.session_id);
}

/// audio-graph-1609 fold-in: companion to `dispatch_projection_decision`'s
/// STOPPING discard branch for the deferred-retry clock-spawn arm (ADR-0045
/// decision 3, audio-graph-bf5d). Same re-acquire-the-lock safety as
/// `abandon_discarded_projection_job` above. Clears `deferred_retry_at_ms`
/// so this lane's armed retry does not sit orphaned with no clock source —
/// see `ProjectionScheduler::abandon_deferred_retry` for the full contract
/// (matches by the exact deadline value; does not touch `last_failed_basis`
/// or `failed_attempts`).
fn abandon_discarded_deferred_retry(
    dispatch: &ProjectionDispatchContext,
    kind: &ProjectionKind,
    deferred_retry_at_ms: u64,
) {
    let mut schedulers = dispatch
        .projection_schedulers
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    schedulers.abandon_deferred_retry(kind, deferred_retry_at_ms);
}

#[derive(Debug, Clone, Copy)]
enum ProjectionJobCompletion {
    Completed,
    Failed,
}

/// Remove the `(kind, job_id)` entry from the live projection-job registry,
/// if present.
///
/// Matches by BOTH `kind` and `job_id` — never by kind alone or by vec
/// position — so a same-kind entry belonging to a DIFFERENT job (e.g. a
/// chained follow-up job registered before the finishing job's own
/// self-deregistration runs) is never removed by mistake. A `job_id` that
/// does not match any registered entry is a no-op: nothing is removed, and
/// in particular no [`std::thread::JoinHandle`] is ever joined here (this is
/// always called with the registry's own handles still owned by the vec —
/// dropping is the only thing that happens to a matched entry, never a join,
/// since the caller may itself be running ON the thread the removed handle
/// refers to, where a self-join would deadlock).
fn deregister_projection_job(
    registry: &crate::state::ProjectionJobRegistry,
    kind: &ProjectionKind,
    job_id: &str,
) {
    let mut guard = registry
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(pos) = guard
        .iter()
        .position(|(entry_kind, entry_job_id, _)| entry_kind == kind && entry_job_id == job_id)
    {
        guard.remove(pos);
    }
}

/// RAII guard that self-deregisters a projection job's own registry entry on
/// drop (audio-graph-9cc1 / ADR-0045 decision 4, drain half).
///
/// `run_projection_job` has several early-return points (superseded-job
/// discards) plus the normal completion/failure tail; holding this guard for
/// the whole function body means every exit path — including an unexpected
/// panic — deregisters exactly once, via [`deregister_projection_job`]'s
/// kind+job_id match.
struct ProjectionJobRegistrationGuard {
    registry: crate::state::ProjectionJobRegistry,
    kind: ProjectionKind,
    job_id: String,
}

impl Drop for ProjectionJobRegistrationGuard {
    fn drop(&mut self) {
        deregister_projection_job(&self.registry, &self.kind, &self.job_id);
    }
}

fn spawn_projection_job(dispatch: ProjectionDispatchContext, job: ProjectionJob) {
    let failure_dispatch = dispatch.clone();
    let failure_job = job.clone();
    let job_id = job.id.clone();
    let job_kind = job.kind.clone();
    let registry = dispatch.projection_job_workers.clone();
    let thread_name = format!("projection-{}", projection_kind_key(&job.kind));
    // The child cannot be handed its own `JoinHandle` (that only exists once
    // `.spawn()` returns to the PARENT), so registration necessarily happens
    // after the thread starts running. A job fast enough to finish and
    // self-deregister before the parent's registry push would land would
    // otherwise leave a permanent phantom entry (never removed — the guard
    // only fires once). This one-shot channel makes registration
    // happens-before any work the job can do, closing that race.
    let (registered_tx, registered_rx) = std::sync::mpsc::channel::<()>();
    match std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            let _ = registered_rx.recv();
            run_projection_job(dispatch, job)
        }) {
        Ok(handle) => {
            // Register the live handle instead of discarding it (previously
            // `Ok(_) => {}`), so `stop_capture_impl` can drain and join it at
            // Stop instead of leaving it running unbounded in the background.
            {
                let mut guard = registry
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                guard.push((job_kind, job_id, handle));
            }
            let _ = registered_tx.send(());
        }
        Err(error) => {
            log::error!(
                "Failed to spawn projection job thread job_id={} error={}",
                job_id,
                error
            );
            finish_projection_scheduler_job(
                failure_dispatch,
                &failure_job,
                ProjectionJobCompletion::Failed,
            );
        }
    }
}

/// One-shot clock thread for ADR-0045 decision 3's single deferred retry
/// (audio-graph-bf5d): the sole real-time source that fires a same-basis
/// retry when NO further final ASR revision ever arrives to drive it
/// event-driven. `dispatch_projection_decision` spawns exactly one of these
/// per armed deferral (`FailedCurrent`'s `deferred_retry_at_ms` was `Some`);
/// a same-basis failure that already exhausted the attempt budget arms
/// none, so no clock thread is spawned for it.
///
/// `deferred_retry_at_ms` is an absolute [`current_unix_millis`]-epoch
/// timestamp, not a duration — the SAME value `fail_in_flight` computed and
/// the SAME clock `current_unix_millis` reads, so the thread's own
/// wall-clock comparisons need no conversion. This is also the seam tests
/// use to shorten the delay: nothing here reads
/// `PROJECTION_DEFERRED_RETRY_DELAY_MS` directly, so a test can pass any
/// near-future timestamp instead of a real ~60s deadline (mirrors
/// `drain_projection_job_workers` taking its flush `timeout` as a parameter
/// rather than hard-coding `PROJECTION_JOB_FLUSH_TIMEOUT` internally).
///
/// Registered in the SAME `projection_job_workers` registry
/// `spawn_projection_job` uses (audio-graph-9cc1), under a synthetic id
/// (`projection-retry:<failed_job_id>`) that can never collide with a real
/// projection job id, so `stop_capture_impl`'s drain sees and joins this
/// thread exactly like a real projection job thread. Registration
/// happens-before the thread does any work, via the SAME one-shot channel
/// handshake `spawn_projection_job` uses — load-bearing here because tests
/// legitimately pass a near-immediate deadline (see above), where the race
/// `spawn_projection_job`'s own comment describes (a thread fast enough to
/// finish before the parent's registry push lands, leaving a permanent
/// phantom entry) is real, not merely theoretical.
///
/// Polls `projection_lane_stopping` every 250ms so it never sleeps past a
/// Stop — bounding how long `drain_projection_job_workers`'s
/// `PROJECTION_JOB_FLUSH_TIMEOUT` budget can be spent waiting on this
/// thread — and re-checks the flag immediately before firing, so a Stop
/// that begins in the final poll window (after the deadline check passed
/// but before the retry is triggered) still wins: no retry fires after
/// `projection_lane_stopping` is observed set, at either check point.
fn spawn_deferred_lane_observation(
    dispatch: ProjectionDispatchContext,
    kind: ProjectionKind,
    failed_job_id: String,
    deferred_retry_at_ms: u64,
) {
    let registry_id = format!("projection-retry:{failed_job_id}");
    let registry = dispatch.projection_job_workers.clone();
    let stopping = dispatch.projection_lane_stopping.clone();
    let thread_name = format!("projection-retry-{}", projection_kind_key(&kind));
    let thread_kind = kind.clone();
    let thread_registry_id = registry_id.clone();
    let thread_registry = registry.clone();
    let (registered_tx, registered_rx) = std::sync::mpsc::channel::<()>();
    match std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            let _ = registered_rx.recv();
            // Self-deregisters on every exit path below (due-and-fired,
            // stopping-observed, or an unexpected panic) — mirrors
            // `run_projection_job`'s `ProjectionJobRegistrationGuard` usage.
            let _registration_guard = ProjectionJobRegistrationGuard {
                registry: thread_registry,
                kind: thread_kind,
                job_id: thread_registry_id,
            };
            loop {
                if stopping.load(Ordering::SeqCst) {
                    log::debug!(
                        "projection_lane_stopping set; deferred retry clock exiting without firing failed_job_id={failed_job_id}"
                    );
                    return;
                }
                if current_unix_millis() >= deferred_retry_at_ms {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            // Re-check immediately before firing: closes the race where Stop
            // begins during the final poll window, after the deadline check
            // above passed but before this thread acts on it.
            if stopping.load(Ordering::SeqCst) {
                log::debug!(
                    "projection_lane_stopping set at the due time; deferred retry clock exiting without firing failed_job_id={failed_job_id}"
                );
                return;
            }
            trigger_deferred_projection_retry(&dispatch, &failed_job_id);
        }) {
        Ok(handle) => {
            {
                let mut guard = registry
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                guard.push((kind, registry_id, handle));
            }
            let _ = registered_tx.send(());
        }
        Err(error) => {
            // Review note (adr0045/bf5d-deferred-retry): unlike
            // `spawn_projection_job`'s `Err` arm (which calls
            // `finish_projection_scheduler_job(Failed)` to keep scheduler
            // state truthful), this arm does NOT clear the scheduler's
            // already-armed `deferred_retry_at_ms` — there is deliberately no
            // scheduler-mutating fallback here. The failure mode is
            // liveness-only, identical to the stop/restart-without-rotation
            // gap documented at `state.projection_lane_stopping`'s Start-path
            // reset in `commands.rs`: the deferral stays live with no clock
            // to fire it, and the lane reverts to event-driven-only for that
            // basis until a new final ASR revision supersedes it via
            // `start_job`. Not corrective (no phantom in-flight job, no
            // double-dispatch risk) — just silent. Deliberately not adding a
            // scheduler-clearing fallback here: doing so would need a new
            // mutating entry point into `ProjectionSchedulers` reachable
            // from OUTSIDE the `observe_ledger`/`fail_in_flight` dispatch
            // tail, for a trigger (thread spawn failure, i.e. OOM or a
            // thread-limit) that is already exceedingly rare, and getting
            // its match-by-kind-and-basis race-safety wrong would risk
            // clearing a deferral a NEWER failure had legitimately re-armed
            // concurrently. Thread-spawn exhaustion severe enough to hit
            // this arm is a whole-process concern well beyond one retry
            // clock in practice.
            log::error!(
                "Failed to spawn deferred projection retry clock thread registry_id={registry_id} error={error}"
            );
        }
    }
}

/// The clock thread's fire action (audio-graph-bf5d): re-observe the ledger
/// exactly like a final ASR revision would, then dispatch the result through
/// the SAME `dispatch_projection_observation` tail — no separate dispatch
/// logic to keep in sync. Reads the CURRENT ledger/scheduler state rather
/// than anything captured at arm-time, so a stale/late fire (e.g. a
/// deferral that outlived its usefulness because the basis moved on) is
/// self-healing by construction: `observe_ledger` decides purely from what
/// is true now, never from what was true when this clock thread was armed.
fn trigger_deferred_projection_retry(dispatch: &ProjectionDispatchContext, failed_job_id: &str) {
    let observation = {
        let ledger = match dispatch.transcript_ledger.lock() {
            Ok(ledger) => ledger,
            Err(poisoned) => {
                log::warn!(
                    "Transcript ledger lock poisoned during deferred projection retry; recovering"
                );
                poisoned.into_inner()
            }
        };
        let mut schedulers = match dispatch.projection_schedulers.lock() {
            Ok(schedulers) => schedulers,
            Err(poisoned) => {
                log::warn!(
                    "Projection scheduler lock poisoned during deferred projection retry; recovering"
                );
                poisoned.into_inner()
            }
        };
        let now_ms = current_unix_millis();
        let observation = schedulers.observe_ledger(&ledger, now_ms);
        log::debug!(
            "projection_schedulers.observe_deferred_retry failed_job_id={failed_job_id} notes={:?} graph={:?}",
            observation.notes,
            observation.graph
        );
        log_attempt_budget_exhausted(
            &observation.notes,
            schedulers.notes().metrics().attempts_exhausted,
        );
        log_attempt_budget_exhausted(
            &observation.graph,
            schedulers.graph().metrics().attempts_exhausted,
        );
        observation
    };
    dispatch_projection_observation(dispatch.clone(), observation);
}

/// Which backend identity a projection data-movement event should record
/// (Codex P2 on PR #77 / seed audio-graph-72d5).
///
/// Under ADR-0038 a job reaches exactly ONE authorized route, so this no longer
/// exists to cover a mid-job provider hop. It still exists because the *served*
/// identity can be sharper than the configured intent: `route.openrouter`
/// sharpens into `route.cerebras_via_openrouter` once the live routing policy is
/// read, and the served model can differ from the requested slug.
enum ProjectionLedgerBackend<'a> {
    /// Pre-call lifecycle marker: the configured provider intent (nothing has been
    /// dispatched yet; the terminal event is authoritative for what left).
    ///
    /// Decision (audio-graph-862c, revised): `Configured` now stamps
    /// `resolve_route(&dispatch.llm_provider).provider_id` — the SAME registry
    /// resolver `authorize_route_dispatch` uses — rather than the coarse
    /// `LlmProvider::runtime_provider_id()` settings-variant tag. Both reads
    /// are of the session-start snapshot ONLY (no live client, no dispatch),
    /// so this is a granularity fix, not a live-vs-snapshot one: for
    /// `Api { endpoint: CEREBRAS_BASE_URL, .. }`, `resolve_route` sharpens
    /// `llm.api` to `llm.cerebras` from the snapshot alone. Leaving it on the
    /// coarse tag made a producer inventory list `llm.api` for a session
    /// whose content only ever egressed to `llm.cerebras` — the harm 862c
    /// names, still open even after `Actual`/`FailedRoute` were sharpened,
    /// because `isContentEgress` (sessionDataRoute.ts) accepts the `started`
    /// (Configured) row on its own and `buildSessionDataRouteReport` keys
    /// transfers by provider id, so the coarse tag rendered as a SECOND
    /// producer row next to the sharp terminal one. `Actual` and
    /// `FailedRoute` still stamp the sharp registry id from the route
    /// actually reached (a live resolution, sharper still when it can be —
    /// e.g. `route.openrouter` → `route.cerebras_via_openrouter`); this
    /// closes the remaining asymmetry rather than widening it, since
    /// `resolve_route` never reports a route the eventual dispatch is not
    /// itself authorized to reach.
    Configured,
    /// Terminal success: the identity reported by the patch provenance (the
    /// registry provider id of the route that actually served the call —
    /// already sharp before audio-graph-862c, since provenance is stamped
    /// from the `AuthorizedRoute`/route table), paired with the SAME
    /// attempted-route identity `FailedRoute` uses below.
    ///
    /// Decision (audio-graph-7da4): carrying it here fixes the OTHER stale
    /// fallback the decision memo names — `actual_backend_identity`'s generic
    /// `llm.api` arm used to read `dispatch.llm_provider.requires_cloud_content_transfer()`
    /// for cloud-ness, which is the session-start snapshot, not the endpoint
    /// this SUCCESSFUL call actually reached. A same-descriptor mid-session
    /// repoint (loopback to any generic cloud `Api` endpoint) could therefore
    /// under- or over-report cloud-ness even on the success path.
    Actual(
        &'a crate::projections::ProjectionProvenance,
        Option<crate::llm::route::AttemptedRouteIdentity>,
    ),
    /// Terminal failure: the identity of the route this attempt actually
    /// reached, captured live at dispatch time (`AttemptedRouteIdentity`,
    /// `llm/route.rs`) — the SAME identity the success path above records via
    /// provenance. `None` only when nothing was dialled (client never
    /// configured, or a cross-provider repoint refused before the wire), in
    /// which case the snapshot-derived fallback below is honest, not stale.
    ///
    /// Decisions (audio-graph-862c, audio-graph-7da4 — Option 1 for both):
    /// before this it was always `(LlmProvider::runtime_provider_id(),
    /// LlmProvider::requires_cloud_content_transfer())`, i.e. entirely the
    /// session-start snapshot. 862c fixed `provider_id` (a failed Cerebras
    /// dispatch was inventoried under a producer that was never dialled);
    /// 7da4 fixes `requires_cloud_transfer` in this same change — a
    /// mid-session repoint from loopback to a cloud `Api` endpoint whose
    /// dispatch then fails must ledger remote egress, not the stale loopback
    /// boolean the snapshot still carries.
    FailedRoute(Option<crate::llm::route::AttemptedRouteIdentity>),
}

/// Map the provenance-reported provider identity to ledger identity:
/// (provider_id, requires_cloud_transfer, has_cached_prefix).
///
/// Post-ADR-0038 provenance already carries the registry id, so the `llm.*` arm is
/// a pass-through. The four legacy arms are REQUIRED, not vestigial: records
/// written by builds before this contract carry the old ad-hoc keys (`"openrouter"`
/// / `"api"` / `"local_llama"` / `"mistralrs"`), and a privacy report reading them
/// must still resolve an identity rather than silently mislabel one.
///
/// `attempted_route` is the SAME live-resolved identity `FailedRoute` reads
/// (`Some` on every call reached from a completed dispatch); it replaces the
/// stale `dispatch.llm_provider.requires_cloud_content_transfer()` fallback in
/// the generic `llm.api` arm, which read the session-start snapshot instead of
/// the endpoint actually dialled (audio-graph-7da4).
fn actual_backend_identity(
    dispatch: &ProjectionDispatchContext,
    backend: &str,
    attempted_route: Option<crate::llm::route::AttemptedRouteIdentity>,
) -> (String, bool, bool) {
    match backend {
        // Pass-through for the post-contract registry id.
        "llm.openrouter" | "openrouter" => ("llm.openrouter".to_string(), true, true),
        "llm.local_llama" | "local_llama" => ("llm.local_llama".to_string(), false, false),
        "llm.mistralrs" | "mistralrs" => ("llm.mistralrs".to_string(), false, false),
        "llm.aws_bedrock" => ("llm.aws_bedrock".to_string(), true, false),
        "llm.api" | "llm.cerebras" | "llm.sambanova" | "api" => {
            // A generic OpenAI-compatible endpoint may be loopback (local) or
            // remote, so the LIVE route's own endpoint check answers precisely.
            // There is no longer a fallback hop onto this backend, so the former
            // blanket "record it as remote" conservatism no longer applies to
            // it; a pinned cloud accelerator (`llm.cerebras` / `llm.sambanova`)
            // is remote by construction.
            let cloud = match backend {
                "llm.cerebras" | "llm.sambanova" => true,
                _ => attempted_route
                    .map(|route| route.requires_cloud_transfer)
                    .unwrap_or_else(|| match &dispatch.llm_provider {
                        LlmProvider::Api { .. } => {
                            dispatch.llm_provider.requires_cloud_content_transfer()
                        }
                        _ => true,
                    }),
            };
            let provider_id = if backend.starts_with("llm.") {
                backend.to_string()
            } else {
                "llm.api".to_string()
            };
            (provider_id, cloud, false)
        }
        other => (
            format!("llm.{other}"),
            dispatch.llm_provider.requires_cloud_content_transfer(),
            false,
        ),
    }
}

/// Build the content-free facts describing this projection submission for the
/// data-movement ledger (ADR-0025 §2g / seed audio-graph-72d5).
///
/// `notes` is the SAME live Notes-kind snapshot (seed audio-graph-253c part 2)
/// this job's actual prompt was (or will be) built with — passing it through
/// `projection_prompt_shape_with_notes` instead of the notes-blind
/// `projection_prompt_shape` is what lets `notes_snapshot_chars` /
/// `notes_snapshot_entries` below ever report non-zero, so the notes content
/// this call moves off-device is ledgered, not silently omitted from the
/// privacy report. Content-free like every other field here: only a char
/// count and an entry count ever leave this function.
// 9 params since the notes-snapshot wiring (seed audio-graph-253c part 2,
// extended by audio-graph-a6b5 W2's no-op-filter count) pushed this one over
// clippy's default threshold of 7.
#[allow(clippy::too_many_arguments)]
fn projection_movement_facts(
    dispatch: &ProjectionDispatchContext,
    job: &ProjectionJob,
    sequence: u64,
    ledger: &crate::projections::TranscriptLedger,
    notes: Option<&MaterializedNotes>,
    tokens_in: u64,
    tokens_out: u64,
    // audio-graph-a6b5 W2: 0 at the pre-generation `Started` call site (no
    // patch exists yet) and at a `FailedRoute` call site (no patch was
    // built); the real count from `ProjectionPatchOutcome::no_op_filtered_count`
    // only at the post-generation `Actual` (success) call site.
    no_op_filtered_count: u32,
    backend: ProjectionLedgerBackend<'_>,
) -> crate::projection_data_movement::ProjectionMovementFacts {
    let shape = crate::projection_llm::projection_prompt_shape_with_notes(job, ledger, notes);
    let (provider_id, requires_cloud_transfer, has_cached_prefix, model_id) = match backend {
        ProjectionLedgerBackend::Actual(provenance, attempted_route) => {
            let (provider_id, cloud, prefix) =
                actual_backend_identity(dispatch, &provenance.provider, attempted_route);
            (provider_id, cloud, prefix, provenance.model.clone())
        }
        // `resolve_route` is the same registry resolver `authorize_route_dispatch`
        // gates on, applied to the session-start snapshot alone — sharper than
        // `runtime_provider_id()` (llm.api -> llm.cerebras for a Cerebras-shaped
        // `Api` endpoint) without needing a live client (audio-graph-862c,
        // revised: see the `Configured` doc comment above).
        ProjectionLedgerBackend::Configured => (
            crate::llm::route::resolve_route(&dispatch.llm_provider)
                .provider_id
                .to_string(),
            dispatch.llm_provider.requires_cloud_content_transfer(),
            matches!(dispatch.llm_provider, LlmProvider::OpenRouter { .. }),
            String::new(),
        ),
        // Stamp BOTH fields from the route actually attempted
        // (`AttemptedRouteIdentity`, captured live at dispatch time — see the
        // `FailedRoute` doc comment above), not the configured snapshot: a
        // failed Cerebras dispatch must ledger `llm.cerebras` (audio-graph-862c),
        // and a failed dispatch reached after a mid-session loopback-to-cloud
        // repoint must ledger remote egress, not the stale loopback boolean the
        // snapshot still carries (audio-graph-7da4). `None` means nothing was
        // dialled (never-configured client, or a pre-wire authorization
        // refusal), so the snapshot-derived values are the honest answer there,
        // not a stale fallback.
        ProjectionLedgerBackend::FailedRoute(attempted_route) => match attempted_route {
            Some(route) => (
                route.provider_id.to_string(),
                route.requires_cloud_transfer,
                false,
                String::new(),
            ),
            None => (
                dispatch.llm_provider.runtime_provider_id().to_string(),
                dispatch.llm_provider.requires_cloud_content_transfer(),
                false,
                String::new(),
            ),
        },
    };
    crate::projection_data_movement::ProjectionMovementFacts {
        session_id: job.session_id.clone(),
        provider_id,
        model_id,
        requires_cloud_transfer,
        cloud_transfer_allowed: dispatch.llm_allow_cloud_fallbacks,
        projection_sequence: sequence,
        has_rolling_summary: shape.has_rolling_summary,
        has_cached_prefix,
        pinned_fact_chars: shape.pinned_fact_chars,
        notes_snapshot_chars: shape.notes_snapshot_chars,
        notes_snapshot_entries: shape.notes_snapshot_entries,
        no_op_filtered_count,
        tokens_in,
        tokens_out,
    }
}

fn run_projection_job(dispatch: ProjectionDispatchContext, job: ProjectionJob) {
    // Self-deregister on every exit path (normal completion, the two
    // superseded-job early returns, or an unexpected panic) — see
    // `ProjectionJobRegistrationGuard`. Held for the whole function body so
    // there is exactly one removal site regardless of which branch below is
    // taken.
    let _registration_guard = ProjectionJobRegistrationGuard {
        registry: dispatch.projection_job_workers.clone(),
        kind: job.kind.clone(),
        job_id: job.id.clone(),
    };
    let sequence = dispatch
        .projection_runtime
        .next_projection_sequence(&job.kind);
    let created_at_ms = current_unix_millis();
    let ledger = dispatch.projection_runtime.transcript_ledger_snapshot();
    // Live Notes-kind notes snapshot (seed audio-graph-253c part 2), cloned
    // under `AppState`'s `materialized_projection_state` lock and released
    // immediately — same clone-and-release shape as `ledger` above, never
    // held across the `generate_projection_patch` dispatch below. Graph-kind
    // jobs never render the notes block, so they carry nothing (mirrors
    // `projection_patch_prompt_messages`'s own `job.kind == Notes` gate).
    // Session-pinned via `materialized_notes_snapshot_for_session`: `None`
    // here can also mean a rotation raced this job's spawn (see that
    // accessor's doc), not only "no notes yet".
    let notes_snapshot = match job.kind {
        ProjectionKind::Notes => dispatch
            .projection_runtime
            .materialized_notes_snapshot_for_session(&job.session_id),
        ProjectionKind::Graph => None,
    };
    let generation_started_ms = current_unix_millis();

    // Ledger the remote-LLM data flow before the call leaves the device
    // (ADR-0025 §2g / seed audio-graph-72d5). Gated inside the builder: a
    // local-only session records a local movement and writes no remote
    // summary/prefix. The started event records the configured intent; the
    // terminal event below records the backend that ACTUALLY served the call.
    {
        let facts = projection_movement_facts(
            &dispatch,
            &job,
            sequence,
            &ledger,
            notes_snapshot.as_ref(),
            0,
            0,
            0,
            ProjectionLedgerBackend::Configured,
        );
        dispatch.data_movement_sink.record(
            &job.session_id,
            &crate::projection_data_movement::build_started_event(&facts),
        );
    }

    let attempt = dispatch.patch_generator.generate_projection_patch(
        job.clone(),
        ledger.clone(),
        notes_snapshot.clone(),
        sequence,
        created_at_ms,
    );
    let attempted_route = attempt.attempted_route;
    match attempt.outcome {
        Ok(outcome) => {
            let generation_latency_ms = current_unix_millis().saturating_sub(generation_started_ms);
            // Terminal event ledgers the ACTUAL backend from the patch
            // provenance, not the configured provider — the executor may have
            // fallen back to a different (possibly remote) backend within this
            // job (Codex P2 on PR #77).
            let movement_facts = projection_movement_facts(
                &dispatch,
                &job,
                sequence,
                &ledger,
                notes_snapshot.as_ref(),
                u64::from(outcome.tokens_used),
                0,
                outcome.no_op_filtered_count,
                ProjectionLedgerBackend::Actual(&outcome.patch.provenance, attempted_route),
            );
            // Test-observability hook only (no-op in production, see the
            // trait doc comment): gives tests value-level visibility into
            // `no_op_filtered_count` before it is folded into (or, for this
            // field specifically, dropped by) `build_terminal_event` — that
            // event carries no `no_op_filtered_count` sink, so without this
            // call nothing outside a source-text inspection test could ever
            // catch `outcome.no_op_filtered_count` being replaced by a
            // hardcoded `0` here.
            dispatch
                .data_movement_sink
                .record_movement_facts(&movement_facts);
            dispatch.data_movement_sink.record(
                &job.session_id,
                &crate::projection_data_movement::build_terminal_event(&movement_facts, true, None),
            );
            if !record_projection_generation_result(
                &dispatch,
                &job,
                generation_latency_ms,
                outcome.tokens_used,
                true,
            ) {
                log::debug!(
                    "Discarding generated patch for superseded projection job_id={} session_id={}",
                    job.id,
                    job.session_id
                );
                finish_projection_scheduler_job(dispatch, &job, ProjectionJobCompletion::Completed);
                return;
            }
            let mut patch = outcome.patch;
            patch.queued_at_ms.get_or_insert(job.queued_at_ms);
            patch
                .generation_latency_ms
                .get_or_insert(generation_latency_ms);
            // `mut`: the apply-success arm below populates
            // `basis_currency_at_apply` on THIS clone only (ticket W3,
            // audio-graph-a6b5) — the `patch` value moved into
            // `apply_runtime_projection_patch` just below is what gets
            // persisted to the canonical projection event log, and it is
            // deliberately never touched, so the persisted log's serialized
            // bytes stay exactly as they were before this ticket (see
            // `ProjectionPatch::basis_currency_at_apply`'s doc comment).
            let mut emitted_patch = patch.clone();
            let apply_started_ms = current_unix_millis();
            // Check ownership, then release the scheduler before validation or
            // disk I/O. Historical Review is read-only and cannot reset live
            // schedulers; the only production reset is session rotation,
            // which changes the runtime session id and is rejected by the
            // materializer. Holding this lock through save would stall final
            // ASR ingestion on slow storage, while taking scheduler -> ledger
            // here would invert ASR's ledger -> scheduler lock order.
            let owns_job = {
                let schedulers = match dispatch.projection_schedulers.lock() {
                    Ok(schedulers) => schedulers,
                    Err(poisoned) => {
                        log::warn!("Projection scheduler lock poisoned before apply; recovering");
                        poisoned.into_inner()
                    }
                };
                schedulers.owns_in_flight(&job.kind, &job.id, &job.session_id)
            };
            if !owns_job {
                log::debug!(
                    "Discarding patch before apply for superseded projection job_id={} session_id={}",
                    job.id,
                    job.session_id
                );
                finish_projection_scheduler_job(dispatch, &job, ProjectionJobCompletion::Completed);
                return;
            }
            let apply_result = dispatch.projection_runtime.apply_runtime_projection_patch(
                &job.session_id,
                &job.basis,
                patch,
            );
            match apply_result {
                Ok(result) => {
                    record_projection_apply_result(
                        &dispatch,
                        &job,
                        current_unix_millis().saturating_sub(apply_started_ms),
                        true,
                    );
                    log::debug!(
                        "Projection job applied job_id={} kind={:?} outcome={:?}",
                        job.id,
                        job.kind,
                        result.outcome
                    );
                    // audio-graph-a6b5 W2: counts only (ADR-0025), never
                    // note/section content — makes the no-op filter (§1.5b)
                    // and the document-order outline replacing the old
                    // recency-sorted notes snapshot (§2.3) verifiable from
                    // logs without needing a log-capture harness (this repo
                    // has none — see the audio-graph-fa56 precedent for why
                    // source-text inspection is the mutation-proof for a
                    // logging-only change). `notes_outline_*` is 0 for a
                    // Graph-kind job (the outline is Notes-kind only).
                    log::debug!(
                        "Projection job movement counts job_id={} kind={:?} \
                         no_op_filtered={} notes_outline_chars={} notes_outline_entries={}",
                        job.id,
                        job.kind,
                        movement_facts.no_op_filtered_count,
                        movement_facts.notes_snapshot_chars,
                        movement_facts.notes_snapshot_entries
                    );
                    // audio-graph-caad / audio-graph-f3d4: the gate now applies a
                    // proven append-only tail instead of discarding it as stale —
                    // split that telemetry from the ordinary current-basis apply
                    // above so it stays distinguishable in logs.
                    if let AppliedBasisCurrency::AppendedTail { ref staleness } =
                        result.basis_currency_at_apply
                    {
                        log::info!(
                            "Projection job applied append-only tail job_id={} kind={:?} staleness={:?}",
                            job.id,
                            job.kind,
                            staleness
                        );
                    }
                    // Ticket W3 (audio-graph-a6b5): populate the additive,
                    // event-payload-only field from the classification the
                    // apply gate already computed and returned above —
                    // never re-derive it, and never write it onto the
                    // `patch` value the apply call already persisted.
                    emitted_patch.basis_currency_at_apply =
                        Some(result.basis_currency_at_apply.clone());
                    emit_projection_runtime_events(&dispatch, &emitted_patch);
                    finish_projection_scheduler_job(
                        dispatch,
                        &job,
                        ProjectionJobCompletion::Completed,
                    );
                }
                Err(error) => {
                    record_projection_apply_result(
                        &dispatch,
                        &job,
                        current_unix_millis().saturating_sub(apply_started_ms),
                        false,
                    );
                    // A proven append-only tail now applies above instead of
                    // reaching this branch, so `StaleBasis` here is
                    // Revised-only by construction: content this apply
                    // covered was actually superseded, not merely trailed by
                    // a later append.
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
                        &job,
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
            let movement_facts = projection_movement_facts(
                &dispatch,
                &job,
                sequence,
                &ledger,
                notes_snapshot.as_ref(),
                0,
                0,
                0,
                ProjectionLedgerBackend::FailedRoute(attempted_route),
            );
            dispatch.data_movement_sink.record(
                &job.session_id,
                &crate::projection_data_movement::build_terminal_event(
                    &movement_facts,
                    false,
                    Some("projection_generation_failed"),
                ),
            );
            record_projection_generation_result(
                &dispatch,
                &job,
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
            finish_projection_scheduler_job(dispatch, &job, ProjectionJobCompletion::Failed);
        }
    }
}

fn record_projection_generation_result(
    dispatch: &ProjectionDispatchContext,
    job: &ProjectionJob,
    latency_ms: u64,
    tokens_used: u32,
    success: bool,
) -> bool {
    let mut schedulers = match dispatch.projection_schedulers.lock() {
        Ok(schedulers) => schedulers,
        Err(poisoned) => {
            log::warn!(
                "Projection scheduler lock poisoned during generation telemetry; recovering"
            );
            poisoned.into_inner()
        }
    };
    let owned = schedulers.record_generation_result_for_job(
        &job.kind,
        &job.id,
        &job.session_id,
        latency_ms,
        tokens_used,
        success,
    );
    if !owned {
        log::debug!(
            "Ignoring generation telemetry for superseded projection job_id={} session_id={}",
            job.id,
            job.session_id
        );
    }
    owned
}

fn record_projection_apply_result(
    dispatch: &ProjectionDispatchContext,
    job: &ProjectionJob,
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
    if !schedulers.record_apply_result_for_job(
        &job.kind,
        &job.id,
        &job.session_id,
        latency_ms,
        accepted,
    ) {
        log::debug!(
            "Ignoring apply telemetry for superseded projection job_id={} session_id={}",
            job.id,
            job.session_id
        );
    }
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
    job: &ProjectionJob,
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
        match (&job.kind, completion) {
            (ProjectionKind::Notes, ProjectionJobCompletion::Completed) => {
                schedulers.complete_notes_in_flight(&job.id, &job.session_id, &ledger, now_ms)
            }
            (ProjectionKind::Graph, ProjectionJobCompletion::Completed) => {
                schedulers.complete_graph_in_flight(&job.id, &job.session_id, &ledger, now_ms)
            }
            (ProjectionKind::Notes, ProjectionJobCompletion::Failed) => {
                schedulers.fail_notes_in_flight(&job.id, &job.session_id, &ledger, now_ms)
            }
            (ProjectionKind::Graph, ProjectionJobCompletion::Failed) => {
                schedulers.fail_graph_in_flight(&job.id, &job.session_id, &ledger, now_ms)
            }
        }
    };
    log::debug!(
        "Projection scheduler completion job_id={} session_id={} kind={:?} completion={:?} decision={:?}",
        job.id,
        job.session_id,
        job.kind,
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

/// Append `segment` to the display transcript's writer slot if one is
/// present; otherwise count the miss in `misses` (audio-graph-64e3). Shared
/// by every call site that persists a final to the display transcript
/// (currently `emit_transcript_and_extract_with_meta`'s streaming tail and
/// `run_speech_processor_diarization_only`'s local-diarization path) so the
/// miss-counting logic can't independently drift out of sync between them.
///
/// Returns `true` iff the segment was actually persisted.
///
/// What actually produces a miss here: `writer` is a `std::sync::Mutex`, so
/// `rotate_session` (state.rs) holding its lock for the whole writer swap
/// makes a concurrent caller BLOCK on `.lock()`, not observe `None` -- once
/// unblocked it sees `Some(new writer)` and (if this really is a stale final
/// racing a rotation) would misattribute the write into the NEW session's
/// file, not miss it -- so a rotation race is NOT what this counter catches.
/// A `None` slot with the OLD session id still published is produced by
/// `lib.rs`'s `graceful_shutdown` (`take_writer` clears the slot without
/// ever joining the speech-processor/receiver threads first) -- i.e. a quit
/// while transcribing. That path is not covered by the deepgram receiver
/// join, and its miss is never surfaced because
/// `warn_if_display_transcript_rows_missing_at_stop` is only called from
/// `stop_capture_impl`/`stop_transcribe`, neither of which `graceful_shutdown`
/// invokes.
///
/// This is a write-ATTEMPT miss counter, not a ledger-finals-vs-display-rows
/// parity check: a final that lands under the WRONG session's writer (writer
/// present, just the wrong session's) is counted as persisted here, not as a
/// miss — see `AppState::display_transcript_write_misses`'s doc for that
/// scoping.
fn persist_display_transcript_segment(
    writer: &Arc<Mutex<Option<crate::persistence::TranscriptWriter>>>,
    misses: &Arc<AtomicU64>,
    segment: &TranscriptSegment,
) -> bool {
    let mut persisted = false;
    if let Ok(writer_guard) = writer.lock()
        && let Some(ref w) = *writer_guard
    {
        w.append(segment);
        persisted = true;
    }
    if !persisted {
        misses.fetch_add(1, Ordering::Relaxed);
    }
    persisted
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
    //
    // A miss here is deliberately NOT logged per-event: the ledger write
    // below still succeeds for this same final (this function only reaches
    // here once the caller's ledger accept already returned true), so a
    // per-segment WARN would fire on every miss and could drown the signal.
    // Instead count it — see `persist_display_transcript_segment`'s doc for
    // what actually produces a miss here and what this counter does and does
    // not cover.
    persist_display_transcript_segment(
        &ctx.transcript_writer,
        &ctx.display_transcript_write_misses,
        &segment,
    );

    // 3. Emit Tauri events.
    emit_asr_span_revision(&ctx.app_handle, asr_payload);
    let event_sink = TauriDiarizationEventSink {
        app_handle: &ctx.app_handle,
    };
    let diarization_session_id = ctx.projection_runtime.current_session_id();
    let diarization_dispatch = DiarizationDispatchContext {
        event_sink: &event_sink,
        speaker_timeline: &ctx.speaker_timeline,
        knowledge_graph: &ctx.knowledge_graph,
        graph_snapshot: &ctx.graph_snapshot,
        transcript_ledger: &ctx.transcript_ledger,
        session_id: &diarization_session_id,
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
        active_session_id_snapshot(&ctx.active_session_id),
        span_id,
        ctx.app_handle.clone(),
        ctx.pending_agent_proposals.clone(),
        ctx.active_session_id.clone(),
        ctx.transcript_ledger.clone(),
    );

    // 4. Update pipeline status counts.
    if let Ok(mut status) = ctx.pipeline_status.write() {
        status.asr = StageStatus::Running {
            processed_count: asr_count,
        };
        // audio-graph-586b: a `Degraded` diarization status (set once, at
        // worker startup, by `apply_diarization_degradation`) must persist
        // for the whole session — this per-segment count update must never
        // clobber it back to a healthy-looking `Running`.
        if !matches!(status.diarization, StageStatus::Degraded { .. }) {
            status.diarization = StageStatus::Running {
                processed_count: diarization_count,
            };
        }
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
    let diarization_session_id = ctx.projection_runtime.current_session_id();
    let diarization_dispatch = DiarizationDispatchContext {
        event_sink: &event_sink,
        speaker_timeline: &ctx.speaker_timeline,
        knowledge_graph: &ctx.knowledge_graph,
        graph_snapshot: &ctx.graph_snapshot,
        transcript_ledger: &ctx.transcript_ledger,
        session_id: &diarization_session_id,
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
    let expected_session_id = active_session_id_snapshot(&ctx.active_session_id);
    let deps = ExtractionDeps {
        active_session_id: &ctx.active_session_id,
        transcript_ledger: &ctx.transcript_ledger,
        expected_session_id: &expected_session_id,
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
    let active_session_id = deps.active_session_id.clone();
    let transcript_ledger = deps.transcript_ledger.clone();
    let expected_session_id = deps.expected_session_id.to_string();
    let llm_allow_cloud_fallbacks = deps.llm_allow_cloud_fallbacks;
    let extraction_count = extraction_count.clone();
    let graph_update_count = graph_update_count.clone();

    let run_extraction = move || {
        let mut local_extraction = extraction_count.load(Ordering::Relaxed);
        let mut local_graph = graph_update_count.load(Ordering::Relaxed);
        let owned_deps = ExtractionDeps {
            active_session_id: &active_session_id,
            transcript_ledger: &transcript_ledger,
            expected_session_id: &expected_session_id,
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
        let committed = process_extraction_and_emit(
            &text,
            &speaker,
            &context,
            &segment_id,
            timestamp,
            &owned_deps,
            &mut local_extraction,
            &mut local_graph,
        );
        if committed {
            extraction_count.store(local_extraction, Ordering::Relaxed);
            graph_update_count.store(local_graph, Ordering::Relaxed);
        }
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
// Provider settings → worker-config mapping
// ---------------------------------------------------------------------------

/// Map the persisted [`AsrProvider::DeepgramStreaming`] settings into the
/// [`crate::asr::deepgram::DeepgramConfig`] handed to the streaming worker.
///
/// This is THE settings→wire boundary for Deepgram: the config produced here
/// feeds `deepgram_listen_url`, so every rule below directly shapes the
/// connection query string. Extracted from `run_speech_processor` so the
/// mapping is unit-testable without spawning worker threads (the historical
/// `model="general"` drift bug lived exactly at this kind of boundary).
///
/// Mapping rules (settings use `0` as the "not configured" sentinel; the
/// client config uses `Option` so unset params are omitted from the URL):
/// - `endpointing_ms` / `utterance_end_ms` / `eot_timeout_ms`: `0` → `None`
///   (leave Deepgram's server default), positive → `Some(value)`.
/// - `eot_threshold`: `0.0` → `None`, positive → `Some(value)`.
/// - `eager_eot_threshold`: forwarded ONLY when `0 < eager <= eot` — Deepgram
///   requires the eager threshold to be at most the main threshold; anything
///   else is dropped rather than sent as an invalid pair.
/// - `model`, `api_key`, `enable_diarization`, `vad_events`: verbatim.
///   (Model validity/aliasing is enforced downstream by
///   `sanitize_deepgram_model` at URL-build time and upstream by
///   `settings::migrate_asr_provider_model` at load time.)
/// - `keyterms`: verbatim (audio-graph-6470). Static, connection-time-only
///   glossary; `deepgram_listen_url` decides per-branch whether/how to send
///   them (v1 only — see that function's Flux-branch comment) and owns their
///   percent-encoding.
///
/// Returns `None` for non-Deepgram variants.
fn deepgram_config_from_settings(
    asr_provider: &AsrProvider,
    content_egress_policy: crate::asr::ProviderContentEgressPolicy,
) -> Option<crate::asr::deepgram::DeepgramConfig> {
    let AsrProvider::DeepgramStreaming {
        api_key,
        model,
        enable_diarization,
        endpointing_ms,
        utterance_end_ms,
        vad_events,
        eot_threshold,
        eager_eot_threshold,
        eot_timeout_ms,
        max_speakers: _,
        keyterms,
    } = asr_provider
    else {
        return None;
    };

    let (endpointing_ms, utterance_end_ms, vad_events) =
        (*endpointing_ms, *utterance_end_ms, *vad_events);
    let (eot_threshold, eager_eot_threshold, eot_timeout_ms) =
        (*eot_threshold, *eager_eot_threshold, *eot_timeout_ms);

    let effective_eager_eot = (eager_eot_threshold > 0.0 && eager_eot_threshold <= eot_threshold)
        .then_some(eager_eot_threshold);
    Some(crate::asr::deepgram::DeepgramConfig {
        api_key: api_key.clone(),
        model: model.clone(),
        enable_diarization: *enable_diarization,
        endpointing_ms: (endpointing_ms > 0).then_some(endpointing_ms),
        utterance_end_ms: (utterance_end_ms > 0).then_some(utterance_end_ms),
        vad_events,
        eot_threshold: (eot_threshold > 0.0).then_some(eot_threshold),
        eager_eot_threshold: effective_eager_eot,
        eot_timeout_ms: (eot_timeout_ms > 0).then_some(eot_timeout_ms),
        keyterms: keyterms.clone(),
        content_egress_policy,
    })
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
        ref model,
        max_speakers,
        ..
    } = asr_provider
    {
        log::info!(
            "Speech processor: ASR provider is Deepgram streaming (model={}) — \
             launching Deepgram streaming worker.",
            model
        );
        let deepgram_config =
            deepgram_config_from_settings(&asr_provider, config.provider_content_egress_policy)
                .expect("DeepgramStreaming variant checked by the enclosing if-let");
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

    let (diarization_config, diarization_degradation) =
        make_diarization_config(&config.models_dir, config.diarization_mode);
    apply_diarization_degradation(
        &shared.app_handle,
        &shared.pipeline_status,
        diarization_degradation,
    );

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
        shared.transcript_ledger.clone(),
        shared.projection_runtime.current_session_id(),
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
        projection_job_workers: shared.projection_job_workers.clone(),
        projection_lane_stopping: shared.projection_lane_stopping.clone(),
        event_sink: Arc::new(TauriProjectionRuntimeEventSink {
            app_handle: shared.app_handle.clone(),
        }),
        patch_generator: Arc::new(ExecutorProjectionPatchGenerator {
            llm_executor: shared.llm_executor.clone(),
            llm_provider: config.llm_provider.clone(),
        }),
        llm_provider: config.llm_provider.clone(),
        llm_allow_cloud_fallbacks: config.llm_allow_cloud_fallbacks,
        data_movement_sink: Arc::new(FileProjectionDataMovementSink),
    };

    // Register the AppHandle with the persistence module (see note in
    // `run_speech_processor`). Diarization-only may be entered directly when
    // Whisper model load fails, so we register here too.
    crate::persistence::register_app_handle(shared.app_handle.clone());

    // Auto-detect Sortformer / clustering models; falls back to Simple if none.
    let (diarization_config, diarization_degradation) =
        make_diarization_config(&config.models_dir, config.diarization_mode);
    apply_diarization_degradation(
        &shared.app_handle,
        &shared.pipeline_status,
        diarization_degradation,
    );

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
        shared.transcript_ledger.clone(),
        shared.projection_runtime.current_session_id(),
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
        // Persist transcript segment asynchronously. This is a second,
        // independent display-write site (local diarization-in-place ASR),
        // not routed through `emit_transcript_and_extract_with_meta`'s shared
        // tail, so it shares `persist_display_transcript_segment` rather than
        // duplicating the miss-counting logic (audio-graph-64e3).
        persist_display_transcript_segment(
            &shared.transcript_writer,
            &shared.display_transcript_write_misses,
            &final_segment,
        );

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
        let diarization_session_id = shared.projection_runtime.current_session_id();
        let diarization_dispatch = DiarizationDispatchContext {
            event_sink: &event_sink,
            speaker_timeline: &shared.speaker_timeline,
            knowledge_graph: &shared.knowledge_graph,
            graph_snapshot: &shared.graph_snapshot,
            transcript_ledger: &shared.transcript_ledger,
            session_id: &diarization_session_id,
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
            active_session_id_snapshot(&shared.active_session_id),
            final_segment.id.clone(),
            shared.app_handle.clone(),
            shared.pending_agent_proposals.clone(),
            shared.active_session_id.clone(),
            shared.transcript_ledger.clone(),
        );

        if let Ok(mut status) = shared.pipeline_status.write() {
            // audio-graph-586b: see the identical guard's comment in
            // `emit_transcript_and_extract_with_meta` — a `Degraded` set at
            // worker startup must survive every per-segment count update.
            if !matches!(status.diarization, StageStatus::Degraded { .. }) {
                status.diarization = StageStatus::Running {
                    processed_count: count,
                };
            }
        }

        // Knowledge Graph Extraction — fire-and-forget
        let expected_session_id = active_session_id_snapshot(&shared.active_session_id);
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
                active_session_id: &shared.active_session_id,
                transcript_ledger: &shared.transcript_ledger,
                expected_session_id: &expected_session_id,
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
    let (diarization_config, diarization_degradation) =
        make_diarization_config(&config.models_dir, config.diarization_mode);
    apply_diarization_degradation(
        &shared.app_handle,
        &shared.pipeline_status,
        diarization_degradation,
    );
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

    // audio-graph-586b review follow-up: captured before `deepgram_config` is
    // moved into the client below. This is the flag that actually gates
    // `diarize=true` on the socket (asr/deepgram.rs) — when it's set, Deepgram
    // labels segments with provider-native speaker ids and the event
    // receiver's own per-segment branch (`speaker_label.is_some()`) skips the
    // local diarization worker entirely for those segments. Reporting a
    // local-engine "basic mode" degradation in that case would be false —
    // see `run_deepgram_event_receiver`'s use of this flag.
    let provider_native_diarization_active = deepgram_config.enable_diarization;

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
    // It outlives `is_transcribing` flipping false by design -- see
    // `run_deepgram_event_receiver`'s doc comment (audio-graph-653a). Its
    // handle is joined (bounded) below, AFTER the sender loop exits and the
    // client disconnects -- see that join's doc comment (audio-graph-64e3)
    // for why a detached, never-joined handle here let tail-of-session
    // finals reach the speaker ledger but silently miss the display
    // transcript.
    let pipeline_status_for_status_update = shared.pipeline_status.clone();
    let receiver_handle = match std::thread::Builder::new()
        .name("deepgram-event-rx".to_string())
        .spawn({
            let shared_for_receiver = shared.clone();
            let config_for_receiver = config.clone();
            let source_id_hint_for_receiver = Arc::clone(&source_id_hint);

            move || {
                run_deepgram_event_receiver(
                    event_rx,
                    shared_for_receiver,
                    config_for_receiver,
                    source_id_hint_for_receiver,
                    max_speakers,
                    provider_native_diarization_active,
                );
            }
        }) {
        Ok(handle) => Some(handle),
        Err(e) => {
            log::error!("Deepgram streaming: failed to spawn event receiver thread: {e}");
            None
        }
    };

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

    // audio-graph-64e3: drop `client` explicitly (rather than waiting for
    // this function's own end-of-scope drop) so the tokio runtime backing
    // it shuts down NOW and every clone of `event_tx` it held goes with it.
    // `run_deepgram_event_receiver`'s loop only breaks out of its
    // `Disconnected` arm once `event_rx.recv_timeout` observes every sender
    // gone -- dropping `client` here, before the join below, is what makes
    // that observation happen instead of hanging until this whole function
    // returns (which used to happen at the SAME point, but after the
    // receiver was already detached and un-joined).
    drop(client);

    // Bounded join on the receiver thread. `disconnect()` above already
    // blocked until Deepgram's close-drain finished, so every tail-of-
    // utterance final is already sitting in `event_rx`'s buffer; the
    // receiver only has to drain it (fast: extraction submission is
    // fire-and-forget, see `coalesce_submit`/`spawn_extraction_task`) and
    // run its own session-end interim-promotion pass. Without this join,
    // `stop_capture_impl` could finish tearing down and release
    // `session_lifecycle` WHILE the receiver was still mid-drain; a
    // `new_session_cmd` racing in right after Stop would then rotate
    // `transcript_writer` (audio-graph-64e3 field evidence: 6 finals landed
    // in `.speaker.jsonl` but never reached the display transcript, no WARN
    // anywhere). Bounded rather than unbounded so Stop stays fast even in
    // the pathological case: on timeout, `join_worker_with_bounded_wait`
    // itself pushes the handle into `shared.retired_session_workers` --
    // NOT a claim that this function's OWN join by `stop_capture_impl` will
    // do it transitively. That distinction matters: the sender loop above
    // typically observes the cleared `is_transcribing` flag and returns from
    // `disconnect()`/`drop(client)` fast enough that THIS function's thread
    // joins well inside `stop_capture_impl`'s 3s bound even when the
    // receiver itself is still draining, so relying on the outer join alone
    // would leave the receiver fully detached with no fencing at all for
    // exactly the pathological case this bound exists for.
    if let Some(handle) = receiver_handle {
        join_worker_with_bounded_wait(
            handle,
            DEEPGRAM_RECEIVER_DRAIN_TIMEOUT,
            "Deepgram event receiver",
            &shared.retired_session_workers,
        );
    }

    log::info!(
        "Deepgram streaming: audio sender exiting. Chunks sent={}",
        chunks_sent
    );
}

/// Wait up to `timeout` for `handle` to finish, polling every 20ms, then join
/// it if it did (propagating a panic only as a WARN log) or retain it in
/// `retired_workers` if it didn't (audio-graph-64e3) -- the SAME idiom
/// `join_worker_with_timeout` (commands.rs) uses for the sp/asr/projection-job
/// joins, so a straggler receiver fences a subsequent Start/New Session via
/// `ensure_session_workers_quiesced` exactly like a timed-out sp/asr worker,
/// instead of being left fully detached.
///
/// Returns `true` iff the thread finished (and was joined) within `timeout`.
/// Never blocks past `timeout` + one poll tick — pushing into
/// `retired_workers` on timeout (rather than blocking here) is what keeps
/// this bounded while still closing the race: the caller's own thread
/// returns immediately either way, but a subsequent session boundary command
/// now sees the still-running handle and refuses to proceed until it drains.
fn join_worker_with_bounded_wait(
    handle: std::thread::JoinHandle<()>,
    timeout: Duration,
    label: &str,
    retired_workers: &Arc<Mutex<Vec<std::thread::JoinHandle<()>>>>,
) -> bool {
    let deadline = Instant::now() + timeout;
    while !handle.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    if handle.is_finished() {
        if let Err(e) = handle.join() {
            log::warn!("{label} panicked during shutdown: {e:?}");
        }
        true
    } else {
        log::warn!(
            "{label} did not finish within {:?}; retaining handle in \
             retired_session_workers so it fences a subsequent session start/rotation \
             instead of racing it",
            timeout
        );
        let mut retired = retired_workers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        retired.push(handle);
        false
    }
}

/// Bound on how long `run_deepgram_speech_processor` waits, after
/// disconnecting the Deepgram client, for the event receiver thread to drain
/// its already-buffered tail events and exit (audio-graph-64e3).
///
/// Chosen because: by the time this wait starts, `disconnect()` has already
/// blocked (up to `DEEPGRAM_CLOSE_DRAIN_TIMEOUT` + 500ms, ~1.4s) until every
/// tail-of-utterance event is sitting in the channel buffer, so the receiver
/// only has to drain what's already there -- writing a handful of JSONL rows
/// and dispatching fire-and-forget extraction/projection work, not waiting on
/// any network or LLM round-trip. 2s is generous headroom over that (same
/// order of magnitude as the existing 3s sp/asr join bound in
/// `stop_capture_impl` and the 5s `TRANSCRIPT_WRITER_SHUTDOWN_TIMEOUT`) while
/// keeping Stop's added worst-case latency bounded.
///
/// Disclosed composed worst case (not accounted for by the paragraph above):
/// the explicit `drop(client)` immediately before this wait runs
/// `DeepgramStreamingClient`'s `Drop`, which calls
/// `rt.shutdown_timeout(Duration::from_secs(3))` -- so the sender-loop
/// thread's total worst case is disconnect's ~1.4s + up to 3s of runtime
/// shutdown + this 2s wait, ~6.4s, which EXCEEDS `stop_capture_impl`'s own
/// 3s join bound on that thread. When that happens `join_worker_with_timeout`
/// spills the sender-loop handle into `retired_session_workers` too (the
/// safe failure mode: `ensure_session_workers_quiesced` then rejects the next
/// Start/New Session with a retry-in-a-moment error instead of racing it) --
/// but that user-visible retry window is a real, newly-introduced consequence
/// of this bound, not merely a theoretical one.
///
/// A timeout on THIS wait specifically does not hang Stop: this function
/// pushes the receiver's handle into `shared.retired_session_workers` itself
/// (see `join_worker_with_bounded_wait`) and returns immediately, so a
/// subsequent Start/New Session is fenced on the receiver directly rather
/// than depending on the outer sp/asr join happening to also time out.
const DEEPGRAM_RECEIVER_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

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

/// A retained "highest-revision-seen" interim for a span that has not yet
/// received a final (audio-graph-e70e). `run_deepgram_event_receiver`'s
/// interim path never persists anything receiver-side — this struct is the
/// state that lets a span whose final never arrives get promoted into a
/// durable `TranscriptSegment` instead of silently vanishing (field
/// evidence: one session lost 14 spans / 96 words this way, accounting for
/// all 5 largest intra-transcript gaps).
///
/// Overwritten (not merged) on every subsequent interim for the same
/// `span_id`: Deepgram's streaming interims are cumulative/refined, so the
/// latest one for a span is always a superset of any earlier one. This also
/// resets `retained_at`, so a span that keeps receiving fresh interims never
/// goes stale for the age-based heartbeat trigger below -- only a span whose
/// updates (interim OR final) have genuinely stopped arriving does.
#[derive(Debug, Clone)]
struct PendingInterimSpan {
    source_id: String,
    text: String,
    start: f64,
    end_time: f64,
    confidence: f32,
    /// Per-word data from the highest-revision interim, kept so promotion
    /// can reuse `group_words_by_speaker` exactly like the real final path
    /// (audio-graph-4aed) instead of collapsing a multi-speaker span onto a
    /// single run.
    words: Vec<crate::asr::deepgram::DeepgramWord>,
    /// When this entry was inserted/overwritten (audio-graph-e70e review
    /// follow-up). Feeds the age-based heartbeat promotion trigger --
    /// `age_expired_pending_span_ids` -- which bounds how long a stalled
    /// span can sit unpromoted, independent of whether another final ever
    /// arrives to fire the past-range trigger.
    retained_at: std::time::Instant,
}

/// Margin added to a pending interim's `end_time` before a later final's
/// `start` (for a DIFFERENT span) is treated as proof the pending span will
/// never get its own final (audio-graph-e70e "past-the-range" promotion
/// trigger). Deepgram emits `Transcript` events in temporal order, so a
/// final's `start` strictly past another pending span's end is decisive on
/// its own; the margin only absorbs provider timestamp jitter between
/// events. It is not load-bearing for correctness — a late genuine final for
/// an already-promoted span still supersedes it via revision numbers (see
/// `promote_pending_interim`), it just wouldn't be as prompt without this
/// margin's cushion against jitter.
const PAST_RANGE_PROMOTION_MARGIN_SECS: f64 = 0.05;

/// Upper bound on how long a pending interim can sit with no update (no
/// fresher interim, no final) before the heartbeat tick promotes it anyway
/// (audio-graph-e70e review follow-up). Covers two gaps the past-range
/// trigger cannot, because that trigger only fires inside the `is_final`
/// branch: (1) a run of consecutive interim-only spans where no later final
/// ever arrives to prove any of them are "in the past", and (2) the tail
/// after the session's last final. It also bounds `pending_interims_by_span`
/// itself -- without this, a sustained finals outage grows the map (and the
/// eventual session-end promotion burst) for the rest of the session with no
/// cap, exactly the regime the ticket's field evidence describes.
///
/// Chosen relative to `COALESCE_MAX_AGE_MS` (3.5s, the extraction-batch age
/// bound below) with headroom so this trigger does not race normal
/// coalescing. An early promotion is safe even when a genuine final was
/// still coming: `promote_pending_interim`'s revision bookkeeping is
/// specifically designed so that late final correctly SUPERSEDES the
/// promoted entry (see its doc comment and
/// `deepgram_late_final_after_promotion_supersedes_not_duplicates`), so the
/// only cost of promoting slightly early is a redundant flat
/// `transcript_buffer` row -- already accepted by design, not a new
/// correctness gap.
const PENDING_INTERIM_MAX_AGE_SECS: f64 = 5.0;

/// Pending span_ids whose retained interim is now provably in the past
/// relative to `current_final_start` (the `start` of a final for some OTHER
/// span), sorted by `start` ascending for deterministic promotion order when
/// one final's arrival proves multiple pending spans stale at once (review
/// finding: `HashMap` iteration order is randomized per process; the
/// session-end flush already sorts for this same reason). Excludes
/// `current_span_id` itself — that one is cleared directly by the final path
/// via `pending_interims_by_span.remove` regardless of this heuristic.
fn past_range_pending_span_ids(
    pending_interims_by_span: &HashMap<String, PendingInterimSpan>,
    current_span_id: &str,
    current_final_start: f64,
) -> Vec<String> {
    let mut matches: Vec<(String, f64)> = pending_interims_by_span
        .iter()
        .filter(|(span_id, pending)| {
            span_id.as_str() != current_span_id
                && pending.end_time + PAST_RANGE_PROMOTION_MARGIN_SECS <= current_final_start
        })
        .map(|(span_id, pending)| (span_id.clone(), pending.start))
        .collect();
    matches.sort_by(|(_, a), (_, b)| a.total_cmp(b));
    matches.into_iter().map(|(span_id, _)| span_id).collect()
}

/// Pending span_ids that have gone `PENDING_INTERIM_MAX_AGE_SECS` without an
/// update (no fresher interim, no final), sorted by `start` ascending for
/// the same deterministic-ordering reason as `past_range_pending_span_ids`.
fn age_expired_pending_span_ids(
    pending_interims_by_span: &HashMap<String, PendingInterimSpan>,
    now: std::time::Instant,
) -> Vec<String> {
    let mut matches: Vec<(String, f64)> = pending_interims_by_span
        .iter()
        .filter(|(_, pending)| {
            now.saturating_duration_since(pending.retained_at)
                .as_secs_f64()
                >= PENDING_INTERIM_MAX_AGE_SECS
        })
        .map(|(span_id, pending)| (span_id.clone(), pending.start))
        .collect();
    matches.sort_by(|(_, a), (_, b)| a.total_cmp(b));
    matches.into_iter().map(|(span_id, _)| span_id).collect()
}

/// Promote a retained pending interim into one or more durable
/// `TranscriptSegment`s (audio-graph-e70e). Mirrors the real final path as
/// closely as possible so a promoted span behaves identically to a genuine
/// final for every downstream consumer (store dedup, projections, session
/// timeline):
///
/// - Splits on per-word speaker changes via `group_words_by_speaker`, exactly
///   like the multi-speaker final path (audio-graph-4aed) — a retained
///   interim that crosses a turn boundary is not collapsed onto its first
///   word's speaker.
/// - Remaps the raw `deepgram-{n}` speaker id through `remap_deepgram_speaker`
///   (consuming a `speaker_map` cap slot). Interims deliberately skip this
///   because they are provisional/revisable (audio-graph-4aed review), but a
///   promoted span IS the permanent record now, so it participates in the
///   same speaker cap as every other final. ADR-0017's `max_speakers` collapse
///   is already a first-seen-order heuristic even for on-time finals, so a
///   promoted span's slot possibly landing "out of order" relative to true
///   speech time (e.g. promoted at session end, well after later finals
///   already claimed slots) is the same class of approximation the cap
///   already accepts, not a new correctness hole. The converse direction
///   also holds and is more load-bearing: because both mid-session triggers
///   promote pending spans BEFORE the current final's own
///   `group_words_by_speaker`/`remap_deepgram_speaker` call runs, a
///   promotion can claim a cap slot that a not-yet-processed genuine final
///   would otherwise have claimed for itself, changing that final's speaker
///   label relative to a replay of the same event stream without this
///   feature. This is intentional, not a regression: every pending span
///   promoted this way is provably earlier in time than the triggering final
///   (`past_range_pending_span_ids`'s `end_time` check, or
///   `age_expired_pending_span_ids`'s staleness check), so remapping it
///   first keeps the cap's first-seen-order heuristic aligned with true
///   chronological speech order instead of mere final-arrival order. Pinned
///   by `deepgram_past_range_promotion_claims_speaker_slot_before_triggering_final`.
/// - Emits through `emit_transcript_and_extract_with_meta`, the same call the
///   final path uses, so persistence/events/extraction/projection dispatch
///   are byte-for-byte identical to a genuine final.
///
/// Revision bookkeeping for the FIRST (or only) run — the one keyed on
/// `span_id`, the same span_id any live interims used — deliberately uses
/// `next_span_revision` (increment, KEEP the map entry) instead of
/// `final_span_revision` (increment, REMOVE). If a late genuine final for
/// this span_id arrives after promotion, it calls `final_span_revision`
/// itself and computes a STRICTLY HIGHER revision number than the promoted
/// one (removing the entry at that point, same as any normal final) — so the
/// transcript ledger's `apply_event` (higher revision replaces the current
/// one) and the frontend's `winningAsrRevisionsBySpan`/`isStaleAsrRevision`
/// dedup (audio-graph-a35a) both let the true final correctly SUPERSEDE the
/// promoted row instead of duplicating it or being rejected as stale. Runs
/// after the first (multi-speaker split only) always use
/// `final_span_revision` — those span_ids are brand new and never had a live
/// interim to keep alive for, matching the real final path exactly.
#[allow(clippy::too_many_arguments)]
fn promote_pending_interim(
    span_id: String,
    pending: PendingInterimSpan,
    ctx: &TranscriptProcessingContext,
    revision_numbers_by_span: &mut HashMap<String, u64>,
    speaker_map: &mut HashMap<u32, u32>,
    last_speaker: &mut u32,
    max_speakers: u32,
    diarization_worker: &mut DiarizationWorker,
    asr_count: &mut u64,
    diarization_count: &mut u64,
    extraction_count: &Arc<AtomicU64>,
    graph_update_count: &Arc<AtomicU64>,
) {
    let PendingInterimSpan {
        source_id,
        text,
        start,
        end_time,
        confidence,
        words,
        retained_at: _,
    } = pending;

    log::info!(
        "Deepgram: promoting interim-only span (no final ever arrived) span_id={} text_len={} word_count={}",
        span_id,
        text.chars().count(),
        words.len(),
    );

    let runs = crate::asr::deepgram::group_words_by_speaker(&words);

    if runs.len() <= 1 {
        let (revision_number, supersedes) = next_span_revision(revision_numbers_by_span, &span_id);
        let speaker_from_deepgram = words.first().and_then(|w| w.speaker).map(|raw| {
            let id = remap_deepgram_speaker(raw, max_speakers, speaker_map, last_speaker);
            format!("Speaker {}", id)
        });

        *asr_count += 1;
        let segment = TranscriptSegment {
            id: uuid::Uuid::new_v4().to_string(),
            source_id: source_id.clone(),
            speaker_id: speaker_from_deepgram.clone(),
            speaker_label: speaker_from_deepgram,
            text,
            start_time: start,
            end_time,
            confidence,
        };

        let final_segment = if segment.speaker_label.is_some() {
            *diarization_count += 1;
            segment.clone()
        } else {
            let input = DiarizationInput {
                transcript: segment.clone(),
                speech_audio: vec![],
                speech_start_time: Duration::from_secs_f64(start),
                speech_end_time: Duration::from_secs_f64(end_time),
            };
            let diarized = diarization_worker.process_input(input);
            *diarization_count += 1;
            let _ = ctx
                .app_handle
                .emit(events::SPEAKER_DETECTED, &diarized.speaker_info);
            diarized.segment
        };

        emit_transcript_and_extract_with_meta(
            final_segment,
            None,
            ctx,
            *asr_count,
            *diarization_count,
            extraction_count,
            graph_update_count,
            AsrRevisionMeta {
                span_id: Some(span_id),
                revision_number: Some(revision_number),
                supersedes,
                raw_event_ref: Some("deepgram.results.interim-promoted".to_string()),
                ..AsrRevisionMeta::default()
            },
        );
        return;
    }

    // Multi-speaker retained interim: split into per-run segments exactly
    // like a multi-speaker final (audio-graph-4aed). Run 0 keeps the
    // pending's own `span_id` (matching the final path's convention so it
    // still closes out any live-interim revision history); the
    // `used_span_ids` collision guard mirrors the final path's, for the same
    // millisecond-quantization reason (see the final path's comment).
    let run_count = runs.len();
    let mut used_span_ids: std::collections::HashSet<String> =
        std::collections::HashSet::with_capacity(run_count);
    used_span_ids.insert(span_id.clone());
    for (run_index, (speaker, run_words)) in runs.into_iter().enumerate() {
        let is_first_run = run_index == 0;
        let is_last_run = run_index + 1 == run_count;
        let run_start = if is_first_run {
            start
        } else {
            run_words.first().map(|w| w.start).unwrap_or(start)
        };
        let run_end = if is_last_run {
            end_time
        } else {
            run_words.last().map(|w| w.end).unwrap_or(end_time)
        };
        let run_text = run_words
            .iter()
            .map(|w| w.display_word())
            .collect::<Vec<_>>()
            .join(" ");
        let run_span_id = if is_first_run {
            span_id.clone()
        } else {
            let mut candidate = provider_start_span_id("deepgram", &source_id, run_start);
            let mut ms_bump: i64 = 0;
            while used_span_ids.contains(&candidate) {
                ms_bump += 1;
                candidate = provider_start_span_id(
                    "deepgram",
                    &source_id,
                    run_start + (ms_bump as f64) / 1000.0,
                );
            }
            candidate
        };
        used_span_ids.insert(run_span_id.clone());

        // Run 0 keeps the promoted span alive (see doc comment above); later
        // runs are brand-new span_ids, closed exactly like the final path.
        let (run_revision_number, run_supersedes) = if is_first_run {
            next_span_revision(revision_numbers_by_span, &run_span_id)
        } else {
            final_span_revision(revision_numbers_by_span, &run_span_id)
        };

        let speaker_from_deepgram = speaker.map(|raw| {
            let id = remap_deepgram_speaker(raw, max_speakers, speaker_map, last_speaker);
            format!("Speaker {}", id)
        });

        *asr_count += 1;
        let run_segment = TranscriptSegment {
            id: uuid::Uuid::new_v4().to_string(),
            source_id: source_id.clone(),
            speaker_id: speaker_from_deepgram.clone(),
            speaker_label: speaker_from_deepgram,
            text: run_text,
            start_time: run_start,
            end_time: run_end,
            confidence,
        };

        let final_run_segment = if run_segment.speaker_label.is_some() {
            *diarization_count += 1;
            run_segment.clone()
        } else {
            let input = DiarizationInput {
                transcript: run_segment.clone(),
                speech_audio: vec![],
                speech_start_time: Duration::from_secs_f64(run_start),
                speech_end_time: Duration::from_secs_f64(run_end),
            };
            let diarized = diarization_worker.process_input(input);
            *diarization_count += 1;
            let _ = ctx
                .app_handle
                .emit(events::SPEAKER_DETECTED, &diarized.speaker_info);
            diarized.segment
        };

        emit_transcript_and_extract_with_meta(
            final_run_segment,
            None,
            ctx,
            *asr_count,
            *diarization_count,
            extraction_count,
            graph_update_count,
            AsrRevisionMeta {
                span_id: Some(run_span_id),
                revision_number: Some(run_revision_number),
                supersedes: run_supersedes,
                raw_event_ref: Some(format!("deepgram.results.interim-promoted:run{run_index}")),
                ..AsrRevisionMeta::default()
            },
        );
    }
}

/// Deepgram event receiver thread — processes transcript events from the
/// Deepgram WebSocket and feeds them into the diarization + storage + events
/// + extraction pipeline (same downstream path as cloud ASR).
///
/// Deliberately takes no `is_transcribing` flag: this thread's lifecycle is
/// tied solely to `event_rx` disconnecting, not to that flag, so it cannot
/// exit (or discard an already-dequeued event) before the Deepgram client's
/// close-drain has delivered the last utterance's finals. See the loop body
/// below for the full audio-graph-653a rationale.
///
/// `provider_native_diarization_active` (audio-graph-586b review follow-up):
/// true iff this session's Deepgram socket was opened with `diarize=true`
/// (`DeepgramConfig::enable_diarization`, captured by the caller before it
/// moved the config into the client). When true, Deepgram itself labels most
/// segments and the per-segment branch below (`speaker_label.is_some()`)
/// takes the provider-label path, never touching the local diarization
/// worker — so a "this build doesn't include the neural diarization engine"
/// banner would be false: no neural engine was needed. The local worker is
/// still constructed as a fallback for the rare segment Deepgram doesn't
/// label (e.g. a run with no speaker info at all), but that fallback firing
/// occasionally does not mean the *session* is running in degraded basic
/// mode, so no `Degraded` status is reported in that case. When false (mode
/// forced `enable_diarization` off, e.g. `DiarizationMode::Local`), every
/// segment falls through to the local worker and the ordinary
/// engine/asset-availability degradation reporting applies.
fn run_deepgram_event_receiver(
    event_rx: crossbeam_channel::Receiver<crate::asr::deepgram::DeepgramEvent>,
    shared: SpeechShared,
    config: SpeechConfig,
    source_id_hint: Arc<RwLock<Option<String>>>,
    max_speakers: u32,
    provider_native_diarization_active: bool,
) {
    use crate::asr::deepgram::{DeepgramEvent, DeepgramTurnKind};
    use crate::diarization::{DiarizationInput, DiarizationWorker, DiarizedTranscript};

    let (diarization_config, diarization_degradation) =
        make_diarization_config(&config.models_dir, config.diarization_mode);
    apply_diarization_degradation(
        &shared.app_handle,
        &shared.pipeline_status,
        diarization_degradation_for_provider_labeled_session(
            provider_native_diarization_active,
            diarization_degradation,
        ),
    );
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
    // audio-graph-e70e: highest-revision interim retained per span with no
    // final yet, so a span whose final never arrives can still be promoted
    // into a durable TranscriptSegment (see `promote_pending_interim`).
    let mut pending_interims_by_span: HashMap<String, PendingInterimSpan> = HashMap::new();
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
        // This loop's lifecycle is driven entirely by the event channel
        // actually disconnecting (below) -- NOT by `is_transcribing`.
        // `is_transcribing` clears on the stop path BEFORE the Deepgram
        // client's close-drain runs (audio-graph-653a): the audio-sender
        // loop only calls `client.disconnect()` after observing the cleared
        // flag, and `disconnect()` itself now blocks until the drain
        // finishes. Breaking here on the flag -- or discarding an
        // already-dequeued event because the flag had cleared -- would
        // throw away exactly the drained tail-of-utterance events the drain
        // exists to recover, even after the client-side fix lets them
        // arrive. The channel closes deterministically and boundedly once
        // the client that owns `event_tx` is torn down (`disconnect()`'s own
        // bounded wait, followed by the caller dropping the client), so
        // waiting for `RecvTimeoutError::Disconnected` cannot hang.
        let event = match event_rx.recv_timeout(Duration::from_millis(500)) {
            Ok(ev) => ev,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                // audio-graph-e70e review follow-up: promote any pending
                // interim that has gone `PENDING_INTERIM_MAX_AGE_SECS`
                // without an update, BEFORE the extraction-coalescing flush
                // below (mirrors the session-end ordering: a promotion's own
                // `emit_transcript_and_extract_with_meta` call enqueues a
                // coalesce_submit that this same tick's flush should be free
                // to consider). Covers mid-session gaps the past-range
                // trigger cannot (a run of consecutive interim-only spans,
                // or the tail after the session's last final) and bounds
                // `pending_interims_by_span`'s growth -- see
                // `PENDING_INTERIM_MAX_AGE_SECS`'s doc comment.
                for stale_span_id in
                    age_expired_pending_span_ids(&pending_interims_by_span, Instant::now())
                {
                    if let Some(stale_pending) = pending_interims_by_span.remove(&stale_span_id) {
                        promote_pending_interim(
                            stale_span_id,
                            stale_pending,
                            &ctx,
                            &mut revision_numbers_by_span,
                            &mut speaker_map,
                            &mut last_speaker,
                            max_speakers,
                            &mut diarization_worker,
                            &mut asr_count,
                            &mut diarization_count,
                            &extraction_count,
                            &graph_update_count,
                        );
                    }
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
                    // Provider-raw speaker id only (`deepgram-{n}`, NOT the
                    // remapped "Speaker N" display label) — audio-graph-4aed
                    // review: interims are provisional/revisable, so this
                    // deliberately does not consume a `speaker_map` remap
                    // slot the way the final path's `remap_deepgram_speaker`
                    // does; it exists purely so the persisted revision ledger
                    // carries SOME speaker evidence for interims instead of
                    // always `None` (previously true for every interim).
                    let interim_speaker_id = words
                        .first()
                        .and_then(|w| w.speaker)
                        .map(|raw| format!("deepgram-{raw}"));
                    log::debug!(
                        "Deepgram: interim transcript metadata provider=deepgram span_id={} revision={} text_len={} confidence={:.3} speaker_present={}",
                        span_id,
                        revision_number,
                        text.chars().count(),
                        confidence,
                        interim_speaker_id.is_some()
                    );
                    // audio-graph-e70e: retain this interim (overwriting any
                    // earlier one for the same span_id) so the span can be
                    // promoted into a durable TranscriptSegment if its final
                    // never arrives — see `promote_pending_interim` and the
                    // Disconnected exit arm below. Cloned rather than moved:
                    // `text`/`source_id`/`words` are still needed below to
                    // emit the live partial exactly as before.
                    pending_interims_by_span.insert(
                        span_id.clone(),
                        PendingInterimSpan {
                            source_id: source_id.clone(),
                            text: text.clone(),
                            start,
                            end_time,
                            confidence,
                            words: words.clone(),
                            retained_at: std::time::Instant::now(),
                        },
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
                            speaker_id: interim_speaker_id,
                            raw_event_ref: Some("deepgram.results.interim".to_string()),
                            ..AsrRevisionMeta::default()
                        },
                    );
                    continue;
                }

                // audio-graph-e70e: a final for this span has now arrived —
                // drop any retained interim so it is never promoted later;
                // the final below is authoritative and is about to persist
                // its own TranscriptSegment(s) for this exact span_id.
                pending_interims_by_span.remove(&span_id);

                // audio-graph-e70e "past-the-range" promotion trigger: this
                // final's `start` is proof that any OTHER still-pending span
                // whose retained interim ended well before it will never get
                // its own final (Deepgram emits `Transcript` events in
                // temporal order) — promote those now instead of waiting for
                // session end. See `past_range_pending_span_ids` /
                // `promote_pending_interim`.
                for stale_span_id in
                    past_range_pending_span_ids(&pending_interims_by_span, &span_id, start)
                {
                    if let Some(stale_pending) = pending_interims_by_span.remove(&stale_span_id) {
                        promote_pending_interim(
                            stale_span_id,
                            stale_pending,
                            &ctx,
                            &mut revision_numbers_by_span,
                            &mut speaker_map,
                            &mut last_speaker,
                            max_speakers,
                            &mut diarization_worker,
                            &mut asr_count,
                            &mut diarization_count,
                            &extraction_count,
                            &graph_update_count,
                        );
                    }
                }

                // Group the final's words into contiguous same-speaker runs
                // (audio-graph-4aed): Deepgram attaches a per-WORD speaker
                // index, so a final that crosses a turn boundary yields 2+
                // runs, split below into one TranscriptSegment per run so
                // each speaker only claims the words actually attributed to
                // them. A final whose words all share one speaker (or carry
                // no speaker at all) yields exactly one run.
                //
                // A word with no speaker (`None`) starts its own run rather
                // than merging into a neighboring speaker's run — with
                // `diarize=true` Deepgram tags every word, so this only
                // fires on partially-tagged finals, and that run falls
                // through to the same empty-audio diarization-fallback path
                // as an undiarized single-run final (below) rather than
                // inheriting a neighbor's speaker with no evidence for it.
                let runs = crate::asr::deepgram::group_words_by_speaker(&words);

                if runs.len() <= 1 {
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
                        source_id: source_id.clone(),
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
                } else {
                    // Multi-speaker final: emit one TranscriptSegment per
                    // same-speaker run. Run 0 keeps the final's own `span_id`
                    // (computed above from `start`, the same key any live
                    // interims for this final used) so it inherits/closes out
                    // those interim revisions exactly like the single-run
                    // path; later runs are keyed on their own first word's
                    // start via `provider_start_span_id`. Word starts are
                    // strictly increasing, so those keys are strictly
                    // increasing too and will not collide UNLESS two runs'
                    // starts round to the same millisecond
                    // (`millis_from_secs` quantizes for span-key stability) —
                    // `used_span_ids` below closes that gap explicitly rather
                    // than leaving it as an unguarded assumption (review
                    // finding: this was previously asserted in comments only,
                    // with no guard/test).
                    //
                    // NOT handled here: a re-sent/corrected final for the
                    // same `start` whose run *shape* changes (e.g. was 2 runs,
                    // now 1) can orphan a previously emitted later-run span at
                    // its old revision with nothing to supersede it — the
                    // same class of cross-final retcon gap as the
                    // pre-existing `final_span_revision` drop-on-reattribution
                    // issue, just visible here across a run boundary instead
                    // of within one span. Left as a follow-up; reconciling it
                    // needs tracking which span_ids a given final `start` has
                    // previously emitted, not just per-span revision counters.
                    let run_count = runs.len();
                    let mut used_span_ids: std::collections::HashSet<String> =
                        std::collections::HashSet::with_capacity(run_count);
                    used_span_ids.insert(span_id.clone());
                    for (run_index, (speaker, run_words)) in runs.into_iter().enumerate() {
                        let is_first_run = run_index == 0;
                        let is_last_run = run_index + 1 == run_count;
                        let run_start = if is_first_run {
                            start
                        } else {
                            run_words.first().map(|w| w.start).unwrap_or(start)
                        };
                        let run_end = if is_last_run {
                            end_time
                        } else {
                            run_words.last().map(|w| w.end).unwrap_or(end_time)
                        };
                        // Join the punctuated form of each word so a split
                        // run keeps the same punctuation/capitalization a
                        // single-speaker final gets for free from the
                        // provider's `text` (audio-graph-4aed review: joining
                        // raw `word` tokens instead would silently lowercase
                        // and de-punctuate every multi-speaker final).
                        let run_text = run_words
                            .iter()
                            .map(|w| w.display_word())
                            .collect::<Vec<_>>()
                            .join(" ");
                        let run_span_id = if is_first_run {
                            span_id.clone()
                        } else {
                            let mut candidate =
                                provider_start_span_id("deepgram", &source_id, run_start);
                            let mut ms_bump: i64 = 0;
                            while used_span_ids.contains(&candidate) {
                                ms_bump += 1;
                                candidate = provider_start_span_id(
                                    "deepgram",
                                    &source_id,
                                    run_start + (ms_bump as f64) / 1000.0,
                                );
                            }
                            candidate
                        };
                        used_span_ids.insert(run_span_id.clone());

                        asr_count += 1;
                        let (run_revision_number, run_supersedes) =
                            final_span_revision(&mut revision_numbers_by_span, &run_span_id);

                        // The speaker-cap remap applies per split run (in word
                        // order), so max_speakers/speaker_map/last_speaker
                        // collapse extra speakers exactly as they would if
                        // these runs had arrived as separate finals.
                        let speaker_from_deepgram = speaker.map(|raw| {
                            let id = remap_deepgram_speaker(
                                raw,
                                max_speakers,
                                &mut speaker_map,
                                &mut last_speaker,
                            );
                            format!("Speaker {}", id)
                        });

                        let run_segment = TranscriptSegment {
                            id: uuid::Uuid::new_v4().to_string(),
                            source_id: source_id.clone(),
                            speaker_id: speaker_from_deepgram.clone(),
                            speaker_label: speaker_from_deepgram,
                            text: run_text,
                            start_time: run_start,
                            end_time: run_end,
                            confidence,
                        };

                        let final_run_segment = if run_segment.speaker_label.is_some() {
                            diarization_count += 1;
                            run_segment.clone()
                        } else {
                            let input = DiarizationInput {
                                transcript: run_segment.clone(),
                                speech_audio: vec![],
                                speech_start_time: Duration::from_secs_f64(run_start),
                                speech_end_time: Duration::from_secs_f64(run_end),
                            };
                            let diarized = diarization_worker.process_input(input);
                            diarization_count += 1;

                            let _ = ctx
                                .app_handle
                                .emit(events::SPEAKER_DETECTED, &diarized.speaker_info);
                            diarized.segment
                        };

                        log::debug!(
                            "Deepgram event receiver: emitted transcript metadata provider=deepgram count={} span_id={} revision={} text_len={} confidence={:.3} speaker_present={} run={}/{}",
                            asr_count,
                            run_span_id,
                            run_revision_number,
                            final_run_segment.text.chars().count(),
                            final_run_segment.confidence,
                            final_run_segment.speaker_label.is_some(),
                            run_index + 1,
                            run_count,
                        );

                        // SPEAKER_DETECTED was already emitted above (if needed)
                        // — pass `None` here so the shared helper doesn't
                        // double-emit.
                        emit_transcript_and_extract_with_meta(
                            final_run_segment,
                            None,
                            &ctx,
                            asr_count,
                            diarization_count,
                            &extraction_count,
                            &graph_update_count,
                            AsrRevisionMeta {
                                span_id: Some(run_span_id),
                                revision_number: Some(run_revision_number),
                                supersedes: run_supersedes,
                                raw_event_ref: Some(format!(
                                    "deepgram.results.final:run{run_index}"
                                )),
                                ..AsrRevisionMeta::default()
                            },
                        );
                    }
                }
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

    // audio-graph-e70e: the event channel has disconnected (session end) —
    // promote every span still waiting on a final that will now never
    // arrive, sorted by start time for deterministic ordering, so words that
    // only ever received interims are not silently dropped from the
    // session's transcript record. Must run BEFORE `flush_pending_now`
    // below: each promotion enqueues a coalesced extraction submission
    // (`emit_transcript_and_extract_with_meta` -> `coalesce_submit`), and
    // that flush call is what drains the coalescing buffer one last time
    // before this thread exits.
    let mut remaining_pending: Vec<(String, PendingInterimSpan)> =
        pending_interims_by_span.drain().collect();
    remaining_pending.sort_by(|(_, a), (_, b)| a.start.total_cmp(&b.start));
    if !remaining_pending.is_empty() {
        log::info!(
            "Deepgram event receiver: promoting {} interim-only span(s) with no final at session end",
            remaining_pending.len(),
        );
    }
    for (stale_span_id, stale_pending) in remaining_pending {
        promote_pending_interim(
            stale_span_id,
            stale_pending,
            &ctx,
            &mut revision_numbers_by_span,
            &mut speaker_map,
            &mut last_speaker,
            max_speakers,
            &mut diarization_worker,
            &mut asr_count,
            &mut diarization_count,
            &extraction_count,
            &graph_update_count,
        );
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

    let mut asr_count: u64 = 0;
    let extraction_count = Arc::new(AtomicU64::new(0));
    let graph_update_count = Arc::new(AtomicU64::new(0));

    // The live v3 path decodes Turn/SpeakerRevision frames through the
    // source-aware `AssemblyAiV3Parser`; speaker attribution comes from the
    // provider's own `speaker_label`/`SpeakerRevision` messages, so no local
    // diarization worker or turn-index bookkeeping is needed here.
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

    log::info!("AssemblyAI event receiver: exiting. ASR segments={asr_count}");
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

    let (diarization_config, diarization_degradation) =
        make_diarization_config(&config.models_dir, config.diarization_mode);
    apply_diarization_degradation(
        &shared.app_handle,
        &shared.pipeline_status,
        diarization_degradation,
    );
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

    let (diarization_config, diarization_degradation) =
        make_diarization_config(&config.models_dir, config.diarization_mode);
    apply_diarization_degradation(
        &shared.app_handle,
        &shared.pipeline_status,
        diarization_degradation,
    );
    let (dummy_diar_tx, _dummy_diar_rx) = crossbeam_channel::unbounded::<DiarizedTranscript>();
    let mut diarization_worker = DiarizationWorker::new(diarization_config, dummy_diar_tx);

    let mut asr_count: u64 = 0;
    let mut diarization_count: u64 = 0;
    let extraction_count = Arc::new(AtomicU64::new(0));
    let graph_update_count = Arc::new(AtomicU64::new(0));
    // Shared mirror of `asr_count` so the reconnect status callback can restore
    // the running transcript count on `Reconnected` without racing the
    // transcript callback that owns the `u64` (M1 / audio-graph-35de).
    let asr_count_shared = Arc::new(AtomicU64::new(0));

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

    // Reconnect status callback (M1 / audio-graph-35de): mirror the WebSocket
    // siblings' `Reconnecting`/`Reconnected` → pipeline `StageStatus` mapping so
    // the UI shows a "reconnecting…" hint (not a healthy dot) while the AWS
    // stream is being re-established, and returns to Running on success.
    let app_handle_for_status = ctx.app_handle.clone();
    let pipeline_status_for_status = ctx.pipeline_status.clone();
    let asr_count_for_status = asr_count_shared.clone();
    let on_status = move |status: crate::asr::aws_transcribe::AwsTranscribeStatus| match status {
        crate::asr::aws_transcribe::AwsTranscribeStatus::Reconnecting {
            attempt,
            backoff_secs,
        } => {
            log::info!("AWS Transcribe: reconnecting attempt={attempt} backoff={backoff_secs}s");
            set_asr_status_and_emit(
                &app_handle_for_status,
                &pipeline_status_for_status,
                StageStatus::Error {
                    message: format!(
                        "AWS Transcribe reconnecting (attempt {attempt}, retry in {backoff_secs}s)"
                    ),
                },
            );
        }
        crate::asr::aws_transcribe::AwsTranscribeStatus::Reconnected => {
            log::info!("AWS Transcribe: reconnected");
            set_asr_status_and_emit(
                &app_handle_for_status,
                &pipeline_status_for_status,
                StageStatus::Running {
                    processed_count: asr_count_for_status.load(Ordering::Relaxed),
                },
            );
        }
    };

    let asr_count_for_transcript = asr_count_shared.clone();
    let result = crate::asr::aws_transcribe::run_aws_transcribe_session(
        processed_rx,
        is_transcribing,
        aws_config,
        move |transcript| {
            asr_count += 1;
            asr_count_for_transcript.store(asr_count, Ordering::Relaxed);
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
        on_status,
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

    let (diarization_config, diarization_degradation) =
        make_diarization_config(&config.models_dir, config.diarization_mode);
    apply_diarization_degradation(
        &shared.app_handle,
        &shared.pipeline_status,
        diarization_degradation,
    );
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

// Unit tests for the settings→worker-config mapping at the pipeline dispatch
// boundary (provider-selection accuracy audit, 2026-07-05). Pins the
// `AsrProvider::DeepgramStreaming` → `DeepgramConfig` rules so a future edit
// can't silently send `endpointing=0` / an invalid eager-EOT pair to the wire
// or drop the user's selected model.
#[cfg(test)]
mod tests_provider_dispatch {
    use super::deepgram_config_from_settings;
    use crate::asr::ProviderContentEgressPolicy;
    use crate::settings::AsrProvider;

    fn deepgram_provider(
        endpointing_ms: u32,
        utterance_end_ms: u32,
        eot_threshold: f32,
        eager_eot_threshold: f32,
        eot_timeout_ms: u32,
    ) -> AsrProvider {
        AsrProvider::DeepgramStreaming {
            api_key: "dg-test-key".into(),
            model: "nova-3".into(),
            enable_diarization: true,
            endpointing_ms,
            utterance_end_ms,
            vad_events: true,
            eot_threshold,
            eager_eot_threshold,
            eot_timeout_ms,
            max_speakers: 0,
            keyterms: vec![],
        }
    }

    #[test]
    fn passthrough_fields_survive_verbatim() {
        // model / api_key / diarization / vad must reach the worker config
        // unchanged — the exact drift class of the historical
        // `model="general"` bug.
        let provider = AsrProvider::DeepgramStreaming {
            api_key: "dg-key-123".into(),
            model: "flux-general-en".into(),
            enable_diarization: false,
            endpointing_ms: 300,
            utterance_end_ms: 1000,
            vad_events: false,
            eot_threshold: 0.7,
            eager_eot_threshold: 0.0,
            eot_timeout_ms: 5000,
            max_speakers: 3,
            keyterms: vec!["KV cache".to_string(), "transformer".to_string()],
        };
        let config = deepgram_config_from_settings(&provider, ProviderContentEgressPolicy::allow())
            .expect("deepgram variant maps");
        assert_eq!(config.api_key, "dg-key-123");
        assert_eq!(config.model, "flux-general-en");
        assert!(!config.enable_diarization);
        assert!(!config.vad_events);
        assert_eq!(
            config.keyterms,
            vec!["KV cache".to_string(), "transformer".to_string()],
            "keyterms must reach the worker config verbatim"
        );
        assert_eq!(
            config.content_egress_policy,
            ProviderContentEgressPolicy::allow()
        );
    }

    #[test]
    fn zero_sentinels_map_to_none_not_some_zero() {
        // 0 means "not configured": the URL builder must not receive
        // Some(0) — that would emit `endpointing=0` etc. and change provider
        // behavior server-side.
        let provider = deepgram_provider(0, 0, 0.0, 0.0, 0);
        let config =
            deepgram_config_from_settings(&provider, ProviderContentEgressPolicy::allow()).unwrap();
        assert_eq!(config.endpointing_ms, None);
        assert_eq!(config.utterance_end_ms, None);
        assert_eq!(config.eot_threshold, None);
        assert_eq!(config.eager_eot_threshold, None);
        assert_eq!(config.eot_timeout_ms, None);
    }

    #[test]
    fn configured_values_map_to_some() {
        let provider = deepgram_provider(300, 1000, 0.5, 0.3, 4000);
        let config =
            deepgram_config_from_settings(&provider, ProviderContentEgressPolicy::allow()).unwrap();
        assert_eq!(config.endpointing_ms, Some(300));
        assert_eq!(config.utterance_end_ms, Some(1000));
        assert_eq!(config.eot_threshold, Some(0.5));
        assert_eq!(config.eager_eot_threshold, Some(0.3));
        assert_eq!(config.eot_timeout_ms, Some(4000));
    }

    #[test]
    fn eager_eot_forwarded_only_when_valid_pair() {
        // eager > eot is an invalid pair — must be dropped, not forwarded.
        let too_eager = deepgram_provider(0, 0, 0.5, 0.9, 0);
        let config =
            deepgram_config_from_settings(&too_eager, ProviderContentEgressPolicy::allow())
                .unwrap();
        assert_eq!(config.eot_threshold, Some(0.5));
        assert_eq!(
            config.eager_eot_threshold, None,
            "eager > eot must not be forwarded"
        );

        // eager == eot is the boundary and is allowed.
        let equal = deepgram_provider(0, 0, 0.5, 0.5, 0);
        let config =
            deepgram_config_from_settings(&equal, ProviderContentEgressPolicy::allow()).unwrap();
        assert_eq!(config.eager_eot_threshold, Some(0.5));

        // eager set but eot unset (0.0): 0.3 > 0.0 fails the `<=` guard, so
        // nothing is forwarded — an eager threshold without a main threshold
        // has no defined meaning on the wire.
        let eager_only = deepgram_provider(0, 0, 0.0, 0.3, 0);
        let config =
            deepgram_config_from_settings(&eager_only, ProviderContentEgressPolicy::allow())
                .unwrap();
        assert_eq!(config.eot_threshold, None);
        assert_eq!(config.eager_eot_threshold, None);
    }

    #[test]
    fn non_deepgram_variants_map_to_none() {
        for provider in [
            AsrProvider::LocalWhisper,
            AsrProvider::Api {
                endpoint: "https://api.openai.com/v1".into(),
                api_key: String::new(),
                model: "whisper-1".into(),
            },
        ] {
            assert!(
                deepgram_config_from_settings(&provider, ProviderContentEgressPolicy::allow())
                    .is_none(),
                "non-deepgram provider must not produce a Deepgram config"
            );
        }
    }

    #[test]
    fn blocked_egress_policy_is_threaded_through() {
        // The privacy gate must ride along into the worker config — a lost
        // policy here would let a local_only session stream audio out.
        let provider = deepgram_provider(300, 1000, 0.5, 0.0, 0);
        let policy = ProviderContentEgressPolicy::block("local_only");
        let config = deepgram_config_from_settings(&provider, policy).unwrap();
        assert_eq!(config.content_egress_policy, policy);
    }
}

// Unit tests for the FA-1 pipeline-status helper: a poisoned `pipeline_status`
// lock must still record the ASR error status (poison recovery), not silently
// swallow it. The emit half is Tauri-bound and exercised at the integration
// layer; the pure write half is tested here.
#[cfg(test)]
mod tests_status {
    use super::{
        DiarizationDegradationReason, DiarizationDispatchContext, DiarizationEventSink,
        ExtractionDeps, PipelineStatus, ProjectionDataMovementSink, ProjectionDispatchContext,
        ProjectionPatchAttempt, ProjectionPatchGenerator, ProjectionRuntimeEventSink,
        SpeechChannels, SpeechConfig, SpeechShared, StageStatus,
        apply_extraction_result_if_current, aws_error_diagnostic, aws_error_for_diagnostic_event,
        cloud_error_code, current_unix_millis, deregister_projection_job,
        diarization_degradation_for_provider_labeled_session,
        diarization_span_revision_for_transcript, dispatch_projection_decision,
        emit_and_dispatch_diarization_span_revision,
        emit_assemblyai_speaker_revision_with_dispatch, final_only_revision_meta,
        final_span_revision, make_diarization_config, moonshine_final_transcript_segment,
        moonshine_revision_meta, next_span_revision, provider_item_span_id,
        provider_sequence_span_id, provider_start_span_id, record_asr_span_revision_event,
        record_asr_span_revision_event_and_observe_projection, revision_ref,
        run_agent_proposal_task, run_moonshine_speech_processor_with_worker, run_projection_job,
        set_asr_status, spawn_deferred_lane_observation, spawn_projection_job,
        speech_error_diagnostic,
    };
    use crate::asr::moonshine::{
        MoonshineAdapterError, MoonshineRuntimeConfig, MoonshineSpanMapper,
        MoonshineStreamingAdapter, MoonshineStreamingWorker, MoonshineTranscriptLine,
    };
    use crate::audio::pipeline::{PROCESSED_AUDIO_SAMPLE_RATE_HZ, ProcessedAudioChunk};
    use crate::events::{self, AsrSpanRevisionPayload, AsrSpanStability, DiarizationSpanStability};
    use crate::graph::entities::{GraphDelta, GraphSnapshot};
    use crate::llm::ProjectionPatchOutcome;
    use crate::persistence::{
        FileMemoryRepository, LocalMemoryRepository, TranscriptEventWriter,
        load_materialized_graph, load_materialized_notes, load_projection_events,
        load_transcript_events,
    };
    use crate::projection_scheduler::{ProjectionSchedulerDecision, ProjectionSchedulers};
    use crate::projections::{
        AppliedBasisCurrency, DiarizationEventStability, MaterializedNotes, ProjectionJob,
        ProjectionKind, ProjectionOperation, ProjectionPatch, ProjectionProvenance,
        SpeakerTimeline, TranscriptLedger,
    };
    use crate::settings::LlmProvider;
    use crate::state::{AppState, TranscriptSegment};
    use std::collections::{HashMap, VecDeque};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier, Mutex, RwLock};
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

    // ── audio-graph-586b: diarization backend-selection honesty ─────────

    #[test]
    fn diarization_mode_off_skips_asset_probing_and_never_degrades() {
        // A user who explicitly turned diarization off must get Simple with
        // no degradation report — probing for (and warning about) a neural
        // model asset they never asked to use would be a false claim about
        // what they configured.
        let models_dir = unique_tempdir("mode-off");
        let (config, degradation) =
            make_diarization_config(&models_dir, crate::settings::DiarizationMode::Off);
        assert!(matches!(
            config.backend,
            crate::diarization::DiarizationBackend::Simple
        ));
        assert_eq!(degradation, None);
        let _ = std::fs::remove_dir_all(&models_dir);
    }

    // Review follow-up (audio-graph-586b): this test is specifically about
    // the "no neural engine AT ALL" configuration — it must NOT run under
    // `--features diarization-clustering`, where a neural engine genuinely
    // IS compiled and an empty temp dir now correctly (per the
    // `ClusteringAssetsNotDownloaded` fix below) reports the
    // clustering-specific code instead. Nor under `--features diarization`,
    // where the same empty temp dir reports `AssetNotDownloaded`. Previously
    // unguarded, this test only stayed green under `diarization-clustering`
    // by accident, via the exact fallthrough bug that finding fixes.
    #[cfg(not(any(feature = "diarization", feature = "diarization-clustering")))]
    #[test]
    fn diarization_mode_provider_without_engine_compiled_reports_degradation() {
        // This test binary's default features compile neither `diarization`
        // nor `diarization-clustering` (see `Cargo.toml`'s `[features]`
        // block — verified against `release.yml`, which passes no feature
        // flags either, so this is also today's SHIPPED configuration, not
        // just the test build). `mode = Provider` (the settings default)
        // must not silently accept the Simple fallback — the caller has to
        // learn about it via the returned reason.
        let models_dir = unique_tempdir("mode-provider-no-engine");
        let (config, degradation) =
            make_diarization_config(&models_dir, crate::settings::DiarizationMode::Provider);
        assert!(matches!(
            config.backend,
            crate::diarization::DiarizationBackend::Simple
        ));
        let reason = degradation.expect(
            "a build with no neural diarization engine compiled in must report a degradation",
        );
        // Review follow-up (audio-graph-586b): exact-equality on the typed
        // code, not a substring match on composed English — the wire value
        // is now a stable `snake_case` code (see
        // `DiarizationDegradationReason::as_wire_code`), not prose.
        assert_eq!(
            reason,
            DiarizationDegradationReason::EngineNotCompiled,
            "reason should name the degraded state, got {reason:?}"
        );
        let _ = std::fs::remove_dir_all(&models_dir);
    }

    #[cfg(feature = "diarization")]
    #[test]
    fn diarization_mode_local_with_missing_sortformer_asset_reports_not_downloaded() {
        let models_dir = unique_tempdir("mode-local-missing-asset");
        let (config, degradation) =
            make_diarization_config(&models_dir, crate::settings::DiarizationMode::Local);
        assert!(matches!(
            config.backend,
            crate::diarization::DiarizationBackend::Simple
        ));
        let reason = degradation.expect("a missing Sortformer asset must report a degradation");
        assert_eq!(
            reason,
            DiarizationDegradationReason::AssetNotDownloaded,
            "reason should name the missing-asset remedy, got {reason:?}"
        );
        let _ = std::fs::remove_dir_all(&models_dir);
    }

    #[cfg(feature = "diarization")]
    #[test]
    fn diarization_mode_hybrid_with_corrupt_sortformer_asset_reports_invalid() {
        let models_dir = unique_tempdir("mode-hybrid-corrupt-asset");
        let path = models_dir.join(crate::models::SORTFORMER_MODEL_FILENAME);
        std::fs::write(&path, vec![0u8; 1024]).expect("write truncated model file");

        let (config, degradation) =
            make_diarization_config(&models_dir, crate::settings::DiarizationMode::Hybrid);
        assert!(matches!(
            config.backend,
            crate::diarization::DiarizationBackend::Simple
        ));
        let reason = degradation.expect("a corrupt Sortformer asset must report a degradation");
        assert_eq!(
            reason,
            DiarizationDegradationReason::AssetInvalid,
            "reason should name the corrupt-asset remedy, got {reason:?}"
        );
        let _ = std::fs::remove_dir_all(&models_dir);
    }

    #[cfg(feature = "diarization")]
    #[test]
    fn diarization_mode_provider_with_ready_sortformer_asset_never_degrades() {
        let models_dir = unique_tempdir("mode-provider-ready-asset");
        let path = models_dir.join(crate::models::SORTFORMER_MODEL_FILENAME);
        let file = std::fs::File::create(&path).expect("create model file");
        file.set_len(crate::models::SORTFORMER_EXPECTED_SIZE)
            .expect("size sparse model file");
        drop(file);

        let (config, degradation) =
            make_diarization_config(&models_dir, crate::settings::DiarizationMode::Provider);
        assert!(matches!(
            config.backend,
            crate::diarization::DiarizationBackend::Sortformer { .. }
        ));
        assert_eq!(degradation, None);
        let _ = std::fs::remove_dir_all(&models_dir);
    }

    #[cfg(feature = "diarization-clustering")]
    #[test]
    fn diarization_mode_hybrid_with_missing_clustering_assets_reports_clustering_not_downloaded() {
        // Review fix (audio-graph-586b): under a build that DOES compile
        // `diarization-clustering`, missing assets must report the
        // clustering-specific code, not the generic "no engine compiled"
        // one — a neural engine genuinely IS present here.
        let models_dir = unique_tempdir("mode-hybrid-missing-clustering-assets");
        let (config, degradation) =
            make_diarization_config(&models_dir, crate::settings::DiarizationMode::Hybrid);
        assert!(matches!(
            config.backend,
            crate::diarization::DiarizationBackend::Simple
        ));
        assert_eq!(
            degradation,
            Some(DiarizationDegradationReason::ClusteringAssetsNotDownloaded),
            "a diarization-clustering build with missing assets must name that specific \
             degradation, not claim no neural engine is compiled"
        );
        let _ = std::fs::remove_dir_all(&models_dir);
    }

    // Review follow-up (audio-graph-586b): every degradation code must be a
    // distinct, stable `snake_case` string — the frontend keys a translated
    // string off it (`pipeline.diarizationDegradedReason.<code>`), so a
    // mutation that collapses two variants onto the same code, or drifts the
    // format, would silently corrupt an unrelated locale's message.
    #[test]
    fn diarization_degradation_reason_wire_codes_are_distinct_snake_case() {
        let all = [
            DiarizationDegradationReason::EngineNotCompiled,
            DiarizationDegradationReason::ClusteringAssetsNotDownloaded,
            DiarizationDegradationReason::AssetNotDownloaded,
            DiarizationDegradationReason::AssetInvalid,
        ];
        let codes: Vec<&str> = all.iter().map(|r| r.as_wire_code()).collect();
        let mut unique = codes.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique.len(),
            codes.len(),
            "every DiarizationDegradationReason variant must have a distinct wire code, got {codes:?}"
        );
        for code in &codes {
            assert!(
                code.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "wire code {code:?} must be snake_case ASCII to match the i18n key convention"
            );
        }
    }

    // ── audio-graph-586b review follow-up: provider-native diarization
    // (e.g. Deepgram `diarize=true`) must suppress the local-engine
    // degradation report, since the local worker was never needed. ────────

    #[test]
    fn provider_native_diarization_active_suppresses_local_engine_degradation() {
        // Default settings (mode=Provider, Deepgram enable_diarization=true)
        // on a shipped build with no neural engine compiled: the local probe
        // legitimately finds "basic mode", but Deepgram is labeling segments
        // natively, so no banner should reach the UI.
        let local_reason = Some(DiarizationDegradationReason::EngineNotCompiled);
        assert_eq!(
            diarization_degradation_for_provider_labeled_session(true, local_reason),
            None,
            "a session where the provider is delivering native speaker labels must never \
             report the local-engine degradation, even when the local probe found one"
        );
    }

    #[test]
    fn provider_native_diarization_inactive_passes_through_local_engine_degradation() {
        // mode=Local (or any config that forced Deepgram's enable_diarization
        // off): every segment falls through to the local worker, so the
        // ordinary honesty behavior from audio-graph-586b's original fix
        // must still apply unchanged.
        let local_reason = Some(DiarizationDegradationReason::EngineNotCompiled);
        assert_eq!(
            diarization_degradation_for_provider_labeled_session(false, local_reason),
            local_reason,
            "when the provider isn't diarizing, the local probe's reason must pass through \
             verbatim"
        );
        assert_eq!(
            diarization_degradation_for_provider_labeled_session(false, None),
            None,
            "no local reason means no degradation report regardless of provider state"
        );
    }

    /// Real accepting writer fixture for ledger-only tests. `None` now means
    /// persistence is unavailable and must reject, so success-path tests use a
    /// repository writer instead of accidentally weakening the production
    /// durability contract.
    struct AcceptingTranscriptEventWriterFixture {
        writer: Arc<Mutex<Option<TranscriptEventWriter>>>,
        root: PathBuf,
    }

    impl AcceptingTranscriptEventWriterFixture {
        fn new(session_id: &str) -> Self {
            let root = unique_tempdir("accepting-transcript-event-writer");
            let repository = Arc::new(FileMemoryRepository::with_data_root(&root));
            let writer = TranscriptEventWriter::repository(session_id, repository)
                .expect("repository transcript event writer");
            Self {
                writer: Arc::new(Mutex::new(Some(writer))),
                root,
            }
        }

        fn writer(&self) -> Arc<Mutex<Option<TranscriptEventWriter>>> {
            self.writer.clone()
        }
    }

    impl Drop for AcceptingTranscriptEventWriterFixture {
        fn drop(&mut self) {
            let writer = self
                .writer
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take();
            if let Some(writer) = writer {
                // Drop must remain panic-free so a failed assertion in the
                // owning test cannot turn into an abort while unwinding.
                let _ = writer.shutdown_with_timeout(Duration::from_secs(2));
            }
            let _ = std::fs::remove_dir_all(&self.root);
        }
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
        /// The identity a real dispatch would have captured from the LIVE
        /// route before the (here, test-driven) outcome. `None` reproduces the
        /// pre-fix "nothing was resolved" case; existing tests that don't call
        /// [`Self::with_attempted_route`] get this default, so they keep
        /// exercising the snapshot-derived fallback unchanged.
        attempted_route: Option<crate::llm::route::AttemptedRouteIdentity>,
        #[allow(clippy::type_complexity)]
        generate: Arc<
            dyn Fn(
                    ProjectionJob,
                    TranscriptLedger,
                    Option<MaterializedNotes>,
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
                Option<MaterializedNotes>,
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
                    attempted_route: None,
                    generate: Arc::new(generate),
                },
                calls,
            )
        }

        /// Attach the route identity a real dispatch would have captured live
        /// before failing, so a test can drive `run_projection_job` through the
        /// SAME `FailedRoute` ledger path a production repoint/Cerebras failure
        /// would (seeds audio-graph-862c / audio-graph-7da4).
        fn with_attempted_route(
            mut self,
            identity: crate::llm::route::AttemptedRouteIdentity,
        ) -> Self {
            self.attempted_route = Some(identity);
            self
        }
    }

    impl ProjectionPatchGenerator for FnProjectionPatchGenerator {
        fn generate_projection_patch(
            &self,
            job: ProjectionJob,
            ledger: TranscriptLedger,
            notes: Option<MaterializedNotes>,
            sequence: u64,
            created_at_ms: u64,
        ) -> ProjectionPatchAttempt {
            self.calls.fetch_add(1, Ordering::SeqCst);
            ProjectionPatchAttempt {
                outcome: (self.generate)(job, ledger, notes, sequence, created_at_ms),
                attempted_route: self.attempted_route,
            }
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

        /// Ticket W3 (audio-graph-a6b5): value-level access to the captured
        /// patches themselves, not just their count — needed to assert
        /// `basis_currency_at_apply` landed on the EMITTED clone with the
        /// real classification the apply gate returned.
        fn patches_snapshot(&self) -> Vec<ProjectionPatch> {
            self.patches
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone()
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

    #[derive(Clone, Default)]
    struct RecordingProjectionDataMovementSink {
        events: Arc<Mutex<Vec<crate::persistence::DataMovementEvent>>>,
        /// Raw `ProjectionMovementFacts` captured via `record_movement_facts`
        /// — value-level visibility that `events` above cannot give for
        /// fields with no `DataMovementEvent`/`MovementCounts` sink (e.g.
        /// `no_op_filtered_count`).
        movement_facts: Arc<Mutex<Vec<crate::projection_data_movement::ProjectionMovementFacts>>>,
    }

    impl ProjectionDataMovementSink for RecordingProjectionDataMovementSink {
        fn record(&self, _session_id: &str, event: &crate::persistence::DataMovementEvent) {
            self.events
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(event.clone());
        }

        fn record_movement_facts(
            &self,
            facts: &crate::projection_data_movement::ProjectionMovementFacts,
        ) {
            self.movement_facts
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(facts.clone());
        }
    }

    fn projection_dispatch_for_app(
        app: &AppState,
        generator: FnProjectionPatchGenerator,
    ) -> (
        ProjectionDispatchContext,
        RecordingProjectionRuntimeEventSink,
    ) {
        let (dispatch, event_sink, _movements) = projection_dispatch_for_app_with_movement(
            app,
            generator,
            LlmProvider::LocalLlama,
            false,
        );
        (dispatch, event_sink)
    }

    fn projection_dispatch_for_app_with_movement(
        app: &AppState,
        generator: FnProjectionPatchGenerator,
        llm_provider: LlmProvider,
        llm_allow_cloud_fallbacks: bool,
    ) -> (
        ProjectionDispatchContext,
        RecordingProjectionRuntimeEventSink,
        RecordingProjectionDataMovementSink,
    ) {
        let event_sink = RecordingProjectionRuntimeEventSink::default();
        let movement_sink = RecordingProjectionDataMovementSink::default();
        (
            ProjectionDispatchContext {
                transcript_ledger: app.transcript_ledger.clone(),
                projection_schedulers: app.projection_schedulers.clone(),
                projection_runtime: app.projection_runtime_handle(),
                projection_job_workers: app.projection_job_workers.clone(),
                projection_lane_stopping: app.projection_lane_stopping.clone(),
                event_sink: Arc::new(event_sink.clone()),
                patch_generator: Arc::new(generator),
                llm_provider,
                llm_allow_cloud_fallbacks,
                data_movement_sink: Arc::new(movement_sink.clone()),
            },
            event_sink,
            movement_sink,
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
                evidence: crate::claim_evidence::EvidenceAnchor::default(),
                heading_level: None,
            }],
            ProjectionKind::Graph => vec![ProjectionOperation::UpsertGraphNode {
                id: format!("node-{}", job.basis.transcript_hash),
                name: "Projection Node".to_string(),
                entity_type: "concept".to_string(),
                description: Some(format!(
                    "Projected {} transcript span(s).",
                    job.basis.span_revisions.len()
                )),
                evidence: crate::claim_evidence::EvidenceAnchor::default(),
            }],
        };
        ProjectionPatch {
            route: None,
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
                route_id: None,
                model_source: crate::llm::route::ModelIdentitySource::Requested,
            },
            queued_at_ms: None,
            generation_latency_ms: None,
            apply_latency_ms: None,
            basis_currency_at_apply: None,
            created_at_ms,
        }
    }

    /// ADR-0045 decision 3 (audio-graph-bf5d) test-cleanup fix: stop the
    /// projection lane and wait for every registered projection job AND
    /// deferred-retry clock thread (`spawn_deferred_lane_observation`) to
    /// self-deregister, mirroring `stop_capture_impl`'s real Stop path.
    ///
    /// Without this, any test that drives a same-basis `Current` failure
    /// under budget through the real dispatch tail arms a REAL
    /// wall-clock-scheduled (~60s) clock thread that this function never
    /// used to know about. `projection_lane_stopping` was never set for such
    /// a test, so nothing shortened that thread's wait: it outlived the
    /// test, `drain_app_writers`'s writer shutdown below, the test's own
    /// `DataDirGuard` drop, and the temp-dir removal that follows it, then
    /// fired minutes later against already-torn-down state (re-locking the
    /// ledger/schedulers and potentially resolving `AUDIOGRAPH_DATA_DIR` to
    /// the developer's real `~/.audiograph` once the guard's directory
    /// override was gone). Setting the flag first makes the clock thread's
    /// own 250ms poll exit promptly instead of waiting out its real deadline;
    /// looping on the registry (rather than trusting one snapshot) covers a
    /// still-running clock thread firing a real retry job that registers a
    /// NEW entry after an earlier pass. Runs before the writer shutdown below
    /// so a retry that was already past its point of no return finishes
    /// emitting through a live writer instead of a torn-down one.
    fn drain_app_writers(app: &AppState) {
        app.projection_lane_stopping.store(true, Ordering::SeqCst);
        let drain_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let empty = app
                .projection_job_workers
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .is_empty();
            if empty {
                break;
            }
            assert!(
                std::time::Instant::now() < drain_deadline,
                "projection job/clock threads did not drain within the test timeout"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

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
            active_session_id: app.session_id.clone(),
            transcript_buffer: app.transcript_buffer.clone(),
            transcript_writer: app.transcript_writer.clone(),
            display_transcript_write_misses: app.display_transcript_write_misses.clone(),
            retired_session_workers: app.retired_session_workers.clone(),
            transcript_event_writer: app.transcript_event_writer.clone(),
            transcript_ledger: app.transcript_ledger.clone(),
            speaker_timeline: app.speaker_timeline.clone(),
            projection_schedulers: app.projection_schedulers.clone(),
            projection_runtime: app.projection_runtime_handle(),
            projection_job_workers: app.projection_job_workers.clone(),
            projection_lane_stopping: app.projection_lane_stopping.clone(),
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

    #[test]
    #[cfg_attr(
        target_os = "macos",
        ignore = "Tauri/Tao App construction must run on the macOS main thread"
    )]
    fn extraction_result_released_after_rotation_cannot_mutate_or_emit() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = unique_tempdir("stale-extraction-generation");
        let _guard = DataDirGuard::set(&dir);
        let app = AppState::new();
        let app_handle = super::shared_test_app_handle();
        let shared = moonshine_shared_for_app(&app, app_handle.clone());
        let expected_session_id = app.current_session_id();
        let extraction_result = app.graph_extractor.extract(
            "Speaker 1",
            "Alice Smith works with Example Labs on Project Aurora.",
        );
        assert!(
            !extraction_result.entities.is_empty(),
            "precondition: completed extraction must carry a graph mutation"
        );

        let emitted = Arc::new(AtomicUsize::new(0));
        let mut listeners = Vec::new();
        for event_name in [
            events::GRAPH_DELTA,
            events::GRAPH_UPDATE,
            events::PIPELINE_STATUS_EVENT,
            events::PIPELINE_LATENCY,
        ] {
            let emitted = emitted.clone();
            listeners.push(app_handle.listen_any(event_name, move |_| {
                emitted.fetch_add(1, Ordering::SeqCst);
            }));
        }

        let ready = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let worker_ready = ready.clone();
        let worker_release = release.clone();
        let worker = std::thread::spawn(move || {
            // Model an extraction whose expensive provider/local work already
            // completed, but whose result is paused immediately before commit.
            worker_ready.wait();
            worker_release.wait();
            let llm_provider = LlmProvider::default();
            let deps = ExtractionDeps {
                active_session_id: &shared.active_session_id,
                transcript_ledger: &shared.transcript_ledger,
                expected_session_id: &expected_session_id,
                llm_engine: &shared.llm_engine,
                api_client: &shared.api_client,
                mistralrs_engine: &shared.mistralrs_engine,
                llm_executor: &shared.llm_executor,
                llm_provider: &llm_provider,
                llm_allow_cloud_fallbacks: false,
                graph_extractor: &shared.graph_extractor,
                knowledge_graph: &shared.knowledge_graph,
                graph_snapshot: &shared.graph_snapshot,
                pipeline_status: &shared.pipeline_status,
                app_handle: &shared.app_handle,
            };
            let mut extraction_count = 0;
            let mut graph_update_count = 0;
            let committed = apply_extraction_result_if_current(
                extraction_result,
                "Speaker 1",
                "old-session-segment",
                1.0,
                Duration::from_millis(25),
                &deps,
                &mut extraction_count,
                &mut graph_update_count,
            );
            (committed, extraction_count, graph_update_count)
        });

        ready.wait();
        let rotated_session_id = "rotated-after-extraction";
        *app.transcript_ledger
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            TranscriptLedger::new(rotated_session_id);
        *app.session_id
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = rotated_session_id.to_string();
        release.wait();

        let (committed, extraction_count, graph_update_count) =
            worker.join().expect("stale extraction worker");
        assert!(!committed, "stale extraction result must be discarded");
        assert_eq!(extraction_count, 0);
        assert_eq!(graph_update_count, 0);
        assert_eq!(
            app.knowledge_graph
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .node_count(),
            0,
            "old extraction must not populate the new session graph"
        );
        assert_eq!(
            app.graph_snapshot
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .stats
                .total_nodes,
            0
        );
        assert_eq!(
            emitted.load(Ordering::SeqCst),
            0,
            "stale extraction must not emit graph, status, or latency events"
        );

        for listener in listeners {
            app_handle.unlisten(listener);
        }
        drain_app_writers(&app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg_attr(
        target_os = "macos",
        ignore = "Tauri/Tao App construction must run on the macOS main thread"
    )]
    fn proposal_task_released_after_rotation_cannot_mutate_persist_or_emit() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = unique_tempdir("stale-proposal-generation");
        let _guard = DataDirGuard::set(&dir);
        let app = AppState::new();
        let app_handle = super::shared_test_app_handle();
        let expected_session_id = app.current_session_id();
        let pending_agent_proposals = app.pending_agent_proposals.clone();
        let active_session_id = app.session_id.clone();
        let transcript_ledger = app.transcript_ledger.clone();
        let segment = TranscriptSegment {
            id: "old-proposal-segment".to_string(),
            source_id: "system".to_string(),
            speaker_id: None,
            speaker_label: Some("Speaker 1".to_string()),
            text: "Remember that Alice Smith owns the launch plan.".to_string(),
            start_time: 1.0,
            end_time: 2.0,
            confidence: 0.95,
        };

        let emitted = Arc::new(AtomicUsize::new(0));
        let mut listeners = Vec::new();
        for event_name in [
            events::AGENT_STATUS,
            events::AGENT_PROPOSAL,
            events::PIPELINE_LATENCY,
        ] {
            let emitted = emitted.clone();
            listeners.push(app_handle.listen_any(event_name, move |_| {
                emitted.fetch_add(1, Ordering::SeqCst);
            }));
        }

        let ready = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let worker_ready = ready.clone();
        let worker_release = release.clone();
        let worker_app = app_handle.clone();
        let worker_expected_session_id = expected_session_id.clone();
        let worker = std::thread::spawn(move || {
            worker_ready.wait();
            worker_release.wait();
            run_agent_proposal_task(
                segment.clone(),
                segment.text.trim().to_string(),
                worker_expected_session_id,
                "old-proposal-span".to_string(),
                worker_app,
                pending_agent_proposals,
                active_session_id,
                transcript_ledger,
            )
        });

        ready.wait();
        let rotated_session_id = "rotated-after-proposal";
        *app.transcript_ledger
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            TranscriptLedger::new(rotated_session_id);
        *app.session_id
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = rotated_session_id.to_string();
        release.wait();

        assert!(
            !worker.join().expect("stale proposal worker"),
            "stale proposal task must be discarded"
        );
        assert!(
            app.pending_agent_proposals
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_empty(),
            "old proposal must not repopulate the new session map"
        );
        for session_id in [&expected_session_id, rotated_session_id] {
            let live_assist = dir.join("live_assist");
            assert!(!live_assist.join(format!("{session_id}.jsonl")).exists());
            assert!(
                !live_assist
                    .join(format!("{session_id}.current.json"))
                    .exists()
            );
        }
        assert_eq!(
            emitted.load(Ordering::SeqCst),
            0,
            "stale proposal must not emit proposal, status, or latency events"
        );

        for listener in listeners {
            app_handle.unlisten(listener);
        }
        drain_app_writers(&app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn moonshine_speech_config(models_dir: PathBuf) -> SpeechConfig {
        SpeechConfig {
            models_dir,
            llm_provider: LlmProvider::default(),
            llm_allow_cloud_fallbacks: true,
            provider_content_egress_policy: crate::asr::ProviderContentEgressPolicy::allow(),
            diarization_mode: crate::settings::DiarizationMode::default(),
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
        let session_id = "assemblyai-diarization-retcon";
        let diarization_dispatch = DiarizationDispatchContext {
            event_sink: &event_sink,
            speaker_timeline: &app.speaker_timeline,
            knowledge_graph: &app.knowledge_graph,
            graph_snapshot: &app.graph_snapshot,
            transcript_ledger: &app.transcript_ledger,
            session_id,
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

    /// audio-graph-719d acceptance: a mid-session speaker relabel dispatched on
    /// the LIVE path (`emit_and_dispatch_diarization_span_revision`) must be
    /// durably appended to the session's `<id>.speaker.jsonl`, and a
    /// `SpeakerTimeline` rebuilt from that on-disk log must reflect the LATEST
    /// (corrected) attribution — proving the retcon survives reload / can be
    /// replayed as the basis-gate's durable ground truth. Before this seed the
    /// live path only mutated the in-memory timeline, so the relabel was lost on
    /// reload (ADR-0026 §3 cross-reload retcon prerequisite).
    #[test]
    fn live_diarization_relabel_persists_to_jsonl_and_replays_latest_attribution() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("live-diarization-persist-replay");
        let _guard = DataDirGuard::set(&dir);

        let app = AppState::new();
        let event_sink = RecordingDiarizationEventSink::default();
        let session_id = "live-diarization-persist-replay";
        let diarization_dispatch = DiarizationDispatchContext {
            event_sink: &event_sink,
            speaker_timeline: &app.speaker_timeline,
            knowledge_graph: &app.knowledge_graph,
            graph_snapshot: &app.graph_snapshot,
            transcript_ledger: &app.transcript_ledger,
            session_id,
        };

        // Shared span id: revision 2 supersedes revision 1 in place, so the
        // durable log carries the correction that replay must collapse to.
        let span_id = "local_clustering:session:0-1000:2";
        let provisional = events::DiarizationSpanRevisionPayload {
            span_id: span_id.to_string(),
            provider: "local_clustering".to_string(),
            timeline_id: "session".to_string(),
            source_id: None,
            speaker_id: Some("2".to_string()),
            speaker_label: Some("Speaker 2".to_string()),
            channel: None,
            start_time: 0.0,
            end_time: 1.0,
            confidence: Some(0.7),
            is_final: false,
            stability: DiarizationSpanStability::Provisional,
            revision_number: 1,
            supersedes: None,
            basis_asr_span_ids: vec![format!("{span_id}-asr")],
            basis_transcript_segment_ids: Vec::new(),
            raw_event_ref: None,
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms: 1_700_000_000_001,
        };
        let relabel = events::DiarizationSpanRevisionPayload {
            speaker_id: Some("alice".to_string()),
            speaker_label: Some("Alice".to_string()),
            confidence: Some(0.95),
            is_final: true,
            stability: DiarizationSpanStability::Stable,
            revision_number: 2,
            supersedes: Some(format!("{span_id}@rev1")),
            received_at_ms: 1_700_000_000_002,
            ..provisional.clone()
        };

        let first = emit_and_dispatch_diarization_span_revision(&diarization_dispatch, provisional);
        assert!(first.accepted);
        let second = emit_and_dispatch_diarization_span_revision(&diarization_dispatch, relabel);
        assert!(second.accepted);

        // The live in-memory timeline collapsed the relabel latest-wins.
        {
            let timeline = app
                .speaker_timeline
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            assert_eq!(timeline.latest_spans.len(), 1);
            assert_eq!(
                timeline.latest_spans[0].speaker_label.as_deref(),
                Some("Alice")
            );
            assert_eq!(timeline.latest_spans[0].revision_number, 2);
        }

        // The durable log carries BOTH revisions in append order (an immutable
        // event log, not a mutated snapshot).
        let repository = FileMemoryRepository::user_data();
        let persisted = repository
            .load_diarization_span_revisions(session_id)
            .expect("load persisted diarization revisions");
        assert_eq!(
            persisted.len(),
            2,
            "both live revisions must be durably appended to the speaker log"
        );
        assert_eq!(persisted[0].revision_number, 1);
        assert_eq!(persisted[0].speaker_label.as_deref(), Some("Speaker 2"));
        assert_eq!(persisted[1].revision_number, 2);
        assert_eq!(persisted[1].speaker_label.as_deref(), Some("Alice"));

        // Replaying the persisted log into a fresh SpeakerTimeline (the reload
        // path) must reflect the LATEST corrected attribution — the relabel is
        // not lost on reload.
        let replayed =
            SpeakerTimeline::replay(session_id, persisted).expect("replay persisted timeline");
        assert_eq!(replayed.accepted_event_count, 2);
        assert_eq!(
            replayed.latest_spans.len(),
            1,
            "relabel collapses by span id"
        );
        assert_eq!(
            replayed.latest_spans[0].speaker_id.as_deref(),
            Some("alice")
        );
        assert_eq!(
            replayed.latest_spans[0].speaker_label.as_deref(),
            Some("Alice")
        );
        assert_eq!(replayed.latest_spans[0].revision_number, 2);
        assert_eq!(
            replayed.latest_spans[0].stability,
            DiarizationEventStability::Stable
        );

        // The trait-level replay convenience folds the same way from disk.
        let replayed_via_trait = repository
            .replay_speaker_timeline(session_id)
            .expect("trait replay of persisted timeline");
        assert_eq!(
            replayed_via_trait.latest_spans[0].speaker_label.as_deref(),
            Some("Alice")
        );

        drain_app_writers(&app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Build a ledger-seeding ASR span with a given `span_id`/`start_time`/
    /// `received_at_ms`, otherwise filled with arbitrary-but-valid fields.
    /// Mirrors the fixtures `graph/temporal.rs` and `commands.rs` used to pin
    /// audio-graph-4b52.
    fn diarization_test_asr_span(
        span_id: &str,
        start_time: f64,
        received_at_ms: u64,
    ) -> events::AsrSpanRevisionPayload {
        events::AsrSpanRevisionPayload {
            span_id: span_id.to_string(),
            provider: "system".to_string(),
            source_id: "mic".to_string(),
            provider_item_id: None,
            transcript_segment_id: Some(format!("{span_id}-segment")),
            speaker_id: None,
            speaker_label: None,
            channel: None,
            text: "hello".to_string(),
            start_time,
            end_time: start_time + 1.0,
            confidence: 0.9,
            is_final: true,
            stability: events::AsrSpanStability::Final,
            revision_number: 1,
            supersedes: None,
            turn_id: None,
            end_of_turn: true,
            raw_event_ref: None,
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms,
        }
    }

    /// A provisional-then-relabel pair of diarization span revisions citing
    /// `basis_asr_span_ids: [anchor_span_id]`, mirroring the shape
    /// `emit_diarization_span_revision_for_transcript` /
    /// `emit_assemblyai_speaker_revision_with_dispatch` build in production
    /// (audio-graph-9b11).
    fn diarization_relabel_pair(
        span_id: &str,
        anchor_span_id: &str,
        start_time: f64,
        provisional_label: &str,
        canonical_label: &str,
    ) -> (
        events::DiarizationSpanRevisionPayload,
        events::DiarizationSpanRevisionPayload,
    ) {
        let provisional = events::DiarizationSpanRevisionPayload {
            span_id: span_id.to_string(),
            provider: "local_clustering".to_string(),
            timeline_id: "session".to_string(),
            source_id: None,
            speaker_id: Some(provisional_label.to_lowercase()),
            speaker_label: Some(provisional_label.to_string()),
            channel: None,
            start_time,
            end_time: start_time + 1.0,
            confidence: Some(0.7),
            is_final: false,
            stability: DiarizationSpanStability::Provisional,
            revision_number: 1,
            supersedes: None,
            basis_asr_span_ids: vec![anchor_span_id.to_string()],
            basis_transcript_segment_ids: Vec::new(),
            raw_event_ref: None,
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms: current_unix_millis(),
        };
        let relabel = events::DiarizationSpanRevisionPayload {
            speaker_id: Some(canonical_label.to_lowercase()),
            speaker_label: Some(canonical_label.to_string()),
            confidence: Some(0.95),
            is_final: true,
            stability: DiarizationSpanStability::Stable,
            revision_number: 2,
            supersedes: Some(revision_ref(span_id, 1)),
            received_at_ms: current_unix_millis(),
            ..provisional.clone()
        };
        (provisional, relabel)
    }

    /// Seed `graph` with a provisional speaker node ("Speaker 2") related to
    /// "Acme", plus an already-known canonical identity ("Alice") — the same
    /// shape `assemblyai_speaker_revision_emission_retcons_graph_on_label_remap`
    /// and `graph/temporal.rs`'s audio-graph-4b52 tests use, so a diarization
    /// relabel of "Speaker 2" -> "Alice" retcons exactly one edge.
    fn seed_provisional_speaker_graph(graph: &mut crate::graph::temporal::TemporalKnowledgeGraph) {
        use crate::graph::entities::{ExtractedEntity, ExtractedRelation, ExtractionResult};
        graph.process_extraction(
            &ExtractionResult {
                entities: vec![
                    ExtractedEntity {
                        name: "Speaker 2".to_string(),
                        entity_type: "Person".to_string(),
                        description: None,
                    },
                    ExtractedEntity {
                        name: "Acme".to_string(),
                        entity_type: "Organization".to_string(),
                        description: None,
                    },
                    ExtractedEntity {
                        name: "Alice".to_string(),
                        entity_type: "Person".to_string(),
                        description: None,
                    },
                ],
                relations: vec![ExtractedRelation {
                    source: "Speaker 2".to_string(),
                    target: "Acme".to_string(),
                    relation_type: "works_at".to_string(),
                    detail: None,
                }],
            },
            1.0,
            "Speaker 2",
            "seg-1",
        );
        let _ = graph.take_delta();
    }

    /// audio-graph-9b11 acceptance (fix for the bug the 4b52 fix escalated):
    /// a live diarization relabel dispatched through
    /// `emit_and_dispatch_diarization_span_revision` must call
    /// `supersede_entity` with a SESSION-RELATIVE timestamp derived from the
    /// revision's own `basis_asr_span_ids` anchor via
    /// `TranscriptLedger::session_relative_timestamp` — not raw
    /// `current_unix_millis() as f64 / 1000.0` (epoch scale). Master's bug:
    /// left as epoch seconds, the re-pointed edge's `valid_from` is always the
    /// graph's maximum, so `evict_excess_edges`'s `min_by` (graph/temporal.rs)
    /// can never select it for eviction — immortal.
    ///
    /// The ledger is seeded with TWO spans: a DECOY sorted first (very
    /// negative `start_time`, so `.first()`-style fallback would land far from
    /// the truth) and the EXACT anchor the revision cites via
    /// `basis_asr_span_ids`. This is deliberate: if the wiring ever regresses
    /// to passing `""` (or otherwise drops the revision's own anchor id)
    /// instead of `diarization_revision_anchor_id(&revision)`, the result
    /// silently lands on the decoy instead of failing to compile — this test
    /// is what catches that class of regression.
    #[test]
    fn live_diarization_retcon_writes_session_relative_valid_from() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("live-diarization-retcon-session-relative");
        let _guard = DataDirGuard::set(&dir);

        let app = AppState::new();
        let event_sink = RecordingDiarizationEventSink::default();
        let session_id = "live-diarization-retcon-session-relative";
        let diarization_dispatch = DiarizationDispatchContext {
            event_sink: &event_sink,
            speaker_timeline: &app.speaker_timeline,
            knowledge_graph: &app.knowledge_graph,
            graph_snapshot: &app.graph_snapshot,
            transcript_ledger: &app.transcript_ledger,
            session_id,
        };

        let anchor_span_id = "asr-span-anchor";
        {
            let mut ledger = app
                .transcript_ledger
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            // Decoy: sorts first (very negative start_time) and is NOT what the
            // revision cites.
            ledger
                .apply_event(crate::projections::TranscriptEvent::from(
                    diarization_test_asr_span("decoy-span", -500.0, 1_800_000_000_000),
                ))
                .expect("seed decoy span");
            // The exact anchor the revision's `basis_asr_span_ids` cites: 5s
            // into the session, recorded "now" so the wall-clock offset to
            // `current_unix_millis()` at dispatch time stays tiny.
            ledger
                .apply_event(crate::projections::TranscriptEvent::from(
                    diarization_test_asr_span(anchor_span_id, 5.0, current_unix_millis()),
                ))
                .expect("seed exact-anchor span");
        }

        {
            let mut graph = app
                .knowledge_graph
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            seed_provisional_speaker_graph(&mut graph);
        }

        let (provisional, relabel) = diarization_relabel_pair(
            "diarization-span-1",
            anchor_span_id,
            5.0,
            "Speaker 2",
            "Alice",
        );
        let first = emit_and_dispatch_diarization_span_revision(&diarization_dispatch, provisional);
        assert!(first.accepted);
        assert!(
            !first.retcon_fired,
            "first-seen provisional should not retcon"
        );

        let second = emit_and_dispatch_diarization_span_revision(&diarization_dispatch, relabel);
        assert!(second.accepted);
        assert!(second.retcon_fired);
        assert_eq!(second.edges_retconned, 1);

        let live_from = app
            .knowledge_graph
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .live_edge_valid_from_for_test("Alice", "Acme", "works_at")
            .expect("the re-pointed live edge should exist");
        assert!(
            (live_from - 5.0).abs() < 5.0,
            "expected session-relative seconds near the exact anchor's \
             start_time (5.0), got {live_from} (looks like raw epoch seconds \
             leaked through, or the decoy span was used instead of the \
             revision's own basis_asr_span_ids anchor)"
        );

        drain_app_writers(&app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// audio-graph-9b11: the SAME bug class/fix as
    /// `graph/temporal.rs`'s `supersede_entity_repointed_edge_evicts_on_same_terms_as_live_path_edge`
    /// (audio-graph-4b52), but proven through the LIVE diarization-dispatch
    /// entry point instead of a direct `supersede_entity` call — this is what
    /// actually exercises the `DiarizationDispatchContext` wiring under test,
    /// not just the ledger helper it calls into.
    #[test]
    fn live_diarization_retcon_edge_evicts_on_same_terms_as_live_path_edge() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("live-diarization-retcon-edge-eviction");
        let _guard = DataDirGuard::set(&dir);

        let app = AppState::new();
        let event_sink = RecordingDiarizationEventSink::default();
        let session_id = "live-diarization-retcon-edge-eviction";
        let diarization_dispatch = DiarizationDispatchContext {
            event_sink: &event_sink,
            speaker_timeline: &app.speaker_timeline,
            knowledge_graph: &app.knowledge_graph,
            graph_snapshot: &app.graph_snapshot,
            transcript_ledger: &app.transcript_ledger,
            session_id,
        };

        let anchor_span_id = "asr-span-anchor";
        {
            let mut ledger = app
                .transcript_ledger
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            ledger
                .apply_event(crate::projections::TranscriptEvent::from(
                    diarization_test_asr_span(anchor_span_id, 5.0, current_unix_millis()),
                ))
                .expect("seed exact-anchor span");
        }

        {
            let mut graph = app
                .knowledge_graph
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            seed_provisional_speaker_graph(&mut graph);
        }

        let (provisional, relabel) = diarization_relabel_pair(
            "diarization-span-1",
            anchor_span_id,
            5.0,
            "Speaker 2",
            "Alice",
        );
        let first = emit_and_dispatch_diarization_span_revision(&diarization_dispatch, provisional);
        assert!(first.accepted);
        let second = emit_and_dispatch_diarization_span_revision(&diarization_dispatch, relabel);
        assert!(second.accepted);
        assert_eq!(second.edges_retconned, 1);

        // Fill the graph with MAX_EDGES more live-path edges between the SAME
        // two surviving nodes (distinct relation types, so each is a new edge
        // rather than a weight-fold), each newer (larger `valid_from`) than
        // the retcon timestamp (~5.0), to push the graph past the eviction
        // threshold — mirrors `graph/temporal.rs`'s
        // `supersede_entity_repointed_edge_evicts_on_same_terms_as_live_path_edge`.
        {
            use crate::graph::entities::{ExtractedRelation, ExtractionResult};
            let mut graph = app
                .knowledge_graph
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            for i in 0..crate::graph::temporal::MAX_EDGES {
                graph.process_extraction(
                    &ExtractionResult {
                        entities: vec![],
                        relations: vec![ExtractedRelation {
                            source: "Alice".to_string(),
                            target: "Acme".to_string(),
                            relation_type: format!("filler-{i}"),
                            detail: None,
                        }],
                    },
                    100.0 + i as f64,
                    "spk",
                    &format!("seg-live-{i}"),
                );
            }
        }

        let graph = app
            .knowledge_graph
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        assert!(
            graph
                .live_edge_valid_from_for_test("Alice", "Acme", "works_at")
                .is_none(),
            "the live-retconned edge must be evictable on the same terms as a \
             live-path edge, not immortal under epoch-scale timestamps"
        );
        assert_eq!(graph.edge_count(), crate::graph::temporal::MAX_EDGES);
        drop(graph);

        drain_app_writers(&app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// audio-graph-9b11 design care: when a live diarization revision cites a
    /// span/segment id that is NOT in the ledger (ledger rotation race, or a
    /// revision producer whose basis ids never made it into the ledger), the
    /// wiring must fall through to `session_relative_timestamp`'s any-span
    /// fallback tier — landing near whatever span IS in the ledger — rather
    /// than silently degrading to epoch seconds or panicking. This pins that
    /// fallback behavior for the live dispatch path specifically (the ledger
    /// helper's own fallback tiers are already unit-tested in
    /// `projections.rs`).
    #[test]
    fn live_diarization_retcon_falls_back_to_any_span_anchor_when_cited_span_is_absent() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("live-diarization-retcon-fallback-anchor");
        let _guard = DataDirGuard::set(&dir);

        let app = AppState::new();
        let event_sink = RecordingDiarizationEventSink::default();
        let session_id = "live-diarization-retcon-fallback-anchor";
        let diarization_dispatch = DiarizationDispatchContext {
            event_sink: &event_sink,
            speaker_timeline: &app.speaker_timeline,
            knowledge_graph: &app.knowledge_graph,
            graph_snapshot: &app.graph_snapshot,
            transcript_ledger: &app.transcript_ledger,
            session_id,
        };

        // The ledger has exactly one span, 9s into the session — but the
        // revision below cites a DIFFERENT span id that never made it into
        // the ledger.
        {
            let mut ledger = app
                .transcript_ledger
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            ledger
                .apply_event(crate::projections::TranscriptEvent::from(
                    diarization_test_asr_span(
                        "the-only-span-in-the-ledger",
                        9.0,
                        current_unix_millis(),
                    ),
                ))
                .expect("seed the only span");
        }

        {
            let mut graph = app
                .knowledge_graph
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            seed_provisional_speaker_graph(&mut graph);
        }

        let (provisional, relabel) = diarization_relabel_pair(
            "diarization-span-1",
            "span-id-never-recorded-in-the-ledger",
            9.0,
            "Speaker 2",
            "Alice",
        );
        let first = emit_and_dispatch_diarization_span_revision(&diarization_dispatch, provisional);
        assert!(first.accepted);
        let second = emit_and_dispatch_diarization_span_revision(&diarization_dispatch, relabel);
        assert!(second.accepted);
        assert_eq!(second.edges_retconned, 1);

        let live_from = app
            .knowledge_graph
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .live_edge_valid_from_for_test("Alice", "Acme", "works_at")
            .expect("the re-pointed live edge should exist");
        assert!(
            (live_from - 9.0).abs() < 5.0,
            "expected the any-span fallback anchor's start_time (9.0), got \
             {live_from} (should fall back to the one span in the ledger, \
             not 0.0 / a panic / epoch seconds)"
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
        let session_id = "session-1";
        let ledger = Arc::new(Mutex::new(TranscriptLedger::new(session_id)));
        let writer_fixture = AcceptingTranscriptEventWriterFixture::new(session_id);
        let writer = writer_fixture.writer();
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
    fn asr_partial_revision_missing_writer_does_not_advance_ledger() {
        let session_id = "session-asr-missing-writer";
        let ledger = Arc::new(Mutex::new(TranscriptLedger::new(session_id)));
        let writer = Arc::new(Mutex::new(None));
        let payload = projection_asr_payload(
            "projection-missing-writer-span",
            1,
            "missing writer must reject this revision",
            false,
        );

        assert!(
            !record_asr_span_revision_event(&ledger, &writer, &payload),
            "an unpoisoned missing writer cannot prove canonical acceptance"
        );
        let guard = ledger.lock().unwrap();
        assert_eq!(guard.accepted_event_count, 0);
        assert!(
            guard.latest_spans.is_empty(),
            "ledger must not advance without a canonical writer"
        );
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
        let session_id = "session-1";
        let ledger = Arc::new(Mutex::new(TranscriptLedger::new(session_id)));
        let writer_fixture = AcceptingTranscriptEventWriterFixture::new(session_id);
        let writer = writer_fixture.writer();
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
            FnProjectionPatchGenerator::new(|job, _ledger, _notes, sequence, created_at_ms| {
                Ok(ProjectionPatchOutcome {
                    patch: test_projection_patch(&job, sequence, created_at_ms),
                    tokens_used: 37,
                    no_op_filtered_count: 0,
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

    /// Ticket W3 (audio-graph-a6b5): the additive, event-payload-only
    /// `basis_currency_at_apply` field. Reuses this file's own
    /// `runtime_projection_dispatch_applies_fake_notes_and_graph_patches`
    /// real-dispatch harness (a same-basis apply through the LIVE
    /// `run_projection_job` wiring, not a synthetic construction) to prove
    /// the field is populated from the REAL apply-gate classification on the
    /// frontend-bound emit clone, and NEVER on the value persisted to the
    /// canonical projection event log.
    ///
    /// Mutation-proof: a mutant that drops the
    /// `emitted_patch.basis_currency_at_apply = Some(...)` assignment in
    /// `run_projection_job` (leaving the field `None` on every emit) makes
    /// the first loop's `assert_eq!` fail. A mutant that instead sets the
    /// field on `patch` (the value moved into
    /// `apply_runtime_projection_patch`, hence persisted) rather than on the
    /// separate `emitted_patch` clone makes the second loop's `assert_eq!`
    /// fail — the persisted log would carry `Some` instead of `None`.
    #[test]
    fn runtime_projection_dispatch_populates_basis_currency_only_on_the_emitted_patch_not_the_persisted_log()
     {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("projection-dispatch-basis-currency");
        let _guard = DataDirGuard::set(&dir);

        let app = AppState::new();
        let session_id = app.current_session_id();
        let (generator, _calls) =
            FnProjectionPatchGenerator::new(|job, _ledger, _notes, sequence, created_at_ms| {
                Ok(ProjectionPatchOutcome {
                    patch: test_projection_patch(&job, sequence, created_at_ms),
                    tokens_used: 37,
                    no_op_filtered_count: 0,
                })
            });
        let (dispatch, event_sink) = projection_dispatch_for_app(&app, generator);
        let writer = app.transcript_event_writer.clone();
        let final_revision =
            projection_asr_payload("projection-basis-currency-span", 1, "Alice met Bob.", true);

        assert!(record_asr_span_revision_event_and_observe_projection(
            &app.transcript_ledger,
            &writer,
            &app.projection_schedulers,
            Some(&dispatch),
            &final_revision
        ));

        wait_until("notes and graph projection dispatch success", || {
            event_sink.patch_count() == 2
        });

        let emitted = event_sink.patches_snapshot();
        assert_eq!(emitted.len(), 2);
        for patch in &emitted {
            assert_eq!(
                patch.basis_currency_at_apply,
                Some(AppliedBasisCurrency::Current),
                "the apply-success emit site must populate this field from \
                 the real classification the apply gate returned — kind={:?}",
                patch.kind
            );
        }

        drain_app_writers(&app);

        let persisted = load_projection_events(&session_id).expect("load projection events");
        assert_eq!(persisted.len(), 2);
        for event in &persisted {
            assert_eq!(
                event.basis_currency_at_apply, None,
                "the PERSISTED canonical log must never gain this field's \
                 value — it is populated only on the frontend-bound emit \
                 clone, never on the value that gets written to disk \
                 (kind={:?})",
                event.kind
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Reviewer adversarial-mutation finding (W3 fix round): the pin above
    /// exercises only a same-basis apply, whose real gate classification is
    /// `AppliedBasisCurrency::Current` — so a mutant that hardcodes
    /// `emitted_patch.basis_currency_at_apply = Some(AppliedBasisCurrency::Current)`
    /// (dropping the `result.basis_currency_at_apply.clone()` re-derivation
    /// entirely) still passes it, and passed the full `cargo test --lib`
    /// suite. This test forces the REAL apply gate to classify as
    /// `AppendedTail` — a new, never-revising span landing on the LIVE
    /// `app.transcript_ledger` strictly between the job's basis being pinned
    /// (`observe_ledger`, synchronous inside
    /// `record_asr_span_revision_event_and_observe_projection`, before the
    /// worker thread carrying this job is even spawned — see
    /// `run_projection_job_notes_snapshot_is_pinned_at_spawn_not_re_read_at_dispatch`)
    /// and the apply call re-reading the ledger fresh
    /// (`transcript_ledger_snapshot`, `state.rs`). The mutation happens
    /// inside the `FnProjectionPatchGenerator` closure itself — the same
    /// "generation took long enough for a new span to land" window
    /// `state.rs`'s own
    /// `runtime_projection_patch_applies_append_only_basis_with_persistence`
    /// exercises synthetically, reproduced here through the real
    /// `run_projection_job` dispatch. A `swap`-guarded `AtomicBool` keeps the
    /// ledger mutation idempotent even though the closure fires once per
    /// dispatched kind (notes AND graph) for this one ASR revision.
    #[test]
    fn runtime_projection_dispatch_emits_appended_tail_when_the_live_ledger_grows_between_job_basis_and_apply()
     {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("projection-dispatch-basis-currency-appended-tail");
        let _guard = DataDirGuard::set(&dir);

        let app = AppState::new();
        let session_id = app.current_session_id();
        let live_ledger = app.transcript_ledger.clone();
        let ledger_grown = Arc::new(AtomicBool::new(false));
        let ledger_grown_for_closure = ledger_grown.clone();
        let (generator, _calls) = FnProjectionPatchGenerator::new(
            move |job, _ledger, _notes, sequence, created_at_ms| {
                if !ledger_grown_for_closure.swap(true, Ordering::SeqCst) {
                    let mut ledger = match live_ledger.lock() {
                        Ok(ledger) => ledger,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    ledger
                        .apply_event(crate::projections::TranscriptEvent {
                            span_id: "projection-basis-currency-tail-span-2".into(),
                            provider: "test".into(),
                            source_id: "test-source".into(),
                            provider_item_id: None,
                            transcript_segment_id: Some(
                                "segment-projection-basis-currency-tail-span-2".into(),
                            ),
                            speaker_id: Some("speaker-1".into()),
                            speaker_label: Some("Speaker 1".into()),
                            channel: None,
                            text: "Carol joined too.".into(),
                            start_time: 5.0,
                            end_time: 6.0,
                            confidence: 1.0,
                            is_final: true,
                            stability: crate::projections::TranscriptEventStability::Final,
                            revision_number: 1,
                            supersedes: None,
                            turn_id: None,
                            end_of_turn: true,
                            raw_event_ref: None,
                            capture_latency_ms: None,
                            asr_latency_ms: None,
                            received_at_ms: 1_700_000_000_500,
                        })
                        .expect("append the live-ledger tail span between basis-pin and apply");
                }
                Ok(ProjectionPatchOutcome {
                    patch: test_projection_patch(&job, sequence, created_at_ms),
                    tokens_used: 37,
                    no_op_filtered_count: 0,
                })
            },
        );
        let (dispatch, event_sink) = projection_dispatch_for_app(&app, generator);
        let writer = app.transcript_event_writer.clone();
        let final_revision = projection_asr_payload(
            "projection-basis-currency-tail-span-1",
            1,
            "Alice met Bob.",
            true,
        );

        assert!(record_asr_span_revision_event_and_observe_projection(
            &app.transcript_ledger,
            &writer,
            &app.projection_schedulers,
            Some(&dispatch),
            &final_revision
        ));

        // `>= 2`, not `== 2`: an `AppendedTail` completion unconditionally
        // chains a follow-up job per lane (ADR-0045 decision 3/4 — see this
        // file's `dispatch_projection_decision` comments) to actually
        // project the newly-appended span, so this scenario legitimately
        // emits MORE than 2 patches once those follow-ups land. Racing to
        // read `patches_snapshot()` at the instant `patch_count()` first
        // hits exactly 2 is exactly what this test must NOT depend on — the
        // property under test is about the FIRST patch per kind (the one
        // whose basis is `job.basis`, pinned before this test's mutation),
        // not about the total count.
        wait_until(
            "notes and graph projection dispatch success (appended tail)",
            || event_sink.patch_count() >= 2,
        );

        let emitted = event_sink.patches_snapshot();
        assert!(
            emitted.len() >= 2,
            "expected at least the two first-round patches, got {}",
            emitted.len()
        );
        for kind in [ProjectionKind::Notes, ProjectionKind::Graph] {
            let first_for_kind = emitted
                .iter()
                .filter(|patch| patch.kind == kind)
                .min_by_key(|patch| patch.sequence)
                .unwrap_or_else(|| panic!("no emitted patch for kind={kind:?}"));
            assert!(
                matches!(
                    first_for_kind.basis_currency_at_apply,
                    Some(AppliedBasisCurrency::AppendedTail { .. })
                ),
                "a live-ledger append strictly between job-basis capture and \
                 apply must classify through the REAL gate as AppendedTail, \
                 not a hardcoded Current — got {:?} for kind={:?}",
                first_for_kind.basis_currency_at_apply,
                kind
            );
        }

        drain_app_writers(&app);

        let persisted = load_projection_events(&session_id).expect("load projection events");
        assert!(
            persisted.len() >= 2,
            "expected at least the two first-round persisted events, got {}",
            persisted.len()
        );
        // Every apply this session persists — first-round AppendedTail
        // applies AND their unconditional follow-ups alike — must carry
        // `None` here; the invariant is about which VALUE flows into
        // persistence, not about which round produced it.
        for event in &persisted {
            assert_eq!(
                event.basis_currency_at_apply, None,
                "the PERSISTED canonical log must still never gain this \
                 field's value, even on the AppendedTail apply path \
                 (kind={:?})",
                event.kind
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Reviewer adversarial-mutation finding (W2 fix round): a mutant that made
    /// `run_projection_job` silently `finish_projection_scheduler_job` +
    /// `return` on `outcome.patch.operations.is_empty()` — WITHOUT ever calling
    /// `apply_runtime_projection_patch` — survived the full 2121-test suite.
    /// `projection_scheduler.rs`'s
    /// `empty_ops_projection_patch_advances_the_coverage_head_through_derive_and_apply`
    /// already pins the two PURE halves (`derive_coverage_heads` picking the
    /// empty patch's higher-sequence basis, and `MaterializedNotes::apply_patch`
    /// committing `last_sequence` unconditionally); this test is the missing
    /// third pin — it drives an `operations: []` outcome through the LIVE
    /// `run_projection_job` wiring and asserts the basis actually lands in both
    /// the in-memory materialized state AND the durable accepted-events log.
    /// Without this durable advance, a session reopen would re-derive
    /// `derive_coverage_heads` from the pre-empty-patch basis and re-send the
    /// same already-fully-covered span forever.
    #[test]
    fn empty_ops_projection_patch_still_advances_last_sequence_and_the_accepted_log_through_run_projection_job()
     {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("projection-empty-ops-dispatch-live-wiring");
        let _guard = DataDirGuard::set(&dir);

        let app = AppState::new();
        let session_id = app.current_session_id();
        let notes_job = {
            let mut ledger = app
                .transcript_ledger
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            ledger
                .apply_event(crate::projections::TranscriptEvent::from(
                    projection_asr_payload(
                        "projection-empty-ops-span",
                        1,
                        "Everything the model said here got filtered as a no-op.",
                        true,
                    ),
                ))
                .expect("seed transcript");
            let mut schedulers = app
                .projection_schedulers
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            match schedulers.observe_ledger(&ledger, 10).notes {
                ProjectionSchedulerDecision::StartJob { job } => job,
                other => panic!("expected notes job, got {other:?}"),
            }
        };

        let (generator, calls) =
            FnProjectionPatchGenerator::new(|job, _ledger, _notes, sequence, created_at_ms| {
                let mut patch = test_projection_patch(&job, sequence, created_at_ms);
                patch.operations.clear();
                Ok(ProjectionPatchOutcome {
                    patch,
                    tokens_used: 11,
                    no_op_filtered_count: 1,
                })
            });
        let (dispatch, event_sink) = projection_dispatch_for_app(&app, generator);

        run_projection_job(dispatch, notes_job);

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        {
            let materialized = app
                .materialized_projection_state
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            assert_eq!(
                materialized.notes.last_sequence, 1,
                "an all-filtered empty patch must still commit its basis so the \
                 next tick does not re-derive and re-send the same span"
            );
            assert!(
                materialized.notes.notes.is_empty(),
                "an empty-ops patch must not create or mutate any note"
            );
        }

        assert_eq!(event_sink.patch_count(), 1);
        assert_eq!(event_sink.notes_count(), 1);

        {
            let schedulers = app
                .projection_schedulers
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            assert_eq!(schedulers.notes().metrics().completed_jobs, 1);
            assert_eq!(schedulers.notes().metrics().accepted_patches, 1);
            assert!(schedulers.notes().in_flight_job().is_none());
        }

        drain_app_writers(&app);
        let notes = load_materialized_notes(&session_id)
            .expect("load notes")
            .expect("notes artifact");
        assert_eq!(notes.last_sequence, 1);
        assert!(notes.notes.is_empty());
        let events = load_projection_events(&session_id).expect("load projection events");
        assert_eq!(
            events.len(),
            1,
            "the accepted log must durably record the empty patch so a session \
             reopen does not reseed to the pre-empty-patch basis"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn runtime_projection_dispatch_discards_same_session_replaced_worker() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("projection-dispatch-same-session-replacement");
        let _guard = DataDirGuard::set(&dir);

        let app = AppState::new();
        let session_id = app.current_session_id();
        let old_job = {
            let mut ledger = app
                .transcript_ledger
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            ledger
                .apply_event(crate::projections::TranscriptEvent::from(
                    projection_asr_payload(
                        "projection-replaced-span",
                        1,
                        "This old worker must not apply.",
                        true,
                    ),
                ))
                .expect("seed transcript");
            let mut schedulers = app
                .projection_schedulers
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            match schedulers.observe_ledger(&ledger, 10).notes {
                ProjectionSchedulerDecision::StartJob { job } => job,
                other => panic!("expected old notes job, got {other:?}"),
            }
        };
        let old_job_id = old_job.id.clone();
        let schedulers_for_reset = app.projection_schedulers.clone();
        let ledger_for_reset = app.transcript_ledger.clone();
        let (generator, _calls) = FnProjectionPatchGenerator::new(
            move |job, _ledger, _notes, sequence, created_at_ms| {
                let ledger = ledger_for_reset
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .clone();
                let replacement_id = {
                    let mut schedulers = schedulers_for_reset
                        .lock()
                        .unwrap_or_else(|p| p.into_inner());
                    schedulers.reset(job.session_id.clone());
                    match schedulers.observe_ledger(&ledger, 20).notes {
                        ProjectionSchedulerDecision::StartJob { job } => job.id,
                        other => panic!("expected replacement notes job, got {other:?}"),
                    }
                };
                assert_ne!(replacement_id, job.id);
                Ok(ProjectionPatchOutcome {
                    patch: test_projection_patch(&job, sequence, created_at_ms),
                    tokens_used: 73,
                    no_op_filtered_count: 0,
                })
            },
        );
        let (dispatch, event_sink) = projection_dispatch_for_app(&app, generator);

        run_projection_job(dispatch, old_job);

        let materialized = app
            .materialized_projection_state
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        assert!(materialized.notes.notes.is_empty());
        drop(materialized);
        assert_eq!(event_sink.patch_count(), 0);
        assert_eq!(event_sink.notes_count(), 0);
        let schedulers = app
            .projection_schedulers
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let replacement_id = schedulers
            .notes()
            .in_flight_job()
            .map(|job| job.id.clone())
            .expect("replacement remains active");
        assert_ne!(replacement_id, old_job_id);
        assert_eq!(schedulers.notes().metrics().tokens_used, 0);
        drop(schedulers);

        drain_app_writers(&app);
        assert!(
            load_projection_events(&session_id)
                .expect("load projection events")
                .is_empty(),
            "superseded worker must not append a projection event"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// audio-graph-9cc1 / ADR-0045 decision 4 (drain half) acceptance: a
    /// projection job thread registers itself in `projection_job_workers` when
    /// spawned and self-deregisters on normal completion, leaving the registry
    /// empty — proven for BOTH kinds, since the graph lane previously had no
    /// tracked handle at all (spawn_projection_job discarded it).
    #[test]
    fn spawn_projection_job_registers_and_self_deregisters_on_normal_completion_for_both_kinds() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("projection-job-registry-self-deregister");
        let _guard = DataDirGuard::set(&dir);

        let app = AppState::new();
        let observation = {
            let mut ledger = app
                .transcript_ledger
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            ledger
                .apply_event(crate::projections::TranscriptEvent::from(
                    projection_asr_payload(
                        "registry-self-deregister-span",
                        1,
                        "Both projection lanes must self-deregister.",
                        true,
                    ),
                ))
                .expect("seed transcript");
            let mut schedulers = app
                .projection_schedulers
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            schedulers.observe_ledger(&ledger, 10)
        };
        let notes_job = match observation.notes {
            ProjectionSchedulerDecision::StartJob { job } => job,
            other => panic!("expected notes job, got {other:?}"),
        };
        let graph_job = match observation.graph {
            ProjectionSchedulerDecision::StartJob { job } => job,
            other => panic!("expected graph job, got {other:?}"),
        };

        let (generator, calls) =
            FnProjectionPatchGenerator::new(|job, _ledger, _notes, sequence, created_at_ms| {
                Ok(ProjectionPatchOutcome {
                    patch: test_projection_patch(&job, sequence, created_at_ms),
                    tokens_used: 5,
                    no_op_filtered_count: 0,
                })
            });
        let (dispatch, event_sink) = projection_dispatch_for_app(&app, generator);

        for job in [notes_job, graph_job] {
            let job_kind = job.kind.clone();
            spawn_projection_job(dispatch.clone(), job);

            // Poll for self-deregistration rather than asserting immediately —
            // the job runs on its own thread, so the only durable guarantee is
            // "eventually empty", proven within a generous test budget.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                let empty = app
                    .projection_job_workers
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .is_empty();
                if empty {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "{job_kind:?} projection job did not self-deregister within the test budget"
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }

        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "both the notes and the graph job must have actually run"
        );
        assert_eq!(event_sink.patch_count(), 2);
        assert!(
            app.projection_job_workers
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .is_empty(),
            "registry must be empty after both jobs self-deregister"
        );

        drain_app_writers(&app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// audio-graph-9cc1 / ADR-0045 decision 4 (adversarial-review fix): a job
    /// that completes WHILE Stop is draining the registry must not chain a
    /// follow-up job past Stop. `complete_graph_in_flight`'s `AppendOnlyStale`
    /// arm starts a follow-up job UNCONDITIONALLY on exactly this shape of
    /// completion — a final ASR revision landed while the job was in flight,
    /// which is the seed's own motivating continuous-speech scenario.
    /// Reproduces the exact interleaving from the finding: while job A is
    /// generating, a new final span lands (moving the ledger past A's basis)
    /// AND Stop begins (`projection_lane_stopping` set) — both before A's own
    /// completion tail (running on A's thread) reaches
    /// `dispatch_projection_decision` and decides whether to chain B. Without
    /// gating that dispatch on the flag, B is spawned and registered strictly
    /// after any single `mem::take` drain snapshot could have already been
    /// taken, so it outlives Stop unbounded and unfenced.
    #[test]
    fn dispatch_projection_decision_refuses_a_follow_up_job_once_stopping() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("projection-lane-stopping-refuses-follow-up");
        let _guard = DataDirGuard::set(&dir);

        let app = AppState::new();
        let job_a = {
            let mut ledger = app
                .transcript_ledger
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            ledger
                .apply_event(crate::projections::TranscriptEvent::from(
                    projection_asr_payload(
                        "stopping-race-span-1",
                        1,
                        "First utterance before Stop races the drain.",
                        true,
                    ),
                ))
                .expect("seed first span");
            let mut schedulers = app
                .projection_schedulers
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            match schedulers.observe_ledger(&ledger, 10).graph {
                ProjectionSchedulerDecision::StartJob { job } => job,
                other => panic!("expected graph StartJob, got {other:?}"),
            }
        };

        // Mid-generation, mimic the race: a final ASR revision lands (a NEW
        // span, so job A's completed basis classifies as `AppendOnlyStale`,
        // not `Revised`) and Stop begins — both BEFORE job A's own tail
        // dispatches the mandatory follow-up. `span_appended` bounds the
        // cascade to at most one extra hop even if the gate under test were
        // ever reverted, so this test cannot hang.
        let ledger_handle = app.transcript_ledger.clone();
        let stopping_flag = app.projection_lane_stopping.clone();
        let span_appended = Arc::new(AtomicBool::new(false));
        let span_appended_in_closure = span_appended.clone();
        let (generator, calls) = FnProjectionPatchGenerator::new(
            move |job, _ledger, _notes, sequence, created_at_ms| {
                if !span_appended_in_closure.swap(true, Ordering::SeqCst) {
                    ledger_handle
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .apply_event(crate::projections::TranscriptEvent::from(
                            projection_asr_payload(
                                "stopping-race-span-2",
                                1,
                                "Second utterance lands while A is mid-generation.",
                                true,
                            ),
                        ))
                        .expect("append second span mid-generation");
                    stopping_flag.store(true, Ordering::SeqCst);
                }
                Ok(ProjectionPatchOutcome {
                    patch: test_projection_patch(&job, sequence, created_at_ms),
                    tokens_used: 5,
                    no_op_filtered_count: 0,
                })
            },
        );
        let (dispatch, event_sink) = projection_dispatch_for_app(&app, generator);

        spawn_projection_job(dispatch, job_a);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let empty = app
                .projection_job_workers
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .is_empty();
            if empty {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "job A did not self-deregister within the test budget"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        // A chained follow-up job would register into the SAME registry we
        // just observed empty; give it a further window to appear before
        // asserting it never did. This is the discriminating check: a
        // reverted gate spawns and runs a second (follow-up) job here.
        std::thread::sleep(std::time::Duration::from_millis(150));

        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "a follow-up job must never be spawned once projection_lane_stopping is set, even \
             though the completing job's AppendOnlyStale basis unconditionally asks the \
             scheduler to start one"
        );
        assert!(
            app.projection_job_workers
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .is_empty(),
            "registry must stay empty — no follow-up thread should ever have been registered"
        );
        assert_eq!(
            event_sink.patch_count(),
            1,
            "only job A's patch should ever have applied"
        );

        drain_app_writers(&app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// audio-graph-1609 acceptance (a): the exact phantom-wedge construction
    /// from the sibling test above (`..._refuses_a_follow_up_job_once_stopping`),
    /// continued past the discard into a same-session restart. Before the
    /// fix, the discarded follow-up's `start_job` call left it recorded as
    /// the graph lane's `in_flight` job forever — no thread would ever
    /// complete or fail it, so restarting on the SAME session (flag
    /// cleared, no `rotate_session`, exactly what the seed's mechanism
    /// describes) would see `in_flight.is_some()` on every subsequent
    /// `observe_ledger` and return `Coalesced` behind a job that was never
    /// actually running — the lane would never project again for the rest
    /// of the session. This test drives past that restart and asserts the
    /// opposite: a new final produces a `StartJob`, not `Coalesced`, and
    /// dispatching it actually runs and applies a second patch.
    #[test]
    fn dispatch_projection_decision_abandon_lets_the_lane_project_again_after_restart() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("projection-lane-stopping-abandon-then-restart");
        let _guard = DataDirGuard::set(&dir);

        let app = AppState::new();
        let job_a = {
            let mut ledger = app
                .transcript_ledger
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            ledger
                .apply_event(crate::projections::TranscriptEvent::from(
                    projection_asr_payload(
                        "abandon-restart-span-1",
                        1,
                        "First utterance before Stop races the drain.",
                        true,
                    ),
                ))
                .expect("seed first span");
            let mut schedulers = app
                .projection_schedulers
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            match schedulers.observe_ledger(&ledger, 10).graph {
                ProjectionSchedulerDecision::StartJob { job } => job,
                other => panic!("expected graph StartJob, got {other:?}"),
            }
        };

        // Same race construction as the sibling test: a final ASR revision
        // lands mid-generation (job A's completion classifies AppendOnlyStale,
        // starting a mandatory follow-up) AND Stop begins, both before job
        // A's own completion tail reaches `dispatch_projection_decision`.
        let ledger_handle = app.transcript_ledger.clone();
        let stopping_flag = app.projection_lane_stopping.clone();
        let span_appended = Arc::new(AtomicBool::new(false));
        let span_appended_in_closure = span_appended.clone();
        let (generator, calls) = FnProjectionPatchGenerator::new(
            move |job, _ledger, _notes, sequence, created_at_ms| {
                if !span_appended_in_closure.swap(true, Ordering::SeqCst) {
                    ledger_handle
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .apply_event(crate::projections::TranscriptEvent::from(
                            projection_asr_payload(
                                "abandon-restart-span-2",
                                1,
                                "Second utterance lands while A is mid-generation.",
                                true,
                            ),
                        ))
                        .expect("append second span mid-generation");
                    stopping_flag.store(true, Ordering::SeqCst);
                }
                Ok(ProjectionPatchOutcome {
                    patch: test_projection_patch(&job, sequence, created_at_ms),
                    tokens_used: 5,
                    no_op_filtered_count: 0,
                })
            },
        );
        let (dispatch, event_sink) = projection_dispatch_for_app(&app, generator);

        spawn_projection_job(dispatch.clone(), job_a);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let empty = app
                .projection_job_workers
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .is_empty();
            if empty {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "job A did not self-deregister within the test budget"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        std::thread::sleep(std::time::Duration::from_millis(150));

        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "precondition: the follow-up must have been discarded (constructing the phantom \
             in-flight job) before this test can meaningfully exercise the restart"
        );

        // Same-session restart: clear the Stop flag WITHOUT rotating the
        // session (`rotate_session` is the New Session path and is not
        // exercised here — this reproduces exactly the same-session
        // Stop->Start the seed's mechanism describes).
        app.projection_lane_stopping.store(false, Ordering::SeqCst);

        // A new final ASR revision lands post-restart.
        app.transcript_ledger
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .apply_event(crate::projections::TranscriptEvent::from(
                projection_asr_payload(
                    "abandon-restart-span-3",
                    1,
                    "Third utterance lands after the same-session restart.",
                    true,
                ),
            ))
            .expect("append third span after restart");

        let graph_decision = {
            let ledger = app
                .transcript_ledger
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let mut schedulers = app
                .projection_schedulers
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            schedulers.observe_ledger(&ledger, 1_000).graph
        };
        match &graph_decision {
            ProjectionSchedulerDecision::StartJob { .. } => {}
            other => panic!(
                "the phantom in-flight job must have been abandoned at discard time — the \
                 lane must start a fresh job on the next observation, not stay wedged behind \
                 Coalesced, got {other:?}"
            ),
        }

        dispatch_projection_decision(dispatch, graph_decision);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if event_sink.patch_count() >= 2 {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the lane never projected again after the same-session restart"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        drain_app_writers(&app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ADR-0045 decision 3 (audio-graph-bf5d) positive control: a `FailedCurrent`
    /// decision with an armed deferral spawns exactly one clock thread, which
    /// — with no further final ASR revision ever arriving — is the sole
    /// reason the retry fires at all. Uses a near-immediate deadline
    /// (`current_unix_millis() + 60`) rather than the real ~60s
    /// `PROJECTION_DEFERRED_RETRY_DELAY_MS`, the same "pass the deadline as a
    /// parameter" seam `drain_projection_job_workers`'s tests use to avoid a
    /// real-time wait.
    #[test]
    fn deferred_retry_clock_thread_fires_the_retry_when_not_stopping() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("deferred-retry-fires-when-not-stopping");
        let _guard = DataDirGuard::set(&dir);

        let app = AppState::new();
        let (generator, calls) =
            FnProjectionPatchGenerator::new(|job, _ledger, _notes, sequence, created_at_ms| {
                Ok(ProjectionPatchOutcome {
                    patch: test_projection_patch(&job, sequence, created_at_ms),
                    tokens_used: 5,
                    no_op_filtered_count: 0,
                })
            });
        let (dispatch, event_sink) = projection_dispatch_for_app(&app, generator);

        let failed_job_id = {
            let mut ledger = app
                .transcript_ledger
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            ledger
                .apply_event(crate::projections::TranscriptEvent::from(
                    projection_asr_payload(
                        "deferred-retry-fires-span-1",
                        1,
                        "Failing basis armed for a deferred retry.",
                        true,
                    ),
                ))
                .expect("seed span");
            let mut schedulers = app
                .projection_schedulers
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let job = match schedulers.observe_ledger(&ledger, 10).notes {
                ProjectionSchedulerDecision::StartJob { job } => job,
                other => panic!("expected notes StartJob, got {other:?}"),
            };
            match schedulers.fail_notes_in_flight(&job.id, &job.session_id, &ledger, 20) {
                ProjectionSchedulerDecision::FailedCurrent {
                    failed_job_id,
                    deferred_retry_at_ms: Some(_),
                    ..
                } => failed_job_id,
                other => panic!("expected an armed deferral, got {other:?}"),
            }
        };

        // Test-shortened deadline: a near-future real wall-clock timestamp
        // instead of the real ~60s delay. The scheduler above armed its OWN
        // `deferred_retry_at_ms` using fake `now_ms` (10/20); that value is
        // intentionally NOT what is passed to the clock thread here — the
        // thread's own clock is always real wall time, so its deadline
        // parameter must be too (mirrors `drain_projection_job_workers`'s
        // tests passing a short `timeout` instead of the real
        // `PROJECTION_JOB_FLUSH_TIMEOUT`).
        let short_deadline = current_unix_millis() + 60;
        spawn_deferred_lane_observation(
            dispatch,
            ProjectionKind::Notes,
            failed_job_id,
            short_deadline,
        );

        // Poll on the applied patch, not merely the generator call count:
        // `calls` increments the instant `generate_projection_patch` is
        // entered, before the retry job's own thread has applied and
        // emitted the patch — the discriminating signal for "the retry
        // actually ran to completion" is the event sink.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if event_sink.patch_count() >= 1 {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the deferred retry never fired within the test budget"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(event_sink.patch_count(), 1);

        let empty_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let empty = app
                .projection_job_workers
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .is_empty();
            if empty {
                break;
            }
            assert!(
                std::time::Instant::now() < empty_deadline,
                "both the clock thread and the retry job it spawned must self-deregister"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        drain_app_writers(&app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ADR-0045 decision 3 acceptance (b) (audio-graph-bf5d): no retry fires
    /// after `projection_lane_stopping` is set — even one already armed with
    /// a deadline that would otherwise have fired. `projection_lane_stopping`
    /// is set BEFORE the clock thread is spawned, so this exercises the
    /// thread's very first poll-loop check (the strongest form: the thread's
    /// entire lifetime happens while stopping is already true). The
    /// discriminating assertion is `calls == 0` — the generator (and
    /// therefore any real retry job thread) must never run — not merely
    /// "the registry ends up empty", which a reverted check would also
    /// eventually satisfy once the (wrongly fired) retry job itself
    /// self-deregisters.
    #[test]
    fn deferred_retry_clock_thread_never_fires_after_projection_lane_stopping_is_set() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("deferred-retry-never-fires-once-stopping");
        let _guard = DataDirGuard::set(&dir);

        let app = AppState::new();
        let (generator, calls) =
            FnProjectionPatchGenerator::new(|job, _ledger, _notes, sequence, created_at_ms| {
                Ok(ProjectionPatchOutcome {
                    patch: test_projection_patch(&job, sequence, created_at_ms),
                    tokens_used: 5,
                    no_op_filtered_count: 0,
                })
            });
        let (dispatch, _event_sink) = projection_dispatch_for_app(&app, generator);

        let failed_job_id = {
            let mut ledger = app
                .transcript_ledger
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            ledger
                .apply_event(crate::projections::TranscriptEvent::from(
                    projection_asr_payload(
                        "deferred-retry-never-fires-span-1",
                        1,
                        "Failing basis armed for a deferred retry that Stop must cancel.",
                        true,
                    ),
                ))
                .expect("seed span");
            let mut schedulers = app
                .projection_schedulers
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let job = match schedulers.observe_ledger(&ledger, 10).graph {
                ProjectionSchedulerDecision::StartJob { job } => job,
                other => panic!("expected graph StartJob, got {other:?}"),
            };
            match schedulers.fail_graph_in_flight(&job.id, &job.session_id, &ledger, 20) {
                ProjectionSchedulerDecision::FailedCurrent {
                    failed_job_id,
                    deferred_retry_at_ms: Some(_),
                    ..
                } => failed_job_id,
                other => panic!("expected an armed deferral, got {other:?}"),
            }
        };

        // Stop begins before the clock thread is even spawned — the
        // strongest ordering: the thread's entire life happens under
        // `projection_lane_stopping = true`.
        app.projection_lane_stopping.store(true, Ordering::SeqCst);

        let short_deadline = current_unix_millis() + 200;
        spawn_deferred_lane_observation(
            dispatch,
            ProjectionKind::Graph,
            failed_job_id,
            short_deadline,
        );

        // Poll well past the deadline the thread was armed with: a reverted
        // stopping-check would still fire the retry once `short_deadline`
        // elapses, so this window must be long enough to catch that mutant,
        // not just long enough to observe a prompt correct exit.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let empty = app
                .projection_job_workers
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .is_empty();
            if empty {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the clock thread must self-deregister promptly (bounded by its 250ms poll), \
                 stopping or not"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        std::thread::sleep(std::time::Duration::from_millis(300));

        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "no retry job must ever run once projection_lane_stopping is set"
        );
        assert!(
            app.projection_schedulers
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .graph()
                .in_flight_job()
                .is_none(),
            "a cancelled deferred retry must not leave a phantom in-flight job"
        );

        drain_app_writers(&app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Review fix (adr0045/bf5d-deferred-retry): pins `dispatch_projection_decision`'s
    /// new `FailedCurrent { deferred_retry_at_ms: Some(_), .. }` arm ITSELF,
    /// not merely `spawn_deferred_lane_observation` (which every other test
    /// in this file calls directly). Feeds a `FailedCurrent` decision
    /// straight into `dispatch_projection_decision` — the production
    /// dispatcher every real caller (`finish_projection_scheduler_job`,
    /// `trigger_deferred_projection_retry`) actually goes through — and
    /// proves it is the arm's own dispatch that spawns the clock thread and
    /// carries the retry to completion. Gutting the arm to `{ .. } => {}`
    /// makes this test time out waiting for a patch that never arrives;
    /// removing the arm entirely fails to compile (a fresh regression this
    /// test alone cannot catch, but the exhaustive match in
    /// `dispatch_projection_decision` already guards that).
    #[test]
    fn dispatch_projection_decision_spawns_and_fires_the_deferred_retry_clock_for_an_armed_failed_current()
     {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("dispatch-decision-arms-deferred-retry");
        let _guard = DataDirGuard::set(&dir);

        let app = AppState::new();
        let (generator, calls) =
            FnProjectionPatchGenerator::new(|job, _ledger, _notes, sequence, created_at_ms| {
                Ok(ProjectionPatchOutcome {
                    patch: test_projection_patch(&job, sequence, created_at_ms),
                    tokens_used: 5,
                    no_op_filtered_count: 0,
                })
            });
        let (dispatch, event_sink) = projection_dispatch_for_app(&app, generator);

        // Arm a real deferral through the scheduler's own `fail_in_flight`
        // (fake `now_ms` values, same idiom every scheduler-level unit test
        // uses) so `failed_job_id` refers to a genuine failed job the
        // eventual `observe_ledger` re-check inside the fired retry can
        // legitimately act on.
        let failed_job_id = {
            let mut ledger = app
                .transcript_ledger
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            ledger
                .apply_event(crate::projections::TranscriptEvent::from(
                    projection_asr_payload(
                        "dispatch-decision-arms-deferred-retry-span-1",
                        1,
                        "Failing basis fed straight into dispatch_projection_decision.",
                        true,
                    ),
                ))
                .expect("seed span");
            let mut schedulers = app
                .projection_schedulers
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let job = match schedulers.observe_ledger(&ledger, 10).notes {
                ProjectionSchedulerDecision::StartJob { job } => job,
                other => panic!("expected notes StartJob, got {other:?}"),
            };
            match schedulers.fail_notes_in_flight(&job.id, &job.session_id, &ledger, 20) {
                ProjectionSchedulerDecision::FailedCurrent {
                    failed_job_id,
                    deferred_retry_at_ms: Some(_),
                    ..
                } => failed_job_id,
                other => panic!("expected an armed deferral, got {other:?}"),
            }
        };

        // The decision fed to `dispatch_projection_decision` here is built
        // BY THIS TEST, not returned by the scheduler above — the point is
        // to exercise the dispatcher's own arm with a real near-future wall
        // clock deadline (the same test-shortened-delay seam
        // `spawn_deferred_lane_observation`'s other tests use), rather than
        // whatever fake epoch the scheduler recorded internally from `now_ms
        // = 20` above.
        let decision = ProjectionSchedulerDecision::FailedCurrent {
            failed_job_id,
            kind: ProjectionKind::Notes,
            deferred_retry_at_ms: Some(current_unix_millis() + 60),
        };
        dispatch_projection_decision(dispatch, decision);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if event_sink.patch_count() >= 1 {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "dispatch_projection_decision's FailedCurrent arm never fired the deferred \
                 retry within the test budget"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(event_sink.patch_count(), 1);

        drain_app_writers(&app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Review fix (adr0045/bf5d-deferred-retry): unlike every other clock-
    /// related test in this file, this one feeds a `FailedCurrent { .. }`
    /// decision straight into `dispatch_projection_decision` (never touching
    /// `spawn_deferred_lane_observation` directly) with
    /// `projection_lane_stopping` already set — proving the dispatcher's
    /// overall behavior at this call boundary is safe: no retry job ever
    /// runs and the registry ends up empty, closing the coverage gap the
    /// review named ("the dispatch-level gate at :2069 is likewise
    /// unpinned").
    ///
    /// Honest limitation: this test cannot, by itself, distinguish "the
    /// dispatch-level check refused to spawn" from "the check was bypassed,
    /// a clock thread spawned, and that thread's OWN first poll check (the
    /// identical `stopping.load` guard, evaluated moments later on the
    /// child) caught it instead" — both are observably identical here
    /// (`calls == 0`, registry empty), for the same reason the review
    /// separately flagged for
    /// `deferred_retry_clock_thread_never_fires_after_projection_lane_stopping_is_set`:
    /// the two checks guard the exact same ordering (stopping set before the
    /// decision/spawn), so a mutant that removes ONLY the dispatch-level
    /// check is functionally caught by the thread's own check regardless,
    /// with no test-observable difference short of instrumenting thread
    /// creation itself. That duplication is intentional defense in depth
    /// (see the `FailedCurrent` arm's own comment), not a bug.
    #[test]
    fn dispatch_projection_decision_refuses_to_arm_a_deferred_retry_clock_once_stopping() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("dispatch-decision-refuses-deferred-retry-once-stopping");
        let _guard = DataDirGuard::set(&dir);

        let app = AppState::new();
        let (generator, calls) =
            FnProjectionPatchGenerator::new(|job, _ledger, _notes, sequence, created_at_ms| {
                Ok(ProjectionPatchOutcome {
                    patch: test_projection_patch(&job, sequence, created_at_ms),
                    tokens_used: 5,
                    no_op_filtered_count: 0,
                })
            });
        let (dispatch, _event_sink) = projection_dispatch_for_app(&app, generator);

        let failed_job_id = {
            let mut ledger = app
                .transcript_ledger
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            ledger
                .apply_event(crate::projections::TranscriptEvent::from(
                    projection_asr_payload(
                        "dispatch-decision-refuses-deferred-retry-span-1",
                        1,
                        "Failing basis that Stop must keep the dispatcher from arming.",
                        true,
                    ),
                ))
                .expect("seed span");
            let mut schedulers = app
                .projection_schedulers
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let job = match schedulers.observe_ledger(&ledger, 10).graph {
                ProjectionSchedulerDecision::StartJob { job } => job,
                other => panic!("expected graph StartJob, got {other:?}"),
            };
            match schedulers.fail_graph_in_flight(&job.id, &job.session_id, &ledger, 20) {
                ProjectionSchedulerDecision::FailedCurrent {
                    failed_job_id,
                    deferred_retry_at_ms: Some(_),
                    ..
                } => failed_job_id,
                other => panic!("expected an armed deferral, got {other:?}"),
            }
        };

        // Set BEFORE the decision ever reaches `dispatch_projection_decision`
        // — the ordering production always produces when Stop begins before
        // a same-basis failure's own dispatch tail runs.
        app.projection_lane_stopping.store(true, Ordering::SeqCst);

        let decision = ProjectionSchedulerDecision::FailedCurrent {
            failed_job_id,
            kind: ProjectionKind::Graph,
            deferred_retry_at_ms: Some(current_unix_millis() + 60),
        };
        dispatch_projection_decision(dispatch, decision);

        // Give a spawned-but-shouldn't-be-spawned clock thread ample time to
        // fire on its near-immediate deadline; the discriminating assertion
        // is `calls == 0` below, not merely registry emptiness (which a
        // wrongly-fired, already-completed retry would also satisfy).
        std::thread::sleep(std::time::Duration::from_millis(300));

        assert!(
            app.projection_job_workers
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .is_empty(),
            "the dispatch-level stopping gate must refuse to register any clock thread"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "no retry job must ever run once projection_lane_stopping is set before the \
             FailedCurrent decision reaches the dispatcher"
        );

        drain_app_writers(&app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// audio-graph-1609 acceptance (d), wiring-level companion to the
    /// scheduler-level `abandon_deferred_retry_*` unit tests in
    /// `projection_scheduler.rs`: proves `dispatch_projection_decision`'s
    /// OWN discard branch actually calls the abandon path, not merely that
    /// `ProjectionScheduler` supports one in isolation. Reuses the sibling
    /// test's exact setup above
    /// (`..._refuses_to_arm_a_deferred_retry_clock_once_stopping`) — a
    /// `FailedCurrent` decision dispatched with `projection_lane_stopping`
    /// already set — then continues past it: the deferral must not survive
    /// as an orphan. A same-basis re-observation, with no clock thread ever
    /// having been spawned, must retry immediately instead of idling
    /// forever waiting on one.
    #[test]
    fn dispatch_projection_decision_abandons_a_discarded_deferred_retry_once_stopping() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("dispatch-decision-abandons-discarded-deferred-retry");
        let _guard = DataDirGuard::set(&dir);

        let app = AppState::new();
        let (generator, calls) =
            FnProjectionPatchGenerator::new(|job, _ledger, _notes, sequence, created_at_ms| {
                Ok(ProjectionPatchOutcome {
                    patch: test_projection_patch(&job, sequence, created_at_ms),
                    tokens_used: 5,
                    no_op_filtered_count: 0,
                })
            });
        let (dispatch, event_sink) = projection_dispatch_for_app(&app, generator);

        let (failed_job_id, deferred_retry_at_ms) = {
            let mut ledger = app
                .transcript_ledger
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            ledger
                .apply_event(crate::projections::TranscriptEvent::from(
                    projection_asr_payload(
                        "dispatch-decision-abandons-deferred-retry-span-1",
                        1,
                        "Failing basis whose deferral must not survive as an orphan.",
                        true,
                    ),
                ))
                .expect("seed span");
            let mut schedulers = app
                .projection_schedulers
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let job = match schedulers.observe_ledger(&ledger, 10).graph {
                ProjectionSchedulerDecision::StartJob { job } => job,
                other => panic!("expected graph StartJob, got {other:?}"),
            };
            match schedulers.fail_graph_in_flight(&job.id, &job.session_id, &ledger, 20) {
                ProjectionSchedulerDecision::FailedCurrent {
                    failed_job_id,
                    deferred_retry_at_ms: Some(deferred_retry_at_ms),
                    ..
                } => (failed_job_id, deferred_retry_at_ms),
                other => panic!("expected an armed deferral, got {other:?}"),
            }
        };

        // Set BEFORE the decision ever reaches `dispatch_projection_decision`
        // — the ordering production always produces when Stop begins before
        // a same-basis failure's own dispatch tail runs.
        app.projection_lane_stopping.store(true, Ordering::SeqCst);

        // The decision's `deferred_retry_at_ms` must be the SAME value the
        // scheduler actually armed above — `abandon_deferred_retry` matches
        // by exact value on purpose (see its doc comment), so a decision
        // carrying a different value here would exercise the "raced a newer
        // re-arm" no-op path instead of the orphan this test targets.
        let decision = ProjectionSchedulerDecision::FailedCurrent {
            failed_job_id,
            kind: ProjectionKind::Graph,
            deferred_retry_at_ms: Some(deferred_retry_at_ms),
        };
        dispatch_projection_decision(dispatch.clone(), decision);

        std::thread::sleep(std::time::Duration::from_millis(150));
        assert!(
            app.projection_job_workers
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .is_empty(),
            "precondition: no clock thread must have been registered for the discarded deferral"
        );

        // Same-session restart: clear the Stop flag WITHOUT rotating the
        // session.
        app.projection_lane_stopping.store(false, Ordering::SeqCst);

        // A same-basis re-observation, long before the real (~60s-future)
        // deferred deadline, must retry immediately — no clock thread exists
        // to wait for.
        let retry_decision = {
            let ledger = app
                .transcript_ledger
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let mut schedulers = app
                .projection_schedulers
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            schedulers.observe_ledger(&ledger, 25).graph
        };
        match &retry_decision {
            ProjectionSchedulerDecision::StartJob { .. } => {}
            other => panic!(
                "the deferral must have been abandoned at discard time — a same-basis \
                 re-observation must retry immediately instead of waiting on a clock that \
                 was never spawned, got {other:?}"
            ),
        }

        dispatch_projection_decision(dispatch, retry_decision);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if event_sink.patch_count() >= 1 {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the abandoned-then-restarted lane never actually retried"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        drain_app_writers(&app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// audio-graph-9cc1 acceptance: removal from the live projection-job
    /// registry must match BOTH `kind` and `job_id`. A mismatched id (or a
    /// matching id under the wrong kind) must never remove — and therefore
    /// never join or drop — a different, still-registered handle. A handle
    /// left sitting in the registry (never removed) is, by this registry's
    /// design, provably never joined: the ONLY thing that happens to a
    /// registered handle is removal (self-deregister or the stop-time drain),
    /// and removal is the sole path to a join/drop.
    #[test]
    fn deregister_projection_job_never_removes_a_mismatched_kind_or_job_id() {
        let registry: crate::state::ProjectionJobRegistry = Arc::new(Mutex::new(Vec::new()));
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let wedged = std::thread::spawn(move || {
            let _ = release_rx.recv();
        });
        registry.lock().unwrap_or_else(|p| p.into_inner()).push((
            ProjectionKind::Graph,
            "real-job".to_string(),
            wedged,
        ));

        // Wrong job_id, correct kind: must be a no-op.
        deregister_projection_job(&registry, &ProjectionKind::Graph, "wrong-job-id");
        assert_eq!(
            registry.lock().unwrap_or_else(|p| p.into_inner()).len(),
            1,
            "a mismatched job_id must never remove the real entry"
        );

        // Correct job_id, wrong kind: must also be a no-op.
        deregister_projection_job(&registry, &ProjectionKind::Notes, "real-job");
        assert_eq!(
            registry.lock().unwrap_or_else(|p| p.into_inner()).len(),
            1,
            "a mismatched kind must never remove the real entry, even with the right job_id"
        );

        // Both match: this is the only call that may remove (and thus drop,
        // never join) the entry.
        deregister_projection_job(&registry, &ProjectionKind::Graph, "real-job");
        assert!(
            registry
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .is_empty(),
            "an exact kind+job_id match must remove the entry"
        );

        // Let the now-detached thread exit; nothing left to join it, which is
        // exactly the point — it was dropped, never joined.
        let _ = release_tx.send(());
    }

    /// Every projection LLM submission appears in the data-movement ledger, and
    /// a local-only session writes NO remote summary/prefix (ADR-0025 §2g / seed
    /// audio-graph-72d5).
    #[test]
    fn runtime_projection_dispatch_ledgers_remote_llm_flow_and_gates_local_only() {
        use crate::persistence::{DataClass, DataMovementEventType, DestinationBoundary};

        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("projection-dispatch-ledger");
        let _guard = DataDirGuard::set(&dir);

        // Seed a long enough transcript that a rolling summary exists.
        let seed_ledger = |app: &AppState| {
            let mut ledger = app
                .transcript_ledger
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            for i in 0..(crate::projections::ROLLING_SUMMARY_HOT_WINDOW_TURNS + 3) {
                ledger
                    .apply_event(crate::projections::TranscriptEvent::from(
                        projection_asr_payload(
                            &format!("ledger-span-{i}"),
                            1,
                            &format!("Turn {i} about a topic."),
                            true,
                        ),
                    ))
                    .expect("seed transcript");
            }
            let observation = {
                let mut schedulers = app
                    .projection_schedulers
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                schedulers.observe_ledger(&ledger, 10)
            };
            match observation.notes {
                ProjectionSchedulerDecision::StartJob { job } => job,
                other => panic!("expected notes start job, got {other:?}"),
            }
        };

        // --- Cloud provider + consent: a remote flow is ledgered. ---
        // The fake patch reports "openrouter" provenance so the terminal event
        // exercises the actual-backend mapping (cached prefix + remote flow).
        let cloud_app = AppState::new();
        let notes_job = seed_ledger(&cloud_app);
        let (generator, _calls) =
            FnProjectionPatchGenerator::new(|job, _ledger, _notes, sequence, created_at_ms| {
                let mut patch = test_projection_patch(&job, sequence, created_at_ms);
                patch.provenance.provider = "openrouter".to_string();
                Ok(ProjectionPatchOutcome {
                    patch,
                    tokens_used: 55,
                    no_op_filtered_count: 0,
                })
            });
        let openrouter = LlmProvider::OpenRouter {
            model: "anthropic/claude-sonnet-4.5".to_string(),
            base_url: "https://openrouter.ai/api/v1".to_string(),
            provider_order: None,
            include_usage_in_stream: true,
            api_key: String::new(),
        };
        let (dispatch, _event_sink, movements) =
            projection_dispatch_for_app_with_movement(&cloud_app, generator, openrouter, true);
        run_projection_job(dispatch, notes_job);

        let recorded = movements.events.lock().unwrap_or_else(|p| p.into_inner());
        assert!(
            recorded
                .iter()
                .any(|e| e.event_type == DataMovementEventType::ProviderCallStarted),
            "started event must be ledgered before the call"
        );
        let terminal = recorded
            .iter()
            .find(|e| e.event_type == DataMovementEventType::ProviderCallSucceeded)
            .expect("succeeded event ledgered");
        assert_eq!(terminal.destination.boundary, DestinationBoundary::Provider);
        assert!(
            terminal.data_classes.contains(&DataClass::Notes),
            "rolling summary (Notes) recorded as a remote flow"
        );
        assert!(
            terminal
                .artifact_refs
                .iter()
                .any(|a| a.kind == "vendor_cached_prompt_prefix"),
            "vendor-cached prefix persistence recorded"
        );
        // Content-free: no transcript text in the serialized ledger.
        let json = serde_json::to_string(&*recorded).expect("serialize ledger");
        assert!(!json.contains("about a topic"));
        drop(recorded);
        drain_app_writers(&cloud_app);

        // --- Same cloud provider, but local-only policy: NO remote flow. ---
        let local_app = AppState::new();
        let notes_job_local = seed_ledger(&local_app);
        let (generator2, _calls2) =
            FnProjectionPatchGenerator::new(|job, _ledger, _notes, sequence, created_at_ms| {
                Ok(ProjectionPatchOutcome {
                    patch: test_projection_patch(&job, sequence, created_at_ms),
                    tokens_used: 55,
                    no_op_filtered_count: 0,
                })
            });
        let openrouter2 = LlmProvider::OpenRouter {
            model: "anthropic/claude-sonnet-4.5".to_string(),
            base_url: "https://openrouter.ai/api/v1".to_string(),
            provider_order: None,
            include_usage_in_stream: true,
            api_key: String::new(),
        };
        let (dispatch2, _event_sink2, movements2) = projection_dispatch_for_app_with_movement(
            &local_app,
            generator2,
            openrouter2,
            false, // cloud transfer NOT allowed
        );
        run_projection_job(dispatch2, notes_job_local);

        let recorded2 = movements2.events.lock().unwrap_or_else(|p| p.into_inner());
        for event in recorded2.iter() {
            assert_eq!(
                event.destination.boundary,
                DestinationBoundary::Local,
                "local-only session must not emit a remote destination"
            );
            assert!(
                !event.data_classes.contains(&DataClass::Notes),
                "local-only session must not ledger a remote rolling summary"
            );
            assert!(
                event.artifact_refs.is_empty(),
                "local-only session must not ledger a vendor cache write"
            );
        }
        drop(recorded2);
        drain_app_writers(&local_app);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Reviewer scope-honesty finding (W2 fix round):
    /// `ProjectionMovementFacts::no_op_filtered_count`'s doc comment claims
    /// it is "populated and unit-tested at the `ProjectionMovementFacts`
    /// level", but before this test nothing asserted a real non-zero value
    /// ever reached that struct through the live `run_projection_job`
    /// wiring — the concrete surviving mutant was
    /// `outcome.no_op_filtered_count` silently replaced by the literal `0`
    /// at the terminal-event call site (every other test in this module sets
    /// `no_op_filtered_count: 0` on its `ProjectionPatchOutcome` double, so
    /// that mutant is indistinguishable from correct code to them). This
    /// test uses a double returning a distinctive non-zero count and reads
    /// it back via `record_movement_facts` (see that trait method's doc
    /// comment for why the ordinary `events` capture cannot see this field at
    /// all: `no_op_filtered_count` has no `MovementCounts` sink by design).
    #[test]
    fn run_projection_job_threads_the_real_no_op_filtered_count_into_movement_facts() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("projection-no-op-filtered-count-movement-facts");
        let _guard = DataDirGuard::set(&dir);

        let app = AppState::new();
        let notes_job = {
            let mut ledger = app
                .transcript_ledger
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            ledger
                .apply_event(crate::projections::TranscriptEvent::from(
                    projection_asr_payload(
                        "no-op-filtered-count-movement-facts-span",
                        1,
                        "Three sections in this tick were byte-identical no-ops.",
                        true,
                    ),
                ))
                .expect("seed transcript");
            let mut schedulers = app
                .projection_schedulers
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            match schedulers.observe_ledger(&ledger, 10).notes {
                ProjectionSchedulerDecision::StartJob { job } => job,
                other => panic!("expected notes job, got {other:?}"),
            }
        };

        let (generator, _calls) =
            FnProjectionPatchGenerator::new(|job, _ledger, _notes, sequence, created_at_ms| {
                Ok(ProjectionPatchOutcome {
                    patch: test_projection_patch(&job, sequence, created_at_ms),
                    tokens_used: 9,
                    no_op_filtered_count: 3,
                })
            });
        let (dispatch, _event_sink, movements) = projection_dispatch_for_app_with_movement(
            &app,
            generator,
            LlmProvider::LocalLlama,
            false,
        );

        run_projection_job(dispatch, notes_job);

        let facts = movements
            .movement_facts
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let terminal_facts = facts
            .last()
            .expect("the apply-success branch must record movement facts");
        assert_eq!(
            terminal_facts.no_op_filtered_count, 3,
            "the real outcome.no_op_filtered_count must reach ProjectionMovementFacts, \
             not a hardcoded 0"
        );
        drop(facts);

        drain_app_writers(&app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ADR-0025 §2g (seed audio-graph-253c part 2): switching
    /// `projection_movement_facts` from `projection_prompt_shape` to
    /// `projection_prompt_shape_with_notes` means the data-movement ledger
    /// records the live notes-state snapshot's char/entry counts for a
    /// Notes-kind tick that actually has a snapshot — and records NOTHING
    /// notes-snapshot-derived for a Graph-kind tick, which never carries one
    /// (`run_projection_job` passes `notes_snapshot: None` for Graph-kind).
    /// The seeded transcript is a single short turn so `has_rolling_summary`
    /// stays false, isolating the notes-snapshot signal specifically from the
    /// unrelated rolling-summary one already covered by the sibling test
    /// above.
    #[test]
    fn run_projection_job_ledgers_notes_snapshot_for_notes_kind_and_omits_it_for_graph_kind() {
        use crate::persistence::DataClass;

        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("movement-facts-notes-snapshot");
        let _guard = DataDirGuard::set(&dir);

        let app = AppState::new();
        let observation = {
            let mut ledger = app
                .transcript_ledger
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            ledger
                .apply_event(crate::projections::TranscriptEvent::from(
                    projection_asr_payload(
                        "movement-facts-notes-span",
                        1,
                        "Short single turn.",
                        true,
                    ),
                ))
                .expect("seed transcript");
            let mut schedulers = app
                .projection_schedulers
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            schedulers.observe_ledger(&ledger, 10)
        };
        let notes_job = match observation.notes {
            ProjectionSchedulerDecision::StartJob { job } => job,
            other => panic!("expected notes start job, got {other:?}"),
        };
        let graph_job = match observation.graph {
            ProjectionSchedulerDecision::StartJob { job } => job,
            other => panic!("expected graph start job, got {other:?}"),
        };

        // Seed an EXISTING note before either tick, so `run_projection_job`'s
        // spawn-time clone (`materialized_notes_snapshot_for_session`) has a
        // real, non-empty snapshot to report.
        {
            let mut materialized = app
                .materialized_projection_state
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let seed_patch = test_projection_patch(&notes_job, 1, 5);
            materialized
                .notes
                .apply_patch(&seed_patch, None)
                .expect("seed note");
        }

        let (generator, _calls) =
            FnProjectionPatchGenerator::new(|job, _ledger, _notes, sequence, created_at_ms| {
                Ok(ProjectionPatchOutcome {
                    patch: test_projection_patch(&job, sequence, created_at_ms),
                    tokens_used: 1,
                    no_op_filtered_count: 0,
                })
            });
        let openrouter = LlmProvider::OpenRouter {
            model: "anthropic/claude-sonnet-4.5".to_string(),
            base_url: "https://openrouter.ai/api/v1".to_string(),
            provider_order: None,
            include_usage_in_stream: true,
            api_key: String::new(),
        };
        let (dispatch, _event_sink, movements) =
            projection_dispatch_for_app_with_movement(&app, generator, openrouter, true);

        run_projection_job(dispatch.clone(), notes_job);
        {
            let recorded = movements.events.lock().unwrap_or_else(|p| p.into_inner());
            assert!(
                !recorded.is_empty(),
                "the Notes-kind tick must record at least one movement event"
            );
            assert!(
                recorded
                    .iter()
                    .any(|e| e.data_classes.contains(&DataClass::Notes)),
                "a Notes-kind tick with an existing snapshot must tag DataClass::Notes \
                 even with no rolling summary present, got: {recorded:?}"
            );
            assert!(
                recorded.iter().any(|e| e
                    .counts
                    .as_ref()
                    .and_then(|c| c.text_chars)
                    .is_some_and(|chars| chars > 0)),
                "the notes snapshot's char count must reach the ledger's text_chars field"
            );
        }
        movements
            .events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();

        run_projection_job(dispatch, graph_job);
        {
            let recorded = movements.events.lock().unwrap_or_else(|p| p.into_inner());
            assert!(
                !recorded.is_empty(),
                "the Graph-kind tick must still record its own movement events"
            );
            assert!(
                !recorded
                    .iter()
                    .any(|e| e.data_classes.contains(&DataClass::Notes)),
                "a Graph-kind tick must never tag DataClass::Notes from a notes \
                 snapshot it never carries, got: {recorded:?}"
            );
        }

        drain_app_writers(&app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// audio-graph-a6b5 W2: pins that the movement-counts `log::debug!` line
    /// inside `run_projection_job`'s apply-success branch still exists and
    /// still carries all three ADR-0025-safe counts (`no_op_filtered_count`,
    /// `notes_snapshot_chars`, `notes_snapshot_entries`). This repo has no
    /// log-capture test harness (see the audio-graph-fa56 precedent,
    /// `commands.rs`'s
    /// `log_abandoned_deferred_retries_after_stop_emits_the_documented_warn_key`,
    /// for the same technique applied to a different WARN), so source-text
    /// inspection is the cheapest mutation-proof available for a
    /// logging-only change: a mutant that deletes this `log::debug!` call
    /// (or swaps an argument for a hardcoded `0`) would make every OTHER
    /// test in this suite pass, since none of them observe emitted log
    /// output — this is the ONE thing that catches that mutant.
    #[test]
    fn run_projection_job_movement_counts_log_line_still_carries_the_no_op_filter_and_outline_counts()
     {
        let source = include_str!("mod.rs");
        let body_start = source
            .find("fn run_projection_job(")
            .expect("run_projection_job must exist in speech/mod.rs");
        let body_end = source[body_start..]
            .find("fn record_projection_generation_result(")
            .map(|relative| body_start + relative)
            .expect("record_projection_generation_result must follow run_projection_job");
        let body = &source[body_start..body_end];

        assert!(
            body.contains("Projection job movement counts"),
            "the movement-counts log line must still exist inside run_projection_job"
        );
        assert!(
            body.contains("movement_facts.no_op_filtered_count"),
            "the log line must still carry the no-op-filtered count"
        );
        assert!(
            body.contains("movement_facts.notes_snapshot_chars"),
            "the log line must still carry the notes-outline char count"
        );
        assert!(
            body.contains("movement_facts.notes_snapshot_entries"),
            "the log line must still carry the notes-outline entry count"
        );
    }

    /// Staleness semantics of the SPAWN-time notes-snapshot clone (seed
    /// audio-graph-253c part 2): `run_projection_job` clones `MaterializedNotes`
    /// ONCE, before dispatch, and never re-reads it for this tick. A patch that
    /// applies to the SAME session's notes state between that clone and
    /// generation — exactly what a second, concurrently-completing projection
    /// job's apply would do — is acceptable staleness: the generator sees the
    /// notes state as of spawn, not as of dispatch, and the missed update is
    /// NOT lost (it stays in the durable materialized state; the NEXT tick's
    /// spawn picks it up). This pins that behavior with a real race rather
    /// than asserting it only in prose.
    #[test]
    fn run_projection_job_notes_snapshot_is_pinned_at_spawn_not_re_read_at_dispatch() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("notes-snapshot-staleness");
        let _guard = DataDirGuard::set(&dir);

        let app = AppState::new();
        let notes_job = {
            let mut ledger = app
                .transcript_ledger
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            ledger
                .apply_event(crate::projections::TranscriptEvent::from(
                    projection_asr_payload("staleness-span", 1, "Short single turn.", true),
                ))
                .expect("seed transcript");
            let mut schedulers = app
                .projection_schedulers
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            match schedulers.observe_ledger(&ledger, 10).notes {
                ProjectionSchedulerDecision::StartJob { job } => job,
                other => panic!("expected notes start job, got {other:?}"),
            }
        };

        let materialized_for_race = app.materialized_projection_state.clone();
        let captured_snapshot: Arc<Mutex<Option<Option<MaterializedNotes>>>> =
            Arc::new(Mutex::new(None));
        let captured_for_closure = captured_snapshot.clone();
        let (generator, _calls) =
            FnProjectionPatchGenerator::new(move |job, _ledger, notes, sequence, created_at_ms| {
                // Record exactly what THIS tick's dispatch received, BEFORE the
                // concurrent mutation simulated just below.
                *captured_for_closure
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = Some(notes.clone());

                // Simulate a SECOND, concurrently-completing job applying its own
                // patch to the SAME session between this job's spawn-time clone
                // and this dispatch.
                {
                    let mut materialized = materialized_for_race
                        .lock()
                        .unwrap_or_else(|p| p.into_inner());
                    let racing_patch = ProjectionPatch {
                        sequence: materialized.notes.last_sequence + 1,
                        kind: ProjectionKind::Notes,
                        llm_request_id: "racing-patch".to_string(),
                        basis: job.basis.clone(),
                        operations: vec![ProjectionOperation::UpsertNote {
                            id: "note:racing".to_string(),
                            title: "Racing note".to_string(),
                            body: "Applied concurrently, mid-dispatch.".to_string(),
                            tags: vec![],
                            evidence: crate::claim_evidence::EvidenceAnchor::default(),
                            heading_level: None,
                        }],
                        confidence: 1.0,
                        provenance: ProjectionProvenance {
                            provider: "race".to_string(),
                            model: "race".to_string(),
                            prompt_id: "race".to_string(),
                            route_id: None,
                            model_source: crate::llm::route::ModelIdentitySource::Requested,
                        },
                        route: None,
                        queued_at_ms: None,
                        generation_latency_ms: None,
                        apply_latency_ms: None,
                        basis_currency_at_apply: None,
                        created_at_ms,
                    };
                    materialized
                        .notes
                        .apply_patch(&racing_patch, None)
                        .expect("apply racing patch");
                }

                Ok(ProjectionPatchOutcome {
                    patch: test_projection_patch(&job, sequence, created_at_ms),
                    tokens_used: 1,
                    no_op_filtered_count: 0,
                })
            });
        let (dispatch, _event_sink) = projection_dispatch_for_app(&app, generator);

        run_projection_job(dispatch, notes_job);

        let received = captured_snapshot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .expect("generator was called");
        let received_notes = received.expect("Notes-kind job must receive Some(snapshot)");
        assert!(
            !received_notes.notes.iter().any(|n| n.id == "note:racing"),
            "the snapshot handed to THIS tick's generator must be pinned at spawn — \
             it must NOT observe a patch applied by a racing job mid-dispatch"
        );

        // Self-correcting: the racing note is NOT lost — it is present in the
        // durable materialized state right now, ready for the NEXT tick's
        // spawn to pick up.
        let materialized = app
            .materialized_projection_state
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        assert!(
            materialized
                .notes
                .notes
                .iter()
                .any(|n| n.id == "note:racing"),
            "the racing job's own apply must not be lost — it is still in the \
             durable state for the next tick to observe"
        );
        drop(materialized);

        drain_app_writers(&app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Session-pinning of the SPAWN-time notes-snapshot clone (seed
    /// audio-graph-253c part 2): unlike ordinary same-session staleness
    /// (pinned above), a snapshot whose session id does not match this job's
    /// own must NEVER reach the prompt. `run_projection_job` must pass `None`
    /// — never the (unrelated) OTHER session's notes — for a job whose
    /// `session_id` a rotation has already outrun by the time it runs (see
    /// `ProjectionRuntimeHandle::materialized_notes_snapshot_for_session`'s
    /// doc for why: `materialized_projection_state` is one `Arc<Mutex<_>>`
    /// reused across rotations, not swapped).
    #[test]
    fn run_projection_job_omits_notes_snapshot_when_job_session_id_does_not_match_current_state() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("notes-snapshot-wrong-session");
        let _guard = DataDirGuard::set(&dir);

        let app = AppState::new();
        let mut notes_job = {
            let mut ledger = app
                .transcript_ledger
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            ledger
                .apply_event(crate::projections::TranscriptEvent::from(
                    projection_asr_payload("wrong-session-span", 1, "Short single turn.", true),
                ))
                .expect("seed transcript");
            let mut schedulers = app
                .projection_schedulers
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            match schedulers.observe_ledger(&ledger, 10).notes {
                ProjectionSchedulerDecision::StartJob { job } => job,
                other => panic!("expected notes start job, got {other:?}"),
            }
        };

        // Seed a note under the REAL (current) session, then hand
        // `run_projection_job` a job carrying a DIFFERENT session id —
        // simulating a job spawned for an old session that a rotation has
        // already replaced by the time it runs.
        {
            let mut materialized = app
                .materialized_projection_state
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let seed_patch = test_projection_patch(&notes_job, 1, 5);
            materialized
                .notes
                .apply_patch(&seed_patch, None)
                .expect("seed note");
        }
        notes_job.session_id = "a-different-session".to_string();

        let captured_notes: Arc<Mutex<Option<Option<MaterializedNotes>>>> =
            Arc::new(Mutex::new(None));
        let captured_for_closure = captured_notes.clone();
        let (generator, _calls) =
            FnProjectionPatchGenerator::new(move |job, _ledger, notes, sequence, created_at_ms| {
                *captured_for_closure
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = Some(notes);
                Ok(ProjectionPatchOutcome {
                    patch: test_projection_patch(&job, sequence, created_at_ms),
                    tokens_used: 1,
                    no_op_filtered_count: 0,
                })
            });
        let (dispatch, _event_sink) = projection_dispatch_for_app(&app, generator);

        run_projection_job(dispatch, notes_job);

        let received = captured_notes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .expect("generator was called");
        assert!(
            received.is_none(),
            "a job whose session id does not match the current materialized \
             state's session must receive None, never the OTHER session's notes"
        );

        drain_app_writers(&app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ----- coverage-gap closure: the real ExecutorProjectionPatchGenerator ---

    /// Loopback HTTP mock mirroring `llm::executor::tests::spawn_capturing_mock`
    /// (duplicated here rather than shared — this codebase's convention for
    /// per-module wire mocks; see `llm::openrouter`, `llm::streaming`, and
    /// `llm::api_client`'s own separate copies). Serves `responses` (one per
    /// connection, in order) and captures each request's raw JSON body so a
    /// test can assert on what a REAL wire call actually carried.
    async fn spawn_capturing_projection_mock(
        responses: Vec<String>,
    ) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock");
        let addr = listener.local_addr().expect("local addr");
        let captured = Arc::new(Mutex::new(Vec::new()));
        let captured_for_task = captured.clone();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            for body in responses {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let mut buf = vec![0u8; 8192];
                let mut total = String::new();
                let mut content_len: Option<usize> = None;
                let mut header_end: Option<usize> = None;
                loop {
                    let n = match stream.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => n,
                        Err(_) => break,
                    };
                    total.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if header_end.is_none()
                        && let Some(hdr_end) = total.find("\r\n\r\n")
                    {
                        header_end = Some(hdr_end);
                        content_len = total[..hdr_end]
                            .to_ascii_lowercase()
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .and_then(|v| v.trim().parse::<usize>().ok());
                    }
                    match (content_len, header_end) {
                        (Some(cl), Some(hdr_end)) if total.len() - (hdr_end + 4) >= cl => break,
                        (None, Some(_)) => break,
                        _ => {}
                    }
                }
                if let Some(hdr_end) = header_end {
                    let request_body = total[hdr_end + 4..].to_string();
                    captured_for_task
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(request_body);
                }
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                     Connection: close\r\n\r\n{}",
                    body.len(),
                    body,
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            }
        });
        (format!("http://{}", addr), captured)
    }

    /// Closes a coverage gap the audio-graph-253c part-2 review flagged: every
    /// other `run_projection_job` test in this module drives the
    /// `FnProjectionPatchGenerator` test double, and the executor-level
    /// live-wire test
    /// (`llm::executor::tests::live_executor_dispatch_carries_job_ones_applied_note_id_into_job_twos_wire_request`)
    /// calls `LlmExecutor::generate_projection_patch` directly — so the ONLY
    /// production implementer of `ProjectionPatchGenerator`,
    /// `ExecutorProjectionPatchGenerator`, had zero coverage of its own. This
    /// test drives the REAL chain end to end: `AppState` ->
    /// `ProjectionSchedulers::observe_ledger` -> `run_projection_job` ->
    /// `ExecutorProjectionPatchGenerator::generate_projection_patch` ->
    /// `LlmExecutor` -> the live worker thread -> `run_projection_patch_dispatch`
    /// -> a REAL wire call against a loopback mock, with job 1's outcome
    /// APPLIED by `run_projection_job` itself (exactly as production does)
    /// before job 2 is scheduled and dispatched.
    ///
    /// Revert proof (performed manually during review, not re-executed at
    /// runtime here): changing `ExecutorProjectionPatchGenerator::generate_projection_patch`
    /// to forward `None` instead of `notes` makes this test fail the same way
    /// `llm::executor::tests::live_executor_dispatch_carries_job_ones_applied_note_id_into_job_twos_wire_request`
    /// fails on `run_projection_patch_dispatch`'s own revert: job 2's captured
    /// wire body omits `note:decision`.
    #[test]
    fn app_state_dispatch_through_real_executor_generator_carries_job_ones_note_into_job_twos_wire_request()
     {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("executor-generator-live-chain");
        let _guard = DataDirGuard::set(&dir);

        let rt = tokio::runtime::Runtime::new().expect("rt");
        let app = AppState::new();

        let make_client = |base: &str| {
            crate::llm::ApiClient::new(crate::llm::ApiConfig {
                endpoint: base.to_string(),
                api_key: Some("sk-executor-chain-probe".to_string()),
                model: "probe-model".to_string(),
                max_tokens: 64,
                temperature: 0.1,
            })
            .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::allow())
        };
        let provider_for = |base: &str| LlmProvider::Api {
            endpoint: base.to_string(),
            api_key: "sk-executor-chain-probe".to_string(),
            model: "probe-model".to_string(),
        };

        // ----- Job 1: first Notes-kind tick, no notes exist yet.
        let job_one = {
            let mut ledger = app
                .transcript_ledger
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            ledger
                .apply_event(crate::projections::TranscriptEvent::from(
                    projection_asr_payload(
                        "executor-chain-span-1",
                        1,
                        "Alice chose Soniox for the pilot.",
                        true,
                    ),
                ))
                .expect("seed transcript");
            let mut schedulers = app
                .projection_schedulers
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            match schedulers.observe_ledger(&ledger, 10).notes {
                ProjectionSchedulerDecision::StartJob { job } => job,
                other => panic!("expected notes start job, got {other:?}"),
            }
        };

        let job_one_response = serde_json::json!({
            "choices": [{
                "message": { "content": "{\"operations\":[{\"type\":\"upsert_note\",\
        \"id\":\"note:decision\",\"title\":\"Provider decision\",\"body\":\"Alice chose Soniox for the pilot.\",\
        \"tags\":[],\"evidence\":{\"claim_class\":\"grounded_inference\",\"span_id\":\"executor-chain-span-1\"}}],\
        \"confidence\":0.9}" },
                "finish_reason": "stop"
            }]
        })
        .to_string();
        let (base_one, captured_one) =
            rt.block_on(spawn_capturing_projection_mock(vec![job_one_response]));

        let executor_one = crate::llm::executor::LlmExecutor::new(
            Arc::new(Mutex::new(None)),
            Arc::new(Mutex::new(Some(make_client(&base_one)))),
            Arc::new(Mutex::new(None)),
            Arc::new(Mutex::new(None)),
        );
        let dispatch_one = ProjectionDispatchContext {
            transcript_ledger: app.transcript_ledger.clone(),
            projection_schedulers: app.projection_schedulers.clone(),
            projection_runtime: app.projection_runtime_handle(),
            projection_job_workers: app.projection_job_workers.clone(),
            projection_lane_stopping: app.projection_lane_stopping.clone(),
            event_sink: Arc::new(RecordingProjectionRuntimeEventSink::default()),
            patch_generator: Arc::new(super::ExecutorProjectionPatchGenerator {
                llm_executor: executor_one,
                llm_provider: provider_for(&base_one),
            }),
            llm_provider: provider_for(&base_one),
            llm_allow_cloud_fallbacks: true,
            data_movement_sink: Arc::new(RecordingProjectionDataMovementSink::default()),
        };

        run_projection_job(dispatch_one, job_one);

        assert_eq!(
            captured_one.lock().unwrap_or_else(|e| e.into_inner()).len(),
            1,
            "job 1 must reach the wire exactly once through the real \
             AppState -> ExecutorProjectionPatchGenerator -> LlmExecutor chain"
        );
        {
            let materialized = app
                .materialized_projection_state
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            assert!(
                materialized
                    .notes
                    .notes
                    .iter()
                    .any(|note| note.id == "note:decision"),
                "run_projection_job must apply job 1's patch to the durable \
                 materialized state, exactly as production does"
            );
        }

        // ----- Job 2: a second Notes-kind tick over an extended basis, whose
        // dispatch must observe job 1's applied note through the SAME live
        // AppState -> generator -> executor chain.
        let job_two = {
            let mut ledger = app
                .transcript_ledger
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            ledger
                .apply_event(crate::projections::TranscriptEvent::from(
                    projection_asr_payload(
                        "executor-chain-span-2",
                        1,
                        "Bob confirmed the timeline.",
                        true,
                    ),
                ))
                .expect("seed second transcript span");
            let mut schedulers = app
                .projection_schedulers
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            match schedulers.observe_ledger(&ledger, 20).notes {
                ProjectionSchedulerDecision::StartJob { job } => job,
                other => panic!("expected second notes start job, got {other:?}"),
            }
        };

        let job_two_response = serde_json::json!({
            "choices": [{
                "message": { "content": "{\"operations\":[{\"type\":\"upsert_note\",\
        \"id\":\"note:decision\",\"title\":\"Provider decision\",\"body\":\"Alice chose Soniox for the pilot, confirmed.\",\
        \"tags\":[],\"evidence\":{\"claim_class\":\"grounded_inference\",\"span_id\":\"executor-chain-span-2\"}}],\
        \"confidence\":0.9}" },
                "finish_reason": "stop"
            }]
        })
        .to_string();
        let (base_two, captured_two) =
            rt.block_on(spawn_capturing_projection_mock(vec![job_two_response]));

        let executor_two = crate::llm::executor::LlmExecutor::new(
            Arc::new(Mutex::new(None)),
            Arc::new(Mutex::new(Some(make_client(&base_two)))),
            Arc::new(Mutex::new(None)),
            Arc::new(Mutex::new(None)),
        );
        let dispatch_two = ProjectionDispatchContext {
            transcript_ledger: app.transcript_ledger.clone(),
            projection_schedulers: app.projection_schedulers.clone(),
            projection_runtime: app.projection_runtime_handle(),
            projection_job_workers: app.projection_job_workers.clone(),
            projection_lane_stopping: app.projection_lane_stopping.clone(),
            event_sink: Arc::new(RecordingProjectionRuntimeEventSink::default()),
            patch_generator: Arc::new(super::ExecutorProjectionPatchGenerator {
                llm_executor: executor_two,
                llm_provider: provider_for(&base_two),
            }),
            llm_provider: provider_for(&base_two),
            llm_allow_cloud_fallbacks: true,
            data_movement_sink: Arc::new(RecordingProjectionDataMovementSink::default()),
        };

        run_projection_job(dispatch_two, job_two);

        let job_two_wire_bodies = captured_two
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        assert_eq!(
            job_two_wire_bodies.len(),
            1,
            "job 2 must reach the wire exactly once"
        );
        assert!(
            job_two_wire_bodies[0].contains("note:decision"),
            "job 2's REAL wire request body must carry job 1's applied note id, \
             proven through the AppState -> ExecutorProjectionPatchGenerator -> \
             LlmExecutor -> worker_loop -> run_projection_patch_dispatch chain, \
             got: {}",
            job_two_wire_bodies[0]
        );

        drain_app_writers(&app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Codex P2 (PR #77), re-framed for ADR-0038: the terminal ledger event must
    /// record the identity that ACTUALLY served the call, not the configured
    /// intent, or privacy reports would understate remote flow.
    ///
    /// The original premise — a LocalLlama-configured session served by OpenRouter
    /// via the executor's attempt chain — is now unreachable: a job resolves
    /// exactly one authorized route. The actual-vs-configured property is still
    /// real and still worth pinning, because the SERVED identity can be sharper
    /// than the configured one (`route.openrouter` sharpens into
    /// `route.cerebras_via_openrouter` once the live routing policy is read, and
    /// the served model can differ from the requested slug). This test therefore
    /// keeps the ledger property and drives it from an OpenRouter-configured
    /// session whose provenance reports the OpenRouter registry id, rather than
    /// from a cross-provider hop that no longer exists.
    #[test]
    fn runtime_projection_dispatch_ledgers_served_route_not_configured_intent() {
        use crate::persistence::{DataClass, DataMovementEventType, DestinationBoundary};

        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("projection-dispatch-fallback-ledger");
        let _guard = DataDirGuard::set(&dir);

        let app = AppState::new();
        {
            let mut ledger = app
                .transcript_ledger
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            for i in 0..(crate::projections::ROLLING_SUMMARY_HOT_WINDOW_TURNS + 3) {
                ledger
                    .apply_event(crate::projections::TranscriptEvent::from(
                        projection_asr_payload(
                            &format!("fallback-span-{i}"),
                            1,
                            &format!("Turn {i} content."),
                            true,
                        ),
                    ))
                    .expect("seed transcript");
            }
        }
        let notes_job = {
            let ledger = app
                .transcript_ledger
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let mut schedulers = app
                .projection_schedulers
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            match schedulers.observe_ledger(&ledger, 10).notes {
                ProjectionSchedulerDecision::StartJob { job } => job,
                other => panic!("expected notes start job, got {other:?}"),
            }
        };

        // Provenance reports what the single authorized OpenRouter route served,
        // including a SERVED model slug that differs from the requested one — the
        // shape `projection_openrouter` now stamps (ADR-0038 defect 3).
        let (generator, _calls) =
            FnProjectionPatchGenerator::new(|job, _ledger, _notes, sequence, created_at_ms| {
                let mut patch = test_projection_patch(&job, sequence, created_at_ms);
                patch.provenance.provider = "llm.openrouter".to_string();
                patch.provenance.model = "anthropic/claude-sonnet-4.5".to_string();
                patch.provenance.route_id = Some("route.openrouter".to_string());
                patch.provenance.model_source = crate::llm::route::ModelIdentitySource::Served;
                Ok(ProjectionPatchOutcome {
                    patch,
                    tokens_used: 42,
                    no_op_filtered_count: 0,
                })
            });
        let (dispatch, _event_sink, movements) = projection_dispatch_for_app_with_movement(
            &app,
            generator,
            // Configured LOCAL: the pre-call marker records that intent, and the
            // terminal event records what the served route actually was. The
            // divergence is the property under test; producing it no longer
            // requires a cross-provider hop, only a generator that reports a
            // different served identity.
            LlmProvider::LocalLlama,
            true,
        );
        run_projection_job(dispatch, notes_job);

        let recorded = movements.events.lock().unwrap_or_else(|p| p.into_inner());
        // The pre-call started event records the configured local intent.
        let started = recorded
            .iter()
            .find(|e| e.event_type == DataMovementEventType::ProviderCallStarted)
            .expect("started event");
        assert_eq!(started.destination.boundary, DestinationBoundary::Local);

        // The terminal event records the ACTUAL remote backend.
        let terminal = recorded
            .iter()
            .find(|e| e.event_type == DataMovementEventType::ProviderCallSucceeded)
            .expect("succeeded event");
        assert_eq!(
            terminal.destination.boundary,
            DestinationBoundary::Provider,
            "a remote served route must be ledgered as a remote flow"
        );
        assert_eq!(
            terminal.destination.provider_id.as_deref(),
            Some("llm.openrouter"),
            "terminal event must name the backend that actually served the call"
        );
        assert!(
            terminal.data_classes.contains(&DataClass::Notes),
            "a remote served route carries the rolling summary off-device"
        );
        assert!(
            terminal
                .artifact_refs
                .iter()
                .any(|a| a.kind == "vendor_cached_prompt_prefix"),
            "openrouter fallback sets the cache breakpoint, so the vendor-side prefix is recorded"
        );
        let model = terminal.model.as_ref().expect("model recorded");
        assert_eq!(
            model.model_id.as_deref(),
            Some("anthropic/claude-sonnet-4.5")
        );
        drop(recorded);
        drain_app_writers(&app);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Seed audio-graph-862c, decision Option 1 (decision memo): a FAILED
    /// dispatch's terminal ledger event must stamp the ATTEMPTED route's
    /// registry provider id — the same identity `Actual` already records via
    /// provenance — instead of `LlmProvider::runtime_provider_id()`'s coarse
    /// `llm.api` collapse of every `Api` endpoint. The attempted identity here
    /// comes from the REAL `crate::llm::route` resolver
    /// (`route_for_api_endpoint` + `AttemptedRouteIdentity::for_route`), not a
    /// hand-typed string, so this pins the actual registry mapping a live
    /// Cerebras dispatch resolves to, then proves `run_projection_job` writes
    /// it onto the `FailedRoute` ledger event.
    ///
    /// The session-start snapshot here is the SAME Cerebras endpoint (no
    /// mid-session repoint — that is audio-graph-7da4's separate test): this
    /// test isolates the provider_id convention question, and a plain
    /// unrepointed Cerebras session is the simplest case that still
    /// distinguishes the two conventions, because `requires_cloud_content_transfer()`
    /// already reads `true` off this exact snapshot, so the pre-3624 fallback
    /// (`dispatch.llm_provider.runtime_provider_id()`) would render the SAME
    /// remote destination but under the literal `"llm.api"` — a revert of this
    /// fix flips the assertion below from `"llm.cerebras"` to `"llm.api"` and
    /// fails, pinning the pre-fix convention as refused.
    #[test]
    fn failed_route_ledgers_the_attempted_cerebras_route_not_the_settings_tag() {
        use crate::llm::route::{AttemptedRouteIdentity, route_for_api_endpoint};
        use crate::persistence::{DataMovementEventType, DestinationBoundary};

        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("projection-dispatch-failed-cerebras");
        let _guard = DataDirGuard::set(&dir);

        let app = AppState::new();
        let notes_job = {
            let mut ledger = app
                .transcript_ledger
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            ledger
                .apply_event(crate::projections::TranscriptEvent::from(
                    projection_asr_payload(
                        "failed-cerebras-span",
                        1,
                        "content that never left the device",
                        true,
                    ),
                ))
                .expect("seed transcript");
            let mut schedulers = app
                .projection_schedulers
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            match schedulers.observe_ledger(&ledger, 10).notes {
                ProjectionSchedulerDecision::StartJob { job } => job,
                other => panic!("expected notes start job, got {other:?}"),
            }
        };

        let live = route_for_api_endpoint(crate::settings::CEREBRAS_BASE_URL);
        let attempted =
            AttemptedRouteIdentity::for_route(live, Some(crate::settings::CEREBRAS_BASE_URL));
        assert_eq!(attempted.provider_id, "llm.cerebras");
        assert!(attempted.requires_cloud_transfer);

        let (generator, _calls) =
            FnProjectionPatchGenerator::new(|job, _ledger, _notes, _sequence, _created_at_ms| {
                Err(format!(
                    "simulated Cerebras dispatch failure for {:?}",
                    job.kind
                ))
            });
        let generator = generator.with_attempted_route(attempted);

        let (dispatch, _event_sink, movements) = projection_dispatch_for_app_with_movement(
            &app,
            generator,
            LlmProvider::Api {
                endpoint: crate::settings::CEREBRAS_BASE_URL.to_string(),
                api_key: "sk-cerebras".to_string(),
                model: "gpt-oss-120b".to_string(),
            },
            true,
        );
        run_projection_job(dispatch, notes_job);

        let recorded = movements.events.lock().unwrap_or_else(|p| p.into_inner());
        let failed = recorded
            .iter()
            .find(|e| e.event_type == DataMovementEventType::ProviderCallFailed)
            .expect("failed event ledgered");
        assert_eq!(failed.destination.boundary, DestinationBoundary::Provider);
        assert_eq!(
            failed.destination.provider_id.as_deref(),
            Some("llm.cerebras"),
            "a failed Cerebras dispatch must ledger the attempted route's registry \
             id, not the settings-variant tag \"llm.api\""
        );
        drop(recorded);
        drain_app_writers(&app);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Seed audio-graph-7da4, decision Option 1: a mid-session repoint from a
    /// loopback `Api` endpoint to a cloud endpoint whose dispatch subsequently
    /// FAILS must ledger remote egress, computed from the attempted route
    /// captured live at dispatch time — never from the session-start
    /// `LlmProvider` snapshot. Before this fix `FailedRoute` read
    /// `dispatch.llm_provider.requires_cloud_content_transfer()`, which is
    /// exactly the stale loopback snapshot named in the decision memo's §4
    /// trace; a revert makes this test observe `DestinationBoundary::Local`
    /// instead of `Provider` and fail.
    #[test]
    fn failed_route_from_a_mid_session_loopback_to_cloud_repoint_ledgers_remote_egress() {
        use crate::llm::route::{AttemptedRouteIdentity, route_for_api_endpoint};
        use crate::persistence::{DataMovementEventType, DestinationBoundary};

        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("projection-dispatch-failed-repoint");
        let _guard = DataDirGuard::set(&dir);

        let app = AppState::new();
        let notes_job = {
            let mut ledger = app
                .transcript_ledger
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            ledger
                .apply_event(crate::projections::TranscriptEvent::from(
                    projection_asr_payload(
                        "failed-repoint-span",
                        1,
                        "content dialled after a mid-session repoint",
                        true,
                    ),
                ))
                .expect("seed transcript");
            let mut schedulers = app
                .projection_schedulers
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            match schedulers.observe_ledger(&ledger, 10).notes {
                ProjectionSchedulerDecision::StartJob { job } => job,
                other => panic!("expected notes start job, got {other:?}"),
            }
        };

        // The identity a real dispatch would have captured LIVE from the
        // re-pointed client, AFTER the session-start snapshot below was taken.
        // Same-descriptor (`llm.api`) repoints — loopback Ollama to any generic
        // cloud `Api` endpoint — are accepted by
        // `AuthorizedRoute::refine_within_authorization`, so this is the
        // majority case the decision memo's §4 names, not the narrower
        // Cerebras/SambaNova one.
        let live = route_for_api_endpoint("https://api.openai.com/v1");
        let attempted = AttemptedRouteIdentity::for_route(live, Some("https://api.openai.com/v1"));
        assert_eq!(attempted.provider_id, "llm.api");
        assert!(attempted.requires_cloud_transfer);

        let (generator, _calls) =
            FnProjectionPatchGenerator::new(|job, _ledger, _notes, _sequence, _created_at_ms| {
                Err(format!(
                    "simulated post-repoint dispatch failure for {:?}",
                    job.kind
                ))
            });
        let generator = generator.with_attempted_route(attempted);

        let (dispatch, _event_sink, movements) = projection_dispatch_for_app_with_movement(
            &app,
            generator,
            // Session-start SNAPSHOT: loopback. The pre-fix fallback
            // (`requires_cloud_content_transfer()` on THIS value) would say
            // `false` even though the live client was repointed to a cloud
            // endpoint mid-session and that dispatch failed.
            LlmProvider::Api {
                endpoint: "http://127.0.0.1:11434/v1".to_string(),
                api_key: String::new(),
                model: "llama3.2".to_string(),
            },
            true,
        );
        run_projection_job(dispatch, notes_job);

        let recorded = movements.events.lock().unwrap_or_else(|p| p.into_inner());
        let failed = recorded
            .iter()
            .find(|e| e.event_type == DataMovementEventType::ProviderCallFailed)
            .expect("failed event ledgered");
        assert_eq!(
            failed.destination.boundary,
            DestinationBoundary::Provider,
            "a failed dispatch to a live-repointed cloud endpoint must ledger \
             remote egress; the pre-fix code read the stale loopback snapshot \
             and would have ledgered Local here"
        );
        drop(recorded);
        drain_app_writers(&app);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Seed audio-graph-7da4's second half: the memo names
    /// `actual_backend_identity`'s generic `llm.api` arm as carrying the SAME
    /// staleness as `FailedRoute` — it fell back to
    /// `dispatch.llm_provider.requires_cloud_content_transfer()` (the
    /// session-start snapshot) even for a call that SUCCEEDED after a
    /// mid-session repoint. Drives a successful dispatch whose provenance
    /// reports the generic `"llm.api"` backend (not sharpened to
    /// `llm.cerebras`/`llm.sambanova`) through `run_projection_job`, with the
    /// snapshot loopback but the attempted route cloud — a revert makes this
    /// test observe `DestinationBoundary::Local` instead of `Provider`.
    #[test]
    fn actual_route_from_a_mid_session_loopback_to_cloud_repoint_ledgers_remote_egress() {
        use crate::llm::route::{AttemptedRouteIdentity, route_for_api_endpoint};
        use crate::persistence::{DataMovementEventType, DestinationBoundary};

        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("projection-dispatch-actual-repoint");
        let _guard = DataDirGuard::set(&dir);

        let app = AppState::new();
        let notes_job = {
            let mut ledger = app
                .transcript_ledger
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            ledger
                .apply_event(crate::projections::TranscriptEvent::from(
                    projection_asr_payload(
                        "actual-repoint-span",
                        1,
                        "content served after a mid-session repoint",
                        true,
                    ),
                ))
                .expect("seed transcript");
            let mut schedulers = app
                .projection_schedulers
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            match schedulers.observe_ledger(&ledger, 10).notes {
                ProjectionSchedulerDecision::StartJob { job } => job,
                other => panic!("expected notes start job, got {other:?}"),
            }
        };

        let live = route_for_api_endpoint("https://api.openai.com/v1");
        let attempted = AttemptedRouteIdentity::for_route(live, Some("https://api.openai.com/v1"));
        assert_eq!(attempted.provider_id, "llm.api");
        assert!(attempted.requires_cloud_transfer);

        let (generator, _calls) =
            FnProjectionPatchGenerator::new(|job, _ledger, _notes, sequence, created_at_ms| {
                let mut patch = test_projection_patch(&job, sequence, created_at_ms);
                // Generic served identity — NOT sharpened to a pinned
                // accelerator — so this exercises the ambiguous arm of
                // `actual_backend_identity` that used to fall back to the
                // stale snapshot.
                patch.provenance.provider = "llm.api".to_string();
                Ok(ProjectionPatchOutcome {
                    patch,
                    tokens_used: 12,
                    no_op_filtered_count: 0,
                })
            });
        let generator = generator.with_attempted_route(attempted);

        let (dispatch, _event_sink, movements) = projection_dispatch_for_app_with_movement(
            &app,
            generator,
            // Session-start SNAPSHOT: loopback. The pre-fix
            // `actual_backend_identity` fallback would say `false` here even
            // though the live client was repointed to a cloud endpoint
            // mid-session and THIS call succeeded on it.
            LlmProvider::Api {
                endpoint: "http://127.0.0.1:11434/v1".to_string(),
                api_key: String::new(),
                model: "llama3.2".to_string(),
            },
            true,
        );
        run_projection_job(dispatch, notes_job);

        let recorded = movements.events.lock().unwrap_or_else(|p| p.into_inner());
        let succeeded = recorded
            .iter()
            .find(|e| e.event_type == DataMovementEventType::ProviderCallSucceeded)
            .expect("succeeded event ledgered");
        assert_eq!(
            succeeded.destination.boundary,
            DestinationBoundary::Provider,
            "a successful dispatch to a live-repointed cloud endpoint must ledger \
             remote egress; the pre-fix code read the stale loopback snapshot \
             and would have ledgered Local here"
        );
        drop(recorded);
        drain_app_writers(&app);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Seed audio-graph-862c, revisited: `Configured` used to stamp
    /// `LlmProvider::runtime_provider_id()` — the coarse settings-variant tag
    /// (`llm.api` for every `Api` endpoint) — even after `Actual`/
    /// `FailedRoute` were sharpened to the registry id the route actually
    /// reached. `isContentEgress` (sessionDataRoute.ts) accepts the `started`
    /// (Configured) row on its own merits, so a session that only ever
    /// egressed to Cerebras rendered a SECOND producer row for the generic
    /// `llm.api` tag next to the sharp `llm.cerebras` terminal row — the exact
    /// harm 862c names, still open even after the terminal events were fixed.
    /// Drives a dispatch whose session-start snapshot is a Cerebras-shaped
    /// `Api` endpoint through `run_projection_job` and asserts the STARTED
    /// event already carries the sharp `llm.cerebras` id from
    /// `resolve_route`, resolved from the snapshot alone (no live client, no
    /// dispatch yet) — a revert makes this test observe `llm.api` instead.
    #[test]
    fn configured_event_stamps_the_sharp_registry_id_not_the_settings_tag() {
        use crate::persistence::DataMovementEventType;

        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("projection-dispatch-configured-sharpening");
        let _guard = DataDirGuard::set(&dir);

        let app = AppState::new();
        let notes_job = {
            let mut ledger = app
                .transcript_ledger
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            ledger
                .apply_event(crate::projections::TranscriptEvent::from(
                    projection_asr_payload(
                        "configured-sharpening-span",
                        1,
                        "content projected under a Cerebras-shaped snapshot",
                        true,
                    ),
                ))
                .expect("seed transcript");
            let mut schedulers = app
                .projection_schedulers
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            match schedulers.observe_ledger(&ledger, 10).notes {
                ProjectionSchedulerDecision::StartJob { job } => job,
                other => panic!("expected notes start job, got {other:?}"),
            }
        };

        let (generator, _calls) =
            FnProjectionPatchGenerator::new(|job, _ledger, _notes, sequence, created_at_ms| {
                Ok(ProjectionPatchOutcome {
                    patch: test_projection_patch(&job, sequence, created_at_ms),
                    tokens_used: 5,
                    no_op_filtered_count: 0,
                })
            });

        let (dispatch, _event_sink, movements) = projection_dispatch_for_app_with_movement(
            &app,
            generator,
            LlmProvider::Api {
                endpoint: crate::settings::CEREBRAS_BASE_URL.to_string(),
                api_key: String::new(),
                model: "gpt-oss-120b".to_string(),
            },
            true,
        );
        run_projection_job(dispatch, notes_job);

        let recorded = movements.events.lock().unwrap_or_else(|p| p.into_inner());
        let started = recorded
            .iter()
            .find(|e| e.event_type == DataMovementEventType::ProviderCallStarted)
            .expect("started event ledgered");
        assert_eq!(
            started.destination.provider_id.as_deref(),
            Some("llm.cerebras"),
            "the pre-dispatch Configured row must name the sharp registry id, \
             not the coarse llm.api settings-variant tag a revert would produce"
        );
        drop(recorded);
        drain_app_writers(&app);

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
            FnProjectionPatchGenerator::new(|job, _ledger, _notes, _sequence, _created_at_ms| {
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
    fn runtime_projection_dispatch_follows_up_append_only_apply_with_current_basis() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("projection-dispatch-stale-repair");
        let _guard = DataDirGuard::set(&dir);

        let app = AppState::new();
        let mutated = Arc::new(AtomicBool::new(false));
        let ledger_for_mutation = app.transcript_ledger.clone();
        let mutated_for_generator = mutated.clone();
        let (generator, calls) = FnProjectionPatchGenerator::new(
            move |job, _ledger, _notes, sequence, created_at_ms| {
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
                    no_op_filtered_count: 0,
                })
            },
        );
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

        // audio-graph-caad / audio-graph-f3d4: the append-only apply above
        // must not be discarded as stale — it materializes its own note over
        // the original 1-span basis — and the scheduler still starts exactly
        // one Background follow-up over the now-current 2-span basis.
        wait_until("append-only projection follow-up completes", || {
            let materialized = app
                .materialized_projection_state
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let schedulers = app
                .projection_schedulers
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            materialized.notes.notes.len() == 2
                && schedulers.notes().metrics().stale_discards == 0
                && schedulers.notes().metrics().repair_jobs_started == 0
                && schedulers.notes().metrics().follow_up_jobs_started == 1
                && schedulers.notes().metrics().completed_jobs == 2
                && schedulers.notes().in_flight_job().is_none()
        });

        {
            let materialized = app
                .materialized_projection_state
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let mut basis_lengths: Vec<usize> = materialized
                .notes
                .notes
                .iter()
                .map(|note| note.basis.span_revisions.len())
                .collect();
            basis_lengths.sort_unstable();
            assert_eq!(
                basis_lengths,
                vec![1, 2],
                "the append-only apply must materialize its own note over the original \
                 1-span basis, alongside the follow-up's note over the full 2-span basis"
            );
        }

        assert!(
            calls.load(Ordering::SeqCst) >= 2,
            "append-only apply should generate the original and one background follow-up"
        );
        assert_eq!(event_sink.patch_count(), 2);
        assert_eq!(event_sink.notes_count(), 2);
        assert_eq!(event_sink.graph_count(), 0);

        // Idle next tick: the follow-up's basis is now current, so observing
        // the ledger again must not start further work.
        {
            let ledger = app
                .transcript_ledger
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let mut schedulers = app
                .projection_schedulers
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let observation = schedulers.observe_ledger(&ledger, current_unix_millis());
            assert!(matches!(
                observation.notes,
                ProjectionSchedulerDecision::Idle
            ));
        }

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
            FnProjectionPatchGenerator::new(|job, _ledger, _notes, sequence, created_at_ms| {
                Ok(ProjectionPatchOutcome {
                    patch: test_projection_patch(&job, sequence, created_at_ms),
                    tokens_used: 37,
                    no_op_filtered_count: 0,
                })
            });
        let (dispatch, event_sink) = projection_dispatch_for_app(&app, generator);
        let writer = app.transcript_event_writer.clone();
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
        let writer_fixture = AcceptingTranscriptEventWriterFixture::new(session_id);
        let writer = writer_fixture.writer();
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
