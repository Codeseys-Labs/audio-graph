//! Lightweight session metadata index for cross-launch continuity.
//!
//! Maintains the user-data-root `sessions.json` — a small JSON array of session
//! descriptors that lets the UI browse past sessions without scanning the
//! transcript / graph directories on disk.
//!
//! The index is a *pointer* to the authoritative data files
//! (`transcripts/<uuid>.jsonl`, `graphs/<uuid>.json`); it is not the data
//! itself. If the index is corrupted or lost, sessions can still be recovered
//! by scanning those directories.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub mod usage;

/// Shared HOME-mutation lock for tests across `sessions` / `sessions::usage` /
/// `state::rotation_tests` etc. These test suites all set HOME to a unique
/// tempdir to isolate `~/.audiograph` writes, which is process-global mutable
/// state — parallel test threads would otherwise clobber each other's HOME.
#[cfg(test)]
pub static TEST_HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Serializes read-modify-write access to `sessions.json` within this process.
///
/// Concurrent writers (e.g. the 30s graph-autosave tick calling `update_stats`
/// at the same instant `finalize_session` runs on shutdown, or an anomaly
/// where two threads race to register) would otherwise risk one overwriting
/// the other's changes because each does load→mutate→save. A process-local
/// mutex is sufficient: only one audio-graph process owns this file.
static INDEX_LOCK: Mutex<()> = Mutex::new(());

pub fn session_id_is_valid(session_id: &str) -> bool {
    !session_id.is_empty()
        && session_id.len() <= 128
        && session_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn validate_session_id(session_id: &str) -> Result<(), String> {
    if session_id.is_empty() || session_id.len() > 128 {
        return Err("Invalid session ID (length)".to_string());
    }
    if !session_id_is_valid(session_id) {
        return Err("Invalid session ID (contains disallowed characters)".to_string());
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub id: String,
    pub title: Option<String>,
    pub created_at: u64,       // unix millis
    pub ended_at: Option<u64>, // unix millis
    pub duration_seconds: Option<u64>,
    pub status: String, // "active" | "complete" | "crashed"
    pub segment_count: u64,
    pub speaker_count: u64,
    pub entity_count: u64,
    pub transcript_path: String,
    pub graph_path: String,
    /// Soft-delete flag. Trashed sessions keep their files on disk but are
    /// hidden from the default list view. `#[serde(default)]` so older
    /// sessions.json files (pre-SessionsBrowser v2) load without migration.
    #[serde(default)]
    pub deleted: bool,
    /// Unix-millis timestamp of when the session was soft-deleted. Used by
    /// `purge_expired_sessions` to hard-delete entries older than the retention
    /// window (30 days). `None` means not deleted or deleted before this field
    /// existed — treat as "just deleted" so it isn't purged immediately.
    #[serde(default)]
    pub deleted_at: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SessionRecoveryReport {
    pub discovered: usize,
    pub recovered: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
}

#[derive(Debug, Default)]
struct RecoveryCandidate {
    id: String,
    transcript_path: Option<PathBuf>,
    transcript_event_path: Option<PathBuf>,
    projection_event_path: Option<PathBuf>,
    notes_path: Option<PathBuf>,
    graph_path: Option<PathBuf>,
    materialized_graph_path: Option<PathBuf>,
    usage_path: Option<PathBuf>,
}

/// User-data-root `sessions.json` (index file, not the data itself).
pub fn sessions_index_path() -> Result<PathBuf, String> {
    crate::user_data::sessions_index_path()
}

/// Read-modify-write-safe index load.
///
/// Distinguishes three outcomes that the lenient [`load_index`] conflates:
/// - file absent (`NotFound`) → `Ok(empty)` — the expected first-run / no-file
///   state, safe to treat as an empty index.
/// - present but malformed JSON → back up the corrupt file (preserving the
///   existing recovery behaviour) and return `Ok(empty)` so the RMW caller can
///   rewrite a fresh index over the already-quarantined corruption.
/// - present but read failed for any OTHER reason (file locked, EIO, permission
///   denied, …) → `Err`. This is the DATA-LOSS guard: a *transient* read error
///   must NOT be mistaken for "no sessions", or the read-modify-write callers
///   (`register_session`, `update_stats`, `finalize_session`, …) would persist
///   a truncated index and clobber every prior session in `sessions.json`.
fn load_index_checked() -> Result<Vec<SessionMetadata>, String> {
    let path = match sessions_index_path() {
        Ok(p) => p,
        Err(e) => return Err(format!("sessions: cannot resolve index path: {}", e)),
    };
    match fs::read_to_string(&path) {
        Ok(contents) => match serde_json::from_str(&contents) {
            Ok(index) => Ok(index),
            Err(e) => {
                log::warn!("sessions: malformed {}: {}", path.display(), e);
                backup_corrupt_index(&path);
                Ok(Vec::new())
            }
        },
        // No file yet is the expected empty state — not an error.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        // ANY other IO error (locked, EIO, permission denied) is transient /
        // unexpected. Returning Err makes RMW callers ABORT instead of
        // overwriting sessions.json with a truncated set (DATA LOSS).
        Err(e) => {
            log::error!(
                "sessions: failed to read index {} ({}); aborting to avoid clobbering it",
                path.display(),
                e
            );
            Err(format!("sessions: read index {}: {}", path.display(), e))
        }
    }
}

/// Lenient index load for read-only callers (UI browse, startup scan,
/// `find_session`). Any failure to read collapses to an empty list — these
/// callers never write the index back, so an empty result cannot cause data
/// loss. Read-modify-write callers must use [`load_index_checked`] instead.
pub fn load_index() -> Vec<SessionMetadata> {
    load_index_checked().unwrap_or_default()
}

fn backup_corrupt_index(path: &Path) {
    let backup = path.with_extension(format!("json.corrupt-{}", now_millis()));
    if let Err(e) = fs::copy(path, &backup) {
        log::warn!(
            "sessions: failed to back up corrupt index {}: {}",
            path.display(),
            e
        );
    } else {
        log::warn!("sessions: backed up corrupt index to {}", backup.display());
    }
}

pub fn save_index(sessions: &[SessionMetadata]) -> Result<(), String> {
    let path = sessions_index_path()?;
    let json = serde_json::to_string_pretty(sessions).map_err(|e| format!("{}", e))?;
    let tmp = path.with_extension("json.tmp");
    write_tmp_synced(&tmp, json.as_bytes())?;
    crate::fs_util::set_owner_only(&tmp);
    fs::rename(&tmp, &path).map_err(|e| format!("{}", e))?;
    crate::fs_util::set_owner_only(&path);
    Ok(())
}

/// Write `bytes` to `tmp` and `fsync` the file before returning.
///
/// `fs::write` + `fs::rename` alone is NOT crash-safe: the rename can be
/// committed to the directory while the file's data blocks are still only in
/// the page cache, so a crash between the two flushes leaves a zero-length (or
/// torn) file replacing the previous known-good one. Calling
/// [`std::fs::File::sync_all`] before the rename forces the data + metadata to
/// stable storage first, so the rename only ever publishes a fully-written
/// file.
fn write_tmp_synced(tmp: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let mut file = fs::File::create(tmp).map_err(|e| format!("create {:?}: {}", tmp, e))?;
    file.write_all(bytes)
        .map_err(|e| format!("write {:?}: {}", tmp, e))?;
    file.sync_all()
        .map_err(|e| format!("sync {:?}: {}", tmp, e))?;
    Ok(())
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Register current session in the index (called at app start).
pub fn register_session(session_id: &str) -> Result<(), String> {
    let _guard = INDEX_LOCK
        .lock()
        .map_err(|e| format!("index lock poisoned: {}", e))?;
    let mut index = load_index_checked()?;
    // Mark any prior "active" sessions (from previous runs that didn't clean
    // up — e.g., SIGKILL, power loss) as "crashed". Skip the CURRENT session
    // id in the unlikely event register_session is called twice for the same
    // ID, which would otherwise cause the second call to self-crash.
    for entry in index.iter_mut() {
        if entry.status == "active" && entry.id != session_id {
            entry.status = "crashed".into();
            if entry.ended_at.is_none() {
                entry.ended_at = Some(now_millis());
            }
        }
    }
    let transcript_path = crate::user_data::transcript_path(session_id)?;
    let graph_path = crate::user_data::graph_path(session_id)?;
    let meta = SessionMetadata {
        id: session_id.to_string(),
        title: None,
        created_at: now_millis(),
        ended_at: None,
        duration_seconds: None,
        status: "active".to_string(),
        segment_count: 0,
        speaker_count: 0,
        entity_count: 0,
        transcript_path: transcript_path.to_string_lossy().to_string(),
        graph_path: graph_path.to_string_lossy().to_string(),
        deleted: false,
        deleted_at: None,
    };
    index.insert(0, meta);
    // Trim to 100 most recent
    if index.len() > 100 {
        index.truncate(100);
    }
    save_index(&index)
}

/// Update stats for current session.
pub fn update_stats(
    session_id: &str,
    segment_count: u64,
    speaker_count: u64,
    entity_count: u64,
) -> Result<(), String> {
    let _guard = INDEX_LOCK
        .lock()
        .map_err(|e| format!("index lock poisoned: {}", e))?;
    let mut index = load_index_checked()?;
    if let Some(entry) = index.iter_mut().find(|e| e.id == session_id) {
        entry.segment_count = segment_count;
        entry.speaker_count = speaker_count;
        entry.entity_count = entity_count;
    }
    save_index(&index)
}

/// Remove a session from the index. Callers are responsible for deleting
/// the transcript/graph files on disk — this only touches the index.
pub fn remove_from_index(session_id: &str) -> Result<(), String> {
    let _guard = INDEX_LOCK
        .lock()
        .map_err(|e| format!("index lock poisoned: {}", e))?;
    let mut index = load_index_checked()?;
    index.retain(|s| s.id != session_id);
    save_index(&index)
}

/// Soft-delete a session: flag `deleted = true` and stamp `deleted_at`, but
/// keep the transcript/graph files on disk. The user can restore it from the
/// trash view, or `purge_expired_sessions` will hard-delete it after 30 days.
pub fn soft_delete_session(session_id: &str) -> Result<(), String> {
    let _guard = INDEX_LOCK
        .lock()
        .map_err(|e| format!("index lock poisoned: {}", e))?;
    let mut index = load_index_checked()?;
    let mut found = false;
    for entry in index.iter_mut() {
        if entry.id == session_id {
            entry.deleted = true;
            entry.deleted_at = Some(now_millis());
            found = true;
            break;
        }
    }
    if !found {
        return Err(format!("session not found: {}", session_id));
    }
    save_index(&index)
}

/// Restore a soft-deleted session: clear the `deleted` flag and `deleted_at`.
/// No-op-equivalent if the session isn't actually deleted, but still succeeds.
pub fn restore_session(session_id: &str) -> Result<(), String> {
    let _guard = INDEX_LOCK
        .lock()
        .map_err(|e| format!("index lock poisoned: {}", e))?;
    let mut index = load_index_checked()?;
    let mut found = false;
    for entry in index.iter_mut() {
        if entry.id == session_id {
            entry.deleted = false;
            entry.deleted_at = None;
            found = true;
            break;
        }
    }
    if !found {
        return Err(format!("session not found: {}", session_id));
    }
    save_index(&index)
}

/// Retention window for trashed sessions: hard-delete trashed entries whose
/// `deleted_at` is older than this. 30 days matches typical OS-level trash
/// behaviour and gives users a generous recovery window.
pub const TRASH_RETENTION_MILLIS: u64 = 30 * 24 * 60 * 60 * 1000;

/// Purge soft-deleted sessions whose `deleted_at` is older than
/// `TRASH_RETENTION_MILLIS`. Removes the index entry and best-effort deletes
/// the transcript + graph files from disk. Entries with `deleted_at = None`
/// are never purged (grace for pre-v2 trash entries that never got stamped).
///
/// Returns the list of purged session IDs so callers can log / report.
pub fn purge_expired_sessions() -> Result<Vec<String>, String> {
    let _guard = INDEX_LOCK
        .lock()
        .map_err(|e| format!("index lock poisoned: {}", e))?;
    let mut index = load_index_checked()?;
    let now = now_millis();
    let mut purged = Vec::new();
    let mut purge_paths = Vec::new();

    index.retain(|entry| {
        if !entry.deleted {
            return true;
        }
        match entry.deleted_at {
            Some(ts) if now.saturating_sub(ts) >= TRASH_RETENTION_MILLIS => {
                purged.push(entry.id.clone());
                purge_paths.push(session_artifact_paths(entry));
                false
            }
            _ => true,
        }
    });

    if purged.is_empty() {
        return Ok(purged);
    }
    save_index(&index)?;

    // Best-effort file cleanup outside the index write — the index is now
    // authoritative regardless of whether unlink succeeds.
    for paths in purge_paths {
        for path in paths {
            let _ = fs::remove_file(path);
        }
    }

    Ok(purged)
}

pub fn find_session(session_id: &str) -> Option<SessionMetadata> {
    load_index()
        .into_iter()
        .find(|entry| entry.id == session_id)
}

pub fn session_file_paths(entry: &SessionMetadata) -> (PathBuf, PathBuf) {
    let transcript = if entry.transcript_path.trim().is_empty() {
        crate::user_data::transcript_path(&entry.id).unwrap_or_else(|_| PathBuf::from(""))
    } else {
        PathBuf::from(&entry.transcript_path)
    };
    let graph = if entry.graph_path.trim().is_empty() {
        crate::user_data::graph_path(&entry.id).unwrap_or_else(|_| PathBuf::from(""))
    } else {
        PathBuf::from(&entry.graph_path)
    };
    (transcript, graph)
}

fn push_artifact_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if path.as_os_str().is_empty() || paths.contains(&path) {
        return;
    }
    paths.push(path);
}

/// Default artifact paths owned by a session in the current storage layout.
pub fn default_session_artifact_paths(session_id: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(path) = crate::user_data::transcript_path(session_id) {
        push_artifact_path(&mut paths, path);
    }
    if let Ok(path) = crate::user_data::transcript_events_path(session_id) {
        push_artifact_path(&mut paths, path);
    }
    if let Ok(path) = crate::user_data::projection_events_path(session_id) {
        push_artifact_path(&mut paths, path);
    }
    if let Ok(path) = crate::user_data::notes_path(session_id) {
        push_artifact_path(&mut paths, path);
    }
    if let Ok(path) = crate::user_data::graph_path(session_id) {
        push_artifact_path(&mut paths, path);
    }
    if let Ok(path) = crate::user_data::materialized_graph_path(session_id) {
        push_artifact_path(&mut paths, path);
    }
    paths
}

/// All known durable files for a session, including legacy index paths and
/// current event/materialized projection artifacts.
pub fn session_artifact_paths(entry: &SessionMetadata) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let (transcript, graph) = session_file_paths(entry);
    push_artifact_path(&mut paths, transcript);
    push_artifact_path(&mut paths, graph);
    for path in default_session_artifact_paths(&entry.id) {
        push_artifact_path(&mut paths, path);
    }
    paths
}

pub fn session_artifact_paths_for_id(session_id: &str) -> Vec<PathBuf> {
    find_session(session_id)
        .as_ref()
        .map(session_artifact_paths)
        .unwrap_or_else(|| default_session_artifact_paths(session_id))
}

fn modified_millis(path: &Path) -> Option<u64> {
    path.metadata()
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as u64)
}

fn collect_candidates_from_dir(
    candidates: &mut HashMap<String, RecoveryCandidate>,
    dir: &Path,
    extension: &str,
    assign: impl Fn(&mut RecoveryCandidate, PathBuf),
) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let mut discovered = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some(extension) {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        if !session_id_is_valid(stem) {
            continue;
        }
        discovered += 1;
        let candidate = candidates
            .entry(stem.to_string())
            .or_insert_with(|| RecoveryCandidate {
                id: stem.to_string(),
                ..RecoveryCandidate::default()
            });
        assign(candidate, path);
    }
    discovered
}

fn collect_candidates_from_dir_by_suffix(
    candidates: &mut HashMap<String, RecoveryCandidate>,
    dir: &Path,
    suffix: &str,
    assign: impl Fn(&mut RecoveryCandidate, PathBuf),
) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let mut discovered = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(filename) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        let Some(session_id) = filename.strip_suffix(suffix) else {
            continue;
        };
        if !session_id_is_valid(session_id) {
            continue;
        }
        discovered += 1;
        let candidate =
            candidates
                .entry(session_id.to_string())
                .or_insert_with(|| RecoveryCandidate {
                    id: session_id.to_string(),
                    ..RecoveryCandidate::default()
                });
        assign(candidate, path);
    }
    discovered
}

fn collect_recovery_candidates() -> (HashMap<String, RecoveryCandidate>, usize) {
    let mut candidates = HashMap::new();
    let mut discovered = 0;
    for root in crate::user_data::recovery_roots() {
        discovered += collect_candidates_from_dir(
            &mut candidates,
            &root.join("transcripts"),
            "jsonl",
            |candidate, path| {
                if candidate.transcript_path.is_none() {
                    candidate.transcript_path = Some(path);
                }
            },
        );
        discovered += collect_candidates_from_dir_by_suffix(
            &mut candidates,
            &root.join("transcripts"),
            ".events.jsonl",
            |candidate, path| {
                if candidate.transcript_event_path.is_none() {
                    candidate.transcript_event_path = Some(path);
                }
            },
        );
        discovered += collect_candidates_from_dir_by_suffix(
            &mut candidates,
            &root.join("projections"),
            ".events.jsonl",
            |candidate, path| {
                if candidate.projection_event_path.is_none() {
                    candidate.projection_event_path = Some(path);
                }
            },
        );
        discovered += collect_candidates_from_dir(
            &mut candidates,
            &root.join("notes"),
            "json",
            |candidate, path| {
                if candidate.notes_path.is_none() {
                    candidate.notes_path = Some(path);
                }
            },
        );
        discovered += collect_candidates_from_dir(
            &mut candidates,
            &root.join("graphs"),
            "json",
            |candidate, path| {
                if candidate.graph_path.is_none() {
                    candidate.graph_path = Some(path);
                }
            },
        );
        discovered += collect_candidates_from_dir_by_suffix(
            &mut candidates,
            &root.join("graphs"),
            ".materialized.json",
            |candidate, path| {
                if candidate.materialized_graph_path.is_none() {
                    candidate.materialized_graph_path = Some(path);
                }
            },
        );
        discovered += collect_candidates_from_dir(
            &mut candidates,
            &root.join("usage"),
            "json",
            |candidate, path| {
                if candidate.usage_path.is_none() {
                    candidate.usage_path = Some(path);
                }
            },
        );
    }
    (candidates, discovered)
}

fn transcript_stats(path: &Path, errors: &mut Vec<String>) -> (u64, u64) {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(e) => {
            errors.push(format!("read transcript {}: {}", path.display(), e));
            return (0, 0);
        }
    };
    let mut segments = 0;
    let mut speakers = HashSet::new();
    for (line_no, line) in contents.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<crate::state::TranscriptSegment>(line) {
            Ok(segment) => {
                segments += 1;
                if let Some(speaker_id) = segment.speaker_id
                    && !speaker_id.trim().is_empty()
                {
                    speakers.insert(speaker_id);
                }
            }
            Err(e) => errors.push(format!(
                "skip malformed transcript line {}:{}: {}",
                path.display(),
                line_no + 1,
                e
            )),
        }
    }
    (segments, speakers.len() as u64)
}

fn transcript_event_stats(path: &Path, errors: &mut Vec<String>) -> (u64, u64) {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(e) => {
            errors.push(format!("read transcript events {}: {}", path.display(), e));
            return (0, 0);
        }
    };
    let mut spans = HashSet::new();
    let mut speakers = HashSet::new();
    for (line_no, line) in contents.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<crate::projections::TranscriptEvent>(line) {
            Ok(event) => {
                spans.insert(event.span_id);
                if let Some(speaker_id) = event.speaker_id
                    && !speaker_id.trim().is_empty()
                {
                    speakers.insert(speaker_id);
                }
            }
            Err(e) => errors.push(format!(
                "skip malformed transcript event line {}:{}: {}",
                path.display(),
                line_no + 1,
                e
            )),
        }
    }
    (spans.len() as u64, speakers.len() as u64)
}

fn graph_entity_count(path: &Path, errors: &mut Vec<String>) -> u64 {
    match crate::graph::temporal::TemporalKnowledgeGraph::load_from_file(path) {
        Ok(graph) => graph.snapshot().stats.total_nodes as u64,
        Err(e) => {
            errors.push(format!("skip malformed graph {}: {}", path.display(), e));
            0
        }
    }
}

fn materialized_graph_entity_count(path: &Path, errors: &mut Vec<String>) -> u64 {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(e) => {
            errors.push(format!("read materialized graph {}: {}", path.display(), e));
            return 0;
        }
    };
    match serde_json::from_str::<crate::projections::MaterializedGraph>(&contents) {
        Ok(graph) => graph.nodes.len() as u64,
        Err(e) => {
            errors.push(format!(
                "skip malformed materialized graph {}: {}",
                path.display(),
                e
            ));
            0
        }
    }
}

fn usage_has_value(path: &Path, errors: &mut Vec<String>) -> bool {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(e) => {
            errors.push(format!("read usage {}: {}", path.display(), e));
            return false;
        }
    };
    match serde_json::from_str::<usage::SessionUsage>(&contents) {
        Ok(usage) => {
            usage.total > 0
                || usage.turns > 0
                || usage.prompt > 0
                || usage.response > 0
                || usage.cached > 0
                || usage.thoughts > 0
                || usage.tool_use > 0
        }
        Err(e) => {
            errors.push(format!("skip malformed usage {}: {}", path.display(), e));
            false
        }
    }
}

fn recovered_metadata(
    candidate: &RecoveryCandidate,
    errors: &mut Vec<String>,
) -> Option<SessionMetadata> {
    let has_session_artifact = candidate.transcript_path.is_some()
        || candidate.transcript_event_path.is_some()
        || candidate.projection_event_path.is_some()
        || candidate.notes_path.is_some()
        || candidate.graph_path.is_some()
        || candidate.materialized_graph_path.is_some();
    if !has_session_artifact {
        if let Some(usage_path) = &candidate.usage_path {
            let _ = usage_has_value(usage_path, errors);
        }
        return None;
    }

    let (segment_count, speaker_count) = candidate
        .transcript_path
        .as_deref()
        .map(|path| transcript_stats(path, errors))
        .or_else(|| {
            candidate
                .transcript_event_path
                .as_deref()
                .map(|path| transcript_event_stats(path, errors))
        })
        .unwrap_or((0, 0));
    let entity_count = candidate
        .graph_path
        .as_deref()
        .map(|path| graph_entity_count(path, errors))
        .or_else(|| {
            candidate
                .materialized_graph_path
                .as_deref()
                .map(|path| materialized_graph_entity_count(path, errors))
        })
        .unwrap_or(0);

    let mut mtimes = Vec::new();
    if let Some(path) = &candidate.transcript_path
        && let Some(ts) = modified_millis(path)
    {
        mtimes.push(ts);
    }
    if let Some(path) = &candidate.graph_path
        && let Some(ts) = modified_millis(path)
    {
        mtimes.push(ts);
    }
    if let Some(path) = &candidate.transcript_event_path
        && let Some(ts) = modified_millis(path)
    {
        mtimes.push(ts);
    }
    if let Some(path) = &candidate.projection_event_path
        && let Some(ts) = modified_millis(path)
    {
        mtimes.push(ts);
    }
    if let Some(path) = &candidate.notes_path
        && let Some(ts) = modified_millis(path)
    {
        mtimes.push(ts);
    }
    if let Some(path) = &candidate.materialized_graph_path
        && let Some(ts) = modified_millis(path)
    {
        mtimes.push(ts);
    }
    if let Some(path) = &candidate.usage_path
        && let Some(ts) = modified_millis(path)
    {
        mtimes.push(ts);
    }

    let created_at = mtimes.iter().copied().min().unwrap_or_else(now_millis);
    let ended_at = mtimes.iter().copied().max();
    let duration_seconds = ended_at.map(|end| end.saturating_sub(created_at) / 1000);
    let transcript_path = candidate
        .transcript_path
        .clone()
        .or_else(|| crate::user_data::transcript_path(&candidate.id).ok())
        .unwrap_or_default();
    let graph_path = candidate
        .graph_path
        .clone()
        .or_else(|| crate::user_data::graph_path(&candidate.id).ok())
        .unwrap_or_default();

    Some(SessionMetadata {
        id: candidate.id.clone(),
        title: None,
        created_at,
        ended_at,
        duration_seconds,
        status: "complete".to_string(),
        segment_count,
        speaker_count,
        entity_count,
        transcript_path: transcript_path.to_string_lossy().to_string(),
        graph_path: graph_path.to_string_lossy().to_string(),
        deleted: false,
        deleted_at: None,
    })
}

pub fn rebuild_index_from_files() -> Result<SessionRecoveryReport, String> {
    let _guard = INDEX_LOCK
        .lock()
        .map_err(|e| format!("index lock poisoned: {}", e))?;
    let mut index = load_index_checked()?;
    let existing_ids: HashSet<String> = index.iter().map(|entry| entry.id.clone()).collect();
    let (candidates, discovered) = collect_recovery_candidates();
    let mut report = SessionRecoveryReport {
        discovered,
        ..SessionRecoveryReport::default()
    };

    for candidate in candidates.values() {
        if existing_ids.contains(&candidate.id) {
            report.skipped += 1;
            continue;
        }
        match recovered_metadata(candidate, &mut report.errors) {
            Some(metadata) => {
                index.push(metadata);
                report.recovered += 1;
            }
            None => report.skipped += 1,
        }
    }

    if report.recovered > 0 {
        index.sort_by_key(|s| std::cmp::Reverse(s.created_at));
        save_index(&index)?;
    }

    Ok(report)
}

/// Mark session as complete on app shutdown.
pub fn finalize_session(session_id: &str) -> Result<(), String> {
    let _guard = INDEX_LOCK
        .lock()
        .map_err(|e| format!("index lock poisoned: {}", e))?;
    let mut index = load_index_checked()?;
    if let Some(entry) = index.iter_mut().find(|e| e.id == session_id) {
        entry.status = "complete".into();
        let end = now_millis();
        entry.ended_at = Some(end);
        // Saturating: a backwards wall clock (NTP step) can make `end` <
        // `created_at`. Raw u64 subtraction would debug-panic / release-wrap to
        // a garbage duration. The autosave + recovery paths already saturate.
        entry.duration_seconds = Some(end.saturating_sub(entry.created_at) / 1000);
    }
    save_index(&index)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
//
// Soft-delete / restore / purge tests. These mutate process-global env so they
// share the same guard pattern and lock used by `sessions::usage::tests` and
// `user_data::tests`.
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn unique_tempdir(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!(
            "audio-graph-sessions-{}-{}-{}-{}",
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
            // SAFETY: serialized by crate::sessions::TEST_HOME_LOCK.
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

    fn make_meta(id: &str) -> SessionMetadata {
        SessionMetadata {
            id: id.to_string(),
            title: None,
            created_at: now_millis(),
            ended_at: None,
            duration_seconds: None,
            status: "complete".to_string(),
            segment_count: 0,
            speaker_count: 0,
            entity_count: 0,
            transcript_path: String::new(),
            graph_path: String::new(),
            deleted: false,
            deleted_at: None,
        }
    }

    #[test]
    fn soft_delete_flags_entry_and_stamps_timestamp() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("soft-delete");
        let _g = HomeGuard::set(&dir);

        save_index(&[make_meta("sid-1")]).expect("seed index");
        let before = now_millis();
        soft_delete_session("sid-1").expect("soft delete");

        let index = load_index();
        let entry = index.iter().find(|e| e.id == "sid-1").expect("found");
        assert!(entry.deleted, "entry must be flagged deleted");
        assert!(
            entry.deleted_at.unwrap() >= before,
            "deleted_at must be set to a recent timestamp"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_clears_deleted_flag() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("restore");
        let _g = HomeGuard::set(&dir);

        save_index(&[make_meta("sid-2")]).expect("seed index");
        soft_delete_session("sid-2").expect("soft delete");
        restore_session("sid-2").expect("restore");

        let index = load_index();
        let entry = index.iter().find(|e| e.id == "sid-2").expect("found");
        assert!(!entry.deleted, "restore must clear deleted flag");
        assert!(entry.deleted_at.is_none(), "restore must clear deleted_at");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn soft_delete_missing_session_errors() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("missing-soft");
        let _g = HomeGuard::set(&dir);

        save_index(&[]).expect("seed empty");
        let err = soft_delete_session("ghost").expect_err("must error");
        assert!(
            err.contains("not found"),
            "error should mention not found: {}",
            err
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn purge_removes_only_expired_trashed_entries() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("purge");
        let _g = HomeGuard::set(&dir);

        let now = now_millis();
        let mut old_trash = make_meta("old-trash");
        old_trash.deleted = true;
        old_trash.deleted_at = Some(now - TRASH_RETENTION_MILLIS - 1000);

        let mut fresh_trash = make_meta("fresh-trash");
        fresh_trash.deleted = true;
        fresh_trash.deleted_at = Some(now - 1000);

        let alive = make_meta("alive");

        let mut pre_v2_trash = make_meta("pre-v2-trash");
        pre_v2_trash.deleted = true;
        pre_v2_trash.deleted_at = None; // never purge entries missing the stamp

        save_index(&[old_trash, fresh_trash, alive, pre_v2_trash]).expect("seed");

        let purged = purge_expired_sessions().expect("purge");
        assert_eq!(purged, vec!["old-trash".to_string()]);

        let remaining: Vec<String> = load_index().into_iter().map(|e| e.id).collect();
        assert_eq!(
            remaining,
            vec![
                "fresh-trash".to_string(),
                "alive".to_string(),
                "pre-v2-trash".to_string(),
            ]
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn purge_removes_all_expired_session_artifacts() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("purge-artifacts");
        let _g = HomeGuard::set(&dir);

        let now = now_millis();
        let mut old_trash = make_meta("old-trash-artifacts");
        old_trash.deleted = true;
        old_trash.deleted_at = Some(now - TRASH_RETENTION_MILLIS - 1000);

        let artifact_paths = default_session_artifact_paths(&old_trash.id);
        for path in &artifact_paths {
            fs::write(path, "{}\n").expect("write session artifact");
            assert!(
                path.exists(),
                "artifact should exist before purge: {path:?}"
            );
        }

        save_index(&[old_trash]).expect("seed");
        let purged = purge_expired_sessions().expect("purge");
        assert_eq!(purged, vec!["old-trash-artifacts".to_string()]);

        for path in artifact_paths {
            assert!(!path.exists(), "purge should remove artifact: {path:?}");
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_session_json_loads_with_deleted_defaulted_to_false() {
        // Pre-SessionsBrowser-v2 files won't have `deleted` / `deleted_at`.
        // `#[serde(default)]` on those fields must let them load cleanly.
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("legacy");
        let _g = HomeGuard::set(&dir);

        let legacy = r#"[{
            "id":"legacy-1",
            "title":null,
            "created_at":1,
            "ended_at":null,
            "duration_seconds":null,
            "status":"complete",
            "segment_count":0,
            "speaker_count":0,
            "entity_count":0,
            "transcript_path":"",
            "graph_path":""
        }]"#;
        let path = sessions_index_path().expect("path");
        fs::write(&path, legacy).unwrap();

        let index = load_index();
        assert_eq!(index.len(), 1);
        assert!(!index[0].deleted);
        assert!(index[0].deleted_at.is_none());

        let _ = fs::remove_dir_all(&dir);
    }

    fn write_transcript(session_id: &str, speaker_id: Option<&str>) -> PathBuf {
        let path = crate::user_data::transcript_path(session_id).expect("transcript path");
        let segment = crate::state::TranscriptSegment {
            id: "seg-1".into(),
            source_id: "test-source".into(),
            speaker_id: speaker_id.map(str::to_string),
            speaker_label: speaker_id.map(str::to_string),
            text: "Recovered transcript".into(),
            start_time: 0.0,
            end_time: 1.0,
            confidence: 0.9,
        };
        let json = serde_json::to_string(&segment).expect("serialize segment");
        fs::write(&path, format!("{json}\n")).expect("write transcript");
        path
    }

    fn write_transcript_events(session_id: &str, speaker_id: Option<&str>) -> PathBuf {
        let path =
            crate::user_data::transcript_events_path(session_id).expect("transcript event path");
        let event = crate::projections::TranscriptEvent {
            span_id: "span-1".into(),
            provider: "test".into(),
            source_id: "test-source".into(),
            provider_item_id: None,
            transcript_segment_id: Some("seg-1".into()),
            speaker_id: speaker_id.map(str::to_string),
            speaker_label: speaker_id.map(str::to_string),
            channel: None,
            text: "Recovered event transcript".into(),
            start_time: 0.0,
            end_time: 1.0,
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
            received_at_ms: now_millis(),
        };
        let json = serde_json::to_string(&event).expect("serialize transcript event");
        fs::write(&path, format!("{json}\n")).expect("write transcript event");
        path
    }

    #[test]
    fn rebuild_index_recovers_orphaned_transcript() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("recover-transcript");
        let _g = HomeGuard::set(&dir);

        let transcript_path = write_transcript("orphan-1", Some("speaker-a"));
        save_index(&[]).expect("seed empty index");

        let report = rebuild_index_from_files().expect("recover");
        assert_eq!(report.recovered, 1);
        assert_eq!(report.skipped, 0);

        let index = load_index();
        assert_eq!(index.len(), 1);
        assert_eq!(index[0].id, "orphan-1");
        assert_eq!(index[0].segment_count, 1);
        assert_eq!(index[0].speaker_count, 1);
        assert_eq!(
            index[0].transcript_path,
            transcript_path.to_string_lossy().to_string()
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rebuild_index_recovers_orphaned_transcript_events() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("recover-transcript-events");
        let _g = HomeGuard::set(&dir);

        write_transcript_events("event-only-1", Some("speaker-event"));
        save_index(&[]).expect("seed empty index");

        let report = rebuild_index_from_files().expect("recover");
        assert_eq!(report.recovered, 1);
        assert_eq!(report.skipped, 0);

        let index = load_index();
        assert_eq!(index.len(), 1);
        assert_eq!(index[0].id, "event-only-1");
        assert_eq!(index[0].segment_count, 1);
        assert_eq!(index[0].speaker_count, 1);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rebuild_index_skips_usage_only_zero_files() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("recover-usage-zero");
        let _g = HomeGuard::set(&dir);

        let usage = usage::SessionUsage {
            session_id: "usage-only".into(),
            ..usage::SessionUsage::default()
        };
        usage::save_usage(&usage).expect("write usage");
        save_index(&[]).expect("seed empty index");

        let report = rebuild_index_from_files().expect("recover");
        assert_eq!(report.recovered, 0);
        assert_eq!(report.skipped, 1);
        assert!(load_index().is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rebuild_index_does_not_duplicate_existing_ids() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("recover-duplicate");
        let _g = HomeGuard::set(&dir);

        write_transcript("existing-1", Some("speaker-a"));
        save_index(&[make_meta("existing-1")]).expect("seed index");

        let report = rebuild_index_from_files().expect("recover");
        assert_eq!(report.recovered, 0);
        assert_eq!(report.skipped, 1);
        assert_eq!(load_index().len(), 1);

        let _ = fs::remove_dir_all(&dir);
    }

    /// Replace the on-disk index file with a *directory* at the same path so
    /// `fs::read_to_string` fails with a non-`NotFound` IO error (reading a
    /// directory as a file). This stands in for any transient read failure
    /// (file locked, EIO) without needing platform-specific fault injection.
    fn make_index_unreadable() -> PathBuf {
        let path = sessions_index_path().expect("index path");
        let _ = fs::remove_file(&path);
        fs::create_dir_all(&path).expect("create dir at index path");
        path
    }

    #[test]
    fn load_index_checked_distinguishes_missing_from_read_error() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("checked-classify");
        let _g = HomeGuard::set(&dir);

        // Missing file → Ok(empty), the expected first-run state.
        assert!(
            load_index_checked().expect("missing must be Ok").is_empty(),
            "absent index must classify as empty, not error"
        );

        // Present-but-unreadable (a directory at the path) → Err, so RMW
        // callers abort rather than treat it as "no sessions".
        let idx_path = make_index_unreadable();
        let err = load_index_checked().expect_err("read error must propagate");
        assert!(
            err.contains("read index"),
            "error must come from the IO-error branch, got: {}",
            err
        );

        let _ = fs::remove_dir_all(&idx_path);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn transient_read_error_does_not_clobber_index() {
        // DATA LOSS guard (finding #50): a transient read failure during a
        // read-modify-write must make the writer ABORT, not overwrite the
        // index with a truncated (empty) set. We simulate the read failure by
        // putting a directory where the index file should be — reads fail, but
        // the path is "present", so it must NOT be mistaken for "no file".
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("no-clobber");
        let _g = HomeGuard::set(&dir);

        let idx_path = make_index_unreadable();

        // Each RMW entry point must surface an error rather than succeed by
        // writing an empty/truncated index over the (unreadable) target.
        assert!(
            register_session("sid-x").is_err(),
            "register_session must abort on a transient read error"
        );
        assert!(
            update_stats("sid-x", 1, 1, 1).is_err(),
            "update_stats must abort on a transient read error"
        );
        assert!(
            finalize_session("sid-x").is_err(),
            "finalize_session must abort on a transient read error"
        );

        // The path must still be the directory we planted — no RMW caller
        // replaced it with a regular (truncated) file.
        assert!(
            idx_path.is_dir(),
            "index path must remain untouched (not clobbered with a truncated file)"
        );

        let _ = fs::remove_dir_all(&idx_path);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn finalize_session_duration_saturates_on_backwards_clock() {
        // finding #51(a): a backwards wall clock (NTP step) can make the
        // finalize timestamp earlier than created_at. Raw u64 subtraction
        // would debug-panic / release-wrap; saturating_sub must clamp the
        // duration to 0 instead of producing garbage.
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("clock-backwards");
        let _g = HomeGuard::set(&dir);

        // Seed a session whose created_at is FAR in the future relative to the
        // wall clock `finalize_session` will read via now_millis().
        let mut meta = make_meta("future-born");
        meta.status = "active".to_string();
        meta.created_at = u64::MAX - 5; // unreachable future → end < created_at
        meta.ended_at = None;
        meta.duration_seconds = None;
        save_index(&[meta]).expect("seed index");

        // Must not panic, even in debug builds where overflow checks are on.
        finalize_session("future-born").expect("finalize must succeed");

        let entry = load_index()
            .into_iter()
            .find(|e| e.id == "future-born")
            .expect("entry present");
        assert_eq!(entry.status, "complete");
        assert_eq!(
            entry.duration_seconds,
            Some(0),
            "backwards-clock duration must saturate to 0, not wrap"
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
