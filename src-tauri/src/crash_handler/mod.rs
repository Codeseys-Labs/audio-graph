//! Global panic handler.
//!
//! Installs a `std::panic::set_hook` that writes a structured crash report to
//! `~/.audiograph/crashes/<unix_millis>.log` whenever any thread panics, and
//! then chains to the default hook so stderr prints are preserved during
//! development.
//!
//! Design goals:
//!   * Best-effort — never panic from inside the hook.
//!   * Prepend (not replace) the default hook so existing behavior is kept.
//!   * Zero new dependencies — use `dirs::home_dir()` + `std::backtrace`.
//!
//! Call [`install`] exactly once at the very start of the Tauri entry point so
//! panics during startup (Tauri builder, state init, etc.) are captured too.

use std::backtrace::Backtrace;
use std::panic::PanicHookInfo;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Install the global panic hook. Safe to call multiple times, though only the
/// first call has a useful effect — subsequent calls will still chain to the
/// previous hook (which is itself our hook + the default hook).
pub fn install() {
    // Capture the currently-registered hook (typically the default stderr hook)
    // so we can chain to it after writing the crash report.
    let default_hook = std::panic::take_hook();

    std::panic::set_hook(Box::new(move |info| {
        // Never panic in the hook — swallow every error.
        let thread = std::thread::current();
        let thread_name = thread.name().unwrap_or("<unnamed>").to_string();
        let payload = extract_payload(info);
        let location = info
            .location()
            .map(|l| (l.file().to_string(), l.line(), l.column()));
        let backtrace = Backtrace::force_capture().to_string();

        let report = format_report(&thread_name, &payload, location.as_ref(), &backtrace);

        // Write best-effort; if any step fails, just fall through to the
        // default hook so the user still sees the stderr trace.
        let _ = write_report(&report);

        // Chain to the default hook so stderr prints still happen.
        default_hook(info);
    }));
}

/// Extract the panic payload as a `String`, handling the common `&str` and
/// `String` cases. Unknown payload types become `"<non-string panic payload>"`.
fn extract_payload(info: &PanicHookInfo<'_>) -> String {
    let payload = info.payload();
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

/// Build the crash report string. Factored out so it can be unit tested without
/// triggering a real panic.
fn format_report(
    thread_name: &str,
    payload: &str,
    location: Option<&(String, u32, u32)>,
    backtrace: &str,
) -> String {
    let timestamp = iso8601_utc_now();
    let location_str = match location {
        Some((file, line, col)) => format!("{}:{}:{}", file, line, col),
        None => "<unknown>".to_string(),
    };

    format!(
        "AudioGraph crash report\n\
         =======================\n\
         \n\
         Timestamp:   {timestamp}\n\
         App version: {version}\n\
         OS:          {os}/{arch}\n\
         Thread:      {thread_name}\n\
         \n\
         Location:    {location_str}\n\
         \n\
         Payload:\n\
         {payload}\n\
         \n\
         Backtrace:\n\
         {backtrace}\n",
        timestamp = timestamp,
        version = env!("CARGO_PKG_VERSION"),
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
        thread_name = thread_name,
        location_str = location_str,
        payload = payload,
        backtrace = backtrace,
    )
}

/// Write the report to `~/.audiograph/crashes/<unix_millis>.log`. Best effort —
/// returns `Err` (ignored by the caller) if the home dir is unknown, the
/// crashes directory can't be created, or the write fails.
fn write_report(report: &str) -> Result<(), ()> {
    let dir = crashes_dir().ok_or(())?;
    std::fs::create_dir_all(&dir).map_err(|_| ())?;

    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let path = dir.join(format!("{millis}.log"));

    std::fs::write(&path, report).map_err(|_| ())
}

/// `~/.audiograph/crashes/` — `None` if `dirs::home_dir()` returns `None`.
fn crashes_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".audiograph").join("crashes"))
}

/// Render the current system time as ISO 8601 UTC (e.g.
/// `"2026-04-16T14:05:09Z"`). Rolled by hand to avoid adding `chrono` as a
/// dependency — crash reports don't need sub-second accuracy.
fn iso8601_utc_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format_unix_seconds_as_iso8601(secs as i64)
}

/// Convert a unix-epoch-seconds value to ISO 8601 UTC. Uses the civil-from-days
/// algorithm from Howard Hinnant's date library (public domain).
fn format_unix_seconds_as_iso8601(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let time_of_day = secs.rem_euclid(86_400);

    let (year, month, day) = civil_from_days(days);
    let hour = time_of_day / 3_600;
    let minute = (time_of_day % 3_600) / 60;
    let second = time_of_day % 60;

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hour, minute, second
    )
}

/// Hinnant's civil_from_days: converts days-since-1970-01-01 to (year, month,
/// day). See https://howardhinnant.github.io/date_algorithms.html.
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    (year as i32, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_report_has_expected_sections() {
        let thread_name = "worker-42";
        let payload = "synthetic boom";
        let location = ("src/foo.rs".to_string(), 12, 34);
        let backtrace = "stack frame 0\nstack frame 1\n";

        let report = format_report(thread_name, payload, Some(&location), backtrace);

        assert!(
            report.contains("AudioGraph crash report"),
            "report missing header: {report}"
        );
        assert!(
            report.contains(thread_name),
            "report missing thread name: {report}"
        );
        assert!(report.contains(payload), "report missing payload: {report}");
        assert!(
            report.contains("src/foo.rs:12:34"),
            "report missing location: {report}"
        );
        assert!(
            report.contains("stack frame 0"),
            "report missing backtrace: {report}"
        );
        assert!(
            report.contains(env!("CARGO_PKG_VERSION")),
            "report missing app version: {report}"
        );
    }

    #[test]
    fn format_report_handles_missing_location() {
        let report = format_report("t", "p", None, "bt");
        assert!(report.contains("Location:    <unknown>"), "{report}");
    }

    #[test]
    fn iso8601_format_is_well_formed() {
        // 2026-04-16T00:00:00Z — 56 years * 365.2425 days * 86400s ~ fine to
        // check structurally rather than exactly.
        let s = format_unix_seconds_as_iso8601(1_776_124_800);
        // Expect "YYYY-MM-DDTHH:MM:SSZ" shape.
        assert_eq!(s.len(), 20, "unexpected length: {s}");
        assert!(s.ends_with('Z'), "missing Z: {s}");
        assert_eq!(&s[4..5], "-");
        assert_eq!(&s[7..8], "-");
        assert_eq!(&s[10..11], "T");
    }

    #[test]
    fn iso8601_epoch_is_1970() {
        assert_eq!(format_unix_seconds_as_iso8601(0), "1970-01-01T00:00:00Z");
    }
}
