//! I/O helpers for persistence that surface storage-full errors to the UI.
//!
//! Historically the app used `let _ = fs::write(...)` at several transcript
//! and graph persistence sites. That pattern silently dropped data when the
//! user's disk filled up during a long capture, and the only visible symptom
//! was a truncated transcript file after the session ended.
//!
//! [`write_or_emit_storage_full`] replaces those silent writes: on ENOSPC /
//! `ERROR_DISK_FULL` it emits a [`CAPTURE_STORAGE_FULL`] Tauri event (so the
//! frontend can show a user-visible error), logs at `error`, and returns the
//! underlying `io::Error` so the caller can stop the write loop. On any
//! other I/O error it logs at `warn` and returns the error unchanged. On
//! success it returns `Ok(())`.
//!
//! Note: this helper is intentionally narrow — only persistence code paths
//! (transcripts, graph snapshots) use it. Credential writes, session-index
//! writes, and model downloads route through their own error paths since
//! the user can retry those directly.

use std::fs;
use std::path::Path;

use crate::events::{self, CaptureStorageFullPayload};

/// Write `bytes` to `path` (truncating any existing file) and surface
/// storage-full errors via the [`CAPTURE_STORAGE_FULL`] Tauri event.
///
/// Semantics:
/// - `Ok(())` on success.
/// - On [`is_storage_full`](events::is_storage_full) errors: emit
///   `capture-storage-full` with `bytes_written: 0` (we can't easily tell how
///   many bytes landed before the OS gave up on a single `fs::write`) and
///   `bytes_lost: bytes.len()`, log at `error`, return the error.
/// - On any other error: log at `warn` and return the error.
pub fn write_or_emit_storage_full(
    app: &tauri::AppHandle,
    path: &Path,
    bytes: &[u8],
) -> Result<(), std::io::Error> {
    match fs::write(path, bytes) {
        Ok(()) => Ok(()),
        Err(e) => {
            handle_write_error(Some(app), path, 0, bytes.len() as u64, &e);
            Err(e)
        }
    }
}

/// Classify an I/O error from an in-progress write and, if it is a storage-full
/// condition, emit [`CAPTURE_STORAGE_FULL`] and log at `error`. Non-storage
/// errors are logged at `warn`.
///
/// Use this inside writer threads that already own a file handle (e.g. the
/// JSONL transcript appender) and therefore can't hand the payload off to
/// [`write_or_emit_storage_full`] directly.
///
/// `bytes_written` is the best-effort count of bytes that landed on disk
/// before the error; `bytes_lost` is the size of the buffer we were trying to
/// push. Either may be `0` if unknown.
pub(crate) fn handle_write_error(
    app: Option<&tauri::AppHandle>,
    path: &Path,
    bytes_written: u64,
    bytes_lost: u64,
    err: &std::io::Error,
) {
    if events::is_storage_full(err) {
        log::error!(
            "Storage full while writing {:?} ({} bytes lost): {}",
            path,
            bytes_lost,
            err
        );
        if let Some(app) = app {
            events::emit_or_log(
                app,
                events::CAPTURE_STORAGE_FULL,
                CaptureStorageFullPayload {
                    path: path.display().to_string(),
                    bytes_written,
                    bytes_lost,
                },
            );
        } else {
            log::warn!(
                "Storage-full event suppressed — no AppHandle registered yet for {:?}",
                path
            );
        }
    } else {
        log::warn!("Write to {:?} failed: {}", path, err);
    }
}
