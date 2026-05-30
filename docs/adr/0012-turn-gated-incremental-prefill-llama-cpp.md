# ADR-0012: Adopt turn-gated incremental prefill on the local llama.cpp engine for entity extraction

## Status

Accepted (2026-05-29). **Phase 0a implemented + validated on Linux (2026-05-30).**
Scoped to the **speech-to-notes / entity-relation (ER) extraction** pipeline.
Backed by research:
[`docs/research/streaming-prefill-local-llm.md`](../research/streaming-prefill-local-llm.md).

### Phase 0a outcome (2026-05-30)

The persistent-context foundation (Option B, minus the instruction-prefix KV
reuse) shipped in `src/llm/engine.rs`: the `LlamaModel` + a single long-lived
`LlamaContext` now live on a dedicated actor thread (the context is `!Send`),
requests/replies cross channels, and the public `&self` API is unchanged. Each
call resets the KV cache (`clear_kv_cache`) so calls stay independent; the
per-call `LlamaContext` allocation is eliminated. The process-global
`LlamaBackend` is now initialized once (`OnceLock`) instead of per-engine.

Validated on Linux (WSL, Rust 1.95.0) against the real LFM2-350M GGUF: a
model-backed, env-gated test (`AG_LLM_TEST_MODEL`, skipped in CI) confirms
free-form generation runs correctly and repeatedly on the reused context. clippy
`-D warnings`, `cargo fmt --check`, and the full 290-test suite are green.

**Three pre-existing bugs surfaced** when this path finally ran end-to-end (it
never had — see below):

1. **Generation-loop KV-position collision (FIXED).** Generated tokens were
   appended at a stale/zero KV position, colliding with the prompt and making
   `llama_decode` return `ret=-1`. Now positioned at `prompt_len + i`.
2. **`BackendAlreadyInitialized` (FIXED).** `LlamaBackend::init()` is process-
   global; a second engine failed. Now shared via a `OnceLock`.
3. **Grammar-sampler abort (FIXED 2026-05-30).** Grammar-constrained extraction
   aborted inside llama.cpp (`llama-grammar.cpp: GGML_ASSERT(!stacks.empty())`,
   an uncatchable SIGABRT) and `llama-cpp-2` 0.1.146 was already the latest
   release. Replaced the GBNF grammar sampler with **generate-then-validate**:
   prompt the model in its native **ChatML** template with a system prompt that
   pins the entities/relations JSON schema (per the LFM2-Extract model card),
   decode greedily (temp=0) with a repetition penalty (the 350M model otherwise
   loops a relation until the token cap truncates the JSON), slice the outer
   JSON object, and `serde`-parse — with the executor's existing fallback if it
   still fails. Validated against the real model: valid, schema-conformant,
   deterministic, cross-call-isolated extraction.

**Coupled follow-up — RESOLVED (2026-05-30).** The extraction model download URL
in `src/models/mod.rs` was also wrong (HF is case-sensitive: the asset is
`LFM2-350M-Extract-Q4_K_M.gguf`; the lowercase form 404'd). Now that bug #3 is
fixed (extraction no longer crashes), the URL casing was corrected, so the local
extraction path is reachable and functional for the first time.

### Remaining phases

- **Phase 0b:** instruction-prefix KV reuse — **investigated 2026-05-30, found
  infeasible for the current extraction model.** LFM2-350M-Extract is a *hybrid
  recurrent* architecture (llama.cpp loads it with `llama_memory_recurrent`).
  Partial KV rollback (`clear_kv_cache_seq` with `p0 > 0`, i.e. "keep the prefix,
  drop the turn") is **not supported on recurrent memory** — the underlying
  `llama_memory_seq_rm` returns `false` without removing, so the next decode
  collides at the prefix boundary (`llama_decode` ret=-1). A correct
  implementation would have to fall back to a full clear + prefix re-prefill on
  every turn, which is equal-or-slower than the Phase 0a full-decode — so prefix
  reuse yields **no benefit** here and was reverted. (Verified against the real
  model: warm reuse decodes the 1st turn but fails the 2nd.) Phase 0b only pays
  off if the extraction model is later swapped for a **non-recurrent**
  (transformer-KV) GGUF, at which point the prefix-reuse path becomes worthwhile.
- **Phase 1 / 2:** streaming-partial overlap + telemetry gating (unchanged).

## Context

AudioGraph's local entity extraction (`speech/mod.rs::process_extraction_and_emit`
→ `LlmExecutor::extract_entities`, `LlmPriority::Background`) runs on finalized
~2 s segments. On the llama.cpp path each call **creates a fresh `LlamaContext`**
and re-decodes the entire prompt — including the **constant instruction prefix** —
then samples (`llm/engine.rs:207-239`). The KV cache is created and thrown away
every call, and nothing overlaps with the time the user is still speaking.

We want the local pipeline to be as fast as possible because: (a) it is the
privacy/offline path, and (b) when the parallel speech-to-speech voice agent is
enabled (ADR-0003 / ADR-0006), it will reuse this machinery under a real-time
latency budget, so SOTA end-to-end latency must be achievable locally.

The technique under consideration — overlap streaming STT with the LLM's
**prefill** phase, deferring **decode** until the turn boundary — is a sharper
version of the HF `streaming-speech-to-speech` "StreamingInput" experiment. It is
only controllable on a **local** engine; remote OpenAI-compatible endpoints expose
no prefill/decode hook (see `docs/research/vllm-rust-frontend.md`).

Research established two decisive facts:

- **`llama-cpp-2` 0.1.139 exposes every primitive needed**: a persistent
  `LlamaContext` whose KV cache survives repeated `decode()` calls; prefill
  without sampling via `LlamaBatch::add(.., logits=false)`; KV rollback via
  `clear_kv_cache_seq(seq, p0, p1)`; position query via `kv_cache_seq_pos_max`;
  and grammar-constrained sampling that composes with deferred decode. The one
  constraint is that `LlamaContext` is `!Send`.
- **`mistral.rs` 0.8 does not expose prefill/decode control** in its public API
  (generation is atomic), though it has automatic prefix caching that already
  amortizes the constant instruction prefix for free.

The streaming-input literature (LiveMind, Stream2LLM) and production guidance
(NVIDIA Riva, AssemblyAI U3 Pro, Deepgram) converge on: prefill only **stable**
partials, drop the last unstable segment, invalidate by **longest-common-prefix**
when a partial is revised, and gate decode on the **turn-final** signal.

## Decision Drivers

- Make the **local** extraction path measurably faster without changing remote
  behavior.
- Reuse the same engine for the future real-time S2S agent, so the design must
  scale down to a strict latency budget later.
- Correctness must not regress: extraction output (grammar-constrained JSON) and
  the existing 148-test suite stay green; revised/retracted ASR partials must not
  corrupt results.
- Honor engine reality: only llama.cpp can do true incremental prefill; mistral.rs
  and remote endpoints cannot.
- Avoid over-engineering: the streaming overlap is speculative and small for short
  turns, so it must be telemetry-gated and provably better than the simpler
  foundation before it ships on.

## Considered Options

- **Option A — Status quo.** Keep per-call fresh-context extraction. No change.
- **Option B — Persistent warm context only (foundation).** A dedicated
  single-thread llama.cpp actor with the constant instruction prefix prefilled
  once and reused across turns; decode still triggered only at turn-final on the
  full finalized transcript. No streaming-partial overlap.
- **Option C — Persistent context + streaming incremental prefill (full idea).**
  Option B, plus: push **stable** ASR partials into the KV cache as prefill during
  the turn, LCP-invalidate on revision, defer grammar-constrained decode to the
  turn-final signal.
- **Option D — Do it in mistral.rs / rely on its internal prefix caching.** Lean
  on mistral.rs's automatic prefix cache instead of llama.cpp control.
- **Option E — Remote vLLM with `StreamingInput`.** Push prefill overlap to a
  server-side vLLM with its native streaming-input API.

## Decision Outcome

Proposed: **adopt Option C as the target architecture, delivered as a layered
rollout where Option B is the committed, always-on foundation and the
streaming-partial overlap of Option C is a feature-flagged, telemetry-gated
extension. The engine is llama.cpp; mistral.rs and remote engines keep their
current atomic extraction behavior.**

Rationale: Option B captures the reliable win (no per-call context creation /
prompt re-decode) at low risk and is the prerequisite for everything else.
Option C adds the speculative overlap the user is after, using the exact llama.cpp
primitives research confirmed, but is gated behind measurement because the
absolute prefill gain is small for short turns and depends on a streaming ASR
(Sherpa local, or Deepgram/AssemblyAI cloud) being active. Doing this on the
`Background`-priority notes pipeline first de-risks the engine before the
real-time S2S agent reuses it.

Option A leaves the core inefficiency in place. Option D is rejected because the
public mistral.rs API cannot separate prefill from decode (its automatic prefix
caching remains a free side-benefit, not the mechanism). Option E is rejected for
the local goal: it cannot run as a native local Windows path and is governed by
the server-side recommendation already recorded in
`docs/research/vllm-rust-frontend.md`.

### Consequences

- **Positive**: Removes per-call `LlamaContext` creation + full instruction-prefix
  re-decode from every local extraction (Option B) — a win on every ASR path.
- **Positive**: When a streaming ASR is active, prefill overlaps the user still
  speaking, so post-turn extraction starts against a warm cache (Option C).
- **Positive**: Establishes and unit-tests the persistent-context prefill engine
  the real-time S2S agent will later reuse.
- **Negative**: `LlamaContext` is `!Send`, forcing a dedicated single-thread actor
  + channel; extraction, chat, and future S2S reasoning now contend for one model
  instance and need a scheduling/priority story (extraction is `Background`).
- **Negative**: Streaming prefill adds KV-rollback complexity (LCP diffing,
  `clear_kv_cache_seq`, `n_past` tracking) and a new class of silent-corruption
  bugs if rollback is wrong.
- **Negative**: Extraction latency/quality becomes coupled to ASR partial
  stability and turn-final signal quality; flaky endpointing wastes prefill or
  splits entities.
- **Negative**: Behavior diverges across engines (llama.cpp incremental vs
  mistral.rs/remote atomic), widening the test surface.
- **Neutral**: With default local Whisper (no partials), Option C degenerates to
  prefilling each finalized ~2 s segment; fine-grained overlap needs Sherpa or a
  cloud streaming ASR.

## Implementation outline (informational, non-binding)

Rollout sequencing is tracked in the issue tracker, not this ADR. Sketch:

- **Phase 0 (Option B):** `LlmEngine` gains a long-lived context owned by a
  dedicated thread/actor; prefill the constant instruction prefix once; per turn
  `clear_kv_cache_seq` back to end-of-instruction, prefill finalized transcript,
  then grammar-constrained decode. Unit-test parity with current output.
- **Phase 1 (Option C):** When a streaming ASR is active, feed stable partials as
  `logits=false` prefill; LCP-invalidate + re-prefill on revision; gate decode on
  the turn-final signal. Add milestone telemetry (turn-final → first extraction
  token → done).
- **Phase 2:** Keep Phase 1 on only where telemetry shows it beats Phase 0.

## References

- Research report: `docs/research/streaming-prefill-local-llm.md`
- vLLM frontend / remote-prefill context: `docs/research/vllm-rust-frontend.md`
- Related ADRs: [ADR-0003](0003-speech-to-speech-agent-provider-matrix.md),
  [ADR-0006](0006-streaming-chat-and-native-s2s-separation.md),
  [ADR-0007](0007-feature-gate-local-ml.md),
  [ADR-0008](0008-conversation-ontology.md)
- Code: `src-tauri/src/llm/engine.rs`, `mistralrs_engine.rs`, `executor.rs`,
  `speech/mod.rs:363,406-425`, `asr/mod.rs`
- HF reference: `../../HF/streaming-speech-to-speech/README.md`
