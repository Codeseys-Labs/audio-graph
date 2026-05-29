//! Runtime log-level control.
//!
//! `env_logger::init()` runs once at startup and respects `RUST_LOG`. After
//! that, the global max level is governed by `log::max_level()` — flipping
//! it at runtime via `log::set_max_level(lvl)` is supported and takes effect
//! immediately for every `log::*!` call in the process.
//!
//! This module exposes:
//!   * [`parse_level`] — turn a user-friendly string ("info", "DEBUG", "off",
//!     unknown garbage) into a [`log::LevelFilter`].
//!   * [`apply_log_level`] — the public entry point the Tauri command + startup
//!     hook both call.

use log::LevelFilter;

/// Parse a case-insensitive level string into a [`LevelFilter`].
///
/// Accepts: "off", "error", "warn", "info", "debug", "trace".
/// Anything else falls back to `Info` — deliberately silent because this is
/// called from user-supplied strings (settings file, IPC command) and noisy
/// failure here would just spam the log we're trying to configure.
pub fn parse_level(s: &str) -> LevelFilter {
    match s.trim().to_ascii_lowercase().as_str() {
        "off" => LevelFilter::Off,
        "error" => LevelFilter::Error,
        "warn" | "warning" => LevelFilter::Warn,
        "info" => LevelFilter::Info,
        "debug" => LevelFilter::Debug,
        "trace" => LevelFilter::Trace,
        _ => LevelFilter::Info,
    }
}

/// Set the global log level at runtime and record the change at info.
///
/// Safe to call any number of times — `log::set_max_level` is a simple
/// atomic store. The info log is emitted *before* the level change so it
/// lands even if the new level is `Off`, giving the user audit evidence in
/// the previous session's log that the change actually took effect.
pub fn apply_log_level(level_str: &str) {
    let lvl = parse_level(level_str);
    log::info!("Applying runtime log level: '{}' → {:?}", level_str, lvl);
    log::set_max_level(lvl);
}

// ===========================================================================
// File logging — a global tee logger (stderr + optional file sink)
// ===========================================================================
//
// We install a single global `log::Log` implementation that formats each
// record like env_logger and writes it to stderr AND, when enabled, to a log
// file. Because it is the process-wide logger, EVERY `log::*!` call anywhere
// in the codebase (and dependencies that use the `log` facade) is captured to
// the file automatically — that is the "log everywhere" guarantee.
//
// The file sink lives behind a `Mutex<Option<_>>` so it can be reconfigured at
// runtime (enable/disable, archive vs overwrite, purge) without re-installing
// the logger (which `log` only allows once).

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

const LOG_FILE_NAME: &str = "audio-graph.log";

/// How the active log file is initialized when file logging starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFileMode {
    /// Rename the previous `audio-graph.log` to `audio-graph-<ts>.log`, then
    /// append to a fresh file. The default — preserves history across runs.
    Archive,
    /// Truncate and reuse the single `audio-graph.log` each launch.
    Overwrite,
}

impl LogFileMode {
    pub fn from_str_or_default(s: Option<&str>) -> Self {
        match s.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            Some("overwrite") => LogFileMode::Overwrite,
            _ => LogFileMode::Archive,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            LogFileMode::Archive => "archive",
            LogFileMode::Overwrite => "overwrite",
        }
    }
}

struct FileSink {
    file: File,
    path: PathBuf,
}
/// Crates whose logs are pure transport/plumbing noise. Capped at WARN so a
/// global debug/trace level (useful for *our* code) doesn't drown the log.
const NOISY_TARGETS: &[&str] = &[
    "tokio_tungstenite",
    "tungstenite",
    "soketto",
    "hyper",
    "hyper_util",
    "h2",
    "reqwest",
    "rustls",
    "tokio_util",
    "mio",
    "want",
    "tao",
    "wry",
    "webview2",
    "tracing",
    // Audio capture backends: per-buffer TRACE spam (e.g. "wasapi::api read
    // 960 frames" was ~99% of the log). Cap to WARN so debug/trace stays usable.
    "wasapi",
    "cpal",
    "symphonia",
    "coreaudio",
];

fn target_cap(target: &str) -> log::LevelFilter {
    if NOISY_TARGETS.iter().any(|n| target.starts_with(n)) {
        log::LevelFilter::Warn.min(log::max_level())
    } else {
        log::max_level()
    }
}

struct AppLogger {
    sink: Mutex<Option<FileSink>>,
}

impl log::Log for AppLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &log::Record) {
        // Per-target cap: noisy dependencies (WebSocket/HTTP/TLS plumbing) are
        // capped at WARN regardless of the global level, so turning the app to
        // debug/trace doesn't bury useful logs under tokio-tungstenite spam.
        if record.level() > target_cap(record.target()) {
            return;
        }
        let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        let line = format!(
            "[{} {:<5} {}] {}\n",
            ts,
            record.level(),
            record.target(),
            record.args()
        );
        // Always mirror to stderr (matches the old env_logger behaviour).
        let _ = std::io::stderr().write_all(line.as_bytes());
        // Tee to the file sink when enabled.
        if let Ok(mut guard) = self.sink.lock() {
            if let Some(sink) = guard.as_mut() {
                let _ = sink.file.write_all(line.as_bytes());
            }
        }
    }

    fn flush(&self) {
        if let Ok(mut guard) = self.sink.lock() {
            if let Some(sink) = guard.as_mut() {
                let _ = sink.file.flush();
            }
        }
    }
}

static LOGGER: OnceLock<AppLogger> = OnceLock::new();

/// Directory that holds log files: `<config_dir>/audio-graph/logs`.
pub fn logs_dir() -> Result<PathBuf, String> {
    let dir = crate::credentials::config_dir()?.join("logs");
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create logs dir: {e}"))?;
    Ok(dir)
}

/// Path of the active log file (whether or not it currently exists).
pub fn log_file_path() -> Result<PathBuf, String> {
    Ok(logs_dir()?.join(LOG_FILE_NAME))
}

/// Install the global tee logger. Call once, as early as possible in `run()`.
///
/// Honors `RUST_LOG` for the initial level (falls back to Info) and starts
/// with file logging ENABLED in Archive mode so startup is always captured;
/// the setup hook later calls [`configure_file_logging`] to apply the user's
/// persisted preference.
pub fn init() {
    let logger = LOGGER.get_or_init(|| AppLogger {
        sink: Mutex::new(None),
    });

    let level = std::env::var("RUST_LOG")
        .ok()
        .map(|s| parse_level(&s))
        .unwrap_or(log::LevelFilter::Info);
    log::set_max_level(level);

    // `set_logger` only succeeds once; ignore the error on repeated calls
    // (e.g. tests) so we degrade gracefully.
    let _ = log::set_logger(logger);

    if let Err(e) = configure_file_logging(true, LogFileMode::Archive) {
        // Can't log via the file yet; stderr still works.
        eprintln!("[logging] file logging unavailable: {e}");
    }
}

/// Enable/disable file logging and (re)open the sink per `mode`.
///
/// Returns the active log file path when enabled, or `None` when disabled.
pub fn configure_file_logging(
    enabled: bool,
    mode: LogFileMode,
) -> Result<Option<PathBuf>, String> {
    let logger = LOGGER.get().ok_or("logger not initialized")?;
    let mut guard = logger
        .sink
        .lock()
        .map_err(|_| "log sink poisoned".to_string())?;

    if !enabled {
        *guard = None;
        return Ok(None);
    }

    let dir = logs_dir()?;
    let path = dir.join(LOG_FILE_NAME);

    if path.exists() && mode == LogFileMode::Archive {
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let archived = dir.join(format!("audio-graph-{stamp}.log"));
        // Best-effort archive; if rename fails we fall through and append.
        let _ = fs::rename(&path, &archived);
    }

    let mut opts = OpenOptions::new();
    opts.create(true).write(true);
    match mode {
        LogFileMode::Archive => {
            opts.append(true);
        }
        LogFileMode::Overwrite => {
            opts.truncate(true);
        }
    }
    let file = opts
        .open(&path)
        .map_err(|e| format!("Failed to open log file {}: {e}", path.display()))?;

    *guard = Some(FileSink {
        file,
        path: path.clone(),
    });
    drop(guard);
    log::info!(
        "File logging enabled ({} mode) → {}",
        mode.as_str(),
        path.display()
    );
    Ok(Some(path))
}

/// Metadata about one log file on disk.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LogFileEntry {
    pub name: String,
    pub size_bytes: u64,
    pub modified_ms: Option<u64>,
    pub is_active: bool,
}

/// Snapshot of the logging state for the Settings UI.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LogInfo {
    pub enabled: bool,
    pub mode: String,
    pub level: String,
    pub dir: String,
    pub active_path: Option<String>,
    pub files: Vec<LogFileEntry>,
}

/// Gather the current logging state + the list of log files on disk.
pub fn log_info(enabled: bool, mode: LogFileMode, level: &str) -> Result<LogInfo, String> {
    let dir = logs_dir()?;
    let active = LOGGER
        .get()
        .and_then(|l| l.sink.lock().ok().and_then(|g| g.as_ref().map(|s| s.path.clone())));

    let mut files = Vec::new();
    if let Ok(read) = fs::read_dir(&dir) {
        for entry in read.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("log") {
                continue;
            }
            let meta = entry.metadata().ok();
            let modified_ms = meta
                .as_ref()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64);
            files.push(LogFileEntry {
                name: path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string(),
                size_bytes: meta.as_ref().map(|m| m.len()).unwrap_or(0),
                modified_ms,
                is_active: active.as_ref() == Some(&path),
            });
        }
    }
    files.sort_by(|a, b| b.modified_ms.cmp(&a.modified_ms));

    Ok(LogInfo {
        enabled,
        mode: mode.as_str().to_string(),
        level: level.to_string(),
        dir: dir.display().to_string(),
        active_path: active.map(|p| p.display().to_string()),
        files,
    })
}

/// Delete every `*.log` file in the logs dir except the currently-open one.
/// Returns the number of files removed.
pub fn purge_logs() -> Result<usize, String> {
    let dir = logs_dir()?;
    let active = LOGGER
        .get()
        .and_then(|l| l.sink.lock().ok().and_then(|g| g.as_ref().map(|s| s.path.clone())));

    let mut removed = 0usize;
    if let Ok(read) = fs::read_dir(&dir) {
        for entry in read.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("log") {
                continue;
            }
            if active.as_ref() == Some(&path) {
                continue; // never delete the open file
            }
            if fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
    }
    log::info!("Purged {removed} archived log file(s)");
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_level_accepts_standard_names() {
        assert_eq!(parse_level("off"), LevelFilter::Off);
        assert_eq!(parse_level("error"), LevelFilter::Error);
        assert_eq!(parse_level("warn"), LevelFilter::Warn);
        assert_eq!(parse_level("info"), LevelFilter::Info);
        assert_eq!(parse_level("debug"), LevelFilter::Debug);
        assert_eq!(parse_level("trace"), LevelFilter::Trace);
    }

    #[test]
    fn parse_level_is_case_insensitive() {
        assert_eq!(parse_level("INFO"), LevelFilter::Info);
        assert_eq!(parse_level("Debug"), LevelFilter::Debug);
        assert_eq!(parse_level("TRACE"), LevelFilter::Trace);
    }

    #[test]
    fn parse_level_trims_whitespace() {
        assert_eq!(parse_level("  warn  "), LevelFilter::Warn);
        assert_eq!(parse_level("\tinfo\n"), LevelFilter::Info);
    }

    #[test]
    fn parse_level_accepts_warning_alias() {
        // Some users / existing config files say "warning" instead of "warn";
        // accept both so we don't silently degrade their preference to Info.
        assert_eq!(parse_level("warning"), LevelFilter::Warn);
        assert_eq!(parse_level("WARNING"), LevelFilter::Warn);
    }

    #[test]
    fn parse_level_falls_back_to_info_on_unknown() {
        assert_eq!(parse_level("verbose"), LevelFilter::Info);
        assert_eq!(parse_level(""), LevelFilter::Info);
        assert_eq!(parse_level("🦀"), LevelFilter::Info);
        assert_eq!(parse_level("42"), LevelFilter::Info);
    }

    #[test]
    fn apply_log_level_updates_max_level() {
        // Drive through a few levels and confirm `log::max_level()` reflects
        // each change. This is the contract the settings UI relies on: the
        // dropdown change becomes the new global ceiling immediately.
        apply_log_level("debug");
        assert_eq!(log::max_level(), LevelFilter::Debug);

        apply_log_level("error");
        assert_eq!(log::max_level(), LevelFilter::Error);

        apply_log_level("off");
        assert_eq!(log::max_level(), LevelFilter::Off);

        // Restore a sensible default so later tests in the same binary
        // aren't silently swallowing logs.
        apply_log_level("info");
        assert_eq!(log::max_level(), LevelFilter::Info);
    }
}
