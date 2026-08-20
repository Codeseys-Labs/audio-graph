//! Context struct types for the speech processor workers.
//!
//! The per-provider worker functions (local Whisper, cloud batch,
//! Deepgram/AssemblyAI/AWS streaming, sherpa-onnx) all share the same large
//! set of dependencies — channels, shared state, and static config. Bundling
//! those into three cohesive structs keeps the worker signatures to 3-5 args
//! and lets us drop the module-level `#![allow(clippy::too_many_arguments)]`.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, RwLock};

use crossbeam_channel::Receiver;
use tauri::AppHandle;

use crate::audio::pipeline::ProcessedAudioChunk;
use crate::events::{AgentProposalPayload, PipelineStatus};
use crate::graph::entities::GraphSnapshot;
use crate::graph::extraction::RuleBasedExtractor;
use crate::graph::temporal::TemporalKnowledgeGraph;
use crate::llm::{ApiClient, LlmEngine, LlmExecutor, MistralRsEngine};
use crate::settings::LlmProvider;
use crate::state::{ProjectionRuntimeHandle, TranscriptSegment};

/// Input/output channels and the cooperative-shutdown flag.
///
/// Owned by whichever worker drives audio in — dropping the receiver ends the
/// upstream pipeline, and toggling `is_transcribing` signals workers to exit.
pub(crate) struct SpeechChannels {
    pub processed_rx: Receiver<ProcessedAudioChunk>,
    pub is_transcribing: Arc<AtomicBool>,
}

/// Shared, cheaply-cloneable state that every worker needs access to.
///
/// All fields are `Arc`-wrapped so cloning this struct is a handful of
/// refcount bumps regardless of how deep the worker needs to pass it.
#[derive(Clone)]
pub(crate) struct SpeechShared {
    /// Backend-owned active session id. Background work captures its expected
    /// value when submitted and revalidates it before committing any result,
    /// so a task queued for session A cannot mutate session B after rotation.
    pub active_session_id: Arc<RwLock<String>>,
    pub transcript_buffer: Arc<RwLock<VecDeque<TranscriptSegment>>>,
    pub transcript_writer: Arc<Mutex<Option<crate::persistence::TranscriptWriter>>>,
    pub transcript_event_writer: Arc<Mutex<Option<crate::persistence::TranscriptEventWriter>>>,
    pub transcript_ledger: Arc<Mutex<crate::projections::TranscriptLedger>>,
    pub speaker_timeline: Arc<Mutex<crate::projections::SpeakerTimeline>>,
    pub projection_schedulers: Arc<Mutex<crate::projection_scheduler::ProjectionSchedulers>>,
    pub projection_runtime: ProjectionRuntimeHandle,
    pub pipeline_status: Arc<RwLock<PipelineStatus>>,
    pub app_handle: AppHandle,
    pub knowledge_graph: Arc<Mutex<TemporalKnowledgeGraph>>,
    pub graph_snapshot: Arc<RwLock<GraphSnapshot>>,
    pub graph_extractor: Arc<RuleBasedExtractor>,
    pub llm_engine: Arc<Mutex<Option<LlmEngine>>>,
    pub api_client: Arc<Mutex<Option<ApiClient>>>,
    pub mistralrs_engine: Arc<Mutex<Option<MistralRsEngine>>>,
    pub llm_executor: LlmExecutor,
    pub pending_agent_proposals: Arc<Mutex<HashMap<String, AgentProposalPayload>>>,
}

/// Immutable, process-local configuration applied to the whole speech session.
///
/// Provider-specific config (cloud endpoints, Deepgram keys, etc.) is passed
/// separately as the per-worker last argument since each worker is selected
/// based on which of these configs the caller supplies.
#[derive(Clone)]
pub(crate) struct SpeechConfig {
    pub models_dir: PathBuf,
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
    pub provider_content_egress_policy: crate::asr::ProviderContentEgressPolicy,
}

/// Borrowed dependencies for entity extraction + graph update + event emit.
///
/// Used by `process_extraction_and_emit` and `spawn_extraction_task` — the
/// pre-refactor form of both of these took 12 scalar args. Grouping them here
/// eliminates the function-level `#[allow(clippy::too_many_arguments)]` on
/// those helpers.
pub(crate) struct ExtractionDeps<'a> {
    /// Published active-session ownership used by the generation fence.
    pub active_session_id: &'a Arc<RwLock<String>>,
    /// The ledger changes session ownership before rotation clears graph state.
    /// Holding this lock across the commit closes the check-then-mutate race.
    pub transcript_ledger: &'a Arc<Mutex<crate::projections::TranscriptLedger>>,
    /// Session generation captured when this extraction task was submitted.
    pub expected_session_id: &'a str,
    pub llm_engine: &'a Arc<Mutex<Option<LlmEngine>>>,
    pub api_client: &'a Arc<Mutex<Option<ApiClient>>>,
    pub mistralrs_engine: &'a Arc<Mutex<Option<MistralRsEngine>>>,
    pub llm_executor: &'a LlmExecutor,
    pub llm_provider: &'a LlmProvider,
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
    pub graph_extractor: &'a Arc<RuleBasedExtractor>,
    pub knowledge_graph: &'a Arc<Mutex<TemporalKnowledgeGraph>>,
    pub graph_snapshot: &'a Arc<RwLock<GraphSnapshot>>,
    pub pipeline_status: &'a Arc<RwLock<PipelineStatus>>,
    pub app_handle: &'a AppHandle,
}
