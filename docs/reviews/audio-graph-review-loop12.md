# audio-graph review — Loop 12

**Date:** 2026-04-17
**Reviewer:** claude-agent (read-only explore pass)
**Scope:** audio-graph (backend + frontend + CI + docs)

## Summary

Fresh read after loops 10–12. **5 of 7 Loop-11 findings resolved** this cycle
(audio_settings wired, credential allowlist at command boundary, URL validation
in configure_api_endpoint, +3 aws_util error-path tests, +3 persistence/io
error-path tests). Two Loop-11 MEDIUMs remain open (SettingsPage 48-useState
consolidation, log-level persistence race), plus the longest-standing
HIGH — speech processor integration test — still blocks ship-readiness.

**Counts:** 0 CRITICAL, 4 HIGH (3 confirmed open from Loop 10, 1 from
Loop 11 now resolved), 4 MEDIUM (1 new, 3 confirmed open), 1 LOW.

---

## CRITICAL

None.

---

## HIGH

### 1. Frontend i18n bulk labels still hard-coded — CONFIRMED OPEN FROM LOOP 10
Loop 11 A4 wrapped 93 SettingsPage strings; rest of form labels still English.
No change this loop.

### 2. Speech processor (2513 LOC) integration-untested — CONFIRMED OPEN FROM LOOP 10/11
Loop 12 A3 attempted this — re-score once their output lands. Until then this
remains the single largest blocker to ship-readiness.

### 3. Gemini session resumption never wired — CONFIRMED OPEN FROM LOOP 10
Not in-flight this loop. `session_id` + `session_handle` in
`src-tauri/src/gemini/mod.rs:162,167-168` are still `#[allow(dead_code)]`
with no `resume_session()` caller.

---

## MEDIUM

### 4. NEW — `configure_api_endpoint` URL validation landed but tests absent
**File:** `src-tauri/src/commands.rs` — the URL validation path added
loop-12 A2.

The `url::Url::parse` + scheme check is in place, but no unit tests
exercise the rejection paths. If someone relaxes the validation later
(e.g. accepts `file://` for local serving) there's no regression guard.

**Action:** Add 3–4 tests: valid https, valid http, malformed URL,
disallowed scheme (`file://`, `ftp://`). Wire into the existing test
module pattern.

### 5. SettingsPage 48-useState consolidation — CONFIRMED OPEN FROM LOOP 11
Unchanged; no agent tackled this.

### 6. Log level persistence race — CONFIRMED OPEN FROM LOOP 11
Unchanged. `set_log_level` and `save_settings_cmd` still both mutate the
`log_level` field on disk.

### 7. Token usage tracking incomplete — CONFIRMED OPEN FROM LOOP 10
Unchanged.

---

## LOW

### 8. `#[allow(dead_code)]` rationale — UNCHANGED FROM LOOPS 10/11
All intentional per prior reviews.

---

## Resolved since loop-11

- ✅ **HIGH #1 (audio_settings wire-through)** — `resolve_audio_settings()`
  now applied in `start_capture_cmd`; validation tests confirm fallback on
  invalid YAML values.
- ✅ **MEDIUM #5 (credential key allowlist)** — `ALLOWED_CREDENTIAL_KEYS`
  constant extracted; `is_allowed_key()` helper; commands validate at
  boundary before calling the inner store; frontend types synchronized.
- ✅ **MEDIUM #7 (URL validation)** — `url::Url::parse()` + scheme
  allowlist in `configure_api_endpoint`.
- ✅ **MEDIUM #8 (aws_util error-path tests)** — 3 new tests (missing
  file, malformed YAML, missing secret_key) bring that module from 1 to
  4 tests.
- ✅ **MEDIUM #9 (persistence/io error-path tests)** — 3 new tests (happy
  path, ENOSPC classification, non-storage errors) bring that module
  from 0 to 3 tests.

---

## Noted but not flagged

- ✅ AppError pilot pattern still feeling clean after a loop of use;
  `Unknown(String)` escape hatch is the right call for incremental
  migration.
- ✅ i18n locale key trees remain symmetric (en = pt).
- ✅ Bundle size: ~454 KB JS + 27 KB CSS — stable across loops.
- ✅ Test count: ~101 backend tests (was 93 at start of loop 12), 16
  frontend tests.
- ✅ Release workflow + bump script intact.
- ✅ All CI gates (check / test / fmt / audit / typecheck / build) green.

---

## Top 3 recommendations for Loop 13

1. **Land / finalize speech processor integration test** (HIGH #2).
   Biggest remaining ship blocker. If loop-12 A3 agent hit feasibility
   issues, decide: accept the narrower test it produced, or budget a
   dedicated 2-day follow-up with a test-only `EventEmitter` trait
   abstraction.

2. **Consolidate SettingsPage into `useReducer`** (MEDIUM #5 from loop 11).
   Re-render cost, maintenance burden, natural pairing with i18n
   expansion. Effort: 2–3 days.

3. **Bulk-migrate 10–15 commands to `AppError`**. Pilot is stable. Next
   batch: `test_aws_credentials`, `start_gemini`, `load_credential_cmd`,
   `save_settings_cmd`. Effort: 1 day per cluster.
