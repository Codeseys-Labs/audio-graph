//! Global panic handler.
//!
//! Installs a `std::panic::set_hook` that writes a structured crash report to
//! the AudioGraph crash-log directory whenever any thread panics, and
//! then chains to the default hook so stderr prints are preserved during
//! development.
//!
//! Design goals:
//!   * Best-effort — never panic from inside the hook.
//!   * Prepend (not replace) the default hook so existing behavior is kept.
//!   * Zero new dependencies — use the shared user-data resolver + `std::backtrace`.
//!
//! Call [`install`] exactly once at the very start of the Tauri entry point so
//! panics during startup (Tauri builder, state init, etc.) are captured too.

use std::backtrace::Backtrace;
use std::cell::Cell;
use std::io::Write;
use std::marker::PhantomData;
use std::panic::PanicHookInfo;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

const CREDENTIAL_BOUNDARY_DIAGNOSTIC: &str = "credential_boundary";

thread_local! {
    static REDACTED_CREDENTIAL_PANIC_DEPTH: Cell<u32> = const { Cell::new(0) };
}

/// Run one credential-owned operation while marking any panic payload as
/// sensitive for the process-wide crash hook.
///
/// This deliberately does not catch or transform an unwind. The owner still
/// decides whether a panic is recoverable; the scope only prevents the hook,
/// which runs before `catch_unwind`, from exporting its payload or contextual
/// paths. The marker is thread-local and nestable, so unrelated panics retain
/// the application's normal diagnostics even while credential work is live.
fn with_redacted_credential_panic_payload<T>(operation: impl FnOnce() -> T) -> T {
    let _scope = RedactedCredentialPanicScope::enter();
    operation()
}

/// Content-free evidence that a credential-owned operation panicked.
///
/// The original opaque payload never crosses this boundary: even destroying a
/// caught payload can run arbitrary `Drop` code and panic again.
#[derive(Debug)]
pub(crate) struct RedactedCredentialPanic;

/// Catch one credential-boundary panic without allowing its opaque payload to
/// outlive the redacted crash-hook scope.
///
/// Rust permits a panic payload's destructor to panic. Destroy the first
/// payload under the same redaction scope and catch that secondary panic too.
/// Its new opaque payload is deliberately forgotten: attempting to destroy it
/// could recurse indefinitely, while returning it would reintroduce the leak
/// this boundary exists to prevent.
pub(crate) fn catch_redacted_credential_panic<T>(
    operation: impl FnOnce() -> T,
) -> Result<T, RedactedCredentialPanic> {
    with_redacted_credential_panic_payload(|| {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation)) {
            Ok(value) => Ok(value),
            Err(payload) => {
                if let Err(secondary_payload) =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        std::mem::drop(payload)
                    }))
                {
                    std::mem::forget(secondary_payload);
                }
                Err(RedactedCredentialPanic)
            }
        }
    })
}

struct RedactedCredentialPanicScope {
    previous_depth: u32,
    // A scope must be restored on the same thread whose hook state it changed.
    _not_send: PhantomData<Rc<()>>,
}

impl RedactedCredentialPanicScope {
    fn enter() -> Self {
        let previous_depth = REDACTED_CREDENTIAL_PANIC_DEPTH.with(|depth| {
            let previous = depth.get();
            let next = previous
                .checked_add(1)
                .expect("credential panic redaction scope overflow");
            depth.set(next);
            previous
        });
        Self {
            previous_depth,
            _not_send: PhantomData,
        }
    }
}

impl Drop for RedactedCredentialPanicScope {
    fn drop(&mut self) {
        // During thread-local destruction `try_with` may fail. There is no
        // subsequent operation on that dying thread to expose, so ignore it.
        let _ = REDACTED_CREDENTIAL_PANIC_DEPTH.try_with(|depth| depth.set(self.previous_depth));
    }
}

fn redact_credential_panic_payload() -> bool {
    // Fail closed if the hook runs during thread-local destruction.
    REDACTED_CREDENTIAL_PANIC_DEPTH
        .try_with(|depth| depth.get() > 0)
        .unwrap_or(true)
}

#[cfg(test)]
pub(crate) fn credential_panic_redaction_active_for_test() -> bool {
    redact_credential_panic_payload()
}

/// Install the global panic hook. Safe to call multiple times, though only the
/// first call has a useful effect — subsequent calls will still chain to the
/// previous hook (which is itself our hook + the default hook).
pub fn install() {
    // Capture the currently-registered hook (typically the default stderr hook)
    // so we can chain to it after writing the crash report.
    let default_hook = std::panic::take_hook();

    std::panic::set_hook(Box::new(move |info| {
        let redact_credential_payload = redact_credential_panic_payload();
        // Never panic in the hook — swallow every error.
        let (thread_name, payload, location, backtrace) = if redact_credential_payload {
            (
                CREDENTIAL_BOUNDARY_DIAGNOSTIC.to_owned(),
                CREDENTIAL_BOUNDARY_DIAGNOSTIC.to_owned(),
                None,
                CREDENTIAL_BOUNDARY_DIAGNOSTIC.to_owned(),
            )
        } else {
            let thread = std::thread::current();
            (
                thread.name().unwrap_or("<unnamed>").to_string(),
                extract_payload(info),
                info.location().map(|location| {
                    (
                        location.file().to_string(),
                        location.line(),
                        location.column(),
                    )
                }),
                Backtrace::force_capture().to_string(),
            )
        };

        let report = format_report(&thread_name, &payload, location.as_ref(), &backtrace);

        // Write best-effort; if any step fails, just fall through to the
        // default hook so the user still sees the stderr trace.
        let _ = write_report(&report);

        // Best-effort anonymous diagnostic (no-op unless analytics is enabled).
        // Only a controlled, id-shaped location marker rides along — never the
        // payload/backtrace (both can carry free prose). `panic_event_name`
        // derives a safe `^[a-z0-9._:-]{1,48}$` id; on failure the scrubber would
        // drop the tag anyway. Never panic in the hook, so this is guarded.
        let event_name = if redact_credential_payload {
            "panic.credential_boundary".to_owned()
        } else {
            panic_event_name(location.as_ref())
        };
        crate::analytics::capture_diagnostic(crate::analytics::DiagEvent {
            name: &event_name,
            category: crate::analytics::Category::Panic,
            level: sentry::Level::Fatal,
            provider: None,
            kind: Some("panic"),
            http_status: None,
            recoverable: Some(false),
        });

        if redact_credential_payload {
            // Passing the original PanicHookInfo to any prior hook would hand
            // it the sensitive payload. Emit one closed notice ourselves.
            let mut stderr = std::io::stderr().lock();
            let _ = writeln!(stderr, "thread panic: {CREDENTIAL_BOUNDARY_DIAGNOSTIC}");
        } else {
            // Ordinary crashes keep the application's existing diagnostics.
            default_hook(info);
        }
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
    let path = dir.join(format!("{millis}.log"));

    std::fs::write(&path, report).map_err(|_| ())
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
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::process::{Command, Output};
    use std::sync::atomic::{AtomicU64, Ordering};

    const PANIC_HOOK_CHILD_MODE: &str = "AUDIOGRAPH_PANIC_HOOK_CHILD_MODE";
    const PANIC_HOOK_CHILD_CANARY: &str = "AUDIOGRAPH_PANIC_HOOK_CHILD_CANARY";
    const PANIC_HOOK_CHILD_ORDINARY_CANARY: &str = "AUDIOGRAPH_PANIC_HOOK_CHILD_ORDINARY_CANARY";

    struct DropPanicsWithCanary(Option<String>);

    impl Drop for DropPanicsWithCanary {
        fn drop(&mut self) {
            if let Some(canary) = self.0.take() {
                std::panic::panic_any(canary);
            }
        }
    }

    fn unique_panic_test_root(label: &str) -> std::path::PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let sequence = COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!(
            "audio-graph-panic-hook-{label}-{}-{sequence}",
            std::process::id()
        ))
    }

    fn spawn_panic_hook_child(mode: &str, canary: &str, root: &std::path::Path) -> Output {
        Command::new(std::env::current_exe().expect("current Rust test executable"))
            .arg("--exact")
            .arg("crash_handler::tests::panic_hook_subprocess_entry")
            .arg("--nocapture")
            .arg("--test-threads=1")
            .env(PANIC_HOOK_CHILD_MODE, mode)
            .env(PANIC_HOOK_CHILD_CANARY, canary)
            .env(crate::user_data::DATA_DIR_ENV, root)
            .env("SENTRY_DSN", "")
            .output()
            .expect("run isolated panic-hook child")
    }

    fn spawn_concurrent_panic_hook_child(
        credential_canary: &str,
        ordinary_canary: &str,
        root: &std::path::Path,
    ) -> Output {
        Command::new(std::env::current_exe().expect("current Rust test executable"))
            .arg("--exact")
            .arg("crash_handler::tests::panic_hook_subprocess_entry")
            .arg("--nocapture")
            .arg("--test-threads=1")
            .env(PANIC_HOOK_CHILD_MODE, "concurrent")
            .env(PANIC_HOOK_CHILD_CANARY, credential_canary)
            .env(PANIC_HOOK_CHILD_ORDINARY_CANARY, ordinary_canary)
            .env(crate::user_data::DATA_DIR_ENV, root)
            .env("SENTRY_DSN", "")
            .output()
            .expect("run concurrent panic-hook child")
    }

    fn crash_reports(root: &std::path::Path) -> String {
        let crash_dir = root.join("crashes");
        let mut reports = String::new();
        for entry in std::fs::read_dir(&crash_dir).expect("child crash directory") {
            let path = entry.expect("crash directory entry").path();
            reports.push_str(&std::fs::read_to_string(path).expect("UTF-8 crash report"));
        }
        reports
    }

    #[test]
    fn panic_hook_subprocess_entry() {
        let Ok(mode) = std::env::var(PANIC_HOOK_CHILD_MODE) else {
            return;
        };
        let canary = std::env::var(PANIC_HOOK_CHILD_CANARY).expect("child panic canary");
        install();
        // Match production order: analytics initializes after AudioGraph's
        // hook. With Sentry's automatic panic integration removed, this must
        // not install a later payload-observing hook.
        crate::analytics::init_if_enabled(true);

        let caught = match mode.as_str() {
            "credential" => catch_unwind(AssertUnwindSafe(|| {
                with_redacted_credential_panic_payload(|| std::panic::panic_any(canary))
            })),
            "credential_payload_drop" => {
                let result = catch_redacted_credential_panic(|| {
                    std::panic::panic_any(DropPanicsWithCanary(Some(canary)))
                });
                assert!(
                    result.is_err(),
                    "credential boundary panic becomes a payload-free marker"
                );
                return;
            }
            "ordinary" => catch_unwind(AssertUnwindSafe(|| std::panic::panic_any(canary))),
            "concurrent" => {
                let ordinary_canary = std::env::var(PANIC_HOOK_CHILD_ORDINARY_CANARY)
                    .expect("ordinary child panic canary");
                let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
                let (ordinary_done_tx, ordinary_done_rx) = std::sync::mpsc::channel();

                let credential_barrier = barrier.clone();
                let credential = std::thread::spawn(move || {
                    with_redacted_credential_panic_payload(|| {
                        credential_barrier.wait();
                        ordinary_done_rx
                            .recv()
                            .expect("ordinary panic completes while credential scope is live");
                        catch_unwind(AssertUnwindSafe(|| std::panic::panic_any(canary)))
                    })
                });
                let ordinary = std::thread::spawn(move || {
                    barrier.wait();
                    let caught =
                        catch_unwind(AssertUnwindSafe(|| std::panic::panic_any(ordinary_canary)));
                    ordinary_done_tx
                        .send(())
                        .expect("credential panic thread remains connected");
                    caught
                });

                assert!(
                    credential.join().expect("credential panic thread").is_err(),
                    "credential child panic remains catchable"
                );
                assert!(
                    ordinary.join().expect("ordinary panic thread").is_err(),
                    "ordinary child panic remains catchable"
                );
                return;
            }
            _ => panic!("unknown panic-hook child mode"),
        };
        assert!(
            caught.is_err(),
            "child panic remains catchable by its owner"
        );
    }

    #[test]
    fn credential_panic_scope_redacts_global_reports_without_weakening_ordinary_panics() {
        let credential_root = unique_panic_test_root("credential");
        let payload_drop_root = unique_panic_test_root("credential-payload-drop");
        let ordinary_root = unique_panic_test_root("ordinary");
        let concurrent_root = unique_panic_test_root("concurrent");
        let credential_canaries = [
            format!("credential-secret-canary-{}", std::process::id()),
            format!("v2/private-locator-canary-{}", std::process::id()),
            format!("/private/path-canary-{}/token", std::process::id()),
            format!("native-provider-prose-canary-{}", std::process::id()),
        ];
        let credential_panic_payload = credential_canaries.join(" :: ");
        let payload_drop_canary = format!("payload-drop-secret-canary-{}", std::process::id());
        let ordinary_canary = format!("ordinary-diagnostic-canary-{}", std::process::id());

        let credential =
            spawn_panic_hook_child("credential", &credential_panic_payload, &credential_root);
        let payload_drop = spawn_panic_hook_child(
            "credential_payload_drop",
            &payload_drop_canary,
            &payload_drop_root,
        );
        let ordinary = spawn_panic_hook_child("ordinary", &ordinary_canary, &ordinary_root);

        assert!(credential.status.success(), "credential child failed");
        assert!(
            payload_drop.status.success(),
            "credential payload-drop child failed"
        );
        assert!(ordinary.status.success(), "ordinary child failed");

        let credential_stderr = String::from_utf8_lossy(&credential.stderr);
        let credential_reports = crash_reports(&credential_root);
        for canary in &credential_canaries {
            assert!(!credential_stderr.contains(canary));
            assert!(!credential_reports.contains(canary));
        }
        assert!(credential_stderr.contains("credential_boundary"));
        assert!(credential_reports.contains("credential_boundary"));

        let payload_drop_stderr = String::from_utf8_lossy(&payload_drop.stderr);
        let payload_drop_reports = crash_reports(&payload_drop_root);
        assert!(!payload_drop_stderr.contains(&payload_drop_canary));
        assert!(!payload_drop_reports.contains(&payload_drop_canary));
        assert!(payload_drop_stderr.contains("credential_boundary"));
        assert!(payload_drop_reports.contains("credential_boundary"));

        let ordinary_stderr = String::from_utf8_lossy(&ordinary.stderr);
        let ordinary_reports = crash_reports(&ordinary_root);
        assert!(ordinary_stderr.contains(&ordinary_canary));
        assert!(ordinary_reports.contains(&ordinary_canary));

        let concurrent = spawn_concurrent_panic_hook_child(
            &credential_panic_payload,
            &ordinary_canary,
            &concurrent_root,
        );
        assert!(concurrent.status.success(), "concurrent child failed");
        let concurrent_stderr = String::from_utf8_lossy(&concurrent.stderr);
        let concurrent_reports = crash_reports(&concurrent_root);
        for canary in &credential_canaries {
            assert!(!concurrent_stderr.contains(canary));
            assert!(!concurrent_reports.contains(canary));
        }
        assert!(concurrent_stderr.contains(&ordinary_canary));
        assert!(concurrent_stderr.contains("credential_boundary"));

        let _ = std::fs::remove_dir_all(credential_root);
        let _ = std::fs::remove_dir_all(payload_drop_root);
        let _ = std::fs::remove_dir_all(ordinary_root);
        let _ = std::fs::remove_dir_all(concurrent_root);
    }

    #[test]
    fn credential_panic_scope_is_nested_thread_local_and_restored() {
        assert!(!credential_panic_redaction_active_for_test());
        with_redacted_credential_panic_payload(|| {
            assert!(credential_panic_redaction_active_for_test());
            with_redacted_credential_panic_payload(|| {
                assert!(credential_panic_redaction_active_for_test());
            });
            assert!(credential_panic_redaction_active_for_test());

            let unrelated_thread = std::thread::spawn(credential_panic_redaction_active_for_test);
            assert!(!unrelated_thread.join().expect("unrelated thread"));
        });
        assert!(!credential_panic_redaction_active_for_test());
    }

    #[test]
    fn sentry_cannot_install_a_later_payload_observing_panic_hook() {
        let manifest_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let manifest = std::fs::read_to_string(manifest_root.join("Cargo.toml"))
            .expect("read AudioGraph Cargo manifest");
        let sentry_start = manifest
            .find("sentry = {")
            .expect("direct Sentry dependency remains declared");
        let sentry_end = manifest[sentry_start..]
            .find("\n] }")
            .map(|offset| sentry_start + offset)
            .expect("Sentry feature list remains closed");
        let sentry_dependency = &manifest[sentry_start..sentry_end];
        assert!(
            !sentry_dependency
                .lines()
                .any(|line| line.trim() == "\"panic\","),
            "Sentry panic integration would observe PanicHookInfo before AudioGraph redaction"
        );

        let lock = std::fs::read_to_string(manifest_root.join("Cargo.lock"))
            .expect("read AudioGraph Cargo lockfile");
        assert!(
            !lock.contains("\nname = \"sentry-panic\"\n"),
            "sentry-panic must not remain in the resolved production graph"
        );
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
