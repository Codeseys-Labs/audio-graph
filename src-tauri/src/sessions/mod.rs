//! Lightweight session metadata index for cross-launch continuity.
//!
//! Maintains `~/.audiograph/sessions.json` — a small JSON array of session
//! descriptors that lets the UI browse past sessions without scanning the
//! transcript / graph directories on disk.
//!
//! The index is a *pointer* to the authoritative data files
//! (`transcripts/<uuid>.jsonl`, `graphs/<uuid>.json`); it is not the data
//! itself. If the index is corrupted or lost, sessions can still be recovered
//! by scanning those directories.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
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

/// `~/.audiograph/sessions.json` (index file, not the data itself).
pub fn sessions_index_path() -> Result<PathBuf, String> {
    let base = dirs::home_dir().ok_or("cannot determine home dir")?;
    let dir = base.join(".audiograph");
    fs::create_dir_all(&dir).map_err(|e| format!("{}", e))?;
    Ok(dir.join("sessions.json"))
}

pub fn load_index() -> Vec<SessionMetadata> {
    match sessions_index_path() {
        Ok(path) if path.exists() => match fs::read_to_string(&path) {
            Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
            Err(_) => Vec::new(),
        },
        _ => Vec::new(),
    }
}

pub fn save_index(sessions: &[SessionMetadata]) -> Result<(), String> {
    let path = sessions_index_path()?;
    let json = serde_json::to_string_pretty(sessions).map_err(|e| format!("{}", e))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, &json).map_err(|e| format!("{}", e))?;
    crate::fs_util::set_owner_only(&tmp);
    fs::rename(&tmp, &path).map_err(|e| format!("{}", e))?;
    crate::fs_util::set_owner_only(&path);
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
    let mut index = load_index();
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
    let base = dirs::home_dir().ok_or("home dir")?.join(".audiograph");
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
        transcript_path: base
            .join("transcripts")
            .join(format!("{}.jsonl", session_id))
            .to_string_lossy()
            .to_string(),
        graph_path: base
            .join("graphs")
            .join(format!("{}.json", session_id))
            .to_string_lossy()
            .to_string(),
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
    let mut index = load_index();
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
    let mut index = load_index();
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
    let mut index = load_index();
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
    let mut index = load_index();
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
    let mut index = load_index();
    let now = now_millis();
    let mut purged = Vec::new();

    index.retain(|entry| {
        if !entry.deleted {
            return true;
        }
        match entry.deleted_at {
            Some(ts) if now.saturating_sub(ts) >= TRASH_RETENTION_MILLIS => {
                purged.push(entry.id.clone());
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
    if let Some(home) = dirs::home_dir() {
        let base = home.join(".audiograph");
        for sid in &purged {
            let t = base.join("transcripts").join(format!("{}.jsonl", sid));
            let g = base.join("graphs").join(format!("{}.json", sid));
            let _ = fs::remove_file(&t);
            let _ = fs::remove_file(&g);
        }
    }

    Ok(purged)
}

/// Mark session as complete on app shutdown.
pub fn finalize_session(session_id: &str) -> Result<(), String> {
    let _guard = INDEX_LOCK
        .lock()
        .map_err(|e| format!("index lock poisoned: {}", e))?;
    let mut index = load_index();
    if let Some(entry) = index.iter_mut().find(|e| e.id == session_id) {
        entry.status = "complete".into();
        let end = now_millis();
        entry.ended_at = Some(end);
        entry.duration_seconds = Some((end - entry.created_at) / 1000);
    }
    save_index(&index)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
//
// Soft-delete / restore / purge tests. These mutate HOME so they share the
// same HomeGuard pattern used by `sessions::usage::tests` and serialize via
// a module-local mutex. Tests that mutate HOME in other modules use their
// own lock, so run with `--test-threads=1` if you see flakiness across
// modules (the baseline `cargo test --lib` is fine).
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
    }

    impl HomeGuard {
        #[allow(unsafe_code)]
        fn set(dir: &std::path::Path) -> Self {
            let prev_home = std::env::var("HOME").ok();
            let prev_userprofile = std::env::var("USERPROFILE").ok();
            // SAFETY: serialized by crate::sessions::TEST_HOME_LOCK; usage::tests uses its
            // own lock but does not run concurrently with this module's tests
            // under `cargo test --lib` when `--test-threads=1`. Under default
            // threading the two locks can race on HOME; tolerated because
            // each test restores HOME on drop.
            unsafe {
                std::env::set_var("HOME", dir);
                std::env::set_var("USERPROFILE", dir);
            }
            Self {
                prev_home,
                prev_userprofile,
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
        let _lock = crate::sessions::TEST_HOME_LOCK.lock().unwrap();
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
        let _lock = crate::sessions::TEST_HOME_LOCK.lock().unwrap();
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
        let _lock = crate::sessions::TEST_HOME_LOCK.lock().unwrap();
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
        let _lock = crate::sessions::TEST_HOME_LOCK.lock().unwrap();
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
    fn legacy_session_json_loads_with_deleted_defaulted_to_false() {
        // Pre-SessionsBrowser-v2 files won't have `deleted` / `deleted_at`.
        // `#[serde(default)]` on those fields must let them load cleanly.
        let _lock = crate::sessions::TEST_HOME_LOCK.lock().unwrap();
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
}
