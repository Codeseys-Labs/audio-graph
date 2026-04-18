//! Context struct types for the speech processor workers.
//!
//! The per-provider worker functions (local Whisper, cloud batch,
//! Deepgram/AssemblyAI/AWS streaming, sherpa-onnx) all share the same large
//! set of dependencies — channels, shared state, and static config. Bundling
//! those into three cohesive structs keeps the worker signatures to 3-5 args
//! and lets us drop the module-level `#![allow(clippy::too_many_arguments)]`.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, RwLock};

use crossbeam_channel::Receiver;
use tauri::AppHandle;

use crate::audio::pipeline::ProcessedAudioChunk;
use crate::events::PipelineStatus;
use crate::graph::entities::GraphSnapshot;
use crate::graph::extraction::RuleBasedExtractor;
use crate::graph::temporal::TemporalKnowledgeGraph;
use crate::llm::{ApiClient, LlmEngine, MistralRsEngine};
use crate::settings::LlmProvider;
use crate::state::TranscriptSegment;

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
    pub transcript_buffer: Arc<RwLock<VecDeque<TranscriptSegment>>>,
    pub transcript_writer: Arc<Mutex<Option<crate::persistence::TranscriptWriter>>>,
    pub pipeline_status: Arc<RwLock<PipelineStatus>>,
    pub app_handle: AppHandle,
    pub knowledge_graph: Arc<Mutex<TemporalKnowledgeGraph>>,
    pub graph_snapshot: Arc<RwLock<GraphSnapshot>>,
    pub graph_extractor: Arc<RuleBasedExtractor>,
    pub llm_engine: Arc<Mutex<Option<LlmEngine>>>,
    pub api_client: Arc<Mutex<Option<ApiClient>>>,
    pub mistralrs_engine: Arc<Mutex<Option<MistralRsEngine>>>,
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
}

/// Borrowed dependencies for entity extraction + graph update + event emit.
///
/// Used by `process_extraction_and_emit` and `spawn_extraction_task` — the
/// pre-refactor form of both of these took 12 scalar args. Grouping them here
/// eliminates the function-level `#[allow(clippy::too_many_arguments)]` on
/// those helpers.
pub(crate) struct ExtractionDeps<'a> {
    pub llm_engine: &'a Arc<Mutex<Option<LlmEngine>>>,
    pub api_client: &'a Arc<Mutex<Option<ApiClient>>>,
    pub mistralrs_engine: &'a Arc<Mutex<Option<MistralRsEngine>>>,
    pub llm_provider: &'a LlmProvider,
    pub graph_extractor: &'a Arc<RuleBasedExtractor>,
    pub knowledge_graph: &'a Arc<Mutex<TemporalKnowledgeGraph>>,
    pub graph_snapshot: &'a Arc<RwLock<GraphSnapshot>>,
    pub pipeline_status: &'a Arc<RwLock<PipelineStatus>>,
    pub app_handle: &'a AppHandle,
}
