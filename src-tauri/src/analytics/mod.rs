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
//!   prose so interpolated transcript text can never leak; keeps ONLY an
//!   allowlist of structured, non-prose tag keys ([`ALLOWLISTED_TAG_KEYS`]) and
//!   validates every surviving tag value (secret-scrub + shape check, see
//!   [`sanitize_tag_value`]), dropping any tag that fails — every other tag key
//!   is discarded; clears `extra` and `logentry.params`; sanitizes attached
//!   breadcrumbs in place (keeping only id-shaped diagnostic breadcrumbs and
//!   dropping any that could carry transcript text or credentials, see
//!   [`sanitize_breadcrumb`]); derives the `fingerprint` from the sanitized
//!   `[category, event.name]` tags so distinct event names never share a Sentry
//!   issue; scrubs EVERY stack frame — across exception, thread, and the
//!   deprecated top-level stacktraces — down to basename paths with
//!   `vars`/`context_line`/`pre_context`/`post_context` cleared (see
//!   [`scrub_frames`]); and keeps only the non-identifying OS / device / Rust
//!   contexts (the exception `type` is kept for triage).
//! - The [`before_breadcrumb`](scrub_breadcrumb) hook sanitizes each breadcrumb
//!   via [`sanitize_breadcrumb`]: it keeps only structured diagnostic
//!   breadcrumbs (an id-shaped `event.name` message plus allowlisted,
//!   shape-checked `data`) and drops everything else, so nothing the SDK
//!   auto-collects (free-prose log records, HTTP URLs) survives — while
//!   metadata-only info beacons still ride along to enrich real errors.
//! - [`capture_message`] / [`capture_anonymous_event`] / [`capture_diagnostic`]
//!   are the **only** intentional issue-creating send paths and must be used
//!   sparingly — NEVER with transcript, audio, or credential data.
//!   [`capture_diagnostic`] is the preferred structured path: callers pass only
//!   enums/ids/numbers (never free-text tags), and its allowlisted tags survive
//!   [`scrub_event`] to give real triage signal. High-frequency info-level
//!   telemetry beacons use [`add_diagnostic_breadcrumb`] instead — that path
//!   attaches a breadcrumb (which enriches the next real error) rather than
//!   creating its own Sentry issue, so info beacons never bury real errors.
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
        // Bound the drain-on-drop window if the guard ever IS dropped (e.g. the
        // OFF path drops it after an explicit `close`). This does NOT rescue a
        // normal process exit — Rust does not run `Drop` for `static`s — so the
        // real flush guarantees come from the explicit `flush`/`flush_on_exit`
        // hooks; this is only a belt on the drop path. Matches the SDK default
        // (2s) but pinned so it can't silently drift.
        shutdown_timeout: Duration::from_millis(2000),
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

/// How long [`flush_on_exit`] waits for the Sentry transport to drain buffered
/// events at process shutdown. Bounded so a wedged network never hangs the quit
/// path — an unsent tail is acceptable, a hung exit is not.
const FLUSH_ON_EXIT_TIMEOUT: Duration = Duration::from_millis(2000);

/// How long [`flush_after_capture`] waits for the transport to drain after a
/// fresh capture (e.g. the startup ping). Short and bounded: it runs off the
/// hot path on a detached thread, and the goal is only to get the most-fragile
/// in-flight event on the wire before the user can act — an unsent tail is
/// acceptable, a lingering thread is not.
const FLUSH_AFTER_CAPTURE_TIMEOUT: Duration = Duration::from_millis(800);

/// Resolve the live Sentry client, if any: prefer the captured `Arc<Client>`
/// (the one OFF would close), else fall back to whatever the current hub has
/// bound. `None` when analytics is disabled / never inited / OFF.
fn live_client() -> Option<Arc<sentry::Client>> {
    let captured = {
        let slot = match client_cell().lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        slot.as_ref().map(Arc::clone)
    };
    captured.or_else(|| sentry::Hub::current().client())
}

/// Flush the Sentry transport, bounded by `timeout`. This is a **flush, not a
/// close**: it drains buffered events but leaves the client/transport alive, so
/// it is safe to call from any exit-adjacent path (window close, exit request,
/// terminal Exit) without breaking the runtime OFF-toggle path — which uses
/// `client.close` (terminal), see [`set_analytics_enabled_runtime`].
///
/// A no-op when analytics is disabled / never inited / OFF (no live client), so
/// it is always safe to call unconditionally. Returns `true` if the transport
/// drained within the timeout (or there was nothing to flush), `false` if the
/// wait expired.
pub fn flush(timeout: Duration) -> bool {
    match live_client() {
        Some(client) => client.flush(Some(timeout)),
        // Analytics disabled or never initialized — nothing to flush.
        None => true,
    }
}

/// Flush the Sentry transport at graceful shutdown, bounded by
/// [`FLUSH_ON_EXIT_TIMEOUT`].
///
/// The client guard lives in a `static OnceLock` and Rust does NOT run `Drop`
/// for `static`s at normal termination, so buffered analytics events are not
/// guaranteed to flush on exit on their own. This is the intentional
/// flush-at-quit hook the `RunEvent::Exit` / `RunEvent::ExitRequested` handlers
/// call so an opted-in user's last buffered events (e.g. a late error) get a
/// bounded chance to leave the machine.
///
/// A no-op when analytics is disabled / never inited (no live client), so it is
/// always safe to call unconditionally from the exit handlers. Returns `true`
/// if the transport drained within the timeout (or there was nothing to flush),
/// `false` if the wait expired.
pub fn flush_on_exit() -> bool {
    let flushed = flush(FLUSH_ON_EXIT_TIMEOUT);
    log::info!(
        "analytics.flush_on_exit timeout_ms={} flushed={}",
        FLUSH_ON_EXIT_TIMEOUT.as_millis(),
        flushed
    );
    flushed
}

/// Flush the Sentry transport shortly after a capture, OFF the hot path, so the
/// most-fragile events (notably the startup ping) get on the wire before the
/// user can act. The 0.48 transport POSTs each envelope on a background thread
/// with no debounce, and a window close on Windows can force-kill the process
/// before that POST lands — while `RunEvent::Exit` never fires on a
/// taskkill/force-quit — so a fresh capture can otherwise be lost.
///
/// Spawns a short-lived detached thread bounded by
/// [`FLUSH_AFTER_CAPTURE_TIMEOUT`] so the caller (the Tauri setup hook / UI
/// thread) is never blocked. A no-op when analytics is disabled / never inited /
/// OFF (no live client): we skip even spawning the thread in that case.
pub fn flush_after_capture() {
    // Snapshot the client on the CALLING thread, not inside the spawned thread:
    // `live_client`'s hub fallback reads `Hub::current()`, which on a freshly
    // spawned thread has no bound client. The captured `Arc<Client>` static
    // covers the common case, but snapshotting here keeps the flush correct even
    // if only the hub is bound.
    let Some(client) = live_client() else {
        // Disabled / never inited / OFF — nothing to flush, don't spawn.
        return;
    };
    // Best-effort telemetry path: never panic if the OS can't hand out a
    // thread. `thread::spawn` unwraps that error internally, so use the
    // fallible `Builder::spawn` and DROP the flush on failure — an unsent tail
    // is acceptable (see `flush`'s contract), a crash on the telemetry path is
    // not.
    let spawned = std::thread::Builder::new()
        .name("sentry-flush".to_string())
        .spawn(move || {
            let flushed = client.flush(Some(FLUSH_AFTER_CAPTURE_TIMEOUT));
            log::debug!(
                "analytics.flush_after_capture timeout_ms={} flushed={}",
                FLUSH_AFTER_CAPTURE_TIMEOUT.as_millis(),
                flushed
            );
        });
    if let Err(e) = spawned {
        log::warn!("analytics.flush_after_capture: failed to spawn flush thread: {e}");
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
/// keeps only the [`ALLOWLISTED_TAG_KEYS`] tags whose values pass
/// [`sanitize_tag_value`] (dropping every other key and every ill-shaped value),
/// clears `extra`/`logentry.params`, sanitizes attached breadcrumbs in place via
/// [`sanitize_breadcrumb_in_place`] (keeping only id-shaped diagnostic
/// breadcrumbs), derives the `fingerprint` from the sanitized
/// `[category, event.name]` tags via [`fingerprint_from_tags`] so distinct event
/// names never share an issue, scrubs every stack frame across exception,
/// thread, and the deprecated top-level stacktraces via [`scrub_frames`]
/// (basename paths + clear vars/source), and keeps only the non-identifying
/// OS / device / Rust contexts.
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

    // Tags are the ONE surviving structured lane. Keep only allowlisted keys
    // (every other key is dropped, so no free-form tag prose leaks), and put
    // each surviving value through `sanitize_tag_value` — a secret-scrub plus a
    // strict shape check — dropping any value that fails.
    event.tags.retain(|key, value| {
        if !is_allowlisted_tag_key(key) {
            return false;
        }
        match sanitize_tag_value(key, value) {
            Some(clean) => {
                *value = clean;
                true
            }
            None => false,
        }
    });

    // Group by [category, event.name] so distinct event names never collapse
    // into one Sentry issue. The default fingerprint groups by message template
    // / transaction; because every free-text field (including the message) is
    // scrubbed to the SAME `OMITTED_MARKER` sentinel above, the default would
    // group ALL diagnostics into a single issue (the AUDIO-GRAPH-3 bug). Deriving
    // the fingerprint from the ALREADY-sanitized allowlisted tags is the maximal
    // privacy posture: both components are a strict subset of the validated tag
    // lane (id-shaped, secret-scrubbed), so no free prose can enter the
    // fingerprint. If neither tag survived (e.g. an SDK-internal event with no
    // structured tags), fall back to the default marker rather than an empty
    // fingerprint, which Sentry would otherwise treat as "group everything".
    event.fingerprint = fingerprint_from_tags(&event.tags);

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

    // Breadcrumbs attached to the outgoing event: sanitize in place rather than
    // clear. `before_breadcrumb` (see `scrub_breadcrumb`) already gates every
    // breadcrumb as it is ADDED, but an event can also arrive here with
    // breadcrumbs the SDK attached without passing through that hook, so this is
    // the belt-and-suspenders backstop — identical policy to `sanitize_breadcrumb`
    // (keep only id-shaped diagnostic breadcrumbs with allowlisted `data`, drop
    // everything else). This is what lets metadata-only info beacons enrich a
    // real error while free-prose auto-crumbs never leak.
    event
        .breadcrumbs
        .values
        .retain_mut(sanitize_breadcrumb_in_place);

    // Keep only safe, non-identifying contexts (OS / device / runtime).
    event
        .contexts
        .retain(|key, _| matches!(key.as_str(), "os" | "device" | "rust" | "runtime"));

    Some(event)
}

/// `before_breadcrumb` scrubber: keep ONLY structured diagnostic breadcrumbs,
/// drop everything else. The SDK can auto-collect breadcrumbs (e.g. log records,
/// HTTP request URLs) that may contain transcript text or credentials, so the
/// default posture is still "drop" — but a breadcrumb that carries an id-shaped
/// `event.name` message and only allowlisted, shape-checked string `data` (the
/// shape [`add_diagnostic_breadcrumb`] emits for info-level telemetry beacons)
/// is structurally safe and is kept so it can enrich the next real error.
/// See [`sanitize_breadcrumb`] for the exact policy.
fn scrub_breadcrumb(breadcrumb: sentry::Breadcrumb) -> Option<sentry::Breadcrumb> {
    sanitize_breadcrumb(breadcrumb)
}

/// The breadcrumb `type` (`ty`) that marks an intentional diagnostic breadcrumb
/// emitted by [`add_diagnostic_breadcrumb`]. Only breadcrumbs of this type
/// survive [`sanitize_breadcrumb`]; any other type (including the SDK default
/// `"default"` used by auto-collected crumbs) is dropped.
const DIAGNOSTIC_BREADCRUMB_TYPE: &str = "info";

/// The breadcrumb `data` keys allowed to survive [`sanitize_breadcrumb`]. This
/// is the breadcrumb analogue of [`ALLOWLISTED_TAG_KEYS`] (minus `event.name`,
/// which rides as the breadcrumb `message`, and `release`/`channel`, which are
/// event-level). Every surviving value is shape-checked by
/// [`sanitize_tag_value`] under its key, so no free prose can ride in `data`.
const ALLOWLISTED_BREADCRUMB_DATA_KEYS: &[&str] =
    &["category", "provider", "kind", "http_status", "recoverable"];

/// Decide whether an owned [`Breadcrumb`](sentry::Breadcrumb) is a structurally
/// safe diagnostic breadcrumb and, if so, return its sanitized form; otherwise
/// return `None` to drop it. This is the owned-value entry point used by the
/// `before_breadcrumb` hook; [`sanitize_breadcrumb_in_place`] is the borrow
/// form used by `scrub_event`'s backstop over already-attached breadcrumbs.
fn sanitize_breadcrumb(mut breadcrumb: sentry::Breadcrumb) -> Option<sentry::Breadcrumb> {
    sanitize_breadcrumb_in_place(&mut breadcrumb).then_some(breadcrumb)
}

/// Sanitize a breadcrumb in place, returning `true` to KEEP it or `false` to
/// DROP it. Keep policy (all conditions must hold), everything else dropped:
/// - `ty == DIAGNOSTIC_BREADCRUMB_TYPE` (marks an intentional diagnostic crumb),
/// - `message` is present and id-shaped (`^[a-z0-9._:-]{1,48}$`) — this is the
///   `event.name`, the crumb's only free-text-shaped field, so gating it on the
///   id shape means no prose survives,
/// - every `data` entry is an allowlisted key ([`ALLOWLISTED_BREADCRUMB_DATA_KEYS`])
///   whose string value passes [`sanitize_tag_value`] (secret-scrub + per-key
///   shape check) — non-allowlisted keys and non-string / ill-shaped values are
///   removed.
///
/// The `timestamp` and `level` fields carry no free text (level is a closed
/// enum, timestamp a number), so they are left as-is. The optional `category`
/// string is dropped unless it is id-shaped (belt-and-suspenders: our emitter
/// only ever sets a closed-enum category, but no free prose may survive here).
fn sanitize_breadcrumb_in_place(breadcrumb: &mut sentry::Breadcrumb) -> bool {
    if breadcrumb.ty != DIAGNOSTIC_BREADCRUMB_TYPE {
        return false;
    }
    // The message is the event.name; it must be id-shaped or the crumb is dropped
    // wholesale (rather than kept with a scrubbed message, which would create an
    // anonymous, signal-free crumb).
    match breadcrumb.message.as_deref() {
        Some(msg) if is_id_shaped(msg) => {}
        _ => return false,
    }
    // Drop a non-id-shaped category rather than forwarding free text.
    if !breadcrumb.category.as_deref().is_none_or(is_id_shaped) {
        breadcrumb.category = None;
    }
    // Keep only allowlisted, shape-checked STRING data. A non-string value (or
    // an allowlisted key whose value fails its per-key shape check) is dropped.
    breadcrumb.data.retain(|key, value| {
        if !ALLOWLISTED_BREADCRUMB_DATA_KEYS.contains(&key.as_str()) {
            return false;
        }
        match value.as_str() {
            Some(s) => match sanitize_tag_value(key, s) {
                Some(clean) => {
                    *value = sentry::protocol::Value::from(clean);
                    true
                }
                None => false,
            },
            None => false,
        }
    });
    true
}

/// Build a Sentry `fingerprint` from the already-sanitized allowlisted tags so
/// events group per event name instead of collapsing into one issue.
///
/// Uses `[category, event.name]` — both are id-shaped, secret-scrubbed values
/// from the validated tag lane, so nothing prose-shaped can enter the
/// fingerprint. Missing components are simply omitted. If NEITHER survives,
/// returns the SDK default marker (`{{ default }}`) rather than an empty
/// fingerprint (Sentry treats an empty fingerprint as "group everything", the
/// very bug this fixes).
fn fingerprint_from_tags(
    tags: &sentry::protocol::Map<String, String>,
) -> std::borrow::Cow<'static, [std::borrow::Cow<'static, str>]> {
    use std::borrow::Cow;
    let mut parts: Vec<Cow<'static, str>> = Vec::with_capacity(2);
    if let Some(category) = tags.get("category") {
        parts.push(Cow::Owned(category.clone()));
    }
    if let Some(name) = tags.get("event.name") {
        parts.push(Cow::Owned(name.clone()));
    }
    if parts.is_empty() {
        // No structured tags — keep Sentry's default grouping rather than an
        // empty fingerprint (which would group everything).
        Cow::Owned(vec![Cow::Borrowed("{{ default }}")])
    } else {
        Cow::Owned(parts)
    }
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

/// A coarse, closed set of diagnostic categories. Callers pick one — they can
/// never inject free-form category text into an event, so the category tag is
/// structurally safe to keep through [`scrub_event`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Asr,
    Llm,
    Tts,
    Audio,
    Command,
    Startup,
    Panic,
    /// Diagnostics originating in the WebView frontend, relayed through the
    /// `report_frontend_diagnostic` command (the browser has no direct Sentry
    /// egress — CSP blocks it — so it forwards structured, controlled ids here).
    Frontend,
    Other,
}

impl Category {
    /// Stable lowercase string representation used as the `category` tag value.
    /// Stable across releases: values are grouped/filtered on in Sentry, so
    /// these strings are part of the wire contract — do not rename casually.
    pub fn as_str(self) -> &'static str {
        match self {
            Category::Asr => "asr",
            Category::Llm => "llm",
            Category::Tts => "tts",
            Category::Audio => "audio",
            Category::Command => "command",
            Category::Startup => "startup",
            Category::Panic => "panic",
            Category::Frontend => "frontend",
            Category::Other => "other",
        }
    }

    /// The category for any diagnostic relayed from the WebView frontend:
    /// always [`Category::Frontend`], by design. The frontend cannot pick its
    /// own category — the backend does — so no free-text category can ride in
    /// from the WebView. There is deliberately no id parameter: the frontend's
    /// category string is never trusted or consulted, so accepting one would be
    /// an API-shape lie (it would look meaningful while being ignored).
    pub(crate) fn frontend() -> Category {
        Category::Frontend
    }
}

/// A structured, privacy-safe diagnostic event. This is the preferred capture
/// path: callers pass only enums, controlled ids, and numbers — there is
/// physically no field for free-text tags, so nothing prose-shaped can ride in.
/// The values still pass through the allowlist + [`sanitize_tag_value`] in
/// [`scrub_event`] as belt-and-suspenders defense.
pub struct DiagEvent<'a> {
    /// Stable event id, e.g. `"asr.stream.error"`. Becomes the `event.name` tag
    /// AND the captured message. Must match the id shape (`^[a-z0-9._:-]{1,48}$`)
    /// or [`scrub_event`] drops the tag.
    pub name: &'a str,
    /// Coarse category — see [`Category`].
    pub category: Category,
    /// Severity level for the captured message.
    pub level: sentry::Level,
    /// Controlled provider id, e.g. `"deepgram"`. `None` omits the tag.
    pub provider: Option<&'a str>,
    /// Controlled error-kind id, e.g. `"parse_error"`. `None` omits the tag.
    pub kind: Option<&'a str>,
    /// HTTP status, when the failure was an HTTP response. `None` omits the tag.
    pub http_status: Option<u16>,
    /// Whether the app recovered from / could retry the failure. `None` omits.
    pub recoverable: Option<bool>,
}

/// Capture a structured [`DiagEvent`] at its level, setting the allowlisted tags
/// on a per-capture scope. Uses [`sentry::with_scope`], which is a no-op
/// passthrough when no client is bound — so this respects the existing toggle
/// semantics (OFF ⇒ unbound hub ⇒ nothing sent) with no extra gate. The event
/// still flows through [`scrub_event`], so even if a caller somehow passed an
/// ill-shaped id the tag would be dropped there.
pub fn capture_diagnostic(ev: DiagEvent<'_>) {
    sentry::with_scope(
        |scope| {
            scope.set_level(Some(ev.level));
            scope.set_tag("event.name", ev.name);
            scope.set_tag("category", ev.category.as_str());
            if let Some(provider) = ev.provider {
                scope.set_tag("provider", provider);
            }
            if let Some(kind) = ev.kind {
                scope.set_tag("kind", kind);
            }
            if let Some(status) = ev.http_status {
                scope.set_tag("http_status", status);
            }
            if let Some(recoverable) = ev.recoverable {
                scope.set_tag("recoverable", recoverable);
            }
        },
        || {
            capture_message(ev.name, ev.level);
        },
    );
}

/// Attach a structured diagnostic [`DiagEvent`] as a **breadcrumb** rather than
/// capturing it as its own Sentry issue. This is the path for high-frequency,
/// info-level telemetry beacons (e.g. `llm.openrouter.routed` routing telemetry):
/// they enrich the *next real error* that fires on the same scope without
/// creating a standalone issue, so they never bury actionable errors or inflate
/// issue counts (the AUDIO-GRAPH-3 problem this addresses).
///
/// Emits a breadcrumb whose `type` is [`DIAGNOSTIC_BREADCRUMB_TYPE`] (marking it
/// as an intentional diagnostic crumb), `message` is the id-shaped `event.name`,
/// `category` is the coarse [`Category`], and `data` carries only the allowlisted
/// structured tags. It rides `sentry::add_breadcrumb`, which passes through the
/// [`scrub_breadcrumb`] hook — so this respects the toggle (no client ⇒ no-op)
/// and the crumb is re-validated on the way in. The `level` is used for the
/// breadcrumb level; callers should use info/debug beacons here and reserve
/// [`capture_diagnostic`] for error/warning-level actionable failures.
pub fn add_diagnostic_breadcrumb(ev: DiagEvent<'_>) {
    let mut data: sentry::protocol::Map<String, sentry::protocol::Value> =
        sentry::protocol::Map::new();
    data.insert(
        "category".to_string(),
        sentry::protocol::Value::from(ev.category.as_str()),
    );
    if let Some(provider) = ev.provider {
        data.insert(
            "provider".to_string(),
            sentry::protocol::Value::from(provider),
        );
    }
    if let Some(kind) = ev.kind {
        data.insert("kind".to_string(), sentry::protocol::Value::from(kind));
    }
    if let Some(status) = ev.http_status {
        // Store as a string so it matches the tag lane's `http_status` shape
        // (`sanitize_tag_value` parses it back), keeping one validation path.
        data.insert(
            "http_status".to_string(),
            sentry::protocol::Value::from(status.to_string()),
        );
    }
    if let Some(recoverable) = ev.recoverable {
        data.insert(
            "recoverable".to_string(),
            sentry::protocol::Value::from(recoverable.to_string()),
        );
    }
    sentry::add_breadcrumb(sentry::Breadcrumb {
        ty: DIAGNOSTIC_BREADCRUMB_TYPE.to_string(),
        category: Some(ev.category.as_str().to_string()),
        level: ev.level,
        message: Some(ev.name.to_string()),
        data,
        ..Default::default()
    });
}

/// The ONLY tag keys allowed to survive [`scrub_event`]. Everything else is a
/// potential free-text leak and is dropped. `release` and `channel` are
/// SDK/build-set structured identifiers; the rest come from [`DiagEvent`].
const ALLOWLISTED_TAG_KEYS: &[&str] = &[
    "event.name",
    "category",
    "provider",
    "kind",
    "http_status",
    "recoverable",
    "release",
    "channel",
];

/// Whether `key` is on the tag allowlist.
fn is_allowlisted_tag_key(key: &str) -> bool {
    ALLOWLISTED_TAG_KEYS.contains(&key)
}

/// Whether `s` matches the controlled-id shape `^[a-z0-9._:-]{1,48}$` (lowercase
/// alphanumerics plus `.`, `_`, `:`, `-`; 1–48 chars). This is deliberately
/// hand-rolled (no regex dependency) and is the shape gate for id-like tags.
fn is_id_shaped(s: &str) -> bool {
    let len = s.chars().count();
    (1..=48).contains(&len)
        && s.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b':' | b'-')
        })
}

/// Validate + normalize a surviving tag value for the allowlisted `key`. Returns
/// `Some(clean_value)` to keep the tag, or `None` to DROP it.
///
/// Belt-and-suspenders defense (the typed [`capture_diagnostic`] API already
/// constrains callers): first run the value through
/// [`crate::error::redacted_provider_diagnostic`] (the shared secret scrubber),
/// then shape-check per key:
/// - `event.name`/`category`/`provider`/`kind`/`channel`: must be id-shaped
///   (`^[a-z0-9._:-]{1,48}$`) — a secret-scrubbed value contains `<redacted>`,
///   which is NOT id-shaped, so it is dropped.
/// - `http_status`: must parse to `100..=599`.
/// - `recoverable`: must be exactly `"true"` or `"false"`.
/// - `release`: SDK-set; kept as-is after the secret-scrub (never id-shaped).
fn sanitize_tag_value(key: &str, value: &str) -> Option<String> {
    let scrubbed = crate::error::redacted_provider_diagnostic(value, std::iter::empty::<&str>());
    match key {
        "event.name" | "category" | "provider" | "kind" | "channel" => {
            is_id_shaped(&scrubbed).then_some(scrubbed)
        }
        "http_status" => match scrubbed.parse::<u16>() {
            Ok(code) if (100..=599).contains(&code) => Some(scrubbed),
            _ => None,
        },
        "recoverable" => matches!(scrubbed.as_str(), "true" | "false").then_some(scrubbed),
        // `release` is SDK-set (`sentry::release_name!()`); keep it after the
        // secret-scrub. Any other key never reaches here (allowlist gates first).
        _ => Some(scrubbed),
    }
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
    // and transcript GONE everywhere, identity nulled, extra/breadcrumbs empty,
    // and thread/top-level frame paths basenamed. This is the proof that
    // "anonymous" holds (the verdict-3 probe, hardened into the gate).
    //
    // For tags specifically it also proves the structured-lane allowlist: a
    // non-allowlisted key (`transcript`) is dropped, an allowlisted key with a
    // secret-shaped value (`provider`) is dropped by the shape check, and the
    // GOOD allowlisted tags (`event.name`, `category`) survive intact.
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

        // Tags: (a) a NON-allowlisted key carrying transcript prose (must be
        // dropped by the key allowlist), (b) an ALLOWLISTED key (`provider`)
        // whose value is secret-shaped (must be dropped by the shape check after
        // the secret-scrub turns it into `<redacted>`), and (c) GOOD allowlisted
        // tags that must survive intact. Plus a custom fingerprint encoding prose.
        event
            .tags
            .insert("transcript".to_string(), TRANSCRIPT.to_string());
        event
            .tags
            .insert("provider".to_string(), SECRET.to_string());
        event
            .tags
            .insert("event.name".to_string(), "asr.stream.error".to_string());
        event.tags.insert("category".to_string(), "asr".to_string());
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

        // Structured-lane allowlist: the non-allowlisted `transcript` tag is
        // gone, the secret-shaped `provider` tag is dropped by the shape check,
        // and the good allowlisted tags survive intact — and NOTHING else.
        assert!(
            !scrubbed.tags.contains_key("transcript"),
            "non-allowlisted tag survived: {:?}",
            scrubbed.tags
        );
        assert!(
            !scrubbed.tags.contains_key("provider"),
            "secret-shaped allowlisted tag was not dropped: {:?}",
            scrubbed.tags
        );
        assert_eq!(
            scrubbed.tags.get("event.name").map(String::as_str),
            Some("asr.stream.error"),
            "good allowlisted event.name tag did not survive: {:?}",
            scrubbed.tags
        );
        assert_eq!(
            scrubbed.tags.get("category").map(String::as_str),
            Some("asr"),
            "good allowlisted category tag did not survive: {:?}",
            scrubbed.tags
        );
        assert_eq!(
            scrubbed.tags.len(),
            2,
            "only the two good allowlisted tags should remain: {:?}",
            scrubbed.tags
        );

        // Fingerprint: the prose-carrying custom fingerprint (`SECRET-TRANSCRIPT`)
        // must be replaced by the sanitized `[category, event.name]` — proving
        // both that no prose survives in the fingerprint AND that distinct event
        // names group into distinct issues (the AUDIO-GRAPH-3 fix).
        let fingerprint: Vec<&str> = scrubbed.fingerprint.iter().map(|c| c.as_ref()).collect();
        assert_eq!(
            fingerprint,
            ["asr", "asr.stream.error"],
            "fingerprint must be rebuilt from sanitized tags: {:?}",
            scrubbed.fingerprint
        );

        // Extra must be empty; logentry params dropped. The prose breadcrumb
        // (default type, non-id message) is dropped by `sanitize_breadcrumb`.
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

    // `scrub_breadcrumb` (the `before_breadcrumb` hook) must DROP every
    // auto-collected / free-prose breadcrumb — anything not marked as an
    // intentional diagnostic breadcrumb — so nothing the SDK collects survives.
    #[test]
    fn scrub_breadcrumb_drops_free_prose_and_auto_collected() {
        // Default-type crumb with free prose (what an SDK log-integration emits).
        let crumb = Breadcrumb {
            message: Some("anything at all".to_string()),
            ..Default::default()
        };
        assert!(
            scrub_breadcrumb(crumb).is_none(),
            "default-type free-prose breadcrumb must be dropped"
        );

        // Even marked as the diagnostic type, a non-id-shaped (prose) message is
        // dropped wholesale — the message IS the event.name and must be id-shaped.
        let prose = Breadcrumb {
            ty: DIAGNOSTIC_BREADCRUMB_TYPE.to_string(),
            message: Some("user said their password out loud".to_string()),
            ..Default::default()
        };
        assert!(
            scrub_breadcrumb(prose).is_none(),
            "diagnostic-typed crumb with a non-id-shaped message must be dropped"
        );

        // A diagnostic-typed crumb with NO message is dropped (no triage signal).
        let no_msg = Breadcrumb {
            ty: DIAGNOSTIC_BREADCRUMB_TYPE.to_string(),
            ..Default::default()
        };
        assert!(
            scrub_breadcrumb(no_msg).is_none(),
            "diagnostic-typed crumb with no message must be dropped"
        );
    }

    // `scrub_breadcrumb` must KEEP a structured diagnostic breadcrumb (id-shaped
    // event.name message + allowlisted, shape-checked string `data`) so that
    // info-level telemetry beacons enrich the next real error — while still
    // stripping any non-allowlisted / secret-shaped / non-string `data` entry.
    #[test]
    fn scrub_breadcrumb_keeps_diagnostic_and_sanitizes_data() {
        const SECRET: &str = "sk-test-supersecret-credential-12345";
        let mut data: Map<String, Value> = Map::new();
        // Good allowlisted, id-shaped values: survive.
        data.insert("category".to_string(), Value::from("llm"));
        data.insert("provider".to_string(), Value::from("openrouter"));
        data.insert("kind".to_string(), Value::from("routed"));
        data.insert("http_status".to_string(), Value::from("200"));
        data.insert("recoverable".to_string(), Value::from("true"));
        // Non-allowlisted key: dropped.
        data.insert("model".to_string(), Value::from("gpt-4o"));
        // Allowlisted key, secret-shaped value: dropped by the shape check.
        data.insert("provider2".to_string(), Value::from(SECRET));
        // Allowlisted key, non-string (numeric) value: dropped (data must be str).
        data.insert("http_status_num".to_string(), Value::from(200));

        let crumb = Breadcrumb {
            ty: DIAGNOSTIC_BREADCRUMB_TYPE.to_string(),
            category: Some("llm".to_string()),
            level: sentry::Level::Info,
            message: Some("llm.openrouter.routed".to_string()),
            data,
            ..Default::default()
        };

        let kept = scrub_breadcrumb(crumb).expect("diagnostic breadcrumb must survive");
        assert_eq!(kept.message.as_deref(), Some("llm.openrouter.routed"));
        assert_eq!(kept.ty, DIAGNOSTIC_BREADCRUMB_TYPE);

        // Only the good allowlisted string data survives; no secret leaks.
        assert_eq!(
            kept.data.get("category").and_then(Value::as_str),
            Some("llm")
        );
        assert_eq!(
            kept.data.get("provider").and_then(Value::as_str),
            Some("openrouter")
        );
        assert_eq!(
            kept.data.get("kind").and_then(Value::as_str),
            Some("routed")
        );
        assert_eq!(
            kept.data.get("http_status").and_then(Value::as_str),
            Some("200")
        );
        assert_eq!(
            kept.data.get("recoverable").and_then(Value::as_str),
            Some("true")
        );
        assert!(
            !kept.data.contains_key("model"),
            "non-allowlisted key survived"
        );
        assert!(
            !kept.data.contains_key("provider2"),
            "secret-shaped value survived"
        );
        assert!(
            !kept.data.contains_key("http_status_num"),
            "non-string value survived"
        );
        assert_eq!(
            kept.data.len(),
            5,
            "exactly the good data entries: {:?}",
            kept.data
        );

        let json = serde_json::to_string(&kept).expect("breadcrumb serializes");
        assert!(
            !json.contains(SECRET),
            "secret leaked in breadcrumb: {json}"
        );
    }

    // The fingerprint helper groups per event name and never carries prose.
    #[test]
    fn fingerprint_from_tags_groups_per_event_name() {
        // Both components present → [category, event.name].
        let mut tags: Map<String, String> = Map::new();
        tags.insert("category".to_string(), "llm".to_string());
        tags.insert(
            "event.name".to_string(),
            "llm.openrouter.http_error".to_string(),
        );
        let fp: Vec<String> = fingerprint_from_tags(&tags)
            .iter()
            .map(|c| c.to_string())
            .collect();
        assert_eq!(fp, ["llm", "llm.openrouter.http_error"]);

        // Two DIFFERENT event names must yield DIFFERENT fingerprints (the whole
        // point: they must not collapse into one Sentry issue).
        let mut other: Map<String, String> = Map::new();
        other.insert("category".to_string(), "frontend".to_string());
        other.insert(
            "event.name".to_string(),
            "frontend.invoke.error".to_string(),
        );
        let fp_other: Vec<String> = fingerprint_from_tags(&other)
            .iter()
            .map(|c| c.to_string())
            .collect();
        assert_ne!(
            fp, fp_other,
            "distinct event names must produce distinct fingerprints"
        );

        // No structured tags → fall back to the SDK default marker (NOT empty,
        // which Sentry treats as group-everything).
        let empty: Map<String, String> = Map::new();
        let fp_empty: Vec<String> = fingerprint_from_tags(&empty)
            .iter()
            .map(|c| c.to_string())
            .collect();
        assert_eq!(fp_empty, ["{{ default }}"]);
    }

    #[test]
    fn category_as_str_is_stable_lowercase() {
        for (cat, expected) in [
            (Category::Asr, "asr"),
            (Category::Llm, "llm"),
            (Category::Tts, "tts"),
            (Category::Audio, "audio"),
            (Category::Command, "command"),
            (Category::Startup, "startup"),
            (Category::Panic, "panic"),
            (Category::Frontend, "frontend"),
            (Category::Other, "other"),
        ] {
            assert_eq!(cat.as_str(), expected);
            // Every category repr must itself pass the id-shape gate so it
            // survives the scrubber as the `category` tag value.
            assert!(
                is_id_shaped(cat.as_str()),
                "category {expected} not id-shaped"
            );
        }
    }

    #[test]
    fn category_frontend_is_always_frontend() {
        // The frontend never picks its own category — the backend fixes it to
        // `Frontend`. This locks in the privacy invariant (audio-graph-5641):
        // there is no input that can steer the category away from `Frontend`,
        // which is why the constructor takes no id argument at all.
        assert_eq!(Category::frontend(), Category::Frontend);
        assert_eq!(Category::frontend().as_str(), "frontend");
    }

    #[test]
    fn sanitize_tag_value_enforces_per_key_shape() {
        // Good id-shaped values survive on id-like keys.
        for key in ["event.name", "category", "provider", "kind", "channel"] {
            assert_eq!(
                sanitize_tag_value(key, "asr.stream.error"),
                Some("asr.stream.error".to_string()),
                "well-shaped id dropped for key {key}"
            );
        }

        // A secret-shaped value becomes `<redacted>` (angle brackets) which is
        // NOT id-shaped, so it is dropped on every id-like key.
        for key in ["event.name", "category", "provider", "kind", "channel"] {
            assert_eq!(
                sanitize_tag_value(key, "sk-test-supersecret-credential-12345"),
                None,
                "secret-shaped value not dropped for key {key}"
            );
        }

        // Shape violations on id-like keys are dropped: uppercase, spaces,
        // over-length, and empty all fail.
        assert_eq!(sanitize_tag_value("provider", "DeepGram"), None);
        assert_eq!(sanitize_tag_value("kind", "parse error"), None);
        assert_eq!(sanitize_tag_value("category", ""), None);
        assert_eq!(sanitize_tag_value("event.name", &"a".repeat(49)), None);
        assert_eq!(
            sanitize_tag_value("event.name", &"a".repeat(48)),
            Some("a".repeat(48))
        );

        // http_status: only numeric 100..=599.
        assert_eq!(sanitize_tag_value("http_status", "200"), Some("200".into()));
        assert_eq!(sanitize_tag_value("http_status", "599"), Some("599".into()));
        assert_eq!(sanitize_tag_value("http_status", "100"), Some("100".into()));
        assert_eq!(sanitize_tag_value("http_status", "99"), None);
        assert_eq!(sanitize_tag_value("http_status", "600"), None);
        assert_eq!(sanitize_tag_value("http_status", "not-a-number"), None);

        // recoverable: exactly "true"/"false".
        assert_eq!(
            sanitize_tag_value("recoverable", "true"),
            Some("true".into())
        );
        assert_eq!(
            sanitize_tag_value("recoverable", "false"),
            Some("false".into())
        );
        assert_eq!(sanitize_tag_value("recoverable", "yes"), None);
        assert_eq!(sanitize_tag_value("recoverable", "True"), None);

        // release: SDK-set; kept as-is after secret-scrub (not id-shaped, but
        // the release lane is exempt from the id gate).
        assert_eq!(
            sanitize_tag_value("release", "audio-graph@1.2.3"),
            Some("audio-graph@1.2.3".to_string())
        );
    }

    /// A transport that clones out the (post-scrub) event of every envelope it
    /// receives so a test can inspect the tags the SDK actually applied.
    struct CapturingTransport {
        events: std::sync::Arc<Mutex<Vec<Event<'static>>>>,
    }
    impl sentry::Transport for CapturingTransport {
        fn send_envelope(&self, envelope: sentry::Envelope) {
            if let Some(event) = envelope.event() {
                self.events
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .push(event.clone());
            }
        }
    }
    fn capturing_client(
        events: std::sync::Arc<Mutex<Vec<Event<'static>>>>,
    ) -> std::sync::Arc<sentry::Client> {
        let transport = std::sync::Arc::new(CapturingTransport { events });
        let options = sentry::ClientOptions {
            dsn: "https://public@example.invalid/1".parse().ok(),
            transport: Some(std::sync::Arc::new(transport)),
            // Use the REAL privacy hooks so the captured event is the fully
            // scrubbed one production would emit.
            before_send: Some(std::sync::Arc::new(scrub_event)),
            before_breadcrumb: Some(std::sync::Arc::new(scrub_breadcrumb)),
            ..Default::default()
        };
        std::sync::Arc::new(sentry::Client::from_config(options))
    }

    // `capture_diagnostic` must set exactly the allowlisted tags on a per-capture
    // scope, and a secret-shaped provider must be scrubbed/dropped by
    // `scrub_event` — while the good structured tags (name/category/kind/
    // http_status/recoverable) survive intact and the message is the event name.
    #[test]
    fn capture_diagnostic_sets_allowlisted_tags_and_drops_secret_provider() {
        // `capture_diagnostic` captures through `Hub::current()` (via
        // `sentry::with_scope`) and needs a bound client. Do the bind + capture
        // on a DEDICATED worker thread with its OWN thread-local hub, so we never
        // bind a client on the shared test-runner thread's `Hub::current()` /
        // `Hub::main()` and leak state into sibling global-state tests. The
        // worker returns the single captured (post-scrub) event; assertions run
        // back on the test thread.
        let ev = std::thread::spawn(|| {
            let events = std::sync::Arc::new(Mutex::new(Vec::<Event<'static>>::new()));
            let client = capturing_client(std::sync::Arc::clone(&events));
            sentry::Hub::current().bind_client(Some(std::sync::Arc::clone(&client)));
            capture_diagnostic(DiagEvent {
                name: "asr.stream.error",
                category: Category::Asr,
                level: sentry::Level::Error,
                // Secret-shaped provider: must be dropped by the shape check
                // after the secret-scrub turns it into `<redacted>`.
                provider: Some("sk-test-supersecret-credential-12345"),
                kind: Some("parse_error"),
                http_status: Some(503),
                recoverable: Some(true),
            });
            client.close(Some(CLOSE_TIMEOUT));
            let mut captured = events.lock().unwrap_or_else(|p| p.into_inner());
            assert_eq!(
                captured.len(),
                1,
                "exactly one diagnostic should be captured"
            );
            captured.pop().unwrap()
        })
        .join()
        .expect("capture worker thread should not panic");

        // Message is the event name, scrubbed to sentinels (id names carry no
        // secret, so the scrubber collapses prose to the omitted marker — the
        // structured signal lives in tags, not the message).
        assert!(ev.message.is_some(), "diagnostic must carry a message");

        // Good structured tags survive intact.
        assert_eq!(
            ev.tags.get("event.name").map(String::as_str),
            Some("asr.stream.error")
        );
        assert_eq!(ev.tags.get("category").map(String::as_str), Some("asr"));
        assert_eq!(ev.tags.get("kind").map(String::as_str), Some("parse_error"));
        assert_eq!(ev.tags.get("http_status").map(String::as_str), Some("503"));
        assert_eq!(ev.tags.get("recoverable").map(String::as_str), Some("true"));

        // Secret-shaped provider tag dropped, and no secret anywhere.
        assert!(
            !ev.tags.contains_key("provider"),
            "secret-shaped provider tag survived: {:?}",
            ev.tags
        );
        let json = serde_json::to_string(&ev).expect("event serializes");
        assert!(
            !json.contains("sk-test-supersecret-credential-12345"),
            "secret provider leaked: {json}"
        );
        assert_eq!(ev.level, sentry::Level::Error, "level must be preserved");

        // No cleanup needed: the bind happened on a dropped worker thread's own
        // thread-local hub, and PROCESS_HUB / the module statics were never
        // touched — so sibling global-state tests still start from a clean
        // baseline (this test's `Hub::current()` never materialized on the
        // shared test-runner thread).
    }

    // The AUDIO-GRAPH-3 regression gate: an info-level telemetry beacon
    // (`add_diagnostic_breadcrumb`) must NOT create its own Sentry event, and
    // must instead attach as a breadcrumb that rides along on the NEXT real error
    // event captured on the same scope — so info beacons enrich errors instead of
    // burying them under their own issues. Same dedicated-worker-thread hub
    // hygiene as the sibling capture test.
    #[test]
    fn info_beacon_becomes_breadcrumb_not_event_and_enriches_next_error() {
        let ev = std::thread::spawn(|| {
            let events = std::sync::Arc::new(Mutex::new(Vec::<Event<'static>>::new()));
            let client = capturing_client(std::sync::Arc::clone(&events));
            sentry::Hub::current().bind_client(Some(std::sync::Arc::clone(&client)));

            // 1. Info beacon → breadcrumb. This must NOT produce an event.
            add_diagnostic_breadcrumb(DiagEvent {
                name: "llm.openrouter.routed",
                category: Category::Llm,
                level: sentry::Level::Info,
                provider: Some("openrouter"),
                kind: Some("routed"),
                http_status: None,
                recoverable: None,
            });
            {
                let captured = events.lock().unwrap_or_else(|p| p.into_inner());
                assert!(
                    captured.is_empty(),
                    "info beacon must NOT create a standalone event: {captured:?}"
                );
            }

            // 2. A real error event captured afterward must carry the beacon as a
            //    breadcrumb (breadcrumbs attach from the scope at capture time).
            capture_diagnostic(DiagEvent {
                name: "llm.openrouter.http_error",
                category: Category::Llm,
                level: sentry::Level::Error,
                provider: Some("openrouter"),
                kind: Some("http_error"),
                http_status: Some(503),
                recoverable: None,
            });
            client.close(Some(CLOSE_TIMEOUT));
            let mut captured = events.lock().unwrap_or_else(|p| p.into_inner());
            assert_eq!(
                captured.len(),
                1,
                "exactly one issue-event (the error) should be captured"
            );
            captured.pop().unwrap()
        })
        .join()
        .expect("capture worker thread should not panic");

        // The captured event is the ERROR (its own fingerprint), and it carries
        // the info beacon as a surviving diagnostic breadcrumb.
        assert_eq!(
            ev.tags.get("event.name").map(String::as_str),
            Some("llm.openrouter.http_error")
        );
        let err_fp: Vec<&str> = ev.fingerprint.iter().map(|c| c.as_ref()).collect();
        assert_eq!(
            err_fp,
            ["llm", "llm.openrouter.http_error"],
            "error event must group by its own [category, event.name]"
        );
        let crumbs = &ev.breadcrumbs.values;
        assert_eq!(
            crumbs.len(),
            1,
            "the routed info beacon must survive as a breadcrumb on the error: {crumbs:?}"
        );
        assert_eq!(crumbs[0].message.as_deref(), Some("llm.openrouter.routed"));
        assert_eq!(crumbs[0].ty, DIAGNOSTIC_BREADCRUMB_TYPE);
        assert_eq!(
            crumbs[0].data.get("provider").and_then(Value::as_str),
            Some("openrouter")
        );
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
        // Also bind on THIS thread's hub for the "while ON" sanity send below.
        // Under `--test-threads=1` libtest runs each test on a fresh thread, so
        // `Hub::current()` equals `Hub::main()` ONLY when this test is the first
        // to touch Sentry (pinning the process hub to its thread). A sibling test
        // that touches Sentry first breaks that identity; binding the current
        // thread's hub makes the sanity send deterministic. This does NOT affect
        // the post-OFF regression proof below: the worker snapshots the client
        // from the PROCESS hub, and OFF closes the shared transport globally.
        sentry::Hub::current().bind_client(Some(std::sync::Arc::clone(&client)));

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
        reset_analytics_statics_and_hub();
    }

    /// Reset the module statics + process hub to a clean OFF baseline so the
    /// global-state tests don't leak into one another (they share `GUARD`,
    /// `CLIENT`, and `Hub::main`). Must run under `--test-threads=1`.
    fn reset_analytics_statics_and_hub() {
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

    fn client_static_is_some() -> bool {
        client_cell()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_some()
    }

    fn guard_static_is_some() -> bool {
        guard_cell()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_some()
    }

    /// ecc4: integration round-trip of the analytics subsystem through the exact
    /// runtime sequence the `set_analytics_enabled` Tauri command drives —
    /// `init_if_enabled(true)` (ON only) followed by
    /// `set_analytics_enabled_runtime(enabled)`. Asserts the Hub/client state
    /// re-inits on ON and tears down on OFF across an OFF -> ON -> OFF toggle.
    ///
    /// The Tauri command's only side effects beyond this pair are persisting the
    /// flag to disk + updating the in-memory settings cache (which need a Tauri
    /// `AppHandle`/`AppState`); the analytics subsystem behavior the seed targets
    /// lives entirely in this runtime pair, so we exercise it directly.
    #[test]
    fn set_analytics_enabled_runtime_round_trip_reinits_and_tears_down() {
        // Start from a known-clean baseline (other global-state tests share the
        // same statics + process hub).
        reset_analytics_statics_and_hub();
        assert!(
            !client_static_is_some(),
            "precondition: no live client before the round-trip"
        );
        assert!(sentry::Hub::main().client().is_none());

        // --- Initial OFF (command path for `enabled = false` skips init) ---
        set_analytics_enabled_runtime(false);
        assert!(
            !client_static_is_some(),
            "OFF must leave no live client in the static"
        );
        assert!(
            !guard_static_is_some(),
            "OFF must drop the init guard (close is terminal)"
        );
        assert!(
            sentry::Hub::main().client().is_none(),
            "OFF must leave the process hub with no bound client"
        );

        // --- ON: re-init a fresh client + bind on the process hub ---
        // Mirrors `set_analytics_enabled(true)` step 1: init-if-needed then bind.
        init_if_enabled(true);
        set_analytics_enabled_runtime(true);
        assert!(
            client_static_is_some(),
            "ON must (re)initialize a live client in the static"
        );
        assert!(
            guard_static_is_some(),
            "ON must hold the init guard for the live client"
        );
        let on_client = sentry::Hub::main().client();
        assert!(
            on_client.is_some(),
            "ON must bind the client on the process hub"
        );
        // The hub's bound client and the captured static must be the same Arc —
        // proving the OFF kill-switch (which closes via the static) targets the
        // very client the hub is sending through.
        {
            let static_client = client_cell()
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .as_ref()
                .map(Arc::clone)
                .expect("client static populated while ON");
            assert!(
                Arc::ptr_eq(&static_client, &on_client.unwrap()),
                "hub-bound client must be the same Arc captured in the static"
            );
        }

        // --- OFF again: tear the subsystem back down (unbind + close + drop) ---
        set_analytics_enabled_runtime(false);
        assert!(
            !client_static_is_some(),
            "second OFF must clear the client static so a later ON re-inits fresh"
        );
        assert!(
            !guard_static_is_some(),
            "second OFF must drop the guard again"
        );
        assert!(
            sentry::Hub::main().client().is_none(),
            "second OFF must unbind the process hub"
        );

        // Leave the shared statics + hub clean for sibling tests.
        reset_analytics_statics_and_hub();
    }

    // The flush primitives added for the FLUSH-TIMING fix must be safe, panic-
    // free no-ops when analytics is disabled / never inited / OFF (no live
    // client) — this is what makes them safe to call unconditionally from the
    // startup-capture path and the exit handlers. Mirrors the global-state
    // hygiene of the toggle tests: start from a clean OFF baseline and restore
    // it. Must run under `--test-threads=1`.
    #[test]
    fn flush_helpers_are_safe_no_ops_when_no_client_bound() {
        // Clean OFF baseline: no captured client, nothing bound on the hub.
        reset_analytics_statics_and_hub();
        // `live_client` falls back to `Hub::current().client()`, and a sibling
        // test may have left a client bound on THIS thread's current hub (which
        // is not `Hub::main()`). Unbind it too, or the flush helpers would take
        // the live-client path instead of the `None => true` no-op branch we
        // mean to exercise here.
        sentry::Hub::current().bind_client(None);
        assert!(
            !client_static_is_some(),
            "precondition: no live client before the no-op flush checks"
        );
        assert!(sentry::Hub::main().client().is_none());
        assert!(
            sentry::Hub::current().client().is_none(),
            "precondition: current-thread hub must have no client so the flush \
             helpers exercise the no-client no-op path"
        );

        // `flush` with no live client returns `true` (nothing to flush) and does
        // not panic, regardless of the timeout passed.
        assert!(
            flush(Duration::from_millis(0)),
            "flush must be a true no-op with no client bound"
        );
        assert!(
            flush(Duration::from_millis(500)),
            "flush must be a true no-op with no client bound"
        );

        // `flush_on_exit` is the exit-handler wrapper; same no-op guarantee.
        assert!(
            flush_on_exit(),
            "flush_on_exit must be a true no-op with no client bound"
        );

        // `flush_after_capture` must not panic and must not spawn a lingering
        // thread when there's no client (it early-returns before spawning). We
        // can only assert it returns without panicking here.
        flush_after_capture();

        // Restore the clean baseline for sibling global-state tests.
        reset_analytics_statics_and_hub();
    }
}
