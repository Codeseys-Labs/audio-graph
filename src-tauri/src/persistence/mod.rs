//! File-based persistence for transcripts and knowledge graph snapshots.
//!
//! Transcripts are appended as JSON lines (`.jsonl`) to a session file.
//! The knowledge graph is serialized as a single JSON file.
//!
//! All file I/O is performed asynchronously via a dedicated writer thread
//! to avoid blocking the speech processor or UI thread.

use std::fs;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::events::{
    LiveAssistCardRecord, LiveAssistCardStatus, PersistenceQueueBackpressurePayload,
};
use crate::projections::{
    DiarizationSpanRevision, HistoricalProjectionReplay, MaterializedGraph, MaterializedNotes,
    MaterializedProjectionState, ProjectionPatch, SpeakerTimeline, TranscriptEvent,
    TranscriptLedger,
};
use crate::promotion::{
    OrgKnowledgeItem, OrgKnowledgeState, PromotionConflictState, PromotionDraft, PromotionEvent,
    PromotionRevocationRequest, PromotionSourceSessionState, PromotionSyncState,
    PromotionSyncStatus, RedactionSnapshot, validate_source_session_state_for_promotion,
};
use crate::sessions::SessionMetadata;
use crate::state::TranscriptSegment;

pub mod data_movement;
pub mod io;
#[cfg(feature = "surrealdb-embedded")]
pub mod surreal;
pub use data_movement::{DataMovementLedgerBuilder, hash_artifact_path};
pub use io::write_or_emit_storage_full;

/// Re-export of the session data-movement ledger audit event schema
/// (seed audio-graph-70a3). The types are defined in the dependency-light
/// `audio-graph-ipc-contract` crate so the frontend TS contract can be
/// generated without linking the full app.
pub use audio_graph_ipc_contract::session_data_movement::{
    ArtifactRef, ArtifactStorageKind, DATA_MOVEMENT_SCHEMA_VERSION, DataClass, DataMovementActor,
    DataMovementDestination, DataMovementEvent, DataMovementEventType, DataMovementResult,
    DataMovementSource, DestinationBoundary, MovementBasis, MovementCounts, MovementModel,
    MovementPolicy, MovementStatus, PrivacyMode, RetentionClass,
};

/// User-facing retry after a `capture-storage-full` banner dismissal.
///
/// Probes the transcripts directory with a tiny write. On success, clears the
/// process-wide storage-full flag so the next real ENOSPC will re-emit, and
/// returns `Ok(())` — the banner should dismiss. On failure (disk still
/// full), leaves the flag set and returns `Err(io::Error)` so the UI keeps
/// the banner visible and can show the user they still need to free space.
///
/// Probing writes rather than trusting the writer-thread state: the writer
/// may not have attempted another segment since the failure, so only a real
/// write can confirm the disk is healthy again.
pub fn retry_storage_write() -> Result<(), std::io::Error> {
    let dir = transcripts_dir().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Transcripts directory could not be resolved (no HOME?)",
        )
    })?;
    io::probe_writable(&dir)?;
    io::clear_storage_full_flag();
    Ok(())
}

// ---------------------------------------------------------------------------
// AppHandle registration for background persistence threads
// ---------------------------------------------------------------------------
//
// The transcript writer and graph autosave threads are spawned before the
// Tauri runtime's `AppHandle` is threaded into the app state (and the
// spawn-site in `lib.rs` is intentionally untouched by this module). They
// still need an `AppHandle` to emit `CAPTURE_STORAGE_FULL` events on
// disk-full errors, so we stash one in a process-wide `OnceLock` that the
// speech processor (which receives an `AppHandle` at startup) initialises.
//
// If the handle hasn't been registered yet — e.g. a disk-full error fires
// before any speech processor has started — we fall back to logging only.

static APP_HANDLE: OnceLock<tauri::AppHandle> = OnceLock::new();

/// Register the Tauri `AppHandle` that persistence background threads should
/// use when emitting `CAPTURE_STORAGE_FULL` events. Safe to call repeatedly;
/// only the first call wins.
pub fn register_app_handle(handle: tauri::AppHandle) {
    let _ = APP_HANDLE.set(handle);
}

/// Return the registered `AppHandle`, if one has been set.
pub(crate) fn app_handle() -> Option<&'static tauri::AppHandle> {
    APP_HANDLE.get()
}

// ---------------------------------------------------------------------------
// Base directory resolution
// ---------------------------------------------------------------------------

/// Resolve the transcripts directory.
pub fn transcripts_dir() -> Option<PathBuf> {
    crate::user_data::transcripts_dir().ok()
}

/// Resolve the transcript event-log path for a session.
pub fn transcript_events_path(session_id: &str) -> Option<PathBuf> {
    crate::user_data::transcript_events_path(session_id).ok()
}

/// Resolve the projection event-log path for a session.
pub fn projection_events_path(session_id: &str) -> Option<PathBuf> {
    crate::user_data::projection_events_path(session_id).ok()
}

/// Resolve the diarization (speaker-timeline) event-log path for a session.
pub fn diarization_events_path(session_id: &str) -> Option<PathBuf> {
    crate::user_data::diarization_events_path(session_id).ok()
}

/// Resolve the data-movement ledger path for a session (seed audio-graph-70a3).
pub fn data_movement_ledger_path(session_id: &str) -> Option<PathBuf> {
    crate::user_data::data_movement_ledger_path(session_id).ok()
}

/// Resolve the graphs directory.
pub fn graphs_dir() -> Option<PathBuf> {
    crate::user_data::graphs_dir().ok()
}

/// Resolve the materialized notes path for a session.
pub fn notes_path(session_id: &str) -> Option<PathBuf> {
    crate::user_data::notes_path(session_id).ok()
}

/// Resolve the materialized projection graph path for a session.
pub fn materialized_graph_path(session_id: &str) -> Option<PathBuf> {
    crate::user_data::materialized_graph_path(session_id).ok()
}

/// Ensure a directory exists, creating it (and parents) if necessary.
fn ensure_dir(dir: &Path) -> Result<(), String> {
    if !dir.exists() {
        fs::create_dir_all(dir)
            .map_err(|e| format!("Failed to create directory {:?}: {}", dir, e))?;
    }
    Ok(())
}

fn push_unique_file_artifact(
    artifacts: &mut Vec<SessionArtifactDescriptor>,
    kind: SessionArtifactKind,
    label: impl Into<String>,
    path: PathBuf,
) {
    if path.as_os_str().is_empty()
        || artifacts.iter().any(|artifact| {
            matches!(&artifact.storage, SessionArtifactStorage::File { path: existing } if existing == &path)
        })
    {
        return;
    }
    artifacts.push(SessionArtifactDescriptor::file(kind, label, path));
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn append_jsonl<T: serde::Serialize>(value: &T, path: &Path, label: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        ensure_dir(parent)?;
    }

    let file = match fs::OpenOptions::new().create(true).append(true).open(path) {
        Ok(file) => file,
        Err(e) => {
            io::handle_write_error(app_handle(), path, 0, 0, &e);
            return Err(format!("Failed to open {label} log {:?}: {}", path, e));
        }
    };
    crate::fs_util::set_owner_only(path);

    let mut writer = BufWriter::new(file);
    if let Err(e) = serde_json::to_writer(&mut writer, value) {
        if e.classify() == serde_json::error::Category::Io {
            let io_err = std::io::Error::from(e);
            io::handle_write_error(app_handle(), path, 0, 0, &io_err);
            return Err(format!(
                "Failed to write {label} log {:?}: {}",
                path, io_err
            ));
        }
        return Err(format!("Failed to serialize {label} log {:?}: {}", path, e));
    }
    if let Err(e) = writer.write_all(b"\n") {
        io::handle_write_error(app_handle(), path, 0, 1, &e);
        return Err(format!("Failed to terminate {label} log {:?}: {}", path, e));
    }
    if let Err(e) = writer.flush() {
        io::handle_write_error(app_handle(), path, 0, 0, &e);
        return Err(format!("Failed to flush {label} log {:?}: {}", path, e));
    }
    let file = match writer.into_inner() {
        Ok(file) => file,
        Err(e) => {
            let io_err = e.into_error();
            io::handle_write_error(app_handle(), path, 0, 0, &io_err);
            return Err(format!(
                "Failed to flush {label} log {:?}: {}",
                path, io_err
            ));
        }
    };
    if let Err(e) = file.sync_all() {
        io::handle_write_error(app_handle(), path, 0, 0, &e);
        return Err(format!("Failed to fsync {label} log {:?}: {}", path, e));
    }

    Ok(())
}

fn load_session_index_from_path(path: &Path) -> Result<Vec<SessionMetadata>, String> {
    match fs::read_to_string(path) {
        Ok(contents) => match serde_json::from_str(&contents) {
            Ok(index) => Ok(index),
            Err(e) => {
                log::warn!("FileMemoryRepository: malformed {}: {}", path.display(), e);
                let backup = path.with_extension(format!("json.corrupt-{}", now_millis()));
                if let Err(copy_err) = fs::copy(path, &backup) {
                    log::warn!(
                        "FileMemoryRepository: failed to back up corrupt index {}: {}",
                        path.display(),
                        copy_err
                    );
                }
                Ok(Vec::new())
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("sessions: read index {}: {}", path.display(), e)),
    }
}

fn save_session_index_to_path(path: &Path, sessions: &[SessionMetadata]) -> Result<(), String> {
    save_json(&sessions, path)
}

fn load_json_array_or_empty<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Vec<T>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    load_json(path)
}

fn ensure_org_visible_record_is_safe(item: &OrgKnowledgeItem) -> Result<(), String> {
    const FORBIDDEN_KEYS: &[&str] = &[
        "api_key",
        "apikey",
        "access_token",
        "auth_token",
        "authorization",
        "bearer_token",
        "credential",
        "credentials",
        "secret",
        "raw_payload",
        "raw_transcript_text",
        "speaker_names",
        "source_ids",
        "provider_ids",
    ];

    fn visit(value: &serde_json::Value, path: &str) -> Result<(), String> {
        match value {
            serde_json::Value::Object(map) => {
                for (key, value) in map {
                    let normalized = key.to_ascii_lowercase();
                    if FORBIDDEN_KEYS.contains(&normalized.as_str()) {
                        return Err(format!(
                            "Org knowledge item contains forbidden org-visible key {path}.{key}"
                        ));
                    }
                    let next_path = if path.is_empty() {
                        key.to_string()
                    } else {
                        format!("{path}.{key}")
                    };
                    visit(value, &next_path)?;
                }
                Ok(())
            }
            serde_json::Value::Array(values) => {
                for (index, value) in values.iter().enumerate() {
                    visit(value, &format!("{path}[{index}]"))?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    let value = serde_json::to_value(item)
        .map_err(|error| format!("Failed to inspect org knowledge item privacy: {error}"))?;
    visit(&value, "org_knowledge_item")
}

fn validate_live_assist_card(session_id: &str, card: &LiveAssistCardRecord) -> Result<(), String> {
    crate::sessions::validate_session_id(session_id)?;
    if card.session_id != session_id {
        return Err(format!(
            "Live assist card {} session mismatch: {} != {}",
            card.proposal.id, card.session_id, session_id
        ));
    }
    if card.proposal.id.trim().is_empty() {
        return Err("Live assist card id is required".to_string());
    }
    if card.proposal.source_segment_id.trim().is_empty() {
        return Err(format!(
            "Live assist card {} source_segment_id is required",
            card.proposal.id
        ));
    }
    if card.source_span_ids.is_empty() && card.graph_context_ids.is_empty() {
        return Err(format!(
            "Live assist card {} must cite transcript spans or graph context",
            card.proposal.id
        ));
    }
    if matches!(card.status, LiveAssistCardStatus::Approved) && card.outcome.is_none() {
        return Err(format!(
            "Approved live assist card {} requires an outcome",
            card.proposal.id
        ));
    }
    if matches!(card.status, LiveAssistCardStatus::Approved)
        && card.projection_patch_sequence.is_none()
    {
        return Err(format!(
            "Approved live assist card {} requires a projection patch sequence",
            card.proposal.id
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionArtifactKind {
    LegacyTranscript,
    TranscriptEvents,
    DiarizationEvents,
    ProjectionEvents,
    MaterializedNotes,
    LegacyGraph,
    MaterializedGraph,
    LiveAssistAudit,
    LiveAssistCurrent,
    DataMovementLedger,
    SessionMetadata,
    RepositoryRecord,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "storage", rename_all = "snake_case")]
pub enum SessionArtifactStorage {
    File { path: PathBuf },
    RepositoryRecord { uri: String },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct SessionArtifactDescriptor {
    pub kind: SessionArtifactKind,
    pub label: String,
    pub storage: SessionArtifactStorage,
}

impl SessionArtifactDescriptor {
    pub fn file(
        kind: SessionArtifactKind,
        label: impl Into<String>,
        path: impl Into<PathBuf>,
    ) -> Self {
        Self {
            kind,
            label: label.into(),
            storage: SessionArtifactStorage::File { path: path.into() },
        }
    }

    pub fn repository_record(
        kind: SessionArtifactKind,
        label: impl Into<String>,
        uri: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            label: label.into(),
            storage: SessionArtifactStorage::RepositoryRecord { uri: uri.into() },
        }
    }

    pub fn file_path(&self) -> Option<&Path> {
        match &self.storage {
            SessionArtifactStorage::File { path } => Some(path.as_path()),
            SessionArtifactStorage::RepositoryRecord { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct SessionArtifactDeleteReport {
    pub session_id: String,
    pub deleted_files: Vec<PathBuf>,
    pub missing_files: Vec<PathBuf>,
    pub failed_files: Vec<String>,
    pub deleted_repository_records: Vec<String>,
}

impl SessionArtifactDeleteReport {
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            deleted_files: Vec::new(),
            missing_files: Vec::new(),
            failed_files: Vec::new(),
            deleted_repository_records: Vec::new(),
        }
    }

    pub fn has_failures(&self) -> bool {
        !self.failed_files.is_empty()
    }
}

/// Backend-owned repository boundary for local session memory artifacts.
///
/// The first implementation is [`FileMemoryRepository`]. Future adapters
/// (SurrealDB, cloud sync, org knowledge promotion) should conform to this
/// shape instead of reaching directly into `user_data` path helpers.
pub trait LocalMemoryRepository: Send + Sync {
    fn load_session_index(&self) -> Result<Vec<SessionMetadata>, String>;
    fn find_session(&self, session_id: &str) -> Result<Option<SessionMetadata>, String>;
    fn register_session(&self, session_id: &str) -> Result<(), String>;
    fn update_session_stats(
        &self,
        session_id: &str,
        segment_count: u64,
        speaker_count: u64,
        entity_count: u64,
    ) -> Result<(), String>;
    fn finalize_session(&self, session_id: &str) -> Result<(), String>;
    fn session_artifacts(&self, session_id: &str)
    -> Result<Vec<SessionArtifactDescriptor>, String>;
    fn session_artifact_paths(&self, session_id: &str) -> Result<Vec<PathBuf>, String> {
        Ok(self
            .session_artifacts(session_id)?
            .into_iter()
            .filter_map(|artifact| match artifact.storage {
                SessionArtifactStorage::File { path } => Some(path),
                SessionArtifactStorage::RepositoryRecord { .. } => None,
            })
            .collect())
    }
    fn delete_session_artifacts(
        &self,
        session_id: &str,
    ) -> Result<SessionArtifactDeleteReport, String> {
        crate::sessions::validate_session_id(session_id)?;
        let mut report = SessionArtifactDeleteReport::new(session_id);
        for path in self.session_artifact_paths(session_id)? {
            match fs::remove_file(&path) {
                Ok(_) => report.deleted_files.push(path),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    report.missing_files.push(path);
                }
                Err(error) => {
                    report
                        .failed_files
                        .push(format!("{}: {error}", path.display()));
                }
            }
        }
        Ok(report)
    }

    fn append_transcript_event(
        &self,
        session_id: &str,
        event: &TranscriptEvent,
    ) -> Result<(), String>;
    fn load_transcript_events(&self, session_id: &str) -> Result<Vec<TranscriptEvent>, String>;

    fn append_projection_patch(
        &self,
        session_id: &str,
        patch: &ProjectionPatch,
    ) -> Result<(), String>;
    fn load_projection_patches(&self, session_id: &str) -> Result<Vec<ProjectionPatch>, String>;

    /// Append an immutable diarization (speaker-timeline) span revision to the
    /// session's durable speaker log.
    ///
    /// Default implementations return an "unsupported" error so adapters that
    /// have not wired durable diarization storage fail loudly rather than
    /// silently dropping speaker retcons.
    fn append_diarization_span_revision(
        &self,
        _session_id: &str,
        _revision: &DiarizationSpanRevision,
    ) -> Result<(), String> {
        Err("append_diarization_span_revision not supported by this repository".to_string())
    }
    /// Load the session's durable diarization (speaker-timeline) span revisions
    /// in append order, for replay into a [`SpeakerTimeline`].
    ///
    /// Like [`Self::append_diarization_span_revision`], the default returns an
    /// "unsupported" error so adapters lacking durable diarization storage fail
    /// loudly rather than masquerading as an empty timeline.
    fn load_diarization_span_revisions(
        &self,
        _session_id: &str,
    ) -> Result<Vec<DiarizationSpanRevision>, String> {
        Err("load_diarization_span_revisions not supported by this repository".to_string())
    }

    /// Append a redacted data-movement audit event to the session's ledger
    /// (seed audio-graph-70a3).
    ///
    /// The ledger answers the trust question "where did this session's data
    /// go?": capture start/stop, provider calls, artifact writes/loads/
    /// export/delete, credential save/delete/readiness, projection jobs/
    /// patches, and org promotion state. The event is redacted by
    /// construction — it carries data classes, provider/model ids, source ids,
    /// destination boundaries, counts/hashes, statuses, and redacted errors,
    /// but never raw audio, raw transcript, prompt bodies, API keys, bearer
    /// tokens, service-account JSON, or full provider payloads.
    ///
    /// The default returns an "unsupported" error so adapters that have not
    /// wired durable ledger storage fail loudly rather than silently dropping
    /// audit events.
    fn append_data_movement_event(
        &self,
        _session_id: &str,
        _event: &DataMovementEvent,
    ) -> Result<(), String> {
        Err("append_data_movement_event not supported by this repository".to_string())
    }

    /// Load the session's data-movement ledger events in append order
    /// (seed audio-graph-70a3), for the privacy route report (audio-graph-51e0).
    ///
    /// Like [`Self::append_data_movement_event`], the default returns an
    /// "unsupported" error so adapters lacking durable ledger storage fail
    /// loudly rather than masquerading as an empty ledger.
    fn load_data_movement_events(
        &self,
        _session_id: &str,
    ) -> Result<Vec<DataMovementEvent>, String> {
        Err("load_data_movement_events not supported by this repository".to_string())
    }

    fn save_materialized_notes(
        &self,
        session_id: &str,
        notes: &MaterializedNotes,
    ) -> Result<(), String>;
    fn load_materialized_notes(
        &self,
        session_id: &str,
    ) -> Result<Option<MaterializedNotes>, String>;

    fn save_materialized_graph(
        &self,
        session_id: &str,
        graph: &MaterializedGraph,
    ) -> Result<(), String>;
    fn load_materialized_graph(
        &self,
        session_id: &str,
    ) -> Result<Option<MaterializedGraph>, String>;

    fn upsert_live_assist_card(
        &self,
        session_id: &str,
        card: &LiveAssistCardRecord,
    ) -> Result<(), String>;
    fn load_live_assist_card_audit(
        &self,
        session_id: &str,
    ) -> Result<Vec<LiveAssistCardRecord>, String>;
    fn load_live_assist_cards(&self, session_id: &str)
    -> Result<Vec<LiveAssistCardRecord>, String>;

    fn append_promotion_event(&self, event: &PromotionEvent) -> Result<(), String>;
    fn load_promotion_events(&self) -> Result<Vec<PromotionEvent>, String>;
    fn append_promotion_draft(&self, draft: &PromotionDraft) -> Result<(), String>;
    fn load_promotion_drafts(&self) -> Result<Vec<PromotionDraft>, String>;
    fn create_promotion_draft_checked(
        &self,
        draft: &PromotionDraft,
        source_state: PromotionSourceSessionState,
    ) -> Result<(), String> {
        validate_source_session_state_for_promotion(source_state).map_err(|error| {
            format!(
                "Blocked promotion draft {} for source session state {source_state:?}: {error:?}",
                draft.id
            )
        })?;
        self.append_promotion_draft(draft)
    }
    fn append_promotion_revocation_request(
        &self,
        request: &PromotionRevocationRequest,
    ) -> Result<(), String>;
    fn load_promotion_revocation_requests(&self)
    -> Result<Vec<PromotionRevocationRequest>, String>;
    fn append_redaction_snapshot(&self, snapshot: &RedactionSnapshot) -> Result<(), String>;
    fn load_redaction_snapshots(&self) -> Result<Vec<RedactionSnapshot>, String>;
    fn upsert_org_knowledge_item(&self, item: &OrgKnowledgeItem) -> Result<(), String>;
    fn load_org_knowledge_item_audit(&self) -> Result<Vec<OrgKnowledgeItem>, String>;
    fn load_org_knowledge_items(&self) -> Result<Vec<OrgKnowledgeItem>, String>;
    fn upsert_promotion_sync_state(&self, state: &PromotionSyncState) -> Result<(), String>;
    fn load_promotion_sync_state_audit(&self) -> Result<Vec<PromotionSyncState>, String>;
    fn load_promotion_sync_states(&self) -> Result<Vec<PromotionSyncState>, String>;
    fn revoke_org_knowledge_item(
        &self,
        item: &OrgKnowledgeItem,
        sync_state: Option<&PromotionSyncState>,
    ) -> Result<(), String>;

    /// Replay the session's transcript event log into a [`TranscriptLedger`]
    /// holding the latest accepted revision per span.
    ///
    /// Loads the rows via [`Self::load_transcript_events`] and folds them
    /// through [`TranscriptLedger::replay`]; a stale or conflicting revision in
    /// the log surfaces as an error rather than being silently dropped.
    fn replay_transcript_ledger(&self, session_id: &str) -> Result<TranscriptLedger, String> {
        let events = self.load_transcript_events(session_id)?;
        TranscriptLedger::replay(session_id, events)
            .map_err(|error| format!("Transcript replay failed for {session_id}: {error:?}"))
    }

    /// Replay the session's diarization event log into a [`SpeakerTimeline`]
    /// holding the latest accepted speaker attribution per span.
    ///
    /// Loads the rows via [`Self::load_diarization_span_revisions`] and folds
    /// them through [`SpeakerTimeline::replay`]. Adapters without durable
    /// diarization storage return the "unsupported" error from
    /// [`Self::load_diarization_span_revisions`]; a session that has never
    /// emitted diarization rows replays an empty timeline.
    fn replay_speaker_timeline(&self, session_id: &str) -> Result<SpeakerTimeline, String> {
        let revisions = self.load_diarization_span_revisions(session_id)?;
        SpeakerTimeline::replay(session_id, revisions)
            .map_err(|error| format!("Speaker timeline replay failed for {session_id}: {error:?}"))
    }

    fn replay_projection_state(
        &self,
        session_id: &str,
    ) -> Result<HistoricalProjectionReplay, String> {
        let transcript_events = self.load_transcript_events(session_id)?;
        let projection_patches = self.load_projection_patches(session_id)?;
        MaterializedProjectionState::replay_accepted_patches_with_transcript_history(
            session_id,
            transcript_events,
            projection_patches,
        )
        .map_err(|error| format!("Projection replay failed for {session_id}: {error:?}"))
    }

    fn load_materialized_projection_state(
        &self,
        session_id: &str,
    ) -> Result<MaterializedProjectionState, String> {
        Ok(MaterializedProjectionState {
            session_id: session_id.to_string(),
            notes: self
                .load_materialized_notes(session_id)?
                .unwrap_or_else(|| MaterializedNotes::new(session_id)),
            graph: self
                .load_materialized_graph(session_id)?
                .unwrap_or_else(|| MaterializedGraph::new(session_id)),
        })
    }
}

/// File-backed [`LocalMemoryRepository`] over AudioGraph's current artifact
/// layout: `sessions.json`, transcript event JSONL, projection patch JSONL,
/// materialized notes JSON, and materialized graph JSON.
#[derive(Debug, Clone, Default)]
pub struct FileMemoryRepository {
    data_root: Option<PathBuf>,
}

impl FileMemoryRepository {
    /// Use the current app data root resolved by [`crate::user_data`].
    pub fn user_data() -> Self {
        Self { data_root: None }
    }

    /// Use an explicit data root. Primarily useful for repository conformance
    /// tests and future migration tooling that should not mutate user data.
    pub fn with_data_root(root: impl Into<PathBuf>) -> Self {
        Self {
            data_root: Some(root.into()),
        }
    }

    fn explicit_root(&self) -> Option<&Path> {
        self.data_root.as_deref()
    }

    fn data_root(&self) -> Result<PathBuf, String> {
        match self.explicit_root() {
            Some(root) => {
                ensure_dir(root)?;
                Ok(root.to_path_buf())
            }
            None => crate::user_data::data_root(),
        }
    }

    fn sessions_index_path(&self) -> Result<PathBuf, String> {
        match self.explicit_root() {
            Some(_) => Ok(self.data_root()?.join("sessions.json")),
            None => crate::user_data::sessions_index_path(),
        }
    }

    fn transcripts_dir_path(&self) -> Result<PathBuf, String> {
        match self.explicit_root() {
            Some(_) => {
                let path = self.data_root()?.join("transcripts");
                ensure_dir(&path)?;
                Ok(path)
            }
            None => crate::user_data::transcripts_dir(),
        }
    }

    fn projections_dir_path(&self) -> Result<PathBuf, String> {
        match self.explicit_root() {
            Some(_) => {
                let path = self.data_root()?.join("projections");
                ensure_dir(&path)?;
                Ok(path)
            }
            None => crate::user_data::projections_dir(),
        }
    }

    fn graphs_dir_path(&self) -> Result<PathBuf, String> {
        match self.explicit_root() {
            Some(_) => {
                let path = self.data_root()?.join("graphs");
                ensure_dir(&path)?;
                Ok(path)
            }
            None => crate::user_data::graphs_dir(),
        }
    }

    fn notes_dir_path(&self) -> Result<PathBuf, String> {
        match self.explicit_root() {
            Some(_) => {
                let path = self.data_root()?.join("notes");
                ensure_dir(&path)?;
                Ok(path)
            }
            None => crate::user_data::notes_dir(),
        }
    }

    fn promotions_dir_path(&self) -> Result<PathBuf, String> {
        let path = self.data_root()?.join("promotions");
        ensure_dir(&path)?;
        Ok(path)
    }

    fn live_assist_dir_path(&self) -> Result<PathBuf, String> {
        let path = self.data_root()?.join("live_assist");
        ensure_dir(&path)?;
        Ok(path)
    }

    fn transcript_path(&self, session_id: &str) -> Result<PathBuf, String> {
        crate::sessions::validate_session_id(session_id)?;
        Ok(self
            .transcripts_dir_path()?
            .join(format!("{session_id}.jsonl")))
    }

    fn transcript_events_path(&self, session_id: &str) -> Result<PathBuf, String> {
        crate::sessions::validate_session_id(session_id)?;
        Ok(self
            .transcripts_dir_path()?
            .join(format!("{session_id}.events.jsonl")))
    }

    fn diarization_events_path(&self, session_id: &str) -> Result<PathBuf, String> {
        crate::sessions::validate_session_id(session_id)?;
        Ok(self
            .transcripts_dir_path()?
            .join(format!("{session_id}.speaker.jsonl")))
    }

    fn ledgers_dir_path(&self) -> Result<PathBuf, String> {
        match self.explicit_root() {
            Some(_) => {
                let path = self.data_root()?.join("ledgers");
                ensure_dir(&path)?;
                Ok(path)
            }
            None => crate::user_data::ledgers_dir(),
        }
    }

    fn data_movement_ledger_path(&self, session_id: &str) -> Result<PathBuf, String> {
        crate::sessions::validate_session_id(session_id)?;
        Ok(self
            .ledgers_dir_path()?
            .join(format!("{session_id}.movements.jsonl")))
    }

    fn projection_events_path(&self, session_id: &str) -> Result<PathBuf, String> {
        crate::sessions::validate_session_id(session_id)?;
        Ok(self
            .projections_dir_path()?
            .join(format!("{session_id}.events.jsonl")))
    }

    fn graph_path(&self, session_id: &str) -> Result<PathBuf, String> {
        crate::sessions::validate_session_id(session_id)?;
        Ok(self.graphs_dir_path()?.join(format!("{session_id}.json")))
    }

    fn materialized_graph_path(&self, session_id: &str) -> Result<PathBuf, String> {
        crate::sessions::validate_session_id(session_id)?;
        Ok(self
            .graphs_dir_path()?
            .join(format!("{session_id}.materialized.json")))
    }

    fn notes_path(&self, session_id: &str) -> Result<PathBuf, String> {
        crate::sessions::validate_session_id(session_id)?;
        Ok(self.notes_dir_path()?.join(format!("{session_id}.json")))
    }

    fn live_assist_audit_path(&self, session_id: &str) -> Result<PathBuf, String> {
        crate::sessions::validate_session_id(session_id)?;
        Ok(self
            .live_assist_dir_path()?
            .join(format!("{session_id}.jsonl")))
    }

    fn live_assist_current_path(&self, session_id: &str) -> Result<PathBuf, String> {
        crate::sessions::validate_session_id(session_id)?;
        Ok(self
            .live_assist_dir_path()?
            .join(format!("{session_id}.current.json")))
    }

    fn promotion_events_path(&self) -> Result<PathBuf, String> {
        Ok(self.promotions_dir_path()?.join("promotion_events.jsonl"))
    }

    fn promotion_drafts_path(&self) -> Result<PathBuf, String> {
        Ok(self.promotions_dir_path()?.join("promotion_drafts.jsonl"))
    }

    fn promotion_revocations_path(&self) -> Result<PathBuf, String> {
        Ok(self
            .promotions_dir_path()?
            .join("promotion_revocations.jsonl"))
    }

    fn redaction_snapshots_path(&self) -> Result<PathBuf, String> {
        Ok(self
            .promotions_dir_path()?
            .join("redaction_snapshots.jsonl"))
    }

    fn org_knowledge_audit_path(&self) -> Result<PathBuf, String> {
        Ok(self
            .promotions_dir_path()?
            .join("org_knowledge_items.jsonl"))
    }

    fn org_knowledge_current_path(&self) -> Result<PathBuf, String> {
        Ok(self
            .promotions_dir_path()?
            .join("org_knowledge_items.current.json"))
    }

    fn promotion_sync_audit_path(&self) -> Result<PathBuf, String> {
        Ok(self
            .promotions_dir_path()?
            .join("promotion_sync_states.jsonl"))
    }

    fn promotion_sync_current_path(&self) -> Result<PathBuf, String> {
        Ok(self
            .promotions_dir_path()?
            .join("promotion_sync_states.current.json"))
    }

    fn default_artifact_descriptors(
        &self,
        session_id: &str,
    ) -> Result<Vec<SessionArtifactDescriptor>, String> {
        let mut artifacts = Vec::new();
        push_unique_file_artifact(
            &mut artifacts,
            SessionArtifactKind::LegacyTranscript,
            "Legacy transcript JSONL",
            self.transcript_path(session_id)?,
        );
        push_unique_file_artifact(
            &mut artifacts,
            SessionArtifactKind::TranscriptEvents,
            "Transcript revision event log",
            self.transcript_events_path(session_id)?,
        );
        push_unique_file_artifact(
            &mut artifacts,
            SessionArtifactKind::DiarizationEvents,
            "Diarization span revision event log",
            self.diarization_events_path(session_id)?,
        );
        push_unique_file_artifact(
            &mut artifacts,
            SessionArtifactKind::ProjectionEvents,
            "Projection patch event log",
            self.projection_events_path(session_id)?,
        );
        push_unique_file_artifact(
            &mut artifacts,
            SessionArtifactKind::MaterializedNotes,
            "Materialized notes snapshot",
            self.notes_path(session_id)?,
        );
        push_unique_file_artifact(
            &mut artifacts,
            SessionArtifactKind::LegacyGraph,
            "Legacy temporal graph snapshot",
            self.graph_path(session_id)?,
        );
        push_unique_file_artifact(
            &mut artifacts,
            SessionArtifactKind::MaterializedGraph,
            "Materialized projection graph snapshot",
            self.materialized_graph_path(session_id)?,
        );
        push_unique_file_artifact(
            &mut artifacts,
            SessionArtifactKind::LiveAssistAudit,
            "Live assist audit log",
            self.live_assist_audit_path(session_id)?,
        );
        push_unique_file_artifact(
            &mut artifacts,
            SessionArtifactKind::LiveAssistCurrent,
            "Live assist current snapshot",
            self.live_assist_current_path(session_id)?,
        );
        push_unique_file_artifact(
            &mut artifacts,
            SessionArtifactKind::DataMovementLedger,
            "Session data-movement ledger",
            self.data_movement_ledger_path(session_id)?,
        );
        Ok(artifacts)
    }

    fn register_session_in_root(&self, session_id: &str) -> Result<(), String> {
        crate::sessions::validate_session_id(session_id)?;
        let index_path = self.sessions_index_path()?;
        let mut index = load_session_index_from_path(&index_path)?;
        for entry in index.iter_mut() {
            if entry.status == "active" && entry.id != session_id {
                entry.status = "crashed".into();
                entry.ended_at.get_or_insert_with(now_millis);
            }
        }

        index.insert(
            0,
            SessionMetadata {
                id: session_id.to_string(),
                title: None,
                created_at: now_millis(),
                ended_at: None,
                duration_seconds: None,
                status: "active".to_string(),
                segment_count: 0,
                speaker_count: 0,
                entity_count: 0,
                transcript_path: self
                    .transcript_path(session_id)?
                    .to_string_lossy()
                    .to_string(),
                graph_path: self.graph_path(session_id)?.to_string_lossy().to_string(),
                deleted: false,
                deleted_at: None,
            },
        );
        if index.len() > 100 {
            index.truncate(100);
        }
        save_session_index_to_path(&index_path, &index)
    }

    fn update_session_stats_in_root(
        &self,
        session_id: &str,
        segment_count: u64,
        speaker_count: u64,
        entity_count: u64,
    ) -> Result<(), String> {
        let index_path = self.sessions_index_path()?;
        let mut index = load_session_index_from_path(&index_path)?;
        if let Some(entry) = index.iter_mut().find(|entry| entry.id == session_id) {
            entry.segment_count = segment_count;
            entry.speaker_count = speaker_count;
            entry.entity_count = entity_count;
        }
        save_session_index_to_path(&index_path, &index)
    }

    fn finalize_session_in_root(&self, session_id: &str) -> Result<(), String> {
        let index_path = self.sessions_index_path()?;
        let mut index = load_session_index_from_path(&index_path)?;
        if let Some(entry) = index.iter_mut().find(|entry| entry.id == session_id) {
            entry.status = "complete".into();
            let end = now_millis();
            entry.ended_at = Some(end);
            entry.duration_seconds = Some(end.saturating_sub(entry.created_at) / 1000);
        }
        save_session_index_to_path(&index_path, &index)
    }
}

impl LocalMemoryRepository for FileMemoryRepository {
    fn load_session_index(&self) -> Result<Vec<SessionMetadata>, String> {
        match self.explicit_root() {
            Some(_) => load_session_index_from_path(&self.sessions_index_path()?),
            None => Ok(crate::sessions::load_index()),
        }
    }

    fn find_session(&self, session_id: &str) -> Result<Option<SessionMetadata>, String> {
        match self.explicit_root() {
            Some(_) => Ok(self
                .load_session_index()?
                .into_iter()
                .find(|entry| entry.id == session_id)),
            None => Ok(crate::sessions::find_session(session_id)),
        }
    }

    fn register_session(&self, session_id: &str) -> Result<(), String> {
        match self.explicit_root() {
            Some(_) => self.register_session_in_root(session_id),
            None => crate::sessions::register_session(session_id),
        }
    }

    fn update_session_stats(
        &self,
        session_id: &str,
        segment_count: u64,
        speaker_count: u64,
        entity_count: u64,
    ) -> Result<(), String> {
        match self.explicit_root() {
            Some(_) => self.update_session_stats_in_root(
                session_id,
                segment_count,
                speaker_count,
                entity_count,
            ),
            None => crate::sessions::update_stats(
                session_id,
                segment_count,
                speaker_count,
                entity_count,
            ),
        }
    }

    fn finalize_session(&self, session_id: &str) -> Result<(), String> {
        match self.explicit_root() {
            Some(_) => self.finalize_session_in_root(session_id),
            None => crate::sessions::finalize_session(session_id),
        }
    }

    fn session_artifacts(
        &self,
        session_id: &str,
    ) -> Result<Vec<SessionArtifactDescriptor>, String> {
        match self.explicit_root() {
            Some(_) => {
                let mut artifacts = Vec::new();
                if let Some(entry) = self.find_session(session_id)? {
                    push_unique_file_artifact(
                        &mut artifacts,
                        SessionArtifactKind::LegacyTranscript,
                        "Indexed transcript artifact",
                        PathBuf::from(entry.transcript_path),
                    );
                    push_unique_file_artifact(
                        &mut artifacts,
                        SessionArtifactKind::LegacyGraph,
                        "Indexed graph artifact",
                        PathBuf::from(entry.graph_path),
                    );
                }
                for artifact in self.default_artifact_descriptors(session_id)? {
                    if let SessionArtifactStorage::File { path } = &artifact.storage {
                        push_unique_file_artifact(
                            &mut artifacts,
                            artifact.kind.clone(),
                            artifact.label.clone(),
                            path.clone(),
                        );
                    }
                }
                Ok(artifacts)
            }
            None => Ok(crate::sessions::session_artifact_paths_for_id(session_id)
                .into_iter()
                .map(|path| {
                    SessionArtifactDescriptor::file(
                        SessionArtifactKind::RepositoryRecord,
                        "User data file artifact",
                        path,
                    )
                })
                .collect()),
        }
    }

    fn append_transcript_event(
        &self,
        session_id: &str,
        event: &TranscriptEvent,
    ) -> Result<(), String> {
        append_jsonl(
            event,
            &self.transcript_events_path(session_id)?,
            "transcript event",
        )
    }

    fn load_transcript_events(&self, session_id: &str) -> Result<Vec<TranscriptEvent>, String> {
        match self.explicit_root() {
            Some(_) => load_jsonl(&self.transcript_events_path(session_id)?),
            None => load_transcript_events(session_id),
        }
    }

    fn append_projection_patch(
        &self,
        session_id: &str,
        patch: &ProjectionPatch,
    ) -> Result<(), String> {
        append_jsonl(
            patch,
            &self.projection_events_path(session_id)?,
            "projection patch",
        )
    }

    fn load_projection_patches(&self, session_id: &str) -> Result<Vec<ProjectionPatch>, String> {
        match self.explicit_root() {
            Some(_) => load_jsonl(&self.projection_events_path(session_id)?),
            None => load_projection_events(session_id),
        }
    }

    fn append_diarization_span_revision(
        &self,
        session_id: &str,
        revision: &DiarizationSpanRevision,
    ) -> Result<(), String> {
        append_jsonl(
            revision,
            &self.diarization_events_path(session_id)?,
            "diarization span revision",
        )
    }

    fn load_diarization_span_revisions(
        &self,
        session_id: &str,
    ) -> Result<Vec<DiarizationSpanRevision>, String> {
        match self.explicit_root() {
            Some(_) => load_jsonl(&self.diarization_events_path(session_id)?),
            None => load_diarization_span_revisions(session_id),
        }
    }

    fn append_data_movement_event(
        &self,
        session_id: &str,
        event: &DataMovementEvent,
    ) -> Result<(), String> {
        append_jsonl(
            event,
            &self.data_movement_ledger_path(session_id)?,
            "data movement event",
        )
    }

    fn load_data_movement_events(
        &self,
        session_id: &str,
    ) -> Result<Vec<DataMovementEvent>, String> {
        load_jsonl(&self.data_movement_ledger_path(session_id)?)
    }

    fn save_materialized_notes(
        &self,
        session_id: &str,
        notes: &MaterializedNotes,
    ) -> Result<(), String> {
        match self.explicit_root() {
            Some(_) => save_json(notes, &self.notes_path(session_id)?),
            None => save_materialized_notes(session_id, notes),
        }
    }

    fn load_materialized_notes(
        &self,
        session_id: &str,
    ) -> Result<Option<MaterializedNotes>, String> {
        match self.explicit_root() {
            Some(_) => {
                let path = self.notes_path(session_id)?;
                if path.exists() {
                    load_json(&path).map(Some)
                } else {
                    Ok(None)
                }
            }
            None => load_materialized_notes(session_id),
        }
    }

    fn save_materialized_graph(
        &self,
        session_id: &str,
        graph: &MaterializedGraph,
    ) -> Result<(), String> {
        match self.explicit_root() {
            Some(_) => save_json(graph, &self.materialized_graph_path(session_id)?),
            None => save_materialized_graph(session_id, graph),
        }
    }

    fn load_materialized_graph(
        &self,
        session_id: &str,
    ) -> Result<Option<MaterializedGraph>, String> {
        match self.explicit_root() {
            Some(_) => {
                let path = self.materialized_graph_path(session_id)?;
                if path.exists() {
                    load_json(&path).map(Some)
                } else {
                    Ok(None)
                }
            }
            None => load_materialized_graph(session_id),
        }
    }

    fn upsert_live_assist_card(
        &self,
        session_id: &str,
        card: &LiveAssistCardRecord,
    ) -> Result<(), String> {
        validate_live_assist_card(session_id, card)?;
        append_jsonl(
            card,
            &self.live_assist_audit_path(session_id)?,
            "live assist card",
        )?;

        let current_path = self.live_assist_current_path(session_id)?;
        let mut current: Vec<LiveAssistCardRecord> = load_json_array_or_empty(&current_path)?;
        if let Some(existing) = current
            .iter_mut()
            .find(|existing| existing.proposal.id == card.proposal.id)
        {
            *existing = card.clone();
        } else {
            current.push(card.clone());
        }
        current.sort_by(|a, b| {
            a.proposal
                .created_at_ms
                .cmp(&b.proposal.created_at_ms)
                .then(a.proposal.id.cmp(&b.proposal.id))
        });
        save_json(&current, &current_path)
    }

    fn load_live_assist_card_audit(
        &self,
        session_id: &str,
    ) -> Result<Vec<LiveAssistCardRecord>, String> {
        load_jsonl(&self.live_assist_audit_path(session_id)?)
    }

    fn load_live_assist_cards(
        &self,
        session_id: &str,
    ) -> Result<Vec<LiveAssistCardRecord>, String> {
        load_json_array_or_empty(&self.live_assist_current_path(session_id)?)
    }

    fn append_promotion_event(&self, event: &PromotionEvent) -> Result<(), String> {
        event
            .validate()
            .map_err(|error| format!("Invalid promotion event {}: {error:?}", event.id))?;
        append_jsonl(event, &self.promotion_events_path()?, "promotion event")
    }

    fn load_promotion_events(&self) -> Result<Vec<PromotionEvent>, String> {
        load_jsonl(&self.promotion_events_path()?)
    }

    fn append_promotion_draft(&self, draft: &PromotionDraft) -> Result<(), String> {
        draft
            .validate()
            .map_err(|error| format!("Invalid promotion draft {}: {error:?}", draft.id))?;
        append_jsonl(draft, &self.promotion_drafts_path()?, "promotion draft")
    }

    fn load_promotion_drafts(&self) -> Result<Vec<PromotionDraft>, String> {
        load_jsonl(&self.promotion_drafts_path()?)
    }

    fn append_promotion_revocation_request(
        &self,
        request: &PromotionRevocationRequest,
    ) -> Result<(), String> {
        request.validate().map_err(|error| {
            format!(
                "Invalid promotion revocation request {}: {error:?}",
                request.id
            )
        })?;
        append_jsonl(
            request,
            &self.promotion_revocations_path()?,
            "promotion revocation request",
        )
    }

    fn load_promotion_revocation_requests(
        &self,
    ) -> Result<Vec<PromotionRevocationRequest>, String> {
        load_jsonl(&self.promotion_revocations_path()?)
    }

    fn append_redaction_snapshot(&self, snapshot: &RedactionSnapshot) -> Result<(), String> {
        snapshot
            .validate()
            .map_err(|error| format!("Invalid redaction snapshot {}: {error:?}", snapshot.id))?;
        append_jsonl(
            snapshot,
            &self.redaction_snapshots_path()?,
            "redaction snapshot",
        )
    }

    fn load_redaction_snapshots(&self) -> Result<Vec<RedactionSnapshot>, String> {
        load_jsonl(&self.redaction_snapshots_path()?)
    }

    fn upsert_org_knowledge_item(&self, item: &OrgKnowledgeItem) -> Result<(), String> {
        item.validate()
            .map_err(|error| format!("Invalid org knowledge item {}: {error:?}", item.id))?;
        ensure_org_visible_record_is_safe(item)?;

        let current_path = self.org_knowledge_current_path()?;
        let mut current: Vec<OrgKnowledgeItem> = load_json_array_or_empty(&current_path)?;
        if let Some(existing) = current.iter().find(|existing| existing.id == item.id) {
            let source_changed = existing.source_local_object_fingerprint
                != item.source_local_object_fingerprint
                || existing.source_promotion_event_id != item.source_promotion_event_id;
            if matches!(
                (&existing.state, &item.state),
                (OrgKnowledgeState::Active, OrgKnowledgeState::Active)
            ) && source_changed
                && matches!(&item.conflict_state, PromotionConflictState::None)
            {
                return Err(format!(
                    "Org knowledge item {} source changed without conflict/review state; create a new promotion draft",
                    item.id
                ));
            }
        }

        append_jsonl(
            item,
            &self.org_knowledge_audit_path()?,
            "org knowledge item",
        )?;

        if let Some(existing) = current.iter_mut().find(|existing| existing.id == item.id) {
            *existing = item.clone();
        } else {
            current.push(item.clone());
        }
        current.sort_by(|a, b| a.id.cmp(&b.id));
        save_json(&current, &current_path)
    }

    fn load_org_knowledge_items(&self) -> Result<Vec<OrgKnowledgeItem>, String> {
        load_json_array_or_empty(&self.org_knowledge_current_path()?)
    }

    fn load_org_knowledge_item_audit(&self) -> Result<Vec<OrgKnowledgeItem>, String> {
        load_jsonl(&self.org_knowledge_audit_path()?)
    }

    fn upsert_promotion_sync_state(&self, state: &PromotionSyncState) -> Result<(), String> {
        state.validate().map_err(|error| {
            format!(
                "Invalid promotion sync state {}: {error:?}",
                state.promotion_event_id
            )
        })?;
        append_jsonl(
            state,
            &self.promotion_sync_audit_path()?,
            "promotion sync state",
        )?;

        let current_path = self.promotion_sync_current_path()?;
        let mut current: Vec<PromotionSyncState> = load_json_array_or_empty(&current_path)?;
        if let Some(existing) = current.iter_mut().find(|existing| {
            existing.promotion_event_id == state.promotion_event_id
                && existing.target_kind == state.target_kind
        }) {
            *existing = state.clone();
        } else {
            current.push(state.clone());
        }
        current.sort_by(|a, b| {
            a.promotion_event_id
                .cmp(&b.promotion_event_id)
                .then(format!("{:?}", a.target_kind).cmp(&format!("{:?}", b.target_kind)))
        });
        save_json(&current, &current_path)
    }

    fn load_promotion_sync_states(&self) -> Result<Vec<PromotionSyncState>, String> {
        load_json_array_or_empty(&self.promotion_sync_current_path()?)
    }

    fn load_promotion_sync_state_audit(&self) -> Result<Vec<PromotionSyncState>, String> {
        load_jsonl(&self.promotion_sync_audit_path()?)
    }

    fn revoke_org_knowledge_item(
        &self,
        item: &OrgKnowledgeItem,
        sync_state: Option<&PromotionSyncState>,
    ) -> Result<(), String> {
        if !matches!(
            &item.state,
            OrgKnowledgeState::Retracted
                | OrgKnowledgeState::Deleted
                | OrgKnowledgeState::RetentionExpired
                | OrgKnowledgeState::PurgePending
                | OrgKnowledgeState::Purged
        ) {
            return Err(format!(
                "Org knowledge item {} revocation must use a terminal/retracted state",
                item.id
            ));
        }
        if item.deleted_at_ms.is_none() {
            return Err(format!(
                "Org knowledge item {} revocation requires deleted_at_ms",
                item.id
            ));
        }
        if item
            .delete_reason
            .as_deref()
            .unwrap_or("")
            .trim()
            .is_empty()
        {
            return Err(format!(
                "Org knowledge item {} revocation requires delete_reason",
                item.id
            ));
        }
        self.upsert_org_knowledge_item(item)?;
        if let Some(sync_state) = sync_state {
            if !matches!(&sync_state.status, PromotionSyncStatus::Revoked) {
                return Err(format!(
                    "Promotion sync state {} revocation must use revoked status",
                    sync_state.promotion_event_id
                ));
            }
            self.upsert_promotion_sync_state(sync_state)?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Async transcript writer (channel-based)
// ---------------------------------------------------------------------------

/// Messages sent to the transcript writer thread.
pub enum TranscriptWriteMsg {
    /// Append a transcript segment as a JSON line.
    Append(TranscriptSegment),
    /// Flush the writer and shut down.
    Shutdown,
}

/// Messages sent to the transcript event writer thread.
// Channel message enum: boxing the large `Append` variant would ripple
// through every send and match site for negligible benefit.
#[allow(clippy::large_enum_variant)]
pub enum TranscriptEventWriteMsg {
    /// Append an immutable transcript span revision as a JSON line.
    Append(TranscriptEvent),
    /// Flush the writer and shut down.
    Shutdown,
}

/// Messages sent to the projection event writer thread.
// Channel message enum: boxing the large `Append` variant would ripple
// through every send and match site for negligible benefit.
#[allow(clippy::large_enum_variant)]
pub enum ProjectionEventWriteMsg {
    /// Append a replayable notes/graph projection patch as a JSON line.
    Append(ProjectionPatch),
    /// Flush the writer and shut down.
    Shutdown,
}

/// Poll interval for the writer's `recv_timeout`. Small enough that shutdown
/// latency is ~tens of ms on an idle channel, large enough that we don't burn
/// CPU when no segments are arriving.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

/// Maximum queued transcript/projection events per durable event writer.
///
/// The policy is drop-new-on-full: producers never block the ASR/projection
/// runtime, queue pressure is surfaced through a redacted event, and the writer
/// drains whatever was already accepted on shutdown.
const EVENT_WRITER_QUEUE_CAPACITY: usize = 2048;

fn emit_event_writer_queue_pressure(
    writer: &'static str,
    is_backpressured: bool,
    queue_capacity: usize,
    dropped_count: u64,
) {
    if let Some(app) = app_handle() {
        crate::events::emit_or_log(
            app,
            crate::events::PERSISTENCE_QUEUE_BACKPRESSURE,
            PersistenceQueueBackpressurePayload {
                writer: writer.to_string(),
                is_backpressured,
                queue_capacity,
                dropped_count,
            },
        );
    } else {
        log::warn!(
            "Persistence queue pressure event suppressed; no AppHandle registered writer={} backpressured={} dropped_count={}",
            writer,
            is_backpressured,
            dropped_count
        );
    }
}

fn note_event_writer_enqueue_success(
    writer: &'static str,
    queue_capacity: usize,
    dropped_count: &AtomicU64,
    queue_full_active: &AtomicBool,
) {
    if queue_full_active.swap(false, Ordering::SeqCst) {
        emit_event_writer_queue_pressure(
            writer,
            false,
            queue_capacity,
            dropped_count.load(Ordering::SeqCst),
        );
    }
}

fn note_event_writer_queue_full(
    writer: &'static str,
    queue_capacity: usize,
    dropped_count: &AtomicU64,
    queue_full_active: &AtomicBool,
) {
    let dropped = dropped_count.fetch_add(1, Ordering::SeqCst) + 1;
    log::warn!(
        "Persistence event writer queue full; dropping new event writer={} capacity={} dropped_count={}",
        writer,
        queue_capacity,
        dropped
    );
    if !queue_full_active.swap(true, Ordering::SeqCst) {
        emit_event_writer_queue_pressure(writer, true, queue_capacity, dropped);
    }
}

fn note_event_writer_disconnected(writer: &'static str) {
    log::warn!(
        "Persistence event writer queue disconnected writer={}",
        writer
    );
}

fn write_segment(writer: &mut BufWriter<fs::File>, segment: &TranscriptSegment, file_path: &Path) {
    match serde_json::to_string(segment) {
        Ok(json) => {
            let bytes_lost = json.len() as u64 + 1;
            if let Err(e) = writeln!(writer, "{}", json) {
                io::handle_write_error(app_handle(), file_path, 0, bytes_lost, &e);
            }
        }
        Err(e) => {
            log::warn!("Transcript writer: serialize error: {}", e);
        }
    }
}

fn write_transcript_event(
    writer: &mut BufWriter<fs::File>,
    event: &TranscriptEvent,
    file_path: &Path,
) {
    match serde_json::to_string(event) {
        Ok(json) => {
            let bytes_lost = json.len() as u64 + 1;
            if let Err(e) = writeln!(writer, "{}", json) {
                io::handle_write_error(app_handle(), file_path, 0, bytes_lost, &e);
            }
        }
        Err(e) => {
            log::warn!("Transcript event writer: serialize error: {}", e);
        }
    }
}

fn write_projection_event(
    writer: &mut BufWriter<fs::File>,
    patch: &ProjectionPatch,
    file_path: &Path,
) {
    match serde_json::to_string(patch) {
        Ok(json) => {
            let bytes_lost = json.len() as u64 + 1;
            if let Err(e) = writeln!(writer, "{}", json) {
                io::handle_write_error(app_handle(), file_path, 0, bytes_lost, &e);
            }
        }
        Err(e) => {
            log::warn!("Projection event writer: serialize error: {}", e);
        }
    }
}

/// Drain any buffered messages after shutdown is requested, so segments that
/// were already in the channel when `shutdown_requested` flipped still land on
/// disk. Stops at the first `Shutdown` message or when the channel empties.
fn drain_remaining(
    rx: &mpsc::Receiver<TranscriptWriteMsg>,
    writer: &mut BufWriter<fs::File>,
    file_path: &Path,
) {
    while let Ok(msg) = rx.try_recv() {
        match msg {
            TranscriptWriteMsg::Append(segment) => {
                write_segment(writer, &segment, file_path);
            }
            TranscriptWriteMsg::Shutdown => break,
        }
    }
}

fn drain_remaining_events(
    rx: &mpsc::Receiver<TranscriptEventWriteMsg>,
    writer: &mut BufWriter<fs::File>,
    file_path: &Path,
) {
    while let Ok(msg) = rx.try_recv() {
        match msg {
            TranscriptEventWriteMsg::Append(event) => {
                write_transcript_event(writer, &event, file_path);
            }
            TranscriptEventWriteMsg::Shutdown => break,
        }
    }
}

fn drain_remaining_projection_events(
    rx: &mpsc::Receiver<ProjectionEventWriteMsg>,
    writer: &mut BufWriter<fs::File>,
    file_path: &Path,
) {
    while let Ok(msg) = rx.try_recv() {
        match msg {
            ProjectionEventWriteMsg::Append(patch) => {
                write_projection_event(writer, &patch, file_path);
            }
            ProjectionEventWriteMsg::Shutdown => break,
        }
    }
}

fn drain_remaining_repository_transcript_events(
    rx: &mpsc::Receiver<TranscriptEventWriteMsg>,
    session_id: &str,
    repository: &dyn LocalMemoryRepository,
) {
    while let Ok(msg) = rx.try_recv() {
        match msg {
            TranscriptEventWriteMsg::Append(event) => {
                if let Err(e) = repository.append_transcript_event(session_id, &event) {
                    log::warn!("Transcript event repository writer: failed to append event: {e}");
                }
            }
            TranscriptEventWriteMsg::Shutdown => break,
        }
    }
}

fn drain_remaining_repository_projection_events(
    rx: &mpsc::Receiver<ProjectionEventWriteMsg>,
    session_id: &str,
    repository: &dyn LocalMemoryRepository,
) {
    while let Ok(msg) = rx.try_recv() {
        match msg {
            ProjectionEventWriteMsg::Append(patch) => {
                if let Err(e) = repository.append_projection_patch(session_id, &patch) {
                    log::warn!("Projection event repository writer: failed to append patch: {e}");
                }
            }
            ProjectionEventWriteMsg::Shutdown => break,
        }
    }
}

/// Handle to the transcript writer thread.
pub struct TranscriptWriter {
    tx: mpsc::Sender<TranscriptWriteMsg>,
    /// Writer thread handle. Taken by `shutdown_with_timeout` so the caller
    /// can wait on it with a bounded timeout; left as `None` after that.
    /// On drop-without-shutdown the handle is simply released (detached).
    handle: Option<std::thread::JoinHandle<()>>,
    /// Shutdown flag shared with the writer thread. Set by `shutdown()` /
    /// `shutdown_with_timeout()`; the writer's `recv_timeout` poll checks it
    /// each tick and exits promptly even if no `Shutdown` message is drained.
    /// Dropping the `Sender` alone is not enough — if the channel still has
    /// buffered `Append` messages, the writer would keep flushing them before
    /// seeing the hang-up, holding the file handle open. The flag lets the
    /// writer short-circuit after draining what's already queued, so a new
    /// writer on the same file path can't overlap.
    shutdown_requested: Arc<AtomicBool>,
}

impl TranscriptWriter {
    /// Spawn a new transcript writer thread for the given session.
    ///
    /// Returns `None` if the base directory cannot be resolved or created.
    pub fn spawn(session_id: &str) -> Option<Self> {
        let dir = transcripts_dir()?;
        if let Err(e) = ensure_dir(&dir) {
            log::warn!("Transcript persistence disabled: {}", e);
            return None;
        }

        let file_path = dir.join(format!("{}.jsonl", session_id));
        let (tx, rx) = mpsc::channel::<TranscriptWriteMsg>();
        let shutdown_requested = Arc::new(AtomicBool::new(false));
        let shutdown_flag = shutdown_requested.clone();
        let thread_path = file_path.clone();

        let handle = std::thread::Builder::new()
            .name("transcript-writer".to_string())
            .spawn(move || {
                let file = match fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&thread_path)
                {
                    Ok(f) => f,
                    Err(e) => {
                        // Classify the open error too — a user out of disk
                        // can hit ENOSPC on the very first file creation.
                        io::handle_write_error(app_handle(), &thread_path, 0, 0, &e);
                        return;
                    }
                };
                // Lock down perms as soon as the file exists. Transcripts can
                // contain sensitive speech content.
                crate::fs_util::set_owner_only(&thread_path);
                let mut writer = BufWriter::new(file);

                // `io::handle_write_error` owns the "first ENOSPC emits, rest
                // log" debounce via the process-wide `STORAGE_FULL_ACTIVE`
                // atomic, so this loop can forward every error through it
                // without its own local flag. The retry command resets the
                // atomic after a successful probe, which in turn lets the
                // *next* real ENOSPC re-emit.
                //
                // Use `recv_timeout` instead of `recv` so we can poll the
                // shutdown flag each tick. Without this, a slow drain of
                // buffered `Append` messages would delay the writer's exit,
                // keeping the file handle open past the point where a new
                // writer (for a rotated session) wants to open the same path.
                'outer: loop {
                    match rx.recv_timeout(POLL_INTERVAL) {
                        Ok(TranscriptWriteMsg::Append(segment)) => {
                            write_segment(&mut writer, &segment, &thread_path);
                            // After writing, if shutdown was requested, drain
                            // anything already queued (best-effort) and exit.
                            if shutdown_flag.load(Ordering::SeqCst) {
                                drain_remaining(&rx, &mut writer, &thread_path);
                                break 'outer;
                            }
                        }
                        Ok(TranscriptWriteMsg::Shutdown) => {
                            break 'outer;
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            if shutdown_flag.load(Ordering::SeqCst) {
                                drain_remaining(&rx, &mut writer, &thread_path);
                                break 'outer;
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                            break 'outer;
                        }
                    }
                }

                // Final flush on channel close. Instrumented (ag#8):
                // the wall-clock cost of this BufWriter::flush is the
                // dominant term in the rotation shutdown budget. Logging
                // it per-rotation gives us the data we need to tune
                // TRANSCRIPT_WRITER_SHUTDOWN_TIMEOUT against real p99.
                let flush_start = std::time::Instant::now();
                let flush_result = writer.flush();
                let flush_elapsed = flush_start.elapsed();
                if let Err(e) = flush_result {
                    io::handle_write_error(app_handle(), &thread_path, 0, 0, &e);
                    log::info!(
                        "transcript_writer.final_flush file={:?} elapsed_ms={} outcome=error",
                        thread_path,
                        flush_elapsed.as_millis()
                    );
                } else {
                    log::info!(
                        "transcript_writer.final_flush file={:?} elapsed_ms={} outcome=ok",
                        thread_path,
                        flush_elapsed.as_millis()
                    );
                }
                log::info!("Transcript writer: shut down for {:?}", thread_path);
            })
            .ok()?;

        Some(Self {
            tx,
            handle: Some(handle),
            shutdown_requested,
        })
    }

    /// Enqueue a transcript segment for writing. Non-blocking.
    pub fn append(&self, segment: &TranscriptSegment) {
        // Best-effort; if the channel is full or closed, we log and move on.
        if let Err(e) = self.tx.send(TranscriptWriteMsg::Append(segment.clone())) {
            log::warn!("Transcript writer: failed to enqueue segment: {}", e);
        }
    }

    /// Signal the writer to flush and shut down.
    ///
    /// Non-blocking: flips the shutdown flag and sends the `Shutdown` sentinel.
    /// The thread will exit on its own after flushing (and draining anything
    /// already queued). Use [`Self::shutdown_with_timeout`] when the caller
    /// needs bounded assurance that flush completed before moving on.
    ///
    /// Setting the flag before sending the message matters: a slow writer
    /// mid-`Append` checks the flag after the write lands and exits on the
    /// next tick instead of draining the whole queue first.
    pub fn shutdown(&self) {
        self.shutdown_requested.store(true, Ordering::SeqCst);
        let _ = self.tx.send(TranscriptWriteMsg::Shutdown);
    }

    /// Signal shutdown and wait (up to `timeout`) for the writer thread to exit.
    ///
    /// Returns `true` if the thread joined within the timeout, `false` if the
    /// wait expired (the thread is left detached — it will eventually exit on
    /// its own when the underlying I/O unsticks, or be torn down at process
    /// exit). On `false` the caller should assume some un-flushed segments may
    /// still be in the writer's BufWriter and proceed with spawning a new
    /// writer anyway — the alternative is blocking the rotation IPC
    /// indefinitely on a wedged disk, which is worse than a rare lost tail.
    ///
    /// Implementation note: `JoinHandle::join` is blocking with no timeout
    /// overload in std. We move the handle into a watchdog thread that
    /// performs the join, and signal completion via a `mpsc` channel so the
    /// calling thread can `recv_timeout`. On timeout the watchdog is itself
    /// leaked — the JoinHandle inside it prevents the writer thread from
    /// becoming a true zombie, just an unobserved one.
    pub fn shutdown_with_timeout(mut self, timeout: std::time::Duration) -> bool {
        self.shutdown_requested.store(true, Ordering::SeqCst);
        let _ = self.tx.send(TranscriptWriteMsg::Shutdown);
        let Some(handle) = self.handle.take() else {
            return true;
        };
        let (done_tx, done_rx) = mpsc::channel::<()>();
        // Watchdog thread: blocks on join, then signals. If join panics in the
        // writer thread we still signal (the `Err` from join is just a panic
        // propagation; we're shutting down anyway).
        let spawned = std::thread::Builder::new()
            .name("transcript-writer-join".to_string())
            .spawn(move || {
                let _ = handle.join();
                let _ = done_tx.send(());
            });
        match spawned {
            Ok(_watchdog) => {
                // `_watchdog`'s JoinHandle is dropped here (detached). That's
                // fine: its lifetime is bounded by the writer thread's join,
                // which is what we want. We wait on done_rx only.
                //
                // Instrumentation (ag#8): time how long the join actually
                // takes. Combined with the writer-thread-side
                // `transcript_writer.final_flush elapsed_ms=…` line, this
                // gives us the full picture — caller-observed wall clock
                // vs. kernel-side flush cost. Once we have a couple of
                // weeks of field data, tune TRANSCRIPT_WRITER_SHUTDOWN_TIMEOUT
                // to p99(join) + safety margin.
                let join_start = std::time::Instant::now();
                let joined = done_rx.recv_timeout(timeout).is_ok();
                let elapsed = join_start.elapsed();
                log::info!(
                    "transcript_writer.shutdown_join elapsed_ms={} timeout_ms={} joined={}",
                    elapsed.as_millis(),
                    timeout.as_millis(),
                    joined
                );
                joined
            }
            Err(e) => {
                log::warn!(
                    "Failed to spawn transcript-writer-join watchdog: {} — \
                     writer thread is detached",
                    e
                );
                // Couldn't spawn the watchdog; we can't bound the wait, so
                // report "timed out" rather than block the caller.
                false
            }
        }
    }
}

/// Handle to the immutable transcript event-log writer thread.
pub struct TranscriptEventWriter {
    tx: mpsc::SyncSender<TranscriptEventWriteMsg>,
    handle: Option<std::thread::JoinHandle<()>>,
    shutdown_requested: Arc<AtomicBool>,
    queue_capacity: usize,
    dropped_event_count: Arc<AtomicU64>,
    queue_full_active: Arc<AtomicBool>,
}

impl TranscriptEventWriter {
    /// Spawn a new transcript event writer thread for the given session.
    pub fn spawn(session_id: &str) -> Option<Self> {
        let file_path = transcript_events_path(session_id)?;
        if let Some(parent) = file_path.parent()
            && let Err(e) = ensure_dir(parent)
        {
            log::warn!("Transcript event persistence disabled: {}", e);
            return None;
        }

        let (tx, rx) = mpsc::sync_channel::<TranscriptEventWriteMsg>(EVENT_WRITER_QUEUE_CAPACITY);
        let shutdown_requested = Arc::new(AtomicBool::new(false));
        let shutdown_flag = shutdown_requested.clone();
        let thread_path = file_path.clone();

        let handle = std::thread::Builder::new()
            .name("transcript-event-writer".to_string())
            .spawn(move || {
                let file = match fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&thread_path)
                {
                    Ok(f) => f,
                    Err(e) => {
                        io::handle_write_error(app_handle(), &thread_path, 0, 0, &e);
                        return;
                    }
                };
                crate::fs_util::set_owner_only(&thread_path);
                let mut writer = BufWriter::new(file);

                'outer: loop {
                    match rx.recv_timeout(POLL_INTERVAL) {
                        Ok(TranscriptEventWriteMsg::Append(event)) => {
                            write_transcript_event(&mut writer, &event, &thread_path);
                            if shutdown_flag.load(Ordering::SeqCst) {
                                drain_remaining_events(&rx, &mut writer, &thread_path);
                                break 'outer;
                            }
                        }
                        Ok(TranscriptEventWriteMsg::Shutdown) => break 'outer,
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            if shutdown_flag.load(Ordering::SeqCst) {
                                drain_remaining_events(&rx, &mut writer, &thread_path);
                                break 'outer;
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break 'outer,
                    }
                }

                let flush_start = std::time::Instant::now();
                let flush_result = writer.flush();
                let flush_elapsed = flush_start.elapsed();
                if let Err(e) = flush_result {
                    io::handle_write_error(app_handle(), &thread_path, 0, 0, &e);
                    log::info!(
                        "transcript_event_writer.final_flush file={:?} elapsed_ms={} outcome=error",
                        thread_path,
                        flush_elapsed.as_millis()
                    );
                } else {
                    log::info!(
                        "transcript_event_writer.final_flush file={:?} elapsed_ms={} outcome=ok",
                        thread_path,
                        flush_elapsed.as_millis()
                    );
                }
                log::info!("Transcript event writer: shut down for {:?}", thread_path);
            })
            .ok()?;

        Some(Self {
            tx,
            handle: Some(handle),
            shutdown_requested,
            queue_capacity: EVENT_WRITER_QUEUE_CAPACITY,
            dropped_event_count: Arc::new(AtomicU64::new(0)),
            queue_full_active: Arc::new(AtomicBool::new(false)),
        })
    }

    #[cfg(test)]
    pub(crate) fn saturated_for_tests(event: TranscriptEvent) -> Self {
        let (tx, rx) = mpsc::sync_channel::<TranscriptEventWriteMsg>(1);
        tx.try_send(TranscriptEventWriteMsg::Append(event))
            .expect("pre-fill transcript event queue");
        std::mem::forget(rx);
        Self {
            tx,
            handle: None,
            shutdown_requested: Arc::new(AtomicBool::new(false)),
            queue_capacity: 1,
            dropped_event_count: Arc::new(AtomicU64::new(0)),
            queue_full_active: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Build a repository-backed transcript event writer.
    ///
    /// This keeps the runtime writer contract available for DB-backed adapters
    /// without changing the default file-backed writer path.
    pub fn repository(
        session_id: impl Into<String>,
        repository: Arc<dyn LocalMemoryRepository>,
    ) -> Option<Self> {
        let session_id = session_id.into();
        if let Err(e) = crate::sessions::validate_session_id(&session_id) {
            log::warn!("Transcript event repository writer disabled: {e}");
            return None;
        }

        let (tx, rx) = mpsc::sync_channel::<TranscriptEventWriteMsg>(EVENT_WRITER_QUEUE_CAPACITY);
        let shutdown_requested = Arc::new(AtomicBool::new(false));
        let shutdown_flag = shutdown_requested.clone();
        let thread_session_id = session_id.clone();
        let thread_repository = repository.clone();

        let handle = std::thread::Builder::new()
            .name("transcript-event-repository-writer".to_string())
            .spawn(move || {
                'outer: loop {
                    match rx.recv_timeout(POLL_INTERVAL) {
                        Ok(TranscriptEventWriteMsg::Append(event)) => {
                            if let Err(e) =
                                thread_repository.append_transcript_event(&thread_session_id, &event)
                            {
                                log::warn!(
                                    "Transcript event repository writer: failed to append event: {e}"
                                );
                            }
                            if shutdown_flag.load(Ordering::SeqCst) {
                                drain_remaining_repository_transcript_events(
                                    &rx,
                                    &thread_session_id,
                                    thread_repository.as_ref(),
                                );
                                break 'outer;
                            }
                        }
                        Ok(TranscriptEventWriteMsg::Shutdown) => break 'outer,
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            if shutdown_flag.load(Ordering::SeqCst) {
                                drain_remaining_repository_transcript_events(
                                    &rx,
                                    &thread_session_id,
                                    thread_repository.as_ref(),
                                );
                                break 'outer;
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break 'outer,
                    }
                }
                log::info!(
                    "Transcript event repository writer: shut down for {}",
                    thread_session_id
                );
            })
            .ok()?;

        Some(Self {
            tx,
            handle: Some(handle),
            shutdown_requested,
            queue_capacity: EVENT_WRITER_QUEUE_CAPACITY,
            dropped_event_count: Arc::new(AtomicU64::new(0)),
            queue_full_active: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Enqueue a transcript event for writing. Non-blocking.
    pub fn append(&self, event: &TranscriptEvent) -> bool {
        match self
            .tx
            .try_send(TranscriptEventWriteMsg::Append(event.clone()))
        {
            Ok(()) => {
                note_event_writer_enqueue_success(
                    "transcript_event",
                    self.queue_capacity,
                    &self.dropped_event_count,
                    &self.queue_full_active,
                );
                true
            }
            Err(mpsc::TrySendError::Full(_)) => {
                note_event_writer_queue_full(
                    "transcript_event",
                    self.queue_capacity,
                    &self.dropped_event_count,
                    &self.queue_full_active,
                );
                false
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                note_event_writer_disconnected("transcript_event");
                false
            }
        }
    }

    pub fn shutdown(&self) {
        self.shutdown_requested.store(true, Ordering::SeqCst);
        let _ = self.tx.try_send(TranscriptEventWriteMsg::Shutdown);
    }

    pub fn shutdown_with_timeout(mut self, timeout: std::time::Duration) -> bool {
        self.shutdown_requested.store(true, Ordering::SeqCst);
        let _ = self.tx.try_send(TranscriptEventWriteMsg::Shutdown);
        let Some(handle) = self.handle.take() else {
            return true;
        };
        let (done_tx, done_rx) = mpsc::channel::<()>();
        let spawned = std::thread::Builder::new()
            .name("transcript-event-writer-join".to_string())
            .spawn(move || {
                let _ = handle.join();
                let _ = done_tx.send(());
            });
        match spawned {
            Ok(_watchdog) => {
                let join_start = std::time::Instant::now();
                let joined = done_rx.recv_timeout(timeout).is_ok();
                let elapsed = join_start.elapsed();
                log::info!(
                    "transcript_event_writer.shutdown_join elapsed_ms={} timeout_ms={} joined={}",
                    elapsed.as_millis(),
                    timeout.as_millis(),
                    joined
                );
                joined
            }
            Err(e) => {
                log::warn!(
                    "Failed to spawn transcript-event-writer-join watchdog: {} — \
                     writer thread is detached",
                    e
                );
                false
            }
        }
    }
}

/// Handle to the durable projection patch event-log writer thread.
pub struct ProjectionEventWriter {
    tx: mpsc::SyncSender<ProjectionEventWriteMsg>,
    handle: Option<std::thread::JoinHandle<()>>,
    shutdown_requested: Arc<AtomicBool>,
    queue_capacity: usize,
    dropped_event_count: Arc<AtomicU64>,
    queue_full_active: Arc<AtomicBool>,
}

impl ProjectionEventWriter {
    /// Spawn a new projection event writer thread for the given session.
    pub fn spawn(session_id: &str) -> Option<Self> {
        let file_path = projection_events_path(session_id)?;
        if let Some(parent) = file_path.parent()
            && let Err(e) = ensure_dir(parent)
        {
            log::warn!("Projection event persistence disabled: {}", e);
            return None;
        }

        let (tx, rx) = mpsc::sync_channel::<ProjectionEventWriteMsg>(EVENT_WRITER_QUEUE_CAPACITY);
        let shutdown_requested = Arc::new(AtomicBool::new(false));
        let shutdown_flag = shutdown_requested.clone();
        let thread_path = file_path.clone();

        let handle = std::thread::Builder::new()
            .name("projection-event-writer".to_string())
            .spawn(move || {
                let file = match fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&thread_path)
                {
                    Ok(f) => f,
                    Err(e) => {
                        io::handle_write_error(app_handle(), &thread_path, 0, 0, &e);
                        return;
                    }
                };
                crate::fs_util::set_owner_only(&thread_path);
                let mut writer = BufWriter::new(file);

                'outer: loop {
                    match rx.recv_timeout(POLL_INTERVAL) {
                        Ok(ProjectionEventWriteMsg::Append(patch)) => {
                            write_projection_event(&mut writer, &patch, &thread_path);
                            if shutdown_flag.load(Ordering::SeqCst) {
                                drain_remaining_projection_events(&rx, &mut writer, &thread_path);
                                break 'outer;
                            }
                        }
                        Ok(ProjectionEventWriteMsg::Shutdown) => break 'outer,
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            if shutdown_flag.load(Ordering::SeqCst) {
                                drain_remaining_projection_events(&rx, &mut writer, &thread_path);
                                break 'outer;
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break 'outer,
                    }
                }

                let flush_start = std::time::Instant::now();
                let flush_result = writer.flush();
                let flush_elapsed = flush_start.elapsed();
                if let Err(e) = flush_result {
                    io::handle_write_error(app_handle(), &thread_path, 0, 0, &e);
                    log::info!(
                        "projection_event_writer.final_flush file={:?} elapsed_ms={} outcome=error",
                        thread_path,
                        flush_elapsed.as_millis()
                    );
                } else {
                    log::info!(
                        "projection_event_writer.final_flush file={:?} elapsed_ms={} outcome=ok",
                        thread_path,
                        flush_elapsed.as_millis()
                    );
                }
                log::info!("Projection event writer: shut down for {:?}", thread_path);
            })
            .ok()?;

        Some(Self {
            tx,
            handle: Some(handle),
            shutdown_requested,
            queue_capacity: EVENT_WRITER_QUEUE_CAPACITY,
            dropped_event_count: Arc::new(AtomicU64::new(0)),
            queue_full_active: Arc::new(AtomicBool::new(false)),
        })
    }

    #[cfg(test)]
    pub(crate) fn saturated_for_tests(patch: ProjectionPatch) -> Self {
        let (tx, rx) = mpsc::sync_channel::<ProjectionEventWriteMsg>(1);
        tx.try_send(ProjectionEventWriteMsg::Append(patch))
            .expect("pre-fill projection event queue");
        std::mem::forget(rx);
        Self {
            tx,
            handle: None,
            shutdown_requested: Arc::new(AtomicBool::new(false)),
            queue_capacity: 1,
            dropped_event_count: Arc::new(AtomicU64::new(0)),
            queue_full_active: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Build a repository-backed projection event writer.
    ///
    /// The default application path still calls [`Self::spawn`] and uses the
    /// bounded file writer with disk-full diagnostics.
    pub fn repository(
        session_id: impl Into<String>,
        repository: Arc<dyn LocalMemoryRepository>,
    ) -> Option<Self> {
        let session_id = session_id.into();
        if let Err(e) = crate::sessions::validate_session_id(&session_id) {
            log::warn!("Projection event repository writer disabled: {e}");
            return None;
        }

        let (tx, rx) = mpsc::sync_channel::<ProjectionEventWriteMsg>(EVENT_WRITER_QUEUE_CAPACITY);
        let shutdown_requested = Arc::new(AtomicBool::new(false));
        let shutdown_flag = shutdown_requested.clone();
        let thread_session_id = session_id.clone();
        let thread_repository = repository.clone();

        let handle = std::thread::Builder::new()
            .name("projection-event-repository-writer".to_string())
            .spawn(move || {
                'outer: loop {
                    match rx.recv_timeout(POLL_INTERVAL) {
                        Ok(ProjectionEventWriteMsg::Append(patch)) => {
                            if let Err(e) =
                                thread_repository.append_projection_patch(&thread_session_id, &patch)
                            {
                                log::warn!(
                                    "Projection event repository writer: failed to append patch: {e}"
                                );
                            }
                            if shutdown_flag.load(Ordering::SeqCst) {
                                drain_remaining_repository_projection_events(
                                    &rx,
                                    &thread_session_id,
                                    thread_repository.as_ref(),
                                );
                                break 'outer;
                            }
                        }
                        Ok(ProjectionEventWriteMsg::Shutdown) => break 'outer,
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            if shutdown_flag.load(Ordering::SeqCst) {
                                drain_remaining_repository_projection_events(
                                    &rx,
                                    &thread_session_id,
                                    thread_repository.as_ref(),
                                );
                                break 'outer;
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break 'outer,
                    }
                }
                log::info!(
                    "Projection event repository writer: shut down for {}",
                    thread_session_id
                );
            })
            .ok()?;

        Some(Self {
            tx,
            handle: Some(handle),
            shutdown_requested,
            queue_capacity: EVENT_WRITER_QUEUE_CAPACITY,
            dropped_event_count: Arc::new(AtomicU64::new(0)),
            queue_full_active: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Enqueue a projection patch for writing. Non-blocking.
    pub fn append(&self, patch: &ProjectionPatch) -> bool {
        match self
            .tx
            .try_send(ProjectionEventWriteMsg::Append(patch.clone()))
        {
            Ok(()) => {
                note_event_writer_enqueue_success(
                    "projection_event",
                    self.queue_capacity,
                    &self.dropped_event_count,
                    &self.queue_full_active,
                );
                true
            }
            Err(mpsc::TrySendError::Full(_)) => {
                note_event_writer_queue_full(
                    "projection_event",
                    self.queue_capacity,
                    &self.dropped_event_count,
                    &self.queue_full_active,
                );
                false
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                note_event_writer_disconnected("projection_event");
                false
            }
        }
    }

    pub fn shutdown(&self) {
        self.shutdown_requested.store(true, Ordering::SeqCst);
        let _ = self.tx.try_send(ProjectionEventWriteMsg::Shutdown);
    }

    pub fn shutdown_with_timeout(mut self, timeout: std::time::Duration) -> bool {
        self.shutdown_requested.store(true, Ordering::SeqCst);
        let _ = self.tx.try_send(ProjectionEventWriteMsg::Shutdown);
        let Some(handle) = self.handle.take() else {
            return true;
        };
        let (done_tx, done_rx) = mpsc::channel::<()>();
        let spawned = std::thread::Builder::new()
            .name("projection-event-writer-join".to_string())
            .spawn(move || {
                let _ = handle.join();
                let _ = done_tx.send(());
            });
        match spawned {
            Ok(_watchdog) => {
                let join_start = std::time::Instant::now();
                let joined = done_rx.recv_timeout(timeout).is_ok();
                let elapsed = join_start.elapsed();
                log::info!(
                    "projection_event_writer.shutdown_join elapsed_ms={} timeout_ms={} joined={}",
                    elapsed.as_millis(),
                    timeout.as_millis(),
                    joined
                );
                joined
            }
            Err(e) => {
                log::warn!(
                    "Failed to spawn projection-event-writer-join watchdog: {} — \
                     writer thread is detached",
                    e
                );
                false
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Knowledge graph persistence
// ---------------------------------------------------------------------------

/// Save a serializable value as pretty-printed JSON to a file.
///
/// Uses an atomic write (tmp file + rename) so a partial write never replaces
/// a known-good file. I/O errors are classified via `io::handle_write_error`
/// so ENOSPC on the tmp file emits `CAPTURE_STORAGE_FULL` to the UI; other
/// errors fall through to the legacy string-return path.
pub fn save_json<T: serde::Serialize>(value: &T, path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        ensure_dir(parent)?;
    }

    // Atomic write: write to temp file, then rename
    let tmp_path = path.with_extension("json.tmp");
    let file = match fs::File::create(&tmp_path) {
        Ok(f) => f,
        Err(e) => {
            io::handle_write_error(app_handle(), &tmp_path, 0, 0, &e);
            return Err(format!("Failed to create temp file {:?}: {}", tmp_path, e));
        }
    };
    let mut writer = BufWriter::new(file);
    if let Err(e) = serde_json::to_writer_pretty(&mut writer, value) {
        // `serde_json::Error::classify() == Category::Io` indicates the
        // underlying writer failed — surface storage-full conditions via
        // the shared handler before returning.
        if e.classify() == serde_json::error::Category::Io {
            let io_err = std::io::Error::from(e);
            io::handle_write_error(app_handle(), &tmp_path, 0, 0, &io_err);
            return Err(format!("Failed to serialize to {:?}: {}", tmp_path, io_err));
        }
        return Err(format!("Failed to serialize to {:?}: {}", tmp_path, e));
    }
    if let Err(e) = writer.flush() {
        io::handle_write_error(app_handle(), &tmp_path, 0, 0, &e);
        return Err(format!("Failed to flush {:?}: {}", tmp_path, e));
    }
    // Recover the underlying File and fsync it BEFORE the rename. `flush()`
    // only pushes the BufWriter's buffer into the OS page cache; without an
    // explicit `sync_all`, a crash after the rename commits but before the
    // data blocks reach stable storage can leave a zero-length file replacing
    // the previous known-good snapshot. into_inner() also flushes, so this
    // both surfaces a late buffer-flush error and hands us the File to sync.
    let file = match writer.into_inner() {
        Ok(f) => f,
        Err(e) => {
            let io_err = e.into_error();
            io::handle_write_error(app_handle(), &tmp_path, 0, 0, &io_err);
            return Err(format!("Failed to flush {:?}: {}", tmp_path, io_err));
        }
    };
    if let Err(e) = file.sync_all() {
        io::handle_write_error(app_handle(), &tmp_path, 0, 0, &e);
        return Err(format!("Failed to fsync {:?}: {}", tmp_path, e));
    }
    drop(file);

    // Lock down perms on the tmp file before rename. Graph JSON can contain
    // excerpts of transcribed speech that should not be world-readable.
    crate::fs_util::set_owner_only(&tmp_path);

    fs::rename(&tmp_path, path)
        .map_err(|e| format!("Failed to rename {:?} → {:?}: {}", tmp_path, path, e))?;

    // Re-apply after rename in case rename semantics differ across platforms.
    crate::fs_util::set_owner_only(path);

    Ok(())
}

/// Persist the current materialized notes artifact for a session.
pub fn save_materialized_notes(session_id: &str, notes: &MaterializedNotes) -> Result<(), String> {
    let path = notes_path(session_id).ok_or_else(|| {
        "Materialized notes persistence disabled: could not resolve notes path".to_string()
    })?;
    save_json(notes, &path)
}

/// Persist the current materialized projection graph artifact for a session.
pub fn save_materialized_graph(session_id: &str, graph: &MaterializedGraph) -> Result<(), String> {
    let path = materialized_graph_path(session_id).ok_or_else(|| {
        "Materialized graph persistence disabled: could not resolve graph path".to_string()
    })?;
    save_json(graph, &path)
}

/// Load JSONL rows from a file. Missing files are treated as empty logs so
/// session restore can handle sessions created before a specific artifact
/// existed.
pub fn load_jsonl<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Vec<T>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    let file = fs::File::open(path).map_err(|e| format!("Failed to open {:?}: {}", path, e))?;
    let reader = BufReader::new(file);
    let mut rows = Vec::new();
    for (line_number, line) in reader.lines().enumerate() {
        let line = line.map_err(|e| {
            format!(
                "Failed to read line {} from {:?}: {}",
                line_number + 1,
                path,
                e
            )
        })?;
        if line.trim().is_empty() {
            continue;
        }
        rows.push(serde_json::from_str(&line).map_err(|e| {
            format!(
                "Failed to deserialize line {} from {:?}: {}",
                line_number + 1,
                path,
                e
            )
        })?);
    }
    Ok(rows)
}

/// Load immutable transcript span revision events for a session.
pub fn load_transcript_events(session_id: &str) -> Result<Vec<TranscriptEvent>, String> {
    let path = transcript_events_path(session_id).ok_or_else(|| {
        "Transcript event persistence disabled: could not resolve transcript event path".to_string()
    })?;
    load_jsonl(&path)
}

/// Load the transcript segment view for a session, preferring the immutable
/// event log.
///
/// Resolution order (read-only; never migrates or mutates either file):
///
/// 1. If the session's `<session>.events.jsonl` event log exists and is
///    non-empty, replay it through [`TranscriptLedger::replay`] and derive the
///    canonical, duplicate-free legacy segment view via
///    [`derive_legacy_transcript_segments`](crate::projections::derive_legacy_transcript_segments).
///    Superseding partial revisions collapse to one segment per final span.
/// 2. Otherwise fall back to the legacy `<session>.jsonl` rows exactly as they
///    were written — each line is a serialized [`TranscriptSegment`] and is
///    returned unchanged, with no migration into the event log.
///
/// An empty event log (header-only or zero lines) is treated as "no event
/// log" so a session that only ever wrote legacy rows still loads them.
pub fn load_transcript_segments_preferring_ledger(
    session_id: &str,
) -> Result<Vec<TranscriptSegment>, String> {
    let events = load_transcript_events(session_id)?;
    if !events.is_empty() {
        let ledger = TranscriptLedger::replay(session_id, events)
            .map_err(|error| format!("Transcript replay failed for {session_id}: {error:?}"))?;
        return Ok(crate::projections::derive_legacy_transcript_segments(
            &ledger,
        ));
    }

    let legacy_path = crate::user_data::transcript_path(session_id)?;
    load_jsonl::<TranscriptSegment>(&legacy_path)
}

/// Load replayable projection patch events for a session.
pub fn load_projection_events(session_id: &str) -> Result<Vec<ProjectionPatch>, String> {
    let path = projection_events_path(session_id).ok_or_else(|| {
        "Projection event persistence disabled: could not resolve projection event path".to_string()
    })?;
    load_jsonl(&path)
}

/// Load immutable diarization span revision events for a session.
pub fn load_diarization_span_revisions(
    session_id: &str,
) -> Result<Vec<DiarizationSpanRevision>, String> {
    let path = diarization_events_path(session_id).ok_or_else(|| {
        "Diarization event persistence disabled: could not resolve diarization event path"
            .to_string()
    })?;
    load_jsonl(&path)
}

/// Load the session's data-movement ledger events in append order
/// (seed audio-graph-70a3).
pub fn load_data_movement_events(session_id: &str) -> Result<Vec<DataMovementEvent>, String> {
    let path = data_movement_ledger_path(session_id).ok_or_else(|| {
        "Data movement ledger persistence disabled: could not resolve ledger path".to_string()
    })?;
    load_jsonl(&path)
}

/// Load a materialized notes artifact, if one exists for the session.
pub fn load_materialized_notes(session_id: &str) -> Result<Option<MaterializedNotes>, String> {
    let path = notes_path(session_id).ok_or_else(|| {
        "Materialized notes persistence disabled: could not resolve notes path".to_string()
    })?;
    if !path.exists() {
        return Ok(None);
    }
    load_json(&path).map(Some)
}

/// Load a materialized projection graph artifact, if one exists for the session.
pub fn load_materialized_graph(session_id: &str) -> Result<Option<MaterializedGraph>, String> {
    let path = materialized_graph_path(session_id).ok_or_else(|| {
        "Materialized graph persistence disabled: could not resolve graph path".to_string()
    })?;
    if !path.exists() {
        return Ok(None);
    }
    load_json(&path).map(Some)
}

/// Load a deserializable value from a JSON file.
pub fn load_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let data = fs::read_to_string(path).map_err(|e| format!("Failed to read {:?}: {}", path, e))?;
    serde_json::from_str(&data).map_err(|e| format!("Failed to deserialize {:?}: {}", path, e))
}

// ---------------------------------------------------------------------------
// Graph auto-save timer
// ---------------------------------------------------------------------------

use crate::graph::temporal::TemporalKnowledgeGraph;
use std::collections::{HashSet, VecDeque};
use std::sync::{Mutex, RwLock};

/// How often the autosave loop wakes to poll its stop flag. The save cadence
/// stays at [`AUTOSAVE_INTERVAL`] (a save fires only once that much wall-time
/// has elapsed since the last one); this shorter poll just lets a stop request
/// at quit be observed within [`AUTOSAVE_POLL_INTERVAL`] instead of blocking on
/// a full 30s `sleep`, so the Exit handler's `signal + join` stays bounded.
const AUTOSAVE_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

/// The graph auto-save cadence: a save tick fires at most this often.
const AUTOSAVE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// Run a single graph-autosave tick: snapshot the current session, save the
/// knowledge graph to disk (when non-empty), and refresh the session index
/// stats. Shared by the periodic autosave loop and the graceful-shutdown final
/// save so both produce identical on-disk state.
///
/// Snapshots `session_id` ONCE so the file path and the `update_stats` call
/// target the same session even if a rotation lands between sub-steps. The
/// caller is responsible for skipping the tick while `rotation_in_progress`.
fn run_autosave_tick(
    dir: &Path,
    session_id: &Arc<RwLock<String>>,
    knowledge_graph: &Arc<Mutex<TemporalKnowledgeGraph>>,
    transcript_buffer: &Arc<RwLock<VecDeque<TranscriptSegment>>>,
) {
    // Snapshot session_id ONCE at tick entry. Every subsequent write in this
    // tick uses `current_sid` — never re-reads `session_id` — so the file path
    // and the stats update are guaranteed to target the same session even if a
    // rotation lands between sub-steps. Poisoned lock → recover; the inner
    // String has no broken invariant.
    let current_sid = match session_id.read() {
        Ok(g) => g.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    };
    let file_path = dir.join(format!("{}.json", current_sid));

    // ── Graph snapshot: save to disk + capture entity count ────────
    let entity_count: u64 = {
        let graph = match knowledge_graph.lock() {
            Ok(g) => g,
            Err(e) => {
                log::warn!("Graph auto-save: lock poisoned, recovering: {}", e);
                e.into_inner()
            }
        };

        let node_count = graph.node_count();
        if node_count > 0
            && let Err(e) = graph.save_to_file(&file_path)
        {
            log::warn!("Graph auto-save: failed: {}", e);
        }
        node_count as u64
    };

    // ── Transcript buffer: segment + unique speaker counts ─────────
    let (segment_count, speaker_count): (u64, u64) = match transcript_buffer.read() {
        Ok(buf) => {
            let segments = buf.len() as u64;
            let speakers: HashSet<&str> =
                buf.iter().filter_map(|s| s.speaker_id.as_deref()).collect();
            (segments, speakers.len() as u64)
        }
        Err(e) => {
            log::warn!("Graph auto-save: transcript buffer lock poisoned: {}", e);
            let buf = e.into_inner();
            let segments = buf.len() as u64;
            let speakers: HashSet<&str> =
                buf.iter().filter_map(|s| s.speaker_id.as_deref()).collect();
            (segments, speakers.len() as u64)
        }
    };

    // ── Refresh session index stats ────────────────────────────────
    // Pass the tick-start-cached `current_sid`, NOT a fresh read of
    // session_id, so the stats update matches the file we just wrote above.
    if let Err(e) =
        crate::sessions::update_stats(&current_sid, segment_count, speaker_count, entity_count)
    {
        log::warn!("Graph auto-save: session stats update failed: {}", e);
    }
}

/// Run one final autosave tick synchronously at graceful shutdown.
///
/// Called from the `RunEvent::Exit` handler AFTER the autosave thread has been
/// signalled to stop and joined, so there is no concurrent writer for the same
/// session file. Best-effort and bounded: it does a single graph save + stats
/// refresh (the same work as one loop tick) so a clean File→Quit doesn't lose
/// up to ~30s of derived-graph state to the next-missed autosave tick.
///
/// A no-op (returns without touching disk) if the graphs directory cannot be
/// resolved — mirrors [`spawn_graph_autosave`] returning `None`.
pub fn autosave_final_save(
    session_id: &Arc<RwLock<String>>,
    knowledge_graph: &Arc<Mutex<TemporalKnowledgeGraph>>,
    transcript_buffer: &Arc<RwLock<VecDeque<TranscriptSegment>>>,
) {
    let Some(dir) = graphs_dir() else {
        log::warn!("Graph auto-save: final save skipped (graphs dir unavailable)");
        return;
    };
    if let Err(e) = ensure_dir(&dir) {
        log::warn!("Graph auto-save: final save skipped: {}", e);
        return;
    }
    run_autosave_tick(&dir, session_id, knowledge_graph, transcript_buffer);
    log::info!("Graph auto-save: final save on exit complete");
}

/// Spawn a background thread that auto-saves the knowledge graph every 30 seconds
/// and refreshes the session index stats (segment/speaker/entity counts).
///
/// `session_id` is shared via `Arc<RwLock<String>>` so
/// [`AppState::rotate_session`](crate::state::AppState::rotate_session) can
/// repoint the autosave target mid-run without respawning this thread. Each
/// tick snapshots the current ID once at entry and uses that single value for
/// both the file path *and* the `update_stats` call — so even if a rotation
/// lands mid-tick, the tick's writes all target the same session.
///
/// `rotation_in_progress` is the shared guard from `AppState`: if a rotation
/// is actively swapping the writer/session_id when the tick fires, we skip
/// this tick and wait for the next one rather than race the rotation.
///
/// `stop` is the shared shutdown flag from `AppState`: the loop polls it every
/// [`AUTOSAVE_POLL_INTERVAL`] and exits promptly when set, so the graceful-
/// shutdown path (`RunEvent::Exit`) can signal it and join this thread within a
/// bounded budget instead of waiting out a full 30s `sleep`. The Exit handler
/// performs the single final save itself via [`autosave_final_save`] AFTER this
/// thread has exited, so there is no double-writer race on the session file.
///
/// Returns the thread handle (or `None` if the graphs directory cannot be resolved).
pub fn spawn_graph_autosave(
    session_id: Arc<RwLock<String>>,
    knowledge_graph: Arc<Mutex<TemporalKnowledgeGraph>>,
    transcript_buffer: Arc<RwLock<VecDeque<TranscriptSegment>>>,
    rotation_in_progress: Arc<std::sync::atomic::AtomicBool>,
    stop: Arc<std::sync::atomic::AtomicBool>,
) -> Option<std::thread::JoinHandle<()>> {
    let dir = graphs_dir()?;
    if let Err(e) = ensure_dir(&dir) {
        log::warn!("Graph auto-save disabled: {}", e);
        return None;
    }

    let handle = std::thread::Builder::new()
        .name("graph-autosave".to_string())
        .spawn(move || {
            log::info!("Graph auto-save: started (every 30s → {:?})", dir);
            let mut last_save = std::time::Instant::now();
            loop {
                std::thread::sleep(AUTOSAVE_POLL_INTERVAL);

                // Graceful-shutdown stop signal. Exit WITHOUT saving here — the
                // Exit handler owns the single final save (via
                // `autosave_final_save`) once this thread has joined, so we
                // never race the final writer for the same session file.
                if stop.load(std::sync::atomic::Ordering::SeqCst) {
                    log::info!("Graph auto-save: stop signalled, exiting");
                    break;
                }

                // Only save once the full cadence has elapsed; the shorter poll
                // above exists solely for stop-signal responsiveness.
                if last_save.elapsed() < AUTOSAVE_INTERVAL {
                    continue;
                }
                last_save = std::time::Instant::now();

                // If a rotation is mid-flight, skip this tick. The in-flight
                // rotation will land soon; the next tick will observe the new
                // session ID atomically. Avoids the window where we could write
                // graph state to the old session file concurrently with the
                // writer-respawn for the new one.
                if rotation_in_progress.load(std::sync::atomic::Ordering::SeqCst) {
                    log::debug!("Graph auto-save: skipping tick (rotation in progress)");
                    continue;
                }

                run_autosave_tick(&dir, &session_id, &knowledge_graph, &transcript_buffer);
            }
        })
        .ok()?;

    Some(handle)
}

// ---------------------------------------------------------------------------
// Tests — transcript writer shutdown contract (ag#7)
// ---------------------------------------------------------------------------
//
// These pin the behavior that matters for session rotation:
//   - `shutdown()` sets the atomic flag before sending the sentinel, so a
//     writer mid-drain observes the flag on the next `recv_timeout` tick and
//     exits instead of flushing the whole backlog.
//   - `drain_remaining` stops at `Shutdown` without over-consuming the channel.
//
// We test `drain_remaining` directly against a synthetic BufWriter (over a
// `Vec<u8>`-backed temp file) rather than going through `TranscriptWriter::spawn`,
// which would require HOME override and conflict with `sessions::usage::tests`
// under parallel execution.

#[cfg(test)]
mod shutdown_tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_tempfile(label: &str) -> std::path::PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!(
            "audio-graph-shutdown-{}-{}-{}-{}",
            label, pid, nanos, n
        ));
        fs::create_dir_all(&dir).expect("create tempdir");
        dir.join("t.jsonl")
    }

    fn seg(id: &str, text: &str) -> TranscriptSegment {
        TranscriptSegment {
            id: id.into(),
            source_id: "test".into(),
            speaker_id: None,
            speaker_label: None,
            text: text.into(),
            start_time: 0.0,
            end_time: 1.0,
            confidence: 1.0,
        }
    }

    fn transcript_event(span_id: &str, text: &str) -> TranscriptEvent {
        TranscriptEvent {
            span_id: span_id.into(),
            provider: "test".into(),
            source_id: "test-source".into(),
            provider_item_id: None,
            transcript_segment_id: Some(format!("segment-{span_id}")),
            speaker_id: Some("speaker-1".into()),
            speaker_label: Some("Speaker 1".into()),
            channel: None,
            text: text.into(),
            start_time: 0.0,
            end_time: 1.0,
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
            received_at_ms: 1_700_000_000_000,
        }
    }

    fn projection_patch(sequence: u64, note_id: &str) -> ProjectionPatch {
        ProjectionPatch {
            sequence,
            kind: crate::projections::ProjectionKind::Notes,
            llm_request_id: format!("llm-request-{sequence}"),
            basis: crate::projections::ProjectionBasis {
                span_revisions: vec![crate::projections::ProjectionBasisSpan {
                    span_id: "span-1".into(),
                    revision_number: sequence,
                }],
                diarization_span_revisions: Vec::new(),
                transcript_hash: format!("fnv1a64:{sequence:016x}"),
            },
            operations: vec![crate::projections::ProjectionOperation::UpsertNote {
                id: note_id.into(),
                title: "Decision".into(),
                body: "Persist projection patches.".into(),
                tags: vec!["decision".into()],
            }],
            confidence: 0.9,
            provenance: crate::projections::ProjectionProvenance {
                provider: "openrouter".into(),
                model: "anthropic/claude-sonnet-4".into(),
                prompt_id: "notes-v1".into(),
            },
            queued_at_ms: None,
            generation_latency_ms: None,
            apply_latency_ms: None,
            created_at_ms: 1_700_000_000_000 + sequence,
        }
    }

    #[test]
    fn drain_remaining_writes_pending_appends_then_stops() {
        // Simulates the writer hitting the shutdown flag mid-queue: the helper
        // must persist everything already in the channel so a caller-observed
        // shutdown doesn't silently drop buffered segments.
        let path = unique_tempfile("drain-pending");
        let file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .expect("open temp file");
        let mut writer = BufWriter::new(file);

        let (tx, rx) = mpsc::channel::<TranscriptWriteMsg>();
        tx.send(TranscriptWriteMsg::Append(seg("a", "first")))
            .unwrap();
        tx.send(TranscriptWriteMsg::Append(seg("b", "second")))
            .unwrap();
        // Shutdown sentinel mid-queue — drain_remaining must stop here.
        tx.send(TranscriptWriteMsg::Shutdown).unwrap();
        // This one must NOT be written — it comes after the sentinel.
        tx.send(TranscriptWriteMsg::Append(seg("c", "after-sentinel")))
            .unwrap();

        drain_remaining(&rx, &mut writer, &path);
        writer.flush().unwrap();

        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains("first"), "first segment must be written");
        assert!(
            contents.contains("second"),
            "second segment must be written"
        );
        assert!(
            !contents.contains("after-sentinel"),
            "drain must stop at Shutdown sentinel, got: {:?}",
            contents
        );

        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn drain_remaining_events_writes_pending_appends_then_stops() {
        let path = unique_tempfile("drain-event-pending");
        let file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .expect("open temp file");
        let mut writer = BufWriter::new(file);

        let (tx, rx) = mpsc::channel::<TranscriptEventWriteMsg>();
        tx.send(TranscriptEventWriteMsg::Append(transcript_event(
            "a", "first",
        )))
        .unwrap();
        tx.send(TranscriptEventWriteMsg::Append(transcript_event(
            "b", "second",
        )))
        .unwrap();
        tx.send(TranscriptEventWriteMsg::Shutdown).unwrap();
        tx.send(TranscriptEventWriteMsg::Append(transcript_event(
            "c",
            "after-sentinel",
        )))
        .unwrap();

        drain_remaining_events(&rx, &mut writer, &path);
        writer.flush().unwrap();

        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains("\"span_id\":\"a\""));
        assert!(contents.contains("\"span_id\":\"b\""));
        assert!(
            !contents.contains("after-sentinel"),
            "event drain must stop at Shutdown sentinel, got: {:?}",
            contents
        );

        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn drain_remaining_projection_events_writes_pending_patches_then_stops() {
        let path = unique_tempfile("drain-projection-pending");
        let file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .expect("open temp file");
        let mut writer = BufWriter::new(file);

        let (tx, rx) = mpsc::channel::<ProjectionEventWriteMsg>();
        tx.send(ProjectionEventWriteMsg::Append(projection_patch(
            1, "note-a",
        )))
        .unwrap();
        tx.send(ProjectionEventWriteMsg::Append(projection_patch(
            2, "note-b",
        )))
        .unwrap();
        tx.send(ProjectionEventWriteMsg::Shutdown).unwrap();
        tx.send(ProjectionEventWriteMsg::Append(projection_patch(
            3,
            "note-after-sentinel",
        )))
        .unwrap();

        drain_remaining_projection_events(&rx, &mut writer, &path);
        writer.flush().unwrap();

        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains("\"sequence\":1"));
        assert!(contents.contains("\"sequence\":2"));
        assert!(
            !contents.contains("note-after-sentinel"),
            "projection drain must stop at Shutdown sentinel, got: {:?}",
            contents
        );

        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn drain_remaining_handles_empty_and_disconnected_channel() {
        // Boundary cases: an empty channel, and a channel whose sender is
        // already dropped. Neither should panic or block; both should simply
        // return with whatever BufWriter state the caller passed in.
        let path = unique_tempfile("drain-empty");
        let file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .expect("open temp file");
        let mut writer = BufWriter::new(file);

        // Empty, still-open channel.
        let (tx, rx) = mpsc::channel::<TranscriptWriteMsg>();
        drain_remaining(&rx, &mut writer, &path);
        drop(tx);

        // Disconnected channel.
        let (tx2, rx2) = mpsc::channel::<TranscriptWriteMsg>();
        drop(tx2);
        drain_remaining(&rx2, &mut writer, &path);

        writer.flush().unwrap();
        let contents = fs::read_to_string(&path).unwrap();
        assert!(
            contents.is_empty(),
            "no segments should be written, got: {:?}",
            contents
        );

        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn shutdown_sets_flag_before_sending_sentinel() {
        // Contract: `shutdown()` must flip `shutdown_requested` *before* the
        // sentinel lands in the channel, so a writer polling the flag on
        // recv_timeout observes shutdown even if it hasn't consumed the
        // sentinel yet. We stand up a fake TranscriptWriter (no real thread)
        // and assert flag state after the call — the send itself is covered
        // by the end-to-end `#[ignore]`d rotation tests in state.rs.
        let (tx, _rx) = mpsc::channel::<TranscriptWriteMsg>();
        let flag = Arc::new(AtomicBool::new(false));
        let writer = TranscriptWriter {
            tx,
            handle: None,
            shutdown_requested: flag.clone(),
        };
        assert!(!flag.load(Ordering::SeqCst));
        writer.shutdown();
        assert!(
            flag.load(Ordering::SeqCst),
            "shutdown() must set the shutdown_requested flag"
        );
    }

    #[test]
    fn event_shutdown_sets_flag_before_sending_sentinel() {
        let (tx, _rx) = mpsc::sync_channel::<TranscriptEventWriteMsg>(1);
        let flag = Arc::new(AtomicBool::new(false));
        let writer = TranscriptEventWriter {
            tx,
            handle: None,
            shutdown_requested: flag.clone(),
            queue_capacity: 1,
            dropped_event_count: Arc::new(AtomicU64::new(0)),
            queue_full_active: Arc::new(AtomicBool::new(false)),
        };
        assert!(!flag.load(Ordering::SeqCst));
        writer.shutdown();
        assert!(
            flag.load(Ordering::SeqCst),
            "event shutdown() must set the shutdown_requested flag"
        );
    }

    #[test]
    fn projection_event_shutdown_sets_flag_before_sending_sentinel() {
        let (tx, _rx) = mpsc::sync_channel::<ProjectionEventWriteMsg>(1);
        let flag = Arc::new(AtomicBool::new(false));
        let writer = ProjectionEventWriter {
            tx,
            handle: None,
            shutdown_requested: flag.clone(),
            queue_capacity: 1,
            dropped_event_count: Arc::new(AtomicU64::new(0)),
            queue_full_active: Arc::new(AtomicBool::new(false)),
        };
        assert!(!flag.load(Ordering::SeqCst));
        writer.shutdown();
        assert!(
            flag.load(Ordering::SeqCst),
            "projection event shutdown() must set the shutdown_requested flag"
        );
    }

    #[test]
    fn event_append_reports_full_bounded_queue_without_blocking() {
        let (tx, _rx) = mpsc::sync_channel::<TranscriptEventWriteMsg>(1);
        let dropped_event_count = Arc::new(AtomicU64::new(0));
        let queue_full_active = Arc::new(AtomicBool::new(false));
        let writer = TranscriptEventWriter {
            tx,
            handle: None,
            shutdown_requested: Arc::new(AtomicBool::new(false)),
            queue_capacity: 1,
            dropped_event_count: dropped_event_count.clone(),
            queue_full_active: queue_full_active.clone(),
        };

        assert!(writer.append(&transcript_event("queued", "first")));
        assert!(!writer.append(&transcript_event("dropped", "second")));
        assert_eq!(dropped_event_count.load(Ordering::SeqCst), 1);
        assert!(queue_full_active.load(Ordering::SeqCst));
    }

    #[test]
    fn projection_event_append_reports_full_bounded_queue_without_blocking() {
        let (tx, _rx) = mpsc::sync_channel::<ProjectionEventWriteMsg>(1);
        let dropped_event_count = Arc::new(AtomicU64::new(0));
        let queue_full_active = Arc::new(AtomicBool::new(false));
        let writer = ProjectionEventWriter {
            tx,
            handle: None,
            shutdown_requested: Arc::new(AtomicBool::new(false)),
            queue_capacity: 1,
            dropped_event_count: dropped_event_count.clone(),
            queue_full_active: queue_full_active.clone(),
        };

        assert!(writer.append(&projection_patch(1, "queued-note")));
        assert!(!writer.append(&projection_patch(2, "dropped-note")));
        assert_eq!(dropped_event_count.load(Ordering::SeqCst), 1);
        assert!(queue_full_active.load(Ordering::SeqCst));
    }

    #[test]
    fn event_shutdown_does_not_block_when_bounded_queue_is_full() {
        let (tx, _rx) = mpsc::sync_channel::<TranscriptEventWriteMsg>(1);
        tx.try_send(TranscriptEventWriteMsg::Append(transcript_event(
            "queued", "first",
        )))
        .expect("pre-fill bounded queue");
        let flag = Arc::new(AtomicBool::new(false));
        let writer = TranscriptEventWriter {
            tx,
            handle: None,
            shutdown_requested: flag.clone(),
            queue_capacity: 1,
            dropped_event_count: Arc::new(AtomicU64::new(0)),
            queue_full_active: Arc::new(AtomicBool::new(false)),
        };

        let started = std::time::Instant::now();
        writer.shutdown();

        assert!(flag.load(Ordering::SeqCst));
        assert!(
            started.elapsed() < std::time::Duration::from_millis(100),
            "shutdown must not block behind a full queue"
        );
    }
}

#[cfg(test)]
mod local_memory_repository_tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicU64, Ordering};

    use serde_json::json;

    use crate::events::{
        AgentActionResult, AgentProposalKind, AgentProposalPayload, LiveAssistCardRecord,
        LiveAssistCardStatus,
    };
    use crate::projections::{
        HistoricalProjectionValidationError, ProjectionBasisStaleness, ProjectionKind,
        ProjectionOperation, ProjectionProvenance, TranscriptLedgerError,
    };
    use crate::promotion::{
        AclInheritanceMode, AclVisibility, ApprovedOrgPayload, DeleteBehavior, OrgKnowledgeKind,
        PROMOTION_SCHEMA_VERSION, PromotionAcl, PromotionActor, PromotionConflictState,
        PromotionDraft, PromotionLineage, PromotionRedactionSummary, PromotionRetention,
        PromotionRevocationRequest, PromotionSourceObjectType, PromotionSourceProvenance,
        PromotionSourceReference, PromotionStatus, PromotionSyncSnapshot, PromotionSyncTargetKind,
        PromotionTarget, RedactionDiffEntry, RetentionCategory,
    };

    fn unique_tempdir(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!(
            "audio-graph-memory-repo-{}-{}-{}-{}",
            label, pid, nanos, n
        ));
        fs::create_dir_all(&dir).expect("create tempdir");
        dir
    }

    fn transcript_event(
        span_id: &str,
        revision_number: u64,
        text: &str,
        received_at_ms: u64,
    ) -> TranscriptEvent {
        TranscriptEvent {
            span_id: span_id.into(),
            provider: "fixture-asr".into(),
            source_id: "source-1".into(),
            provider_item_id: Some(format!("provider-{span_id}-{revision_number}")),
            transcript_segment_id: Some(format!("segment-{span_id}")),
            speaker_id: Some("speaker-1".into()),
            speaker_label: Some("Speaker 1".into()),
            channel: None,
            text: text.into(),
            start_time: revision_number as f64,
            end_time: revision_number as f64 + 0.75,
            confidence: 0.94,
            is_final: true,
            stability: crate::projections::TranscriptEventStability::Final,
            revision_number,
            supersedes: None,
            turn_id: Some(format!("turn-{revision_number}")),
            end_of_turn: true,
            raw_event_ref: None,
            capture_latency_ms: Some(10),
            asr_latency_ms: Some(80),
            received_at_ms,
        }
    }

    fn note_patch(
        sequence: u64,
        basis: crate::projections::ProjectionBasis,
        created_at_ms: u64,
    ) -> ProjectionPatch {
        ProjectionPatch {
            sequence,
            kind: crate::projections::ProjectionKind::Notes,
            llm_request_id: format!("llm-notes-{sequence}"),
            basis,
            operations: vec![crate::projections::ProjectionOperation::UpsertNote {
                id: "note-1".into(),
                title: "Decision".into(),
                body: "Persist the memory repository boundary.".into(),
                tags: vec!["architecture".into()],
            }],
            confidence: 0.91,
            provenance: crate::projections::ProjectionProvenance {
                provider: "openrouter".into(),
                model: "model-a".into(),
                prompt_id: "notes-v1".into(),
            },
            queued_at_ms: Some(created_at_ms.saturating_sub(50)),
            generation_latency_ms: Some(120),
            apply_latency_ms: None,
            created_at_ms,
        }
    }

    fn graph_patch(
        sequence: u64,
        basis: crate::projections::ProjectionBasis,
        created_at_ms: u64,
    ) -> ProjectionPatch {
        ProjectionPatch {
            sequence,
            kind: crate::projections::ProjectionKind::Graph,
            llm_request_id: format!("llm-graph-{sequence}"),
            basis,
            operations: vec![crate::projections::ProjectionOperation::UpsertGraphNode {
                id: "node-repository".into(),
                name: "LocalMemoryRepository".into(),
                entity_type: "architecture_component".into(),
                description: Some("Backend-owned local memory boundary".into()),
            }],
            confidence: 0.88,
            provenance: crate::projections::ProjectionProvenance {
                provider: "openrouter".into(),
                model: "model-a".into(),
                prompt_id: "graph-v1".into(),
            },
            queued_at_ms: Some(created_at_ms.saturating_sub(50)),
            generation_latency_ms: Some(140),
            apply_latency_ms: None,
            created_at_ms,
        }
    }

    fn projection_patch(
        sequence: u64,
        kind: ProjectionKind,
        basis: crate::projections::ProjectionBasis,
        operations: Vec<ProjectionOperation>,
        created_at_ms: u64,
    ) -> ProjectionPatch {
        ProjectionPatch {
            sequence,
            kind,
            llm_request_id: format!("llm-conformance-{sequence}"),
            basis,
            operations,
            confidence: 0.9,
            provenance: ProjectionProvenance {
                provider: "test-provider".into(),
                model: "projection-conformance".into(),
                prompt_id: "repository-replay-parity".into(),
            },
            queued_at_ms: Some(created_at_ms.saturating_sub(25)),
            generation_latency_ms: Some(50),
            apply_latency_ms: Some(5),
            created_at_ms,
        }
    }

    pub(crate) fn assert_repository_replay_parity_conformance(
        repo: &dyn LocalMemoryRepository,
        session_id: &str,
    ) {
        let first_revision = transcript_event(
            "span-conformance-1",
            1,
            "Alice owns the old launch plan.",
            1_000,
        );
        let second_revision = transcript_event(
            "span-conformance-1",
            2,
            "Alice owns the revised launch plan.",
            2_000,
        );
        repo.append_transcript_event(session_id, &first_revision)
            .expect("append first revision");
        repo.append_transcript_event(session_id, &second_revision)
            .expect("append replacement revision");

        let ledger = repo
            .replay_transcript_ledger(session_id)
            .expect("replay replacement revisions");
        assert_eq!(ledger.accepted_event_count, 2);
        assert_eq!(ledger.latest_spans.len(), 1);
        assert_eq!(ledger.latest_spans[0].revision_number, 2);
        assert_eq!(
            ledger.latest_spans[0].text,
            "Alice owns the revised launch plan."
        );
        let basis = ledger.current_basis();

        let notes_seed = projection_patch(
            1,
            ProjectionKind::Notes,
            basis.clone(),
            vec![
                ProjectionOperation::UpsertNote {
                    id: "note-a".into(),
                    title: "Launch owner".into(),
                    body: "Alice owns the launch plan.".into(),
                    tags: vec!["launch".into()],
                },
                ProjectionOperation::UpsertNote {
                    id: "note-b".into(),
                    title: "Temporary note".into(),
                    body: "This should be removed by a later diff.".into(),
                    tags: vec!["temporary".into()],
                },
            ],
            3_000,
        );
        let notes_diff = projection_patch(
            2,
            ProjectionKind::Notes,
            basis.clone(),
            vec![
                ProjectionOperation::UpsertNote {
                    id: "note-a".into(),
                    title: "Launch owner".into(),
                    body: "Alice owns the revised launch plan.".into(),
                    tags: vec!["launch".into(), "revised".into()],
                },
                ProjectionOperation::UpsertNote {
                    id: "note-c".into(),
                    title: "Follow up".into(),
                    body: "Confirm launch readiness next week.".into(),
                    tags: vec!["follow-up".into()],
                },
                ProjectionOperation::DeleteNote {
                    id: "note-b".into(),
                },
                ProjectionOperation::ReorderNote {
                    id: "note-a".into(),
                    after_id: Some("note-c".into()),
                },
            ],
            4_000,
        );
        let graph_seed = projection_patch(
            1,
            ProjectionKind::Graph,
            basis.clone(),
            vec![
                ProjectionOperation::UpsertGraphNode {
                    id: "person-alice".into(),
                    name: "Alice".into(),
                    entity_type: "Person".into(),
                    description: Some("Launch owner".into()),
                },
                ProjectionOperation::UpsertGraphNode {
                    id: "project-launch".into(),
                    name: "Launch".into(),
                    entity_type: "Project".into(),
                    description: Some("Revised launch plan".into()),
                },
                ProjectionOperation::UpsertGraphEdge {
                    id: "edge-owns".into(),
                    source: "person-alice".into(),
                    target: "project-launch".into(),
                    relation_type: "owns".into(),
                    label: Some("owns launch".into()),
                    weight: 0.82,
                },
            ],
            5_000,
        );
        let graph_edge_invalidation = projection_patch(
            2,
            ProjectionKind::Graph,
            basis.clone(),
            vec![ProjectionOperation::InvalidateGraphEdge {
                id: "edge-owns".into(),
            }],
            6_000,
        );
        let graph_node_invalidation = projection_patch(
            3,
            ProjectionKind::Graph,
            basis.clone(),
            vec![ProjectionOperation::InvalidateGraphNode {
                id: "project-launch".into(),
            }],
            7_000,
        );

        for patch in [
            &notes_seed,
            &graph_seed,
            &notes_diff,
            &graph_edge_invalidation,
            &graph_node_invalidation,
        ] {
            repo.append_projection_patch(session_id, patch)
                .expect("append projection patch");
        }

        let replay = repo
            .replay_projection_state(session_id)
            .expect("replay projection state");
        assert_eq!(replay.validation.checked_patch_count, 5);
        assert_eq!(replay.validation.invalid_patch_count, 0);

        let notes = &replay.state.notes;
        assert_eq!(notes.last_sequence, 2);
        assert_eq!(
            notes
                .notes
                .iter()
                .map(|note| note.id.as_str())
                .collect::<Vec<_>>(),
            vec!["note-c", "note-a"]
        );
        assert_eq!(notes.notes[1].body, "Alice owns the revised launch plan.");
        assert!(notes.notes.iter().all(|note| note.id != "note-b"));

        let graph = &replay.state.graph;
        assert_eq!(graph.last_sequence, 3);
        let launch = graph
            .nodes
            .iter()
            .find(|node| node.id == "project-launch")
            .expect("launch node exists");
        assert_eq!(launch.valid_until_ms, Some(7_000));
        let owns = graph
            .edges
            .iter()
            .find(|edge| edge.id == "edge-owns")
            .expect("owns edge exists");
        assert_eq!(owns.valid_until_ms, Some(6_000));
        assert_eq!(
            graph
                .nodes
                .iter()
                .filter(|node| node.valid_until_ms.is_none())
                .map(|node| node.id.as_str())
                .collect::<Vec<_>>(),
            vec!["person-alice"]
        );
        assert!(
            graph.edges.iter().all(|edge| edge.valid_until_ms.is_some()),
            "edge invalidation should leave no active edges"
        );

        repo.save_materialized_notes(session_id, &replay.state.notes)
            .expect("save replayed notes");
        repo.save_materialized_graph(session_id, &replay.state.graph)
            .expect("save replayed graph");
        assert_eq!(
            repo.load_materialized_projection_state(session_id)
                .expect("load materialized projection state"),
            replay.state
        );

        let stale_session_id = format!("{session_id}-stale");
        let latest = transcript_event("span-stale", 2, "Latest transcript text.", 2_000);
        let stale = transcript_event("span-stale", 1, "Stale transcript text.", 3_000);
        repo.append_transcript_event(&stale_session_id, &latest)
            .expect("append latest stale-session revision");
        repo.append_transcript_event(&stale_session_id, &stale)
            .expect("append stale stale-session revision");
        let error = repo
            .replay_transcript_ledger(&stale_session_id)
            .expect_err("stale transcript replay must fail");
        assert!(
            error.contains("StaleTranscriptRevision")
                || error.contains(&format!(
                    "{:?}",
                    TranscriptLedgerError::StaleTranscriptRevision {
                        span_id: "span-stale".into(),
                        current_revision: 2,
                        incoming_revision: 1,
                    }
                )),
            "unexpected stale replay error: {error}"
        );
    }

    fn sample_agent_proposal(id: &str, kind: AgentProposalKind) -> AgentProposalPayload {
        AgentProposalPayload {
            id: id.into(),
            source_segment_id: "span-live-1".into(),
            source_id: "default-mic".into(),
            speaker_label: Some("Speaker 1".into()),
            kind,
            title: "Follow up on launch risk".into(),
            body: "Review this for an action item, decision, or relationship: launch risk".into(),
            confidence: 0.86,
            created_at_ms: 1_700_000_000_000,
        }
    }

    fn sample_live_assist_card(
        session_id: &str,
        id: &str,
        status: LiveAssistCardStatus,
    ) -> LiveAssistCardRecord {
        let projection_patch_sequence =
            matches!(status, LiveAssistCardStatus::Approved).then_some(3);
        let outcome = matches!(status, LiveAssistCardStatus::Approved).then(|| AgentActionResult {
            proposal_id: id.into(),
            action: "graph_update".into(),
            message: "Approved live assist card".into(),
            graph_updated: true,
            timestamp_ms: 1_700_000_000_100,
        });
        LiveAssistCardRecord {
            session_id: session_id.into(),
            proposal: sample_agent_proposal(id, AgentProposalKind::GraphSuggestion),
            status,
            source_span_ids: vec!["span-live-1".into()],
            graph_context_ids: Vec::new(),
            outcome,
            projection_patch_sequence,
            created_at_ms: 1_700_000_000_000,
            updated_at_ms: 1_700_000_000_100,
        }
    }

    fn sample_promotion_payload() -> ApprovedOrgPayload {
        ApprovedOrgPayload {
            kind: OrgKnowledgeKind::Note,
            title: Some("Approved summary".into()),
            body: Some("Redacted approved body.".into()),
            fields: BTreeMap::from([("topic".into(), json!("roadmap"))]),
            approved_payload_hash: "sha256:approved".into(),
        }
    }

    fn sample_promotion_acl() -> PromotionAcl {
        PromotionAcl {
            acl_policy_id: "acl-workspace".into(),
            acl_visibility: AclVisibility::Workspace,
            acl_principals: vec!["workspace:workspace-1".into()],
            acl_inheritance_mode: AclInheritanceMode::NarrowerOfSourceAndTarget,
        }
    }

    fn sample_promotion_retention() -> PromotionRetention {
        PromotionRetention {
            retention_policy_id: "retention-org-memory".into(),
            retention_legal_basis: "user_approved_org_memory".into(),
            retention_category: RetentionCategory::OrgKnowledge,
            expires_at_ms: None,
            delete_behavior: DeleteBehavior::RetractRemote,
        }
    }

    fn sample_source_reference() -> PromotionSourceReference {
        PromotionSourceReference {
            source_object_type: PromotionSourceObjectType::MaterializedNote,
            source_object_id: "note-1".into(),
            source_object_version: "sequence:7".into(),
            source_session_id: "session-1".into(),
            source_span_ids: vec!["span-1".into()],
            source_projection_sequence: Some(7),
            source_basis_hash: "sha256:basis".into(),
            source_hash: "sha256:source".into(),
            source_basis: crate::projections::ProjectionBasis {
                span_revisions: vec![crate::projections::ProjectionBasisSpan {
                    span_id: "span-1".into(),
                    revision_number: 2,
                }],
                diarization_span_revisions: Vec::new(),
                transcript_hash: "sha256:transcript".into(),
            },
            source_provenance: PromotionSourceProvenance {
                asr_provider: Some("soniox".into()),
                source_id: Some("default-mic".into()),
                speaker_ids: vec!["speaker-local-1".into()],
                span_revisions: vec![crate::projections::ProjectionBasisSpan {
                    span_id: "span-1".into(),
                    revision_number: 2,
                }],
                llm: Some(crate::projections::ProjectionProvenance {
                    provider: "openrouter".into(),
                    model: "model-a".into(),
                    prompt_id: "projection-v1".into(),
                }),
                confidence: Some(0.91),
                created_at_ms: 1_700_000_000_000,
                updated_at_ms: 1_700_000_000_100,
            },
        }
    }

    fn sample_promotion_draft() -> PromotionDraft {
        PromotionDraft {
            id: "promotion-draft-1".into(),
            schema_version: PROMOTION_SCHEMA_VERSION,
            created_at_ms: 1_700_000_000_000,
            actor: PromotionActor {
                actor_user_id: "user-1".into(),
                actor_local_profile_id: Some("profile-1".into()),
                actor_device_id: "device-1".into(),
                delegated_service_id: None,
            },
            target: PromotionTarget {
                source_workspace_id: Some("local-workspace".into()),
                target_org_id: "org-1".into(),
                target_workspace_id: "workspace-1".into(),
                target_collection_id: Some("collection-1".into()),
            },
            source: sample_source_reference(),
            candidate_payload_hash: "sha256:candidate".into(),
            requested_redaction_fields: vec!["speaker_name".into()],
            reviewer_user_id: Some("reviewer-1".into()),
            note_redacted: Some("Candidate requires speaker redaction before approval.".into()),
            status: PromotionStatus::Draft,
        }
    }

    fn sample_promotion_event() -> PromotionEvent {
        PromotionEvent {
            id: "promotion-1".into(),
            schema_version: PROMOTION_SCHEMA_VERSION,
            created_at_ms: 1_700_000_000_000,
            actor: PromotionActor {
                actor_user_id: "user-1".into(),
                actor_local_profile_id: Some("profile-1".into()),
                actor_device_id: "device-1".into(),
                delegated_service_id: None,
            },
            target: PromotionTarget {
                source_workspace_id: Some("local-workspace".into()),
                target_org_id: "org-1".into(),
                target_workspace_id: "workspace-1".into(),
                target_collection_id: Some("collection-1".into()),
            },
            source: sample_source_reference(),
            redaction: PromotionRedactionSummary {
                redaction_policy_id: "policy-1".into(),
                redaction_policy_version: "2026-06-26".into(),
                redaction_snapshot_hash: "sha256:redaction".into(),
                redaction_diff: vec![RedactionDiffEntry {
                    field: "body".into(),
                    reason: "speaker_name".into(),
                    before_hash: "sha256:before".into(),
                    after_hash: "sha256:after".into(),
                }],
                redacted_fields: vec!["speaker_name".into()],
                manual_redaction_overrides: vec!["alias-speaker-a".into()],
            },
            reviewer_user_id: "reviewer-1".into(),
            approved_payload_hash: "sha256:approved".into(),
            payload_snapshot: sample_promotion_payload(),
            acl: sample_promotion_acl(),
            retention: sample_promotion_retention(),
            sync: PromotionSyncSnapshot {
                target_kind: PromotionSyncTargetKind::Disabled,
                sync_target_id: None,
                status: PromotionSyncStatus::NotConfigured,
                remote_id: None,
                remote_revision: None,
                remote_etag: None,
                sync_error_code: None,
                sync_error_message_redacted: None,
            },
            lineage: PromotionLineage {
                parent_promotion_id: None,
                supersedes_promotion_id: None,
                conflict_group_id: Some("conflict-group-1".into()),
            },
            conflict_state: PromotionConflictState::None,
            requested_at_ms: 1_700_000_000_000,
            approved_at_ms: Some(1_700_000_000_100),
            status: PromotionStatus::ApprovedLocal,
        }
    }

    fn sample_promotion_revocation_request() -> PromotionRevocationRequest {
        PromotionRevocationRequest {
            id: "revocation-request-1".into(),
            schema_version: PROMOTION_SCHEMA_VERSION,
            promotion_event_id: "promotion-1".into(),
            org_knowledge_item_id: "org-item-1".into(),
            requested_by_user_id: "reviewer-1".into(),
            requested_at_ms: 1_700_000_000_200,
            reason_code: "source_retracted".into(),
            reason_redacted: "Reviewer requested retraction after source review.".into(),
            target_kind: PromotionSyncTargetKind::Disabled,
        }
    }

    fn sample_redaction_snapshot() -> RedactionSnapshot {
        RedactionSnapshot {
            id: "redaction-1".into(),
            schema_version: PROMOTION_SCHEMA_VERSION,
            promotion_event_id: "promotion-1".into(),
            source_object_type: PromotionSourceObjectType::MaterializedNote,
            source_object_id: "note-1".into(),
            policy_id: "policy-1".into(),
            policy_version: "2026-06-26".into(),
            redacted_fields: vec!["speaker_name".into()],
            removed_span_ids: vec!["span-private".into()],
            speaker_alias_map: BTreeMap::from([("speaker-local-1".into(), "Speaker A".into())]),
            entity_alias_map: BTreeMap::new(),
            manual_overrides: vec!["remove private name".into()],
            payload_before_hash: "sha256:before".into(),
            payload_after_hash: "sha256:after".into(),
            approved_payload_hash: "sha256:approved".into(),
            reviewed_by_user_id: "reviewer-1".into(),
            reviewed_at_ms: 1_700_000_000_100,
        }
    }

    fn sample_org_knowledge_item() -> OrgKnowledgeItem {
        OrgKnowledgeItem {
            id: "org-item-1".into(),
            schema_version: PROMOTION_SCHEMA_VERSION,
            org_id: "org-1".into(),
            workspace_id: "workspace-1".into(),
            kind: OrgKnowledgeKind::Note,
            current_revision_id: "org-item-1-r1".into(),
            revision_number: 1,
            title: Some("Approved summary".into()),
            body: Some("Redacted approved body.".into()),
            tags: vec!["roadmap".into()],
            content_hash: "sha256:content".into(),
            redacted_payload: sample_promotion_payload(),
            graph_subject_id: None,
            graph_object_id: None,
            relation_type: None,
            confidence: Some(0.91),
            source_promotion_event_id: "promotion-1".into(),
            promotion_event_ids: vec!["promotion-1".into()],
            source_local_object_fingerprint: "sha256:local-object".into(),
            source_session_fingerprint: "sha256:session".into(),
            provenance_summary: "Approved redacted note from session-1".into(),
            full_provenance_pointer: "promotion://promotion-1".into(),
            acl: sample_promotion_acl(),
            retention: sample_promotion_retention(),
            created_by_user_id: "reviewer-1".into(),
            created_at_ms: 1_700_000_000_100,
            updated_at_ms: 1_700_000_000_100,
            valid_from_ms: 1_700_000_000_100,
            valid_until_ms: None,
            deleted_at_ms: None,
            delete_reason: None,
            state: OrgKnowledgeState::Active,
            conflict_state: PromotionConflictState::None,
            sync_state: PromotionSyncSnapshot {
                target_kind: PromotionSyncTargetKind::Disabled,
                sync_target_id: None,
                status: PromotionSyncStatus::NotConfigured,
                remote_id: None,
                remote_revision: None,
                remote_etag: None,
                sync_error_code: None,
                sync_error_message_redacted: None,
            },
            remote_revision: None,
        }
    }

    fn sample_sync_state() -> PromotionSyncState {
        PromotionSyncState {
            promotion_event_id: "promotion-1".into(),
            target_kind: PromotionSyncTargetKind::ApiServer,
            remote_id: None,
            remote_revision: None,
            remote_etag: None,
            queued_at_ms: Some(1_700_000_000_100),
            last_attempt_at_ms: None,
            last_success_at_ms: None,
            retry_count: 0,
            status: PromotionSyncStatus::Queued,
            last_error_code: None,
            last_error_message_redacted: None,
        }
    }

    #[test]
    fn file_memory_repository_manages_session_metadata_in_explicit_root() {
        let dir = unique_tempdir("sessions");
        let repo = FileMemoryRepository::with_data_root(&dir);

        repo.register_session("session-1")
            .expect("register session");
        repo.update_session_stats("session-1", 7, 2, 3)
            .expect("update stats");
        repo.finalize_session("session-1")
            .expect("finalize session");

        let index = repo.load_session_index().expect("load index");
        assert_eq!(index.len(), 1);
        assert_eq!(index[0].id, "session-1");
        assert_eq!(index[0].status, "complete");
        assert_eq!(index[0].segment_count, 7);
        assert_eq!(index[0].speaker_count, 2);
        assert_eq!(index[0].entity_count, 3);
        assert!(
            index[0].transcript_path.ends_with("session-1.jsonl"),
            "transcript path must preserve current artifact naming"
        );
        assert!(
            index[0].graph_path.ends_with("session-1.json"),
            "graph path must preserve current artifact naming"
        );

        let found = repo
            .find_session("session-1")
            .expect("find session")
            .expect("session exists");
        assert_eq!(found.id, "session-1");

        let artifacts = repo
            .session_artifact_paths("session-1")
            .expect("artifact paths");
        assert!(
            artifacts
                .iter()
                .any(|path| path.ends_with("session-1.events.jsonl")),
            "repository must expose transcript/projection event artifacts"
        );
        assert!(
            artifacts
                .iter()
                .any(|path| path.ends_with("session-1.materialized.json")),
            "repository must expose materialized graph artifact"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_memory_repository_round_trips_events_replay_and_materialized_state() {
        let dir = unique_tempdir("roundtrip");
        let repo = FileMemoryRepository::with_data_root(&dir);
        let session_id = "memory-session-1";
        let first = transcript_event("span-1", 1, "We need a memory boundary.", 1_000);
        let second = transcript_event("span-2", 1, "The file adapter stays default.", 3_000);

        repo.append_transcript_event(session_id, &first)
            .expect("append first transcript event");
        repo.append_transcript_event(session_id, &second)
            .expect("append second transcript event");
        assert_eq!(
            repo.load_transcript_events(session_id)
                .expect("load transcript events"),
            vec![first.clone(), second.clone()]
        );

        let basis_one = crate::projections::ProjectionBasis::from_transcript_events(
            std::slice::from_ref(&first),
        );
        let basis_two = crate::projections::ProjectionBasis::from_transcript_events(&[
            first.clone(),
            second.clone(),
        ]);
        let notes_patch = note_patch(1, basis_one, 2_000);
        let graph_patch = graph_patch(2, basis_two, 4_000);
        repo.append_projection_patch(session_id, &notes_patch)
            .expect("append notes patch");
        repo.append_projection_patch(session_id, &graph_patch)
            .expect("append graph patch");

        assert_eq!(
            repo.load_projection_patches(session_id)
                .expect("load projection patches"),
            vec![notes_patch.clone(), graph_patch.clone()]
        );
        let ledger = repo
            .replay_transcript_ledger(session_id)
            .expect("replay transcript ledger");
        assert_eq!(ledger.latest_spans.len(), 2);

        let replay = repo
            .replay_projection_state(session_id)
            .expect("replay projection state");
        assert_eq!(replay.validation.invalid_patch_count, 0);
        assert_eq!(replay.state.notes.notes.len(), 1);
        assert_eq!(replay.state.graph.nodes.len(), 1);
        assert_eq!(replay.state.graph.nodes[0].id, "node-repository");

        repo.save_materialized_notes(session_id, &replay.state.notes)
            .expect("save materialized notes");
        repo.save_materialized_graph(session_id, &replay.state.graph)
            .expect("save materialized graph");
        let materialized = repo
            .load_materialized_projection_state(session_id)
            .expect("load materialized state");
        assert_eq!(materialized.notes.notes[0].id, "note-1");
        assert_eq!(materialized.graph.nodes[0].name, "LocalMemoryRepository");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_memory_repository_restores_full_session_projection_after_artifact_loss() {
        let dir = unique_tempdir("restore-full-session-projection");
        let repo = FileMemoryRepository::with_data_root(&dir);
        let session_id = "memory-session-restore";
        let first = transcript_event(
            "span-restore-1",
            1,
            "Alice confirmed the backend repository boundary.",
            1_000,
        );
        let second = transcript_event(
            "span-restore-2",
            1,
            "Bob will verify projection replay after artifact loss.",
            2_000,
        );

        for event in [&first, &second] {
            repo.append_transcript_event(session_id, event)
                .expect("append transcript event");
        }

        let basis = crate::projections::ProjectionBasis::from_transcript_events(&[
            first.clone(),
            second.clone(),
        ]);
        let notes_seed = projection_patch(
            1,
            ProjectionKind::Notes,
            basis.clone(),
            vec![ProjectionOperation::UpsertNote {
                id: "note-restore-summary".into(),
                title: "Repository restore".into(),
                body: "Materialized notes can be rebuilt from transcript and projection events."
                    .into(),
                tags: vec!["persistence".into(), "projection".into()],
            }],
            3_000,
        );
        let graph_seed = projection_patch(
            1,
            ProjectionKind::Graph,
            basis.clone(),
            vec![
                ProjectionOperation::UpsertGraphNode {
                    id: "person-alice".into(),
                    name: "Alice".into(),
                    entity_type: "Person".into(),
                    description: Some("Confirmed the repository boundary".into()),
                },
                ProjectionOperation::UpsertGraphNode {
                    id: "task-restore".into(),
                    name: "Projection replay restore".into(),
                    entity_type: "Task".into(),
                    description: Some("Verify restore after materialized artifact loss".into()),
                },
                ProjectionOperation::UpsertGraphEdge {
                    id: "edge-alice-restore".into(),
                    source: "person-alice".into(),
                    target: "task-restore".into(),
                    relation_type: "confirmed".into(),
                    label: Some("confirmed restore work".into()),
                    weight: 0.84,
                },
            ],
            4_000,
        );
        let notes_update = projection_patch(
            2,
            ProjectionKind::Notes,
            basis,
            vec![
                ProjectionOperation::UpsertNote {
                    id: "note-restore-summary".into(),
                    title: "Repository restore".into(),
                    body:
                        "Transcript events plus projection patches rebuild notes and graph state."
                            .into(),
                    tags: vec!["persistence".into(), "projection".into(), "replay".into()],
                },
                ProjectionOperation::UpsertNote {
                    id: "note-restore-risk".into(),
                    title: "Remaining command gap".into(),
                    body: "load_session state isolation still needs command-level coverage.".into(),
                    tags: vec!["follow-up".into()],
                },
            ],
            5_000,
        );

        for patch in [&notes_seed, &graph_seed, &notes_update] {
            repo.append_projection_patch(session_id, patch)
                .expect("append projection patch");
        }

        let expected = repo
            .replay_projection_state(session_id)
            .expect("replay expected projection state");
        assert_eq!(expected.validation.checked_patch_count, 3);
        assert_eq!(expected.validation.invalid_patch_count, 0);
        assert_eq!(expected.state.notes.last_sequence, 2);
        assert_eq!(expected.state.notes.notes.len(), 2);
        assert_eq!(expected.state.graph.last_sequence, 1);
        assert_eq!(expected.state.graph.nodes.len(), 2);
        assert_eq!(expected.state.graph.edges.len(), 1);

        let mut stale_notes = expected.state.notes.clone();
        stale_notes.last_sequence = 0;
        stale_notes.notes[0].body = "Stale materialized notes artifact.".into();
        let mut stale_graph = expected.state.graph.clone();
        stale_graph.last_sequence = 0;
        stale_graph.nodes[0].name = "Stale graph artifact".into();
        repo.save_materialized_notes(session_id, &stale_notes)
            .expect("save stale notes artifact");
        repo.save_materialized_graph(session_id, &stale_graph)
            .expect("save stale graph artifact");

        let loaded_stale = repo
            .load_materialized_projection_state(session_id)
            .expect("load stale materialized state");
        assert_ne!(loaded_stale, expected.state);
        assert_eq!(
            loaded_stale.notes.notes[0].body,
            "Stale materialized notes artifact."
        );
        assert_eq!(loaded_stale.graph.nodes[0].name, "Stale graph artifact");

        let replay_from_stale_artifacts = repo
            .replay_projection_state(session_id)
            .expect("replay ignores stale materialized artifacts");
        assert_eq!(replay_from_stale_artifacts.state, expected.state);
        repo.save_materialized_notes(session_id, &replay_from_stale_artifacts.state.notes)
            .expect("repair notes from replay");
        repo.save_materialized_graph(session_id, &replay_from_stale_artifacts.state.graph)
            .expect("repair graph from replay");
        assert_eq!(
            repo.load_materialized_projection_state(session_id)
                .expect("load repaired materialized state"),
            expected.state
        );

        fs::remove_file(repo.notes_path(session_id).expect("notes path"))
            .expect("remove materialized notes artifact");
        fs::remove_file(
            repo.materialized_graph_path(session_id)
                .expect("materialized graph path"),
        )
        .expect("remove materialized graph artifact");

        let missing_materialized = repo
            .load_materialized_projection_state(session_id)
            .expect("missing materialized artifacts load as empty defaults");
        assert!(missing_materialized.notes.notes.is_empty());
        assert!(missing_materialized.graph.nodes.is_empty());
        assert!(missing_materialized.graph.edges.is_empty());

        let restarted = FileMemoryRepository::with_data_root(&dir);
        let replay_after_artifact_loss = restarted
            .replay_projection_state(session_id)
            .expect("replay after materialized artifact loss");
        assert_eq!(replay_after_artifact_loss.state, expected.state);
        restarted
            .save_materialized_notes(session_id, &replay_after_artifact_loss.state.notes)
            .expect("resave notes after artifact loss");
        restarted
            .save_materialized_graph(session_id, &replay_after_artifact_loss.state.graph)
            .expect("resave graph after artifact loss");
        assert_eq!(
            restarted
                .load_materialized_projection_state(session_id)
                .expect("load restored materialized state"),
            expected.state
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn repository_replay_skips_invalid_historical_projection_basis_and_reports_error() {
        let dir = unique_tempdir("invalid-historical-basis");
        let repo = FileMemoryRepository::with_data_root(&dir);
        let session_id = "memory-session-invalid-basis";
        let first = transcript_event("span-retcon", 1, "The old transcript text.", 1_000);
        let corrected = transcript_event("span-retcon", 2, "The corrected transcript text.", 2_000);

        repo.append_transcript_event(session_id, &first)
            .expect("append first transcript revision");
        repo.append_transcript_event(session_id, &corrected)
            .expect("append corrected transcript revision");

        let stale_basis = crate::projections::ProjectionBasis::from_transcript_events(
            std::slice::from_ref(&first),
        );
        let current_basis = crate::projections::ProjectionBasis::from_transcript_events(
            std::slice::from_ref(&corrected),
        );
        let stale_patch = projection_patch(
            1,
            ProjectionKind::Notes,
            stale_basis,
            vec![ProjectionOperation::UpsertNote {
                id: "note-stale".into(),
                title: "Stale patch".into(),
                body: "This patch was based on a superseded transcript revision.".into(),
                tags: vec!["stale".into()],
            }],
            3_000,
        );
        let repair_patch = projection_patch(
            2,
            ProjectionKind::Notes,
            current_basis,
            vec![ProjectionOperation::UpsertNote {
                id: "note-current".into(),
                title: "Current patch".into(),
                body: "This patch uses the corrected transcript revision.".into(),
                tags: vec!["current".into()],
            }],
            4_000,
        );

        repo.append_projection_patch(session_id, &stale_patch)
            .expect("append stale projection patch");
        repo.append_projection_patch(session_id, &repair_patch)
            .expect("append repair projection patch");

        let replay = repo
            .replay_projection_state(session_id)
            .expect("replay with historical validation report");
        assert_eq!(replay.validation.checked_patch_count, 2);
        assert_eq!(replay.validation.invalid_patch_count, 1);
        assert!(matches!(
            replay.validation.errors.first(),
            Some(HistoricalProjectionValidationError::StaleBasis {
                sequence: 1,
                kind: ProjectionKind::Notes,
                staleness: ProjectionBasisStaleness::StaleSpanRevision {
                    span_id,
                    current_revision: 2,
                    basis_revision: 1,
                },
            }) if span_id == "span-retcon"
        ));
        assert_eq!(replay.state.notes.last_sequence, 2);
        assert_eq!(
            replay
                .state
                .notes
                .notes
                .iter()
                .map(|note| note.id.as_str())
                .collect::<Vec<_>>(),
            vec!["note-current"]
        );

        repo.save_materialized_notes(session_id, &replay.state.notes)
            .expect("save valid replayed notes");
        assert_eq!(
            repo.load_materialized_notes(session_id)
                .expect("load materialized notes")
                .expect("notes artifact exists")
                .notes
                .iter()
                .map(|note| note.id.as_str())
                .collect::<Vec<_>>(),
            vec!["note-current"]
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn transcript_event_writer_can_append_through_repository_handle() {
        let dir = unique_tempdir("repository-transcript-writer");
        let repo = Arc::new(FileMemoryRepository::with_data_root(&dir));
        let repository: Arc<dyn LocalMemoryRepository> = repo.clone();
        let session_id = "repository-writer-session";
        let first = transcript_event(
            "span-repo-1",
            1,
            "Repository-backed transcript write.",
            1_000,
        );
        let second = transcript_event("span-repo-2", 1, "File writer stays default.", 2_000);
        let writer =
            TranscriptEventWriter::repository(session_id, repository).expect("repository writer");

        writer.append(&first);
        writer.append(&second);
        assert!(
            writer.shutdown_with_timeout(std::time::Duration::from_secs(2)),
            "repository writer should drain and join"
        );

        assert_eq!(
            repo.load_transcript_events(session_id)
                .expect("load repository transcript events"),
            vec![first, second]
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn projection_event_writer_can_append_through_repository_handle() {
        let dir = unique_tempdir("repository-projection-writer");
        let repo = Arc::new(FileMemoryRepository::with_data_root(&dir));
        let repository: Arc<dyn LocalMemoryRepository> = repo.clone();
        let session_id = "repository-projection-session";
        let event = transcript_event("span-projection-repo", 1, "Project this span.", 1_000);
        let basis = crate::projections::ProjectionBasis::from_transcript_events(
            std::slice::from_ref(&event),
        );
        let notes_patch = note_patch(1, basis.clone(), 2_000);
        let graph_patch = graph_patch(2, basis, 3_000);
        let writer =
            ProjectionEventWriter::repository(session_id, repository).expect("repository writer");

        assert!(writer.append(&notes_patch));
        assert!(writer.append(&graph_patch));
        assert!(
            writer.shutdown_with_timeout(std::time::Duration::from_secs(2)),
            "repository writer should drain and join"
        );

        assert_eq!(
            repo.load_projection_patches(session_id)
                .expect("load repository projection patches"),
            vec![notes_patch, graph_patch]
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_memory_repository_passes_replay_parity_conformance_suite() {
        let dir = unique_tempdir("replay-parity-conformance");
        let repo = FileMemoryRepository::with_data_root(&dir);

        super::repository_conformance::assert_repository_replay_parity_conformance(
            &repo,
            "replay-parity-session",
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_memory_repository_persists_live_assist_cards_and_outcomes() {
        let dir = unique_tempdir("live-assist-cards");
        let repo = FileMemoryRepository::with_data_root(&dir);
        let session_id = "session-live-assist";
        let pending =
            sample_live_assist_card(session_id, "card-pending", LiveAssistCardStatus::Pending);
        let approved =
            sample_live_assist_card(session_id, "card-approved", LiveAssistCardStatus::Approved);
        let dismissed = sample_live_assist_card(
            session_id,
            "card-dismissed",
            LiveAssistCardStatus::Dismissed,
        );

        repo.upsert_live_assist_card(session_id, &pending)
            .expect("upsert pending card");
        repo.upsert_live_assist_card(session_id, &approved)
            .expect("upsert approved card");
        repo.upsert_live_assist_card(session_id, &dismissed)
            .expect("upsert dismissed card");

        let restarted = FileMemoryRepository::with_data_root(&dir);
        assert_eq!(
            restarted
                .load_live_assist_card_audit(session_id)
                .expect("live assist audit survives restart"),
            vec![pending.clone(), approved.clone(), dismissed.clone()]
        );
        assert_eq!(
            restarted
                .load_live_assist_cards(session_id)
                .expect("current live assist cards survive restart"),
            vec![approved.clone(), dismissed, pending]
        );
        assert_eq!(
            approved
                .outcome
                .as_ref()
                .map(|outcome| outcome.action.as_str()),
            Some("graph_update")
        );
        assert_eq!(approved.projection_patch_sequence, Some(3));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_memory_repository_rejects_live_assist_cards_without_citations_or_outcome() {
        let dir = unique_tempdir("live-assist-invalid");
        let repo = FileMemoryRepository::with_data_root(&dir);
        let session_id = "session-live-assist";
        let mut no_citation = sample_live_assist_card(
            session_id,
            "card-no-citation",
            LiveAssistCardStatus::Pending,
        );
        no_citation.source_span_ids.clear();
        no_citation.graph_context_ids.clear();

        let error = repo
            .upsert_live_assist_card(session_id, &no_citation)
            .expect_err("live assist cards must cite transcript spans or graph context");
        assert!(
            error.contains("must cite transcript spans or graph context"),
            "unexpected error: {error}"
        );

        let mut missing_outcome = sample_live_assist_card(
            session_id,
            "card-missing-outcome",
            LiveAssistCardStatus::Approved,
        );
        missing_outcome.outcome = None;
        let error = repo
            .upsert_live_assist_card(session_id, &missing_outcome)
            .expect_err("approved live assist cards require outcomes");
        assert!(
            error.contains("requires an outcome"),
            "unexpected error: {error}"
        );
        let mut missing_projection = sample_live_assist_card(
            session_id,
            "card-missing-projection",
            LiveAssistCardStatus::Approved,
        );
        missing_projection.projection_patch_sequence = None;
        let error = repo
            .upsert_live_assist_card(session_id, &missing_projection)
            .expect_err("approved live assist cards require projection patch evidence");
        assert!(
            error.contains("requires a projection patch sequence"),
            "unexpected error: {error}"
        );
        assert!(
            repo.load_live_assist_cards(session_id)
                .expect("load current live assist cards")
                .is_empty(),
            "invalid live assist cards must not materialize"
        );
        assert!(
            repo.load_live_assist_card_audit(session_id)
                .expect("load live assist audit")
                .is_empty(),
            "invalid live assist cards must not enter audit log"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_memory_repository_exposes_live_assist_artifact_paths() {
        let dir = unique_tempdir("live-assist-artifact-paths");
        let repo = FileMemoryRepository::with_data_root(&dir);
        let artifacts = repo
            .session_artifacts("session-live-assist")
            .expect("session artifact descriptors");
        let paths = repo
            .session_artifact_paths("session-live-assist")
            .expect("session artifact paths");

        assert!(
            artifacts.iter().any(|artifact| matches!(
                (&artifact.kind, &artifact.storage),
                (
                    SessionArtifactKind::LiveAssistAudit,
                    SessionArtifactStorage::File { path },
                ) if path.ends_with("session-live-assist.jsonl")
            )),
            "live assist audit descriptor should be a file artifact: {artifacts:?}"
        );
        assert!(
            artifacts.iter().any(|artifact| matches!(
                (&artifact.kind, &artifact.storage),
                (
                    SessionArtifactKind::LiveAssistCurrent,
                    SessionArtifactStorage::File { path },
                ) if path.ends_with("session-live-assist.current.json")
            )),
            "live assist current descriptor should be a file artifact: {artifacts:?}"
        );
        assert!(
            paths
                .iter()
                .any(|path| path.ends_with("session-live-assist.jsonl")),
            "live assist audit log should be part of session artifacts: {paths:?}"
        );
        assert!(
            paths
                .iter()
                .any(|path| path.ends_with("session-live-assist.current.json")),
            "live assist current snapshot should be part of session artifacts: {paths:?}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_memory_repository_delete_session_artifacts_reports_file_cleanup() {
        let dir = unique_tempdir("delete-session-artifacts");
        let repo = FileMemoryRepository::with_data_root(&dir);
        let session_id = "session-delete-artifacts";
        let event = transcript_event("span-delete", 1, "Delete file artifacts.", 1_000);

        repo.register_session(session_id).expect("register session");
        repo.append_transcript_event(session_id, &event)
            .expect("append transcript event");
        let transcript_events_path = repo
            .session_artifact_paths(session_id)
            .expect("artifact paths")
            .into_iter()
            .find(|path| path.ends_with("session-delete-artifacts.events.jsonl"))
            .expect("transcript events path");
        assert!(transcript_events_path.exists());

        let report = repo
            .delete_session_artifacts(session_id)
            .expect("delete artifacts");
        assert!(!report.has_failures(), "unexpected failures: {report:?}");
        assert!(
            report
                .deleted_files
                .iter()
                .any(|path| path == &transcript_events_path),
            "transcript event log should be deleted: {report:?}"
        );
        assert!(!transcript_events_path.exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_memory_repository_persists_promotion_audit_without_projection_side_effects() {
        let dir = unique_tempdir("promotion-audit");
        let repo = FileMemoryRepository::with_data_root(&dir);
        let draft = sample_promotion_draft();
        let event = sample_promotion_event();
        let snapshot = sample_redaction_snapshot();
        let item = sample_org_knowledge_item();
        let sync_state = sample_sync_state();
        let revocation_request = sample_promotion_revocation_request();

        repo.append_promotion_draft(&draft)
            .expect("append promotion draft");
        repo.append_promotion_event(&event)
            .expect("append promotion event");
        repo.append_redaction_snapshot(&snapshot)
            .expect("append redaction snapshot");
        repo.upsert_org_knowledge_item(&item)
            .expect("upsert org item");
        repo.upsert_promotion_sync_state(&sync_state)
            .expect("upsert sync state");
        repo.append_promotion_revocation_request(&revocation_request)
            .expect("append revocation request");

        let restarted = FileMemoryRepository::with_data_root(&dir);
        assert_eq!(
            restarted
                .load_promotion_drafts()
                .expect("promotion drafts survive restart"),
            vec![draft]
        );
        assert_eq!(
            restarted
                .load_promotion_events()
                .expect("promotion events survive restart"),
            vec![event]
        );
        assert_eq!(
            restarted
                .load_redaction_snapshots()
                .expect("redaction snapshots survive restart"),
            vec![snapshot]
        );
        assert_eq!(
            restarted
                .load_org_knowledge_item_audit()
                .expect("org item audit survives restart"),
            vec![item.clone()]
        );
        assert_eq!(
            restarted
                .load_org_knowledge_items()
                .expect("current org items survive restart"),
            vec![item.clone()]
        );
        assert_eq!(
            restarted
                .load_promotion_sync_state_audit()
                .expect("sync audit survives restart"),
            vec![sync_state.clone()]
        );
        assert_eq!(
            restarted
                .load_promotion_sync_states()
                .expect("current sync survives restart"),
            vec![sync_state]
        );
        assert_eq!(
            restarted
                .load_promotion_revocation_requests()
                .expect("revocation requests survive restart"),
            vec![revocation_request]
        );
        assert!(
            restarted
                .load_transcript_events("session-1")
                .expect("promotion persistence must not create transcript events")
                .is_empty()
        );
        assert!(
            restarted
                .load_projection_patches("session-1")
                .expect("promotion persistence must not create projection patches")
                .is_empty()
        );

        let org_visible_json =
            serde_json::to_string(&item).expect("serialize org-visible current item");
        for forbidden in [
            "api_key",
            "raw_transcript_text",
            "speaker_names",
            "source_ids",
            "provider_ids",
        ] {
            assert!(
                !org_visible_json.contains(forbidden),
                "org-visible item must omit {forbidden}: {org_visible_json}"
            );
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_memory_repository_blocked_promotion_event_has_no_current_side_effects() {
        let dir = unique_tempdir("promotion-blocked-event");
        let repo = FileMemoryRepository::with_data_root(&dir);
        let mut event = sample_promotion_event();
        event.id = "promotion-blocked-1".into();
        event.status = PromotionStatus::BlockedByStaleSource;
        event.conflict_state = PromotionConflictState::SourceSuperseded;
        event.approved_at_ms = None;

        repo.append_promotion_event(&event)
            .expect("append blocked promotion event");

        let events = repo.load_promotion_events().expect("load promotion events");
        assert_eq!(events, vec![event]);
        assert!(
            repo.load_org_knowledge_items()
                .expect("load current org items")
                .is_empty(),
            "blocked promotion events must not materialize org-visible items"
        );
        assert!(
            repo.load_promotion_sync_states()
                .expect("load current sync states")
                .is_empty(),
            "blocked promotion events must not enqueue sync state"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_memory_repository_preserves_org_item_conflict_state() {
        let dir = unique_tempdir("promotion-conflict-state");
        let repo = FileMemoryRepository::with_data_root(&dir);
        let mut item = sample_org_knowledge_item();
        item.conflict_state = PromotionConflictState::RemoteNewer;
        item.sync_state.target_kind = PromotionSyncTargetKind::ApiServer;
        item.sync_state.sync_target_id = Some("org-api-1".into());
        item.sync_state.status = PromotionSyncStatus::Conflict;
        item.sync_state.remote_revision = Some("remote-r2".into());
        item.remote_revision = Some("remote-r2".into());

        repo.upsert_org_knowledge_item(&item)
            .expect("upsert conflicted org item");

        let current = repo
            .load_org_knowledge_items()
            .expect("load current org items");
        assert_eq!(current.len(), 1);
        assert_eq!(
            current[0].conflict_state,
            PromotionConflictState::RemoteNewer
        );
        assert_eq!(current[0].sync_state.status, PromotionSyncStatus::Conflict);
        assert_eq!(current[0].remote_revision.as_deref(), Some("remote-r2"));
        assert_eq!(
            repo.load_org_knowledge_item_audit()
                .expect("load org item audit"),
            vec![item]
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_memory_repository_rejects_silent_active_org_item_source_mutation() {
        let dir = unique_tempdir("promotion-source-mutation");
        let repo = FileMemoryRepository::with_data_root(&dir);
        let active = sample_org_knowledge_item();
        repo.upsert_org_knowledge_item(&active)
            .expect("upsert active item");

        let mut changed = active.clone();
        changed.current_revision_id = "org-item-1-r2".into();
        changed.revision_number = 2;
        changed.updated_at_ms = 1_700_000_000_200;
        changed.source_promotion_event_id = "promotion-2".into();
        changed.promotion_event_ids = vec!["promotion-2".into()];
        changed.source_local_object_fingerprint = "sha256:local-object-v2".into();

        let error = repo
            .upsert_org_knowledge_item(&changed)
            .expect_err("changed source must create a new promotion draft");
        assert!(
            error.contains("source changed without conflict/review state"),
            "unexpected error: {error}"
        );
        assert_eq!(
            repo.load_org_knowledge_items()
                .expect("load current org items"),
            vec![active.clone()]
        );
        assert_eq!(
            repo.load_org_knowledge_item_audit()
                .expect("load org item audit"),
            vec![active]
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_memory_repository_rejects_unredacted_revocation_requests() {
        let dir = unique_tempdir("promotion-unredacted-revocation");
        let repo = FileMemoryRepository::with_data_root(&dir);
        let mut request = sample_promotion_revocation_request();
        request.reason_redacted = "Authorization: Bearer sk-private".into();

        let error = repo
            .append_promotion_revocation_request(&request)
            .expect_err("revocation reasons must be redacted");
        assert!(
            error.contains("UnredactedErrorMessage"),
            "unexpected error: {error}"
        );
        assert!(
            repo.load_promotion_revocation_requests()
                .expect("load revocation requests")
                .is_empty(),
            "rejected revocation request must not enter audit log"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn checked_promotion_draft_creation_does_not_append_blocked_source_sessions() {
        let dir = unique_tempdir("promotion-draft-blocked-source");
        let repo = FileMemoryRepository::with_data_root(&dir);
        let draft = sample_promotion_draft();

        for state in [
            PromotionSourceSessionState::SoftDeleted,
            PromotionSourceSessionState::RetentionExpired,
        ] {
            let error = repo
                .create_promotion_draft_checked(&draft, state)
                .expect_err("blocked source sessions must not create promotion drafts");
            assert!(
                error.contains("BlockedSourceSessionState"),
                "unexpected error for {state:?}: {error}"
            );
            assert!(
                repo.load_promotion_drafts()
                    .expect("load promotion drafts")
                    .is_empty(),
                "blocked state {state:?} must leave draft audit empty"
            );
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn checked_promotion_draft_creation_allows_explicit_review_restore() {
        let dir = unique_tempdir("promotion-draft-restored-source");
        let repo = FileMemoryRepository::with_data_root(&dir);
        let draft = sample_promotion_draft();

        repo.create_promotion_draft_checked(
            &draft,
            PromotionSourceSessionState::ExplicitlyRestoredForReview,
        )
        .expect("explicit review restore permits draft creation");

        assert_eq!(
            repo.load_promotion_drafts().expect("load promotion drafts"),
            vec![draft]
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_memory_repository_rejects_private_org_visible_payload_fields() {
        let dir = unique_tempdir("promotion-private-fields");
        let repo = FileMemoryRepository::with_data_root(&dir);
        let mut item = sample_org_knowledge_item();
        item.redacted_payload
            .fields
            .insert("api_key".into(), json!("sk-local-secret"));

        let error = repo
            .upsert_org_knowledge_item(&item)
            .expect_err("org-visible records must reject credential-like fields");
        assert!(
            error.contains("PrivatePayloadField") || error.contains("forbidden org-visible key"),
            "unexpected error: {error}"
        );
        assert!(
            repo.load_org_knowledge_items()
                .expect("load current org items")
                .is_empty(),
            "rejected org item must not be materialized"
        );
        assert!(
            repo.load_org_knowledge_item_audit()
                .expect("load org item audit")
                .is_empty(),
            "rejected org item must not enter audit log"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_memory_repository_revokes_org_item_and_sync_state() {
        let dir = unique_tempdir("promotion-revocation");
        let repo = FileMemoryRepository::with_data_root(&dir);
        let active = sample_org_knowledge_item();
        repo.upsert_org_knowledge_item(&active)
            .expect("upsert active item");

        let mut revoked = active.clone();
        revoked.revision_number = 2;
        revoked.current_revision_id = "org-item-1-r2".into();
        revoked.updated_at_ms = 1_700_000_000_200;
        revoked.deleted_at_ms = Some(1_700_000_000_200);
        revoked.delete_reason = Some("reviewer_revoked".into());
        revoked.state = OrgKnowledgeState::Retracted;
        revoked.sync_state.status = PromotionSyncStatus::Revoked;

        let mut sync_state = sample_sync_state();
        sync_state.status = PromotionSyncStatus::Revoked;
        sync_state.last_attempt_at_ms = Some(1_700_000_000_200);

        repo.revoke_org_knowledge_item(&revoked, Some(&sync_state))
            .expect("revoke org item");

        let current = repo
            .load_org_knowledge_items()
            .expect("load current org items");
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].state, OrgKnowledgeState::Retracted);
        assert_eq!(current[0].deleted_at_ms, Some(1_700_000_000_200));
        assert_eq!(
            current[0].delete_reason.as_deref(),
            Some("reviewer_revoked")
        );

        let audit = repo
            .load_org_knowledge_item_audit()
            .expect("load org item audit");
        assert_eq!(
            audit.len(),
            2,
            "active and revoked revisions must be audited"
        );
        assert_eq!(audit[0].state, OrgKnowledgeState::Active);
        assert_eq!(audit[1].state, OrgKnowledgeState::Retracted);

        let sync_current = repo
            .load_promotion_sync_states()
            .expect("load current sync states");
        assert_eq!(sync_current, vec![sync_state.clone()]);
        assert_eq!(
            repo.load_promotion_sync_state_audit()
                .expect("load sync audit"),
            vec![sync_state]
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_memory_repository_missing_logs_load_as_empty_state() {
        let dir = unique_tempdir("missing");
        let repo = FileMemoryRepository::with_data_root(&dir);
        let session_id = "empty-session";

        assert!(
            repo.load_transcript_events(session_id)
                .expect("missing transcript log")
                .is_empty()
        );
        assert!(
            repo.load_projection_patches(session_id)
                .expect("missing projection log")
                .is_empty()
        );
        let materialized = repo
            .load_materialized_projection_state(session_id)
            .expect("missing materialized state defaults");
        assert_eq!(materialized.session_id, session_id);
        assert!(materialized.notes.notes.is_empty());
        assert!(materialized.graph.nodes.is_empty());
        assert!(materialized.graph.edges.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_memory_repository_rejects_path_like_session_ids() {
        let dir = unique_tempdir("invalid-session-id");
        let repo = FileMemoryRepository::with_data_root(&dir);
        let event = transcript_event("span-1", 1, "Invalid session ids stay rejected.", 1_000);

        for invalid in ["", "../escape", "nested/session", "nested\\session"] {
            assert!(
                repo.register_session(invalid).is_err(),
                "register_session must reject {invalid:?}"
            );
            assert!(
                repo.append_transcript_event(invalid, &event).is_err(),
                "append_transcript_event must reject {invalid:?}"
            );
            assert!(
                repo.load_transcript_events(invalid).is_err(),
                "load_transcript_events must reject {invalid:?}"
            );
            assert!(
                repo.session_artifact_paths(invalid).is_err(),
                "session_artifact_paths must reject {invalid:?}"
            );
        }

        let _ = fs::remove_dir_all(&dir);
    }

    /// RAII guard that points `AUDIOGRAPH_DATA_DIR` at an isolated tempdir and
    /// restores the previous value on drop. Mutating process env requires the
    /// `crate::sessions::TEST_HOME_LOCK` to be held by the caller.
    struct DataDirGuard {
        prev: Option<std::ffi::OsString>,
    }

    impl DataDirGuard {
        #[allow(unsafe_code)]
        fn set(path: &Path) -> Self {
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

    fn legacy_segment(
        id: &str,
        source_id: &str,
        text: &str,
        start: f64,
        end: f64,
    ) -> TranscriptSegment {
        TranscriptSegment {
            id: id.into(),
            source_id: source_id.into(),
            speaker_id: Some("speaker-1".into()),
            speaker_label: Some("Speaker 1".into()),
            text: text.into(),
            start_time: start,
            end_time: end,
            confidence: 0.9,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn provider_span(
        provider: &str,
        source_id: &str,
        span_id: &str,
        segment_id: Option<&str>,
        revision_number: u64,
        text: &str,
        is_final: bool,
        received_at_ms: u64,
    ) -> TranscriptEvent {
        TranscriptEvent {
            span_id: span_id.into(),
            provider: provider.into(),
            source_id: source_id.into(),
            provider_item_id: None,
            transcript_segment_id: segment_id.map(str::to_string),
            speaker_id: None,
            speaker_label: None,
            channel: None,
            text: text.into(),
            start_time: revision_number as f64,
            end_time: revision_number as f64 + 0.5,
            confidence: 0.92,
            is_final,
            stability: if is_final {
                crate::projections::TranscriptEventStability::Final
            } else {
                crate::projections::TranscriptEventStability::Partial
            },
            revision_number,
            supersedes: (revision_number > 1).then(|| format!("{span_id}@rev1")),
            turn_id: None,
            end_of_turn: is_final,
            raw_event_ref: None,
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms,
        }
    }

    #[test]
    fn load_transcript_segments_prefers_event_log_and_derives_duplicate_free_view() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("prefer-ledger");
        let _guard = DataDirGuard::set(&dir);
        let session_id = "session-prefer-ledger";

        // Two stable spans, each a partial superseded by a final revision.
        let events = [
            provider_span(
                "openai_realtime",
                "system",
                "openai_realtime:system:item-1",
                Some("openai_realtime:system:item-1-seg"),
                1,
                "partial one",
                false,
                1_000,
            ),
            provider_span(
                "openai_realtime",
                "system",
                "openai_realtime:system:item-1",
                Some("openai_realtime:system:item-1-seg"),
                2,
                "final one",
                true,
                1_100,
            ),
            provider_span(
                "deepgram",
                "system",
                "deepgram:system:start-2000",
                Some("deepgram:system:start-2000-seg"),
                1,
                "partial two",
                false,
                1_200,
            ),
            provider_span(
                "deepgram",
                "system",
                "deepgram:system:start-2000",
                Some("deepgram:system:start-2000-seg"),
                2,
                "final two",
                true,
                1_300,
            ),
        ];
        let events_path =
            crate::user_data::transcript_events_path(session_id).expect("events path");
        for event in &events {
            append_jsonl(event, &events_path, "transcript event").expect("append transcript event");
        }
        // A stale legacy file must be ignored when the event log is present.
        let legacy_path = crate::user_data::transcript_path(session_id).expect("legacy path");
        append_jsonl(
            &legacy_segment("stale-legacy", "system", "stale legacy row", 0.0, 1.0),
            &legacy_path,
            "legacy transcript segment",
        )
        .expect("write stale legacy row");

        let segments =
            load_transcript_segments_preferring_ledger(session_id).expect("load preferring ledger");

        assert_eq!(
            segments.len(),
            2,
            "event-log session must derive a duplicate-free canonical view"
        );
        // Both final revisions share start_time == 2.0, so the canonical view
        // orders them by span_id (`deepgram:...` < `openai_realtime:...`).
        assert_eq!(
            segments.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(),
            vec!["final two", "final one"],
            "derived view reflects latest accepted revisions, not the stale legacy row"
        );
        assert_eq!(
            segments.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            vec![
                "deepgram:system:start-2000-seg",
                "openai_realtime:system:item-1-seg",
            ]
        );
        assert!(
            segments.iter().all(|s| s.text != "stale legacy row"),
            "legacy rows must not leak into the event-derived view"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_transcript_segments_falls_back_to_legacy_jsonl_unchanged() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("legacy-only");
        let _guard = DataDirGuard::set(&dir);
        let session_id = "session-legacy-only";

        // Legacy-only session: no .events.jsonl, only the legacy .jsonl rows.
        let legacy_path = crate::user_data::transcript_path(session_id).expect("legacy path");
        let rows = [
            legacy_segment("seg-1", "system", "hello world", 0.0, 1.5),
            legacy_segment("seg-2", "mic-1", "second utterance", 1.5, 3.0),
        ];
        for row in &rows {
            append_jsonl(row, &legacy_path, "legacy transcript segment").expect("write legacy row");
        }

        let segments = load_transcript_segments_preferring_ledger(session_id)
            .expect("load legacy-only session");

        assert_eq!(segments.len(), 2, "legacy rows must load unchanged");
        assert_eq!(
            segments.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            vec!["seg-1", "seg-2"],
            "legacy segment ids are returned verbatim with no migration"
        );
        assert_eq!(segments[0].text, "hello world");
        assert_eq!(segments[1].source_id, "mic-1");
        assert_eq!(segments[1].text, "second utterance");

        // No event log should have been created by the read path.
        let events_path =
            crate::user_data::transcript_events_path(session_id).expect("events path");
        assert!(
            !events_path.exists(),
            "the read path must not migrate legacy rows into an event log"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_transcript_segments_superseding_partials_yield_one_segment_per_final_span() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("superseding-partials");
        let _guard = DataDirGuard::set(&dir);
        let session_id = "session-superseding";

        // A single span that receives three partial revisions then a final one.
        let span_id = "deepgram:system:start-5000";
        let events_path =
            crate::user_data::transcript_events_path(session_id).expect("events path");
        for (revision, text, is_final, received) in [
            (1u64, "the", false, 5_000u64),
            (2, "the quick", false, 5_050),
            (3, "the quick brown", false, 5_100),
            (4, "the quick brown fox", true, 5_150),
        ] {
            append_jsonl(
                &provider_span(
                    "deepgram",
                    "system",
                    span_id,
                    Some("deepgram:system:start-5000-seg"),
                    revision,
                    text,
                    is_final,
                    received,
                ),
                &events_path,
                "transcript event",
            )
            .expect("append revision");
        }

        let segments =
            load_transcript_segments_preferring_ledger(session_id).expect("load preferring ledger");

        assert_eq!(
            segments.len(),
            1,
            "four superseding revisions of one span collapse to a single segment"
        );
        assert_eq!(segments[0].text, "the quick brown fox");
        assert_eq!(segments[0].id, "deepgram:system:start-5000-seg");

        let _ = fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
pub(crate) mod repository_conformance {
    pub(crate) use super::local_memory_repository_tests::assert_repository_replay_parity_conformance;
}

// ---------------------------------------------------------------------------
// Tests — graph autosave stop signal + graceful-shutdown final save
// ---------------------------------------------------------------------------
//
// Pins the graceful-shutdown wiring the RunEvent::Exit handler relies on:
//   - `spawn_graph_autosave`'s loop observes the shared `stop` flag within a
//     poll interval and exits (so Exit can signal + join it bounded).
//   - `autosave_final_save` writes the current session's graph to disk (so a
//     clean File->Quit doesn't lose up to ~30s of derived-graph state).
//
// Both touch `graphs_dir()`, which resolves from HOME / AUDIOGRAPH_DATA_DIR, so
// they mutate the process env under `crate::sessions::TEST_HOME_LOCK` (shared
// with sessions/rotation tests) and run `--test-threads=1`.
#[cfg(test)]
mod autosave_shutdown_tests {
    use super::*;
    use crate::graph::entities::ExtractedEntity;
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::AtomicU64;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    fn unique_tempdir(label: &str) -> std::path::PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!(
            "audio-graph-autosave-shutdown-{}-{}-{}-{}",
            label, pid, nanos, n
        ));
        fs::create_dir_all(&dir).expect("create tempdir");
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
            // SAFETY: serialized by crate::sessions::TEST_HOME_LOCK; the caller
            // MUST hold that lock for the lifetime of this guard.
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

    #[test]
    fn autosave_loop_exits_promptly_on_stop_signal() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("stop-signal");
        let _g = HomeGuard::set(&dir);

        let session_id = Arc::new(RwLock::new("autosave-stop-session".to_string()));
        let graph = Arc::new(Mutex::new(TemporalKnowledgeGraph::new()));
        let buffer = Arc::new(RwLock::new(VecDeque::new()));
        let rotation = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));

        let handle = spawn_graph_autosave(session_id, graph, buffer, rotation, stop.clone())
            .expect("autosave thread must spawn with HOME override");

        // Signal stop and confirm the thread joins well within the 30s cadence —
        // proving the loop polls the flag on the short interval, not the sleep.
        stop.store(true, Ordering::SeqCst);
        let start = Instant::now();
        // Join on a watchdog so a regression (loop ignoring the flag) fails the
        // test with a timeout rather than hanging the suite for 30s.
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            let _ = handle.join();
            let _ = done_tx.send(());
        });
        assert!(
            done_rx.recv_timeout(Duration::from_secs(5)).is_ok(),
            "autosave loop must exit within 5s of the stop signal (elapsed {:?})",
            start.elapsed()
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn autosave_final_save_writes_current_session_graph() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("final-save");
        let _g = HomeGuard::set(&dir);

        let session_id = Arc::new(RwLock::new("autosave-final-session".to_string()));
        let graph = Arc::new(Mutex::new(TemporalKnowledgeGraph::new()));
        {
            let mut g = graph.lock().unwrap();
            g.add_entity(
                &ExtractedEntity {
                    name: "AudioGraph".to_string(),
                    entity_type: "Product".to_string(),
                    description: Some("Streaming speech knowledge graph app.".to_string()),
                },
                0.0,
                "speaker-1",
            );
            assert_eq!(g.node_count(), 1, "seeded graph must have one node");
        }
        let buffer = Arc::new(RwLock::new(VecDeque::new()));

        // The one-shot final save the Exit handler performs after the autosave
        // thread has stopped.
        autosave_final_save(&session_id, &graph, &buffer);

        let expected = graphs_dir()
            .expect("graphs dir resolves under HOME override")
            .join("autosave-final-session.json");
        assert!(
            expected.exists(),
            "final save must write the current session graph to {:?}",
            expected
        );
        let contents = fs::read_to_string(&expected).expect("read saved graph");
        assert!(
            contents.contains("AudioGraph"),
            "saved graph must contain the seeded entity, got: {contents}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn autosave_final_save_empty_graph_writes_no_file() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("final-save-empty");
        let _g = HomeGuard::set(&dir);

        let session_id = Arc::new(RwLock::new("autosave-empty-session".to_string()));
        let graph = Arc::new(Mutex::new(TemporalKnowledgeGraph::new()));
        let buffer = Arc::new(RwLock::new(VecDeque::new()));

        autosave_final_save(&session_id, &graph, &buffer);

        // An empty graph (node_count == 0) is not persisted — mirrors the loop
        // tick's `node_count > 0` guard.
        let graph_file = graphs_dir()
            .expect("graphs dir resolves under HOME override")
            .join("autosave-empty-session.json");
        assert!(
            !graph_file.exists(),
            "final save of an empty graph must not write a file at {:?}",
            graph_file
        );

        let _ = fs::remove_dir_all(&dir);
    }

    // -----------------------------------------------------------------------
    // Data-movement ledger (seed audio-graph-70a3)
    // -----------------------------------------------------------------------

    fn provider_call_event(session_id: &str, created_at_ms: u64) -> DataMovementEvent {
        DataMovementLedgerBuilder::new(
            session_id,
            DataMovementActor::System,
            DataMovementEventType::ProviderCallSucceeded,
            MovementPolicy {
                privacy_mode: PrivacyMode::ByokCloud,
                user_visible: true,
                retention_class: RetentionClass::Transient,
            },
            DataMovementDestination::provider("llm.openrouter", "chat_completions"),
        )
        .created_at_ms(created_at_ms)
        .data_classes([DataClass::TranscriptText, DataClass::Prompts])
        .model(MovementModel {
            provider_id: Some("llm.openrouter".to_string()),
            model_id: Some("anthropic/claude-sonnet-4".to_string()),
        })
        .counts(MovementCounts {
            audio_ms: None,
            text_chars: Some(1200),
            tokens_in: Some(300),
            tokens_out: Some(80),
            bytes: None,
        })
        .basis(MovementBasis {
            transcript_sequence: Some(12),
            projection_sequence: Some(4),
        })
        .build()
    }

    #[test]
    fn data_movement_ledger_appends_and_loads_in_order() {
        let dir = unique_tempdir("data-movement-append");
        let repo = FileMemoryRepository::with_data_root(&dir);
        let session_id = "session-ledger";

        // Empty ledger loads as empty, not an error.
        assert!(
            repo.load_data_movement_events(session_id)
                .expect("empty ledger loads")
                .is_empty()
        );

        let first = provider_call_event(session_id, 1_000);
        let second = DataMovementLedgerBuilder::new(
            session_id,
            DataMovementActor::User,
            DataMovementEventType::ArtifactExported,
            MovementPolicy {
                privacy_mode: PrivacyMode::LocalOnly,
                user_visible: true,
                retention_class: RetentionClass::SessionArtifact,
            },
            DataMovementDestination {
                boundary: DestinationBoundary::Export,
                provider_id: None,
                endpoint_class: None,
            },
        )
        .created_at_ms(2_000)
        .data_classes([DataClass::TranscriptText, DataClass::Notes])
        .artifact(
            "transcript_events",
            ArtifactStorageKind::File,
            std::path::Path::new("/home/alice/.audiograph/transcripts/session-ledger.events.jsonl"),
        )
        .build();

        repo.append_data_movement_event(session_id, &first)
            .expect("append first");
        repo.append_data_movement_event(session_id, &second)
            .expect("append second");

        let loaded = repo
            .load_data_movement_events(session_id)
            .expect("load ledger");
        assert_eq!(loaded, vec![first, second]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn data_movement_ledger_records_provider_model_boundary_counts_and_basis() {
        let dir = unique_tempdir("data-movement-provider");
        let repo = FileMemoryRepository::with_data_root(&dir);
        let session_id = "session-provider-call";

        repo.append_data_movement_event(session_id, &provider_call_event(session_id, 5_000))
            .expect("append provider call");

        let loaded = repo
            .load_data_movement_events(session_id)
            .expect("load ledger");
        assert_eq!(loaded.len(), 1);
        let event = &loaded[0];

        // Data classes recorded.
        assert!(event.data_classes.contains(&DataClass::TranscriptText));
        assert!(event.data_classes.contains(&DataClass::Prompts));
        // Provider / model ids recorded.
        assert_eq!(event.destination.boundary, DestinationBoundary::Provider);
        assert_eq!(
            event.destination.provider_id.as_deref(),
            Some("llm.openrouter")
        );
        assert_eq!(
            event.destination.endpoint_class.as_deref(),
            Some("chat_completions")
        );
        assert_eq!(
            event.model.as_ref().and_then(|m| m.model_id.as_deref()),
            Some("anthropic/claude-sonnet-4")
        );
        // Counts recorded.
        let counts = event.counts.as_ref().expect("counts recorded");
        assert_eq!(counts.tokens_in, Some(300));
        assert_eq!(counts.tokens_out, Some(80));
        assert_eq!(counts.text_chars, Some(1200));
        // Basis recorded.
        assert_eq!(
            event.basis.as_ref().and_then(|b| b.transcript_sequence),
            Some(12)
        );
        // Status.
        assert_eq!(event.result.status, MovementStatus::Succeeded);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn data_movement_ledger_records_delete_export_with_hashed_paths_not_raw() {
        let dir = unique_tempdir("data-movement-delete");
        let repo = FileMemoryRepository::with_data_root(&dir);
        let session_id = "session-delete";
        let raw_path =
            std::path::Path::new("/home/alice/.audiograph/transcripts/session-delete.events.jsonl");

        let delete_event = DataMovementLedgerBuilder::new(
            session_id,
            DataMovementActor::User,
            DataMovementEventType::ArtifactHardDeleted,
            MovementPolicy {
                privacy_mode: PrivacyMode::LocalOnly,
                user_visible: true,
                retention_class: RetentionClass::SessionArtifact,
            },
            DataMovementDestination::local(),
        )
        .data_classes([DataClass::TranscriptText])
        .artifact("transcript_events", ArtifactStorageKind::File, raw_path)
        .build();

        repo.append_data_movement_event(session_id, &delete_event)
            .expect("append delete");

        // The persisted ledger bytes must not contain the raw path or username.
        let ledger_bytes = fs::read_to_string(
            repo.data_movement_ledger_path(session_id)
                .expect("ledger path"),
        )
        .expect("read ledger");
        assert!(
            !ledger_bytes.contains("/home/alice"),
            "raw artifact path leaked into ledger: {ledger_bytes}"
        );
        assert!(ledger_bytes.contains("h64:"), "expected hashed path ref");

        let loaded = repo
            .load_data_movement_events(session_id)
            .expect("load ledger");
        let artifact = &loaded[0].artifact_refs[0];
        assert_eq!(artifact.kind, "transcript_events");
        assert!(
            artifact
                .path_hash
                .as_deref()
                .expect("hash")
                .starts_with("h64:")
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn data_movement_ledger_records_redacted_failed_provider_error() {
        let dir = unique_tempdir("data-movement-failed");
        let repo = FileMemoryRepository::with_data_root(&dir);
        let session_id = "session-failed";

        // Runtime-assembled key-shaped sentinel — no static sk- literal appears
        // in source (avoids tripping secret scanners on the fake test sentinel)
        // while still exercising the redactor on real credential shape.
        let fake_key = ["s", "k", "-", &"A".repeat(24)].concat();

        let failed = DataMovementLedgerBuilder::new(
            session_id,
            DataMovementActor::Provider,
            DataMovementEventType::ProviderCallFailed,
            MovementPolicy {
                privacy_mode: PrivacyMode::ByokCloud,
                user_visible: true,
                retention_class: RetentionClass::Diagnostic,
            },
            DataMovementDestination::provider("llm.openrouter", "chat_completions"),
        )
        .data_classes([DataClass::ProviderDiagnostics])
        .result(DataMovementResult::failed(
            "provider_auth",
            format!("rejected key {fake_key} returned 401 Unauthorized"),
        ))
        .build();

        repo.append_data_movement_event(session_id, &failed)
            .expect("append failed");

        let ledger_bytes = fs::read_to_string(
            repo.data_movement_ledger_path(session_id)
                .expect("ledger path"),
        )
        .expect("read ledger");
        assert!(
            !ledger_bytes.contains(&fake_key),
            "API key leaked into ledger: {ledger_bytes}"
        );

        let loaded = repo
            .load_data_movement_events(session_id)
            .expect("load ledger");
        let event = &loaded[0];
        assert_eq!(event.result.status, MovementStatus::Failed);
        assert_eq!(event.result.error_code.as_deref(), Some("provider_auth"));
        let message = event
            .result
            .error_message_redacted
            .as_deref()
            .expect("redacted message");
        assert!(message.contains("<redacted>"));
        assert!(message.contains("401"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn data_movement_ledger_is_a_session_artifact_for_deletion() {
        let dir = unique_tempdir("data-movement-artifact-descriptor");
        let repo = FileMemoryRepository::with_data_root(&dir);
        let session_id = "session-artifact-ledger";

        let artifacts = repo
            .session_artifacts(session_id)
            .expect("session artifact descriptors");
        assert!(
            artifacts.iter().any(|artifact| matches!(
                (&artifact.kind, &artifact.storage),
                (
                    SessionArtifactKind::DataMovementLedger,
                    SessionArtifactStorage::File { path },
                ) if path.ends_with("session-artifact-ledger.movements.jsonl")
            )),
            "data-movement ledger should be a deletable session artifact: {artifacts:?}"
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
