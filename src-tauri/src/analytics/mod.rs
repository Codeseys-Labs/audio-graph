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
//!   `request`; reduces the event message and every exception value to
//!   redaction sentinels via [`scrub_free_text`] — which first runs the text
//!   through [`crate::error::redacted_provider_diagnostic`] (the same scrubber
//!   used for provider error excerpts) to mark secrets, then drops ALL
//!   remaining free prose so interpolated transcript text can never leak;
//!   clears `extra` and all breadcrumbs (which could carry transcript text or
//!   credentials); scrubs stack-frame paths down to basenames; and keeps only
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
//! Sentry's [`ClientInitGuard`](sentry::ClientInitGuard) cannot be cheaply
//! re-initialized, so "enabled/disabled" is modelled as a bind/unbind of the
//! client on the current [`Hub`](sentry::Hub), with the guard held for the
//! process lifetime so buffered events flush on exit:
//!
//! - **Startup** ([`init_if_enabled`]): the client is initialized only when the
//!   persisted setting is `true`. The returned guard is stored in a
//!   module-static so it lives for the whole process (flush-on-exit).
//! - **Runtime ON** ([`set_analytics_enabled_runtime`]`(true)`): rebinds the
//!   existing client to the hub (`Hub::current().bind_client(Some(..))`). If
//!   the client was never initialized (setting started `false`), the caller
//!   first calls [`init_if_enabled`]`(true)`.
//! - **Runtime OFF** ([`set_analytics_enabled_runtime`]`(false)`): unbinds the
//!   client (`Hub::current().bind_client(None)`) so no further events are sent.
//!   The guard is intentionally **not** dropped — dropping it would flush and
//!   tear down for good, and we want a cheap re-enable plus flush-on-exit.

use std::sync::{Arc, Mutex, OnceLock};

use serde::Serialize;

/// The default Sentry DSN. A DSN is a **client-side public key** that only
/// authorizes *sending* events to a project — it is NOT a secret and is safe to
/// embed. Override at runtime via the `SENTRY_DSN` environment variable (e.g.
/// to point at a self-hosted relay or to disable by supplying an empty value).
const DEFAULT_DSN: &str = "https://1e39b03ea3018d02551500bf428306b9@o4511644093448192.ingest.us.sentry.io/4511644102885381";

/// Process-lifetime holder for the Sentry client guard. Holding the guard keeps
/// the client alive so buffered events flush on exit; runtime ON/OFF only
/// binds/unbinds it on the hub rather than dropping it.
static GUARD: OnceLock<Mutex<Option<sentry::ClientInitGuard>>> = OnceLock::new();

/// Captured `Arc<Client>` from the moment of init, so runtime re-enable can
/// rebind the client on the hub after a previous OFF unbound it. Separate from
/// [`GUARD`] (which owns lifetime/flush) because the hub holds an `Arc` to the
/// client, not the guard.
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
    if slot.is_some() {
        // Already initialized — keep the existing guard/client.
        return;
    }
    // `sentry::init` binds the client to the current hub. Capture the bound
    // `Arc<Client>` so a later runtime re-enable can rebind it after an OFF.
    let guard = sentry::init(client_options());
    if let Some(client) = sentry::Hub::current().client() {
        let mut client_slot = match client_cell().lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        *client_slot = Some(client);
    }
    *slot = Some(guard);
}

/// Toggle analytics at runtime by binding/unbinding the client on the current
/// hub. Turning **ON** rebinds (or, if never inited, the caller must call
/// [`init_if_enabled`]`(true)` first — see [`crate::commands::set_analytics_enabled`]).
/// Turning **OFF** unbinds the client so no further events are sent, WITHOUT
/// dropping the guard (so flush-on-exit and cheap re-enable still work).
pub fn set_analytics_enabled_runtime(enabled: bool) {
    let hub = sentry::Hub::current();
    if enabled {
        let slot = match client_cell().lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(client) = slot.as_ref() {
            hub.bind_client(Some(Arc::clone(client)));
        }
    } else {
        hub.bind_client(None);
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

/// `before_send` scrubber. Strips identity (`server_name`/`user`/`request`),
/// reduces the message and every exception value to redaction sentinels via
/// [`scrub_free_text`] (dropping all free prose so no transcript can leak),
/// clears `extra` and all breadcrumbs, reduces stack-frame paths to basenames,
/// and keeps only the non-identifying OS / device / Rust contexts.
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

    // Reduce every exception value the same way + reduce stack-frame paths to
    // basenames. The exception `type` is kept untouched for triage value.
    for exception in event.exception.values.iter_mut() {
        if let Some(value) = exception.value.take() {
            exception.value = Some(scrub_free_text(&value));
        }
        if let Some(stacktrace) = exception.stacktrace.as_mut() {
            for frame in stacktrace.frames.iter_mut() {
                frame.abs_path = frame.abs_path.as_deref().map(basename);
                frame.filename = frame.filename.as_deref().map(basename);
            }
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
    use sentry::protocol::{Breadcrumb, Event, Exception, Frame, Map, Stacktrace, User, Value};

    // Load-bearing privacy gate: an event carrying a fake secret + fake
    // transcript line (in the message AND an exception value) + user/IP/
    // server_name + extra + breadcrumb must come out the other side of
    // `scrub_event` with the secret and transcript GONE, identity nulled, and
    // extra/breadcrumbs empty. This is the proof that "anonymous" holds.
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

        // Exception carrying both the secret and the transcript, plus a frame
        // whose abs_path embeds a username.
        event.exception.values.push(Exception {
            ty: "RuntimeError".to_string(),
            value: Some(format!("failed with {SECRET}; heard: {TRANSCRIPT}")),
            stacktrace: Some(Stacktrace {
                frames: vec![Frame {
                    abs_path: Some("/Users/alice/secret-project/src/main.rs".to_string()),
                    filename: Some("/Users/alice/secret-project/src/main.rs".to_string()),
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

        // The redaction sentinel must be present where the secret/transcript was.
        assert!(scrubbed.message.as_deref().unwrap().contains("<redacted>"));

        // Identity must be nulled.
        assert!(scrubbed.server_name.is_none());
        assert!(scrubbed.user.is_none());
        assert!(scrubbed.request.is_none());

        // Extra + breadcrumbs must be empty.
        assert!(scrubbed.extra.is_empty());
        assert!(scrubbed.breadcrumbs.values.is_empty());

        // Frame paths reduced to basenames.
        let frame = &scrubbed.exception.values[0]
            .stacktrace
            .as_ref()
            .unwrap()
            .frames[0];
        assert_eq!(frame.abs_path.as_deref(), Some("main.rs"));
        assert_eq!(frame.filename.as_deref(), Some("main.rs"));
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
}
