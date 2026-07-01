# ADR-0023: Anonymous Analytics — Raw Sentry Rust SDK over tauri-plugin-sentry

## Status

accepted — 2026-06-28

## Context

AudioGraph is a local-first Tauri 2.11 desktop app (Windows/macOS/Linux). The
maintainer wants an **opt-in, anonymous** error/diagnostics channel — "anonymous
analytics" — to see what goes wrong in real use, sitting alongside the existing
local file-logging toggle ([`src-tauri/src/logging/mod.rs`]). Both must be
independently toggleable (either, both, or neither), and analytics must default
**off** (opt-in) and never leak transcripts, audio, credentials, IPs, or
usernames.

The obvious candidate was the official-ish **`tauri-plugin-sentry`** (by
`timfish`, who maintains Sentry's Electron/Node native bits): a genuine Tauri-2
plugin that injects `@sentry/browser` into every webview AND routes browser
events back through the Rust SDK with merged OS/device context — plus optional
native-minidump capture via `sentry-rust-minidump`. A full pre-implementation
research dive (`docs/reviews/_sentry-2026-06-28/research-findings.md`, run by the
analytics deep-work loop) evaluated it against the bare `sentry` crate.

## Decision

**Use the raw `sentry` crate (0.48.3) on the backend; do NOT adopt
`tauri-plugin-sentry`.** A webview-side `@sentry/browser` integration is
deferred (v1 is backend-only).

### Why not the plugin

- **Version dealbreaker:** the plugin's latest release (v0.5.0, 2025-09-03)
  hard-pins `sentry = "0.42"`. Its public API takes the app's own
  `sentry::Client` by value (`tauri_plugin_sentry::init(&client)`), so the app
  and plugin MUST link the same semver-compatible `sentry`. `sentry` is pre-1.0,
  so every minor is a breaking bump — 0.42 and 0.48 are incompatible. Adopting
  the plugin forces the whole app down to 0.42 and surrenders our control of the
  SDK version, in exchange for webview auto-capture glue we can replicate in
  ~15 lines if/when we want it.
- **Minidump cost:** the plugin's default `minidump` feature re-spawns the exe
  as a separate crash-reporter process (`minidumper-child`) and vendors
  `sentry-rust-minidump 0.13`. That is real value for native-crash capture but
  is out of scope for an anonymous opt-in v1, and it compounds the version pin.

### What we adopt instead

- `sentry = { version = "0.48.3", default-features = false, features =
  ["backtrace", "contexts", "panic", "reqwest", "rustls"] }` — pure-Rust, no C
  toolchain, `rustls` to avoid OpenSSL/native-tls on Linux CI. Compiles in both
  the default (`local-ml`) and `--no-default-features --features cloud` builds.
- A new [`src-tauri/src/analytics`] module: opt-in `init_if_enabled`, a
  runtime toggle (`set_analytics_enabled_runtime`), `set_analytics_enabled` /
  `get_analytics_info` Tauri commands, and the `AppSettings.analytics_enabled:
  Option<bool>` setting (default `Some(false)`).
- UI: a "Privacy & Diagnostics" toggle in `LoggingSettings.tsx`, fully
  independent of the file-logging controls, off by default.

### Privacy invariants (load-bearing)

- `send_default_pii = false` (the Sentry doc example sets it `true`; we override
  because this channel is anonymous) — no IP, cookies, or request bodies.
- A `before_send` scrubber nulls `server_name`/`user`/`request`; reduces every
  free-text field (message, `logentry`, `transaction`, `culprit`, exception
  values) to redaction sentinels via the same scrubber used for provider error
  excerpts ([`crate::error::redacted_provider_diagnostic`]) and then drops all
  remaining prose so interpolated transcript text cannot leak; clears
  `tags`/`extra`/`logentry.params`/breadcrumbs; resets `fingerprint`; and scrubs
  EVERY stack frame (exception, thread, deprecated top-level) to basename paths
  with `vars`/`context_line`/`pre_context`/`post_context` cleared.
- A `before_breadcrumb` hook drops every breadcrumb.
- A load-bearing unit test (`scrub_event_strips_secret_transcript_and_identity`)
  plants a fake secret + transcript + user/IP into every scrubbable field and
  asserts they are gone; a second test
  (`off_is_thread_global_worker_hub_cannot_send_after_off`) proves the OFF kill
  switch is thread-global.

### Toggle semantics

OFF closes the shared client transport on the **process hub** (`Hub::main`) — a
thread-global kill, since every thread's hub holds a clone of the same
`Arc<Client>` — and drops the guard; a later ON re-inits a fresh client. This
does NOT rely on `Drop` of the static guard at process exit (Rust does not run
`Drop` for `static`s at normal termination).

### DSN

The DSN is a **client-side public ingest key**, safe to embed. It ships as a
default const, overridable via the `SENTRY_DSN` env var; an explicitly-empty
`SENTRY_DSN` is a kill switch (analytics stays a no-op even if "enabled").

## Consequences

- We keep full control of the `sentry` version and avoid the 0.42 pin.
- **Webview JS errors are not captured in v1** (no `@sentry/browser`). If wanted
  later, add `@sentry/browser` + `Sentry.init({ sendDefaultPii: false, beforeSend
  })` gated by the same toggle — a follow-up seed.
- **No sourcemap/debug-symbol upload** (would need a build-time
  `SENTRY_AUTH_TOKEN`; the maintainer declined a build secret). Webview frames
  would be minified and native minidumps unsymbolicated — acceptable for
  anonymous opt-in telemetry. Follow-up only.
- Native-crash minidump capture is deferred; if added later use
  `sentry-rust-minidump 0.16` directly (tracks `sentry ^0.48`, compatible with
  our choice — unlike the plugin's vendored 0.13).

## Alternatives considered

- **`tauri-plugin-sentry` (v0.5.0):** rejected — 0.42 pin conflicts with the
  0.48 requirement and removes SDK-version control (see above).
- **Local-logging only (no remote channel):** insufficient — the maintainer
  cannot see failures from real-world use without an opt-in report path. Local
  logging is kept as the independent, default-on sibling.
