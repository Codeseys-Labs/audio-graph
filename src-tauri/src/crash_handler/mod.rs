//! Global panic handler.
//!
//! Installs a `std::panic::set_hook` that writes a structured crash report to
//! the AudioGraph crash-log directory whenever any thread panics, and
//! then chains to the default hook for ordinary panics. Credential boundaries
//! record only their operation code and suppress payload-bearing stderr.
//!
//! Design goals:
//!   * Best-effort — never panic from inside the hook.
//!   * Zero new dependencies — use the shared user-data resolver + `std::backtrace`.
//!
//! Call [`install`] exactly once at the very start of the Tauri entry point so
//! panics during startup (Tauri builder, state init, etc.) are captured too.

use std::backtrace::Backtrace;
use std::cell::Cell;
use std::io::Write;
use std::panic::PanicHookInfo;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedactedPanicOp {
    KeychainRead,
    KeychainWrite,
    KeychainDelete,
    LegacyFileRead,
    LegacyFileWrite,
    LegacyStateIo,
    CredentialBoundary,
    LlmClientBuild,
    LlmModelLoad,
    LlmExecutorJob,
}

impl RedactedPanicOp {
    pub fn as_code(&self) -> &'static str {
        match self {
            Self::KeychainRead => "credential.keychain_read",
            Self::KeychainWrite => "credential.keychain_write",
            Self::KeychainDelete => "credential.keychain_delete",
            Self::LegacyFileRead => "credential.legacy_file_read",
            Self::LegacyFileWrite => "credential.legacy_file_write",
            Self::LegacyStateIo => "credential.legacy_state_io",
            Self::CredentialBoundary => "credential.boundary",
            Self::LlmClientBuild => "llm.client_build",
            Self::LlmModelLoad => "llm.model_load",
            Self::LlmExecutorJob => "llm.executor_job",
        }
    }
}

thread_local! {
    static ACTIVE_OP: Cell<Option<RedactedPanicOp>> = const { Cell::new(None) };
}

/// The hook observes the innermost operation before unwinding drops this frame.
/// Enter on the thread performing the credential operation.
pub fn with_redacted_panic_scope<R>(op: RedactedPanicOp, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<RedactedPanicOp>);
    impl Drop for Restore {
        fn drop(&mut self) {
            ACTIVE_OP.set(self.0);
        }
    }

    let _restore = Restore(ACTIVE_OP.replace(Some(op)));
    f()
}

/// Content-free evidence that a redacted operation panicked. The opaque
/// payload never leaves [`catch_redacted_panic`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RedactedPanic;

/// Catch a panic from `f` and dispose of its payload inside `op`'s redaction
/// scope. A payload's destructor may itself panic, so the drop is caught too.
/// That secondary payload is forgotten, because dropping it could panic again
/// and returning it would carry it past the scope.
pub fn catch_redacted_panic<R>(
    op: RedactedPanicOp,
    f: impl FnOnce() -> R,
) -> Result<R, RedactedPanic> {
    with_redacted_panic_scope(op, || {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
            Ok(value) => Ok(value),
            Err(payload) => {
                if let Err(secondary) =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(payload)))
                {
                    std::mem::forget(secondary);
                }
                Err(RedactedPanic)
            }
        }
    })
}

/// Install the global panic hook. Safe to call multiple times, though only the
/// first call has a useful effect — subsequent calls will still chain to the
/// previous hook (which is itself our hook + the default hook).
pub fn install() {
    // Capture the currently-registered hook (typically the default stderr hook)
    // so we can chain to it after writing the crash report.
    let default_hook = std::panic::take_hook();

    std::panic::set_hook(Box::new(move |info| {
        if let Some(op) = ACTIVE_OP.get() {
            // Thread names, source paths, symbolized frames and the previous
            // hook can all expose private data independently of the payload.
            let code = op.as_code();
            let _ = write_report(&format!(
                "AudioGraph crash report\n\nPayload:\n[redacted panic payload: {code}]\n"
            ));
            capture_panic_diagnostic(code);
            return;
        }
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

        // Ordinary analytics events carry only a normalized location marker.
        let event_name = panic_event_name(location.as_ref());
        capture_panic_diagnostic(&event_name);

        // Chain to the default hook so stderr prints still happen.
        default_hook(info);
    }));
}

fn capture_panic_diagnostic(name: &str) {
    crate::analytics::capture_diagnostic(crate::analytics::DiagEvent {
        name,
        category: crate::analytics::Category::Panic,
        level: sentry::Level::Fatal,
        provider: None,
        kind: Some("panic"),
        http_status: None,
        recoverable: Some(false),
    });
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

/// Derive a stable, id-shaped panic event name for the anonymous diagnostic
/// channel from the panic location (file basename + line). The result matches
/// the analytics id shape `^[a-z0-9._:-]{1,48}$` so it survives the tag
/// allowlist; anything else is normalized to `-` or falls back to a constant.
/// NEVER includes the panic payload/message (potential free prose).
fn panic_event_name(location: Option<&(String, u32, u32)>) -> String {
    let Some((file, line, _col)) = location else {
        return "panic.unknown".to_string();
    };
    // Basename without extension, lowercased, non-id chars -> '-'.
    let base = file
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(file)
        .strip_suffix(".rs")
        .unwrap_or_else(|| file.rsplit(['/', '\\']).next().unwrap_or(file));
    let safe: String = base
        .chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let safe = if safe.is_empty() { "unknown" } else { &safe };
    // "panic." + basename + ":" + line, clamped to the 48-char id-shape ceiling.
    let mut name = format!("panic.{safe}:{line}");
    if name.chars().count() > 48 {
        name = name.chars().take(48).collect();
    }
    name
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

/// Write the report to the user-data crash directory. Best effort —
/// returns `Err` (ignored by the caller) if the home dir is unknown, the
/// crashes directory can't be created, or the write fails.
fn write_report(report: &str) -> Result<(), ()> {
    let dir = crashes_dir()?;

    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let mut path = dir.join(format!("{millis}.log"));
    for suffix in 1_u64.. {
        match std::fs::File::create_new(&path) {
            Ok(mut file) => return file.write_all(report.as_bytes()).map_err(|_| ()),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                path = dir.join(format!("{millis}-{suffix}.log"));
            }
            Err(_) => return Err(()),
        }
    }
    Err(())
}

/// User-data-root `crashes/`.
fn crashes_dir() -> Result<std::path::PathBuf, ()> {
    crate::user_data::crashes_dir().map_err(|_| ())
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
    fn scope_restores_previous_operation_on_return_and_unwind() {
        use RedactedPanicOp::*;
        with_redacted_panic_scope(LegacyFileRead, || {
            assert_eq!(ACTIVE_OP.get(), Some(LegacyFileRead));
            let value = with_redacted_panic_scope(LegacyFileWrite, || {
                assert_eq!(ACTIVE_OP.get(), Some(LegacyFileWrite));
                42
            });
            assert_eq!(value, 42);
            assert_eq!(ACTIVE_OP.get(), Some(LegacyFileRead));
            let result = std::panic::catch_unwind(|| {
                with_redacted_panic_scope(LegacyStateIo, || panic!("test unwind"))
            });
            assert!(result.is_err());
            assert_eq!(ACTIVE_OP.get(), Some(LegacyFileRead));
        });
        assert_eq!(ACTIVE_OP.get(), None);
    }

    #[test]
    fn catch_redacted_panic_drops_a_panicking_payload_inside_the_scope() {
        use std::sync::{Arc, Mutex};

        struct PanickingPayload(Arc<Mutex<Option<Option<RedactedPanicOp>>>>);
        impl Drop for PanickingPayload {
            fn drop(&mut self) {
                *self.0.lock().unwrap() = Some(ACTIVE_OP.get());
                panic!("non-secret payload destructor test unwind");
            }
        }

        let observed = Arc::new(Mutex::new(None));
        let payload = PanickingPayload(Arc::clone(&observed));
        let result: Result<(), RedactedPanic> =
            catch_redacted_panic(RedactedPanicOp::KeychainRead, move || {
                std::panic::panic_any(payload)
            });

        assert_eq!(result, Err(RedactedPanic));
        assert_eq!(
            *observed.lock().unwrap(),
            Some(Some(RedactedPanicOp::KeychainRead))
        );
        assert_eq!(ACTIVE_OP.get(), None);
        assert_eq!(
            catch_redacted_panic(RedactedPanicOp::KeychainWrite, || 7),
            Ok(7)
        );
    }

    const CHILD_NONCE: &str = "AUDIOGRAPH_TEST_CRASH_NONCE";
    const ORDINARY_PAYLOAD: &str = "ordinary diagnostic control";

    fn private_fields(nonce: &str) -> Vec<String> {
        let secret = format!("sk-canary-{nonce}");
        vec![
            format!("locator:{nonce}"),
            format!("/private/path/{nonce}"),
            format!("native-target-{nonce}"),
            format!("platform diagnostic {nonce}"),
            format!("exact-length={}", secret.len()),
            format!("fingerprint-canary-{nonce}"),
            secret,
        ]
    }

    #[test]
    #[ignore = "invoked in isolation by concurrent_hook_reports"]
    fn panic_hook_child() {
        let Ok(nonce) = std::env::var(CHILD_NONCE) else {
            return;
        };
        let private = private_fields(&nonce).join(" ");
        install();
        std::thread::Builder::new()
            .name(format!("private-thread-{nonce}"))
            .spawn(move || {
                with_redacted_panic_scope(RedactedPanicOp::CredentialBoundary, || {
                    let ready = std::sync::Barrier::new(2);
                    std::thread::scope(|threads| {
                        let ordinary = threads.spawn(|| {
                            ready.wait();
                            panic!("{ORDINARY_PAYLOAD}");
                        });
                        let result = std::panic::catch_unwind(|| {
                            with_redacted_panic_scope(RedactedPanicOp::KeychainRead, || {
                                ready.wait();
                                panic!("{private}");
                            })
                        });
                        assert!(result.is_err());
                        assert!(ordinary.join().is_err());
                    });
                });
            })
            .unwrap()
            .join()
            .unwrap();

        // Exercise an actual public credential entry point with a panicking
        // reader, before it can make any call to the machine's keychain.
        struct PanickingReader(String);
        impl std::io::Read for PanickingReader {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                panic!("{}", self.0)
            }
        }
        let result = std::panic::catch_unwind(|| {
            crate::credentials::save_typesafe_credential_from_reader(PanickingReader(nonce))
        });
        assert!(result.is_err());
    }

    #[test]
    fn concurrent_hook_reports() {
        let nonce = uuid::Uuid::new_v4().to_string();
        let root = std::env::temp_dir().join(format!("audiograph-crashes-{nonce}"));
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "crash_handler::tests::panic_hook_child",
                "--ignored",
                "--nocapture",
            ])
            .env(crate::user_data::DATA_DIR_ENV, &root)
            .env(CHILD_NONCE, &nonce)
            .env("RUST_BACKTRACE", "0")
            .output()
            .unwrap();
        assert!(output.status.success(), "child status {}", output.status);
        let reports: Vec<_> = std::fs::read_dir(root.join("crashes"))
            .unwrap()
            .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
            .collect();
        std::fs::remove_dir_all(root).unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(reports.len(), 3);
        for code in ["credential.keychain_read", "credential.boundary"] {
            let expected =
                format!("AudioGraph crash report\n\nPayload:\n[redacted panic payload: {code}]\n");
            assert!(
                reports.contains(&expected),
                "missing closed report for {code}"
            );
        }
        assert!(
            reports
                .iter()
                .any(|s| s.contains(ORDINARY_PAYLOAD) && s.contains("Backtrace:"))
        );
        assert!(stderr.contains(ORDINARY_PAYLOAD));
        for field in private_fields(&nonce).iter().chain([&nonce]) {
            assert!(!stderr.contains(field), "private field in child stderr");
            assert!(
                reports.iter().all(|s| !s.contains(field)),
                "private field in report"
            );
        }
    }

    #[test]
    fn panic_event_name_is_id_shaped_and_payload_free() {
        // id shape: ^[a-z0-9._:-]{1,48}$
        let is_id = |s: &str| {
            !s.is_empty()
                && s.chars().count() <= 48
                && s.chars().all(|c| {
                    c.is_ascii_lowercase()
                        || c.is_ascii_digit()
                        || matches!(c, '.' | '_' | ':' | '-')
                })
        };

        // Basic case: basename without extension + line, lowercased.
        let n = panic_event_name(Some(&("src/audio/Capture.rs".to_string(), 288, 4)));
        assert_eq!(n, "panic.capture:288");
        assert!(is_id(&n), "must be id-shaped: {n}");

        // Missing location falls back to a constant id.
        let n = panic_event_name(None);
        assert_eq!(n, "panic.unknown");
        assert!(is_id(&n));

        // Non-id chars (spaces, slashes, weird names) normalize to '-' and stay id-shaped.
        let n = panic_event_name(Some(&("weird name!.rs".to_string(), 1, 1)));
        assert!(is_id(&n), "normalized name must be id-shaped: {n}");

        // Over-long basenames are clamped to the 48-char ceiling.
        let long = format!("{}.rs", "a".repeat(80));
        let n = panic_event_name(Some(&(long, 9, 9)));
        assert!(n.chars().count() <= 48, "must clamp to 48: {n}");
        assert!(is_id(&n));
    }

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
