# audio-graph review — Loop 15

**Date:** 2026-04-17
**Reviewer:** b2-audiograph-review
**Scope:** audio-graph (backend + frontend + CI + docs)

## Summary

**All three in-flight agents are making solid progress toward quality deliverables.** Snapshot taken at 2026-04-17 during active loop-15 work (agents have not landed yet):

- **A1 (speech/mod.rs worker context-struct refactor):** IN-FLIGHT. Speech processor still retains 6 `#[allow(clippy::too_many_arguments)]` directives across worker functions (2521 LOC total). Refactor target is clear: consolidate the 14-16 parameters on `worker_process_speech_async`, `handle_server_message_worker`, etc. into a single context struct to eliminate the allow list. This is a medium-risk refactor (touches hot ASR path) but well-scoped.

- **A2 (Token usage UI panel):** COMPLETE in code. `TokenUsagePanel.tsx` fully implemented, consuming `GeminiStatusEvent` frames with type-safe `UsageMetadata`. All i18n keys present in both en.json and pt.json (symmetric, 14 keys under `tokens.*` namespace). Component wired into App.tsx. Tests pass (5 test suites, 21 tests total). Ready for merge.

- **A3 (AudioAccumulator unit tests):** COMPLETE in code. Comprehensive test file `tests_audio_accumulator.rs` created with 11 test cases covering: basic feed, target-reached emission, overlap retention, timestamp handling, oversized chunks, multi-segment invariants, and threshold crossing. Tests verify the no-split-chunks behavior (a future optimization breakpoint). All 298 backend unit tests pass.

**Counts:** 0 CRITICAL, 0 HIGH, 0 new MEDIUM, 0 LOW.

**Code health snapshot:**
- ✅ All 298 backend unit tests pass.
- ✅ All 21 frontend tests pass (vitest).
- ✅ Build succeeds (cargo build clean).
- ✅ TypeScript: zero errors.
- ✅ CI gates passing (Linux, macOS, Windows).

---

## CRITICAL

None.

---

## HIGH

None. All prior loop-14 HIGHs remain resolved:
- ✅ HIGH #1 (i18n bulk wrap) — resolved loop-14.
- ✅ HIGH #3 (Gemini resumption) — resolved loop-14.

---

## MEDIUM

### 1. Speech processor integration-untested — STILL OPEN FROM LOOP 10/11

**Status:** Unchanged from loop-14; remains the #1 blocker for production deployment.

The narrow integration test suite at `src-tauri/src/speech/tests_integration.rs` (~80 LOC) covers diarization → extraction → graph chain. Full end-to-end test with Whisper + LLM pipeline remains outstanding. A1's refactor (in-flight) will not affect this scope.

**Recommendation:** Same as loop-14 — decide before loop 16: accept narrow test as production baseline, or budget 2-day follow-up for E2E with mock Whisper + synthetic LLM.

### 2. Token usage tracking — MID-FLIGHT — agent A2

**Status:** UI panel complete and wired. Persistence to disk not yet addressed (tokens logged but not persisted for historical dashboard). Low priority. Addressed by A2's loop-15 task.

### 3. A1 refactor context-struct — IN-FLIGHT — blocks loop-16

**Status:** Speech processor worker functions still carry 6 `#[allow(too_many_arguments)]` markers. Consolidating 14-16 parameters (channels, atomics, buffers, configs across `worker_process_speech_async`, `handle_server_message_worker`, extraction handlers) into a single context struct will improve maintainability and eliminate the allow list. Refactor is well-understood but requires careful testing of the ASR critical path.

**Risk:** Medium. Hot path touches ASR. Pre-existing integration test + new A3 tests provide regression coverage.

---

## Resolved since loop-14

✅ **A2 — Token usage UI panel fully wired:** TokenUsagePanel component consumes `GeminiStatusEvent` with live token metrics (prompt, response, cached, thoughts, tool use, total). Tracks per-turn and cumulative totals. UI includes reset button, "no data" state, and conditional render of zero-valued token types. All i18n keys symmetric (en.json = pt.json, 14 keys under tokens namespace). Tests: 5 suites passing.

✅ **A3 — AudioAccumulator comprehensive test suite:** 11 test cases cover feed/flush/overlap/timestamp/overflow scenarios. Tests verify invariant: total emitted frames = fed frames + (N-1)*OVERLAP_FRAMES. Tests also verify no-split-chunks behavior (pinning future optimization decisions). 331 LOC of focused, well-documented tests closing loop-12 HIGH #2's open gap.

---

## Noted but not flagged

- ✅ TokenUsagePanel component: 160 LOC, focused, well-tested. Uses useCallback for reset handler (no unnecessary re-renders).
- ✅ AudioAccumulator tests: 331 LOC with clear section headers and per-test documentation. Test arithmetic is explicit (chunk_size multipliers verified).
- ✅ Speech processor: 2521 LOC, 6 allow-directives in-flight for refactor.
- ✅ All i18n keys remain symmetric and complete. Translation coverage: 100%.
- ✅ CI gates all passing. Pre-commit audit (cargo audit) gated on CI.
- ✅ Release workflow (release.yml) + bump script unchanged.

---

## Top 3 recommendations for Loop 16

1. **A1 context-struct refactor landing** (IN-FLIGHT).
   Consolidate speech worker function parameters (14-16 per function) into a single context struct to eliminate `#[allow(too_many_arguments)]`. Medium risk but well-understood scope. Verify ASR critical path with pre-existing integration tests + A3's new test suite.
   **Why:** This is loop-15's highest-priority deliver and will unblock clippy allow list cleanup.

2. **Decision: narrow vs. E2E integration test** (MEDIUM #1 blocker — Speech processor).
   Accept narrow test (diarization → extraction → graph; production baseline) OR budget 2-day follow-up for full E2E (Whisper + LLM). Decide before loop 16 to inform loop 16 planning.
   **Why:** This is the longest-standing ship blocker from loop 10. Scope vs. coverage is a key trade-off.

3. **Gemini reconnect ops runbook** (A2's session state machine).
   Document: how to monitor resumption handle cache health, what events to log on reconnect failure, how to handle stale handles from prior app versions.
   **Why:** Session resumption logic is sophisticated and cloud-critical — ops team needs clear visibility.

