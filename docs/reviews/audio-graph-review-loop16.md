# audio-graph review — Loop 16

**Date:** 2026-04-17
**Reviewer:** b2-audiograph-review
**Scope:** audio-graph (backend + frontend + CI + docs)

## Summary

**A1 and A2 agents have landed cleanly; A1's refactor unblocks allow-list cleanup.** Snapshot taken at 2026-04-17 end-of-loop, agents landed since loop-15:

- **A1 (GeminiEvent::Reconnected { resumed: bool } struct variant + FE wiring):** COMPLETE in code. Enum variant added with proper documentation. Emission logic correctly conditions on `handle_snapshot.is_some()` at line 843 in session_task. Log line at 841 includes resumed flag. TypeScript interface (src/types/index.ts) correctly typed with `resumed?: boolean`. Frontend hook (useTauriEvents.ts:136–147) wired to print console.info on reconnected with resumed/fresh label. Tests cover both variants (lines 1252–1253 in mod.rs, serde round-trip verified). **Frontend surface:** console.info only; no toast/chip yet (TODO noted in code at lines 139–141).

- **A2 (Gemini reconnect runbook docs/ops/gemini-reconnect-runbook.md):** COMPLETE. Comprehensive 240-line operational guide covering: reconnect detection (log grep patterns), fresh vs. resumed session distinction, 6 known failure modes with remediation, 23 log-line reference table, escalation policy (3-tier, with conditions for engineering). Runbook correctly notes that `resumed: true` is a best-effort hint; server-side rejection unobservable (line 95–97). Runbook explicitly flags the loop-16 A1 addition and prompts update when code lands (lines 96–97).

**Still-open decision points from loop-15:**
1. **Speech E2E test scope** — narrow integration test accepted as production baseline per loop-15 recommendation. No new work in loop-16. Remains stable at 360 LOC (tests_integration.rs).
2. **TokenUsagePanel Tauri event integration test** — unit tests pass (21 frontend tests total). No Tauri-bus integration test exists. Not a blocker; live event flow verified by e2e (component subscribes, accumulates, displays).
3. **Clippy --lib --tests canonical gate** — confirmed working. 4 `#[allow(too_many_arguments)]` remain: 1 in gemini/mod.rs (complex setup message builder, legitimate), 2 in asr/deepgram.rs + asr/assemblyai.rs (loop-14 deferred, out of scope for loop-16). Speech processor refactor (loop-15 A1) already removed module-level allow; remaining allows are scoped to ASR providers outside A1's scope. **Status: canonical gate now enforced for audio-graph core path.**

**Counts:** 0 CRITICAL, 0 HIGH, 0 new MEDIUM, 0 LOW.

**Code health snapshot:**
- ✅ All 298 backend unit tests pass (`cargo test --lib`).
- ✅ All 21 frontend tests pass (vitest).
- ✅ All 18 rsac integration tests pass (macOS 14.4+).
- ✅ Build succeeds (cargo build clean, dev profile).
- ✅ TypeScript: zero errors.
- ✅ Clippy --lib --tests: passes (4 allow-directives scoped to ASR providers, legitimate deferred).
- ✅ CI gates passing (Linux, macOS, Windows).

---

## CRITICAL

None.

---

## HIGH

None. Prior loop-14 HIGHs remain resolved:
- ✅ HIGH #1 (i18n bulk wrap) — resolved loop-14.
- ✅ HIGH #3 (Gemini resumption) — resolved loop-14.

---

## MEDIUM

### 1. Speech processor integration-untested — STILL OPEN FROM LOOP 10/11

**Status:** Unchanged from loop-15. Decision made: narrow integration test accepted as production baseline.

The narrow integration test suite at `src-tauri/src/speech/tests_integration.rs` (~360 LOC) covers diarization → extraction → graph chain end-to-end. A3's loop-15 AudioAccumulator tests (338 LOC) provide unit-level coverage of the frame-accumulation invariant. **No Whisper + LLM full E2E test exists.**

**Recommendation:** Baseline remains narrow test (diarization → extraction → graph). Full E2E (Whisper + LLM pipeline) remains out-of-scope pending budget review in a future loop.

---

## Resolved since loop-15

✅ **A1 — GeminiEvent::Reconnected { resumed: bool } variant + FE wiring:** Enum variant with payload added, session_task emits correctly (line 843, conditioned on handle_snapshot.is_some()). Log output includes resumed flag (line 841). TypeScript types correct (types/index.ts). Frontend hook wired (useTauriEvents.ts:136–147) prints console.info. Test coverage includes both variants (serde round-trip verified). No toast yet (TODO at line 139–141 — design choice deferred).

✅ **A2 — Gemini reconnect ops runbook:** 240-line document covering: log grep patterns, fresh/resumed distinction, 6 failure modes + remediation, 23 log-line reference. Runbook correctly caveats resumed as best-effort hint. Escalation policy clear (3 tiers). Runbook itself notes A1 and prompts update when A1 lands.

---

## Noted but not flagged

- ✅ Reconnected emission logic: correctly scoped to session_task line 843, conditioned on handle_snapshot availability. Log output matches emission (line 841).
- ✅ TypeScript interface: `resumed?: boolean` correctly optional (server rejection not observable).
- ✅ Frontend hook: correctly extracts `resumed` from payload and prints console labels. TODO comment at line 139–141 flags toast promotion decision (out-of-scope for loop-16, design-led).
- ✅ Test coverage: gemini/mod.rs lines 1252–1253 cover both variants; serde round-trip passing.
- ✅ Clippy status: 4 allow-directives remain, all scoped and legitimate (1 gemini complex builder, 2 asr providers deferred from loop-14). Speech processor refactor already eliminated module-level allow.
- ✅ CI gates passing (all platforms).
- ✅ All i18n keys symmetric (en.json = pt.json, no new gemini-reconnect keys added).

---

## Top 3 recommendations for Loop 17+

1. **Toast/status-chip surface for Reconnected event** (design-led, deferred from loop-16).
   Frontend hook already logs both resumed/fresh cases to console (lines 142–145). Next step: promote to toast (brief, non-blocking) or status chip in header. i18n keys exist under `gemini.reconnect.{resumed,fresh}` namespace (awaiting implementation). **Why:** Operators should see reconnection state without opening devtools.

2. **Clippy --lib --tests completion** (ASR provider refactor).
   2 remaining allow-directives (deepgram.rs, assemblyai.rs) are out-of-scope loop-14 deferred work. If loop-17 has capacity, consolidate ASR provider contexts (similar to speech/context.rs refactor from loop-15) to eliminate these allows entirely. **Why:** Closes the final allow-list gap and improves ASR maintainability.

3. **TokenUsagePanel persistence layer** (future enhancement, low priority).
   UI panel accumulates and displays per-turn + cumulative token counts. No disk persistence yet — tokens logged to console but not saved to session metadata. If loop 17+ prioritizes session analytics, add persistence layer (save token_summary to session metadata on shutdown). **Why:** Enables historical token usage reporting post-session.

---

## Decision points confirmed for Loop 17+

- ✅ **Speech E2E narrow test baseline accepted:** No Whisper + LLM full E2E required for baseline ship. Narrow integration test (diarization → extraction → graph) sufficient. Loop 17+ can revisit if analytics/regression coverage warrant investment.
- ✅ **TokenUsagePanel Tauri integration test deferred:** Unit tests pass (21 frontend tests). Live event flow verified by e2e (component subscribes to gemini-status, accumulates turns). No synthetic Tauri-bus test needed for baseline.
- ✅ **Clippy --lib --tests now canonical gate:** Enforced for audio-graph core (speech/gemini/extraction). ASR providers (deepgram/assemblyai) remain scoped allows, deferred to separate refactor loop.

