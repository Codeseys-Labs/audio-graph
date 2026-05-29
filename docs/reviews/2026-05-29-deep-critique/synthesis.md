# Deep Multi-Agent Critique — Cross-Facet Synthesis

> **Date:** 2026-05-29 · **Commit:** `a998170` (master)
> **Method:** 6 parallel Codex (`gpt-5.5`) `exec review` facets — concurrency,
> architecture, security, frontend, performance, executive — each an independent
> cross-model pass over `src-tauri/src/` (Rust) and `src/` (React).
> Raw per-facet output: `facet{1..6}-*.out.md` in this directory.
>
> Findings marked **✅ VERIFIED** were checked against the actual source by
> Kilo after the critique. Unverified findings are the reviewers' claims and
> should be confirmed before acting.

---

## Executive verdict

**Overall quality: 7/10.** Strong modularization, bounded channels, conscious
long-session work (rayon pools, caps), and an unusually broad test suite for a
project this young. The risk surface is concentrated in **long-session
correctness** (graph deltas, unbounded queues) and **lifecycle/backpressure**
(stop/start, Gemini inline extraction).

**Single biggest blocker to fix first:** the graph-delta **edge-removal ID
mismatch** — stale edges accumulate in the UI until a full snapshot.

---

## Convergent findings (flagged by 2+ facets = high confidence)

### C1 — Gemini extraction runs synchronously on the event loop  ✅ VERIFIED
*Facets: Concurrency (MED), Architecture (LOW), Executive (HIGH)*

`commands.rs:2217-2252` calls `process_extraction_and_emit(...)` **inline** on
the `gemini-event-receiver` thread, whereas the speech path submits extraction
to a rayon pool (`speech/mod.rs:692-733`). If the LLM stalls, Gemini Live event
handling (transcripts, status, reconnect) lags behind, and because the event
channel is bounded(128) the WS runtime's `send` can eventually block
(`gemini/mod.rs:291`).
**Fix:** route Gemini finals through the same rayon extraction pool /
`spawn_extraction_task` path instead of running inline.

### C2 — Unbounded queues OOM under slow LLM in long sessions  ✅ VERIFIED
*Facets: Concurrency (MED, Gemini audio), Performance (HIGH, executor + chat_history)*

The LLM executor's `interactive`/`background` queues are plain unbounded
`VecDeque`s (`executor.rs:73-76`); every transcript segment submits an
extraction job (`speech/mod.rs:729-733`) that blocks on the single executor
worker. If extraction is slower than ~1 segment/1.5 s, the backlog grows without
bound. Same class: Gemini audio `unbounded_channel` during long reconnects
(`gemini/mod.rs:380`), and `chat_history` is unbounded and cloned wholesale per
chat request (`commands.rs:1144-1174`).
**Fix:** bound the background queue with a drop-oldest/coalesce policy (extraction
is already lossy at ingest via `try_send`), cap `chat_history`, and add a
soft cap + drop counter on the Gemini audio queue.

### C3 — Whole-graph RAG dumped into every chat prompt
*Facets: Security (MED, exfiltration), Performance (MED, token cost/latency)*

`prepare_chat_request` serializes **all** entities + relations + last-10
transcript into every prompt (`commands.rs:1097-1138`). At the 1000-node/5000-edge
cap this is large, slow, token-expensive, and ships maximal session data to a
user-configurable endpoint.
**Fix:** top-k / neighborhood retrieval instead of full-graph dump; add a privacy
note on the custom-endpoint field.

### C4 — `speech/mod.rs` is a ~2.7k-line god-module + streaming-ASR duplication
*Facets: Architecture (HIGH + MED), echoed implicitly by Concurrency/Performance*

One file owns provider routing, Whisper load/validate, accumulation, diarization,
extraction, graph updates, agent proposals, and 4 provider processors. The 4
streaming ASR clients (`deepgram.rs`, `assemblyai.rs`, `aws_transcribe.rs`,
`sherpa`) each re-implement runtime + audio sender + event receiver + 1/2/5/10
backoff.
**Fix (incremental):** extract `accumulator.rs`, `tail.rs`, `extraction.rs`,
`agent.rs`, `providers/*.rs`; factor a `StreamingWsAsrSession` helper owning
runtime/channel/backoff/reconnect, with providers supplying `open_ws`, frame
encoding, and event parsing.

### C5 — Inconsistent error handling & lock-poison recovery
*Facets: Architecture (MED), Concurrency (notes), Executive (LOW)*

`AppError` exists but many boundaries still return `Result<_, String>` /
`map_err(format!)` (`asr/mod.rs:220-251`, `executor.rs:174-191`), and critical
event emits use `let _ =` instead of `events::emit_or_log` (`speech/mod.rs:442`,
`commands.rs:2221`). Graph lock recovers poison (`speech/mod.rs:431-434`) but
some command paths surface poison as opaque errors (`commands.rs:578-581`).
**Fix:** standardize on `AppError` + `emit_or_log` at backend boundaries; pick one
poison policy.

---

## High-value single-facet findings

### H1 — Graph-delta edge-removal ID mismatch (stale edges)  ✅ VERIFIED — **TOP BLOCKER**
*Facet: Executive (HIGH)*

Eviction emits removal IDs as `format!("edge-evicted-{:?}", idx)`
(`graph/temporal.rs:344`), but snapshot/delta build link IDs as
`format!("edge-{:?}", idx)` (`temporal.rs:393`, `:508`). The frontend removes by
exact `graphLinkId()` (`store/index.ts:346-356`), so **evicted edges are never
removed via delta** — they linger until a full `graph-update` snapshot. Related:
relation `weight` bumps mutate the edge (`temporal.rs:191-196`) but `GraphDelta`
has no `updated_edges`, so edge strength is stale between snapshots.
**Fix:** emit `edge-{idx}` (matching format) on eviction; add `updated_edges` to
`GraphDelta` for weight changes.

### H2 — Stop/start is signal-only; can orphan duplicate consumers  ✅ plausible
*Facet: Concurrency (HIGH)*

`stop_transcribe` clears `is_transcribing` and drops handles **without joining**
(`commands.rs:929-939`); `stop_gemini` likewise (`commands.rs:2379-2409`). A fast
restart can flip the flag back on before the old worker observes it, leaving two
consumers splitting the same `speech_audio_rx`. Start guards are also non-atomic
(`commands.rs:2056-2115`).
**Fix:** join worker threads on stop (with timeout), or use a generation/epoch
token so stale workers exit; make start guards `compare_exchange`.

### H3 — Tail audio dropped on stop
*Facet: Concurrency (MED)*

Shutdown flush uses `try_send` and ignores `Full` for the final accumulated
segment (`speech/mod.rs:1244-1247`, `:1677-1679`) — can lose the end of an
utterance exactly when the user stops.
**Fix:** bounded blocking send with timeout for the flush path only.

### H4 — Windows credential ACLs not actually restricted
*Facet: Security (MED)*

`set_owner_only` only clears the readonly bit on Windows and relies on parent
ACLs (`fs_util/mod.rs:16-26`); the `.tmp` is created with default perms before
chmod on Unix (`credentials/mod.rs:156-163`). `load_all_credentials_cmd` returns
full secrets to the frontend (`commands.rs:2939-2942`).
**Fix:** real Windows ACL hardening (or document the gap); create temp files with
restrictive perms first; return presence/redacted values to the UI.

### H5 — Frontend high-frequency events un-throttled (except chat)
*Facet: Frontend (MED)*

`transcript-update`, `asr-partial`, `graph-delta`, `pipeline-latency` write to
Zustand on every event (`useTauriEvents.ts:252-320`); only chat deltas are
coalesced (~30 fps). Also `SettingsPage`/`ExpressSetup` subscribe to the whole
store and re-render on every flood (`SettingsPage.tsx:111-124`).
**Fix:** coalesce `asr-partial`/`pipeline-latency`/`graph-delta` like chat;
narrow whole-store subscriptions to selectors.

### H6 — Frontend listener cleanup unmount race
*Facet: Frontend (MED)*

Cleanup iterates the current `unlisten` array; if unmount happens before the
`Promise.all` of `listen()` resolves, late listeners install but never unlisten
(`useTauriEvents.ts:187-188`, `388-395`).
**Fix:** track a cancelled flag and unlisten any handlers resolved after unmount.

### H7 — Graph tooltip interpolates unescaped text  ✅ plausible
*Facet: Frontend (LOW)*

Entity names/descriptions (model-derived) are interpolated as HTML in tooltips
(`KnowledgeGraphViewer.tsx:273-284`).
**Fix:** escape/sanitize tooltip content.

---

## Prioritized action plan (impact × effort)

| # | Action | Severity | Effort | Source |
|---|---|---|---|---|
| 1 | Fix graph-delta edge-removal ID + add `updated_edges` | **HIGH** | S | H1 ✅ |
| 2 | Bound executor background queue + cap chat_history + Gemini audio soft-cap | **HIGH** | M | C2 ✅ |
| 3 | Move Gemini extraction off the event loop onto the rayon pool | **HIGH** | S | C1 ✅ |
| 4 | Join/epoch-guard workers on stop; atomic start guards | **HIGH** | M | H2 |
| 5 | Top-k graph RAG instead of full-graph dump | MED | M | C3 |
| 6 | Real Windows ACLs + restrictive temp perms + redacted credential reads | MED | M | H4 |
| 7 | Coalesce remaining high-rate frontend events; selector-scope Settings | MED | S | H5 |
| 8 | Bounded blocking flush for tail audio on stop | MED | S | H3 |
| 9 | Fix listener unmount race; escape graph tooltips | MED/LOW | S | H6/H7 |
| 10 | Refactor `speech/mod.rs` seams + `StreamingWsAsrSession` helper | MED | L | C4 |
| 11 | Standardize `AppError` + `emit_or_log`; one poison policy | MED | M | C5 |

## Test gaps the critique surfaced
- No direct unit tests for `setGraphSnapshot`/`applyGraphDelta` reducers
  (removals, identity preservation, **the stale-edge-ID case in H1**).
- Untested backend paths: dispatcher fan-out, provider reconnect/backoff, graph
  eviction/dedup, lock-poison recovery, accumulator overlap, stop/start lifecycle.
- Some timing-dependent tests (Gemini polling sleeps, toast timers).

## What the critique validated (positive signal)
- Graph mutex is **not** held across LLM/HTTP calls (extraction completes first).
- Bounded channels + drop-on-backpressure is consistent on the live audio path.
- Whisper/llama models loaded once and reused (no per-segment reload).
- Tauri capability surface is minimal (`core:default` only); CSP is self+IPC;
  Gemini key is sent as `x-goog-api-key` header, not a URL query param.
- Broad frontend Vitest suite + centralized typed event bridge.

## Reviewer framing (meta — what AI reviewers could NOT check)
Real capture quality and cross-platform device behavior, actual provider
latency/reconnect under network faults, Gemini Live turn-taking feel, audio
output quality/barge-in timing, and real 10-hour memory/CPU curves. These need
a human on real hardware.

---

## Update 2026-05-29 (evening): test-harness diagnosis + core-test runner

**Windows test-harness root cause (was mis-attributed to "ggml DLLs").** Pinned
with `dumpbin`: the crate's `cargo test` binary aborts at load with
`STATUS_ENTRYPOINT_NOT_FOUND (0xC0000139)`. The debug test exe links a *mixed*
MSVC CRT (release `msvcp140`/`vcomp140` + debug `vcruntime140d`/`ucrtbased` from
the cmake-built C++ ML libs). But the **release** test exe links a *consistent*
release CRT and still fails identically, and System32 `vcomp140` (14.51) exports
every OpenMP symbol the binary imports — so it is a deeper test-link-specific
native conflict from whisper-rs/llama-cpp-2/mistralrs, not a simple CRT or
OpenMP mismatch. The clean permanent fix is **ADR-0007** (feature-gate the ML
crates so a test/cloud build links none of them).

**Workaround shipped — `scripts/run-core-tests.ps1`.** Runs the ML-free module
tests for real on Windows via throwaway harness crates that `#[path]`-include
the actual source (no copies/drift) and stub the few `crate::` deps. Verified:
graph::temporal (3 — incl. the H1 edge-id regression + updated_edges + eviction
scheme) and audio (12 — mix_math/mixer/backpressure) = **15 passed**. This
executes the deep-critique Fix #1 tests that previously could only be run "on a
properly configured machine".
