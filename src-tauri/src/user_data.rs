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
    if let Some(legacy) = legacy_data_root() {
        if !roots.iter().any(|root| same_path(root, &legacy)) {
            roots.push(legacy);
        }
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

pub fn graphs_dir() -> Result<PathBuf, String> {
    ensure_dir(data_root()?.join("graphs"))
}

pub fn usage_dir() -> Result<PathBuf, String> {
    ensure_dir(data_root()?.join("usage"))
}

pub fn crashes_dir() -> Result<PathBuf, String> {
    ensure_dir(data_root()?.join("crashes"))
}

pub fn transcript_path(session_id: &str) -> Result<PathBuf, String> {
    Ok(transcripts_dir()?.join(format!("{session_id}.jsonl")))
}

pub fn graph_path(session_id: &str) -> Result<PathBuf, String> {
    Ok(graphs_dir()?.join(format!("{session_id}.json")))
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
        let _lock = crate::sessions::TEST_HOME_LOCK.lock().unwrap();
        let dir = unique_tempdir("override");
        let _guard = EnvGuard::set_data_dir(&dir);

        assert_eq!(data_root().unwrap(), dir);
        assert!(transcripts_dir().unwrap().ends_with("transcripts"));
        assert!(graphs_dir().unwrap().ends_with("graphs"));
        assert!(usage_dir().unwrap().ends_with("usage"));
        assert_eq!(recovery_roots(), vec![dir.clone()]);

        let _ = fs::remove_dir_all(&dir);
    }
}
