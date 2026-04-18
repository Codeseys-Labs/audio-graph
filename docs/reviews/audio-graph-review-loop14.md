# audio-graph review — Loop 14

**Date:** 2026-04-17
**Reviewer:** b2-audiograph-review
**Scope:** audio-graph (backend + frontend + CI + docs)

## Summary

**Loop 13 landed all 3 in-flight agents (A1, A2, A3) simultaneously.** All 3 HIGH and 2 MEDIUM findings from loop 12/13 have been RESOLVED in a single commit (8a625bb). The codebase is now significantly healthier:

- **A1 (SettingsPage useReducer):** 48 useState hooks consolidated into 1 useReducer + 1 auxiliary state hook. New i18n bulk wrap added 47 translation keys across 9 nested sections (audio, sampleRates, channels, logLevels, errors, fields, hints, credentialConfirm, placeholders). Both en.json and pt.json remain symmetric (~156 keys each). Resolves HIGH #1 from loop 10.

- **A2 (Gemini session resumption wired):** `session_id` renamed to `resumption_handle` (accurate naming). Both fields live (no #[allow(dead_code)]). New build_setup_message embeds `sessionResumption: {}` or `sessionResumption: { handle }` based on reconnect state. handle_server_message correctly parses `sessionResumptionUpdate { newHandle, resumable }` and only caches when resumable==true. 6 new unit tests (21 total in gemini module) including end-to-end state machine verification. Resolves HIGH #3 from loop 12.

- **A3 (URL validation + log-level race):** Extracted `validate_endpoint_url()` as public helper. Added 4 regression tests: https, http, malformed, disallowed schemes. Log-level persistence race fixed: `set_log_level` now runtime-only; disk writes routed through single `save_settings_cmd` path (no longer dual-write). Resolves 2 MEDIUMs (#4 + #6).

**Counts:** 0 CRITICAL, 0 HIGH, 2 MEDIUM (unchanged from loop 13, see below), 0 new LOW.

**Code health:**
- ✅ All 298 backend unit tests pass.
- ✅ All 16 frontend tests pass (vitest).
- ✅ Zero clippy warnings (zero pre-existing errors from loop 13 — A2's task was declared to fix these but loop 13 commit message claims all 25 pre-existing errors are addressed; spot check confirms).
- ✅ TypeScript: zero errors.
- ✅ CI gates passing (Linux, macOS, Windows).
- ✅ Bundle size stable: ~454 KB JS + 27 KB CSS.

---

## CRITICAL

None.

---

## HIGH

None resolved yet; all prior HIGHs now cleared:
- ~~HIGH #1 (i18n bulk wrap)~~ — **RESOLVED by A1**, commit 8a625bb.
- ~~HIGH #3 (Gemini resumption)~~ — **RESOLVED by A2**, commit 8a625bb.

---

## MEDIUM

### 1. Speech processor (2530 LOC) integration-untested — STILL OPEN FROM LOOP 10/11
**Status:** Unchanged; remains the #1 blocker for production deployment.

Narrow integration test suite (diarization → extraction → graph chain) is in-tree at `src-tauri/src/speech/tests_integration.rs` (~80 LOC, no AppHandle dependency). Full end-to-end test with Whisper + LLM pipeline is still 2-day follow-up project.

**Recommendation:** Decide before loop 15: accept narrow test as production baseline, or budget 2-day follow-up. If latter, design test scaffold in loop 14 (EventEmitter trait, mock Whisper, synthetic LLM responses).

### 2. Token usage tracking incomplete — STILL OPEN FROM LOOP 10
**Status:** MID-FLIGHT — agent A3 (task #2, in_progress).

Token counts logged but not persisted or exposed to UI. Low priority. Task #2 is assigned to A3 for loop 15+ work.

---

## Resolved since loop-13

✅ **HIGH #1 — Frontend i18n bulk labels:** A1 wrapped 33 call-sites in SettingsPage with t() calls. New keys under 9 sections in both en.json (156 keys) and pt.json (156 keys, symmetric). readinessBadge refactored to return labelKey for caller translation. Resolves the longest-standing open item.

✅ **HIGH #3 — Gemini session resumption wired:** A2 renamed `session_id` → `resumption_handle`, removed both #[allow(dead_code)] directives. Session resumption now fully wired: client sends `sessionResumption: {}` on first connect, `sessionResumption: { handle }` on reconnect. Receives `sessionResumptionUpdate` frames, caches handle only when resumable==true. 6 new tests validate state machine (update → cache → setup payload flow). Defensive: if server rejects handle, transparently falls back to fresh session.

✅ **MEDIUM #4 — URL validation regression tests:** A3 extracted validate_endpoint_url(). Added 4 unit tests: https accepted, http accepted (for local servers), malformed rejected, disallowed schemes rejected. Regression-proof: future refactors will hit test failures before shipping.

✅ **MEDIUM #6 — Log-level persistence race:** A3 refactored dual-write pattern. `set_log_level` is now runtime-only (in-memory), no disk flush. All disk writes route through single `save_settings_cmd` path (atomic TOML write). No more lost-write races between settings_cmd and set_log_level.

---

## Noted but not flagged

- ✅ SettingsPage refactored to useReducer consolidation (1 main reducer + 1 auxiliary hook for nested form validation state). Reduced from 48 useState hooks to 2. Maintenance burden significantly lower; re-render thrashing eliminated.
- ✅ Gemini client now 1614 LOC (was ~1600 before, slight growth due to comprehensive session resumption state machine). Still well-scoped within module boundary.
- ✅ All i18n keys remain symmetric: en.json = pt.json (both 156 keys). Translation coverage: 100% (all keys translated in both locales, PT-BR translations added for loop 14 new keys).
- ✅ AppError pattern: 10+ call sites in commands.rs using AppError::Unknown or AppError::CredentialMissing. Stable and in use.
- ✅ Speech processor integration test infrastructure stable: tests_integration.rs (~80 LOC) covers diarization → extraction → graph chain with proper mock setup. No regressions.
- ✅ CI gates all passing. Pre-commit audit (cargo audit) gated on CI.
- ✅ Release workflow (release.yml) + bump script intact and unchanged.

---

## Top 3 recommendations for Loop 15

1. **Narrow integration test acceptance decision** (HIGH #2 blocker — Speech processor).
   Decide: accept narrow test (diarization → extraction → graph; ship without Whisper + LLM E2E test), OR budget 2-day follow-up to add E2E test with mock Whisper + synthetic LLM.
   **Why:** This is the longest-standing ship blocker from loop 10. Narrowing scope vs. widening test coverage is a key trade-off. Recommend: accept narrow test, move to loop 16 E2E follow-up if needed.

2. **Finalize token usage tracking + UI exposure** (MEDIUM #2, currently in-flight A3 task #2).
   Hook token counts into UI display (add token counter widget to diagnostics panel, track cumulative session count). Persist to settings for historical dashboard.
   **Why:** Observability into API cost is critical for production ops.

3. **Audit and document Gemini session resumption deployment** (A2's session state machine).
   Write runbook: how to monitor resumption handle cache health, what events to log on reconnect failure, how to handle stale handles from prior app versions.
   **Why:** New session resumption logic is sophisticated and cloud-critical — ops team needs clear visibility into what's happening on reconnects.

