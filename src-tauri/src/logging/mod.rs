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
