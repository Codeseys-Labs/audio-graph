//! Tauri IPC command handlers.
//!
//! Each function here is exposed to the frontend via `tauri::generate_handler![]`.
//! They access `AppState` through Tauri's managed state.
//!
//! Heavy processing logic (speech, extraction) lives in the [`crate::speech`]
//! module — this file only contains thin `#[tauri::command]` wrappers.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tauri::{Emitter, State};
use tokio_util::sync::CancellationToken;

use crate::audio::consumer::{
    ConsumerActiveFn, ProcessedAudioConsumerDescriptor, ProcessedAudioConsumerRegistration,
    ProcessedAudioConsumerStage, ProcessedAudioDropPolicy, ProcessedAudioMixingMode,
    ProcessedAudioSourceFilter,
};
use crate::audio::pipeline::{AudioPipeline, ProcessedAudioChunk};
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
    pub projection_events: Vec<crate::projections::ProjectionPatch>,
    pub live_assist_cards: Vec<crate::events::LiveAssistCardRecord>,
    pub notes: Option<crate::projections::MaterializedNotes>,
    pub materialized_graph: Option<crate::projections::MaterializedGraph>,
}

/// Schema version for the session export bundle. Bump when the bundle's shape
/// changes so importers / migration tooling can branch on it. This is the
/// "schema metadata" the session-artifact-migration acceptance requires.
pub const SESSION_EXPORT_SCHEMA_VERSION: u32 = 1;

/// A self-describing, self-contained snapshot of every durable artifact a
/// session owns. Assembled from the event-sourced logs (transcript events,
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

/// Join a worker thread on shutdown, waiting up to `timeout` for it to observe
/// the stop flag and exit. Polls `is_finished()` so a wedged worker can never
/// hang the Stop command — on timeout the handle is detached (dropped) with a
/// warning instead of blocking forever. (Critique H2: prevents Stop→Start
/// races leaving duplicate consumers/workers alive.)
fn join_worker_with_timeout(
    handle: std::thread::JoinHandle<()>,
    timeout: std::time::Duration,
    name: &str,
) {
    let deadline = std::time::Instant::now() + timeout;
    while !handle.is_finished() {
        if std::time::Instant::now() >= deadline {
            log::warn!("{name} did not exit within {timeout:?} on stop; detaching handle");
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    if let Err(e) = handle.join() {
        log::warn!("{name} panicked during shutdown: {e:?}");
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

    let (source_id, target, source_descriptor) =
        resolve_capture_start_target(source_id, capture_target, source)?;

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

    // 1. Start capture via the manager.
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

    // 2. Start pipeline thread if not already running.
    {
        let mut pipeline_handle = state
            .pipeline_thread
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        if pipeline_handle.is_none() {
            let rx = state.pipeline_rx.clone();
            let tx = state.processed_tx.clone();
            let handle = std::thread::Builder::new()
                .name("audio-pipeline".to_string())
                .spawn(move || {
                    let mut pipeline = AudioPipeline::new(rx, tx);
                    pipeline.run();
                })
                .map_err(|e| format!("Failed to spawn pipeline thread: {}", e))?;
            *pipeline_handle = Some(handle);
            log::info!("Pipeline thread spawned");
        }
    }

    // 2b. Start dispatcher thread: reads from processed_rx and fans out through
    //     the processed-audio consumer registry.
    {
        let mut dispatcher_handle = state
            .dispatcher_thread
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
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
                    while let Ok(chunk) = processed_rx.recv() {
                        chunks_seen += 1;
                        let summary = consumers.dispatch(chunk);
                        if summary.dropped_chunks > 0 {
                            total_dropped += summary.dropped_chunks as u64;
                            if total_dropped % 50 == summary.dropped_chunks as u64 {
                                log::warn!(
                                    "Audio dispatcher: processed-audio consumers dropped {} \
                                     oldest/newest chunk(s) total (consumer behind real time)",
                                    total_dropped
                                );
                            }
                        }

                        if summary.dropped_chunks > 0
                            || last_health_emit.elapsed() >= std::time::Duration::from_secs(2)
                        {
                            let payload = consumers.health_payload();
                            let _ = app_handle.emit(events::AUDIO_CONSUMER_HEALTH, &payload);
                            last_health_emit = std::time::Instant::now();
                        }
                    }
                    let payload = consumers.health_payload();
                    let _ = app_handle.emit(events::AUDIO_CONSUMER_HEALTH, &payload);
                    log::info!(
                        "Audio dispatcher: exiting (pipeline channel closed). \
                         chunks_seen={}, total consumer drops={}",
                        chunks_seen,
                        total_dropped
                    );
                })
                .map_err(|e| format!("Failed to spawn dispatcher thread: {}", e))?;
            *dispatcher_handle = Some(handle);
            log::info!("Audio dispatcher thread spawned");
        }
    }

    // 3. Update state flags.
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
    stop_capture_impl(source_id, state.inner(), &app).await
}

/// Implementation of [`stop_capture`] that operates on borrowed state/app so it
/// can be exercised from tests without constructing a per-test Tauri/tao app.
async fn stop_capture_impl(
    source_id: String,
    state: &AppState,
    app: &tauri::AppHandle,
) -> AppResult<()> {
    log::info!("stop_capture called for source: {}", source_id);

    let remaining;
    {
        let mut manager = state
            .capture_manager
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        manager.stop_capture(&source_id)?;
        remaining = manager.active_captures().len();
    }

    if remaining == 0 {
        if let Ok(mut capturing) = state.is_capturing.write() {
            *capturing = false;
        }
        // Also stop transcription since there's no more audio flowing
        state.is_transcribing.store(false, Ordering::SeqCst);
        // Clean up speech processor thread handle
        if let Ok(mut sp_handle) = state.speech_processor_thread.lock() {
            *sp_handle = None;
        }
        // Clean up ASR worker thread handle
        if let Ok(mut asr_handle) = state.asr_worker_thread.lock() {
            *asr_handle = None;
        }
        // Also stop Gemini notes if running.
        if let Ok(mut gemini_active) = state.is_gemini_active.write()
            && *gemini_active
        {
            *gemini_active = false;
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
                std::thread::spawn(move || {
                    if let Some(h) = audio_h {
                        join_worker_with_timeout(
                            h,
                            std::time::Duration::from_secs(3),
                            "Gemini audio worker (capture stop)",
                        );
                    }
                    if let Some(h) = event_h {
                        join_worker_with_timeout(
                            h,
                            std::time::Duration::from_secs(3),
                            "Gemini event worker (capture stop)",
                        );
                    }
                });
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
        if converse_active {
            stop_converse_runtime(state, "capture stop").await?;
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
        let settings = read_settings_for_session_content(state.inner(), "asr_session")?;
        let mut asr_provider = settings.asr_provider.clone();
        asr_provider.apply_diarization_settings(&settings.diarization);
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

    // 1. Start speech processor thread (ASR + Diarization orchestrator).
    //    The speech processor reads directly from the processed audio channel,
    //    accumulates chunks into ~2s segments, and runs ASR inline.
    {
        let mut sp_handle = state
            .speech_processor_thread
            .lock()
            .map_err(|e| AppError::Unknown(format!("Lock error: {}", e)))?;
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

            let settings = read_settings_for_session_content(state.inner(), "asr_session")?;
            let mut asr_provider = settings.asr_provider.clone();
            asr_provider.apply_diarization_settings(&settings.diarization);
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
            let transcript_event_writer = state.transcript_event_writer.clone();
            let transcript_ledger = state.transcript_ledger.clone();
            let speaker_timeline = state.speaker_timeline.clone();
            let projection_schedulers = state.projection_schedulers.clone();
            let projection_runtime = state.projection_runtime_handle();

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
                        transcript_event_writer,
                        transcript_ledger,
                        speaker_timeline,
                        projection_schedulers,
                        projection_runtime,
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
                    };
                    let config = speech::SpeechConfig {
                        models_dir,
                        llm_provider,
                        llm_allow_cloud_fallbacks,
                        provider_content_egress_policy,
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
    if let Ok(mut status) = state.pipeline_status.write() {
        status.asr = StageStatus::Running { processed_count: 0 };
        status.diarization = StageStatus::Running { processed_count: 0 };
        status.entity_extraction = StageStatus::Running { processed_count: 0 };
        status.graph = StageStatus::Running { processed_count: 0 };
    }

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
    log::info!("stop_transcribe called");

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
    let _ = tokio::task::spawn_blocking(move || {
        if let Some(h) = sp {
            join_worker_with_timeout(h, std::time::Duration::from_secs(3), "speech processor");
        }
        if let Some(h) = asr {
            join_worker_with_timeout(h, std::time::Duration::from_secs(3), "ASR worker");
        }
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

    let settings = read_settings_for_session_content(state.inner(), "llm_chat")?;
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

    let settings = read_settings_for_session_content(state.inner(), "llm_chat")?;
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
    let allow_cloud_fallbacks = settings
        .privacy_mode
        .allows_session_cloud_content_transfer();
    // `chat_with_history` now returns the reply text plus the token usage the
    // backend reported. The native `LlmEngine` surfaces a real (prompt +
    // completion) count; the cloud backends routed through this blocking path
    // (Bedrock via ApiClient, OpenRouter blocking, mistral.rs) report 0 because
    // their `chat_with_history` signatures don't carry usage yet — never
    // fabricated. On error we synthesize a fallback message with no count.
    let (response_text, tokens_used) = match tokio::task::spawn_blocking(move || {
        executor.chat_with_history_with_policy(
            messages,
            graph_context,
            llm_provider,
            allow_cloud_fallbacks,
        )
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
    sync_llm_api_client_from_settings_cache(state.inner())?;
    sync_openrouter_client_from_settings_cache(state.inner())?;

    let settings = read_settings_for_session_content(state.inner(), "notes_synthesis")?;
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
    let allow_cloud_fallbacks = settings
        .privacy_mode
        .allows_session_cloud_content_transfer();
    let outcome = tokio::task::spawn_blocking(move || {
        executor.chat_with_history_with_policy(
            messages,
            graph_context,
            llm_provider,
            allow_cloud_fallbacks,
        )
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

fn approved_agent_projection_patch(
    state: &AppState,
    proposal: &events::AgentProposalPayload,
) -> Result<u64, String> {
    let runtime = state.projection_runtime_handle();
    let session_id = runtime.current_session_id();
    let ledger = runtime.transcript_ledger_snapshot();
    let basis = ledger.current_basis();
    let now_ms = unix_millis();

    let (kind, operations, prompt_id) = match &proposal.kind {
        events::AgentProposalKind::Note => (
            crate::projections::ProjectionKind::Notes,
            vec![crate::projections::ProjectionOperation::UpsertNote {
                id: format!("live-assist-note-{}", proposal.id),
                title: proposal.title.clone(),
                body: proposal.body.clone(),
                tags: vec!["live-assist".to_string(), "approved".to_string()],
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
        },
        queued_at_ms: Some(now_ms),
        generation_latency_ms: Some(0),
        apply_latency_ms: None,
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
    let projection_patch_sequence = match approved_agent_projection_patch(&state, &proposal) {
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
        let mut graph = state
            .knowledge_graph
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        let timestamp = proposal.created_at_ms as f64 / 1000.0;
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

    let mut graph = state
        .knowledge_graph
        .lock()
        .map_err(|e| format!("Lock error: {}", e))?;
    graph.process_extraction(
        &extraction,
        unix_millis() as f64 / 1000.0,
        &speaker,
        &segment_id,
    );
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
    let timestamp = unix_millis() as f64 / 1000.0;
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
    let _ = tokio::task::spawn_blocking(move || {
        if let Some(h) = audio_h {
            join_worker_with_timeout(h, std::time::Duration::from_secs(3), "Gemini audio worker");
        }
        if let Some(h) = event_h {
            join_worker_with_timeout(h, std::time::Duration::from_secs(3), "Gemini event worker");
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
    let _ = tokio::task::spawn_blocking(move || {
        if let Some(h) = audio_h {
            join_worker_with_timeout(h, std::time::Duration::from_secs(3), &audio_join_name);
        }
        if let Some(h) = conv_h {
            join_worker_with_timeout(h, std::time::Duration::from_secs(3), &driver_join_name);
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
    let _ = tokio::task::spawn_blocking(move || {
        if let Some(h) = audio_h {
            join_worker_with_timeout(h, std::time::Duration::from_secs(3), &audio_join_name);
        }
        if let Some(h) = event_h {
            join_worker_with_timeout(h, std::time::Duration::from_secs(3), &driver_join_name);
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
        let mut missing_timestamp = patch.basis.span_revisions.is_empty();

        for span in &patch.basis.span_revisions {
            match timing_by_span_revision
                .get(&(span.span_id.clone(), span.revision_number))
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

fn projection_replay_report_for_session(session_id: &str) -> AppResult<ProjectionReplayReport> {
    validate_session_id(session_id).map_err(AppError::from)?;

    let repository = FileMemoryRepository::user_data();
    let transcript_events = repository.load_transcript_events(session_id)?;
    let projection_events = repository.load_projection_patches(session_id)?;
    let stored_notes = repository.load_materialized_notes(session_id)?;
    let stored_graph = repository.load_materialized_graph(session_id)?;

    let (transcript_replay_error, transcript_span_count) =
        match crate::projections::TranscriptLedger::replay(session_id, transcript_events.clone()) {
            Ok(ledger) => (None, ledger.latest_spans.len()),
            Err(error) => (Some(format!("{:?}", error)), 0),
        };

    let (projection_replay_error, projection_history_validation, replayed_state) =
        match crate::projections::MaterializedProjectionState::replay_accepted_patches_with_transcript_history(
            session_id,
            transcript_events.clone(),
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

fn indexed_session_paths(
    session_id: &str,
) -> Result<(std::path::PathBuf, std::path::PathBuf), String> {
    validate_session_id(session_id)?;
    if let Some(metadata) = crate::sessions::find_session(session_id) {
        return Ok(crate::sessions::session_file_paths(&metadata));
    }
    Ok((
        crate::user_data::transcript_path(session_id)?,
        crate::user_data::graph_path(session_id)?,
    ))
}

fn read_session_transcript(session_id: &str) -> Result<Vec<TranscriptSegment>, String> {
    validate_session_id(session_id)?;
    let (path, _) = indexed_session_paths(session_id)?;
    if !path.exists() {
        return Err(format!("Transcript file not found: {}", path.display()));
    }
    let contents = std::fs::read_to_string(&path).map_err(|e| format!("{}", e))?;
    let mut segments = Vec::new();
    for line in contents.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<TranscriptSegment>(line) {
            Ok(seg) => segments.push(seg),
            Err(e) => log::warn!("Skipping malformed transcript line: {}", e),
        }
    }
    Ok(segments)
}

/// Load a past session's transcript from disk. Returns the parsed
/// `TranscriptSegment`s from `~/.audiograph/transcripts/<session_id>.jsonl`.
#[tauri::command]
pub fn load_session_transcript(session_id: String) -> AppResult<Vec<TranscriptSegment>> {
    read_session_transcript(&session_id).map_err(AppError::from)
}

/// Load a past session's data-movement ledger (seed audio-graph-70a3) for the
/// privacy route report UI (seed audio-graph-51e0).
///
/// Returns the append-ordered, already-redacted [`DataMovementEvent`]s from
/// `~/.audiograph/ledgers/<session_id>.movements.jsonl`. The ledger schema is
/// redaction-safe by construction — it carries only data *classes*, boundary
/// hops, provider/model ids, hashed artifact paths, and pre-redacted error
/// messages, never secrets or raw payloads — so the events can be surfaced to
/// the user verbatim. A session that never moved any data (or whose ledger
/// file does not exist) yields an empty vec, which the UI renders as
/// "no content left the device".
#[tauri::command]
pub fn load_session_data_movement_cmd(
    session_id: String,
) -> AppResult<Vec<crate::persistence::DataMovementEvent>> {
    // Defense-in-depth: reject path-traversal session ids before joining the id
    // into the ledgers directory (audio-graph-e692). Mirrors every sibling
    // session command, which all validate first.
    validate_session_id(&session_id)?;
    crate::persistence::load_data_movement_events(&session_id).map_err(AppError::from)
}

fn materialized_notes_has_content(notes: &crate::projections::MaterializedNotes) -> bool {
    notes.last_sequence > 0 || !notes.notes.is_empty()
}

fn materialized_graph_has_content(graph: &crate::projections::MaterializedGraph) -> bool {
    graph.last_sequence > 0 || !graph.nodes.is_empty() || !graph.edges.is_empty()
}

fn choose_materialized_notes(
    loaded: Option<crate::projections::MaterializedNotes>,
    replayed: Option<&crate::projections::MaterializedProjectionState>,
) -> Option<crate::projections::MaterializedNotes> {
    let replayed = replayed
        .map(|state| state.notes.clone())
        .filter(materialized_notes_has_content);
    match (loaded, replayed) {
        (Some(loaded), Some(replayed)) if replayed.last_sequence > loaded.last_sequence => {
            Some(replayed)
        }
        (Some(loaded), _) => Some(loaded),
        (None, replayed) => replayed,
    }
}

fn choose_materialized_graph(
    loaded: Option<crate::projections::MaterializedGraph>,
    replayed: Option<&crate::projections::MaterializedProjectionState>,
) -> Option<crate::projections::MaterializedGraph> {
    let replayed = replayed
        .map(|state| state.graph.clone())
        .filter(materialized_graph_has_content);
    match (loaded, replayed) {
        (Some(loaded), Some(replayed)) if replayed.last_sequence > loaded.last_sequence => {
            Some(replayed)
        }
        (Some(loaded), _) => Some(loaded),
        (None, replayed) => replayed,
    }
}

/// Load a past session's transcript and graph snapshot into the active UI view.
#[tauri::command]
pub fn load_session(session_id: String, state: State<'_, AppState>) -> AppResult<LoadedSession> {
    load_session_impl(session_id, state.inner())
}

/// Implementation of [`load_session`] that operates on borrowed state so it can
/// be exercised from tests without constructing a per-test Tauri/tao app.
fn load_session_impl(session_id: String, state: &AppState) -> AppResult<LoadedSession> {
    validate_session_id(&session_id)?;
    let (transcript_path, graph_path) = indexed_session_paths(&session_id)?;
    let has_any_artifact = crate::sessions::session_artifact_paths_for_id(&session_id)
        .iter()
        .any(|path| path.exists());
    if !has_any_artifact {
        return Err(AppError::SessionInvalid {
            reason: format!("Session files not found: {}", session_id),
        });
    }
    let transcript = if transcript_path.exists() {
        read_session_transcript(&session_id)?
    } else {
        Vec::new()
    };
    let loaded_graph = if graph_path.exists() {
        crate::graph::temporal::TemporalKnowledgeGraph::load_from_file(&graph_path)?
    } else {
        crate::graph::temporal::TemporalKnowledgeGraph::new()
    };
    let snapshot = loaded_graph.snapshot();
    let repository = FileMemoryRepository::user_data();
    let transcript_events = repository.load_transcript_events(&session_id)?;
    // Diarization span revisions (audio-graph-0b33): the persisted speaker log
    // the live path now writes (audio-graph-719d). Surfacing it lets the
    // frontend resolve trusted latest-wins speaker attribution on reload rather
    // than trusting the inline ASR labels. A session that never emitted
    // diarization rows loads an empty vec.
    let diarization_events = repository.load_diarization_span_revisions(&session_id)?;
    let projection_events = repository.load_projection_patches(&session_id)?;
    let live_assist_cards = repository.load_live_assist_cards(&session_id)?;
    let notes = repository.load_materialized_notes(&session_id)?;
    let materialized_graph = repository.load_materialized_graph(&session_id)?;
    let replayed_projection_state = if projection_events.is_empty() {
        None
    } else {
        match crate::projections::MaterializedProjectionState::replay_accepted_patches_with_transcript_history(
            &session_id,
            transcript_events.clone(),
            projection_events.clone(),
        ) {
            Ok(replay) => {
                if replay.validation.invalid_patch_count > 0 {
                    log::warn!(
                        "Projection replay for session {} skipped {} patch(es) with invalid historical basis: {:?}",
                        session_id,
                        replay.validation.invalid_patch_count,
                        replay.validation.first_error_summary()
                    );
                }
                Some(replay.state)
            }
            Err(e) => {
                log::warn!(
                    "Failed to replay projection events for session {}: {:?}",
                    session_id,
                    e
                );
                None
            }
        }
    };
    let notes = choose_materialized_notes(notes, replayed_projection_state.as_ref());
    let materialized_graph =
        choose_materialized_graph(materialized_graph, replayed_projection_state.as_ref());
    let loaded_ledger =
        crate::projections::TranscriptLedger::replay(&session_id, transcript_events.clone())
            .map_err(|e| {
                format!(
                    "Failed to replay transcript ledger for session {}: {:?}",
                    session_id, e
                )
            })?;

    {
        let mut graph = state
            .knowledge_graph
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        *graph = loaded_graph;
    }
    if let Ok(mut gs) = state.graph_snapshot.write() {
        *gs = snapshot.clone();
    }
    {
        let mut ledger = state
            .transcript_ledger
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        *ledger = loaded_ledger;
    }
    {
        let mut materialized = state
            .materialized_projection_state
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        *materialized = crate::projections::MaterializedProjectionState {
            session_id: session_id.clone(),
            notes: notes
                .clone()
                .unwrap_or_else(|| crate::projections::MaterializedNotes::new(&session_id)),
            graph: materialized_graph
                .clone()
                .unwrap_or_else(|| crate::projections::MaterializedGraph::new(&session_id)),
        };
    }
    {
        let mut schedulers = state
            .projection_schedulers
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        schedulers.reset(session_id.clone());
        // Rehydrate the scheduler queue from the persisted snapshot if present.
        // Best-effort: a missing or corrupt snapshot is not fatal — the
        // scheduler just starts clean and re-queues on the next
        // observe_ledger call.
        if let Some(queue_snapshot) = crate::persistence::load_scheduler_queue_state(&session_id) {
            schedulers.restore_from_snapshot(queue_snapshot);
        }
    }

    Ok(LoadedSession {
        transcript,
        graph: snapshot,
        transcript_events,
        diarization_events,
        projection_events,
        live_assist_cards,
        notes,
        materialized_graph,
    })
}

/// Assemble a complete [`SessionExportBundle`] for a session from its durable
/// on-disk artifacts.
///
/// Reads (all read-only, none mutate state):
///   - legacy transcript segments (`transcripts/<id>.jsonl`)
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
fn session_export_bundle(session_id: &str) -> AppResult<SessionExportBundle> {
    validate_session_id(session_id)?;

    let has_any_artifact = crate::sessions::session_artifact_paths_for_id(session_id)
        .iter()
        .any(|path| path.exists());
    if !has_any_artifact {
        return Err(AppError::SessionInvalid {
            reason: format!("Session files not found: {}", session_id),
        });
    }

    let (transcript_path, graph_path) = indexed_session_paths(session_id)?;
    let transcript = if transcript_path.exists() {
        read_session_transcript(session_id)?
    } else {
        Vec::new()
    };
    let graph = if graph_path.exists() {
        Some(
            crate::graph::temporal::TemporalKnowledgeGraph::load_from_file(&graph_path)?.snapshot(),
        )
    } else {
        None
    };

    let repository = FileMemoryRepository::user_data();
    let transcript_events = repository.load_transcript_events(session_id)?;
    let diarization_events = repository.load_diarization_span_revisions(session_id)?;
    let projection_events = repository.load_projection_patches(session_id)?;
    let notes = repository.load_materialized_notes(session_id)?;
    let materialized_graph = repository.load_materialized_graph(session_id)?;

    Ok(SessionExportBundle {
        schema_version: SESSION_EXPORT_SCHEMA_VERSION,
        session_id: session_id.to_string(),
        metadata: crate::sessions::find_session(session_id),
        transcript,
        transcript_events,
        diarization_events,
        projection_events,
        notes,
        materialized_graph,
        graph,
    })
}

/// Export every durable artifact a session owns as a single self-contained
/// bundle: transcript segments + transcript event log + diarization event log
/// + projection event log + materialized notes + materialized graph + legacy
/// graph snapshot, plus a schema version and the index metadata.
///
/// This is the session-level counterpart to the in-memory `export_transcript`
/// / `export_graph` commands: it works on any on-disk session (not just the
/// active one) and captures the whole event-sourced lifecycle boundary rather
/// than only the legacy graph snapshot.
#[tauri::command]
pub fn export_session_bundle(session_id: String) -> AppResult<SessionExportBundle> {
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
fn session_timeline(session_id: &str) -> AppResult<Vec<crate::timeline::TimelineEntry>> {
    validate_session_id(session_id)?;

    let has_any_artifact = crate::sessions::session_artifact_paths_for_id(session_id)
        .iter()
        .any(|path| path.exists());
    if !has_any_artifact {
        return Err(AppError::SessionInvalid {
            reason: format!("Session files not found: {}", session_id),
        });
    }

    let (_transcript_path, graph_path) = indexed_session_paths(session_id)?;
    let repository = FileMemoryRepository::user_data();
    let transcript_events = repository.load_transcript_events(session_id)?;
    let diarization_events = repository.load_diarization_span_revisions(session_id)?;

    let ledger = crate::projections::TranscriptLedger::replay(session_id, transcript_events)
        .map_err(|e| {
            format!("Failed to replay transcript ledger for session {session_id}: {e:?}")
        })?;
    let speakers = crate::projections::SpeakerTimeline::replay(session_id, diarization_events)
        .map_err(|e| {
            format!("Failed to replay speaker timeline for session {session_id}: {e:?}")
        })?;
    let live_graph = if graph_path.exists() {
        crate::graph::temporal::TemporalKnowledgeGraph::load_from_file(&graph_path)?
    } else {
        crate::graph::temporal::TemporalKnowledgeGraph::new()
    };

    Ok(crate::timeline::build_session_timeline(
        &ledger,
        &speakers,
        &live_graph,
    ))
}

/// Fold a session's durable logs into its [`crate::timeline::TimelineEntry`]
/// list — "who said what, when, in relation to what" (epic 0d72 P1,
/// ADR-0026 §4.1). Ordered by media-clock start time, duplicate-free, with
/// latest-wins speaker attribution and forward links to the live graph edges
/// each utterance produced.
#[tauri::command]
pub fn build_session_timeline_cmd(
    session_id: String,
) -> AppResult<Vec<crate::timeline::TimelineEntry>> {
    session_timeline(&session_id)
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
pub fn delete_session(session_id: String) -> AppResult<()> {
    validate_session_id(&session_id)?;
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
pub fn delete_session_permanently(session_id: String) -> AppResult<()> {
    validate_session_id(&session_id)?;
    let artifact_paths = crate::sessions::session_artifact_paths_for_id(&session_id);
    crate::sessions::remove_from_index(&session_id)?;
    for path in artifact_paths {
        match std::fs::remove_file(&path) {
            Ok(_) => log::info!("Deleted session artifact: {}", path.display()),
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                log::warn!(
                    "Failed to delete session artifact {}: {}",
                    path.display(),
                    e
                );
            }
            _ => {}
        }
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
pub fn purge_expired_sessions() -> AppResult<Vec<String>> {
    let purged = crate::sessions::purge_expired_sessions()?;
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
///   1. Finalize current session's sessions-index entry (status → complete).
///   2. Re-save the current session's usage record so on-disk totals are
///      flushed before the ID rotates.
///   3. Seed a fresh zeroed usage file for the new session so
///      `get_current_session_usage` returns zeros immediately after rotation.
///   4. Rotate `AppState::session_id` in place:
///        - The transcript writer is respawned against the new ID's file.
///        - The graph-autosave thread re-reads the ID on its next 30s tick
///          and starts writing to the new session's file.
///        - The Gemini event thread re-reads the ID on the next TurnComplete.
///   5. Register the new session in the sessions index so list_sessions
///      shows it alongside the previous one.
///
/// Returns the new session ID.
#[tauri::command]
pub fn new_session_cmd(state: State<'_, AppState>) -> AppResult<String> {
    let previous_id = state.current_session_id();

    // 1. Finalize current session's index entry. Best-effort: a failed
    //    finalize must not prevent us handing the caller a fresh UUID.
    if let Err(e) = crate::sessions::finalize_session(&previous_id) {
        log::warn!("new_session_cmd: finalize current failed: {}", e);
    }

    // 2. Re-save the current session's usage record. If the file is missing
    //    this is a harmless zero-write; if it exists, `save_usage` is a
    //    no-op rewrite of the same bytes. Either way, it guarantees the
    //    file is present on disk before the caller moves on.
    let current = crate::sessions::usage::load_usage(&previous_id);
    if let Err(e) = crate::sessions::usage::save_usage(&current) {
        log::warn!("new_session_cmd: save current usage failed: {}", e);
    }

    // 3. Seed a fresh usage file for the next session. Do this BEFORE the
    //    rotate so `get_current_session_usage` immediately reads zeroes.
    let new_id = uuid::Uuid::new_v4().to_string();
    let fresh = crate::sessions::usage::SessionUsage {
        session_id: new_id.clone(),
        ..crate::sessions::usage::SessionUsage::default()
    };
    crate::sessions::usage::save_usage(&fresh)?;

    // 4. Rotate in-process. `rotate_session` swaps the session_id Arc and
    //    respawns the transcript writer; the autosave + gemini-event
    //    threads pick up the change on their next iteration.
    //
    //    Concurrent-rotate guard: if another rotation is already in flight,
    //    skip and return the current session ID. The caller sees a successful
    //    rotation either way (the in-flight rotate will land a fresh ID);
    //    they just don't get the one *we* seeded. The usage file we wrote in
    //    step 3 is then orphaned — harmless, since seed files are zeroed and
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
    }

    // 5. Register new session in the index so it shows up in list_sessions
    //    (status "active"). Best-effort: failure just means the UI won't
    //    see the entry until the next restart rediscovers it.
    if let Err(e) = crate::sessions::register_session(&new_id) {
        log::warn!("new_session_cmd: register_session failed: {}", e);
    }

    log::info!("new_session_cmd: rotated {} → {}", previous_id, new_id);
    Ok(new_id)
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
    }
}

fn should_probe_provider(
    descriptor: &crate::provider_registry::ProviderDescriptor,
    active_ids: &HashSet<&'static str>,
    settings: &crate::settings::AppSettings,
    store: &crate::credentials::CredentialStore,
) -> bool {
    if !provider_has_automatic_health_probe(descriptor, settings) {
        return false;
    }
    if !missing_required_credentials(descriptor, settings, store).is_empty() {
        return false;
    }
    if provider_config_readiness_message(descriptor, settings).is_some() {
        return false;
    }
    match descriptor.id {
        "asr.deepgram" | "asr.assemblyai" | "asr.soniox" | "llm.cerebras" | "llm.openrouter"
        | "tts.deepgram_aura" => true,
        "realtime_agent.gemini_live" => active_ids.contains(descriptor.id),
        "asr.api" | "asr.aws_transcribe" | "llm.api" | "llm.aws_bedrock" => {
            active_ids.contains(descriptor.id)
        }
        _ => false,
    }
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
    let active_ids = active_provider_ids(&settings, native_realtime_active);
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

/// List available AWS profiles from ~/.aws/config and ~/.aws/credentials.
#[tauri::command]
pub fn list_aws_profiles() -> Vec<String> {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return vec![],
    };
    let mut profiles = std::collections::BTreeSet::new();

    for filename in &["config", "credentials"] {
        let path = home.join(".aws").join(filename);
        if let Ok(contents) = std::fs::read_to_string(&path) {
            for line in contents.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("[profile ") && trimmed.ends_with(']') {
                    let name = &trimmed[9..trimmed.len() - 1];
                    profiles.insert(name.to_string());
                } else if trimmed == "[default]" {
                    profiles.insert("default".to_string());
                } else if *filename == "credentials"
                    && trimmed.starts_with('[')
                    && trimmed.ends_with(']')
                {
                    let name = &trimmed[1..trimmed.len() - 1];
                    profiles.insert(name.to_string());
                }
            }
        }
    }

    profiles.into_iter().collect()
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
    format!("HTTP {}: {}", status, body)
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
    if body.is_empty() {
        format!("Deepgram returned HTTP {}", status)
    } else {
        format!("Deepgram returned HTTP {}: {}", status, body)
    }
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
    if body.is_empty() {
        format!("Soniox returned HTTP {}", status)
    } else {
        format!("Soniox returned HTTP {}: {}", status, body)
    }
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
        return Err(AppError::Unknown(format!(
            "AssemblyAI returned HTTP {}",
            status
        )));
    }
    Ok("AssemblyAI account key is valid via REST; v3 streaming socket smoke not run".to_string())
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
        return Err(AppError::Unknown(format!(
            "Gemini API returned HTTP {}",
            status
        )));
    }
    Ok("Gemini API key is valid".to_string())
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
    let api_key = openrouter_api_key_from_store(&store)?;
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
    let api_key = openrouter_api_key_from_store(&store)?;
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
            basis,
            operations: vec![crate::projections::ProjectionOperation::UpsertNote {
                id: "note-report".to_string(),
                title: "Private title".to_string(),
                body: note_body.to_string(),
                tags: vec!["private".to_string()],
            }],
            confidence: 0.9,
            provenance: crate::projections::ProjectionProvenance {
                provider: "test".to_string(),
                model: "projection-report".to_string(),
                prompt_id: "report-notes-v1".to_string(),
            },
            queued_at_ms: Some(1_700_000_050_000 + sequence),
            generation_latency_ms: Some(30 + sequence),
            apply_latency_ms: Some(5 + sequence),
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
            basis,
            operations: vec![crate::projections::ProjectionOperation::UpsertGraphNode {
                id: "node-report".to_string(),
                name: "Private Node".to_string(),
                entity_type: "PrivateEntity".to_string(),
                description: Some("Private graph description".to_string()),
            }],
            confidence: 0.86,
            provenance: crate::projections::ProjectionProvenance {
                provider: "test".to_string(),
                model: "projection-report".to_string(),
                prompt_id: "report-graph-v1".to_string(),
            },
            queued_at_ms: Some(1_700_000_150_000 + sequence),
            generation_latency_ms: Some(40 + sequence),
            apply_latency_ms: Some(6 + sequence),
            created_at_ms: 1_700_000_200_000 + sequence,
        }
    }

    fn invalid_graph_patch() -> crate::projections::ProjectionPatch {
        crate::projections::ProjectionPatch {
            sequence: 1,
            kind: crate::projections::ProjectionKind::Graph,
            llm_request_id: "report-invalid-graph".to_string(),
            basis: crate::projections::ProjectionBasis {
                span_revisions: Vec::new(),
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
            }],
            confidence: 0.5,
            provenance: crate::projections::ProjectionProvenance {
                provider: "test".to_string(),
                model: "projection-report".to_string(),
                prompt_id: "report-invalid-v1".to_string(),
            },
            queued_at_ms: None,
            generation_latency_ms: None,
            apply_latency_ms: None,
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
            updated_by_sequence: 0,
            updated_at_ms: 1,
            basis,
            provenance: crate::projections::ProjectionProvenance {
                provider: "test".to_string(),
                model: "stale-artifact".to_string(),
                prompt_id: "stale-notes".to_string(),
            },
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
            basis,
            provenance: crate::projections::ProjectionProvenance {
                provider: "test".to_string(),
                model: "stale-artifact".to_string(),
                prompt_id: "stale-graph".to_string(),
            },
        });
        graph
    }

    fn leaked_active_projection_state() -> crate::projections::MaterializedProjectionState {
        let session_id = "active-session-before-load";
        let basis = crate::projections::ProjectionBasis {
            span_revisions: Vec::new(),
            diarization_span_revisions: Vec::new(),
            transcript_hash: "active-before-load".to_string(),
            summarized_through_revision: None,
        };
        let provenance = crate::projections::ProjectionProvenance {
            provider: "test".to_string(),
            model: "active-before-load".to_string(),
            prompt_id: "active-before-load".to_string(),
        };
        let mut state = crate::projections::MaterializedProjectionState::new(session_id);
        state.notes.last_sequence = 99;
        state
            .notes
            .notes
            .push(crate::projections::MaterializedNote {
                id: "leaked-note".to_string(),
                title: "Leaked title".to_string(),
                body: "Prior active-session note must be replaced by load_session.".to_string(),
                tags: vec!["leak".to_string()],
                updated_by_sequence: 99,
                updated_at_ms: 99,
                basis: basis.clone(),
                provenance: provenance.clone(),
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
                    "Prior active-session graph must be replaced by load_session.".to_string(),
                ),
                confidence: 0.99,
                valid_from_ms: 99,
                valid_until_ms: None,
                updated_by_sequence: 99,
                updated_at_ms: 99,
                basis,
                provenance,
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
        let events = load_session_data_movement_cmd("never-recorded".to_string())
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

        let loaded = load_session_data_movement_cmd(session_id.to_string()).expect("ledger loads");
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
            let err = load_session_data_movement_cmd(malicious.to_string())
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
    #[cfg_attr(
        target_os = "macos",
        ignore = "Tauri/Tao App construction must run on the macOS main thread"
    )]
    fn load_session_replays_projection_state_when_materialized_artifacts_are_missing() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("load-session-missing-projections");
        let _guard = HomeGuard::set(&dir);

        let session_id = "load-session-missing-projections";
        seed_replayable_projection_session(session_id, "Replayed note from event log.");

        let state = AppState::new();
        let loaded = load_session_impl(session_id.to_string(), &state)
            .expect("load session should replay missing materialized projections");

        let notes = loaded.notes.expect("missing notes artifact should replay");
        assert_eq!(notes.last_sequence, 1);
        assert_eq!(notes.notes.len(), 1);
        assert_eq!(notes.notes[0].id, "note-report");
        assert_eq!(notes.notes[0].body, "Replayed note from event log.");

        let graph = loaded
            .materialized_graph
            .expect("missing graph artifact should replay");
        assert_eq!(graph.last_sequence, 1);
        assert_eq!(graph.nodes.len(), 1);
        assert_eq!(graph.nodes[0].id, "node-report");

        let restored = state
            .materialized_projection_state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        assert_eq!(restored.session_id, session_id);
        assert_eq!(restored.notes.last_sequence, 1);
        assert_eq!(restored.graph.last_sequence, 1);

        drain_test_writers(&state);
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
        let timeline = session_timeline(session_id).expect("fold reloaded session timeline");

        let entry = timeline
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
    #[cfg_attr(
        target_os = "macos",
        ignore = "Tauri/Tao App construction must run on the macOS main thread"
    )]
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

        let state = AppState::new();
        let loaded = load_session_impl(session_id.to_string(), &state)
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

        drain_test_writers(&state);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// audio-graph-0b33: a session that never emitted diarization loads an empty
    /// `diarization_events` vec (not an error), so the frontend join is a no-op
    /// and old transcript-only sessions still load.
    #[test]
    #[cfg_attr(
        target_os = "macos",
        ignore = "Tauri/Tao App construction must run on the macOS main thread"
    )]
    fn load_session_diarization_events_empty_without_speaker_log() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("load-session-no-diarization");
        let _guard = HomeGuard::set(&dir);

        let session_id = "load-session-no-diarization";
        seed_replayable_projection_session(session_id, "Note without diarization.");

        let state = AppState::new();
        let loaded = load_session_impl(session_id.to_string(), &state)
            .expect("load session without diarization should still succeed");

        assert!(
            loaded.diarization_events.is_empty(),
            "a session with no speaker log must load an empty diarization_events vec"
        );

        drain_test_writers(&state);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg_attr(
        target_os = "macos",
        ignore = "Tauri/Tao App construction must run on the macOS main thread"
    )]
    fn load_session_replaces_stale_artifacts_and_prior_projection_state() {
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

        let loaded = load_session_impl(session_id.to_string(), &state)
            .expect("load session should replace stale materialized projections");

        let loaded_notes = loaded.notes.expect("stale notes artifact should replay");
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

        let loaded_graph = loaded
            .materialized_graph
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
        assert_eq!(restored.session_id, session_id);
        assert_eq!(restored.notes.last_sequence, 1);
        assert_eq!(restored.graph.last_sequence, 1);
        assert_eq!(restored.notes.notes[0].id, "note-report");
        assert_eq!(restored.graph.nodes[0].id, "node-report");
        assert!(
            restored
                .notes
                .notes
                .iter()
                .all(|note| note.id != "leaked-note"),
            "prior active-session note leaked into loaded materialized state"
        );
        assert!(
            restored
                .graph
                .nodes
                .iter()
                .all(|node| node.id != "leaked-node"),
            "prior active-session graph leaked into loaded materialized state"
        );

        let ledger = state
            .transcript_ledger
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        assert_eq!(ledger.session_id, session_id);
        assert_eq!(ledger.accepted_event_count, 1);

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
    #[cfg_attr(
        target_os = "macos",
        ignore = "Tauri/Tao App construction must run on the macOS main thread"
    )]
    fn load_session_does_not_leak_prior_session_projection_state_across_restart() {
        // Crash/restart leak guard: after loading session A, loading a DIFFERENT
        // session B must fully rotate the materialized projection state to B's
        // artifacts, leaving none of A's notes/graph nodes behind. This models a
        // restart that reuses the same process (AppState) to open two sessions
        // in sequence and proves prior-session projection/graph state cannot
        // leak into the next session.
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("load-session-cross-session-leak");
        let _guard = HomeGuard::set(&dir);

        let session_a = "leak-session-a";
        let session_b = "leak-session-b";
        seed_replayable_projection_session(session_a, "Session A note.");
        seed_replayable_projection_session(session_b, "Session B note.");

        let state = AppState::new();

        // 1. Load session A: its projection state materializes into AppState.
        let loaded_a = load_session_impl(session_a.to_string(), &state)
            .expect("load session A should succeed");
        assert_eq!(
            loaded_a.notes.expect("A notes").notes[0].body,
            "Session A note."
        );
        {
            let materialized = state
                .materialized_projection_state
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            assert_eq!(materialized.session_id, session_a);
            assert_eq!(materialized.notes.notes[0].body, "Session A note.");
        }

        // 2. Load session B in the SAME process. B's artifacts must fully
        //    replace A's — no A note/graph node may survive.
        let loaded_b = load_session_impl(session_b.to_string(), &state)
            .expect("load session B should succeed");
        assert_eq!(
            loaded_b.notes.expect("B notes").notes[0].body,
            "Session B note."
        );

        let materialized = state
            .materialized_projection_state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        assert_eq!(
            materialized.session_id, session_b,
            "materialized state must be rebound to session B"
        );
        assert!(
            materialized
                .notes
                .notes
                .iter()
                .all(|note| note.body != "Session A note."),
            "session A note leaked into session B materialized state"
        );
        assert_eq!(
            materialized.notes.notes.len(),
            1,
            "session B materialized state must hold only session B's single note"
        );

        // The transcript ledger must likewise be rebound to session B.
        let ledger = state
            .transcript_ledger
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        assert_eq!(
            ledger.session_id, session_b,
            "transcript ledger must be rebound to session B"
        );

        drain_test_writers(&state);
        let _ = std::fs::remove_dir_all(&dir);
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
            basis: basis.clone(),
            operations: vec![
                crate::projections::ProjectionOperation::UpsertGraphNode {
                    id: "person:alice".to_string(),
                    name: "Alice".to_string(),
                    entity_type: "person".to_string(),
                    description: None,
                },
                crate::projections::ProjectionOperation::UpsertGraphNode {
                    id: "person:alicia".to_string(),
                    name: "Alicia".to_string(),
                    entity_type: "person".to_string(),
                    description: None,
                },
                crate::projections::ProjectionOperation::UpsertGraphNode {
                    id: "project:audio-graph".to_string(),
                    name: "AudioGraph".to_string(),
                    entity_type: "project".to_string(),
                    description: None,
                },
                crate::projections::ProjectionOperation::UpsertGraphEdge {
                    id: "edge:alice:owns".to_string(),
                    source: "person:alice".to_string(),
                    target: "project:audio-graph".to_string(),
                    relation_type: "owns".to_string(),
                    label: None,
                    weight: 0.8,
                },
                crate::projections::ProjectionOperation::UpsertGraphEdge {
                    id: "edge:alicia:owns".to_string(),
                    source: "person:alicia".to_string(),
                    target: "project:audio-graph".to_string(),
                    relation_type: "owns".to_string(),
                    label: None,
                    weight: 0.6,
                },
            ],
            confidence: 0.9,
            provenance: crate::projections::ProjectionProvenance {
                provider: "test".to_string(),
                model: "projection-report".to_string(),
                prompt_id: "report-graph-v1".to_string(),
            },
            queued_at_ms: None,
            generation_latency_ms: None,
            apply_latency_ms: None,
            created_at_ms: 1_700_000_300_001,
        };
        let retcon_graph_patch = crate::projections::ProjectionPatch {
            sequence: 2,
            kind: crate::projections::ProjectionKind::Graph,
            llm_request_id: "report-graph-retcon".to_string(),
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
            },
            queued_at_ms: None,
            generation_latency_ms: None,
            apply_latency_ms: None,
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
    fn materialized_projection_restore_prefers_replayed_state_when_artifact_missing_or_behind() {
        let mut replayed_state = crate::projections::MaterializedProjectionState::new("session-1");
        replayed_state.notes.last_sequence = 3;
        replayed_state.graph.last_sequence = 4;

        assert_eq!(
            choose_materialized_notes(None, Some(&replayed_state))
                .expect("missing notes should replay")
                .last_sequence,
            3
        );
        assert_eq!(
            choose_materialized_graph(None, Some(&replayed_state))
                .expect("missing graph should replay")
                .last_sequence,
            4
        );

        let mut old_notes = crate::projections::MaterializedNotes::new("session-1");
        old_notes.last_sequence = 1;
        assert_eq!(
            choose_materialized_notes(Some(old_notes), Some(&replayed_state))
                .expect("stale notes should replay")
                .last_sequence,
            3
        );

        let mut current_graph = crate::projections::MaterializedGraph::new("session-1");
        current_graph.last_sequence = 5;
        assert_eq!(
            choose_materialized_graph(Some(current_graph), Some(&replayed_state))
                .expect("current graph artifact should win")
                .last_sequence,
            5
        );

        let empty_replay = crate::projections::MaterializedProjectionState::new("session-empty");
        assert!(
            choose_materialized_notes(None, Some(&empty_replay)).is_none(),
            "empty replay should not fabricate a notes artifact"
        );
        assert!(
            choose_materialized_graph(None, Some(&empty_replay)).is_none(),
            "empty replay should not fabricate a graph artifact"
        );
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

    #[test]
    fn save_credential_empty_value_skips_without_epoch_bump_or_rehydrate() {
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
    fn gemini_api_key_readiness_probe_follows_native_realtime_mode() {
        let descriptor = crate::provider_registry::provider_registry()
            .iter()
            .find(|descriptor| descriptor.id == "realtime_agent.gemini_live")
            .expect("gemini descriptor");
        let settings = crate::settings::AppSettings::default();
        let mut store = crate::credentials::CredentialStore::default();
        store.gemini_api_key = Some("gemini-saved".to_string());

        let notes_ids = active_provider_ids(&settings, false);
        assert!(!notes_ids.contains("realtime_agent.gemini_live"));
        assert!(!should_probe_provider(
            descriptor, &notes_ids, &settings, &store
        ));
        assert_eq!(
            provider_readiness_config_fingerprint(descriptor, &settings, &notes_ids),
            "inactive"
        );

        let native_ids = active_provider_ids(&settings, true);
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
    // `_lock` serializes process-global HOME mutation across tests; it is
    // deliberately held for the whole single-threaded test body, `.await`s
    // included, so the lint is allowed at the function scope.
    #[allow(clippy::await_holding_lock)]
    async fn openrouter_saved_key_catalog_commands_require_saved_credential() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("openrouter-saved-key-missing");
        let _guard = HomeGuard::set(&dir);

        let providers_err =
            list_openrouter_providers_cmd(Some("http://127.0.0.1:9/api/v1".to_string()))
                .await
                .expect_err("missing saved key should fail before provider request");
        assert_openrouter_credential_missing(providers_err);

        let endpoints_err = list_openrouter_model_endpoints_cmd(
            "openai/gpt-4".to_string(),
            Some("http://127.0.0.1:9/api/v1".to_string()),
        )
        .await
        .expect_err("missing saved key should fail before endpoint request");
        assert_openrouter_credential_missing(endpoints_err);

        let _ = std::fs::remove_dir_all(&dir);
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
    }

    #[test]
    fn gemini_api_key_resolution_uses_saved_key_when_draft_is_blank() {
        let mut store = crate::credentials::CredentialStore::default();
        store.gemini_api_key = Some("  AIza-saved  ".to_string());

        let api_key = gemini_api_key_from_store(&store).expect("saved key");

        assert_eq!(api_key, "AIza-saved");
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
    fn soniox_readiness_uses_saved_key_for_non_selectable_planned_provider() {
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
        assert!(should_probe_provider(
            descriptor,
            &active_provider_ids(&settings, false),
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
        {
            state
                .capture_manager
                .lock()
                .expect("capture manager")
                .insert_synthetic_handle("system", false);
            *state.is_capturing.write().unwrap() = true;
            state.is_transcribing.store(true, Ordering::SeqCst);
            *state.is_gemini_active.write().unwrap() = true;
            set_running_capture_pipeline_status(&state);
            register_test_gemini_notes_consumer(&state);
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

        let (status_tx, status_rx) = std::sync::mpsc::channel();
        let listener_id = app_handle.listen_any(events::PIPELINE_STATUS_EVENT, move |event| {
            if let Ok(payload) = serde_json::from_str::<PipelineStatus>(event.payload()) {
                let _ = status_tx.send(payload);
            }
        });

        stop_capture_impl("system".to_string(), &state, &app_handle)
            .await
            .expect("final source stop should succeed");

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
        assert_pipeline_status_idle(&state.pipeline_status.read().unwrap());

        let emitted = status_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("final source stop should emit idle pipeline status");
        assert_pipeline_status_idle(&emitted);

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

        stop_capture_impl("system".to_string(), &state, &app_handle)
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
