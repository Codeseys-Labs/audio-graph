//! Tauri event name constants and payload types.
//!
//! These constants define the event names emitted from the Rust backend
//! to the frontend. The frontend subscribes using `listen()` from `@tauri-apps/api`.

/// Event emitted when a new transcript segment is available.
pub const TRANSCRIPT_UPDATE: &str = "transcript-update";

/// Event emitted when a streaming ASR provider produces an interim hypothesis.
pub const ASR_PARTIAL: &str = "asr-partial";

/// Event emitted when a provider/local ASR path produces a transcript-span
/// revision. This is the normalized, provider-neutral event intended for the
/// event-sourced transcript/notes/graph pipeline. It is emitted alongside the
/// legacy `asr-partial` and `transcript-update` events while the UI migrates.
pub const ASR_SPAN_REVISION: &str = "asr-span-revision";

/// Event emitted when provider or local diarization revises a speaker timeline
/// span. This keeps speaker attribution diffable instead of forcing consumers
/// to infer timeline edits from append-only transcript rows.
pub const DIARIZATION_SPAN_REVISION: &str = "diarization-span-revision";

/// Event emitted when a provider or local fallback identifies speech turn
/// lifecycle boundaries. This is intentionally separate from transcript
/// events: graph/notes can use conservative final boundaries while the
/// speech-to-speech agent can react to eager/cancel/resume signals.
pub const TURN_EVENT: &str = "turn-event";

/// Event emitted when the knowledge graph changes (full snapshot).
///
/// Contract with [`GRAPH_DELTA`]: snapshots are authoritative resync points.
/// Within one backend graph mutation we emit the delta first, then the snapshot
/// if a full refresh is due. The frontend may apply deltas for low-latency
/// updates, but any snapshot replaces the current graph state and therefore
/// supersedes every delta produced by earlier graph mutations. Snapshot
/// receivers should not try to merge a snapshot into stale local graph data,
/// except for view-only fields such as force-layout node positions.
///
/// Emitted less frequently from streaming extraction and immediately after
/// explicit graph actions that need deterministic UI confirmation.
pub const GRAPH_UPDATE: &str = "graph-update";

/// Event emitted with incremental graph changes (delta updates).
///
/// Deltas are ordered best-effort updates between [`GRAPH_UPDATE`] snapshots.
/// They are generated from the graph's in-memory change buffer and cleared by
/// `take_delta()`. A receiver that misses a delta should rely on the next full
/// snapshot to recover; deltas must not be replayed after a newer snapshot has
/// been applied unless a future sequence/basis field proves they belong after
/// that snapshot.
///
/// Emitted on every graph mutation when the mutation changed nodes or edges.
pub const GRAPH_DELTA: &str = "graph-delta";

/// Event emitted after an accepted transcript-derived projection patch has
/// passed runtime validation, persistence, and materializer application.
pub const PROJECTION_PATCH: &str = "projection-patch";

/// Event emitted after a notes projection patch updates the materialized notes
/// artifact for the active session.
pub const MATERIALIZED_NOTES_UPDATE: &str = "materialized-notes-update";

/// Event emitted after a graph projection patch updates the materialized graph
/// artifact for the active session.
pub const MATERIALIZED_GRAPH_UPDATE: &str = "materialized-graph-update";

/// Event emitted periodically (every ~2s) or on status change.
pub const PIPELINE_STATUS_EVENT: &str = "pipeline-status";

/// Event emitted when a pipeline stage completes work and can report elapsed
/// wall-clock time. Kept separate from [`PIPELINE_STATUS_EVENT`] so latency
/// instrumentation can be added incrementally without changing the status
/// enum's serialization shape.
pub const PIPELINE_LATENCY: &str = "pipeline-latency";

/// Event emitted when the agent/react loop changes state.
pub const AGENT_STATUS: &str = "agent-status";

/// Event emitted when the agent/react loop proposes an action or note for
/// the user to inspect. Proposals stay advisory until the user approves them.
pub const AGENT_PROPOSAL: &str = "agent-proposal";

/// Event emitted when a new speaker is first identified.
pub const SPEAKER_DETECTED: &str = "speaker-detected";

/// Event emitted when a capture error occurs.
pub const CAPTURE_ERROR: &str = "capture-error";

/// Event emitted when a persistence write fails because the underlying storage
/// is full (ENOSPC / ERROR_DISK_FULL). The frontend should surface this as a
/// user-visible error so the operator can free disk space before more
/// transcript/graph data is lost.
pub const CAPTURE_STORAGE_FULL: &str = "capture-storage-full";

/// Event emitted when a persistence event-writer queue starts or stops dropping
/// events because the storage sink is not keeping up. Payload intentionally
/// omits session ids, file paths, transcript text, and provider payloads.
pub const PERSISTENCE_QUEUE_BACKPRESSURE: &str = "persistence-queue-backpressure";

/// Event emitted when the backpressure state of a capture source changes —
/// i.e. the rsac ring buffer has started or stopped dropping buffers because
/// the consumer (this app's pipeline) isn't keeping up. Edge-triggered: fires
/// only on transitions (false→true or true→false), not continuously.
pub const CAPTURE_BACKPRESSURE: &str = "capture-backpressure";

/// Event emitted by the processed-audio dispatcher with per-consumer queue
/// health and drop counters.
/// Payload: [`crate::audio::consumer::ProcessedAudioConsumerHealthPayload`].
pub const AUDIO_CONSUMER_HEALTH: &str = "audio-consumer-health";

/// Event emitted when Gemini Live produces a transcription.
pub const GEMINI_TRANSCRIPTION: &str = "gemini-transcription";

/// Event emitted when Gemini Live produces a model response.
pub const GEMINI_RESPONSE: &str = "gemini-response";

/// Event emitted when the Gemini Live connection status changes.
pub const GEMINI_STATUS: &str = "gemini-status";

/// Event emitted with the OpenAI Realtime S2S assistant's spoken-reply
/// transcript (the S2S voice agent, parallel to `GEMINI_RESPONSE`).
pub const OPENAI_REALTIME_RESPONSE: &str = "openai-realtime-response";

/// Event emitted when the OpenAI Realtime S2S connection status changes
/// (connected / disconnected / reconnecting / reconnected / error — same
/// envelope shape the frontend already routes for `GEMINI_STATUS`).
pub const OPENAI_REALTIME_STATUS: &str = "openai-realtime-status";

/// Event emitted throughout a model download with elapsed + byte counters so
/// the frontend can compute an ETA. Throttled to roughly 1 Hz; also fires once
/// on completion or error.
pub const MODEL_DOWNLOAD_PROGRESS: &str = "model-download-progress";

/// Event emitted when an AWS call (Transcribe streaming, STS preflight) fails
/// with a credential- or region-class error that the frontend should surface
/// via a localized toast with recovery guidance (ag#13).
pub const AWS_ERROR: &str = "aws-error";

// The streaming-chat token deltas + terminal frame (previously the
// `chat-token-delta` / `chat-token-done` events, plan A3 / ADR-0006) moved off
// the event system onto a per-invocation `tauri::ipc::Channel<ChatStreamEvent>`
// returned by `start_streaming_chat` (audio-graph-1534) — the event system is
// not designed for the 20-100+/sec per-token throughput. See
// `crate::llm::streaming::ChatStreamEvent`.

/// Event emitted after a chat/LLM completion's provider-reported token usage
/// has been persisted to the session usage file.
/// Payload: [`LlmUsageUpdatePayload`].
pub const LLM_USAGE_UPDATE: &str = "llm-usage-update";

/// Event emitted when runtime privacy policy blocks a content-bearing provider
/// call before audio, transcript, graph context, prompts, or generated text can
/// leave the process.
pub const PRIVACY_POLICY_BLOCKED: &str = "privacy-policy-blocked";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LlmUsageUpdatePayload {
    pub session_id: String,
    pub total_tokens: u64,
    pub session_llm_total: u64,
    pub session_llm_turns: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PrivacyPolicyBlockedPayload {
    pub session_id: Option<String>,
    pub privacy_mode: String,
    pub action: String,
    pub provider: String,
    pub data_classes: Vec<String>,
    pub reason: String,
    pub timestamp_ms: u64,
}

/// Status of an individual pipeline stage.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum StageStatus {
    #[default]
    Idle,
    Running {
        processed_count: u64,
    },
    Error {
        message: String,
    },
}

/// Overall pipeline status, combining all stages.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PipelineStatus {
    pub capture: StageStatus,
    pub pipeline: StageStatus,
    pub asr: StageStatus,
    pub diarization: StageStatus,
    pub entity_extraction: StageStatus,
    pub graph: StageStatus,
}

/// Interim ASR hypothesis from streaming providers.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AsrPartialPayload {
    pub provider: String,
    pub source_id: String,
    pub text: String,
    pub start_time: f64,
    pub end_time: f64,
    pub confidence: f32,
    pub timestamp_ms: u64,
}

/// Stability/finality state for a normalized ASR span revision.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AsrSpanStability {
    Partial,
    Final,
}

/// Provider-neutral transcript span revision.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AsrSpanRevisionPayload {
    /// Stable span id within the provider/source stream when known. Legacy
    /// paths use a deterministic time-based id for partials and the transcript
    /// segment id for final-only paths until provider adapters can supply
    /// stronger identities.
    pub span_id: String,
    pub provider: String,
    pub source_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_item_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript_segment_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    pub text: String,
    pub start_time: f64,
    pub end_time: f64,
    pub confidence: f32,
    pub is_final: bool,
    pub stability: AsrSpanStability,
    /// Monotonic within a span once provider adapters can preserve provider
    /// item identity. First additive slice uses 1 for the emitted revision.
    pub revision_number: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    pub end_of_turn: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_event_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_latency_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asr_latency_ms: Option<u64>,
    pub received_at_ms: u64,
}

/// Stability/finality state for a provider-neutral diarization span revision.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiarizationSpanStability {
    /// A rolling/local or streaming-provider attribution that may be remapped.
    Provisional,
    /// The span has survived a stabilization window but can still be retconned
    /// by later full-session/provider revisions.
    Stable,
    /// The provider or offline reconciliation considers this span complete.
    Final,
}

/// Provider-neutral speaker-timeline span revision.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DiarizationSpanRevisionPayload {
    /// Stable id for the logical speaker span being revised.
    pub span_id: String,
    /// Provider/engine that produced the attribution, e.g. `deepgram`,
    /// `aws_transcribe`, `soniox`, or `local_clustering`.
    pub provider: String,
    /// Logical timeline being revised. Provider diarization may use a source id;
    /// session-level local diarization can use `session`.
    pub timeline_id: String,
    /// Capture source when the attribution is source-local. Session-level local
    /// diarization may leave this unset until multichannel source attribution is
    /// wired.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    pub start_time: f64,
    pub end_time: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    pub is_final: bool,
    pub stability: DiarizationSpanStability,
    pub revision_number: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
    pub basis_asr_span_ids: Vec<String>,
    pub basis_transcript_segment_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_event_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_latency_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asr_latency_ms: Option<u64>,
    pub received_at_ms: u64,
}

/// Normalized turn lifecycle event kind shared by cloud and local providers.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnEventKind {
    SpeechStarted,
    SpeechFinal,
    UtteranceEnd,
    EagerEndOfTurn,
    EndOfTurn,
    TurnResumed,
    LocalWindow,
}

/// Provider-neutral turn lifecycle payload.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TurnEventPayload {
    pub provider: String,
    pub source_id: String,
    pub kind: TurnEventKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_time: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_time: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_index: Option<u64>,
    pub timestamp_ms: u64,
}

/// Per-stage latency sample emitted by backend workers.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PipelineLatencyPayload {
    /// Stage key matching the frontend `PipelineStatus` keys where possible:
    /// `capture`, `pipeline`, `asr`, `diarization`, `entity_extraction`,
    /// `graph`, or a future extension such as `agent`.
    pub stage: String,
    /// Optional source id when the timing belongs to a capture/source path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    /// Optional transcript/audio segment id when the timing belongs to a
    /// logical speech segment.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub segment_id: Option<String>,
    /// Wall-clock duration for the just-completed stage.
    pub latency_ms: f64,
    /// Unix timestamp in milliseconds when the sample was emitted.
    pub timestamp_ms: u64,
}

/// Agent/react loop status state.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatusState {
    Idle,
    Running,
    Error,
}

/// Status update for the agent/react loop.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentStatusPayload {
    pub state: AgentStatusState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_segment_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub timestamp_ms: u64,
}

/// Kind of advisory agent proposal.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentProposalKind {
    Note,
    Question,
    GraphSuggestion,
}

/// Advisory proposal emitted by the agent/react loop.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct AgentProposalPayload {
    pub id: String,
    pub source_segment_id: String,
    pub source_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker_label: Option<String>,
    pub kind: AgentProposalKind,
    pub title: String,
    pub body: String,
    pub confidence: f32,
    pub created_at_ms: u64,
}

/// Result returned after the user approves an agent proposal.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct AgentActionResult {
    pub proposal_id: String,
    pub action: String,
    pub message: String,
    pub graph_updated: bool,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LiveAssistCardStatus {
    Pending,
    Approved,
    Dismissed,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct LiveAssistCardRecord {
    pub session_id: String,
    pub proposal: AgentProposalPayload,
    pub status: LiveAssistCardStatus,
    #[serde(default)]
    pub source_span_ids: Vec<String>,
    #[serde(default)]
    pub graph_context_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<AgentActionResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection_patch_sequence: Option<u64>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

/// Payload for capture error events.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CaptureErrorPayload {
    pub source_id: String,
    pub error: String,
    pub recoverable: bool,
}

/// Payload for `CAPTURE_STORAGE_FULL` events.
///
/// Emitted when a persistence write fails because the underlying storage is
/// full (ENOSPC / ERROR_DISK_FULL). Use the `bytes_lost` field to tell the
/// user how much data failed to hit disk on this attempt; `bytes_written`
/// is best-effort and is `0` when the error happens on the initial open.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CaptureStorageFullPayload {
    /// Absolute path the app tried to write to.
    pub path: String,
    /// Bytes successfully written before the error (best-effort).
    pub bytes_written: u64,
    /// Bytes the app was trying to write when the error occurred (best-effort:
    /// the size of the buffer we were attempting to persist).
    pub bytes_lost: u64,
}

/// Payload for persistence queue pressure transitions.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PersistenceQueueBackpressurePayload {
    /// Stable writer identifier, e.g. `transcript_event` or `projection_event`.
    pub writer: String,
    /// `true` after the writer starts dropping new events because the queue is
    /// full; `false` after a later enqueue succeeds.
    pub is_backpressured: bool,
    /// Configured queue capacity for this writer.
    pub queue_capacity: usize,
    /// Cumulative count of dropped events in this process for the writer handle.
    pub dropped_count: u64,
}

/// Payload for capture-backpressure state-change events.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CaptureBackpressurePayload {
    pub source_id: String,
    /// `true` when the ring buffer has started dropping; `false` when recovery
    /// is detected. The frontend should surface this as a transient warning
    /// (e.g. a pill badge) rather than a fatal error.
    pub is_backpressured: bool,
}

/// Payload for `AWS_ERROR` events (ag#13).
///
/// `error` carries the structured classification (a [`crate::aws_util::UiAwsError`]
/// serialized with `category` / payload fields). `raw_message` is the original aws-sdk
/// error string, kept so the frontend can log or disclose details when the
/// category alone isn't enough (e.g. unexpected `Unknown` bucket).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AwsErrorPayload {
    pub error: crate::aws_util::UiAwsError,
    pub raw_message: String,
}

/// Emit a Tauri event and log any emission failure at `error` level.
///
/// The default `let _ = app.emit(...)` pattern silently swallows emission
/// errors, which makes failed frontend notifications undebuggable. Use this
/// helper instead so failures surface in logs.
pub fn emit_or_log<P>(app: &tauri::AppHandle, event: &str, payload: P)
where
    P: serde::Serialize + Clone,
{
    use tauri::Emitter;
    if let Err(e) = app.emit(event, payload) {
        log::error!("Failed to emit event '{}': {}", event, e);
    }
}

/// Heuristic classifier for capture errors into recoverable vs fatal.
///
/// Used at capture-error emit sites to populate `CaptureErrorPayload.recoverable`.
/// Fatal errors indicate the source cannot be used again without user action
/// (permission, device disconnection). Recoverable errors may succeed on retry.
pub fn classify_capture_error(err: &str) -> bool {
    let lower = err.to_lowercase();
    let fatal_markers = [
        "permission denied",
        "not permitted",
        "unauthorized",
        "disconnected",
        "device not found",
        "no such device",
        "device removed",
        "access denied",
        "not supported",
        "unsupported",
    ];
    if fatal_markers.iter().any(|m| lower.contains(m)) {
        return false;
    }
    // Default to recoverable for unclassified errors — user can retry.
    true
}

/// Returns `true` if this I/O error indicates the underlying storage is full
/// (ENOSPC on Unix, ERROR_DISK_FULL on Windows).
///
/// `std::io::ErrorKind::StorageFull` was stabilised relatively recently and
/// the mapping from raw OS codes into that kind still varies across Rust
/// versions and platforms, so we check both the kind and the `raw_os_error`
/// signatures defensively — whichever trips first wins.
pub fn is_storage_full(err: &std::io::Error) -> bool {
    // Prefer the symbolic kind when available; fall through to raw_os_error
    // if the current toolchain doesn't map the error to `StorageFull` yet.
    if err.kind() == std::io::ErrorKind::StorageFull {
        return true;
    }
    matches!(err.raw_os_error(), Some(28) | Some(112))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_storage_full_detects_enospc() {
        // ENOSPC is 28 on Linux and macOS.
        let err = std::io::Error::from_raw_os_error(28);
        assert!(is_storage_full(&err));
    }

    #[test]
    fn is_storage_full_detects_windows_disk_full() {
        // ERROR_DISK_FULL is 112 on Windows.
        let err = std::io::Error::from_raw_os_error(112);
        assert!(is_storage_full(&err));
    }

    #[test]
    fn is_storage_full_ignores_unrelated_errors() {
        // EACCES / generic errors must not be misclassified as storage-full.
        let err = std::io::Error::from_raw_os_error(13);
        assert!(!is_storage_full(&err));

        let other = std::io::Error::other("boom");
        assert!(!is_storage_full(&other));
    }

    #[test]
    fn asr_span_revision_serializes_snake_case_contract() {
        let payload = AsrSpanRevisionPayload {
            span_id: "deepgram:system:1000-2000".to_string(),
            provider: "deepgram".to_string(),
            source_id: "system".to_string(),
            provider_item_id: Some("provider-item-1".to_string()),
            transcript_segment_id: Some("segment-1".to_string()),
            speaker_id: Some("speaker-0".to_string()),
            speaker_label: Some("Speaker 0".to_string()),
            channel: Some("left".to_string()),
            text: "hello".to_string(),
            start_time: 1.0,
            end_time: 2.0,
            confidence: 0.9,
            is_final: true,
            stability: AsrSpanStability::Final,
            revision_number: 2,
            supersedes: Some("rev-1".to_string()),
            turn_id: Some("turn-1".to_string()),
            end_of_turn: true,
            raw_event_ref: Some("deepgram.results[0]".to_string()),
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms: 1_700_000_000_000,
        };

        let json = serde_json::to_value(payload).expect("serialize payload");
        assert_eq!(json["stability"], "final");
        assert_eq!(json["span_id"], "deepgram:system:1000-2000");
        assert_eq!(json["provider_item_id"], "provider-item-1");
        assert_eq!(json["transcript_segment_id"], "segment-1");
        assert_eq!(json["revision_number"], 2);
        assert_eq!(json["end_of_turn"], true);
        assert_eq!(json["received_at_ms"], 1_700_000_000_000u64);
    }

    #[test]
    fn live_assist_card_record_serializes_status_and_outcome_contract() {
        let card = LiveAssistCardRecord {
            session_id: "session-1".to_string(),
            proposal: AgentProposalPayload {
                id: "card-1".to_string(),
                source_segment_id: "segment-1".to_string(),
                source_id: "default-mic".to_string(),
                speaker_label: Some("Speaker 1".to_string()),
                kind: AgentProposalKind::Question,
                title: "Question from Speaker 1".to_string(),
                body: "Consider answering or linking this question: What changed?".to_string(),
                confidence: 0.92,
                created_at_ms: 1_700_000_000_000,
            },
            status: LiveAssistCardStatus::Approved,
            source_span_ids: vec!["span-1".to_string()],
            graph_context_ids: vec!["node-1".to_string()],
            outcome: Some(AgentActionResult {
                proposal_id: "card-1".to_string(),
                action: "graph_update".to_string(),
                message: "Approved live assist card".to_string(),
                graph_updated: true,
                timestamp_ms: 1_700_000_000_100,
            }),
            projection_patch_sequence: Some(7),
            created_at_ms: 1_700_000_000_000,
            updated_at_ms: 1_700_000_000_100,
        };

        let json = serde_json::to_value(card).expect("serialize live assist card");
        assert_eq!(json["status"], "approved");
        assert_eq!(json["proposal"]["kind"], "question");
        assert_eq!(json["source_span_ids"][0], "span-1");
        assert_eq!(json["outcome"]["action"], "graph_update");
        assert_eq!(json["projection_patch_sequence"], 7);
    }

    #[test]
    fn diarization_span_revision_serializes_snake_case_contract() {
        let payload = DiarizationSpanRevisionPayload {
            span_id: "local_clustering:session:1000-2000:speaker-c-0".to_string(),
            provider: "local_clustering".to_string(),
            timeline_id: "session".to_string(),
            source_id: None,
            speaker_id: Some("speaker-c-0".to_string()),
            speaker_label: Some("Speaker 1".to_string()),
            channel: Some("mixed".to_string()),
            start_time: 1.0,
            end_time: 2.0,
            confidence: None,
            is_final: false,
            stability: DiarizationSpanStability::Provisional,
            revision_number: 1,
            supersedes: None,
            basis_asr_span_ids: vec!["asr:1".to_string()],
            basis_transcript_segment_ids: vec!["segment-1".to_string()],
            raw_event_ref: Some("window_start_sample:16000".to_string()),
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms: 1_700_000_000_000,
        };

        let json = serde_json::to_value(payload).expect("serialize payload");
        assert_eq!(json["stability"], "provisional");
        assert_eq!(json["timeline_id"], "session");
        assert_eq!(
            json["span_id"],
            "local_clustering:session:1000-2000:speaker-c-0"
        );
        assert_eq!(json["speaker_id"], "speaker-c-0");
        assert_eq!(json["basis_asr_span_ids"][0], "asr:1");
        assert!(
            json.get("source_id").is_none(),
            "session-level local timelines should not invent a source id"
        );
        assert!(
            json.get("confidence").is_none(),
            "uncalibrated local confidence should be omitted"
        );
        assert!(
            json.get("capture_latency_ms").is_none(),
            "missing capture latency should be omitted"
        );
        assert!(
            json.get("asr_latency_ms").is_none(),
            "missing ASR latency should be omitted"
        );
    }

    #[test]
    fn persistence_queue_backpressure_payload_is_redacted() {
        let payload = PersistenceQueueBackpressurePayload {
            writer: "transcript_event".to_string(),
            is_backpressured: true,
            queue_capacity: 2048,
            dropped_count: 3,
        };

        let json = serde_json::to_value(payload).expect("serialize payload");
        assert_eq!(json["writer"], "transcript_event");
        assert_eq!(json["is_backpressured"], true);
        assert_eq!(json["queue_capacity"], 2048);
        assert_eq!(json["dropped_count"], 3);
        assert!(
            json.get("session_id").is_none(),
            "queue diagnostics must not expose session ids"
        );
        assert!(
            json.get("path").is_none(),
            "queue diagnostics must not expose local file paths"
        );
        assert!(
            json.get("text").is_none(),
            "queue diagnostics must not expose transcript text"
        );
    }
}
