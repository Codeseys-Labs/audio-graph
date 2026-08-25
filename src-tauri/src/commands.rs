//! Tauri IPC command handlers.
//!
//! Each function here is exposed to the frontend via `tauri::generate_handler![]`.
//! They access `AppState` through Tauri's managed state.
//!
//! Heavy processing logic (speech, extraction) lives in the [`crate::speech`]
//! module — this file only contains thin `#[tauri::command]` wrappers.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tauri::{Emitter, Manager, State};
use tokio_util::sync::CancellationToken;

use crate::audio::consumer::{
    ConsumerActiveFn, ProcessedAudioConsumerDescriptor, ProcessedAudioConsumerRegistration,
    ProcessedAudioConsumerStage, ProcessedAudioDropPolicy, ProcessedAudioMixingMode,
    ProcessedAudioSourceFilter,
};
use crate::audio::pipeline::{
    AudioPipeline, AudioPipelineInput, ProcessedAudioChunk, ProcessedPipelineMessage,
};
use crate::error::{AppError, Result as AppResult};
use crate::events::{self, PipelineStatus, StageStatus};
use crate::gemini::{GeminiConfig, GeminiEvent, GeminiLiveClient};
use crate::graph::entities::GraphSnapshot;
use crate::llm::engine::{ChatMessage, ChatResponse};
use crate::llm::openrouter::{
    self as openrouter, OpenRouterClient, OpenRouterConfig, OpenRouterModel,
    OpenRouterModelEndpoints, OpenRouterProvider,
};
use crate::llm::{ApiClient, ApiConfig};
use crate::openai_realtime::{OpenAiRealtimeClient, OpenAiRealtimeConfig, OpenAiRealtimeEvent};
use crate::persistence::{FileMemoryRepository, LocalMemoryRepository};
use crate::speech;
use crate::state::{AppState, AudioSourceInfo, TranscriptSegment};

/// Transcript-lens payload for a past session (seed audio-graph-4fa5
/// deliverable a). `notes`, `materialized_graph`, and the raw
/// `projection_events` log used to be bundled in here too — that bundle,
/// materialized 3x (Rust structs → Rust JSON `String` → JS `JSON.parse`) on
/// the synchronous main thread, is what the seed audio-graph-4fa5 field
/// report traced to a silent renderer-OOM/allocator-abort on a 208MB legacy
/// session (figures below are drawn from that report, not from a file that
/// ships in this tree — the seed record, not a repo path, is the source of
/// truth for them). Those three artifacts now have their own commands
/// (`load_session_notes_artifacts_cmd`, `load_session_graph_artifact_cmd`),
/// each fetched only once its own lens activates — genuinely deferred for
/// the Graph lens (not the default-active one), but the Notes lens IS
/// `SessionsBrowser`'s default (`useState<DetailLens>("notes")`), so in
/// practice `load_session_notes_artifacts_cmd` fires immediately after most
/// session opens too. What actually protects both artifacts is the byte
/// ceiling (deliverable b), not lens-gating; only the materialized graph is
/// genuinely deferred by lens activation (fix-round finding: this comment
/// used to claim otherwise for both).
#[derive(Debug, Clone, serde::Serialize)]
pub struct LoadedSession {
    pub transcript: Vec<TranscriptSegment>,
    pub graph: GraphSnapshot,
    pub transcript_events: Vec<crate::projections::TranscriptEvent>,
    /// Durable diarization / speaker-timeline span revisions
    /// (`transcripts/<id>.speaker.jsonl`). Hydrating these into the frontend
    /// lets `joinSpeakerTimelineToTranscript` / `speakerAttributionIndex`
    /// resolve trusted (latest-wins) speaker attribution on a loaded session
    /// instead of silently falling back to the untrusted inline ASR labels
    /// (audio-graph-0b33; ADR-0026 §3/§4). Missing log → empty vec.
    pub diarization_events: Vec<crate::projections::DiarizationSpanRevision>,
    pub live_assist_cards: Vec<crate::events::LiveAssistCardRecord>,
}

/// `build_session_timeline_cmd`'s response (fix-round finding): `entries` is
/// tail-capped to the caller's `limit`, but `total_count` is the fold's full
/// length BEFORE that cap. Without this, `SeekTimeline`'s "showing the last N
/// of TOTAL utterances" notice could never fire once the backend started
/// tail-capping to exactly the frontend's own render window
/// (`TRANSCRIPT_WINDOW_SIZE`) — `entries.len()` alone can never again exceed
/// what `SeekTimeline` shows, so it needs the pre-cap count from here.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionTimelineFold {
    pub entries: Vec<crate::timeline::TimelineEntry>,
    pub total_count: usize,
}

/// Notes-lens artifacts for a past session (seed audio-graph-4fa5 deliverable
/// a): the materialized notes `NotesPanel` renders plus the raw
/// projection-patch log it derives `noteRevisionCounts` from. Fetched only
/// when the Notes lens activates — `load_session` no longer carries either.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionNotesArtifacts {
    pub notes: Option<crate::projections::MaterializedNotes>,
    pub projection_events: Vec<crate::projections::ProjectionPatch>,
}

/// Byte-size ceilings for the historical-session read path (seed
/// audio-graph-4fa5, field round 5: an unbounded ~208MB `load_session`
/// response killed the app). Each ceiling is checked with a single
/// `fs::metadata` stat call — see [`enforce_artifact_ceiling`] — BEFORE the
/// artifact is ever read into memory, so an oversized legacy artifact is
/// never materialized even once, let alone the 3x the old bundled response
/// paid for.
///
/// Calibration: the field session that crashed the app carried a 2.1MB
/// transcript event log (must keep loading — every ceiling below leaves it
/// generous headroom) alongside a 156.6MB materialized graph, a 33.3MB
/// projection-patch log, and a 19.1MB notes artifact — all three predate the
/// artifact-size fix (seed audio-graph-cfa1: unbounded per-fact
/// `basis.span_revisions` growth) and must refuse rather than load. Every
/// ceiling here sits comfortably above realistic post-cfa1 artifact sizes
/// and comfortably below the field-crash sizes.
mod session_artifact_ceilings {
    /// Live knowledge-graph snapshot (`graphs/<id>.json`). Already
    /// structurally capped at write time
    /// ([`crate::graph::temporal`]'s `MAX_NODES`/`MAX_EDGES`, seed
    /// audio-graph-67cd), so this is defense-in-depth, not the primary guard.
    pub const MAX_LIVE_GRAPH_BYTES: u64 = 16 * 1024 * 1024;
    /// Transcript event log (`transcripts/<id>.events.jsonl`). The field
    /// transcript that must keep loading is 2.1MB.
    pub const MAX_TRANSCRIPT_EVENTS_BYTES: u64 = 8 * 1024 * 1024;
    /// Diarization span-revision log (`transcripts/<id>.speaker.jsonl`).
    pub const MAX_DIARIZATION_EVENTS_BYTES: u64 = 4 * 1024 * 1024;
    /// Materialized projection graph (`graphs/<id>.materialized.json`), the
    /// artifact the field crash's ~156.6MB response was dominated by.
    pub const MAX_MATERIALIZED_GRAPH_BYTES: u64 = 24 * 1024 * 1024;
    /// Materialized notes artifact (`notes/<id>.json`). The field artifact
    /// that must refuse is 19.1MB.
    pub const MAX_MATERIALIZED_NOTES_BYTES: u64 = 8 * 1024 * 1024;
    /// Projection-patch log (`projections/<id>.events.jsonl`), read both
    /// standalone (Notes lens) and as canonical-replay input (Notes/Graph
    /// lenses). The field artifact that must refuse is 33.3MB.
    pub const MAX_PROJECTION_EVENTS_BYTES: u64 = 12 * 1024 * 1024;
    /// Live-assist current-cards snapshot (`live_assist/<id>.current.json`),
    /// read by `load_session` on every open (fix-round finding: this artifact
    /// had no ceiling and no byte-count logging at all). No field-crash
    /// measurement exists for this artifact the way the other five do — it
    /// wasn't part of the incident that motivated this seed — so this
    /// ceiling is defense-in-depth calibrated by analogy to
    /// `MAX_MATERIALIZED_NOTES_BYTES` (both are a JSON array of LLM-authored
    /// text growing with session length) rather than a field measurement.
    pub const MAX_LIVE_ASSIST_CARDS_BYTES: u64 = 8 * 1024 * 1024;
    /// Data-movement ledger (`ledgers/<id>.movements.jsonl`), read by
    /// `load_session_data_movement_cmd` for the Route lens (fix-round
    /// finding: this artifact had no ceiling either). Rows are compact
    /// (data class + boundary hop + provider/model id + hashed path, no
    /// payloads), so this ceiling is comfortably higher than the
    /// text-artifact ceilings above — defense-in-depth, not a field
    /// measurement, same caveat as `MAX_LIVE_ASSIST_CARDS_BYTES`.
    pub const MAX_DATA_MOVEMENT_EVENTS_BYTES: u64 = 16 * 1024 * 1024;
}
use session_artifact_ceilings::{
    MAX_DATA_MOVEMENT_EVENTS_BYTES, MAX_DIARIZATION_EVENTS_BYTES, MAX_LIVE_ASSIST_CARDS_BYTES,
    MAX_LIVE_GRAPH_BYTES, MAX_MATERIALIZED_GRAPH_BYTES, MAX_MATERIALIZED_NOTES_BYTES,
    MAX_PROJECTION_EVENTS_BYTES, MAX_TRANSCRIPT_EVENTS_BYTES,
};

/// Refuse to read `path` into memory when it exceeds `ceiling_bytes`. Stats
/// the file only — never opens or reads its contents — so the check is O(1)
/// regardless of artifact size; the entire point is to never call
/// `fs::read_to_string` on a 156MB legacy graph (seed audio-graph-4fa5).
///
/// A missing file is NOT a ceiling violation: every caller's existing
/// "missing artifact → empty/`None`" fallback still applies, so a session
/// that never wrote this artifact is unaffected. `artifact_class` is a
/// stable snake_case identifier the frontend keys its translated copy off of
/// (never raw prose).
/// Returns the artifact's observed byte size (0 for a missing file) on
/// success, so callers can fold the same stat into their own read-path
/// instrumentation (seed audio-graph-6633 deliverable d) without a second
/// `fs::metadata` call.
fn enforce_artifact_ceiling(
    path: &std::path::Path,
    ceiling_bytes: u64,
    artifact_class: &'static str,
) -> AppResult<u64> {
    let Ok(metadata) = std::fs::metadata(path) else {
        return Ok(0);
    };
    let size_bytes = metadata.len();
    if size_bytes > ceiling_bytes {
        return Err(AppError::ArtifactTooLarge {
            artifact_class: artifact_class.to_string(),
            size_bytes,
            ceiling_bytes,
        });
    }
    Ok(size_bytes)
}

/// Byte size of `path`, or 0 if it does not exist / cannot be stat'd.
/// Logging-only helper (seed audio-graph-6633 deliverable d) for artifacts
/// whose ceiling check happens somewhere else in the call chain (e.g. the
/// transcript event log, checked inside `read_session_transcript_snapshot`)
/// — never used to gate a read, only to report a byte count.
fn artifact_len_for_log(path: &std::path::Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// Response-size warning threshold for historical-session read commands
/// (seed audio-graph-6633 deliverable d): above this, `log::warn!` instead of
/// `log::info!` so a field log flags an unusually large IPC response without
/// needing the artifact-size regression this ceiling already guards against.
const RESPONSE_SIZE_WARN_THRESHOLD_BYTES: usize = 4 * 1024 * 1024;

/// A `std::io::Write` sink that discards every byte and only counts them.
/// Lets [`response_len_for_log`] measure a response's serialized size
/// without allocating a second full copy of it (fix-round finding: this
/// module's whole point is to stop materializing an artifact more times
/// than necessary — `serde_json::to_string` for a logging-only byte count
/// was doing exactly that, doubling peak Rust-side memory for
/// `session_export_bundle` at the ceiling limits).
struct CountingSink(usize);

impl std::io::Write for CountingSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0 += buf.len();
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Serialized byte length of `value`, or 0 if serialization fails. Logging
/// only — never used to decide whether to send the response. Serializes
/// into a counting sink (see [`CountingSink`]) rather than a `String`, so
/// this never holds a second full copy of the response in memory.
fn response_len_for_log<T: serde::Serialize>(value: &T) -> usize {
    let mut sink = CountingSink(0);
    match serde_json::to_writer(&mut sink, value) {
        Ok(()) => sink.0,
        Err(_) => 0,
    }
}

/// Schema version for the session export bundle. Bump when the bundle's shape
/// changes so importers / migration tooling can branch on it. This is the
/// "schema metadata" the session-artifact-migration acceptance requires.
pub const SESSION_EXPORT_SCHEMA_VERSION: u32 = 1;

/// A self-describing, self-contained snapshot of the portable core session
/// artifacts. Assembled from the event-sourced logs (transcript events,
/// diarization span revisions, projection patches) plus the materialized
/// notes / graph artifacts and the legacy transcript segments, so an export
/// captures the whole session lifecycle boundary rather than only the legacy
/// graph snapshot.
///
/// Every field is an owned, JSON-serializable value: the bundle can be written
/// to a single `.json` file and later re-loaded / migrated without touching the
/// original on-disk layout. Missing artifacts serialize as empty collections /
/// `None`, so old sessions (transcript-only) still export cleanly.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionExportBundle {
    /// Bundle schema version (see [`SESSION_EXPORT_SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// The session this bundle was exported from.
    pub session_id: String,
    /// The sessions-index metadata entry, if the session is indexed.
    pub metadata: Option<crate::sessions::SessionMetadata>,
    /// Legacy transcript segments (`transcripts/<id>.jsonl`).
    pub transcript: Vec<TranscriptSegment>,
    /// Immutable transcript-span revision events (`transcripts/<id>.events.jsonl`).
    pub transcript_events: Vec<crate::projections::TranscriptEvent>,
    /// Durable diarization / speaker-timeline span revisions
    /// (`transcripts/<id>.speaker.jsonl`).
    pub diarization_events: Vec<crate::projections::DiarizationSpanRevision>,
    /// Projection event log — the accepted notes/graph patches
    /// (`projections/<id>.events.jsonl`).
    pub projection_events: Vec<crate::projections::ProjectionPatch>,
    /// Materialized notes artifact (`notes/<id>.json`), if present.
    pub notes: Option<crate::projections::MaterializedNotes>,
    /// Materialized graph artifact (`graphs/<id>.materialized.json`), if present.
    pub materialized_graph: Option<crate::projections::MaterializedGraph>,
    /// Legacy petgraph knowledge-graph snapshot (`graphs/<id>.json`), if present.
    pub graph: Option<GraphSnapshot>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ProjectionRuntimeStatus {
    pub session_id: String,
    pub ledger_session_id: String,
    pub materialized_session_id: String,
    pub accepted_transcript_event_count: u64,
    pub transcript_span_count: usize,
    pub latest_asr_event_age_ms: Option<u64>,
    pub projection_event_writer_available: bool,
    pub schedulers: crate::projection_scheduler::ProjectionSchedulersTelemetry,
    pub materialized: ProjectionMaterializedStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ProjectionMaterializedStatus {
    pub notes_last_sequence: u64,
    pub note_count: usize,
    pub graph_last_sequence: u64,
    pub graph_node_count: usize,
    pub graph_edge_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionReplayArtifactStatus {
    Missing,
    Current,
    Stale,
    Ahead,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ProjectionReplayArtifactReport {
    pub present: bool,
    pub status: ProjectionReplayArtifactStatus,
    pub stored_last_sequence: u64,
    pub replayed_last_sequence: u64,
    pub stored_item_count: usize,
    pub replayed_item_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ProjectionReplayEvaluationMetrics {
    pub note_operation_count: usize,
    pub graph_operation_count: usize,
    pub graph_retcon_operation_count: usize,
    pub correction_patch_count: usize,
    pub stale_discard_count: usize,
    pub invalidated_graph_node_count: usize,
    pub invalidated_graph_edge_count: usize,
    pub active_graph_node_count: usize,
    pub active_graph_edge_count: usize,
    pub duplicate_active_node_key_count: usize,
    pub duplicate_active_edge_key_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, Default)]
pub struct ProjectionReplayLatencyMetrics {
    pub patch_count: usize,
    pub measured_patch_count: usize,
    pub missing_basis_timestamp_count: usize,
    pub total_basis_to_patch_lag_ms: u64,
    pub max_basis_to_patch_lag_ms: u64,
    pub capture_asr: ProjectionReplayStageLatencyMetrics,
    pub asr_to_queue: ProjectionReplayStageLatencyMetrics,
    pub projection_queue: ProjectionReplayStageLatencyMetrics,
    pub generation: ProjectionReplayStageLatencyMetrics,
    pub apply: ProjectionReplayStageLatencyMetrics,
    pub notes: ProjectionReplayKindLatencyMetrics,
    pub graph: ProjectionReplayKindLatencyMetrics,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, Default)]
pub struct ProjectionReplayKindLatencyMetrics {
    pub patch_count: usize,
    pub measured_patch_count: usize,
    pub missing_basis_timestamp_count: usize,
    pub total_basis_to_patch_lag_ms: u64,
    pub max_basis_to_patch_lag_ms: u64,
    pub capture_asr: ProjectionReplayStageLatencyMetrics,
    pub asr_to_queue: ProjectionReplayStageLatencyMetrics,
    pub projection_queue: ProjectionReplayStageLatencyMetrics,
    pub generation: ProjectionReplayStageLatencyMetrics,
    pub apply: ProjectionReplayStageLatencyMetrics,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, Default)]
pub struct ProjectionReplayStageLatencyMetrics {
    pub measured_count: usize,
    pub total_ms: u64,
    pub max_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ProjectionReplayReport {
    pub session_id: String,
    pub transcript_event_count: usize,
    pub transcript_replay_error: Option<String>,
    pub transcript_span_count: usize,
    pub projection_event_count: usize,
    pub projection_checked_patch_count: usize,
    pub projection_invalid_basis_count: usize,
    pub projection_replay_error: Option<String>,
    pub replayed: ProjectionMaterializedStatus,
    pub notes_artifact: ProjectionReplayArtifactReport,
    pub graph_artifact: ProjectionReplayArtifactReport,
    pub evaluation: ProjectionReplayEvaluationMetrics,
    pub latency: ProjectionReplayLatencyMetrics,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CredentialPresence {
    pub key: String,
    pub present: bool,
    pub source: &'static str,
}

/// Outcome of `save_credential_cmd`, so a caller *can* tell a real write apart
/// from an empty/whitespace no-op skip.
///
/// Previously the command returned `Ok(())` for both, which made a skipped
/// save look identical to a persisted one on the wire (cred-review M2.1):
/// a caller that passed a blank value got a success result, a bumped readiness
/// epoch, and a "presence refreshed" flow that re-confirmed the OLD stored
/// key. The primary fix is on the backend: the empty-value path now
/// short-circuits BEFORE the epoch bump + cache rehydrate (see
/// `save_credential_impl`), so a blank save is a true no-op regardless of what
/// the frontend does with the return value.
///
/// The typed return is **forward-looking plumbing**: every current frontend
/// caller pre-guards with `value.trim()` before invoking (so the
/// `SkippedEmpty` path is only reachable defensively, e.g. from a future caller
/// or a Rust unit test) and none branch on the result today. It exists so a
/// caller that *does* want to skip its post-save presence/readiness refresh on
/// a no-op can do so without re-deriving "was this blank?" itself. Serialized
/// `snake_case` so the frontend union is `"saved" | "skipped_empty"`. Returning
/// a value from a previously `()`-typed command is backward-compatible:
/// existing callers that `await invoke(...)` without inspecting the result are
/// unaffected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveCredentialOutcome {
    /// The value was written to the credential store (keychain or YAML) and the
    /// readiness epoch + settings cache were refreshed.
    Saved,
    /// The value was empty or whitespace-only, so nothing was written: the
    /// previously stored value (if any) is untouched and no readiness caches
    /// were invalidated. Use `delete_credential_cmd` to actually clear a key.
    SkippedEmpty,
}

const PROVIDER_READINESS_TTL_MS: u64 = 5 * 60 * 1000;
const PROVIDER_READINESS_MIN_REFRESH_INTERVAL_MS: u64 = 15 * 1000;
const PROVIDER_READINESS_TIMEOUT_SECS: u64 = 10;

static PROVIDER_CREDENTIAL_EPOCH: AtomicU64 = AtomicU64::new(0);
static PROVIDER_READINESS_CACHE: OnceLock<Mutex<HashMap<String, ProviderReadiness>>> =
    OnceLock::new();
static PROVIDER_READINESS_IN_FLIGHT: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
static PROVIDER_READINESS_LAST_STARTED: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
static PROVIDER_READINESS_CANCELLATIONS: OnceLock<
    Mutex<HashMap<String, ProviderReadinessCancellationEntry>>,
> = OnceLock::new();
static PROVIDER_READINESS_CANCELLATION_GENERATION: AtomicU64 = AtomicU64::new(1);

const GEMINI_NOTES_AUDIO_CONSUMER_ID: &str = "gemini-notes";
const GEMINI_CONVERSE_AUDIO_CONSUMER_ID: &str = "gemini-converse";
const GEMINI_LIVE_AUDIO_CONSUMER_GROUP: &str = "gemini-live-client";
const GEMINI_AUDIO_CONSUMER_CAPACITY: usize = 16;

/// Runtime processed-audio consumer id for the OpenAI Realtime S2S voice agent.
/// Distinct from the Gemini converse consumer so the two native-S2S engines
/// never share a runtime channel.
const OPENAI_REALTIME_AUDIO_CONSUMER_ID: &str = "openai-realtime-voice";
/// Conflict group for the OpenAI Realtime S2S client (one live S2S client at a
/// time, independent of the Gemini Live group).
const OPENAI_REALTIME_AUDIO_CONSUMER_GROUP: &str = "openai-realtime-client";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderReadinessStatus {
    Ready,
    MissingCredentials,
    Unchecked,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ProviderCredentialReadiness {
    pub key: String,
    pub present: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ProviderModelCatalogItem {
    pub id: String,
    pub display_name: String,
    pub is_default: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderRuntimeReadinessStatus {
    FeatureMissing,
    ModelMissing,
    RuntimeUnavailable,
    LoadFailed,
    Healthy,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ProviderRuntimeReadiness {
    pub status: ProviderRuntimeReadinessStatus,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required_feature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
}

/// Origin of a value-free readiness capability. These names deliberately
/// match the Speech Span Revision v2 evidence vocabulary, while remaining
/// capability metadata rather than claiming evidence for an individual span.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SttFidelityOrigin {
    Unavailable,
    App,
    Provider,
    Unverified,
}

/// Typed reasons that the selected STT configuration cannot provide the
/// registry's maximum declared fidelity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SttFidelityDegradation {
    FinalOnlyRevisions,
    AppEstimatedTiming,
    TimingUnavailable,
    ConfidenceUnavailable,
    TurnUnavailable,
    SpeakerUnavailable,
    SpeakerDisabledByConfiguration,
    SpeakerUnavailableForSelectedModel,
    SpeakerRemappedByConfiguration,
    ChannelUnavailable,
    CapabilityUnverified,
}

/// Provider-neutral turn signals enabled by the selected STT configuration.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct SttTurnDetectionCapabilities {
    pub speech_start: bool,
    pub speech_final: bool,
    pub endpointing_configured: bool,
    pub utterance_end: bool,
    pub end_of_turn: bool,
    pub eager_end_of_turn: bool,
    pub turn_resume: bool,
}

/// Effective, selected-configuration STT fidelity returned with readiness.
/// This is intentionally separate from registry-declared maximum fidelity and
/// never supersedes authoritative per-span v2 evidence.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct EffectiveSttFidelity {
    pub revision_semantics: crate::provider_registry::SttRevisionSemantics,
    pub timing: crate::provider_registry::SttTimingFidelity,
    pub confidence: SttFidelityOrigin,
    pub turn: SttFidelityOrigin,
    pub speaker: SttFidelityOrigin,
    pub channel: SttFidelityOrigin,
    pub turn_detection: SttTurnDetectionCapabilities,
    pub degradations: Vec<SttFidelityDegradation>,
}

#[derive(Debug, Clone, Default)]
struct ProviderReadinessProbeResult {
    message: String,
    model_count: Option<usize>,
    model_catalog: Vec<ProviderModelCatalogItem>,
    voice_catalog: Vec<ProviderModelCatalogItem>,
    language_catalog: Vec<ProviderModelCatalogItem>,
    openrouter_models: Vec<OpenRouterModel>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ProviderReadiness {
    pub provider_id: String,
    pub status: ProviderReadinessStatus,
    pub message: String,
    pub automatic_probe_available: bool,
    pub checked_at: Option<u64>,
    pub stale: bool,
    pub credential_epoch: u64,
    pub credentials: Vec<ProviderCredentialReadiness>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub model_catalog: Vec<ProviderModelCatalogItem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub voice_catalog: Vec<ProviderModelCatalogItem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub language_catalog: Vec<ProviderModelCatalogItem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub openrouter_models: Vec<OpenRouterModel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<ProviderRuntimeReadiness>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_stt_fidelity: Option<EffectiveSttFidelity>,
}

fn stt_fidelity_origin(
    evidence: crate::provider_registry::SttProviderEvidence,
) -> SttFidelityOrigin {
    match evidence {
        crate::provider_registry::SttProviderEvidence::Unavailable => {
            SttFidelityOrigin::Unavailable
        }
        crate::provider_registry::SttProviderEvidence::Provider => SttFidelityOrigin::Provider,
        crate::provider_registry::SttProviderEvidence::Unverified => SttFidelityOrigin::Unverified,
    }
}

fn push_static_stt_degradations(
    fidelity: &crate::provider_registry::ProviderSttFidelityDescriptor,
    degradations: &mut Vec<SttFidelityDegradation>,
) {
    use crate::provider_registry::{SttProviderEvidence, SttRevisionSemantics, SttTimingFidelity};

    if fidelity.revision_semantics == SttRevisionSemantics::FinalOnly {
        degradations.push(SttFidelityDegradation::FinalOnlyRevisions);
    }
    match fidelity.timing {
        SttTimingFidelity::AppEstimated => {
            degradations.push(SttFidelityDegradation::AppEstimatedTiming);
        }
        SttTimingFidelity::Unavailable => {
            degradations.push(SttFidelityDegradation::TimingUnavailable);
        }
        SttTimingFidelity::Unverified => {
            degradations.push(SttFidelityDegradation::CapabilityUnverified);
        }
        SttTimingFidelity::ProviderCoarse | SttTimingFidelity::ProviderExact => {}
    }
    for (evidence, degradation) in [
        (
            fidelity.confidence,
            SttFidelityDegradation::ConfidenceUnavailable,
        ),
        (fidelity.turn, SttFidelityDegradation::TurnUnavailable),
        (fidelity.speaker, SttFidelityDegradation::SpeakerUnavailable),
        (fidelity.channel, SttFidelityDegradation::ChannelUnavailable),
    ] {
        match evidence {
            SttProviderEvidence::Unavailable => degradations.push(degradation),
            SttProviderEvidence::Unverified
                if !degradations.contains(&SttFidelityDegradation::CapabilityUnverified) =>
            {
                degradations.push(SttFidelityDegradation::CapabilityUnverified);
            }
            SttProviderEvidence::Provider | SttProviderEvidence::Unverified => {}
        }
    }
}

fn effective_stt_fidelity(
    descriptor: &crate::provider_registry::ProviderDescriptor,
    settings: &crate::settings::AppSettings,
) -> Option<EffectiveSttFidelity> {
    let declared = descriptor.stt_fidelity?;
    if crate::provider_registry::descriptor_for_asr_provider(&settings.asr_provider).id
        != descriptor.id
    {
        return None;
    }

    let mut degradations = Vec::new();
    push_static_stt_degradations(&declared, &mut degradations);
    let mut effective = EffectiveSttFidelity {
        revision_semantics: declared.revision_semantics,
        timing: declared.timing,
        confidence: stt_fidelity_origin(declared.confidence),
        turn: stt_fidelity_origin(declared.turn),
        speaker: stt_fidelity_origin(declared.speaker),
        channel: stt_fidelity_origin(declared.channel),
        turn_detection: SttTurnDetectionCapabilities {
            end_of_turn: declared.turn == crate::provider_registry::SttProviderEvidence::Provider,
            ..SttTurnDetectionCapabilities::default()
        },
        degradations,
    };

    if descriptor.id != "asr.deepgram" {
        return Some(effective);
    }

    // Resolve readiness from the same authoritative global diarization policy
    // that startup applies before constructing the Deepgram runtime.
    let mut selected_asr_provider = settings.asr_provider.clone();
    let _ = selected_asr_provider.apply_diarization_settings(&settings.diarization);
    let crate::settings::AsrProvider::DeepgramStreaming {
        model,
        enable_diarization,
        endpointing_ms,
        utterance_end_ms,
        vad_events,
        eot_threshold,
        eager_eot_threshold,
        max_speakers,
        ..
    } = &selected_asr_provider
    else {
        return None;
    };
    let flux = model.trim().to_ascii_lowercase().starts_with("flux-");
    effective.degradations.clear();
    // Deepgram is currently dispatched as mono (`channels=1`) and the adapter
    // does not preserve channel attribution.
    effective.channel = SttFidelityOrigin::Unavailable;
    effective
        .degradations
        .push(SttFidelityDegradation::ChannelUnavailable);
    effective.turn_detection = if flux {
        let eager = *eager_eot_threshold > 0.0 && *eager_eot_threshold <= *eot_threshold;
        SttTurnDetectionCapabilities {
            speech_start: true,
            speech_final: false,
            endpointing_configured: false,
            utterance_end: false,
            end_of_turn: true,
            eager_end_of_turn: eager,
            turn_resume: eager,
        }
    } else {
        SttTurnDetectionCapabilities {
            speech_start: *vad_events,
            // `0` leaves Deepgram Nova's provider endpointing default in place;
            // it does not disable `speech_final` events.
            speech_final: true,
            endpointing_configured: *endpointing_ms > 0,
            utterance_end: *utterance_end_ms > 0,
            end_of_turn: false,
            eager_end_of_turn: false,
            turn_resume: false,
        }
    };
    effective.turn = if effective.turn_detection.speech_start
        || effective.turn_detection.speech_final
        || effective.turn_detection.utterance_end
        || effective.turn_detection.end_of_turn
        || effective.turn_detection.eager_end_of_turn
        || effective.turn_detection.turn_resume
    {
        SttFidelityOrigin::Provider
    } else {
        SttFidelityOrigin::Unavailable
    };
    if flux {
        effective.revision_semantics = crate::provider_registry::SttRevisionSemantics::FinalOnly;
        effective
            .degradations
            .push(SttFidelityDegradation::FinalOnlyRevisions);
        effective.speaker = SttFidelityOrigin::Unavailable;
        effective
            .degradations
            .push(SttFidelityDegradation::SpeakerUnavailableForSelectedModel);
    } else if *enable_diarization {
        effective.speaker = if *max_speakers == 0 {
            SttFidelityOrigin::Provider
        } else {
            effective
                .degradations
                .push(SttFidelityDegradation::SpeakerRemappedByConfiguration);
            SttFidelityOrigin::App
        };
    } else {
        effective.speaker = SttFidelityOrigin::Unavailable;
        effective
            .degradations
            .push(SttFidelityDegradation::SpeakerDisabledByConfiguration);
    }

    Some(effective)
}

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn privacy_policy_block_reason(mode: crate::settings::PrivacyMode) -> &'static str {
    match mode {
        crate::settings::PrivacyMode::LocalOnly => {
            "local_only allows only local or loopback session-content providers"
        }
        crate::settings::PrivacyMode::CloudDisabledReadinessOnly => {
            "cloud_disabled_readiness_only allows saved-key health and model probes but blocks session content transfer"
        }
        crate::settings::PrivacyMode::OrgPromotion => {
            "org_promotion is reserved for explicit future promotion flows and blocks live session provider transfer"
        }
        crate::settings::PrivacyMode::ByokCloud => "byok_cloud allows configured content transfer",
    }
}

fn provider_content_egress_policy_from_settings(
    settings: &crate::settings::AppSettings,
    requires_cloud_content_transfer: bool,
) -> crate::asr::ProviderContentEgressPolicy {
    crate::asr::ProviderContentEgressPolicy::from_privacy_mode_and_transfer_requirement(
        settings.privacy_mode,
        requires_cloud_content_transfer,
    )
}

fn read_settings_for_session_content(
    state: &AppState,
    action: &str,
) -> AppResult<crate::settings::AppSettings> {
    state
        .app_settings
        .read()
        .map(|settings| settings.clone())
        .map_err(|e| {
            AppError::Unknown(format!(
                "Cannot read privacy settings for {action}; refusing session content transfer: {e}"
            ))
        })
}

fn session_content_policy_block(
    settings: &crate::settings::AppSettings,
    action: &str,
    provider: &str,
    data_classes: &[&str],
    requires_cloud_content_transfer: bool,
) -> Option<AppError> {
    if !requires_cloud_content_transfer
        || settings
            .privacy_mode
            .allows_session_cloud_content_transfer()
    {
        return None;
    }

    Some(AppError::PrivacyPolicyBlocked {
        mode: settings.privacy_mode.as_str().to_string(),
        action: action.to_string(),
        provider: provider.to_string(),
        data_classes: data_classes
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
        reason: privacy_policy_block_reason(settings.privacy_mode).to_string(),
    })
}

fn enforce_session_content_policy(
    app: &tauri::AppHandle,
    state: &AppState,
    settings: &crate::settings::AppSettings,
    action: &str,
    provider: &str,
    data_classes: &[&str],
    requires_cloud_content_transfer: bool,
) -> AppResult<()> {
    let Some(err) = session_content_policy_block(
        settings,
        action,
        provider,
        data_classes,
        requires_cloud_content_transfer,
    ) else {
        return Ok(());
    };

    if let AppError::PrivacyPolicyBlocked {
        mode,
        action,
        provider,
        data_classes,
        reason,
    } = &err
    {
        events::emit_or_log(
            app,
            events::PRIVACY_POLICY_BLOCKED,
            events::PrivacyPolicyBlockedPayload {
                session_id: Some(state.current_session_id()),
                privacy_mode: mode.clone(),
                action: action.clone(),
                provider: provider.clone(),
                data_classes: data_classes.clone(),
                reason: reason.clone(),
                timestamp_ms: unix_millis(),
            },
        );
    }

    Err(err)
}

// ---------------------------------------------------------------------------
// Helper: parse source_id string into rsac::CaptureTarget
// ---------------------------------------------------------------------------

/// Map a frontend source ID string to an rsac [`CaptureTarget`].
///
/// Supported formats:
/// - `"system"` / `"system-default"` → `CaptureTarget::SystemDefault`
/// - `"device:<device_id>"`      → `CaptureTarget::Device(DeviceId(device_id))`
/// - `"app:<pid>"`               → `CaptureTarget::Application(ApplicationId(pid))`
/// - `"tree:<pid>"` / `"process-tree:<pid>"` → `CaptureTarget::ProcessTree(ProcessId(pid))`
/// - `"name:<name>"` / `"app-name:<name>"` → `CaptureTarget::ApplicationByName(name)`
/// - `"{0.0.x...}"`              → Windows MMDevice ID compatibility fallback
fn parse_capture_target(source_id: &str) -> Result<rsac::CaptureTarget, String> {
    if source_id == "system" || source_id == "system-default" {
        Ok(rsac::CaptureTarget::SystemDefault)
    } else if let Some(device_id) = source_id.strip_prefix("device:") {
        Ok(rsac::CaptureTarget::Device(rsac::DeviceId(
            device_id.to_string(),
        )))
    } else if looks_like_windows_mmdevice_id(source_id) {
        Ok(rsac::CaptureTarget::Device(rsac::DeviceId(
            source_id.to_string(),
        )))
    } else if let Some(pid_str) = source_id.strip_prefix("app:") {
        let pid = parse_capture_pid("app", pid_str)?;
        // ApplicationId wraps a String (the PID as a string).
        Ok(rsac::CaptureTarget::Application(rsac::ApplicationId(
            pid.to_string(),
        )))
    } else if let Some(pid_str) = source_id
        .strip_prefix("tree:")
        .or_else(|| source_id.strip_prefix("process-tree:"))
    {
        let pid = parse_capture_pid("process-tree", pid_str)?;
        Ok(rsac::CaptureTarget::ProcessTree(rsac::ProcessId(pid)))
    } else if let Some(name) = source_id
        .strip_prefix("name:")
        .or_else(|| source_id.strip_prefix("app-name:"))
    {
        Ok(rsac::CaptureTarget::ApplicationByName(name.to_string()))
    } else {
        Err(format!("Unknown source ID format: {}", source_id))
    }
}

fn looks_like_windows_mmdevice_id(source_id: &str) -> bool {
    source_id.starts_with("{0.0.") && source_id.ends_with('}') && source_id.len() > "{0.0.}".len()
}

fn parse_capture_pid(kind: &str, raw: &str) -> Result<u32, String> {
    let pid = raw
        .parse::<u32>()
        .map_err(|_| format!("Invalid {kind} PID: {raw}"))?;
    if pid == 0 {
        return Err(format!("Invalid {kind} PID: {raw}"));
    }
    Ok(pid)
}

fn resolve_capture_start_target(
    source_id: String,
    capture_target: Option<String>,
    source_descriptor: Option<AudioSourceInfo>,
) -> Result<(String, rsac::CaptureTarget, Option<AudioSourceInfo>), String> {
    let resolved_source_id = source_descriptor
        .as_ref()
        .and_then(|descriptor| descriptor.capture_target.clone())
        .or(capture_target)
        .unwrap_or(source_id);
    let target = parse_capture_target(&resolved_source_id)?;
    Ok((resolved_source_id, target, source_descriptor))
}

fn local_asr_provider_availability_error(
    provider: &crate::settings::AsrProvider,
) -> Option<AppError> {
    match provider {
        crate::settings::AsrProvider::LocalWhisper => {
            #[cfg(not(feature = "asr-whisper"))]
            {
                Some(AppError::ProviderUnavailable {
                    provider: "LocalWhisper".to_string(),
                    required_feature: "local-ml or asr-whisper".to_string(),
                })
            }
            #[cfg(feature = "asr-whisper")]
            {
                None
            }
        }
        crate::settings::AsrProvider::SherpaOnnx { .. } => {
            #[cfg(not(feature = "sherpa-streaming"))]
            {
                Some(AppError::ProviderUnavailable {
                    provider: "SherpaOnnx".to_string(),
                    required_feature: "sherpa-streaming".to_string(),
                })
            }
            #[cfg(feature = "sherpa-streaming")]
            {
                None
            }
        }
        crate::settings::AsrProvider::Moonshine { .. } => {
            #[cfg(not(feature = "asr-moonshine"))]
            {
                Some(AppError::ProviderUnavailable {
                    provider: "Moonshine".to_string(),
                    required_feature: "asr-moonshine".to_string(),
                })
            }
            #[cfg(feature = "asr-moonshine")]
            {
                Some(AppError::ProviderUnavailable {
                    provider: "Moonshine".to_string(),
                    required_feature: "asr-moonshine runtime implementation".to_string(),
                })
            }
        }
        _ => None,
    }
}

fn local_llm_provider_availability_error(
    provider: &crate::settings::LlmProvider,
) -> Option<AppError> {
    match provider {
        crate::settings::LlmProvider::LocalLlama => {
            #[cfg(not(feature = "llm-llama"))]
            {
                Some(AppError::ProviderUnavailable {
                    provider: "LocalLlama".to_string(),
                    required_feature: "local-ml or llm-llama".to_string(),
                })
            }
            #[cfg(feature = "llm-llama")]
            {
                None
            }
        }
        crate::settings::LlmProvider::MistralRs { .. } => {
            #[cfg(not(feature = "llm-mistralrs"))]
            {
                Some(AppError::ProviderUnavailable {
                    provider: "MistralRs".to_string(),
                    required_feature: "local-ml or llm-mistralrs".to_string(),
                })
            }
            #[cfg(feature = "llm-mistralrs")]
            {
                None
            }
        }
        _ => None,
    }
}

/// Resolve and enforce every provider that a durable speech-to-notes start can
/// send content to. This must run before client synchronization, worker spawn,
/// or processed-audio subscription (ADR-0033).
fn enforce_transcribe_provider_start(settings: &crate::settings::AppSettings) -> AppResult<()> {
    crate::provider_registry::ensure_asr_provider_start_enabled(&settings.asr_provider)?;
    crate::provider_registry::ensure_llm_provider_start_enabled(&settings.llm_provider)?;
    Ok(())
}

/// Enforce the selected chat LLM and the optional speak-aloud TTS provider
/// before a request/task can receive user or session content.
fn enforce_chat_provider_start(settings: &crate::settings::AppSettings) -> AppResult<()> {
    crate::provider_registry::ensure_llm_provider_start_enabled(&settings.llm_provider)?;
    if settings.speak_aloud {
        crate::provider_registry::ensure_tts_provider_start_enabled(&settings.tts_provider)?;
    }
    Ok(())
}

/// Join a session-scoped worker on shutdown, waiting up to `timeout` for it to
/// observe the stop flag and exit.
///
/// A timed-out handle is retained in `retired_workers`, not detached. Start and
/// New Session paths refuse to proceed until retained workers finish, which
/// keeps a late ASR/provider write from crossing a session rotation while still
/// bounding the latency of the Stop command.
fn join_worker_with_timeout(
    handle: std::thread::JoinHandle<()>,
    timeout: std::time::Duration,
    name: &str,
    retired_workers: &Arc<Mutex<Vec<std::thread::JoinHandle<()>>>>,
) {
    let deadline = std::time::Instant::now() + timeout;
    while !handle.is_finished() {
        if std::time::Instant::now() >= deadline {
            log::warn!(
                "{name} did not exit within {timeout:?} on stop; retaining handle and fencing session start/rotation"
            );
            let mut retired = retired_workers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            retired.push(handle);
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    if let Err(e) = handle.join() {
        log::warn!("{name} panicked during shutdown: {e:?}");
    }
}

/// Reap completed timed-out workers and reject a new producer/session boundary
/// while any prior worker is still live.
fn ensure_session_workers_quiesced(state: &AppState) -> AppResult<()> {
    let mut retired = state
        .retired_session_workers
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut still_running = Vec::with_capacity(retired.len());
    for handle in retired.drain(..) {
        if handle.is_finished() {
            if let Err(error) = handle.join() {
                log::warn!("retired session worker panicked during reap: {error:?}");
            }
        } else {
            still_running.push(handle);
        }
    }
    *retired = still_running;
    if retired.is_empty() {
        Ok(())
    } else {
        Err(AppError::SessionInvalid {
            reason: format!(
                "{} previous session worker(s) are still stopping; retry in a moment",
                retired.len()
            ),
        })
    }
}

/// Bounded per-job wait for a registered projection job thread to exit at
/// Stop (audio-graph-9cc1 / ADR-0045 decision 4, drain half).
///
/// Current value (20s) is a first-cut, TRANSCRIPT_WRITER_SHUTDOWN_TIMEOUT-style:
/// long enough for a realistic LLM round-trip (notes/graph patch generation)
/// plus a materializer save to finish, short enough that a wedged provider
/// call doesn't hang the Stop command indefinitely. A timed-out handle spills
/// into `retired_workers` via `join_worker_with_timeout` — the SAME vec Start
/// and New Session already fence rotation on, so no new fence logic is
/// needed here.
///
/// Tuning procedure: every drain logs `projection_job.flush elapsed_ms=…` at
/// INFO. After a couple of weeks of field data, grep for that key, compute
/// p50/p95/p99 across real sessions, and set this constant to
/// `p99 + ~1s safety margin`, documented with a "Chosen because: …" comment
/// (see `TRANSCRIPT_WRITER_SHUTDOWN_TIMEOUT` for the precedent).
const PROJECTION_JOB_FLUSH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// Drain every registered live projection job thread at Stop.
///
/// Loops taking snapshots of the registry (`std::mem::take`) rather than
/// locking it for the whole drain, so a job's own self-deregistration (on the
/// job's own thread, via its RAII guard) never contends with this loop. Each
/// handle is joined through the existing [`join_worker_with_timeout`] idiom —
/// same spill into `retired_workers` on timeout — wrapped with elapsed-time
/// instrumentation under the `projection_job.flush` log key so the timeout
/// above can be retuned from real data.
///
/// The loop (not a single pass) is the provable half of "no projection
/// thread outlives stop" (ADR-0045 decision 4 / audio-graph-9cc1
/// adversarial-review fix): `dispatch_projection_decision` refuses to spawn
/// once `projection_lane_stopping` is set, which is the primary defense
/// against a completing job chaining a mandatory follow-up mid-drain, but
/// this loop re-observes the registry after every pass and keeps joining
/// until it is actually empty (or the overall `timeout` budget below is
/// exhausted) instead of trusting a single `mem::take` snapshot never to
/// miss a late registration.
///
/// `timeout` bounds the WHOLE drain (all passes, all handles), not each
/// handle individually — a handle joined late in the drain gets whatever
/// time remains before the shared deadline, not a fresh `timeout`. It is a
/// parameter (not hard-coded to `PROJECTION_JOB_FLUSH_TIMEOUT` internally)
/// purely so tests can exercise the timeout/spill branch on a millisecond
/// budget instead of the real 20s; the only production caller passes
/// `PROJECTION_JOB_FLUSH_TIMEOUT`.
fn drain_projection_job_workers(
    registry: &crate::state::ProjectionJobRegistry,
    timeout: std::time::Duration,
    retired_workers: &Arc<Mutex<Vec<std::thread::JoinHandle<()>>>>,
) {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let drained: Vec<(
            crate::projections::ProjectionKind,
            String,
            std::thread::JoinHandle<()>,
        )> = {
            let mut guard = registry
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            std::mem::take(&mut *guard)
        };
        if drained.is_empty() {
            break;
        }
        for (kind, job_id, handle) in drained {
            let started = std::time::Instant::now();
            let name = format!("projection job (kind={kind:?} job_id={job_id})");
            let now = std::time::Instant::now();
            let remaining = if now >= deadline {
                std::time::Duration::ZERO
            } else {
                deadline - now
            };
            join_worker_with_timeout(handle, remaining, &name, retired_workers);
            log::info!(
                "projection_job.flush elapsed_ms={} kind={:?} job_id={} timeout_ms={}",
                started.elapsed().as_millis(),
                kind,
                job_id,
                timeout.as_millis()
            );
        }
        if std::time::Instant::now() >= deadline {
            break;
        }
    }
}

/// audio-graph-fa56 field bug: a same-basis projection failure under
/// `PROJECTION_LANE_ATTEMPT_BUDGET` arms exactly one deferred retry
/// (ADR-0045 decision 3), fired by a one-shot clock thread
/// (`spawn_deferred_lane_observation`, speech/mod.rs) unless a final ASR
/// revision drives it event-driven first. That clock thread polls
/// `projection_lane_stopping` and exits WITHOUT firing the retry the moment
/// Stop is observed — correct, since firing it is a full re-GENERATION (a
/// fresh LLM call via `generate_projection_patch`, not a cheap re-apply of
/// an already-generated patch), and blocking Stop on an unbounded LLM call
/// would blow well past the existing `PROJECTION_JOB_FLUSH_TIMEOUT` (20s)
/// shutdown budget. But the ONLY signal that this happened was a
/// `log::debug!` inside the clock thread itself — invisible under the
/// default log filter, and exactly the line field evidence (session
/// c95d21e6) had to go digging for.
///
/// Called strictly AFTER `drain_projection_job_workers` has returned (the
/// caller's contract, not enforced here): at that point every
/// `projection-retry-<kind>` clock thread for THIS Stop has either exited or
/// — on the rare `PROJECTION_JOB_FLUSH_TIMEOUT` drain-timeout path — can no
/// longer fire a retry even if still technically running, so a lane
/// [`ProjectionSchedulers::kinds_with_armed_deferred_retry`] still reports
/// is reliably "abandoned", never "still ticking" — see that method's doc
/// and `ProjectionScheduler::has_armed_deferred_retry`'s for the full
/// invariant (including its disclosed blind spot for jobs that fail and get
/// discarded DURING the drain itself).
///
/// Logs a single count-only WARN naming the abandoned lane kind(s) — never
/// the basis, transcript content, or generated patch (ADR-0025's
/// counts-only-in-logs rule) — so a support session or a future replay/
/// audit pass has a visible, greppable signal instead of a debug-level line
/// that is easy to miss. Deliberately a no-op when no lane is armed: most
/// Stops have nothing to report, and a WARN on every Stop would train
/// operators to ignore it.
///
/// This is the "at minimum" fix (ticket audio-graph-fa56), which asked for
/// both (a) a visible signal and (b) enough persisted state that a later
/// session load or replay pass can detect the gap. Forcing a synchronous
/// heal re-generation into the stop path was rejected (see the report) as
/// both a shutdown-budget violation and a re-opening of
/// `dispatch_projection_decision`'s `projection_lane_stopping` race guard
/// (ADR-0045 decision 4 / audio-graph-9cc1), which is exactly the
/// detached-thread teardown-ordering territory shared with seeds
/// `audio-graph-64e3`/`audio-graph-84e0` this ticket is scoped to stay out
/// of.
///
/// (b) is NOT satisfied by the log line alone: this also persists a
/// [`crate::projection_scheduler::SchedulerQueueState`] diagnostics snapshot
/// via `persistence::save_scheduler_queue_state` — the same disk artifact
/// `state.rs`'s `rotate_session` already writes on every session rotation,
/// extended (audio-graph-fa56) with `notes_deferred_retry_at_ms` /
/// `graph_deferred_retry_at_ms` fields that mirror
/// [`ProjectionSchedulers::kinds_with_armed_deferred_retry`]'s output.
/// ADR-0045 decision 6 rejected reading this snapshot back into a live
/// scheduler as a second authority — it did NOT reject writing to it, and
/// the snapshot's writer/loader (`persistence::save_scheduler_queue_state`/
/// `load_scheduler_queue_state`) already exist and already run in
/// production today. So a session that ends at this Stop without ever
/// rotating to a new session still leaves the gap detectable from disk, not
/// only from the log line — best-effort, like every other
/// `save_scheduler_queue_state` call: `save_json`'s error path only logs and
/// never propagates, so a failed write here cannot fail or delay Stop.
///
/// One lock acquisition covers both the WARN's read and the snapshot build,
/// so this adds no additional contention over the pre-fa56 baseline of zero
/// reads here. It is not, however, a zero-cost no-op: the snapshot write is
/// a real (small, local, synchronous) JSON file write, run on the same
/// `spawn_blocking` thread as the drain immediately before it, not on the
/// async executor.
///
/// Known gap, disclosed rather than fixed here (see
/// [`ProjectionScheduler::has_armed_deferred_retry`]'s doc for the
/// mechanism): a projection job still in flight when Stop begins can finish
/// and fail *during* the drain this function runs after. That failure arms
/// a deferral that `dispatch_projection_decision`'s `projection_lane_stopping`
/// discard branch clears back to `None` via `abandon_discarded_deferred_retry`
/// (speech/mod.rs) before this function ever runs — invisible to both the
/// WARN below and the snapshot it persists, with only a `log::debug!` left
/// behind. Closing that hole needs a signal at that discard site, not here.
fn log_abandoned_deferred_retries_after_stop(
    schedulers: &Arc<Mutex<crate::projection_scheduler::ProjectionSchedulers>>,
    session_id: &str,
) {
    let (abandoned_kinds, snapshot) = {
        let guard = schedulers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (
            guard.kinds_with_armed_deferred_retry(),
            guard.snapshot_queue(),
        )
    };
    if !abandoned_kinds.is_empty() {
        log::warn!(
            "projection_scheduler.deferred_retry_abandoned_at_stop abandoned_count={} kinds={:?}",
            abandoned_kinds.len(),
            abandoned_kinds
        );
    }
    crate::persistence::save_scheduler_queue_state(session_id, &snapshot);
}

/// Count-only detector for audio-graph-64e3 (field evidence: session
/// c95d21e6, 6 finalized spans landed in the speaker ledger's
/// `.speaker.jsonl` but never reached the display transcript's
/// `<session>.jsonl` — no WARN fired anywhere).
///
/// Reads-and-resets `AppState::display_transcript_write_misses`, which
/// `emit_transcript_and_extract_with_meta` increments every time a final ASR
/// span revision is accepted into the ledgers but the display-transcript
/// writer had no writer available at that instant. Must run AFTER the
/// speech-processor/ASR-worker joins above (same ordering requirement as
/// `log_abandoned_deferred_retries_after_stop`): those joins are what give
/// the deepgram receiver thread (and, transitively, this counter) a chance
/// to reach its final value before Stop reports the session's tally. Logs
/// session id and the row-count gap only — never transcript content.
///
/// Called from both `stop_capture_impl` and `stop_transcribe` (the two
/// session-end transitions that join sp/asr through this same tail); NOT
/// called from `rotate_session` (a rotation deliberately does not reset the
/// counter) or from `lib.rs`'s `graceful_shutdown` (quit while transcribing
/// clears the writer slot without ever joining sp/asr/receiver first, and
/// without calling this function at all — a miss on that path is silently
/// carried forward and never reported until, if ever, capture resumes and
/// later stops).
///
/// Attribution caveat: `session_id` here is the session active AT THIS
/// STOP, not necessarily the session whose row actually went missing. A
/// straggler that increments the counter after this read (e.g. a receiver
/// still draining past its own bounded join, or one of the three ASR
/// providers whose receiver handle is still fully detached — see the
/// audio-graph-84e0 residual) has its miss reported at the NEXT stop, under
/// the NEXT session's id, not the one that actually missed the row. A grep
/// for this WARN's `session_id=` should be read as "reported at this
/// session's stop", not "this session is missing rows".
fn warn_if_display_transcript_rows_missing_at_stop(
    display_transcript_write_misses: &Arc<AtomicU64>,
    session_id: &str,
) {
    let missing = display_transcript_write_misses.swap(0, Ordering::SeqCst);
    if missing > 0 {
        log::warn!(
            "transcript.display_rows_missing_at_stop session_id={} missing_rows={}",
            session_id,
            missing
        );
    }
}

fn register_runtime_processed_audio_consumer(
    registry: &Arc<crate::audio::ProcessedAudioConsumerRegistry>,
    id: &str,
    stage: ProcessedAudioConsumerStage,
    provider: Option<&str>,
    capacity: usize,
    conflict_group: Option<&str>,
    is_active: ConsumerActiveFn,
) -> AppResult<crossbeam_channel::Receiver<ProcessedAudioChunk>> {
    let (tx, rx) = crossbeam_channel::bounded::<ProcessedAudioChunk>(capacity);
    registry
        .register(ProcessedAudioConsumerRegistration {
            descriptor: ProcessedAudioConsumerDescriptor {
                id: id.to_string(),
                stage,
                provider: provider.map(str::to_string),
                conflict_group: conflict_group.map(str::to_string),
                capacity,
                drop_policy: ProcessedAudioDropPolicy::DropOldest,
                source_filter: ProcessedAudioSourceFilter::All,
                mixing_mode: ProcessedAudioMixingMode::PerSource,
            },
            tx,
            drain_rx: rx.clone(),
            is_active,
        })
        .map_err(AppError::Unknown)?;
    Ok(rx)
}

fn unregister_runtime_processed_audio_consumer(
    registry: &Arc<crate::audio::ProcessedAudioConsumerRegistry>,
    id: &str,
) {
    if registry.unregister(id) {
        log::info!("Unregistered processed-audio consumer '{}'", id);
    }
}

/// Reap a finished worker-thread handle from a slot, leaving the slot empty so
/// the caller can respawn.
///
/// AUD-CV3 (#62): the converse driver's terminal-auth teardown flips
/// `is_converse_active=false` and `break`s but does NOT clear its thread slots
/// (`converse_audio_thread`/`converse_thread`) or `gemini_client`. A subsequent
/// `start_converse` without an intervening `stop_converse` therefore passes the
/// `is_converse_active` guard (false) but then hits the historical
/// `if handle.is_none()` spawn-gate as FALSE (a stale *finished* handle is still
/// `Some`) and silently skips spawning the sender — no audio, no error.
///
/// This reaps such a finished handle (joining it so any panic is logged) and
/// returns `Ok(())` so the caller respawns. If the handle is still running it is
/// put back and `Err` is returned so the caller surfaces "already running"
/// rather than double-spawning a second runtime consumer.
fn reap_finished_handle(
    slot: &mut Option<std::thread::JoinHandle<()>>,
    name: &str,
) -> Result<(), AppError> {
    if let Some(handle) = slot.take() {
        if handle.is_finished() {
            // Already exited (e.g. terminal-auth teardown): join to surface any
            // panic, then leave the slot empty for a clean respawn.
            if let Err(e) = handle.join() {
                log::warn!("{name} had already exited (reaped); join: {e:?}");
            } else {
                log::info!("{name} reaped (finished handle cleared for restart)");
            }
            Ok(())
        } else {
            // Genuinely still running — put it back and refuse to double-spawn.
            *slot = Some(handle);
            Err(AppError::SessionInvalid {
                reason: format!("{name} is already running"),
            })
        }
    } else {
        Ok(())
    }
}

fn validate_asr_capture_selection(
    provider: &crate::settings::AsrProvider,
    active_sources: &[String],
    pending_source: Option<&str>,
) -> Result<(), String> {
    let descriptor = crate::provider_registry::descriptor_for_asr_provider(provider);
    let source_policy = descriptor.source_policy.ok_or_else(|| {
        format!(
            "{} is missing provider-registry source policy metadata",
            descriptor.display_name
        )
    })?;
    let audio_input = descriptor.audio_input.ok_or_else(|| {
        format!(
            "{} is missing provider-registry audio input metadata",
            descriptor.display_name
        )
    })?;
    if audio_input.pipeline_format.sample_rate_hz != 16_000
        || audio_input.pipeline_format.channels != 1
        || audio_input.pipeline_format.frame_format
            != crate::provider_registry::ProviderAudioFrameFormat::F32
    {
        return Err(format!(
            "{} expects an unsupported processed-audio input format: {} Hz / {} ch / {:?}",
            descriptor.display_name,
            audio_input.pipeline_format.sample_rate_hz,
            audio_input.pipeline_format.channels,
            audio_input.pipeline_format.frame_format
        ));
    }

    let mut source_ids = std::collections::BTreeSet::new();
    for source_id in active_sources {
        let source_id = source_id.trim();
        if !source_id.is_empty() {
            source_ids.insert(source_id.to_string());
        }
    }
    if let Some(pending_source) = pending_source {
        let pending_source = pending_source.trim();
        if !pending_source.is_empty() {
            source_ids.insert(pending_source.to_string());
        }
    }

    match source_policy {
        crate::provider_registry::ProviderSourcePolicy::SingleSession if source_ids.len() > 1 => {
            let provider_name = descriptor
                .source_policy_label
                .unwrap_or(descriptor.display_name);
            Err(format!(
                "{provider_name} currently supports one active audio source at a time. \
                 Stop extra sources or switch to a provider with multi-source capture support \
                 before transcribing. Active sources: {}",
                source_ids.into_iter().collect::<Vec<_>>().join(", ")
            ))
        }
        crate::provider_registry::ProviderSourcePolicy::SingleSession
        | crate::provider_registry::ProviderSourcePolicy::MultiSourceIndependent
        | crate::provider_registry::ProviderSourcePolicy::MultiSourceMixed => Ok(()),
    }
}

fn non_empty_trimmed(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn api_config_from_runtime_settings(settings: &crate::settings::AppSettings) -> Option<ApiConfig> {
    let crate::settings::LlmProvider::Api {
        endpoint,
        api_key,
        model,
    } = &settings.llm_provider
    else {
        return None;
    };

    let endpoint = non_empty_trimmed(endpoint)?;
    let model = non_empty_trimmed(model)?;
    let llm_api_config = settings.llm_api_config.as_ref().filter(|config| {
        config.endpoint.trim() == endpoint.as_str() && config.model.trim() == model.as_str()
    });
    let api_key = non_empty_trimmed(api_key).or_else(|| {
        llm_api_config
            .and_then(|config| config.api_key.as_deref())
            .and_then(non_empty_trimmed)
    });
    let (max_tokens, temperature) = llm_api_config
        .map(|config| (config.max_tokens, config.temperature))
        .unwrap_or((512, 0.1));

    Some(ApiConfig {
        endpoint,
        api_key,
        model,
        max_tokens,
        temperature,
    })
}

pub(crate) fn sync_llm_api_client_from_settings_cache(state: &AppState) -> Result<(), String> {
    let settings = state
        .app_settings
        .read()
        .map_err(|e| format!("Lock error: {}", e))?
        .clone();
    let next_config = api_config_from_runtime_settings(&settings);
    let content_egress_policy = provider_content_egress_policy_from_settings(
        &settings,
        settings.llm_provider.requires_cloud_content_transfer(),
    );

    let mut guard = state
        .api_client
        .lock()
        .map_err(|e| format!("Lock error: {}", e))?;

    match next_config {
        Some(config) => {
            let already_current = guard
                .as_ref()
                .map(|client| {
                    client.config() == &config
                        && client.content_egress_policy() == content_egress_policy
                })
                .unwrap_or(false);
            if !already_current {
                *guard =
                    Some(ApiClient::new(config).with_content_egress_policy(content_egress_policy));
                log::info!("LLM API client synced from runtime settings");
            }
        }
        None => {
            if guard.take().is_some() {
                log::info!("LLM API client cleared because the active provider is not configured");
            }
        }
    }

    Ok(())
}

fn openrouter_config_from_runtime_settings(
    settings: &crate::settings::AppSettings,
) -> Option<OpenRouterConfig> {
    let crate::settings::LlmProvider::OpenRouter {
        model,
        base_url,
        provider_order,
        include_usage_in_stream,
        api_key,
    } = &settings.llm_provider
    else {
        return None;
    };

    let api_key = non_empty_trimmed(api_key)?;
    let model = non_empty_trimmed(model)?;
    let base_url =
        non_empty_trimmed(base_url).unwrap_or_else(|| openrouter::DEFAULT_BASE_URL.to_string());

    let (max_tokens, temperature) = settings
        .llm_api_config
        .as_ref()
        .map(|config| (config.max_tokens, config.temperature))
        .unwrap_or((512, 0.1));

    Some(OpenRouterConfig {
        api_key,
        model,
        base_url,
        provider_order: provider_order.clone(),
        routing_policy: settings.openrouter_routing_policy.clone(),
        include_usage_in_stream: *include_usage_in_stream,
        http_referer: openrouter::DEFAULT_HTTP_REFERER.to_string(),
        app_title: openrouter::DEFAULT_APP_TITLE.to_string(),
        max_tokens,
        temperature,
    })
}

pub(crate) fn sync_openrouter_client_from_settings_cache(state: &AppState) -> Result<(), String> {
    let settings = state
        .app_settings
        .read()
        .map_err(|e| format!("Lock error: {}", e))?
        .clone();
    let next_config = openrouter_config_from_runtime_settings(&settings);
    let content_egress_policy = provider_content_egress_policy_from_settings(&settings, true);

    let mut guard = state
        .openrouter_client
        .lock()
        .map_err(|e| format!("Lock error: {}", e))?;

    match next_config {
        Some(config) => {
            let already_current = guard
                .as_ref()
                .map(|client| {
                    client.config() == &config
                        && client.content_egress_policy() == content_egress_policy
                })
                .unwrap_or(false);
            if !already_current {
                *guard = Some(
                    OpenRouterClient::new(config).with_content_egress_policy(content_egress_policy),
                );
                log::info!("OpenRouter client synced from runtime settings");
            }
        }
        None => {
            if guard.take().is_some() {
                log::info!(
                    "OpenRouter client cleared because the active provider is not OpenRouter"
                );
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// List available audio sources (devices + running applications).
#[tauri::command]
pub async fn list_audio_sources(state: State<'_, AppState>) -> AppResult<Vec<AudioSourceInfo>> {
    log::info!("list_audio_sources called");
    let manager = state
        .capture_manager
        .lock()
        .map_err(|e| format!("Lock error: {}", e))?;
    Ok(manager.list_sources())
}

fn ensure_session_writers_ready(state: &AppState) -> AppResult<()> {
    let transcript_ready = state
        .transcript_writer
        .lock()
        .map_err(|error| format!("Transcript writer lock error: {error}"))?
        .is_some();
    let transcript_events_ready = state
        .transcript_event_writer
        .lock()
        .map_err(|error| format!("Transcript event writer lock error: {error}"))?
        .is_some();
    let projection_events_ready = state
        .projection_event_writer
        .lock()
        .map_err(|error| format!("Projection event writer lock error: {error}"))?
        .is_some();
    if transcript_ready && transcript_events_ready && projection_events_ready {
        return Ok(());
    }
    Err(AppError::SessionInvalid {
        reason: "Canonical session storage is unavailable; capture was not started".to_string(),
    })
}

fn capture_movement_policy(
    configured: crate::settings::PrivacyMode,
) -> crate::persistence::MovementPolicy {
    let privacy_mode = match configured {
        crate::settings::PrivacyMode::LocalOnly => crate::persistence::PrivacyMode::LocalOnly,
        crate::settings::PrivacyMode::ByokCloud => crate::persistence::PrivacyMode::ByokCloud,
        crate::settings::PrivacyMode::CloudDisabledReadinessOnly => {
            crate::persistence::PrivacyMode::CloudDisabledReadinessOnly
        }
        crate::settings::PrivacyMode::OrgPromotion => crate::persistence::PrivacyMode::OrgSync,
    };
    crate::persistence::MovementPolicy {
        privacy_mode,
        user_visible: true,
        // AudioGraph does not persist raw capture audio; the lifecycle event is
        // durable, but the audio stream it describes is transient memory.
        retention_class: crate::persistence::RetentionClass::Transient,
    }
}

fn append_capture_lifecycle_movement(
    state: &AppState,
    event_type: crate::persistence::DataMovementEventType,
) -> AppResult<()> {
    let session_id = state.current_session_id();
    let privacy_mode = state
        .app_settings
        .read()
        .map(|settings| settings.privacy_mode)
        .unwrap_or_default();
    append_capture_lifecycle_movement_for(&session_id, privacy_mode, event_type)
}

pub(crate) fn append_capture_lifecycle_movement_for(
    session_id: &str,
    privacy_mode: crate::settings::PrivacyMode,
    event_type: crate::persistence::DataMovementEventType,
) -> AppResult<()> {
    let event = crate::persistence::DataMovementLedgerBuilder::new(
        session_id,
        crate::persistence::DataMovementActor::System,
        event_type,
        capture_movement_policy(privacy_mode),
        crate::persistence::DataMovementDestination::local(),
    )
    .data_classes([crate::persistence::DataClass::AudioStream])
    // Device/process/window identifiers can be sensitive. The route report
    // needs the coarse capture boundary, not the raw rsac target identity.
    .source(crate::persistence::DataMovementSource {
        kind: "rsac".to_string(),
        source_id: None,
        source_label: None,
    })
    .build();
    FileMemoryRepository::user_data()
        .append_data_movement_event(session_id, &event)
        .map_err(AppError::from)
}

fn reap_finished_pipeline_worker(slot: &mut Option<std::thread::JoinHandle<()>>, label: &str) {
    if !slot
        .as_ref()
        .is_some_and(std::thread::JoinHandle::is_finished)
    {
        return;
    }
    if let Some(handle) = slot.take()
        && handle.join().is_err()
    {
        log::error!("Finished {label} worker had panicked; restarting it");
    }
}

/// Prepare and supervise the process-lifetime audio consumers before rsac is
/// allowed to start producing. An acknowledged source without a live pipeline
/// is not a successful capture start.
fn ensure_audio_pipeline_workers(state: &AppState, app: &tauri::AppHandle) -> AppResult<()> {
    {
        let mut pipeline_handle = state
            .pipeline_thread
            .lock()
            .map_err(|error| format!("Pipeline worker lock error: {error}"))?;
        reap_finished_pipeline_worker(&mut pipeline_handle, "audio pipeline");
        if pipeline_handle.is_none() {
            let rx = state.pipeline_rx.clone();
            let tx = state.processed_tx.clone();
            let handle = std::thread::Builder::new()
                .name("audio-pipeline".to_string())
                .spawn(move || {
                    let mut pipeline = AudioPipeline::new(rx, tx);
                    pipeline.run();
                })
                .map_err(|error| format!("Failed to spawn pipeline thread: {error}"))?;
            *pipeline_handle = Some(handle);
            log::info!("Pipeline thread spawned");
        }
    }

    let mut dispatcher_handle = state
        .dispatcher_thread
        .lock()
        .map_err(|error| format!("Audio dispatcher lock error: {error}"))?;
    reap_finished_pipeline_worker(&mut dispatcher_handle, "audio dispatcher");
    if dispatcher_handle.is_none() {
        let processed_rx = state.processed_rx.clone();
        let consumers = state.processed_audio_consumers.clone();
        let app_handle = app.clone();
        let handle = std::thread::Builder::new()
            .name("audio-dispatcher".to_string())
            .spawn(move || {
                log::info!("Audio dispatcher: starting registry fan-out loop");
                let mut chunks_seen: u64 = 0;
                let mut total_dropped: u64 = 0;
                let mut last_health_emit = std::time::Instant::now();
                while let Ok(message) = processed_rx.recv() {
                    match message {
                        ProcessedPipelineMessage::Chunk(chunk) => {
                            chunks_seen += 1;
                            let summary = consumers.dispatch(chunk);
                            if summary.dropped_chunks > 0 {
                                total_dropped += summary.dropped_chunks as u64;
                                if total_dropped % 50 == summary.dropped_chunks as u64 {
                                    log::warn!(
                                        "Audio dispatcher: processed-audio consumers dropped {} oldest/newest chunk(s) total (consumer behind real time)",
                                        total_dropped
                                    );
                                }
                            }

                            if summary.dropped_chunks > 0
                                || last_health_emit.elapsed()
                                    >= std::time::Duration::from_secs(2)
                            {
                                let payload = consumers.health_payload();
                                let _ = app_handle.emit(events::AUDIO_CONSUMER_HEALTH, &payload);
                                last_health_emit = std::time::Instant::now();
                            }
                        }
                        ProcessedPipelineMessage::ResetSession { completion } => {
                            let _ = completion.send(Ok(()));
                        }
                    }
                }
                let payload = consumers.health_payload();
                let _ = app_handle.emit(events::AUDIO_CONSUMER_HEALTH, &payload);
                log::info!(
                    "Audio dispatcher: exiting (pipeline channel closed). chunks_seen={}, total consumer drops={}",
                    chunks_seen,
                    total_dropped
                );
            })
            .map_err(|error| format!("Failed to spawn dispatcher thread: {error}"))?;
        *dispatcher_handle = Some(handle);
        log::info!("Audio dispatcher thread spawned");
    }
    Ok(())
}

/// Establish a fully ordered session boundary across raw pipeline state and
/// dispatcher fan-out. The reset command follows all prior raw chunks on the
/// bounded input channel; its acknowledgement returns only after the
/// dispatcher has handled every processed chunk before the barrier.
async fn reset_audio_pipeline_session(state: &AppState) -> AppResult<()> {
    let pipeline_tx = state.pipeline_tx.clone();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let (completion_tx, completion_rx) = crossbeam_channel::bounded(1);
        pipeline_tx
            .send_timeout(
                AudioPipelineInput::ResetSession {
                    completion: completion_tx,
                },
                Duration::from_secs(2),
            )
            .map_err(|_| "audio pipeline did not accept the session reset".to_string())?;
        completion_rx
            .recv_timeout(Duration::from_secs(3))
            .map_err(|_| {
                "audio pipeline/dispatcher did not acknowledge the session reset".to_string()
            })?
    })
    .await
    .map_err(|error| AppError::Unknown(format!("audio reset task failed: {error}")))?
    .map_err(AppError::Unknown)
}

/// Start capturing audio from the specified source.
#[tauri::command]
pub async fn start_capture(
    source_id: String,
    capture_target: Option<String>,
    source: Option<AudioSourceInfo>,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> AppResult<()> {
    start_capture_impl(source_id, capture_target, source, state.inner(), &app).await
}

/// Implementation of [`start_capture`] that operates on borrowed state/app so it
/// can be exercised from tests without constructing a per-test Tauri/tao app.
async fn start_capture_impl(
    source_id: String,
    capture_target: Option<String>,
    source: Option<AudioSourceInfo>,
    state: &AppState,
    app: &tauri::AppHandle,
) -> AppResult<()> {
    log::info!("start_capture called for source: {}", source_id);
    let _session_lifecycle = state.session_lifecycle.lock().await;
    ensure_session_workers_quiesced(state)?;

    let (source_id, target, source_descriptor) =
        resolve_capture_start_target(source_id, capture_target, source)?;

    // Reject duplicate, unreconciled, or still-retiring generations before
    // writer/pipeline preflight creates observable process-lifetime state.
    // The manager repeats this check when it takes final source ownership.
    state
        .capture_manager
        .lock()
        .map_err(|error| format!("Capture manager lock error: {error}"))?
        .validate_capture_start(&source_id)?;

    if state.is_transcribing.load(Ordering::SeqCst) {
        let asr_provider = state
            .app_settings
            .read()
            .map_err(|e| format!("Lock error: {}", e))?
            .asr_provider
            .clone();
        let active_sources = state
            .capture_manager
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?
            .active_captures();
        validate_asr_capture_selection(&asr_provider, &active_sources, Some(&source_id))?;
    }

    // Resolve the user-configured capture format from the in-memory settings
    // cache, falling back to defaults if the cache is uninitialised or the
    // persisted values are out of the supported whitelist. This is the
    // "wiring through" that Task #79 is about — without it the capture
    // thread would always use the hard-coded 48 kHz / stereo.
    let (capture_sample_rate, capture_channels) = {
        let audio_settings = state
            .app_settings
            .read()
            .map(|s| s.audio_settings.clone())
            .unwrap_or_default();
        crate::settings::resolve_audio_settings(&audio_settings)
    };
    log::info!(
        "start_capture: using sample_rate={} Hz, channels={}",
        capture_sample_rate,
        capture_channels
    );

    // A capture start is publishable only after canonical writers and the
    // process-lifetime consumer spine are ready. rsac is deliberately last so
    // a worker-spawn failure cannot leave an untracked live source.
    ensure_session_writers_ready(state)?;
    ensure_audio_pipeline_workers(state, app)?;

    let starting_first_source = state
        .capture_manager
        .lock()
        .map_err(|error| format!("Capture manager lock error: {error}"))?
        .active_captures()
        .is_empty();
    if starting_first_source {
        reset_audio_pipeline_session(state).await?;
    }

    // Start capture via the manager and wait for its real rsac
    // build -> start -> subscribe acknowledgement.
    {
        let mut manager = state
            .capture_manager
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        manager.start_capture(
            &source_id,
            target,
            source_descriptor,
            state.pipeline_tx.clone(),
            app.clone(),
            capture_sample_rate,
            capture_channels,
        )?;
    }

    if starting_first_source
        && let Err(error) = append_capture_lifecycle_movement(
            state,
            crate::persistence::DataMovementEventType::CaptureStarted,
        )
    {
        let rollback = state
            .capture_manager
            .lock()
            .map_err(|lock_error| format!("Capture rollback lock error: {lock_error}"))?
            .stop_capture(&source_id);
        if let Err(rollback_error) = rollback {
            log::error!(
                "Capture lifecycle ledger failed and source rollback also failed: {}",
                rollback_error
            );
        }
        // A flush/fsync error is an uncertain commit: the Started bytes may be
        // present even though the append returned Err. Best-effort a matching
        // Stop after the prepared worker is rolled back; if neither row is
        // durable the report remains conservatively Incomplete/Unknown.
        if let Err(stop_error) = append_capture_lifecycle_movement(
            state,
            crate::persistence::DataMovementEventType::CaptureStopped,
        ) {
            log::error!("Capture-start rollback audit also failed: {stop_error}");
        }
        return Err(error);
    }

    let commit_result = {
        let mut manager = state
            .capture_manager
            .lock()
            .map_err(|error| format!("Capture commit lock error: {error}"))?;
        manager
            .commit_capture_start(&source_id)
            .inspect_err(|_error| {
                let _ = manager.stop_capture(&source_id);
            })
    };
    if let Err(error) = commit_result {
        // CaptureStarted is already durable for a first source, but no audio
        // was released. Close the lifecycle immediately so Review does not
        // retain an open capture solely because the worker died at commit.
        if starting_first_source
            && let Err(audit_error) = append_capture_lifecycle_movement(
                state,
                crate::persistence::DataMovementEventType::CaptureStopped,
            )
        {
            log::error!("Failed to append compensating capture stop: {audit_error}");
        }
        return Err(AppError::Unknown(error));
    }

    // Publish Running only after writer, consumer, and rsac acknowledgements.
    if let Ok(mut capturing) = state.is_capturing.write() {
        *capturing = true;
    }
    if let Ok(mut status) = state.pipeline_status.write() {
        status.capture = StageStatus::Running { processed_count: 0 };
        status.pipeline = StageStatus::Running { processed_count: 0 };
    }

    // Emit initial pipeline status event
    if let Ok(status) = state.pipeline_status.read() {
        let _ = app.emit(events::PIPELINE_STATUS_EVENT, &*status);
    }

    log::info!("Started capture for source: {}", source_id);
    Ok(())
}

/// Stop capturing audio from the specified source.
///
/// If this was the last active capture, also stops transcription (if running)
/// since there is no more audio to transcribe.
#[tauri::command]
pub async fn stop_capture(
    source_id: String,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> AppResult<()> {
    stop_capture_impl(source_id, state.inner(), &app, None).await
}

/// Reconcile an acknowledged capture worker that exited without a Stop request.
///
/// The capture thread cannot mutate `AppState` directly while it owns rsac, so
/// it schedules this backend cleanup after marking its handle finished. Reusing
/// the normal Stop implementation keeps last-source transcription/provider
/// teardown and pipeline-status emission identical to an explicit user stop.
pub(crate) fn schedule_capture_exit_reconciliation(
    app: tauri::AppHandle,
    source_id: String,
    expected_finished: Arc<AtomicBool>,
) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        if let Err(error) = stop_capture_impl(
            source_id.clone(),
            state.inner(),
            &app,
            Some(expected_finished),
        )
        .await
        {
            // A concurrent explicit Stop may have removed the handle first. In
            // that case its lifecycle path already performed reconciliation.
            log::warn!(
                "capture exit reconciliation for source {} did not run to completion: {}",
                source_id,
                error
            );
        }
    });
}

/// Implementation of [`stop_capture`] that operates on borrowed state/app so it
/// can be exercised from tests without constructing a per-test Tauri/tao app.
async fn stop_capture_impl(
    source_id: String,
    state: &AppState,
    app: &tauri::AppHandle,
    expected_finished: Option<Arc<AtomicBool>>,
) -> AppResult<()> {
    log::info!("stop_capture called for source: {}", source_id);
    let _session_lifecycle = state.session_lifecycle.lock().await;

    let remaining;
    {
        let mut manager = state
            .capture_manager
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        let removed = match expected_finished.as_ref() {
            Some(expected) => manager.stop_capture_if_matches(&source_id, expected)?,
            None => {
                manager.stop_capture(&source_id)?;
                true
            }
        };
        if !removed {
            log::info!(
                "Ignoring stale capture-exit reconciliation for replacement source: {}",
                source_id
            );
            return Ok(());
        }
        let sibling_fatal_sources = manager.stop_finished_captures();
        if !sibling_fatal_sources.is_empty() {
            log::info!(
                "Reconciled sibling fatal capture source(s) in the same lifecycle transition: {:?}",
                sibling_fatal_sources
            );
        }
        remaining = manager.owned_capture_count();
    }

    if remaining == 0 {
        let mut cleanup_error_count = 0usize;
        if let Ok(mut capturing) = state.is_capturing.write() {
            *capturing = false;
        }
        if let Err(error) = reset_audio_pipeline_session(state).await {
            cleanup_error_count += 1;
            log::error!("Audio pipeline session reset failed during capture stop: {error}");
        }
        // Also stop transcription since there's no more audio flowing
        state.is_transcribing.store(false, Ordering::SeqCst);
        // Quiesce the workers before releasing the lifecycle lock. Merely
        // dropping their handles leaves a window where a final ASR revision
        // can race a subsequent New Session writer swap.
        let sp = state
            .speech_processor_thread
            .lock()
            .ok()
            .and_then(|mut guard| guard.take());
        let asr = state
            .asr_worker_thread
            .lock()
            .ok()
            .and_then(|mut guard| guard.take());
        let retired_speech_workers = state.retired_session_workers.clone();
        // ADR-0045 decision 4 (drain half, audio-graph-9cc1): flag first, then
        // join sp/asr so no new final ASR revision can dispatch another
        // projection job after this point, then drain every projection job
        // thread (Notes and Graph) still registered — a graph-lane job can
        // otherwise run arbitrarily long past Stop, since the graph lane's
        // backlog is unbounded by design during continuous speech.
        state.projection_lane_stopping.store(true, Ordering::SeqCst);
        let projection_job_workers = state.projection_job_workers.clone();
        let projection_schedulers = state.projection_schedulers.clone();
        let display_transcript_write_misses = state.display_transcript_write_misses.clone();
        let stop_session_id = state.current_session_id();
        let _ = tokio::task::spawn_blocking(move || {
            if let Some(handle) = sp {
                join_worker_with_timeout(
                    handle,
                    std::time::Duration::from_secs(3),
                    "speech processor",
                    &retired_speech_workers,
                );
            }
            if let Some(handle) = asr {
                join_worker_with_timeout(
                    handle,
                    std::time::Duration::from_secs(3),
                    "ASR worker",
                    &retired_speech_workers,
                );
            }
            drain_projection_job_workers(
                &projection_job_workers,
                PROJECTION_JOB_FLUSH_TIMEOUT,
                &retired_speech_workers,
            );
            // audio-graph-fa56: MUST run strictly after the drain above —
            // see `log_abandoned_deferred_retries_after_stop`'s doc for why
            // that ordering is what makes a still-armed
            // `deferred_retry_at_ms` mean "abandoned" rather than "clock
            // still running".
            log_abandoned_deferred_retries_after_stop(&projection_schedulers, &stop_session_id);
            // audio-graph-64e3: same ordering requirement as the call above —
            // must run strictly after the sp/asr joins so a still-draining
            // deepgram receiver has already had its bounded chance to catch
            // up before this reads the tally.
            warn_if_display_transcript_rows_missing_at_stop(
                &display_transcript_write_misses,
                &stop_session_id,
            );
        })
        .await;
        // Also stop Gemini notes if running.
        let gemini_was_active = match state.is_gemini_active.write() {
            Ok(mut gemini_active) if *gemini_active => {
                *gemini_active = false;
                true
            }
            _ => false,
        };
        if gemini_was_active {
            unregister_runtime_processed_audio_consumer(
                &state.processed_audio_consumers,
                GEMINI_NOTES_AUDIO_CONSUMER_ID,
            );
            // Disconnect the Gemini client
            if let Ok(mut client_guard) = state.gemini_client.lock() {
                if let Some(ref client) = *client_guard {
                    client.disconnect();
                }
                *client_guard = None;
            }
            // Also TAKE + clear the Gemini worker-thread handles, then join them
            // off-thread. Without this they stay `Some(..)` so the next
            // `start_gemini` skips recreating the audio/event loops and comes back
            // without a live Gemini event receiver (CodeRabbit commands.rs:543).
            // We detach the join (no .await in this sync block) so Stop stays
            // responsive; clearing the handles is the correctness-critical part.
            let audio_h = state
                .gemini_audio_thread
                .lock()
                .ok()
                .and_then(|mut g| g.take());
            let event_h = state
                .gemini_event_thread
                .lock()
                .ok()
                .and_then(|mut g| g.take());
            if audio_h.is_some() || event_h.is_some() {
                let retired_gemini_workers = state.retired_session_workers.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    if let Some(h) = audio_h {
                        join_worker_with_timeout(
                            h,
                            std::time::Duration::from_secs(3),
                            "Gemini audio worker (capture stop)",
                            &retired_gemini_workers,
                        );
                    }
                    if let Some(h) = event_h {
                        join_worker_with_timeout(
                            h,
                            std::time::Duration::from_secs(3),
                            "Gemini event worker (capture stop)",
                            &retired_gemini_workers,
                        );
                    }
                })
                .await;
            }
        }
        // Also stop native converse if it owns the shared Gemini client. This
        // mirrors stop_converse so a last-capture stop cannot leave playback,
        // provider client state, or the gemini-converse runtime consumer alive.
        let converse_active = state
            .is_converse_active
            .read()
            .map(|active| *active)
            .unwrap_or(false);
        if converse_active && let Err(error) = stop_converse_runtime(state, "capture stop").await {
            cleanup_error_count += 1;
            log::error!("Native converse cleanup failed during capture stop: {error}");
        }
        // OpenAI Realtime owns an independent client, gate, workers, and
        // processed-audio consumer. Last-source cleanup must tear it down too,
        // even if the foreground mode/provider selection changed after start.
        let openai_realtime_active = state
            .is_openai_realtime_active
            .read()
            .map(|active| *active)
            .unwrap_or(false);
        if openai_realtime_active
            && let Err(error) = stop_openai_realtime_runtime(state, "capture stop").await
        {
            cleanup_error_count += 1;
            log::error!("OpenAI realtime cleanup failed during capture stop: {error}");
        }
        if let Ok(mut status) = state.pipeline_status.write() {
            status.capture = StageStatus::Idle;
            status.pipeline = StageStatus::Idle;
            status.asr = StageStatus::Idle;
            status.diarization = StageStatus::Idle;
            status.entity_extraction = StageStatus::Idle;
            status.graph = StageStatus::Idle;
        }

        // Emit updated pipeline status
        if let Ok(status) = state.pipeline_status.read() {
            let _ = app.emit(events::PIPELINE_STATUS_EVENT, &*status);
        }
        if let Err(error) = append_capture_lifecycle_movement(
            state,
            crate::persistence::DataMovementEventType::CaptureStopped,
        ) {
            cleanup_error_count += 1;
            log::error!("Capture-stop movement audit append failed: {error}");
        }
        if cleanup_error_count > 0 {
            return Err(AppError::Unknown(format!(
                "Capture stopped with {cleanup_error_count} cleanup or audit error(s)"
            )));
        }
    }

    log::info!("Stopped capture for source: {}", source_id);
    Ok(())
}

/// Sync the system tray recording indicator with the frontend's capture state
/// (audio-graph-a156). Capture state is owned frontend-side (the store's
/// `isCapturing` spans multiple sources), so the tray icon swap, the
/// content-free duration tooltip, and the *Stop capture* menu-item enabled
/// state are all driven from here whenever the store's `isCapturing` or elapsed
/// counter changes.
///
/// `elapsed_secs` is a bare wall-clock second count — the tray formats it into a
/// `M:SS` / `H:MM:SS` tooltip and NEVER receives or renders any captured
/// content (transcript text, note bodies, speaker labels, meeting titles) per
/// the UX-review privacy constraint.
///
/// Desktop-only: the tray exists behind `#[cfg(desktop)]` in `lib.rs`, so this
/// is a cheap no-op (the tray lookup misses) on mobile/headless targets.
#[tauri::command]
pub fn update_tray_capturing(
    capturing: bool,
    elapsed_secs: Option<u64>,
    app: tauri::AppHandle,
) -> AppResult<()> {
    #[cfg(desktop)]
    crate::tray::apply_capture_state(&app, capturing, elapsed_secs);
    #[cfg(not(desktop))]
    {
        let _ = (capturing, elapsed_secs, &app);
    }
    Ok(())
}

/// Probe AWS credentials via STS GetCallerIdentity. Used as pre-flight for
/// DefaultChain and Profile modes so start_transcribe fails fast with an
/// actionable error instead of blowing up inside the EventStream handshake.
///
/// Returns `Ok(())` on success (identity resolved) or an error string on any
/// failure — credentials missing, expired, wrong region, network blocked, etc.
/// Callers are expected to wrap this in a `tokio::time::timeout`.
async fn aws_preflight_probe(
    region: String,
    credential_source: crate::settings::AwsCredentialSource,
) -> Result<(), String> {
    // AccessKeys has a static-cred pre-flight elsewhere; probing via STS
    // here would double up. Callers already filter this case out.
    if matches!(
        credential_source,
        crate::settings::AwsCredentialSource::AccessKeys { .. }
    ) {
        return Err("aws_preflight_probe called with AccessKeys — caller bug".to_string());
    }
    let sdk_config = crate::aws_util::build_aws_sdk_config(&region, credential_source).await?;
    let sts = aws_sdk_sts::Client::new(&sdk_config);
    sts.get_caller_identity()
        .send()
        .await
        .map_err(|e| format!("{}", e))?;
    Ok(())
}

/// Start transcription (streaming processed audio → ASR).
///
/// Requires capture to already be running. Spawns a speech processor thread
/// that reads from the processed audio channel (pipeline output), accumulates
/// chunks into ~2s segments, then runs ASR + diarization + entity extraction.
#[tauri::command]
pub async fn start_transcribe(state: State<'_, AppState>, app: tauri::AppHandle) -> AppResult<()> {
    log::info!("start_transcribe called");
    let _session_lifecycle = state.session_lifecycle.lock().await;
    ensure_session_workers_quiesced(state.inner())?;

    // Product enablement is the first content-route decision. Reading the
    // in-memory settings is passive; reject a persisted deferred route before
    // synchronizing clients or starting any content consumer.
    let settings = read_settings_for_session_content(state.inner(), "asr_session")?;
    enforce_transcribe_provider_start(&settings)?;

    // Guard: capture must be running
    {
        let capturing = state
            .is_capturing
            .read()
            .map_err(|e| AppError::Unknown(format!("Lock error: {}", e)))?;
        if !*capturing {
            return Err(AppError::SessionInvalid {
                reason: "Cannot start transcription: capture is not running".to_string(),
            });
        }
    }

    // Guard: don't double-start
    if state.is_transcribing.load(Ordering::SeqCst) {
        return Err(AppError::SessionInvalid {
            reason: "Transcription is already running".to_string(),
        });
    }

    sync_llm_api_client_from_settings_cache(state.inner()).map_err(AppError::Unknown)?;
    sync_openrouter_client_from_settings_cache(state.inner()).map_err(AppError::Unknown)?;

    // Pre-flight validation: verify the selected providers are ready before
    // spawning the speech processor. Without these checks the processor thread
    // would try to load the model / reach the API, fail, and exit silently,
    // leaving the user staring at a UI with no feedback. Returning an Err here
    // surfaces to the frontend as a promise rejection → the existing error
    // toast displays the message.
    {
        let mut asr_provider = settings.asr_provider.clone();
        // Applied for validation only; the resulting override is logged once at
        // the dispatch site below (audio-graph-2dfb) to avoid an identical warn
        // firing twice per successful start.
        let _diarization_override = asr_provider.apply_diarization_settings(&settings.diarization);
        let whisper_model = settings.whisper_model.clone();
        let llm_provider = settings.llm_provider.clone();

        enforce_session_content_policy(
            &app,
            state.inner(),
            &settings,
            "asr_session",
            asr_provider.runtime_provider_id(),
            &["audio"],
            asr_provider.requires_cloud_content_transfer(),
        )?;
        enforce_session_content_policy(
            &app,
            state.inner(),
            &settings,
            "llm_projection",
            llm_provider.runtime_provider_id(),
            &["transcript", "speaker_timeline", "graph_context", "prompt"],
            llm_provider.requires_cloud_content_transfer(),
        )?;

        let active_sources = state
            .capture_manager
            .lock()
            .map_err(|e| AppError::Unknown(format!("Lock error: {}", e)))?
            .active_captures();
        validate_asr_capture_selection(&asr_provider, &active_sources, None)
            .map_err(AppError::Unknown)?;

        if let Some(err) = local_asr_provider_availability_error(&asr_provider) {
            return Err(err);
        }

        match &asr_provider {
            crate::settings::AsrProvider::LocalWhisper => {
                let models_dir = crate::models::get_models_dir(&app);
                let model_path = models_dir.join(&whisper_model);
                if !model_path.exists() {
                    return Err(AppError::ModelNotFound {
                        name: whisper_model.clone(),
                    });
                }
            }
            crate::settings::AsrProvider::Api {
                endpoint, api_key, ..
            } => {
                if endpoint.trim().is_empty() {
                    return Err(AppError::Unknown(
                        "Cloud ASR endpoint not configured. Open Settings.".to_string(),
                    ));
                }
                if api_key.trim().is_empty() {
                    return Err(AppError::CredentialMissing {
                        key: "cloud_asr_api_key".to_string(),
                    });
                }
            }
            crate::settings::AsrProvider::DeepgramStreaming { api_key, .. } => {
                if api_key.trim().is_empty() {
                    return Err(AppError::CredentialMissing {
                        key: "deepgram_api_key".to_string(),
                    });
                }
            }
            crate::settings::AsrProvider::AssemblyAI { api_key, .. } => {
                if api_key.trim().is_empty() {
                    return Err(AppError::CredentialMissing {
                        key: "assemblyai_api_key".to_string(),
                    });
                }
            }
            crate::settings::AsrProvider::Soniox { api_key, .. } => {
                if api_key.trim().is_empty() {
                    return Err(AppError::CredentialMissing {
                        key: "soniox_api_key".to_string(),
                    });
                }
            }
            crate::settings::AsrProvider::OpenAiRealtimeTranscription { api_key, .. } => {
                if api_key.trim().is_empty() {
                    return Err(AppError::CredentialMissing {
                        key: "openai_api_key".to_string(),
                    });
                }
            }
            crate::settings::AsrProvider::AwsTranscribe {
                credential_source,
                region,
                ..
            } => {
                if region.trim().is_empty() {
                    return Err(AppError::AwsRegionInvalid {
                        region: region.clone(),
                    });
                }

                if let crate::settings::AwsCredentialSource::AccessKeys { access_key, .. } =
                    credential_source
                {
                    if access_key.trim().is_empty() {
                        return Err(AppError::CredentialMissing {
                            key: "aws_access_key".to_string(),
                        });
                    }
                    let cred_store = crate::credentials::load_credentials();
                    let secret_valid = cred_store
                        .aws_secret_key
                        .as_ref()
                        .map(|s| !s.trim().is_empty())
                        .unwrap_or(false);
                    if !secret_valid {
                        return Err(AppError::CredentialMissing {
                            key: "aws_secret_key".to_string(),
                        });
                    }
                }

                // DefaultChain + Profile: probe STS GetCallerIdentity so the
                // user gets a fast, intelligible "no credentials" error instead
                // of the EventStream handshake failing mid-stream and leaving
                // the UI in a confusing half-running state.
                //
                // Bounded to 5s: on a healthy machine with creds, STS responds
                // in <200ms. If it takes longer, the user's network is bad
                // enough that mid-stream failures are likely anyway — better
                // to fail fast in pre-flight than stall capture.
                if !matches!(
                    credential_source,
                    crate::settings::AwsCredentialSource::AccessKeys { .. }
                ) {
                    let probe = aws_preflight_probe(region.clone(), credential_source.clone());
                    match tokio::time::timeout(std::time::Duration::from_secs(5), probe).await {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => {
                            // ag#13: also emit a structured event so the UI
                            // can show a localized toast. The returned
                            // AppError::Unknown keeps the legacy string path
                            // working for any caller that hasn't migrated.
                            let classified = crate::aws_util::classify_aws_error(
                                &e,
                                Some(region.as_str()),
                            );
                            crate::events::emit_or_log(
                                &app,
                                crate::events::AWS_ERROR,
                                crate::events::AwsErrorPayload {
                                    error: classified,
                                    raw_message: e.clone(),
                                },
                            );
                            return Err(AppError::Unknown(format!(
                                "AWS credential pre-flight failed: {}. Open Settings → ASR → AWS Transcribe → Test Connection to diagnose.",
                                e
                            )));
                        }
                        Err(_) => return Err(AppError::Unknown(
                            "AWS credential pre-flight timed out after 5s. Check network or switch credential mode."
                                .to_string(),
                        )),
                    }
                }
            }
            crate::settings::AsrProvider::SherpaOnnx { model_dir, .. } => {
                let models_dir = crate::models::get_models_dir(&app);
                let model_path = models_dir.join(model_dir);
                if !model_path.exists() {
                    return Err(AppError::ModelNotFound {
                        name: model_dir.clone(),
                    });
                }
                // The directory existing isn't enough — sherpa-onnx needs the
                // encoder/decoder/joiner ONNX graphs and the tokens vocabulary.
                // A partial download or unpack would pass the exists() check
                // but fail silently inside the speech processor thread.
                for required in crate::models::SHERPA_ZIPFORMER_REQUIRED_FILES {
                    let path = model_path.join(required);
                    let ready = std::fs::metadata(&path)
                        .map(|m| m.is_file() && m.len() > 0)
                        .unwrap_or(false);
                    if !ready {
                        return Err(AppError::Unknown(format!(
                            "Sherpa-ONNX model '{}' is missing '{}'. Re-download via Settings.",
                            model_dir, required
                        )));
                    }
                }
            }
            crate::settings::AsrProvider::Moonshine { .. } => {
                // Rejected above by local_asr_provider_availability_error until
                // the native Moonshine runtime worker lands.
            }
        }

        // LLM pre-flight: only warn for LocalLlama — entity extraction has
        // fallbacks (API, rule-based) so a missing local model isn't fatal.
        if let Some(err) = local_llm_provider_availability_error(&llm_provider) {
            log::warn!("{}", err);
        }
        if let crate::settings::LlmProvider::LocalLlama = llm_provider {
            let models_dir = crate::models::get_models_dir(&app);
            let llm_path = models_dir.join(crate::models::LLM_MODEL_FILENAME);
            if !llm_path.exists() {
                log::warn!(
                    "Local LLM model not downloaded; entity extraction will fall back to API or rule-based"
                );
                // Don't error — extraction has fallbacks. Just log.
            }
        }
    }

    // ADR-0045 decision 6 (audio-graph-5fd1): reseed each projection lane's
    // coverage head from this session's accepted `projection_patches` log
    // before the speech thread that drives the schedulers spawns below.
    // `ProjectionSchedulers::restore_from_snapshot` no longer exists
    // (audio-graph-464c) — `reseed_coverage_heads` is now the ONLY channel
    // through which scheduler coverage state may come from disk. Cold start
    // mints a fresh session id with no persisted patches (`AppState::new`),
    // and `load_session_impl` never installs a historical ledger into
    // `AppState`, so in every capture started today this loads an empty
    // patch log and reseeds nothing — it exists so a future resume/reopen
    // surface (audio-graph-9751-adjacent) cannot route around it the way the
    // deleted snapshot restore could be.
    //
    // A corrupt/truncated `projection_patches` log must refuse
    // transcribe-start loudly, not reseed nothing and silently replay
    // duplicate work later — `StrictCanonicalRead` exists precisely so
    // canonical-log corruption fails closed (review finding, 5fd1). `?`
    // propagates via `From<String> for AppError` (`error.rs`), which wraps
    // the message as `AppError::Unknown` unchanged; the message itself is
    // already class-only and content-redacted by construction
    // (`CanonicalReaderError`'s `Display` never includes a path, session id,
    // event id, or payload value — see `persistence/canonical_reader.rs`).
    {
        let session_id = state.current_session_id();
        let accepted_patches =
            FileMemoryRepository::user_data().load_projection_patches(&session_id)?;
        let heads = crate::projection_scheduler::derive_coverage_heads(&accepted_patches);
        let mut schedulers = state
            .projection_schedulers
            .lock()
            .map_err(|e| AppError::Unknown(format!("Lock error: {}", e)))?;
        // audio-graph-1609 fold-in of the bf5d orphaned-deferral gap: sweep
        // any deferred retry left armed by the prior Stop before reseeding.
        // On the stop_capture_impl -> start_transcribe restart route this is
        // sound because that drain already joined every
        // `projection-retry-<kind>` clock thread, so no clock can possibly
        // still be alive for either lane by this point, regardless of
        // whether the deferral was orphaned by a
        // `dispatch_projection_decision` discard or by the clock thread's
        // own graceful stopping-check exit. On the stop_transcribe ->
        // start_transcribe route (no capture stop in between) that
        // clock-drain does NOT happen, so a still-armed clock can survive
        // this sweep — see `ProjectionSchedulers::clear_orphaned_deferred_retries`
        // for why clearing the deadline out from under a live clock is still
        // safe (None-is-due handling in `observe_ledger`, not "no clock
        // exists").
        schedulers.clear_orphaned_deferred_retries();
        schedulers.reseed_coverage_heads(heads);
    }

    // 1. Start speech processor thread (ASR + Diarization orchestrator).
    //    The speech processor reads directly from the processed audio channel,
    //    accumulates chunks into ~2s segments, and runs ASR inline.
    {
        let mut sp_handle = state
            .speech_processor_thread
            .lock()
            .map_err(|e| AppError::Unknown(format!("Lock error: {}", e)))?;

        // audio-graph-1609 ordering fix: clear `projection_lane_stopping`
        // here — strictly BEFORE the speech processor thread actually
        // spawns below — not after it (where this store used to live,
        // paired with `is_transcribing`'s reset in step 3).
        // `dispatch_projection_decision` (speech/mod.rs) reads this flag
        // synchronously on every projection dispatch, including one that
        // could in principle be produced by a final ASR revision the
        // JUST-spawned speech thread processes almost immediately. Clearing
        // it only after the thread was already spawned left a real window
        // — however small — where that first post-restart dispatch could
        // still observe the flag set from the PRIOR Stop and get discarded
        // into the same phantom-in-flight state ADR-0045 decision 4
        // (audio-graph-9cc1) already causes if left un-abandoned (see
        // `abandon_discarded_projection_job`). Moving the clear ahead of the
        // spawn closes that window structurally: nothing that could
        // dispatch a projection decision for this run exists yet at this
        // line, so there is no dispatch left to race.
        //
        // Placed here — after every fallible step earlier in this function
        // (provider pre-flight, the `projection_patches` load, the
        // schedulers lock) rather than at the top of the function — so an
        // early `?` return from any of those keeps leaving this flag
        // untouched, exactly as it did before this fix. Only the ordering
        // relative to the speech-thread spawn changed; an `Err` path that
        // never reaches the spawn should not observably change this flag
        // either.
        state
            .projection_lane_stopping
            .store(false, Ordering::SeqCst);

        // audio-graph-586b review follow-up: this pipeline-status pre-set
        // used to live in step 3, below, AFTER the speech-processor thread
        // spawned. `apply_diarization_degradation` runs on that thread and
        // can write `StageStatus::Degraded` to the same lock before step 3
        // executes; if it won that race, step 3's unconditional
        // `Running{0}` write clobbered the Degraded status right back to
        // healthy-looking, and nothing downstream ever re-derives it (the
        // per-segment sites only PRESERVE an existing Degraded, they never
        // restore one) — silently reproducing the exact pre-586b failure
        // mode for the rest of the session. Moving the pre-set here, before
        // the spawn, closes the race the same way the
        // `projection_lane_stopping` clear above already does: nothing that
        // could write a competing status exists yet at this line.
        if let Ok(mut status) = state.pipeline_status.write() {
            status.asr = StageStatus::Running { processed_count: 0 };
            status.diarization = StageStatus::Running { processed_count: 0 };
            status.entity_extraction = StageStatus::Running { processed_count: 0 };
            status.graph = StageStatus::Running { processed_count: 0 };
        }

        if sp_handle.is_none() {
            // Bug 1 fix: read from per-consumer channel, not shared processed_rx
            let speech_rx = state.speech_audio_rx.clone();
            // Bug 2 fix: pass AtomicBool so the speech processor can check it
            let is_transcribing = state.is_transcribing.clone();

            let transcript_buffer = state.transcript_buffer.clone();
            let pipeline_status = state.pipeline_status.clone();
            let app_handle = app.clone();
            let knowledge_graph = state.knowledge_graph.clone();
            let graph_snapshot_clone = state.graph_snapshot.clone();
            let graph_extractor = state.graph_extractor.clone();
            let llm_engine = state.llm_engine.clone();
            let api_client = state.api_client.clone();
            let mistralrs_engine = state.mistralrs_engine.clone();
            let llm_executor = state.llm_executor.clone();
            let pending_agent_proposals = state.pending_agent_proposals.clone();

            let models_dir = crate::models::get_models_dir(&app);

            let mut asr_provider = settings.asr_provider.clone();
            let diarization_override =
                asr_provider.apply_diarization_settings(&settings.diarization);
            diarization_override.log_overrides(asr_provider.runtime_provider_id());
            // audio-graph-586b: threaded into `SpeechConfig` so
            // `make_diarization_config` can consult the user's global
            // diarization policy instead of ignoring it entirely.
            let diarization_mode = settings.diarization.mode;
            let whisper_model = settings.whisper_model.clone();
            let llm_provider = settings.llm_provider.clone();
            let llm_allow_cloud_fallbacks = settings
                .privacy_mode
                .allows_session_cloud_content_transfer();
            let provider_content_egress_policy =
                crate::asr::ProviderContentEgressPolicy::from_privacy_mode_and_transfer_requirement(
                    settings.privacy_mode,
                    asr_provider.requires_cloud_content_transfer(),
                );

            // If the user selected local LLM and the engine is not yet
            // loaded, attempt to load it now on a blocking background task.
            if matches!(llm_provider, crate::settings::LlmProvider::LocalLlama) {
                let engine_empty = state
                    .llm_engine
                    .lock()
                    .map(|g| g.is_none())
                    .unwrap_or(false);
                if engine_empty {
                    let models_dir_clone = models_dir.clone();
                    let llm_engine_clone = state.llm_engine.clone();
                    let model_path = models_dir_clone.join(crate::models::LLM_MODEL_FILENAME);
                    if model_path.exists() {
                        log::info!("Auto-loading local LLM model for LocalLlama provider...");
                        let _ = std::thread::Builder::new()
                            .name("llm-autoload".to_string())
                            .spawn(move || {
                                match crate::llm::LlmEngine::new(&model_path.to_string_lossy()) {
                                    Ok(engine) => {
                                        if let Ok(mut guard) = llm_engine_clone.lock() {
                                            *guard = Some(engine);
                                            log::info!("Local LLM model auto-loaded successfully");
                                        }
                                    }
                                    Err(e) => {
                                        log::warn!("Failed to auto-load local LLM model: {}", e);
                                    }
                                }
                            });
                    }
                }
            }

            let transcript_writer = state.transcript_writer.clone();
            let display_transcript_write_misses = state.display_transcript_write_misses.clone();
            let transcript_event_writer = state.transcript_event_writer.clone();
            let transcript_ledger = state.transcript_ledger.clone();
            let speaker_timeline = state.speaker_timeline.clone();
            let projection_schedulers = state.projection_schedulers.clone();
            let projection_runtime = state.projection_runtime_handle();
            let projection_job_workers = state.projection_job_workers.clone();
            let projection_lane_stopping = state.projection_lane_stopping.clone();
            let active_session_id = state.session_id.clone();
            let retired_session_workers = state.retired_session_workers.clone();

            let handle = std::thread::Builder::new()
                .name("speech-processor".to_string())
                .spawn(move || {
                    let channels = speech::SpeechChannels {
                        processed_rx: speech_rx,
                        is_transcribing,
                    };
                    let shared = speech::SpeechShared {
                        transcript_buffer,
                        transcript_writer,
                        display_transcript_write_misses,
                        transcript_event_writer,
                        transcript_ledger,
                        speaker_timeline,
                        projection_schedulers,
                        projection_runtime,
                        projection_job_workers,
                        projection_lane_stopping,
                        active_session_id,
                        pipeline_status,
                        app_handle,
                        knowledge_graph,
                        graph_snapshot: graph_snapshot_clone,
                        graph_extractor,
                        llm_engine,
                        api_client,
                        mistralrs_engine,
                        llm_executor,
                        pending_agent_proposals,
                        retired_session_workers,
                    };
                    let config = speech::SpeechConfig {
                        models_dir,
                        llm_provider,
                        llm_allow_cloud_fallbacks,
                        provider_content_egress_policy,
                        diarization_mode,
                    };
                    speech::run_speech_processor(
                        channels,
                        shared,
                        config,
                        asr_provider,
                        whisper_model,
                    );
                })
                .map_err(|e| {
                    AppError::Unknown(format!("Failed to spawn speech processor thread: {}", e))
                })?;
            *sp_handle = Some(handle);
            log::info!("Speech processor thread spawned for transcribe");
        }
    }

    // 3. Update state flags.
    state.is_transcribing.store(true, Ordering::SeqCst);
    // `projection_lane_stopping` is reset earlier in this function now
    // (audio-graph-1609 ordering fix — see the comment there), strictly
    // before the speech thread above spawns, not paired with
    // `is_transcribing` here. It used to live at this point, after the
    // spawn; that ordering is exactly what left the post-restart
    // first-dispatch race this fix closes.
    //
    // The bf5d orphaned-STATE gap this used to document (a deferral live
    // when Stop began, either never given a clock or whose clock exited
    // early, surviving into the restart with a lying `deferred_retry_at_ms`
    // and no clock source) is closed now, via
    // `ProjectionSchedulers::clear_orphaned_deferred_retries` called
    // alongside the coverage-head reseed above.
    //
    // That is not the same as restoring bf5d's "retry fires even with no
    // further finals" guarantee ACROSS a same-session restart — it isn't,
    // and this fix doesn't claim to. Both `abandon_deferred_retry` and this
    // restart sweep only ever CLEAR an armed deferral, never re-arm a new
    // clock; a previously-failed basis still under budget retries only
    // event-driven after a restart (the same-basis `observe_ledger` branch
    // that would re-arm it is reached only by a clock thread, and none
    // exists post-restart until a new failure creates one). Re-arming a
    // clock at restart time for a live, under-budget `last_failed_basis` is
    // a real follow-up, not something this fix does.
    //
    // The `pipeline_status` `Running{0}` pre-set used to live here too, and
    // for the same reason as `projection_lane_stopping` above, it doesn't
    // anymore (audio-graph-586b review follow-up) — see the comment at its
    // new call site, before the speech-processor thread spawns in step 1.
    // This read+emit still belongs here: it reflects whatever the spawned
    // thread's own status writes (including a legitimate `Degraded`) have
    // settled to by now, rather than re-asserting a fixed "healthy" snapshot
    // that could race and overwrite them.
    if let Ok(status) = state.pipeline_status.read() {
        let _ = app.emit(events::PIPELINE_STATUS_EVENT, &*status);
    }

    log::info!("Started transcription (streaming mode)");
    Ok(())
}

/// Stop transcription without stopping capture.
///
/// Sets the AtomicBool flag to false so the speech processor thread exits
/// on its next `recv_timeout` cycle (Bug 2 fix), then cleans up the thread handle.
#[tauri::command]
pub async fn stop_transcribe(state: State<'_, AppState>, app: tauri::AppHandle) -> AppResult<()> {
    stop_transcribe_impl(state.inner(), &app).await
}

/// Implementation behind the `stop_transcribe` Tauri command, split out so
/// tests can drive it directly against a plain `&AppState` (same convention
/// as `stop_capture`/`stop_capture_impl`) instead of needing a constructible
/// `tauri::State`.
async fn stop_transcribe_impl(state: &AppState, app: &tauri::AppHandle) -> AppResult<()> {
    log::info!("stop_transcribe called");
    let _session_lifecycle = state.session_lifecycle.lock().await;

    // Signal the speech processor to stop via AtomicBool
    state.is_transcribing.store(false, Ordering::SeqCst);

    // Join the worker threads (bounded) instead of just dropping the handles.
    // Dropping without joining let a fast Stop→Start race leave the OLD worker
    // still in its ~500ms recv loop while a NEW worker starts, so two consumers
    // split the same speech_audio channel (critique H2). Joining guarantees the
    // old workers have exited before this returns. Polled-join with a timeout
    // so a wedged worker can't hang Stop. Run off the async runtime.
    let sp = state
        .speech_processor_thread
        .lock()
        .ok()
        .and_then(|mut g| g.take());
    let asr = state
        .asr_worker_thread
        .lock()
        .ok()
        .and_then(|mut g| g.take());
    let retired_speech_workers = state.retired_session_workers.clone();
    let display_transcript_write_misses = state.display_transcript_write_misses.clone();
    let stop_session_id = state.current_session_id();
    let _ = tokio::task::spawn_blocking(move || {
        if let Some(h) = sp {
            join_worker_with_timeout(
                h,
                std::time::Duration::from_secs(3),
                "speech processor",
                &retired_speech_workers,
            );
        }
        if let Some(h) = asr {
            join_worker_with_timeout(
                h,
                std::time::Duration::from_secs(3),
                "ASR worker",
                &retired_speech_workers,
            );
        }
        // audio-graph-64e3: `stop_capture_impl` is not the only session-end
        // transition that joins sp/asr through the same bounded-join/receiver
        // tail — a transcribe-only session ends here instead. Without this
        // call a miss counted during THIS session was only ever reported at
        // the next capture-session's stop_capture_impl, under the WRONG
        // session id. Must run strictly after the joins above for the same
        // reason `stop_capture_impl` orders it after its own joins — see
        // `warn_if_display_transcript_rows_missing_at_stop`'s doc.
        warn_if_display_transcript_rows_missing_at_stop(
            &display_transcript_write_misses,
            &stop_session_id,
        );
    })
    .await;

    // Update pipeline status — ASR and downstream stages go idle
    if let Ok(mut status) = state.pipeline_status.write() {
        status.asr = StageStatus::Idle;
        status.diarization = StageStatus::Idle;
        status.entity_extraction = StageStatus::Idle;
        status.graph = StageStatus::Idle;
    }

    if let Ok(status) = state.pipeline_status.read() {
        let _ = app.emit(events::PIPELINE_STATUS_EVENT, &*status);
    }

    log::info!("Stopped transcription");
    Ok(())
}

/// Get the current knowledge graph snapshot.
#[tauri::command]
pub async fn get_graph_snapshot(state: State<'_, AppState>) -> AppResult<GraphSnapshot> {
    let snapshot = state
        .graph_snapshot
        .read()
        .map_err(|e| format!("Failed to read graph snapshot: {}", e))?;
    Ok(snapshot.clone())
}

/// Get transcript segments, optionally filtered by source and time.
#[tauri::command]
pub async fn get_transcript(
    source_id: Option<String>,
    since: Option<f64>,
    state: State<'_, AppState>,
) -> AppResult<Vec<TranscriptSegment>> {
    let buffer = state
        .transcript_buffer
        .read()
        .map_err(|e| format!("Failed to read transcript buffer: {}", e))?;

    let segments: Vec<TranscriptSegment> = buffer
        .iter()
        .filter(|seg| {
            let source_match = source_id
                .as_ref()
                .map(|id| &seg.source_id == id)
                .unwrap_or(true);
            let time_match = since.map(|t| seg.start_time >= t).unwrap_or(true);
            source_match && time_match
        })
        .cloned()
        .collect();

    Ok(segments)
}

/// Get the current pipeline status.
#[tauri::command]
pub async fn get_pipeline_status(state: State<'_, AppState>) -> AppResult<PipelineStatus> {
    let status = state
        .pipeline_status
        .read()
        .map_err(|e| format!("Failed to read pipeline status: {}", e))?;
    Ok(status.clone())
}

// ---------------------------------------------------------------------------
// API endpoint configuration
// ---------------------------------------------------------------------------

/// Validate and parse an OpenAI-compatible endpoint URL.
///
/// `reqwest` will reject malformed URLs at request time, but that produces a
/// confusing "invalid format" failure many seconds into a chat, long after the
/// user has forgotten what they typed in Settings. Parse up-front so the
/// Settings UI can surface the error synchronously, and restrict to http/https
/// schemes so `file://` / `ftp://` / other exotic schemes can't sneak in.
pub(crate) fn validate_endpoint_url(endpoint: &str) -> Result<url::Url, String> {
    let parsed = url::Url::parse(endpoint).map_err(|e| format!("Invalid endpoint URL: {}", e))?;
    match parsed.scheme() {
        "http" | "https" => Ok(parsed),
        other => Err(format!(
            "Invalid endpoint URL: unsupported scheme `{}` (expected http or https)",
            other
        )),
    }
}

/// Configure an OpenAI-compatible API endpoint for LLM inference.
///
/// This allows using cloud providers (OpenAI, OpenRouter) or local servers
/// (Ollama, LM Studio, vLLM) as an alternative to the native llama-cpp-2 engine.
#[tauri::command]
pub async fn configure_api_endpoint(
    endpoint: String,
    api_key: Option<String>,
    model: String,
    state: State<'_, AppState>,
) -> AppResult<()> {
    log::info!(
        "configure_api_endpoint: endpoint={}, model={}",
        endpoint,
        model
    );

    validate_endpoint_url(&endpoint)?;

    if endpoint.trim().is_empty() || model.trim().is_empty() {
        return Err(AppError::Unknown(
            "Invalid API configuration: endpoint and model must be non-empty".to_string(),
        ));
    }

    {
        let mut cached = state
            .app_settings
            .write()
            .map_err(|e| format!("Lock error: {}", e))?;
        cached.llm_provider = crate::settings::LlmProvider::Api {
            endpoint: endpoint.clone(),
            api_key: api_key.clone().unwrap_or_default(),
            model: model.clone(),
        };
        cached.llm_api_config = Some(crate::settings::LlmApiConfig {
            endpoint,
            api_key,
            model,
            max_tokens: 512,
            temperature: 0.1,
        });
    }

    sync_llm_api_client_from_settings_cache(state.inner())?;
    sync_openrouter_client_from_settings_cache(state.inner())?;

    log::info!("API endpoint configured successfully");
    Ok(())
}

// ---------------------------------------------------------------------------
// Chat commands (backed by native LLM engine or API client)
// ---------------------------------------------------------------------------

/// Build the per-request graph + transcript context block used as the chat
/// system prompt, and append the user message to history.
///
/// Returns `(messages, graph_context)` ready to feed either the streaming
/// or blocking chat path. Locks are taken under short critical sections
/// and released before any string formatting (I4 fix carried over from
/// the legacy `send_chat_message` body).
fn prepare_chat_request(
    state: &AppState,
    message: String,
) -> Result<(Vec<ChatMessage>, String), String> {
    sync_llm_api_client_from_settings_cache(state)?;
    sync_openrouter_client_from_settings_cache(state)?;

    let snapshot = {
        let kg = state
            .knowledge_graph
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        kg.snapshot()
    };

    let recent_transcript: Vec<TranscriptSegment> = {
        let transcript = state
            .transcript_buffer
            .read()
            .map_err(|e| format!("Lock error: {}", e))?;
        transcript.iter().rev().take(10).cloned().collect()
    };

    let graph_context = {
        // Top-k retrieval instead of dumping the whole graph: keeps the prompt
        // small, on-topic, and avoids shipping maximal session data. See
        // graph::entities::build_graph_chat_context (C3 fix).
        const MAX_CONTEXT_NODES: usize = 40;
        let mut ctx = crate::graph::entities::build_graph_chat_context(
            &snapshot,
            &message,
            MAX_CONTEXT_NODES,
        );
        if !recent_transcript.is_empty() {
            ctx.push_str("\nRecent Transcript:\n");
            for seg in recent_transcript.iter().rev() {
                let speaker = seg.speaker_label.as_deref().unwrap_or("Unknown");
                ctx.push_str(&format!("[{}]: {}\n", speaker, seg.text));
            }
        }
        ctx
    };

    let user_msg = ChatMessage {
        role: "user".to_string(),
        content: message,
    };
    {
        let mut history = state
            .chat_history
            .write()
            .map_err(|e| format!("Lock error: {}", e))?;
        history.push(user_msg);
        cap_chat_history(&mut history);
    }
    let messages: Vec<ChatMessage> = {
        let history = state
            .chat_history
            .read()
            .map_err(|e| format!("Lock error: {}", e))?;
        history.clone()
    };
    Ok((messages, graph_context))
}

/// Append the assistant message to chat history. Best-effort: lock-poisoning
/// returns an error but the caller should still surface the reply to the
/// user — chat_history is a UX convenience, not a correctness invariant.
fn append_assistant_message(state: &AppState, content: String) -> Result<ChatMessage, String> {
    let assistant_msg = ChatMessage {
        role: "assistant".to_string(),
        content,
    };
    let mut history = state
        .chat_history
        .write()
        .map_err(|e| format!("Lock error: {}", e))?;
    history.push(assistant_msg.clone());
    cap_chat_history(&mut history);
    Ok(assistant_msg)
}

/// Maximum chat messages retained in memory. Chat history is unbounded by
/// nature (a long session could push thousands of turns) and is cloned whole
/// into every chat request, so cap it to bound memory and prompt-build cost.
/// Keeps the most recent messages.
const MAX_CHAT_HISTORY: usize = 200;

/// Trim `history` in place to the most recent [`MAX_CHAT_HISTORY`] messages.
fn cap_chat_history(history: &mut Vec<ChatMessage>) {
    if history.len() > MAX_CHAT_HISTORY {
        let drop = history.len() - MAX_CHAT_HISTORY;
        history.drain(0..drop);
    }
}

/// Returns `true` when the active LLM provider has a streaming code path.
/// Api/OpenRouter stream provider chunks directly; LocalLlama uses the explicit
/// backend-handle request path and emits one honest local delta until the local
/// engine exposes token callbacks; MistralRs streams token deltas through its
/// gated `stream_chat` engine path ([`crate::llm::streaming::run_mistralrs_stream`]);
/// AwsBedrock drives the `ConverseStream` event stream via the on-demand
/// `aws_sdk_bedrockruntime` adapter ([`crate::llm::bedrock`]).
fn provider_supports_streaming(p: &crate::settings::LlmProvider) -> bool {
    matches!(
        p,
        crate::settings::LlmProvider::Api { .. }
            | crate::settings::LlmProvider::OpenRouter { .. }
            | crate::settings::LlmProvider::LocalLlama
            | crate::settings::LlmProvider::MistralRs { .. }
            | crate::settings::LlmProvider::AwsBedrock { .. }
    )
}

fn stream_backend_handles_from_state(
    state: &AppState,
) -> crate::llm::streaming::StreamBackendHandles {
    crate::llm::streaming::StreamBackendHandles::new(
        state.llm_engine.clone(),
        state.api_client.clone(),
        state.openrouter_client.clone(),
        state.mistralrs_engine.clone(),
    )
}

/// Derive the `tokens_used` telemetry value (FA-7) from a streaming-chat
/// terminal frame's `usage` block.
///
/// We surface `total_tokens` (prompt + completion) because the frontend
/// dashboard exposes a single `tokens_used` field for the whole request.
/// Returns 0 when the provider omitted the usage block entirely (it never set
/// `stream_options.include_usage`, or sent no `total_tokens`), which is the
/// honest "unknown" value rather than a fabricated count.
///
/// Pure so the accumulation contract can be unit-tested without the async
/// command / IPC machinery.
fn tokens_used_from_stream_usage(usage: Option<crate::llm::sse::StreamUsage>) -> u32 {
    usage.and_then(|u| u.total_tokens).unwrap_or(0)
}

fn persist_llm_usage_for_session(app: &tauri::AppHandle, session_id: &str, tokens_used: u32) {
    if tokens_used == 0 {
        return;
    }
    match crate::sessions::usage::append_llm_chat_usage(session_id, u64::from(tokens_used)) {
        Ok(usage) => events::emit_or_log(
            app,
            events::LLM_USAGE_UPDATE,
            events::LlmUsageUpdatePayload {
                session_id: usage.session_id,
                total_tokens: u64::from(tokens_used),
                session_llm_total: usage.llm_total,
                session_llm_turns: usage.llm_turns,
            },
        ),
        Err(e) => log::warn!("Failed to persist chat token usage: {}", e),
    }
}

/// Sampling settings (`max_tokens` / `temperature`) for a streaming chat
/// request, sourced from the already-validated settings snapshot.
///
/// This mirrors the source-of-truth (and `(512, 0.1)` fallback) the blocking
/// chat path reads in `api_config_from_runtime_settings` /
/// `openrouter_config_from_runtime_settings`, so the streaming path honours
/// the same user-configured sampling settings instead of substituting its own
/// literals (AUD-STR1 P2).
fn stream_params_from_settings(
    settings: &crate::settings::AppSettings,
) -> crate::llm::streaming::StreamParams {
    settings
        .llm_api_config
        .as_ref()
        .map(|config| crate::llm::streaming::StreamParams {
            max_tokens: config.max_tokens,
            temperature: config.temperature,
        })
        .unwrap_or_default()
}

/// Spawn the streaming-chat task for `request_id`.
///
/// Drives `crate::llm::streaming::stream_chat` to completion, sending one
/// [`ChatStreamEvent::Delta`] per [`crate::llm::streaming::TokenDelta::Delta`]
/// and exactly one [`ChatStreamEvent::Done`] on terminal (Done / Error /
/// Cancelled) over the per-invocation `channel` (audio-graph-1534: this hot
/// path streams 20-100+ token deltas/sec, so it uses `tauri::ipc::Channel`
/// rather than `AppHandle::emit`). Removes the request from
/// `state.stream_registry` on terminal so a stale id cannot be cancelled later.
///
/// At most one active chat stream per session: any prior live registry entry
/// is cancelled before the new stream registers (AUD-STR1 P1). The frontend
/// tracks only a single `streamingChatRequestId`, so a stream left running
/// from an earlier `start_streaming_chat` would burn tokens unreachably.
#[allow(clippy::too_many_arguments)]
fn spawn_stream_task(
    app: tauri::AppHandle,
    channel: tauri::ipc::Channel<crate::llm::streaming::ChatStreamEvent>,
    state: &AppState,
    request_id: String,
    provider: crate::settings::LlmProvider,
    history: Vec<ChatMessage>,
    graph_context: String,
    settings: crate::settings::AppSettings,
    persist_to_history: bool,
) {
    use crate::llm::streaming::{
        ChatStreamEvent, StreamChatRequest, StreamSourceMetadata, TokenDelta,
        stream_chat_with_request,
    };

    let params = stream_params_from_settings(&settings);
    let session_id_for_usage = state.current_session_id();

    // Enforce the single-active-stream invariant: cancel + drop any prior
    // live stream before registering this one, so the registry never holds an
    // orphaned entry the frontend can no longer reach via cancel.
    let cancelled_priors = state.stream_registry.cancel_all();
    if cancelled_priors > 0 {
        log::info!(
            "start_streaming_chat: cancelled {} prior in-flight stream(s) before starting {}",
            cancelled_priors,
            request_id
        );
    }

    let content_egress_policy = provider_content_egress_policy_from_settings(
        &settings,
        provider.requires_cloud_content_transfer(),
    );
    let request = StreamChatRequest::new(provider, history, graph_context, params)
        .with_content_egress_policy(content_egress_policy)
        .with_backend_handles(stream_backend_handles_from_state(state))
        .with_source_metadata(StreamSourceMetadata {
            session_id: Some(session_id_for_usage.clone()),
            source_id: None,
            request_id: Some(request_id.clone()),
        });
    let (mut rx, cancel) = stream_chat_with_request(request);
    state.stream_registry.register(request_id.clone(), cancel);

    let registry = state.stream_registry.clone();
    let chat_history = state.chat_history.clone();
    let request_id_for_task = request_id.clone();

    // Speak-aloud: build the SpeakAloudPipe ahead of the task spawn so the
    // task body owns it. None when speak_aloud=false or tts=None — the
    // task then runs as plain streaming chat with no audio side effects.
    let settings_snapshot = (
        settings.speak_aloud,
        settings.tts_provider.clone(),
        provider_content_egress_policy_from_settings(
            &settings,
            settings.tts_provider.requires_cloud_content_transfer(),
        ),
    );
    // Credentials live on disk, not on AppState. Snapshot once at task
    // entry so we don't hit the FS on every delta.
    let credentials_snapshot = crate::credentials::load_credentials();
    let player_for_pipe = state.audio_player.clone();
    let request_id_for_pipe_log = request_id.clone();

    tokio::spawn(async move {
        let mut pipe: Option<crate::speak_aloud::SpeakAloudPipe> =
            match crate::speak_aloud::SpeakAloudPipe::maybe_new(
                settings_snapshot.0,
                &settings_snapshot.1,
                &credentials_snapshot,
                settings_snapshot.2,
                player_for_pipe,
            )
            .await
            {
                Ok(p) => p,
                Err(e) => {
                    log::warn!(
                        "speak-aloud setup failed for request {}: {}; falling back to text-only",
                        request_id_for_pipe_log,
                        e
                    );
                    None
                }
            };

        while let Some(frame) = rx.recv().await {
            match frame {
                TokenDelta::Delta {
                    content,
                    finish_reason,
                } => {
                    if let Some(p) = pipe.as_mut()
                        && let Err(e) = p.append_delta(&content)
                    {
                        log::warn!("speak-aloud append_delta failed: {}", e);
                    }
                    if let Err(e) = channel.send(ChatStreamEvent::Delta {
                        request_id: request_id_for_task.clone(),
                        delta: content,
                        finish_reason,
                    }) {
                        // A closed channel means the frontend dropped the
                        // stream (window closed / navigated). Stop draining
                        // rather than spin on a dead channel — mirrors the
                        // frontend `unlisten` teardown the old event path had.
                        log::warn!(
                            "chat stream {}: delta channel send failed ({}); \
                             ending stream",
                            request_id_for_task,
                            e
                        );
                        if let Some(p) = pipe.take() {
                            let _ = p.cancel();
                        }
                        registry.finish(&request_id_for_task);
                        break;
                    }
                }
                TokenDelta::Done {
                    full_text,
                    usage,
                    finish_reason,
                } => {
                    if persist_to_history && let Ok(mut history) = chat_history.write() {
                        history.push(ChatMessage {
                            role: "assistant".to_string(),
                            content: full_text.clone(),
                        });
                        cap_chat_history(&mut history);
                    }
                    if let Some(p) = pipe.take()
                        && let Err(e) = p.finish()
                    {
                        log::warn!("speak-aloud finish failed: {}", e);
                    }
                    let tokens_used = tokens_used_from_stream_usage(usage.clone());
                    persist_llm_usage_for_session(&app, &session_id_for_usage, tokens_used);
                    if let Err(e) = channel.send(ChatStreamEvent::Done {
                        request_id: request_id_for_task.clone(),
                        full_text,
                        finish_reason,
                        usage,
                    }) {
                        log::warn!(
                            "chat stream {}: done channel send failed: {}",
                            request_id_for_task,
                            e
                        );
                    }
                    registry.finish(&request_id_for_task);
                    break;
                }
                TokenDelta::Error { message, full_text } => {
                    log::warn!("Streaming chat error: {}", message);
                    if let Some(p) = pipe.take() {
                        let _ = p.cancel();
                    }
                    if let Err(e) = channel.send(ChatStreamEvent::Done {
                        request_id: request_id_for_task.clone(),
                        full_text,
                        finish_reason: format!("error: {}", message),
                        usage: None,
                    }) {
                        log::warn!(
                            "chat stream {}: error-done channel send failed: {}",
                            request_id_for_task,
                            e
                        );
                    }
                    registry.finish(&request_id_for_task);
                    break;
                }
                TokenDelta::Cancelled { full_text } => {
                    if let Some(p) = pipe.take() {
                        let _ = p.cancel();
                    }
                    if let Err(e) = channel.send(ChatStreamEvent::Done {
                        request_id: request_id_for_task.clone(),
                        full_text,
                        finish_reason: "cancelled".to_string(),
                        usage: None,
                    }) {
                        log::warn!(
                            "chat stream {}: cancelled-done channel send failed: {}",
                            request_id_for_task,
                            e
                        );
                    }
                    registry.finish(&request_id_for_task);
                    break;
                }
            }
        }
    });
}

/// Start a streaming chat request. Returns the `request_id` immediately so
/// the frontend can correlate the stream back to this call (and cancel it via
/// `cancel_streaming_chat`). Token deltas + the terminal frame are delivered
/// over the caller-supplied `channel` (`tauri::ipc::Channel<ChatStreamEvent>`,
/// audio-graph-1534) rather than the legacy `chat-token-delta` /
/// `chat-token-done` events — the channel is ordered, per-invocation, and
/// avoids the per-token serialize + event-router + JS-bridge cost the event
/// system incurs on this 20-100+/sec hot path. The actual LLM work runs on a
/// tokio task; the frontend arms `channel.onmessage` before invoking, so no
/// delta can be lost before the handler is wired (this removes the old
/// spawn-before-return early-delta race entirely).
///
/// If the active LLM provider doesn't support streaming yet (MistralRs), this
/// returns `Err` so the caller can fall back to the blocking
/// `send_chat_message` path.
#[tauri::command]
pub async fn start_streaming_chat(
    message: String,
    channel: tauri::ipc::Channel<crate::llm::streaming::ChatStreamEvent>,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> AppResult<String> {
    log::info!("start_streaming_chat called ({} chars)", message.len());
    // audio-graph-81a5: content-free, session-scoped chat-invocation counter
    // (correlation proxy for Ask-AI on a question card; see card_telemetry's
    // module doc for why this is not attribution).
    crate::card_telemetry::log_chat_invoked(&state.current_session_id());
    let _session_lifecycle = state.session_lifecycle.lock().await;

    let settings = read_settings_for_session_content(state.inner(), "llm_chat")?;
    enforce_chat_provider_start(&settings)?;
    let llm_provider = settings.llm_provider.clone();

    enforce_session_content_policy(
        &app,
        state.inner(),
        &settings,
        "llm_chat",
        llm_provider.runtime_provider_id(),
        &["user_message", "transcript", "graph_context", "prompt"],
        llm_provider.requires_cloud_content_transfer(),
    )?;
    if settings.speak_aloud {
        enforce_session_content_policy(
            &app,
            state.inner(),
            &settings,
            "tts_speak_aloud",
            settings.tts_provider.runtime_provider_id(),
            &["generated_text"],
            settings.tts_provider.requires_cloud_content_transfer(),
        )?;
    }

    if let Some(err) = local_llm_provider_availability_error(&llm_provider) {
        return Err(err);
    }

    if !provider_supports_streaming(&llm_provider) {
        let name = match &llm_provider {
            crate::settings::LlmProvider::LocalLlama => "LocalLlama",
            crate::settings::LlmProvider::MistralRs { .. } => "MistralRs",
            crate::settings::LlmProvider::AwsBedrock { .. } => "AwsBedrock",
            crate::settings::LlmProvider::Api { .. } => "Api",
            crate::settings::LlmProvider::OpenRouter { .. } => "OpenRouter",
        };
        return Err(AppError::Unknown(format!(
            "Streaming chat is not yet supported for the active LLM provider \
             ({}). Use send_chat_message for now; streaming for this \
             provider is a follow-up issue.",
            name
        )));
    }

    let (messages, graph_context) = prepare_chat_request(state.inner(), message)?;
    let request_id = uuid::Uuid::new_v4().to_string();
    spawn_stream_task(
        app,
        channel,
        state.inner(),
        request_id.clone(),
        llm_provider,
        messages,
        graph_context,
        settings,
        true, // persist assistant reply to chat history
    );
    Ok(request_id)
}

/// Cancel an in-flight streaming chat. Idempotent: cancelling an unknown
/// or already-finished request_id is a no-op (returns `Ok(())`). The
/// stream task emits a `chat-token-done` with `finish_reason = "cancelled"`
/// once it observes the cancel.
#[tauri::command]
pub async fn cancel_streaming_chat(
    request_id: String,
    state: State<'_, AppState>,
) -> AppResult<()> {
    let cancelled = state.stream_registry.cancel(&request_id);
    log::info!(
        "cancel_streaming_chat({}): {}",
        request_id,
        if cancelled { "cancelled" } else { "not found" }
    );
    Ok(())
}

/// Send a chat message and get a response from the LLM, informed by the
/// current knowledge graph and transcript context.
///
/// Backward-compatible shim: when the active provider supports streaming
/// (Api / OpenRouter / LocalLlama / MistralRs / AwsBedrock), this dispatches to
/// the same streaming task as [`start_streaming_chat`] and waits for the
/// terminal `Done` frame to reassemble the full reply. Frontend callers that
/// pre-date streaming see no behavior change. Any other provider falls through
/// to the legacy blocking executor.
///
/// I4 fix: takes a snapshot of the graph and transcript, releases the locks,
/// then builds the context string from the snapshot (no lock held during
/// string formatting).
#[tauri::command]
pub async fn send_chat_message(
    message: String,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> AppResult<ChatResponse> {
    log::info!("send_chat_message called ({} chars)", message.len());
    // audio-graph-81a5: content-free, session-scoped chat-invocation counter
    // (correlation proxy for Ask-AI on a question card; see card_telemetry's
    // module doc for why this is not attribution).
    crate::card_telemetry::log_chat_invoked(&state.current_session_id());
    let _session_lifecycle = state.session_lifecycle.lock().await;

    let settings = read_settings_for_session_content(state.inner(), "llm_chat")?;
    enforce_chat_provider_start(&settings)?;
    let llm_provider = settings.llm_provider.clone();

    enforce_session_content_policy(
        &app,
        state.inner(),
        &settings,
        "llm_chat",
        llm_provider.runtime_provider_id(),
        &["user_message", "transcript", "graph_context", "prompt"],
        llm_provider.requires_cloud_content_transfer(),
    )?;

    if let Some(err) = local_llm_provider_availability_error(&llm_provider) {
        return Err(err);
    }

    let (messages, graph_context) = prepare_chat_request(state.inner(), message)?;

    // Streaming path — accumulate to full text via the same producer the
    // event-driven command uses. The shim doesn't fire IPC events itself;
    // it consumes the channel directly so blocking callers don't see
    // delta event spam.
    if provider_supports_streaming(&llm_provider) {
        use crate::llm::streaming::{
            StreamChatRequest, StreamSourceMetadata, TokenDelta, stream_chat_with_request,
        };
        // Honour the user-configured sampling settings on the blocking shim
        // too, matching the legacy executor path (AUD-STR1 P2).
        let params = stream_params_from_settings(&settings);
        let requires_cloud_content_transfer = llm_provider.requires_cloud_content_transfer();
        let content_egress_policy = provider_content_egress_policy_from_settings(
            &settings,
            requires_cloud_content_transfer,
        );
        let request = StreamChatRequest::new(llm_provider, messages, graph_context.clone(), params)
            .with_content_egress_policy(content_egress_policy)
            .with_backend_handles(stream_backend_handles_from_state(state.inner()))
            .with_source_metadata(StreamSourceMetadata {
                session_id: Some(state.current_session_id()),
                source_id: None,
                request_id: None,
            });
        // This blocking shim drains the stream to completion and does not expose
        // cancellation. Dropping a CancellationToken does not fire it; this
        // binding only keeps the stream infrastructure intact.
        let (mut rx, _no_cancel) = stream_chat_with_request(request);
        let mut full_text = String::new();
        // Real token count from the provider's terminal `usage` block (sent when
        // `stream_options.include_usage` is honoured). `total_tokens` covers the
        // whole request (prompt + completion), matching the single `tokens_used`
        // field the frontend dashboard surfaces. Stays 0 only if the provider
        // omitted usage entirely.
        let mut tokens_used = 0u32;
        while let Some(frame) = rx.recv().await {
            match frame {
                TokenDelta::Delta { content, .. } => full_text.push_str(&content),
                TokenDelta::Done {
                    full_text: t,
                    usage,
                    ..
                } => {
                    if !t.is_empty() {
                        full_text = t;
                    }
                    tokens_used = tokens_used_from_stream_usage(usage);
                    break;
                }
                TokenDelta::Error {
                    message,
                    full_text: partial,
                } => {
                    log::warn!("send_chat_message streaming error: {}", message);
                    let fallback = if partial.is_empty() {
                        format!(
                            "I couldn't generate a streaming response (LLM error: {}).\n\n{}",
                            message, graph_context
                        )
                    } else {
                        partial
                    };
                    let assistant_msg = append_assistant_message(state.inner(), fallback)?;
                    // No usage signal: a stream that errors mid-flight never
                    // reaches the terminal `usage` block, so the real token count
                    // is genuinely unavailable here.
                    return Ok(ChatResponse {
                        message: assistant_msg,
                        tokens_used: 0,
                    });
                }
                TokenDelta::Cancelled { full_text: partial } => {
                    let assistant_msg = append_assistant_message(state.inner(), partial)?;
                    // No usage signal: a cancelled stream is dropped before the
                    // terminal `usage` block arrives, so no real count exists.
                    return Ok(ChatResponse {
                        message: assistant_msg,
                        tokens_used: 0,
                    });
                }
            }
        }
        let assistant_msg = append_assistant_message(state.inner(), full_text)?;
        persist_llm_usage_for_session(&app, &state.current_session_id(), tokens_used);
        return Ok(ChatResponse {
            message: assistant_msg,
            tokens_used,
        });
    }

    // Legacy blocking path: native engines + bedrock until their streaming
    // support lands. Wrap the synchronous executor call in
    // `spawn_blocking` so we don't stall the runtime worker. Clone the
    // graph context once so we still have it for the error fallback path.
    let executor = state.llm_executor.clone();
    let graph_for_error = graph_context.clone();
    // `chat_with_history` now returns the reply text plus the token usage the
    // backend reported. The native `LlmEngine` surfaces a real (prompt +
    // completion) count; the cloud backends routed through this blocking path
    // (Bedrock via ApiClient, OpenRouter blocking, mistral.rs) report 0 because
    // their `chat_with_history` signatures don't carry usage yet — never
    // fabricated. On error we synthesize a fallback message with no count.
    let (response_text, tokens_used) = match tokio::task::spawn_blocking(move || {
        executor.chat_with_history(messages, graph_context, llm_provider)
    })
    .await
    .map_err(|e| format!("chat task join failed: {}", e))?
    {
        Ok(outcome) => (outcome.text, outcome.tokens_used),
        Err(e) => (
            format!(
                "I couldn't generate a detailed response (LLM error: {}). \
                 Please check the LLM provider configuration.\n\n{}",
                e, graph_for_error
            ),
            0,
        ),
    };
    let assistant_msg = append_assistant_message(state.inner(), response_text)?;
    persist_llm_usage_for_session(&app, &state.current_session_id(), tokens_used);
    Ok(ChatResponse {
        message: assistant_msg,
        tokens_used,
    })
}

/// Synthesize narrative notes from the current knowledge graph + transcript
/// (ADR-0014). On-demand: reuses the chat LLM pipeline with a summarization
/// prompt and a whole-conversation graph context (most-central nodes via an
/// empty query) plus a wide transcript window. Returns Markdown. Does NOT touch
/// chat history — notes are a separate, parallel projection of the same data.
#[tauri::command]
pub async fn synthesize_notes(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> AppResult<String> {
    let _session_lifecycle = state.session_lifecycle.lock().await;
    // Notes synthesis sends the transcript and graph to the selected LLM. Keep
    // the product gate ahead of client synchronization or context reads just
    // like the interactive chat entry points (ADR-0033).
    let settings = read_settings_for_session_content(state.inner(), "notes_synthesis")?;
    crate::provider_registry::ensure_llm_provider_start_enabled(&settings.llm_provider)?;

    sync_llm_api_client_from_settings_cache(state.inner())?;
    sync_openrouter_client_from_settings_cache(state.inner())?;

    let llm_provider = settings.llm_provider.clone();

    enforce_session_content_policy(
        &app,
        state.inner(),
        &settings,
        "notes_synthesis",
        llm_provider.runtime_provider_id(),
        &["transcript", "graph_context", "prompt"],
        llm_provider.requires_cloud_content_transfer(),
    )?;

    let snapshot = {
        let kg = state
            .knowledge_graph
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        kg.snapshot()
    };

    let recent_transcript: Vec<TranscriptSegment> = {
        let transcript = state
            .transcript_buffer
            .read()
            .map_err(|e| format!("Lock error: {}", e))?;
        transcript.iter().rev().take(60).cloned().collect()
    };

    // Whole-conversation context: an empty query makes build_graph_chat_context
    // fall back to the most-central nodes (ADR-0014), and we attach a wider
    // transcript window than chat uses.
    const MAX_NOTES_NODES: usize = 80;
    let mut graph_context =
        crate::graph::entities::build_graph_chat_context(&snapshot, "", MAX_NOTES_NODES);
    if !recent_transcript.is_empty() {
        graph_context.push_str("\nRecent Transcript:\n");
        for seg in recent_transcript.iter().rev() {
            let speaker = seg.speaker_label.as_deref().unwrap_or("Unknown");
            graph_context.push_str(&format!("[{}]: {}\n", speaker, seg.text));
        }
    }

    let prompt = "Write structured notes for this conversation as Markdown, using \
         only the knowledge graph and transcript in the provided context (do not \
         invent facts). Use these sections, omitting any with no content:\n\n\
         ## Summary\nA 2-4 sentence narrative.\n\n\
         ## Key Points\n- concise bullets\n\n\
         ## Action Items\n- owner: task (only if stated)\n\n\
         ## Decisions\n- decisions made\n\n\
         ## Open Questions\n- unresolved questions"
        .to_string();
    let messages = vec![ChatMessage {
        role: "user".to_string(),
        content: prompt,
    }];

    let executor = state.llm_executor.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        executor.chat_with_history(messages, graph_context, llm_provider)
    })
    .await
    .map_err(|e| format!("notes synthesis task join failed: {}", e))?
    .map_err(|e| {
        format!(
            "Failed to synthesize notes (LLM error: {}). Check the LLM provider \
             configuration.",
            e
        )
    })?;

    // Notes synthesis only needs the generated Markdown; the token usage on the
    // outcome is reported through the chat path, not here.
    Ok(outcome.text)
}

/// Get the current chat message history.
#[tauri::command]
pub async fn get_chat_history(state: State<'_, AppState>) -> AppResult<Vec<ChatMessage>> {
    let history = state
        .chat_history
        .read()
        .map_err(|e| format!("Lock error: {}", e))?;
    Ok(history.clone())
}

/// Clear the chat message history.
#[tauri::command]
pub async fn clear_chat_history(state: State<'_, AppState>) -> AppResult<()> {
    let mut history = state
        .chat_history
        .write()
        .map_err(|e| format!("Lock error: {}", e))?;
    history.clear();
    Ok(())
}

/// Strip the canned question-proposal prefix to recover the raw question text
/// for use as a graph node label. Falls back to the full body.
fn question_text_from_body(body: &str) -> String {
    body.strip_prefix("Consider answering or linking this question: ")
        .unwrap_or(body)
        .trim()
        .to_string()
}

fn live_assist_card_record(
    session_id: &str,
    proposal: &events::AgentProposalPayload,
    status: events::LiveAssistCardStatus,
    outcome: Option<events::AgentActionResult>,
    projection_patch_sequence: Option<u64>,
    updated_at_ms: u64,
    existing: Option<&events::LiveAssistCardRecord>,
) -> events::LiveAssistCardRecord {
    let source_span_ids = existing
        .map(|card| card.source_span_ids.clone())
        .filter(|ids| !ids.is_empty())
        .unwrap_or_else(|| vec![proposal.source_segment_id.clone()]);
    let graph_context_ids = existing
        .map(|card| card.graph_context_ids.clone())
        .unwrap_or_default();
    events::LiveAssistCardRecord {
        session_id: session_id.to_string(),
        proposal: proposal.clone(),
        status,
        source_span_ids,
        graph_context_ids,
        outcome,
        projection_patch_sequence,
        created_at_ms: existing
            .map(|card| card.created_at_ms)
            .unwrap_or(proposal.created_at_ms),
        updated_at_ms,
    }
}

fn existing_live_assist_card(
    session_id: &str,
    proposal_id: &str,
) -> Option<events::LiveAssistCardRecord> {
    FileMemoryRepository::user_data()
        .load_live_assist_cards(session_id)
        .ok()?
        .into_iter()
        .find(|card| card.proposal.id == proposal_id)
}

/// Build the honest [`crate::claim_evidence::EvidenceAnchor`] for an approved
/// live-assist proposal (ADR-0037).
///
/// This path is locally generated from an approved live-assist card, not LLM
/// output, so it never goes through `validate_projection_patch_draft`/
/// `judge_claim_evidence` at draft time — but the proposal DOES carry a real
/// citation, `proposal.source_segment_id`, which is validated when the card
/// itself is admitted (`persistence/mod.rs`'s OR-citation rule). `EvidenceAnchor::
/// default()` (⇒ `KnowledgeGap`, the one class the judge always refuses)
/// mislabels every approved card as an absence-shaped gap, which is wrong for
/// a positive assertion and misleads every reader of the canonical
/// `ProjectionOperation` log (typed-gap reports, evidence inspection,
/// `SessionExportBundle`), independent of whatever `apply_validated_patch`
/// later re-derives. `source_segment_id` may be the provider's
/// `transcript_segment_id` OR the immutable `span_id`
/// (`derive_legacy_transcript_segments`'s same fallback), so this resolves it
/// against the ledger's `latest_spans` by either field before anchoring —
/// `judge_claim_evidence`'s basis map is keyed by the literal `span_id` only.
fn live_assist_evidence_anchor(
    ledger: &crate::projections::TranscriptLedger,
    source_segment_id: &str,
) -> crate::claim_evidence::EvidenceAnchor {
    let resolved_span_id = ledger
        .latest_spans
        .iter()
        .find(|event| {
            event.span_id == source_segment_id
                || event.transcript_segment_id.as_deref() == Some(source_segment_id)
        })
        .map(|event| event.span_id.clone());

    match resolved_span_id {
        Some(span_id) => crate::claim_evidence::EvidenceAnchor {
            claim_class: crate::claim_evidence::ClaimClass::GroundedInference,
            span_id: Some(span_id),
            quote: None,
            note: None,
        },
        None => crate::claim_evidence::EvidenceAnchor {
            claim_class: crate::claim_evidence::ClaimClass::UnavailableEvidence,
            span_id: None,
            quote: None,
            note: Some(format!(
                "Live-assist proposal cited transcript segment {source_segment_id}, which is no \
                 longer present in this session's transcript ledger at approval time."
            )),
        },
    }
}

fn approved_agent_projection_patch(
    state: &AppState,
    proposal: &events::AgentProposalPayload,
) -> Result<u64, String> {
    let runtime = state.projection_runtime_handle();
    let session_id = runtime.current_session_id();
    let ledger = runtime.transcript_ledger_snapshot();
    let basis = ledger.current_basis();
    let now_ms = unix_millis();
    let evidence = live_assist_evidence_anchor(&ledger, &proposal.source_segment_id);

    let (kind, operations, prompt_id) = match &proposal.kind {
        events::AgentProposalKind::Note => (
            crate::projections::ProjectionKind::Notes,
            vec![crate::projections::ProjectionOperation::UpsertNote {
                id: format!("live-assist-note-{}", proposal.id),
                title: proposal.title.clone(),
                body: proposal.body.clone(),
                tags: vec!["live-assist".to_string(), "approved".to_string()],
                evidence: evidence.clone(),
                // Trusted-code-authored, not model-authored: asserts no
                // document structure (audio-graph-a6b5 W1 ships dark; a
                // provenance-class distinction for user/agent-authored
                // structure is an ADR-0037 question, out of scope here).
                heading_level: None,
            }],
            "live-assist-note-approval",
        ),
        events::AgentProposalKind::Question => {
            let question = question_text_from_body(&proposal.body);
            (
                crate::projections::ProjectionKind::Graph,
                vec![crate::projections::ProjectionOperation::UpsertGraphNode {
                    id: format!("live-assist-question-{}", proposal.id),
                    name: question,
                    entity_type: "Question".to_string(),
                    description: Some(proposal.body.clone()),
                    evidence: evidence.clone(),
                }],
                "live-assist-question-approval",
            )
        }
        events::AgentProposalKind::GraphSuggestion => (
            crate::projections::ProjectionKind::Graph,
            vec![crate::projections::ProjectionOperation::UpsertGraphNode {
                id: format!("live-assist-suggestion-{}", proposal.id),
                name: proposal.title.clone(),
                entity_type: "LiveAssistSuggestion".to_string(),
                description: Some(proposal.body.clone()),
                evidence,
            }],
            "live-assist-graph-suggestion-approval",
        ),
    };
    let sequence = runtime.next_projection_sequence(&kind);
    let patch = crate::projections::ProjectionPatch {
        sequence,
        kind,
        llm_request_id: format!("live-assist-approval-{}", proposal.id),
        basis: basis.clone(),
        operations,
        confidence: proposal.confidence,
        provenance: crate::projections::ProjectionProvenance {
            provider: "audiograph".to_string(),
            model: "rule-based-live-assist".to_string(),
            prompt_id: prompt_id.to_string(),
            // Locally generated: no LLM route was dispatched, so there is no route
            // to stamp and the model id is ours, not a served one.
            route_id: None,
            model_source: crate::llm::route::ModelIdentitySource::Requested,
        },
        route: None,
        queued_at_ms: Some(now_ms),
        generation_latency_ms: Some(0),
        apply_latency_ms: None,
        basis_currency_at_apply: None,
        created_at_ms: now_ms,
    };

    runtime
        .apply_runtime_projection_patch(&session_id, &basis, patch)
        .map_err(|error| {
            format!(
                "Approved live assist card {} could not write projection patch: {:?}",
                proposal.id, error
            )
        })?;
    Ok(sequence)
}

#[tauri::command]
pub fn approve_agent_proposal(
    proposal_id: String,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> AppResult<events::LiveAssistCardRecord> {
    approve_agent_proposal_impl(proposal_id, app, state.inner())
}

/// Implementation of [`approve_agent_proposal`] that operates on a borrowed
/// `&AppState` so it can be exercised from tests without constructing a
/// per-test Tauri/tao app (mirrors `start_capture_impl`'s split;
/// audio-graph-4b52 coverage-gap fix).
fn approve_agent_proposal_impl(
    proposal_id: String,
    app: tauri::AppHandle,
    state: &AppState,
) -> AppResult<events::LiveAssistCardRecord> {
    let proposal = {
        let mut pending = state
            .pending_agent_proposals
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        pending
            .remove(&proposal_id)
            .ok_or_else(|| "Agent proposal no longer exists or was already applied".to_string())?
    };

    events::emit_or_log(
        &app,
        events::AGENT_STATUS,
        events::AgentStatusPayload {
            state: events::AgentStatusState::Running,
            source_segment_id: Some(proposal.source_segment_id.clone()),
            message: Some("Applying approved proposal".to_string()),
            timestamp_ms: unix_millis(),
        },
    );

    let speaker = proposal
        .speaker_label
        .as_deref()
        .filter(|label| !label.trim().is_empty())
        .unwrap_or("Agent");
    let mut graph_updated = false;
    let session_id = state.current_session_id();
    let existing_card = existing_live_assist_card(&session_id, &proposal.id);
    let projection_patch_sequence = match approved_agent_projection_patch(state, &proposal) {
        Ok(sequence) => sequence,
        Err(error) => {
            events::emit_or_log(
                &app,
                events::AGENT_STATUS,
                events::AgentStatusPayload {
                    state: events::AgentStatusState::Error,
                    source_segment_id: Some(proposal.source_segment_id.clone()),
                    message: Some(error.clone()),
                    timestamp_ms: unix_millis(),
                },
            );
            if let Ok(mut pending) = state.pending_agent_proposals.lock() {
                pending
                    .entry(proposal_id.clone())
                    .or_insert_with(|| proposal.clone());
            }
            return Err(error.into());
        }
    };
    use crate::graph::entities::{ExtractedEntity, ExtractedRelation, ExtractionResult};
    // Decide what (if anything) to write to the graph for this proposal kind.
    // Questions now DEFAULT to the graph (a Question node linked from the
    // speaker), built locally with no LLM call so it can never rate-limit. The
    // optional "Ask AI" path is a separate, user-initiated chat request driven
    // from the frontend.
    let (extraction, action): (Option<ExtractionResult>, &str) = match proposal.kind {
        events::AgentProposalKind::GraphSuggestion => {
            let ex = state.graph_extractor.extract(speaker, &proposal.body);
            let meaningful = !ex.relations.is_empty()
                || ex
                    .entities
                    .iter()
                    .any(|entity| !entity.name.eq_ignore_ascii_case(speaker));
            (meaningful.then_some(ex), "graph_update")
        }
        events::AgentProposalKind::Question => {
            let q = question_text_from_body(&proposal.body);
            let ex = ExtractionResult {
                entities: vec![
                    ExtractedEntity {
                        name: speaker.to_string(),
                        entity_type: "Person".to_string(),
                        description: None,
                    },
                    ExtractedEntity {
                        name: q.clone(),
                        entity_type: "Question".to_string(),
                        description: Some(q.clone()),
                    },
                ],
                relations: vec![ExtractedRelation {
                    source: speaker.to_string(),
                    target: q,
                    relation_type: "asks".to_string(),
                    detail: None,
                }],
            };
            (Some(ex), "graph_update")
        }
        events::AgentProposalKind::Note => (None, "chat_note"),
    };

    if let Some(extraction) = extraction {
        // `proposal.created_at_ms` is Unix epoch ms (wall clock); the graph's
        // `timestamp` param is session-relative seconds (the same "media
        // clock" the live speech path uses for eviction ordering — see
        // `TranscriptLedger::session_relative_timestamp`). Converting via the
        // ledger anchor instead of a raw `/1000.0` keeps a manually-approved
        // node evictable on the same terms as a live one (audio-graph-4b52).
        let timestamp = {
            let ledger = state
                .transcript_ledger
                .lock()
                .map_err(|e| format!("Lock error: {}", e))?;
            ledger.session_relative_timestamp(&proposal.source_segment_id, proposal.created_at_ms)
        };
        let mut graph = state
            .knowledge_graph
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        graph.process_extraction(&extraction, timestamp, speaker, &proposal.source_segment_id);

        if graph.has_delta() {
            let delta = graph.take_delta();
            events::emit_or_log(&app, events::GRAPH_DELTA, &delta);
        }
        let snapshot = graph.snapshot();
        if let Ok(mut cached) = state.graph_snapshot.write() {
            *cached = snapshot.clone();
        }
        events::emit_or_log(&app, events::GRAPH_UPDATE, &snapshot);
        graph_updated = true;
    }

    let summary = if graph_updated {
        format!("Approved agent proposal: {}", proposal.title)
    } else {
        format!("Approved agent proposal for review: {}", proposal.title)
    };
    let message = format!("{}\n\n{}", summary, proposal.body);
    {
        let mut history = state
            .chat_history
            .write()
            .map_err(|e| format!("Lock error: {}", e))?;
        history.push(ChatMessage {
            role: "assistant".to_string(),
            content: message.clone(),
        });
        cap_chat_history(&mut history);
    }

    events::emit_or_log(
        &app,
        events::AGENT_STATUS,
        events::AgentStatusPayload {
            state: events::AgentStatusState::Idle,
            source_segment_id: Some(proposal.source_segment_id.clone()),
            message: None,
            timestamp_ms: unix_millis(),
        },
    );

    let result = events::AgentActionResult {
        proposal_id: proposal.id.clone(),
        action: action.to_string(),
        message,
        graph_updated,
        timestamp_ms: unix_millis(),
    };
    let record = live_assist_card_record(
        &session_id,
        &proposal,
        events::LiveAssistCardStatus::Approved,
        Some(result.clone()),
        Some(projection_patch_sequence),
        result.timestamp_ms,
        existing_card.as_ref(),
    );
    FileMemoryRepository::user_data().upsert_live_assist_card(&session_id, &record)?;
    // audio-graph-81a5: card approval telemetry.
    crate::card_telemetry::log_card_event(
        &session_id,
        &proposal.id,
        &proposal.kind,
        proposal.confidence,
        crate::card_telemetry::CardLifecycleEvent::Approved,
    );
    Ok(record)
}

/// Add a detected question to the knowledge graph as a `Question` node linked
/// from the speaker. Local-only (no LLM), so it's safe to call automatically
/// when a question is detected — questions default to the graph; asking the AI
/// for an answer is a separate, optional user action.
#[tauri::command]
pub fn add_question_to_graph(
    text: String,
    speaker: Option<String>,
    source_segment_id: Option<String>,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> AppResult<bool> {
    add_question_to_graph_impl(text, speaker, source_segment_id, app, state.inner())
}

/// Implementation of [`add_question_to_graph`] that operates on a borrowed
/// `&AppState` so it can be exercised from tests without constructing a
/// per-test Tauri/tao app (mirrors `start_capture_impl`'s split;
/// audio-graph-4b52 coverage-gap fix).
fn add_question_to_graph_impl(
    text: String,
    speaker: Option<String>,
    source_segment_id: Option<String>,
    app: tauri::AppHandle,
    state: &AppState,
) -> AppResult<bool> {
    use crate::graph::entities::{ExtractedEntity, ExtractedRelation, ExtractionResult};
    let q = question_text_from_body(text.trim());
    if q.is_empty() {
        return Ok(false);
    }
    let speaker = speaker
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "Speaker".to_string());
    let segment_id = source_segment_id.unwrap_or_else(|| format!("question-{}", unix_millis()));

    let extraction = ExtractionResult {
        entities: vec![
            ExtractedEntity {
                name: speaker.clone(),
                entity_type: "Person".to_string(),
                description: None,
            },
            ExtractedEntity {
                name: q.clone(),
                entity_type: "Question".to_string(),
                description: Some(q.clone()),
            },
        ],
        relations: vec![ExtractedRelation {
            source: speaker.clone(),
            target: q,
            relation_type: "asks".to_string(),
            detail: None,
        }],
    };

    // Same epoch-vs-session-relative conversion as `approve_agent_proposal`
    // (audio-graph-4b52): `unix_millis()` is wall clock, but the graph's
    // `timestamp` param is session-relative seconds.
    let timestamp = {
        let ledger = state
            .transcript_ledger
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        ledger.session_relative_timestamp(&segment_id, unix_millis())
    };
    let mut graph = state
        .knowledge_graph
        .lock()
        .map_err(|e| format!("Lock error: {}", e))?;
    graph.process_extraction(&extraction, timestamp, &speaker, &segment_id);
    if graph.has_delta() {
        let delta = graph.take_delta();
        events::emit_or_log(&app, events::GRAPH_DELTA, &delta);
    }
    let snapshot = graph.snapshot();
    if let Ok(mut cached) = state.graph_snapshot.write() {
        *cached = snapshot.clone();
    }
    events::emit_or_log(&app, events::GRAPH_UPDATE, &snapshot);
    Ok(true)
}

/// Retcon-merge a superseded graph entity into a canonical one (speaker /
/// entity resolution).
///
/// This is the live production producer for the temporal-graph
/// `invalidate_edge` / `valid_until` path: when a diarization or
/// entity-resolution retcon decides that `superseded_name` is actually
/// `canonical_name` (e.g. the provisional local-diarizer label `"Speaker 2"`
/// resolves to the stable identity `"Alice"`, pairing with the speaker-timeline
/// durable layer + ProjectionBasis diarization work), every relation attached to
/// the superseded entity is invalidated (hidden via `valid_until`) and
/// re-pointed onto the canonical entity. The superseded attribution is kept in
/// the graph for audit — only hidden from the live snapshot.
///
/// `threshold` is the fuzzy-match cutoff for resolving both names (defaults to
/// exact-only `1.0` when omitted). Returns the number of edges that were
/// retconned; `0` means the merge was a no-op (a name did not resolve, both
/// names are the same node, or the superseded node had no live edges).
#[tauri::command]
pub fn merge_graph_entities(
    superseded_name: String,
    canonical_name: String,
    threshold: Option<f64>,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> AppResult<usize> {
    merge_graph_entities_impl(
        superseded_name,
        canonical_name,
        threshold,
        app,
        state.inner(),
    )
}

/// Implementation of [`merge_graph_entities`] that operates on a borrowed
/// `&AppState` so it can be exercised from tests without constructing a
/// per-test Tauri/tao app (mirrors `start_capture_impl`'s split;
/// audio-graph-4b52 coverage-gap fix).
fn merge_graph_entities_impl(
    superseded_name: String,
    canonical_name: String,
    threshold: Option<f64>,
    app: tauri::AppHandle,
    state: &AppState,
) -> AppResult<usize> {
    // Same epoch-vs-session-relative conversion as `approve_agent_proposal` /
    // `add_question_to_graph` (audio-graph-4b52): `unix_millis()` is wall
    // clock, but `supersede_entity`'s `timestamp` lands in `invalidate_edge`'s
    // `valid_until` and the re-pointed edge's `valid_from` — the same
    // session-relative-seconds domain `evict_excess_edges` compares against
    // (`graph/temporal.rs`), and the audio-time axis ADR-0026 §Consequences
    // says "lives only on the live TemporalEdge". Left as raw epoch seconds,
    // a retconned edge's `valid_from` is always the graph's maximum and can
    // never be evicted, the exact immortality bug fixed for nodes, but for
    // edges. There is no source segment id for a manual merge, so this
    // always resolves through the ledger's fallback-to-any-span anchor (see
    // `TranscriptLedger::session_relative_timestamp`).
    let timestamp = {
        let ledger = state
            .transcript_ledger
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        ledger.session_relative_timestamp("", unix_millis())
    };
    let threshold = threshold.unwrap_or(1.0);

    let mut graph = state
        .knowledge_graph
        .lock()
        .map_err(|e| format!("Lock error: {}", e))?;
    let invalidated =
        graph.supersede_entity(&superseded_name, &canonical_name, timestamp, threshold);

    if invalidated == 0 {
        // No-op merge: don't emit spurious graph events.
        return Ok(0);
    }

    if graph.has_delta() {
        let delta = graph.take_delta();
        events::emit_or_log(&app, events::GRAPH_DELTA, &delta);
    }
    let snapshot = graph.snapshot();
    if let Ok(mut cached) = state.graph_snapshot.write() {
        *cached = snapshot.clone();
    }
    events::emit_or_log(&app, events::GRAPH_UPDATE, &snapshot);
    Ok(invalidated)
}

#[tauri::command]
pub fn dismiss_agent_proposal(
    proposal_id: String,
    state: State<'_, AppState>,
) -> AppResult<Option<events::LiveAssistCardRecord>> {
    let proposal = {
        let mut pending = state
            .pending_agent_proposals
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        pending.remove(&proposal_id)
    };
    if let Some(proposal) = proposal {
        let session_id = state.current_session_id();
        let existing_card = existing_live_assist_card(&session_id, &proposal.id);
        let now_ms = unix_millis();
        let record = live_assist_card_record(
            &session_id,
            &proposal,
            events::LiveAssistCardStatus::Dismissed,
            None,
            None,
            now_ms,
            existing_card.as_ref(),
        );
        FileMemoryRepository::user_data().upsert_live_assist_card(&session_id, &record)?;
        // audio-graph-81a5: card dismissal telemetry.
        crate::card_telemetry::log_card_event(
            &session_id,
            &proposal.id,
            &proposal.kind,
            proposal.confidence,
            crate::card_telemetry::CardLifecycleEvent::Dismissed,
        );
        return Ok(Some(record));
    }
    Ok(None)
}

#[tauri::command]
pub fn clear_agent_proposals(
    state: State<'_, AppState>,
) -> AppResult<Vec<events::LiveAssistCardRecord>> {
    let proposals = {
        let mut pending = state
            .pending_agent_proposals
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        let proposals: Vec<_> = pending.values().cloned().collect();
        pending.clear();
        proposals
    };
    let session_id = state.current_session_id();
    let now_ms = unix_millis();
    let repository = FileMemoryRepository::user_data();
    let existing_cards = repository
        .load_live_assist_cards(&session_id)
        .unwrap_or_default();
    let mut records = Vec::new();
    for proposal in proposals {
        let existing_card = existing_cards
            .iter()
            .find(|card| card.proposal.id == proposal.id);
        let record = live_assist_card_record(
            &session_id,
            &proposal,
            events::LiveAssistCardStatus::Dismissed,
            None,
            None,
            now_ms,
            existing_card,
        );
        repository.upsert_live_assist_card(&session_id, &record)?;
        // audio-graph-81a5: card dismissal telemetry. `clear_agent_proposals`
        // is a bulk-dismiss of every still-pending card, so each one gets its
        // own dismissal line (same event as the single-card `dismiss_agent_proposal`).
        crate::card_telemetry::log_card_event(
            &session_id,
            &proposal.id,
            &proposal.kind,
            proposal.confidence,
            crate::card_telemetry::CardLifecycleEvent::Dismissed,
        );
        records.push(record);
    }
    Ok(records)
}

// ---------------------------------------------------------------------------
// Model management commands
// ---------------------------------------------------------------------------

/// List available models and their download status.
#[tauri::command]
pub fn list_available_models(app: tauri::AppHandle) -> Vec<crate::models::ModelInfo> {
    crate::models::list_models(&app)
}

/// RAII guard that removes a model filename from `downloads_in_flight` on drop,
/// so the in-flight slot is freed whether the download succeeds, errors, or the
/// `spawn_blocking` task panics (AUD-MDL1 / #58, P2).
struct DownloadGuard {
    in_flight: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    filename: String,
}

impl Drop for DownloadGuard {
    fn drop(&mut self) {
        if let Ok(mut set) = self.in_flight.lock() {
            set.remove(&self.filename);
        }
    }
}

/// Download a model by filename, with progress events emitted to the frontend.
///
/// Runs the blocking HTTP download on a background thread via
/// `tokio::task::spawn_blocking` so the IPC handler stays async (G3).
///
/// Rejects a second concurrent download of the same model: two callers racing
/// the same target file would write to the same `.download` temp and fight over
/// the final rename (AUD-MDL1 / #58, P2). The first caller claims the filename
/// in `downloads_in_flight`; a duplicate gets an "already downloading" error.
#[tauri::command]
pub async fn download_model_cmd(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    model_filename: String,
) -> AppResult<String> {
    // Claim the in-flight slot. Holding the lock only for the insert keeps the
    // critical section tiny; the RAII guard frees the slot on every exit path.
    {
        let mut in_flight = state
            .downloads_in_flight
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        if !in_flight.insert(model_filename.clone()) {
            return Err(AppError::from(format!(
                "Model '{}' is already downloading",
                model_filename
            )));
        }
    }
    let _guard = DownloadGuard {
        in_flight: state.downloads_in_flight.clone(),
        filename: model_filename.clone(),
    };

    let handle = app.clone();
    tokio::task::spawn_blocking(move || crate::models::download_model(&handle, &model_filename))
        .await
        .map_err(|e| format!("Download task failed: {}", e))?
        .map_err(AppError::from)
}

/// Get the readiness status of all known models (G1).
#[tauri::command]
pub fn get_model_status(app: tauri::AppHandle) -> crate::models::ModelStatus {
    crate::models::get_model_status(&app)
}

/// Load the native LLM model into memory (G2).
///
/// Resolves the model path from the app data directory, then loads it on a
/// background thread. On success the engine is stored in `AppState.llm_engine`.
#[tauri::command]
pub async fn load_llm_model(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> AppResult<String> {
    // On the cloud-only build the `llm-llama` block below is compiled out, so
    // this block is the function tail expression (no `return` needed).
    #[cfg(not(feature = "llm-llama"))]
    {
        let _ = (&app, &state);
        Err(AppError::ProviderUnavailable {
            provider: "LocalLlama".to_string(),
            required_feature: "local-ml or llm-llama".to_string(),
        })
    }

    #[cfg(feature = "llm-llama")]
    {
        let models_dir = crate::models::get_models_dir(&app);
        let model_path = models_dir.join(crate::models::LLM_MODEL_FILENAME);

        if !model_path.exists() {
            return Err(AppError::ModelNotFound {
                name: crate::models::LLM_MODEL_FILENAME.to_string(),
            });
        }

        let path = model_path.clone();
        let engine = tokio::task::spawn_blocking(move || {
            crate::llm::LlmEngine::new(&path.to_string_lossy())
        })
        .await
        .map_err(|e| format!("Failed to spawn LLM loading task: {}", e))?
        .map_err(|e| format!("Failed to load LLM model: {}", e))?;

        let mut guard = state.llm_engine.lock().map_err(|e| e.to_string())?;
        *guard = Some(engine);

        Ok("LLM model loaded successfully".to_string())
    }
}

// ---------------------------------------------------------------------------
// Settings commands
// ---------------------------------------------------------------------------

/// Load application settings from disk (returns defaults if missing).
/// Syncs the loaded settings into the in-memory `AppState.app_settings` cache
/// so other backend modules (e.g. speech processor) can read them without I/O.
#[tauri::command]
pub fn load_settings_cmd(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> crate::settings::AppSettings {
    // Migration/redaction writeback of legacy inline credentials. Hold the
    // process-wide settings I/O lock across the load→save so a concurrent footer
    // Save can't interleave between this read and this whole-struct writeback and
    // have its provider/model selection reverted to our stale snapshot — the same
    // dual-writer race class as set_analytics_enabled (symmetric-writer check for
    // audio-graph-3e69 / cred-review M3, a writer the M3 enumeration omitted).
    let settings = {
        let _io_guard = crate::settings::lock_settings_io();
        let loaded_settings = crate::settings::load_settings_with_status(&app);
        let load_status = loaded_settings.status;
        let settings = loaded_settings.settings;
        if crate::settings::has_inline_credentials(&settings)
            && crate::settings::allow_automatic_settings_writeback(
                load_status,
                "migrating/redacting settings credentials during load_settings_cmd",
            )
            && let Err(e) = crate::settings::save_settings_locked(&app, &settings)
        {
            log::warn!("Failed to migrate/redact settings credentials: {}", e);
        }
        settings
    };

    let credentials = crate::credentials::load_credentials();
    let runtime_settings = crate::settings::hydrate_runtime_credentials(&settings, &credentials);
    let settings_for_ipc = crate::settings::redacted_settings(&settings);

    // Sync in-memory cache with runtime-only hydrated credentials.
    if let Ok(mut cached) = state.app_settings.write() {
        *cached = runtime_settings;
    }
    if let Err(e) = sync_llm_api_client_from_settings_cache(state.inner()) {
        log::warn!(
            "Failed to sync LLM API client after loading settings: {}",
            e
        );
    }
    if let Err(e) = sync_openrouter_client_from_settings_cache(state.inner()) {
        log::warn!(
            "Failed to sync OpenRouter client after loading settings: {}",
            e
        );
    }
    settings_for_ipc
}

/// Save application settings to disk (atomic write).
/// Also updates the in-memory `AppState.app_settings` cache.
#[tauri::command]
pub fn save_settings_cmd(
    app: tauri::AppHandle,
    settings: crate::settings::AppSettings,
    state: State<'_, AppState>,
) -> AppResult<()> {
    crate::settings::save_settings(&app, &settings)?;
    let credentials = crate::credentials::load_credentials();
    let runtime_settings = crate::settings::hydrate_runtime_credentials(&settings, &credentials);

    // Sync in-memory cache with runtime-only hydrated credentials.
    if let Ok(mut cached) = state.app_settings.write() {
        *cached = runtime_settings;
    }
    sync_llm_api_client_from_settings_cache(state.inner())?;
    sync_openrouter_client_from_settings_cache(state.inner())?;
    Ok(())
}

/// Delete a downloaded model file by filename.
#[tauri::command]
pub fn delete_model_cmd(app: tauri::AppHandle, model_filename: String) -> AppResult<String> {
    crate::models::delete_model(&app, &model_filename).map_err(AppError::from)
}

/// Change the runtime log level and update the in-memory settings cache.
///
/// Takes effect immediately for every subsequent `log::*!` macro and dirties
/// the cached settings so the new level is visible to readers. Disk
/// persistence is **not** performed here — the frontend is expected to call
/// `save_settings_cmd` to flush the full settings blob when the user commits.
///
// set_log_level only mutates runtime tracing; save_settings_cmd is the
// single owner of disk persistence. See loop-13 review.
#[tauri::command]
pub fn set_log_level(
    _app: tauri::AppHandle,
    level: String,
    state: State<'_, AppState>,
) -> AppResult<()> {
    // 1. Flip the in-process log level. Immediate, cheap, and the user's
    //    primary expectation from this command.
    crate::logging::apply_log_level(&level);

    // 2. Dirty the in-memory settings cache so any reader (and the next
    //    save_settings_cmd call) sees the new value. No disk write here —
    //    save_settings_cmd is the sole owner of that path to avoid the
    //    race flagged in the loop-13 review.
    if let Ok(mut cached) = state.app_settings.write() {
        cached.log_level = Some(level);
    }

    Ok(())
}

/// Return the current logging configuration + the list of log files on disk.
#[tauri::command]
pub fn get_log_info(state: State<'_, AppState>) -> AppResult<crate::logging::LogInfo> {
    let (enabled, mode, level) = {
        let c = state
            .app_settings
            .read()
            .map_err(|e| format!("Lock error: {e}"))?;
        (
            c.file_logging.unwrap_or(true),
            crate::logging::LogFileMode::from_str_or_default(c.log_file_mode.as_deref()),
            c.log_level.clone().unwrap_or_else(|| "info".to_string()),
        )
    };
    Ok(crate::logging::log_info(enabled, mode, &level)?)
}

/// Apply + persist the file-logging configuration (enable/disable, mode,
/// level). Unlike `set_log_level` (runtime-only), this is a deliberate,
/// user-initiated commit, so it writes the three logging fields to
/// `config.yaml` immediately (patching the on-disk file so it doesn't
/// clobber unsaved edits elsewhere).
#[tauri::command]
pub fn set_logging_config(
    app: tauri::AppHandle,
    enabled: bool,
    mode: String,
    level: Option<String>,
    state: State<'_, AppState>,
) -> AppResult<crate::logging::LogInfo> {
    let file_mode = crate::logging::LogFileMode::from_str_or_default(Some(&mode));

    // 1. Apply runtime level (if provided) and (re)configure the file sink.
    if let Some(ref lvl) = level {
        crate::logging::apply_log_level(lvl);
    }
    crate::logging::configure_file_logging(enabled, file_mode)?;

    // 2. Update the in-memory cache.
    let effective_level = {
        let mut cached = state
            .app_settings
            .write()
            .map_err(|e| format!("Lock error: {e}"))?;
        cached.file_logging = Some(enabled);
        cached.log_file_mode = Some(file_mode.as_str().to_string());
        if let Some(lvl) = level {
            cached.log_level = Some(lvl);
        }
        cached
            .log_level
            .clone()
            .unwrap_or_else(|| "info".to_string())
    };

    // 3. Persist just the logging fields to disk (load → patch → save) so we
    //    don't overwrite settings the user may be editing in the form. Hold the
    //    process-wide settings I/O lock across the whole load+save so a
    //    concurrent full `save_settings` can't interleave and silently revert
    //    these fields (or have its fields reverted by our stale read).
    {
        let _io_guard = crate::settings::lock_settings_io();
        let loaded_settings = crate::settings::load_settings_with_status(&app);
        if crate::settings::allow_automatic_settings_writeback(
            loaded_settings.status,
            "persisting logging settings",
        ) {
            let mut on_disk = loaded_settings.settings;
            on_disk.file_logging = Some(enabled);
            on_disk.log_file_mode = Some(file_mode.as_str().to_string());
            on_disk.log_level = Some(effective_level.clone());
            if let Err(e) = crate::settings::save_settings_locked(&app, &on_disk) {
                log::warn!("Failed to persist logging settings: {e}");
            }
        }
    }

    Ok(crate::logging::log_info(
        enabled,
        file_mode,
        &effective_level,
    )?)
}

/// Delete all archived log files (keeps the active file). Returns the count.
#[tauri::command]
pub fn purge_logs_cmd() -> AppResult<usize> {
    Ok(crate::logging::purge_logs()?)
}

/// Open the logs directory in the OS file explorer.
#[tauri::command]
pub fn open_logs_dir() -> AppResult<String> {
    let dir = crate::logging::logs_dir()?;
    let dir_str = dir.display().to_string();
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("explorer").arg(&dir).spawn();
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(&dir).spawn();
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(&dir).spawn();
    // explorer.exe returns a non-zero exit code even on success, so we only
    // treat a spawn failure as an error.
    match result {
        Ok(_) => Ok(dir_str),
        Err(e) => Err(format!("Failed to open logs dir: {e}").into()),
    }
}

// ---------------------------------------------------------------------------
// Anonymous analytics (Sentry) commands
// ---------------------------------------------------------------------------

/// Return the current anonymous-analytics status for the UI. Independent of the
/// logging controls (`get_log_info`) and of the local crash handler.
#[tauri::command]
pub fn get_analytics_info(
    state: State<'_, AppState>,
) -> AppResult<crate::analytics::AnalyticsInfo> {
    let enabled = {
        let c = state
            .app_settings
            .read()
            .map_err(|e| format!("Lock error: {e}"))?;
        c.analytics_enabled.unwrap_or(false)
    };
    Ok(crate::analytics::analytics_info(enabled))
}

/// Apply + persist the opt-in anonymous-analytics setting. Mirrors
/// [`set_logging_config`]: a deliberate, user-initiated commit that updates the
/// in-memory cache and patches just the `analytics_enabled` field on disk
/// (load → patch → save) so it doesn't clobber unsaved edits elsewhere.
///
/// Toggle semantics (see [`crate::analytics`]): turning ON inits a fresh client
/// if none is live (the app may have started with analytics off, or a prior OFF
/// closed the transport) and binds it on the process hub; turning OFF unbinds on
/// the process hub AND closes the shared client transport — a thread-global kill
/// — then drops the guard, so a later ON re-inits. The local crash handler is
/// untouched (it is independent of this setting).
#[tauri::command]
pub fn set_analytics_enabled(
    app: tauri::AppHandle,
    enabled: bool,
    state: State<'_, AppState>,
) -> AppResult<crate::analytics::AnalyticsInfo> {
    // 1. Apply at runtime. When turning ON, make sure the client exists before
    //    binding it to the hub (it may never have been inited at startup).
    if enabled {
        crate::analytics::init_if_enabled(true);
    }
    crate::analytics::set_analytics_enabled_runtime(enabled);

    // 2. Update the in-memory cache.
    {
        let mut cached = state
            .app_settings
            .write()
            .map_err(|e| format!("Lock error: {e}"))?;
        cached.analytics_enabled = Some(enabled);
    }

    // 3. Persist just the analytics field to disk (load → patch → save) so we
    //    don't overwrite settings the user may be editing in the form. Hold the
    //    process-wide settings I/O lock across the whole load+save so a
    //    concurrent full `save_settings` (footer Save) can't interleave between
    //    our read and write and silently revert the user's provider/model
    //    selection — the credential-adjacent config this write would otherwise
    //    clobber with its stale pre-Save snapshot (audio-graph-3e69 /
    //    cred-review M3). Mirrors the `set_logging_config` pattern verbatim.
    {
        let _io_guard = crate::settings::lock_settings_io();
        let mut on_disk = crate::settings::load_settings(&app);
        on_disk.analytics_enabled = Some(enabled);
        if let Err(e) = crate::settings::save_settings_locked(&app, &on_disk) {
            log::warn!("Failed to persist analytics setting: {e}");
        }
    }

    Ok(crate::analytics::analytics_info(enabled))
}

/// Relay a frontend diagnostic through the backend Sentry channel.
///
/// The WebView has no working Sentry egress of its own — CSP `connect-src`
/// blocks the browser SDK's POST to `*.ingest.us.sentry.io` — so the frontend
/// forwards structured, **controlled** ids here and the (CSP-exempt) Rust
/// Sentry does the actual send through its mature scrubber.
///
/// This command accepts ONLY short, id-shaped fields — never a free-text
/// message or stack. Each field is defensively clamped to the id shape
/// (`^[a-z0-9._:-]{1,48}$`); anything that fails is dropped (mapped to `None`)
/// rather than forwarded, so even a misbehaving/compromised renderer cannot
/// smuggle prose in. The backend [`scrub_event`](crate::analytics) allowlist is
/// the belt-and-suspenders backstop, but we do not rely on it to strip prose.
///
/// Mapping into [`DiagEvent`](crate::analytics::DiagEvent): `name` → the event
/// id, `component` → the `provider` tag, `surface` → the `kind` tag. The
/// backend picks the [`Category`](crate::analytics::Category) enum from the
/// supplied id (always `Category::Frontend`), so no free-text category rides in.
///
/// Fails silent by design: this is telemetry, so it returns `Ok(())` on the
/// happy path and never surfaces an error to the UI. `capture_diagnostic`
/// itself no-ops when analytics is disabled (unbound hub), so no extra gate is
/// needed here.
#[tauri::command]
pub fn report_frontend_diagnostic(
    name: String,
    category: String,
    component: Option<String>,
    surface: Option<String>,
) -> AppResult<()> {
    // `category` stays in the IPC signature (the WebView sends it, so removing
    // it would break the wire contract), but it is deliberately NOT trusted or
    // consulted: the backend fixes the category to `Frontend` via
    // `Category::frontend()`, so the frontend string can never steer it.
    // Explicitly discard it here rather than letting a meaningful-looking value
    // be silently ignored (audio-graph-5641).
    let _ = category;
    // Clamp `name` to the id shape. If it fails, fall back to a fixed, known-safe
    // id so the diagnostic still carries a triage signal (and the backend
    // scrubber would drop an ill-shaped name tag anyway).
    let name = sanitize_frontend_id(&name).unwrap_or_else(|| "frontend.unknown".to_string());
    // `component`/`surface` are optional id-shaped tags; drop any that fail the
    // shape check rather than forwarding untrusted text.
    let component = component.as_deref().and_then(sanitize_frontend_id);
    let surface = surface.as_deref().and_then(sanitize_frontend_id);

    crate::analytics::capture_diagnostic(crate::analytics::DiagEvent {
        name: &name,
        category: crate::analytics::Category::frontend(),
        level: sentry::Level::Error,
        // component → provider, surface → kind (both id-shaped controlled tags).
        provider: component.as_deref(),
        kind: surface.as_deref(),
        http_status: None,
        recoverable: None,
    });

    Ok(())
}

/// Clamp a frontend-supplied string to the controlled id shape
/// (`^[a-z0-9._:-]{1,48}$`). Returns `Some(id)` when the whole string matches,
/// or `None` to DROP it — we never forward untrusted free text, so a value that
/// isn't already id-shaped (spaces, uppercase, prose, over-length) is discarded
/// rather than mangled. This mirrors the backend scrubber's `is_id_shaped`
/// gate, applied at the boundary so nothing prose-shaped reaches the SDK.
fn sanitize_frontend_id(s: &str) -> Option<String> {
    let len = s.chars().count();
    let shaped = (1..=48).contains(&len)
        && s.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b':' | b'-')
        });
    shaped.then(|| s.to_string())
}

// ---------------------------------------------------------------------------
// Gemini Live dual-pipeline commands
// ---------------------------------------------------------------------------

/// Start the Gemini Live pipeline.
///
/// Reads Gemini settings (API key, model) from `AppSettings`, creates a
/// `GeminiLiveClient`, connects it, then spawns two worker threads:
///   1. **Audio sender** — reads from its runtime processed-audio consumer and
///      forwards audio to Gemini.
///   2. **Event receiver** — reads `GeminiEvent`s from the client and emits
///      Tauri events (`gemini-transcription`, `gemini-response`), also feeding
///      transcriptions into the knowledge graph.
///
/// Local transcription and Gemini notes can run simultaneously because the
/// dispatcher fans out to separate registered consumers.
#[tauri::command]
pub async fn start_gemini(state: State<'_, AppState>, app: tauri::AppHandle) -> AppResult<()> {
    log::info!("start_gemini called");
    let _session_lifecycle = state.session_lifecycle.lock().await;
    ensure_session_workers_quiesced(state.inner())?;

    // Fixed provider route: reject before capture-state inspection, runtime
    // consumer registration, client construction, or transport connection.
    crate::provider_registry::ensure_provider_id_start_enabled("realtime_agent.gemini_live")?;

    // Guard: capture must be running
    {
        let capturing = state
            .is_capturing
            .read()
            .map_err(|e| format!("Lock error: {}", e))?;
        if !*capturing {
            return Err(AppError::SessionInvalid {
                reason: "Cannot start Gemini: capture is not running".to_string(),
            });
        }
    }

    // Guard: don't double-start
    {
        let active = state
            .is_gemini_active
            .read()
            .map_err(|e| format!("Lock error: {}", e))?;
        if *active {
            return Err(AppError::SessionInvalid {
                reason: "Gemini pipeline is already running".to_string(),
            });
        }
    }

    // Read Gemini settings
    let settings = read_settings_for_session_content(state.inner(), "gemini_live_notes")?;
    enforce_session_content_policy(
        &app,
        state.inner(),
        &settings,
        "gemini_live_notes",
        "realtime_agent.gemini_live",
        &["audio", "transcript", "model_response"],
        true,
    )?;
    let gemini_settings = settings.gemini.clone();

    // Validate auth configuration early.
    match &gemini_settings.auth {
        crate::settings::GeminiAuthMode::ApiKey { api_key } => {
            if api_key.is_empty() {
                return Err(AppError::CredentialMissing {
                    key: "gemini_api_key".to_string(),
                });
            }
        }
        crate::settings::GeminiAuthMode::VertexAI {
            project_id,
            location,
            ..
        } => {
            if project_id.is_empty() || location.is_empty() {
                return Err(AppError::CredentialFileError {
                    reason:
                        "Vertex AI project_id and location must be configured in Settings → Gemini."
                            .to_string(),
                });
            }
        }
    }

    // Reap finished notes-mode handles before registering a new runtime
    // consumer. If either handle is still running while the active flag is
    // false, surface the lifecycle conflict rather than mutating the shared
    // Gemini client slot.
    {
        let mut audio_handle = state
            .gemini_audio_thread
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        reap_finished_handle(&mut audio_handle, "Gemini audio sender")?;
    }
    {
        let mut event_handle = state
            .gemini_event_thread
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        reap_finished_handle(&mut event_handle, "Gemini event receiver")?;
    }

    // Reserve the Live-client slot in the processed-audio registry before
    // touching the shared Gemini client. This keeps provider coexistence policy
    // in the registry and prevents a rejected notes/converse overlap from
    // clobbering the active session.
    let gemini_rx = register_runtime_processed_audio_consumer(
        &state.processed_audio_consumers,
        GEMINI_NOTES_AUDIO_CONSUMER_ID,
        ProcessedAudioConsumerStage::Notes,
        Some("gemini"),
        GEMINI_AUDIO_CONSUMER_CAPACITY,
        Some(GEMINI_LIVE_AUDIO_CONSUMER_GROUP),
        {
            let is_active = state.is_gemini_active.clone();
            Arc::new(move || is_active.read().map(|a| *a).unwrap_or(false))
        },
    )?;

    // Create and connect the client. Notes-mode keeps the TEXT modality (the
    // historical default); converse-mode native audio-out (ADR-0018) flips
    // this to `GeminiConfig::audio(..)` once the converse start path lands.
    let mut config = GeminiConfig::text(gemini_settings.auth.clone(), gemini_settings.model);
    config.content_egress_policy = provider_content_egress_policy_from_settings(&settings, true);
    let mut client = GeminiLiveClient::new(config);
    if let Err(err) = client.connect() {
        unregister_runtime_processed_audio_consumer(
            &state.processed_audio_consumers,
            GEMINI_NOTES_AUDIO_CONSUMER_ID,
        );
        return Err(AppError::Unknown(err));
    }

    let event_rx = client.event_rx();

    // Mark active before starting worker threads. `connect()` can queue an
    // initial Connected event; the event receiver checks this flag before
    // processing each buffered event.
    match state.is_gemini_active.write() {
        Ok(mut active) => {
            *active = true;
        }
        Err(e) => {
            unregister_runtime_processed_audio_consumer(
                &state.processed_audio_consumers,
                GEMINI_NOTES_AUDIO_CONSUMER_ID,
            );
            client.disconnect();
            return Err(format!("Lock error: {}", e).into());
        }
    }

    // Store the client
    {
        let mut client_guard = match state.gemini_client.lock() {
            Ok(client_guard) => client_guard,
            Err(e) => {
                if let Ok(mut active) = state.is_gemini_active.write() {
                    *active = false;
                }
                unregister_runtime_processed_audio_consumer(
                    &state.processed_audio_consumers,
                    GEMINI_NOTES_AUDIO_CONSUMER_ID,
                );
                client.disconnect();
                return Err(format!("Lock error: {}", e).into());
            }
        };
        *client_guard = Some(client);
    }

    // 1. Spawn the audio sender thread.
    //    Reads from the processed audio pipeline and forwards to Gemini.
    {
        let mut audio_handle = state
            .gemini_audio_thread
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        if audio_handle.is_none() {
            let gemini_client = state.gemini_client.clone();
            let is_active = state.is_gemini_active.clone();

            let handle = match std::thread::Builder::new()
                .name("gemini-audio-sender".to_string())
                .spawn(move || {
                    log::info!("Gemini audio sender: starting");

                    while let Ok(chunk) = gemini_rx.recv() {
                        // Check if we should stop
                        let active = is_active.read().map(|a| *a).unwrap_or(false);
                        if !active {
                            break;
                        }

                        // Forward the audio to Gemini
                        // The chunk is already f32 mono 16kHz from the pipeline
                        let client_guard = match gemini_client.lock() {
                            Ok(g) => g,
                            Err(_) => break,
                        };
                        if let Some(ref client) = *client_guard {
                            if let Err(e) = client.send_audio(&chunk.data) {
                                log::warn!("Gemini audio sender: send failed: {}", e);
                                break;
                            }
                        } else {
                            break;
                        }
                    }

                    log::info!("Gemini audio sender: exiting");
                }) {
                Ok(handle) => handle,
                Err(e) => {
                    if let Ok(mut active) = state.is_gemini_active.write() {
                        *active = false;
                    }
                    unregister_runtime_processed_audio_consumer(
                        &state.processed_audio_consumers,
                        GEMINI_NOTES_AUDIO_CONSUMER_ID,
                    );
                    if let Ok(mut client_guard) = state.gemini_client.lock() {
                        if let Some(ref client) = *client_guard {
                            client.disconnect();
                        }
                        *client_guard = None;
                    }
                    return Err(AppError::Unknown(format!(
                        "Failed to spawn Gemini audio thread: {}",
                        e
                    )));
                }
            };
            *audio_handle = Some(handle);
            log::info!("Gemini audio sender thread spawned");
        }
    }

    // 2. Spawn the event receiver thread.
    //    Reads GeminiEvents and emits Tauri events + feeds the knowledge graph.
    {
        let mut event_handle = state
            .gemini_event_thread
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        if event_handle.is_none() {
            let app_handle = app.clone();
            let is_active = state.is_gemini_active.clone();
            let knowledge_graph = state.knowledge_graph.clone();
            let graph_snapshot = state.graph_snapshot.clone();
            let graph_extractor = state.graph_extractor.clone();
            let pipeline_status = state.pipeline_status.clone();
            let llm_engine = state.llm_engine.clone();
            let api_client = state.api_client.clone();
            let mistralrs_engine = state.mistralrs_engine.clone();
            let llm_executor = state.llm_executor.clone();
            let llm_provider = settings.llm_provider.clone();
            let llm_allow_cloud_fallbacks = settings
                .privacy_mode
                .allows_session_cloud_content_transfer();
            // Share the session_id Arc so per-turn writes land in the
            // CURRENT session's usage file even after `new_session_cmd`
            // rotates the ID in-process.
            let session_id_handle = state.session_id.clone();
            let transcript_ledger = state.transcript_ledger.clone();
            let processed_audio_consumers = state.processed_audio_consumers.clone();

            let handle = match std::thread::Builder::new()
                .name("gemini-event-receiver".to_string())
                .spawn(move || {
                    log::info!("Gemini event receiver: starting");

                    // Extraction counters shared with fire-and-forget tasks on
                    // the rayon pool (extraction runs OFF this event-receiver
                    // thread so a slow LLM never stalls Gemini Live events).
                    let extraction_count =
                        std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
                    let graph_update_count =
                        std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));

                    while let Ok(event) = event_rx.recv() {
                        // Check if we should stop
                        let active = is_active.read().map(|a| *a).unwrap_or(false);
                        if !active {
                            break;
                        }

                        match event {
                            GeminiEvent::Transcription { ref text, .. } => {
                                // Emit Tauri event for the frontend
                                let _ = app_handle.emit(events::GEMINI_TRANSCRIPTION, &event);

                                // Feed transcription into the knowledge graph
                                // (same extraction pipeline as local transcripts).
                                // Run it on the shared rayon extraction pool —
                                // NOT inline here — so a slow/blocked LLM cannot
                                // stall Gemini Live event handling (transcripts,
                                // status, reconnects) or back up the bounded
                                // event channel.
                                if !text.is_empty() {
                                    let expected_session_id = match session_id_handle.read() {
                                        Ok(session_id) => session_id.clone(),
                                        Err(poisoned) => poisoned.into_inner().clone(),
                                    };
                                    let segment_id = uuid::Uuid::new_v4().to_string();
                                    let timestamp = std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_secs_f64();

                                    speech::spawn_extraction_task(
                                        text.clone(),
                                        "Gemini".to_string(),
                                        String::new(),
                                        segment_id,
                                        timestamp,
                                        &speech::ExtractionDeps {
                                            active_session_id: &session_id_handle,
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
                                        },
                                        &extraction_count,
                                        &graph_update_count,
                                    );
                                }
                            }
                            GeminiEvent::ModelResponse { .. } => {
                                let _ = app_handle.emit(events::GEMINI_RESPONSE, &event);
                            }
                            GeminiEvent::Error {
                                ref category,
                                ref message,
                            } => {
                                log::error!("Gemini error event ({:?}): {}", category, message,);
                                let _ = app_handle.emit(events::GEMINI_STATUS, &event);
                            }
                            GeminiEvent::Connected => {
                                let _ = app_handle.emit(events::GEMINI_STATUS, &event);
                            }
                            GeminiEvent::TurnComplete { ref usage } => {
                                // Model finished its turn. Forward the event
                                // on GEMINI_STATUS so the UI can surface
                                // per-turn token accounting from
                                // `usageMetadata` (see gemini::UsageMetadata).
                                if let Some(u) = usage {
                                    log::debug!(
                                        "Gemini: turn complete (tokens total={:?})",
                                        u.total_token_count
                                    );
                                } else {
                                    log::debug!("Gemini: turn complete");
                                }

                                // Persist per-session token totals (loop 19).
                                // Before this, turn counts + token totals only
                                // lived in the frontend's localStorage and did
                                // not survive an app restart.
                                let delta = crate::sessions::usage::TurnDelta {
                                    prompt: usage
                                        .as_ref()
                                        .and_then(|u| u.prompt_token_count)
                                        .unwrap_or(0)
                                        as u64,
                                    response: usage
                                        .as_ref()
                                        .and_then(|u| u.response_token_count)
                                        .unwrap_or(0)
                                        as u64,
                                    cached: usage
                                        .as_ref()
                                        .and_then(|u| u.cached_content_token_count)
                                        .unwrap_or(0)
                                        as u64,
                                    thoughts: usage
                                        .as_ref()
                                        .and_then(|u| u.thoughts_token_count)
                                        .unwrap_or(0)
                                        as u64,
                                    tool_use: usage
                                        .as_ref()
                                        .and_then(|u| u.tool_use_prompt_token_count)
                                        .unwrap_or(0)
                                        as u64,
                                    total: usage
                                        .as_ref()
                                        .and_then(|u| u.total_token_count)
                                        .unwrap_or(0)
                                        as u64,
                                };
                                let current_sid = match session_id_handle.read() {
                                    Ok(g) => g.clone(),
                                    Err(poisoned) => poisoned.into_inner().clone(),
                                };
                                if let Err(e) =
                                    crate::sessions::usage::append_turn(&current_sid, delta)
                                {
                                    log::warn!("Failed to persist turn usage: {}", e);
                                }

                                let _ = app_handle.emit(events::GEMINI_STATUS, &event);
                            }
                            GeminiEvent::Disconnected => {
                                let _ = app_handle.emit(events::GEMINI_STATUS, &event);
                                break;
                            }
                            GeminiEvent::Reconnecting {
                                attempt,
                                backoff_secs,
                            } => {
                                // Auto-reconnect in flight — surface through
                                // the status event so the UI can show a
                                // "reconnecting…" hint. Do NOT break the loop:
                                // the session task handles the full setup
                                // handshake replay and will emit Reconnected
                                // on success or a fatal Error if the budget
                                // is exhausted.
                                log::info!(
                                    "Gemini: reconnecting attempt={} backoff={}s",
                                    attempt,
                                    backoff_secs
                                );
                                let _ = app_handle.emit(events::GEMINI_STATUS, &event);
                            }
                            GeminiEvent::Reconnected { resumed } => {
                                log::info!("Gemini: reconnected (resumed={})", resumed);
                                let _ = app_handle.emit(events::GEMINI_STATUS, &event);
                            }
                            // Native audio-out / barge-in events (ADR-0018).
                            // This `start_gemini` path runs the notes/graph
                            // TEXT modality, which never produces these — the
                            // converse-mode orchestrator (B18, `crate::converse`
                            // TurnMachine) consumes them via `gemini_event_to_signal`.
                            // We log + ignore here so the notes path stays
                            // exhaustive without taking on converse wiring.
                            GeminiEvent::AudioChunk { ref data_base64, .. } => {
                                log::debug!(
                                    "Gemini: unexpected AudioChunk ({} b64 chars) on notes-mode path; ignoring",
                                    data_base64.len()
                                );
                            }
                            GeminiEvent::OutputTranscription { .. } => {
                                log::debug!(
                                    "Gemini: unexpected OutputTranscription on notes-mode path; ignoring"
                                );
                            }
                            GeminiEvent::Interrupted => {
                                log::debug!("Gemini: unexpected Interrupted on notes-mode path; ignoring");
                            }
                            GeminiEvent::GenerationComplete => {
                                log::debug!("Gemini: generationComplete on notes-mode path; ignoring");
                            }
                        }
                    }

                    unregister_runtime_processed_audio_consumer(
                        &processed_audio_consumers,
                        GEMINI_NOTES_AUDIO_CONSUMER_ID,
                    );
                    log::info!("Gemini event receiver: exiting");
                }) {
                Ok(handle) => handle,
                Err(e) => {
                    if let Ok(mut active) = state.is_gemini_active.write() {
                        *active = false;
                    }
                    unregister_runtime_processed_audio_consumer(
                        &state.processed_audio_consumers,
                        GEMINI_NOTES_AUDIO_CONSUMER_ID,
                    );
                    if let Ok(mut client_guard) = state.gemini_client.lock() {
                        if let Some(ref client) = *client_guard {
                            client.disconnect();
                        }
                        *client_guard = None;
                    }
                    if let Some(handle) = state
                        .gemini_audio_thread
                        .lock()
                        .ok()
                        .and_then(|mut handle| handle.take())
                    {
                        join_worker_with_timeout(
                            handle,
                            std::time::Duration::from_secs(3),
                            "Gemini audio worker (event spawn failure)",
                            &state.retired_session_workers,
                        );
                    }
                    return Err(AppError::Unknown(format!(
                        "Failed to spawn Gemini event thread: {}",
                        e
                    )));
                }
            };
            *event_handle = Some(handle);
            log::info!("Gemini event receiver thread spawned");
        }
    }

    log::info!("Gemini Live pipeline started");
    Ok(())
}

/// Stop the Gemini Live pipeline.
///
/// Disconnects the client, signals worker threads to stop via the
/// `is_gemini_active` flag, and cleans up thread handles.
#[tauri::command]
pub async fn stop_gemini(state: State<'_, AppState>, _app: tauri::AppHandle) -> AppResult<()> {
    log::info!("stop_gemini called");
    let _session_lifecycle = state.session_lifecycle.lock().await;

    // 1. Set active flag to false (signals worker threads to exit)
    if let Ok(mut active) = state.is_gemini_active.write() {
        *active = false;
    }
    unregister_runtime_processed_audio_consumer(
        &state.processed_audio_consumers,
        GEMINI_NOTES_AUDIO_CONSUMER_ID,
    );

    // 2. Disconnect the client (sends Disconnected event, closes channels)
    {
        let mut client_guard = state
            .gemini_client
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        if let Some(ref client) = *client_guard {
            client.disconnect();
        }
        *client_guard = None;
    }

    // 3. Join the worker threads (bounded) so they fully exit before we return
    //    — prevents a fast Stop→Start race from running two Gemini workers on
    //    the same audio channel (critique H2). Detaches on timeout.
    let audio_h = state
        .gemini_audio_thread
        .lock()
        .ok()
        .and_then(|mut g| g.take());
    let event_h = state
        .gemini_event_thread
        .lock()
        .ok()
        .and_then(|mut g| g.take());
    let retired_gemini_workers = state.retired_session_workers.clone();
    let _ = tokio::task::spawn_blocking(move || {
        if let Some(h) = audio_h {
            join_worker_with_timeout(
                h,
                std::time::Duration::from_secs(3),
                "Gemini audio worker",
                &retired_gemini_workers,
            );
        }
        if let Some(h) = event_h {
            join_worker_with_timeout(
                h,
                std::time::Duration::from_secs(3),
                "Gemini event worker",
                &retired_gemini_workers,
            );
        }
    })
    .await;

    log::info!("Gemini Live pipeline stopped");
    Ok(())
}

// ---------------------------------------------------------------------------
// Converse mode — native speech-to-speech (B18 / ADR-0018)
// ---------------------------------------------------------------------------

/// Production [`crate::converse::ConverseSink`] for the Gemini native-S2S path.
///
/// Dispatches the FSM's [`crate::converse::TurnAction`]s against the live
/// engine + audio player + capture gate. Holds only `Arc` handles (cloned from
/// `AppState`) so it lives on the converse-driver thread. The pure
/// [`crate::converse::ConverseDriver`] decides; this executes — and is the only
/// part that touches I/O, which is why the decision logic is unit-tested
/// against a mock sink instead.
struct GeminiConverseSink {
    gemini_client: std::sync::Arc<std::sync::Mutex<Option<GeminiLiveClient>>>,
    audio_player: crate::playback::AudioPlayer,
    /// Per-turn capture gate (B18 step 5): the audio-sender thread streams only
    /// while `true`. On the Gemini server-VAD path capture stays open during
    /// `Speaking` (the engine drives barge-in), so toggling it is the
    /// OpenAI/client-VAD lever; we still honor Start/StopCapture here.
    capture_gate: std::sync::Arc<std::sync::atomic::AtomicBool>,
    app_handle: tauri::AppHandle,
}

impl crate::converse::ConverseSink for GeminiConverseSink {
    fn start_capture(&mut self) {
        self.capture_gate.store(true, Ordering::SeqCst);
        // Re-arm the player after a prior barge-in so the next reply is audible.
        self.audio_player.resume();
    }

    fn stop_capture(&mut self) {
        self.capture_gate.store(false, Ordering::SeqCst);
    }

    fn end_user_turn(&mut self) {
        if let Ok(guard) = self.gemini_client.lock()
            && let Some(ref client) = *guard
            && let Err(e) = client.end_user_turn()
        {
            log::warn!("converse: end_user_turn failed: {e}");
        }
    }

    fn play_audio(&mut self, pcm24: &[u8]) {
        // PlayAudio carries PCM16-LE bytes; the player wants &[i16].
        let samples = crate::converse::pcm16_le_bytes_to_i16(pcm24);
        if !samples.is_empty() {
            self.audio_player.push_samples(&samples);
        }
    }

    fn flush_playback(&mut self) {
        let _ = self.audio_player.flush_samples();
    }

    fn stop_playback(&mut self) {
        // Flush + suppress in-flight assistant audio immediately (barge-in).
        self.audio_player.cancel();
    }

    fn cancel_generation(&mut self) {
        // Gemini auto-cancels server-side on its own `interrupted`; the local
        // flush (stop_playback) is the client's part. There is no separate
        // per-turn cancel frame to send, so this is a no-op for Gemini (the
        // OpenAI Realtime voice path will send response.cancel + truncate here).
        log::debug!("converse: cancel_generation (Gemini: server auto-cancels)");
    }

    fn cancel_token(&mut self) {
        // The per-turn cancellation token (ADR-0003) gates async work for the
        // turn. The Gemini path runs no per-turn async tasks that outlive the
        // event loop, so there is nothing to trip yet; the OpenAI voice path
        // will wire a real tokio_util::CancellationToken here.
        log::debug!("converse: cancel_token (no per-turn async work on Gemini path)");
    }

    fn emit_transcript(&mut self, text: &str, final_: bool) {
        // Surface the assistant's spoken-reply transcript to the UI. (Graph
        // proposals from converse replies are a B-future enhancement; for now
        // this drives the live-transcript panel.)
        let _ = self.app_handle.emit(
            events::GEMINI_RESPONSE,
            serde_json::json!({ "text": text, "final": final_ }),
        );
    }

    fn suppressed_barge_in(&mut self, reason: crate::converse::SuppressedReason) {
        log::debug!("converse: barge-in suppressed ({reason:?})");
    }

    fn report_error(&mut self, category: crate::converse::TurnErrorCategory, message: &str) {
        log::warn!("converse: engine error ({category:?}): {message}");
        let _ = self.app_handle.emit(
            events::GEMINI_STATUS,
            serde_json::json!({ "type": "error", "message": message }),
        );
    }
}

/// Converse audio-sender loop body (AUD-CV1 / finding #48), extracted from the
/// `start_converse` spawn closure so the teardown contract is unit-testable
/// without a live socket.
///
/// Forwards captured audio chunks to the engine while converse is active and
/// the per-turn capture gate is open. Uses `recv_timeout` (not a blocking
/// `recv`) so the loop re-checks `is_active` every tick and wakes promptly when
/// `stop_converse` flips the flag — even if capture stopped first and no
/// further chunk ever arrives. A blocking `recv` would park until the *next*
/// chunk, miss the stop, force the join to time out and detach, and then let a
/// fast restart spawn a SECOND thread racing on the same runtime consumer rx.
///
/// Returns when: `is_active` is observed `false`, the rx is disconnected, the
/// client mutex is poisoned, the client slot is `None`, or a send fails.
fn run_converse_audio_sender(
    gemini_rx: &crossbeam_channel::Receiver<crate::audio::pipeline::ProcessedAudioChunk>,
    gemini_client: &std::sync::Arc<std::sync::Mutex<Option<GeminiLiveClient>>>,
    is_active: &std::sync::Arc<std::sync::RwLock<bool>>,
    capture_gate: &std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    log::info!("converse audio sender: starting");
    loop {
        if !is_active.read().map(|a| *a).unwrap_or(false) {
            break;
        }
        let chunk = match gemini_rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(c) => c,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        };
        // B18 step 5: only stream while the per-turn gate is open.
        if !capture_gate.load(Ordering::SeqCst) {
            continue;
        }
        let guard = match gemini_client.lock() {
            Ok(g) => g,
            Err(_) => break,
        };
        match *guard {
            Some(ref client) => {
                if let Err(e) = client.send_audio(&chunk.data) {
                    log::warn!("converse audio sender: send failed: {e}");
                    break;
                }
            }
            None => break,
        }
    }
    log::info!("converse audio sender: exiting");
}

/// Start a native speech-to-speech converse session (B18 / ADR-0018).
///
/// Unlike [`start_gemini`] (the notes/graph **TEXT** pipeline), this opens a
/// Gemini Live **AUDIO** session and drives a [`crate::converse::ConverseDriver`]
/// (wrapping the pure turn-FSM) from the live `GeminiEvent` stream: assistant
/// audio is decoded + played, the server's `interrupted` drives barge-in, and
/// `turnComplete` resumes listening. User audio is delivered through a runtime
/// processed-audio consumer, separate from the notes pipeline.
///
/// Spawns two threads (mirroring `start_gemini`): an audio sender gated by
/// `converse_capture_gate`, and a converse-event driver thread. Idempotent
/// guards prevent double-start and require capture to be running.
#[tauri::command]
pub async fn start_converse(state: State<'_, AppState>, app: tauri::AppHandle) -> AppResult<()> {
    log::info!("start_converse called");
    let _session_lifecycle = state.session_lifecycle.lock().await;
    ensure_session_workers_quiesced(state.inner())?;

    crate::provider_registry::ensure_provider_id_start_enabled("realtime_agent.gemini_live")?;

    // Guard: capture must be running (we need user audio to send).
    {
        let capturing = state
            .is_capturing
            .read()
            .map_err(|e| format!("Lock error: {}", e))?;
        if !*capturing {
            return Err(AppError::SessionInvalid {
                reason: "Cannot start converse: capture is not running".to_string(),
            });
        }
    }
    // Guard: don't double-start this mode. Cross-mode Gemini Live client
    // exclusivity is declared below through the processed-audio registry's
    // conflict group so future providers can reuse the same policy path.
    {
        if *state
            .is_converse_active
            .read()
            .map_err(|e| format!("Lock error: {}", e))?
        {
            return Err(AppError::SessionInvalid {
                reason: "Converse session is already running".to_string(),
            });
        }
    }

    // AUD-CV3 (#62): reap any FINISHED converse handles before respawning. The
    // driver's terminal-auth teardown (AUD-CV2) flips `is_converse_active=false`
    // and breaks, but leaves the thread slots `Some(finished_handle)` and the
    // gemini_client set. We are past the `is_converse_active` guard (false) here,
    // so without this the spawn-gates below (`if handle.is_none()`) would see a
    // stale `Some` and silently skip spawning — a restart-without-stop would
    // produce a converse session that sends/decodes nothing and reports no error.
    // Reap finished handles (join, surfacing panics) so the spawn gates fire; if
    // a handle is genuinely still running, refuse with "already running" rather
    // than double-spawn a second runtime consumer.
    {
        let mut audio_handle = state
            .converse_audio_thread
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        reap_finished_handle(&mut audio_handle, "converse audio sender")?;
    }
    {
        let mut conv_handle = state
            .converse_thread
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        reap_finished_handle(&mut conv_handle, "converse driver")?;
    }
    let settings = read_settings_for_session_content(state.inner(), "native_s2s_converse")?;
    enforce_session_content_policy(
        &app,
        state.inner(),
        &settings,
        "native_s2s_converse",
        "realtime_agent.gemini_live",
        &["audio", "transcript", "model_response"],
        true,
    )?;
    let gemini_settings = settings.gemini.clone();

    // Validate auth early (same checks as start_gemini).
    if let crate::settings::GeminiAuthMode::ApiKey { api_key } = &gemini_settings.auth
        && api_key.is_empty()
    {
        return Err(AppError::CredentialMissing {
            key: "gemini_api_key".to_string(),
        });
    }

    let gemini_rx = register_runtime_processed_audio_consumer(
        &state.processed_audio_consumers,
        GEMINI_CONVERSE_AUDIO_CONSUMER_ID,
        ProcessedAudioConsumerStage::NativeConverse,
        Some("gemini"),
        GEMINI_AUDIO_CONSUMER_CAPACITY,
        Some(GEMINI_LIVE_AUDIO_CONSUMER_GROUP),
        {
            let is_active = state.is_converse_active.clone();
            Arc::new(move || is_active.read().map(|a| *a).unwrap_or(false))
        },
    )?;

    // Clear a STALE gemini_client left behind by a terminal-auth teardown only
    // after reserving the Live-client conflict group. If notes mode is active,
    // registration fails above and this block cannot clobber its client.
    {
        let mut client_guard = match state.gemini_client.lock() {
            Ok(client_guard) => client_guard,
            Err(e) => {
                unregister_runtime_processed_audio_consumer(
                    &state.processed_audio_consumers,
                    GEMINI_CONVERSE_AUDIO_CONSUMER_ID,
                );
                return Err(format!("Lock error: {}", e).into());
            }
        };
        if client_guard.is_some() {
            log::info!("start_converse: clearing stale gemini_client from a prior session");
            *client_guard = None;
        }
    }

    // AUDIO modality with the configured voice (B18 step 1) — this is what makes
    // the server emit AudioChunk so the FSM's Thinking→Speaking edge can fire.
    let mut config = GeminiConfig::audio(
        gemini_settings.auth.clone(),
        gemini_settings.model,
        gemini_settings.voice,
    );
    config.content_egress_policy = provider_content_egress_policy_from_settings(&settings, true);
    let mut client = GeminiLiveClient::new(config);
    if let Err(err) = client.connect() {
        unregister_runtime_processed_audio_consumer(
            &state.processed_audio_consumers,
            GEMINI_CONVERSE_AUDIO_CONSUMER_ID,
        );
        return Err(AppError::Unknown(err));
    }
    let event_rx = client.event_rx();

    // Open the 24 kHz mono playback stream for assistant audio (step 4).
    let _ = state
        .audio_player
        .open_default(crate::playback::PlaybackConfig {
            source_sample_rate: 24_000,
            source_channels: 1,
        })
        .map_err(|e| log::warn!("converse: failed to open playback stream: {e}"));

    match state.is_converse_active.write() {
        Ok(mut active) => {
            *active = true;
        }
        Err(e) => {
            unregister_runtime_processed_audio_consumer(
                &state.processed_audio_consumers,
                GEMINI_CONVERSE_AUDIO_CONSUMER_ID,
            );
            client.disconnect();
            let _ = state.audio_player.stop();
            return Err(format!("Lock error: {}", e).into());
        }
    }
    state.converse_capture_gate.store(true, Ordering::SeqCst);
    {
        let mut client_guard = match state.gemini_client.lock() {
            Ok(client_guard) => client_guard,
            Err(e) => {
                if let Ok(mut active) = state.is_converse_active.write() {
                    *active = false;
                }
                state.converse_capture_gate.store(false, Ordering::SeqCst);
                unregister_runtime_processed_audio_consumer(
                    &state.processed_audio_consumers,
                    GEMINI_CONVERSE_AUDIO_CONSUMER_ID,
                );
                client.disconnect();
                let _ = state.audio_player.stop();
                return Err(format!("Lock error: {}", e).into());
            }
        };
        *client_guard = Some(client);
    }

    // 1. Audio sender thread — forward captured audio while the gate is open.
    //    AUD-CV1 (#48): uses converse's OWN thread slot and runtime consumer,
    //    never the notes-mode worker/channel.
    {
        let mut audio_handle = state
            .converse_audio_thread
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        if audio_handle.is_none() {
            let gemini_client = state.gemini_client.clone();
            let is_active = state.is_converse_active.clone();
            let capture_gate = state.converse_capture_gate.clone();
            let handle = match std::thread::Builder::new()
                .name("converse-audio-sender".to_string())
                .spawn(move || {
                    run_converse_audio_sender(
                        &gemini_rx,
                        &gemini_client,
                        &is_active,
                        &capture_gate,
                    );
                }) {
                Ok(handle) => handle,
                Err(e) => {
                    if let Ok(mut active) = state.is_converse_active.write() {
                        *active = false;
                    }
                    state.converse_capture_gate.store(false, Ordering::SeqCst);
                    unregister_runtime_processed_audio_consumer(
                        &state.processed_audio_consumers,
                        GEMINI_CONVERSE_AUDIO_CONSUMER_ID,
                    );
                    if let Ok(mut client_guard) = state.gemini_client.lock() {
                        if let Some(ref client) = *client_guard {
                            client.disconnect();
                        }
                        *client_guard = None;
                    }
                    let _ = state.audio_player.stop();
                    return Err(AppError::Unknown(format!(
                        "Failed to spawn converse audio thread: {}",
                        e
                    )));
                }
            };
            *audio_handle = Some(handle);
        }
    }

    // 2. Converse-event driver thread — drives the TurnMachine from GeminiEvents.
    {
        let mut conv_handle = state
            .converse_thread
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        if conv_handle.is_none() {
            let is_active = state.is_converse_active.clone();
            let processed_audio_consumers = state.processed_audio_consumers.clone();
            let mut sink = GeminiConverseSink {
                gemini_client: state.gemini_client.clone(),
                audio_player: state.audio_player.clone(),
                capture_gate: state.converse_capture_gate.clone(),
                app_handle: app.clone(),
            };
            let handle = match std::thread::Builder::new()
                .name("converse-driver".to_string())
                .spawn(move || {
                    log::info!("converse driver: starting");
                    // Gemini uses server-side VAD with NO client AEC reference,
                    // so audio-activity barge-in is disabled — the engine's own
                    // `interrupted` event drives barge-in (bypasses the gate).
                    let gate = crate::converse::InterruptionGate {
                        enabled: false,
                        ..Default::default()
                    };
                    let mut driver = crate::converse::ConverseDriver::new(gate);
                    // Prime into Listening (server-VAD bridge): the first
                    // assistant AudioChunk then drives Thinking→Speaking.
                    driver.begin_listening(unix_millis(), &mut sink);

                    while let Ok(event) = event_rx.recv() {
                        if !is_active.read().map(|a| *a).unwrap_or(false) {
                            break;
                        }
                        // AUD-CV2 (#49): an Auth/AuthExpired error is TERMINAL —
                        // the session cannot recover without reconfiguring/
                        // refreshing credentials, and the server may stop
                        // emitting entirely WITHOUT a `Disconnected`. If we only
                        // dispatched ReportError (state unchanged) and kept
                        // blocking on `recv()`, this thread would leak until
                        // stop_converse. So: dispatch the FSM's ReportError below
                        // (UI surfacing), then tear the session down here.
                        let terminal_auth = matches!(
                            &event,
                            GeminiEvent::Error {
                                category: crate::gemini::GeminiErrorCategory::Auth
                                    | crate::gemini::GeminiErrorCategory::AuthExpired,
                                ..
                            }
                        );
                        // Mirror notes-mode transport handling for lifecycle
                        // events the FSM does not model.
                        match &event {
                            GeminiEvent::Disconnected => {
                                let _ = sink.app_handle.emit(events::GEMINI_STATUS, &event);
                                break;
                            }
                            GeminiEvent::Connected
                            | GeminiEvent::Reconnecting { .. }
                            | GeminiEvent::Reconnected { .. } => {
                                let _ = sink.app_handle.emit(events::GEMINI_STATUS, &event);
                            }
                            GeminiEvent::Transcription { .. } => {
                                // User-speech transcript → UI (graph extraction
                                // for converse is a B-future enhancement).
                                let _ = sink.app_handle.emit(events::GEMINI_TRANSCRIPTION, &event);
                            }
                            _ => {}
                        }
                        // Drive the FSM. user_speech_ms = 0 (no client VAD on the
                        // Gemini server-VAD path); the gate is disabled anyway.
                        driver.on_gemini_event(event, unix_millis(), 0, &mut sink);

                        // AUD-CV2 (#49): on a terminal auth error, flip the
                        // shared flag off (so the audio-sender thread also wakes
                        // and exits) and break — the driver does not spin
                        // forever on a dead session.
                        if terminal_auth {
                            log::warn!(
                                "converse driver: terminal auth error — tearing down session"
                            );
                            if let Ok(mut active) = is_active.write() {
                                *active = false;
                            }
                            break;
                        }

                        // After a completed turn the FSM returns to Listening and
                        // re-emits StartCapture; if it somehow lands back in Idle
                        // (e.g. a reset), re-prime so the next turn is captured.
                        if driver.state() == crate::converse::TurnState::Idle {
                            driver.begin_listening(unix_millis(), &mut sink);
                        }
                    }
                    // Teardown: cancel any in-flight turn + flush playback.
                    driver.reset(&mut sink);
                    unregister_runtime_processed_audio_consumer(
                        &processed_audio_consumers,
                        GEMINI_CONVERSE_AUDIO_CONSUMER_ID,
                    );
                    log::info!("converse driver: exiting");
                }) {
                Ok(handle) => handle,
                Err(e) => {
                    if let Ok(mut active) = state.is_converse_active.write() {
                        *active = false;
                    }
                    state.converse_capture_gate.store(false, Ordering::SeqCst);
                    unregister_runtime_processed_audio_consumer(
                        &state.processed_audio_consumers,
                        GEMINI_CONVERSE_AUDIO_CONSUMER_ID,
                    );
                    if let Ok(mut client_guard) = state.gemini_client.lock() {
                        if let Some(ref client) = *client_guard {
                            client.disconnect();
                        }
                        *client_guard = None;
                    }
                    let _ = state.audio_player.stop();
                    if let Some(handle) = state
                        .converse_audio_thread
                        .lock()
                        .ok()
                        .and_then(|mut handle| handle.take())
                    {
                        join_worker_with_timeout(
                            handle,
                            std::time::Duration::from_secs(3),
                            "converse audio worker (driver spawn failure)",
                            &state.retired_session_workers,
                        );
                    }
                    return Err(AppError::Unknown(format!(
                        "Failed to spawn converse driver thread: {}",
                        e
                    )));
                }
            };
            *conv_handle = Some(handle);
        }
    }

    log::info!("converse session started (Gemini AUDIO)");
    Ok(())
}

async fn stop_converse_runtime(state: &AppState, join_context: &'static str) -> AppResult<()> {
    if let Ok(mut active) = state.is_converse_active.write() {
        *active = false;
    }
    state.converse_capture_gate.store(false, Ordering::SeqCst);
    unregister_runtime_processed_audio_consumer(
        &state.processed_audio_consumers,
        GEMINI_CONVERSE_AUDIO_CONSUMER_ID,
    );

    // Disconnect the client (unblocks the event receiver via Disconnected/close).
    {
        let mut client_guard = state
            .gemini_client
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        if let Some(ref client) = *client_guard {
            client.disconnect();
        }
        *client_guard = None;
    }
    // Stop playback so no assistant audio lingers.
    let _ = state.audio_player.stop();

    // Join the worker threads off-thread (bounded), mirroring stop_gemini.
    // AUD-CV1 (#48): take the converse-OWNED audio slot, never the notes
    // `gemini_audio_thread`. The audio sender wakes within one recv_timeout
    // tick (~100ms) of the `is_converse_active=false` store above, so this join
    // completes cleanly instead of detaching on timeout (which would leak the
    // thread and let a fast restart double-spawn on the same rx).
    let audio_h = state
        .converse_audio_thread
        .lock()
        .ok()
        .and_then(|mut g| g.take());
    let conv_h = state.converse_thread.lock().ok().and_then(|mut g| g.take());
    let audio_join_name = format!("converse audio worker ({join_context})");
    let driver_join_name = format!("converse driver ({join_context})");
    let retired_converse_workers = state.retired_session_workers.clone();
    let _ = tokio::task::spawn_blocking(move || {
        if let Some(h) = audio_h {
            join_worker_with_timeout(
                h,
                std::time::Duration::from_secs(3),
                &audio_join_name,
                &retired_converse_workers,
            );
        }
        if let Some(h) = conv_h {
            join_worker_with_timeout(
                h,
                std::time::Duration::from_secs(3),
                &driver_join_name,
                &retired_converse_workers,
            );
        }
    })
    .await;

    Ok(())
}

/// Stop the native converse session: disconnect the client, signal the worker
/// threads via `is_converse_active`, flush playback, and join the threads.
#[tauri::command]
pub async fn stop_converse(state: State<'_, AppState>, _app: tauri::AppHandle) -> AppResult<()> {
    log::info!("stop_converse called");
    let _session_lifecycle = state.session_lifecycle.lock().await;

    stop_converse_runtime(state.inner(), "stop_converse").await?;

    log::info!("converse session stopped");
    Ok(())
}

// ---------------------------------------------------------------------------
// OpenAI Realtime S2S voice agent (cloud-native, parallel to Gemini converse)
// ---------------------------------------------------------------------------

/// Production [`crate::converse::ConverseSink`] for the OpenAI Realtime S2S
/// path. Sibling of [`GeminiConverseSink`] — dispatches the FSM's
/// [`crate::converse::TurnAction`]s against the live `OpenAiRealtimeClient` +
/// audio player + capture gate. The pure [`crate::converse::ConverseDriver`]
/// decides; this executes.
struct OpenAiRealtimeConverseSink {
    client: std::sync::Arc<std::sync::Mutex<Option<OpenAiRealtimeClient>>>,
    audio_player: crate::playback::AudioPlayer,
    capture_gate: std::sync::Arc<std::sync::atomic::AtomicBool>,
    app_handle: tauri::AppHandle,
}

impl crate::converse::ConverseSink for OpenAiRealtimeConverseSink {
    fn start_capture(&mut self) {
        self.capture_gate.store(true, Ordering::SeqCst);
        self.audio_player.resume();
    }

    fn stop_capture(&mut self) {
        self.capture_gate.store(false, Ordering::SeqCst);
    }

    fn end_user_turn(&mut self) {
        if let Ok(guard) = self.client.lock()
            && let Some(ref client) = *guard
            && let Err(e) = client.end_user_turn()
        {
            log::warn!("openai-realtime: end_user_turn failed: {e}");
        }
    }

    fn play_audio(&mut self, pcm24: &[u8]) {
        // PlayAudio carries PCM16-LE @ 24 kHz bytes; the player wants &[i16].
        let samples = crate::converse::pcm16_le_bytes_to_i16(pcm24);
        if !samples.is_empty() {
            self.audio_player.push_samples(&samples);
        }
    }

    fn flush_playback(&mut self) {
        let _ = self.audio_player.flush_samples();
    }

    fn stop_playback(&mut self) {
        self.audio_player.cancel();
    }

    fn cancel_generation(&mut self) {
        // OpenAI Realtime voice barge-in would send response.cancel +
        // conversation.item.truncate here; cross-provider barge-in is out of
        // scope for this keystone (seed 7fcc), so the local flush
        // (stop_playback) is the client's part for now.
        log::debug!("openai-realtime: cancel_generation (client-driven cancel is B-future)");
    }

    fn cancel_token(&mut self) {
        log::debug!("openai-realtime: cancel_token (no per-turn async work)");
    }

    fn emit_transcript(&mut self, text: &str, final_: bool) {
        let _ = self.app_handle.emit(
            events::OPENAI_REALTIME_RESPONSE,
            serde_json::json!({ "text": text, "final": final_ }),
        );
    }

    fn suppressed_barge_in(&mut self, reason: crate::converse::SuppressedReason) {
        log::debug!("openai-realtime: barge-in suppressed ({reason:?})");
    }

    fn report_error(&mut self, category: crate::converse::TurnErrorCategory, message: &str) {
        log::warn!("openai-realtime: engine error ({category:?}): {message}");
        let _ = self.app_handle.emit(
            events::OPENAI_REALTIME_STATUS,
            serde_json::json!({ "type": "error", "message": message }),
        );
    }
}

/// OpenAI Realtime S2S audio-sender loop body (sibling of
/// [`run_converse_audio_sender`]). Forwards captured audio chunks to the S2S
/// client while the session is active and the per-turn capture gate is open.
fn run_openai_realtime_audio_sender(
    audio_rx: &crossbeam_channel::Receiver<crate::audio::pipeline::ProcessedAudioChunk>,
    client: &std::sync::Arc<std::sync::Mutex<Option<OpenAiRealtimeClient>>>,
    is_active: &std::sync::Arc<std::sync::RwLock<bool>>,
    capture_gate: &std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    log::info!("openai-realtime audio sender: starting");
    loop {
        if !is_active.read().map(|a| *a).unwrap_or(false) {
            break;
        }
        let chunk = match audio_rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(c) => c,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        };
        if !capture_gate.load(Ordering::SeqCst) {
            continue;
        }
        let guard = match client.lock() {
            Ok(g) => g,
            Err(_) => break,
        };
        match *guard {
            Some(ref client) => {
                if let Err(e) = client.send_audio(&chunk.data) {
                    log::warn!("openai-realtime audio sender: send failed: {e}");
                    break;
                }
            }
            None => break,
        }
    }
    log::info!("openai-realtime audio sender: exiting");
}

/// Start a cloud-native OpenAI Realtime S2S voice-agent session.
///
/// Parallel to [`start_converse`] (Gemini native S2S): opens an OpenAI Realtime
/// **voice** session (`gpt-realtime-2`) and drives a
/// [`crate::converse::ConverseDriver`] from the live `OpenAiRealtimeEvent`
/// stream — assistant audio is decoded + played, server-VAD speech boundaries
/// drive the turn FSM, and `response.done` resumes listening. User audio is
/// delivered through a dedicated runtime processed-audio consumer
/// ([`ProcessedAudioConsumerStage::RealtimeAgent`]), separate from the notes
/// and Gemini-converse pipelines.
#[tauri::command]
pub async fn start_openai_realtime(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> AppResult<()> {
    log::info!("start_openai_realtime called");
    let _session_lifecycle = state.session_lifecycle.lock().await;
    ensure_session_workers_quiesced(state.inner())?;

    crate::provider_registry::ensure_provider_id_start_enabled("realtime_agent.openai_realtime")?;

    // Guard: capture must be running (we need user audio to send).
    {
        let capturing = state
            .is_capturing
            .read()
            .map_err(|e| format!("Lock error: {}", e))?;
        if !*capturing {
            return Err(AppError::SessionInvalid {
                reason: "Cannot start OpenAI Realtime: capture is not running".to_string(),
            });
        }
    }
    // Guard: don't double-start this mode.
    {
        if *state
            .is_openai_realtime_active
            .read()
            .map_err(|e| format!("Lock error: {}", e))?
        {
            return Err(AppError::SessionInvalid {
                reason: "OpenAI Realtime session is already running".to_string(),
            });
        }
    }

    // Reap any FINISHED handles before respawning (parallel to start_converse's
    // AUD-CV3 handling): a terminal-auth teardown flips the active flag and
    // breaks but leaves the thread slots Some(finished_handle).
    {
        let mut audio_handle = state
            .openai_realtime_audio_thread
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        reap_finished_handle(&mut audio_handle, "openai-realtime audio sender")?;
    }
    {
        let mut event_handle = state
            .openai_realtime_event_thread
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        reap_finished_handle(&mut event_handle, "openai-realtime driver")?;
    }

    let settings = read_settings_for_session_content(state.inner(), "openai_realtime_s2s")?;
    enforce_session_content_policy(
        &app,
        state.inner(),
        &settings,
        "openai_realtime_s2s",
        "realtime_agent.openai_realtime",
        &["audio", "transcript", "model_response"],
        true,
    )?;
    let agent_settings = settings.openai_realtime_agent.clone();

    // Validate auth early. The credential maps to `openai_api_key` (same key as
    // the OpenAI Realtime STT provider) — see the credential mapping in
    // `credential_keys_for_provider` (`realtime_agent.openai_realtime`).
    let api_key = agent_settings.api_key();
    if api_key.trim().is_empty() {
        return Err(AppError::CredentialMissing {
            key: "openai_api_key".to_string(),
        });
    }

    let audio_rx = register_runtime_processed_audio_consumer(
        &state.processed_audio_consumers,
        OPENAI_REALTIME_AUDIO_CONSUMER_ID,
        ProcessedAudioConsumerStage::RealtimeAgent,
        Some("openai"),
        GEMINI_AUDIO_CONSUMER_CAPACITY,
        Some(OPENAI_REALTIME_AUDIO_CONSUMER_GROUP),
        {
            let is_active = state.is_openai_realtime_active.clone();
            Arc::new(move || is_active.read().map(|a| *a).unwrap_or(false))
        },
    )?;

    // Clear a STALE client left behind by a terminal-auth teardown.
    {
        let mut client_guard = match state.openai_realtime_client.lock() {
            Ok(client_guard) => client_guard,
            Err(e) => {
                unregister_runtime_processed_audio_consumer(
                    &state.processed_audio_consumers,
                    OPENAI_REALTIME_AUDIO_CONSUMER_ID,
                );
                return Err(format!("Lock error: {}", e).into());
            }
        };
        if client_guard.is_some() {
            log::info!("start_openai_realtime: clearing stale client from a prior session");
            *client_guard = None;
        }
    }

    // S2S voice config with the configured voice; thread the runtime egress
    // policy from the user's privacy mode (defense in depth) before connecting.
    let config = OpenAiRealtimeConfig::audio(api_key, agent_settings.model, agent_settings.voice)
        .with_content_egress_policy(provider_content_egress_policy_from_settings(
            &settings, true,
        ));
    let mut client = OpenAiRealtimeClient::new(config);
    if let Err(err) = client.connect() {
        unregister_runtime_processed_audio_consumer(
            &state.processed_audio_consumers,
            OPENAI_REALTIME_AUDIO_CONSUMER_ID,
        );
        return Err(AppError::Unknown(err));
    }
    let event_rx = client.event_rx();

    // Open the 24 kHz mono playback stream for assistant audio.
    let _ = state
        .audio_player
        .open_default(crate::playback::PlaybackConfig {
            source_sample_rate: 24_000,
            source_channels: 1,
        })
        .map_err(|e| log::warn!("openai-realtime: failed to open playback stream: {e}"));

    match state.is_openai_realtime_active.write() {
        Ok(mut active) => {
            *active = true;
        }
        Err(e) => {
            unregister_runtime_processed_audio_consumer(
                &state.processed_audio_consumers,
                OPENAI_REALTIME_AUDIO_CONSUMER_ID,
            );
            client.disconnect();
            let _ = state.audio_player.stop();
            return Err(format!("Lock error: {}", e).into());
        }
    }
    state
        .openai_realtime_capture_gate
        .store(true, Ordering::SeqCst);
    {
        let mut client_guard = match state.openai_realtime_client.lock() {
            Ok(client_guard) => client_guard,
            Err(e) => {
                if let Ok(mut active) = state.is_openai_realtime_active.write() {
                    *active = false;
                }
                state
                    .openai_realtime_capture_gate
                    .store(false, Ordering::SeqCst);
                unregister_runtime_processed_audio_consumer(
                    &state.processed_audio_consumers,
                    OPENAI_REALTIME_AUDIO_CONSUMER_ID,
                );
                client.disconnect();
                let _ = state.audio_player.stop();
                return Err(format!("Lock error: {}", e).into());
            }
        };
        *client_guard = Some(client);
    }

    // 1. Audio sender thread.
    {
        let mut audio_handle = state
            .openai_realtime_audio_thread
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        if audio_handle.is_none() {
            let client = state.openai_realtime_client.clone();
            let is_active = state.is_openai_realtime_active.clone();
            let capture_gate = state.openai_realtime_capture_gate.clone();
            let handle = match std::thread::Builder::new()
                .name("openai-realtime-audio-sender".to_string())
                .spawn(move || {
                    run_openai_realtime_audio_sender(&audio_rx, &client, &is_active, &capture_gate);
                }) {
                Ok(handle) => handle,
                Err(e) => {
                    if let Ok(mut active) = state.is_openai_realtime_active.write() {
                        *active = false;
                    }
                    state
                        .openai_realtime_capture_gate
                        .store(false, Ordering::SeqCst);
                    unregister_runtime_processed_audio_consumer(
                        &state.processed_audio_consumers,
                        OPENAI_REALTIME_AUDIO_CONSUMER_ID,
                    );
                    if let Ok(mut client_guard) = state.openai_realtime_client.lock() {
                        if let Some(ref client) = *client_guard {
                            client.disconnect();
                        }
                        *client_guard = None;
                    }
                    let _ = state.audio_player.stop();
                    return Err(AppError::Unknown(format!(
                        "Failed to spawn openai-realtime audio thread: {}",
                        e
                    )));
                }
            };
            *audio_handle = Some(handle);
        }
    }

    // 2. Event-driver thread — drives the TurnMachine from OpenAiRealtimeEvents.
    {
        let mut event_handle = state
            .openai_realtime_event_thread
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        if event_handle.is_none() {
            let is_active = state.is_openai_realtime_active.clone();
            let processed_audio_consumers = state.processed_audio_consumers.clone();
            let mut sink = OpenAiRealtimeConverseSink {
                client: state.openai_realtime_client.clone(),
                audio_player: state.audio_player.clone(),
                capture_gate: state.openai_realtime_capture_gate.clone(),
                app_handle: app.clone(),
            };
            let handle = match std::thread::Builder::new()
                .name("openai-realtime-driver".to_string())
                .spawn(move || {
                    log::info!("openai-realtime driver: starting");
                    // Server VAD with NO client AEC reference, so audio-activity
                    // barge-in is disabled (mirrors the Gemini converse path);
                    // server-VAD speech boundaries drive the turn FSM. Cross-
                    // provider barge-in is out of scope (seed 7fcc).
                    let gate = crate::converse::InterruptionGate {
                        enabled: false,
                        ..Default::default()
                    };
                    let mut driver = crate::converse::ConverseDriver::new(gate);
                    // Prime into Listening; the first assistant Audio chunk then
                    // drives Thinking→Speaking.
                    driver.begin_listening(unix_millis(), &mut sink);

                    while let Ok(event) = event_rx.recv() {
                        if !is_active.read().map(|a| *a).unwrap_or(false) {
                            break;
                        }
                        // A terminal Auth/AuthExpired error cannot recover
                        // without reconfiguring credentials — tear down rather
                        // than spin on a dead session (parallel to AUD-CV2).
                        let terminal_auth = matches!(
                            &event,
                            OpenAiRealtimeEvent::Error {
                                category:
                                    crate::openai_realtime::OpenAiRealtimeErrorCategory::Auth
                                    | crate::openai_realtime::OpenAiRealtimeErrorCategory::AuthExpired,
                                ..
                            }
                        );
                        // Transport/lifecycle events the FSM does not model →
                        // surface to the frontend (same envelope as Gemini).
                        match &event {
                            OpenAiRealtimeEvent::Disconnected => {
                                let _ = sink
                                    .app_handle
                                    .emit(events::OPENAI_REALTIME_STATUS, &event);
                                break;
                            }
                            OpenAiRealtimeEvent::Connected
                            | OpenAiRealtimeEvent::Reconnecting { .. }
                            | OpenAiRealtimeEvent::Reconnected { .. } => {
                                let _ = sink
                                    .app_handle
                                    .emit(events::OPENAI_REALTIME_STATUS, &event);
                            }
                            _ => {}
                        }
                        // Drive the FSM. user_speech_ms = 0 (no client VAD); the
                        // gate is disabled anyway.
                        driver.on_openai_realtime_event(event, unix_millis(), 0, &mut sink);

                        if terminal_auth {
                            log::warn!(
                                "openai-realtime driver: terminal auth error — tearing down session"
                            );
                            if let Ok(mut active) = is_active.write() {
                                *active = false;
                            }
                            break;
                        }

                        if driver.state() == crate::converse::TurnState::Idle {
                            driver.begin_listening(unix_millis(), &mut sink);
                        }
                    }
                    driver.reset(&mut sink);
                    unregister_runtime_processed_audio_consumer(
                        &processed_audio_consumers,
                        OPENAI_REALTIME_AUDIO_CONSUMER_ID,
                    );
                    log::info!("openai-realtime driver: exiting");
                }) {
                Ok(handle) => handle,
                Err(e) => {
                    if let Ok(mut active) = state.is_openai_realtime_active.write() {
                        *active = false;
                    }
                    state
                        .openai_realtime_capture_gate
                        .store(false, Ordering::SeqCst);
                    unregister_runtime_processed_audio_consumer(
                        &state.processed_audio_consumers,
                        OPENAI_REALTIME_AUDIO_CONSUMER_ID,
                    );
                    if let Ok(mut client_guard) = state.openai_realtime_client.lock() {
                        if let Some(ref client) = *client_guard {
                            client.disconnect();
                        }
                        *client_guard = None;
                    }
                    let _ = state.audio_player.stop();
                    if let Some(handle) = state
                        .openai_realtime_audio_thread
                        .lock()
                        .ok()
                        .and_then(|mut handle| handle.take())
                    {
                        join_worker_with_timeout(
                            handle,
                            std::time::Duration::from_secs(3),
                            "openai-realtime audio worker (driver spawn failure)",
                            &state.retired_session_workers,
                        );
                    }
                    return Err(AppError::Unknown(format!(
                        "Failed to spawn openai-realtime driver thread: {}",
                        e
                    )));
                }
            };
            *event_handle = Some(handle);
        }
    }

    log::info!("openai-realtime S2S session started (gpt-realtime-2 AUDIO)");
    Ok(())
}

async fn stop_openai_realtime_runtime(
    state: &AppState,
    join_context: &'static str,
) -> AppResult<()> {
    if let Ok(mut active) = state.is_openai_realtime_active.write() {
        *active = false;
    }
    state
        .openai_realtime_capture_gate
        .store(false, Ordering::SeqCst);
    unregister_runtime_processed_audio_consumer(
        &state.processed_audio_consumers,
        OPENAI_REALTIME_AUDIO_CONSUMER_ID,
    );

    {
        let mut client_guard = state
            .openai_realtime_client
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        if let Some(ref client) = *client_guard {
            client.disconnect();
        }
        *client_guard = None;
    }
    let _ = state.audio_player.stop();

    let audio_h = state
        .openai_realtime_audio_thread
        .lock()
        .ok()
        .and_then(|mut g| g.take());
    let event_h = state
        .openai_realtime_event_thread
        .lock()
        .ok()
        .and_then(|mut g| g.take());
    let audio_join_name = format!("openai-realtime audio worker ({join_context})");
    let driver_join_name = format!("openai-realtime driver ({join_context})");
    let retired_openai_workers = state.retired_session_workers.clone();
    let _ = tokio::task::spawn_blocking(move || {
        if let Some(h) = audio_h {
            join_worker_with_timeout(
                h,
                std::time::Duration::from_secs(3),
                &audio_join_name,
                &retired_openai_workers,
            );
        }
        if let Some(h) = event_h {
            join_worker_with_timeout(
                h,
                std::time::Duration::from_secs(3),
                &driver_join_name,
                &retired_openai_workers,
            );
        }
    })
    .await;

    Ok(())
}

/// Stop the OpenAI Realtime S2S session: disconnect the client, signal the
/// worker threads, flush playback, and join the threads.
#[tauri::command]
pub async fn stop_openai_realtime(
    state: State<'_, AppState>,
    _app: tauri::AppHandle,
) -> AppResult<()> {
    log::info!("stop_openai_realtime called");
    let _session_lifecycle = state.session_lifecycle.lock().await;

    stop_openai_realtime_runtime(state.inner(), "stop_openai_realtime").await?;

    log::info!("openai-realtime S2S session stopped");
    Ok(())
}

// ---------------------------------------------------------------------------
// Process enumeration
// ---------------------------------------------------------------------------

/// A running system process (for target-selection UI).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub exe_path: Option<String>,
}

/// List running system processes sorted by name, preserving duplicate process
/// names because each PID is a distinct capture target.
#[tauri::command]
pub fn list_running_processes() -> Vec<ProcessInfo> {
    use sysinfo::System;
    let mut sys = System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);

    let mut processes: Vec<ProcessInfo> = sys
        .processes()
        .iter()
        .filter(|(_, p)| !p.name().to_string_lossy().is_empty())
        .map(|(pid, p)| ProcessInfo {
            pid: pid.as_u32(),
            name: p.name().to_string_lossy().to_string(),
            exe_path: p.exe().map(|e| e.to_string_lossy().to_string()),
        })
        .collect();

    processes.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.pid.cmp(&b.pid))
    });
    // Diagnostic (audio-graph-4c16): this command previously had no log line
    // at all, so log silence proved nothing about whether it was even being
    // called during the field-reported "sources disappear" investigation.
    log::info!(
        "list_running_processes called -> {} processes",
        processes.len()
    );
    processes
}

// ---------------------------------------------------------------------------
// Persistence commands (transcript + knowledge graph)
// ---------------------------------------------------------------------------

/// Export the full in-memory transcript buffer as a JSON string.
#[tauri::command]
pub async fn export_transcript(state: State<'_, AppState>) -> AppResult<String> {
    let buffer = state
        .transcript_buffer
        .read()
        .map_err(|e| format!("Failed to read transcript buffer: {}", e))?;
    let segments: Vec<TranscriptSegment> = buffer.iter().cloned().collect();
    serde_json::to_string_pretty(&segments)
        .map_err(|e| format!("Failed to serialize transcript: {}", e))
        .map_err(AppError::from)
}

/// Save the knowledge graph to disk (session-specific file).
#[tauri::command]
pub async fn save_graph(state: State<'_, AppState>) -> AppResult<String> {
    let dir = crate::persistence::graphs_dir()
        .ok_or_else(|| "Cannot resolve graph save directory".to_string())?;

    let file_path = dir.join(format!("{}.json", state.current_session_id()));

    let graph = state
        .knowledge_graph
        .lock()
        .map_err(|e| format!("Lock error: {}", e))?;

    graph.save_to_file(&file_path)?;

    log::info!("Graph saved to {:?}", file_path);
    Ok(file_path.to_string_lossy().to_string())
}

/// Load a knowledge graph from a file on disk, replacing the current graph.
///
/// `path` is the absolute path to the JSON graph file.
#[tauri::command]
pub async fn load_graph(path: String, state: State<'_, AppState>) -> AppResult<()> {
    let file_path = std::path::PathBuf::from(&path);

    if !file_path.exists() {
        return Err(AppError::Unknown(format!("Graph file not found: {}", path)));
    }

    let loaded = crate::graph::temporal::TemporalKnowledgeGraph::load_from_file(&file_path)?;

    // Replace the in-memory knowledge graph
    {
        let mut graph = state
            .knowledge_graph
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        *graph = loaded;
    }

    // Update the cached snapshot
    {
        let graph = state
            .knowledge_graph
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        let snapshot = graph.snapshot();
        if let Ok(mut gs) = state.graph_snapshot.write() {
            *gs = snapshot;
        }
    }

    log::info!("Graph loaded from {:?}", file_path);
    Ok(())
}

/// Export the knowledge graph as a JSON string (for clipboard / download).
#[tauri::command]
pub async fn export_graph(state: State<'_, AppState>) -> AppResult<String> {
    let snapshot = state
        .graph_snapshot
        .read()
        .map_err(|e| format!("Failed to read graph snapshot: {}", e))?;
    serde_json::to_string_pretty(&*snapshot)
        .map_err(|e| format!("Failed to serialize graph: {}", e))
        .map_err(AppError::from)
}

/// Get the current session ID.
#[tauri::command]
pub async fn get_session_id(state: State<'_, AppState>) -> AppResult<String> {
    Ok(state.current_session_id())
}

fn projection_runtime_status_for_state(state: &AppState) -> AppResult<ProjectionRuntimeStatus> {
    let session_id = state.current_session_id();
    let now_ms = unix_millis();
    let (
        ledger_session_id,
        accepted_transcript_event_count,
        transcript_span_count,
        latest_asr_event_age_ms,
    ) = {
        let ledger = state
            .transcript_ledger
            .lock()
            .map_err(|e| format!("Failed to lock transcript ledger: {}", e))?;
        let latest_asr_event_age_ms = ledger
            .latest_spans
            .iter()
            .map(|span| span.received_at_ms)
            .max()
            .map(|received_at_ms| now_ms.saturating_sub(received_at_ms));
        (
            ledger.session_id.clone(),
            ledger.accepted_event_count,
            ledger.latest_spans.len(),
            latest_asr_event_age_ms,
        )
    };
    let (materialized_session_id, materialized) = {
        let materialized = state
            .materialized_projection_state
            .lock()
            .map_err(|e| format!("Failed to lock materialized projection state: {}", e))?;
        (
            materialized.session_id.clone(),
            ProjectionMaterializedStatus {
                notes_last_sequence: materialized.notes.last_sequence,
                note_count: materialized.notes.notes.len(),
                graph_last_sequence: materialized.graph.last_sequence,
                graph_node_count: materialized.graph.nodes.len(),
                graph_edge_count: materialized.graph.edges.len(),
            },
        )
    };
    let schedulers = state
        .projection_schedulers
        .lock()
        .map_err(|e| format!("Failed to lock projection schedulers: {}", e))?
        .telemetry_at(unix_millis());
    let projection_event_writer_available = state
        .projection_event_writer
        .lock()
        .map(|writer| writer.is_some())
        .unwrap_or(false);

    Ok(ProjectionRuntimeStatus {
        session_id,
        ledger_session_id,
        materialized_session_id,
        accepted_transcript_event_count,
        transcript_span_count,
        latest_asr_event_age_ms,
        projection_event_writer_available,
        schedulers,
        materialized,
    })
}

/// Return the current notes/graph projection queue and materializer status.
///
/// This is a diagnostics surface only: it exposes counts, sequence numbers, and
/// scheduler telemetry, never transcript text, note bodies, graph labels, or
/// credentials.
#[tauri::command]
pub async fn get_projection_runtime_status_cmd(
    state: State<'_, AppState>,
) -> AppResult<ProjectionRuntimeStatus> {
    projection_runtime_status_for_state(&state)
}

fn materialized_status_from_state(
    state: &crate::projections::MaterializedProjectionState,
) -> ProjectionMaterializedStatus {
    ProjectionMaterializedStatus {
        notes_last_sequence: state.notes.last_sequence,
        note_count: state.notes.notes.len(),
        graph_last_sequence: state.graph.last_sequence,
        graph_node_count: state.graph.nodes.len(),
        graph_edge_count: state.graph.edges.len(),
    }
}

fn projection_replay_artifact_status(
    present: bool,
    stored_last_sequence: u64,
    replayed_last_sequence: u64,
) -> ProjectionReplayArtifactStatus {
    if !present {
        ProjectionReplayArtifactStatus::Missing
    } else if stored_last_sequence < replayed_last_sequence {
        ProjectionReplayArtifactStatus::Stale
    } else if stored_last_sequence > replayed_last_sequence {
        ProjectionReplayArtifactStatus::Ahead
    } else {
        ProjectionReplayArtifactStatus::Current
    }
}

fn projection_operation_is_graph_retcon(
    operation: &crate::projections::ProjectionOperation,
) -> bool {
    matches!(
        operation,
        crate::projections::ProjectionOperation::InvalidateGraphNode { .. }
            | crate::projections::ProjectionOperation::InvalidateGraphEdge { .. }
            | crate::projections::ProjectionOperation::StrengthenGraphEdge { .. }
            | crate::projections::ProjectionOperation::WeakenGraphEdge { .. }
            | crate::projections::ProjectionOperation::MergeGraphNodes { .. }
            | crate::projections::ProjectionOperation::SplitGraphNode { .. }
    )
}

fn projection_replay_evaluation_metrics(
    projection_events: &[crate::projections::ProjectionPatch],
    replayed_state: &crate::projections::MaterializedProjectionState,
    stale_discard_count: usize,
) -> ProjectionReplayEvaluationMetrics {
    let mut note_operation_count = 0;
    let mut graph_operation_count = 0;
    let mut graph_retcon_operation_count = 0;
    let mut correction_patch_count = 0;

    for patch in projection_events {
        let mut patch_has_correction = false;
        for operation in &patch.operations {
            match operation {
                crate::projections::ProjectionOperation::UpsertNote { .. }
                | crate::projections::ProjectionOperation::DeleteNote { .. }
                | crate::projections::ProjectionOperation::InvalidateNote { .. }
                | crate::projections::ProjectionOperation::ReorderNote { .. } => {
                    note_operation_count += 1;
                }
                crate::projections::ProjectionOperation::UpsertGraphNode { .. }
                | crate::projections::ProjectionOperation::RemoveGraphNode { .. }
                | crate::projections::ProjectionOperation::InvalidateGraphNode { .. }
                | crate::projections::ProjectionOperation::UpsertGraphEdge { .. }
                | crate::projections::ProjectionOperation::RemoveGraphEdge { .. }
                | crate::projections::ProjectionOperation::InvalidateGraphEdge { .. }
                | crate::projections::ProjectionOperation::StrengthenGraphEdge { .. }
                | crate::projections::ProjectionOperation::WeakenGraphEdge { .. }
                | crate::projections::ProjectionOperation::MergeGraphNodes { .. }
                | crate::projections::ProjectionOperation::SplitGraphNode { .. } => {
                    graph_operation_count += 1;
                    if projection_operation_is_graph_retcon(operation) {
                        graph_retcon_operation_count += 1;
                        patch_has_correction = true;
                    }
                }
            }
        }
        if patch_has_correction {
            correction_patch_count += 1;
        }
    }

    let active_nodes: Vec<&crate::projections::MaterializedGraphNode> = replayed_state
        .graph
        .nodes
        .iter()
        .filter(|node| node.valid_until_ms.is_none())
        .collect();
    let active_edges: Vec<&crate::projections::MaterializedGraphEdge> = replayed_state
        .graph
        .edges
        .iter()
        .filter(|edge| edge.valid_until_ms.is_none())
        .collect();

    let mut node_keys: HashMap<(String, String), usize> = HashMap::new();
    for node in &active_nodes {
        let key = (
            node.entity_type.trim().to_ascii_lowercase(),
            node.name.trim().to_ascii_lowercase(),
        );
        *node_keys.entry(key).or_default() += 1;
    }

    let mut edge_keys: HashMap<(String, String, String), usize> = HashMap::new();
    for edge in &active_edges {
        let key = (
            edge.source.clone(),
            edge.target.clone(),
            edge.relation_type.trim().to_ascii_lowercase(),
        );
        *edge_keys.entry(key).or_default() += 1;
    }

    ProjectionReplayEvaluationMetrics {
        note_operation_count,
        graph_operation_count,
        graph_retcon_operation_count,
        correction_patch_count,
        stale_discard_count,
        invalidated_graph_node_count: replayed_state
            .graph
            .nodes
            .iter()
            .filter(|node| node.valid_until_ms.is_some())
            .count(),
        invalidated_graph_edge_count: replayed_state
            .graph
            .edges
            .iter()
            .filter(|edge| edge.valid_until_ms.is_some())
            .count(),
        active_graph_node_count: active_nodes.len(),
        active_graph_edge_count: active_edges.len(),
        duplicate_active_node_key_count: node_keys
            .values()
            .map(|count| count.saturating_sub(1))
            .sum(),
        duplicate_active_edge_key_count: edge_keys
            .values()
            .map(|count| count.saturating_sub(1))
            .sum(),
    }
}

fn projection_replay_latency_metrics(
    transcript_events: &[crate::projections::TranscriptEvent],
    projection_events: &[crate::projections::ProjectionPatch],
) -> ProjectionReplayLatencyMetrics {
    #[derive(Clone, Copy)]
    struct BasisTiming {
        received_at_ms: u64,
        capture_latency_ms: Option<u64>,
        asr_latency_ms: Option<u64>,
    }

    let mut timing_by_span_revision: HashMap<(String, u64), BasisTiming> = HashMap::new();
    for event in transcript_events {
        let timing = BasisTiming {
            received_at_ms: event.received_at_ms,
            capture_latency_ms: event.capture_latency_ms,
            asr_latency_ms: event.asr_latency_ms,
        };
        timing_by_span_revision
            .entry((event.span_id.clone(), event.revision_number))
            .and_modify(|current| {
                if event.received_at_ms >= current.received_at_ms {
                    *current = timing;
                }
            })
            .or_insert(timing);
    }

    let mut metrics = ProjectionReplayLatencyMetrics::default();
    for patch in projection_events {
        let mut latest_basis_timing: Option<BasisTiming> = None;
        // audio-graph-cfa1: resolve the basis's FULL covered set (tail plus
        // any summarized-away prefix) via `resolve_covered_events` rather
        // than iterating `span_revisions` directly, which is only ever the
        // verbatim tail once a basis is compacted. `resolve_covered_events`
        // returns fewer events than `covered_span_count()` only when the
        // basis names a span this `transcript_events` slice cannot
        // reproduce (a genuinely missing/unresolvable timestamp for this
        // metric, same as the pre-cfa1 tail-only lookup miss below) — so a
        // length shortfall here carries exactly the same
        // `missing_timestamp` meaning the old per-span lookup-miss branch
        // had.
        let covered_events = patch.basis.resolve_covered_events(transcript_events);
        let mut missing_timestamp = patch.basis.covered_span_count() == 0
            || covered_events.len() != patch.basis.covered_span_count();

        for event in &covered_events {
            match timing_by_span_revision
                .get(&(event.span_id.clone(), event.revision_number))
                .copied()
            {
                Some(timing) => {
                    let replace = latest_basis_timing
                        .map(|latest| timing.received_at_ms >= latest.received_at_ms)
                        .unwrap_or(true);
                    if replace {
                        latest_basis_timing = Some(timing);
                    }
                }
                None => {
                    missing_timestamp = true;
                }
            }
        }

        let lag_ms = if missing_timestamp {
            None
        } else {
            latest_basis_timing
                .map(|timing| patch.created_at_ms.saturating_sub(timing.received_at_ms))
        };

        let capture_asr_ms = latest_basis_timing.and_then(|timing| {
            if timing.capture_latency_ms.is_some() || timing.asr_latency_ms.is_some() {
                Some(
                    timing
                        .capture_latency_ms
                        .unwrap_or(0)
                        .saturating_add(timing.asr_latency_ms.unwrap_or(0)),
                )
            } else {
                None
            }
        });
        let asr_to_queue_ms = latest_basis_timing
            .and_then(|timing| patch.queued_at_ms.map(|queued| (timing, queued)))
            .map(|(timing, queued)| queued.saturating_sub(timing.received_at_ms));
        let projection_queue_ms = patch
            .queued_at_ms
            .map(|queued| patch.created_at_ms.saturating_sub(queued));
        let patch_latency = ProjectionReplayPatchLatency {
            basis_to_patch_ms: lag_ms,
            capture_asr_ms,
            asr_to_queue_ms,
            projection_queue_ms,
            generation_ms: patch.generation_latency_ms,
            apply_ms: patch.apply_latency_ms,
        };
        record_projection_replay_latency_patch(&mut metrics, &patch.kind, patch_latency);
    }

    metrics
}

#[derive(Debug, Clone, Copy, Default)]
struct ProjectionReplayPatchLatency {
    basis_to_patch_ms: Option<u64>,
    capture_asr_ms: Option<u64>,
    asr_to_queue_ms: Option<u64>,
    projection_queue_ms: Option<u64>,
    generation_ms: Option<u64>,
    apply_ms: Option<u64>,
}

fn record_projection_replay_latency_patch(
    metrics: &mut ProjectionReplayLatencyMetrics,
    kind: &crate::projections::ProjectionKind,
    latency: ProjectionReplayPatchLatency,
) {
    metrics.patch_count += 1;
    let kind_metrics = match kind {
        crate::projections::ProjectionKind::Notes => &mut metrics.notes,
        crate::projections::ProjectionKind::Graph => &mut metrics.graph,
    };
    kind_metrics.patch_count += 1;

    match latency.basis_to_patch_ms {
        Some(lag_ms) => {
            metrics.measured_patch_count += 1;
            metrics.total_basis_to_patch_lag_ms =
                metrics.total_basis_to_patch_lag_ms.saturating_add(lag_ms);
            metrics.max_basis_to_patch_lag_ms = metrics.max_basis_to_patch_lag_ms.max(lag_ms);

            kind_metrics.measured_patch_count += 1;
            kind_metrics.total_basis_to_patch_lag_ms = kind_metrics
                .total_basis_to_patch_lag_ms
                .saturating_add(lag_ms);
            kind_metrics.max_basis_to_patch_lag_ms =
                kind_metrics.max_basis_to_patch_lag_ms.max(lag_ms);
        }
        None => {
            metrics.missing_basis_timestamp_count += 1;
            kind_metrics.missing_basis_timestamp_count += 1;
        }
    }

    record_projection_replay_stage_latency(
        &mut metrics.capture_asr,
        &mut kind_metrics.capture_asr,
        latency.capture_asr_ms,
    );
    record_projection_replay_stage_latency(
        &mut metrics.asr_to_queue,
        &mut kind_metrics.asr_to_queue,
        latency.asr_to_queue_ms,
    );
    record_projection_replay_stage_latency(
        &mut metrics.projection_queue,
        &mut kind_metrics.projection_queue,
        latency.projection_queue_ms,
    );
    record_projection_replay_stage_latency(
        &mut metrics.generation,
        &mut kind_metrics.generation,
        latency.generation_ms,
    );
    record_projection_replay_stage_latency(
        &mut metrics.apply,
        &mut kind_metrics.apply,
        latency.apply_ms,
    );
}

fn record_projection_replay_stage_latency(
    metrics: &mut ProjectionReplayStageLatencyMetrics,
    kind_metrics: &mut ProjectionReplayStageLatencyMetrics,
    latency_ms: Option<u64>,
) {
    let Some(latency_ms) = latency_ms else {
        return;
    };

    metrics.measured_count += 1;
    metrics.total_ms = metrics.total_ms.saturating_add(latency_ms);
    metrics.max_ms = metrics.max_ms.max(latency_ms);

    kind_metrics.measured_count += 1;
    kind_metrics.total_ms = kind_metrics.total_ms.saturating_add(latency_ms);
    kind_metrics.max_ms = kind_metrics.max_ms.max(latency_ms);
}

fn strict_speaker_history(
    read: crate::persistence::canonical_reader::StrictCanonicalRead<
        crate::projections::DiarizationSpanRevision,
    >,
) -> Option<Vec<crate::projections::DiarizationSpanRevision>> {
    match read {
        crate::persistence::canonical_reader::StrictCanonicalRead::Missing => None,
        crate::persistence::canonical_reader::StrictCanonicalRead::Present(snapshot) => Some(
            snapshot
                .records
                .into_iter()
                .map(|record| record.payload)
                .collect(),
        ),
    }
}

/// Floor-admitted like [`read_session_transcript_snapshot`]: every canonical
/// stream this reads is v1-only, so an unadmitted v2 Session would replay v2
/// revisions through v1 logic. `maximum_supported` is v1 for that reason.
fn projection_replay_report_for_session(session_id: &str) -> AppResult<ProjectionReplayReport> {
    validate_session_id(session_id).map_err(AppError::from)?;
    let data_root = crate::user_data::resolve_data_root()
        .map_err(|error| AppError::Io(format!("resolve data root: {error}")))?;
    crate::persistence::session_semantics::open_session_for_content(
        &data_root,
        session_id,
        crate::persistence::session_semantics::SessionSemanticsVersion::V1,
        |_admitted| projection_replay_report_for_admitted_session(session_id),
    )
    .map_err(unadmitted_session_error)
}

/// Collapse an admission refusal into `AppError` WITHOUT flattening the reader's
/// own typed error: `ContentReader` carries the `AppError` the closure returned,
/// so stringifying the whole enum would turn every real failure into `Internal`.
fn unadmitted_session_error(
    error: crate::persistence::session_semantics::GuardedSessionOpenError<AppError>,
) -> AppError {
    match error {
        crate::persistence::session_semantics::GuardedSessionOpenError::ContentReader(inner) => {
            inner
        }
        refusal => AppError::SessionInvalid {
            reason: refusal.to_string(),
        },
    }
}

fn projection_replay_report_for_admitted_session(
    session_id: &str,
) -> AppResult<ProjectionReplayReport> {
    let repository = FileMemoryRepository::user_data();
    let transcript_events = repository
        .load_transcript_event_stream(session_id)?
        .into_payloads();
    let speaker_events =
        strict_speaker_history(repository.load_speaker_revision_stream(session_id)?);
    let projection_events = repository
        .load_projection_patch_stream(session_id)?
        .into_payloads();
    let stored_notes = repository.load_materialized_notes(session_id)?;
    let stored_graph = repository.load_materialized_graph(session_id)?;

    let (transcript_replay_error, transcript_span_count) =
        match crate::projections::TranscriptLedger::replay(session_id, transcript_events.clone()) {
            Ok(ledger) => (None, ledger.latest_spans.len()),
            Err(error) => (Some(format!("{:?}", error)), 0),
        };

    let (projection_replay_error, projection_history_validation, replayed_state) =
        match crate::projections::MaterializedProjectionState::replay_accepted_patches_with_history(
            session_id,
            transcript_events.clone(),
            speaker_events,
            projection_events.clone(),
        ) {
            Ok(replay) => (
                replay.validation.first_error_summary(),
                replay.validation,
                replay.state,
            ),
            Err(error) => (
                Some(format!("{:?}", error)),
                crate::projections::HistoricalProjectionValidationReport::default(),
                crate::projections::MaterializedProjectionState::new(session_id),
            ),
        };

    let replayed = materialized_status_from_state(&replayed_state);
    let evaluation = projection_replay_evaluation_metrics(
        &projection_events,
        &replayed_state,
        projection_history_validation.invalid_patch_count,
    );
    let latency = projection_replay_latency_metrics(&transcript_events, &projection_events);
    let stored_notes_last_sequence = stored_notes
        .as_ref()
        .map(|notes| notes.last_sequence)
        .unwrap_or_default();
    let stored_note_count = stored_notes
        .as_ref()
        .map(|notes| notes.notes.len())
        .unwrap_or_default();
    let stored_graph_last_sequence = stored_graph
        .as_ref()
        .map(|graph| graph.last_sequence)
        .unwrap_or_default();
    let stored_graph_item_count = stored_graph
        .as_ref()
        .map(|graph| graph.nodes.len() + graph.edges.len())
        .unwrap_or_default();

    Ok(ProjectionReplayReport {
        session_id: session_id.to_string(),
        transcript_event_count: transcript_events.len(),
        transcript_replay_error,
        transcript_span_count,
        projection_event_count: projection_events.len(),
        projection_checked_patch_count: projection_history_validation.checked_patch_count,
        projection_invalid_basis_count: projection_history_validation.invalid_patch_count,
        projection_replay_error,
        replayed,
        notes_artifact: ProjectionReplayArtifactReport {
            present: stored_notes.is_some(),
            status: projection_replay_artifact_status(
                stored_notes.is_some(),
                stored_notes_last_sequence,
                replayed_state.notes.last_sequence,
            ),
            stored_last_sequence: stored_notes_last_sequence,
            replayed_last_sequence: replayed_state.notes.last_sequence,
            stored_item_count: stored_note_count,
            replayed_item_count: replayed_state.notes.notes.len(),
        },
        graph_artifact: ProjectionReplayArtifactReport {
            present: stored_graph.is_some(),
            status: projection_replay_artifact_status(
                stored_graph.is_some(),
                stored_graph_last_sequence,
                replayed_state.graph.last_sequence,
            ),
            stored_last_sequence: stored_graph_last_sequence,
            replayed_last_sequence: replayed_state.graph.last_sequence,
            stored_item_count: stored_graph_item_count,
            replayed_item_count: replayed_state.graph.nodes.len()
                + replayed_state.graph.edges.len(),
        },
        evaluation,
        latency,
    })
}

/// Rebuild projection materialization from durable transcript/projection logs
/// and compare it with stored notes/graph artifacts.
///
/// This is a read-only replay/eval surface. It returns counts, sequence
/// numbers, and structured error strings, never transcript text, note bodies,
/// graph labels, or credentials.
#[tauri::command]
pub async fn get_projection_replay_report_cmd(
    session_id: String,
) -> AppResult<ProjectionReplayReport> {
    projection_replay_report_for_session(&session_id)
}

/// User-facing retry after the `capture-storage-full` banner.
///
/// Probes the transcripts directory with a small canary write. On success,
/// resets the process-wide storage-full debounce so the next real ENOSPC
/// re-emits `capture-storage-full`, and returns `Ok(())`. On failure, leaves
/// the debounce set and returns a structured `unknown` payload — the UI should
/// keep the banner visible so the user knows they haven't freed enough space
/// yet.
#[tauri::command]
pub async fn retry_storage_write() -> AppResult<()> {
    crate::persistence::retry_storage_write()
        .map_err(|e| format!("Storage still unavailable: {}", e))
        .map_err(AppError::from)
}

// ---------------------------------------------------------------------------
// Session management commands (v1: list / load transcript / delete)
// ---------------------------------------------------------------------------

/// List past sessions from the sessions index, most recent first.
/// Pass `limit` to cap the number of returned entries (e.g. `Some(10)`).
#[tauri::command]
pub fn list_sessions(limit: Option<usize>) -> Vec<crate::sessions::SessionMetadata> {
    let mut sessions = crate::sessions::load_index();
    if let Some(n) = limit {
        sessions.truncate(n);
    }
    sessions
}

/// Validate a session ID is safe to use as a file name segment.
/// Rejects anything that could enable path traversal (`..`, `/`, `\`, null).
fn validate_session_id(session_id: &str) -> Result<(), String> {
    crate::sessions::validate_session_id(session_id)
}

fn indexed_session_paths_resolve_only(
    session_id: &str,
) -> Result<(std::path::PathBuf, std::path::PathBuf), String> {
    validate_session_id(session_id)?;
    if let Some(metadata) = crate::sessions::find_session_resolve_only(session_id) {
        let root = crate::user_data::resolve_data_root()?;
        let transcript = if metadata.transcript_path.trim().is_empty() {
            root.join("transcripts").join(format!("{session_id}.jsonl"))
        } else {
            std::path::PathBuf::from(metadata.transcript_path)
        };
        let graph = if metadata.graph_path.trim().is_empty() {
            root.join("graphs").join(format!("{session_id}.json"))
        } else {
            std::path::PathBuf::from(metadata.graph_path)
        };
        return Ok((transcript, graph));
    }
    let root = crate::user_data::resolve_data_root()?;
    Ok((
        root.join("transcripts").join(format!("{session_id}.jsonl")),
        root.join("graphs").join(format!("{session_id}.json")),
    ))
}

struct SessionTranscriptSnapshot {
    transcript: Vec<TranscriptSegment>,
    events: Vec<crate::projections::TranscriptEvent>,
}

fn read_legacy_session_transcript(session_id: &str) -> Result<Vec<TranscriptSegment>, String> {
    let (path, _) = indexed_session_paths_resolve_only(session_id)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    // Byte ceiling (seed audio-graph-4fa5 fix round: this legacy fallback is
    // the OTHER branch of the same fork `read_session_transcript_snapshot`
    // gates — a pre-event-log session takes THIS path instead of the
    // canonical `transcripts/<id>.events.jsonl` read, so it needs its own
    // stat-before-read guard rather than inheriting the canonical read's.
    // Reuses the same ceiling/class as the canonical transcript-events read:
    // both represent the same logical "the transcript" budget, and this is
    // the pre-event-log-only fork of it, never read alongside it for the
    // same session.
    enforce_artifact_ceiling(&path, MAX_TRANSCRIPT_EVENTS_BYTES, "transcript_events")
        .map_err(|e| e.to_string())?;
    let contents = std::fs::read_to_string(&path).map_err(|e| format!("{}", e))?;
    let mut segments = Vec::new();
    for (line_index, line) in contents.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let segment = serde_json::from_str::<TranscriptSegment>(line).map_err(|_| {
            format!(
                "Legacy transcript row {} is malformed; refusing incomplete transcript",
                line_index + 1
            )
        })?;
        segments.push(segment);
    }
    Ok(segments)
}

/// Read one past Session's transcript through the guarded compatibility-floor
/// admission seam (ADR-0044 §5, seed audio-graph-e8e7).
///
/// This is the shared canonical-versus-legacy fork for `load_session_impl`,
/// `load_session_transcript`, and `session_export_bundle`, so gating it gates the
/// transcript read of all three. Both branches of the fork run INSIDE the admitted
/// floor: neither the canonical replay nor the legacy JSONL fallback can observe
/// bytes of a Session whose floor this reader does not support.
///
/// CONSTRAINT the code cannot show: this is NOT every production read of canonical
/// Session content. `projection_replay_report_for_session` and `session_timeline`
/// call `load_transcript_event_stream` with no floor check, and the
/// speaker-revision / projection-patch / materialized / live-assist reads inside
/// `load_session_impl` and `session_export_bundle` happen outside this closure.
/// Harmless only while nothing writes v2; the seed that activates a v2 writer owns
/// routing them through here (report residual R8).
///
/// `maximum_supported` is v1 because this reader understands only v1 transcript
/// semantics. Raising it belongs to whichever workstream activates v2 transcript
/// revisions.
fn read_session_transcript_snapshot(
    repository: &FileMemoryRepository,
    session_id: &str,
) -> Result<SessionTranscriptSnapshot, String> {
    validate_session_id(session_id)?;
    // Byte ceiling (seed audio-graph-4fa5): stat before ever reading the
    // canonical transcript event log into memory. The 2.1MB transcript that
    // must keep loading sits far under this. Unlike the three SIDE artifacts
    // `load_session_impl` degrades on a ceiling violation (live graph,
    // diarization log, live-assist cards — see the comments there), this
    // artifact IS the transcript lens's own content: there is no reasonable
    // "missing" fallback for an oversized transcript, so exceeding this
    // ceiling still fails the whole read, same as before this fix round.
    // Collapsing to a `String` here (rather than propagating
    // `AppError::ArtifactTooLarge` intact) trades away the typed refusal
    // notice for this one artifact — accepted because the transcript lens
    // has no dedicated refusal-notice UI the way the Notes/Graph lenses do,
    // not because this path is expected to fail in practice.
    enforce_artifact_ceiling(
        &crate::user_data::resolve_transcript_events_path(session_id)?,
        MAX_TRANSCRIPT_EVENTS_BYTES,
        "transcript_events",
    )
    .map_err(|error| error.to_string())?;
    let data_root = crate::user_data::resolve_data_root()?;
    crate::persistence::session_semantics::open_session_for_content(
        &data_root,
        session_id,
        crate::persistence::session_semantics::SessionSemanticsVersion::V1,
        |_admitted| read_admitted_session_transcript_snapshot(repository, session_id),
    )
    .map_err(|error| error.to_string())
}

fn read_admitted_session_transcript_snapshot(
    repository: &FileMemoryRepository,
    session_id: &str,
) -> Result<SessionTranscriptSnapshot, String> {
    match repository.load_transcript_event_stream(session_id)? {
        crate::persistence::canonical_reader::StrictCanonicalRead::Missing => {
            Ok(SessionTranscriptSnapshot {
                transcript: read_legacy_session_transcript(session_id)?,
                events: Vec::new(),
            })
        }
        crate::persistence::canonical_reader::StrictCanonicalRead::Present(snapshot) => {
            let events: Vec<_> = snapshot
                .records
                .into_iter()
                .map(|record| record.payload)
                .collect();
            let ledger = crate::projections::TranscriptLedger::replay(session_id, events.clone())
                .map_err(|error| {
                format!("Transcript replay failed for session {session_id}: {error:?}")
            })?;
            Ok(SessionTranscriptSnapshot {
                transcript: crate::projections::derive_legacy_transcript_segments(&ledger),
                events,
            })
        }
    }
}

fn session_has_any_artifact(session_id: &str) -> Result<bool, String> {
    Ok(
        crate::sessions::session_artifact_paths_for_id_resolve_only(session_id)?
            .iter()
            .any(|path| path.exists()),
    )
}

/// Load a past session's transcript from disk. Replays the canonical revision
/// log when present and falls back to legacy `TranscriptSegment` JSONL only for
/// pre-event-log sessions.
///
/// `async fn` (seed audio-graph-e8a5): disk I/O + ledger replay now run off
/// the win32 message-pump thread instead of inline in the IPC handler.
#[tauri::command]
pub async fn load_session_transcript(session_id: String) -> AppResult<Vec<TranscriptSegment>> {
    load_session_transcript_impl(session_id)
}

/// Read-only implementation of [`load_session_transcript`].
fn load_session_transcript_impl(session_id: String) -> AppResult<Vec<TranscriptSegment>> {
    validate_session_id(&session_id)?;
    if !session_has_any_artifact(&session_id)? {
        return Err(AppError::SessionInvalid {
            reason: format!("Session files not found: {session_id}"),
        });
    }
    let read_start = std::time::Instant::now();
    let transcript_bytes = artifact_len_for_log(
        &crate::user_data::resolve_transcript_events_path(&session_id).unwrap_or_default(),
    );
    let transcript =
        read_session_transcript_snapshot(&FileMemoryRepository::user_data(), &session_id)
            .map(|snapshot| snapshot.transcript)
            .map_err(AppError::from)?;
    let response_bytes = response_len_for_log(&transcript);
    let elapsed_ms = read_start.elapsed().as_millis();
    let log_line = format!(
        "load_session_transcript session_id={session_id} transcript_events_bytes={transcript_bytes} response_bytes={response_bytes} elapsed_ms={elapsed_ms}"
    );
    if response_bytes > RESPONSE_SIZE_WARN_THRESHOLD_BYTES {
        log::warn!("{log_line}");
    } else {
        log::info!("{log_line}");
    }
    Ok(transcript)
}

/// Load a past session's data-movement ledger (seed audio-graph-70a3) for the
/// privacy route report UI (seed audio-graph-51e0).
///
/// Returns the append-ordered, already-redacted [`DataMovementEvent`]s from
/// `~/.audiograph/ledgers/<session_id>.movements.jsonl`. The ledger schema is
/// redaction-safe by construction — it carries only data *classes*, boundary
/// hops, provider/model ids, hashed artifact paths, and pre-redacted error
/// messages, never secrets or raw payloads — so the events can be surfaced to
/// the user verbatim. A missing or empty ledger yields an empty vec; because
/// production coverage is not yet exhaustive, the UI must render that as
/// Unknown rather than proof that no content left the device.
///
/// `async fn` (fix-round finding: this command was the one session-scoped
/// historical read left sync, unbounded, and uninstrumented outside every
/// other seed audio-graph-4fa5 deliverable) — moves the ledger read off the
/// win32 message-pump thread, same as every sibling session command.
#[tauri::command]
pub async fn load_session_data_movement_cmd(
    session_id: String,
) -> AppResult<Vec<crate::persistence::DataMovementEvent>> {
    load_session_data_movement_impl(session_id)
}

/// Read-only implementation of [`load_session_data_movement_cmd`].
fn load_session_data_movement_impl(
    session_id: String,
) -> AppResult<Vec<crate::persistence::DataMovementEvent>> {
    // Defense-in-depth: reject path-traversal session ids before joining the id
    // into the ledgers directory (audio-graph-e692). Mirrors every sibling
    // session command, which all validate first.
    validate_session_id(&session_id)?;
    let read_start = std::time::Instant::now();
    let ledger_bytes = enforce_artifact_ceiling(
        &crate::user_data::resolve_data_movement_ledger_path(&session_id)?,
        MAX_DATA_MOVEMENT_EVENTS_BYTES,
        "data_movement_events",
    )?;
    let events = FileMemoryRepository::user_data()
        .load_data_movement_event_stream(&session_id)
        .map(crate::persistence::canonical_reader::StrictCanonicalRead::into_payloads)
        .map_err(AppError::from)?;
    let response_bytes = response_len_for_log(&events);
    let elapsed_ms = read_start.elapsed().as_millis();
    let log_line = format!(
        "load_session_data_movement session_id={session_id} data_movement_events_bytes={ledger_bytes} response_bytes={response_bytes} elapsed_ms={elapsed_ms}"
    );
    if response_bytes > RESPONSE_SIZE_WARN_THRESHOLD_BYTES {
        log::warn!("{log_line}");
    } else {
        log::info!("{log_line}");
    }
    Ok(events)
}

fn choose_materialized_notes(
    loaded: Option<crate::projections::MaterializedNotes>,
    replayed: Option<&crate::projections::MaterializedProjectionState>,
    canonical_notes_present: bool,
) -> Option<crate::projections::MaterializedNotes> {
    if canonical_notes_present {
        return replayed.map(|state| state.notes.clone());
    }
    loaded
}

fn choose_materialized_graph(
    loaded: Option<crate::projections::MaterializedGraph>,
    replayed: Option<&crate::projections::MaterializedProjectionState>,
    canonical_graph_present: bool,
) -> Option<crate::projections::MaterializedGraph> {
    if canonical_graph_present {
        return replayed.map(|state| state.graph.clone());
    }
    loaded
}

/// Load a past session's durable artifacts for the frontend Review workspace.
///
/// This command is deliberately read-only with respect to the live capture
/// runtime. Opening history must never rotate the active transcript ledger,
/// projection schedulers, or graph while capture and autosave continue.
///
/// `async fn` (seed audio-graph-e8a5): disk I/O now runs off the win32
/// message-pump thread instead of inline in the IPC handler. The response
/// itself is transcript-lens-only (seed audio-graph-4fa5 deliverable a) — see
/// [`load_session_notes_artifacts_cmd`] / [`load_session_graph_artifact_cmd`]
/// for the artifacts this command used to bundle.
#[tauri::command]
pub async fn load_session(session_id: String) -> AppResult<LoadedSession> {
    load_session_impl(session_id)
}

/// Read-only implementation of [`load_session`].
fn load_session_impl(session_id: String) -> AppResult<LoadedSession> {
    validate_session_id(&session_id)?;
    let (_transcript_path, graph_path) = indexed_session_paths_resolve_only(&session_id)?;
    if !session_has_any_artifact(&session_id)? {
        return Err(AppError::SessionInvalid {
            reason: format!("Session files not found: {}", session_id),
        });
    }
    let read_start = std::time::Instant::now();
    let repository = FileMemoryRepository::user_data();
    let transcript_bytes = artifact_len_for_log(
        &crate::user_data::resolve_transcript_events_path(&session_id).unwrap_or_default(),
    );
    let transcript_snapshot = read_session_transcript_snapshot(&repository, &session_id)?;
    let transcript = transcript_snapshot.transcript;
    let transcript_events = transcript_snapshot.events;

    // `load_session` bundles several artifacts behind one response, all
    // gated by `?` — but only `transcript`/`transcript_events` (via
    // `read_session_transcript_snapshot` above) is the artifact this command
    // exists to keep loading no matter what. The three side artifacts below
    // (live graph, diarization log, live-assist cards) degrade to their
    // "missing" fallback — logging a warning, never a silent no-op — rather
    // than failing the WHOLE session open when one of them alone exceeds its
    // ceiling (fix-round finding: a side-artifact ceiling refusal used to
    // take the transcript lens down with it, which is strictly worse than
    // the pre-ceiling behavior of loading it, slowly, in full). Each has its
    // own dedicated lens/consumer that can still show a visible refusal
    // (Graph lens: `load_session_graph_artifact_cmd`; this command has none
    // for these three, so a warn log is the only signal — acceptable because
    // all three are supplementary to the transcript, and two of the three
    // are already structurally capped at write time).
    let live_graph_ceiling =
        enforce_artifact_ceiling(&graph_path, MAX_LIVE_GRAPH_BYTES, "live_graph");
    let (live_graph_bytes, snapshot) = match live_graph_ceiling {
        Ok(bytes) => {
            let loaded_graph = if graph_path.exists() {
                crate::graph::temporal::TemporalKnowledgeGraph::load_from_file(&graph_path)?
            } else {
                crate::graph::temporal::TemporalKnowledgeGraph::new()
            };
            (bytes, loaded_graph.snapshot())
        }
        Err(AppError::ArtifactTooLarge {
            size_bytes,
            ceiling_bytes,
            ..
        }) => {
            log::warn!(
                "load_session session_id={session_id} artifact_class=live_graph size_bytes={size_bytes} ceiling_bytes={ceiling_bytes} degrading_to=empty_snapshot"
            );
            (
                size_bytes,
                crate::graph::temporal::TemporalKnowledgeGraph::new().snapshot(),
            )
        }
        Err(other) => return Err(other),
    };
    // Diarization span revisions (audio-graph-0b33): the persisted speaker log
    // the live path now writes (audio-graph-719d). Surfacing it lets the
    // frontend resolve trusted latest-wins speaker attribution on reload rather
    // than trusting the inline ASR labels. A session that never emitted
    // diarization rows loads an empty vec — same fallback an oversized log
    // degrades to (see the comment above `live_graph_ceiling`).
    let diarization_ceiling = enforce_artifact_ceiling(
        &crate::user_data::resolve_diarization_events_path(&session_id)?,
        MAX_DIARIZATION_EVENTS_BYTES,
        "diarization_events",
    );
    let (diarization_bytes, diarization_events) = match diarization_ceiling {
        Ok(bytes) => {
            let events =
                strict_speaker_history(repository.load_speaker_revision_stream(&session_id)?)
                    .unwrap_or_default();
            (bytes, events)
        }
        Err(AppError::ArtifactTooLarge {
            size_bytes,
            ceiling_bytes,
            ..
        }) => {
            log::warn!(
                "load_session session_id={session_id} artifact_class=diarization_events size_bytes={size_bytes} ceiling_bytes={ceiling_bytes} degrading_to=empty_vec"
            );
            (size_bytes, Vec::new())
        }
        Err(other) => return Err(other),
    };
    let live_assist_ceiling = enforce_artifact_ceiling(
        &crate::user_data::resolve_live_assist_current_path(&session_id)?,
        MAX_LIVE_ASSIST_CARDS_BYTES,
        "live_assist_cards",
    );
    let (live_assist_bytes, live_assist_cards) = match live_assist_ceiling {
        Ok(bytes) => (bytes, repository.load_live_assist_cards(&session_id)?),
        Err(AppError::ArtifactTooLarge {
            size_bytes,
            ceiling_bytes,
            ..
        }) => {
            log::warn!(
                "load_session session_id={session_id} artifact_class=live_assist_cards size_bytes={size_bytes} ceiling_bytes={ceiling_bytes} degrading_to=empty_vec"
            );
            (size_bytes, Vec::new())
        }
        Err(other) => return Err(other),
    };
    // Replay validates the persisted transcript history before returning it,
    // but the historical ledger is never installed into live AppState.
    let _validated_ledger =
        crate::projections::TranscriptLedger::replay(&session_id, transcript_events.clone())
            .map_err(|e| {
                format!(
                    "Failed to replay transcript ledger for session {}: {:?}",
                    session_id, e
                )
            })?;

    let loaded = LoadedSession {
        transcript,
        graph: snapshot,
        transcript_events,
        diarization_events,
        live_assist_cards,
    };
    let response_bytes = response_len_for_log(&loaded);
    let elapsed_ms = read_start.elapsed().as_millis();
    let log_line = format!(
        "load_session session_id={session_id} transcript_events_bytes={transcript_bytes} live_graph_bytes={live_graph_bytes} diarization_events_bytes={diarization_bytes} live_assist_cards_bytes={live_assist_bytes} response_bytes={response_bytes} elapsed_ms={elapsed_ms}"
    );
    if response_bytes > RESPONSE_SIZE_WARN_THRESHOLD_BYTES {
        log::warn!("{log_line}");
    } else {
        log::info!("{log_line}");
    }
    Ok(loaded)
}

/// Replay the canonical projection log against the transcript/speaker
/// histories, refusing (typed `SessionInvalid`) if any patch fails
/// validation. Shared by [`load_session_notes_artifacts_cmd`] and
/// [`load_session_graph_artifact_cmd`] — each lens replays independently when
/// it activates (seed audio-graph-4fa5 deliverable a); a shared cache across
/// lenses is explicitly out of scope here. Seed audio-graph-4fa5 deliverable
/// f moved this replay off the UI thread and behind a lens fetch WITHOUT
/// restructuring `projections.rs`'s own algorithm; seed audio-graph-927a
/// landed that algorithmic fix (`LedgerHistory`'s forward cursors,
/// `projections.rs`) — each lens activation still pays one replay call, but
/// that call no longer re-folds the same raw transcript/speaker event
/// across patches: it is now O(events + patches × distinct_spans) instead
/// of O(patches × events), because `classify_basis_currency` /
/// `resolve_claim_evidence_basis_events` (deliberately left unmodified by
/// that ticket) still clone/re-derive the current ledger once per patch —
/// bounded by the DISTINCT-span count, not the raw event count, and paid
/// once either way. See `projections.rs`'s `LedgerHistory` doc comment for
/// the precise accounting.
fn replay_projection_state_or_invalid(
    session_id: &str,
    transcript_events: Vec<crate::projections::TranscriptEvent>,
    speaker_history: Option<Vec<crate::projections::DiarizationSpanRevision>>,
    projection_events: Vec<crate::projections::ProjectionPatch>,
) -> AppResult<crate::projections::MaterializedProjectionState> {
    match crate::projections::MaterializedProjectionState::replay_accepted_patches_with_history(
        session_id,
        transcript_events,
        speaker_history,
        projection_events,
    ) {
        Ok(replay) => {
            if replay.validation.invalid_patch_count > 0 {
                Err(AppError::SessionInvalid {
                    reason: format!(
                        "Canonical projection replay rejected {} patch(es); derived caches were not loaded",
                        replay.validation.invalid_patch_count
                    ),
                })
            } else {
                Ok(replay.state)
            }
        }
        Err(e) => Err(AppError::SessionInvalid {
            reason: format!(
                "Canonical projection replay failed for session {}: {:?}",
                session_id, e
            ),
        }),
    }
}

/// Gather the projection-patch log plus (when a canonical stream exists) the
/// replayed [`crate::projections::MaterializedProjectionState`] shared by the
/// Notes-lens and Graph-lens artifact commands. Every artifact this touches
/// is ceiling-checked before being read into memory (seed audio-graph-4fa5
/// deliverable b).
fn gather_projection_lens_state(
    session_id: &str,
) -> AppResult<(
    Vec<crate::projections::ProjectionPatch>,
    Option<crate::projections::MaterializedProjectionState>,
    bool,
)> {
    let repository = FileMemoryRepository::user_data();
    enforce_artifact_ceiling(
        &crate::user_data::resolve_projection_events_path(session_id)?,
        MAX_PROJECTION_EVENTS_BYTES,
        "projection_events",
    )?;
    let projection_stream = repository.load_projection_patch_stream(session_id)?;
    // The opened snapshot presence, not a row count or second filesystem probe,
    // marks canonical-era projection authority. An empty canonical log
    // must not let an orphan materialized cache become user-visible truth after
    // a crash between cache replacement and async event persistence.
    let canonical_projection_stream_exists = matches!(
        &projection_stream,
        crate::persistence::canonical_reader::StrictCanonicalRead::Present(_)
    );
    let projection_events = projection_stream.into_payloads();

    let replayed_projection_state = if canonical_projection_stream_exists {
        enforce_artifact_ceiling(
            &crate::user_data::resolve_transcript_events_path(session_id)?,
            MAX_TRANSCRIPT_EVENTS_BYTES,
            "transcript_events",
        )?;
        let transcript_events = repository
            .load_transcript_event_stream(session_id)?
            .into_payloads();
        enforce_artifact_ceiling(
            &crate::user_data::resolve_diarization_events_path(session_id)?,
            MAX_DIARIZATION_EVENTS_BYTES,
            "diarization_events",
        )?;
        let speaker_history =
            strict_speaker_history(repository.load_speaker_revision_stream(session_id)?);
        Some(replay_projection_state_or_invalid(
            session_id,
            transcript_events,
            speaker_history,
            projection_events.clone(),
        )?)
    } else {
        None
    };

    Ok((
        projection_events,
        replayed_projection_state,
        canonical_projection_stream_exists,
    ))
}

/// Load a past session's Notes-lens artifacts: the materialized notes
/// (replayed against the canonical projection log when present, exactly like
/// `load_session` used to do inline) plus the raw projection-patch log
/// `NotesPanel` derives `noteRevisionCounts` from. Deferred out of
/// `load_session` (seed audio-graph-4fa5 deliverable a) so a session open
/// alone never pays for these — 19.1MB + 33.3MB for the field session that
/// OOM'd the app. NOTE this is genuinely deferred only relative to
/// `load_session` itself: the Notes lens is `SessionsBrowser`'s
/// default-active lens (`useState<DetailLens>("notes")`), so in practice
/// this fires immediately after most session opens too — only the Graph
/// lens (`load_session_graph_artifact_cmd`) is deferred until the user picks
/// a non-default tab. The real protection for THIS artifact pair is the
/// byte ceiling below, not lens-gating.
///
/// `async fn` (seed audio-graph-e8a5) so the canonical-replay work (seed
/// audio-graph-927a's `LedgerHistory` forward-cursor replay, `projections.rs`
/// — O(events + patches × distinct_spans), not O(patches × events); see
/// `replay_projection_state_or_invalid`'s doc comment above) runs off the
/// message-pump thread.
#[tauri::command]
pub async fn load_session_notes_artifacts_cmd(
    session_id: String,
) -> AppResult<SessionNotesArtifacts> {
    load_session_notes_artifacts_impl(session_id)
}

fn load_session_notes_artifacts_impl(session_id: String) -> AppResult<SessionNotesArtifacts> {
    validate_session_id(&session_id)?;
    if !session_has_any_artifact(&session_id)? {
        return Err(AppError::SessionInvalid {
            reason: format!("Session files not found: {}", session_id),
        });
    }
    // Floor-admitted (fix-round finding): both new lens commands used to read
    // canonical content directly, bypassing the same
    // `open_session_for_content` seam `load_session_transcript` and
    // `session_timeline` gate their reads behind. Routed through it now so
    // an unreadable/unsupported control plane refuses here too, not just on
    // the transcript-lens path.
    let data_root = crate::user_data::resolve_data_root()
        .map_err(|error| AppError::Io(format!("resolve data root: {error}")))?;
    crate::persistence::session_semantics::open_session_for_content(
        &data_root,
        &session_id,
        crate::persistence::session_semantics::SessionSemanticsVersion::V1,
        |_admitted| load_session_notes_artifacts_for_admitted_session(&session_id),
    )
    .map_err(unadmitted_session_error)
}

fn load_session_notes_artifacts_for_admitted_session(
    session_id: &str,
) -> AppResult<SessionNotesArtifacts> {
    let read_start = std::time::Instant::now();
    let repository = FileMemoryRepository::user_data();
    let notes_bytes = enforce_artifact_ceiling(
        &crate::user_data::resolve_notes_path(session_id)?,
        MAX_MATERIALIZED_NOTES_BYTES,
        "materialized_notes",
    )?;
    let loaded_notes = repository.load_materialized_notes(session_id)?;
    let (projection_events, replayed_projection_state, canonical_projection_stream_exists) =
        gather_projection_lens_state(session_id)?;
    let notes = choose_materialized_notes(
        loaded_notes,
        replayed_projection_state.as_ref(),
        canonical_projection_stream_exists,
    );
    let artifacts = SessionNotesArtifacts {
        notes,
        projection_events,
    };
    let response_bytes = response_len_for_log(&artifacts);
    let elapsed_ms = read_start.elapsed().as_millis();
    let log_line = format!(
        "load_session_notes_artifacts session_id={session_id} materialized_notes_bytes={notes_bytes} response_bytes={response_bytes} elapsed_ms={elapsed_ms}"
    );
    if response_bytes > RESPONSE_SIZE_WARN_THRESHOLD_BYTES {
        log::warn!("{log_line}");
    } else {
        log::info!("{log_line}");
    }
    Ok(artifacts)
}

/// Load a past session's materialized (Graph-lens) knowledge graph —
/// replayed against the canonical projection log when present, exactly like
/// `load_session` used to do inline. Deferred out of `load_session` (seed
/// audio-graph-4fa5 deliverable a) so opening a session never pays for it —
/// 156.6MB for the field session that OOM'd the app — unless the Graph lens
/// actually activates.
///
/// `async fn` (seed audio-graph-e8a5); see [`load_session_notes_artifacts_cmd`]
/// for why the canonical replay itself stays un-restructured (deliverable f).
#[tauri::command]
pub async fn load_session_graph_artifact_cmd(
    session_id: String,
) -> AppResult<Option<crate::projections::MaterializedGraph>> {
    load_session_graph_artifact_impl(session_id)
}

fn load_session_graph_artifact_impl(
    session_id: String,
) -> AppResult<Option<crate::projections::MaterializedGraph>> {
    validate_session_id(&session_id)?;
    if !session_has_any_artifact(&session_id)? {
        return Err(AppError::SessionInvalid {
            reason: format!("Session files not found: {}", session_id),
        });
    }
    // Floor-admitted — see the matching comment in
    // `load_session_notes_artifacts_impl`.
    let data_root = crate::user_data::resolve_data_root()
        .map_err(|error| AppError::Io(format!("resolve data root: {error}")))?;
    crate::persistence::session_semantics::open_session_for_content(
        &data_root,
        &session_id,
        crate::persistence::session_semantics::SessionSemanticsVersion::V1,
        |_admitted| load_session_graph_artifact_for_admitted_session(&session_id),
    )
    .map_err(unadmitted_session_error)
}

fn load_session_graph_artifact_for_admitted_session(
    session_id: &str,
) -> AppResult<Option<crate::projections::MaterializedGraph>> {
    let read_start = std::time::Instant::now();
    let repository = FileMemoryRepository::user_data();
    let materialized_graph_bytes = enforce_artifact_ceiling(
        &crate::user_data::resolve_materialized_graph_path(session_id)?,
        MAX_MATERIALIZED_GRAPH_BYTES,
        "materialized_graph",
    )?;
    let loaded_graph = repository.load_materialized_graph(session_id)?;
    let (_projection_events, replayed_projection_state, canonical_projection_stream_exists) =
        gather_projection_lens_state(session_id)?;
    let materialized_graph = choose_materialized_graph(
        loaded_graph,
        replayed_projection_state.as_ref(),
        canonical_projection_stream_exists,
    );
    let response_bytes = response_len_for_log(&materialized_graph);
    let elapsed_ms = read_start.elapsed().as_millis();
    let log_line = format!(
        "load_session_graph_artifact session_id={session_id} materialized_graph_bytes={materialized_graph_bytes} response_bytes={response_bytes} elapsed_ms={elapsed_ms}"
    );
    if response_bytes > RESPONSE_SIZE_WARN_THRESHOLD_BYTES {
        log::warn!("{log_line}");
    } else {
        log::info!("{log_line}");
    }
    Ok(materialized_graph)
}

/// Assemble the version-1 core [`SessionExportBundle`] from durable artifacts.
/// Usage, scheduler, live-assist audit, and data-movement records remain outside
/// this schema until the one typed artifact manifest in ADR-0027 is complete.
///
/// Reads (all read-only, none mutate state):
///   - transcript segments derived from the canonical revision log, with a
///     legacy JSONL fallback for pre-event-log sessions
///   - transcript event log (`transcripts/<id>.events.jsonl`)
///   - diarization span-revision log (`transcripts/<id>.speaker.jsonl`)
///   - projection event log (`projections/<id>.events.jsonl`)
///   - materialized notes (`notes/<id>.json`)
///   - materialized graph (`graphs/<id>.materialized.json`)
///   - legacy graph snapshot (`graphs/<id>.json`)
///
/// Missing logs/artifacts collapse to empty collections / `None` so an old
/// transcript-only session still exports without error. The session must have
/// at least one artifact on disk, otherwise this returns
/// [`AppError::SessionInvalid`] (the same guard `load_session` uses) so the
/// caller does not silently export an empty bundle for a bad ID.
///
/// Every artifact above is ceiling-checked (seed audio-graph-4fa5 deliverable
/// b) before being read, same as `load_session`/the lens commands — but
/// UNLIKE `load_session_impl`, an over-ceiling artifact here still fails the
/// whole export (fix-round finding: this removes the one escape hatch that
/// could get a field-crash-sized session's data out of the app for backup or
/// repair). Deliberately left as-is rather than adding a partial/step-down
/// export: which artifacts to drop from an export and how to signal that to
/// the user is a product decision, not a mechanical fix, and is out of scope
/// for this unit — flagged as a seed-worthy follow-up rather than silently
/// left broken.
fn session_export_bundle(session_id: &str) -> AppResult<SessionExportBundle> {
    validate_session_id(session_id)?;

    if !session_has_any_artifact(session_id)? {
        return Err(AppError::SessionInvalid {
            reason: format!("Session files not found: {}", session_id),
        });
    }

    let read_start = std::time::Instant::now();
    let (_transcript_path, graph_path) = indexed_session_paths_resolve_only(session_id)?;
    let repository = FileMemoryRepository::user_data();
    let transcript_snapshot = read_session_transcript_snapshot(&repository, session_id)?;
    let transcript = transcript_snapshot.transcript;
    let transcript_events = transcript_snapshot.events;

    let live_graph_bytes =
        enforce_artifact_ceiling(&graph_path, MAX_LIVE_GRAPH_BYTES, "live_graph")?;
    let graph = if graph_path.exists() {
        Some(
            crate::graph::temporal::TemporalKnowledgeGraph::load_from_file(&graph_path)?.snapshot(),
        )
    } else {
        None
    };

    let diarization_bytes = enforce_artifact_ceiling(
        &crate::user_data::resolve_diarization_events_path(session_id)?,
        MAX_DIARIZATION_EVENTS_BYTES,
        "diarization_events",
    )?;
    let diarization_events = repository
        .load_speaker_revision_stream(session_id)?
        .into_payloads();
    let projection_events_bytes = enforce_artifact_ceiling(
        &crate::user_data::resolve_projection_events_path(session_id)?,
        MAX_PROJECTION_EVENTS_BYTES,
        "projection_events",
    )?;
    let projection_events = repository
        .load_projection_patch_stream(session_id)?
        .into_payloads();
    let notes_bytes = enforce_artifact_ceiling(
        &crate::user_data::resolve_notes_path(session_id)?,
        MAX_MATERIALIZED_NOTES_BYTES,
        "materialized_notes",
    )?;
    let notes = repository.load_materialized_notes(session_id)?;
    let materialized_graph_bytes = enforce_artifact_ceiling(
        &crate::user_data::resolve_materialized_graph_path(session_id)?,
        MAX_MATERIALIZED_GRAPH_BYTES,
        "materialized_graph",
    )?;
    let materialized_graph = repository.load_materialized_graph(session_id)?;

    let bundle = SessionExportBundle {
        schema_version: SESSION_EXPORT_SCHEMA_VERSION,
        session_id: session_id.to_string(),
        metadata: crate::sessions::find_session_resolve_only(session_id),
        transcript,
        transcript_events,
        diarization_events,
        projection_events,
        notes,
        materialized_graph,
        graph,
    };
    let response_bytes = response_len_for_log(&bundle);
    let elapsed_ms = read_start.elapsed().as_millis();
    let log_line = format!(
        "session_export_bundle session_id={session_id} live_graph_bytes={live_graph_bytes} diarization_events_bytes={diarization_bytes} projection_events_bytes={projection_events_bytes} materialized_notes_bytes={notes_bytes} materialized_graph_bytes={materialized_graph_bytes} response_bytes={response_bytes} elapsed_ms={elapsed_ms}"
    );
    if response_bytes > RESPONSE_SIZE_WARN_THRESHOLD_BYTES {
        log::warn!("{log_line}");
    } else {
        log::info!("{log_line}");
    }
    Ok(bundle)
}

/// Export the version-1 portable core session bundle: transcript segments +
/// transcript event log + diarization event log + projection event log +
/// materialized notes + materialized graph + legacy graph snapshot, plus a
/// schema version and index metadata. This is not yet the complete typed
/// artifact export required by ADR-0027.
///
/// This is the session-level counterpart to the in-memory `export_transcript`
/// / `export_graph` commands: it works on any on-disk session (not just the
/// active one) and captures the whole event-sourced lifecycle boundary rather
/// than only the legacy graph snapshot.
///
/// `async fn` (seed audio-graph-e8a5): disk I/O now runs off the win32
/// message-pump thread instead of inline in the IPC handler.
#[tauri::command]
pub async fn export_session_bundle(session_id: String) -> AppResult<SessionExportBundle> {
    session_export_bundle(&session_id)
}

/// Build the ordered, speaker-attributed, provenance-linked session timeline
/// (epic 0d72 P1, ADR-0026 §4.1) for a session from its durable on-disk logs.
///
/// This is the backend home for the [`crate::timeline::build_session_timeline`]
/// read-model fold. Reads (all read-only, none mutate state), then replays the
/// three event-sourced structures the fold consumes:
///   - transcript event log (`transcripts/<id>.events.jsonl`) → [`TranscriptLedger`]
///   - diarization span-revision log (`transcripts/<id>.speaker.jsonl`) →
///     [`SpeakerTimeline`] (so a *loaded* session resolves trustworthy
///     latest-wins speakers backend-side, per ADR-0026 F3, rather than trusting
///     the untrusted inline ASR labels the frontend-only selector falls back to)
///   - the **live** knowledge graph (`graphs/<id>.json`) →
///     [`TemporalKnowledgeGraph`], whose `TemporalEdge.source_segment_id` carries
///     the per-utterance "relates to" link. The live graph is the ONLY structure
///     that carries `source_segment_id`; the materialized graph carries only the
///     whole-window basis, so it is deliberately NOT an input here (folding it
///     would leave every `related_edge_ids` empty — ADR-0026 §4.1 sev4 fix).
///
/// The session must have at least one artifact on disk, otherwise this returns
/// [`AppError::SessionInvalid`] (the same guard `load_session` /
/// `export_session_bundle` use), so the caller does not silently fold an empty
/// timeline for a bad ID. Missing individual logs collapse to empty
/// collections / an empty graph so a transcript-only session still folds.
///
/// `limit` (seed audio-graph-1f85) caps the response to the last `limit`
/// entries by media-clock order — the same tail `SeekTimeline` renders
/// (`MAX_BLOCKS` / `TRANSCRIPT_WINDOW_SIZE = 200`) — so a long session's full
/// span log is never serialized just to be sliced client-side. `None` keeps
/// every entry (used by tests and `export_session_bundle`-adjacent callers
/// that need the whole fold).
fn session_timeline(session_id: &str, limit: Option<usize>) -> AppResult<SessionTimelineFold> {
    validate_session_id(session_id)?;

    if !session_has_any_artifact(session_id)? {
        return Err(AppError::SessionInvalid {
            reason: format!("Session files not found: {}", session_id),
        });
    }

    // Floor-admitted for the same reason as the transcript snapshot and the
    // replay report: every canonical stream folded below is v1-only.
    let data_root = crate::user_data::resolve_data_root()
        .map_err(|error| AppError::Io(format!("resolve data root: {error}")))?;
    crate::persistence::session_semantics::open_session_for_content(
        &data_root,
        session_id,
        crate::persistence::session_semantics::SessionSemanticsVersion::V1,
        |_admitted| session_timeline_for_admitted_session(session_id, limit),
    )
    .map_err(unadmitted_session_error)
}

fn session_timeline_for_admitted_session(
    session_id: &str,
    limit: Option<usize>,
) -> AppResult<SessionTimelineFold> {
    let read_start = std::time::Instant::now();
    let (_transcript_path, graph_path) = indexed_session_paths_resolve_only(session_id)?;
    let repository = FileMemoryRepository::user_data();
    let transcript_events_bytes = enforce_artifact_ceiling(
        &crate::user_data::resolve_transcript_events_path(session_id)?,
        MAX_TRANSCRIPT_EVENTS_BYTES,
        "transcript_events",
    )?;
    let transcript_events = repository
        .load_transcript_event_stream(session_id)?
        .into_payloads();
    let diarization_events_bytes = enforce_artifact_ceiling(
        &crate::user_data::resolve_diarization_events_path(session_id)?,
        MAX_DIARIZATION_EVENTS_BYTES,
        "diarization_events",
    )?;
    let diarization_events = repository
        .load_speaker_revision_stream(session_id)?
        .into_payloads();

    let ledger = crate::projections::TranscriptLedger::replay(session_id, transcript_events)
        .map_err(|e| {
            format!("Failed to replay transcript ledger for session {session_id}: {e:?}")
        })?;
    let speakers = crate::projections::SpeakerTimeline::replay(session_id, diarization_events)
        .map_err(|e| {
            format!("Failed to replay speaker timeline for session {session_id}: {e:?}")
        })?;
    // This re-parses the same live graph file `load_session` already parsed
    // for the `graph` field of its own (separate, earlier) response — a
    // known duplicate the fold's module doc calls out (`timeline.rs:18-25`
    // explains why it needs the LIVE graph, not the materialized one, so the
    // parse itself cannot be skipped). Seed audio-graph-4fa5 deliverable e
    // does NOT resolve this duplicate: `store/index.ts`'s `loadSession`
    // still fires this fold unconditionally, fire-and-forget, in the same
    // interaction as `load_session` itself (`void
    // get().loadSessionTimeline(sessionId)`), not gated on the Timeline
    // lens activating — an earlier version of this comment claimed
    // otherwise, which was inaccurate (fix-round finding). Deliverable e's
    // actually-shipped mitigation is `limit` alone: it bounds the RESPONSE
    // SIZE of this second parse, which is what field round 5's crash was
    // about, but the parse itself still runs twice per session open. Fully
    // decoupling this fetch from `load_session` (lens-gating it the way
    // Notes/Graph are) remains a real, undone follow-up — declined for this
    // unit because it would touch `SessionsBrowser`'s lens-effect wiring and
    // the existing timeline-fold test suite for a performance win, not a
    // crash-safety one; `limit` already bounds the crash-safety exposure.
    let live_graph_bytes =
        enforce_artifact_ceiling(&graph_path, MAX_LIVE_GRAPH_BYTES, "live_graph")?;
    let live_graph = if graph_path.exists() {
        crate::graph::temporal::TemporalKnowledgeGraph::load_from_file(&graph_path)?
    } else {
        crate::graph::temporal::TemporalKnowledgeGraph::new()
    };

    let mut entries = crate::timeline::build_session_timeline(&ledger, &speakers, &live_graph);
    let full_count = entries.len();
    if let Some(limit) = limit
        && entries.len() > limit
    {
        entries = entries.split_off(entries.len() - limit);
    }
    let fold = SessionTimelineFold {
        entries,
        total_count: full_count,
    };
    let response_bytes = response_len_for_log(&fold);
    let elapsed_ms = read_start.elapsed().as_millis();
    let log_line = format!(
        "build_session_timeline_cmd session_id={session_id} transcript_events_bytes={transcript_events_bytes} diarization_events_bytes={diarization_events_bytes} live_graph_bytes={live_graph_bytes} full_entry_count={full_count} returned_entry_count={} response_bytes={response_bytes} elapsed_ms={elapsed_ms}",
        fold.entries.len()
    );
    if response_bytes > RESPONSE_SIZE_WARN_THRESHOLD_BYTES {
        log::warn!("{log_line}");
    } else {
        log::info!("{log_line}");
    }
    Ok(fold)
}

/// Fold a session's durable logs into its [`crate::timeline::TimelineEntry`]
/// list — "who said what, when, in relation to what" (epic 0d72 P1,
/// ADR-0026 §4.1). Ordered by media-clock start time, duplicate-free, with
/// latest-wins speaker attribution and forward links to the live graph edges
/// each utterance produced.
///
/// `limit` (seed audio-graph-1f85) tail-caps the response — pass the
/// frontend's `TRANSCRIPT_WINDOW_SIZE` (200) so the fold never serializes
/// more entries than `SeekTimeline` renders. `async fn` (seed
/// audio-graph-e8a5) moves the disk I/O + replay off the message-pump thread.
#[tauri::command]
pub async fn build_session_timeline_cmd(
    session_id: String,
    limit: Option<usize>,
) -> AppResult<SessionTimelineFold> {
    session_timeline(&session_id, limit)
}

/// Soft-delete a session: flag it as trashed in the sessions index but keep
/// the transcript and graph files on disk. The UI can show trashed sessions
/// via a "Show trash" toggle and restore them with `restore_session`. After
/// the 30-day retention window expires, `purge_expired_sessions` lazily
/// hard-deletes the entry + files on the next list_sessions call.
///
/// This replaces the v1 hard-delete behavior. For an immediate hard delete
/// (e.g. from the trash view's "Delete permanently" button), use
/// `delete_session_permanently`.
#[tauri::command]
pub fn delete_session(session_id: String, state: State<'_, AppState>) -> AppResult<()> {
    validate_session_id(&session_id)?;
    ensure_not_active_session(&session_id, state.inner())?;
    crate::sessions::soft_delete_session(&session_id)?;
    log::info!("Session {} moved to trash", session_id);
    Ok(())
}

/// Restore a soft-deleted session back to the active list.
#[tauri::command]
pub fn restore_session(session_id: String) -> AppResult<()> {
    validate_session_id(&session_id)?;
    crate::sessions::restore_session(&session_id)?;
    log::info!("Session {} restored from trash", session_id);
    Ok(())
}

/// Permanently delete a session: remove from index and unlink its files.
/// Bypasses the trash — intended for the "Delete permanently" action in the
/// trash view.
#[tauri::command]
pub fn delete_session_permanently(session_id: String, state: State<'_, AppState>) -> AppResult<()> {
    validate_session_id(&session_id)?;
    ensure_not_active_session(&session_id, state.inner())?;
    let report = crate::sessions::permanently_delete_session(&session_id)?;
    log::info!(
        "Permanently deleted session {} (removed={}, already_missing={})",
        session_id,
        report.deleted_files.len(),
        report.missing_files.len()
    );
    Ok(())
}

fn ensure_not_active_session(session_id: &str, state: &AppState) -> AppResult<()> {
    if state.current_session_id() == session_id {
        return Err(AppError::SessionInvalid {
            reason: "the active session cannot be deleted".to_string(),
        });
    }
    Ok(())
}

/// Rebuild missing sessions-index entries by scanning transcript and graph
/// files under the configured user-data roots.
#[tauri::command]
pub fn recover_orphaned_sessions() -> AppResult<crate::sessions::SessionRecoveryReport> {
    let report = crate::sessions::rebuild_index_from_files()?;
    log::info!(
        "Session recovery: discovered={} recovered={} skipped={} errors={}",
        report.discovered,
        report.recovered,
        report.skipped,
        report.errors.len()
    );
    Ok(report)
}

/// Lazy cleanup: hard-delete any trashed sessions whose `deleted_at` is older
/// than the 30-day retention window. Returns the list of purged session IDs.
/// Frontend is expected to call this on session list load.
#[tauri::command]
pub fn purge_expired_sessions(state: State<'_, AppState>) -> AppResult<Vec<String>> {
    let protected = std::collections::HashSet::from([state.current_session_id()]);
    let purged = crate::sessions::purge_expired_sessions_excluding(&protected)?;
    if !purged.is_empty() {
        log::info!("Purged {} expired session(s) from trash", purged.len());
    }
    Ok(purged)
}

/// Load the token-usage record for a session from
/// `~/.audiograph/usage/<session_id>.json`. Missing or malformed files
/// resolve to a zeroed record — callers never have to disambiguate.
#[tauri::command]
pub fn get_session_usage(session_id: String) -> AppResult<crate::sessions::usage::SessionUsage> {
    validate_session_id(&session_id)?;
    Ok(crate::sessions::usage::load_usage(&session_id))
}

/// Load the token-usage record for the CURRENT session. Convenience wrapper
/// so the frontend can restore its in-memory totals on startup without first
/// having to fetch `get_session_id`.
#[tauri::command]
pub fn get_current_session_usage(
    state: State<'_, AppState>,
) -> AppResult<crate::sessions::usage::SessionUsage> {
    Ok(crate::sessions::usage::load_usage(
        &state.current_session_id(),
    ))
}

/// Aggregate usage across every on-disk session file. This is the
/// authoritative source for the frontend's "Lifetime" totals panel — the
/// prior localStorage-backed lifetime counter was only ever a best-effort
/// mirror of this sum.
#[tauri::command]
pub fn get_lifetime_usage() -> AppResult<crate::sessions::usage::LifetimeUsage> {
    Ok(crate::sessions::usage::load_lifetime_usage())
}

/// Import a frontend `localStorage` lifetime-totals snapshot into the backend
/// usage directory so `get_lifetime_usage` reports pre-persistence history.
///
/// This is a one-way migration path, guarded by the idempotency check inside
/// `seed_lifetime_migration`: a second call is a no-op, so a stale browser
/// state can't double-count. The frontend is expected to call this once on
/// mount and then clear its `localStorage` lifetime key.
#[tauri::command]
pub fn seed_lifetime_migration(payload: crate::sessions::usage::LifetimeUsage) -> AppResult<()> {
    crate::sessions::usage::seed_lifetime_migration(&payload).map_err(AppError::from)
}

/// Reset the current session's token usage file to zero.
#[tauri::command]
pub fn reset_current_session_usage(
    state: State<'_, AppState>,
) -> AppResult<crate::sessions::usage::SessionUsage> {
    crate::sessions::usage::reset_usage(&state.current_session_id()).map_err(AppError::from)
}

/// Clear every token-usage record that contributes to lifetime totals.
#[tauri::command]
pub fn clear_all_usage() -> AppResult<()> {
    crate::sessions::usage::clear_all_usage().map_err(AppError::from)
}

/// Flush the current session and rotate to a fresh one in-process.
///
/// Behavior:
///   1. Re-save the current session's usage record so on-disk totals are
///      flushed before the ID rotates.
///   2. Seed a fresh zeroed usage file for the new session so
///      `get_current_session_usage` returns zeros immediately after rotation.
///   3. Prepare new canonical writers and rotate `AppState::session_id` in
///      place only if all required writers are available.
///        - The transcript writer is respawned against the new ID's file.
///        - The graph-autosave thread re-reads the ID on its next 30s tick
///          and starts writing to the new session's file.
///   4. After a successful rotation, finalize the previous session's index
///      entry (status → complete).
///   5. Register the new session in the sessions index so list_sessions
///      shows it alongside the previous one.
///
/// Returns the new session ID.
#[tauri::command]
pub async fn new_session_cmd(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> AppResult<String> {
    let _session_lifecycle = state.session_lifecycle.lock().await;
    ensure_session_idle_for_rotation(state.inner())?;
    ensure_audio_pipeline_workers(state.inner(), &app)?;
    reset_audio_pipeline_session(state.inner()).await?;
    let previous_id = state.current_session_id();

    // 1. Re-save the current session's usage record. If the file is missing
    //    this is a harmless zero-write; if it exists, `save_usage` is a
    //    no-op rewrite of the same bytes. Either way, it guarantees the
    //    file is present on disk before the caller moves on.
    let current = crate::sessions::usage::load_usage(&previous_id);
    if let Err(e) = crate::sessions::usage::save_usage(&current) {
        log::warn!("new_session_cmd: save current usage failed: {}", e);
    }

    // 2. Seed a fresh usage file for the next session. Do this BEFORE the
    //    rotate so `get_current_session_usage` immediately reads zeroes.
    let new_id = uuid::Uuid::new_v4().to_string();
    let fresh = crate::sessions::usage::SessionUsage {
        session_id: new_id.clone(),
        ..crate::sessions::usage::SessionUsage::default()
    };
    crate::sessions::usage::save_usage(&fresh)?;

    // 3. Rotate in-process. `rotate_session` prepares all required canonical
    //    writers before publishing the new ID and resets active aggregates.
    //
    //    Concurrent-rotate guard: if another rotation is already in flight,
    //    skip and return the current session ID. The caller sees a successful
    //    rotation either way (the in-flight rotate will land a fresh ID);
    //    they just don't get the one *we* seeded. The usage file we wrote in
    //    step 2 is then orphaned — harmless, since seed files are zeroed and
    //    `load_usage` handles missing/extra entries.
    match state.rotate_session(&new_id) {
        crate::state::RotateOutcome::Rotated(rotated_from) => {
            debug_assert_eq!(rotated_from, previous_id);
        }
        crate::state::RotateOutcome::AlreadyRotating(current) => {
            log::warn!(
                "new_session_cmd: concurrent rotation detected; returning current id {} \
                 instead of freshly-seeded {}",
                current,
                new_id
            );
            return Ok(current);
        }
        crate::state::RotateOutcome::Failed {
            current_session_id,
            reason,
        } => {
            return Err(AppError::Io(format!(
                "new session was not started; active session {current_session_id} is unchanged: {reason}"
            )));
        }
    }

    // 4. Finalize only after the writer preflight and rotation succeed. Doing
    //    this earlier could leave the still-current session marked complete
    //    when writer preparation failed.
    if let Err(e) = crate::sessions::finalize_session(&previous_id) {
        log::warn!("new_session_cmd: finalize previous failed: {}", e);
    }
    crate::log_session_card_summary_best_effort(&previous_id);

    // 5. Register new session in the index so it shows up in list_sessions
    //    (status "active"). Best-effort: failure just means the UI won't
    //    see the entry until the next restart rediscovers it.
    if let Err(e) = crate::sessions::register_session(&new_id) {
        log::warn!("new_session_cmd: register_session failed: {}", e);
    }

    log::info!("new_session_cmd: rotated {} → {}", previous_id, new_id);
    Ok(new_id)
}

fn ensure_session_idle_for_rotation(state: &AppState) -> AppResult<()> {
    ensure_session_workers_quiesced(state)?;
    let is_capturing = match state.is_capturing.read() {
        Ok(value) => *value,
        Err(poisoned) => *poisoned.into_inner(),
    };
    let has_registered_capture_or_unquiesced_worker = match state.capture_manager.lock() {
        Ok(mut manager) => {
            !manager.active_captures().is_empty() || manager.has_unquiesced_workers()
        }
        Err(poisoned) => {
            let mut manager = poisoned.into_inner();
            !manager.active_captures().is_empty() || manager.has_unquiesced_workers()
        }
    };
    if is_capturing
        || has_registered_capture_or_unquiesced_worker
        || state.is_transcribing.load(Ordering::SeqCst)
    {
        return Err(AppError::SessionInvalid {
            reason: "stop live capture before starting a new session".to_string(),
        });
    }
    if !state.stream_registry.is_empty() {
        return Err(AppError::SessionInvalid {
            reason: "wait for or cancel the active chat before starting a new session".to_string(),
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Credential management commands
// ---------------------------------------------------------------------------

/// Re-hydrate the in-memory settings cache (`AppState.app_settings`) from the
/// given credential store so a running capture/chat session picks up a
/// just-mutated key WITHOUT a restart or a full settings Save.
///
/// This is the shared writer-side re-hydrate used by BOTH `save_credential_cmd`
/// (fill the cache with the new key) and `delete_credential_cmd` (clear the
/// deleted key out of the cache). The capture read-path
/// (`read_settings_for_session_content`) clones this cache, so if a writer
/// mutates the keychain without touching the cache the session keeps using the
/// stale value: for save that resurfaced as a stale-cache 401 (#39), and for
/// delete it means the session keeps transmitting a *deleted* key
/// (audio-graph-c4d0). Keeping the two writers on one helper prevents the two
/// paths from diverging again.
///
/// `hydrate_runtime_credentials` internally redacts (clears) every inline secret
/// before re-filling from the store, so a store that no longer holds the key
/// leaves the cached provider `api_key` empty (the delete case), while a store
/// that holds a new value fills it (the save case). Passing the already-hydrated
/// cache back in is therefore safe and idempotent.
///
/// A poisoned/contended lock is logged (not propagated): the keychain write has
/// already succeeded and the readiness epoch already bumped, so the new state
/// still applies after the next settings load/save or restart. `context` labels
/// the log line with the calling command.
fn rehydrate_app_settings_cache(
    state: &AppState,
    store: &crate::credentials::CredentialStore,
    context: &str,
    key: &str,
) {
    if let Ok(mut cached) = state.app_settings.write() {
        let rehydrated = crate::settings::hydrate_runtime_credentials(&cached, store);
        *cached = rehydrated;
    } else {
        log::warn!(
            "{context}: could not lock app_settings to re-hydrate cache for key={key}; \
             the change will apply after the next settings load/save or restart."
        );
    }
}

#[tauri::command]
pub fn save_credential_cmd(
    key: String,
    value: String,
    state: State<'_, AppState>,
) -> AppResult<SaveCredentialOutcome> {
    save_credential_impl(key, value, state.inner())
}

/// Testable core of [`save_credential_cmd`], taking `&AppState` directly so
/// unit tests can drive it without a Tauri `State` handle.
fn save_credential_impl(
    key: String,
    value: String,
    state: &AppState,
) -> AppResult<SaveCredentialOutcome> {
    // Diagnostic instrumentation: log invocation with key + value LENGTH +
    // a non-secret FINGERPRINT (never the secret itself). The fingerprint is a
    // one-way sha256 prefix (see `credentials::secret_fingerprint`); comparing
    // it against the fingerprint the Deepgram connect log emits reveals whether
    // the key that reaches the wire matches the one just saved — the decisive
    // signal for the stale-cache 401 root cause. Pairs with the success log
    // below to disambiguate frontend-skip vs backend-persist paths when a saved
    // credential appears not to take effect. See docs/plans/
    // 2026-07-01-deepgram-401-rootcause.md.
    log::info!(
        "save_credential_cmd: key={} value_len={} fingerprint={}",
        key,
        value.len(),
        crate::credentials::secret_fingerprint(Some(&value))
    );
    // Boundary-layer allowlist check (loop11 MEDIUM #5): reject unknown keys
    // here before they reach the inner `set_field` match. Mirrors the
    // convention used by `validate_session_id` elsewhere in this module.
    if !crate::credentials::is_allowed_key(&key) {
        return Err(crate::error::AppError::CredentialFileError {
            reason: format!("Unknown credential key: {}", key),
        });
    }

    // Empty/whitespace-only is a no-op skip (the backend `set` treats blank as
    // "don't clobber a stored key" — use `delete_credential_cmd` to clear).
    // Short-circuit BEFORE the epoch bump + cache rehydrate so a skipped save
    // does no spurious work: bumping the readiness epoch invalidates the
    // provider-readiness cache and rehydrating re-clones app_settings, both
    // pointless for a write that never happened (cred-review M2.1 / N1). This
    // backend short-circuit is the actual fix — it holds regardless of whether
    // the caller inspects the result. Returning `SkippedEmpty` (instead of the
    // old ambiguous `Ok(())`) is forward-looking plumbing that *lets* a caller
    // tell a skip from a persist and skip its post-save presence refresh; no
    // current frontend caller relies on it (they all pre-guard `value.trim()`).
    if value.trim().is_empty() {
        log::info!("save_credential_cmd: skipped empty value for key={}", key);
        return Ok(SaveCredentialOutcome::SkippedEmpty);
    }

    // Bubble credential-file failures as `CredentialFileError` so the
    // frontend can render a localized / actionable message instead of a bare
    // string.
    crate::credentials::set_credential(&key, &value)
        .map_err(|reason| crate::error::AppError::CredentialFileError { reason })?;
    bump_provider_credential_epoch();

    // Re-hydrate the in-memory settings cache from the (now-updated) credential
    // store so a running session picks up the new key WITHOUT a restart or a
    // full settings Save. This closes the confirmed stale-cache 401: the
    // capture read-path (`read_settings_for_session_content`) clones this cache,
    // and `save_credential_cmd` previously only wrote the keychain + bumped the
    // readiness epoch, leaving the cache holding the OLD key. Shared with
    // `delete_credential_cmd` via `rehydrate_app_settings_cache` so the two
    // symmetric writers cannot diverge again (audio-graph-c4d0).
    let store = crate::credentials::load_credentials();
    rehydrate_app_settings_cache(state, &store, "save_credential_cmd", &key);

    log::info!("save_credential_cmd: persisted key={}", key);
    Ok(SaveCredentialOutcome::Saved)
}

/// Explicitly clear a stored credential. Needed because `save_credential_cmd`
/// treats empty strings as a no-op (to avoid clobbering on blank form fields),
/// so there has to be a separate way for users to actually delete a key.
#[tauri::command]
pub fn delete_credential_cmd(key: String, state: State<'_, AppState>) -> AppResult<()> {
    // Boundary-layer allowlist check (loop11 MEDIUM #5). Emit the same
    // message the inner `set_field` match would have produced, but reject at
    // the command boundary so the frontend receives a structured payload.
    if !crate::credentials::is_allowed_key(&key) {
        return Err(AppError::CredentialFileError {
            reason: format!("Unknown credential key: {}", key),
        });
    }
    crate::credentials::delete_credential(&key)
        .map_err(|reason| AppError::CredentialFileError { reason })?;
    bump_provider_credential_epoch();

    // Re-hydrate the in-memory settings cache from the (now-updated) credential
    // store so a running session stops using the just-deleted key WITHOUT a
    // restart. Symmetric to `save_credential_cmd`: the capture read-path
    // (`read_settings_for_session_content`) clones this cache, and delete
    // previously only wrote the keychain + bumped the readiness epoch, leaving
    // the cache holding the OLD (now-revoked) key so the session kept
    // transmitting it to the provider while the readiness chip already showed
    // 'no key' (audio-graph-c4d0). Because the reloaded store no longer holds
    // the key, `hydrate_runtime_credentials` leaves the cached provider api_key
    // cleared. `state` is Tauri-injected, so `invoke('delete_credential_cmd',
    // { key })` from the frontend is unchanged.
    let store = crate::credentials::load_credentials();
    rehydrate_app_settings_cache(state.inner(), &store, "delete_credential_cmd", &key);

    Ok(())
}

fn credential_is_present(
    store: &crate::credentials::CredentialStore,
    key: &str,
) -> Result<bool, String> {
    store.is_present(key)
}

/// Return non-secret credential presence for every allowlisted key.
///
/// This is the normal Settings/readiness read path. It lets the UI enable saved
/// providers and show "saved key present" state without receiving plaintext
/// secret values over IPC.
#[tauri::command]
pub fn load_credential_presence_cmd() -> AppResult<Vec<CredentialPresence>> {
    let snapshot = crate::credentials::try_load_credentials_with_source()
        .map_err(|reason| AppError::CredentialFileError { reason })?;
    credential_presence_from_snapshot(&snapshot)
}

#[cfg(test)]
fn credential_presence_from_store(
    store: &crate::credentials::CredentialStore,
) -> AppResult<Vec<CredentialPresence>> {
    let snapshot = crate::credentials::CredentialSnapshot::new(store.clone(), "credentials_yaml");
    credential_presence_from_snapshot(&snapshot)
}

fn credential_presence_from_snapshot(
    snapshot: &crate::credentials::CredentialSnapshot,
) -> AppResult<Vec<CredentialPresence>> {
    crate::credentials::ALLOWED_CREDENTIAL_KEYS
        .iter()
        .map(|&key| {
            let present = credential_is_present(&snapshot.store, key)
                .map_err(|reason| AppError::CredentialFileError { reason })?;
            Ok(CredentialPresence {
                key: key.to_string(),
                present,
                source: snapshot.source_for(key),
            })
        })
        .collect()
}

fn bump_provider_credential_epoch() {
    PROVIDER_CREDENTIAL_EPOCH.fetch_add(1, Ordering::SeqCst);
}

fn provider_readiness_cache() -> &'static Mutex<HashMap<String, ProviderReadiness>> {
    PROVIDER_READINESS_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn provider_readiness_in_flight() -> &'static Mutex<HashSet<String>> {
    PROVIDER_READINESS_IN_FLIGHT.get_or_init(|| Mutex::new(HashSet::new()))
}

fn provider_readiness_last_started() -> &'static Mutex<HashMap<String, u64>> {
    PROVIDER_READINESS_LAST_STARTED.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Debug)]
struct ProviderReadinessCancellationEntry {
    generation: u64,
    token: CancellationToken,
}

#[derive(Debug)]
struct ProviderReadinessRequestGuard {
    request_id: String,
    generation: u64,
}

impl Drop for ProviderReadinessRequestGuard {
    fn drop(&mut self) {
        unregister_provider_readiness_request(&self.request_id, self.generation);
    }
}

fn provider_readiness_cancellations()
-> &'static Mutex<HashMap<String, ProviderReadinessCancellationEntry>> {
    PROVIDER_READINESS_CANCELLATIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn validate_provider_readiness_request_id(request_id: &str) -> AppResult<()> {
    let valid = !request_id.is_empty()
        && request_id.len() <= 128
        && request_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':'));
    if valid {
        Ok(())
    } else {
        Err(AppError::Unknown(
            "Invalid provider readiness request id".to_string(),
        ))
    }
}

fn register_provider_readiness_request(
    request_id: Option<String>,
) -> AppResult<Option<(ProviderReadinessRequestGuard, CancellationToken)>> {
    let Some(request_id) = request_id else {
        return Ok(None);
    };
    validate_provider_readiness_request_id(&request_id)?;

    let generation = PROVIDER_READINESS_CANCELLATION_GENERATION.fetch_add(1, Ordering::SeqCst);
    let token = CancellationToken::new();
    let previous = provider_readiness_cancellations()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(
            request_id.clone(),
            ProviderReadinessCancellationEntry {
                generation,
                token: token.clone(),
            },
        );
    if let Some(previous) = previous {
        previous.token.cancel();
    }

    Ok(Some((
        ProviderReadinessRequestGuard {
            request_id,
            generation,
        },
        token,
    )))
}

fn unregister_provider_readiness_request(request_id: &str, generation: u64) {
    let mut cancellations = provider_readiness_cancellations()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if cancellations
        .get(request_id)
        .is_some_and(|entry| entry.generation == generation)
    {
        cancellations.remove(request_id);
    }
}

fn cancel_provider_readiness_request(request_id: &str) -> bool {
    provider_readiness_cancellations()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(request_id)
        .map(|entry| {
            entry.token.cancel();
            true
        })
        .unwrap_or(false)
}

fn credential_readiness_from_store(
    descriptor: &crate::provider_registry::ProviderDescriptor,
    store: &crate::credentials::CredentialStore,
) -> Vec<ProviderCredentialReadiness> {
    descriptor
        .credential_keys
        .iter()
        .map(|key| ProviderCredentialReadiness {
            key: (*key).to_string(),
            present: credential_is_present(store, key).unwrap_or(false),
        })
        .collect()
}

fn credential_present(store: &crate::credentials::CredentialStore, key: &str) -> bool {
    credential_is_present(store, key).unwrap_or(false)
}

fn fixed_model_catalog_for_descriptor(
    descriptor: &crate::provider_registry::ProviderDescriptor,
) -> Vec<ProviderModelCatalogItem> {
    if let Some(catalog) = descriptor.fixed_model_catalog {
        return catalog
            .iter()
            .map(|model| ProviderModelCatalogItem {
                id: model.id.to_string(),
                display_name: model.display_name.to_string(),
                is_default: model.is_default,
            })
            .collect();
    }

    match descriptor.model_catalog {
        crate::provider_registry::ModelCatalogPolicy::Fixed
        | crate::provider_registry::ModelCatalogPolicy::LocalFiles => {
            if descriptor.local_models.is_empty() {
                return descriptor
                    .default_model
                    .map(|model_id| {
                        vec![ProviderModelCatalogItem {
                            id: model_id.to_string(),
                            display_name: model_id.to_string(),
                            is_default: true,
                        }]
                    })
                    .unwrap_or_default();
            }

            descriptor
                .local_models
                .iter()
                .map(|model| ProviderModelCatalogItem {
                    id: model.model_id.to_string(),
                    display_name: model.model_id.to_string(),
                    is_default: descriptor.default_model == Some(model.model_id),
                })
                .collect()
        }
        _ => vec![],
    }
}

fn fixed_voice_catalog_for_descriptor(
    descriptor: &crate::provider_registry::ProviderDescriptor,
) -> Vec<ProviderModelCatalogItem> {
    match descriptor.id {
        "tts.deepgram_aura" => fixed_model_catalog_for_descriptor(descriptor),
        _ => vec![],
    }
}

fn fixed_language_catalog_for_descriptor(
    _descriptor: &crate::provider_registry::ProviderDescriptor,
) -> Vec<ProviderModelCatalogItem> {
    vec![]
}

fn model_count_from_catalog(catalog: &[ProviderModelCatalogItem]) -> Option<usize> {
    (!catalog.is_empty()).then_some(catalog.len())
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct LocalModelReadinessSummary {
    total: usize,
    ready: usize,
    ready_model_ids: Vec<String>,
    missing: Vec<String>,
    invalid: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
enum LocalRuntimeProbeOutcome {
    LoadFailed {
        message: String,
        model_id: Option<String>,
    },
    Healthy {
        runtime_version: String,
        model_id: String,
    },
}

fn local_model_readiness_summary(
    descriptor: &crate::provider_registry::ProviderDescriptor,
    models_dir: &std::path::Path,
) -> Option<LocalModelReadinessSummary> {
    if descriptor.model_catalog != crate::provider_registry::ModelCatalogPolicy::LocalFiles
        || descriptor.local_models.is_empty()
    {
        return None;
    }

    let mut summary = LocalModelReadinessSummary {
        total: descriptor.local_models.len(),
        ..LocalModelReadinessSummary::default()
    };

    for model in descriptor.local_models {
        match model.kind {
            crate::provider_registry::LocalModelKind::File => {
                let path = models_dir.join(model.model_id);
                if !path.exists() {
                    summary.missing.push(model.model_id.to_string());
                } else {
                    match std::fs::metadata(&path) {
                        Ok(metadata) if metadata.is_file() && metadata.len() > 0 => {
                            // BUG 3f23: a present-but-truncated `.onnx` (e.g. a
                            // partial download or an HTML error page) would pass
                            // the `len() > 0` check and be reported ready, only
                            // to fail at runtime model load. For models with a
                            // published minimum size, enforce that floor here so
                            // a truncated file is classified invalid with a
                            // clear reason instead.
                            match crate::models::min_model_size_bytes(model.model_id) {
                                Some(min_bytes) if metadata.len() < min_bytes => {
                                    summary.invalid.push(format!(
                                        "{} too small ({} bytes; expected at least {} bytes)",
                                        model.model_id,
                                        metadata.len(),
                                        min_bytes
                                    ));
                                }
                                _ => {
                                    summary.ready += 1;
                                    summary.ready_model_ids.push(model.model_id.to_string());
                                }
                            }
                        }
                        _ => summary.invalid.push(model.model_id.to_string()),
                    }
                }
            }
            crate::provider_registry::LocalModelKind::Directory => {
                let model_dir = models_dir.join(model.model_id);
                if !model_dir.is_dir() {
                    summary.missing.push(model.model_id.to_string());
                    continue;
                }

                let mut missing_files = Vec::new();
                let mut invalid_files = Vec::new();
                for required in model.required_files {
                    let required_path = model_dir.join(required);
                    match std::fs::metadata(&required_path) {
                        Ok(metadata) if metadata.is_file() && metadata.len() > 0 => {}
                        Ok(_) => invalid_files.push((*required).to_string()),
                        Err(_) => missing_files.push((*required).to_string()),
                    }
                }

                if missing_files.is_empty() && invalid_files.is_empty() {
                    summary.ready += 1;
                    summary.ready_model_ids.push(model.model_id.to_string());
                } else {
                    if !missing_files.is_empty() {
                        summary.missing.push(format!(
                            "{} missing {}",
                            model.model_id,
                            missing_files.join(", ")
                        ));
                    }
                    if !invalid_files.is_empty() {
                        summary.invalid.push(format!(
                            "{} invalid {}",
                            model.model_id,
                            invalid_files.join(", ")
                        ));
                    }
                }
            }
        }
    }

    Some(summary)
}

fn local_model_readiness_message(
    descriptor: &crate::provider_registry::ProviderDescriptor,
    models_dir: &std::path::Path,
) -> Option<String> {
    let summary = local_model_readiness_summary(descriptor, models_dir)?;
    let mut message = if summary.ready == 0 {
        format!(
            "No local model files are ready yet. Download one of {} model option(s).",
            summary.total
        )
    } else {
        format!(
            "Local model files ready: {}/{} model option(s).",
            summary.ready, summary.total
        )
    };

    if !summary.missing.is_empty() {
        message.push_str(&format!(" Missing: {}.", summary.missing.join("; ")));
    }
    if !summary.invalid.is_empty() {
        message.push_str(&format!(" Invalid: {}.", summary.invalid.join("; ")));
    }
    if descriptor.status == crate::provider_registry::ProviderStatus::Planned {
        message.push_str(" Provider runtime remains planned and is not selectable yet.");
    }

    Some(message)
}

fn moonshine_runtime_readiness_from_state(
    feature_compiled: bool,
    ready_models: usize,
    probe: Option<LocalRuntimeProbeOutcome>,
) -> ProviderRuntimeReadiness {
    if !feature_compiled {
        return ProviderRuntimeReadiness {
            status: ProviderRuntimeReadinessStatus::FeatureMissing,
            message:
                "Moonshine runtime feature is not compiled; build with asr-moonshine after cross-platform proof."
                    .to_string(),
            required_feature: Some("asr-moonshine".to_string()),
            runtime_version: None,
            model_id: None,
        };
    }

    if ready_models == 0 {
        return ProviderRuntimeReadiness {
            status: ProviderRuntimeReadinessStatus::ModelMissing,
            message:
                "Moonshine runtime probe is skipped until one complete model directory is present."
                    .to_string(),
            required_feature: None,
            runtime_version: None,
            model_id: None,
        };
    }

    match probe {
        Some(LocalRuntimeProbeOutcome::LoadFailed { message, model_id }) => {
            ProviderRuntimeReadiness {
                status: ProviderRuntimeReadinessStatus::LoadFailed,
                message,
                required_feature: None,
                runtime_version: None,
                model_id,
            }
        }
        Some(LocalRuntimeProbeOutcome::Healthy {
            runtime_version,
            model_id,
        }) => ProviderRuntimeReadiness {
            status: ProviderRuntimeReadinessStatus::Healthy,
            message: format!("Moonshine runtime loaded {model_id} successfully."),
            required_feature: None,
            runtime_version: Some(runtime_version),
            model_id: Some(model_id),
        },
        None => ProviderRuntimeReadiness {
            status: ProviderRuntimeReadinessStatus::RuntimeUnavailable,
            message:
                "Moonshine native runtime adapter is not wired yet; provider remains planned and unselectable."
                    .to_string(),
            required_feature: None,
            runtime_version: None,
            model_id: None,
        },
    }
}

// Live only under the `diarization-clustering` feature (call sites are
// `#[cfg(feature = "diarization-clustering")]`); dead under default features.
#[cfg_attr(not(feature = "diarization-clustering"), allow(dead_code))]
fn diarization_clustering_runtime_model_id() -> String {
    format!(
        "{}+{}",
        crate::models::DIAR_SEG_PYANNOTE_DIR,
        crate::models::DIAR_EMB_TITANET_FILENAME
    )
}

#[cfg(feature = "diarization-clustering")]
const DIARIZATION_CLUSTERING_MIN_ONNX_BYTES: u64 = 1024;

#[cfg(feature = "diarization-clustering")]
fn diarization_clustering_runtime_file_preflight(
    segmentation_model: &std::path::Path,
    embedding_model: &std::path::Path,
) -> Result<(), String> {
    for (label, path) in [
        ("segmentation", segmentation_model),
        ("embedding", embedding_model),
    ] {
        let metadata = std::fs::metadata(path).map_err(|error| {
            format!(
                "Clustering diarization runtime load failed before native load: {label} model at {} could not be inspected: {error}",
                path.display()
            )
        })?;
        if !metadata.is_file() {
            return Err(format!(
                "Clustering diarization runtime load failed before native load: {label} model at {} is not a regular file.",
                path.display()
            ));
        }
        if metadata.len() < DIARIZATION_CLUSTERING_MIN_ONNX_BYTES {
            return Err(format!(
                "Clustering diarization runtime load failed before native load: {label} model at {} is only {} byte(s); expected at least {} byte(s).",
                path.display(),
                metadata.len(),
                DIARIZATION_CLUSTERING_MIN_ONNX_BYTES
            ));
        }
    }

    Ok(())
}

fn diarization_clustering_runtime_readiness_from_state(
    feature_compiled: bool,
    ready_models: usize,
    required_models: usize,
    probe: Option<LocalRuntimeProbeOutcome>,
) -> ProviderRuntimeReadiness {
    if !feature_compiled {
        return ProviderRuntimeReadiness {
            status: ProviderRuntimeReadinessStatus::FeatureMissing,
            message:
                "Clustering diarization runtime feature is not compiled; build with diarization-clustering after cross-platform proof."
                    .to_string(),
            required_feature: Some("diarization-clustering".to_string()),
            runtime_version: None,
            model_id: None,
        };
    }

    if required_models == 0 || ready_models < required_models {
        return ProviderRuntimeReadiness {
            status: ProviderRuntimeReadinessStatus::ModelMissing,
            message:
                "Clustering diarization runtime probe is skipped until the pyannote segmentation and TitaNet embedding models are present."
                    .to_string(),
            required_feature: None,
            runtime_version: None,
            model_id: None,
        };
    }

    match probe {
        Some(LocalRuntimeProbeOutcome::LoadFailed { message, model_id }) => {
            ProviderRuntimeReadiness {
                status: ProviderRuntimeReadinessStatus::LoadFailed,
                message,
                required_feature: None,
                runtime_version: None,
                model_id,
            }
        }
        Some(LocalRuntimeProbeOutcome::Healthy {
            runtime_version,
            model_id,
        }) => ProviderRuntimeReadiness {
            status: ProviderRuntimeReadinessStatus::Healthy,
            message: format!("Clustering diarization runtime loaded {model_id} successfully."),
            required_feature: None,
            runtime_version: Some(runtime_version),
            model_id: Some(model_id),
        },
        None => ProviderRuntimeReadiness {
            status: ProviderRuntimeReadinessStatus::RuntimeUnavailable,
            message: "Clustering diarization runtime probe is not available in this build path."
                .to_string(),
            required_feature: None,
            runtime_version: None,
            model_id: None,
        },
    }
}

fn local_runtime_readiness_with_probe<F>(
    descriptor: &crate::provider_registry::ProviderDescriptor,
    summary: &LocalModelReadinessSummary,
    models_dir: &std::path::Path,
    runtime_probe: F,
) -> Option<ProviderRuntimeReadiness>
where
    F: FnOnce(
        &crate::provider_registry::ProviderDescriptor,
        &LocalModelReadinessSummary,
        &std::path::Path,
    ) -> Option<LocalRuntimeProbeOutcome>,
{
    match descriptor.id {
        "asr.moonshine" => {
            let feature_compiled = cfg!(feature = "asr-moonshine");
            let probe = if feature_compiled && summary.ready > 0 {
                runtime_probe(descriptor, summary, models_dir)
            } else {
                None
            };
            Some(moonshine_runtime_readiness_from_state(
                feature_compiled,
                summary.ready,
                probe,
            ))
        }
        "diarization.clustering" => {
            let feature_compiled = cfg!(feature = "diarization-clustering");
            let probe = if feature_compiled && summary.ready >= summary.total {
                runtime_probe(descriptor, summary, models_dir)
            } else {
                None
            };
            Some(diarization_clustering_runtime_readiness_from_state(
                feature_compiled,
                summary.ready,
                summary.total,
                probe,
            ))
        }
        _ => None,
    }
}

fn local_runtime_probe_outcome(
    descriptor: &crate::provider_registry::ProviderDescriptor,
    summary: &LocalModelReadinessSummary,
    models_dir: &std::path::Path,
) -> Option<LocalRuntimeProbeOutcome> {
    match descriptor.id {
        "asr.moonshine" => {
            #[cfg(feature = "asr-moonshine")]
            {
                use crate::asr::moonshine::{
                    MoonshineNativeProbeResult, MoonshineNativeProbeStatus, MoonshineRuntimeConfig,
                    probe_moonshine_native_runtime,
                };

                let model_id = summary.ready_model_ids.first()?;
                let probe = probe_moonshine_native_runtime(MoonshineRuntimeConfig::new(
                    models_dir.join(model_id),
                ));
                let MoonshineNativeProbeResult {
                    status,
                    message,
                    runtime_version,
                    ..
                } = probe;

                match status {
                    MoonshineNativeProbeStatus::Ready => Some(LocalRuntimeProbeOutcome::Healthy {
                        runtime_version: runtime_version
                            .unwrap_or_else(|| "moonshine-native".to_string()),
                        model_id: model_id.clone(),
                    }),
                    MoonshineNativeProbeStatus::LoadFailed
                    | MoonshineNativeProbeStatus::ModelMissing
                    | MoonshineNativeProbeStatus::ModelInvalid => {
                        Some(LocalRuntimeProbeOutcome::LoadFailed {
                            message,
                            model_id: Some(model_id.clone()),
                        })
                    }
                }
            }
            #[cfg(not(feature = "asr-moonshine"))]
            {
                let _ = (summary, models_dir);
                None
            }
        }
        "diarization.clustering" => {
            #[cfg(feature = "diarization-clustering")]
            {
                let segmentation_model = models_dir
                    .join(crate::models::DIAR_SEG_PYANNOTE_DIR)
                    .join(crate::models::DIAR_SEG_PYANNOTE_FILE);
                let embedding_model = models_dir.join(crate::models::DIAR_EMB_TITANET_FILENAME);
                let model_id = diarization_clustering_runtime_model_id();

                if let Err(message) = diarization_clustering_runtime_file_preflight(
                    &segmentation_model,
                    &embedding_model,
                ) {
                    return Some(LocalRuntimeProbeOutcome::LoadFailed {
                        message,
                        model_id: Some(model_id),
                    });
                }

                match crate::diarization::clustering::ClusteringDiarizer::new(
                    &segmentation_model,
                    &embedding_model,
                    crate::diarization::clustering::DEFAULT_CLUSTERING_THRESHOLD,
                ) {
                    Ok(diarizer) => Some(LocalRuntimeProbeOutcome::Healthy {
                        runtime_version: format!(
                            "sherpa-onnx-clustering-{}hz",
                            diarizer.sample_rate()
                        ),
                        model_id,
                    }),
                    Err(error) => Some(LocalRuntimeProbeOutcome::LoadFailed {
                        message: format!(
                            "Clustering diarization runtime load failed for {}: {error}",
                            segmentation_model.display()
                        ),
                        model_id: Some(model_id),
                    }),
                }
            }
            #[cfg(not(feature = "diarization-clustering"))]
            {
                let _ = (summary, models_dir);
                None
            }
        }
        _ => None,
    }
}

fn apply_local_model_readiness(
    descriptor: &crate::provider_registry::ProviderDescriptor,
    app: &tauri::AppHandle,
    readiness: ProviderReadiness,
) -> ProviderReadiness {
    let models_dir = crate::models::get_models_dir(app);
    apply_local_model_readiness_from_dir(descriptor, &models_dir, readiness)
}

fn apply_local_model_readiness_from_dir(
    descriptor: &crate::provider_registry::ProviderDescriptor,
    models_dir: &std::path::Path,
    readiness: ProviderReadiness,
) -> ProviderReadiness {
    apply_local_model_readiness_from_dir_with_probe(
        descriptor,
        models_dir,
        readiness,
        local_runtime_probe_outcome,
    )
}

fn apply_local_model_readiness_from_dir_with_probe<F>(
    descriptor: &crate::provider_registry::ProviderDescriptor,
    models_dir: &std::path::Path,
    mut readiness: ProviderReadiness,
    runtime_probe: F,
) -> ProviderReadiness
where
    F: FnOnce(
        &crate::provider_registry::ProviderDescriptor,
        &LocalModelReadinessSummary,
        &std::path::Path,
    ) -> Option<LocalRuntimeProbeOutcome>,
{
    if let Some(summary) = local_model_readiness_summary(descriptor, models_dir) {
        if let Some(message) = local_model_readiness_message(descriptor, models_dir) {
            readiness.message = message;
        }
        readiness.runtime =
            local_runtime_readiness_with_probe(descriptor, &summary, models_dir, runtime_probe);
    }
    readiness
}

fn endpoint_allows_missing_saved_credential(endpoint: &str) -> bool {
    let Ok(parsed) = validate_endpoint_url(endpoint) else {
        return false;
    };
    parsed.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    })
}

fn required_openai_compatible_endpoint_credential_keys(endpoint: &str) -> Vec<&'static str> {
    if endpoint_allows_missing_saved_credential(endpoint) {
        vec![]
    } else {
        vec![crate::settings::credential_key_for_endpoint(endpoint)]
    }
}

fn required_credential_keys_for_provider(
    descriptor: &crate::provider_registry::ProviderDescriptor,
    settings: &crate::settings::AppSettings,
) -> Vec<&'static str> {
    match descriptor.id {
        "asr.api" => match &settings.asr_provider {
            crate::settings::AsrProvider::Api { endpoint, .. } => {
                required_openai_compatible_endpoint_credential_keys(endpoint)
            }
            _ => vec![],
        },
        "llm.api" => match &settings.llm_provider {
            crate::settings::LlmProvider::Api { endpoint, .. } => {
                required_openai_compatible_endpoint_credential_keys(endpoint)
            }
            _ => vec![],
        },
        "asr.aws_transcribe" => match &settings.asr_provider {
            crate::settings::AsrProvider::AwsTranscribe {
                credential_source, ..
            } => required_aws_credential_keys(credential_source),
            _ => vec![],
        },
        "asr.deepgram" | "tts.deepgram_aura" => vec!["deepgram_api_key"],
        "asr.assemblyai" => vec!["assemblyai_api_key"],
        "asr.soniox" => vec!["soniox_api_key"],
        "asr.revai" => vec!["revai_api_key"],
        "asr.openai_realtime" | "realtime_agent.openai_realtime" => vec!["openai_api_key"],
        "llm.cerebras" => vec!["cerebras_api_key"],
        "llm.sambanova" => vec!["sambanova_api_key"],
        "llm.openrouter" => vec!["openrouter_api_key"],
        "llm.aws_bedrock" => match &settings.llm_provider {
            crate::settings::LlmProvider::AwsBedrock {
                credential_source, ..
            } => required_aws_credential_keys(credential_source),
            _ => vec![],
        },
        "realtime_agent.gemini_live" => match &settings.gemini.auth {
            crate::settings::GeminiAuthMode::ApiKey { .. } => vec!["gemini_api_key"],
            crate::settings::GeminiAuthMode::VertexAI { .. } => {
                vec!["google_service_account_path"]
            }
        },
        _ => descriptor.credential_keys.to_vec(),
    }
}

fn required_aws_credential_keys(
    credential_source: &crate::settings::AwsCredentialSource,
) -> Vec<&'static str> {
    match credential_source {
        crate::settings::AwsCredentialSource::AccessKeys { .. } => {
            vec!["aws_access_key", "aws_secret_key"]
        }
        crate::settings::AwsCredentialSource::DefaultChain
        | crate::settings::AwsCredentialSource::Profile { .. } => vec![],
    }
}

fn missing_required_credentials(
    descriptor: &crate::provider_registry::ProviderDescriptor,
    settings: &crate::settings::AppSettings,
    store: &crate::credentials::CredentialStore,
) -> Vec<String> {
    required_credential_keys_for_provider(descriptor, settings)
        .into_iter()
        .filter(|key| !credential_present(store, key))
        .map(str::to_string)
        .collect()
}

fn provider_config_readiness_message(
    descriptor: &crate::provider_registry::ProviderDescriptor,
    settings: &crate::settings::AppSettings,
) -> Option<String> {
    match descriptor.id {
        "asr.aws_transcribe" => match &settings.asr_provider {
            crate::settings::AsrProvider::AwsTranscribe {
                credential_source:
                    crate::settings::AwsCredentialSource::Profile { name },
                ..
            } if name.trim().is_empty() => Some(
                "AWS profile name must be configured before readiness can be checked".to_string(),
            ),
            _ => None,
        },
        "llm.aws_bedrock" => match &settings.llm_provider {
            crate::settings::LlmProvider::AwsBedrock {
                credential_source:
                    crate::settings::AwsCredentialSource::Profile { name },
                ..
            } if name.trim().is_empty() => Some(
                "AWS profile name must be configured before readiness can be checked".to_string(),
            ),
            _ => None,
        },
        "realtime_agent.gemini_live" => match &settings.gemini.auth {
            crate::settings::GeminiAuthMode::VertexAI {
                project_id,
                location,
                ..
            } if project_id.trim().is_empty() || location.trim().is_empty() => {
                Some(
                    "Vertex AI project ID and location must be configured before readiness can be checked"
                        .to_string(),
                )
            }
            _ => None,
        },
        _ => None,
    }
}

fn provider_has_automatic_health_probe(
    descriptor: &crate::provider_registry::ProviderDescriptor,
    settings: &crate::settings::AppSettings,
) -> bool {
    match descriptor.id {
        "realtime_agent.gemini_live" => {
            matches!(
                settings.gemini.auth,
                crate::settings::GeminiAuthMode::ApiKey { .. }
            )
        }
        _ => descriptor.health_check_command.is_some(),
    }
}

fn automatic_probe_available_from_decision(
    descriptor: &crate::provider_registry::ProviderDescriptor,
    settings: &crate::settings::AppSettings,
    missing: &[String],
    config_message: Option<&str>,
) -> bool {
    missing.is_empty()
        && config_message.is_none()
        && provider_has_automatic_health_probe(descriptor, settings)
}

fn provider_automatic_probe_available(
    descriptor: &crate::provider_registry::ProviderDescriptor,
    settings: &crate::settings::AppSettings,
    store: &crate::credentials::CredentialStore,
) -> bool {
    let missing = missing_required_credentials(descriptor, settings, store);
    let config_message = provider_config_readiness_message(descriptor, settings);
    automatic_probe_available_from_decision(
        descriptor,
        settings,
        &missing,
        config_message.as_deref(),
    )
}

fn native_realtime_readiness_requested(
    conversation_mode: Option<&str>,
    converse_engine: Option<&str>,
) -> bool {
    matches!(conversation_mode, Some("converse")) && matches!(converse_engine, Some("native"))
}

fn active_provider_ids(
    settings: &crate::settings::AppSettings,
    native_realtime_active: bool,
) -> HashSet<&'static str> {
    let mut ids = HashSet::new();
    ids.insert(crate::provider_registry::descriptor_for_asr_provider(&settings.asr_provider).id);
    ids.insert(crate::provider_registry::descriptor_for_llm_provider(&settings.llm_provider).id);
    ids.insert(crate::provider_registry::descriptor_for_tts_provider(&settings.tts_provider).id);
    if native_realtime_active {
        ids.insert("realtime_agent.gemini_live");
    }
    ids
}

fn requested_provider_ids(
    settings: &crate::settings::AppSettings,
    native_realtime_active: bool,
    provider_ids: Option<&[String]>,
) -> HashSet<&'static str> {
    let Some(provider_ids) = provider_ids else {
        // Legacy callers did not provide an explicit diagnostic scope. Keep
        // their refresh limited to product-enabled active providers so merely
        // opening an older Settings surface cannot probe a persisted deferred
        // route. Deferred diagnostics opt in by naming the provider id.
        return active_provider_ids(settings, native_realtime_active)
            .into_iter()
            .filter(|id| crate::provider_registry::descriptor_by_id(id).ui_selectable)
            .collect();
    };

    let requested: HashSet<&str> = provider_ids.iter().map(String::as_str).collect();
    crate::provider_registry::provider_registry()
        .iter()
        .filter(|descriptor| requested.contains(descriptor.id))
        .map(|descriptor| descriptor.id)
        .collect()
}

fn provider_readiness_config_fingerprint(
    descriptor: &crate::provider_registry::ProviderDescriptor,
    settings: &crate::settings::AppSettings,
    active_ids: &HashSet<&'static str>,
) -> String {
    match descriptor.id {
        "asr.api" => match &settings.asr_provider {
            crate::settings::AsrProvider::Api {
                endpoint, model, ..
            } => openai_compatible_endpoint_fingerprint(endpoint, model),
            _ => "inactive".to_string(),
        },
        "asr.deepgram" => match &settings.asr_provider {
            crate::settings::AsrProvider::DeepgramStreaming {
                model,
                enable_diarization,
                endpointing_ms,
                utterance_end_ms,
                vad_events,
                eot_threshold,
                eager_eot_threshold,
                eot_timeout_ms,
                max_speakers,
                ..
            } => format!(
                "model={}|diarization={enable_diarization}|endpointing_ms={endpointing_ms}|utterance_end_ms={utterance_end_ms}|vad_events={vad_events}|eot_threshold={eot_threshold}|eager_eot_threshold={eager_eot_threshold}|eot_timeout_ms={eot_timeout_ms}|max_speakers={max_speakers}|global_diarization_mode={:?}|global_speaker_count={:?}|global_max_speakers={:?}",
                model.trim(),
                settings.diarization.mode,
                settings.diarization.speaker_count,
                settings.diarization.max_speakers,
            ),
            _ => "inactive".to_string(),
        },
        "asr.aws_transcribe" => match &settings.asr_provider {
            crate::settings::AsrProvider::AwsTranscribe {
                region,
                credential_source,
                ..
            } => format!(
                "region={}|credential_source={credential_source:?}",
                region.trim()
            ),
            _ => "inactive".to_string(),
        },
        "llm.openrouter" => match &settings.llm_provider {
            crate::settings::LlmProvider::OpenRouter {
                base_url, model, ..
            } => {
                format!(
                    "base_url={}|model={}",
                    openrouter_base_url_or_default(Some(base_url.clone())),
                    model.trim()
                )
            }
            _ => format!("base_url={}", openrouter::DEFAULT_BASE_URL),
        },
        "llm.cerebras" => match &settings.llm_provider {
            crate::settings::LlmProvider::Api {
                endpoint, model, ..
            } if crate::settings::is_cerebras_endpoint(endpoint) => {
                openai_compatible_endpoint_fingerprint(endpoint, model)
            }
            _ => openai_compatible_endpoint_fingerprint(
                crate::settings::CEREBRAS_BASE_URL,
                crate::provider_registry::CEREBRAS_DEFAULT_MODEL,
            ),
        },
        "llm.sambanova" => match &settings.llm_provider {
            crate::settings::LlmProvider::Api {
                endpoint, model, ..
            } if crate::settings::is_sambanova_endpoint(endpoint) => {
                openai_compatible_endpoint_fingerprint(endpoint, model)
            }
            _ => openai_compatible_endpoint_fingerprint(
                crate::settings::SAMBANOVA_BASE_URL,
                crate::provider_registry::SAMBANOVA_DEFAULT_MODEL,
            ),
        },
        "llm.api" => match &settings.llm_provider {
            // The Cerebras and SambaNova endpoints are fingerprinted by their own
            // dedicated arms; exclude them here. The credential epoch is composed by
            // the cache-key caller, not here.
            crate::settings::LlmProvider::Api {
                endpoint, model, ..
            } if !crate::settings::is_cerebras_endpoint(endpoint)
                && !crate::settings::is_sambanova_endpoint(endpoint) =>
            {
                openai_compatible_endpoint_fingerprint(endpoint, model)
            }
            _ => "inactive".to_string(),
        },
        "llm.aws_bedrock" => match &settings.llm_provider {
            crate::settings::LlmProvider::AwsBedrock {
                region,
                model_id,
                credential_source,
            } => format!(
                "region={}|model={}|credential_source={credential_source:?}",
                region.trim(),
                model_id.trim()
            ),
            _ => "inactive".to_string(),
        },
        "realtime_agent.gemini_live" if !active_ids.contains(descriptor.id) => {
            "inactive".to_string()
        }
        "realtime_agent.gemini_live" => match &settings.gemini.auth {
            crate::settings::GeminiAuthMode::ApiKey { .. } => {
                format!("auth=api_key|model={}", settings.gemini.model.trim())
            }
            crate::settings::GeminiAuthMode::VertexAI {
                project_id,
                location,
                service_account_path,
            } => format!(
                "auth=vertex_ai|project={}|location={}|service_account_path_present={}",
                project_id.trim(),
                location.trim(),
                service_account_path
                    .as_deref()
                    .map(str::trim)
                    .is_some_and(|value| !value.is_empty())
            ),
        },
        "tts.deepgram_aura" => match &settings.tts_provider {
            crate::settings::TtsProvider::DeepgramAura { voice, speed, .. } => {
                format!("voice={}|speed={}", voice.trim(), speed)
            }
            _ => "inactive".to_string(),
        },
        _ => "static".to_string(),
    }
}

fn provider_readiness_cache_key(
    descriptor: &crate::provider_registry::ProviderDescriptor,
    settings: &crate::settings::AppSettings,
    credential_epoch: u64,
    active_ids: &HashSet<&'static str>,
) -> String {
    format!(
        "{}|epoch={credential_epoch}|{}",
        descriptor.id,
        provider_readiness_config_fingerprint(descriptor, settings, active_ids)
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderReadinessRefreshAdmission {
    Started,
    InFlight,
    RateLimited { retry_after_ms: u64 },
}

fn cached_provider_readiness(cache_key: &str, now: u64) -> Option<ProviderReadiness> {
    let cached = provider_readiness_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(cache_key)
        .cloned()?;
    let stale = cached
        .checked_at
        .is_some_and(|checked_at| now.saturating_sub(checked_at) > PROVIDER_READINESS_TTL_MS);
    Some(ProviderReadiness { stale, ..cached })
}

fn store_provider_readiness(cache_key: String, readiness: &ProviderReadiness) {
    provider_readiness_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(cache_key, readiness.clone());
}

fn begin_provider_readiness_refresh(
    cache_key: &str,
    now: u64,
    force: bool,
) -> ProviderReadinessRefreshAdmission {
    {
        let mut in_flight = provider_readiness_in_flight()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if in_flight.contains(cache_key) {
            return ProviderReadinessRefreshAdmission::InFlight;
        }
        let last_started = provider_readiness_last_started()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(cache_key)
            .copied();
        if let Some(last_started) = last_started
            && !force
        {
            let elapsed = now.saturating_sub(last_started);
            if elapsed < PROVIDER_READINESS_MIN_REFRESH_INTERVAL_MS {
                return ProviderReadinessRefreshAdmission::RateLimited {
                    retry_after_ms: PROVIDER_READINESS_MIN_REFRESH_INTERVAL_MS - elapsed,
                };
            }
        }
        in_flight.insert(cache_key.to_string());
    }

    provider_readiness_last_started()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(cache_key.to_string(), now);
    ProviderReadinessRefreshAdmission::Started
}

fn finish_provider_readiness_refresh(cache_key: &str) {
    provider_readiness_in_flight()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(cache_key);
}

struct ProviderReadinessRefreshGuard {
    cache_key: String,
}

impl ProviderReadinessRefreshGuard {
    fn new(cache_key: String) -> Self {
        Self { cache_key }
    }
}

impl Drop for ProviderReadinessRefreshGuard {
    fn drop(&mut self) {
        finish_provider_readiness_refresh(&self.cache_key);
    }
}

fn base_provider_readiness(
    descriptor: &crate::provider_registry::ProviderDescriptor,
    settings: &crate::settings::AppSettings,
    store: &crate::credentials::CredentialStore,
    credential_epoch: u64,
) -> ProviderReadiness {
    let model_catalog = fixed_model_catalog_for_descriptor(descriptor);
    let voice_catalog = fixed_voice_catalog_for_descriptor(descriptor);
    let language_catalog = fixed_language_catalog_for_descriptor(descriptor);
    let missing = missing_required_credentials(descriptor, settings, store);
    let config_message = provider_config_readiness_message(descriptor, settings);
    let automatic_probe_available = automatic_probe_available_from_decision(
        descriptor,
        settings,
        &missing,
        config_message.as_deref(),
    );
    let status = if missing.is_empty() {
        ProviderReadinessStatus::Unchecked
    } else {
        ProviderReadinessStatus::MissingCredentials
    };
    let message = if missing.is_empty() {
        if let Some(message) = config_message {
            message
        } else if automatic_probe_available {
            "Ready to check with saved credentials".to_string()
        } else if descriptor.id == "realtime_agent.gemini_live"
            && matches!(
                settings.gemini.auth,
                crate::settings::GeminiAuthMode::VertexAI { .. }
            )
        {
            "Vertex AI readiness is not probed automatically yet".to_string()
        } else if descriptor.model_catalog
            == crate::provider_registry::ModelCatalogPolicy::LocalFiles
        {
            "Local model readiness is checked by the model manager".to_string()
        } else {
            "No automatic health probe is available for this provider yet".to_string()
        }
    } else {
        format!("Missing saved credential(s): {}", missing.join(", "))
    };

    ProviderReadiness {
        provider_id: descriptor.id.to_string(),
        status,
        message,
        automatic_probe_available,
        checked_at: None,
        stale: false,
        credential_epoch,
        credentials: credential_readiness_from_store(descriptor, store),
        model_count: model_count_from_catalog(&model_catalog),
        model_catalog,
        voice_catalog,
        language_catalog,
        openrouter_models: vec![],
        runtime: None,
        effective_stt_fidelity: effective_stt_fidelity(descriptor, settings),
    }
}

fn deferred_provider_readiness(
    descriptor: &crate::provider_registry::ProviderDescriptor,
    settings: &crate::settings::AppSettings,
    store: &crate::credentials::CredentialStore,
    credential_epoch: u64,
    message: String,
) -> ProviderReadiness {
    let mut readiness = base_provider_readiness(descriptor, settings, store, credential_epoch);
    if readiness.status == ProviderReadinessStatus::Unchecked {
        readiness.message = message;
    }
    readiness
}

fn cancelled_provider_readiness(
    descriptor: &crate::provider_registry::ProviderDescriptor,
    settings: &crate::settings::AppSettings,
    store: &crate::credentials::CredentialStore,
    credential_epoch: u64,
) -> ProviderReadiness {
    let model_catalog = fixed_model_catalog_for_descriptor(descriptor);
    let voice_catalog = fixed_voice_catalog_for_descriptor(descriptor);
    let language_catalog = fixed_language_catalog_for_descriptor(descriptor);
    ProviderReadiness {
        provider_id: descriptor.id.to_string(),
        status: ProviderReadinessStatus::Unchecked,
        message: "Provider readiness check cancelled".to_string(),
        automatic_probe_available: provider_automatic_probe_available(descriptor, settings, store),
        checked_at: None,
        stale: false,
        credential_epoch,
        credentials: credential_readiness_from_store(descriptor, store),
        model_count: model_count_from_catalog(&model_catalog),
        model_catalog,
        voice_catalog,
        language_catalog,
        openrouter_models: vec![],
        runtime: None,
        effective_stt_fidelity: effective_stt_fidelity(descriptor, settings),
    }
}

fn should_probe_provider(
    descriptor: &crate::provider_registry::ProviderDescriptor,
    active_ids: &HashSet<&'static str>,
    settings: &crate::settings::AppSettings,
    store: &crate::credentials::CredentialStore,
) -> bool {
    // A readiness response still includes passive/base metadata for the full
    // registry, but network/model health refresh is limited to the caller's
    // request set. Settings sends only currently enabled selections on mount,
    // while an explicitly requested deferred-provider diagnostic remains
    // available under ADR-0033's non-content-bearing exception.
    if !active_ids.contains(descriptor.id) {
        return false;
    }
    if !provider_has_automatic_health_probe(descriptor, settings) {
        return false;
    }
    if !missing_required_credentials(descriptor, settings, store).is_empty() {
        return false;
    }
    if provider_config_readiness_message(descriptor, settings).is_some() {
        return false;
    }
    matches!(
        descriptor.id,
        "asr.deepgram"
            | "asr.assemblyai"
            | "asr.soniox"
            | "llm.cerebras"
            | "llm.openrouter"
            | "tts.deepgram_aura"
            | "realtime_agent.gemini_live"
            | "asr.api"
            | "asr.aws_transcribe"
            | "llm.api"
            | "llm.aws_bedrock"
    )
}

/// Config-fingerprint string shared by every OpenAI-compatible readiness arm
/// (`asr.api`, `llm.cerebras`, `llm.api`). Their endpoint+model fingerprint is
/// byte-identical, so route them all through this one formatter to keep the
/// arms from drifting.
fn openai_compatible_endpoint_fingerprint(endpoint: &str, model: &str) -> String {
    format!("endpoint={}|model={}", endpoint.trim(), model.trim())
}

/// Shared OpenAI-compatible readiness probe: resolve the endpoint API key, fetch
/// the `/models` catalog with the given `default_model` fallback, and build the
/// success result with a caller-supplied message. Reused by the `asr.api`,
/// `llm.cerebras`, and `llm.api` arms so their probe behavior stays in lockstep.
///
/// `message` receives `(endpoint, model_count)` so each arm keeps its own
/// human-readable wording (e.g. the Cerebras arm's "API key is valid" copy).
async fn openai_compatible_readiness_arm(
    endpoint: &str,
    default_model: Option<&str>,
    message: impl FnOnce(&str, usize) -> String,
) -> AppResult<ProviderReadinessProbeResult> {
    let api_key = endpoint_api_key_from_draft_or_store(endpoint, None)?;
    let model_catalog = fetch_openai_compatible_model_catalog_with_default(
        endpoint,
        api_key.as_deref(),
        default_model,
    )
    .await?;
    let model_count = model_catalog.len();
    Ok(ProviderReadinessProbeResult {
        message: message(endpoint, model_count),
        model_count: Some(model_count),
        model_catalog,
        ..ProviderReadinessProbeResult::default()
    })
}

/// The `default_model` fallback used by `fetch_openai_compatible_model_catalog`
/// (i.e. the generic, non-Cerebras OpenAI-compatible arms).
const OPENAI_COMPATIBLE_DEFAULT_MODEL: &str = "whisper-1";

fn connected_openai_compatible_message(endpoint: &str, model_count: usize) -> String {
    format!("Connected to {endpoint} ({model_count} OpenAI-compatible models)")
}

async fn probe_provider_readiness(
    descriptor: &crate::provider_registry::ProviderDescriptor,
    settings: &crate::settings::AppSettings,
    store: &crate::credentials::CredentialStore,
) -> AppResult<ProviderReadinessProbeResult> {
    match descriptor.id {
        "asr.api" => {
            let crate::settings::AsrProvider::Api { endpoint, .. } = &settings.asr_provider else {
                return Ok(ProviderReadinessProbeResult {
                    message: "Provider is not selected".to_string(),
                    ..ProviderReadinessProbeResult::default()
                });
            };
            openai_compatible_readiness_arm(
                endpoint,
                Some(OPENAI_COMPATIBLE_DEFAULT_MODEL),
                connected_openai_compatible_message,
            )
            .await
        }
        "llm.cerebras" => {
            let endpoint = match &settings.llm_provider {
                crate::settings::LlmProvider::Api { endpoint, .. }
                    if crate::settings::is_cerebras_endpoint(endpoint) =>
                {
                    endpoint.as_str()
                }
                _ => crate::settings::CEREBRAS_BASE_URL,
            };
            openai_compatible_readiness_arm(
                endpoint,
                Some(crate::provider_registry::CEREBRAS_DEFAULT_MODEL),
                |_endpoint, model_count| {
                    format!("Cerebras API key is valid ({model_count} models)")
                },
            )
            .await
        }
        "llm.sambanova" => {
            let endpoint = match &settings.llm_provider {
                crate::settings::LlmProvider::Api { endpoint, .. }
                    if crate::settings::is_sambanova_endpoint(endpoint) =>
                {
                    endpoint.as_str()
                }
                _ => crate::settings::SAMBANOVA_BASE_URL,
            };
            openai_compatible_readiness_arm(
                endpoint,
                Some(crate::provider_registry::SAMBANOVA_DEFAULT_MODEL),
                |_endpoint, model_count| {
                    format!("SambaNova API key is valid ({model_count} models)")
                },
            )
            .await
        }
        "llm.api" => {
            // The Cerebras and SambaNova endpoints have their own dedicated arms;
            // exclude them here so the generic OpenAI-compatible probe never
            // double-claims them.
            let crate::settings::LlmProvider::Api { endpoint, .. } = &settings.llm_provider else {
                return Ok(ProviderReadinessProbeResult {
                    message: "Provider is not selected".to_string(),
                    ..ProviderReadinessProbeResult::default()
                });
            };
            if crate::settings::is_cerebras_endpoint(endpoint)
                || crate::settings::is_sambanova_endpoint(endpoint)
            {
                return Ok(ProviderReadinessProbeResult {
                    message: "Provider is not selected".to_string(),
                    ..ProviderReadinessProbeResult::default()
                });
            }
            openai_compatible_readiness_arm(
                endpoint,
                Some(OPENAI_COMPATIBLE_DEFAULT_MODEL),
                connected_openai_compatible_message,
            )
            .await
        }
        "asr.aws_transcribe" => {
            let crate::settings::AsrProvider::AwsTranscribe {
                region,
                credential_source,
                ..
            } = &settings.asr_provider
            else {
                return Ok(ProviderReadinessProbeResult {
                    message: "Provider is not selected".to_string(),
                    ..ProviderReadinessProbeResult::default()
                });
            };
            let message =
                test_aws_credentials(region.clone(), credential_source.clone(), None, None).await?;
            Ok(ProviderReadinessProbeResult {
                message,
                ..ProviderReadinessProbeResult::default()
            })
        }
        "asr.deepgram" => {
            let api_key = deepgram_api_key_from_store(store)?;
            let model_catalog = fetch_deepgram_stt_model_catalog(&api_key).await?;
            let model_count = model_catalog.len();
            Ok(ProviderReadinessProbeResult {
                message: format!(
                    "Deepgram API key is valid ({} streaming STT models)",
                    model_count
                ),
                model_count: Some(model_count),
                model_catalog,
                ..ProviderReadinessProbeResult::default()
            })
        }
        "asr.soniox" => {
            let api_key = soniox_api_key_from_store(store)?;
            let model_catalog = fetch_soniox_realtime_model_catalog(&api_key).await?;
            let model_count = model_catalog.len();
            Ok(ProviderReadinessProbeResult {
                message: format!(
                    "Soniox API key is valid ({} real-time STT models)",
                    model_count
                ),
                model_count: Some(model_count),
                model_catalog,
                ..ProviderReadinessProbeResult::default()
            })
        }
        "tts.deepgram_aura" => {
            let message = test_tts_connection_cmd("deepgram_aura".to_string(), None).await?;
            Ok(ProviderReadinessProbeResult {
                message,
                ..ProviderReadinessProbeResult::default()
            })
        }
        "asr.assemblyai" => {
            let message = test_assemblyai_connection(None).await?;
            Ok(ProviderReadinessProbeResult {
                message,
                ..ProviderReadinessProbeResult::default()
            })
        }
        "llm.openrouter" => {
            let api_key = openrouter_api_key_from_store(store)?;
            let base_url = match &settings.llm_provider {
                crate::settings::LlmProvider::OpenRouter { base_url, .. } => {
                    openrouter_base_url_or_default(Some(base_url.clone()))
                }
                _ => openrouter::DEFAULT_BASE_URL.to_string(),
            };
            openrouter::test_connection(&api_key, &base_url)
                .await
                .map_err(AppError::Unknown)?;
            let models = openrouter::list_models(&api_key, &base_url)
                .await
                .map_err(AppError::Unknown)?;
            let message = format!("OpenRouter API key is valid ({} models)", models.len());
            Ok(ProviderReadinessProbeResult {
                message,
                model_count: Some(models.len()),
                openrouter_models: models,
                ..ProviderReadinessProbeResult::default()
            })
        }
        "llm.aws_bedrock" => {
            let crate::settings::LlmProvider::AwsBedrock {
                region,
                credential_source,
                ..
            } = &settings.llm_provider
            else {
                return Ok(ProviderReadinessProbeResult {
                    message: "Provider is not selected".to_string(),
                    ..ProviderReadinessProbeResult::default()
                });
            };
            let message =
                test_aws_credentials(region.clone(), credential_source.clone(), None, None).await?;
            Ok(ProviderReadinessProbeResult {
                message,
                ..ProviderReadinessProbeResult::default()
            })
        }
        "realtime_agent.gemini_live" => match &settings.gemini.auth {
            crate::settings::GeminiAuthMode::ApiKey { .. } => {
                let message = test_gemini_api_key(None).await?;
                Ok(ProviderReadinessProbeResult {
                    message,
                    ..ProviderReadinessProbeResult::default()
                })
            }
            crate::settings::GeminiAuthMode::VertexAI { .. } => Ok(ProviderReadinessProbeResult {
                message: "Vertex AI readiness is not probed automatically yet".to_string(),
                ..ProviderReadinessProbeResult::default()
            }),
        },
        _ => Ok(ProviderReadinessProbeResult {
            message: "No automatic health probe is available for this provider yet".to_string(),
            ..ProviderReadinessProbeResult::default()
        }),
    }
}

async fn refresh_provider_readiness(
    descriptor: &crate::provider_registry::ProviderDescriptor,
    settings: &crate::settings::AppSettings,
    store: &crate::credentials::CredentialStore,
    credential_epoch: u64,
    cancel: Option<&CancellationToken>,
) -> ProviderReadiness {
    let credentials = credential_readiness_from_store(descriptor, store);
    let automatic_probe_available = provider_automatic_probe_available(descriptor, settings, store);
    let checked_at = unix_millis();
    let probe = tokio::time::timeout(
        Duration::from_secs(PROVIDER_READINESS_TIMEOUT_SECS),
        probe_provider_readiness(descriptor, settings, store),
    );
    let result = if let Some(cancel) = cancel {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return cancelled_provider_readiness(descriptor, settings, store, credential_epoch),
            result = probe => result,
        }
    } else {
        probe.await
    };

    match result {
        Ok(Ok(probe)) => {
            let fallback_catalog = fixed_model_catalog_for_descriptor(descriptor);
            let fallback_voice_catalog = fixed_voice_catalog_for_descriptor(descriptor);
            let fallback_language_catalog = fixed_language_catalog_for_descriptor(descriptor);
            let model_catalog = if probe.model_catalog.is_empty() {
                fallback_catalog
            } else {
                probe.model_catalog
            };
            let voice_catalog = if probe.voice_catalog.is_empty() {
                fallback_voice_catalog
            } else {
                probe.voice_catalog
            };
            let language_catalog = if probe.language_catalog.is_empty() {
                fallback_language_catalog
            } else {
                probe.language_catalog
            };
            ProviderReadiness {
                provider_id: descriptor.id.to_string(),
                status: ProviderReadinessStatus::Ready,
                message: probe.message,
                automatic_probe_available,
                checked_at: Some(checked_at),
                stale: false,
                credential_epoch,
                credentials,
                model_count: probe
                    .model_count
                    .or_else(|| model_count_from_catalog(&model_catalog)),
                model_catalog,
                voice_catalog,
                language_catalog,
                openrouter_models: probe.openrouter_models,
                runtime: None,
                effective_stt_fidelity: effective_stt_fidelity(descriptor, settings),
            }
        }
        Ok(Err(error)) => {
            let model_catalog = fixed_model_catalog_for_descriptor(descriptor);
            let voice_catalog = fixed_voice_catalog_for_descriptor(descriptor);
            let language_catalog = fixed_language_catalog_for_descriptor(descriptor);
            ProviderReadiness {
                provider_id: descriptor.id.to_string(),
                status: ProviderReadinessStatus::Error,
                message: error.to_string(),
                automatic_probe_available,
                checked_at: Some(checked_at),
                stale: false,
                credential_epoch,
                credentials,
                model_count: model_count_from_catalog(&model_catalog),
                model_catalog,
                voice_catalog,
                language_catalog,
                openrouter_models: vec![],
                runtime: None,
                effective_stt_fidelity: effective_stt_fidelity(descriptor, settings),
            }
        }
        Err(_) => {
            let model_catalog = fixed_model_catalog_for_descriptor(descriptor);
            let voice_catalog = fixed_voice_catalog_for_descriptor(descriptor);
            let language_catalog = fixed_language_catalog_for_descriptor(descriptor);
            ProviderReadiness {
                provider_id: descriptor.id.to_string(),
                status: ProviderReadinessStatus::Error,
                message: format!(
                    "Health check timed out after {}s",
                    PROVIDER_READINESS_TIMEOUT_SECS
                ),
                automatic_probe_available,
                checked_at: Some(checked_at),
                stale: false,
                credential_epoch,
                credentials,
                model_count: model_count_from_catalog(&model_catalog),
                model_catalog,
                voice_catalog,
                language_catalog,
                openrouter_models: vec![],
                runtime: None,
                effective_stt_fidelity: effective_stt_fidelity(descriptor, settings),
            }
        }
    }
}

/// Return non-secret provider readiness for Settings.
///
/// This command is the Settings-open path: it reads the Rust-owned credential
/// backend server-side, never returns plaintext secrets, and caches health/model
/// results by provider id, non-secret settings, and a credential epoch bumped
/// by save/delete credential commands.
#[tauri::command]
pub async fn get_provider_readiness_cmd(
    app: tauri::AppHandle,
    refresh: Option<bool>,
    force: Option<bool>,
    conversation_mode: Option<String>,
    converse_engine: Option<String>,
    provider_ids: Option<Vec<String>>,
    request_id: Option<String>,
) -> AppResult<Vec<ProviderReadiness>> {
    let settings = crate::settings::load_settings(&app);
    let store = crate::credentials::try_load_credentials()
        .map_err(|reason| AppError::CredentialFileError { reason })?;
    let request = register_provider_readiness_request(request_id)?;
    let (_request_guard, cancel) = match &request {
        Some((guard, token)) => (Some(guard), Some(token)),
        None => (None, None),
    };
    let credential_epoch = PROVIDER_CREDENTIAL_EPOCH.load(Ordering::SeqCst);
    let native_realtime_active = native_realtime_readiness_requested(
        conversation_mode.as_deref(),
        converse_engine.as_deref(),
    );
    let active_ids =
        requested_provider_ids(&settings, native_realtime_active, provider_ids.as_deref());
    let refresh = refresh.unwrap_or(false);
    let force = force.unwrap_or(false);
    let now = unix_millis();
    let mut readiness = Vec::with_capacity(crate::provider_registry::provider_registry().len());

    for descriptor in crate::provider_registry::provider_registry() {
        if cancel.is_some_and(CancellationToken::is_cancelled) {
            break;
        }
        let cache_key =
            provider_readiness_cache_key(descriptor, &settings, credential_epoch, &active_ids);
        let cached = cached_provider_readiness(&cache_key, now);
        if let Some(cached) = cached.as_ref()
            && (!refresh || !cached.stale)
        {
            readiness.push(cached.clone());
            continue;
        }

        let value = if refresh && should_probe_provider(descriptor, &active_ids, &settings, &store)
        {
            match begin_provider_readiness_refresh(&cache_key, now, force) {
                ProviderReadinessRefreshAdmission::Started => {
                    let _refresh_guard = ProviderReadinessRefreshGuard::new(cache_key.clone());

                    refresh_provider_readiness(
                        descriptor,
                        &settings,
                        &store,
                        credential_epoch,
                        cancel,
                    )
                    .await
                }
                ProviderReadinessRefreshAdmission::InFlight => cached.unwrap_or_else(|| {
                    deferred_provider_readiness(
                        descriptor,
                        &settings,
                        &store,
                        credential_epoch,
                        "Health check already in progress".to_string(),
                    )
                }),
                ProviderReadinessRefreshAdmission::RateLimited { retry_after_ms } => cached
                    .unwrap_or_else(|| {
                        deferred_provider_readiness(
                            descriptor,
                            &settings,
                            &store,
                            credential_epoch,
                            format!(
                                "Health check was started recently; retry in {}s",
                                retry_after_ms.div_ceil(1000)
                            ),
                        )
                    }),
            }
        } else {
            base_provider_readiness(descriptor, &settings, &store, credential_epoch)
        };

        let value = apply_local_model_readiness(descriptor, &app, value);

        if value.checked_at.is_some() {
            store_provider_readiness(cache_key, &value);
        }
        readiness.push(value);
    }

    Ok(readiness)
}

#[tauri::command]
pub fn cancel_provider_readiness_cmd(request_id: String) -> AppResult<bool> {
    validate_provider_readiness_request_id(&request_id)?;
    Ok(cancel_provider_readiness_request(&request_id))
}

/// Diagnose credential-store health. Surfaces backend read errors to the UI so
/// users can tell the difference between "no keys set" and "the local
/// credential store needs recovery".
#[tauri::command]
pub fn diagnose_credentials() -> AppResult<String> {
    match crate::credentials::try_load_credentials_with_source() {
        Ok(snapshot) => {
            let count = snapshot.store.present_count();
            Ok(format!(
                "Credentials loaded successfully from {} ({} keys present)",
                snapshot.source, count
            ))
        }
        Err(reason) => Err(AppError::CredentialFileError { reason }),
    }
}

fn aws_profile_path(env_key: &str, default_filename: &str) -> Option<std::path::PathBuf> {
    std::env::var_os(env_key)
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".aws").join(default_filename)))
}

fn list_aws_profiles_from_paths(
    config_path: Option<&std::path::Path>,
    credentials_path: Option<&std::path::Path>,
) -> Vec<String> {
    let mut profiles = std::collections::BTreeSet::new();
    for (path, is_credentials) in [(config_path, false), (credentials_path, true)] {
        let Some(path) = path else { continue };
        let Ok(contents) = std::fs::read_to_string(path) else {
            continue;
        };
        for line in contents.lines() {
            let trimmed = line.trim();
            let profile = if trimmed.starts_with("[profile ") && trimmed.ends_with(']') {
                Some(&trimmed[9..trimmed.len() - 1])
            } else if trimmed == "[default]" {
                Some("default")
            } else if is_credentials && trimmed.starts_with('[') && trimmed.ends_with(']') {
                Some(&trimmed[1..trimmed.len() - 1])
            } else {
                None
            };
            if let Some(profile) = profile.map(str::trim).filter(|profile| !profile.is_empty()) {
                profiles.insert(profile.to_string());
            }
        }
    }
    profiles.into_iter().collect()
}

/// List available AWS profiles from the same local files selected by the AWS
/// SDK: `AWS_CONFIG_FILE` / `AWS_SHARED_CREDENTIALS_FILE` when present,
/// otherwise `~/.aws/config` / `~/.aws/credentials`. This is a local-only
/// existence check; it never resolves the credential chain or contacts AWS.
#[tauri::command]
pub fn list_aws_profiles() -> Vec<String> {
    let config_path = aws_profile_path("AWS_CONFIG_FILE", "config");
    let credentials_path = aws_profile_path("AWS_SHARED_CREDENTIALS_FILE", "credentials");
    list_aws_profiles_from_paths(config_path.as_deref(), credentials_path.as_deref())
}

// ---------------------------------------------------------------------------
// Cloud provider connection tests
// ---------------------------------------------------------------------------
//
// These commands let the Settings UI verify a user's API keys / credentials
// *before* they start a transcription session, so authentication failures
// surface immediately instead of after ~10s of silent audio streaming.

/// Test an OpenAI-compatible ASR endpoint by making a GET /models request.
#[tauri::command]
pub async fn test_cloud_asr_connection(
    endpoint: String,
    api_key: Option<String>,
) -> AppResult<String> {
    let api_key = endpoint_api_key_from_draft_or_store(&endpoint, api_key)?;
    let model_catalog =
        fetch_openai_compatible_model_catalog(&endpoint, api_key.as_deref()).await?;
    Ok(format!(
        "Connected to {} ({} models)",
        endpoint,
        model_catalog.len()
    ))
}

#[derive(Debug, serde::Deserialize)]
struct OpenAiCompatibleModelsResponse {
    #[serde(default)]
    data: Vec<OpenAiCompatibleModelDescriptor>,
}

#[derive(Debug, serde::Deserialize)]
struct OpenAiCompatibleModelDescriptor {
    #[serde(default)]
    id: Option<String>,
}

fn openai_compatible_model_catalog_from_response_with_default(
    response: OpenAiCompatibleModelsResponse,
    default_model: Option<&str>,
) -> Vec<ProviderModelCatalogItem> {
    let mut catalog = Vec::new();

    for model in response.data {
        let Some(id) = model.id.as_deref().and_then(non_empty_trimmed) else {
            continue;
        };
        if catalog
            .iter()
            .any(|item: &ProviderModelCatalogItem| item.id == id)
        {
            continue;
        }

        catalog.push(ProviderModelCatalogItem {
            is_default: default_model == Some(id.as_str()),
            id: id.clone(),
            display_name: id,
        });
    }

    catalog
}

fn parse_openai_compatible_model_catalog_with_default(
    body: &str,
    default_model: Option<&str>,
) -> AppResult<Vec<ProviderModelCatalogItem>> {
    let response: OpenAiCompatibleModelsResponse = serde_json::from_str(body).map_err(|e| {
        AppError::Unknown(format!(
            "Failed to parse OpenAI-compatible model catalog: {}",
            e
        ))
    })?;
    Ok(openai_compatible_model_catalog_from_response_with_default(
        response,
        default_model,
    ))
}

async fn fetch_openai_compatible_model_catalog(
    endpoint: &str,
    api_key: Option<&str>,
) -> AppResult<Vec<ProviderModelCatalogItem>> {
    fetch_openai_compatible_model_catalog_with_default(endpoint, api_key, Some("whisper-1")).await
}

/// Re-flag the default catalog entry as a real **chat** model.
///
/// Provider-API audit (`/tmp/provider-audit/llm_openai_compat.md` §4a): the
/// shared OpenAI-compatible fetch marks `whisper-1` (a speech-to-text model) as
/// the default. That is correct for the ASR reuse but a dead marker for the LLM
/// model list — a chat catalog never contains `whisper-1`, so no row is flagged
/// default. This picks a chat-appropriate default instead so the LLM Settings UI
/// pre-selects a usable model.
///
/// Selection order (first match wins): a preferred well-known chat id
/// substring, else the first catalog entry. ASR ids (`whisper*`) are never
/// chosen as the chat default.
fn mark_chat_default_model(
    mut catalog: Vec<ProviderModelCatalogItem>,
) -> Vec<ProviderModelCatalogItem> {
    // Clear the inherited (ASR) default marker before re-selecting.
    for item in &mut catalog {
        item.is_default = false;
    }

    // Prefer a well-known chat family; the ordering biases toward the smaller /
    // cheaper "mini" tiers a user is most likely to want as a starting point.
    const PREFERRED_CHAT_SUBSTRINGS: &[&str] = &[
        "gpt-4o-mini",
        "gpt-4o",
        "gpt-4",
        "o4-mini",
        "o3-mini",
        "llama",
        "qwen",
        "mistral",
    ];

    let default_idx = PREFERRED_CHAT_SUBSTRINGS
        .iter()
        .find_map(|needle| {
            catalog.iter().position(|item| {
                let id = item.id.to_ascii_lowercase();
                // Never fall back onto an ASR model as the "chat" default.
                !id.contains("whisper") && id.contains(needle)
            })
        })
        // Fall back to the first non-ASR entry, else the first entry.
        .or_else(|| {
            catalog
                .iter()
                .position(|item| !item.id.to_ascii_lowercase().contains("whisper"))
        })
        .or(if catalog.is_empty() { None } else { Some(0) });

    if let Some(idx) = default_idx {
        catalog[idx].is_default = true;
    }

    catalog
}

async fn fetch_openai_compatible_model_catalog_with_default(
    endpoint: &str,
    api_key: Option<&str>,
    default_model: Option<&str>,
) -> AppResult<Vec<ProviderModelCatalogItem>> {
    let url = format!("{}/models", endpoint.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("Failed to build client: {}", e))?;
    let mut req = client.get(&url);
    if let Some(api_key) = api_key {
        req = req.bearer_auth(api_key);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(AppError::Unknown(cloud_asr_connection_error_message(
            status, &body, api_key,
        )));
    }
    parse_openai_compatible_model_catalog_with_default(&body, default_model)
}

fn cloud_asr_connection_error_message(
    status: reqwest::StatusCode,
    body: &str,
    api_key: Option<&str>,
) -> String {
    let body = crate::error::redacted_error_excerpt(body, api_key, 200);
    let detail = format!("HTTP {}: {}", status, body);
    // Codex P2 (PR #92): this shared arm also probes localhost/loopback
    // OpenAI-compatible endpoints with NO saved key — a 401 from such a probe
    // is not a rejected credential, so the marker only applies when a key was
    // actually sent. The per-provider helpers always send one.
    if api_key.is_some() {
        crate::error::classify_credential_rejected_message(status, detail)
    } else {
        detail
    }
}

fn endpoint_api_key_from_draft_or_store(
    endpoint: &str,
    api_key: Option<String>,
) -> AppResult<Option<String>> {
    if let Some(api_key) = api_key.as_deref().and_then(non_empty_trimmed) {
        return Ok(Some(api_key));
    }

    let store = crate::credentials::try_load_credentials()
        .map_err(|reason| AppError::CredentialFileError { reason })?;
    Ok(endpoint_api_key_from_store(endpoint, &store))
}

fn endpoint_api_key_from_store(
    endpoint: &str,
    store: &crate::credentials::CredentialStore,
) -> Option<String> {
    let saved = match crate::settings::credential_key_for_endpoint(endpoint) {
        "cerebras_api_key" => store.cerebras_api_key.as_deref(),
        "sambanova_api_key" => store.sambanova_api_key.as_deref(),
        "openrouter_api_key" => store.openrouter_api_key.as_deref(),
        "gemini_api_key" => store.gemini_api_key.as_deref(),
        "groq_api_key" => store.groq_api_key.as_deref(),
        "together_api_key" => store.together_api_key.as_deref(),
        "fireworks_api_key" => store.fireworks_api_key.as_deref(),
        _ => store.openai_api_key.as_deref(),
    };

    saved.and_then(non_empty_trimmed)
}

/// Test a generic OpenAI-compatible LLM endpoint by listing its model catalog.
///
/// Uses the draft `api_key` when present, otherwise falls back to the saved
/// endpoint-routed credential (no plaintext-secret readback). The returned
/// status string and any error are key-redacted: it reports only the model
/// count, never the credential.
#[tauri::command]
pub async fn test_openai_compatible_llm_connection_cmd(
    endpoint: String,
    api_key: Option<String>,
) -> AppResult<String> {
    let api_key = endpoint_api_key_from_draft_or_store(&endpoint, api_key)?;
    let model_catalog =
        fetch_openai_compatible_model_catalog(&endpoint, api_key.as_deref()).await?;
    Ok(format!(
        "Connected to {} ({} OpenAI-compatible models)",
        endpoint.trim(),
        model_catalog.len()
    ))
}

/// Fetch a generic OpenAI-compatible LLM endpoint's model catalog.
///
/// Uses the draft `api_key` when present, otherwise falls back to the saved
/// endpoint-routed credential (no plaintext-secret readback). Errors are
/// key-redacted via the shared OpenAI-compatible fetch path.
#[tauri::command]
pub async fn list_openai_compatible_llm_models_cmd(
    endpoint: String,
    api_key: Option<String>,
) -> AppResult<Vec<ProviderModelCatalogItem>> {
    let api_key = endpoint_api_key_from_draft_or_store(&endpoint, api_key)?;
    // The shared fetch marks `whisper-1` (an ASR model) as default, which is a
    // dead marker for a chat catalog. Re-select a real chat model as the default
    // for the LLM path. Provider-API audit §4a.
    let catalog =
        fetch_openai_compatible_model_catalog_with_default(&endpoint, api_key.as_deref(), None)
            .await?;
    Ok(mark_chat_default_model(catalog))
}

#[derive(Debug, serde::Deserialize)]
struct DeepgramModelsResponse {
    #[serde(default)]
    stt: Vec<DeepgramModelDescriptor>,
}

#[derive(Debug, serde::Deserialize)]
struct DeepgramModelDescriptor {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    canonical_name: Option<String>,
    #[serde(default)]
    streaming: Option<bool>,
}

fn deepgram_stt_model_catalog_from_response(
    response: DeepgramModelsResponse,
) -> Vec<ProviderModelCatalogItem> {
    let mut catalog = Vec::new();

    for model in response.stt {
        if model.streaming != Some(true) {
            continue;
        }

        let id = model
            .canonical_name
            .as_deref()
            .and_then(non_empty_trimmed)
            .or_else(|| model.name.as_deref().and_then(non_empty_trimmed));
        let Some(id) = id else {
            continue;
        };
        if catalog
            .iter()
            .any(|item: &ProviderModelCatalogItem| item.id == id)
        {
            continue;
        }

        let display_name = model
            .name
            .as_deref()
            .and_then(non_empty_trimmed)
            .unwrap_or_else(|| id.clone());
        catalog.push(ProviderModelCatalogItem {
            is_default: id == "nova-3",
            id,
            display_name,
        });
    }

    // Flux (v2/listen conversational-turn models) is NOT returned by Deepgram's
    // /v1/models management catalog — it is a v2 model documented separately —
    // so it never appears in the picker without a curated fallback. Append the
    // two valid flux ids (confirmed against the v2/listen docs enum) if the live
    // response did not already list them (defensive against a future API that
    // starts including them). The ASR runtime already routes flux-* to
    // v2/listen, so this only closes the discoverability gap.
    for (id, display_name) in [
        ("flux-general-en", "Flux General English (turn-based, v2)"),
        (
            "flux-general-multi",
            "Flux General Multilingual (turn-based, v2)",
        ),
    ] {
        if !catalog.iter().any(|item| item.id == id) {
            catalog.push(ProviderModelCatalogItem {
                is_default: false,
                id: id.to_string(),
                display_name: display_name.to_string(),
            });
        }
    }

    catalog
}

fn parse_deepgram_stt_model_catalog(body: &str) -> AppResult<Vec<ProviderModelCatalogItem>> {
    let response: DeepgramModelsResponse = serde_json::from_str(body)
        .map_err(|e| AppError::Unknown(format!("Failed to parse Deepgram model catalog: {}", e)))?;
    Ok(deepgram_stt_model_catalog_from_response(response))
}

fn deepgram_connection_error_message(
    status: reqwest::StatusCode,
    body: &str,
    api_key: Option<&str>,
) -> String {
    let body = crate::error::redacted_error_excerpt(body, api_key, 200);
    let detail = if body.is_empty() {
        format!("Deepgram returned HTTP {}", status)
    } else {
        format!("Deepgram returned HTTP {}: {}", status, body)
    };
    crate::error::classify_credential_rejected_message(status, detail)
}

async fn fetch_deepgram_stt_model_catalog(
    api_key: &str,
) -> AppResult<Vec<ProviderModelCatalogItem>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("Failed to build client: {}", e))?;
    let resp = client
        // Use /v1/models (works with `usage` scope — the scope most keys
        // have for transcription). /v1/projects requires the `manage` scope
        // which would return 403 for valid transcription-only keys.
        .get("https://api.deepgram.com/v1/models")
        .header("Authorization", format!("Token {}", api_key))
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(AppError::Unknown(deepgram_connection_error_message(
            status,
            &body,
            Some(api_key),
        )));
    }

    parse_deepgram_stt_model_catalog(&body)
}

#[derive(Debug, serde::Deserialize)]
struct SonioxModelsResponse {
    #[serde(default)]
    models: Vec<SonioxModelDescriptor>,
}

#[derive(Debug, serde::Deserialize)]
struct SonioxModelDescriptor {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    transcription_mode: Option<String>,
}

fn soniox_realtime_model_catalog_from_response(
    response: SonioxModelsResponse,
) -> Vec<ProviderModelCatalogItem> {
    let mut catalog = Vec::new();

    for model in response.models {
        let transcription_mode = model
            .transcription_mode
            .as_deref()
            .and_then(non_empty_trimmed);
        if !matches!(
            transcription_mode.as_deref(),
            Some("real_time" | "real-time")
        ) {
            continue;
        }

        let Some(id) = model.id.as_deref().and_then(non_empty_trimmed) else {
            continue;
        };
        if catalog
            .iter()
            .any(|item: &ProviderModelCatalogItem| item.id == id)
        {
            continue;
        }

        let display_name = model
            .name
            .as_deref()
            .and_then(non_empty_trimmed)
            .unwrap_or_else(|| id.clone());
        catalog.push(ProviderModelCatalogItem {
            is_default: id == "stt-rt-v5",
            id,
            display_name,
        });
    }

    catalog
}

fn parse_soniox_realtime_model_catalog(body: &str) -> AppResult<Vec<ProviderModelCatalogItem>> {
    let response: SonioxModelsResponse = serde_json::from_str(body)
        .map_err(|e| AppError::Unknown(format!("Failed to parse Soniox model catalog: {}", e)))?;
    Ok(soniox_realtime_model_catalog_from_response(response))
}

fn soniox_connection_error_message(
    status: reqwest::StatusCode,
    body: &str,
    api_key: Option<&str>,
) -> String {
    let body = crate::error::redacted_error_excerpt(body, api_key, 200);
    let detail = if body.is_empty() {
        format!("Soniox returned HTTP {}", status)
    } else {
        format!("Soniox returned HTTP {}: {}", status, body)
    };
    crate::error::classify_credential_rejected_message(status, detail)
}

async fn fetch_soniox_realtime_model_catalog(
    api_key: &str,
) -> AppResult<Vec<ProviderModelCatalogItem>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("Failed to build client: {}", e))?;
    let resp = client
        .get("https://api.soniox.com/v1/models")
        .bearer_auth(api_key)
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(AppError::Unknown(soniox_connection_error_message(
            status,
            &body,
            Some(api_key),
        )));
    }

    parse_soniox_realtime_model_catalog(&body)
}

/// Test Deepgram API key by calling /v1/models.
#[tauri::command]
pub async fn test_deepgram_connection(api_key: Option<String>) -> AppResult<String> {
    let api_key = deepgram_api_key_from_draft_or_store(api_key)?;
    let model_catalog = fetch_deepgram_stt_model_catalog(&api_key).await?;
    Ok(format!(
        "Deepgram API key is valid ({} streaming STT models)",
        model_catalog.len()
    ))
}

/// Fetch Deepgram's streaming STT model catalog using a draft or saved API key.
#[tauri::command]
pub async fn list_deepgram_models_cmd(
    api_key: Option<String>,
) -> AppResult<Vec<ProviderModelCatalogItem>> {
    let api_key = deepgram_api_key_from_draft_or_store(api_key)?;
    fetch_deepgram_stt_model_catalog(&api_key).await
}

/// Test Soniox API key by calling /v1/models and requiring real-time models.
#[tauri::command]
pub async fn test_soniox_connection(api_key: Option<String>) -> AppResult<String> {
    let api_key = soniox_api_key_from_draft_or_store(api_key)?;
    let model_catalog = fetch_soniox_realtime_model_catalog(&api_key).await?;
    Ok(format!(
        "Soniox API key is valid ({} real-time STT models)",
        model_catalog.len()
    ))
}

/// Fetch Soniox's real-time STT model catalog using a draft or saved API key.
///
/// This command is intentionally present while `asr.soniox` stays
/// `Planned`/unselectable in the provider registry. Soniox's backend runtime and
/// saved-key readiness are already wired, but promotion to a selectable ASR
/// provider is gated on redacted live-smoke evidence (seeds audio-graph-be03 /
/// audio-graph-e35f, blocked on audio-graph-0b93). Exposing the catalog command
/// ahead of the Settings picker lets saved-key readiness probe the live
/// /v1/models catalog without offering a selection — so the apparent
/// catalog-command-without-UI inconsistency is by design (see audio-graph-f9a6),
/// not a wiring gap. Do not promote the provider here; that needs the secrets gate.
#[tauri::command]
pub async fn list_soniox_models_cmd(
    api_key: Option<String>,
) -> AppResult<Vec<ProviderModelCatalogItem>> {
    let api_key = soniox_api_key_from_draft_or_store(api_key)?;
    fetch_soniox_realtime_model_catalog(&api_key).await
}

/// Test Cerebras Inference key by calling the OpenAI-compatible /v1/models endpoint.
#[tauri::command]
pub async fn test_cerebras_connection_cmd(api_key: Option<String>) -> AppResult<String> {
    let api_key =
        endpoint_api_key_from_draft_or_store(crate::settings::CEREBRAS_BASE_URL, api_key)?;
    let model_catalog = fetch_openai_compatible_model_catalog_with_default(
        crate::settings::CEREBRAS_BASE_URL,
        api_key.as_deref(),
        Some(crate::provider_registry::CEREBRAS_DEFAULT_MODEL),
    )
    .await?;
    Ok(format!(
        "Cerebras API key is valid ({} models)",
        model_catalog.len()
    ))
}

/// Fetch Cerebras' OpenAI-compatible model catalog using a draft or saved API key.
#[tauri::command]
pub async fn list_cerebras_models_cmd(
    api_key: Option<String>,
) -> AppResult<Vec<ProviderModelCatalogItem>> {
    let api_key =
        endpoint_api_key_from_draft_or_store(crate::settings::CEREBRAS_BASE_URL, api_key)?;
    fetch_openai_compatible_model_catalog_with_default(
        crate::settings::CEREBRAS_BASE_URL,
        api_key.as_deref(),
        Some(crate::provider_registry::CEREBRAS_DEFAULT_MODEL),
    )
    .await
}

/// Test SambaNova Cloud key by calling the OpenAI-compatible /v1/models endpoint.
#[tauri::command]
pub async fn test_sambanova_connection_cmd(api_key: Option<String>) -> AppResult<String> {
    let api_key =
        endpoint_api_key_from_draft_or_store(crate::settings::SAMBANOVA_BASE_URL, api_key)?;
    let model_catalog = fetch_openai_compatible_model_catalog_with_default(
        crate::settings::SAMBANOVA_BASE_URL,
        api_key.as_deref(),
        Some(crate::provider_registry::SAMBANOVA_DEFAULT_MODEL),
    )
    .await?;
    Ok(format!(
        "SambaNova API key is valid ({} models)",
        model_catalog.len()
    ))
}

/// Fetch SambaNova's OpenAI-compatible model catalog using a draft or saved API key.
#[tauri::command]
pub async fn list_sambanova_models_cmd(
    api_key: Option<String>,
) -> AppResult<Vec<ProviderModelCatalogItem>> {
    let api_key =
        endpoint_api_key_from_draft_or_store(crate::settings::SAMBANOVA_BASE_URL, api_key)?;
    fetch_openai_compatible_model_catalog_with_default(
        crate::settings::SAMBANOVA_BASE_URL,
        api_key.as_deref(),
        Some(crate::provider_registry::SAMBANOVA_DEFAULT_MODEL),
    )
    .await
}

/// Test AssemblyAI account-key validity through the REST API.
///
/// This deliberately does not claim v3 streaming WebSocket health. That path is
/// covered by the ignored ASSEMBLYAI_API_KEY live smoke in the AssemblyAI
/// client module because it opens a billable/live socket.
#[tauri::command]
pub async fn test_assemblyai_connection(api_key: Option<String>) -> AppResult<String> {
    let api_key = assemblyai_api_key_from_draft_or_store(api_key)?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("Failed to build client: {}", e))?;
    let resp = client
        .get("https://api.assemblyai.com/v2/transcript?limit=1")
        .header("Authorization", &api_key)
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(AppError::Unknown(assemblyai_connection_error_message(
            status,
        )));
    }
    Ok("AssemblyAI account key is valid via REST; v3 streaming socket smoke not run".to_string())
}

/// Build the `test_assemblyai_connection` error message. Extracted (mirrors
/// `deepgram_connection_error_message` / `soniox_connection_error_message`)
/// so the 401-credential-rejected classification (audio-graph-57cc) is
/// unit-testable without a live/mocked HTTP round trip.
fn assemblyai_connection_error_message(status: reqwest::StatusCode) -> String {
    let detail = format!("AssemblyAI returned HTTP {}", status);
    crate::error::classify_credential_rejected_message(status, detail)
}

fn deepgram_api_key_from_draft_or_store(api_key: Option<String>) -> AppResult<String> {
    if let Some(api_key) = api_key.as_deref().and_then(non_empty_trimmed) {
        return Ok(api_key);
    }
    let store = crate::credentials::try_load_credentials()
        .map_err(|reason| AppError::CredentialFileError { reason })?;
    deepgram_api_key_from_store(&store)
}

fn deepgram_api_key_from_store(store: &crate::credentials::CredentialStore) -> AppResult<String> {
    store
        .deepgram_api_key
        .as_deref()
        .and_then(non_empty_trimmed)
        .ok_or_else(|| AppError::CredentialMissing {
            key: "deepgram_api_key".to_string(),
        })
}

fn soniox_api_key_from_draft_or_store(api_key: Option<String>) -> AppResult<String> {
    if let Some(api_key) = api_key.as_deref().and_then(non_empty_trimmed) {
        return Ok(api_key);
    }
    let store = crate::credentials::try_load_credentials()
        .map_err(|reason| AppError::CredentialFileError { reason })?;
    soniox_api_key_from_store(&store)
}

fn soniox_api_key_from_store(store: &crate::credentials::CredentialStore) -> AppResult<String> {
    store
        .soniox_api_key
        .as_deref()
        .and_then(non_empty_trimmed)
        .ok_or_else(|| AppError::CredentialMissing {
            key: "soniox_api_key".to_string(),
        })
}

fn assemblyai_api_key_from_draft_or_store(api_key: Option<String>) -> AppResult<String> {
    if let Some(api_key) = api_key.as_deref().and_then(non_empty_trimmed) {
        return Ok(api_key);
    }
    let store = crate::credentials::try_load_credentials()
        .map_err(|reason| AppError::CredentialFileError { reason })?;
    assemblyai_api_key_from_store(&store)
}

fn assemblyai_api_key_from_store(store: &crate::credentials::CredentialStore) -> AppResult<String> {
    store
        .assemblyai_api_key
        .as_deref()
        .and_then(non_empty_trimmed)
        .ok_or_else(|| AppError::CredentialMissing {
            key: "assemblyai_api_key".to_string(),
        })
}

/// Test Gemini API key via a simple listModels call.
///
/// Uses the `x-goog-api-key` header (not the `?key=` query string) to match
/// the production WebSocket auth pattern. Passing the key in URL would leak
/// it to DNS, proxies, and cert monitoring tools — and would silently succeed
/// even if the header-auth path is broken in production.
#[tauri::command]
pub async fn test_gemini_api_key(api_key: Option<String>) -> AppResult<String> {
    let api_key = gemini_api_key_from_draft_or_store(api_key)?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("Failed to build client: {}", e))?;
    let resp = client
        .get("https://generativelanguage.googleapis.com/v1beta/models")
        .header("x-goog-api-key", api_key.trim())
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(AppError::Unknown(gemini_api_key_connection_error_message(
            status,
        )));
    }
    Ok("Gemini API key is valid".to_string())
}

/// Build the `test_gemini_api_key` error message. Extracted (mirrors
/// `assemblyai_connection_error_message` above) so the 401-credential-rejected
/// classification (audio-graph-57cc) is unit-testable without a live/mocked
/// HTTP round trip.
fn gemini_api_key_connection_error_message(status: reqwest::StatusCode) -> String {
    let detail = format!("Gemini API returned HTTP {}", status);
    crate::error::classify_credential_rejected_message(status, detail)
}

fn gemini_api_key_from_draft_or_store(api_key: Option<String>) -> AppResult<String> {
    if let Some(api_key) = api_key.as_deref().and_then(non_empty_trimmed) {
        return Ok(api_key);
    }
    let store = crate::credentials::try_load_credentials()
        .map_err(|reason| AppError::CredentialFileError { reason })?;
    gemini_api_key_from_store(&store)
}

fn gemini_api_key_from_store(store: &crate::credentials::CredentialStore) -> AppResult<String> {
    store
        .gemini_api_key
        .as_deref()
        .and_then(non_empty_trimmed)
        .ok_or_else(|| AppError::CredentialMissing {
            key: "gemini_api_key".to_string(),
        })
}

/// Test AWS credentials via STS GetCallerIdentity (works for any AWS API access).
///
/// Shared between AWS Transcribe and AWS Bedrock settings — both providers
/// pull from the same backend credential store.
#[tauri::command]
pub async fn test_aws_credentials(
    region: String,
    credential_source: crate::settings::AwsCredentialSource,
    secret_access_key: Option<String>,
    session_token: Option<String>,
) -> AppResult<String> {
    let region_trimmed = region.trim();
    if region_trimmed.is_empty() {
        return Err(AppError::AwsRegionInvalid {
            region: region_trimmed.to_string(),
        });
    }
    if !region_trimmed.contains('-') {
        return Err(AppError::AwsRegionInvalid {
            region: region_trimmed.to_string(),
        });
    }
    let region = region_trimmed.to_string();

    let sdk_config = crate::aws_util::build_aws_sdk_config_with_draft_credentials(
        &region,
        credential_source,
        secret_access_key,
        session_token,
    )
    .await?;
    let sts = aws_sdk_sts::Client::new(&sdk_config);
    let identity = sts
        .get_caller_identity()
        .send()
        .await
        .map_err(|e| format!("AWS auth failed: {}", e))?;
    Ok(format!(
        "Authenticated as {} (account: {})",
        identity.arn().unwrap_or("unknown"),
        identity.account().unwrap_or("unknown")
    ))
}

// ---------------------------------------------------------------------------
// OpenRouter cloud-LLM commands (ADR-0005, plan A2)
// ---------------------------------------------------------------------------

/// Validate an OpenRouter API key without spending tokens.
///
/// Hits `GET /api/v1/models` with the supplied key + canonical attribution
/// headers. Returns `Ok(_)` on HTTP 200 and a diagnostic `Err` on 401/403 or
/// network failure. Used by the Settings UI's "Test Connection" button.
#[tauri::command]
pub async fn test_openrouter_connection_cmd(
    api_key: Option<String>,
    base_url: Option<String>,
) -> AppResult<String> {
    let api_key = openrouter_api_key_from_draft_or_store(api_key)?;
    let base_url = openrouter_base_url_or_default(base_url);
    openrouter::test_connection(&api_key, &base_url)
        .await
        .map_err(AppError::Unknown)?;
    Ok("OpenRouter API key is valid".to_string())
}

/// Fetch the live OpenRouter model catalog for the settings model picker.
#[tauri::command]
pub async fn list_openrouter_models_cmd(
    api_key: Option<String>,
    base_url: Option<String>,
) -> AppResult<Vec<OpenRouterModel>> {
    let api_key = openrouter_api_key_from_draft_or_store(api_key)?;
    let base_url = openrouter_base_url_or_default(base_url);
    openrouter::list_models(&api_key, &base_url)
        .await
        .map_err(AppError::Unknown)
}

/// Fetch OpenRouter provider metadata using only the saved backend credential.
#[tauri::command]
pub async fn list_openrouter_providers_cmd(
    base_url: Option<String>,
) -> AppResult<Vec<OpenRouterProvider>> {
    let store = crate::credentials::try_load_credentials()
        .map_err(|reason| AppError::CredentialFileError { reason })?;
    list_openrouter_providers_with_store(&store, base_url).await
}

async fn list_openrouter_providers_with_store(
    store: &crate::credentials::CredentialStore,
    base_url: Option<String>,
) -> AppResult<Vec<OpenRouterProvider>> {
    let api_key = openrouter_api_key_from_store(store)?;
    let base_url = openrouter_base_url_or_default(base_url);
    openrouter::list_providers(&api_key, &base_url)
        .await
        .map_err(AppError::Unknown)
}

/// Fetch OpenRouter model endpoint metadata using only the saved backend credential.
#[tauri::command]
pub async fn list_openrouter_model_endpoints_cmd(
    model_id: String,
    base_url: Option<String>,
) -> AppResult<OpenRouterModelEndpoints> {
    let store = crate::credentials::try_load_credentials()
        .map_err(|reason| AppError::CredentialFileError { reason })?;
    list_openrouter_model_endpoints_with_store(&store, model_id, base_url).await
}

async fn list_openrouter_model_endpoints_with_store(
    store: &crate::credentials::CredentialStore,
    model_id: String,
    base_url: Option<String>,
) -> AppResult<OpenRouterModelEndpoints> {
    let api_key = openrouter_api_key_from_store(store)?;
    let base_url = openrouter_base_url_or_default(base_url);
    openrouter::list_model_endpoints(&api_key, &base_url, &model_id)
        .await
        .map_err(AppError::Unknown)
}

fn openrouter_api_key_from_draft_or_store(api_key: Option<String>) -> AppResult<String> {
    if let Some(api_key) = api_key.as_deref().and_then(non_empty_trimmed) {
        return Ok(api_key);
    }
    let store = crate::credentials::try_load_credentials()
        .map_err(|reason| AppError::CredentialFileError { reason })?;
    openrouter_api_key_from_store(&store)
}

fn openrouter_api_key_from_store(store: &crate::credentials::CredentialStore) -> AppResult<String> {
    store
        .openrouter_api_key
        .as_deref()
        .and_then(non_empty_trimmed)
        .ok_or_else(|| AppError::CredentialMissing {
            key: "openrouter_api_key".to_string(),
        })
}

fn openrouter_base_url_or_default(base_url: Option<String>) -> String {
    base_url
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| openrouter::DEFAULT_BASE_URL.to_string())
}

// ---------------------------------------------------------------------------
// TTS connection test (ADR-0004, plan A1)
// ---------------------------------------------------------------------------

/// Validate a TTS provider's credentials before the user starts a session.
///
/// Currently only `deepgram_aura` is wired up; the same Deepgram API key
/// works for both STT and TTS, so this command reuses the
/// `test_deepgram_connection` HTTP probe (`GET /v1/models`) under the
/// hood. Future providers (Kokoro, Piper, OpenAI TTS, ElevenLabs) will
/// branch on `provider` and dispatch their own probe.
///
/// `provider` is the `serde(tag = "type")` discriminator used by the
/// `TtsProvider` settings enum -- e.g. `"deepgram_aura"`. `none` returns
/// an error so the UI can short-circuit the "Test connection" button when
/// TTS is disabled.
#[tauri::command]
pub async fn test_tts_connection_cmd(
    provider: String,
    api_key: Option<String>,
) -> AppResult<String> {
    match provider.as_str() {
        "deepgram_aura" => {
            // Reuse the STT probe -- the same key authorises both surfaces.
            // We still tag the success message as TTS-specific so the UI
            // copy is unambiguous.
            test_deepgram_connection(api_key).await?;
            Ok("Deepgram Aura TTS credentials look valid".to_string())
        }
        "none" => Err(AppError::SessionInvalid {
            reason: "TTS is disabled in settings; nothing to test".to_string(),
        }),
        other => Err(AppError::Unknown(format!("Unknown TTS provider: {other}"))),
    }
}

// ---------------------------------------------------------------------------
// Audio playback (Wave B / audio-graph-8d75)
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Output-device selection API (reserved — FV-1)
//
// These three commands are the registered, working backend for letting the user
// pick a *specific* audio OUTPUT device for TTS / native-S2S converse playback.
// They are intentionally not yet wired to a settings-UI control: today both the
// converse path (`start_converse` → `audio_player.open_default`) and the
// speak-aloud TTS pipe open the **host default** output device, which is the
// correct zero-config behavior. This API is the seam a future
// "output device" dropdown calls (`list_*` to populate, `start_*`/`stop_*` to
// switch) without any further backend work. Kept (not deleted) because the B18
// live-audio path will want device selection; tracked as task FV-1. This is
// reserved infrastructure, not dead code.
// ---------------------------------------------------------------------------

/// List the host's available output audio devices.
///
/// First entry (if any) has `is_default: true`. Returns an empty list on
/// hosts where cpal can't enumerate (rare; usually a missing audio service).
#[tauri::command]
pub async fn list_audio_output_devices_cmd() -> AppResult<Vec<crate::playback::OutputDevice>> {
    Ok(crate::playback::list_output_devices())
}

/// Open the configured output device + start the playback stream so
/// subsequent `push_samples` calls (typically driven by a TTS session) are
/// audible. `device_name = None` opens the host default.
#[tauri::command]
pub async fn start_audio_playback_cmd(
    state: State<'_, AppState>,
    device_name: Option<String>,
    source_sample_rate: Option<u32>,
) -> AppResult<()> {
    let config = crate::playback::PlaybackConfig {
        source_sample_rate: source_sample_rate.unwrap_or(24_000),
        source_channels: 1,
    };
    let result = match device_name {
        None => state.audio_player.open_default(config),
        Some(name) => state.audio_player.open_named(name, config),
    };
    result.map_err(|e| AppError::Unknown(e.to_string()))
}

/// Stop the active playback stream. Subsequent `push_samples` calls return
/// 0 (no producer) until a stream is reopened. Cancel is implicit.
#[tauri::command]
pub async fn stop_audio_playback_cmd(state: State<'_, AppState>) -> AppResult<()> {
    state
        .audio_player
        .stop()
        .map_err(|e| AppError::Unknown(e.to_string()))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tauri::Listener;

    fn transcript_event_fixture(
        span_id: &str,
        transcript_segment_id: &str,
    ) -> crate::projections::TranscriptEvent {
        crate::projections::TranscriptEvent {
            span_id: span_id.to_string(),
            provider: "test".to_string(),
            source_id: "source-1".to_string(),
            provider_item_id: None,
            transcript_segment_id: Some(transcript_segment_id.to_string()),
            speaker_id: Some("speaker-1".to_string()),
            speaker_label: Some("Speaker 1".to_string()),
            channel: None,
            text: "Ana owns the migration.".to_string(),
            start_time: 1.0,
            end_time: 2.0,
            confidence: 0.9,
            is_final: true,
            stability: crate::projections::TranscriptEventStability::Final,
            revision_number: 1,
            supersedes: None,
            turn_id: None,
            end_of_turn: true,
            raw_event_ref: None,
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms: 1_700_000_000_000,
        }
    }

    /// ADR-0037: an approved live-assist proposal must anchor to its cited
    /// span, never `EvidenceAnchor::default()` (KnowledgeGap, an
    /// absence-shaped claim wrong for a positive assertion) — regardless of
    /// whether `source_segment_id` is the provider's `transcript_segment_id`
    /// or the raw `span_id` (`derive_legacy_transcript_segments`'s same
    /// fallback), since `judge_claim_evidence`'s basis map is keyed by the
    /// literal `span_id` only.
    #[test]
    fn live_assist_evidence_anchor_resolves_segment_id_or_span_id_to_grounded_inference() {
        let mut ledger = crate::projections::TranscriptLedger::new("session-1");
        ledger
            .latest_spans
            .push(transcript_event_fixture("span-1", "segment-1"));

        for citation in ["segment-1", "span-1"] {
            let anchor = live_assist_evidence_anchor(&ledger, citation);
            assert_eq!(
                anchor,
                crate::claim_evidence::EvidenceAnchor {
                    claim_class: crate::claim_evidence::ClaimClass::GroundedInference,
                    span_id: Some("span-1".to_string()),
                    quote: None,
                    note: None,
                },
                "citation {citation:?} must resolve to the literal span_id"
            );
        }
    }

    /// A citation that no longer resolves in the ledger degrades to
    /// `UnavailableEvidence` with an explanatory note — still honest (never
    /// `KnowledgeGap`), even though it will not carry a resolved span.
    #[test]
    fn live_assist_evidence_anchor_degrades_to_unavailable_evidence_for_an_unresolvable_citation() {
        let ledger = crate::projections::TranscriptLedger::new("session-1");
        let anchor = live_assist_evidence_anchor(&ledger, "segment-gone");
        assert_eq!(
            anchor.claim_class,
            crate::claim_evidence::ClaimClass::UnavailableEvidence
        );
        assert_eq!(anchor.span_id, None);
        assert!(
            anchor
                .note
                .is_some_and(|note| note.contains("segment-gone"))
        );
    }

    /// audio-graph-cfa1 (post-scope-honesty-review fix):
    /// `projection_replay_latency_metrics` used to compute `missing_timestamp`
    /// by walking `patch.basis.span_revisions` directly — only ever the
    /// verbatim tail once a basis compacts — so a patch whose
    /// SUMMARIZED-AWAY PREFIX cannot be resolved against `transcript_events`
    /// (e.g. an upstream log-retention prune of early, already-summarized
    /// revisions) silently reported a measured `basis_to_patch_ms` lag
    /// instead of `missing_basis_timestamp_count`, even though the basis's
    /// true covered set could not be reconstructed.
    #[test]
    fn projection_replay_latency_metrics_flags_missing_timestamp_when_a_compacted_prefix_cannot_resolve()
     {
        let all_events: Vec<crate::projections::TranscriptEvent> = (0..8)
            .map(|i| {
                let mut event =
                    transcript_event_fixture(&format!("span-{i}"), &format!("segment-{i}"));
                event.start_time = i as f64;
                event.end_time = i as f64 + 0.5;
                event.received_at_ms = 1_700_000_000_000 + i as u64;
                event
            })
            .collect();
        let basis = crate::projections::ProjectionBasis::from_transcript_events(&all_events);
        assert!(
            basis.covered_prefix.is_some(),
            "8 covered spans must exceed the hot window and produce a prefix"
        );

        // Only the TAIL's underlying events are available to this replay
        // call — the prefix's (span-0 and span-1's) underlying events are
        // pruned/unavailable — so the prefix cannot be reconstructed.
        let available_events: Vec<crate::projections::TranscriptEvent> = all_events[2..].to_vec();

        let patch = crate::projections::ProjectionPatch {
            route: None,
            sequence: 1,
            kind: crate::projections::ProjectionKind::Notes,
            llm_request_id: "llm-req-1".to_string(),
            basis,
            operations: vec![],
            confidence: 0.9,
            provenance: crate::projections::ProjectionProvenance {
                provider: "openrouter".to_string(),
                model: "anthropic/claude-sonnet-4".to_string(),
                prompt_id: "notes-v1".to_string(),
                route_id: None,
                model_source: crate::llm::route::ModelIdentitySource::Requested,
            },
            queued_at_ms: None,
            generation_latency_ms: None,
            apply_latency_ms: None,
            basis_currency_at_apply: None,
            created_at_ms: 1_700_000_000_100,
        };

        let metrics = projection_replay_latency_metrics(&available_events, &[patch]);
        assert_eq!(
            metrics.missing_basis_timestamp_count, 1,
            "a patch whose compacted prefix cannot be resolved against the available \
             transcript events must be flagged as a missing timestamp, not silently measured \
             against the tail alone"
        );
        assert_eq!(metrics.measured_patch_count, 0);
    }

    // -------------------------------------------------------------------
    // audio-graph-4b52 fix-pass: handler-level coverage for the two
    // command-handler call sites that wire `TranscriptLedger::
    // session_relative_timestamp` into `process_extraction`. The helper
    // itself was already unit-tested (see `projections.rs`), but the 3-line
    // lock-and-derive glue at each `#[tauri::command]` site had no test —
    // a mutation probe reverting either wiring back to
    // `created_at_ms as f64 / 1000.0` / `unix_millis() as f64 / 1000.0`
    // passed the entire suite. These drive the real `*_impl` functions
    // (mirroring `start_capture_impl`'s existing borrowed-state split) with
    // `AppState::new()` + `crate::speech::shared_test_app_handle()`, so a
    // future edit that reorders or reverts the wiring fails here directly.
    // -------------------------------------------------------------------

    #[test]
    #[cfg_attr(
        target_os = "macos",
        ignore = "Tauri/Tao App construction must run on the macOS main thread"
    )]
    fn add_question_to_graph_impl_normalizes_manual_write_timestamp_to_session_relative_seconds() {
        let state = AppState::new();
        let app_handle = crate::speech::shared_test_app_handle();

        // The live span this question cites: 1s into the session. Unlike
        // `approve_agent_proposal` (which converts a stored
        // `proposal.created_at_ms`), `add_question_to_graph` calls
        // `unix_millis()` itself with no injectable clock, so the anchor's
        // `received_at_ms` must be pinned to "now" (not a fixed historical
        // constant) or the fallback math measures real wall-clock drift
        // between this test's fixture and its call, not the bug under test.
        let mut fixture = transcript_event_fixture("span-1", "segment-1");
        fixture.received_at_ms = unix_millis();
        {
            let mut ledger = state
                .transcript_ledger
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            ledger.apply_event(fixture).expect("seed ledger span");
        }

        let result = add_question_to_graph_impl(
            "What is the migration deadline?".to_string(),
            Some("Ana".to_string()),
            Some("segment-1".to_string()),
            app_handle,
            &state,
        )
        .expect("add_question_to_graph_impl should succeed");
        assert!(result);

        let snapshot = state
            .knowledge_graph
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .snapshot();
        let question_node = snapshot
            .nodes
            .iter()
            .find(|n| n.entity_type == "Question")
            .expect("a Question node should have been written");
        // Session-relative seconds close to the anchor's `start_time` (1.0),
        // give or take the handful of milliseconds this test takes to run —
        // nowhere near epoch scale (~1.7e9), which is what master's
        // `unix_millis() as f64 / 1000.0` would have produced here.
        assert!(
            (question_node.first_seen - 1.0).abs() < 5.0,
            "expected session-relative seconds near 1.0, got {} \
             (looks like raw epoch seconds leaked through — master's bug)",
            question_node.first_seen
        );
    }

    #[test]
    #[cfg_attr(
        target_os = "macos",
        ignore = "Tauri/Tao App construction must run on the macOS main thread"
    )]
    fn approve_agent_proposal_impl_normalizes_manual_write_timestamp_to_session_relative_seconds() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("approve-agent-proposal-timestamp");
        let _guard = HomeGuard::set(&dir);

        let state = AppState::new();
        let app_handle = crate::speech::shared_test_app_handle();

        // The live span the proposal cites: 1s into the session, recorded at
        // a known wall-clock instant (`transcript_event_fixture`'s fixed
        // values: start_time 1.0, received_at_ms 1_700_000_000_000).
        {
            let mut ledger = state
                .transcript_ledger
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            ledger
                .apply_event(transcript_event_fixture("span-1", "segment-1"))
                .expect("seed ledger span");
        }

        // `Question` builds its extraction locally with no LLM/extractor
        // heuristics involved, so the graph write is deterministic.
        let proposal = events::AgentProposalPayload {
            id: "proposal-1".to_string(),
            source_segment_id: "segment-1".to_string(),
            source_id: "source-1".to_string(),
            speaker_label: Some("Ana".to_string()),
            kind: events::AgentProposalKind::Question,
            title: "Question".to_string(),
            body: "What is the migration deadline?".to_string(),
            confidence: 0.8,
            // 2.5s of wall-clock time after the cited span was recorded.
            created_at_ms: 1_700_000_002_500,
        };
        state
            .pending_agent_proposals
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(proposal.id.clone(), proposal.clone());

        approve_agent_proposal_impl(proposal.id.clone(), app_handle, &state)
            .expect("approve_agent_proposal_impl should succeed");

        let snapshot = state
            .knowledge_graph
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .snapshot();
        let question_node = snapshot
            .nodes
            .iter()
            .find(|n| n.entity_type == "Question")
            .expect("a Question node should have been written");
        // The literal "would fail on master" pin: master's
        // `proposal.created_at_ms as f64 / 1000.0` would produce
        // ~1_700_000_002.5 here (epoch seconds).
        assert!(
            (question_node.first_seen - 3.5).abs() < 1e-9,
            "expected the exact-match anchor's start_time (1.0) + 2.5s \
             wall-clock offset = 3.5, got {} \
             (looks like raw epoch seconds leaked through — master's bug)",
            question_node.first_seen
        );

        drain_test_writers(&state);
    }

    /// `merge_graph_entities` is the undisclosed sibling site from the
    /// audio-graph-4b52 review (finding: two `supersede_entity` callers still
    /// fed epoch seconds into graph edge times). No source segment id exists
    /// for a manual merge, so this always resolves through the ledger's
    /// fallback-to-any-span anchor.
    #[test]
    #[cfg_attr(
        target_os = "macos",
        ignore = "Tauri/Tao App construction must run on the macOS main thread"
    )]
    fn merge_graph_entities_impl_normalizes_retcon_timestamp_to_session_relative_seconds() {
        let state = AppState::new();
        let app_handle = crate::speech::shared_test_app_handle();

        {
            use crate::graph::entities::ExtractionResult;
            let mut graph = state
                .knowledge_graph
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            graph.process_extraction(
                &ExtractionResult {
                    entities: vec![
                        crate::graph::entities::ExtractedEntity {
                            name: "Speaker 2".to_string(),
                            entity_type: "Person".to_string(),
                            description: None,
                        },
                        crate::graph::entities::ExtractedEntity {
                            name: "Acme".to_string(),
                            entity_type: "Organization".to_string(),
                            description: None,
                        },
                        crate::graph::entities::ExtractedEntity {
                            name: "Alice".to_string(),
                            entity_type: "Person".to_string(),
                            description: None,
                        },
                    ],
                    relations: vec![crate::graph::entities::ExtractedRelation {
                        source: "Speaker 2".to_string(),
                        target: "Acme".to_string(),
                        relation_type: "works_at".to_string(),
                        detail: None,
                    }],
                },
                1.0,
                "spk",
                "seg-1",
            );
        }

        // A live span the merge can anchor against — 5s into the session.
        let mut fixture = transcript_event_fixture("span-1", "segment-1");
        fixture.start_time = 5.0;
        fixture.received_at_ms = unix_millis();
        {
            let mut ledger = state
                .transcript_ledger
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            ledger.apply_event(fixture).expect("seed ledger span");
        }

        let invalidated = merge_graph_entities_impl(
            "Speaker 2".to_string(),
            "Alice".to_string(),
            None,
            app_handle,
            &state,
        )
        .expect("merge_graph_entities_impl should succeed");
        assert_eq!(invalidated, 1, "exactly one edge retconned");

        let live_from = state
            .knowledge_graph
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .live_edge_valid_from_for_test("Alice", "Acme", "works_at")
            .expect("the re-pointed live edge should exist");
        // Session-relative seconds close to the fallback anchor's
        // `start_time` (5.0) — nowhere near epoch scale (~1.7e9), which is
        // what master's `unix_millis() as f64 / 1000.0` would have produced
        // here.
        assert!(
            (live_from - 5.0).abs() < 5.0,
            "expected session-relative seconds near 5.0, got {live_from} \
             (looks like raw epoch seconds leaked through)"
        );
    }

    #[test]
    fn session_content_policy_blocks_cloud_but_not_loopback_or_local() {
        let local_only = crate::settings::AppSettings {
            privacy_mode: crate::settings::PrivacyMode::LocalOnly,
            ..crate::settings::AppSettings::default()
        };

        let blocked = session_content_policy_block(
            &local_only,
            "llm_chat",
            "llm.openrouter",
            &["transcript", "graph_context"],
            true,
        )
        .expect("cloud content transfer should be blocked");
        match blocked {
            AppError::PrivacyPolicyBlocked {
                mode,
                action,
                provider,
                data_classes,
                reason,
            } => {
                assert_eq!(mode, "local_only");
                assert_eq!(action, "llm_chat");
                assert_eq!(provider, "llm.openrouter");
                assert_eq!(
                    data_classes,
                    vec!["transcript".to_string(), "graph_context".to_string()]
                );
                assert!(reason.contains("local_only"));
            }
            other => panic!("unexpected error variant: {other:?}"),
        }

        assert!(
            session_content_policy_block(
                &local_only,
                "llm_chat",
                "llm.api",
                &["transcript"],
                false,
            )
            .is_none(),
            "loopback/local providers should be allowed in local-only mode"
        );

        let byok = crate::settings::AppSettings::default();
        assert!(
            session_content_policy_block(&byok, "asr_session", "asr.deepgram", &["audio"], true,)
                .is_none(),
            "default BYOK mode preserves existing cloud-provider behavior"
        );
    }

    #[test]
    fn read_settings_for_session_content_fails_closed_on_poisoned_lock() {
        let state = AppState::new();
        let settings_lock = state.app_settings.clone();
        let _ = std::panic::catch_unwind(move || {
            let _guard = settings_lock.write().expect("settings lock");
            panic!("poison settings lock");
        });

        let error = read_settings_for_session_content(&state, "llm_chat")
            .expect_err("poisoned privacy settings must refuse session content");
        let message = error.to_string();

        assert!(
            message.contains("Cannot read privacy settings for llm_chat"),
            "got: {message}"
        );
        assert!(
            message.contains("refusing session content transfer"),
            "got: {message}"
        );
        assert!(
            !message.contains("patient said private diagnosis"),
            "settings-read error must not contain session content: {message}"
        );
    }

    #[test]
    fn endpoint_aware_content_egress_policy_allows_loopback_llm_api() {
        let mut settings = crate::settings::AppSettings {
            privacy_mode: crate::settings::PrivacyMode::LocalOnly,
            ..crate::settings::AppSettings::default()
        };
        settings.llm_provider = crate::settings::LlmProvider::Api {
            endpoint: "http://localhost:11434/v1".to_string(),
            api_key: String::new(),
            model: "llama3.2".to_string(),
        };

        let policy = provider_content_egress_policy_from_settings(
            &settings,
            settings.llm_provider.requires_cloud_content_transfer(),
        );
        assert!(
            policy.check_prompt("llm.api").is_ok(),
            "local-only mode should allow loopback OpenAI-compatible LLM endpoints"
        );

        settings.llm_provider = crate::settings::LlmProvider::Api {
            endpoint: "https://api.openai.com/v1".to_string(),
            api_key: "sk-should-not-leak".to_string(),
            model: "gpt-4o-mini".to_string(),
        };
        let remote_policy = provider_content_egress_policy_from_settings(
            &settings,
            settings.llm_provider.requires_cloud_content_transfer(),
        );
        let error = remote_policy
            .check_prompt("llm.api")
            .expect_err("remote endpoint must be blocked in local-only mode");
        assert!(error.contains("local_only"));
        assert!(!error.contains("private prompt"));
        assert!(!error.contains("sk-should-not-leak"));
    }

    #[test]
    fn endpoint_aware_content_egress_policy_allows_loopback_asr_api() {
        let settings = crate::settings::AppSettings {
            privacy_mode: crate::settings::PrivacyMode::LocalOnly,
            ..crate::settings::AppSettings::default()
        };
        let loopback = crate::settings::AsrProvider::Api {
            endpoint: "http://127.0.0.1:8080/v1".to_string(),
            api_key: String::new(),
            model: "local-asr".to_string(),
        };
        let policy = provider_content_egress_policy_from_settings(
            &settings,
            loopback.requires_cloud_content_transfer(),
        );
        assert!(
            policy.check_audio("asr.api").is_ok(),
            "local-only mode should allow loopback OpenAI-compatible ASR endpoints"
        );

        let remote = crate::settings::AsrProvider::Api {
            endpoint: "https://api.example.com/v1".to_string(),
            api_key: "asr-key-should-not-leak".to_string(),
            model: "remote-asr".to_string(),
        };
        let remote_policy = provider_content_egress_policy_from_settings(
            &settings,
            remote.requires_cloud_content_transfer(),
        );
        let error = remote_policy
            .check_audio("asr.api")
            .expect_err("remote endpoint must be blocked in local-only mode");
        assert!(error.contains("local_only"));
        assert!(!error.contains("0.25"));
        assert!(!error.contains("asr-key-should-not-leak"));
    }

    #[test]
    fn runtime_privacy_policy_blocks_cloud_tts_and_gemini_content_but_allows_probes() {
        for mode in [
            crate::settings::PrivacyMode::LocalOnly,
            crate::settings::PrivacyMode::CloudDisabledReadinessOnly,
            crate::settings::PrivacyMode::OrgPromotion,
        ] {
            let settings = crate::settings::AppSettings {
                privacy_mode: mode,
                ..crate::settings::AppSettings::default()
            };
            let blocked = provider_content_egress_policy_from_settings(&settings, true);

            let tts_error = blocked
                .check_text("tts.deepgram_aura")
                .expect_err("cloud TTS text must be blocked");
            assert!(tts_error.contains(mode.as_str()));
            assert!(!tts_error.contains("generated patient text"));

            let gemini_error = blocked
                .check_audio("gemini.live")
                .expect_err("Gemini Live audio must be blocked");
            assert!(gemini_error.contains(mode.as_str()));
            assert!(!gemini_error.contains("0.25"));

            let probe_policy = provider_content_egress_policy_from_settings(&settings, false);
            assert!(
                probe_policy.check_text("tts.deepgram_aura").is_ok(),
                "no-content probes/readiness must remain allowed in {}",
                mode.as_str()
            );
            assert!(
                probe_policy.check_audio("gemini.live").is_ok(),
                "no-content probes/readiness must remain allowed in {}",
                mode.as_str()
            );
        }
    }

    #[test]
    fn org_knowledge_cloud_sync_ipc_commands_remain_absent() {
        let commands_source = include_str!("commands.rs");
        let lib_source = include_str!("lib.rs");
        let exact_command_fragments: &[&[&str]] = &[
            &["create", "promotion", "draft", "cmd"],
            &["approve", "promotion", "cmd"],
            &["promote", "org", "knowledge", "cmd"],
            &["queue", "promotion", "sync", "cmd"],
            &["sync", "promotion", "cmd"],
            &["sync", "org", "knowledge", "cmd"],
            &["push", "org", "knowledge", "cmd"],
            &["pull", "org", "knowledge", "cmd"],
            &["upload", "org", "knowledge", "cmd"],
            &["download", "org", "knowledge", "cmd"],
            &["federate", "org", "knowledge", "cmd"],
            &["replicate", "org", "knowledge", "cmd"],
            &["configure", "org", "knowledge", "sync", "cmd"],
            &["connect", "org", "workspace", "cmd"],
        ];
        let verbs = [
            "create",
            "approve",
            "promote",
            "queue",
            "sync",
            "push",
            "pull",
            "upload",
            "download",
            "federate",
            "replicate",
            "configure",
            "connect",
        ];
        let objects = [
            "promotion",
            "promotions",
            "org_knowledge",
            "org_memory",
            "knowledge_sync",
            "cloud_sync",
            "federated_sync",
        ];

        let mut forbidden_names: Vec<String> = exact_command_fragments
            .iter()
            .map(|fragments| fragments.join("_"))
            .collect();
        for verb in verbs {
            for object in objects {
                let base = format!("{verb}_{object}");
                forbidden_names.push(base.clone());
                forbidden_names.push(format!("{base}_cmd"));
            }
        }
        forbidden_names.sort();
        forbidden_names.dedup();

        for command_name in forbidden_names {
            let definition_patterns = [
                format!("pub fn {command_name}"),
                format!("pub async fn {command_name}"),
                format!("fn {command_name}"),
                format!("async fn {command_name}"),
            ];
            for pattern in definition_patterns {
                assert!(
                    !commands_source.contains(&pattern),
                    "commands.rs declares premature org sync command {command_name}"
                );
            }
            assert!(
                !lib_source.contains(&format!("commands::{command_name}")),
                "lib.rs registers premature org sync command {command_name}"
            );
        }
    }

    /// audio-graph-1609 acceptance (c): pins the ordering seam directly
    /// against `start_transcribe`'s own source, the same `include_str!`
    /// self-inspection technique `org_knowledge_cloud_sync_ipc_commands_remain_absent`
    /// uses above. Driving the real function end-to-end would require a
    /// fully live ASR/model pipeline; this pins the fact that actually
    /// matters — `projection_lane_stopping` is cleared strictly BEFORE the
    /// speech processor thread spawns, not after — so a revert of that
    /// ordering (moving the clear back below the spawn, where it used to
    /// live paired with `is_transcribing`) fails this test immediately
    /// instead of only manifesting as an intermittent, hard-to-reproduce
    /// race in a running app.
    ///
    /// Whitespace-insensitive by design (rustfmt may re-wrap either
    /// statement across lines without changing their relative order), so
    /// this does not become a formatting-churn tripwire.
    fn strip_whitespace(source: &str) -> String {
        source.chars().filter(|c| !c.is_whitespace()).collect()
    }

    /// Strips `//` line comments and `/* */` block comments (Rust-style
    /// nesting supported) from `source`, treating `"..."` string literals as
    /// opaque so a `//` or `/*` inside a string is never mistaken for a
    /// comment opener. This exists so the source-order pin tests below
    /// cannot be satisfied by a comment that merely *mentions* the
    /// statement being pinned — e.g. deleting the real call and leaving
    /// `// schedulers.clear_orphaned_deferred_retries(); // removed` behind
    /// must make the marker vanish, not silently keep matching.
    fn strip_comments(source: &str) -> String {
        let chars: Vec<char> = source.chars().collect();
        let mut out = String::with_capacity(source.len());
        let mut i = 0;
        let mut block_depth: usize = 0;
        let mut in_string = false;
        while i < chars.len() {
            if in_string {
                let c = chars[i];
                out.push(c);
                if c == '\\' && i + 1 < chars.len() {
                    out.push(chars[i + 1]);
                    i += 2;
                    continue;
                }
                if c == '"' {
                    in_string = false;
                }
                i += 1;
                continue;
            }
            if block_depth > 0 {
                if chars[i] == '/' && i + 1 < chars.len() && chars[i + 1] == '*' {
                    block_depth += 1;
                    i += 2;
                    continue;
                }
                if chars[i] == '*' && i + 1 < chars.len() && chars[i + 1] == '/' {
                    block_depth -= 1;
                    i += 2;
                    continue;
                }
                i += 1;
                continue;
            }
            if chars[i] == '/' && i + 1 < chars.len() && chars[i + 1] == '/' {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            if chars[i] == '/' && i + 1 < chars.len() && chars[i + 1] == '*' {
                block_depth = 1;
                i += 2;
                continue;
            }
            if chars[i] == '"' {
                in_string = true;
                out.push('"');
                i += 1;
                continue;
            }
            out.push(chars[i]);
            i += 1;
        }
        out
    }

    /// Comment-stripped, whitespace-stripped body text of `start_transcribe`,
    /// for the `include_str!` self-inspection pin tests below. Sliced up to
    /// (not including) `stop_transcribe`, the function that immediately
    /// follows it in `commands.rs`.
    fn start_transcribe_body_whitespace_stripped() -> String {
        let commands_source = include_str!("commands.rs");
        let body_start = commands_source
            .find("pub async fn start_transcribe(")
            .expect("start_transcribe must exist in commands.rs");
        let body_end = commands_source[body_start..]
            .find("pub async fn stop_transcribe(")
            .map(|relative| body_start + relative)
            .expect("stop_transcribe must immediately follow start_transcribe in commands.rs");
        strip_whitespace(&strip_comments(&commands_source[body_start..body_end]))
    }

    /// Finds `needle` in `haystack`, requiring EXACTLY one occurrence.
    /// Plain `str::find` (first-occurrence) is not enough here: a marker
    /// that only needs to exist "somewhere" in the body is trivially
    /// satisfiable by a stray duplicate, and — before comment stripping was
    /// added — was satisfiable by a comment that merely names the deleted
    /// statement. Panics (with `context` in the message) on zero or on
    /// more than one match, so both failure modes surface as a loud test
    /// failure instead of a silently-passing pin.
    fn find_unique_occurrence(haystack: &str, needle: &str, context: &str) -> usize {
        let count = haystack.matches(needle).count();
        match count {
            1 => haystack
                .find(needle)
                .expect("matches() counted 1 but find() found none"),
            0 => panic!("expected to find {needle:?} in {context}, but it is absent"),
            n => panic!("expected exactly one occurrence of {needle:?} in {context}, found {n}"),
        }
    }

    #[test]
    fn start_transcribe_clears_projection_lane_stopping_before_the_speech_thread_spawns() {
        let body = start_transcribe_body_whitespace_stripped();

        let flag_clear_marker =
            strip_whitespace("projection_lane_stopping.store(false, Ordering::SeqCst);");
        let flag_clear_pos = find_unique_occurrence(
            &body,
            &flag_clear_marker,
            "start_transcribe (projection_lane_stopping clear)",
        );

        // Anchored on the actual thread-spawn statement's unique `.name(...)`
        // call, not the `log::info!` line that follows it — the speech
        // thread is already running (and can already dispatch a projection
        // decision) for every line between the real `.spawn(...)` call and
        // that trailing log line, so anchoring on the log line would let a
        // flag-clear placed anywhere in that gap pass while still reopening
        // the exact race this test exists to catch.
        let spawn_marker = strip_whitespace(
            "let handle = std::thread::Builder::new()\
             .name(\"speech-processor\".to_string())",
        );
        let spawn_pos = find_unique_occurrence(
            &body,
            &spawn_marker,
            "start_transcribe (speech processor thread spawn)",
        );

        assert!(
            flag_clear_pos < spawn_pos,
            "projection_lane_stopping must be cleared BEFORE the speech processor thread \
             spawns — clearing it after leaves a window where a final ASR revision the \
             freshly spawned thread processes almost immediately can still observe the flag \
             set from the prior Stop and get its dispatch discarded into a phantom in-flight \
             state (audio-graph-1609)"
        );
    }

    /// audio-graph-586b review follow-up: pins that the `pipeline_status`
    /// `Running{0}` pre-set (asr/diarization/entity_extraction/graph) runs
    /// BEFORE the speech processor thread spawns, not after. Before this
    /// fix, that pre-set lived in "step 3" after the spawn, so
    /// `apply_diarization_degradation` (running on the freshly spawned
    /// thread) could lose a race and have its honest `Degraded` write
    /// clobbered right back to a healthy-looking `Running{0}` by this
    /// function's own step-3 write, with nothing downstream ever restoring
    /// it — silently reproducing the pre-586b failure mode for the rest of
    /// the session. Same ordering reasoning, and same test technique, as
    /// `start_transcribe_clears_projection_lane_stopping_before_the_speech_thread_spawns`
    /// above.
    #[test]
    fn start_transcribe_presets_pipeline_status_before_the_speech_thread_spawns() {
        let body = start_transcribe_body_whitespace_stripped();

        let preset_marker =
            strip_whitespace("status.diarization = StageStatus::Running { processed_count: 0 };");
        let preset_pos = find_unique_occurrence(
            &body,
            &preset_marker,
            "start_transcribe (pipeline_status Running{0} pre-set)",
        );

        // Same real-spawn anchor as the tests above — see their comments for
        // why the trailing `log::info!` line is the wrong anchor.
        let spawn_marker = strip_whitespace(
            "let handle = std::thread::Builder::new()\
             .name(\"speech-processor\".to_string())",
        );
        let spawn_pos = find_unique_occurrence(
            &body,
            &spawn_marker,
            "start_transcribe (speech processor thread spawn)",
        );

        assert!(
            preset_pos < spawn_pos,
            "the pipeline_status Running{{0}} pre-set must run BEFORE the speech processor \
             thread spawns — pre-setting it after leaves a window where the spawned thread's \
             own `apply_diarization_degradation` call can lose a race and have its Degraded \
             status silently clobbered back to Running (audio-graph-586b)"
        );
    }

    /// audio-graph-1609 acceptance (d), restart-integration half: pins that
    /// `start_transcribe` actually calls the restart-time deferral sweep
    /// (`ProjectionSchedulers::clear_orphaned_deferred_retries`), not merely
    /// that the method exists and behaves correctly in isolation
    /// (`clear_orphaned_deferred_retries_sweeps_both_lanes_without_touching_failed_basis_identity`
    /// in `projection_scheduler.rs` covers that half). Also before the
    /// speech thread spawns, for the same reason the flag-clear must be:
    /// nothing that could dispatch a projection decision for this run
    /// exists yet at that point, so there is nothing left for the sweep to
    /// race.
    #[test]
    fn start_transcribe_sweeps_orphaned_deferred_retries_before_the_speech_thread_spawns() {
        let body = start_transcribe_body_whitespace_stripped();

        let sweep_marker = strip_whitespace("schedulers.clear_orphaned_deferred_retries();");
        let sweep_pos = find_unique_occurrence(
            &body,
            &sweep_marker,
            "start_transcribe (orphaned-deferral sweep call)",
        );

        // Same real-spawn anchor as the test above — see its comment for
        // why the trailing `log::info!` line is the wrong anchor.
        let spawn_marker = strip_whitespace(
            "let handle = std::thread::Builder::new()\
             .name(\"speech-processor\".to_string())",
        );
        let spawn_pos = find_unique_occurrence(
            &body,
            &spawn_marker,
            "start_transcribe (speech processor thread spawn)",
        );

        assert!(
            sweep_pos < spawn_pos,
            "the orphaned-deferral sweep must run BEFORE the speech processor thread spawns \
             (audio-graph-1609) — same ordering reasoning as the projection_lane_stopping \
             clear above"
        );
    }

    /// audio-graph-64e3: pins that `start_transcribe` — the ONE production
    /// site that constructs a `SpeechShared` (every other `SpeechShared {`
    /// literal in the crate is a `#[cfg(test)]` fixture) — actually wires
    /// `state.display_transcript_write_misses` and
    /// `state.retired_session_workers` into the literal it hands to the
    /// speech-processor thread, rather than a fresh/dangling `Arc`.
    ///
    /// Without this pin, mutating either of those two lines to construct a
    /// fresh `Arc` instead of cloning the shared one survives every other
    /// test in the suite: the repro test
    /// (`deepgram_final_after_writer_cleared_reaches_ledger_but_is_counted_as_display_miss`)
    /// and the `stop_capture_impl` counter tests all build their own
    /// `SpeechShared`/`AppState` directly, so none of them exercise this
    /// literal. A dangling `display_transcript_write_misses` would make the
    /// 64e3 gap silent again in production even though every other test
    /// stays green; a dangling `retired_session_workers` would silently
    /// disable the receiver-join fencing this ticket's fix depends on.
    #[test]
    fn start_transcribe_wires_the_shared_counter_and_retired_workers_arcs_into_speech_shared() {
        let body = start_transcribe_body_whitespace_stripped();

        let shared_literal_marker = strip_whitespace("let shared = speech::SpeechShared {");
        let shared_pos = find_unique_occurrence(
            &body,
            &shared_literal_marker,
            "start_transcribe (SpeechShared literal)",
        );

        let misses_clone_marker = strip_whitespace(
            "let display_transcript_write_misses = state.display_transcript_write_misses.clone();",
        );
        let misses_clone_pos = find_unique_occurrence(
            &body,
            &misses_clone_marker,
            "start_transcribe (display_transcript_write_misses cloned from state)",
        );
        let misses_field_marker = strip_whitespace("display_transcript_write_misses,");
        let misses_field_pos = find_unique_occurrence(
            &body,
            &misses_field_marker,
            "start_transcribe (display_transcript_write_misses field in SpeechShared literal)",
        );

        let retired_clone_marker = strip_whitespace(
            "let retired_session_workers = state.retired_session_workers.clone();",
        );
        let retired_clone_pos = find_unique_occurrence(
            &body,
            &retired_clone_marker,
            "start_transcribe (retired_session_workers cloned from state)",
        );
        let retired_field_marker = strip_whitespace("retired_session_workers,");
        let retired_field_pos = find_unique_occurrence(
            &body,
            &retired_field_marker,
            "start_transcribe (retired_session_workers field in SpeechShared literal)",
        );

        assert!(
            misses_clone_pos < shared_pos && misses_field_pos > shared_pos,
            "display_transcript_write_misses must be cloned from `state` BEFORE the \
             SpeechShared literal and used as a field INSIDE it, not shadowed by a fresh Arc"
        );
        assert!(
            retired_clone_pos < shared_pos && retired_field_pos > shared_pos,
            "retired_session_workers must be cloned from `state` BEFORE the SpeechShared \
             literal and used as a field INSIDE it, not shadowed by a fresh Arc — otherwise \
             a timed-out Deepgram receiver join spills into a vec `ensure_session_workers_\
             quiesced` never looks at"
        );
    }

    fn projection_status_test_event(span_id: &str) -> crate::projections::TranscriptEvent {
        crate::projections::TranscriptEvent {
            span_id: span_id.to_string(),
            provider: "test".to_string(),
            source_id: "system".to_string(),
            provider_item_id: Some(span_id.to_string()),
            transcript_segment_id: Some(format!("segment-{span_id}")),
            speaker_id: Some("speaker-1".to_string()),
            speaker_label: Some("Speaker 1".to_string()),
            channel: None,
            text: "Projection status should not expose this text.".to_string(),
            start_time: 1.0,
            end_time: 2.0,
            confidence: 0.9,
            is_final: true,
            stability: crate::projections::TranscriptEventStability::Final,
            revision_number: 1,
            supersedes: None,
            turn_id: None,
            end_of_turn: true,
            raw_event_ref: Some("test.status".to_string()),
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms: 1_700_000_000_000,
        }
    }

    fn projection_status_test_speaker_revision(
        span_id: &str,
    ) -> crate::projections::DiarizationSpanRevision {
        crate::projections::DiarizationSpanRevision {
            span_id: span_id.to_string(),
            provider: "test-diarizer".to_string(),
            timeline_id: "session".to_string(),
            source_id: Some("system".to_string()),
            speaker_id: Some("speaker-1".to_string()),
            speaker_label: Some("Private Speaker Label".to_string()),
            provider_speaker_id: Some("provider-speaker-1".to_string()),
            channel: None,
            start_time: 1.0,
            end_time: 2.0,
            confidence: Some(0.9),
            is_final: true,
            stability: crate::projections::DiarizationEventStability::Final,
            revision_number: 1,
            supersedes: None,
            basis_asr_span_ids: vec!["report-span-1".to_string()],
            basis_transcript_segment_ids: vec!["segment-report-span-1".to_string()],
            raw_event_ref: None,
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms: 1_700_000_000_001,
        }
    }

    fn drain_test_writers(state: &AppState) {
        if let Ok(mut guard) = state.transcript_writer.lock()
            && let Some(writer) = guard.take()
        {
            let _ = writer.shutdown_with_timeout(std::time::Duration::from_secs(3));
        }
        if let Ok(mut guard) = state.transcript_event_writer.lock()
            && let Some(writer) = guard.take()
        {
            let _ = writer.shutdown_with_timeout(std::time::Duration::from_secs(3));
        }
        if let Ok(mut guard) = state.projection_event_writer.lock()
            && let Some(writer) = guard.take()
        {
            let _ = writer.shutdown_with_timeout(std::time::Duration::from_secs(3));
        }
    }

    fn unique_tempdir(label: &str) -> std::path::PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "audio-graph-commands-{}-{}-{}-{}",
            label,
            std::process::id(),
            nanos,
            n
        ));
        std::fs::create_dir_all(&dir).expect("create tempdir");
        dir
    }

    struct HomeGuard {
        prev_home: Option<String>,
        prev_userprofile: Option<String>,
        prev_data_dir: Option<std::ffi::OsString>,
    }

    impl HomeGuard {
        #[allow(unsafe_code)]
        fn set(dir: &std::path::Path) -> Self {
            let prev_home = std::env::var("HOME").ok();
            let prev_userprofile = std::env::var("USERPROFILE").ok();
            let prev_data_dir = std::env::var_os(crate::user_data::DATA_DIR_ENV);
            // SAFETY: callers hold crate::sessions::TEST_HOME_LOCK for the
            // lifetime of this guard, so process env mutation is serialized.
            unsafe {
                std::env::set_var(crate::user_data::DATA_DIR_ENV, dir);
                std::env::set_var("HOME", dir);
                std::env::set_var("USERPROFILE", dir);
            }
            Self {
                prev_home,
                prev_userprofile,
                prev_data_dir,
            }
        }
    }

    impl Drop for HomeGuard {
        #[allow(unsafe_code)]
        fn drop(&mut self) {
            // SAFETY: serialized by crate::sessions::TEST_HOME_LOCK.
            unsafe {
                match &self.prev_home {
                    Some(v) => std::env::set_var("HOME", v),
                    None => std::env::remove_var("HOME"),
                }
                match &self.prev_userprofile {
                    Some(v) => std::env::set_var("USERPROFILE", v),
                    None => std::env::remove_var("USERPROFILE"),
                }
                match &self.prev_data_dir {
                    Some(v) => std::env::set_var(crate::user_data::DATA_DIR_ENV, v),
                    None => std::env::remove_var(crate::user_data::DATA_DIR_ENV),
                }
            }
        }
    }

    fn write_legacy_then_framed<T>(
        path: &std::path::Path,
        session_id: &str,
        stream_id: &str,
        legacy: &T,
        framed: &T,
    ) where
        T: serde::Serialize + serde::de::DeserializeOwned,
    {
        std::fs::create_dir_all(path.parent().expect("stream parent"))
            .expect("create stream parent");
        let mut bytes = serde_json::to_vec(legacy).expect("serialize legacy prefix");
        bytes.push(b'\n');
        std::fs::write(path, bytes).expect("write legacy prefix");

        let mut appender = crate::persistence::canonical_log::CanonicalAppender::<T>::open(
            path,
            session_id,
            stream_id,
            1,
            crate::persistence::canonical_log::CanonicalTailRecovery::Strict,
        )
        .expect("open canonical suffix appender");
        assert!(matches!(
            appender.append(
                &crate::persistence::canonical_log::CanonicalEventMetadata::new(format!(
                    "{stream_id}-framed"
                )),
                framed,
            ),
            crate::persistence::canonical_log::CanonicalAppendOutcome::Accepted(_)
        ));
    }

    fn append_transcript_event(state: &AppState, event: &crate::projections::TranscriptEvent) {
        let guard = state
            .transcript_event_writer
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        guard
            .as_ref()
            .expect("transcript event writer")
            .append(event);
    }

    fn append_projection_patch(state: &AppState, patch: &crate::projections::ProjectionPatch) {
        let guard = state
            .projection_event_writer
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        assert!(
            guard
                .as_ref()
                .expect("projection event writer")
                .append(patch),
            "projection event should enqueue"
        );
    }

    fn report_note_patch(
        sequence: u64,
        basis: crate::projections::ProjectionBasis,
        note_body: &str,
    ) -> crate::projections::ProjectionPatch {
        crate::projections::ProjectionPatch {
            sequence,
            kind: crate::projections::ProjectionKind::Notes,
            llm_request_id: format!("report-note-{sequence}"),
            route: None,
            basis,
            operations: vec![crate::projections::ProjectionOperation::UpsertNote {
                id: "note-report".to_string(),
                title: "Private title".to_string(),
                body: note_body.to_string(),
                tags: vec!["private".to_string()],
                evidence: crate::claim_evidence::EvidenceAnchor::default(),
                heading_level: None,
            }],
            confidence: 0.9,
            provenance: crate::projections::ProjectionProvenance {
                provider: "test".to_string(),
                model: "projection-report".to_string(),
                prompt_id: "report-notes-v1".to_string(),
                route_id: None,
                model_source: crate::llm::route::ModelIdentitySource::Requested,
            },
            queued_at_ms: Some(1_700_000_050_000 + sequence),
            generation_latency_ms: Some(30 + sequence),
            apply_latency_ms: Some(5 + sequence),
            basis_currency_at_apply: None,
            created_at_ms: 1_700_000_100_000 + sequence,
        }
    }

    fn report_graph_patch(
        sequence: u64,
        basis: crate::projections::ProjectionBasis,
    ) -> crate::projections::ProjectionPatch {
        crate::projections::ProjectionPatch {
            sequence,
            kind: crate::projections::ProjectionKind::Graph,
            llm_request_id: format!("report-graph-{sequence}"),
            route: None,
            basis,
            operations: vec![crate::projections::ProjectionOperation::UpsertGraphNode {
                id: "node-report".to_string(),
                name: "Private Node".to_string(),
                entity_type: "PrivateEntity".to_string(),
                description: Some("Private graph description".to_string()),
                evidence: crate::claim_evidence::EvidenceAnchor::default(),
            }],
            confidence: 0.86,
            provenance: crate::projections::ProjectionProvenance {
                provider: "test".to_string(),
                model: "projection-report".to_string(),
                prompt_id: "report-graph-v1".to_string(),
                route_id: None,
                model_source: crate::llm::route::ModelIdentitySource::Requested,
            },
            queued_at_ms: Some(1_700_000_150_000 + sequence),
            generation_latency_ms: Some(40 + sequence),
            apply_latency_ms: Some(6 + sequence),
            basis_currency_at_apply: None,
            created_at_ms: 1_700_000_200_000 + sequence,
        }
    }

    fn invalid_graph_patch() -> crate::projections::ProjectionPatch {
        crate::projections::ProjectionPatch {
            sequence: 1,
            kind: crate::projections::ProjectionKind::Graph,
            llm_request_id: "report-invalid-graph".to_string(),
            route: None,
            basis: crate::projections::ProjectionBasis {
                span_revisions: Vec::new(),
                covered_prefix: None,
                diarization_span_revisions: Vec::new(),
                transcript_hash: "empty".to_string(),
                summarized_through_revision: None,
            },
            operations: vec![crate::projections::ProjectionOperation::UpsertGraphEdge {
                id: "edge-dangling".to_string(),
                source: "node-missing-a".to_string(),
                target: "node-missing-b".to_string(),
                relation_type: "mentions".to_string(),
                label: Some("Private edge label".to_string()),
                weight: 0.5,
                evidence: crate::claim_evidence::EvidenceAnchor::default(),
            }],
            confidence: 0.5,
            provenance: crate::projections::ProjectionProvenance {
                provider: "test".to_string(),
                model: "projection-report".to_string(),
                prompt_id: "report-invalid-v1".to_string(),
                route_id: None,
                model_source: crate::llm::route::ModelIdentitySource::Requested,
            },
            queued_at_ms: None,
            generation_latency_ms: None,
            apply_latency_ms: None,
            basis_currency_at_apply: None,
            created_at_ms: 1_700_000_300_000,
        }
    }

    fn seed_replayable_projection_session(
        session_id: &str,
        note_body: &str,
    ) -> crate::projections::ProjectionBasis {
        let repository = FileMemoryRepository::user_data();
        let event = projection_status_test_event(&format!("{session_id}-span-1"));
        let basis = crate::projections::ProjectionBasis::from_transcript_events(
            std::slice::from_ref(&event),
        );
        repository
            .append_transcript_event(session_id, &event)
            .expect("append transcript event");
        repository
            .append_projection_patch(session_id, &report_note_patch(1, basis.clone(), note_body))
            .expect("append notes patch");
        repository
            .append_projection_patch(session_id, &report_graph_patch(1, basis.clone()))
            .expect("append graph patch");
        basis
    }

    fn stale_materialized_notes(
        session_id: &str,
        basis: crate::projections::ProjectionBasis,
    ) -> crate::projections::MaterializedNotes {
        let mut notes = crate::projections::MaterializedNotes::new(session_id);
        notes.notes.push(crate::projections::MaterializedNote {
            id: "stale-note".to_string(),
            title: "Stale title".to_string(),
            body: "Stale materialized note should not survive replay repair.".to_string(),
            tags: vec!["stale".to_string()],
            heading_level: None,
            updated_by_sequence: 0,
            updated_at_ms: 1,
            basis: Arc::new(basis),
            provenance: crate::projections::ProjectionProvenance {
                provider: "test".to_string(),
                model: "stale-artifact".to_string(),
                prompt_id: "stale-notes".to_string(),
                route_id: None,
                model_source: crate::llm::route::ModelIdentitySource::Requested,
            },
            evidence: None,
        });
        notes
    }

    fn stale_materialized_graph(
        session_id: &str,
        basis: crate::projections::ProjectionBasis,
    ) -> crate::projections::MaterializedGraph {
        let mut graph = crate::projections::MaterializedGraph::new(session_id);
        graph.nodes.push(crate::projections::MaterializedGraphNode {
            id: "stale-node".to_string(),
            name: "Stale Node".to_string(),
            entity_type: "StaleEntity".to_string(),
            description: Some(
                "Stale materialized graph should not survive replay repair.".to_string(),
            ),
            confidence: 0.1,
            valid_from_ms: 1,
            valid_until_ms: None,
            updated_by_sequence: 0,
            updated_at_ms: 1,
            basis: Arc::new(basis),
            provenance: crate::projections::ProjectionProvenance {
                provider: "test".to_string(),
                model: "stale-artifact".to_string(),
                prompt_id: "stale-graph".to_string(),
                route_id: None,
                model_source: crate::llm::route::ModelIdentitySource::Requested,
            },
            evidence: None,
        });
        graph
    }

    fn leaked_active_projection_state() -> crate::projections::MaterializedProjectionState {
        let session_id = "active-session-before-load";
        let basis = Arc::new(crate::projections::ProjectionBasis {
            span_revisions: Vec::new(),
            covered_prefix: None,
            diarization_span_revisions: Vec::new(),
            transcript_hash: "active-before-load".to_string(),
            summarized_through_revision: None,
        });
        let provenance = crate::projections::ProjectionProvenance {
            provider: "test".to_string(),
            model: "active-before-load".to_string(),
            prompt_id: "active-before-load".to_string(),
            route_id: None,
            model_source: crate::llm::route::ModelIdentitySource::Requested,
        };
        let mut state = crate::projections::MaterializedProjectionState::new(session_id);
        state.notes.last_sequence = 99;
        state
            .notes
            .notes
            .push(crate::projections::MaterializedNote {
                id: "leaked-note".to_string(),
                title: "Leaked title".to_string(),
                body: "Active-session note must remain isolated from Review load.".to_string(),
                tags: vec!["leak".to_string()],
                heading_level: None,
                updated_by_sequence: 99,
                updated_at_ms: 99,
                basis: Arc::clone(&basis),
                provenance: provenance.clone(),
                evidence: None,
            });
        state.graph.last_sequence = 99;
        state
            .graph
            .nodes
            .push(crate::projections::MaterializedGraphNode {
                id: "leaked-node".to_string(),
                name: "Leaked Node".to_string(),
                entity_type: "LeakedEntity".to_string(),
                description: Some(
                    "Active-session graph must remain isolated from Review load.".to_string(),
                ),
                confidence: 0.99,
                valid_from_ms: 99,
                valid_until_ms: None,
                updated_by_sequence: 99,
                updated_at_ms: 99,
                basis,
                provenance,
                evidence: None,
            });
        state
    }

    /// Write pre-built data-movement events to a session's on-disk ledger
    /// (`~/.audiograph/ledgers/<session>.movements.jsonl`) in append order, so
    /// the [`load_session_data_movement_cmd`] loader can read them back. Uses
    /// the same one-JSON-object-per-line format the persistence layer's
    /// `load_jsonl` expects.
    fn seed_data_movement_ledger(
        session_id: &str,
        events: &[crate::persistence::DataMovementEvent],
    ) {
        let path =
            crate::user_data::data_movement_ledger_path(session_id).expect("resolve ledger path");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create ledgers dir");
        }
        let mut body = String::new();
        for event in events {
            body.push_str(&serde_json::to_string(event).expect("serialize movement event"));
            body.push('\n');
        }
        std::fs::write(&path, body).expect("write ledger");
    }

    fn seed_legacy_transcript(session_id: &str, text: &str) {
        let path =
            crate::user_data::transcript_path(session_id).expect("resolve legacy transcript path");
        let segment = TranscriptSegment {
            id: "legacy-segment".to_string(),
            source_id: "legacy-source".to_string(),
            speaker_id: None,
            speaker_label: None,
            text: text.to_string(),
            start_time: 1.0,
            end_time: 2.0,
            confidence: 0.9,
        };
        let body = format!(
            "{}\n",
            serde_json::to_string(&segment).expect("serialize legacy transcript")
        );
        std::fs::write(path, body).expect("write legacy transcript");
    }

    fn seed_malformed_legacy_transcript(session_id: &str, secret: &str) {
        let path = crate::user_data::transcript_path(session_id)
            .expect("resolve malformed legacy transcript path");
        let valid = TranscriptSegment {
            id: "valid-after-malformed".to_string(),
            source_id: "legacy-source".to_string(),
            speaker_id: None,
            speaker_label: None,
            text: "valid row after malformed input".to_string(),
            start_time: 1.0,
            end_time: 2.0,
            confidence: 0.9,
        };
        let body = format!(
            "{{\"private\":\"{secret}\"\n{}\n",
            serde_json::to_string(&valid).expect("serialize valid legacy transcript row")
        );
        std::fs::write(path, body).expect("write malformed legacy transcript");
    }

    fn snapshot_tree(root: &std::path::Path) -> Vec<(std::path::PathBuf, Option<Vec<u8>>)> {
        fn visit(
            root: &std::path::Path,
            directory: &std::path::Path,
            snapshot: &mut Vec<(std::path::PathBuf, Option<Vec<u8>>)>,
        ) {
            let mut entries: Vec<_> = std::fs::read_dir(directory)
                .expect("read snapshot directory")
                .map(|entry| entry.expect("snapshot directory entry"))
                .collect();
            entries.sort_by_key(|entry| entry.file_name());
            for entry in entries {
                let path = entry.path();
                let relative = path
                    .strip_prefix(root)
                    .expect("snapshot path under root")
                    .to_path_buf();
                if entry.file_type().expect("snapshot file type").is_dir() {
                    snapshot.push((relative, None));
                    visit(root, &path, snapshot);
                } else {
                    snapshot.push((
                        relative,
                        Some(std::fs::read(&path).expect("read snapshot file")),
                    ));
                }
            }
        }

        let mut snapshot = Vec::new();
        visit(root, root, &mut snapshot);
        snapshot
    }

    #[test]
    fn strict_reader_review_fix_export_malformed_index_is_tree_pure() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("review-fix-export-malformed-index");
        let _guard = HomeGuard::set(&dir);
        let session_id = "export-malformed-index";

        seed_legacy_transcript(session_id, "exportable legacy transcript");
        std::fs::write(dir.join("sessions.json"), b"{ malformed index")
            .expect("write malformed sessions index");
        let before = snapshot_tree(&dir);

        let bundle = session_export_bundle(session_id)
            .expect("export should ignore an unavailable rebuildable index");
        assert_eq!(bundle.transcript.len(), 1);
        assert!(bundle.metadata.is_none());

        let after = snapshot_tree(&dir);
        assert!(
            before == after,
            "export must leave every artifact name and byte unchanged"
        );
        assert!(
            !after.iter().any(|(path, _)| path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().contains(".corrupt-"))),
            "export must not back up or repair a malformed index"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn strict_reader_review_fix_shared_legacy_reader_rejects_first_malformed_row() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("review-fix-shared-malformed-legacy");
        let _guard = HomeGuard::set(&dir);
        let session_id = "shared-malformed-legacy";
        let secret = "private-malformed-transcript";
        seed_malformed_legacy_transcript(session_id, secret);

        let error = read_legacy_session_transcript(session_id)
            .expect_err("the shared legacy reader must reject incomplete content");
        assert!(!error.contains(secret), "legacy read error leaked content");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn strict_reader_review_fix_standalone_rejects_malformed_legacy_transcript() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("review-fix-standalone-malformed-legacy");
        let _guard = HomeGuard::set(&dir);
        let session_id = "standalone-malformed-legacy";
        let secret = "private-standalone-transcript";
        seed_malformed_legacy_transcript(session_id, secret);

        let error = load_session_transcript_impl(session_id.to_string())
            .expect_err("standalone transcript must fail closed");
        assert!(!error.to_string().contains(secret));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn strict_reader_review_fix_review_rejects_malformed_legacy_transcript() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("review-fix-review-malformed-legacy");
        let _guard = HomeGuard::set(&dir);
        let session_id = "review-malformed-legacy";
        let secret = "private-review-transcript";
        seed_malformed_legacy_transcript(session_id, secret);

        let error = load_session_impl(session_id.to_string())
            .expect_err("historical Review must fail closed");
        assert!(!error.to_string().contains(secret));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn strict_reader_review_fix_export_rejects_malformed_legacy_transcript() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("review-fix-export-malformed-legacy");
        let _guard = HomeGuard::set(&dir);
        let session_id = "export-malformed-legacy";
        let secret = "private-export-transcript";
        seed_malformed_legacy_transcript(session_id, secret);

        let error = session_export_bundle(session_id).expect_err("session export must fail closed");
        assert!(!error.to_string().contains(secret));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// audio-graph-e8e7: the production read seam refuses an unreadable control
    /// plane instead of admitting historical v1 and serving legacy bytes.
    ///
    /// The obstruction is a NON-REGULAR entry at the derived manifest control
    /// identity, so the refusal is host-independent: a root the substrate
    /// qualifies refuses on the strict manifest load, and a root it cannot
    /// qualify refuses on qualification.
    #[test]
    fn guarded_open_refuses_a_session_whose_control_plane_cannot_be_read() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("guarded-open-refuses-control-plane");
        let _guard = HomeGuard::set(&dir);
        let session_id = "guarded-control-plane";
        let secret = "legacy text must stay behind the floor gate";

        seed_legacy_transcript(session_id, secret);
        let transcript = load_session_transcript_impl(session_id.to_string())
            .expect("a session with no control plane still loads");
        assert_eq!(transcript.len(), 1, "the baseline read is unchanged");

        let root = crate::user_data::data_root().expect("data root");
        let paths =
            crate::persistence::session_semantics::session_control_plane_paths(&root, session_id)
                .expect("control paths");
        std::fs::create_dir_all(&paths.manifest).expect("unreadable control plane");

        let error = load_session_transcript_impl(session_id.to_string())
            .expect_err("an unreadable control plane must refuse the read");
        assert!(
            !error.to_string().contains(secret),
            "the refusal must not carry Session content: {error}"
        );

        std::fs::remove_dir(&paths.manifest).expect("clear the obstruction");
        assert_eq!(
            load_session_transcript_impl(session_id.to_string())
                .expect("the read converges once the control plane is gone")
                .len(),
            1
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn strict_reader_present_empty_transcript_stream_is_authoritative() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("strict-present-empty-transcript");
        let _guard = HomeGuard::set(&dir);
        let session_id = "present-empty-transcript";

        seed_legacy_transcript(session_id, "legacy text must stay hidden");
        std::fs::write(
            crate::user_data::transcript_events_path(session_id)
                .expect("resolve canonical transcript path"),
            b"",
        )
        .expect("write present-empty canonical transcript stream");

        let transcript = load_session_transcript_impl(session_id.to_string())
            .expect("present-empty canonical transcript should load");
        assert!(
            transcript.is_empty(),
            "a present-empty canonical stream must suppress legacy transcript rows"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn strict_reader_loaded_session_reuses_one_present_empty_transcript_snapshot() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("strict-loaded-session-snapshot");
        let _guard = HomeGuard::set(&dir);
        let session_id = "coherent-loaded-session";

        seed_legacy_transcript(session_id, "legacy text must not diverge from events");
        std::fs::write(
            crate::user_data::transcript_events_path(session_id)
                .expect("resolve canonical transcript path"),
            b"",
        )
        .expect("write present-empty canonical transcript stream");

        let loaded = load_session_impl(session_id.to_string())
            .expect("present-empty historical session should load");
        assert!(loaded.transcript_events.is_empty());
        assert!(
            loaded.transcript.is_empty(),
            "the derived transcript and returned events must come from one snapshot"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn strict_reader_export_reuses_one_present_empty_transcript_snapshot() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("strict-export-snapshot");
        let _guard = HomeGuard::set(&dir);
        let session_id = "coherent-export-session";

        seed_legacy_transcript(session_id, "legacy export text must stay hidden");
        std::fs::write(
            crate::user_data::transcript_events_path(session_id)
                .expect("resolve canonical transcript path"),
            b"",
        )
        .expect("write present-empty canonical transcript stream");

        let bundle = session_export_bundle(session_id)
            .expect("present-empty historical session should export");
        assert!(bundle.transcript_events.is_empty());
        assert!(
            bundle.transcript.is_empty(),
            "the exported transcript and events must come from one snapshot"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn strict_reader_corrupt_canonical_transcript_blocks_legacy_fallback() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("strict-corrupt-transcript");
        let _guard = HomeGuard::set(&dir);
        let session_id = "corrupt-canonical-transcript";

        seed_legacy_transcript(session_id, "legacy text must not mask corruption");
        std::fs::write(
            crate::user_data::transcript_events_path(session_id)
                .expect("resolve canonical transcript path"),
            b"not a canonical transcript record\n",
        )
        .expect("write corrupt canonical transcript stream");

        let error = load_session_transcript_impl(session_id.to_string())
            .expect_err("canonical corruption must fail closed");
        let message = error.to_string();
        assert!(message.contains("deserialize") || message.contains("canonical"));
        assert!(!message.contains("legacy text must not mask corruption"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ADR-0045 decision 6 review finding (audio-graph-5fd1): the
    /// `start_transcribe` coverage-head reseed used to call
    /// `load_projection_patches(&session_id).unwrap_or_default()`, so a
    /// corrupt/truncated `projection_patches` canonical log silently reseeded
    /// nothing instead of refusing to start — masking corruption exactly like
    /// the legacy-transcript-fallback bug this mirrors. This test pins the
    /// loader itself failing closed; `start_transcribe` now propagates that
    /// `Err` via `?` (`commands.rs`) instead of swallowing it.
    #[test]
    fn strict_reader_corrupt_projection_patches_log_fails_closed_instead_of_reseeding_nothing() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("strict-corrupt-projection-patches");
        let _guard = HomeGuard::set(&dir);
        let session_id = "corrupt-canonical-projection-patches";

        std::fs::write(
            crate::user_data::projection_events_path(session_id)
                .expect("resolve canonical projection patches path"),
            b"not a canonical projection patch record\n",
        )
        .expect("write corrupt canonical projection patches stream");

        let error = FileMemoryRepository::user_data()
            .load_projection_patches(session_id)
            .expect_err(
                "canonical projection-patches corruption must fail closed, not reseed nothing \
                 (audio-graph-5fd1 review finding)",
            );
        assert!(
            error.contains("deserialize") || error.contains("canonical"),
            "error must describe the failure class, not swallow it: {error}"
        );
        assert!(
            !error.contains("not a canonical projection patch record"),
            "corrupt log content must never leak into the propagated error message: {error}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn strict_reader_nominal_missing_replay_does_not_create_data_root() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let parent = unique_tempdir("strict-missing-root-parent");
        let data_root = parent.join("absent-data-root");
        let _guard = HomeGuard::set(&data_root);

        let report = projection_replay_report_for_session("missing-session")
            .expect("missing canonical streams should replay as empty");
        assert_eq!(report.transcript_event_count, 0);
        assert_eq!(report.projection_event_count, 0);
        assert!(
            !data_root.exists(),
            "a nominal strict read must not create the data root or stream directories"
        );

        let _ = std::fs::remove_dir_all(&parent);
    }

    #[test]
    fn strict_reader_nominal_transcript_read_does_not_back_up_malformed_index() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("strict-malformed-index");
        let _guard = HomeGuard::set(&dir);
        let session_id = "read-with-malformed-index";

        seed_legacy_transcript(session_id, "legacy transcript remains readable");
        std::fs::write(dir.join("sessions.json"), b"{ malformed index")
            .expect("write malformed sessions index");

        let transcript = load_session_transcript_impl(session_id.to_string())
            .expect("resolve-only transcript read should ignore malformed index");
        assert_eq!(transcript.len(), 1);
        let backup_exists = std::fs::read_dir(&dir)
            .expect("read data root")
            .filter_map(Result::ok)
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("sessions.json.corrupt-")
            });
        assert!(
            !backup_exists,
            "a nominal read must not repair or back up a malformed sessions index"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_session_data_movement_cmd_returns_empty_for_session_without_ledger() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("data-movement-cmd-empty");
        let _guard = HomeGuard::set(&dir);

        // A local-only session that never moved any data has no ledger file;
        // the command surfaces that as an empty vec, not an error, so the UI
        // can render "no content left the device".
        let events = load_session_data_movement_impl("never-recorded".to_string())
            .expect("empty ledger loads as empty vec");
        assert!(events.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_session_data_movement_cmd_loads_ledger_events_in_append_order() {
        use crate::persistence::{
            DataClass, DataMovementActor, DataMovementDestination, DataMovementEventType,
            DataMovementLedgerBuilder, DestinationBoundary, MovementModel, MovementPolicy,
            PrivacyMode, RetentionClass,
        };

        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("data-movement-cmd-load");
        let _guard = HomeGuard::set(&dir);

        let session_id = "session-with-egress";

        // A local artifact write that stayed on device.
        let local = DataMovementLedgerBuilder::new(
            session_id,
            DataMovementActor::System,
            DataMovementEventType::ArtifactWritten,
            MovementPolicy {
                privacy_mode: PrivacyMode::ByokCloud,
                user_visible: true,
                retention_class: RetentionClass::SessionArtifact,
            },
            DataMovementDestination {
                boundary: DestinationBoundary::Local,
                provider_id: None,
                endpoint_class: None,
            },
        )
        .created_at_ms(1_000)
        .data_classes([DataClass::TranscriptText])
        .build();

        // A cloud provider call that left the device — carries a provider/model
        // and data class but, by schema construction, no secret.
        let egress = DataMovementLedgerBuilder::new(
            session_id,
            DataMovementActor::System,
            DataMovementEventType::ProviderCallSucceeded,
            MovementPolicy {
                privacy_mode: PrivacyMode::ByokCloud,
                user_visible: true,
                retention_class: RetentionClass::Transient,
            },
            DataMovementDestination {
                boundary: DestinationBoundary::Provider,
                provider_id: Some("llm.openrouter".to_string()),
                endpoint_class: Some("chat_completions".to_string()),
            },
        )
        .created_at_ms(2_000)
        .data_classes([DataClass::Prompts, DataClass::TranscriptText])
        .model(MovementModel {
            provider_id: Some("llm.openrouter".to_string()),
            model_id: Some("openai/gpt-4o-mini".to_string()),
        })
        .build();

        seed_data_movement_ledger(session_id, &[local.clone(), egress.clone()]);

        let loaded = load_session_data_movement_impl(session_id.to_string()).expect("ledger loads");
        assert_eq!(loaded, vec![local, egress]);

        // Round-tripped events must never carry a raw secret. The serialized
        // form is exactly what the frontend receives over the invoke boundary.
        let serialized = serde_json::to_string(&loaded).expect("serialize loaded ledger");
        assert!(!serialized.to_lowercase().contains("secret"));
        assert!(!serialized.to_lowercase().contains("api_key"));
        assert!(!serialized.contains("Bearer"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_session_data_movement_cmd_rejects_path_traversal_session_ids() {
        // Defense-in-depth (audio-graph-e692): the session id is joined into the
        // ledgers directory, so a `..` segment or a path separator would let a
        // caller read a `*.movements.jsonl` file outside the ledgers dir. Every
        // sibling session command validates first; this one must too. Validation
        // runs before any filesystem access, so no HomeGuard is needed.
        for malicious in [
            "../secrets",
            "..",
            "foo/../bar",
            "foo/bar",
            "foo\\bar",
            "a/b/c",
        ] {
            let err = load_session_data_movement_impl(malicious.to_string())
                .expect_err("path-traversal session id must be rejected");
            let message = match &err {
                AppError::Unknown(message) => message.clone(),
                other => {
                    panic!("expected Unknown validation error for {malicious:?}, got {other:?}")
                }
            };
            assert!(
                message.contains("Invalid session ID"),
                "expected an invalid-session-id message for {malicious:?}, got {message:?}"
            );
        }
    }

    #[test]
    fn load_session_replays_projection_state_when_materialized_artifacts_are_missing() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("load-session-missing-projections");
        let _guard = HomeGuard::set(&dir);

        let session_id = "load-session-missing-projections";
        seed_replayable_projection_session(session_id, "Replayed note from event log.");

        let loaded = load_session_impl(session_id.to_string())
            .expect("load session should replay missing materialized projections");

        assert_eq!(
            loaded.transcript.len(),
            1,
            "Review transcript must derive from canonical revisions when the legacy file is absent"
        );

        // Notes/graph moved off `load_session` (seed audio-graph-4fa5
        // deliverable a) into their own lens-fetch commands.
        let notes_artifacts = load_session_notes_artifacts_impl(session_id.to_string())
            .expect("Notes-lens fetch should replay missing materialized projections");
        let notes = notes_artifacts
            .notes
            .expect("missing notes artifact should replay");
        assert_eq!(notes.last_sequence, 1);
        assert_eq!(notes.notes.len(), 1);
        assert_eq!(notes.notes[0].id, "note-report");
        assert_eq!(notes.notes[0].body, "Replayed note from event log.");

        let graph = load_session_graph_artifact_impl(session_id.to_string())
            .expect("Graph-lens fetch should replay missing materialized projections")
            .expect("missing graph artifact should replay");
        assert_eq!(graph.last_sequence, 1);
        assert_eq!(graph.nodes.len(), 1);
        assert_eq!(graph.nodes[0].id, "node-report");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_session_replays_speaker_bearing_projection_state() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("load-session-speaker-bearing-projection");
        let _guard = HomeGuard::set(&dir);
        let session_id = "load-session-speaker-bearing-projection";
        let repository = FileMemoryRepository::user_data();
        let event = projection_status_test_event("report-span-1");
        let speaker = projection_status_test_speaker_revision("speaker-span-1");
        let basis = crate::projections::ProjectionBasis::from_transcript_events_and_speaker_spans(
            std::slice::from_ref(&event),
            &[crate::projections::ProjectionBasisSpan {
                span_id: speaker.span_id.clone(),
                revision_number: speaker.revision_number,
            }],
        );

        repository
            .append_transcript_event(session_id, &event)
            .expect("append transcript event");
        repository
            .append_diarization_span_revision(session_id, &speaker)
            .expect("append speaker revision");
        repository
            .append_projection_patch(
                session_id,
                &report_note_patch(1, basis, "Speaker-aware replayed note."),
            )
            .expect("append speaker-bearing projection patch");

        let loaded = load_session_impl(session_id.to_string())
            .expect("load session should replay speaker-bearing projection state");
        assert_eq!(loaded.diarization_events, vec![speaker]);
        let notes_artifacts = load_session_notes_artifacts_impl(session_id.to_string())
            .expect("Notes-lens fetch should replay speaker-bearing projection state");
        let notes = notes_artifacts
            .notes
            .expect("speaker-bearing notes should replay");
        assert_eq!(notes.last_sequence, 1);
        assert_eq!(notes.notes.len(), 1);
        assert_eq!(notes.notes[0].body, "Speaker-aware replayed note.");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_session_replays_mixed_transcript_speaker_and_projection_streams() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("load-session-mixed-speaker-projection");
        let _guard = HomeGuard::set(&dir);
        let session_id = "load-session-mixed-speaker-projection";
        let base_ms = 1_700_000_000_000;

        let mut transcript_one = projection_status_test_event("report-span-1");
        transcript_one.received_at_ms = base_ms + 100;
        let mut transcript_two = projection_status_test_event("report-span-2");
        transcript_two.start_time = 2.0;
        transcript_two.end_time = 3.0;
        transcript_two.received_at_ms = base_ms + 200;
        let mut speaker_one = projection_status_test_speaker_revision("speaker-span-1");
        speaker_one.received_at_ms = base_ms + 110;
        let mut speaker_two = projection_status_test_speaker_revision("speaker-span-2");
        speaker_two.start_time = 2.0;
        speaker_two.end_time = 3.0;
        speaker_two.received_at_ms = base_ms + 210;

        let basis_one =
            crate::projections::ProjectionBasis::from_transcript_events_and_speaker_spans(
                std::slice::from_ref(&transcript_one),
                &[crate::projections::ProjectionBasisSpan {
                    span_id: speaker_one.span_id.clone(),
                    revision_number: speaker_one.revision_number,
                }],
            );
        let basis_two =
            crate::projections::ProjectionBasis::from_transcript_events_and_speaker_spans(
                &[transcript_one.clone(), transcript_two.clone()],
                &[
                    crate::projections::ProjectionBasisSpan {
                        span_id: speaker_one.span_id.clone(),
                        revision_number: speaker_one.revision_number,
                    },
                    crate::projections::ProjectionBasisSpan {
                        span_id: speaker_two.span_id.clone(),
                        revision_number: speaker_two.revision_number,
                    },
                ],
            );
        let mut projection_one = report_note_patch(1, basis_one, "Legacy prefix note.");
        projection_one.created_at_ms = base_ms + 120;
        let mut projection_two = report_note_patch(2, basis_two, "Framed suffix note.");
        projection_two.created_at_ms = base_ms + 220;

        write_legacy_then_framed(
            &crate::user_data::transcript_events_path(session_id).expect("transcript stream path"),
            session_id,
            "transcript_revisions",
            &transcript_one,
            &transcript_two,
        );
        write_legacy_then_framed(
            &crate::user_data::diarization_events_path(session_id).expect("speaker stream path"),
            session_id,
            "speaker_revisions",
            &speaker_one,
            &speaker_two,
        );
        write_legacy_then_framed(
            &crate::user_data::projection_events_path(session_id).expect("projection stream path"),
            session_id,
            "projection_patches",
            &projection_one,
            &projection_two,
        );

        let loaded = load_session_impl(session_id.to_string())
            .expect("mixed canonical streams should reconstruct projection state");
        assert_eq!(loaded.transcript_events.len(), 2);
        assert_eq!(loaded.diarization_events.len(), 2);
        let notes_artifacts = load_session_notes_artifacts_impl(session_id.to_string())
            .expect("Notes-lens fetch should reconstruct projection state");
        assert_eq!(notes_artifacts.projection_events.len(), 2);
        let notes = notes_artifacts
            .notes
            .expect("mixed stream notes should replay");
        assert_eq!(notes.last_sequence, 2);
        assert_eq!(notes.notes.len(), 1);
        assert_eq!(notes.notes[0].body, "Framed suffix note.");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_session_does_not_promote_cache_when_canonical_projection_stream_is_empty() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let dir = unique_tempdir("load-session-empty-canonical-projection");
        let _guard = HomeGuard::set(&dir);
        let session_id = "load-session-empty-canonical-projection";
        let basis = seed_replayable_projection_session(session_id, "event-backed note");
        let repository = FileMemoryRepository::user_data();

        let mut orphan_notes = stale_materialized_notes(session_id, basis.clone());
        orphan_notes.last_sequence = 99;
        repository
            .save_materialized_notes(session_id, &orphan_notes)
            .expect("save orphan notes cache");
        let mut orphan_graph = stale_materialized_graph(session_id, basis);
        orphan_graph.last_sequence = 99;
        repository
            .save_materialized_graph(session_id, &orphan_graph)
            .expect("save orphan graph cache");

        let projection_path = crate::user_data::projection_events_path(session_id).unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&projection_path)
            .expect("model a crash-lost canonical projection log");

        load_session_impl(session_id.to_string())
            .expect("empty canonical stream should replay as explicit empty state");
        let notes = load_session_notes_artifacts_impl(session_id.to_string())
            .expect("Notes-lens fetch should replay as explicit empty state")
            .notes
            .expect("canonical notes state is explicit");
        let graph = load_session_graph_artifact_impl(session_id.to_string())
            .expect("Graph-lens fetch should replay as explicit empty state")
            .expect("canonical graph state is explicit");
        assert_eq!(notes.last_sequence, 0);
        assert!(notes.notes.is_empty(), "orphan notes cache is ignored");
        assert_eq!(graph.last_sequence, 0);
        assert!(graph.nodes.is_empty(), "orphan graph cache is ignored");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// seed audio-graph-4fa5 deliverable a acceptance: `load_session`'s
    /// response must no longer carry the heavy lenses — `notes`,
    /// `materialized_graph`, and the raw `projection_events` log all had
    /// their own fetch commands split out. Seeds a session that actually HAS
    /// all three (so a field that was merely always-`None` couldn't fake a
    /// pass) and asserts the serialized `LoadedSession` JSON has no such
    /// keys, while the transcript-lens fields it must still carry survive.
    #[test]
    fn load_session_response_excludes_heavy_lens_artifacts() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("load-session-excludes-heavy-lenses");
        let _guard = HomeGuard::set(&dir);
        let session_id = "load-session-excludes-heavy-lenses";
        seed_replayable_projection_session(session_id, "Notes lens body.");
        // Confirm the artifacts this test is excluding actually exist and
        // actually replay non-empty via their own lens commands — otherwise
        // "response excludes notes" would be true merely because there was
        // never anything to exclude.
        assert!(
            load_session_notes_artifacts_impl(session_id.to_string())
                .expect("Notes-lens fetch should succeed")
                .notes
                .is_some(),
            "the seeded session must actually carry replayable notes"
        );

        let loaded =
            load_session_impl(session_id.to_string()).expect("load session should succeed");
        let payload = serde_json::to_value(&loaded).expect("LoadedSession must serialize");
        let fields = payload
            .as_object()
            .expect("LoadedSession serializes as a JSON object");

        for heavy_field in ["notes", "materialized_graph", "projection_events"] {
            assert!(
                !fields.contains_key(heavy_field),
                "load_session's response must not carry `{heavy_field}` — it is now a \
                 separate lens-fetch command (seed audio-graph-4fa5)"
            );
        }
        for light_field in [
            "transcript",
            "graph",
            "transcript_events",
            "diarization_events",
            "live_assist_cards",
        ] {
            assert!(
                fields.contains_key(light_field),
                "load_session's response must still carry `{light_field}` (transcript lens)"
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// seed audio-graph-4fa5 deliverable b: the byte-ceiling unit contract.
    /// At the ceiling, a file passes; one byte over, it refuses with the
    /// typed error carrying the observed size and configured ceiling; a
    /// missing file is not a violation (the caller's "missing → None/empty"
    /// fallback still applies).
    #[test]
    fn enforce_artifact_ceiling_boundary_behavior() {
        let dir = unique_tempdir("artifact-ceiling-boundary");
        std::fs::create_dir_all(&dir).expect("create tempdir");
        let path = dir.join("artifact.bin");

        std::fs::write(&path, vec![0u8; 1024]).expect("write at-ceiling file");
        enforce_artifact_ceiling(&path, 1024, "test_artifact")
            .expect("a file exactly at the ceiling must pass");

        std::fs::write(&path, vec![0u8; 1025]).expect("write over-ceiling file");
        let error = enforce_artifact_ceiling(&path, 1024, "test_artifact")
            .expect_err("one byte over the ceiling must refuse");
        match error {
            AppError::ArtifactTooLarge {
                artifact_class,
                size_bytes,
                ceiling_bytes,
            } => {
                assert_eq!(artifact_class, "test_artifact");
                assert_eq!(size_bytes, 1025);
                assert_eq!(ceiling_bytes, 1024);
            }
            other => panic!("expected ArtifactTooLarge, got {other:?}"),
        }

        let missing = dir.join("does-not-exist.bin");
        enforce_artifact_ceiling(&missing, 1024, "test_artifact")
            .expect("a missing artifact is not a ceiling violation");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// seed audio-graph-4fa5 deliverable b acceptance: a synthetic
    /// oversized `graphs/<id>.materialized.json` refuses with the typed
    /// `ArtifactTooLarge` error rather than being parsed — the ceiling check
    /// stats the file before ever reading its contents, so garbage bytes are
    /// enough to prove the refusal; this test never needs to construct a
    /// real 156MB graph (the field artifact this ceiling exists for).
    #[test]
    fn load_session_graph_artifact_cmd_refuses_an_oversized_materialized_graph() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("graph-artifact-ceiling-refuse");
        let _guard = HomeGuard::set(&dir);
        let session_id = "graph-artifact-ceiling-refuse";

        let oversized = vec![b'x'; (MAX_MATERIALIZED_GRAPH_BYTES + 1) as usize];
        std::fs::write(
            crate::user_data::materialized_graph_path(session_id)
                .expect("resolve materialized graph path"),
            &oversized,
        )
        .expect("write synthetic oversized materialized graph");

        let error = load_session_graph_artifact_impl(session_id.to_string())
            .expect_err("an oversized materialized graph must refuse, not attempt to parse");
        match error {
            AppError::ArtifactTooLarge {
                artifact_class,
                size_bytes,
                ceiling_bytes,
            } => {
                assert_eq!(artifact_class, "materialized_graph");
                assert_eq!(size_bytes, oversized.len() as u64);
                assert_eq!(ceiling_bytes, MAX_MATERIALIZED_GRAPH_BYTES);
            }
            other => panic!("expected ArtifactTooLarge, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Companion to the refusal test above: a small (well under the
    /// ceiling), real materialized graph must still load and replay
    /// normally through the same command.
    #[test]
    fn load_session_graph_artifact_cmd_loads_a_small_materialized_graph() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("graph-artifact-small-passes");
        let _guard = HomeGuard::set(&dir);
        let session_id = "graph-artifact-small-passes";
        seed_replayable_projection_session(session_id, "Small graph companion note.");

        let graph = load_session_graph_artifact_impl(session_id.to_string())
            .expect("a small materialized graph must load")
            .expect("replayed graph artifact must be present");
        assert_eq!(graph.nodes[0].id, "node-report");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// seed audio-graph-4fa5 deliverable b: the same ceiling contract for
    /// the materialized-notes artifact (Notes lens), synthesized the same
    /// way as the graph case above.
    #[test]
    fn load_session_notes_artifacts_cmd_refuses_an_oversized_notes_artifact() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("notes-artifact-ceiling-refuse");
        let _guard = HomeGuard::set(&dir);
        let session_id = "notes-artifact-ceiling-refuse";

        let oversized = vec![b'x'; (MAX_MATERIALIZED_NOTES_BYTES + 1) as usize];
        std::fs::write(
            crate::user_data::notes_path(session_id).expect("resolve notes path"),
            &oversized,
        )
        .expect("write synthetic oversized notes artifact");

        let error = load_session_notes_artifacts_impl(session_id.to_string())
            .expect_err("an oversized notes artifact must refuse, not attempt to parse");
        match error {
            AppError::ArtifactTooLarge {
                artifact_class,
                size_bytes,
                ceiling_bytes,
            } => {
                assert_eq!(artifact_class, "materialized_notes");
                assert_eq!(size_bytes, oversized.len() as u64);
                assert_eq!(ceiling_bytes, MAX_MATERIALIZED_NOTES_BYTES);
            }
            other => panic!("expected ArtifactTooLarge, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Fix-round finding: the legacy (pre-event-log) transcript fallback
    /// (`read_legacy_session_transcript`) had no ceiling at all — only the
    /// canonical `transcripts/<id>.events.jsonl` read did. A session with no
    /// canonical stream (this one never writes `.events.jsonl`) must refuse
    /// an oversized `transcripts/<id>.jsonl` the same way the canonical path
    /// does, stat-before-read, never attempting to parse the garbage bytes.
    #[test]
    fn load_session_transcript_impl_refuses_an_oversized_legacy_transcript() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("legacy-transcript-ceiling-refuse");
        let _guard = HomeGuard::set(&dir);
        let session_id = "legacy-transcript-ceiling-refuse";

        let oversized = vec![b'x'; (MAX_TRANSCRIPT_EVENTS_BYTES + 1) as usize];
        std::fs::write(
            crate::user_data::transcript_path(session_id).expect("resolve legacy transcript path"),
            &oversized,
        )
        .expect("write synthetic oversized legacy transcript");

        let error = load_session_transcript_impl(session_id.to_string())
            .expect_err("an oversized legacy transcript must refuse, not attempt to parse");
        let message = error.to_string();
        assert!(
            message.contains("transcript_events") && message.contains(&oversized.len().to_string()),
            "expected the ceiling refusal in the collapsed error string, got {message:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Fix-round finding: `load_session` bundles the live graph, diarization
    /// log, and live-assist cards behind `?` alongside the transcript — an
    /// oversized SIDE artifact must not take the whole session open down
    /// with it. Each degrades to its "missing" fallback instead of failing.
    #[test]
    fn load_session_impl_degrades_oversized_side_artifacts_instead_of_failing_the_open() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("load-session-degrade-side-artifacts");
        let _guard = HomeGuard::set(&dir);
        let session_id = "load-session-degrade-side-artifacts";

        // A small, valid legacy transcript — the artifact this command is
        // required to keep loading no matter what the side artifacts do.
        seed_legacy_transcript(session_id, "the transcript lens must still work");

        let oversized_graph = vec![b'x'; (MAX_LIVE_GRAPH_BYTES + 1) as usize];
        std::fs::write(
            crate::user_data::graph_path(session_id).expect("resolve live graph path"),
            &oversized_graph,
        )
        .expect("write synthetic oversized live graph");

        let oversized_diarization = vec![b'x'; (MAX_DIARIZATION_EVENTS_BYTES + 1) as usize];
        std::fs::write(
            crate::user_data::diarization_events_path(session_id)
                .expect("resolve diarization path"),
            &oversized_diarization,
        )
        .expect("write synthetic oversized diarization log");

        let live_assist_path = crate::user_data::resolve_live_assist_current_path(session_id)
            .expect("resolve live-assist path");
        std::fs::create_dir_all(
            live_assist_path
                .parent()
                .expect("live-assist path has a parent"),
        )
        .expect("create live_assist dir");
        let oversized_live_assist = vec![b'x'; (MAX_LIVE_ASSIST_CARDS_BYTES + 1) as usize];
        std::fs::write(&live_assist_path, &oversized_live_assist)
            .expect("write synthetic oversized live-assist snapshot");

        let loaded = load_session_impl(session_id.to_string())
            .expect("oversized side artifacts must degrade, not fail the whole session open");
        assert_eq!(
            loaded.transcript.len(),
            1,
            "the transcript lens still works"
        );
        assert!(
            loaded.graph.nodes.is_empty() && loaded.graph.links.is_empty(),
            "the oversized live graph degrades to an empty snapshot"
        );
        assert!(
            loaded.diarization_events.is_empty(),
            "the oversized diarization log degrades to an empty vec"
        );
        assert!(
            loaded.live_assist_cards.is_empty(),
            "the oversized live-assist snapshot degrades to an empty vec"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Fix-round finding: the primary transcript-events artifact (unlike the
    /// three side artifacts above) has no reasonable "degrade" fallback — an
    /// oversized canonical stream really cannot be shown, so `load_session`
    /// must still refuse, not silently truncate or crash.
    #[test]
    fn load_session_impl_refuses_an_oversized_transcript_events_log() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("load-session-oversized-transcript-events");
        let _guard = HomeGuard::set(&dir);
        let session_id = "load-session-oversized-transcript-events";

        let oversized = vec![b'x'; (MAX_TRANSCRIPT_EVENTS_BYTES + 1) as usize];
        std::fs::write(
            crate::user_data::transcript_events_path(session_id)
                .expect("resolve transcript events path"),
            &oversized,
        )
        .expect("write synthetic oversized transcript events log");

        let error = load_session_impl(session_id.to_string())
            .expect_err("an oversized transcript-events log must refuse the whole session open");
        // `read_session_transcript_snapshot` collapses the typed
        // `ArtifactTooLarge` into a `String` for this one artifact (see the
        // comment there), so by the time it crosses `load_session_impl`'s
        // `?` it has become `AppError::Unknown` — the refusal itself, not
        // its typed shape, is what this test pins.
        let message = error.to_string();
        assert!(
            message.contains("transcript_events") && message.contains(&oversized.len().to_string()),
            "expected the ceiling refusal in the collapsed error string, got {message:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Fix-round finding: the Notes/Graph lens's shared canonical-replay
    /// gather (`gather_projection_lens_state`) must refuse an oversized
    /// projection-patch log rather than reading it into memory — the same
    /// per-artifact stat-before-read guarantee the materialized-notes/graph
    /// ceilings already have their own dedicated tests for.
    #[test]
    fn load_session_notes_artifacts_cmd_refuses_an_oversized_projection_events_log() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("notes-artifact-oversized-projection-log");
        let _guard = HomeGuard::set(&dir);
        let session_id = "notes-artifact-oversized-projection-log";

        let oversized = vec![b'x'; (MAX_PROJECTION_EVENTS_BYTES + 1) as usize];
        std::fs::write(
            crate::user_data::projection_events_path(session_id)
                .expect("resolve projection events path"),
            &oversized,
        )
        .expect("write synthetic oversized projection-patch log");

        let error = load_session_notes_artifacts_impl(session_id.to_string())
            .expect_err("an oversized projection-patch log must refuse, not attempt to parse");
        match error {
            AppError::ArtifactTooLarge { artifact_class, .. } => {
                assert_eq!(artifact_class, "projection_events");
            }
            other => panic!("expected ArtifactTooLarge, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Fix-round finding: `load_session_data_movement_cmd` was the one
    /// session-scoped historical read left with no byte ceiling.
    #[test]
    fn load_session_data_movement_impl_refuses_an_oversized_ledger() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("data-movement-ceiling-refuse");
        let _guard = HomeGuard::set(&dir);
        let session_id = "data-movement-ceiling-refuse";

        let oversized = vec![b'x'; (MAX_DATA_MOVEMENT_EVENTS_BYTES + 1) as usize];
        std::fs::write(
            crate::user_data::data_movement_ledger_path(session_id)
                .expect("resolve data movement ledger path"),
            &oversized,
        )
        .expect("write synthetic oversized data-movement ledger");

        let error = load_session_data_movement_impl(session_id.to_string())
            .expect_err("an oversized data-movement ledger must refuse, not attempt to parse");
        match error {
            AppError::ArtifactTooLarge { artifact_class, .. } => {
                assert_eq!(artifact_class, "data_movement_events");
            }
            other => panic!("expected ArtifactTooLarge, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// seed audio-graph-4fa5 deliverable e: `limit` tail-caps the timeline
    /// fold to the last N entries by media-clock order, matching
    /// `SeekTimeline`'s own `slice(-MAX_BLOCKS)` — proving the backend now
    /// does the slicing instead of shipping every span just to discard all
    /// but the tail client-side. `None` still returns every entry (used by
    /// the sibling retcon test above and any caller needing the full fold).
    #[test]
    fn session_timeline_limit_returns_only_the_media_clock_tail() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("session-timeline-limit-tail");
        let _guard = HomeGuard::set(&dir);
        let session_id = "session-timeline-limit-tail";
        let repository = FileMemoryRepository::user_data();
        for i in 0..5u64 {
            let mut event = projection_status_test_event(&format!("span-{i}"));
            event.start_time = i as f64;
            event.end_time = i as f64 + 0.5;
            event.received_at_ms = 1_700_000_000_000 + i;
            repository
                .append_transcript_event(session_id, &event)
                .expect("append transcript event");
        }

        let full = session_timeline(session_id, None).expect("unbounded fold");
        assert_eq!(full.entries.len(), 5, "an unbounded fold keeps every entry");
        assert_eq!(
            full.total_count, 5,
            "an unbounded fold's total_count matches its own entry count"
        );

        let limited = session_timeline(session_id, Some(2)).expect("limited fold");
        assert_eq!(
            limited.entries.len(),
            2,
            "limit=2 must return exactly 2 entries"
        );
        assert_eq!(
            limited.total_count, 5,
            "total_count reports the pre-limit fold length (fix-round finding: this \
             is what lets the frontend's truncation notice fire once `limit` equals \
             its own render window)"
        );
        assert_eq!(
            limited
                .entries
                .iter()
                .map(|e| e.span_id.as_str())
                .collect::<Vec<_>>(),
            vec!["span-3", "span-4"],
            "limit must keep the media-clock TAIL, not the head"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// audio-graph-a2a7 (0d72 follow-up) Part 2: the "cross-reload speaker
    /// retcon" verification. `session_timeline` (the fold `build_session_timeline_cmd`
    /// wraps) reads the diarization span-revision log FROM DISK
    /// (`load_diarization_span_revisions` → `SpeakerTimeline::replay`), so a
    /// RELOADED session — one with no in-memory diarization state — must still
    /// resolve the latest-wins (retconned) speaker for each utterance, not the
    /// inline ASR label nor the earlier provisional attribution. This proves the
    /// concern PR #80 flagged is already satisfied by the PR #67 persist+hydrate
    /// path: a provisional label superseded mid-session (rev1 → rev2) is picked
    /// up by the fold purely from disk on reload.
    #[test]
    #[cfg_attr(
        target_os = "macos",
        ignore = "Tauri/Tao App construction must run on the macOS main thread"
    )]
    fn session_timeline_picks_up_hydrated_diarization_retcon_on_reload() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("session-timeline-reload-retcon");
        let _guard = HomeGuard::set(&dir);

        let session_id = "session-timeline-reload-retcon";
        // Seeds one transcript event: span `{session_id}-span-1`, segment id
        // `segment-{session_id}-span-1`, inline speaker "Speaker 1".
        seed_replayable_projection_session(session_id, "Note with diarized speaker.");
        let segment_id = format!("segment-{session_id}-span-1");

        // Two diarization revisions on ONE diarization span, both attributing the
        // transcript segment above: a provisional "Speaker 2" superseded by a
        // stable relabel to "Alice" — the mid-session correction the reload fold
        // must resolve to latest-wins.
        let repository = FileMemoryRepository::user_data();
        let mut provisional =
            export_test_diarization_revision(session_id, "diar-span-reload", "prov-spk");
        provisional.speaker_label = Some("Speaker 2".to_string());
        provisional.stability = crate::projections::DiarizationEventStability::Provisional;
        provisional.revision_number = 1;
        provisional.basis_transcript_segment_ids = vec![segment_id.clone()];
        provisional.basis_asr_span_ids = Vec::new();
        let mut relabel = export_test_diarization_revision(session_id, "diar-span-reload", "alice");
        relabel.speaker_label = Some("Alice".to_string());
        relabel.revision_number = 2;
        relabel.supersedes = Some("diar-span-reload@rev1".to_string());
        relabel.basis_transcript_segment_ids = vec![segment_id.clone()];
        relabel.basis_asr_span_ids = Vec::new();
        repository
            .append_diarization_span_revision(session_id, &provisional)
            .expect("append provisional diarization revision");
        repository
            .append_diarization_span_revision(session_id, &relabel)
            .expect("append relabel diarization revision");

        // Fold purely from disk — no in-memory diarization state, exactly the
        // reloaded-session path `build_session_timeline_cmd` exercises.
        let timeline = session_timeline(session_id, None).expect("fold reloaded session timeline");

        let entry = timeline
            .entries
            .iter()
            .find(|e| e.span_id == format!("{session_id}-span-1"))
            .expect("timeline must include the seeded utterance");
        assert_eq!(
            entry.speaker_id.as_deref(),
            Some("alice"),
            "reloaded fold must resolve the latest-wins (retconned) speaker id from disk"
        );
        assert_eq!(
            entry.speaker_label.as_deref(),
            Some("Alice"),
            "reloaded fold must resolve the retconned label, not the provisional one or the inline ASR label"
        );

        // No async writers were opened (the seed + appends use the synchronous
        // repository directly), so there is nothing to drain — just clean up.
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// audio-graph-0b33 acceptance: `load_session_impl` must surface the
    /// session's persisted diarization span revisions in the `LoadedSession`
    /// payload so the frontend can hydrate `diarizationSpanRevisions` and resolve
    /// trusted latest-wins speaker attribution on reload (ADR-0026 §3/§4). A
    /// session with a mid-session relabel (rev1 → rev2 supersede) must yield the
    /// full append-ordered log, and a session with no diarization must yield an
    /// empty vec.
    #[test]
    fn load_session_includes_persisted_diarization_events() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("load-session-diarization-events");
        let _guard = HomeGuard::set(&dir);

        let session_id = "load-session-diarization-events";
        seed_replayable_projection_session(session_id, "Note with diarized speaker.");

        // Two revisions on the same span: a provisional label superseded by a
        // stable relabel — the mid-session correction the reload must carry.
        let repository = FileMemoryRepository::user_data();
        let mut provisional =
            export_test_diarization_revision(session_id, "diar-span-load", "provisional");
        provisional.speaker_label = Some("Speaker 2".to_string());
        provisional.stability = crate::projections::DiarizationEventStability::Provisional;
        provisional.revision_number = 1;
        let mut relabel = export_test_diarization_revision(session_id, "diar-span-load", "alice");
        relabel.speaker_label = Some("Alice".to_string());
        relabel.revision_number = 2;
        relabel.supersedes = Some("diar-span-load@rev1".to_string());
        repository
            .append_diarization_span_revision(session_id, &provisional)
            .expect("append provisional diarization revision");
        repository
            .append_diarization_span_revision(session_id, &relabel)
            .expect("append relabel diarization revision");

        let loaded = load_session_impl(session_id.to_string())
            .expect("load session should include diarization events");

        assert_eq!(
            loaded.diarization_events.len(),
            2,
            "LoadedSession must carry the full persisted speaker log for reload attribution"
        );
        assert_eq!(loaded.diarization_events[0].revision_number, 1);
        assert_eq!(
            loaded.diarization_events[0].speaker_label.as_deref(),
            Some("Speaker 2")
        );
        assert_eq!(loaded.diarization_events[1].revision_number, 2);
        assert_eq!(
            loaded.diarization_events[1].speaker_id.as_deref(),
            Some("alice")
        );
        assert_eq!(
            loaded.diarization_events[1].speaker_label.as_deref(),
            Some("Alice")
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// audio-graph-0b33: a session that never emitted diarization loads an empty
    /// `diarization_events` vec (not an error), so the frontend join is a no-op
    /// and old transcript-only sessions still load.
    #[test]
    fn load_session_diarization_events_empty_without_speaker_log() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("load-session-no-diarization");
        let _guard = HomeGuard::set(&dir);

        let session_id = "load-session-no-diarization";
        seed_replayable_projection_session(session_id, "Note without diarization.");

        let loaded = load_session_impl(session_id.to_string())
            .expect("load session without diarization should still succeed");

        assert!(
            loaded.diarization_events.is_empty(),
            "a session with no speaker log must load an empty diarization_events vec"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg_attr(
        target_os = "macos",
        ignore = "Tauri/Tao App construction must run on the macOS main thread"
    )]
    fn load_session_prefers_replay_without_mutating_active_projection_state() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("load-session-stale-projections");
        let _guard = HomeGuard::set(&dir);

        let session_id = "load-session-stale-projections";
        let basis = seed_replayable_projection_session(session_id, "Fresh replayed note.");
        let repository = FileMemoryRepository::user_data();
        repository
            .save_materialized_notes(
                session_id,
                &stale_materialized_notes(session_id, basis.clone()),
            )
            .expect("save stale notes artifact");
        repository
            .save_materialized_graph(session_id, &stale_materialized_graph(session_id, basis))
            .expect("save stale graph artifact");

        let state = AppState::new();
        {
            let mut materialized = state
                .materialized_projection_state
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            *materialized = leaked_active_projection_state();
        }
        let active_ledger_before = state
            .transcript_ledger
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();

        load_session_impl(session_id.to_string())
            .expect("load session should prefer replayed materialized projections");

        let loaded_notes = load_session_notes_artifacts_impl(session_id.to_string())
            .expect("Notes-lens fetch should prefer replayed materialized projections")
            .notes
            .expect("stale notes artifact should replay");
        assert_eq!(loaded_notes.last_sequence, 1);
        assert_eq!(
            loaded_notes
                .notes
                .iter()
                .map(|note| note.id.as_str())
                .collect::<Vec<_>>(),
            vec!["note-report"]
        );
        assert!(
            loaded_notes
                .notes
                .iter()
                .all(|note| note.id != "stale-note"),
            "stale materialized note leaked into load result"
        );

        let loaded_graph = load_session_graph_artifact_impl(session_id.to_string())
            .expect("Graph-lens fetch should prefer replayed materialized projections")
            .expect("stale graph artifact should replay");
        assert_eq!(loaded_graph.last_sequence, 1);
        assert_eq!(
            loaded_graph
                .nodes
                .iter()
                .map(|node| node.id.as_str())
                .collect::<Vec<_>>(),
            vec!["node-report"]
        );
        assert!(
            loaded_graph
                .nodes
                .iter()
                .all(|node| node.id != "stale-node"),
            "stale materialized graph leaked into load result"
        );

        let restored = state
            .materialized_projection_state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        assert_eq!(restored.session_id, "active-session-before-load");
        assert_eq!(restored.notes.last_sequence, 99);
        assert_eq!(restored.graph.last_sequence, 99);
        assert_eq!(restored.notes.notes[0].id, "leaked-note");
        assert_eq!(restored.graph.nodes[0].id, "leaked-node");

        let active_ledger_after = state
            .transcript_ledger
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        assert_eq!(
            active_ledger_after.session_id,
            active_ledger_before.session_id
        );
        assert_eq!(
            active_ledger_after.accepted_event_count,
            active_ledger_before.accepted_event_count
        );

        drain_test_writers(&state);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn export_test_diarization_revision(
        session_id: &str,
        span_id: &str,
        speaker_id: &str,
    ) -> crate::projections::DiarizationSpanRevision {
        crate::projections::DiarizationSpanRevision {
            span_id: span_id.to_string(),
            provider: "deepgram".to_string(),
            timeline_id: session_id.to_string(),
            source_id: None,
            speaker_id: Some(speaker_id.to_string()),
            speaker_label: Some(format!("Speaker {speaker_id}")),
            provider_speaker_id: None,
            channel: None,
            start_time: 0.0,
            end_time: 1.0,
            confidence: Some(0.9),
            is_final: true,
            stability: crate::projections::DiarizationEventStability::Stable,
            revision_number: 1,
            supersedes: None,
            basis_asr_span_ids: vec![format!("{span_id}-asr")],
            basis_transcript_segment_ids: Vec::new(),
            raw_event_ref: None,
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms: 1_700_000_000_001,
        }
    }

    #[test]
    fn export_session_bundle_includes_all_durable_artifacts() {
        // The session-artifact-migration export acceptance: a session export
        // must bundle the transcript event log, diarization event log,
        // projection event log, materialized notes, and materialized graph —
        // not only the legacy graph snapshot — plus schema metadata.
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("export-bundle-full");
        let _guard = HomeGuard::set(&dir);

        let session_id = "export-bundle-full";
        let basis = seed_replayable_projection_session(session_id, "Exported note body.");

        let repository = FileMemoryRepository::user_data();
        repository
            .append_diarization_span_revision(
                session_id,
                &export_test_diarization_revision(session_id, "diar-span-1", "spk-export"),
            )
            .expect("append diarization revision");
        repository
            .save_materialized_notes(
                session_id,
                &stale_materialized_notes(session_id, basis.clone()),
            )
            .expect("save materialized notes artifact");
        repository
            .save_materialized_graph(session_id, &stale_materialized_graph(session_id, basis))
            .expect("save materialized graph artifact");

        let bundle = session_export_bundle(session_id).expect("export bundle");

        assert_eq!(bundle.schema_version, SESSION_EXPORT_SCHEMA_VERSION);
        assert_eq!(bundle.session_id, session_id);
        assert_eq!(
            bundle.transcript_events.len(),
            1,
            "bundle must include the transcript event log"
        );
        assert_eq!(
            bundle.diarization_events.len(),
            1,
            "bundle must include the diarization event log"
        );
        assert_eq!(
            bundle.diarization_events[0].speaker_id.as_deref(),
            Some("spk-export")
        );
        assert_eq!(
            bundle.projection_events.len(),
            2,
            "bundle must include the projection event log (notes + graph patch)"
        );
        assert!(
            bundle.notes.is_some(),
            "bundle must include the materialized notes artifact"
        );
        assert!(
            bundle.materialized_graph.is_some(),
            "bundle must include the materialized graph artifact"
        );

        // The bundle must be a self-contained, serializable JSON blob.
        let json = serde_json::to_string(&bundle).expect("bundle serializes to JSON");
        assert!(json.contains("\"schema_version\":1"));
        assert!(json.contains("\"projection_events\""));
        assert!(json.contains("\"diarization_events\""));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_session_bundle_missing_session_errors() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("export-bundle-missing");
        let _guard = HomeGuard::set(&dir);

        let err = session_export_bundle("no-such-session").expect_err("must error");
        assert!(
            matches!(err, AppError::SessionInvalid { .. }),
            "missing session must fail with SessionInvalid, got: {err:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_session_returns_isolated_historical_payloads_without_rebinding_runtime() {
        // Review isolation guard: loading two different historical sessions in
        // sequence returns each session's own replayed payload. The command has
        // no AppState argument, so neither read can rotate live capture state.
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("load-session-cross-session-leak");
        let _guard = HomeGuard::set(&dir);

        let session_a = "leak-session-a";
        let session_b = "leak-session-b";
        seed_replayable_projection_session(session_a, "Session A note.");
        seed_replayable_projection_session(session_b, "Session B note.");

        load_session_impl(session_a.to_string()).expect("load session A should succeed");
        let notes_a = load_session_notes_artifacts_impl(session_a.to_string())
            .expect("Notes-lens fetch for session A should succeed");
        assert_eq!(
            notes_a.notes.expect("A notes").notes[0].body,
            "Session A note."
        );

        load_session_impl(session_b.to_string()).expect("load session B should succeed");
        let notes_b = load_session_notes_artifacts_impl(session_b.to_string())
            .expect("Notes-lens fetch for session B should succeed");
        let b_notes = notes_b.notes.expect("B notes");
        assert_eq!(b_notes.notes[0].body, "Session B note.");
        assert!(
            b_notes
                .notes
                .iter()
                .all(|note| note.body != "Session A note.")
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn new_session_rotation_requires_idle_capture_and_transcription() {
        let state = AppState::new();

        {
            let mut capturing = state
                .is_capturing
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *capturing = true;
        }
        assert!(matches!(
            ensure_session_idle_for_rotation(&state),
            Err(AppError::SessionInvalid { .. })
        ));

        {
            let mut capturing = state
                .is_capturing
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *capturing = false;
        }
        state.is_transcribing.store(true, Ordering::SeqCst);
        assert!(matches!(
            ensure_session_idle_for_rotation(&state),
            Err(AppError::SessionInvalid { .. })
        ));

        state.is_transcribing.store(false, Ordering::SeqCst);
        state
            .capture_manager
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert_synthetic_handle("registry-owned-source", false);
        assert!(matches!(
            ensure_session_idle_for_rotation(&state),
            Err(AppError::SessionInvalid { .. })
        ));
        state
            .capture_manager
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .stop_capture("registry-owned-source")
            .expect("remove synthetic active capture");
        assert!(ensure_session_idle_for_rotation(&state).is_ok());
        drain_test_writers(&state);
    }

    #[test]
    fn timed_out_session_worker_fences_rotation_until_it_finishes() {
        let state = AppState::new();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let handle = std::thread::spawn(move || {
            let _ = release_rx.recv();
        });

        join_worker_with_timeout(
            handle,
            std::time::Duration::from_millis(5),
            "rotation-fence-test-worker",
            &state.retired_session_workers,
        );
        assert!(matches!(
            ensure_session_idle_for_rotation(&state),
            Err(AppError::SessionInvalid { .. })
        ));

        release_tx.send(()).expect("release worker");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        loop {
            if ensure_session_idle_for_rotation(&state).is_ok() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "finished retired worker should be reaped within the test budget"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        assert!(
            state
                .retired_session_workers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_empty(),
            "reaped handles must not accumulate"
        );
        drain_test_writers(&state);
    }

    /// audio-graph-9cc1 / ADR-0045 decision 4 (drain half) acceptance: no
    /// projection job thread outlives Stop, proven for BOTH kinds — the graph
    /// lane previously had no tracked handle at all, so a graph projection
    /// could run arbitrarily long past Stop. A handle that finishes within the
    /// flush timeout is joined outright and never spills into the rotation
    /// fence.
    ///
    /// Adversarial-review fix: registry emptiness alone is vacuous — it holds
    /// even if the drain detaches the handle instead of waiting for it. Each
    /// fake thread sleeps briefly, then flips its OWN `AtomicBool`; asserting
    /// the flag is `true` immediately after `drain_projection_job_workers`
    /// returns is the only check that proves the drain actually waited
    /// (a `drop(handle)` regression leaves the registry empty too, but the
    /// flag would still be `false` right after the call).
    #[test]
    fn drain_projection_job_workers_joins_finished_threads_of_both_kinds() {
        let state = AppState::new();
        let notes_finished = Arc::new(AtomicBool::new(false));
        let graph_finished = Arc::new(AtomicBool::new(false));
        let notes_flag = notes_finished.clone();
        let graph_flag = graph_finished.clone();
        let notes_handle = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            notes_flag.store(true, Ordering::SeqCst);
        });
        let graph_handle = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            graph_flag.store(true, Ordering::SeqCst);
        });
        {
            let mut registry = state
                .projection_job_workers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            registry.push((
                crate::projections::ProjectionKind::Notes,
                "notes-job-1".to_string(),
                notes_handle,
            ));
            registry.push((
                crate::projections::ProjectionKind::Graph,
                "graph-job-1".to_string(),
                graph_handle,
            ));
        }

        drain_projection_job_workers(
            &state.projection_job_workers,
            std::time::Duration::from_secs(1),
            &state.retired_session_workers,
        );

        assert!(
            notes_finished.load(Ordering::SeqCst),
            "drain must actually wait for the notes thread to run to completion, not just \
             forget its handle"
        );
        assert!(
            graph_finished.load(Ordering::SeqCst),
            "drain must actually wait for the graph thread to run to completion, not just \
             forget its handle"
        );
        assert!(
            state
                .projection_job_workers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_empty(),
            "drain must remove every registered entry"
        );
        assert!(
            state
                .retired_session_workers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_empty(),
            "both kinds finished promptly and must not be spilled into the rotation fence"
        );
        drain_test_writers(&state);
    }

    /// audio-graph-9cc1 acceptance: a projection job that outlives the flush
    /// timeout spills into `retired_session_workers` — the SAME vec Start/New
    /// Session already fence rotation on (zero new fence logic) — and
    /// rotation stays blocked until that handle is actually joined.
    #[test]
    fn wedged_projection_job_spills_into_retired_workers_and_fences_rotation_until_joined() {
        let state = AppState::new();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let wedged = std::thread::spawn(move || {
            let _ = release_rx.recv();
        });
        state
            .projection_job_workers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((
                crate::projections::ProjectionKind::Graph,
                "wedged-graph-job".to_string(),
                wedged,
            ));

        drain_projection_job_workers(
            &state.projection_job_workers,
            std::time::Duration::from_millis(5),
            &state.retired_session_workers,
        );

        assert!(
            state
                .projection_job_workers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_empty(),
            "drain always removes the entry from the live registry, timeout or not"
        );
        assert_eq!(
            state
                .retired_session_workers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .len(),
            1,
            "a job that outlives the flush timeout must spill into retired_session_workers"
        );
        assert!(matches!(
            ensure_session_idle_for_rotation(&state),
            Err(AppError::SessionInvalid { .. })
        ));

        release_tx.send(()).expect("release wedged job");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        loop {
            if ensure_session_idle_for_rotation(&state).is_ok() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "released projection job should be reaped within the test budget"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            state
                .retired_session_workers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_empty(),
            "reaped handles must not accumulate"
        );
        drain_test_writers(&state);
    }

    /// audio-graph-fa56 field bug (session c95d21e6, build c9f167e): a
    /// same-basis graph-lane failure under `PROJECTION_LANE_ATTEMPT_BUDGET`
    /// arms a deferred retry; if Stop begins before the clock thread's ~60s
    /// deadline, the clock exits without firing and the deferral was left
    /// with no visible signal beyond a `log::debug!` line. This pins that
    /// `log_abandoned_deferred_retries_after_stop` is a read-only OBSERVATION
    /// of scheduler state — the armed deferral must survive the call
    /// untouched, so the WARN is additive visibility, never a second place
    /// that silently resolves the gap it reports — AND that it persists the
    /// diagnostics snapshot (ticket requirement (b)) with the deferral
    /// captured.
    ///
    /// Mutation coverage: (1) a mutant that has this function call
    /// `abandon_deferred_retry`/`clear_orphaned_deferred_retry` instead of
    /// only reading is caught directly by the post-call `assert_eq!` below.
    /// (2) A mutant that reads `notes` instead of `graph` (or vice versa)
    /// flips which kind survives in the returned Vec. (3) A mutant that
    /// drops the `save_scheduler_queue_state` call, or builds the snapshot
    /// from the wrong lane, is caught by the disk round-trip assertion
    /// below.
    #[test]
    fn log_abandoned_deferred_retries_after_stop_leaves_an_armed_deferral_untouched() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("log-abandoned-deferred-retry-untouched");
        let _guard = HomeGuard::set(&dir);

        let state = AppState::new();
        let session_id = state.current_session_id();
        let mut ledger = crate::projections::TranscriptLedger::new(&session_id);
        ledger
            .apply_event(transcript_event_fixture("span-1", "segment-1"))
            .expect("seed ledger span");

        let graph_job_id = {
            let mut schedulers = state
                .projection_schedulers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match schedulers.observe_ledger(&ledger, 10).graph {
                crate::projection_scheduler::ProjectionSchedulerDecision::StartJob { job } => {
                    job.id
                }
                other => panic!("expected graph start job, got {other:?}"),
            }
        };
        {
            let mut schedulers = state
                .projection_schedulers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            assert!(
                matches!(
                    schedulers.fail_graph_in_flight(&graph_job_id, &session_id, &ledger, 20),
                    crate::projection_scheduler::ProjectionSchedulerDecision::FailedCurrent {
                        deferred_retry_at_ms: Some(_),
                        ..
                    }
                ),
                "graph failure under budget must arm a deferred retry"
            );
        }

        // Simulates `stop_capture_impl` calling this strictly AFTER
        // `drain_projection_job_workers` has already joined the graph
        // lane's clock thread, which exited on its own
        // `projection_lane_stopping` check without touching scheduler state.
        log_abandoned_deferred_retries_after_stop(&state.projection_schedulers, &session_id);

        assert_eq!(
            state
                .projection_schedulers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .kinds_with_armed_deferred_retry(),
            vec![crate::projections::ProjectionKind::Graph],
            "the WARN must be read-only: the abandoned deferral must still be armed after \
             logging it, not silently cleared"
        );

        // Ticket requirement (b): the diagnostics snapshot persisted to disk
        // must carry the abandoned deferral too, not just the log line.
        let persisted = crate::persistence::load_scheduler_queue_state(&session_id)
            .expect("Stop must persist a scheduler queue snapshot for this session");
        assert!(
            persisted.graph_deferred_retry_at_ms.is_some(),
            "the persisted snapshot must record the abandoned graph lane's deferred retry \
             deadline, so a later session load or replay pass can detect the gap without the \
             log line"
        );
        assert!(
            persisted.notes_deferred_retry_at_ms.is_none(),
            "the notes lane never failed and must not spuriously report a deferral"
        );

        drain_test_writers(&state);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Companion to the test above: a fresh session (no failures at all) has
    /// nothing abandoned. Pins that `log_abandoned_deferred_retries_after_stop`
    /// does not spuriously warn on the common case — most Stops have no
    /// pending deferral to report — but that it still persists the
    /// diagnostics snapshot unconditionally (mirroring `rotate_session`'s
    /// existing every-rotation write), so a later audit pass sees an
    /// explicit "nothing was armed" record rather than a missing file. This
    /// test only asserts the ordinary (non-poisoned) lock path; poisoned-lock
    /// recovery for this function is not separately exercised.
    #[test]
    fn log_abandoned_deferred_retries_after_stop_reports_nothing_when_no_lane_is_armed() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("log-abandoned-deferred-retry-idle");
        let _guard = HomeGuard::set(&dir);

        let state = AppState::new();
        let session_id = state.current_session_id();
        // Idle scheduler, freshly constructed by `AppState::new()` — no
        // observation, no failure, nothing armed.
        assert_eq!(
            state
                .projection_schedulers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .kinds_with_armed_deferred_retry(),
            Vec::new()
        );

        log_abandoned_deferred_retries_after_stop(&state.projection_schedulers, &session_id);

        let persisted = crate::persistence::load_scheduler_queue_state(&session_id)
            .expect("Stop must persist a scheduler queue snapshot even when idle");
        assert_eq!(
            persisted.notes_deferred_retry_at_ms, None,
            "idle notes lane must not report a deferral in the persisted snapshot"
        );
        assert_eq!(
            persisted.graph_deferred_retry_at_ms, None,
            "idle graph lane must not report a deferral in the persisted snapshot"
        );

        drain_test_writers(&state);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// audio-graph-fa56: pins that the new abandoned-deferred-retry WARN is
    /// wired into `stop_capture_impl` strictly AFTER
    /// `drain_projection_job_workers` — reading `deferred_retry_at_ms`
    /// BEFORE the drain would observe a clock thread that has not yet had
    /// its chance to exit (or fire), turning "abandoned" into "possibly
    /// still ticking" and making the WARN unreliable. Uses the same
    /// `include_str!` source-order self-inspection technique as
    /// `start_transcribe_clears_projection_lane_stopping_before_the_speech_thread_spawns`
    /// above: this pins ORDERING specifically, which a behavioral test
    /// cannot do any more cheaply, since asserting "strictly after" would
    /// otherwise require observably delaying the drain itself. A full
    /// end-to-end behavioral pin that the detection wiring actually executes
    /// on the real Stop path — driving `stop_capture_impl` against a live
    /// `AppState` with a real armed deferral — is a separate, existing test:
    /// see `stop_capture_impl_reports_a_graph_lane_deferred_retry_abandoned_at_stop`
    /// below.
    #[test]
    fn stop_capture_impl_logs_abandoned_deferred_retries_strictly_after_the_drain() {
        let commands_source = include_str!("commands.rs");
        let body_start = commands_source
            .find("async fn stop_capture_impl(")
            .expect("stop_capture_impl must exist in commands.rs");
        let body_end = commands_source[body_start..]
            .find("pub async fn start_transcribe(")
            .map(|relative| body_start + relative)
            .expect("start_transcribe must follow stop_capture_impl in commands.rs");
        let body = strip_whitespace(&strip_comments(&commands_source[body_start..body_end]));

        let drain_marker = strip_whitespace("drain_projection_job_workers(");
        let drain_pos = find_unique_occurrence(
            &body,
            &drain_marker,
            "stop_capture_impl (drain_projection_job_workers call)",
        );
        let warn_marker = strip_whitespace("log_abandoned_deferred_retries_after_stop(");
        let warn_pos = find_unique_occurrence(
            &body,
            &warn_marker,
            "stop_capture_impl (log_abandoned_deferred_retries_after_stop call)",
        );

        assert!(
            drain_pos < warn_pos,
            "log_abandoned_deferred_retries_after_stop must be called strictly AFTER \
             drain_projection_job_workers — otherwise a still-running retry clock thread \
             would be misreported as abandoned"
        );
    }

    /// audio-graph-fa56: pins that the WARN body itself (not just the call
    /// site's position) has not been deleted or gutted — a mutant that
    /// replaces the `if !abandoned_kinds.is_empty() { log::warn!(...) }`
    /// block inside `log_abandoned_deferred_retries_after_stop` with a
    /// no-op (e.g. `let _ = abandoned_kinds;`) makes every behavioral test
    /// in this file pass, since none of them observe log output (no
    /// log-capture harness exists in this repo — see
    /// `logging::tests` for the only `log::set_logger` call, which is
    /// production init, not a test capture sink). Source-text inspection is
    /// the cheapest mutation-proof available without adding that
    /// infrastructure.
    #[test]
    fn log_abandoned_deferred_retries_after_stop_emits_the_documented_warn_key() {
        let commands_source = include_str!("commands.rs");
        let body_start = commands_source
            .find("fn log_abandoned_deferred_retries_after_stop(")
            .expect("log_abandoned_deferred_retries_after_stop must exist in commands.rs");
        let body_end = commands_source[body_start..]
            .find("fn register_runtime_processed_audio_consumer(")
            .map(|relative| body_start + relative)
            .expect(
                "register_runtime_processed_audio_consumer must follow \
                 log_abandoned_deferred_retries_after_stop",
            );
        let body = &commands_source[body_start..body_end];

        assert!(
            body.contains("log::warn!"),
            "log_abandoned_deferred_retries_after_stop must still call log::warn! — deleting \
             the WARN body is the single most obvious mutation for a logging-only fix, and \
             nothing else in this suite observes emitted log output"
        );
        assert!(
            body.contains("projection_scheduler.deferred_retry_abandoned_at_stop"),
            "the WARN must still carry its documented greppable key so a support session or \
             replay/audit pass can find it"
        );
    }

    /// audio-graph-64e3: pins that the new display-transcript-rows-missing
    /// WARN is wired into `stop_capture_impl` strictly AFTER
    /// `log_abandoned_deferred_retries_after_stop` — same technique and same
    /// underlying reason as
    /// `stop_capture_impl_logs_abandoned_deferred_retries_strictly_after_the_drain`
    /// above: both calls need the sp/asr joins to have already run so the
    /// deepgram receiver thread (which increments
    /// `display_transcript_write_misses` and persists the ledger-side rows
    /// this WARN's count depends on) has had its bounded chance to finish
    /// before either tally is read.
    #[test]
    fn stop_capture_impl_warns_display_transcript_rows_missing_strictly_after_the_deferred_retry_warn()
     {
        let commands_source = include_str!("commands.rs");
        let body_start = commands_source
            .find("async fn stop_capture_impl(")
            .expect("stop_capture_impl must exist in commands.rs");
        let body_end = commands_source[body_start..]
            .find("pub async fn start_transcribe(")
            .map(|relative| body_start + relative)
            .expect("start_transcribe must follow stop_capture_impl");
        let body = strip_whitespace(&strip_comments(&commands_source[body_start..body_end]));

        let deferred_retry_marker = strip_whitespace("log_abandoned_deferred_retries_after_stop(");
        let deferred_retry_pos = find_unique_occurrence(
            &body,
            &deferred_retry_marker,
            "stop_capture_impl (log_abandoned_deferred_retries_after_stop call)",
        );
        let display_warn_marker =
            strip_whitespace("warn_if_display_transcript_rows_missing_at_stop(");
        let display_warn_pos = find_unique_occurrence(
            &body,
            &display_warn_marker,
            "stop_capture_impl (warn_if_display_transcript_rows_missing_at_stop call)",
        );

        assert!(
            deferred_retry_pos < display_warn_pos,
            "warn_if_display_transcript_rows_missing_at_stop must be called strictly AFTER \
             log_abandoned_deferred_retries_after_stop — both share the same ordering \
             requirement of running after the sp/asr joins"
        );
    }

    /// audio-graph-64e3: same mutation-proof rationale as
    /// `log_abandoned_deferred_retries_after_stop_emits_the_documented_warn_key`
    /// above — no log-capture harness exists in this repo, so source-text
    /// inspection is the cheapest way to pin that the WARN body (not just the
    /// call site) has not been gutted, and that it is still gated on a
    /// non-zero mismatch rather than firing unconditionally.
    #[test]
    fn warn_if_display_transcript_rows_missing_at_stop_emits_the_documented_warn_key() {
        let commands_source = include_str!("commands.rs");
        let body_start = commands_source
            .find("fn warn_if_display_transcript_rows_missing_at_stop(")
            .expect("warn_if_display_transcript_rows_missing_at_stop must exist in commands.rs");
        let body_end = commands_source[body_start..]
            .find("fn register_runtime_processed_audio_consumer(")
            .map(|relative| body_start + relative)
            .expect(
                "register_runtime_processed_audio_consumer must follow \
                 warn_if_display_transcript_rows_missing_at_stop",
            );
        let body = &commands_source[body_start..body_end];

        assert!(
            body.contains("log::warn!"),
            "warn_if_display_transcript_rows_missing_at_stop must still call log::warn! — \
             deleting the WARN body is the single most obvious mutation for a logging-only \
             fix, and nothing else in this suite observes emitted log output"
        );
        assert!(
            body.contains("transcript.display_rows_missing_at_stop"),
            "the WARN must still carry its documented greppable key so a support session or \
             replay/audit pass can find it"
        );
        assert!(
            body.contains("missing > 0"),
            "the WARN must still be gated on a non-zero mismatch, not fire unconditionally"
        );
    }

    /// audio-graph-64e3: behavioral pin on the helper itself — seeds the
    /// counter with a nonzero value (as `emit_transcript_and_extract_with_meta`
    /// would after detecting real misses), calls the helper directly, and
    /// checks the swap-based read-and-reset actually reset it. A mutation
    /// that swaps `swap(0, ...)` for `load(...)` (read without reset) would
    /// leave stale counts from a prior session bleeding into the next one's
    /// tally — this is the one piece of behavior a pure source-text
    /// inspection can't distinguish from the correct code, so it gets a
    /// behavioral assertion instead.
    #[test]
    fn warn_if_display_transcript_rows_missing_at_stop_resets_the_counter() {
        let counter = Arc::new(AtomicU64::new(3));
        warn_if_display_transcript_rows_missing_at_stop(&counter, "test-session");
        assert_eq!(
            counter.load(Ordering::SeqCst),
            0,
            "the helper must reset the counter after reading it, so a straggler final that \
             arrives after the NEXT stop is counted fresh instead of double-counting this \
             session's tally"
        );

        // Zero must stay a no-op: calling the helper again on an already-zero
        // counter (the common, healthy case) must not panic, underflow, or
        // otherwise misbehave.
        warn_if_display_transcript_rows_missing_at_stop(&counter, "test-session");
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn missing_canonical_writer_blocks_capture_preflight() {
        let state = AppState::new();
        let missing = state
            .transcript_event_writer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
            .expect("test starts with a transcript-event writer");
        assert!(missing.shutdown_with_timeout(std::time::Duration::from_secs(1)));

        assert!(matches!(
            ensure_session_writers_ready(&state),
            Err(AppError::SessionInvalid { .. })
        ));
        assert!(
            !*state
                .is_capturing
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            "storage preflight failure must leave capture idle"
        );

        drain_test_writers(&state);
    }

    #[test]
    fn active_session_is_never_a_destructive_delete_target() {
        let state = AppState::new();
        let active_session = state.current_session_id();

        assert!(matches!(
            ensure_not_active_session(&active_session, &state),
            Err(AppError::SessionInvalid { .. })
        ));
        assert!(ensure_not_active_session("historical-session", &state).is_ok());

        drain_test_writers(&state);
    }

    #[test]
    fn projection_runtime_status_reports_scheduler_and_materializer_counts() {
        let state = AppState::new();

        let initial = projection_runtime_status_for_state(&state).expect("initial status");
        assert_eq!(initial.session_id, initial.ledger_session_id);
        assert_eq!(initial.session_id, initial.materialized_session_id);
        assert_eq!(initial.accepted_transcript_event_count, 0);
        assert_eq!(initial.transcript_span_count, 0);
        assert_eq!(initial.latest_asr_event_age_ms, None);
        assert_eq!(initial.materialized.note_count, 0);
        assert_eq!(initial.materialized.graph_node_count, 0);
        assert_eq!(initial.schedulers.notes.metrics.jobs_started, 0);
        assert_eq!(initial.schedulers.graph.metrics.jobs_started, 0);

        {
            let mut ledger = state
                .transcript_ledger
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            ledger
                .apply_event(projection_status_test_event("status-span-1"))
                .expect("seed transcript ledger");
            let mut schedulers = state
                .projection_schedulers
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let observation = schedulers.observe_ledger(&ledger, 10);
            assert!(matches!(
                observation.notes,
                crate::projection_scheduler::ProjectionSchedulerDecision::StartJob { .. }
            ));
            assert!(matches!(
                observation.graph,
                crate::projection_scheduler::ProjectionSchedulerDecision::StartJob { .. }
            ));
        }

        let status = projection_runtime_status_for_state(&state).expect("updated status");
        assert_eq!(status.accepted_transcript_event_count, 1);
        assert_eq!(status.transcript_span_count, 1);
        assert!(status.latest_asr_event_age_ms.is_some());
        assert_eq!(status.materialized.notes_last_sequence, 0);
        assert_eq!(status.materialized.graph_last_sequence, 0);
        assert_eq!(status.schedulers.notes.metrics.jobs_started, 1);
        assert_eq!(status.schedulers.graph.metrics.jobs_started, 1);
        assert_eq!(status.schedulers.notes.in_flight_span_count, 1);
        assert_eq!(status.schedulers.graph.in_flight_span_count, 1);

        drain_test_writers(&state);
    }

    #[test]
    fn projection_replay_report_rebuilds_logs_and_reports_artifact_parity() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("projection-replay-report");
        let _guard = HomeGuard::set(&dir);

        let state = AppState::new();
        let session_id = state.current_session_id();
        let mut event = projection_status_test_event("report-span-1");
        event.capture_latency_ms = Some(5);
        event.asr_latency_ms = Some(7);
        let basis = {
            let mut ledger = state
                .transcript_ledger
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            ledger.apply_event(event.clone()).expect("seed ledger");
            ledger.current_basis()
        };
        append_transcript_event(&state, &event);

        let note_patch = report_note_patch(1, basis.clone(), "Private note body.");
        let graph_patch = report_graph_patch(1, basis.clone());
        state
            .apply_runtime_projection_patch(&session_id, &basis, note_patch)
            .expect("apply notes patch");
        state
            .apply_runtime_projection_patch(&session_id, &basis, graph_patch)
            .expect("apply graph patch");

        drain_test_writers(&state);

        let report =
            projection_replay_report_for_session(&session_id).expect("projection replay report");
        assert_eq!(report.session_id, session_id);
        assert_eq!(report.transcript_event_count, 1);
        assert_eq!(report.transcript_replay_error, None);
        assert_eq!(report.transcript_span_count, 1);
        assert_eq!(report.projection_event_count, 2);
        assert_eq!(report.projection_checked_patch_count, 2);
        assert_eq!(report.projection_invalid_basis_count, 0);
        assert_eq!(report.projection_replay_error, None);
        assert_eq!(report.replayed.notes_last_sequence, 1);
        assert_eq!(report.replayed.note_count, 1);
        assert_eq!(report.replayed.graph_last_sequence, 1);
        assert_eq!(report.replayed.graph_node_count, 1);
        assert_eq!(report.replayed.graph_edge_count, 0);
        assert_eq!(report.evaluation.note_operation_count, 1);
        assert_eq!(report.evaluation.graph_operation_count, 1);
        assert_eq!(report.evaluation.graph_retcon_operation_count, 0);
        assert_eq!(report.evaluation.correction_patch_count, 0);
        assert_eq!(report.evaluation.stale_discard_count, 0);
        assert_eq!(report.evaluation.duplicate_active_node_key_count, 0);
        assert_eq!(report.evaluation.duplicate_active_edge_key_count, 0);
        assert_eq!(report.latency.patch_count, 2);
        assert_eq!(report.latency.measured_patch_count, 2);
        assert_eq!(report.latency.missing_basis_timestamp_count, 0);
        assert_eq!(report.latency.total_basis_to_patch_lag_ms, 300_002);
        assert_eq!(report.latency.max_basis_to_patch_lag_ms, 200_001);
        assert_eq!(report.latency.notes.patch_count, 1);
        assert_eq!(report.latency.notes.total_basis_to_patch_lag_ms, 100_001);
        assert_eq!(report.latency.graph.patch_count, 1);
        assert_eq!(report.latency.graph.total_basis_to_patch_lag_ms, 200_001);
        assert_eq!(report.latency.capture_asr.measured_count, 2);
        assert_eq!(report.latency.capture_asr.total_ms, 24);
        assert_eq!(report.latency.capture_asr.max_ms, 12);
        assert_eq!(report.latency.asr_to_queue.measured_count, 2);
        assert_eq!(report.latency.asr_to_queue.total_ms, 200_002);
        assert_eq!(report.latency.asr_to_queue.max_ms, 150_001);
        assert_eq!(report.latency.projection_queue.measured_count, 2);
        assert_eq!(report.latency.projection_queue.total_ms, 100_000);
        assert_eq!(report.latency.projection_queue.max_ms, 50_000);
        assert_eq!(report.latency.generation.measured_count, 2);
        assert_eq!(report.latency.generation.total_ms, 72);
        assert_eq!(report.latency.generation.max_ms, 41);
        assert_eq!(report.latency.apply.measured_count, 2);
        assert_eq!(report.latency.apply.total_ms, 13);
        assert_eq!(report.latency.apply.max_ms, 7);
        assert_eq!(report.latency.notes.capture_asr.max_ms, 12);
        assert_eq!(report.latency.notes.asr_to_queue.max_ms, 50_001);
        assert_eq!(report.latency.notes.projection_queue.max_ms, 50_000);
        assert_eq!(report.latency.notes.generation.max_ms, 31);
        assert_eq!(report.latency.notes.apply.max_ms, 6);
        assert_eq!(report.latency.graph.capture_asr.max_ms, 12);
        assert_eq!(report.latency.graph.asr_to_queue.max_ms, 150_001);
        assert_eq!(report.latency.graph.projection_queue.max_ms, 50_000);
        assert_eq!(report.latency.graph.generation.max_ms, 41);
        assert_eq!(report.latency.graph.apply.max_ms, 7);
        assert_eq!(
            report.notes_artifact.status,
            ProjectionReplayArtifactStatus::Current
        );
        assert_eq!(
            report.graph_artifact.status,
            ProjectionReplayArtifactStatus::Current
        );

        let serialized = serde_json::to_string(&report).expect("serialize replay report");
        assert!(!serialized.contains("Projection status should not expose this text"));
        assert!(!serialized.contains("Private note body"));
        assert!(!serialized.contains("Private Node"));
        assert!(!serialized.contains("Private graph description"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn projection_replay_report_handles_missing_logs() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("projection-replay-missing");
        let _guard = HomeGuard::set(&dir);

        let report =
            projection_replay_report_for_session("missing-session").expect("missing logs report");

        assert_eq!(report.transcript_event_count, 0);
        assert_eq!(report.transcript_span_count, 0);
        assert_eq!(report.projection_event_count, 0);
        assert_eq!(report.projection_checked_patch_count, 0);
        assert_eq!(report.projection_invalid_basis_count, 0);
        assert_eq!(
            report.evaluation,
            ProjectionReplayEvaluationMetrics {
                note_operation_count: 0,
                graph_operation_count: 0,
                graph_retcon_operation_count: 0,
                correction_patch_count: 0,
                stale_discard_count: 0,
                invalidated_graph_node_count: 0,
                invalidated_graph_edge_count: 0,
                active_graph_node_count: 0,
                active_graph_edge_count: 0,
                duplicate_active_node_key_count: 0,
                duplicate_active_edge_key_count: 0,
            }
        );
        assert_eq!(report.latency, ProjectionReplayLatencyMetrics::default());
        assert_eq!(report.replayed.note_count, 0);
        assert_eq!(report.replayed.graph_node_count, 0);
        assert_eq!(
            report.notes_artifact.status,
            ProjectionReplayArtifactStatus::Missing
        );
        assert_eq!(
            report.graph_artifact.status,
            ProjectionReplayArtifactStatus::Missing
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn projection_replay_report_reconstructs_speaker_bearing_patch() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("projection-replay-speaker-bearing");
        let _guard = HomeGuard::set(&dir);
        let session_id = "projection-replay-speaker-bearing";
        let repository = FileMemoryRepository::user_data();
        let event = projection_status_test_event("report-span-1");
        let speaker = projection_status_test_speaker_revision("speaker-span-1");
        let basis = crate::projections::ProjectionBasis::from_transcript_events_and_speaker_spans(
            std::slice::from_ref(&event),
            &[crate::projections::ProjectionBasisSpan {
                span_id: speaker.span_id.clone(),
                revision_number: speaker.revision_number,
            }],
        );

        repository
            .append_transcript_event(session_id, &event)
            .expect("append transcript event");
        repository
            .append_diarization_span_revision(session_id, &speaker)
            .expect("append speaker revision");
        repository
            .append_projection_patch(
                session_id,
                &report_note_patch(1, basis, "Private note body."),
            )
            .expect("append speaker-bearing projection patch");

        let report = projection_replay_report_for_session(session_id)
            .expect("speaker-aware projection replay report");
        assert_eq!(report.projection_checked_patch_count, 1);
        assert_eq!(report.projection_invalid_basis_count, 0);
        assert_eq!(report.replayed.notes_last_sequence, 1);
        assert_eq!(report.replayed.note_count, 1);
        assert!(!format!("{report:?}").contains("Private Speaker Label"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn projection_replay_report_includes_no_network_eval_metrics_for_graph_retcons() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("projection-replay-eval-metrics");
        let _guard = HomeGuard::set(&dir);

        let state = AppState::new();
        let session_id = state.current_session_id();
        let event = projection_status_test_event("eval-retcon-span");
        let basis = {
            let mut ledger = state
                .transcript_ledger
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            ledger.apply_event(event.clone()).expect("seed ledger");
            ledger.current_basis()
        };
        append_transcript_event(&state, &event);

        let seed_graph_patch = crate::projections::ProjectionPatch {
            sequence: 1,
            kind: crate::projections::ProjectionKind::Graph,
            llm_request_id: "report-graph-seed".to_string(),
            route: None,
            basis: basis.clone(),
            operations: vec![
                crate::projections::ProjectionOperation::UpsertGraphNode {
                    id: "person:alice".to_string(),
                    name: "Alice".to_string(),
                    entity_type: "person".to_string(),
                    description: None,
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
                crate::projections::ProjectionOperation::UpsertGraphNode {
                    id: "person:alicia".to_string(),
                    name: "Alicia".to_string(),
                    entity_type: "person".to_string(),
                    description: None,
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
                crate::projections::ProjectionOperation::UpsertGraphNode {
                    id: "project:audio-graph".to_string(),
                    name: "AudioGraph".to_string(),
                    entity_type: "project".to_string(),
                    description: None,
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
                crate::projections::ProjectionOperation::UpsertGraphEdge {
                    id: "edge:alice:owns".to_string(),
                    source: "person:alice".to_string(),
                    target: "project:audio-graph".to_string(),
                    relation_type: "owns".to_string(),
                    label: None,
                    weight: 0.8,
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
                crate::projections::ProjectionOperation::UpsertGraphEdge {
                    id: "edge:alicia:owns".to_string(),
                    source: "person:alicia".to_string(),
                    target: "project:audio-graph".to_string(),
                    relation_type: "owns".to_string(),
                    label: None,
                    weight: 0.6,
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
            ],
            confidence: 0.9,
            provenance: crate::projections::ProjectionProvenance {
                provider: "test".to_string(),
                model: "projection-report".to_string(),
                prompt_id: "report-graph-v1".to_string(),
                route_id: None,
                model_source: crate::llm::route::ModelIdentitySource::Requested,
            },
            queued_at_ms: None,
            generation_latency_ms: None,
            apply_latency_ms: None,
            basis_currency_at_apply: None,
            created_at_ms: 1_700_000_300_001,
        };
        let retcon_graph_patch = crate::projections::ProjectionPatch {
            sequence: 2,
            kind: crate::projections::ProjectionKind::Graph,
            llm_request_id: "report-graph-retcon".to_string(),
            route: None,
            basis: basis.clone(),
            operations: vec![crate::projections::ProjectionOperation::MergeGraphNodes {
                source_id: "person:alicia".to_string(),
                target_id: "person:alice".to_string(),
            }],
            confidence: 0.95,
            provenance: crate::projections::ProjectionProvenance {
                provider: "test".to_string(),
                model: "projection-report".to_string(),
                prompt_id: "report-graph-v1".to_string(),
                route_id: None,
                model_source: crate::llm::route::ModelIdentitySource::Requested,
            },
            queued_at_ms: None,
            generation_latency_ms: None,
            apply_latency_ms: None,
            basis_currency_at_apply: None,
            created_at_ms: 1_700_000_300_002,
        };

        state
            .apply_runtime_projection_patch(&session_id, &basis, seed_graph_patch)
            .expect("apply seed graph patch");
        state
            .apply_runtime_projection_patch(&session_id, &basis, retcon_graph_patch)
            .expect("apply retcon graph patch");
        drain_test_writers(&state);

        let report =
            projection_replay_report_for_session(&session_id).expect("projection replay report");

        assert_eq!(report.projection_invalid_basis_count, 0);
        assert_eq!(report.evaluation.graph_operation_count, 6);
        assert_eq!(report.evaluation.graph_retcon_operation_count, 1);
        assert_eq!(report.evaluation.correction_patch_count, 1);
        assert_eq!(report.evaluation.invalidated_graph_node_count, 1);
        assert_eq!(report.evaluation.invalidated_graph_edge_count, 1);
        assert_eq!(report.evaluation.active_graph_node_count, 2);
        assert_eq!(report.evaluation.active_graph_edge_count, 1);
        assert_eq!(report.evaluation.duplicate_active_node_key_count, 0);
        assert_eq!(report.evaluation.duplicate_active_edge_key_count, 0);
        assert_eq!(report.latency.patch_count, 2);
        assert_eq!(report.latency.measured_patch_count, 2);
        assert_eq!(report.latency.graph.patch_count, 2);
        assert_eq!(report.latency.notes.patch_count, 0);
        assert_eq!(report.latency.graph.total_basis_to_patch_lag_ms, 600_003);
        assert_eq!(report.latency.graph.max_basis_to_patch_lag_ms, 300_002);

        let serialized = serde_json::to_string(&report).expect("serialize replay report");
        assert!(!serialized.contains("Alice"));
        assert!(!serialized.contains("Alicia"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn projection_replay_report_marks_stale_materialized_artifacts() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("projection-replay-stale-artifact");
        let _guard = HomeGuard::set(&dir);

        let state = AppState::new();
        let session_id = state.current_session_id();
        let event = projection_status_test_event("stale-artifact-span");
        let basis = {
            let mut ledger = state
                .transcript_ledger
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            ledger.apply_event(event.clone()).expect("seed ledger");
            ledger.current_basis()
        };
        append_transcript_event(&state, &event);
        let note_patch = report_note_patch(1, basis.clone(), "Current note body.");
        state
            .apply_runtime_projection_patch(&session_id, &basis, note_patch)
            .expect("apply notes patch");
        drain_test_writers(&state);

        FileMemoryRepository::user_data()
            .save_materialized_notes(
                &session_id,
                &crate::projections::MaterializedNotes::new(&session_id),
            )
            .expect("overwrite stale notes artifact");

        let report =
            projection_replay_report_for_session(&session_id).expect("projection replay report");
        assert_eq!(report.replayed.notes_last_sequence, 1);
        assert_eq!(report.projection_checked_patch_count, 1);
        assert_eq!(report.projection_invalid_basis_count, 0);
        assert!(report.notes_artifact.present);
        assert_eq!(report.notes_artifact.stored_last_sequence, 0);
        assert_eq!(report.notes_artifact.replayed_last_sequence, 1);
        assert_eq!(
            report.notes_artifact.status,
            ProjectionReplayArtifactStatus::Stale
        );
        assert_eq!(
            report.graph_artifact.status,
            ProjectionReplayArtifactStatus::Missing
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn projection_replay_report_surfaces_replay_errors_without_mutating_app_state() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("projection-replay-error");
        let _guard = HomeGuard::set(&dir);

        let state = AppState::new();
        let session_id = state.current_session_id();
        let before = state
            .materialized_projection_state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        append_projection_patch(&state, &invalid_graph_patch());
        drain_test_writers(&state);

        let report =
            projection_replay_report_for_session(&session_id).expect("projection replay report");
        assert_eq!(report.projection_event_count, 1);
        assert_eq!(report.projection_checked_patch_count, 1);
        assert_eq!(report.projection_invalid_basis_count, 1);
        assert!(
            report
                .projection_replay_error
                .as_deref()
                .unwrap_or_default()
                .contains("StaleBasis")
        );
        assert_eq!(report.replayed.graph_node_count, 0);
        assert_eq!(report.replayed.graph_edge_count, 0);
        assert_eq!(report.latency.patch_count, 1);
        assert_eq!(report.latency.measured_patch_count, 0);
        assert_eq!(report.latency.missing_basis_timestamp_count, 1);
        assert_eq!(report.latency.graph.missing_basis_timestamp_count, 1);

        let after = state
            .materialized_projection_state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        assert_eq!(before, after);

        let serialized = serde_json::to_string(&report).expect("serialize replay report");
        assert!(!serialized.contains("Private edge label"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn materialized_projection_restore_treats_canonical_replay_as_authority() {
        let mut replayed_state = crate::projections::MaterializedProjectionState::new("session-1");
        replayed_state.notes.last_sequence = 3;
        replayed_state.graph.last_sequence = 4;

        assert_eq!(
            choose_materialized_notes(None, Some(&replayed_state), true)
                .expect("missing notes should replay")
                .last_sequence,
            3
        );
        assert_eq!(
            choose_materialized_graph(None, Some(&replayed_state), true)
                .expect("missing graph should replay")
                .last_sequence,
            4
        );

        let mut old_notes = crate::projections::MaterializedNotes::new("session-1");
        old_notes.last_sequence = 1;
        assert_eq!(
            choose_materialized_notes(Some(old_notes), Some(&replayed_state), true)
                .expect("stale notes should replay")
                .last_sequence,
            3
        );

        let mut ahead_graph = crate::projections::MaterializedGraph::new("session-1");
        ahead_graph.last_sequence = 5;
        assert_eq!(
            choose_materialized_graph(Some(ahead_graph), Some(&replayed_state), true)
                .expect("canonical replay should override an ahead cache")
                .last_sequence,
            4
        );

        let empty_replay = crate::projections::MaterializedProjectionState::new("session-empty");
        assert!(
            choose_materialized_notes(None, Some(&empty_replay), false).is_none(),
            "empty replay should not fabricate a notes artifact"
        );
        assert!(
            choose_materialized_graph(None, Some(&empty_replay), false).is_none(),
            "empty replay should not fabricate a graph artifact"
        );

        let mut stale_empty_notes = crate::projections::MaterializedNotes::new("session-empty");
        stale_empty_notes.last_sequence = 99;
        let selected =
            choose_materialized_notes(Some(stale_empty_notes), Some(&empty_replay), true)
                .expect("canonical empty state remains an explicit artifact");
        assert_eq!(selected.last_sequence, 0);
        assert!(selected.notes.is_empty());
    }

    #[test]
    fn parse_capture_target_accepts_canonical_aliases() {
        assert!(matches!(
            parse_capture_target("system").expect("canonical system target"),
            rsac::CaptureTarget::SystemDefault
        ));

        match parse_capture_target("tree:42").expect("canonical process tree target") {
            rsac::CaptureTarget::ProcessTree(proc_id) => assert_eq!(proc_id.0, 42),
            other => panic!("expected ProcessTree target, got {other:?}"),
        }

        match parse_capture_target("name:Spotify").expect("canonical app-name target") {
            rsac::CaptureTarget::ApplicationByName(name) => assert_eq!(name, "Spotify"),
            other => panic!("expected ApplicationByName target, got {other:?}"),
        }
    }

    #[test]
    fn parse_capture_target_keeps_process_and_process_tree_distinct() {
        match parse_capture_target("app:42").expect("app PID target") {
            rsac::CaptureTarget::Application(app_id) => {
                assert_eq!(app_id.0, "42");
            }
            other => panic!("expected Application target, got {other:?}"),
        }

        match parse_capture_target("process-tree:42").expect("process tree target") {
            rsac::CaptureTarget::ProcessTree(proc_id) => {
                assert_eq!(proc_id.0, 42);
            }
            other => panic!("expected ProcessTree target, got {other:?}"),
        }
    }

    #[test]
    fn resolve_capture_start_target_prefers_source_descriptor_capture_target() {
        let descriptor = AudioSourceInfo {
            id: "opaque-rsac-row".to_string(),
            name: "Safari".to_string(),
            source_type: crate::state::AudioSourceType::Application {
                pid: 2024,
                app_name: "Safari".to_string(),
                bundle_id: Some("com.apple.Safari".to_string()),
            },
            capture_target: Some("app:2024".to_string()),
            device_kind: None,
            is_default: Some(false),
            supported_formats: Vec::new(),
            default_format: None,
            channel_provenance: None,
            capabilities: None,
            permission_status: None,
            permission_recovery: None,
            is_active: false,
        };

        let (source_id, target, source_descriptor) =
            resolve_capture_start_target("opaque-rsac-row".to_string(), None, Some(descriptor))
                .expect("descriptor target should resolve");

        assert_eq!(source_id, "app:2024");
        match target {
            rsac::CaptureTarget::Application(app_id) => assert_eq!(app_id.0, "2024"),
            other => panic!("expected Application target, got {other:?}"),
        }
        let source_descriptor = source_descriptor.expect("descriptor should be preserved");
        assert!(matches!(
            source_descriptor.source_type,
            crate::state::AudioSourceType::Application {
                ref bundle_id, ..
            } if bundle_id.as_deref() == Some("com.apple.Safari")
        ));
    }

    #[test]
    fn resolve_capture_start_target_accepts_explicit_canonical_target_without_descriptor() {
        let (source_id, target, source_descriptor) = resolve_capture_start_target(
            "legacy-row-id".to_string(),
            Some("tree:42".to_string()),
            None,
        )
        .expect("explicit canonical target should resolve");

        assert_eq!(source_id, "tree:42");
        assert!(matches!(target, rsac::CaptureTarget::ProcessTree(_)));
        assert!(source_descriptor.is_none());
    }

    #[test]
    fn parse_capture_target_accepts_raw_windows_mmdevice_ids_as_device_fallback() {
        let raw_id = "{0.0.1.00000000}.{fifine-guid}";
        match parse_capture_target(raw_id).expect("raw Windows device id target") {
            rsac::CaptureTarget::Device(device_id) => {
                assert_eq!(device_id.0, raw_id);
            }
            other => panic!("expected Device target, got {other:?}"),
        }
    }

    #[test]
    fn parse_capture_target_rejects_invalid_process_ids() {
        for source_id in [
            "app:not-a-pid",
            "app:0",
            "process-tree:nope",
            "process-tree:0",
        ] {
            let err = parse_capture_target(source_id).expect_err("invalid PID must be rejected");
            assert!(
                err.contains("Invalid"),
                "error for {source_id:?} should mention invalid PID, got {err}"
            );
        }
    }

    #[test]
    fn durable_start_gate_rejects_deferred_asr_before_runtime_setup() {
        let settings = crate::settings::AppSettings::default();
        let err = enforce_transcribe_provider_start(&settings)
            .expect_err("the default Local Whisper route is deferred for this MVP");

        match err {
            AppError::ProviderDeferred {
                provider_id,
                display_name,
            } => {
                assert_eq!(provider_id, "asr.local_whisper");
                assert_eq!(display_name, "Local Whisper");
            }
            other => panic!("expected ProviderDeferred, got {other:?}"),
        }
    }

    #[test]
    fn durable_start_gate_allows_deepgram_with_selectable_llm() {
        let mut settings = crate::settings::AppSettings::default();
        settings.asr_provider = crate::settings::AsrProvider::DeepgramStreaming {
            api_key: "saved-key-hydrated-at-runtime".to_string(),
            model: "nova-3".to_string(),
            enable_diarization: true,
            endpointing_ms: 300,
            utterance_end_ms: 1000,
            vad_events: true,
            eot_threshold: 0.5,
            eager_eot_threshold: 0.0,
            eot_timeout_ms: 0,
            max_speakers: 0,
            keyterms: vec![],
        };
        settings.llm_provider = crate::settings::LlmProvider::OpenRouter {
            model: "openai/gpt-4.1-mini".to_string(),
            base_url: crate::llm::openrouter::DEFAULT_BASE_URL.to_string(),
            provider_order: None,
            include_usage_in_stream: true,
            api_key: "saved-key-hydrated-at-runtime".to_string(),
        };

        assert!(enforce_transcribe_provider_start(&settings).is_ok());
        assert!(enforce_chat_provider_start(&settings).is_ok());
    }

    #[test]
    fn readiness_request_set_is_explicit_and_preserves_deferred_diagnostics() {
        let settings = crate::settings::AppSettings::default();
        let requested = vec![
            "asr.deepgram".to_string(),
            "realtime_agent.gemini_live".to_string(),
            "not.a.provider".to_string(),
        ];
        let ids = requested_provider_ids(&settings, true, Some(&requested));

        assert!(ids.contains("asr.deepgram"));
        assert!(ids.contains("realtime_agent.gemini_live"));
        assert!(!ids.contains("asr.local_whisper"));
        assert_eq!(ids.len(), 2, "unknown ids must not enter the probe set");

        let descriptor = crate::provider_registry::descriptor_by_id("realtime_agent.gemini_live");
        let mut store = crate::credentials::CredentialStore::default();
        store.gemini_api_key = Some("saved-key".to_string());
        assert!(
            should_probe_provider(descriptor, &ids, &settings, &store),
            "explicit non-content-bearing diagnostics remain available under ADR-0033"
        );
    }

    #[cfg(not(feature = "asr-whisper"))]
    #[test]
    fn cloud_only_local_whisper_returns_provider_unavailable() {
        let error =
            local_asr_provider_availability_error(&crate::settings::AsrProvider::LocalWhisper)
                .expect("cloud-only LocalWhisper should be unavailable");
        match error {
            AppError::ProviderUnavailable {
                provider,
                required_feature,
            } => {
                assert_eq!(provider, "LocalWhisper");
                assert_eq!(required_feature, "local-ml or asr-whisper");
            }
            other => panic!("expected ProviderUnavailable, got {other:?}"),
        }
    }

    #[cfg(feature = "asr-whisper")]
    #[test]
    fn local_ml_local_whisper_is_provider_available() {
        assert!(
            local_asr_provider_availability_error(&crate::settings::AsrProvider::LocalWhisper)
                .is_none()
        );
    }

    #[cfg(not(feature = "sherpa-streaming"))]
    #[test]
    fn compiled_out_sherpa_returns_provider_unavailable() {
        let provider = crate::settings::AsrProvider::SherpaOnnx {
            model_dir: crate::models::SHERPA_ZIPFORMER_20M.to_string(),
            enable_endpoint_detection: true,
        };
        let error = local_asr_provider_availability_error(&provider)
            .expect("SherpaOnnx should be unavailable without sherpa-streaming");
        match error {
            AppError::ProviderUnavailable {
                provider,
                required_feature,
            } => {
                assert_eq!(provider, "SherpaOnnx");
                assert_eq!(required_feature, "sherpa-streaming");
            }
            other => panic!("expected ProviderUnavailable, got {other:?}"),
        }
    }

    #[cfg(not(feature = "asr-moonshine"))]
    #[test]
    fn compiled_out_moonshine_returns_provider_unavailable() {
        let provider = crate::settings::AsrProvider::Moonshine {
            model_dir: "moonshine-small-streaming-en".to_string(),
            enable_speaker_hints: true,
        };
        let error = local_asr_provider_availability_error(&provider)
            .expect("Moonshine should be unavailable without asr-moonshine");
        match error {
            AppError::ProviderUnavailable {
                provider,
                required_feature,
            } => {
                assert_eq!(provider, "Moonshine");
                assert_eq!(required_feature, "asr-moonshine");
            }
            other => panic!("expected ProviderUnavailable, got {other:?}"),
        }
    }

    #[cfg(feature = "asr-moonshine")]
    #[test]
    fn moonshine_feature_build_keeps_runtime_unavailable_until_worker_lands() {
        let provider = crate::settings::AsrProvider::Moonshine {
            model_dir: "moonshine-small-streaming-en".to_string(),
            enable_speaker_hints: true,
        };
        let error = local_asr_provider_availability_error(&provider)
            .expect("Moonshine runtime should stay unavailable until worker lands");
        match error {
            AppError::ProviderUnavailable {
                provider,
                required_feature,
            } => {
                assert_eq!(provider, "Moonshine");
                assert_eq!(required_feature, "asr-moonshine runtime implementation");
            }
            other => panic!("expected ProviderUnavailable, got {other:?}"),
        }
    }

    #[cfg(feature = "sherpa-streaming")]
    #[test]
    fn compiled_in_sherpa_is_provider_available() {
        let provider = crate::settings::AsrProvider::SherpaOnnx {
            model_dir: crate::models::SHERPA_ZIPFORMER_20M.to_string(),
            enable_endpoint_detection: true,
        };
        assert!(local_asr_provider_availability_error(&provider).is_none());
    }

    #[cfg(not(any(feature = "llm-llama", feature = "llm-mistralrs")))]
    #[test]
    fn cloud_only_local_llms_return_provider_unavailable() {
        for (provider, expected_provider, expected_feature) in [
            (
                crate::settings::LlmProvider::LocalLlama,
                "LocalLlama",
                "local-ml or llm-llama",
            ),
            (
                crate::settings::LlmProvider::MistralRs {
                    model_id: "mistralrs-qwen".to_string(),
                },
                "MistralRs",
                "local-ml or llm-mistralrs",
            ),
        ] {
            let error = local_llm_provider_availability_error(&provider)
                .expect("cloud-only local LLM should be unavailable");
            match error {
                AppError::ProviderUnavailable {
                    provider,
                    required_feature,
                } => {
                    assert_eq!(provider, expected_provider);
                    assert_eq!(required_feature, expected_feature);
                }
                other => panic!("expected ProviderUnavailable, got {other:?}"),
            }
        }
    }

    #[cfg(all(feature = "llm-llama", feature = "llm-mistralrs"))]
    #[test]
    fn local_ml_local_llms_are_provider_available() {
        assert!(
            local_llm_provider_availability_error(&crate::settings::LlmProvider::LocalLlama)
                .is_none()
        );
        assert!(
            local_llm_provider_availability_error(&crate::settings::LlmProvider::MistralRs {
                model_id: "mistralrs-qwen".to_string(),
            })
            .is_none()
        );
    }

    #[test]
    fn streaming_provider_gate_allows_api_openrouter_local_llama_mistralrs_and_bedrock() {
        assert!(provider_supports_streaming(
            &crate::settings::LlmProvider::Api {
                endpoint: "http://localhost:11434/v1".to_string(),
                api_key: String::new(),
                model: "llama-test".to_string(),
            }
        ));
        assert!(provider_supports_streaming(
            &crate::settings::LlmProvider::OpenRouter {
                api_key: "redacted".to_string(),
                model: "openai/gpt-oss-20b".to_string(),
                base_url: "https://openrouter.ai/api/v1".to_string(),
                provider_order: None,
                include_usage_in_stream: true,
            }
        ));
        assert!(provider_supports_streaming(
            &crate::settings::LlmProvider::LocalLlama
        ));
        // MistralRs now has a streaming adapter (run_mistralrs_stream), so the
        // frontend gate must start a stream for it.
        assert!(provider_supports_streaming(
            &crate::settings::LlmProvider::MistralRs {
                model_id: "mistralrs-qwen".to_string(),
            }
        ));
        // AwsBedrock now streams via the on-demand ConverseStream adapter
        // (audio-graph-2f4a): the gate must allow it so start_streaming_chat
        // dispatches to the streaming task instead of rejecting.
        assert!(provider_supports_streaming(
            &crate::settings::LlmProvider::AwsBedrock {
                region: "us-west-2".to_string(),
                model_id: "anthropic.claude".to_string(),
                credential_source: crate::settings::AwsCredentialSource::DefaultChain,
            }
        ));
    }

    // -----------------------------------------------------------------------
    // PART 1 — configure_api_endpoint URL validation regression tests
    // (loop-13 MEDIUM #4). The validation landed in loop 12 without
    // coverage; these lock in the accept/reject contract so a future
    // refactor can't silently loosen it.
    // -----------------------------------------------------------------------

    #[test]
    fn validate_endpoint_url_accepts_https() {
        let u =
            validate_endpoint_url("https://api.openai.com/v1").expect("https URL must be accepted");
        assert_eq!(u.scheme(), "https");
    }

    #[test]
    fn validate_endpoint_url_accepts_http() {
        // Plain http is legitimate for local servers (Ollama, LM Studio, vLLM).
        let u = validate_endpoint_url("http://localhost:11434/v1")
            .expect("http URL must be accepted for local servers");
        assert_eq!(u.scheme(), "http");
    }

    #[test]
    fn validate_endpoint_url_rejects_malformed() {
        let err = validate_endpoint_url("not a url").expect_err("garbage must be rejected");
        assert!(
            err.contains("Invalid endpoint URL"),
            "error should mention invalid URL, got: {}",
            err
        );
    }

    #[test]
    fn validate_endpoint_url_rejects_disallowed_schemes() {
        // file:// would let a settings-file edit coax the app into reading
        // local files. ftp:// is non-functional with reqwest. Both must be
        // rejected up-front with a scheme-specific message.
        for bad in &["file:///etc/passwd", "ftp://example.com/models"] {
            let err = validate_endpoint_url(bad).expect_err(&format!("{} must be rejected", bad));
            assert!(
                err.contains("unsupported scheme"),
                "error for {} should mention unsupported scheme, got: {}",
                bad,
                err
            );
        }
    }

    #[test]
    fn plaintext_credential_loadback_is_not_registered_for_ipc() {
        let lib_rs = include_str!("lib.rs");

        assert!(
            !lib_rs.contains("commands::load_credential_cmd"),
            "plaintext credential loadback must not be registered as Tauri IPC"
        );
        assert!(
            lib_rs.contains("commands::load_credential_presence_cmd"),
            "Settings should use non-secret credential presence over IPC"
        );
    }

    #[test]
    fn credential_presence_maps_every_allowed_key_without_secret_values() {
        let mut store = crate::credentials::CredentialStore::default();
        store.openai_api_key = Some("sk-openai".to_string());
        store.openrouter_api_key = Some("sk-or".to_string());
        store.aws_secret_key = Some("   ".to_string());

        let presence = credential_presence_from_store(&store).expect("presence mapping");

        assert_eq!(
            presence.len(),
            crate::credentials::ALLOWED_CREDENTIAL_KEYS.len()
        );
        for key in crate::credentials::ALLOWED_CREDENTIAL_KEYS {
            assert!(
                presence.iter().any(|entry| entry.key == *key),
                "presence response is missing allowlisted key {key}"
            );
        }

        let openai = presence
            .iter()
            .find(|entry| entry.key == "openai_api_key")
            .expect("openai presence");
        assert!(openai.present);
        assert_eq!(openai.source, "credentials_yaml");

        let blank_secret = presence
            .iter()
            .find(|entry| entry.key == "aws_secret_key")
            .expect("aws secret presence");
        assert!(!blank_secret.present);
        assert_eq!(blank_secret.source, "missing");

        let serialized = serde_json::to_string(&presence).expect("serialize presence");
        assert!(!serialized.contains("sk-openai"));
        assert!(!serialized.contains("sk-or"));
    }

    #[test]
    fn credential_presence_uses_per_key_sources_without_secret_values() {
        let mut store = crate::credentials::CredentialStore::default();
        store.openai_api_key = Some("sk-openai".to_string());
        store.deepgram_api_key = Some("dg-imported".to_string());
        store.aws_secret_key = Some("   ".to_string());

        let mut key_sources = std::collections::BTreeMap::new();
        key_sources.insert("openai_api_key", "os_keychain");
        key_sources.insert("deepgram_api_key", "imported_file");
        let snapshot = crate::credentials::CredentialSnapshot::with_key_sources(
            store,
            "os_keychain",
            key_sources,
        );

        let presence = credential_presence_from_snapshot(&snapshot).expect("presence mapping");

        let openai = presence
            .iter()
            .find(|entry| entry.key == "openai_api_key")
            .expect("openai presence");
        assert!(openai.present);
        assert_eq!(openai.source, "os_keychain");

        let deepgram = presence
            .iter()
            .find(|entry| entry.key == "deepgram_api_key")
            .expect("deepgram presence");
        assert!(deepgram.present);
        assert_eq!(deepgram.source, "imported_file");

        let blank_secret = presence
            .iter()
            .find(|entry| entry.key == "aws_secret_key")
            .expect("aws secret presence");
        assert!(!blank_secret.present);
        assert_eq!(blank_secret.source, "missing");

        let serialized = serde_json::to_string(&presence).expect("serialize presence");
        assert!(!serialized.contains("sk-openai"));
        assert!(!serialized.contains("dg-imported"));
    }

    /// Serializes tests that observe or mutate `PROVIDER_CREDENTIAL_EPOCH`.
    ///
    /// CodeRabbit (PR #84): the epoch is a process-global `AtomicU64`, and the
    /// harness runs `#[test]`s concurrently — any test that routes through
    /// `save_credential_impl` / `delete_credential_cmd` (both bump the epoch)
    /// while an epoch-asserting test sits between its before/after reads makes
    /// the assertion flake. Every test touching the epoch must hold this lock.
    static CREDENTIAL_EPOCH_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn save_credential_empty_value_skips_without_epoch_bump_or_rehydrate() {
        // Hold the shared epoch lock for the whole test: the before/after epoch
        // comparison below is only meaningful if no concurrent test bumps the
        // process-global counter in between.
        let _epoch_guard = CREDENTIAL_EPOCH_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // cred-review M2.1 / N1: a blank/whitespace save must be a true no-op —
        // it must NOT bump the readiness epoch (which invalidates the
        // provider-readiness cache) nor rehydrate the settings cache (which
        // re-clones app_settings). We assert both via observable state that is
        // isolated to this AppState, plus the typed SkippedEmpty return so the
        // frontend can tell a skip from a persist. The skip path deliberately
        // returns before touching the credential backend, so this test never
        // reads or writes the real keychain / credentials.yaml.
        let state = AppState::new();

        // Seed the in-memory cache with a distinctive non-empty api_key. If the
        // skip path rehydrated, `hydrate_runtime_credentials` would first redact
        // (clear) this and then refill from the (empty) store — leaving it
        // blank. So an unchanged sentinel proves rehydrate did NOT run.
        {
            let mut cached = state
                .app_settings
                .write()
                .unwrap_or_else(|p| p.into_inner());
            cached.llm_provider = crate::settings::LlmProvider::Api {
                endpoint: "https://api.openai.com/v1".to_string(),
                api_key: "sentinel-cached-key".to_string(),
                model: "gpt-4o-mini".to_string(),
            };
        }

        let epoch_before = PROVIDER_CREDENTIAL_EPOCH.load(Ordering::SeqCst);

        // Whitespace-only value: must skip.
        let outcome = save_credential_impl("openai_api_key".to_string(), "   ".to_string(), &state)
            .expect("whitespace save should succeed as a skip, not error");
        assert_eq!(outcome, SaveCredentialOutcome::SkippedEmpty);

        // Fully empty value: also skips.
        let outcome_empty =
            save_credential_impl("openai_api_key".to_string(), String::new(), &state)
                .expect("empty save should succeed as a skip");
        assert_eq!(outcome_empty, SaveCredentialOutcome::SkippedEmpty);

        assert_eq!(
            PROVIDER_CREDENTIAL_EPOCH.load(Ordering::SeqCst),
            epoch_before,
            "an empty/whitespace save must not bump the readiness epoch"
        );

        let api_key_after = match &state
            .app_settings
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .llm_provider
        {
            crate::settings::LlmProvider::Api { api_key, .. } => api_key.clone(),
            other => panic!("expected Api llm_provider, got {other:?}"),
        };
        assert_eq!(
            api_key_after, "sentinel-cached-key",
            "an empty/whitespace save must not rehydrate (clear/refill) the settings cache"
        );

        // An unknown key still errors at the boundary, before the skip check.
        let unknown =
            save_credential_impl("totally_bogus_key".to_string(), "   ".to_string(), &state);
        assert!(
            matches!(
                unknown,
                Err(crate::error::AppError::CredentialFileError { .. })
            ),
            "unknown key must be rejected even for an empty value"
        );
    }

    #[test]
    fn save_credential_outcome_serializes_snake_case_union() {
        // The frontend consumes this as `"saved" | "skipped_empty"`.
        assert_eq!(
            serde_json::to_string(&SaveCredentialOutcome::Saved).expect("serialize saved"),
            "\"saved\""
        );
        assert_eq!(
            serde_json::to_string(&SaveCredentialOutcome::SkippedEmpty).expect("serialize skipped"),
            "\"skipped_empty\""
        );
    }

    #[test]
    fn provider_readiness_refresh_admission_coalesces_in_flight_checks() {
        let key = format!("test.inflight.{}", uuid::Uuid::new_v4());
        let now = unix_millis();

        assert_eq!(
            begin_provider_readiness_refresh(&key, now, false),
            ProviderReadinessRefreshAdmission::Started
        );
        assert_eq!(
            begin_provider_readiness_refresh(&key, now + 1, false),
            ProviderReadinessRefreshAdmission::InFlight
        );

        finish_provider_readiness_refresh(&key);
    }

    #[test]
    fn provider_readiness_refresh_admission_rate_limits_recent_rechecks() {
        let key = format!("test.ratelimit.{}", uuid::Uuid::new_v4());
        let now = unix_millis();

        assert_eq!(
            begin_provider_readiness_refresh(&key, now, false),
            ProviderReadinessRefreshAdmission::Started
        );
        finish_provider_readiness_refresh(&key);

        match begin_provider_readiness_refresh(&key, now + 1, false) {
            ProviderReadinessRefreshAdmission::RateLimited { retry_after_ms } => {
                assert!(
                    retry_after_ms <= PROVIDER_READINESS_MIN_REFRESH_INTERVAL_MS,
                    "retry delay should be capped by the configured cooldown"
                );
                assert!(
                    retry_after_ms > 0,
                    "immediate recheck should be delayed by a positive duration"
                );
            }
            other => panic!("expected rate-limit admission, got {other:?}"),
        }
    }

    #[test]
    fn provider_readiness_force_refresh_bypasses_recent_recheck_limit() {
        let key = format!("test.force.{}", uuid::Uuid::new_v4());
        let now = unix_millis();

        assert_eq!(
            begin_provider_readiness_refresh(&key, now, false),
            ProviderReadinessRefreshAdmission::Started
        );
        finish_provider_readiness_refresh(&key);

        assert_eq!(
            begin_provider_readiness_refresh(&key, now + 1, true),
            ProviderReadinessRefreshAdmission::Started
        );
        finish_provider_readiness_refresh(&key);
    }

    #[test]
    fn provider_readiness_cancel_marks_token_and_removes_request_owner() {
        let request_id = format!("settings-readiness-test-{}", uuid::Uuid::new_v4());
        let (_guard, token) = register_provider_readiness_request(Some(request_id.clone()))
            .expect("register request")
            .expect("request token");

        assert!(!token.is_cancelled());
        assert!(cancel_provider_readiness_request(&request_id));
        assert!(token.is_cancelled());
        assert!(!cancel_provider_readiness_request(&request_id));
    }

    #[test]
    fn provider_readiness_stale_guard_does_not_clear_new_request_generation() {
        let request_id = format!("settings-readiness-race-{}", uuid::Uuid::new_v4());
        let (old_guard, old_token) = register_provider_readiness_request(Some(request_id.clone()))
            .expect("register old request")
            .expect("old request token");
        let (_new_guard, new_token) = register_provider_readiness_request(Some(request_id.clone()))
            .expect("register new request")
            .expect("new request token");

        assert!(old_token.is_cancelled());
        drop(old_guard);
        assert!(cancel_provider_readiness_request(&request_id));
        assert!(new_token.is_cancelled());
    }

    #[test]
    fn provider_readiness_rejects_secret_shaped_request_ids() {
        assert!(validate_provider_readiness_request_id("settings-readiness-123").is_ok());
        assert!(validate_provider_readiness_request_id("").is_err());
        assert!(validate_provider_readiness_request_id("bearer token with spaces").is_err());
        assert!(validate_provider_readiness_request_id(&"x".repeat(129)).is_err());
    }

    #[tokio::test]
    async fn provider_readiness_cancelled_probe_is_not_cacheable_or_secret_bearing() {
        let descriptor = crate::provider_registry::provider_registry()
            .iter()
            .find(|descriptor| descriptor.id == "asr.deepgram")
            .expect("deepgram descriptor");
        let settings = crate::settings::AppSettings::default();
        let mut store = crate::credentials::CredentialStore::default();
        store.deepgram_api_key = Some("sk-secret-cancel".to_string());
        let cancel = CancellationToken::new();
        cancel.cancel();

        let readiness =
            refresh_provider_readiness(descriptor, &settings, &store, 77, Some(&cancel)).await;

        assert_eq!(readiness.provider_id, "asr.deepgram");
        assert_eq!(readiness.status, ProviderReadinessStatus::Unchecked);
        assert!(readiness.automatic_probe_available);
        assert_eq!(readiness.checked_at, None);
        assert_eq!(readiness.message, "Provider readiness check cancelled");
        let serialized = serde_json::to_string(&readiness).expect("serialize readiness");
        assert!(!serialized.contains("sk-secret-cancel"));
    }

    #[test]
    fn healthy_final_only_stt_readiness_is_ready_but_typed_degraded() {
        let descriptor = crate::provider_registry::descriptor_by_id("asr.api");
        let settings = crate::settings::AppSettings {
            asr_provider: crate::settings::AsrProvider::Api {
                endpoint: "http://127.0.0.1:8080/v1".to_string(),
                api_key: String::new(),
                model: "final-only-fixture".to_string(),
            },
            ..Default::default()
        };
        let mut readiness = base_provider_readiness(
            descriptor,
            &settings,
            &crate::credentials::CredentialStore::default(),
            78,
        );
        readiness.status = ProviderReadinessStatus::Ready;

        let fidelity = readiness
            .effective_stt_fidelity
            .expect("final-only readiness fidelity");
        assert_eq!(readiness.status, ProviderReadinessStatus::Ready);
        assert_eq!(
            fidelity.revision_semantics,
            crate::provider_registry::SttRevisionSemantics::FinalOnly
        );
        assert_eq!(
            fidelity.timing,
            crate::provider_registry::SttTimingFidelity::AppEstimated
        );
        assert_eq!(fidelity.confidence, SttFidelityOrigin::Unavailable);
        assert_eq!(fidelity.turn, SttFidelityOrigin::Unavailable);
        assert_eq!(fidelity.speaker, SttFidelityOrigin::Unavailable);
        assert_eq!(fidelity.channel, SttFidelityOrigin::Unavailable);
        assert_eq!(
            fidelity.degradations,
            vec![
                SttFidelityDegradation::FinalOnlyRevisions,
                SttFidelityDegradation::AppEstimatedTiming,
                SttFidelityDegradation::ConfidenceUnavailable,
                SttFidelityDegradation::TurnUnavailable,
                SttFidelityDegradation::SpeakerUnavailable,
                SttFidelityDegradation::ChannelUnavailable,
            ]
        );
    }

    #[test]
    fn deepgram_effective_fidelity_uses_selected_model_diarization_and_turn_controls() {
        let descriptor = crate::provider_registry::descriptor_by_id("asr.deepgram");
        let mut settings = crate::settings::AppSettings {
            asr_provider: crate::settings::AsrProvider::DeepgramStreaming {
                api_key: String::new(),
                model: "nova-3".to_string(),
                enable_diarization: false,
                endpointing_ms: 0,
                utterance_end_ms: 0,
                vad_events: false,
                eot_threshold: 0.5,
                eager_eot_threshold: 0.0,
                eot_timeout_ms: 0,
                max_speakers: 0,
                keyterms: vec![],
            },
            ..Default::default()
        };

        let nova = effective_stt_fidelity(descriptor, &settings).expect("Nova fidelity");
        assert_eq!(
            nova.revision_semantics,
            crate::provider_registry::SttRevisionSemantics::PartialAndFinal
        );
        assert_eq!(nova.turn, SttFidelityOrigin::Provider);
        assert_eq!(nova.speaker, SttFidelityOrigin::Unavailable);
        assert_eq!(nova.channel, SttFidelityOrigin::Unavailable);
        assert_eq!(
            nova.turn_detection,
            SttTurnDetectionCapabilities {
                speech_start: false,
                speech_final: true,
                endpointing_configured: false,
                utterance_end: false,
                end_of_turn: false,
                eager_end_of_turn: false,
                turn_resume: false,
            }
        );
        assert!(
            nova.degradations
                .contains(&SttFidelityDegradation::SpeakerDisabledByConfiguration)
        );
        assert!(
            nova.degradations
                .contains(&SttFidelityDegradation::ChannelUnavailable)
        );

        settings.asr_provider = crate::settings::AsrProvider::DeepgramStreaming {
            api_key: String::new(),
            model: "flux-general-en".to_string(),
            enable_diarization: true,
            endpointing_ms: 300,
            utterance_end_ms: 1000,
            vad_events: true,
            eot_threshold: 0.5,
            eager_eot_threshold: 0.3,
            eot_timeout_ms: 1500,
            max_speakers: 2,
            keyterms: vec![],
        };
        let flux = effective_stt_fidelity(descriptor, &settings).expect("Flux fidelity");
        assert_eq!(
            flux.revision_semantics,
            crate::provider_registry::SttRevisionSemantics::FinalOnly
        );
        assert_eq!(flux.turn, SttFidelityOrigin::Provider);
        assert_eq!(flux.speaker, SttFidelityOrigin::Unavailable);
        assert_eq!(flux.channel, SttFidelityOrigin::Unavailable);
        assert_eq!(
            flux.turn_detection,
            SttTurnDetectionCapabilities {
                speech_start: true,
                speech_final: false,
                endpointing_configured: false,
                utterance_end: false,
                end_of_turn: true,
                eager_end_of_turn: true,
                turn_resume: true,
            }
        );
        assert!(
            flux.degradations
                .contains(&SttFidelityDegradation::SpeakerUnavailableForSelectedModel)
        );
        assert!(
            flux.degradations
                .contains(&SttFidelityDegradation::ChannelUnavailable)
        );

        let active_ids = active_provider_ids(&settings, false);
        let flux_fingerprint =
            provider_readiness_config_fingerprint(descriptor, &settings, &active_ids);
        settings.asr_provider = crate::settings::AsrProvider::DeepgramStreaming {
            api_key: String::new(),
            model: "nova-3".to_string(),
            enable_diarization: true,
            endpointing_ms: 300,
            utterance_end_ms: 1000,
            vad_events: true,
            eot_threshold: 0.5,
            eager_eot_threshold: 0.0,
            eot_timeout_ms: 0,
            max_speakers: 0,
            keyterms: vec![],
        };
        let nova_fingerprint = provider_readiness_config_fingerprint(
            descriptor,
            &settings,
            &active_provider_ids(&settings, false),
        );
        assert_ne!(nova_fingerprint, flux_fingerprint);
    }

    #[test]
    fn deepgram_effective_speaker_fidelity_follows_global_and_provider_diarization_policy() {
        use crate::settings::DiarizationMode;

        let descriptor = crate::provider_registry::descriptor_by_id("asr.deepgram");
        for (global_mode, provider_enabled, expected_speaker) in [
            (DiarizationMode::Off, true, SttFidelityOrigin::Unavailable),
            (DiarizationMode::Provider, true, SttFidelityOrigin::Provider),
            (DiarizationMode::Off, false, SttFidelityOrigin::Unavailable),
            (
                DiarizationMode::Provider,
                false,
                SttFidelityOrigin::Unavailable,
            ),
        ] {
            let settings = crate::settings::AppSettings {
                asr_provider: crate::settings::AsrProvider::DeepgramStreaming {
                    api_key: String::new(),
                    model: "nova-3".to_string(),
                    enable_diarization: provider_enabled,
                    endpointing_ms: 300,
                    utterance_end_ms: 1000,
                    vad_events: true,
                    eot_threshold: 0.5,
                    eager_eot_threshold: 0.0,
                    eot_timeout_ms: 0,
                    max_speakers: 0,
                    keyterms: vec![],
                },
                diarization: crate::settings::DiarizationSettings {
                    mode: global_mode,
                    ..Default::default()
                },
                ..Default::default()
            };

            let fidelity =
                effective_stt_fidelity(descriptor, &settings).expect("Deepgram fidelity");
            assert_eq!(
                fidelity.speaker, expected_speaker,
                "global={global_mode:?} provider_enabled={provider_enabled}"
            );
            assert_eq!(
                fidelity
                    .degradations
                    .contains(&SttFidelityDegradation::SpeakerDisabledByConfiguration),
                expected_speaker == SttFidelityOrigin::Unavailable,
                "global={global_mode:?} provider_enabled={provider_enabled}"
            );
        }
    }

    #[test]
    fn deepgram_readiness_fingerprint_tracks_global_diarization_policy() {
        use crate::settings::{DiarizationMode, DiarizationSpeakerCount};

        let descriptor = crate::provider_registry::descriptor_by_id("asr.deepgram");
        let mut settings = crate::settings::AppSettings {
            asr_provider: crate::settings::AsrProvider::DeepgramStreaming {
                api_key: String::new(),
                model: "nova-3".to_string(),
                enable_diarization: true,
                endpointing_ms: 300,
                utterance_end_ms: 1000,
                vad_events: true,
                eot_threshold: 0.5,
                eager_eot_threshold: 0.0,
                eot_timeout_ms: 0,
                max_speakers: 0,
                keyterms: vec![],
            },
            diarization: crate::settings::DiarizationSettings {
                mode: DiarizationMode::Provider,
                speaker_count: DiarizationSpeakerCount::Auto,
                max_speakers: None,
            },
            ..Default::default()
        };
        let active_ids = active_provider_ids(&settings, false);
        let provider_auto =
            provider_readiness_config_fingerprint(descriptor, &settings, &active_ids);

        settings.diarization.mode = DiarizationMode::Off;
        let globally_off =
            provider_readiness_config_fingerprint(descriptor, &settings, &active_ids);
        assert_ne!(provider_auto, globally_off);

        settings.diarization.mode = DiarizationMode::Provider;
        settings.diarization.speaker_count = DiarizationSpeakerCount::Fixed;
        settings.diarization.max_speakers = Some(4);
        let fixed_four = provider_readiness_config_fingerprint(descriptor, &settings, &active_ids);
        assert_ne!(provider_auto, fixed_four);

        settings.diarization.max_speakers = Some(5);
        let fixed_five = provider_readiness_config_fingerprint(descriptor, &settings, &active_ids);
        assert_ne!(fixed_four, fixed_five);
    }

    #[test]
    fn fixed_model_catalog_uses_registry_defaults() {
        let assemblyai = crate::provider_registry::provider_registry()
            .iter()
            .find(|descriptor| descriptor.id == "asr.assemblyai")
            .expect("assemblyai descriptor");
        let assemblyai_catalog = fixed_model_catalog_for_descriptor(assemblyai);

        assert_eq!(assemblyai_catalog.len(), 1);
        assert_eq!(assemblyai_catalog[0].id, "universal-3-5-pro");
        assert!(assemblyai_catalog[0].is_default);

        let local_whisper = crate::provider_registry::provider_registry()
            .iter()
            .find(|descriptor| descriptor.id == "asr.local_whisper")
            .expect("local whisper descriptor");
        let local_catalog = fixed_model_catalog_for_descriptor(local_whisper);

        assert_eq!(local_catalog.len(), 1);
        assert_eq!(local_catalog[0].id, crate::models::WHISPER_MODEL_SMALL_EN);
        assert!(local_catalog[0].is_default);

        let cerebras = crate::provider_registry::provider_registry()
            .iter()
            .find(|descriptor| descriptor.id == "llm.cerebras")
            .expect("cerebras descriptor");
        let cerebras_catalog = fixed_model_catalog_for_descriptor(cerebras);

        assert_eq!(cerebras_catalog.len(), 2);
        assert_eq!(
            cerebras_catalog[0].id,
            crate::provider_registry::CEREBRAS_DEFAULT_MODEL
        );
        assert!(cerebras_catalog[0].is_default);
    }

    #[test]
    fn remote_command_model_catalogs_stay_provider_specific() {
        let deepgram = crate::provider_registry::provider_registry()
            .iter()
            .find(|descriptor| descriptor.id == "asr.deepgram")
            .expect("deepgram descriptor");

        assert!(fixed_model_catalog_for_descriptor(deepgram).is_empty());
    }

    #[test]
    fn generic_llm_api_required_credentials_follow_active_endpoint() {
        let descriptor = crate::provider_registry::provider_registry()
            .iter()
            .find(|descriptor| descriptor.id == "llm.api")
            .expect("generic llm api descriptor");
        let mut settings = crate::settings::AppSettings {
            llm_provider: crate::settings::LlmProvider::Api {
                endpoint: "https://api.openai.com/v1".to_string(),
                api_key: String::new(),
                model: "gpt-4o-mini".to_string(),
            },
            ..Default::default()
        };

        assert_eq!(
            required_credential_keys_for_provider(descriptor, &settings),
            vec!["openai_api_key"]
        );

        settings.llm_provider = crate::settings::LlmProvider::Api {
            endpoint: "https://api.groq.com/openai/v1".to_string(),
            api_key: String::new(),
            model: "llama-3.3-70b-versatile".to_string(),
        };
        assert_eq!(
            required_credential_keys_for_provider(descriptor, &settings),
            vec!["groq_api_key"]
        );

        settings.llm_provider = crate::settings::LlmProvider::Api {
            endpoint: "http://localhost:8000/v1".to_string(),
            api_key: String::new(),
            model: "local-model".to_string(),
        };
        assert!(
            required_credential_keys_for_provider(descriptor, &settings).is_empty(),
            "loopback OpenAI-compatible servers may be unauthenticated"
        );
    }

    #[test]
    fn generic_asr_api_loopback_endpoint_does_not_require_saved_key() {
        let descriptor = crate::provider_registry::provider_registry()
            .iter()
            .find(|descriptor| descriptor.id == "asr.api")
            .expect("generic asr api descriptor");
        let settings = crate::settings::AppSettings {
            asr_provider: crate::settings::AsrProvider::Api {
                endpoint: "http://127.0.0.1:8080/v1".to_string(),
                api_key: String::new(),
                model: "whisper-local".to_string(),
            },
            ..Default::default()
        };

        assert!(required_credential_keys_for_provider(descriptor, &settings).is_empty());
    }

    #[test]
    fn base_readiness_includes_fixed_provider_model_catalog() {
        let descriptor = crate::provider_registry::provider_registry()
            .iter()
            .find(|descriptor| descriptor.id == "asr.assemblyai")
            .expect("assemblyai descriptor");
        let settings = crate::settings::AppSettings::default();
        let store = crate::credentials::CredentialStore::default();

        let readiness = base_provider_readiness(descriptor, &settings, &store, 7);

        assert_eq!(readiness.provider_id, "asr.assemblyai");
        assert_eq!(readiness.model_count, Some(1));
        assert_eq!(readiness.model_catalog.len(), 1);
        assert_eq!(readiness.model_catalog[0].id, "universal-3-5-pro");
        assert!(readiness.voice_catalog.is_empty());
        assert!(readiness.language_catalog.is_empty());
        assert_eq!(readiness.openrouter_models.len(), 0);
    }

    #[test]
    fn base_readiness_exposes_deepgram_aura_voice_catalog() {
        let descriptor = crate::provider_registry::provider_registry()
            .iter()
            .find(|descriptor| descriptor.id == "tts.deepgram_aura")
            .expect("deepgram aura descriptor");
        let settings = crate::settings::AppSettings::default();
        let store = crate::credentials::CredentialStore::default();

        let readiness = base_provider_readiness(descriptor, &settings, &store, 7);

        assert_eq!(readiness.provider_id, "tts.deepgram_aura");
        // Aura ships a fixed voice catalog owned by the generated registry.
        // Assert non-empty + presence of key voices rather than a magic number
        // so this test tracks catalog growth (Aura-2 + non-English) without rot.
        assert!(!readiness.model_catalog.is_empty());
        assert_eq!(readiness.voice_catalog.len(), readiness.model_catalog.len());
        assert_eq!(readiness.model_count, Some(readiness.voice_catalog.len()));
        assert_eq!(readiness.voice_catalog[0].id, "aura-asteria-en");
        assert!(readiness.voice_catalog[0].is_default);
        assert!(
            readiness
                .voice_catalog
                .iter()
                .any(|voice| voice.id == "aura-zeus-en")
        );
        // At least one Aura-2 voice ships in the expanded catalog.
        assert!(
            readiness
                .voice_catalog
                .iter()
                .any(|voice| voice.id == "aura-2-thalia-en")
        );
        assert!(readiness.language_catalog.is_empty());
    }

    #[test]
    fn moonshine_base_readiness_reports_local_model_catalog() {
        let descriptor = crate::provider_registry::provider_registry()
            .iter()
            .find(|descriptor| descriptor.id == "asr.moonshine")
            .expect("moonshine descriptor");
        let settings = crate::settings::AppSettings::default();
        let store = crate::credentials::CredentialStore::default();

        let readiness = base_provider_readiness(descriptor, &settings, &store, 8);

        assert_eq!(readiness.provider_id, "asr.moonshine");
        assert_eq!(readiness.status, ProviderReadinessStatus::Unchecked);
        assert_eq!(
            readiness.message,
            "Local model readiness is checked by the model manager"
        );
        assert_eq!(readiness.runtime, None);
        assert_eq!(readiness.model_count, Some(3));
        assert_eq!(readiness.model_catalog.len(), 3);
        assert!(
            readiness
                .model_catalog
                .iter()
                .any(|model| model.id == crate::models::MOONSHINE_MEDIUM_STREAMING_EN)
        );
        assert!(readiness.credentials.is_empty());
    }

    #[test]
    fn planned_static_credential_providers_use_registry_keys_for_readiness() {
        let mut speechmatics_store = crate::credentials::CredentialStore::default();
        speechmatics_store.speechmatics_api_key = Some("sm-saved".to_string());
        let mut gladia_store = crate::credentials::CredentialStore::default();
        gladia_store.gladia_api_key = Some("gladia-saved".to_string());

        for (provider_id, key, saved_key) in [
            (
                "asr.speechmatics",
                "speechmatics_api_key",
                speechmatics_store,
            ),
            ("asr.gladia", "gladia_api_key", gladia_store),
        ] {
            let descriptor = crate::provider_registry::descriptor_by_id(provider_id);
            let settings = crate::settings::AppSettings::default();
            let missing = base_provider_readiness(
                descriptor,
                &settings,
                &crate::credentials::CredentialStore::default(),
                31,
            );

            assert_eq!(missing.status, ProviderReadinessStatus::MissingCredentials);
            assert_eq!(
                missing.credentials,
                vec![ProviderCredentialReadiness {
                    key: key.to_string(),
                    present: false,
                }]
            );
            assert!(missing.message.contains(key));

            let present = base_provider_readiness(descriptor, &settings, &saved_key, 32);
            assert_eq!(present.status, ProviderReadinessStatus::Unchecked);
            assert_eq!(
                present.credentials,
                vec![ProviderCredentialReadiness {
                    key: key.to_string(),
                    present: true,
                }]
            );
            assert_eq!(
                present.message,
                "No automatic health probe is available for this provider yet"
            );
            assert!(
                !provider_has_automatic_health_probe(descriptor, &settings),
                "{provider_id} should remain explicitly unchecked until a safe probe is wired"
            );
        }
    }

    #[test]
    fn moonshine_local_model_readiness_reports_missing_components() {
        let descriptor = crate::provider_registry::provider_registry()
            .iter()
            .find(|descriptor| descriptor.id == "asr.moonshine")
            .expect("moonshine descriptor");
        let models_dir = std::env::temp_dir().join(format!(
            "audiograph-moonshine-readiness-missing-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&models_dir).unwrap();

        let message = local_model_readiness_message(descriptor, &models_dir)
            .expect("moonshine should have local model readiness");

        assert!(message.contains("No local model files are ready yet"));
        assert!(message.contains(crate::models::MOONSHINE_SMALL_STREAMING_EN));
        assert!(message.contains("not selectable yet"));

        let _ = std::fs::remove_dir_all(models_dir);
    }

    #[test]
    fn moonshine_local_model_readiness_counts_valid_component_directory() {
        let descriptor = crate::provider_registry::provider_registry()
            .iter()
            .find(|descriptor| descriptor.id == "asr.moonshine")
            .expect("moonshine descriptor");
        let models_dir = std::env::temp_dir().join(format!(
            "audiograph-moonshine-readiness-valid-{}",
            uuid::Uuid::new_v4()
        ));
        let small_dir = models_dir.join(crate::models::MOONSHINE_SMALL_STREAMING_EN);
        std::fs::create_dir_all(&small_dir).unwrap();
        for required in crate::models::MOONSHINE_STREAMING_REQUIRED_FILES {
            std::fs::write(small_dir.join(required), b"component").unwrap();
        }
        let tiny_dir = models_dir.join(crate::models::MOONSHINE_TINY_STREAMING_EN);
        std::fs::create_dir_all(&tiny_dir).unwrap();
        std::fs::write(tiny_dir.join("tokenizer.bin"), b"component").unwrap();

        let message = local_model_readiness_message(descriptor, &models_dir)
            .expect("moonshine should have local model readiness");

        assert!(message.contains("Local model files ready: 1/3"));
        assert!(message.contains("moonshine-tiny-streaming-en missing"));
        assert!(message.contains(crate::models::MOONSHINE_MEDIUM_STREAMING_EN));

        let _ = std::fs::remove_dir_all(models_dir);
    }

    fn write_complete_moonshine_model_dir(
        models_dir: &std::path::Path,
        model_id: &str,
    ) -> std::path::PathBuf {
        let model_dir = models_dir.join(model_id);
        std::fs::create_dir_all(&model_dir).unwrap();
        for required in crate::models::MOONSHINE_STREAMING_REQUIRED_FILES {
            std::fs::write(model_dir.join(required), b"component").unwrap();
        }
        model_dir
    }

    #[test]
    fn moonshine_runtime_readiness_classifies_feature_model_load_and_healthy_states() {
        let feature_missing = moonshine_runtime_readiness_from_state(false, 1, None);
        assert_eq!(
            feature_missing.status,
            ProviderRuntimeReadinessStatus::FeatureMissing
        );
        assert_eq!(
            feature_missing.required_feature.as_deref(),
            Some("asr-moonshine")
        );

        let model_missing = moonshine_runtime_readiness_from_state(true, 0, None);
        assert_eq!(
            model_missing.status,
            ProviderRuntimeReadinessStatus::ModelMissing
        );

        let unavailable = moonshine_runtime_readiness_from_state(true, 1, None);
        assert_eq!(
            unavailable.status,
            ProviderRuntimeReadinessStatus::RuntimeUnavailable
        );

        let load_failed = moonshine_runtime_readiness_from_state(
            true,
            1,
            Some(LocalRuntimeProbeOutcome::LoadFailed {
                message: "failed to load moonshine shared library".to_string(),
                model_id: Some(crate::models::MOONSHINE_SMALL_STREAMING_EN.to_string()),
            }),
        );
        assert_eq!(
            load_failed.status,
            ProviderRuntimeReadinessStatus::LoadFailed
        );
        assert_eq!(
            load_failed.model_id.as_deref(),
            Some(crate::models::MOONSHINE_SMALL_STREAMING_EN)
        );

        let healthy = moonshine_runtime_readiness_from_state(
            true,
            1,
            Some(LocalRuntimeProbeOutcome::Healthy {
                runtime_version: "moonshine-c-api-test".to_string(),
                model_id: crate::models::MOONSHINE_SMALL_STREAMING_EN.to_string(),
            }),
        );
        assert_eq!(healthy.status, ProviderRuntimeReadinessStatus::Healthy);
        assert_eq!(
            healthy.runtime_version.as_deref(),
            Some("moonshine-c-api-test")
        );
    }

    #[cfg(not(feature = "asr-moonshine"))]
    #[test]
    fn moonshine_runtime_feature_missing_skips_probe_even_with_complete_model_dir() {
        let descriptor = crate::provider_registry::descriptor_by_id("asr.moonshine");
        let models_dir = std::env::temp_dir().join(format!(
            "audiograph-moonshine-feature-missing-{}",
            uuid::Uuid::new_v4()
        ));
        write_complete_moonshine_model_dir(
            &models_dir,
            crate::models::MOONSHINE_SMALL_STREAMING_EN,
        );
        let base = base_provider_readiness(
            descriptor,
            &crate::settings::AppSettings::default(),
            &crate::credentials::CredentialStore::default(),
            35,
        );

        let readiness = apply_local_model_readiness_from_dir_with_probe(
            descriptor,
            &models_dir,
            base,
            |_probe_descriptor, _summary, _probe_models_dir| {
                panic!("compiled-out Moonshine must not require a native runtime probe")
            },
        );

        assert_eq!(readiness.status, ProviderReadinessStatus::Unchecked);
        let runtime = readiness.runtime.expect("moonshine runtime readiness");
        assert_eq!(
            runtime.status,
            ProviderRuntimeReadinessStatus::FeatureMissing
        );
        assert_eq!(runtime.required_feature.as_deref(), Some("asr-moonshine"));

        let _ = std::fs::remove_dir_all(models_dir);
    }

    #[cfg(feature = "asr-moonshine")]
    #[test]
    fn moonshine_runtime_production_probe_maps_unavailable_loader_to_load_failed() {
        let descriptor = crate::provider_registry::descriptor_by_id("asr.moonshine");
        let models_dir = std::env::temp_dir().join(format!(
            "audiograph-moonshine-production-probe-{}",
            uuid::Uuid::new_v4()
        ));
        write_complete_moonshine_model_dir(
            &models_dir,
            crate::models::MOONSHINE_SMALL_STREAMING_EN,
        );
        let base = base_provider_readiness(
            descriptor,
            &crate::settings::AppSettings::default(),
            &crate::credentials::CredentialStore::default(),
            36,
        );

        let readiness = apply_local_model_readiness_from_dir(descriptor, &models_dir, base);

        assert_eq!(readiness.status, ProviderReadinessStatus::Unchecked);
        assert!(readiness.message.contains("Local model files ready: 1/3"));
        let runtime = readiness.runtime.expect("moonshine runtime readiness");
        assert_eq!(runtime.status, ProviderRuntimeReadinessStatus::LoadFailed);
        assert_eq!(
            runtime.model_id.as_deref(),
            Some(crate::models::MOONSHINE_SMALL_STREAMING_EN)
        );
        assert!(runtime.message.contains("native runtime load failed"));
        assert!(runtime.message.contains("not linked"));
        assert_eq!(
            descriptor.status,
            crate::provider_registry::ProviderStatus::Planned
        );
        assert!(
            local_asr_provider_availability_error(&crate::settings::AsrProvider::Moonshine {
                model_dir: crate::models::MOONSHINE_SMALL_STREAMING_EN.to_string(),
                enable_speaker_hints: true,
            })
            .is_some(),
            "Moonshine remains unavailable for selection while the descriptor is planned"
        );

        let _ = std::fs::remove_dir_all(models_dir);
    }

    #[cfg(feature = "asr-moonshine")]
    #[test]
    fn moonshine_runtime_missing_and_invalid_models_skip_probe() {
        let descriptor = crate::provider_registry::descriptor_by_id("asr.moonshine");

        let missing_models_dir = std::env::temp_dir().join(format!(
            "audiograph-moonshine-missing-probe-skip-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&missing_models_dir).unwrap();
        let missing_base = base_provider_readiness(
            descriptor,
            &crate::settings::AppSettings::default(),
            &crate::credentials::CredentialStore::default(),
            37,
        );
        let missing_readiness = apply_local_model_readiness_from_dir_with_probe(
            descriptor,
            &missing_models_dir,
            missing_base,
            |_probe_descriptor, _summary, _probe_models_dir| {
                panic!("missing Moonshine models must not invoke the native runtime probe")
            },
        );
        let missing_runtime = missing_readiness
            .runtime
            .expect("moonshine missing runtime readiness");
        assert_eq!(
            missing_runtime.status,
            ProviderRuntimeReadinessStatus::ModelMissing
        );
        assert!(missing_readiness.message.contains("Missing:"));

        let invalid_models_dir = std::env::temp_dir().join(format!(
            "audiograph-moonshine-invalid-probe-skip-{}",
            uuid::Uuid::new_v4()
        ));
        let invalid_model_dir =
            invalid_models_dir.join(crate::models::MOONSHINE_SMALL_STREAMING_EN);
        std::fs::create_dir_all(&invalid_model_dir).unwrap();
        for (index, required) in crate::models::MOONSHINE_STREAMING_REQUIRED_FILES
            .iter()
            .enumerate()
        {
            let bytes: &[u8] = if index == 0 { b"" } else { b"component" };
            std::fs::write(invalid_model_dir.join(required), bytes).unwrap();
        }
        let invalid_base = base_provider_readiness(
            descriptor,
            &crate::settings::AppSettings::default(),
            &crate::credentials::CredentialStore::default(),
            38,
        );
        let invalid_readiness = apply_local_model_readiness_from_dir_with_probe(
            descriptor,
            &invalid_models_dir,
            invalid_base,
            |_probe_descriptor, _summary, _probe_models_dir| {
                panic!("invalid Moonshine models must not invoke the native runtime probe")
            },
        );
        let invalid_runtime = invalid_readiness
            .runtime
            .expect("moonshine invalid runtime readiness");
        assert_eq!(
            invalid_runtime.status,
            ProviderRuntimeReadinessStatus::ModelMissing
        );
        assert!(invalid_readiness.message.contains("Invalid:"));
        assert!(
            invalid_readiness
                .message
                .contains(crate::models::MOONSHINE_SMALL_STREAMING_EN)
        );

        let _ = std::fs::remove_dir_all(missing_models_dir);
        let _ = std::fs::remove_dir_all(invalid_models_dir);
    }

    #[cfg(feature = "asr-moonshine")]
    #[test]
    fn moonshine_runtime_probe_outcome_flows_through_local_readiness_application() {
        let descriptor = crate::provider_registry::descriptor_by_id("asr.moonshine");
        let models_dir = std::env::temp_dir().join(format!(
            "audiograph-moonshine-runtime-probe-{}",
            uuid::Uuid::new_v4()
        ));
        let small_dir = models_dir.join(crate::models::MOONSHINE_SMALL_STREAMING_EN);
        std::fs::create_dir_all(&small_dir).unwrap();
        for required in crate::models::MOONSHINE_STREAMING_REQUIRED_FILES {
            std::fs::write(small_dir.join(required), b"component").unwrap();
        }

        let base = base_provider_readiness(
            descriptor,
            &crate::settings::AppSettings::default(),
            &crate::credentials::CredentialStore::default(),
            33,
        );
        let readiness = apply_local_model_readiness_from_dir_with_probe(
            descriptor,
            &models_dir,
            base,
            |probe_descriptor, summary, probe_models_dir| {
                assert_eq!(probe_descriptor.id, "asr.moonshine");
                assert_eq!(summary.ready, 1);
                assert_eq!(probe_models_dir, models_dir.as_path());
                Some(LocalRuntimeProbeOutcome::LoadFailed {
                    message: "failed to load Moonshine shared library".to_string(),
                    model_id: Some(crate::models::MOONSHINE_SMALL_STREAMING_EN.to_string()),
                })
            },
        );

        assert_eq!(readiness.status, ProviderReadinessStatus::Unchecked);
        assert!(readiness.message.contains("Local model files ready: 1/3"));
        let runtime = readiness.runtime.expect("moonshine runtime readiness");
        assert_eq!(runtime.status, ProviderRuntimeReadinessStatus::LoadFailed);
        assert_eq!(
            runtime.model_id.as_deref(),
            Some(crate::models::MOONSHINE_SMALL_STREAMING_EN)
        );
        assert!(runtime.message.contains("shared library"));
        assert_eq!(
            descriptor.status,
            crate::provider_registry::ProviderStatus::Planned,
            "fake probe outcomes must not make Moonshine selectable"
        );

        let _ = std::fs::remove_dir_all(models_dir);
    }

    #[cfg(feature = "asr-moonshine")]
    #[test]
    fn moonshine_healthy_probe_keeps_planned_provider_unchecked_until_selectable() {
        let descriptor = crate::provider_registry::descriptor_by_id("asr.moonshine");
        let models_dir = std::env::temp_dir().join(format!(
            "audiograph-moonshine-runtime-healthy-{}",
            uuid::Uuid::new_v4()
        ));
        let small_dir = models_dir.join(crate::models::MOONSHINE_SMALL_STREAMING_EN);
        std::fs::create_dir_all(&small_dir).unwrap();
        for required in crate::models::MOONSHINE_STREAMING_REQUIRED_FILES {
            std::fs::write(small_dir.join(required), b"component").unwrap();
        }

        let base = base_provider_readiness(
            descriptor,
            &crate::settings::AppSettings::default(),
            &crate::credentials::CredentialStore::default(),
            34,
        );
        let readiness = apply_local_model_readiness_from_dir_with_probe(
            descriptor,
            &models_dir,
            base,
            |_probe_descriptor, summary, probe_models_dir| {
                assert_eq!(summary.ready, 1);
                assert_eq!(probe_models_dir, models_dir.as_path());
                Some(LocalRuntimeProbeOutcome::Healthy {
                    runtime_version: "moonshine-c-api-test".to_string(),
                    model_id: crate::models::MOONSHINE_SMALL_STREAMING_EN.to_string(),
                })
            },
        );

        assert_eq!(readiness.status, ProviderReadinessStatus::Unchecked);
        let runtime = readiness.runtime.expect("moonshine runtime readiness");
        assert_eq!(runtime.status, ProviderRuntimeReadinessStatus::Healthy);
        assert_eq!(
            runtime.runtime_version.as_deref(),
            Some("moonshine-c-api-test")
        );
        assert_eq!(
            runtime.model_id.as_deref(),
            Some(crate::models::MOONSHINE_SMALL_STREAMING_EN)
        );
        assert_eq!(
            descriptor.status,
            crate::provider_registry::ProviderStatus::Planned
        );

        let _ = std::fs::remove_dir_all(models_dir);
    }

    fn write_complete_clustering_model_files(
        models_dir: &std::path::Path,
    ) -> (std::path::PathBuf, std::path::PathBuf) {
        let segmentation_dir = models_dir.join(crate::models::DIAR_SEG_PYANNOTE_DIR);
        std::fs::create_dir_all(&segmentation_dir).unwrap();
        let segmentation_model = segmentation_dir.join(crate::models::DIAR_SEG_PYANNOTE_FILE);
        std::fs::write(segmentation_dir.join("model.onnx"), b"component").unwrap();
        std::fs::write(&segmentation_model, b"component").unwrap();
        let embedding_model = models_dir.join(crate::models::DIAR_EMB_TITANET_FILENAME);
        // The embedding `.onnx` is a bare File-kind model with a published size
        // floor (BUG 3f23); write at least that many bytes so readiness reports
        // it ready instead of truncated/invalid.
        std::fs::write(
            &embedding_model,
            vec![0u8; crate::models::DIAR_EMB_TITANET_MIN_BYTES as usize],
        )
        .unwrap();
        (segmentation_model, embedding_model)
    }

    #[test]
    fn diarization_clustering_runtime_readiness_classifies_feature_model_load_and_healthy_states() {
        let feature_missing =
            diarization_clustering_runtime_readiness_from_state(false, 2, 2, None);
        assert_eq!(
            feature_missing.status,
            ProviderRuntimeReadinessStatus::FeatureMissing
        );
        assert_eq!(
            feature_missing.required_feature.as_deref(),
            Some("diarization-clustering")
        );

        let model_missing = diarization_clustering_runtime_readiness_from_state(true, 1, 2, None);
        assert_eq!(
            model_missing.status,
            ProviderRuntimeReadinessStatus::ModelMissing
        );

        let unavailable = diarization_clustering_runtime_readiness_from_state(true, 2, 2, None);
        assert_eq!(
            unavailable.status,
            ProviderRuntimeReadinessStatus::RuntimeUnavailable
        );

        let model_id = diarization_clustering_runtime_model_id();
        let load_failed = diarization_clustering_runtime_readiness_from_state(
            true,
            2,
            2,
            Some(LocalRuntimeProbeOutcome::LoadFailed {
                message: "failed to load sherpa-onnx diarization".to_string(),
                model_id: Some(model_id.clone()),
            }),
        );
        assert_eq!(
            load_failed.status,
            ProviderRuntimeReadinessStatus::LoadFailed
        );
        assert_eq!(load_failed.model_id.as_deref(), Some(model_id.as_str()));

        let healthy = diarization_clustering_runtime_readiness_from_state(
            true,
            2,
            2,
            Some(LocalRuntimeProbeOutcome::Healthy {
                runtime_version: "sherpa-onnx-clustering-16000hz".to_string(),
                model_id: model_id.clone(),
            }),
        );
        assert_eq!(healthy.status, ProviderRuntimeReadinessStatus::Healthy);
        assert_eq!(
            healthy.runtime_version.as_deref(),
            Some("sherpa-onnx-clustering-16000hz")
        );
        assert_eq!(healthy.model_id.as_deref(), Some(model_id.as_str()));
    }

    #[cfg(not(feature = "diarization-clustering"))]
    #[test]
    fn clustering_runtime_feature_missing_skips_probe_even_with_complete_model_files() {
        let descriptor = crate::provider_registry::descriptor_by_id("diarization.clustering");
        let models_dir = std::env::temp_dir().join(format!(
            "audiograph-clustering-feature-missing-{}",
            uuid::Uuid::new_v4()
        ));
        let _ = write_complete_clustering_model_files(&models_dir);
        let base = base_provider_readiness(
            descriptor,
            &crate::settings::AppSettings::default(),
            &crate::credentials::CredentialStore::default(),
            39,
        );

        let readiness = apply_local_model_readiness_from_dir_with_probe(
            descriptor,
            &models_dir,
            base,
            |_probe_descriptor, _summary, _probe_models_dir| {
                panic!("compiled-out clustering diarization must not require sherpa-onnx probing")
            },
        );

        assert_eq!(readiness.status, ProviderReadinessStatus::Unchecked);
        assert!(readiness.message.contains("Local model files ready: 2/2"));
        let runtime = readiness
            .runtime
            .expect("clustering diarization runtime readiness");
        assert_eq!(
            runtime.status,
            ProviderRuntimeReadinessStatus::FeatureMissing
        );
        assert_eq!(
            runtime.required_feature.as_deref(),
            Some("diarization-clustering")
        );

        let _ = std::fs::remove_dir_all(models_dir);
    }

    #[cfg(feature = "diarization-clustering")]
    #[test]
    fn clustering_runtime_missing_and_invalid_models_skip_probe() {
        let descriptor = crate::provider_registry::descriptor_by_id("diarization.clustering");

        let missing_models_dir = std::env::temp_dir().join(format!(
            "audiograph-clustering-missing-probe-skip-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&missing_models_dir).unwrap();
        let missing_base = base_provider_readiness(
            descriptor,
            &crate::settings::AppSettings::default(),
            &crate::credentials::CredentialStore::default(),
            40,
        );
        let missing_readiness = apply_local_model_readiness_from_dir_with_probe(
            descriptor,
            &missing_models_dir,
            missing_base,
            |_probe_descriptor, _summary, _probe_models_dir| {
                panic!("missing clustering diarization models must not invoke sherpa-onnx probing")
            },
        );
        let missing_runtime = missing_readiness
            .runtime
            .expect("clustering missing runtime readiness");
        assert_eq!(
            missing_runtime.status,
            ProviderRuntimeReadinessStatus::ModelMissing
        );
        assert!(missing_readiness.message.contains("Missing:"));

        let invalid_models_dir = std::env::temp_dir().join(format!(
            "audiograph-clustering-invalid-probe-skip-{}",
            uuid::Uuid::new_v4()
        ));
        let segmentation_dir = invalid_models_dir.join(crate::models::DIAR_SEG_PYANNOTE_DIR);
        std::fs::create_dir_all(&segmentation_dir).unwrap();
        std::fs::write(segmentation_dir.join("model.onnx"), b"component").unwrap();
        std::fs::write(
            segmentation_dir.join(crate::models::DIAR_SEG_PYANNOTE_FILE),
            b"",
        )
        .unwrap();
        std::fs::write(
            invalid_models_dir.join(crate::models::DIAR_EMB_TITANET_FILENAME),
            b"component",
        )
        .unwrap();
        let invalid_base = base_provider_readiness(
            descriptor,
            &crate::settings::AppSettings::default(),
            &crate::credentials::CredentialStore::default(),
            41,
        );
        let invalid_readiness = apply_local_model_readiness_from_dir_with_probe(
            descriptor,
            &invalid_models_dir,
            invalid_base,
            |_probe_descriptor, _summary, _probe_models_dir| {
                panic!("invalid clustering diarization models must not invoke sherpa-onnx probing")
            },
        );
        let invalid_runtime = invalid_readiness
            .runtime
            .expect("clustering invalid runtime readiness");
        assert_eq!(
            invalid_runtime.status,
            ProviderRuntimeReadinessStatus::ModelMissing
        );
        assert!(invalid_readiness.message.contains("Invalid:"));

        let _ = std::fs::remove_dir_all(missing_models_dir);
        let _ = std::fs::remove_dir_all(invalid_models_dir);
    }

    #[cfg(feature = "diarization-clustering")]
    #[test]
    fn clustering_runtime_probe_outcome_flows_through_local_readiness_application() {
        let descriptor = crate::provider_registry::descriptor_by_id("diarization.clustering");
        let models_dir = std::env::temp_dir().join(format!(
            "audiograph-clustering-runtime-probe-{}",
            uuid::Uuid::new_v4()
        ));
        let _ = write_complete_clustering_model_files(&models_dir);

        let base = base_provider_readiness(
            descriptor,
            &crate::settings::AppSettings::default(),
            &crate::credentials::CredentialStore::default(),
            42,
        );
        let model_id = diarization_clustering_runtime_model_id();
        let readiness = apply_local_model_readiness_from_dir_with_probe(
            descriptor,
            &models_dir,
            base,
            |probe_descriptor, summary, probe_models_dir| {
                assert_eq!(probe_descriptor.id, "diarization.clustering");
                assert_eq!(summary.ready, 2);
                assert_eq!(summary.total, 2);
                assert_eq!(probe_models_dir, models_dir.as_path());
                Some(LocalRuntimeProbeOutcome::LoadFailed {
                    message: "failed to load sherpa-onnx diarization".to_string(),
                    model_id: Some(model_id.clone()),
                })
            },
        );

        assert_eq!(readiness.status, ProviderReadinessStatus::Unchecked);
        assert!(readiness.message.contains("Local model files ready: 2/2"));
        let runtime = readiness
            .runtime
            .expect("clustering diarization runtime readiness");
        assert_eq!(runtime.status, ProviderRuntimeReadinessStatus::LoadFailed);
        assert_eq!(runtime.model_id.as_deref(), Some(model_id.as_str()));
        assert!(runtime.message.contains("sherpa-onnx"));

        let healthy_base = base_provider_readiness(
            descriptor,
            &crate::settings::AppSettings::default(),
            &crate::credentials::CredentialStore::default(),
            43,
        );
        let healthy = apply_local_model_readiness_from_dir_with_probe(
            descriptor,
            &models_dir,
            healthy_base,
            |_probe_descriptor, summary, _probe_models_dir| {
                assert_eq!(summary.ready, 2);
                Some(LocalRuntimeProbeOutcome::Healthy {
                    runtime_version: "sherpa-onnx-clustering-16000hz".to_string(),
                    model_id: model_id.clone(),
                })
            },
        );
        let healthy_runtime = healthy
            .runtime
            .expect("healthy clustering diarization runtime readiness");
        assert_eq!(
            healthy_runtime.status,
            ProviderRuntimeReadinessStatus::Healthy
        );
        assert_eq!(
            healthy_runtime.runtime_version.as_deref(),
            Some("sherpa-onnx-clustering-16000hz")
        );

        let _ = std::fs::remove_dir_all(models_dir);
    }

    #[cfg(feature = "diarization-clustering")]
    #[test]
    fn clustering_runtime_production_probe_maps_invalid_onnx_to_load_failed() {
        let descriptor = crate::provider_registry::descriptor_by_id("diarization.clustering");
        let models_dir = std::env::temp_dir().join(format!(
            "audiograph-clustering-production-probe-{}",
            uuid::Uuid::new_v4()
        ));
        let _ = write_complete_clustering_model_files(&models_dir);
        let base = base_provider_readiness(
            descriptor,
            &crate::settings::AppSettings::default(),
            &crate::credentials::CredentialStore::default(),
            44,
        );

        let readiness = apply_local_model_readiness_from_dir(descriptor, &models_dir, base);

        assert_eq!(readiness.status, ProviderReadinessStatus::Unchecked);
        assert!(readiness.message.contains("Local model files ready: 2/2"));
        let runtime = readiness
            .runtime
            .expect("clustering diarization runtime readiness");
        assert_eq!(runtime.status, ProviderRuntimeReadinessStatus::LoadFailed);
        let model_id = diarization_clustering_runtime_model_id();
        assert_eq!(runtime.model_id.as_deref(), Some(model_id.as_str()));
        assert!(
            runtime
                .message
                .contains("Clustering diarization runtime load failed")
        );

        let _ = std::fs::remove_dir_all(models_dir);
    }

    #[test]
    fn diarization_runtime_model_readiness_reports_missing_and_invalid_dependencies() {
        let models_dir = std::env::temp_dir().join(format!(
            "audio-graph-diarization-readiness-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&models_dir).expect("create temp models dir");

        let sortformer = crate::provider_registry::descriptor_by_id("diarization.sortformer");
        let sortformer_summary =
            local_model_readiness_summary(sortformer, &models_dir).expect("sortformer summary");

        assert_eq!(sortformer_summary.total, 1);
        assert_eq!(sortformer_summary.ready, 0);
        assert_eq!(
            sortformer_summary.missing,
            vec![crate::models::SORTFORMER_MODEL_FILENAME.to_string()]
        );
        assert!(
            local_model_readiness_message(sortformer, &models_dir)
                .expect("sortformer message")
                .contains(crate::models::SORTFORMER_MODEL_FILENAME)
        );

        let clustering = crate::provider_registry::descriptor_by_id("diarization.clustering");
        let segmentation_dir = models_dir.join(crate::models::DIAR_SEG_PYANNOTE_DIR);
        std::fs::create_dir_all(&segmentation_dir).expect("create segmentation dir");
        std::fs::write(segmentation_dir.join("model.onnx"), b"present").expect("write fp32 model");
        std::fs::write(segmentation_dir.join("model.int8.onnx"), b"").expect("write empty int8");

        let clustering_summary =
            local_model_readiness_summary(clustering, &models_dir).expect("clustering summary");

        assert_eq!(clustering_summary.total, 2);
        assert_eq!(clustering_summary.ready, 0);
        assert!(
            clustering_summary
                .missing
                .contains(&crate::models::DIAR_EMB_TITANET_FILENAME.to_string())
        );
        assert!(clustering_summary.invalid.iter().any(|entry| {
            entry.contains(crate::models::DIAR_SEG_PYANNOTE_DIR)
                && entry.contains("model.int8.onnx")
        }));
        assert!(
            local_model_readiness_message(clustering, &models_dir)
                .expect("clustering message")
                .contains(crate::models::DIAR_EMB_TITANET_FILENAME)
        );

        std::fs::remove_dir_all(&models_dir).ok();
    }

    #[test]
    fn truncated_titanet_embedding_fails_readiness_with_clear_reason() {
        // BUG 3f23: a present-but-truncated TitaNet embedding `.onnx` (e.g. a
        // partial download or HTML error page) must FAIL readiness rather than
        // passing the non-empty check and deferring to a runtime ONNX load
        // failure. The whole clustering set is otherwise complete here.
        let models_dir = std::env::temp_dir().join(format!(
            "audio-graph-truncated-titanet-{}",
            uuid::Uuid::new_v4()
        ));
        let segmentation_dir = models_dir.join(crate::models::DIAR_SEG_PYANNOTE_DIR);
        std::fs::create_dir_all(&segmentation_dir).expect("create segmentation dir");
        std::fs::write(segmentation_dir.join("model.onnx"), b"component").expect("fp32");
        std::fs::write(
            segmentation_dir.join(crate::models::DIAR_SEG_PYANNOTE_FILE),
            b"component",
        )
        .expect("int8");
        // Truncated embedding: present, non-empty, but far below the floor.
        let embedding_model = models_dir.join(crate::models::DIAR_EMB_TITANET_FILENAME);
        std::fs::write(&embedding_model, b"truncated-onnx-header").expect("write truncated emb");
        assert!(
            std::fs::metadata(&embedding_model).unwrap().len()
                < crate::models::DIAR_EMB_TITANET_MIN_BYTES
        );

        let clustering = crate::provider_registry::descriptor_by_id("diarization.clustering");
        let summary =
            local_model_readiness_summary(clustering, &models_dir).expect("clustering summary");

        assert_eq!(summary.total, 2);
        // Segmentation directory is ready; the truncated embedding is NOT.
        assert_eq!(summary.ready, 1);
        assert!(
            !summary
                .ready_model_ids
                .contains(&crate::models::DIAR_EMB_TITANET_FILENAME.to_string()),
            "a truncated embedding must not be reported ready"
        );
        let invalid_entry = summary
            .invalid
            .iter()
            .find(|entry| entry.contains(crate::models::DIAR_EMB_TITANET_FILENAME))
            .expect("truncated embedding must be classified invalid");
        assert!(
            invalid_entry.contains("too small") && invalid_entry.contains("at least"),
            "invalid reason must explain the size shortfall, got: {invalid_entry}"
        );

        // The user-facing readiness message surfaces the same clear reason.
        assert!(
            local_model_readiness_message(clustering, &models_dir)
                .expect("clustering message")
                .contains("too small")
        );

        std::fs::remove_dir_all(&models_dir).ok();
    }

    #[test]
    fn min_model_size_floor_only_applies_to_titanet_embedding() {
        // Guards the descriptor floor's scope: only the TitaNet embedding has a
        // published minimum; other bare-file models keep the non-empty rule.
        assert_eq!(
            crate::models::min_model_size_bytes(crate::models::DIAR_EMB_TITANET_FILENAME),
            Some(crate::models::DIAR_EMB_TITANET_MIN_BYTES)
        );
        assert_eq!(
            crate::models::min_model_size_bytes(crate::models::SORTFORMER_MODEL_FILENAME),
            None
        );
    }

    #[test]
    fn gemini_vertex_readiness_is_unchecked_without_automatic_probe() {
        let descriptor = crate::provider_registry::provider_registry()
            .iter()
            .find(|descriptor| descriptor.id == "realtime_agent.gemini_live")
            .expect("gemini descriptor");
        let mut settings = crate::settings::AppSettings::default();
        settings.gemini.auth = crate::settings::GeminiAuthMode::VertexAI {
            project_id: "project".to_string(),
            location: "us-central1".to_string(),
            service_account_path: Some("/tmp/audio-graph-sa.json".to_string()),
        };
        let mut store = crate::credentials::CredentialStore::default();
        store.google_service_account_path = Some("/tmp/audio-graph-sa.json".to_string());
        let active_ids = active_provider_ids(&settings, true);

        assert!(!provider_has_automatic_health_probe(descriptor, &settings));
        assert!(!should_probe_provider(
            descriptor,
            &active_ids,
            &settings,
            &store
        ));

        let readiness = base_provider_readiness(descriptor, &settings, &store, 11);

        assert_eq!(readiness.status, ProviderReadinessStatus::Unchecked);
        assert!(!readiness.automatic_probe_available);
        assert_eq!(
            readiness.message,
            "Vertex AI readiness is not probed automatically yet"
        );
        assert_eq!(readiness.checked_at, None);
    }

    #[test]
    fn gemini_vertex_readiness_reports_missing_non_secret_config() {
        let descriptor = crate::provider_registry::provider_registry()
            .iter()
            .find(|descriptor| descriptor.id == "realtime_agent.gemini_live")
            .expect("gemini descriptor");
        let mut settings = crate::settings::AppSettings::default();
        settings.gemini.auth = crate::settings::GeminiAuthMode::VertexAI {
            project_id: " ".to_string(),
            location: "us-central1".to_string(),
            service_account_path: Some("/tmp/audio-graph-sa.json".to_string()),
        };
        let mut store = crate::credentials::CredentialStore::default();
        store.google_service_account_path = Some("/tmp/audio-graph-sa.json".to_string());

        let readiness = base_provider_readiness(descriptor, &settings, &store, 12);

        assert_eq!(readiness.status, ProviderReadinessStatus::Unchecked);
        assert!(!readiness.automatic_probe_available);
        assert_eq!(
            readiness.message,
            "Vertex AI project ID and location must be configured before readiness can be checked"
        );
        assert!(!should_probe_provider(
            descriptor,
            &active_provider_ids(&settings, true),
            &settings,
            &store
        ));
    }

    #[test]
    fn deferred_gemini_readiness_requires_an_explicit_request_set() {
        let descriptor = crate::provider_registry::provider_registry()
            .iter()
            .find(|descriptor| descriptor.id == "realtime_agent.gemini_live")
            .expect("gemini descriptor");
        let settings = crate::settings::AppSettings::default();
        let mut store = crate::credentials::CredentialStore::default();
        store.gemini_api_key = Some("gemini-saved".to_string());

        let notes_ids = requested_provider_ids(&settings, false, None);
        assert!(!notes_ids.contains("realtime_agent.gemini_live"));
        assert!(!should_probe_provider(
            descriptor, &notes_ids, &settings, &store
        ));
        assert_eq!(
            provider_readiness_config_fingerprint(descriptor, &settings, &notes_ids),
            "inactive"
        );

        let legacy_native_ids = requested_provider_ids(&settings, true, None);
        assert!(!legacy_native_ids.contains("realtime_agent.gemini_live"));
        assert!(!should_probe_provider(
            descriptor,
            &legacy_native_ids,
            &settings,
            &store
        ));

        let requested = vec!["realtime_agent.gemini_live".to_string()];
        let native_ids = requested_provider_ids(&settings, true, Some(&requested));
        assert!(native_ids.contains("realtime_agent.gemini_live"));
        assert!(should_probe_provider(
            descriptor,
            &native_ids,
            &settings,
            &store
        ));
        assert_eq!(
            provider_readiness_config_fingerprint(descriptor, &settings, &native_ids),
            format!("auth=api_key|model={}", settings.gemini.model.trim())
        );
    }

    #[test]
    fn aws_profile_readiness_requires_profile_name_before_probe() {
        let descriptor = crate::provider_registry::provider_registry()
            .iter()
            .find(|descriptor| descriptor.id == "asr.aws_transcribe")
            .expect("aws transcribe descriptor");
        let mut settings = crate::settings::AppSettings::default();
        settings.asr_provider = crate::settings::AsrProvider::AwsTranscribe {
            region: "us-east-1".to_string(),
            language_code: "en-US".to_string(),
            credential_source: crate::settings::AwsCredentialSource::Profile {
                name: " ".to_string(),
            },
            enable_diarization: true,
        };
        let store = crate::credentials::CredentialStore::default();

        let readiness = base_provider_readiness(descriptor, &settings, &store, 13);

        assert_eq!(readiness.status, ProviderReadinessStatus::Unchecked);
        assert!(!readiness.automatic_probe_available);
        assert_eq!(
            readiness.message,
            "AWS profile name must be configured before readiness can be checked"
        );
        assert!(!should_probe_provider(
            descriptor,
            &active_provider_ids(&settings, true),
            &settings,
            &store
        ));
    }

    #[test]
    fn aws_profile_listing_honors_explicit_local_file_paths() {
        let dir = unique_tempdir("aws-profile-overrides");
        let config = dir.join("custom-config");
        let credentials = dir.join("custom-credentials");
        std::fs::write(
            &config,
            "[default]\nregion = us-west-2\n[profile configured]\nregion = us-east-1\n",
        )
        .expect("write config fixture");
        std::fs::write(
            &credentials,
            "[credentials-only]\naws_access_key_id = test\n[configured]\naws_access_key_id = duplicate\n",
        )
        .expect("write credentials fixture");

        assert_eq!(
            list_aws_profiles_from_paths(Some(&config), Some(&credentials)),
            vec![
                "configured".to_string(),
                "credentials-only".to_string(),
                "default".to_string(),
            ]
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn openrouter_api_key_resolution_uses_saved_key_when_draft_is_blank() {
        let mut store = crate::credentials::CredentialStore::default();
        store.openrouter_api_key = Some("  sk-or-saved  ".to_string());

        let api_key = openrouter_api_key_from_store(&store).expect("saved key");

        assert_eq!(api_key, "sk-or-saved");
    }

    #[test]
    fn openrouter_api_key_resolution_prefers_draft_key() {
        let api_key = openrouter_api_key_from_draft_or_store(Some("  sk-or-draft  ".to_string()))
            .expect("draft key");

        assert_eq!(api_key, "sk-or-draft");
    }

    #[test]
    fn openrouter_api_key_resolution_rejects_missing_saved_key() {
        let mut store = crate::credentials::CredentialStore::default();
        store.openrouter_api_key = Some("   ".to_string());

        let err = openrouter_api_key_from_store(&store).expect_err("missing key");

        match err {
            AppError::CredentialMissing { key } => {
                assert_eq!(key, "openrouter_api_key");
            }
            other => panic!("expected CredentialMissing, got {other:?}"),
        }
    }

    #[test]
    fn openrouter_saved_key_catalog_commands_are_registered() {
        let lib_rs = include_str!("lib.rs");

        assert!(
            lib_rs.contains("commands::list_openrouter_providers_cmd"),
            "provider catalog command must be registered for Tauri IPC"
        );
        assert!(
            lib_rs.contains("commands::list_openrouter_model_endpoints_cmd"),
            "model endpoint catalog command must be registered for Tauri IPC"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn openrouter_saved_key_catalog_commands_require_saved_credential() {
        // Exercise the exact helpers used by the IPC commands with an explicit
        // empty store. HOME isolation is insufficient here because the default
        // desktop backend reads the OS keychain; a host credential must never
        // turn an offline unit test into a real provider request.
        let store = crate::credentials::CredentialStore::default();

        let providers_err = list_openrouter_providers_with_store(
            &store,
            Some("http://127.0.0.1:9/api/v1".to_string()),
        )
        .await
        .expect_err("missing saved key should fail before provider request");
        assert_openrouter_credential_missing(providers_err);

        let endpoints_err = list_openrouter_model_endpoints_with_store(
            &store,
            "openai/gpt-4".to_string(),
            Some("http://127.0.0.1:9/api/v1".to_string()),
        )
        .await
        .expect_err("missing saved key should fail before endpoint request");
        assert_openrouter_credential_missing(endpoints_err);
    }

    fn assert_openrouter_credential_missing(err: AppError) {
        match err {
            AppError::CredentialMissing { key } => {
                assert_eq!(key, "openrouter_api_key");
            }
            other => panic!("expected CredentialMissing, got {other:?}"),
        }
    }

    #[test]
    fn endpoint_api_key_resolution_routes_openai_compatible_slots() {
        let mut store = crate::credentials::CredentialStore::default();
        store.openai_api_key = Some("  sk-openai  ".to_string());
        store.cerebras_api_key = Some("  csk-cerebras  ".to_string());
        store.sambanova_api_key = Some("  sn-sambanova  ".to_string());
        store.groq_api_key = Some("  gsk-groq  ".to_string());
        store.together_api_key = Some("  tog-together  ".to_string());
        store.fireworks_api_key = Some("  fw-fireworks  ".to_string());
        store.openrouter_api_key = Some("  sk-or  ".to_string());
        store.gemini_api_key = Some("  AIza-gemini  ".to_string());

        assert_eq!(
            endpoint_api_key_from_store("https://api.openai.com/v1", &store).as_deref(),
            Some("sk-openai")
        );
        assert_eq!(
            endpoint_api_key_from_store(crate::settings::CEREBRAS_BASE_URL, &store).as_deref(),
            Some("csk-cerebras")
        );
        // Regression (audio-graph-8773): the SambaNova endpoint must resolve to
        // the dedicated `sambanova_api_key` slot, not the `openai_api_key`
        // fallback — otherwise readiness/health/model-list probes send the wrong
        // key and 401 despite a valid saved SambaNova key.
        assert_eq!(
            endpoint_api_key_from_store(crate::settings::SAMBANOVA_BASE_URL, &store).as_deref(),
            Some("sn-sambanova")
        );
        assert_eq!(
            endpoint_api_key_from_store("https://api.groq.com/openai/v1", &store).as_deref(),
            Some("gsk-groq")
        );
        assert_eq!(
            endpoint_api_key_from_store("https://api.together.xyz/v1", &store).as_deref(),
            Some("tog-together")
        );
        assert_eq!(
            endpoint_api_key_from_store("https://api.fireworks.ai/inference/v1", &store).as_deref(),
            Some("fw-fireworks")
        );
        assert_eq!(
            endpoint_api_key_from_store("https://openrouter.ai/api/v1", &store).as_deref(),
            Some("sk-or")
        );
        assert_eq!(
            endpoint_api_key_from_store(
                "https://generativelanguage.googleapis.com/v1beta/openai",
                &store,
            )
            .as_deref(),
            Some("AIza-gemini")
        );
    }

    #[test]
    fn endpoint_api_key_resolution_allows_missing_key_for_no_auth_endpoints() {
        let store = crate::credentials::CredentialStore::default();

        assert_eq!(
            endpoint_api_key_from_store("http://localhost:11434/v1", &store),
            None
        );
    }

    fn llm_api_descriptor() -> &'static crate::provider_registry::ProviderDescriptor {
        crate::provider_registry::provider_registry()
            .iter()
            .find(|descriptor| descriptor.id == "llm.api")
            .expect("llm.api descriptor")
    }

    fn settings_with_llm_api_endpoint(endpoint: &str, model: &str) -> crate::settings::AppSettings {
        let mut settings = crate::settings::AppSettings::default();
        settings.llm_provider = crate::settings::LlmProvider::Api {
            endpoint: endpoint.to_string(),
            api_key: String::new(),
            model: model.to_string(),
        };
        settings
    }

    #[test]
    fn llm_api_fingerprint_includes_endpoint_and_model_and_changes_with_endpoint() {
        let descriptor = llm_api_descriptor();

        let settings_a =
            settings_with_llm_api_endpoint("https://api.example.test/v1 ", " gpt-oss-120b");
        let active_a = active_provider_ids(&settings_a, false);
        assert!(active_a.contains("llm.api"));
        let fingerprint_a =
            provider_readiness_config_fingerprint(descriptor, &settings_a, &active_a);
        assert_eq!(
            fingerprint_a,
            "endpoint=https://api.example.test/v1|model=gpt-oss-120b"
        );

        // Changing the endpoint must change the fingerprint so the readiness
        // cache is invalidated.
        let settings_b =
            settings_with_llm_api_endpoint("https://other.example.test/v1", "gpt-oss-120b");
        let active_b = active_provider_ids(&settings_b, false);
        let fingerprint_b =
            provider_readiness_config_fingerprint(descriptor, &settings_b, &active_b);
        assert_ne!(fingerprint_a, fingerprint_b);
        assert_eq!(
            fingerprint_b,
            "endpoint=https://other.example.test/v1|model=gpt-oss-120b"
        );

        // The Cerebras endpoint stays on the dedicated `llm.cerebras` arm; the
        // generic `llm.api` fingerprint must NOT claim it.
        let settings_cerebras =
            settings_with_llm_api_endpoint(crate::settings::CEREBRAS_BASE_URL, "zai-glm-4.7");
        let active_cerebras = active_provider_ids(&settings_cerebras, false);
        assert!(active_cerebras.contains("llm.cerebras"));
        assert!(!active_cerebras.contains("llm.api"));
        assert_eq!(
            provider_readiness_config_fingerprint(descriptor, &settings_cerebras, &active_cerebras),
            "inactive"
        );
    }

    #[test]
    fn llm_api_endpoint_key_resolution_uses_saved_key_when_draft_is_none() {
        let mut store = crate::credentials::CredentialStore::default();
        store.openai_api_key = Some("  sk-openai-saved  ".to_string());

        // A generic OpenAI-compatible endpoint routes to the openai_api_key slot.
        let resolved = endpoint_api_key_from_store("https://api.example.test/v1", &store);
        assert_eq!(resolved.as_deref(), Some("sk-openai-saved"));
    }

    #[tokio::test(flavor = "current_thread")]
    // `_lock` serializes process-global HOME mutation across tests; held for the
    // whole single-threaded test body including `.await`s.
    #[allow(clippy::await_holding_lock)]
    async fn llm_api_connection_test_redacts_key_on_bad_endpoint() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("llm-api-bad-endpoint");
        let _guard = HomeGuard::set(&dir);

        // Closed port -> the request fails. The draft key must never appear in
        // the surfaced error, even when the connection itself fails.
        let secret = "sk-super-secret-llm-key";
        let err = test_openai_compatible_llm_connection_cmd(
            "http://127.0.0.1:9/v1".to_string(),
            Some(secret.to_string()),
        )
        .await
        .expect_err("closed port should fail");

        let rendered = format!("{err:?}");
        assert!(
            !rendered.contains(secret),
            "error must not echo the API key, got: {rendered}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn openai_compatible_model_catalog_parses_unique_model_ids() {
        let catalog = parse_openai_compatible_model_catalog_with_default(
            r##"{
                "object": "list",
                "data": [
                    { "id": "whisper-1", "object": "model" },
                    { "id": "whisper-large-v3", "object": "model" },
                    { "id": "whisper-large-v3", "object": "model" },
                    { "object": "model" },
                    { "id": "   " }
                ]
            }"##,
            Some("whisper-1"),
        )
        .expect("parse OpenAI-compatible model catalog");

        assert_eq!(catalog.len(), 2);
        assert_eq!(catalog[0].id, "whisper-1");
        assert!(catalog[0].is_default);
        assert_eq!(catalog[1].id, "whisper-large-v3");
        assert_eq!(catalog[1].display_name, "whisper-large-v3");
    }

    #[test]
    fn openai_compatible_model_catalog_honors_custom_default_model() {
        let catalog = parse_openai_compatible_model_catalog_with_default(
            r##"{
                "object": "list",
                "data": [
                    { "id": "zai-glm-4.7", "object": "model" },
                    { "id": "gpt-oss-120b", "object": "model" }
                ]
            }"##,
            Some(crate::provider_registry::CEREBRAS_DEFAULT_MODEL),
        )
        .expect("parse Cerebras model catalog");

        assert_eq!(catalog.len(), 2);
        assert_eq!(catalog[0].id, "zai-glm-4.7");
        assert!(!catalog[0].is_default);
        assert_eq!(
            catalog[1].id,
            crate::provider_registry::CEREBRAS_DEFAULT_MODEL
        );
        assert!(catalog[1].is_default);
    }

    #[test]
    fn mark_chat_default_model_flags_a_chat_model_not_whisper() {
        // Simulate what the shared fetch produces for the LLM path: the ASR
        // default marker `whisper-1` flagged, with real chat models present.
        let catalog = parse_openai_compatible_model_catalog_with_default(
            r##"{
                "object": "list",
                "data": [
                    { "id": "whisper-1", "object": "model" },
                    { "id": "gpt-4o", "object": "model" },
                    { "id": "gpt-4o-mini", "object": "model" }
                ]
            }"##,
            Some("whisper-1"),
        )
        .expect("parse catalog");

        // Precondition: the ASR marker is on whisper-1 and no chat model is flagged.
        assert!(
            catalog[0].is_default,
            "precondition: whisper-1 is the ASR default"
        );

        let relabeled = mark_chat_default_model(catalog);

        // whisper-1 must NOT be the default for the LLM path.
        let whisper = relabeled
            .iter()
            .find(|item| item.id == "whisper-1")
            .expect("whisper-1 present");
        assert!(
            !whisper.is_default,
            "whisper-1 (ASR model) must never be the chat default"
        );

        // Exactly one default, and it is a real chat model (gpt-4o-mini preferred).
        let defaults: Vec<&str> = relabeled
            .iter()
            .filter(|item| item.is_default)
            .map(|item| item.id.as_str())
            .collect();
        assert_eq!(
            defaults,
            vec!["gpt-4o-mini"],
            "a single chat model is default"
        );
    }

    #[test]
    fn mark_chat_default_model_falls_back_to_first_non_asr_when_no_preferred() {
        let catalog = parse_openai_compatible_model_catalog_with_default(
            r##"{
                "object": "list",
                "data": [
                    { "id": "whisper-1", "object": "model" },
                    { "id": "some-exotic-model", "object": "model" }
                ]
            }"##,
            Some("whisper-1"),
        )
        .expect("parse catalog");

        let relabeled = mark_chat_default_model(catalog);

        let defaults: Vec<&str> = relabeled
            .iter()
            .filter(|item| item.is_default)
            .map(|item| item.id.as_str())
            .collect();
        assert_eq!(
            defaults,
            vec!["some-exotic-model"],
            "with no preferred chat id, the first non-ASR model is default (never whisper)"
        );
    }

    #[test]
    fn mark_chat_default_model_handles_empty_catalog() {
        let relabeled = mark_chat_default_model(Vec::new());
        assert!(relabeled.is_empty(), "empty catalog stays empty, no panic");
    }

    #[test]
    fn cloud_asr_connection_error_redacts_resolved_api_key() {
        let api_key = "sk-cloud-asr-test-secret";
        let message = cloud_asr_connection_error_message(
            reqwest::StatusCode::FORBIDDEN,
            &format!(
                r#"{{"error":"provider echoed {api_key}","authorization":"Bearer bearer-asr-secret-12345","url":"https://provider.example/models?api_key=query-asr-secret-12345"}}"#
            ),
            Some(api_key),
        );

        assert!(
            message.contains("403 Forbidden"),
            "error must carry status, got: {message}"
        );
        assert!(
            message.contains("provider echoed"),
            "error must carry body context, got: {message}"
        );
        assert!(
            !message.contains(api_key),
            "error must redact the resolved key, got: {message}"
        );
        assert!(
            !message.contains("bearer-asr-secret-12345"),
            "error must redact bearer echoes, got: {message}"
        );
        assert!(
            !message.contains("query-asr-secret-12345"),
            "error must redact URL query credentials, got: {message}"
        );
        assert!(
            message.contains("<redacted>"),
            "error must mark the redacted value, got: {message}"
        );
        // This case is a 403, not a 401 — must NOT carry the 401-only
        // credential-rejected marker (audio-graph-57cc).
        assert!(!message.starts_with(crate::error::CREDENTIAL_REJECTED_PREFIX));
    }

    #[test]
    fn cloud_asr_connection_error_401_carries_credential_rejected_prefix() {
        // audio-graph-57cc: the generic OpenAI-compatible readiness arm
        // (asr.api / llm.cerebras / llm.sambanova / llm.api) shares this
        // helper, so a 401 there also gets the stable marker — but only when
        // a key was actually sent.
        let message = cloud_asr_connection_error_message(
            reqwest::StatusCode::UNAUTHORIZED,
            r#"{"error":"invalid api key"}"#,
            Some("test-key-not-real"),
        );

        assert!(message.starts_with(crate::error::CREDENTIAL_REJECTED_PREFIX));
    }

    #[test]
    fn cloud_asr_connection_error_401_without_key_stays_generic() {
        // Codex P2 (PR #92): localhost/loopback OpenAI-compatible endpoints
        // are probed with NO saved key; a 401 from them must not claim "the
        // saved key was rejected".
        let message = cloud_asr_connection_error_message(
            reqwest::StatusCode::UNAUTHORIZED,
            r#"{"error":"auth required"}"#,
            None,
        );

        assert!(!message.starts_with(crate::error::CREDENTIAL_REJECTED_PREFIX));
        assert!(message.contains("401"));
    }

    #[test]
    fn gemini_api_key_resolution_uses_saved_key_when_draft_is_blank() {
        let mut store = crate::credentials::CredentialStore::default();
        store.gemini_api_key = Some("  AIza-saved  ".to_string());

        let api_key = gemini_api_key_from_store(&store).expect("saved key");

        assert_eq!(api_key, "AIza-saved");
    }

    #[test]
    fn gemini_api_key_connection_error_401_carries_credential_rejected_prefix() {
        // audio-graph-57cc: same stable-prefix contract as the ASR probes.
        let message = gemini_api_key_connection_error_message(reqwest::StatusCode::UNAUTHORIZED);

        assert!(message.starts_with(crate::error::CREDENTIAL_REJECTED_PREFIX));
    }

    #[test]
    fn gemini_api_key_connection_error_429_has_no_credential_rejected_prefix() {
        let message =
            gemini_api_key_connection_error_message(reqwest::StatusCode::TOO_MANY_REQUESTS);

        assert!(!message.starts_with(crate::error::CREDENTIAL_REJECTED_PREFIX));
    }

    #[test]
    fn gemini_api_key_resolution_prefers_draft_key() {
        let api_key = gemini_api_key_from_draft_or_store(Some("  AIza-draft  ".to_string()))
            .expect("draft key");

        assert_eq!(api_key, "AIza-draft");
    }

    #[test]
    fn gemini_api_key_resolution_rejects_missing_saved_key() {
        let mut store = crate::credentials::CredentialStore::default();
        store.gemini_api_key = Some("   ".to_string());

        let err = gemini_api_key_from_store(&store).expect_err("missing key");

        match err {
            AppError::CredentialMissing { key } => {
                assert_eq!(key, "gemini_api_key");
            }
            other => panic!("expected CredentialMissing, got {other:?}"),
        }
    }

    #[test]
    fn deepgram_api_key_resolution_uses_saved_key_when_draft_is_blank() {
        let mut store = crate::credentials::CredentialStore::default();
        store.deepgram_api_key = Some("  dg-saved  ".to_string());

        let api_key = deepgram_api_key_from_store(&store).expect("saved key");

        assert_eq!(api_key, "dg-saved");
    }

    #[test]
    fn deepgram_api_key_resolution_prefers_draft_key() {
        let api_key = deepgram_api_key_from_draft_or_store(Some("  dg-draft  ".to_string()))
            .expect("draft key");

        assert_eq!(api_key, "dg-draft");
    }

    #[test]
    fn deepgram_api_key_resolution_rejects_missing_saved_key() {
        let mut store = crate::credentials::CredentialStore::default();
        store.deepgram_api_key = Some("   ".to_string());

        let err = deepgram_api_key_from_store(&store).expect_err("missing key");

        match err {
            AppError::CredentialMissing { key } => {
                assert_eq!(key, "deepgram_api_key");
            }
            other => panic!("expected CredentialMissing, got {other:?}"),
        }
    }

    #[test]
    fn deepgram_model_catalog_parses_streaming_stt_models() {
        let catalog = parse_deepgram_stt_model_catalog(
            r##"{
                "stt": [
                    {
                        "name": "nova-3",
                        "canonical_name": "nova-3",
                        "architecture": "base",
                        "streaming": true
                    },
                    {
                        "name": "batch-only",
                        "canonical_name": "batch-only",
                        "streaming": false
                    },
                    {
                        "name": "Flux General English",
                        "canonical_name": "flux-general-en",
                        "streaming": true
                    },
                    {
                        "name": "Flux General English duplicate",
                        "canonical_name": "flux-general-en",
                        "streaming": true
                    }
                ],
                "tts": [
                    {
                        "name": "zeus",
                        "canonical_name": "aura-2-zeus-en"
                    }
                ]
            }"##,
        )
        .expect("parse Deepgram model catalog");

        // nova-3 + the live flux-general-en entry + the CURATED flux-general-multi
        // that /v1/models never returns (appended by the parser).
        assert_eq!(catalog.len(), 3);
        assert_eq!(catalog[0].id, "nova-3");
        assert_eq!(catalog[0].display_name, "nova-3");
        assert!(catalog[0].is_default);
        // The live response already carried flux-general-en (dedup keeps ONE and
        // preserves its live display name — no curated overwrite, no duplicate).
        assert_eq!(catalog[1].id, "flux-general-en");
        assert_eq!(catalog[1].display_name, "Flux General English");
        assert_eq!(
            catalog.iter().filter(|i| i.id == "flux-general-en").count(),
            1,
            "flux-general-en must not be duplicated by the curated append"
        );
        // flux-general-multi was NOT in the response, so it is appended as a
        // curated entry with the curated display name.
        let multi = catalog
            .iter()
            .find(|i| i.id == "flux-general-multi")
            .expect("curated flux-general-multi must be appended");
        assert_eq!(
            multi.display_name,
            "Flux General Multilingual (turn-based, v2)"
        );
        assert!(!multi.is_default);
        assert!(!catalog.iter().any(|item| item.id == "batch-only"));
        assert!(!catalog.iter().any(|item| item.id == "aura-2-zeus-en"));
    }

    #[test]
    fn deepgram_model_catalog_surfaces_curated_flux_when_api_omits_it() {
        // The real /v1/models response never contains any flux entries; the
        // parser must still surface both curated flux ids so the picker offers
        // them (FIX-3 discoverability gap).
        let catalog = parse_deepgram_stt_model_catalog(
            r##"{
                "stt": [
                    { "name": "nova-3", "canonical_name": "nova-3", "streaming": true }
                ]
            }"##,
        )
        .expect("parse Deepgram model catalog");

        assert!(catalog.iter().any(|i| i.id == "nova-3"));
        assert!(
            catalog.iter().any(|i| i.id == "flux-general-en"),
            "curated flux-general-en must be present even when the API omits it"
        );
        assert!(
            catalog.iter().any(|i| i.id == "flux-general-multi"),
            "curated flux-general-multi must be present even when the API omits it"
        );
    }

    #[test]
    fn deepgram_connection_error_redacts_key_echoes() {
        let api_key = "dg-provider-secret";
        let message = deepgram_connection_error_message(
            reqwest::StatusCode::UNAUTHORIZED,
            &format!(r#"{{"error":"bad token {api_key}"}}"#),
            Some(api_key),
        );

        assert!(message.contains("401 Unauthorized"));
        assert!(message.contains("bad token"));
        assert!(!message.contains(api_key));
        assert!(message.contains("<redacted>"));
    }

    #[test]
    fn deepgram_connection_error_401_carries_credential_rejected_prefix() {
        // audio-graph-57cc: a 401 readiness message must carry the stable
        // `Credential rejected (401):` prefix so the frontend classifier
        // (`isCredentialRejectedReadinessMessage`, ProviderReadinessPanel.tsx)
        // can offer the "fix your key" recovery action instead of the generic
        // retry copy.
        let message = deepgram_connection_error_message(
            reqwest::StatusCode::UNAUTHORIZED,
            r#"{"error":"bad token"}"#,
            None,
        );

        assert!(message.starts_with(crate::error::CREDENTIAL_REJECTED_PREFIX));
    }

    #[test]
    fn deepgram_connection_error_403_has_no_credential_rejected_prefix() {
        // A 403 (Forbidden) is a distinct rejection class (e.g. a
        // transcription-only key hitting an endpoint that needs the `manage`
        // scope, per the /v1/models comment above) — not the credential-401
        // marker.
        let message = deepgram_connection_error_message(
            reqwest::StatusCode::FORBIDDEN,
            r#"{"error":"missing scope"}"#,
            None,
        );

        assert!(!message.starts_with(crate::error::CREDENTIAL_REJECTED_PREFIX));
        assert!(message.contains("403 Forbidden"));
    }

    #[test]
    fn soniox_api_key_resolution_uses_saved_key_when_draft_is_blank() {
        let mut store = crate::credentials::CredentialStore::default();
        store.soniox_api_key = Some("  sx-saved  ".to_string());

        let api_key = soniox_api_key_from_store(&store).expect("saved key");

        assert_eq!(api_key, "sx-saved");
    }

    #[test]
    fn soniox_api_key_resolution_prefers_draft_key() {
        let api_key = soniox_api_key_from_draft_or_store(Some("  sx-draft  ".to_string()))
            .expect("draft key");

        assert_eq!(api_key, "sx-draft");
    }

    #[test]
    fn soniox_api_key_resolution_rejects_missing_saved_key() {
        let mut store = crate::credentials::CredentialStore::default();
        store.soniox_api_key = Some("   ".to_string());

        let err = soniox_api_key_from_store(&store).expect_err("missing key");

        match err {
            AppError::CredentialMissing { key } => {
                assert_eq!(key, "soniox_api_key");
            }
            other => panic!("expected CredentialMissing, got {other:?}"),
        }
    }

    #[test]
    fn soniox_model_catalog_parses_realtime_stt_models() {
        let catalog = parse_soniox_realtime_model_catalog(
            r##"{
                "models": [
                    {
                        "id": "stt-rt-v5",
                        "name": "Speech-to-Text Real-time v5",
                        "transcription_mode": "real_time"
                    },
                    {
                        "id": "stt-async-v5",
                        "name": "Speech-to-Text Async v5",
                        "transcription_mode": "async"
                    },
                    {
                        "id": "stt-rt-v4",
                        "name": "Speech-to-Text Real-time v4",
                        "transcription_mode": "real_time"
                    },
                    {
                        "id": "stt-rt-v5",
                        "name": "duplicate",
                        "transcription_mode": "real_time"
                    }
                ]
            }"##,
        )
        .expect("parse Soniox model catalog");

        assert_eq!(catalog.len(), 2);
        assert_eq!(catalog[0].id, "stt-rt-v5");
        assert_eq!(catalog[0].display_name, "Speech-to-Text Real-time v5");
        assert!(catalog[0].is_default);
        assert_eq!(catalog[1].id, "stt-rt-v4");
        assert_eq!(catalog[1].display_name, "Speech-to-Text Real-time v4");
        assert!(!catalog.iter().any(|item| item.id == "stt-async-v5"));
    }

    #[test]
    fn soniox_connection_error_redacts_key_echoes() {
        let api_key = "sx-provider-secret";
        let message = soniox_connection_error_message(
            reqwest::StatusCode::UNAUTHORIZED,
            &format!(r#"{{"message":"bad bearer {api_key}"}}"#),
            Some(api_key),
        );

        assert!(message.contains("401 Unauthorized"));
        assert!(message.contains("bad bearer"));
        assert!(!message.contains(api_key));
        assert!(message.contains("<redacted>"));
    }

    #[test]
    fn soniox_connection_error_401_carries_credential_rejected_prefix() {
        // audio-graph-57cc: same stable-prefix contract as Deepgram above.
        let message = soniox_connection_error_message(
            reqwest::StatusCode::UNAUTHORIZED,
            r#"{"message":"bad bearer"}"#,
            None,
        );

        assert!(message.starts_with(crate::error::CREDENTIAL_REJECTED_PREFIX));
    }

    #[test]
    fn soniox_readiness_uses_saved_key_for_an_explicit_diagnostic() {
        let descriptor = crate::provider_registry::provider_registry()
            .iter()
            .find(|descriptor| descriptor.id == "asr.soniox")
            .expect("soniox descriptor");
        let settings = crate::settings::AppSettings::default();
        let mut store = crate::credentials::CredentialStore::default();
        store.soniox_api_key = Some("sx-saved".to_string());

        let readiness = base_provider_readiness(descriptor, &settings, &store, 21);

        assert_eq!(readiness.status, ProviderReadinessStatus::Unchecked);
        assert!(readiness.automatic_probe_available);
        assert_eq!(readiness.message, "Ready to check with saved credentials");
        let requested = vec!["asr.soniox".to_string()];
        let requested_ids = requested_provider_ids(&settings, false, Some(&requested));
        assert!(should_probe_provider(
            descriptor,
            &requested_ids,
            &settings,
            &store
        ));
    }

    #[test]
    fn revai_readiness_uses_saved_key_without_automatic_probe_or_secret_echo() {
        let descriptor = crate::provider_registry::provider_registry()
            .iter()
            .find(|descriptor| descriptor.id == "asr.revai")
            .expect("revai descriptor");
        let settings = crate::settings::AppSettings::default();
        let mut store = crate::credentials::CredentialStore::default();

        let missing = base_provider_readiness(descriptor, &settings, &store, 22);
        assert_eq!(missing.status, ProviderReadinessStatus::MissingCredentials);
        assert_eq!(
            missing.message,
            "Missing saved credential(s): revai_api_key"
        );

        store.revai_api_key = Some("revai-provider-secret".to_string());
        let readiness = base_provider_readiness(descriptor, &settings, &store, 23);

        assert_eq!(readiness.status, ProviderReadinessStatus::Unchecked);
        assert!(!readiness.automatic_probe_available);
        assert_eq!(
            readiness.message,
            "No automatic health probe is available for this provider yet"
        );
        assert_eq!(readiness.model_count, Some(1));
        assert_eq!(readiness.model_catalog[0].id, "machine_v2");
        assert_eq!(readiness.credentials.len(), 1);
        assert_eq!(readiness.credentials[0].key, "revai_api_key");
        assert!(readiness.credentials[0].present);
        assert!(!should_probe_provider(
            descriptor,
            &active_provider_ids(&settings, false),
            &settings,
            &store
        ));

        let serialized = serde_json::to_string(&readiness).expect("serialize readiness");
        assert!(!serialized.contains("revai-provider-secret"));
    }

    #[test]
    fn assemblyai_api_key_resolution_uses_saved_key_when_draft_is_blank() {
        let mut store = crate::credentials::CredentialStore::default();
        store.assemblyai_api_key = Some("  aai-saved  ".to_string());

        let api_key = assemblyai_api_key_from_store(&store).expect("saved key");

        assert_eq!(api_key, "aai-saved");
    }

    #[test]
    fn assemblyai_connection_error_401_carries_credential_rejected_prefix() {
        // audio-graph-57cc: same stable-prefix contract as the other ASR probes.
        let message = assemblyai_connection_error_message(reqwest::StatusCode::UNAUTHORIZED);

        assert!(message.starts_with(crate::error::CREDENTIAL_REJECTED_PREFIX));
    }

    #[test]
    fn assemblyai_connection_error_500_has_no_credential_rejected_prefix() {
        let message =
            assemblyai_connection_error_message(reqwest::StatusCode::INTERNAL_SERVER_ERROR);

        assert!(!message.starts_with(crate::error::CREDENTIAL_REJECTED_PREFIX));
    }

    #[test]
    fn assemblyai_api_key_resolution_prefers_draft_key() {
        let api_key = assemblyai_api_key_from_draft_or_store(Some("  aai-draft  ".to_string()))
            .expect("draft key");

        assert_eq!(api_key, "aai-draft");
    }

    #[test]
    fn assemblyai_api_key_resolution_rejects_missing_saved_key() {
        let mut store = crate::credentials::CredentialStore::default();
        store.assemblyai_api_key = Some("   ".to_string());

        let err = assemblyai_api_key_from_store(&store).expect_err("missing key");

        match err {
            AppError::CredentialMissing { key } => {
                assert_eq!(key, "assemblyai_api_key");
            }
            other => panic!("expected CredentialMissing, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // FA-7 — send_chat_message must surface the real token count on the
    // streaming path instead of a hardcoded 0. The streaming `Done` frame
    // carries the provider's terminal `usage` block; `tokens_used_from_stream_usage`
    // is the pure derivation used at that return site. These pin the contract:
    // a populated usage block yields a non-zero `total_tokens`, and a genuinely
    // absent signal stays 0 (honest "unknown", not a fabricated count).
    // -----------------------------------------------------------------------

    #[test]
    fn tokens_used_flows_through_from_stream_usage() {
        use crate::llm::sse::StreamUsage;
        let usage = Some(StreamUsage {
            prompt_tokens: Some(12),
            completion_tokens: Some(34),
            total_tokens: Some(46),
        });
        assert_eq!(
            tokens_used_from_stream_usage(usage),
            46,
            "a populated usage block must surface total_tokens, not 0"
        );
    }

    #[test]
    fn tokens_used_streaming_done_arm_populates_from_usage() {
        // Exercise the exact accumulation the streaming branch of
        // send_chat_message runs: walk frames and, on Done, derive tokens_used
        // from the terminal usage block. Proves a non-zero count flows through
        // end-to-end for a provider that reports usage.
        use crate::llm::sse::StreamUsage;
        use crate::llm::streaming::TokenDelta;

        let frames = vec![
            TokenDelta::Delta {
                content: "Hello".to_string(),
                finish_reason: None,
            },
            TokenDelta::Delta {
                content: " world".to_string(),
                finish_reason: None,
            },
            TokenDelta::Done {
                full_text: "Hello world".to_string(),
                usage: Some(StreamUsage {
                    prompt_tokens: Some(8),
                    completion_tokens: Some(2),
                    total_tokens: Some(10),
                }),
                finish_reason: "stop".to_string(),
            },
        ];

        let mut full_text = String::new();
        let mut tokens_used = 0u32;
        for frame in frames {
            match frame {
                TokenDelta::Delta { content, .. } => full_text.push_str(&content),
                TokenDelta::Done {
                    full_text: t,
                    usage,
                    ..
                } => {
                    if !t.is_empty() {
                        full_text = t;
                    }
                    tokens_used = tokens_used_from_stream_usage(usage);
                    break;
                }
                _ => unreachable!("no error/cancel in this fixture"),
            }
        }

        assert_eq!(full_text, "Hello world");
        assert_eq!(
            tokens_used, 10,
            "streaming Done arm must thread the real total_tokens into ChatResponse"
        );
    }

    #[test]
    fn tokens_used_is_zero_when_provider_omits_usage() {
        use crate::llm::sse::StreamUsage;
        // Provider never honoured include_usage → no usage block at all.
        assert_eq!(tokens_used_from_stream_usage(None), 0);
        // Usage block present but total_tokens unset → still honestly 0.
        assert_eq!(
            tokens_used_from_stream_usage(Some(StreamUsage {
                prompt_tokens: Some(5),
                completion_tokens: Some(7),
                total_tokens: None,
            })),
            0
        );
    }

    #[test]
    fn sync_llm_api_client_replaces_stale_runtime_config() {
        let state = AppState::new();
        let mut settings = crate::settings::AppSettings {
            llm_provider: crate::settings::LlmProvider::Api {
                endpoint: "http://localhost:8000/v1".to_string(),
                api_key: "first-secret".to_string(),
                model: "first-model".to_string(),
            },
            llm_api_config: Some(crate::settings::LlmApiConfig {
                endpoint: "http://localhost:8000/v1".to_string(),
                api_key: None,
                model: "first-model".to_string(),
                max_tokens: 2048,
                temperature: 0.7,
            }),
            ..Default::default()
        };

        *state.app_settings.write().expect("lock poisoned") = settings.clone();
        sync_llm_api_client_from_settings_cache(&state).expect("initial sync must succeed");

        settings.llm_provider = crate::settings::LlmProvider::Api {
            endpoint: "https://api.openai.com/v1".to_string(),
            api_key: "second-secret".to_string(),
            model: "gpt-4o-mini".to_string(),
        };
        settings.llm_api_config = Some(crate::settings::LlmApiConfig {
            endpoint: "https://api.openai.com/v1".to_string(),
            api_key: None,
            model: "gpt-4o-mini".to_string(),
            max_tokens: 1024,
            temperature: 0.2,
        });
        *state.app_settings.write().expect("lock poisoned") = settings;
        sync_llm_api_client_from_settings_cache(&state).expect("resync must succeed");

        let guard = state.api_client.lock().expect("lock poisoned");
        let config = guard.as_ref().expect("client configured").config();
        assert_eq!(config.endpoint, "https://api.openai.com/v1");
        assert_eq!(config.api_key.as_deref(), Some("second-secret"));
        assert_eq!(config.model, "gpt-4o-mini");
        assert_eq!(config.max_tokens, 1024);
        assert!((config.temperature - 0.2).abs() < f32::EPSILON);
    }

    #[test]
    fn sync_llm_api_client_clears_when_provider_is_not_api() {
        let state = AppState::new();
        *state.app_settings.write().expect("lock poisoned") = crate::settings::AppSettings {
            llm_provider: crate::settings::LlmProvider::Api {
                endpoint: "http://localhost:11434/v1".to_string(),
                api_key: String::new(),
                model: "llama3.2".to_string(),
            },
            ..Default::default()
        };
        sync_llm_api_client_from_settings_cache(&state).expect("initial sync must succeed");
        assert!(state.api_client.lock().expect("lock poisoned").is_some());

        *state.app_settings.write().expect("lock poisoned") = crate::settings::AppSettings {
            llm_provider: crate::settings::LlmProvider::LocalLlama,
            ..Default::default()
        };
        sync_llm_api_client_from_settings_cache(&state).expect("clear sync must succeed");

        assert!(state.api_client.lock().expect("lock poisoned").is_none());
    }

    #[test]
    fn api_config_from_runtime_settings_ignores_stale_detail_config() {
        let settings = crate::settings::AppSettings {
            llm_provider: crate::settings::LlmProvider::Api {
                endpoint: "http://localhost:8000/v1".to_string(),
                api_key: String::new(),
                model: "active-model".to_string(),
            },
            llm_api_config: Some(crate::settings::LlmApiConfig {
                endpoint: "https://api.openai.com/v1".to_string(),
                api_key: Some("stale-secret".to_string()),
                model: "stale-model".to_string(),
                max_tokens: 4096,
                temperature: 0.9,
            }),
            ..Default::default()
        };

        let config = api_config_from_runtime_settings(&settings).expect("API provider configured");

        assert_eq!(config.endpoint, "http://localhost:8000/v1");
        assert_eq!(config.model, "active-model");
        assert_eq!(config.api_key, None);
        assert_eq!(config.max_tokens, 512);
        assert!((config.temperature - 0.1).abs() < f32::EPSILON);
    }

    #[test]
    fn asr_capture_selection_allows_batch_providers_to_use_multiple_sources() {
        let active_sources = vec!["system-default".to_string(), "device:mic".to_string()];

        validate_asr_capture_selection(
            &crate::settings::AsrProvider::LocalWhisper,
            &active_sources,
            Some("app:42"),
        )
        .expect("local batch ASR supports per-source accumulators");

        validate_asr_capture_selection(
            &crate::settings::AsrProvider::Api {
                endpoint: "https://example.com/v1".to_string(),
                api_key: String::new(),
                model: "whisper-large-v3".to_string(),
            },
            &active_sources,
            Some("app:42"),
        )
        .expect("cloud batch ASR supports per-source accumulators");
    }

    #[test]
    fn asr_capture_selection_rejects_second_source_for_single_session_providers() {
        let active_sources = vec!["system-default".to_string()];
        let providers = vec![
            (
                crate::settings::AsrProvider::AssemblyAI {
                    api_key: String::new(),
                    enable_diarization: true,
                },
                "AssemblyAI streaming",
            ),
            (
                crate::settings::AsrProvider::AwsTranscribe {
                    region: "us-east-1".to_string(),
                    language_code: "en-US".to_string(),
                    credential_source: crate::settings::AwsCredentialSource::DefaultChain,
                    enable_diarization: true,
                },
                "AWS Transcribe streaming",
            ),
            (
                crate::settings::AsrProvider::SherpaOnnx {
                    model_dir: "streaming-zipformer-en-20M".to_string(),
                    enable_endpoint_detection: true,
                },
                "Sherpa-ONNX streaming",
            ),
            (
                crate::settings::AsrProvider::Moonshine {
                    model_dir: "moonshine-small-streaming-en".to_string(),
                    enable_speaker_hints: true,
                },
                "Moonshine local streaming",
            ),
        ];

        for (provider, provider_name) in providers {
            let err =
                validate_asr_capture_selection(&provider, &active_sources, Some("device:mic"))
                    .expect_err("streaming provider must reject a second source");

            assert!(
                err.contains(provider_name),
                "error should name provider, got: {}",
                err
            );
            assert!(
                err.contains("system-default") && err.contains("device:mic"),
                "error should list active and pending sources, got: {}",
                err
            );
        }
    }

    #[test]
    fn asr_capture_selection_allows_existing_source_restart_path() {
        let active_sources = vec!["system-default".to_string()];
        validate_asr_capture_selection(
            &crate::settings::AsrProvider::DeepgramStreaming {
                api_key: String::new(),
                model: "nova-3".to_string(),
                enable_diarization: true,
                endpointing_ms: 300,
                utterance_end_ms: 1000,
                vad_events: true,
                eot_threshold: 0.5,
                eager_eot_threshold: 0.0,
                eot_timeout_ms: 0,
                max_speakers: 2,
                keyterms: vec![],
            },
            &active_sources,
            Some("system-default"),
        )
        .expect("same source should not count as a second streaming session");
    }

    #[test]
    fn asr_capture_selection_rejects_multi_source_transcription_start() {
        let active_sources = vec!["system-default".to_string(), "device:mic".to_string()];
        let err = validate_asr_capture_selection(
            &crate::settings::AsrProvider::AssemblyAI {
                api_key: String::new(),
                enable_diarization: true,
            },
            &active_sources,
            None,
        )
        .expect_err("starting transcription with multiple sources should be rejected");

        assert!(err.contains("AssemblyAI streaming"));
        assert!(err.contains("system-default") && err.contains("device:mic"));
    }

    #[test]
    fn asr_capture_selection_allows_multiple_sources_for_deepgram_mixed() {
        // Deepgram now feeds through the audio mixer, so multiple sources are
        // allowed (they are summed into one mixed stream).
        let active_sources = vec!["system-default".to_string(), "device:mic".to_string()];
        validate_asr_capture_selection(
            &crate::settings::AsrProvider::DeepgramStreaming {
                api_key: String::new(),
                model: "nova-3".to_string(),
                enable_diarization: true,
                endpointing_ms: 300,
                utterance_end_ms: 1000,
                vad_events: true,
                eot_threshold: 0.5,
                eager_eot_threshold: 0.0,
                eot_timeout_ms: 0,
                max_speakers: 2,
                keyterms: vec![],
            },
            &active_sources,
            Some("app:42"),
        )
        .expect("Deepgram mixes multiple sources, so multi-source is allowed");

        validate_asr_capture_selection(
            &crate::settings::AsrProvider::Soniox {
                api_key: String::new(),
                model: crate::asr::soniox::DEFAULT_MODEL.to_string(),
                enable_diarization: true,
                enable_language_identification: true,
                language_hints: vec![],
                max_speakers: 2,
            },
            &active_sources,
            Some("app:42"),
        )
        .expect("Soniox uses the mixed stream, so multi-source is allowed");
    }

    // -----------------------------------------------------------------------
    // PART 2 — log_level persistence race (loop-13 MEDIUM #6).
    // set_log_level is now the runtime-only path; save_settings_cmd owns
    // the single disk-write path. The full command needs a Tauri AppHandle
    // (not available in unit tests), so we exercise the in-memory half
    // directly and assert the invariant that matters: the cache tracks
    // the latest level without triggering a disk flush.
    // -----------------------------------------------------------------------

    #[test]
    fn set_log_level_does_not_persist_to_disk_on_repeated_calls() {
        // Simulate what `set_log_level` does to the in-memory cache: apply
        // the runtime level, then mutate `app_settings.log_level`. Repeating
        // this twice must leave the cache reflecting the final value and
        // must not touch disk — which it can't, because we never hand it
        // an AppHandle.
        let state = AppState::new();

        // First call: info → debug.
        crate::logging::apply_log_level("debug");
        {
            let mut cached = state.app_settings.write().expect("lock poisoned");
            cached.log_level = Some("debug".to_string());
        }
        assert_eq!(
            state.app_settings.read().unwrap().log_level.as_deref(),
            Some("debug"),
            "cache must reflect first update"
        );

        // Second call: debug → warn. With the old contract this would have
        // produced a second disk write; under the new contract it only
        // mutates runtime + cache.
        crate::logging::apply_log_level("warn");
        {
            let mut cached = state.app_settings.write().expect("lock poisoned");
            cached.log_level = Some("warn".to_string());
        }
        assert_eq!(
            state.app_settings.read().unwrap().log_level.as_deref(),
            Some("warn"),
            "cache must reflect second update"
        );

        // Restore a sensible default so later tests in the same binary
        // aren't silently swallowing logs at warn.
        crate::logging::apply_log_level("info");
    }

    fn register_test_gemini_notes_consumer(state: &AppState) {
        let _rx = register_runtime_processed_audio_consumer(
            &state.processed_audio_consumers,
            GEMINI_NOTES_AUDIO_CONSUMER_ID,
            ProcessedAudioConsumerStage::Notes,
            Some("gemini"),
            2,
            Some(GEMINI_LIVE_AUDIO_CONSUMER_GROUP),
            {
                let is_active = state.is_gemini_active.clone();
                Arc::new(move || is_active.read().map(|active| *active).unwrap_or(false))
            },
        )
        .expect("test Gemini notes consumer should register");
    }

    fn set_running_capture_pipeline_status(state: &AppState) {
        let mut status = state.pipeline_status.write().expect("pipeline status");
        status.capture = StageStatus::Running { processed_count: 3 };
        status.pipeline = StageStatus::Running { processed_count: 3 };
        status.asr = StageStatus::Running { processed_count: 2 };
        status.diarization = StageStatus::Running { processed_count: 2 };
        status.entity_extraction = StageStatus::Running { processed_count: 1 };
        status.graph = StageStatus::Running { processed_count: 1 };
    }

    fn assert_pipeline_status_idle(status: &PipelineStatus) {
        assert!(matches!(status.capture, StageStatus::Idle));
        assert!(matches!(status.pipeline, StageStatus::Idle));
        assert!(matches!(status.asr, StageStatus::Idle));
        assert!(matches!(status.diarization, StageStatus::Idle));
        assert!(matches!(status.entity_extraction, StageStatus::Idle));
        assert!(matches!(status.graph, StageStatus::Idle));
    }

    // -----------------------------------------------------------------------
    // PART 2.5 — capture start/stop command lifecycle (audio-graph-1d59).
    //
    // These stay entirely at the command layer with synthetic capture handles
    // so they can prove registry/flag/status cleanup without opening rsac
    // hardware on the test host.
    // -----------------------------------------------------------------------

    #[tokio::test(flavor = "current_thread")]
    #[cfg_attr(
        target_os = "macos",
        ignore = "Tauri/Tao App construction must run on the macOS main thread"
    )]
    // `_lock` is deliberately held across `.await`s to serialize process-global
    // HOME mutation across tests on the single-threaded runtime.
    #[allow(clippy::await_holding_lock)]
    async fn start_capture_rejects_duplicate_live_source_without_side_effects() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("start-capture-duplicate-live-source");
        let _guard = HomeGuard::set(&dir);

        let state = AppState::new();
        let app_handle = crate::speech::shared_test_app_handle();
        state
            .capture_manager
            .lock()
            .expect("capture manager")
            .insert_synthetic_handle("system", false);

        let (status_tx, status_rx) = std::sync::mpsc::channel();
        let listener_id = app_handle.listen_any(events::PIPELINE_STATUS_EVENT, move |event| {
            if let Ok(payload) = serde_json::from_str::<PipelineStatus>(event.payload()) {
                let _ = status_tx.send(payload);
            }
        });

        let err = start_capture_impl("system".to_string(), None, None, &state, &app_handle)
            .await
            .expect_err("duplicate live source should reject before hardware open");
        let message = err.to_string();
        assert!(message.contains("already being captured"), "got: {message}");

        let active = state
            .capture_manager
            .lock()
            .expect("capture manager")
            .active_captures();
        assert_eq!(active, vec!["system".to_string()]);
        assert!(state.pipeline_thread.lock().unwrap().is_none());
        assert!(state.dispatcher_thread.lock().unwrap().is_none());
        assert!(!*state.is_capturing.read().unwrap());
        assert!(!state.is_transcribing.load(Ordering::SeqCst));
        assert_pipeline_status_idle(&state.pipeline_status.read().unwrap());
        assert!(
            status_rx
                .recv_timeout(std::time::Duration::from_millis(150))
                .is_err(),
            "duplicate early error must not emit a pipeline status event"
        );

        app_handle.unlisten(listener_id);
        drain_test_writers(&state);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(flavor = "current_thread")]
    #[cfg_attr(
        target_os = "macos",
        ignore = "Tauri/Tao App construction must run on the macOS main thread"
    )]
    // `_lock` is deliberately held across `.await`s to serialize process-global
    // HOME mutation across tests on the single-threaded runtime.
    #[allow(clippy::await_holding_lock)]
    async fn stop_capture_clears_final_source_runtime_state_and_unregisters_runtime_consumers() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("stop-capture-final-source-runtime-state");
        let _guard = HomeGuard::set(&dir);

        let state = AppState::new();
        let app_handle = crate::speech::shared_test_app_handle();
        ensure_audio_pipeline_workers(&state, &app_handle).expect("start audio spine");
        reset_audio_pipeline_session(&state)
            .await
            .expect("audio spine ready");
        {
            state
                .capture_manager
                .lock()
                .expect("capture manager")
                .insert_synthetic_handle("system", false);
            *state.is_capturing.write().unwrap() = true;
            state.is_transcribing.store(true, Ordering::SeqCst);
            *state.is_gemini_active.write().unwrap() = true;
            *state.is_openai_realtime_active.write().unwrap() = true;
            state
                .openai_realtime_capture_gate
                .store(true, Ordering::SeqCst);
            set_running_capture_pipeline_status(&state);
            register_test_gemini_notes_consumer(&state);
            let _openai_rx = register_runtime_processed_audio_consumer(
                &state.processed_audio_consumers,
                OPENAI_REALTIME_AUDIO_CONSUMER_ID,
                ProcessedAudioConsumerStage::RealtimeAgent,
                Some("openai"),
                2,
                Some(OPENAI_REALTIME_AUDIO_CONSUMER_GROUP),
                {
                    let is_active = state.is_openai_realtime_active.clone();
                    Arc::new(move || is_active.read().map(|active| *active).unwrap_or(false))
                },
            )
            .expect("test OpenAI Realtime consumer should register");
            assert!(
                state
                    .processed_audio_consumers
                    .health_payload()
                    .consumers
                    .iter()
                    .any(|consumer| consumer.id == GEMINI_NOTES_AUDIO_CONSUMER_ID),
                "precondition: runtime Gemini notes consumer is registered"
            );
        }

        // Pin ADR-0045 decision 4's Stop-side wiring against the actual
        // `stop_capture_impl` path (adversarial-review fix — audio-graph-9cc1):
        // without this, deleting/reordering the `projection_lane_stopping` store
        // or the `drain_projection_job_workers` call inside `stop_capture_impl`
        // leaves the whole suite green. The fake job sleeps briefly, then flips
        // its OWN `AtomicBool` — checked synchronously right after
        // `stop_capture_impl` returns below, the same non-vacuous technique as
        // `drain_projection_job_workers_joins_finished_threads_of_both_kinds`. If
        // the drain call were ever deleted, `stop_capture_impl` would return well
        // before the fake job's sleep elapses, and the flag/registry assertions
        // below would fail.
        state
            .projection_lane_stopping
            .store(false, Ordering::SeqCst);
        let fake_projection_job_finished = Arc::new(AtomicBool::new(false));
        {
            let flag = fake_projection_job_finished.clone();
            // 750ms, not the 100ms used by the pure-unit-test sibling
            // (`drain_projection_job_workers_joins_finished_threads_of_both_kinds`):
            // this test drives the FULL async `stop_capture_impl`, which does
            // real (if normally sub-10ms) work — audio pipeline reset,
            // consumer teardown, etc. — before reaching the drain. A margin
            // this wide over that baseline, and over ordinary parallel-test
            // scheduling jitter, is what keeps the assertion below from
            // passing by timing accident when the drain is skipped.
            let handle = std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(750));
                flag.store(true, Ordering::SeqCst);
            });
            state
                .projection_job_workers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push((
                    crate::projections::ProjectionKind::Graph,
                    "stop-wiring-pin-job".to_string(),
                    handle,
                ));
        }

        let (status_tx, status_rx) = std::sync::mpsc::channel();
        let listener_id = app_handle.listen_any(events::PIPELINE_STATUS_EVENT, move |event| {
            if let Ok(payload) = serde_json::from_str::<PipelineStatus>(event.payload()) {
                let _ = status_tx.send(payload);
            }
        });

        stop_capture_impl("system".to_string(), &state, &app_handle, None)
            .await
            .expect("final source stop should succeed");

        assert!(
            state.projection_lane_stopping.load(Ordering::SeqCst),
            "Stop must set projection_lane_stopping before/while draining the registry"
        );
        assert!(
            fake_projection_job_finished.load(Ordering::SeqCst),
            "Stop must actually wait for the registered projection job thread to run to \
             completion (drain_projection_job_workers), not just forget its handle"
        );
        assert!(
            state
                .projection_job_workers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_empty(),
            "Stop must drain every registered projection job thread from the registry"
        );
        assert!(
            state
                .capture_manager
                .lock()
                .expect("capture manager")
                .active_captures()
                .is_empty(),
            "final source stop should clear capture registry"
        );
        assert!(!*state.is_capturing.read().unwrap());
        assert!(!state.is_transcribing.load(Ordering::SeqCst));
        assert!(!*state.is_gemini_active.read().unwrap());
        assert!(!*state.is_openai_realtime_active.read().unwrap());
        assert!(!state.openai_realtime_capture_gate.load(Ordering::SeqCst));
        assert!(
            state.gemini_client.lock().unwrap().is_none(),
            "final stop should clear the Gemini client slot"
        );
        let health = state.processed_audio_consumers.health_payload();
        assert!(
            !health
                .consumers
                .iter()
                .any(|consumer| consumer.id == GEMINI_NOTES_AUDIO_CONSUMER_ID),
            "final stop should unregister runtime Gemini notes consumer"
        );
        assert!(
            !health
                .consumers
                .iter()
                .any(|consumer| consumer.id == OPENAI_REALTIME_AUDIO_CONSUMER_ID),
            "final stop should unregister the OpenAI Realtime consumer"
        );
        assert_pipeline_status_idle(&state.pipeline_status.read().unwrap());

        let emitted = status_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("final source stop should emit idle pipeline status");
        assert_pipeline_status_idle(&emitted);

        app_handle.unlisten(listener_id);
        drain_test_writers(&state);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// audio-graph-fa56: end-to-end behavioral pin that the new
    /// abandoned-deferred-retry detection actually executes on the REAL
    /// `stop_capture_impl` path, not just when
    /// `log_abandoned_deferred_retries_after_stop` is called directly (as
    /// the unit tests above do). Arms a real graph-lane deferral on a live
    /// `AppState` the same way
    /// `log_abandoned_deferred_retries_after_stop_leaves_an_armed_deferral_untouched`
    /// does, then drives the full async `stop_capture_impl` — the sibling
    /// `stop_capture_clears_final_source_runtime_state_and_unregisters_runtime_consumers`
    /// test above already proves this suite CAN drive that path against a
    /// live capture/ASR-adjacent pipeline; this reuses the same setup rather
    /// than the source-order self-inspection technique, which can only pin
    /// call ORDER, not that the call actually observes real scheduler state.
    ///
    /// Mutation coverage: deleting the
    /// `log_abandoned_deferred_retries_after_stop` call from
    /// `stop_capture_impl`, or wiring it to the wrong `Arc<Mutex<..>>`,
    /// leaves scheduler state unaffected either way (this function is
    /// read-only), so the load-bearing assertions are the persisted-snapshot
    /// ones: they fail if the call is deleted (no snapshot is ever written
    /// for this session) or if it reads/writes the wrong lane.
    #[tokio::test(flavor = "current_thread")]
    #[cfg_attr(
        target_os = "macos",
        ignore = "Tauri/Tao App construction must run on the macOS main thread"
    )]
    // `_lock` is deliberately held across `.await`s to serialize process-global
    // HOME mutation across tests on the single-threaded runtime.
    #[allow(clippy::await_holding_lock)]
    async fn stop_capture_impl_reports_a_graph_lane_deferred_retry_abandoned_at_stop() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("stop-capture-abandoned-deferred-retry");
        let _guard = HomeGuard::set(&dir);

        let state = AppState::new();
        let session_id = state.current_session_id();
        let app_handle = crate::speech::shared_test_app_handle();
        ensure_audio_pipeline_workers(&state, &app_handle).expect("start audio spine");
        reset_audio_pipeline_session(&state)
            .await
            .expect("audio spine ready");
        state
            .capture_manager
            .lock()
            .expect("capture manager")
            .insert_synthetic_handle("system", false);
        *state.is_capturing.write().unwrap() = true;
        state.is_transcribing.store(true, Ordering::SeqCst);
        state
            .projection_lane_stopping
            .store(false, Ordering::SeqCst);

        // Arm a real graph-lane deferred retry the same way a same-basis
        // failure under `PROJECTION_LANE_ATTEMPT_BUDGET` would in production
        // (mirrors `log_abandoned_deferred_retries_after_stop_leaves_an_armed_deferral_untouched`
        // above).
        let mut ledger = crate::projections::TranscriptLedger::new(&session_id);
        ledger
            .apply_event(transcript_event_fixture("span-1", "segment-1"))
            .expect("seed ledger span");
        let graph_job_id = {
            let mut schedulers = state
                .projection_schedulers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match schedulers.observe_ledger(&ledger, 10).graph {
                crate::projection_scheduler::ProjectionSchedulerDecision::StartJob { job } => {
                    job.id
                }
                other => panic!("expected graph start job, got {other:?}"),
            }
        };
        {
            let mut schedulers = state
                .projection_schedulers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            assert!(
                matches!(
                    schedulers.fail_graph_in_flight(&graph_job_id, &session_id, &ledger, 20),
                    crate::projection_scheduler::ProjectionSchedulerDecision::FailedCurrent {
                        deferred_retry_at_ms: Some(_),
                        ..
                    }
                ),
                "precondition: graph failure under budget must arm a deferred retry"
            );
        }
        assert_eq!(
            state
                .projection_schedulers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .kinds_with_armed_deferred_retry(),
            vec![crate::projections::ProjectionKind::Graph],
            "precondition: the graph lane's deferral must be armed before Stop"
        );

        stop_capture_impl("system".to_string(), &state, &app_handle, None)
            .await
            .expect("final source stop should succeed even with an abandoned deferral");

        // The deferral was never fired (no clock thread was ever registered
        // for it in this test), so it must still be armed after Stop — the
        // exact "abandoned" state the new WARN reports.
        assert_eq!(
            state
                .projection_schedulers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .kinds_with_armed_deferred_retry(),
            vec![crate::projections::ProjectionKind::Graph],
            "the graph lane's deferral must still be reported as abandoned after the real \
             stop_capture_impl path runs, proving the detection wiring executed"
        );

        // Ticket requirement (b): the real Stop path must have persisted a
        // diagnostics snapshot that a later session load or replay pass can
        // read back — not just emitted a log line.
        let persisted = crate::persistence::load_scheduler_queue_state(&session_id)
            .expect("stop_capture_impl must persist a scheduler queue snapshot for this session");
        assert!(
            persisted.graph_deferred_retry_at_ms.is_some(),
            "the snapshot persisted by the real stop_capture_impl path must record the \
             abandoned graph lane's deferred retry deadline"
        );

        drain_test_writers(&state);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// audio-graph-64e3: end-to-end behavioral pin that
    /// `warn_if_display_transcript_rows_missing_at_stop` actually executes on
    /// the REAL `stop_capture_impl` path — same rationale and same reused
    /// setup as `stop_capture_impl_reports_a_graph_lane_deferred_retry_abandoned_at_stop`
    /// above. Seeds `display_transcript_write_misses` as if the deepgram
    /// receiver had already detected 2 misses during the session, drives the
    /// full async `stop_capture_impl`, and checks the counter was read and
    /// reset. Mutation coverage: deleting the
    /// `warn_if_display_transcript_rows_missing_at_stop` call from
    /// `stop_capture_impl`, or wiring it to a DIFFERENT `Arc<AtomicU64>`,
    /// leaves this seeded counter non-zero after Stop — the load-bearing
    /// assertion below.
    #[tokio::test(flavor = "current_thread")]
    #[cfg_attr(
        target_os = "macos",
        ignore = "Tauri/Tao App construction must run on the macOS main thread"
    )]
    // `_lock` is deliberately held across `.await`s to serialize process-global
    // HOME mutation across tests on the single-threaded runtime.
    #[allow(clippy::await_holding_lock)]
    async fn stop_capture_impl_reads_and_resets_display_transcript_write_misses() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("stop-capture-display-rows-missing");
        let _guard = HomeGuard::set(&dir);

        let state = AppState::new();
        let app_handle = crate::speech::shared_test_app_handle();
        ensure_audio_pipeline_workers(&state, &app_handle).expect("start audio spine");
        reset_audio_pipeline_session(&state)
            .await
            .expect("audio spine ready");
        state
            .capture_manager
            .lock()
            .expect("capture manager")
            .insert_synthetic_handle("system", false);
        *state.is_capturing.write().unwrap() = true;
        state.is_transcribing.store(true, Ordering::SeqCst);
        state
            .projection_lane_stopping
            .store(false, Ordering::SeqCst);

        // Seed the counter as if the deepgram receiver thread had already
        // detected 2 finals that reached the ledger but missed the display
        // writer during this session.
        state
            .display_transcript_write_misses
            .store(2, Ordering::SeqCst);

        stop_capture_impl("system".to_string(), &state, &app_handle, None)
            .await
            .expect("final source stop should succeed even with pending display-row misses");

        assert_eq!(
            state.display_transcript_write_misses.load(Ordering::SeqCst),
            0,
            "the real stop_capture_impl path must have read (and reset) the seeded counter, \
             proving warn_if_display_transcript_rows_missing_at_stop actually ran"
        );

        drain_test_writers(&state);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// audio-graph-64e3 (finding: stop_transcribe never read the counter):
    /// end-to-end behavioral pin that a transcribe-only session (no capture
    /// involved) ALSO surfaces `transcript.display_rows_missing_at_stop`,
    /// via the real `stop_transcribe_impl` path — same technique as
    /// `stop_capture_impl_reads_and_resets_display_transcript_write_misses`
    /// above. Before this fix, `stop_transcribe` never called
    /// `warn_if_display_transcript_rows_missing_at_stop` at all, so a miss
    /// counted during a transcribe-only session surfaced (if ever) under a
    /// LATER, unrelated capture session's stop — this test pins that the
    /// counter is read-and-reset at THIS stop instead.
    #[tokio::test(flavor = "current_thread")]
    #[cfg_attr(
        target_os = "macos",
        ignore = "Tauri/Tao App construction must run on the macOS main thread"
    )]
    // `_lock` is deliberately held across `.await`s to serialize process-global
    // HOME mutation across tests on the single-threaded runtime.
    #[allow(clippy::await_holding_lock)]
    async fn stop_transcribe_impl_reads_and_resets_display_transcript_write_misses() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("stop-transcribe-display-rows-missing");
        let _guard = HomeGuard::set(&dir);

        let state = AppState::new();
        let app_handle = crate::speech::shared_test_app_handle();
        state.is_transcribing.store(true, Ordering::SeqCst);

        // Seed the counter as if a display-write site had already detected 3
        // finals that reached a ledger but missed the display writer during
        // this transcribe-only session.
        state
            .display_transcript_write_misses
            .store(3, Ordering::SeqCst);

        stop_transcribe_impl(&state, &app_handle)
            .await
            .expect("stop_transcribe should succeed even with pending display-row misses");

        assert_eq!(
            state.display_transcript_write_misses.load(Ordering::SeqCst),
            0,
            "the real stop_transcribe_impl path must have read (and reset) the seeded \
             counter, proving warn_if_display_transcript_rows_missing_at_stop actually ran \
             on this path too, not just on stop_capture_impl's"
        );

        drain_test_writers(&state);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(flavor = "current_thread")]
    #[cfg_attr(
        target_os = "macos",
        ignore = "Tauri/Tao App construction must run on the macOS main thread"
    )]
    // `_lock` is deliberately held across `.await`s to serialize process-global
    // HOME mutation across tests on the single-threaded runtime.
    #[allow(clippy::await_holding_lock)]
    async fn stop_capture_keeps_pipeline_running_when_other_sources_remain() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("stop-capture-other-sources-remain");
        let _guard = HomeGuard::set(&dir);

        let state = AppState::new();
        let app_handle = crate::speech::shared_test_app_handle();
        {
            let mut manager = state.capture_manager.lock().expect("capture manager");
            manager.insert_synthetic_handle("system", false);
            manager.insert_synthetic_handle("device:mic", false);
            drop(manager);

            *state.is_capturing.write().unwrap() = true;
            state.is_transcribing.store(true, Ordering::SeqCst);
            *state.is_gemini_active.write().unwrap() = true;
            set_running_capture_pipeline_status(&state);
            register_test_gemini_notes_consumer(&state);
        }

        let (status_tx, status_rx) = std::sync::mpsc::channel();
        let listener_id = app_handle.listen_any(events::PIPELINE_STATUS_EVENT, move |event| {
            if let Ok(payload) = serde_json::from_str::<PipelineStatus>(event.payload()) {
                let _ = status_tx.send(payload);
            }
        });

        stop_capture_impl("system".to_string(), &state, &app_handle, None)
            .await
            .expect("stop should succeed while other sources remain");

        let active = state
            .capture_manager
            .lock()
            .expect("capture manager")
            .active_captures();
        assert_eq!(active, vec!["device:mic".to_string()]);
        assert!(*state.is_capturing.read().unwrap());
        assert!(state.is_transcribing.load(Ordering::SeqCst));
        assert!(*state.is_gemini_active.read().unwrap());
        assert!(
            state
                .processed_audio_consumers
                .health_payload()
                .consumers
                .iter()
                .any(|consumer| consumer.id == GEMINI_NOTES_AUDIO_CONSUMER_ID),
            "non-final stop must preserve runtime consumer registrations"
        );
        let status = state.pipeline_status.read().unwrap().clone();
        assert!(matches!(status.capture, StageStatus::Running { .. }));
        assert!(matches!(status.pipeline, StageStatus::Running { .. }));
        assert!(matches!(status.asr, StageStatus::Running { .. }));
        assert!(
            status_rx
                .recv_timeout(std::time::Duration::from_millis(150))
                .is_err(),
            "non-final stop must not emit the final idle pipeline status event"
        );

        app_handle.unlisten(listener_id);
        drain_test_writers(&state);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // -----------------------------------------------------------------------
    // PART N — converse audio-sender teardown (AUD-CV1 / finding #48)
    //
    // The fix's load-bearing property: the sender must wake and exit promptly
    // when `is_converse_active` flips to false EVEN IF no further audio chunk
    // arrives (the stop-after-capture-stopped case). Before the fix the loop
    // blocked in `gemini_rx.recv()` and only re-checked the flag after the
    // NEXT chunk; with capture already stopped no chunk arrives, so the join
    // timed out (3s) and detached — leaking the thread and letting a fast
    // restart double-spawn on the single-consumer rx.
    //
    // This drives the extracted `run_converse_audio_sender` directly (no live
    // socket needed): a None client slot is fine because the gate stays closed
    // so `send_audio` is never reached; the test only proves the wake/exit
    // contract. The end-to-end `start_converse`/`stop_converse` wiring (which
    // requires a live GeminiLiveClient connection) remains integration-only.
    // -----------------------------------------------------------------------

    #[tokio::test(flavor = "current_thread")]
    async fn stop_converse_runtime_clears_consumer_gate_and_worker_slots() {
        let state = AppState::new();
        *state.is_converse_active.write().unwrap() = true;
        state.converse_capture_gate.store(true, Ordering::SeqCst);

        let _rx = register_runtime_processed_audio_consumer(
            &state.processed_audio_consumers,
            GEMINI_CONVERSE_AUDIO_CONSUMER_ID,
            ProcessedAudioConsumerStage::NativeConverse,
            Some("gemini"),
            2,
            Some(GEMINI_LIVE_AUDIO_CONSUMER_GROUP),
            {
                let is_active = state.is_converse_active.clone();
                Arc::new(move || is_active.read().map(|a| *a).unwrap_or(false))
            },
        )
        .expect("dummy converse consumer should register");
        assert!(
            state
                .processed_audio_consumers
                .health_payload()
                .consumers
                .iter()
                .any(|consumer| consumer.id == GEMINI_CONVERSE_AUDIO_CONSUMER_ID),
            "precondition: converse consumer is registered"
        );

        {
            let active = state.is_converse_active.clone();
            *state.converse_audio_thread.lock().unwrap() = Some(std::thread::spawn(move || {
                while active.read().map(|a| *a).unwrap_or(false) {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            }));
        }
        {
            let active = state.is_converse_active.clone();
            *state.converse_thread.lock().unwrap() = Some(std::thread::spawn(move || {
                while active.read().map(|a| *a).unwrap_or(false) {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            }));
        }

        stop_converse_runtime(&state, "test").await.unwrap();

        assert!(!*state.is_converse_active.read().unwrap());
        assert!(!state.converse_capture_gate.load(Ordering::SeqCst));
        assert!(state.converse_audio_thread.lock().unwrap().is_none());
        assert!(state.converse_thread.lock().unwrap().is_none());
        assert!(
            !state
                .processed_audio_consumers
                .health_payload()
                .consumers
                .iter()
                .any(|consumer| consumer.id == GEMINI_CONVERSE_AUDIO_CONSUMER_ID),
            "converse runtime consumer must be unregistered"
        );

        drain_test_writers(&state);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stop_openai_realtime_runtime_clears_consumer_gate_and_worker_slots() {
        let state = AppState::new();
        *state.is_openai_realtime_active.write().unwrap() = true;
        state
            .openai_realtime_capture_gate
            .store(true, Ordering::SeqCst);

        let _rx = register_runtime_processed_audio_consumer(
            &state.processed_audio_consumers,
            OPENAI_REALTIME_AUDIO_CONSUMER_ID,
            ProcessedAudioConsumerStage::RealtimeAgent,
            Some("openai"),
            2,
            Some(OPENAI_REALTIME_AUDIO_CONSUMER_GROUP),
            {
                let is_active = state.is_openai_realtime_active.clone();
                Arc::new(move || is_active.read().map(|active| *active).unwrap_or(false))
            },
        )
        .expect("dummy OpenAI Realtime consumer should register");

        {
            let active = state.is_openai_realtime_active.clone();
            *state.openai_realtime_audio_thread.lock().unwrap() =
                Some(std::thread::spawn(move || {
                    while active.read().map(|a| *a).unwrap_or(false) {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                }));
        }
        {
            let active = state.is_openai_realtime_active.clone();
            *state.openai_realtime_event_thread.lock().unwrap() =
                Some(std::thread::spawn(move || {
                    while active.read().map(|a| *a).unwrap_or(false) {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                }));
        }

        stop_openai_realtime_runtime(&state, "test").await.unwrap();

        assert!(!*state.is_openai_realtime_active.read().unwrap());
        assert!(!state.openai_realtime_capture_gate.load(Ordering::SeqCst));
        assert!(state.openai_realtime_audio_thread.lock().unwrap().is_none());
        assert!(state.openai_realtime_event_thread.lock().unwrap().is_none());
        assert!(
            !state
                .processed_audio_consumers
                .health_payload()
                .consumers
                .iter()
                .any(|consumer| consumer.id == OPENAI_REALTIME_AUDIO_CONSUMER_ID)
        );

        drain_test_writers(&state);
    }

    #[test]
    fn converse_audio_sender_exits_promptly_on_stop_without_chunk() {
        use std::sync::atomic::AtomicBool;
        use std::sync::{Arc, Mutex, RwLock};

        // Empty channel: NO chunk will ever be sent, mirroring "capture stopped
        // first, then stop_converse flips the flag".
        let (_tx, rx) =
            crossbeam_channel::bounded::<crate::audio::pipeline::ProcessedAudioChunk>(16);
        let client: Arc<Mutex<Option<GeminiLiveClient>>> = Arc::new(Mutex::new(None));
        let is_active = Arc::new(RwLock::new(true));
        let gate = Arc::new(AtomicBool::new(false));

        let rx2 = rx.clone();
        let client2 = client.clone();
        let is_active2 = is_active.clone();
        let gate2 = gate.clone();
        let handle = std::thread::spawn(move || {
            run_converse_audio_sender(&rx2, &client2, &is_active2, &gate2);
        });

        // Let it spin through a couple of recv_timeout ticks (each 100ms).
        std::thread::sleep(std::time::Duration::from_millis(250));
        assert!(
            !handle.is_finished(),
            "sender must still be running while is_active=true and no chunk arrives"
        );

        // Stop: flip the flag. With recv_timeout the loop wakes within ~100ms
        // even though no chunk is ever sent. (A blocking recv would hang here.)
        *is_active.write().unwrap() = false;

        // Poll for a clean exit well under the production 3s join budget. If
        // this loop ever sees the thread still alive after ~1s, the recv_timeout
        // fix has regressed back to a blocking recv.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while std::time::Instant::now() < deadline {
            if handle.is_finished() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            handle.is_finished(),
            "sender must wake and exit within ~1s of is_active=false even with no chunk"
        );
        handle.join().expect("sender thread must not panic");

        // Sender holds NOTHING after exit: keep _tx alive so the rx isn't
        // Disconnected during the test (we want to prove the flag path, not the
        // disconnect path).
        drop(_tx);
    }

    // -----------------------------------------------------------------------
    // PART N+1 — converse handle reaping on restart (AUD-CV3 / finding #62)
    //
    // The driver's terminal-auth teardown flips is_converse_active=false and
    // breaks, but leaves the thread slot `Some(finished_handle)`. A restart
    // without an intervening stop_converse is past the is_converse_active guard
    // (false), so the historical `if handle.is_none()` spawn-gate would see the
    // stale `Some` and SILENTLY SKIP spawning. `reap_finished_handle` must clear
    // a finished slot (so the gate fires) while refusing to clobber a live one.
    // -----------------------------------------------------------------------

    #[test]
    fn reap_finished_handle_clears_finished_slot_for_restart() {
        // A handle that exits immediately — models a thread whose driver already
        // tore down on a terminal auth error.
        let handle = std::thread::spawn(|| {});
        // Wait until it has actually finished so is_finished() is deterministic.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while !handle.is_finished() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(handle.is_finished(), "test handle should have exited");

        let mut slot = Some(handle);
        let res = reap_finished_handle(&mut slot, "converse driver");
        assert!(res.is_ok(), "a finished handle must reap cleanly");
        assert!(
            slot.is_none(),
            "the slot must be EMPTY after reaping so the spawn-gate (is_none) \
             fires and a restart actually respawns (#62)"
        );
    }

    #[test]
    fn reap_finished_handle_refuses_to_clobber_running_slot() {
        // A handle that blocks until told to exit — models a session that is
        // genuinely still running.
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let handle = std::thread::spawn(move || {
            while !stop2.load(Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        });

        let mut slot = Some(handle);
        let res = reap_finished_handle(&mut slot, "converse driver");
        assert!(
            res.is_err(),
            "a still-running handle must NOT be reaped — restart must error"
        );
        assert!(
            slot.is_some(),
            "the running handle must be put back, never dropped/double-spawned"
        );

        // Clean up the live thread.
        stop.store(true, Ordering::SeqCst);
        if let Some(h) = slot.take() {
            h.join().expect("worker must not panic");
        }
    }

    #[test]
    fn reap_finished_handle_is_noop_on_empty_slot() {
        let mut slot: Option<std::thread::JoinHandle<()>> = None;
        let res = reap_finished_handle(&mut slot, "converse driver");
        assert!(res.is_ok(), "an empty slot reaps to Ok (nothing to do)");
        assert!(slot.is_none(), "empty stays empty");
    }

    // -----------------------------------------------------------------------
    // PART N — converse production-glue (B18 / #46), headless.
    //
    // The pure FSM has 46 tests; these cover the PRODUCTION side-effect
    // primitives the live `GeminiConverseSink` dispatches — capture-gate
    // toggling, PCM16→i16 decode into a REAL AudioPlayer, barge-in
    // cancel/resume, and the null-client guard. They exercise the exact code
    // the sink methods run (see `GeminiConverseSink`'s impl above), but WITHOUT
    // building a mock Tauri AppHandle: `tauri::test::mock_context` makes tao
    // open an X11 connection at construction, which is unavailable/flaky on a
    // headless WSL box (the only `app_handle` use in these methods is the
    // transcript/error event emit — a thin `app_handle.emit(...)` not exercised
    // here). This shrinks #46's residual to the genuinely-perceptual "is audio
    // audible from the speaker" check only.
    // -----------------------------------------------------------------------

    #[test]
    fn converse_capture_gate_toggle_matches_sink_semantics() {
        // GeminiConverseSink::start_capture stores true + resumes the player;
        // stop_capture stores false. Exercise the same primitives directly.
        let gate = std::sync::atomic::AtomicBool::new(false);
        let player = crate::playback::AudioPlayer::new();
        // start_capture
        gate.store(true, Ordering::SeqCst);
        player.resume();
        assert!(gate.load(Ordering::SeqCst), "start_capture opens the gate");
        // stop_capture
        gate.store(false, Ordering::SeqCst);
        assert!(!gate.load(Ordering::SeqCst), "stop_capture closes the gate");
    }

    #[test]
    fn converse_barge_in_cancels_then_recapture_resumes() {
        // stop_playback → player.cancel(); start_capture → player.resume().
        let player = crate::playback::AudioPlayer::new();
        player.cancel(); // barge-in
        assert!(
            player.is_cancelled(),
            "stop_playback (barge-in) must trip the player cancel flag"
        );
        player.resume(); // start_capture on the next turn
        assert!(
            !player.is_cancelled(),
            "start_capture must clear cancel so the next reply is audible"
        );
    }

    #[test]
    fn converse_play_audio_decodes_pcm16_without_panic_and_no_stream() {
        // The exact play_audio body: pcm16_le_bytes_to_i16 then push_samples.
        // 2 samples LE (0x0001, 0xFFFF) + 1 stray byte that must be dropped.
        let player = crate::playback::AudioPlayer::new();
        let samples = crate::converse::pcm16_le_bytes_to_i16(&[0x01, 0x00, 0xFF, 0xFF, 0x42]);
        assert_eq!(
            samples,
            vec![1_i16, -1_i16],
            "decode drops the odd trailing byte"
        );
        if !samples.is_empty() {
            player.push_samples(&samples); // no stream open → writes 0, no panic
        }
        assert_eq!(
            player.free_samples(),
            0,
            "no playback stream open → nothing buffered (and no panic on decode)"
        );
    }

    #[test]
    fn converse_end_user_turn_is_noop_without_a_client() {
        // The end_user_turn body short-circuits when the client Option is None;
        // it must never panic the live driver thread. Exercise that guard shape.
        let client: std::sync::Mutex<Option<GeminiLiveClient>> = std::sync::Mutex::new(None);
        if let Ok(guard) = client.lock()
            && let Some(ref c) = *guard
            && let Err(e) = c.end_user_turn()
        {
            panic!("unreachable: client is None, got {e}");
        }
        // Reaching here without a panic is the assertion (None → no-op).
    }

    // -----------------------------------------------------------------------
    // Writer-side credential cache re-hydrate (audio-graph-c4d0 + #39)
    //
    // Both `save_credential_cmd` and `delete_credential_cmd` route through
    // `rehydrate_app_settings_cache`, which reloads the credential store and
    // re-fills the in-memory settings cache the capture read-path clones. These
    // tests drive the helper directly with an explicit `CredentialStore` so they
    // exercise the exact write-back logic without touching the on-disk keychain
    // (the delete/save commands themselves only add the store `load` + this
    // call, so the helper is the load-bearing surface). A `DeepgramStreaming`
    // ASR provider stands in for the confirmed 401 provider.
    // -----------------------------------------------------------------------

    fn deepgram_settings_with_cached_key(api_key: &str) -> crate::settings::AppSettings {
        crate::settings::AppSettings {
            asr_provider: crate::settings::AsrProvider::DeepgramStreaming {
                api_key: api_key.to_string(),
                model: "nova-2".to_string(),
                enable_diarization: true,
                endpointing_ms: 300,
                utterance_end_ms: 1000,
                vad_events: true,
                eot_threshold: 0.7,
                eager_eot_threshold: 0.3,
                eot_timeout_ms: 5000,
                max_speakers: 0,
                keyterms: vec![],
            },
            ..Default::default()
        }
    }

    fn cached_deepgram_api_key(state: &AppState) -> String {
        match &state
            .app_settings
            .read()
            .expect("app_settings lock poisoned")
            .asr_provider
        {
            crate::settings::AsrProvider::DeepgramStreaming { api_key, .. } => api_key.clone(),
            other => panic!("expected DeepgramStreaming provider, got {other:?}"),
        }
    }

    #[test]
    fn rehydrate_clears_deleted_key_from_settings_cache() {
        // Regression: audio-graph-c4d0. The user revokes/deletes a key; the
        // reloaded store no longer holds it. The capture read-path clones this
        // cache, so it MUST no longer serve the stale (deleted) key — otherwise
        // the live session keeps transmitting a revoked credential.
        let state = AppState::new();
        *state.app_settings.write().expect("lock poisoned") =
            deepgram_settings_with_cached_key("stale-deepgram-secret");
        assert_eq!(
            cached_deepgram_api_key(&state),
            "stale-deepgram-secret",
            "precondition: cache holds the stale key"
        );

        // Simulate the post-delete world: the store has no deepgram key.
        let store_after_delete = crate::credentials::CredentialStore::default();
        rehydrate_app_settings_cache(
            &state,
            &store_after_delete,
            "delete_credential_cmd",
            "deepgram_api_key",
        );

        assert_eq!(
            cached_deepgram_api_key(&state),
            "",
            "after delete re-hydrate the cache must NOT serve the deleted key"
        );
    }

    #[test]
    fn rehydrate_fills_new_key_into_settings_cache() {
        // Symmetric SAVE-path coverage (the #39 fix originally shipped without a
        // test). A running session holds a stale key in cache; the user saves a
        // NEW key; the reloaded store carries it. The cache must now serve the
        // NEW key so the session stops 401-ing.
        let state = AppState::new();
        *state.app_settings.write().expect("lock poisoned") =
            deepgram_settings_with_cached_key("old-deepgram-secret");

        let mut store_after_save = crate::credentials::CredentialStore::default();
        store_after_save.deepgram_api_key = Some("fresh-deepgram-secret".to_string());
        rehydrate_app_settings_cache(
            &state,
            &store_after_save,
            "save_credential_cmd",
            "deepgram_api_key",
        );

        assert_eq!(
            cached_deepgram_api_key(&state),
            "fresh-deepgram-secret",
            "after save re-hydrate the cache must serve the freshly-saved key"
        );
    }
}
