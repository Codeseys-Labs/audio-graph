# safeInvoke adoption audit — audio-graph-3e71

Date: 2026-07-06
Seed: audio-graph-3e71 — "Adopt safeInvoke (analytics-capturing IPC wrapper)
across direct invoke call sites"

## Problem

`src/analytics/safeInvoke.ts` exists to relay Tauri command failures to the
anonymous backend diagnostics channel (ADR-0023) and rethrow, but before this
change it had ~1 production adopter (`useNativeCapture`, PR #73) plus 5 calls in
the store. Every other direct `invoke(...)` call site bypassed error-capture
analytics entirely, so real-world IPC failures were invisible to telemetry.

## Design chosen: import-alias chokepoint

Rather than rewrite ~90 call bodies (`invoke(...)` → `safeInvoke(...)`), each
module swaps ONE import line:

```ts
// before
import { invoke } from "@tauri-apps/api/core";
// after
import { safeInvoke as invoke } from "../analytics/safeInvoke";
```

Every `invoke(...)` call in the file then resolves to `safeInvoke` unchanged.
This yields the smallest reviewable diff (one import per file), zero call-body
churn, and — critically — keeps every existing test green, because tests mock
`@tauri-apps/api/core`'s `invoke` at the module boundary and `safeInvoke` calls
that same mocked `invoke` internally.

### Two prerequisite fixes to make safeInvoke a true drop-in

Adopting the alias surfaced two behaviors that would otherwise change on
migration; both are fixed in this PR:

1. **Arity preservation** (`safeInvoke.ts`). The prior implementation always
   forwarded a trailing `options` arg to `invoke` (a 3-arg call even when the
   caller passed 1-2 args). Vitest's `toHaveBeenCalledWith` is arity-sensitive
   (empirically confirmed), so `expect(invoke).toHaveBeenCalledWith("cmd", args)`
   at migrated sites would fail against a recorded 3-arg call. `safeInvoke` now
   forwards the caller's EXACT positional arity.

2. **Robust fail-silent relay** (`sentry.ts`). `captureFrontendError` did
   `invoke("report_frontend_diagnostic").catch(...)`. Under a test mock (or a
   missing command / non-Tauri runtime), `invoke` returns a non-thenable, so
   `.catch` threw synchronously — and on the failure path that thrown error
   CLOBBERED the caller's original error (proved: `loadSessionTimeline`'s "fold
   blew up" became "Cannot read properties of undefined"). The relay is now
   wrapped in `Promise.resolve(...)` + try/catch so it can never throw into the
   caller, honoring its documented fail-silent contract.

### Double-report prevention

`safeInvoke` reports to analytics **then rethrows the original error unchanged**.
Callers keep their existing `catch → errorToMessage → setError` (store) or
`catch → toast/alert` (components) behavior — the display-layer humanization
(PR #71) is untouched. Because the report happens exactly once at the single
IPC chokepoint and callers only *display* (never re-report) the rethrown error,
each failure is captured EXACTLY ONCE.

### Privacy invariant (ADR-0023)

The diagnostic carries only the command NAME (as `component`) plus the fixed
`category`/`surface` ids. It NEVER serializes the invoke `args`/payload, nor the
caught error's message/stack. Verified by unit + integration tests that plant a
secret + transcript in both the args and the error and assert they never appear
in the serialized event. (safeInvoke already omitted args before this change;
this PR pins it with tests since it was 0%-covered.)

## Exceptions (intentionally NOT migrated)

- `src/analytics/sentry.ts` — the relay itself calls
  `invoke("report_frontend_diagnostic")`. Routing it through `safeInvoke` would
  recurse infinitely on failure. It keeps the direct `@tauri-apps/api/core`
  import.
- `src/analytics/safeInvoke.ts` — the wrapper; imports the real `invoke`.
- Tests — mock `invoke` at the module boundary; left untouched (the alias swap
  keeps every mock working).

## Inventory of migrated production call sites (~97 across 10 files)

Each file below had its `invoke` import aliased to `safeInvoke`; all call sites
in the file are now captured, with ONE sanctioned exception: `start_streaming_chat`
in `src/store/index.ts` stays on raw `invoke` because its rejection is a documented
capability probe (backend returns `Err` to signal "provider doesn't stream" so the
caller falls back to `send_chat_message` — commands.rs:2292-2294); capturing it
would log an expected control path as an error on every non-streaming chat.

| File | Sites | Classification |
| --- | --- | --- |
| `src/store/index.ts` | ~48 | (b) store actions with their own `catch → setError` via `errorToMessage`; a few fire-and-forget (`add_question_to_graph`) and passthrough getters (`export_transcript`/`export_graph`/`get_session_id`, already safeInvoke) |
| `src/components/settings/useSettingsController.tsx` | ~22 | (c) component-local try/catch (credential save/delete, provider connection tests, model catalog fetches) |
| `src/components/TokenUsagePanel.tsx` | ~9 | (a) telemetry-adjacent fire-and-forget (usage seed/refresh) + (c) local try/catch |
| `src/components/ExpressSetup.tsx` | ~6 | (c) first-run credential + settings save, local try/catch |
| `src/components/LoggingSettings.tsx` | ~6 | (c) log/analytics config reads + writes, local try/catch |
| `src/components/ProjectionRuntimeStatusPanel.tsx` | ~2 | (c) diagnostics refresh + replay report |
| `src/App.tsx` | ~1 | (d) root credential-presence bootstrap |
| `src/components/NotesPanel.tsx` | ~1 | (c) `synthesize_notes`, local try/catch |
| `src/components/SessionDataRoutePanel.tsx` | ~1 | (c) data-route report fetch |
| `src/components/StorageBanner.tsx` | ~1 | (c) `retry_storage_write`, local try/catch |

Classification legend (from the seed):
- (a) fire-and-forget / telemetry-adjacent
- (b) store actions with their own `catch → setError`
- (c) component-local try/catch
- (d) hooks / bootstrap

## Tests added

- `src/analytics/safeInvoke.test.ts` (safeInvoke was 0%-covered): success
  passthrough (no capture), failure → capture once with command name → rethrow
  original, privacy (args + error never serialized), exact-arity forwarding, and
  the analytics-relay-failure-does-not-clobber-caller regression.
- `src/store/index.test.ts` → new describe block: a migrated store action
  (`fetchModels`) on failure still sets error state AND relays exactly one
  command-name diagnostic (never args); on success relays none.
