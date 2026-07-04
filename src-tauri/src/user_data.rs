//! User-data path resolution.
//!
//! Secrets intentionally stay in `dirs::config_dir()/audio-graph`; this module
//! owns the non-secret session artifact root used by transcripts, graphs,
//! token usage, crash reports, and the sessions index. The default remains the
//! legacy `~/.audiograph` location so existing installs stay readable while
//! callers stop hand-assembling that path.

use std::fs;
use std::path::{Path, PathBuf};

pub const DATA_DIR_ENV: &str = "AUDIOGRAPH_DATA_DIR";

fn home_data_root() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".audiograph"))
}

fn env_data_root() -> Option<PathBuf> {
    std::env::var_os(DATA_DIR_ENV)
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

fn ensure_dir(path: PathBuf) -> Result<PathBuf, String> {
    fs::create_dir_all(&path).map_err(|e| format!("create {}: {}", path.display(), e))?;
    Ok(path)
}

pub fn data_root() -> Result<PathBuf, String> {
    let root = env_data_root()
        .or_else(home_data_root)
        .ok_or_else(|| "cannot determine AudioGraph data directory".to_string())?;
    ensure_dir(root)
}

pub fn legacy_data_root() -> Option<PathBuf> {
    home_data_root()
}

pub fn recovery_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(root) = data_root() {
        roots.push(root);
    }
    if env_data_root().is_some() {
        return roots;
    }
    if let Some(legacy) = legacy_data_root()
        && !roots.iter().any(|root| same_path(root, &legacy))
    {
        roots.push(legacy);
    }
    roots
}

fn same_path(a: &Path, b: &Path) -> bool {
    a == b || (a.exists() && b.exists() && a.canonicalize().ok() == b.canonicalize().ok())
}

pub fn sessions_index_path() -> Result<PathBuf, String> {
    Ok(data_root()?.join("sessions.json"))
}

pub fn transcripts_dir() -> Result<PathBuf, String> {
    ensure_dir(data_root()?.join("transcripts"))
}

pub fn projections_dir() -> Result<PathBuf, String> {
    ensure_dir(data_root()?.join("projections"))
}

pub fn graphs_dir() -> Result<PathBuf, String> {
    ensure_dir(data_root()?.join("graphs"))
}

pub fn notes_dir() -> Result<PathBuf, String> {
    ensure_dir(data_root()?.join("notes"))
}

pub fn usage_dir() -> Result<PathBuf, String> {
    ensure_dir(data_root()?.join("usage"))
}

pub fn crashes_dir() -> Result<PathBuf, String> {
    ensure_dir(data_root()?.join("crashes"))
}

/// Directory holding per-session data-movement ledger event logs
/// (seed audio-graph-70a3).
pub fn ledgers_dir() -> Result<PathBuf, String> {
    ensure_dir(data_root()?.join("ledgers"))
}

/// Defense-in-depth guard for the session-id path builders below.
///
/// Every resolver that interpolates `session_id` into a file name calls this
/// first, so an id containing `..`, `/`, `\`, or other separators returns
/// `Err` before any path is assembled — even for callers that skip the command
/// layer's own `validate_session_id` (seed audio-graph-62e6). Delegates to the
/// single canonical validator in `sessions` (rejects empty / >128 chars /
/// anything outside `[A-Za-z0-9_-]`) so the rules stay in one place — no
/// circular dependency, since both are modules of the same crate.
fn guard_session_id(session_id: &str) -> Result<(), String> {
    crate::sessions::validate_session_id(session_id)
}

pub fn transcript_path(session_id: &str) -> Result<PathBuf, String> {
    guard_session_id(session_id)?;
    Ok(transcripts_dir()?.join(format!("{session_id}.jsonl")))
}

pub fn transcript_events_path(session_id: &str) -> Result<PathBuf, String> {
    guard_session_id(session_id)?;
    Ok(transcripts_dir()?.join(format!("{session_id}.events.jsonl")))
}

pub fn projection_events_path(session_id: &str) -> Result<PathBuf, String> {
    guard_session_id(session_id)?;
    Ok(projections_dir()?.join(format!("{session_id}.events.jsonl")))
}

pub fn diarization_events_path(session_id: &str) -> Result<PathBuf, String> {
    guard_session_id(session_id)?;
    Ok(transcripts_dir()?.join(format!("{session_id}.speaker.jsonl")))
}

/// Path to a session's data-movement ledger event log (seed audio-graph-70a3).
pub fn data_movement_ledger_path(session_id: &str) -> Result<PathBuf, String> {
    guard_session_id(session_id)?;
    Ok(ledgers_dir()?.join(format!("{session_id}.movements.jsonl")))
}

pub fn graph_path(session_id: &str) -> Result<PathBuf, String> {
    guard_session_id(session_id)?;
    Ok(graphs_dir()?.join(format!("{session_id}.json")))
}

pub fn materialized_graph_path(session_id: &str) -> Result<PathBuf, String> {
    guard_session_id(session_id)?;
    Ok(graphs_dir()?.join(format!("{session_id}.materialized.json")))
}

pub fn notes_path(session_id: &str) -> Result<PathBuf, String> {
    guard_session_id(session_id)?;
    Ok(notes_dir()?.join(format!("{session_id}.json")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn unique_tempdir(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!(
            "audio-graph-user-data-{label}-{}-{n}",
            std::process::id()
        ))
    }

    struct EnvGuard {
        prev_data_dir: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        #[allow(unsafe_code)]
        fn set_data_dir(path: &Path) -> Self {
            let prev_data_dir = std::env::var_os(DATA_DIR_ENV);
            unsafe {
                std::env::set_var(DATA_DIR_ENV, path);
            }
            Self { prev_data_dir }
        }
    }

    impl Drop for EnvGuard {
        #[allow(unsafe_code)]
        fn drop(&mut self) {
            unsafe {
                match &self.prev_data_dir {
                    Some(value) => std::env::set_var(DATA_DIR_ENV, value),
                    None => std::env::remove_var(DATA_DIR_ENV),
                }
            }
        }
    }

    #[test]
    fn env_override_controls_non_secret_data_root() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("override");
        let _guard = EnvGuard::set_data_dir(&dir);

        assert_eq!(data_root().unwrap(), dir);
        assert!(transcripts_dir().unwrap().ends_with("transcripts"));
        assert!(projections_dir().unwrap().ends_with("projections"));
        assert!(graphs_dir().unwrap().ends_with("graphs"));
        assert!(notes_dir().unwrap().ends_with("notes"));
        assert!(usage_dir().unwrap().ends_with("usage"));
        assert!(
            transcript_events_path("session-1")
                .unwrap()
                .ends_with("session-1.events.jsonl")
        );
        assert!(
            projection_events_path("session-1")
                .unwrap()
                .ends_with("session-1.events.jsonl")
        );
        assert!(
            materialized_graph_path("session-1")
                .unwrap()
                .ends_with("session-1.materialized.json")
        );
        assert!(notes_path("session-1").unwrap().ends_with("session-1.json"));
        assert!(ledgers_dir().unwrap().ends_with("ledgers"));
        assert!(
            data_movement_ledger_path("session-1")
                .unwrap()
                .ends_with("session-1.movements.jsonl")
        );
        assert_eq!(recovery_roots(), vec![dir.clone()]);

        let _ = fs::remove_dir_all(&dir);
    }

    /// Defense-in-depth (seed audio-graph-62e6): the session-id path builders
    /// must reject a traversal / separator id with `Err` *before* they assemble
    /// any path, so an unsanitized id can never escape the intended directory.
    /// No env guard needed — the validator runs first and short-circuits before
    /// `data_root()` touches the filesystem.
    #[test]
    fn path_resolvers_reject_traversal_session_ids() {
        let malicious = [
            "../evil",
            "../../etc/passwd",
            "foo/../bar",
            "foo/bar",
            "foo\\bar",
            "..",
            "",
        ];
        for id in malicious {
            assert!(
                data_movement_ledger_path(id).is_err(),
                "data_movement_ledger_path must reject {id:?}"
            );
            assert!(
                transcript_path(id).is_err(),
                "transcript_path must reject {id:?}"
            );
            assert!(
                transcript_events_path(id).is_err(),
                "transcript_events_path must reject {id:?}"
            );
            assert!(
                projection_events_path(id).is_err(),
                "projection_events_path must reject {id:?}"
            );
            assert!(
                diarization_events_path(id).is_err(),
                "diarization_events_path must reject {id:?}"
            );
            assert!(graph_path(id).is_err(), "graph_path must reject {id:?}");
        }
    }

    /// A well-formed id still resolves to the expected file name (the guard
    /// must not disturb the happy path).
    #[test]
    fn path_resolvers_accept_valid_session_ids() {
        let _lock = crate::sessions::TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = unique_tempdir("valid-id");
        let _guard = EnvGuard::set_data_dir(&dir);

        let id = "session_ABC-123";
        assert!(
            transcript_path(id)
                .unwrap()
                .ends_with("session_ABC-123.jsonl")
        );
        assert!(
            data_movement_ledger_path(id)
                .unwrap()
                .ends_with("session_ABC-123.movements.jsonl")
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
