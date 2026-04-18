# audio-graph review — Loop 13

**Date:** 2026-04-17
**Reviewer:** b2-audiograph-review
**Scope:** audio-graph (backend + frontend + CI + docs)

## Summary

Quiet loop: in-flight agents A1, A2, A3 are still developing. Master branch unchanged since loop-12 review commit (d54a5bf). **All 7 loop-12 findings remain in their prior state**: HIGH #1 (i18n bulk wrap) and HIGH #3 (Gemini resumption) still open; MEDIUM #4–#7 unchanged. The codebase remains stable (2530 LOC speech processor, 48 useState hooks in SettingsPage, 101+ backend tests). Audit gates + CI passing.

**Counts:** 0 CRITICAL, 3 HIGH (same as loop-12), 4 MEDIUM (same as loop-12), 0 new LOW.

---

## CRITICAL

None.

---

## HIGH

### 1. Frontend i18n bulk labels — STILL OPEN FROM LOOP 10
**Status:** MID-FLIGHT — agent A1 in progress.

Loop-12 A4 wrapped 93 SettingsPage strings in `src/pages/SettingsPage.tsx`; the A1 task for loop-13 aims to complete the bulk wrap of remaining form field labels. Codebase unchanged; task is assigned to A1 (in-flight). Once landed, this resolves the longest-standing HIGH.

**Expectation:** Task resolves by end of loop-13.

### 2. Speech processor (2530 LOC) integration-untested — STILL OPEN FROM LOOP 10/11
**Status:** Partial integration test infrastructure in place (src-tauri/src/speech/tests_integration.rs, ~80 LOC).

The narrower test suite (diarization → extraction → graph chain, minus AppHandle) is in the codebase and documented. Full end-to-end test with Whisper + LLM pipeline is still out of scope (2-day follow-up project). Blocks ship-readiness; remains the #1 blocker for production deployment.

### 3. Gemini session resumption never wired — STILL OPEN FROM LOOP 10
**Status:** MID-FLIGHT — agent A2 in progress.

`src-tauri/src/gemini/mod.rs:162,167-168` — `session_id` and `session_handle` fields still marked `#[allow(dead_code)]`. No `resume_session()` caller exists. Task A2 is in-flight to wire the resumption flow. Expected to land by end of loop-13.

**File:** src-tauri/src/gemini/mod.rs:162–168
**Expectation:** Task resolves by end of loop-13.

---

## MEDIUM

### 4. `configure_api_endpoint` URL validation has no regression tests — STILL OPEN FROM LOOP 12
**Status:** MID-FLIGHT — agent A3 in progress.

**File:** src-tauri/src/commands.rs:783–831

Loop-12 A2 added `url::Url::parse()` + scheme validation (`http`, `https` allowed). The code is correct and in production use, but there are **no unit tests** exercising the rejection paths:
- Valid `https://` endpoint → accepted
- Valid `http://` endpoint → accepted
- Malformed URL → rejected with clear error
- Disallowed scheme (`file://`, `ftp://`) → rejected

Without these guards, a future refactor (e.g., "let's allow local file serving") could silently break the validation. Task A3 is in-flight to add 3–4 tests. Expected to land by end of loop-13.

### 5. SettingsPage 48-useState consolidation — CONFIRMED OPEN FROM LOOP 11
**Status:** NOT in this loop — remains open.

**File:** src/components/SettingsPage.tsx:41–145 (48 useState hooks)
**LOC:** 1737 total

Loop-11 flagged this for maintenance burden and re-render cost; loop-12 had no agent tackle it. No change this loop. The form is fully functional but could benefit from `useReducer` consolidation — a logical follow-up to A1's i18n work since both deal with SettingsPage state management. Estimated effort: 2–3 days.

### 6. Log level persistence race — CONFIRMED OPEN FROM LOOP 11
**Status:** MID-FLIGHT — agent A3 in progress.

**File:** src-tauri/src/commands.rs:1156–1177 (`set_log_level`), src-tauri/src/commands.rs:1130–1141 (`save_settings_cmd`)

Both commands mutate the `log_level` field on disk:
- `save_settings_cmd` → `settings.log_level = ...` → persists entire settings object
- `set_log_level` → loads, mutates `settings.log_level`, saves

If a user rapidly clicks "Save Settings" (with log level changed) and also calls `set_log_level` from the UI, a lost-write or stale-read could occur (both operate on filesystem without locking). Task A3 is in-flight to add a mutex or consolidate the writers. Expected to land by end of loop-13.

### 7. Token usage tracking incomplete — CONFIRMED OPEN FROM LOOP 10
**Status:** Not in-flight; remains open.

Unchanged from loop 11. No agent assigned. Token counts are logged but not persisted or exposed to the UI. Low priority for loop-13.

---

## Resolved since loop-12

None (master unchanged since loop-12 review).

---

## Noted but not flagged

- ✅ AppError pilot pattern stable and in use (10+ call sites in commands.rs using AppError::Unknown or AppError::CredentialMissing).
- ✅ i18n locale key trees remain symmetric (en.json = pt.json, ~109 keys each).
- ✅ Bundle size stable: ~454 KB JS + 27 KB CSS.
- ✅ Test count: 101+ backend tests (speech integration suite in place); 16+ frontend tests.
- ✅ CI gates passing (check / test / fmt / audit / typecheck / build on Linux, macOS, Windows).
- ✅ Release workflow (release.yml) + bump script intact.
- ✅ All speech processor plumbing (diarization → extraction → graph) covered by integration tests.

---

## Top 3 recommendations for Loop 14

1. **Land A1/A2/A3 loop-13 work** (expected by end of this loop).
   - A1: Finish i18n bulk wrap (resolves HIGH #1).
   - A2: Wire Gemini session resumption (resolves HIGH #3).
   - A3: Add `configure_api_endpoint` tests + lock log-level race (resolves 2 MEDIUMs).
   Combined, this clears 3 HIGHs and 2 MEDIUMs.

2. **Consolidate SettingsPage into `useReducer`** (MEDIUM #5 from loop 11, still open).
   Effort: 2–3 days. Natural pairing with A1's i18n completion. Reduces re-render thrashing and maintenance burden. Candidate for a dedicated loop-14 agent if A1/A2/A3 landing is smooth.

3. **Plan full speech integration test** (HIGH #2 — the longest-standing ship blocker).
   Narrow test is in-tree; decision needed: accept it as the production baseline, or budget a 2-day follow-up. If the latter, design the test scaffold (EventEmitter trait, mock Whisper, synthetic LLM responses) in loop-14 and execute in loop-15.

