//! Application state managed by Tauri.
//!
//! `AppState` is registered with `tauri::Builder::manage()` and accessed
//! in command handlers via `State<'_, AppState>`.
//!
//! Some runtime constants still live close to their owners, while user-facing
//! defaults are parsed from `config/default.toml` through `crate::config`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use crate::audio::AudioCaptureManager;
use crate::audio::consumer::{
    ProcessedAudioConsumerDescriptor, ProcessedAudioConsumerRegistration,
    ProcessedAudioConsumerRegistry, ProcessedAudioConsumerStage, ProcessedAudioDropPolicy,
    ProcessedAudioMixingMode, ProcessedAudioSourceFilter,
};
use crate::audio::pipeline::{AudioPipelineInput, ProcessedAudioChunk, ProcessedPipelineMessage};
use crate::events::PipelineStatus;
use crate::gemini::GeminiLiveClient;
use crate::graph::entities::GraphSnapshot;
use crate::graph::extraction::RuleBasedExtractor;
use crate::graph::temporal::TemporalKnowledgeGraph;
use crate::llm::engine::ChatMessage;
use crate::llm::streaming::StreamRegistry;
use crate::llm::{ApiClient, LlmEngine, LlmExecutor, MistralRsEngine, OpenRouterClient};
use crate::openai_realtime::OpenAiRealtimeClient;
use crate::persistence::{
    FileMemoryRepository, LocalMemoryRepository, ProjectionEventWriter, TranscriptEventWriter,
    TranscriptWriter,
};
use crate::projection_scheduler::ProjectionSchedulers;
use crate::projections::{
    AppliedBasisCurrency, MaterializedGraph, MaterializedNotes, MaterializedProjectionApplyOutcome,
    MaterializedProjectionState, ProjectionApplyError, ProjectionBasis, ProjectionKind,
    ProjectionPatch, SpeakerTimeline, TranscriptLedger,
};

/// Transcript segment for frontend consumption.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TranscriptSegment {
    pub id: String,
    pub source_id: String,
    pub speaker_id: Option<String>,
    pub speaker_label: Option<String>,
    pub text: String,
    pub start_time: f64,
    pub end_time: f64,
    pub confidence: f32,
}

pub use audio_graph_ipc_contract::{
    AudioChannelProvenanceKind, AudioDeviceKind, AudioFormatInfo, AudioPermissionKind,
    AudioPermissionRecoveryAction, AudioPermissionRecoveryActionKind, AudioPermissionRecoveryHint,
    AudioPermissionRecoveryPlatform, AudioPermissionStatus, AudioSampleFormat,
    AudioSourceCapabilities, AudioSourceChannelInfo, AudioSourceChannelLayout,
    AudioSourceChannelProvenance, AudioSourceInfo, AudioSourceType,
};

/// Speaker information for the frontend.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SpeakerInfo {
    pub id: String,
    pub label: String,
    pub color: String,
    pub total_speaking_time: f64,
    pub segment_count: u32,
}

/// Live projection job thread handles, keyed by `(kind, job_id)`
/// (audio-graph-9cc1 / ADR-0045 decision 4, drain half).
///
/// Named alias for `clippy::type_complexity` — see
/// [`AppState::projection_job_workers`] for the full rationale.
pub type ProjectionJobRegistry =
    Arc<Mutex<Vec<(ProjectionKind, String, std::thread::JoinHandle<()>)>>>;

/// Central application state, shared across Tauri commands and worker threads.
pub struct AppState {
    /// Unique session ID for the currently-active session (UUID v4).
    ///
    /// Wrapped in `Arc<RwLock<...>>` so `new_session_cmd` can rotate the ID
    /// in-process without restarting the app. Persistence threads that were
    /// spawned with a clone of this `Arc` re-read the current ID on each
    /// tick / write, so rotation takes effect without respawning them
    /// (transcript writer is the exception — it owns a file handle and is
    /// respawned on rotation, see [`AppState::rotate_session`]).
    pub session_id: Arc<RwLock<String>>,

    /// Buffer of transcript segments (most recent last).
    pub transcript_buffer: Arc<RwLock<VecDeque<TranscriptSegment>>>,

    /// Async transcript writer (appends to JSONL file on disk).
    pub transcript_writer: Arc<Mutex<Option<TranscriptWriter>>>,

    /// Async transcript event writer (appends immutable span revisions to JSONL).
    pub transcript_event_writer: Arc<Mutex<Option<TranscriptEventWriter>>>,

    /// Canonical transcript span ledger for projection-basis checks.
    pub transcript_ledger: Arc<Mutex<TranscriptLedger>>,

    /// Provider-neutral diarization span ledger for the active session.
    pub speaker_timeline: Arc<Mutex<SpeakerTimeline>>,

    /// Current materialized notes/graph projection state for the active session.
    pub materialized_projection_state: Arc<Mutex<MaterializedProjectionState>>,

    /// Runtime notes/graph projection schedulers for the active session.
    pub projection_schedulers: Arc<Mutex<ProjectionSchedulers>>,

    /// Async projection event writer (appends replayable notes/graph patches to JSONL).
    pub projection_event_writer: Arc<Mutex<Option<ProjectionEventWriter>>>,

    /// Live projection job thread handles, keyed by `(kind, job_id)`
    /// (audio-graph-9cc1 / ADR-0045 decision 4, drain half).
    ///
    /// `spawn_projection_job` registers `(kind, job_id, handle)` here instead
    /// of discarding the handle; `run_projection_job` self-deregisters its own
    /// entry on every exit path via an RAII guard, matched by BOTH `kind` and
    /// `job_id` — a same-kind follow-up job spawned before the finishing job's
    /// guard runs must never be mistaken for it and joined/dropped early.
    /// `stop_capture_impl` drains this registry at Stop (see
    /// `PROJECTION_JOB_FLUSH_TIMEOUT` in `commands.rs`), so a projection job
    /// thread — Notes or Graph — can no longer outlive Stop the way the prior
    /// fire-and-forget spawn allowed. This specifically closes the drain gap
    /// for the Graph lane, whose backlog is unbounded by design during
    /// continuous speech (ADR-0045 decision 4).
    ///
    /// Kept separate from `retired_session_workers`: that vec fences
    /// Start/New Session while non-empty, but a live (not-yet-timed-out)
    /// projection job is ordinary in-flight work, not a stop-time straggler.
    /// Only handles that exceed the flush timeout spill into
    /// `retired_session_workers`.
    pub projection_job_workers: ProjectionJobRegistry,

    /// Set at Stop, before the projection job registry above is drained
    /// (ADR-0045 decision 4). This seed only sets the flag; the deferred-retry
    /// lane (ADR-0045 decision 3) is the intended reader — a scheduled retry
    /// thread polls this so it wakes and exits instead of firing after Stop
    /// has already begun tearing the session down.
    pub projection_lane_stopping: Arc<AtomicBool>,

    /// Current knowledge graph snapshot.
    pub graph_snapshot: Arc<RwLock<GraphSnapshot>>,

    /// Handle to the graph auto-save background thread.
    pub graph_autosave_thread: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,

    /// Current pipeline status.
    pub pipeline_status: Arc<RwLock<PipelineStatus>>,

    /// Whether capture is currently active.
    pub is_capturing: Arc<RwLock<bool>>,

    /// Whether transcribe mode is active (AtomicBool for lock-free flag checks
    /// from the speech processor thread — fixes Bug 2: stop_transcribe now
    /// actually terminates the speech processor).
    pub is_transcribing: Arc<AtomicBool>,

    // ── Knowledge graph infrastructure ──────────────────────────────────
    /// The temporal knowledge graph engine.
    pub knowledge_graph: Arc<Mutex<TemporalKnowledgeGraph>>,

    /// Rule-based entity extractor (fallback when no LLM available).
    pub graph_extractor: Arc<RuleBasedExtractor>,

    /// Native LLM engine for entity extraction + chat.
    pub llm_engine: Arc<Mutex<Option<LlmEngine>>>,

    /// OpenAI-compatible API client (alternative to native LLM).
    pub api_client: Arc<Mutex<Option<ApiClient>>>,

    /// OpenRouter chat-completion client (first-class provider — ADR-0005).
    /// Synced from `LlmProvider::OpenRouter` settings; remains `None` for
    /// any other LLM provider.
    pub openrouter_client: Arc<Mutex<Option<OpenRouterClient>>>,

    /// mistral.rs engine for entity extraction + chat (Candle backend).
    pub mistralrs_engine: Arc<Mutex<Option<MistralRsEngine>>>,

    /// Priority executor for LLM-backed chat and background entity extraction.
    pub llm_executor: LlmExecutor,

    /// Registry of in-flight streaming-chat requests keyed by `request_id`.
    /// `start_streaming_chat` inserts a `(request_id, CancellationToken)`
    /// pair; the stream task removes it on completion;
    /// `cancel_streaming_chat` removes-and-fires-the-token in one step
    /// (plan A3 / ADR-0006).
    pub stream_registry: StreamRegistry,

    /// TTS audio playback (Wave B / audio-graph-8d75 / ADR-0004 consumer).
    /// Owns a dedicated `std::thread` running the cpal `Stream`. Exposed as
    /// a tauri::State so commands + the speak-aloud streaming task can push
    /// PCM samples into it.
    pub audio_player: crate::playback::AudioPlayer,

    /// Chat message history for the sidebar.
    pub chat_history: Arc<RwLock<Vec<ChatMessage>>>,

    /// Agent/react proposals that are awaiting user approval.
    pub pending_agent_proposals: Arc<Mutex<HashMap<String, crate::events::AgentProposalPayload>>>,

    // ── Audio capture infrastructure ────────────────────────────────────
    /// The capture manager (behind Mutex because AudioCaptureManager has &mut self methods).
    pub capture_manager: Arc<Mutex<AudioCaptureManager>>,

    /// Sender side of the raw audio channel (capture → pipeline).
    pub pipeline_tx: crossbeam_channel::Sender<AudioPipelineInput>,

    /// Receiver side — cloneable, workers call `.clone()` to get their own handle.
    pub pipeline_rx: crossbeam_channel::Receiver<AudioPipelineInput>,

    /// Sender for processed audio (pipeline → downstream ASR).
    pub processed_tx: crossbeam_channel::Sender<ProcessedPipelineMessage>,

    /// Receiver for processed audio — used by the dispatcher thread.
    pub processed_rx: crossbeam_channel::Receiver<ProcessedPipelineMessage>,

    /// Registry of active processed-audio consumers fed by the dispatcher.
    pub processed_audio_consumers: Arc<ProcessedAudioConsumerRegistry>,

    /// Handle to the pipeline worker thread.
    pub pipeline_thread: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,

    // ── Processed-audio consumers ──────────────────────────────────────
    // The pipeline emits to `processed_tx` → `processed_rx`. A dispatcher
    // thread reads from `processed_rx` and fans out through
    // `processed_audio_consumers`. Long-lived speech uses the fixed channel
    // below; provider modes such as Gemini notes/converse register their own
    // runtime channels when started.
    /// Per-speech-processor channel (dispatcher → speech processor).
    pub speech_audio_tx: crossbeam_channel::Sender<ProcessedAudioChunk>,
    pub speech_audio_rx: crossbeam_channel::Receiver<ProcessedAudioChunk>,

    /// Handle to the dispatcher thread that fans out processed audio.
    pub dispatcher_thread: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,

    // ── Speech processing pipeline ─────────────────────────────────────
    /// Handle to the speech processor (ASR + diarization) orchestrator thread.
    pub speech_processor_thread: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,

    /// Handle to the ASR worker thread (decoupled from accumulator).
    pub asr_worker_thread: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,

    // ── Gemini Live pipeline ───────────────────────────────────────────────
    /// Whether the Gemini Live pipeline is active.
    pub is_gemini_active: Arc<RwLock<bool>>,

    /// The Gemini Live client instance (created on start_gemini, dropped on stop).
    pub gemini_client: Arc<Mutex<Option<GeminiLiveClient>>>,

    /// Handle to the Gemini audio sender thread.
    pub gemini_audio_thread: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,

    /// Handle to the Gemini event receiver thread.
    pub gemini_event_thread: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,

    // ── Converse mode (native S2S, B18 / ADR-0018) ─────────────────────────
    /// Whether a converse session (native speech-to-speech) is active. Distinct
    /// from `is_gemini_active` (the notes/graph TEXT pipeline) so the two modes
    /// can be reasoned about independently.
    pub is_converse_active: Arc<RwLock<bool>>,

    /// Per-turn capture gate for converse mode. The audio-sender thread streams
    /// to the engine only while this is `true`; the `ConverseDriver` toggles it
    /// via `StartCapture`/`StopCapture` so a barge-in can actually stop the mic
    /// (B18 step 5). On the Gemini server-VAD path capture stays open during
    /// `Speaking`, so this is primarily the OpenAI/client-VAD lever.
    pub converse_capture_gate: Arc<AtomicBool>,

    /// Handle to the converse event-driver thread (drives the `TurnMachine`).
    pub converse_thread: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,

    /// Handle to the converse audio-sender thread. **Distinct** from
    /// [`Self::gemini_audio_thread`] (AUD-CV1 / finding #48): the converse and
    /// notes modes must never share a sender slot or processed-audio channel.
    /// Each mode registers a runtime consumer with the audio registry when
    /// started.
    pub converse_audio_thread: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,

    // ── OpenAI Realtime S2S voice agent (cloud-native, parallel to converse) ─
    /// Whether an OpenAI Realtime S2S voice-agent session is active. Distinct
    /// from `is_converse_active` (the Gemini native-S2S path) and
    /// `is_gemini_active` (notes/graph TEXT) so all three modes are reasoned
    /// about independently and never share runtime slots.
    pub is_openai_realtime_active: Arc<RwLock<bool>>,

    /// The OpenAI Realtime S2S client (created on `start_openai_realtime`,
    /// dropped on `stop_openai_realtime`). Sibling of [`Self::gemini_client`].
    pub openai_realtime_client: Arc<Mutex<Option<OpenAiRealtimeClient>>>,

    /// Per-turn capture gate for the OpenAI Realtime S2S path (server-VAD bridge
    /// / client-VAD lever), parallel to [`Self::converse_capture_gate`].
    pub openai_realtime_capture_gate: Arc<AtomicBool>,

    /// Handle to the OpenAI Realtime S2S audio-sender thread. **Distinct** from
    /// the converse and notes sender slots — each S2S mode owns its own.
    pub openai_realtime_audio_thread: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,

    /// Handle to the OpenAI Realtime S2S event-driver thread (drives the
    /// `ConverseDriver` from the S2S event stream).
    pub openai_realtime_event_thread: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,

    // ── Settings ─────────────────────────────────────────────────────────
    /// Persisted application settings (ASR provider, LLM config, audio params).
    pub app_settings: Arc<RwLock<crate::settings::AppSettings>>,

    /// Serializes every transition that can start/stop content producers or
    /// rotate the active session. This closes the check-then-start window where
    /// `new_session_cmd` could observe Idle while `start_capture` concurrently
    /// began writing through the old session's canonical writer handles.
    pub session_lifecycle: Arc<tokio::sync::Mutex<()>>,

    /// Worker handles that exceeded the bounded Stop join budget but may still
    /// hold clones of session-scoped ledgers, writers, or audio receivers.
    ///
    /// Stop remains responsive by parking timed-out handles here instead of
    /// blocking forever. Start and New Session paths reap finished handles and
    /// reject while any remain live, so a late ASR/provider write can never
    /// cross into a newly published session.
    pub retired_session_workers: Arc<Mutex<Vec<std::thread::JoinHandle<()>>>>,

    /// Guard flag preventing concurrent `rotate_session` calls from racing.
    ///
    /// `rotate_session` uses `compare_exchange(false, true)` to claim the
    /// rotation slot; concurrent callers see `AlreadyRotating` and back off
    /// rather than double-shutting-down the transcript writer or racing on
    /// the `session_id` write lock.
    pub rotation_in_progress: Arc<AtomicBool>,

    /// Set of model filenames with an in-flight download (AUD-MDL1 / #58, P2).
    ///
    /// Without this guard two `download_model_cmd` callers could race the same
    /// target file — both would write to the same `.download` temp + rename,
    /// corrupting each other's bytes or fighting over the final rename.
    /// `download_model_cmd` inserts the filename before `spawn_blocking` and an
    /// RAII guard removes it on completion; a duplicate request is rejected with
    /// an "already downloading" error rather than racing.
    pub downloads_in_flight: Arc<Mutex<HashSet<String>>>,

    /// Graceful-shutdown stop signal for the graph auto-save daemon.
    ///
    /// The autosave loop (see
    /// [`spawn_graph_autosave`](crate::persistence::spawn_graph_autosave))
    /// polls this every ~500ms and exits promptly when set. The
    /// `RunEvent::Exit` handler flips it, joins the thread within a bounded
    /// budget, and then performs ONE final synchronous save
    /// ([`autosave_final_save`](crate::persistence::autosave_final_save)) —
    /// otherwise a clean File→Quit would lose up to ~30s of derived-graph
    /// state to the next-missed autosave tick.
    pub autosave_stop: Arc<AtomicBool>,
}

/// Outcome of a `rotate_session` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RotateOutcome {
    /// Rotation completed; returns the previous session ID that was swapped out.
    Rotated(String),
    /// Another rotation is already in progress; returns the current session ID
    /// (which is either the target of the in-flight rotation or the pre-existing
    /// one — either way, the caller should treat it as "a rotation just happened").
    AlreadyRotating(String),
    /// Required canonical writers for the new session could not be prepared;
    /// the currently published session and its writers remain unchanged.
    Failed {
        current_session_id: String,
        reason: String,
    },
}

impl RotateOutcome {
    /// Convenience: the session ID that was swapped out if we rotated, or the
    /// current ID if rotation was skipped. Callers that just want "whatever was
    /// there before" can use this.
    pub fn previous_or_current(&self) -> &str {
        match self {
            RotateOutcome::Rotated(prev) => prev,
            RotateOutcome::AlreadyRotating(curr) => curr,
            RotateOutcome::Failed {
                current_session_id, ..
            } => current_session_id,
        }
    }
}

/// Successful runtime application of a transcript-derived notes/graph patch.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ProjectionRuntimeApplyResult {
    pub session_id: String,
    pub outcome: MaterializedProjectionApplyOutcome,
    pub projection_event_enqueued: bool,
    /// Whether the rebuildable materialized JSON cache was refreshed. A false
    /// value means the canonical patch was accepted and live state advanced,
    /// but restart must rebuild the snapshot from the event stream.
    pub materialized_snapshot_saved: bool,
    /// What the gate proved about the patch's basis when it applied —
    /// `Current` or a proven append-only tail (audio-graph-caad /
    /// audio-graph-f3d4). Telemetry substrate: callers split
    /// applied-append-only from the ordinary current-basis path without
    /// re-deriving the classification themselves.
    pub basis_currency_at_apply: AppliedBasisCurrency,
}

/// Why a runtime projection patch was rejected before becoming active state.
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProjectionRuntimeApplyError {
    RotationInProgress {
        current_session_id: String,
    },
    SessionMismatch {
        expected_session_id: String,
        current_session_id: String,
        ledger_session_id: String,
        materialized_session_id: String,
    },
    PatchBasisMismatch {
        // Boxed to keep the error enum (and `Result<_, _>` returns) small;
        // `ProjectionBasis` is ~144 bytes and only ever constructed here.
        expected: Box<ProjectionBasis>,
        actual: Box<ProjectionBasis>,
    },
    Apply {
        error: ProjectionApplyError,
    },
    SaveMaterializedNotes {
        session_id: String,
        error: String,
    },
    SaveMaterializedGraph {
        session_id: String,
        error: String,
    },
    ProjectionEventWriterUnavailable {
        session_id: String,
    },
    ProjectionEventEnqueueFailed {
        session_id: String,
    },
    SessionChangedDuringApply {
        expected_session_id: String,
        current_session_id: String,
    },
}

/// Cloneable subset of `AppState` needed by background projection workers.
///
/// Speech ingestion should not depend on the full Tauri state object. This
/// handle keeps projection patch generation on the runtime-owned ledger,
/// materializers, event writer, and rotation guard.
#[derive(Clone)]
pub struct ProjectionRuntimeHandle {
    session_id: Arc<RwLock<String>>,
    rotation_in_progress: Arc<AtomicBool>,
    transcript_ledger: Arc<Mutex<TranscriptLedger>>,
    materialized_projection_state: Arc<Mutex<MaterializedProjectionState>>,
    projection_event_writer: Arc<Mutex<Option<ProjectionEventWriter>>>,
}

impl ProjectionRuntimeHandle {
    #[cfg(test)]
    pub(crate) fn in_memory_for_tests(session_id: impl Into<String>) -> Self {
        let session_id = session_id.into();
        Self {
            session_id: Arc::new(RwLock::new(session_id.clone())),
            rotation_in_progress: Arc::new(AtomicBool::new(false)),
            transcript_ledger: Arc::new(Mutex::new(TranscriptLedger::new(session_id.clone()))),
            materialized_projection_state: Arc::new(Mutex::new(MaterializedProjectionState::new(
                session_id,
            ))),
            projection_event_writer: Arc::new(Mutex::new(None)),
        }
    }

    pub fn current_session_id(&self) -> String {
        match self.session_id.read() {
            Ok(g) => g.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    pub fn transcript_ledger_snapshot(&self) -> TranscriptLedger {
        match self.transcript_ledger.lock() {
            Ok(ledger) => ledger.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    pub fn next_projection_sequence(&self, kind: &ProjectionKind) -> u64 {
        let materialized = match self.materialized_projection_state.lock() {
            Ok(materialized) => materialized,
            Err(poisoned) => poisoned.into_inner(),
        };
        match kind {
            ProjectionKind::Notes => materialized.notes.last_sequence.saturating_add(1),
            ProjectionKind::Graph => materialized.graph.last_sequence.saturating_add(1),
        }
    }

    pub fn materialized_projection_snapshot(&self) -> MaterializedProjectionState {
        match self.materialized_projection_state.lock() {
            Ok(materialized) => materialized.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    pub fn apply_runtime_projection_patch(
        &self,
        expected_session_id: &str,
        expected_basis: &ProjectionBasis,
        patch: ProjectionPatch,
    ) -> Result<ProjectionRuntimeApplyResult, ProjectionRuntimeApplyError> {
        let repository = FileMemoryRepository::user_data();
        self.apply_runtime_projection_patch_with_savers(
            expected_session_id,
            expected_basis,
            patch,
            |session_id, notes| repository.save_materialized_notes(session_id, notes),
            |session_id, graph| repository.save_materialized_graph(session_id, graph),
        )
    }

    fn apply_runtime_projection_patch_with_savers<SaveNotes, SaveGraph>(
        &self,
        expected_session_id: &str,
        expected_basis: &ProjectionBasis,
        patch: ProjectionPatch,
        save_notes: SaveNotes,
        save_graph: SaveGraph,
    ) -> Result<ProjectionRuntimeApplyResult, ProjectionRuntimeApplyError>
    where
        SaveNotes: FnMut(&str, &MaterializedNotes) -> Result<(), String>,
        SaveGraph: FnMut(&str, &MaterializedGraph) -> Result<(), String>,
    {
        let ledger = self.transcript_ledger_snapshot();
        self.apply_runtime_projection_patch_with_ledger_and_savers(
            expected_session_id,
            expected_basis,
            patch,
            &ledger,
            save_notes,
            save_graph,
        )
    }

    fn apply_runtime_projection_patch_with_ledger_and_savers<SaveNotes, SaveGraph>(
        &self,
        expected_session_id: &str,
        expected_basis: &ProjectionBasis,
        mut patch: ProjectionPatch,
        ledger: &TranscriptLedger,
        mut save_notes: SaveNotes,
        mut save_graph: SaveGraph,
    ) -> Result<ProjectionRuntimeApplyResult, ProjectionRuntimeApplyError>
    where
        SaveNotes: FnMut(&str, &MaterializedNotes) -> Result<(), String>,
        SaveGraph: FnMut(&str, &MaterializedGraph) -> Result<(), String>,
    {
        if self.rotation_in_progress.load(Ordering::SeqCst) {
            return Err(ProjectionRuntimeApplyError::RotationInProgress {
                current_session_id: self.current_session_id(),
            });
        }

        let current_session_id = self.current_session_id();

        let mut materialized_guard = match self.materialized_projection_state.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };

        if current_session_id != expected_session_id
            || ledger.session_id != expected_session_id
            || materialized_guard.session_id != expected_session_id
        {
            return Err(ProjectionRuntimeApplyError::SessionMismatch {
                expected_session_id: expected_session_id.to_string(),
                current_session_id,
                ledger_session_id: ledger.session_id.clone(),
                materialized_session_id: materialized_guard.session_id.clone(),
            });
        }

        if patch.basis != *expected_basis {
            return Err(ProjectionRuntimeApplyError::PatchBasisMismatch {
                expected: Box::new(expected_basis.clone()),
                actual: Box::new(patch.basis),
            });
        }

        {
            let guard = match self.projection_event_writer.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            if guard.is_none() {
                return Err(
                    ProjectionRuntimeApplyError::ProjectionEventWriterUnavailable {
                        session_id: expected_session_id.to_string(),
                    },
                );
            }
        }

        let apply_started = Instant::now();
        let mut next_materialized = materialized_guard.clone();
        let (outcome, basis_currency_at_apply) = next_materialized
            .apply_validated_patch_reporting_currency(ledger, &patch)
            .map_err(|error| ProjectionRuntimeApplyError::Apply { error })?;

        patch.apply_latency_ms.get_or_insert(
            apply_started
                .elapsed()
                .as_millis()
                .min(u128::from(u64::MAX)) as u64,
        );

        let projection_event_enqueued = {
            let guard = match self.projection_event_writer.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            let Some(writer) = guard.as_ref() else {
                return Err(
                    ProjectionRuntimeApplyError::ProjectionEventWriterUnavailable {
                        session_id: expected_session_id.to_string(),
                    },
                );
            };
            writer.append(&patch)
        };
        if !projection_event_enqueued {
            return Err(ProjectionRuntimeApplyError::ProjectionEventEnqueueFailed {
                session_id: expected_session_id.to_string(),
            });
        }

        let snapshot_save = match &patch.kind {
            ProjectionKind::Notes => save_notes(expected_session_id, &next_materialized.notes),
            ProjectionKind::Graph => save_graph(expected_session_id, &next_materialized.graph),
        };
        let materialized_snapshot_saved = match snapshot_save {
            Ok(()) => true,
            Err(error) => {
                // The projection event is the canonical accepted record; the
                // JSON snapshot is a rebuildable cache. Returning an apply
                // failure here used to leave live last_sequence stale after an
                // accepted event, so the next patch reused that sequence and
                // made canonical replay fail. Advance live state and surface
                // cache lag explicitly instead.
                log::warn!(
                    "Materialized projection snapshot lagging after canonical enqueue session_id={} kind={:?}: {}",
                    expected_session_id,
                    patch.kind,
                    error
                );
                false
            }
        };

        let current_session_id = self.current_session_id();
        if self.rotation_in_progress.load(Ordering::SeqCst)
            || current_session_id != expected_session_id
        {
            return Err(ProjectionRuntimeApplyError::SessionChangedDuringApply {
                expected_session_id: expected_session_id.to_string(),
                current_session_id,
            });
        }

        *materialized_guard = next_materialized;
        Ok(ProjectionRuntimeApplyResult {
            session_id: expected_session_id.to_string(),
            outcome,
            projection_event_enqueued,
            materialized_snapshot_saved,
            basis_currency_at_apply,
        })
    }
}

impl AppState {
    /// Create a new `AppState` with empty defaults.
    pub fn new() -> Self {
        // Bounded channels prevent OOM if downstream consumers stall.
        // Capacities chosen per architecture spec:
        //   pipeline: 64 chunks (~2s of audio at 32ms/chunk)
        //   processed: 16 chunks (processing is quick)
        let (pipeline_tx, pipeline_rx) = crossbeam_channel::bounded::<AudioPipelineInput>(64);
        let (processed_tx, processed_rx) =
            crossbeam_channel::bounded::<ProcessedPipelineMessage>(16);

        // Per-consumer fan-out channels (Bug 1 fix):
        // Each downstream consumer gets its own channel so both receive ALL chunks.
        // Speech channel sized at 1024 (~32s at 32ms/chunk) to absorb ASR latency
        // spikes. Cloud ASR providers (OpenAI Whisper, Groq) can take 1–5s per
        // request; at 256 chunks (~8s) a single slow burst would overflow the
        // channel and drop audio. 1024 gives the accumulator enough headroom
        // to keep producing segments while the ASR worker waits on HTTP.
        let (speech_audio_tx, speech_audio_rx) =
            crossbeam_channel::bounded::<ProcessedAudioChunk>(1024);
        let is_transcribing = Arc::new(AtomicBool::new(false));
        let is_gemini_active = Arc::new(RwLock::new(false));
        let processed_audio_consumers = Arc::new(ProcessedAudioConsumerRegistry::new());
        if let Err(e) = processed_audio_consumers.register(ProcessedAudioConsumerRegistration {
            descriptor: ProcessedAudioConsumerDescriptor {
                id: "speech".to_string(),
                stage: ProcessedAudioConsumerStage::Speech,
                provider: None,
                conflict_group: None,
                capacity: 1024,
                drop_policy: ProcessedAudioDropPolicy::DropOldest,
                source_filter: ProcessedAudioSourceFilter::All,
                mixing_mode: ProcessedAudioMixingMode::PerSource,
            },
            tx: speech_audio_tx.clone(),
            drain_rx: speech_audio_rx.clone(),
            is_active: {
                let is_transcribing = is_transcribing.clone();
                Arc::new(move || is_transcribing.load(Ordering::Relaxed))
            },
        }) {
            log::warn!("Failed to register speech audio consumer: {}", e);
        }

        let session_id = uuid::Uuid::new_v4().to_string();

        // Spawn transcript writers (best-effort — if base dir is unavailable, None)
        let transcript_writer = TranscriptWriter::spawn(&session_id);
        let transcript_event_writer = TranscriptEventWriter::spawn(&session_id);
        let projection_event_writer = ProjectionEventWriter::spawn(&session_id);
        let transcript_ledger = TranscriptLedger::new(session_id.clone());
        let speaker_timeline = SpeakerTimeline::new(session_id.clone());
        let materialized_projection_state = MaterializedProjectionState::new(session_id.clone());
        let projection_schedulers = ProjectionSchedulers::new(session_id.clone());
        if transcript_writer.is_some() {
            log::info!("Transcript persistence enabled for session {}", session_id);
        } else {
            log::warn!("Transcript persistence disabled (could not resolve data directory)");
        }
        if transcript_event_writer.is_some() {
            log::info!(
                "Transcript event persistence enabled for session {}",
                session_id
            );
        } else {
            log::warn!("Transcript event persistence disabled (could not resolve data directory)");
        }
        if projection_event_writer.is_some() {
            log::info!(
                "Projection event persistence enabled for session {}",
                session_id
            );
        } else {
            log::warn!("Projection event persistence disabled (could not resolve data directory)");
        }

        let llm_engine = Arc::new(Mutex::new(None));
        let api_client = Arc::new(Mutex::new(None));
        let openrouter_client = Arc::new(Mutex::new(None));
        let mistralrs_engine = Arc::new(Mutex::new(None));
        let llm_executor = LlmExecutor::new(
            llm_engine.clone(),
            api_client.clone(),
            openrouter_client.clone(),
            mistralrs_engine.clone(),
        );

        Self {
            session_id: Arc::new(RwLock::new(session_id)),
            transcript_buffer: Arc::new(RwLock::new(VecDeque::with_capacity(500))),
            transcript_writer: Arc::new(Mutex::new(transcript_writer)),
            transcript_event_writer: Arc::new(Mutex::new(transcript_event_writer)),
            transcript_ledger: Arc::new(Mutex::new(transcript_ledger)),
            speaker_timeline: Arc::new(Mutex::new(speaker_timeline)),
            materialized_projection_state: Arc::new(Mutex::new(materialized_projection_state)),
            projection_schedulers: Arc::new(Mutex::new(projection_schedulers)),
            projection_event_writer: Arc::new(Mutex::new(projection_event_writer)),
            projection_job_workers: Arc::new(Mutex::new(Vec::new())),
            projection_lane_stopping: Arc::new(AtomicBool::new(false)),
            graph_snapshot: Arc::new(RwLock::new(GraphSnapshot::default())),
            graph_autosave_thread: Arc::new(Mutex::new(None)),
            pipeline_status: Arc::new(RwLock::new(PipelineStatus::default())),
            is_capturing: Arc::new(RwLock::new(false)),
            is_transcribing,
            knowledge_graph: Arc::new(Mutex::new(TemporalKnowledgeGraph::new())),
            graph_extractor: Arc::new(RuleBasedExtractor::new()),
            llm_engine,
            api_client,
            openrouter_client,
            mistralrs_engine,
            llm_executor,
            stream_registry: StreamRegistry::new(),
            audio_player: crate::playback::AudioPlayer::new(),
            chat_history: Arc::new(RwLock::new(Vec::new())),
            pending_agent_proposals: Arc::new(Mutex::new(HashMap::new())),
            capture_manager: Arc::new(Mutex::new(AudioCaptureManager::new())),
            pipeline_tx,
            pipeline_rx,
            processed_tx,
            processed_rx,
            processed_audio_consumers,
            speech_audio_tx,
            speech_audio_rx,
            dispatcher_thread: Arc::new(Mutex::new(None)),
            pipeline_thread: Arc::new(Mutex::new(None)),
            speech_processor_thread: Arc::new(Mutex::new(None)),
            asr_worker_thread: Arc::new(Mutex::new(None)),
            is_gemini_active,
            gemini_client: Arc::new(Mutex::new(None)),
            gemini_audio_thread: Arc::new(Mutex::new(None)),
            gemini_event_thread: Arc::new(Mutex::new(None)),
            is_converse_active: Arc::new(RwLock::new(false)),
            converse_capture_gate: Arc::new(AtomicBool::new(false)),
            converse_thread: Arc::new(Mutex::new(None)),
            converse_audio_thread: Arc::new(Mutex::new(None)),
            is_openai_realtime_active: Arc::new(RwLock::new(false)),
            openai_realtime_client: Arc::new(Mutex::new(None)),
            openai_realtime_capture_gate: Arc::new(AtomicBool::new(false)),
            openai_realtime_audio_thread: Arc::new(Mutex::new(None)),
            openai_realtime_event_thread: Arc::new(Mutex::new(None)),
            app_settings: Arc::new(RwLock::new(crate::settings::AppSettings::default())),
            session_lifecycle: Arc::new(tokio::sync::Mutex::new(())),
            retired_session_workers: Arc::new(Mutex::new(Vec::new())),
            rotation_in_progress: Arc::new(AtomicBool::new(false)),
            downloads_in_flight: Arc::new(Mutex::new(HashSet::new())),
            autosave_stop: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Read the current session ID. On lock poisoning, recovers the inner
    /// value — session_id is a plain String so poisoning carries no
    /// invariant-violation risk.
    pub fn current_session_id(&self) -> String {
        match self.session_id.read() {
            Ok(g) => g.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    pub fn projection_runtime_handle(&self) -> ProjectionRuntimeHandle {
        ProjectionRuntimeHandle {
            session_id: self.session_id.clone(),
            rotation_in_progress: self.rotation_in_progress.clone(),
            transcript_ledger: self.transcript_ledger.clone(),
            materialized_projection_state: self.materialized_projection_state.clone(),
            projection_event_writer: self.projection_event_writer.clone(),
        }
    }

    /// Apply one accepted notes/graph projection patch for the active session.
    ///
    /// Callers must pass the session id and basis from the queued
    /// `ProjectionJob`; the patch is rejected if model output rewrites either
    /// boundary. Transcript ingestion is not blocked during disk I/O: the
    /// ledger is cloned for validation, while materialized projection updates
    /// are serialized so concurrent patch commits cannot clobber each other.
    pub fn apply_runtime_projection_patch(
        &self,
        expected_session_id: &str,
        expected_basis: &ProjectionBasis,
        patch: ProjectionPatch,
    ) -> Result<ProjectionRuntimeApplyResult, ProjectionRuntimeApplyError> {
        let repository = FileMemoryRepository::user_data();
        self.apply_runtime_projection_patch_with_savers(
            expected_session_id,
            expected_basis,
            patch,
            |session_id, notes| repository.save_materialized_notes(session_id, notes),
            |session_id, graph| repository.save_materialized_graph(session_id, graph),
        )
    }

    fn apply_runtime_projection_patch_with_savers<SaveNotes, SaveGraph>(
        &self,
        expected_session_id: &str,
        expected_basis: &ProjectionBasis,
        patch: ProjectionPatch,
        save_notes: SaveNotes,
        save_graph: SaveGraph,
    ) -> Result<ProjectionRuntimeApplyResult, ProjectionRuntimeApplyError>
    where
        SaveNotes: FnMut(&str, &MaterializedNotes) -> Result<(), String>,
        SaveGraph: FnMut(&str, &MaterializedGraph) -> Result<(), String>,
    {
        self.projection_runtime_handle()
            .apply_runtime_projection_patch_with_savers(
                expected_session_id,
                expected_basis,
                patch,
                save_notes,
                save_graph,
            )
    }

    /// Rotate to a new session in-process.
    ///
    /// 1. Claims the `rotation_in_progress` guard atomically; a concurrent
    ///    rotate returns `RotateOutcome::AlreadyRotating(current_id)` without
    ///    touching state.
    /// 2. Prepares transcript, transcript-event, and projection-event writers
    ///    for the new session. Failure returns `RotateOutcome::Failed` while
    ///    the current session remains published and unchanged.
    /// 3. Shuts down and swaps the old writers, resets every active
    ///    session-scoped ledger/view/queue, then publishes the new session ID
    ///    last so readers cannot observe a new ID paired with old aggregates.
    /// 4. The graph-autosave thread reads `session_id` via the shared
    ///    `Arc<RwLock<String>>` on each tick, so it picks up the new ID
    ///    within the next 30s without being respawned.
    ///
    /// The guard in step 1 is released on return via an RAII guard, so the
    /// flag is cleared even on early returns / panics inside step 3.
    pub fn rotate_session(&self, new_session_id: &str) -> RotateOutcome {
        // Step 1: concurrent-rotate guard. `compare_exchange(false, true)`
        // fails iff another thread already claimed it — in that case we skip
        // the rotation entirely and return the current ID. Using SeqCst to
        // pair with the Drop (which stores false) and to be maximally safe
        // about cross-thread visibility of the writer/session_id mutations.
        if self
            .rotation_in_progress
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return RotateOutcome::AlreadyRotating(self.current_session_id());
        }
        // From here until the end of the function, we own the rotation slot.
        // `_guard` releases it on drop regardless of how we exit.
        let _guard = RotationGuard {
            flag: &self.rotation_in_progress,
        };

        // Prepare every required canonical writer before mutating active
        // ownership. A new session without one of these writers must not be
        // reported as successfully rotated.
        let new_transcript_writer =
            match crate::persistence::TranscriptWriter::spawn(new_session_id) {
                Some(writer) => writer,
                None => {
                    return RotateOutcome::Failed {
                        current_session_id: self.current_session_id(),
                        reason: "failed to prepare transcript writer".to_string(),
                    };
                }
            };
        let new_transcript_event_writer =
            match crate::persistence::TranscriptEventWriter::spawn(new_session_id) {
                Some(writer) => writer,
                None => {
                    let _ = new_transcript_writer
                        .shutdown_with_timeout(TRANSCRIPT_WRITER_SHUTDOWN_TIMEOUT);
                    return RotateOutcome::Failed {
                        current_session_id: self.current_session_id(),
                        reason: "failed to prepare transcript-event writer".to_string(),
                    };
                }
            };
        let new_projection_event_writer =
            match crate::persistence::ProjectionEventWriter::spawn(new_session_id) {
                Some(writer) => writer,
                None => {
                    let _ = new_transcript_writer
                        .shutdown_with_timeout(TRANSCRIPT_WRITER_SHUTDOWN_TIMEOUT);
                    let _ = new_transcript_event_writer
                        .shutdown_with_timeout(TRANSCRIPT_WRITER_SHUTDOWN_TIMEOUT);
                    return RotateOutcome::Failed {
                        current_session_id: self.current_session_id(),
                        reason: "failed to prepare projection-event writer".to_string(),
                    };
                }
            };

        // Keep the old id published until every session-scoped aggregate and
        // canonical writer has been reset. The graph autosave loop also sees
        // `rotation_in_progress` and skips this entire transition.
        let prev = self.current_session_id();

        {
            let mut ledger = match self.transcript_ledger.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            *ledger = TranscriptLedger::new(new_session_id);
        }
        {
            let mut timeline = match self.speaker_timeline.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            *timeline = SpeakerTimeline::new(new_session_id);
        }
        {
            let mut materialized = match self.materialized_projection_state.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            *materialized = MaterializedProjectionState::new(new_session_id);
        }
        {
            let mut schedulers = match self.projection_schedulers.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            // Persist the outgoing session's queue state before the reset
            // wipes it. Diagnostics only (ADR-0045 decision 6,
            // audio-graph-464c/5fd1): nothing reads this back as authority —
            // `ProjectionSchedulers::restore_from_snapshot`, which used to
            // rehydrate it on load, is deleted. The one disk-to-scheduler
            // channel left is `reseed_coverage_heads`, fed by
            // `derive_coverage_heads` over the accepted `projection_patches`
            // log, not this snapshot.
            crate::persistence::save_scheduler_queue_state(&prev, &schedulers.snapshot_queue());
            schedulers.reset(new_session_id);
        }
        // ADR-0045 decision 4 (audio-graph-9cc1, adversarial-review fix): a
        // rotated session starts with a clean projection lane, same as the
        // scheduler reset just above. Without this, a rotation that follows
        // a Stop would carry the latched `true` forward into the new
        // session, permanently disabling the deferred-retry lane (ADR-0045
        // decision 3) that is documented to poll this flag.
        self.projection_lane_stopping.store(false, Ordering::SeqCst);
        {
            let mut transcript = match self.transcript_buffer.write() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            transcript.clear();
        }
        {
            let mut graph = match self.knowledge_graph.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            *graph = TemporalKnowledgeGraph::new();
        }
        {
            let mut graph_snapshot = match self.graph_snapshot.write() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            *graph_snapshot = GraphSnapshot::default();
        }
        {
            let mut history = match self.chat_history.write() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            history.clear();
        }
        {
            let mut proposals = match self.pending_agent_proposals.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            proposals.clear();
        }
        {
            let mut status = match self.pipeline_status.write() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            *status = PipelineStatus::default();
        }

        // No producer can enter while `session_lifecycle` is held by the
        // command. Remove any residual chunks from the just-stopped session so
        // a later transcriber cannot consume old audio under the new id.
        while self.pipeline_rx.try_recv().is_ok() {}
        while self.processed_rx.try_recv().is_ok() {}
        while self.speech_audio_rx.try_recv().is_ok() {}

        // Respawn transcript writers for the new session. The old writers are
        // asked to shut down gracefully; their joins are bounded by
        // TRANSCRIPT_WRITER_SHUTDOWN_TIMEOUT so a stuck disk cannot block the
        // IPC caller. If a new writer fails to spawn (e.g. base dir not
        // resolvable), we leave its slot empty — transcript persistence is
        // best-effort and already handles None elsewhere.
        let mut writer_slot = match self.transcript_writer.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(old) = writer_slot.take()
            && !old.shutdown_with_timeout(TRANSCRIPT_WRITER_SHUTDOWN_TIMEOUT)
        {
            log::warn!(
                "Transcript writer for session {} did not finish flush within {:?}; \
                     dropping JoinHandle and proceeding with new writer",
                prev,
                TRANSCRIPT_WRITER_SHUTDOWN_TIMEOUT
            );
        }
        *writer_slot = Some(new_transcript_writer);

        let mut event_writer_slot = match self.transcript_event_writer.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(old) = event_writer_slot.take()
            && !old.shutdown_with_timeout(TRANSCRIPT_WRITER_SHUTDOWN_TIMEOUT)
        {
            log::warn!(
                "Transcript event writer for session {} did not finish flush within {:?}; \
                     dropping JoinHandle and proceeding with new writer",
                prev,
                TRANSCRIPT_WRITER_SHUTDOWN_TIMEOUT
            );
        }
        *event_writer_slot = Some(new_transcript_event_writer);
        let mut projection_writer_slot = match self.projection_event_writer.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(old) = projection_writer_slot.take()
            && !old.shutdown_with_timeout(TRANSCRIPT_WRITER_SHUTDOWN_TIMEOUT)
        {
            log::warn!(
                "Projection event writer for session {} did not finish flush within {:?}; \
                     dropping JoinHandle and proceeding with new writer",
                prev,
                TRANSCRIPT_WRITER_SHUTDOWN_TIMEOUT
            );
        }
        *projection_writer_slot = Some(new_projection_event_writer);
        if writer_slot.is_some() {
            log::info!("Rotated transcript writer to session {}", new_session_id);
        } else {
            log::warn!(
                "Failed to spawn transcript writer for rotated session {}",
                new_session_id
            );
        }
        if event_writer_slot.is_some() {
            log::info!(
                "Rotated transcript event writer to session {}",
                new_session_id
            );
        } else {
            log::warn!(
                "Failed to spawn transcript event writer for rotated session {}",
                new_session_id
            );
        }
        if projection_writer_slot.is_some() {
            log::info!(
                "Rotated projection event writer to session {}",
                new_session_id
            );
        } else {
            log::warn!(
                "Failed to spawn projection event writer for rotated session {}",
                new_session_id
            );
        }

        // Publish the new ownership epoch last. Readers can observe either the
        // complete old session or the complete new session, never a new id with
        // old writers/graphs/buffers.
        {
            let mut guard = match self.session_id.write() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            *guard = new_session_id.to_string();
        }

        RotateOutcome::Rotated(prev)
    }
}

/// Bounded wait for the old transcript writer's flush+join on rotation.
///
/// Current value (5s) is chosen empirically as a first-cut: long enough for a
/// healthy BufWriter flush of any realistic transcript buffer, short enough
/// that a wedged disk (hang, NFS stall) doesn't block `new_session_cmd` from
/// the UI. On timeout the writer thread keeps running detached — it will
/// eventually exit on its own when the disk recovers; if it never does, the
/// process is in worse shape than a leaked thread handle.
///
/// Tuning procedure (ag#8): `TranscriptWriter::shutdown_with_timeout` and the
/// writer thread's final flush both log their elapsed wall-clock at INFO level
/// (`transcript_writer.shutdown_join elapsed_ms=…` and
/// `transcript_writer.final_flush elapsed_ms=…`). After ~1–2 weeks of
/// real-world usage, grep logs for those keys, compute p50/p95/p99, and set
/// this constant to `p99 + ~1s safety margin`. Document the chosen value with
/// a "Chosen because: p99 = Xms over N rotations on dates …" comment.
const TRANSCRIPT_WRITER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// RAII guard that clears `rotation_in_progress` on drop, so early returns /
/// panics inside `rotate_session` don't wedge the flag in the set state.
struct RotationGuard<'a> {
    flag: &'a AtomicBool,
}

impl Drop for RotationGuard<'_> {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::SeqCst);
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests — in-process session rotation (loop 20)
// ---------------------------------------------------------------------------
//
// These exercise `AppState::rotate_session` directly rather than going through
// the Tauri command.
//
// Two of the three tests are purely in-memory: id swap and the concurrent
// reader smoke test. They don't touch HOME and are safe in parallel.
//
// The third test verifies the respawned transcript writer opens a new file
// on disk — which requires mutating HOME. `sessions::usage::tests` also
// mutates HOME under its own test lock, so running these in parallel would
// stomp each other's env overrides. That test is therefore `#[ignore]`d and
// run explicitly via `cargo test --lib -- --ignored --test-threads=1
// rotate_session_respawns_transcript_writer_to_new_file`. The two parallel-
// safe tests provide the bulk of the coverage; the ignored test is the
// belt-and-braces proof for a human spot-check.

#[cfg(test)]
mod rotation_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_tempdir(label: &str) -> std::path::PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!(
            "audio-graph-rotation-{}-{}-{}-{}",
            label, pid, nanos, n
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
            // SAFETY: serialized by crate::sessions::TEST_HOME_LOCK; callers
            // MUST hold that lock for the lifetime of this guard. Mirrors
            // the invariant in `sessions::tests::HomeGuard`.
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

    fn drain_writers(app: &AppState) {
        {
            let mut guard = app
                .transcript_writer
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            if let Some(w) = guard.take() {
                let joined = w.shutdown_with_timeout(Duration::from_secs(3));
                assert!(joined, "writer must finish flush within 3s on drain");
            }
        }
        {
            let mut guard = app
                .transcript_event_writer
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            if let Some(w) = guard.take() {
                let joined = w.shutdown_with_timeout(Duration::from_secs(3));
                assert!(joined, "event writer must finish flush within 3s on drain");
            }
        }
        {
            let mut guard = app
                .projection_event_writer
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            if let Some(w) = guard.take() {
                let joined = w.shutdown_with_timeout(Duration::from_secs(3));
                assert!(
                    joined,
                    "projection event writer must finish flush within 3s on drain"
                );
            }
        }
    }

    fn projection_transcript_event(
        span_id: &str,
        revision_number: u64,
        text: &str,
    ) -> crate::projections::TranscriptEvent {
        crate::projections::TranscriptEvent {
            span_id: span_id.into(),
            provider: "test".into(),
            source_id: "test-source".into(),
            provider_item_id: None,
            transcript_segment_id: Some(format!("segment-{span_id}")),
            speaker_id: Some("speaker-1".into()),
            speaker_label: Some("Speaker 1".into()),
            channel: None,
            text: text.into(),
            start_time: revision_number as f64,
            end_time: revision_number as f64 + 1.0,
            confidence: 1.0,
            is_final: true,
            stability: crate::projections::TranscriptEventStability::Final,
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

    fn runtime_note_patch(
        sequence: u64,
        basis: ProjectionBasis,
        note_id: &str,
        body: &str,
    ) -> ProjectionPatch {
        ProjectionPatch {
            route: None,
            sequence,
            kind: ProjectionKind::Notes,
            llm_request_id: format!("llm-notes-{sequence}"),
            basis,
            operations: vec![crate::projections::ProjectionOperation::UpsertNote {
                id: note_id.into(),
                title: "Decision".into(),
                body: body.into(),
                tags: vec!["decision".into()],
                evidence: crate::claim_evidence::EvidenceAnchor::default(),
            }],
            confidence: 0.91,
            provenance: crate::projections::ProjectionProvenance {
                provider: "openrouter".into(),
                model: "anthropic/claude-sonnet-4".into(),
                prompt_id: "notes-v1".into(),
                route_id: None,
                model_source: crate::llm::route::ModelIdentitySource::Requested,
            },
            queued_at_ms: None,
            generation_latency_ms: None,
            apply_latency_ms: None,
            created_at_ms: 1_700_000_000_100 + sequence,
        }
    }

    fn runtime_graph_patch(sequence: u64, basis: ProjectionBasis) -> ProjectionPatch {
        ProjectionPatch {
            route: None,
            sequence,
            kind: ProjectionKind::Graph,
            llm_request_id: format!("llm-graph-{sequence}"),
            basis,
            operations: vec![crate::projections::ProjectionOperation::UpsertGraphNode {
                id: "node-audiograph".into(),
                name: "AudioGraph".into(),
                entity_type: "Product".into(),
                description: Some("Streaming speech knowledge graph app.".into()),
                evidence: crate::claim_evidence::EvidenceAnchor::default(),
            }],
            confidence: 0.87,
            provenance: crate::projections::ProjectionProvenance {
                provider: "openrouter".into(),
                model: "anthropic/claude-sonnet-4".into(),
                prompt_id: "graph-v1".into(),
                route_id: None,
                model_source: crate::llm::route::ModelIdentitySource::Requested,
            },
            queued_at_ms: None,
            generation_latency_ms: None,
            apply_latency_ms: None,
            created_at_ms: 1_700_000_000_200 + sequence,
        }
    }

    #[test]
    fn downloads_in_flight_rejects_duplicate_filename() {
        // AUD-MDL1 / #58 P2: the concurrent-download guard is a HashSet keyed by
        // filename. The first claim inserts (returns true); a second claim of the
        // same filename must observe it already present (returns false) so
        // `download_model_cmd` can reject the duplicate. A *different* filename
        // must still be claimable concurrently.
        let app = AppState::new();
        {
            let mut set = app.downloads_in_flight.lock().unwrap();
            assert!(
                set.insert("ggml-small.en.bin".to_string()),
                "first claim wins"
            );
            assert!(
                !set.insert("ggml-small.en.bin".to_string()),
                "second claim of the same model must be rejected"
            );
            assert!(
                set.insert("ggml-tiny.en.bin".to_string()),
                "a different model must still be claimable"
            );
            // Releasing the first frees its slot for a later download.
            assert!(set.remove("ggml-small.en.bin"));
            assert!(
                set.insert("ggml-small.en.bin".to_string()),
                "after release the slot is reclaimable"
            );
        }

        // Drain any spawned writers so their threads don't linger past the test.
        drain_writers(&app);
    }

    #[test]
    fn runtime_projection_patch_persists_notes_and_projection_event() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("projection-notes");
        let _g = HomeGuard::set(&dir);

        let app = AppState::new();
        let session_id = app.current_session_id();
        let basis = {
            let mut ledger = app.transcript_ledger.lock().unwrap();
            ledger
                .apply_event(projection_transcript_event(
                    "span-1",
                    1,
                    "We decided to persist projection patches.",
                ))
                .expect("seed transcript ledger");
            ledger.current_basis()
        };
        let expected_body = "Persist projection patches and materialized notes.";
        let patch = runtime_note_patch(1, basis.clone(), "note-1", expected_body);

        let result = app
            .apply_runtime_projection_patch(&session_id, &basis, patch.clone())
            .expect("runtime notes projection apply");
        assert_eq!(
            result.outcome,
            MaterializedProjectionApplyOutcome::Notes {
                last_sequence: 1,
                note_count: 1,
            }
        );
        assert!(result.projection_event_enqueued);
        assert_eq!(
            app.materialized_projection_state
                .lock()
                .unwrap()
                .notes
                .notes[0]
                .id,
            "note-1"
        );

        drain_writers(&app);

        let repository = FileMemoryRepository::user_data();
        let notes = repository
            .load_materialized_notes(&session_id)
            .expect("load notes")
            .expect("notes artifact exists");
        assert_eq!(notes.session_id, session_id);
        assert_eq!(notes.last_sequence, 1);
        assert_eq!(notes.notes[0].body, expected_body);

        let events = repository
            .load_projection_patches(&session_id)
            .expect("load projection events");
        assert_eq!(events.len(), 1);
        assert!(
            events[0].apply_latency_ms.is_some(),
            "persisted projection event should record apply latency"
        );
        let mut expected_patch = patch.clone();
        expected_patch.apply_latency_ms = events[0].apply_latency_ms;
        assert_eq!(events, vec![expected_patch]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn runtime_projection_patch_can_enqueue_event_through_repository_writer() {
        let dir = unique_tempdir("projection-repository-writer");
        let repo = Arc::new(FileMemoryRepository::with_data_root(&dir));
        let repository: Arc<dyn LocalMemoryRepository> = repo.clone();
        let session_id = "runtime-repository-session";
        let runtime = ProjectionRuntimeHandle::in_memory_for_tests(session_id);
        let basis = {
            let mut ledger = runtime.transcript_ledger.lock().unwrap();
            ledger
                .apply_event(projection_transcript_event(
                    "span-repository-writer",
                    1,
                    "Repository writers should preserve runtime projection patches.",
                ))
                .expect("seed transcript ledger");
            ledger.current_basis()
        };
        {
            let mut writer = runtime.projection_event_writer.lock().unwrap();
            *writer = ProjectionEventWriter::repository(session_id, repository);
        }

        let expected_body = "Repository writer routes projection patches.";
        let patch = runtime_note_patch(1, basis.clone(), "note-repository-writer", expected_body);
        let notes_repo = repo.clone();
        let graph_repo = repo.clone();
        let result = runtime
            .apply_runtime_projection_patch_with_savers(
                session_id,
                &basis,
                patch.clone(),
                move |session_id, notes| notes_repo.save_materialized_notes(session_id, notes),
                move |session_id, graph| graph_repo.save_materialized_graph(session_id, graph),
            )
            .expect("runtime repository projection apply");
        assert_eq!(
            result.outcome,
            MaterializedProjectionApplyOutcome::Notes {
                last_sequence: 1,
                note_count: 1,
            }
        );
        assert!(result.projection_event_enqueued);

        let writer = runtime
            .projection_event_writer
            .lock()
            .unwrap()
            .take()
            .expect("repository projection writer");
        assert!(
            writer.shutdown_with_timeout(Duration::from_secs(2)),
            "repository projection writer should drain accepted patch"
        );

        let notes = repo
            .load_materialized_notes(session_id)
            .expect("load repository notes")
            .expect("repository notes artifact exists");
        assert_eq!(notes.notes[0].body, expected_body);

        let events = repo
            .load_projection_patches(session_id)
            .expect("load repository projection events");
        assert_eq!(events.len(), 1);
        assert!(
            events[0].apply_latency_ms.is_some(),
            "repository projection event should record apply latency"
        );
        let mut expected_patch = patch.clone();
        expected_patch.apply_latency_ms = events[0].apply_latency_ms;
        assert_eq!(events, vec![expected_patch]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn canonical_projection_sequence_advances_when_snapshot_cache_save_fails() {
        let dir = unique_tempdir("projection-snapshot-cache-lag");
        let repo = Arc::new(FileMemoryRepository::with_data_root(&dir));
        let repository: Arc<dyn LocalMemoryRepository> = repo.clone();
        let session_id = "runtime-snapshot-cache-lag";
        let runtime = ProjectionRuntimeHandle::in_memory_for_tests(session_id);
        let first_basis = {
            let mut ledger = runtime.transcript_ledger.lock().unwrap();
            ledger
                .apply_event(projection_transcript_event(
                    "span-cache-lag-1",
                    1,
                    "The first canonical patch outlives its derived cache.",
                ))
                .expect("seed first transcript event");
            ledger.current_basis()
        };
        {
            let mut writer = runtime.projection_event_writer.lock().unwrap();
            *writer = ProjectionEventWriter::repository(session_id, repository);
        }

        let first = runtime
            .apply_runtime_projection_patch_with_savers(
                session_id,
                &first_basis,
                runtime_note_patch(1, first_basis.clone(), "note-cache-lag-1", "First note."),
                |_session_id, _notes| Err("injected snapshot failure".to_string()),
                |_session_id, _graph| Ok(()),
            )
            .expect("canonical acceptance must survive derived-cache failure");
        assert!(!first.materialized_snapshot_saved);
        assert_eq!(
            runtime
                .materialized_projection_snapshot()
                .notes
                .last_sequence,
            1
        );

        let second_basis = {
            let mut ledger = runtime.transcript_ledger.lock().unwrap();
            ledger
                .apply_event(projection_transcript_event(
                    "span-cache-lag-2",
                    1,
                    "The next basis must receive a fresh sequence.",
                ))
                .expect("seed second transcript event");
            ledger.current_basis()
        };
        let second_sequence = runtime.next_projection_sequence(&ProjectionKind::Notes);
        assert_eq!(
            second_sequence, 2,
            "accepted canonical head owns sequence 1"
        );
        let second = runtime
            .apply_runtime_projection_patch_with_savers(
                session_id,
                &second_basis,
                runtime_note_patch(
                    second_sequence,
                    second_basis.clone(),
                    "note-cache-lag-2",
                    "Second note.",
                ),
                |_session_id, _notes| Ok(()),
                |_session_id, _graph| Ok(()),
            )
            .expect("second canonical patch applies");
        assert!(second.materialized_snapshot_saved);

        let writer = runtime
            .projection_event_writer
            .lock()
            .unwrap()
            .take()
            .expect("repository projection writer");
        assert!(writer.shutdown_with_timeout(Duration::from_secs(2)));
        let events = repo
            .load_projection_patches(session_id)
            .expect("load canonical projection events");
        assert_eq!(
            events
                .iter()
                .map(|patch| patch.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        let replayed = MaterializedProjectionState::replay_accepted_patches(session_id, events)
            .expect("canonical history remains replayable");
        assert_eq!(replayed, runtime.materialized_projection_snapshot());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn runtime_projection_patch_queue_full_does_not_save_materialized_state() {
        let session_id = "runtime-projection-queue-full";
        let runtime = ProjectionRuntimeHandle::in_memory_for_tests(session_id);
        let basis = {
            let mut ledger = runtime.transcript_ledger.lock().unwrap();
            ledger
                .apply_event(projection_transcript_event(
                    "span-queue-full",
                    1,
                    "Queue-full should not save materialized state without an event.",
                ))
                .expect("seed transcript ledger");
            ledger.current_basis()
        };
        let patch = runtime_note_patch(1, basis.clone(), "note-queue-full", "Do not save.");
        {
            let mut writer = runtime.projection_event_writer.lock().unwrap();
            *writer = Some(ProjectionEventWriter::saturated_for_tests(patch.clone()));
        }
        let notes_saved = Arc::new(AtomicBool::new(false));
        let graph_saved = Arc::new(AtomicBool::new(false));
        let notes_saved_for_closure = notes_saved.clone();
        let graph_saved_for_closure = graph_saved.clone();

        let error = runtime
            .apply_runtime_projection_patch_with_savers(
                session_id,
                &basis,
                patch,
                move |_session_id, _notes| {
                    notes_saved_for_closure.store(true, Ordering::SeqCst);
                    Ok(())
                },
                move |_session_id, _graph| {
                    graph_saved_for_closure.store(true, Ordering::SeqCst);
                    Ok(())
                },
            )
            .expect_err("full projection queue must reject runtime apply");

        assert!(matches!(
            error,
            ProjectionRuntimeApplyError::ProjectionEventEnqueueFailed { .. }
        ));
        assert!(
            !notes_saved.load(Ordering::SeqCst),
            "materialized notes must not save after event enqueue failure"
        );
        assert!(
            !graph_saved.load(Ordering::SeqCst),
            "materialized graph must not save after event enqueue failure"
        );
        assert!(
            runtime
                .materialized_projection_snapshot()
                .notes
                .notes
                .is_empty(),
            "in-memory materialized state must not advance after enqueue failure"
        );
    }

    #[test]
    fn runtime_projection_patch_persists_materialized_graph() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("projection-graph");
        let _g = HomeGuard::set(&dir);

        let app = AppState::new();
        let session_id = app.current_session_id();
        let basis = {
            let mut ledger = app.transcript_ledger.lock().unwrap();
            ledger
                .apply_event(projection_transcript_event(
                    "span-graph",
                    1,
                    "AudioGraph connects transcripts to temporal graph state.",
                ))
                .expect("seed transcript ledger");
            ledger.current_basis()
        };
        let patch = runtime_graph_patch(1, basis.clone());

        let result = app
            .apply_runtime_projection_patch(&session_id, &basis, patch.clone())
            .expect("runtime graph projection apply");
        assert_eq!(
            result.outcome,
            MaterializedProjectionApplyOutcome::Graph {
                last_sequence: 1,
                node_count: 1,
                edge_count: 0,
            }
        );

        drain_writers(&app);

        let repository = FileMemoryRepository::user_data();
        let graph = repository
            .load_materialized_graph(&session_id)
            .expect("load materialized graph")
            .expect("graph artifact exists");
        assert_eq!(graph.session_id, session_id);
        assert_eq!(graph.nodes[0].id, "node-audiograph");

        let events = repository
            .load_projection_patches(&session_id)
            .expect("load projection events");
        assert_eq!(events.len(), 1);
        assert!(
            events[0].apply_latency_ms.is_some(),
            "persisted projection event should record apply latency"
        );
        let mut expected_patch = patch.clone();
        expected_patch.apply_latency_ms = events[0].apply_latency_ms;
        assert_eq!(events, vec![expected_patch]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// audio-graph-caad / audio-graph-f3d4: an append-only tail must apply
    /// live, not be silently discarded as stale.
    #[test]
    fn runtime_projection_patch_applies_append_only_basis_with_persistence() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("projection-append-only");
        let _g = HomeGuard::set(&dir);

        let app = AppState::new();
        let session_id = app.current_session_id();
        let old_basis = {
            let mut ledger = app.transcript_ledger.lock().unwrap();
            ledger
                .apply_event(projection_transcript_event("span-1", 1, "Old context."))
                .expect("seed first transcript event");
            let basis = ledger.current_basis();
            ledger
                .apply_event(projection_transcript_event("span-2", 1, "New context."))
                .expect("seed newer transcript event");
            basis
        };
        let patch = runtime_note_patch(1, old_basis.clone(), "note-append-only", "Still current.");

        let result = app
            .apply_runtime_projection_patch(&session_id, &old_basis, patch)
            .expect("an append-only basis must apply, not be discarded as stale");
        assert_eq!(
            result.outcome,
            MaterializedProjectionApplyOutcome::Notes {
                last_sequence: 1,
                note_count: 1,
            }
        );
        assert!(matches!(
            result.basis_currency_at_apply,
            AppliedBasisCurrency::AppendedTail { .. }
        ));
        assert!(result.projection_event_enqueued);
        assert_eq!(
            app.materialized_projection_state
                .lock()
                .unwrap()
                .notes
                .notes[0]
                .id,
            "note-append-only"
        );

        drain_writers(&app);

        let repository = FileMemoryRepository::user_data();
        let notes = repository
            .load_materialized_notes(&session_id)
            .expect("load notes")
            .expect("notes artifact exists");
        assert_eq!(notes.notes[0].body, "Still current.");

        let events = repository
            .load_projection_patches(&session_id)
            .expect("load projection events");
        assert_eq!(
            events.len(),
            1,
            "the append-only apply must enqueue its projection event"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn runtime_projection_patch_rejects_revised_basis_without_persistence() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("projection-revised");
        let _g = HomeGuard::set(&dir);

        let app = AppState::new();
        let session_id = app.current_session_id();
        let old_basis = {
            let mut ledger = app.transcript_ledger.lock().unwrap();
            ledger
                .apply_event(projection_transcript_event("span-1", 1, "Old context."))
                .expect("seed first transcript event");
            let basis = ledger.current_basis();
            ledger
                .apply_event(projection_transcript_event("span-1", 2, "Revised context."))
                .expect("seed revised transcript event");
            basis
        };
        let patch = runtime_note_patch(1, old_basis.clone(), "note-revised", "Outdated note.");

        let error = app
            .apply_runtime_projection_patch(&session_id, &old_basis, patch)
            .expect_err("a revised basis must be rejected");
        assert!(matches!(
            error,
            ProjectionRuntimeApplyError::Apply {
                error: ProjectionApplyError::StaleBasis { .. }
            }
        ));
        assert!(
            app.materialized_projection_state
                .lock()
                .unwrap()
                .notes
                .notes
                .is_empty(),
            "revised patch must not mutate materialized notes"
        );

        drain_writers(&app);

        let repository = FileMemoryRepository::user_data();
        assert!(
            repository
                .load_projection_patches(&session_id)
                .expect("load projection events")
                .is_empty(),
            "revised patch must not enqueue a projection event"
        );
        assert!(
            repository
                .load_materialized_notes(&session_id)
                .expect("load notes")
                .is_none(),
            "revised patch must not write a materialized notes artifact"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Round-trip guard for the `load_session_impl` (`commands.rs`) derived-
    /// cache refusal: an accepted append-only patch must replay through
    /// `replay_accepted_patches_with_history` to the SAME materialized state
    /// the live gate produced, with `invalid_patch_count == 0` — otherwise
    /// the first persisted append-only patch would make reopening this
    /// session refuse to load its derived caches.
    #[test]
    fn runtime_projection_patch_applies_append_only_basis_and_replays_identically() {
        let dir = unique_tempdir("projection-append-only-replay");
        let repo = Arc::new(FileMemoryRepository::with_data_root(&dir));
        let repository: Arc<dyn LocalMemoryRepository> = repo.clone();
        let session_id = "runtime-append-only-replay";
        let runtime = ProjectionRuntimeHandle::in_memory_for_tests(session_id);

        let old_event = projection_transcript_event("span-1", 1, "Old context.");
        let new_event = projection_transcript_event("span-2", 1, "New context.");
        let old_basis = {
            let mut ledger = runtime.transcript_ledger.lock().unwrap();
            ledger
                .apply_event(old_event.clone())
                .expect("seed old transcript event");
            let basis = ledger.current_basis();
            ledger
                .apply_event(new_event.clone())
                .expect("seed newer transcript event");
            basis
        };
        {
            let mut writer = runtime.projection_event_writer.lock().unwrap();
            *writer = ProjectionEventWriter::repository(session_id, repository);
        }

        let patch = runtime_note_patch(
            1,
            old_basis.clone(),
            "note-append-only-replay",
            "Still current.",
        );
        let notes_repo = repo.clone();
        let graph_repo = repo.clone();
        let result = runtime
            .apply_runtime_projection_patch_with_savers(
                session_id,
                &old_basis,
                patch,
                move |session_id, notes| notes_repo.save_materialized_notes(session_id, notes),
                move |session_id, graph| graph_repo.save_materialized_graph(session_id, graph),
            )
            .expect("an append-only basis must apply live");
        assert!(matches!(
            result.basis_currency_at_apply,
            AppliedBasisCurrency::AppendedTail { .. }
        ));
        let live_state = runtime.materialized_projection_snapshot();

        let writer = runtime
            .projection_event_writer
            .lock()
            .unwrap()
            .take()
            .expect("repository projection writer");
        assert!(writer.shutdown_with_timeout(Duration::from_secs(2)));

        let events = repo
            .load_projection_patches(session_id)
            .expect("load persisted projection events");
        assert_eq!(events.len(), 1);

        let replay = MaterializedProjectionState::replay_accepted_patches_with_history(
            session_id,
            vec![old_event, new_event],
            None,
            events,
        )
        .expect("an accepted append-only patch must replay");
        assert_eq!(
            replay.validation.invalid_patch_count, 0,
            "an accepted append-only patch must not be rejected on replay — this is the \
             invariant load_session_impl relies on to load derived caches"
        );
        assert_eq!(replay.state, live_state);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotate_session_swaps_session_id_atomically() {
        // Pure in-memory — we do NOT rely on HOME resolving to anything in
        // particular. `AppState::new()` may or may not spawn a transcript
        // writer depending on ambient HOME, but the id swap is independent.
        let app = AppState::new();
        let original = app.current_session_id();
        assert!(!original.is_empty(), "session_id must be populated at init");

        let new_id = "rotated-session-aaa";
        let outcome = app.rotate_session(new_id);
        assert_eq!(
            outcome,
            RotateOutcome::Rotated(original.clone()),
            "rotate_session must report Rotated(previous_id) on first call"
        );
        assert_eq!(
            app.current_session_id(),
            new_id,
            "current_session_id must reflect the new id after rotation"
        );

        // Drain any spawned writers so their threads don't linger past the test.
        drain_writers(&app);
    }

    #[test]
    fn rotate_session_resets_projection_schedulers() {
        let app = AppState::new();
        {
            let mut ledger = app.transcript_ledger.lock().unwrap();
            ledger
                .apply_event(projection_transcript_event(
                    "span-before-rotation",
                    1,
                    "Schedule before rotation.",
                ))
                .expect("seed transcript event");
            let mut schedulers = app.projection_schedulers.lock().unwrap();
            let observation = schedulers.observe_ledger(&ledger, 1);
            assert!(matches!(
                observation.notes,
                crate::projection_scheduler::ProjectionSchedulerDecision::StartJob { .. }
            ));
            assert!(matches!(
                observation.graph,
                crate::projection_scheduler::ProjectionSchedulerDecision::StartJob { .. }
            ));
            assert_eq!(schedulers.notes().metrics().jobs_started, 1);
            assert_eq!(schedulers.graph().metrics().jobs_started, 1);
        }

        app.rotate_session("rotated-session-schedulers");

        {
            let ledger = app.transcript_ledger.lock().unwrap();
            assert!(
                ledger.current_basis().span_revisions.is_empty(),
                "rotation should clear transcript basis for the new session"
            );
            let schedulers = app.projection_schedulers.lock().unwrap();
            assert_eq!(schedulers.notes().metrics().jobs_started, 0);
            assert_eq!(schedulers.graph().metrics().jobs_started, 0);
            assert!(schedulers.notes().in_flight_job().is_none());
            assert!(schedulers.graph().in_flight_job().is_none());
        }

        drain_writers(&app);
    }

    #[test]
    fn rotate_session_clears_transcript_and_graph_aggregates_before_publish() {
        let app = AppState::new();
        app.transcript_buffer
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push_back(TranscriptSegment {
                id: "old-segment".to_string(),
                source_id: "old-source".to_string(),
                speaker_id: None,
                speaker_label: None,
                text: "must not enter the new session".to_string(),
                start_time: 0.0,
                end_time: 1.0,
                confidence: 1.0,
            });
        let old_snapshot = {
            let mut graph = app
                .knowledge_graph
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            graph.add_entity(
                &crate::graph::entities::ExtractedEntity {
                    name: "Old session entity".to_string(),
                    entity_type: "Topic".to_string(),
                    description: None,
                },
                1.0,
                "old-speaker",
            );
            graph.snapshot()
        };
        *app.graph_snapshot
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = old_snapshot;

        app.rotate_session("aggregate-reset-session");

        assert_eq!(app.current_session_id(), "aggregate-reset-session");
        assert!(
            app.transcript_buffer
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_empty(),
            "old transcript segments must not survive session rotation"
        );
        assert_eq!(
            app.knowledge_graph
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .node_count(),
            0,
            "old graph entities must not survive session rotation"
        );
        assert_eq!(
            app.graph_snapshot
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .stats
                .total_nodes,
            0,
            "autosave-visible graph snapshot must be empty for the new session"
        );

        drain_writers(&app);
    }

    #[test]
    #[ignore = "mutates AUDIOGRAPH_DATA_DIR; run with --test-threads=1"]
    fn rotate_session_writer_open_failure_preserves_current_session() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let dir = unique_tempdir("writer-open-failure");
        let _guard = HomeGuard::set(&dir);
        let app = AppState::new();
        let original_session_id = app.current_session_id();
        app.transcript_buffer
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push_back(TranscriptSegment {
                id: "old-session-segment".to_string(),
                source_id: "test".to_string(),
                speaker_id: None,
                speaker_label: None,
                text: "must survive failed rotation".to_string(),
                start_time: 0.0,
                end_time: 1.0,
                confidence: 1.0,
            });

        let target_session_id = "writer-open-must-fail";
        let blocked_path = crate::user_data::transcript_path(target_session_id)
            .expect("resolve target transcript path");
        std::fs::create_dir_all(&blocked_path)
            .expect("directory-shaped target forces file open failure");

        let outcome = app.rotate_session(target_session_id);
        assert!(
            matches!(
                outcome,
                RotateOutcome::Failed {
                    ref current_session_id,
                    ..
                } if current_session_id == &original_session_id
            ),
            "unwritable canonical target must reject rotation: {outcome:?}"
        );
        assert_eq!(app.current_session_id(), original_session_id);
        assert_eq!(
            app.transcript_buffer
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .len(),
            1,
            "failed preflight must not clear active aggregates"
        );
        assert!(
            app.transcript_writer
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_some()
        );
        assert!(
            app.transcript_event_writer
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_some()
        );
        assert!(
            app.projection_event_writer
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_some()
        );

        std::fs::remove_dir(&blocked_path).expect("remove blocked target");
        drain_writers(&app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[ignore = "mutates AUDIOGRAPH_DATA_DIR; conflicts with sessions::usage::tests — run with --test-threads=1"]
    fn rotate_session_respawns_transcript_writer_to_new_file() {
        // SAFETY invariant for HomeGuard requires the shared test-env lock;
        // acquire it before constructing the guard so HOME / USERPROFILE /
        // AUDIOGRAPH_DATA_DIR mutation is serialized with sessions tests.
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("writer-respawn");
        let _g = HomeGuard::set(&dir);

        let app = AppState::new();
        let original = app.current_session_id();

        {
            let guard = app.transcript_writer.lock().unwrap();
            assert!(
                guard.is_some(),
                "initial AppState must have a transcript writer with HOME override"
            );
        }

        let new_id = "rotated-session-bbb";
        app.rotate_session(new_id);

        {
            let guard = app.transcript_writer.lock().unwrap();
            assert!(
                guard.is_some(),
                "rotate_session must leave a live writer in place"
            );
        }

        use crate::state::TranscriptSegment;
        let segment = TranscriptSegment {
            id: "seg-1".into(),
            source_id: "test".into(),
            speaker_id: None,
            speaker_label: None,
            text: "post-rotation line".into(),
            start_time: 0.0,
            end_time: 1.0,
            confidence: 1.0,
        };
        {
            let guard = app.transcript_writer.lock().unwrap();
            guard.as_ref().expect("writer present").append(&segment);
        }

        // Signal shutdown + wait briefly for the append to flush. Shutdown
        // drains the channel and flushes the BufWriter before exiting.
        drain_writers(&app);
        std::thread::sleep(std::time::Duration::from_millis(150));

        // Resolve expected paths the same way production code does — via
        // `user_data::transcript_path`, which honors `AUDIOGRAPH_DATA_DIR`
        // — rather than hand-assembling a `HOME`-relative `.audiograph`
        // path. Windows known-folder home resolution ignores the `HOME` /
        // `USERPROFILE` env overrides `HomeGuard` sets, so a manually
        // reconstructed path drifts from the writer's actual target; the
        // canonical resolver stays correct because `AUDIOGRAPH_DATA_DIR`
        // takes priority over home-dir lookup on every platform.
        let new_file = crate::user_data::transcript_path(new_id).expect("resolve new session id");
        assert!(
            new_file.exists(),
            "rotated writer must have opened {:?}",
            new_file
        );
        let contents = std::fs::read_to_string(&new_file).unwrap();
        assert!(
            contents.contains("post-rotation line"),
            "segment appended post-rotation must land in new session file, got: {:?}",
            contents
        );

        let original_file =
            crate::user_data::transcript_path(&original).expect("resolve original session id");
        if original_file.exists() {
            let original_contents = std::fs::read_to_string(&original_file).unwrap();
            assert!(
                !original_contents.contains("post-rotation line"),
                "post-rotation segment must not land in the old session file"
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn current_session_id_readable_while_rotation_in_progress() {
        // Pure in-memory smoke test — no HOME mutation needed.
        let app = Arc::new(AppState::new());
        let reader_app = app.clone();

        let reader = std::thread::spawn(move || {
            for _ in 0..1000 {
                let id = reader_app.current_session_id();
                assert!(!id.is_empty());
            }
        });

        for i in 0..5 {
            app.rotate_session(&format!("rotation-{}", i));
        }

        reader.join().expect("reader thread must not panic");
        // Only the most recent rotation needs to have landed — if the reader
        // or scheduler interleaved things such that some rotations raced
        // (hit AlreadyRotating), current_session_id() is still one of the
        // attempted values. In practice rotations are fast enough that they
        // all land sequentially; the assertion below is the strict case and
        // any flake would indicate the guard is doing its job.
        let current = app.current_session_id();
        assert!(
            current.starts_with("rotation-"),
            "final session id must be one of the rotation-N values, got {}",
            current
        );

        drain_writers(&app);
    }

    #[test]
    fn rotate_session_rejects_concurrent_entry() {
        // Directly exercise the compare_exchange guard by flipping the flag
        // manually. The second rotate MUST observe the flag-set state and
        // return AlreadyRotating without touching session_id or the writer.
        let app = AppState::new();
        let original = app.current_session_id();

        // Claim the slot (simulating an in-flight rotation).
        app.rotation_in_progress
            .store(true, std::sync::atomic::Ordering::SeqCst);

        let outcome = app.rotate_session("should-not-land");
        match outcome {
            RotateOutcome::AlreadyRotating(curr) => {
                assert_eq!(
                    curr, original,
                    "AlreadyRotating must carry the unchanged current session id"
                );
            }
            RotateOutcome::Rotated(_) => {
                panic!("rotate_session must not succeed while rotation_in_progress is set");
            }
            RotateOutcome::Failed { reason, .. } => {
                panic!("rotation guard should reject before writer preflight: {reason}");
            }
        }
        assert_eq!(
            app.current_session_id(),
            original,
            "session_id must not have changed when rotation was rejected"
        );

        // Release and confirm a subsequent rotation now succeeds.
        app.rotation_in_progress
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let outcome = app.rotate_session("now-it-lands");
        assert!(matches!(outcome, RotateOutcome::Rotated(_)));
        assert_eq!(app.current_session_id(), "now-it-lands");

        {
            let mut guard = app
                .transcript_writer
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            if let Some(w) = guard.take() {
                w.shutdown();
            }
        }
    }

    /// Torture test: 1000 threads alternating rotate_session / current_session_id
    /// for 10 seconds. Asserts no deadlock (10s wall-clock budget), no panic,
    /// and that the final state is readable + reflects one of the attempted IDs.
    ///
    /// Gated behind `#[ignore]` AND `RSAC_TORTURE=1` so it only runs under
    /// explicit opt-in. Without the env var, even `--ignored` makes it a
    /// no-op. Run with:
    ///
    /// ```text
    /// RSAC_TORTURE=1 cargo test --lib -- --ignored --test-threads=1 \
    ///   rotation_under_concurrent_load
    /// ```
    #[test]
    #[ignore = "torture test; gated on RSAC_TORTURE=1, run with --test-threads=1"]
    fn rotation_under_concurrent_load() {
        if std::env::var("RSAC_TORTURE").ok().as_deref() != Some("1") {
            eprintln!(
                "Skipping rotation_under_concurrent_load: set RSAC_TORTURE=1 to actually run"
            );
            return;
        }

        use std::sync::atomic::AtomicUsize;
        use std::time::{Duration, Instant};

        let app = Arc::new(AppState::new());
        let stop = Arc::new(AtomicBool::new(false));
        let rotate_ok = Arc::new(AtomicUsize::new(0));
        let rotate_skipped = Arc::new(AtomicUsize::new(0));
        let reads = Arc::new(AtomicUsize::new(0));

        // Heartbeat channel (ag#9): every worker ticks `(thread_id, iter)` at
        // least every HEARTBEAT_INTERVAL. A monitor thread tracks the latest
        // tick per thread; if any thread has missed HEARTBEAT_STALL_BUDGET
        // worth of wall-time we know *which* one stalled and can panic with a
        // pointed message instead of the generic "duration exceeded" that an
        // Instant-based deadline gives. Unbounded so workers never block on
        // send — the monitor drains as fast as it can recv_timeout, and any
        // lag would be a monitoring artifact, not a real stall.
        let (hb_tx, hb_rx) = crossbeam_channel::unbounded::<(usize, u64)>();
        const HEARTBEAT_INTERVAL: Duration = Duration::from_millis(100);
        // With 500 rotators contending for `rotation_in_progress` +
        // `session_id.write()` + `TranscriptWriter::spawn` (which does real
        // fs::create), a single rotator can legitimately wait seconds between
        // iterations. This budget is the "your thread is *definitely* stuck"
        // line, not the "your thread is moving slowly" line — false positives
        // are worse than late detection because a spurious panic masks real
        // bugs. 12s > the 10s test duration, so a true deadlock (thread never
        // makes progress for the full run) is the main signal this fires on.
        const HEARTBEAT_STALL_BUDGET: Duration = Duration::from_secs(12);

        let total_threads: usize = 1000;
        let mut handles = Vec::with_capacity(total_threads);

        for i in 0..total_threads {
            let app = app.clone();
            let stop = stop.clone();
            let rotate_ok = rotate_ok.clone();
            let rotate_skipped = rotate_skipped.clone();
            let reads = reads.clone();
            let hb_tx = hb_tx.clone();
            let h = std::thread::Builder::new()
                .name(format!("torture-{}", i))
                .spawn(move || {
                    let mut local_iter: u64 = 0;
                    let mut last_hb = Instant::now();
                    while !stop.load(Ordering::SeqCst) {
                        if i % 2 == 0 {
                            // Rotate-heavy path.
                            let new_id = format!("t{}-i{}", i, local_iter);
                            match app.rotate_session(&new_id) {
                                RotateOutcome::Rotated(_) => {
                                    rotate_ok.fetch_add(1, Ordering::Relaxed);
                                }
                                RotateOutcome::AlreadyRotating(_) => {
                                    rotate_skipped.fetch_add(1, Ordering::Relaxed);
                                }
                                RotateOutcome::Failed { reason, .. } => {
                                    panic!("torture rotation writer preflight failed: {reason}");
                                }
                            }
                        } else {
                            // Read-heavy path.
                            let id = app.current_session_id();
                            assert!(!id.is_empty(), "session_id must never be empty");
                            reads.fetch_add(1, Ordering::Relaxed);
                        }
                        local_iter = local_iter.wrapping_add(1);
                        // Heartbeat if enough wall-time has passed; rate-limit
                        // so the channel isn't hammered 10k times/sec/thread.
                        if last_hb.elapsed() >= HEARTBEAT_INTERVAL {
                            let _ = hb_tx.try_send((i, local_iter));
                            last_hb = Instant::now();
                        }
                    }
                })
                .expect("spawn torture thread");
            handles.push(h);
        }
        // Drop the producer-side clone held by the main thread so the monitor's
        // recv returns Disconnected once every worker exits. Each worker still
        // owns its own clone, so sends from workers keep working.
        drop(hb_tx);

        // Monitor thread: consumes heartbeats, tracks per-thread last-seen,
        // panics with the specific stuck thread_id if anyone goes silent for
        // HEARTBEAT_STALL_TICKS * HEARTBEAT_INTERVAL.
        let stop_mon = stop.clone();
        let monitor = std::thread::Builder::new()
            .name("torture-monitor".to_string())
            .spawn(move || -> Option<(usize, Duration)> {
                let mut last_seen: Vec<Option<Instant>> = vec![None; total_threads];
                loop {
                    match hb_rx.recv_timeout(HEARTBEAT_INTERVAL) {
                        Ok((tid, _iter)) => {
                            if tid < last_seen.len() {
                                last_seen[tid] = Some(Instant::now());
                            }
                        }
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                            // All workers have exited — normal shutdown.
                            return None;
                        }
                    }
                    // Once the main thread has signalled stop, workers will
                    // finish their current iteration and exit — heartbeats
                    // will stop arriving, but that is the expected shutdown
                    // condition, not a stall. Keep draining so `recv_timeout`
                    // eventually observes Disconnected, but skip the stall
                    // check.
                    if stop_mon.load(Ordering::SeqCst) {
                        continue;
                    }
                    // Check for stalls. A thread with no heartbeat *ever* is
                    // ignored — it may just not have had a chance to beat yet.
                    // Once a thread has beat at least once, any gap greater
                    // than HEARTBEAT_STALL_BUDGET is reported with its id so
                    // the failure points at the specific stuck thread.
                    let now = Instant::now();
                    for (tid, slot) in last_seen.iter().enumerate() {
                        if let Some(ts) = slot {
                            let gap = now.duration_since(*ts);
                            if gap > HEARTBEAT_STALL_BUDGET {
                                return Some((tid, gap));
                            }
                        }
                    }
                }
            })
            .expect("spawn monitor");

        let duration = Duration::from_secs(10);
        std::thread::sleep(duration);
        stop.store(true, Ordering::SeqCst);

        for h in handles {
            h.join().expect("torture thread panicked");
        }

        // Monitor exits when all workers drop their senders (Disconnected).
        // If it saw a stall before that, it returns Some((tid, gap)).
        let stall = monitor.join().expect("monitor thread panicked");
        if let Some((tid, gap)) = stall {
            panic!(
                "torture-{} stopped heartbeating for {:?} — likely deadlock",
                tid, gap
            );
        }

        // Final state must be readable.
        let final_id = app.current_session_id();
        assert!(!final_id.is_empty(), "final session id must be non-empty");

        // Sanity: we did meaningful work (at least some rotations + reads).
        let r_ok = rotate_ok.load(Ordering::Relaxed);
        let r_skip = rotate_skipped.load(Ordering::Relaxed);
        let reads_total = reads.load(Ordering::Relaxed);
        assert!(
            r_ok > 0,
            "at least one rotation must have succeeded (got ok={}, skip={})",
            r_ok,
            r_skip
        );
        assert!(reads_total > 0, "at least one read must have happened");

        eprintln!(
            "torture summary: rotations ok={}, rotations skipped={}, reads={}",
            r_ok, r_skip, reads_total
        );

        // Drain the writer so its thread doesn't outlive the test process.
        let mut guard = app
            .transcript_writer
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if let Some(w) = guard.take() {
            // Use the bounded-timeout variant explicitly, as a smoke test of
            // the new path.
            let joined = w.shutdown_with_timeout(Duration::from_secs(3));
            assert!(joined, "writer must finish flush within 3s on drain");
        }
        let mut event_guard = app
            .transcript_event_writer
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if let Some(w) = event_guard.take() {
            let joined = w.shutdown_with_timeout(Duration::from_secs(3));
            assert!(joined, "event writer must finish flush within 3s on drain");
        }
        let mut projection_guard = app
            .projection_event_writer
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if let Some(w) = projection_guard.take() {
            let joined = w.shutdown_with_timeout(Duration::from_secs(3));
            assert!(
                joined,
                "projection event writer must finish flush within 3s on drain"
            );
        }
    }
}
