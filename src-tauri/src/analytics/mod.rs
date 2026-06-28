//! Opt-in, anonymous diagnostics analytics (Sentry Rust SDK).
//!
//! This module wires the Sentry Rust SDK as a **runtime-toggleable**,
//! **opt-in**, **anonymous** error/diagnostics channel that helps the
//! maintainer see what goes wrong in real use without leaking any private
//! data. It lives alongside the existing local file-logging toggle
//! ([`crate::logging`]); the two are fully independent — the user may enable
//! either, both, or neither. It is also independent of the local crash handler
//! ([`crate::crash_handler::install`]), which writes local crash reports and is
//! never gated by this setting.
//!
//! ## Privacy invariants (load-bearing)
//!
//! - `send_default_pii = false` — never attach IP, cookies, or request bodies.
//! - The [`before_send`](scrub_event) hook nulls `server_name`, `user`, and
//!   `request`; reduces every free-text field — the event message, `logentry`,
//!   `transaction`, `culprit`, and every exception value — to redaction
//!   sentinels via [`scrub_free_text`], which first runs the text through
//!   [`crate::error::redacted_provider_diagnostic`] (the same scrubber used for
//!   provider error excerpts) to mark secrets, then drops ALL remaining free
//!   prose so interpolated transcript text can never leak; clears `tags`,
//!   `extra`, `logentry.params`, and all breadcrumbs (which could carry
//!   transcript text or credentials); resets the `fingerprint`; scrubs EVERY
//!   stack frame — across exception, thread, and the deprecated top-level
//!   stacktraces — down to basename paths with `vars`/`context_line`/
//!   `pre_context`/`post_context` cleared (see [`scrub_frames`]); and keeps only
//!   the non-identifying OS / device / Rust contexts (the exception `type` is
//!   kept for triage).
//! - The [`before_breadcrumb`](scrub_breadcrumb) hook drops every breadcrumb so
//!   nothing the SDK auto-collects survives.
//! - [`capture_message`] / [`capture_anonymous_event`] are the **only**
//!   intentional send paths and must be used sparingly — NEVER with transcript,
//!   audio, or credential data.
//!
//! ## Toggle semantics
//!
//! "enabled/disabled" is modelled on the **process hub**
//! ([`Hub::main`](sentry::Hub::main)) — the template every thread-local hub is
//! cloned from — so a toggle is visible to threads spawned afterward:
//!
//! - **Startup** ([`init_if_enabled`]): the client is initialized only when the
//!   persisted setting is `true`. The guard is stored in a module-static and
//!   the bound `Arc<Client>` is captured for runtime control.
//! - **Runtime ON** ([`set_analytics_enabled_runtime`]`(true)`): if a live
//!   client exists, rebinds it on [`Hub::main`](sentry::Hub::main); otherwise
//!   the caller first calls [`init_if_enabled`]`(true)` to init a FRESH client
//!   (a prior OFF closed the transport — see below).
//! - **Runtime OFF** ([`set_analytics_enabled_runtime`]`(false)`): unbinds the
//!   client on [`Hub::main`](sentry::Hub::main) AND calls `client.close(..)` to
//!   shut down the shared transport — a thread-global kill, since every
//!   thread's hub holds a clone of the same `Arc<Client>`. The guard is then
//!   dropped (close is terminal), so a later ON re-inits a fresh client.
//!
//! Note: `close` on OFF is what makes the kill thread-global; it does NOT rely
//! on `Drop` of the static guard at process exit (Rust does not run `Drop` for
//! `static`s at normal termination), so do not assume guaranteed flush-on-exit
//! of the last buffered event.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde::Serialize;

/// How long [`set_analytics_enabled_runtime`]`(false)` waits for the transport
/// to flush in-flight events when closing the client on OFF.
const CLOSE_TIMEOUT: Duration = Duration::from_millis(500);

/// The default Sentry DSN. A DSN is a **client-side public key** that only
/// authorizes *sending* events to a project — it is NOT a secret and is safe to
/// embed. Override at runtime via the `SENTRY_DSN` environment variable (e.g.
/// to point at a self-hosted relay or to disable by supplying an empty value).
const DEFAULT_DSN: &str = "https://1e39b03ea3018d02551500bf428306b9@o4511644093448192.ingest.us.sentry.io/4511644102885381";

/// Process-lifetime holder for the Sentry client guard. Holding the guard keeps
/// the client alive so buffered events flush on exit; runtime ON/OFF only
/// binds/unbinds it on the hub rather than dropping it.
static GUARD: OnceLock<Mutex<Option<sentry::ClientInitGuard>>> = OnceLock::new();

/// Captured `Arc<Client>` from the moment of init, so OFF can close the
/// transport at the client level (a thread-global kill, since every thread's
/// hub holds a clone of this same `Arc<Client>`). Cleared on OFF; a subsequent
/// ON re-inits a fresh client. Separate from [`GUARD`] (which owns
/// lifetime/flush) because the hub holds an `Arc` to the client, not the guard.
static CLIENT: OnceLock<Mutex<Option<Arc<sentry::Client>>>> = OnceLock::new();

fn guard_cell() -> &'static Mutex<Option<sentry::ClientInitGuard>> {
    GUARD.get_or_init(|| Mutex::new(None))
}

fn client_cell() -> &'static Mutex<Option<Arc<sentry::Client>>> {
    CLIENT.get_or_init(|| Mutex::new(None))
}

/// Resolve the effective DSN: `SENTRY_DSN` env override, else the embedded
/// default. An explicitly-empty `SENTRY_DSN` yields `None` (analytics stays a
/// no-op even if "enabled"), which is a convenient kill switch.
fn resolve_dsn() -> Option<String> {
    match std::env::var("SENTRY_DSN") {
        Ok(v) if v.trim().is_empty() => None,
        Ok(v) => Some(v),
        Err(_) => Some(DEFAULT_DSN.to_string()),
    }
}

/// Whether a DSN is configured (env override non-empty, or the embedded
/// default). Surfaced to the UI so it can explain why analytics may be inert.
fn dsn_configured() -> bool {
    resolve_dsn().is_some()
}

/// Build the anonymized [`sentry::ClientOptions`].
///
/// `..Default::default()` is used for forward-compatibility with new 0.48.x
/// fields. `send_default_pii` is forced `false` (the SDK doc examples set it
/// `true`; we override because this channel is anonymous).
fn client_options() -> sentry::ClientOptions {
    sentry::ClientOptions {
        dsn: resolve_dsn().and_then(|d| d.parse().ok()),
        // ANONYMOUS: never attach IP / cookies / request bodies.
        send_default_pii: false,
        // We are not doing release-health; avoid emitting session envelopes.
        auto_session_tracking: false,
        release: sentry::release_name!(),
        environment: Some(
            if cfg!(debug_assertions) {
                "development"
            } else {
                "production"
            }
            .into(),
        ),
        before_send: Some(Arc::new(scrub_event)),
        before_breadcrumb: Some(Arc::new(scrub_breadcrumb)),
        ..Default::default()
    }
}

/// Initialize the Sentry client iff `enabled`, storing the guard in the
/// process-lifetime static. Idempotent: a second call while already inited is a
/// no-op (the existing guard is kept).
pub fn init_if_enabled(enabled: bool) {
    if !enabled {
        return;
    }
    let cell = guard_cell();
    let mut slot = match cell.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    let mut client_slot = match client_cell().lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    if slot.is_some() && client_slot.is_some() {
        // Already initialized with a live client — keep the existing guard.
        return;
    }
    // Fresh init (first time, or re-enable after a prior OFF closed the
    // transport). `sentry::init` binds the client to the current hub; capture
    // the bound `Arc<Client>` so OFF can later close it at the client level.
    let guard = sentry::init(client_options());
    *client_slot = sentry::Hub::current().client();
    *slot = Some(guard);
}

/// Toggle analytics at runtime by binding/unbinding the client on the current
/// hub. Turning **ON** rebinds (or, if never inited, the caller must call
/// [`init_if_enabled`]`(true)` first — see [`crate::commands::set_analytics_enabled`]).
/// Turning **OFF** unbinds the client so no further events are sent, WITHOUT
/// dropping the guard (so flush-on-exit and cheap re-enable still work).
pub fn set_analytics_enabled_runtime(enabled: bool) {
    // Bind/unbind on the PROCESS hub, not `Hub::current()`. The process hub is
    // the template `Hub::new_from_top` clones for each new thread-local hub, so
    // mutating it (rather than only the calling thread's hub) is what makes the
    // toggle visible to threads spawned after this point.
    let hub = sentry::Hub::main();
    if enabled {
        let client_slot = match client_cell().lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(client) = client_slot.as_ref() {
            hub.bind_client(Some(Arc::clone(client)));
        }
    } else {
        // Unbind on the process hub (covers not-yet-materialized threads + the
        // init thread)...
        hub.bind_client(None);
        // ...then close the client transport. This is the load-bearing,
        // thread-global step: every thread's hub holds a clone of this same
        // `Arc<Client>` sharing one transport slot, so closing it stops sends
        // from worker/audio/panic-thread hubs that already snapshotted it.
        let mut client_slot = match client_cell().lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(client) = client_slot.take() {
            client.close(Some(CLOSE_TIMEOUT));
        }
        // Drop the guard: `close` is terminal, so a later ON must re-init.
        let mut guard_slot = match guard_cell().lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        *guard_slot = None;
    }
}

/// Placeholder substituted for any free-form diagnostic prose. See
/// [`scrub_free_text`] for why prose is dropped wholesale.
const OMITTED_MARKER: &str = "<redacted: diagnostic text omitted for privacy>";

/// Scrub a free-form diagnostic string for the anonymous channel.
///
/// `before_send` text fields (the event message and exception values) are the
/// single highest transcript-leak risk: app code routinely interpolates
/// arbitrary runtime strings — which can include transcript text, file paths,
/// or user input — into error messages, and that prose is indistinguishable
/// from a transcript line by any pattern matcher. So we take a two-stage,
/// privacy-maximal approach:
///
/// 1. Run the text through [`crate::error::redacted_provider_diagnostic`], the
///    same scrubber used for provider error excerpts, which replaces known
///    secret shapes (API keys, bearer tokens, AWS keys, URL userinfo, …) with
///    the `<redacted>` sentinel.
/// 2. Drop ALL remaining free prose, keeping only the redaction sentinels that
///    step 1 produced (so a reviewer can still see *that* a secret was present
///    and where, without the surrounding — potentially private — text). When
///    no sentinel survives, the field collapses to [`OMITTED_MARKER`].
///
/// This guarantees no transcript, file path, or other free text leaves the
/// machine while preserving the structural signal (exception type, stack
/// frames, and "a secret appeared here").
fn scrub_free_text(s: &str) -> String {
    let secret_scrubbed = crate::error::redacted_provider_diagnostic(s, std::iter::empty::<&str>());
    // Keep only the redaction sentinels; discard every prose segment between
    // them. `<redacted>` is the sentinel emitted by the error-module scrubber.
    let sentinel = "<redacted>";
    let sentinels = secret_scrubbed.matches(sentinel).count();
    if sentinels == 0 {
        OMITTED_MARKER.to_string()
    } else {
        // Re-emit just the sentinels (joined) so secrets show as `<redacted>`
        // and nothing else (including transcript prose) survives.
        std::iter::repeat_n(sentinel, sentinels)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Scrub a slice of stack [`Frame`](sentry::protocol::Frame)s in place so they
/// carry no private data: reduce `abs_path`/`filename` to basenames (absolute
/// developer/build paths can embed usernames) and clear every free-text source
/// field — `vars` (local variable captures can hold transcript text or
/// credentials), `context_line`, and the surrounding `pre_context`/
/// `post_context` source snippets. Applied uniformly to exception, thread, and
/// the deprecated top-level stacktrace frames so no frame source escapes.
fn scrub_frames(frames: &mut [sentry::protocol::Frame]) {
    for frame in frames.iter_mut() {
        frame.abs_path = frame.abs_path.as_deref().map(basename);
        frame.filename = frame.filename.as_deref().map(basename);
        // Source/locals can carry transcript text or credentials — drop them.
        frame.vars.clear();
        frame.context_line = None;
        frame.pre_context.clear();
        frame.post_context.clear();
    }
}

/// `before_send` scrubber. Strips identity (`server_name`/`user`/`request`),
/// reduces every free-text field (message, `logentry`, `transaction`,
/// `culprit`, and every exception value) to redaction sentinels via
/// [`scrub_free_text`] (dropping all free prose so no transcript can leak),
/// clears `tags`/`extra`/`logentry.params`/breadcrumbs, resets the
/// `fingerprint`, scrubs every stack frame across exception, thread, and the
/// deprecated top-level stacktraces via [`scrub_frames`] (basename paths + clear
/// vars/source), and keeps only the non-identifying OS / device / Rust contexts.
fn scrub_event(
    mut event: sentry::protocol::Event<'static>,
) -> Option<sentry::protocol::Event<'static>> {
    // Strip identity / network metadata.
    event.server_name = None;
    event.user = None;
    event.request = None;

    // Reduce the top-level message to sentinels only (no free prose).
    if let Some(msg) = event.message.take() {
        event.message = Some(scrub_free_text(&msg));
    }

    // Reduce the structured log entry: scrub its (potentially interpolated)
    // message and drop its positional params (free-form `Value`s that can carry
    // transcript text or credentials).
    if let Some(logentry) = event.logentry.as_mut() {
        logentry.message = scrub_free_text(&logentry.message);
        logentry.params.clear();
    }

    // Reduce other free-text identifiers to sentinels (these can be set to
    // interpolated runtime strings via scope/transactions).
    if let Some(transaction) = event.transaction.take() {
        event.transaction = Some(scrub_free_text(&transaction));
    }
    if let Some(culprit) = event.culprit.take() {
        event.culprit = Some(scrub_free_text(&culprit));
    }

    // Tags are free-form key/value text — clear the whole map. Reset the
    // fingerprint to the default (a custom fingerprint can encode free prose).
    event.tags.clear();
    event.fingerprint = Default::default();

    // Reduce every exception value + scrub its stack frames (basename paths and
    // clear vars/source). The exception `type` is kept untouched for triage.
    for exception in event.exception.values.iter_mut() {
        if let Some(value) = exception.value.take() {
            exception.value = Some(scrub_free_text(&value));
        }
        if let Some(stacktrace) = exception.stacktrace.as_mut() {
            scrub_frames(&mut stacktrace.frames);
        }
    }

    // Scrub the deprecated top-level stacktrace's frames the same way.
    if let Some(stacktrace) = event.stacktrace.as_mut() {
        scrub_frames(&mut stacktrace.frames);
    }

    // Scrub every thread's stacktrace frames (NOT covered by the exception loop;
    // `attach_stacktrace` ships these with absolute paths + locals).
    for thread in event.threads.values.iter_mut() {
        if let Some(stacktrace) = thread.stacktrace.as_mut() {
            scrub_frames(&mut stacktrace.frames);
        }
    }

    // Drop anything that could carry transcript text or credentials.
    event.extra.clear();
    event.breadcrumbs.values.clear();

    // Keep only safe, non-identifying contexts (OS / device / runtime).
    event
        .contexts
        .retain(|key, _| matches!(key.as_str(), "os" | "device" | "rust" | "runtime"));

    Some(event)
}

/// `before_breadcrumb` scrubber: drop every breadcrumb. The SDK can
/// auto-collect breadcrumbs (e.g. log records) that may contain transcript
/// text or credentials, so none are allowed to survive.
fn scrub_breadcrumb(_breadcrumb: sentry::Breadcrumb) -> Option<sentry::Breadcrumb> {
    None
}

/// Reduce a filesystem path to its basename so absolute developer/build paths
/// (which can embed usernames) never leave the machine.
fn basename(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

/// Capture an anonymous diagnostic message at the given level. Use sparingly.
/// NEVER pass transcript, audio, or credential data — the message still passes
/// through [`scrub_event`], but callers must treat this as best-effort defense,
/// not license to send private data.
pub fn capture_message(message: &str, level: sentry::Level) {
    sentry::capture_message(message, level);
}

/// Capture a named anonymous event at [`Info`](sentry::Level::Info) level (e.g.
/// a startup ping). Same NEVER-private-data rule as [`capture_message`].
pub fn capture_anonymous_event(name: &str) {
    capture_message(name, sentry::Level::Info);
}

/// UI-facing analytics status. `pii_disabled` is always `true` (a structural
/// invariant of this module: `send_default_pii` is forced `false`).
#[derive(Debug, Clone, Serialize)]
pub struct AnalyticsInfo {
    /// Whether anonymous analytics is currently enabled.
    pub enabled: bool,
    /// Whether a DSN is configured (embedded default or `SENTRY_DSN` override).
    pub dsn_configured: bool,
    /// Always `true` — `send_default_pii` is forced off.
    pub pii_disabled: bool,
}

/// Build the current [`AnalyticsInfo`] for the given enabled state.
pub fn analytics_info(enabled: bool) -> AnalyticsInfo {
    AnalyticsInfo {
        enabled,
        dsn_configured: dsn_configured(),
        pii_disabled: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentry::protocol::{
        Breadcrumb, Event, Exception, Frame, LogEntry, Map, Stacktrace, Thread, User, Value,
    };

    // Load-bearing privacy gate: an event carrying a fake secret + fake
    // transcript line planted into EVERY free-text / source-bearing field —
    // message, logentry (message + params), transaction, culprit, tags,
    // fingerprint, exception value, an exception-frame's vars/context_line, the
    // deprecated top-level stacktrace, AND a threads[].stacktrace frame
    // (vars + context_line + abs_path) — plus user/IP/server_name + extra +
    // breadcrumb, must come out the other side of `scrub_event` with the secret
    // and transcript GONE everywhere, identity nulled, tags/extra/breadcrumbs
    // empty, and thread/top-level frame paths basenamed. This is the proof that
    // "anonymous" holds (the verdict-3 probe, hardened into the gate).
    #[test]
    fn scrub_event_strips_secret_transcript_and_identity() {
        const SECRET: &str = "sk-test-supersecret-credential-12345";
        const TRANSCRIPT: &str = "patient said their social security number aloud";

        let mut event: Event<'static> = Event {
            message: Some(format!("boom: token={SECRET} transcript=\"{TRANSCRIPT}\"")),
            server_name: Some("alices-macbook.local".into()),
            ..Default::default()
        };

        // Identity / network metadata.
        event.user = Some(User {
            id: Some("user-42".to_string()),
            email: Some("alice@example.com".to_string()),
            ip_address: Some(sentry::protocol::IpAddress::Exact(
                "203.0.113.7".parse().unwrap(),
            )),
            ..Default::default()
        });

        // Structured log entry: interpolated message + positional params.
        event.logentry = Some(LogEntry {
            message: format!("logentry: {SECRET} / {TRANSCRIPT}"),
            params: vec![Value::from(SECRET), Value::from(TRANSCRIPT)],
        });

        // Free-text identifiers that scope/transactions can set.
        event.transaction = Some(format!("txn {SECRET} {TRANSCRIPT}"));
        event.culprit = Some(format!("culprit {SECRET} {TRANSCRIPT}"));

        // Free-form tags + a custom fingerprint encoding prose.
        event
            .tags
            .insert("transcript".to_string(), TRANSCRIPT.to_string());
        event.tags.insert("api_key".to_string(), SECRET.to_string());
        event.fingerprint = vec![std::borrow::Cow::Owned(format!("{SECRET}-{TRANSCRIPT}"))].into();

        // Exception carrying both the secret and the transcript, plus a frame
        // whose abs_path embeds a username AND whose vars/context_line carry
        // private data.
        let mut exc_vars: Map<String, Value> = Map::new();
        exc_vars.insert("heard".to_string(), Value::from(TRANSCRIPT));
        exc_vars.insert("key".to_string(), Value::from(SECRET));
        event.exception.values.push(Exception {
            ty: "RuntimeError".to_string(),
            value: Some(format!("failed with {SECRET}; heard: {TRANSCRIPT}")),
            stacktrace: Some(Stacktrace {
                frames: vec![Frame {
                    abs_path: Some("/Users/alice/secret-project/src/main.rs".to_string()),
                    filename: Some("/Users/alice/secret-project/src/main.rs".to_string()),
                    context_line: Some(format!("let x = \"{SECRET}\"; // {TRANSCRIPT}")),
                    pre_context: vec![format!("// {TRANSCRIPT}")],
                    post_context: vec![format!("// {SECRET}")],
                    vars: exc_vars,
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        });

        // Deprecated top-level stacktrace with a private frame.
        event.stacktrace = Some(Stacktrace {
            frames: vec![Frame {
                abs_path: Some("/home/alice/work/top.rs".to_string()),
                filename: Some("/home/alice/work/top.rs".to_string()),
                context_line: Some(format!("top {SECRET} {TRANSCRIPT}")),
                ..Default::default()
            }],
            ..Default::default()
        });

        // Thread stacktrace frame with vars + context_line + abs_path that the
        // exception loop does NOT reach.
        let mut thread_vars: Map<String, Value> = Map::new();
        thread_vars.insert("buf".to_string(), Value::from(TRANSCRIPT));
        thread_vars.insert("token".to_string(), Value::from(SECRET));
        event.threads.values.push(Thread {
            stacktrace: Some(Stacktrace {
                frames: vec![Frame {
                    abs_path: Some("/home/alice/audio/worker.rs".to_string()),
                    filename: Some("/home/alice/audio/worker.rs".to_string()),
                    context_line: Some(format!("emit({SECRET}, {TRANSCRIPT})")),
                    vars: thread_vars,
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        });

        // Extra + breadcrumb that could carry private data.
        let mut extra: Map<String, Value> = Map::new();
        extra.insert("transcript".to_string(), Value::from(TRANSCRIPT));
        extra.insert("api_key".to_string(), Value::from(SECRET));
        event.extra = extra;

        event.breadcrumbs.values.push(Breadcrumb {
            message: Some(format!("user typed {SECRET} / {TRANSCRIPT}")),
            ..Default::default()
        });

        let scrubbed = scrub_event(event).expect("event should not be dropped");

        // Serialize the whole event and assert nothing leaks anywhere.
        let json = serde_json::to_string(&scrubbed).expect("event serializes");
        assert!(
            !json.contains(SECRET),
            "secret leaked through scrubber: {json}"
        );
        assert!(
            !json.contains(TRANSCRIPT),
            "transcript leaked through scrubber: {json}"
        );
        assert!(
            !json.contains("alice@example.com"),
            "user email leaked: {json}"
        );
        assert!(!json.contains("203.0.113.7"), "IP leaked: {json}");
        assert!(
            !json.contains("alices-macbook.local"),
            "server_name leaked: {json}"
        );
        assert!(
            !json.contains("/Users/alice"),
            "absolute frame path leaked: {json}"
        );
        assert!(
            !json.contains("/home/alice"),
            "absolute thread/top-level frame path leaked: {json}"
        );

        // The redaction sentinel must be present where the secret/transcript was.
        assert!(scrubbed.message.as_deref().unwrap().contains("<redacted>"));

        // Identity must be nulled.
        assert!(scrubbed.server_name.is_none());
        assert!(scrubbed.user.is_none());
        assert!(scrubbed.request.is_none());

        // Tags + extra + breadcrumbs must be empty; logentry params dropped.
        assert!(scrubbed.tags.is_empty());
        assert!(scrubbed.extra.is_empty());
        assert!(scrubbed.breadcrumbs.values.is_empty());
        assert!(scrubbed.logentry.as_ref().unwrap().params.is_empty());

        // Exception frame paths basenamed + vars/source cleared.
        let exc_frame = &scrubbed.exception.values[0]
            .stacktrace
            .as_ref()
            .unwrap()
            .frames[0];
        assert_eq!(exc_frame.abs_path.as_deref(), Some("main.rs"));
        assert_eq!(exc_frame.filename.as_deref(), Some("main.rs"));
        assert!(exc_frame.vars.is_empty());
        assert!(exc_frame.context_line.is_none());
        assert!(exc_frame.pre_context.is_empty());
        assert!(exc_frame.post_context.is_empty());

        // Top-level stacktrace frame basenamed.
        let top_frame = &scrubbed.stacktrace.as_ref().unwrap().frames[0];
        assert_eq!(top_frame.abs_path.as_deref(), Some("top.rs"));
        assert_eq!(top_frame.filename.as_deref(), Some("top.rs"));

        // Thread stacktrace frame basenamed + vars/source cleared.
        let thread_frame = &scrubbed.threads.values[0]
            .stacktrace
            .as_ref()
            .unwrap()
            .frames[0];
        assert_eq!(thread_frame.abs_path.as_deref(), Some("worker.rs"));
        assert_eq!(thread_frame.filename.as_deref(), Some("worker.rs"));
        assert!(thread_frame.vars.is_empty());
        assert!(thread_frame.context_line.is_none());
    }

    #[test]
    fn scrub_breadcrumb_drops_everything() {
        let crumb = Breadcrumb {
            message: Some("anything at all".to_string()),
            ..Default::default()
        };
        assert!(scrub_breadcrumb(crumb).is_none());
    }

    #[test]
    fn analytics_info_reports_pii_disabled() {
        let info = analytics_info(true);
        assert!(info.enabled);
        assert!(info.pii_disabled);
        // Embedded default DSN means dsn_configured is true unless SENTRY_DSN
        // is explicitly set to empty in the environment.
        let info_off = analytics_info(false);
        assert!(!info_off.enabled);
        assert!(info_off.pii_disabled);
    }

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    /// A transport that counts every envelope it is asked to send. Used to prove
    /// the OFF kill switch is thread-global: after OFF, even a worker hub that
    /// snapshotted the client BEFORE the toggle must not transmit.
    struct CountingTransport {
        count: std::sync::Arc<AtomicUsize>,
    }
    impl sentry::Transport for CountingTransport {
        fn send_envelope(&self, _envelope: sentry::Envelope) {
            self.count.fetch_add(1, Ordering::SeqCst);
        }
        // `flush`/`shutdown` use the trait defaults (return true); we only care
        // about whether `send_envelope` is ever reached.
    }
    fn counting_client(count: std::sync::Arc<AtomicUsize>) -> std::sync::Arc<sentry::Client> {
        // Double-wrap: sentry impls `TransportFactory for Arc<T: Transport>`, so
        // the `transport` field (`Arc<dyn TransportFactory>`) needs the inner
        // `Arc<CountingTransport>` (a TransportFactory) wrapped in an outer Arc.
        let transport = std::sync::Arc::new(CountingTransport { count });
        let options = sentry::ClientOptions {
            dsn: "https://public@example.invalid/1".parse().ok(),
            transport: Some(std::sync::Arc::new(transport)),
            // Keep the same privacy hooks the real client uses; an event still
            // reaches the transport (post-scrub) when sending is allowed, so the
            // counter is a faithful "did anything go out" probe.
            before_send: Some(std::sync::Arc::new(scrub_event)),
            before_breadcrumb: Some(std::sync::Arc::new(scrub_breadcrumb)),
            ..Default::default()
        };
        std::sync::Arc::new(sentry::Client::from_config(options))
    }

    // Regression gate for the OFF kill switch: it must be THREAD-GLOBAL. A
    // worker thread that snapshotted its hub (cloning the bound client) BEFORE
    // the toggle must NOT transmit after `set_analytics_enabled_runtime(false)`
    // — which is why OFF closes the shared client transport rather than only
    // unbinding the calling thread's hub.
    #[test]
    fn off_is_thread_global_worker_hub_cannot_send_after_off() {
        let count = std::sync::Arc::new(AtomicUsize::new(0));
        let client = counting_client(std::sync::Arc::clone(&count));

        // Simulate the post-init state production reaches: client stored in the
        // module static and bound on the PROCESS hub.
        {
            let mut slot = match client_cell().lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            *slot = Some(std::sync::Arc::clone(&client));
        }
        sentry::Hub::main().bind_client(Some(std::sync::Arc::clone(&client)));

        // Sanity: while ON, the calling thread's hub can send.
        sentry::Hub::current().capture_event(sentry::protocol::Event {
            message: Some("while-on".into()),
            ..Default::default()
        });
        assert!(
            count.load(Ordering::SeqCst) >= 1,
            "counting transport should receive events while analytics is ON"
        );

        // Worker thread snapshots ITS OWN thread-local hub BEFORE OFF.
        let (ready_tx, ready_rx) = mpsc::channel::<()>();
        let (go_tx, go_rx) = mpsc::channel::<()>();
        let (done_tx, done_rx) = mpsc::channel::<()>();
        let worker = std::thread::spawn(move || {
            let worker_hub = sentry::Hub::current();
            assert!(
                worker_hub.client().is_some(),
                "worker hub should have snapshotted the bound client before OFF"
            );
            ready_tx.send(()).unwrap();
            go_rx.recv().unwrap();
            // Post-OFF send on the PRE-OFF snapshotted hub: a no-op under the fix.
            worker_hub.capture_event(sentry::protocol::Event {
                message: Some("after-off-from-worker".into()),
                ..Default::default()
            });
            done_tx.send(()).unwrap();
        });

        ready_rx.recv().unwrap();
        let before_off = count.load(Ordering::SeqCst);

        // Flip OFF via the production runtime path (process-hub unbind + close).
        set_analytics_enabled_runtime(false);

        go_tx.send(()).unwrap();
        done_rx.recv().unwrap();
        worker.join().unwrap();

        assert_eq!(
            count.load(Ordering::SeqCst),
            before_off,
            "worker thread's pre-OFF hub still sent after OFF — disable is not thread-global"
        );

        // Cleanup so later tests in the same binary start clean.
        sentry::Hub::main().bind_client(None);
        {
            let mut slot = match client_cell().lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            *slot = None;
        }
        {
            let mut slot = match guard_cell().lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            *slot = None;
        }
    }
}
