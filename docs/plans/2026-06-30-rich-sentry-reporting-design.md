# Rich Sentry Reporting — Design

Date: 2026-06-30
Status: approved (brainstorm), implementing on `fix/gtk-test-harness-65f0`

## Problem

The Sentry foundation (`src-tauri/src/analytics/mod.rs`) is mature and
privacy-safe, but capture coverage is one line: the whole app sends exactly one
intentional event — `capture_anonymous_event("app.startup")`. No backend error
capture, no panic→Sentry bridge, no frontend Sentry at all (`@sentry/*` is not a
dependency). Errors that reach the UI never reach Sentry.

Goal: rich, structured, **privacy-preserving** error/diagnostic reporting across
backend + frontend, so a maintainer running a placeholder build can triage what
goes wrong.

## Privacy invariant (unchanged, load-bearing)

`scrub_event` already: nulls identity, reduces ALL free prose (message,
exception values, transaction, culprit) to `<redacted>` sentinels, basenames
stack frames + clears vars/source, drops breadcrumbs/extra, keeps only
os/device/rust/runtime contexts. `send_default_pii=false`. This design does NOT
weaken any of that. Free text stays scrubbed.

## The one structural change: a validated safe-fields allowlist

Today `scrub_event` clears ALL tags → every error looks identical → low triage
value. Fix: a **typed capture API** + a **key allowlist** + **per-value shape
validation** (belt-and-suspenders).

### Backend API (`analytics/mod.rs`)

```rust
pub enum Category { Asr, Llm, Tts, Audio, Command, Startup, Panic, Other }

pub struct DiagEvent<'a> {
    pub name: &'a str,             // stable id, e.g. "asr.stream.error"
    pub category: Category,
    pub level: sentry::Level,
    pub provider: Option<&'a str>, // controlled id, e.g. "deepgram"
    pub kind: Option<&'a str>,     // controlled error-kind, e.g. "parse_error"
    pub http_status: Option<u16>,
    pub recoverable: Option<bool>,
}

pub fn capture_diagnostic(ev: DiagEvent<'_>);
```

Callers pass only enums/ids/numbers — physically cannot pass free-text tags.
`capture_diagnostic` sets the allowlisted tags on a scope, then captures at the
given level. No-op when analytics is off (unbound hub).

### Scrubber allowlist (the surviving structured lane)

`scrub_event` keeps ONLY these tag keys (drops every other tag):
`event.name`, `category`, `provider`, `kind`, `http_status`, `recoverable`,
`release`, `channel`.

Each surviving VALUE is validated:
1. run through `crate::error::redacted_provider_diagnostic` (existing secret
   scrubber), then
2. shape-checked: `name`/`provider`/`kind`/`category`/`channel` must match
   `^[a-z0-9._:-]{1,48}$`; `http_status` numeric 100–599; `recoverable` bool.
   Anything failing the shape check is DROPPED (not kept). release is SDK-set.

This is the only new lane. Everything else stays maximally scrubbed.

## Backend capture sites (chokepoints, ~6–10, one line each)

1. **Panic bridge** (`crash_handler/mod.rs`): before delegating to the default
   hook, best-effort `capture_diagnostic({ category: Panic, level: Fatal,
   name: "panic.<file-basename>:<line>" })`. Never panics inside the hook.
2. **ASR/TTS boundary errors** (`asr/*`, `tts/*`): where a `Result::Err` becomes
   user-visible — stream + readiness/model-discovery failures. Tag provider +
   kind + http_status.
3. **LLM request failures** (`llm/streaming.rs`, `llm/openrouter.rs`).
4. **Audio capture errors** (`audio/capture.rs`, already classified
   `recoverable`) → category Audio + recoverable.
5. **Tauri command boundary** (`commands.rs`): a thin helper that captures when a
   command returns Err → category Command, name = "<command>". Catches
   everything reaching the frontend without per-command instrumentation.

NOT captured: happy-path, per-`?` intermediate errors, hot audio-loop errors.

## Frontend (`@sentry/browser`)

- Add `@sentry/browser` dependency (browser SDK; not the framework wrapper).
- Init in `src/main.tsx`, gated on the `analytics_enabled` setting fetched at
  startup (mirror backend gating). Same embedded DSN (public key).
- Frontend `beforeSend` scrubber mirrors the backend allowlist: keep only
  `event.name` + tags in {category, component, surface}; drop message/breadcrumbs/
  request/user; no free text survives.
- `ErrorBoundary` component wrapping `<App/>` → captures React render errors
  (category=frontend, component=<boundary>).
- `window.addEventListener('error' | 'unhandledrejection')` → capture with
  category=frontend.
- `safeInvoke(cmd, args)` wrapper around Tauri `invoke` that captures failures
  (category=frontend, surface="invoke", name=cmd) and rethrows.

## Testing

Backend:
- Update `scrub_event_strips_secret_transcript_and_identity`: the
  `tags.is_empty()` assertion becomes "only allowlisted keys survive"; add a
  planted non-allowlisted tag + a bad-shape allowlisted value, assert both gone
  and the good allowlisted tags survive.
- New test: `capture_diagnostic` sets exactly the allowlisted tags; a
  secret-shaped provider value is scrubbed/dropped.

Frontend:
- `beforeSend` scrubber test: an event with a planted secret + free message
  comes out with only allowlisted structured fields, no PII.

## Rollout

- Analytics stays **off by default** (opt-in). To see telemetry on the
  placeholder build, toggle it on in Settings — OR set `analytics_enabled: true`
  as a build-channel default for preview builds (decide at release time).
- Sequencing: land this on `fix/gtk-test-harness-65f0` (PR #22, already green),
  keep green, squash-merge, then cut the placeholder release so the build has
  rich telemetry.

## Non-goals

- No release-health/session tracking (`auto_session_tracking` stays false).
- No performance tracing/spans (errors + diagnostics only for v1).
- No new free-text lane; the scrubber's prose-dropping is inviolable.
