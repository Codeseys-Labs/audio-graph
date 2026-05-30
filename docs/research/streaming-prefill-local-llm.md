# Streaming / Incremental Prefill for the Local LLM Engine — Research Report

**Date:** 2026-05-29
**Scope:** Evaluate whether AudioGraph can overlap streaming STT with the local
LLM's **prefill** phase — feeding partial transcripts into the KV cache *without
decoding* during a turn, then triggering decode only when the turn boundary
fires — and apply it to the **speech-to-notes / entity-relation (ER) extraction**
pipeline first. Determine engine feasibility (`llama-cpp-2`, `mistral.rs`),
the correct algorithm, the required ASR signals, and the honest expected value.
**Method:** DeepWiki analysis of the `llama-cpp-rs` and `mistral.rs` codebases,
docs.rs/crates.io for the exact crate versions in use, plus the streaming-input
/ speculative-inference literature and production vendor guidance. Sources at the
bottom. Companion decision: [ADR-0012](../adr/0012-turn-gated-incremental-prefill-llama-cpp.md).

---

## Problem statement

Today entity extraction (`speech/mod.rs::process_extraction_and_emit` →
`llm_executor.extract_entities(... LlmPriority::Background)`) runs on **finalized
~2 s segments** (`speech/mod.rs:363` `TARGET_FRAMES = 16_000 * 2`). On the local
llama.cpp path each call **creates a fresh `LlamaContext`** and re-decodes the
*entire* prompt — including the **constant instruction prefix** — every time
(`llm/engine.rs:207-239`). The KV cache is born and discarded per call. Nothing
overlaps with the time the user is still speaking.

The idea under evaluation (a sharper framing of the HF
`streaming-speech-to-speech` "StreamingInput" experiment): while STT partials
arrive during a turn, **prefill** them into a persistent KV cache (no sampling);
when the turn-final signal fires (Deepgram `EndOfTurn`/`is_final`, AssemblyAI
`end_of_turn`, or VAD end-of-utterance), append the instruction suffix and run
the grammar-constrained **decode** against an already-warm cache. This is only
controllable on a **local** engine — remote OpenAI-compatible endpoints expose
no prefill/decode hook.

---

## Engine feasibility (the decisive question)

### `llama-cpp-2` 0.1.139 — ✅ fully capable

The crate exposes the exact low-level primitives required (verified against the
`utilityai/llama-cpp-rs` codebase and `llama-cpp-sys-2` docs.rs):

| Need | API | Notes |
|---|---|---|
| Persistent context, KV survives across calls | `LlamaContext::decode(&mut LlamaBatch)` called repeatedly on one live context | KV cache is internal context state; positions tracked per sequence. |
| Prefill **without** sampling | `LlamaBatch::add(token, pos, &[seq], logits=false)` / `add_sequence(&toks, seq, false)` | `logits=false` ⇒ no logits materialized ⇒ nothing to sample. Pure prompt ingestion. |
| Rollback on revised partial | `clear_kv_cache_seq(seq, p0, p1)` | Removes tokens in `[p0, p1)` for a sequence — the core of LCP-based invalidation. Also `clear_kv_cache`, `kv_cache_seq_keep`, `kv_cache_seq_add`. |
| Query current position (`n_past`) | `kv_cache_seq_pos_max(seq)` | Largest position present for the sequence. |
| Grammar-constrained decode | `LlamaSampler::grammar(...)` (already used at `engine.rs:168`) | Grammar affects **sampling only**, so it composes cleanly with deferred decode. |

**Conclusion:** Incremental prefill → later grammar-constrained decode on a single
persistent context is directly supported. The only structural constraint is that
`LlamaContext` is **`!Send`** (already noted in `engine.rs:6,54`) — the live
context must be owned by a single dedicated thread/actor and fed over a channel,
rather than created per call on an arbitrary thread.

### `mistral.rs` 0.8 — ❌ not via the public API

Internally mistral.rs *does* separate prefill (`SequenceState::RunningPrompt`)
from decode (`RunningCompletion`) and ships **automatic prefix caching**
(`PrefixCacheManagerV2`, on by default with PagedAttention). But the **public
Rust surface** we use (`Model`, `GgufModelBuilder`, `send_chat_request`,
`generate_structured`) does **not** expose:

- feeding a partial prompt to warm the cache without decoding, then resuming;
- incrementally extending a prompt token-by-token from the caller side;
- explicit per-request prefix-cache control.

From the caller's perspective, generation is **atomic** (prompt in → completion
out). The one free win: its automatic prefix caching will already amortize the
constant instruction prefix across repeated extraction calls, so mistral.rs gets
a *partial* benefit with zero work — but it cannot do true streaming-partial
overlap without patching the crate.

**Conclusion:** This initiative targets **llama.cpp**. mistral.rs and all remote
engines retain their current atomic behavior.

---

## ASR signal availability (what feeds the prefill)

Streaming-partial overlap needs two signals: a stream of **stable** partials and
a **turn-final** boundary. AudioGraph's ASR providers differ sharply:

| Provider | Partials | Turn-final signal | Fit for streaming prefill |
|---|---|---|---|
| **Local Whisper** (`asr/mod.rs` `AsrWorker`) | None — transcribes whole ~2 s `SpeechSegment`s | None (fixed-window fallback) | Coarse: can only prefill each ~2 s segment as it finalizes. Overlap limited to multi-segment turns. |
| **Sherpa streaming** (local, `asr/sherpa_streaming.rs`, feature-gated) | Yes (Zipformer, word-level) | VAD / endpointing | ✅ Local fine-grained partials — the local streaming path. |
| **Deepgram** (`asr/deepgram.rs`) | Yes (interim) | `is_final` / Flux `EndOfTurn` / `TurnResumed` | ✅ Cloud partials + explicit end-of-turn. |
| **AssemblyAI** (`asr/assemblyai.rs`) | Yes (U3 Pro stable segments) | `end_of_turn`, `word_is_final` | ✅ Stable-segment partials by design. |

**Implication:** With default local Whisper, the streaming-prefill *overlap* is
coarse (2 s granularity). Fine-grained overlap requires Sherpa (local) or a cloud
streaming ASR. The persistent-context + prefix-cache foundation, by contrast,
helps **every** path because it removes per-call context creation + prompt
re-decode regardless of ASR granularity.

---

## Algorithm (grounded in prior art)

The streaming-input literature converges on the same recipe, and it maps 1:1 onto
the llama.cpp primitives above:

1. **Prefill only the stable prefix; drop the last unstable segment.** LiveMind
   (arXiv 2406.14319) sends only segments it assumes won't change; production
   pipelines gate on a stability flag (NVIDIA Riva: `stability == 1.0`;
   AssemblyAI: `word_is_final`; Deepgram: `is_final`). Prefilling unstable tokens
   pollutes the KV cache.
2. **On a revised partial, invalidate by longest-common-prefix.** Stream2LLM
   (arXiv 2604.16395) keeps the KV for the unchanged prefix and invalidates only
   the changed tail. In llama.cpp: diff the new stable text against what was
   prefilled, `clear_kv_cache_seq(seq, divergence_pos, max)`, then prefill the new
   tail from `divergence_pos`.
3. **Gate decode strictly on the turn-final signal.** During the turn only
   `decode()` with `logits=false` runs (prefill). On turn-final, append the
   `"\nOutput JSON:"` suffix tokens, request logits on the last token, and run the
   grammar-constrained sampler.
4. **Reuse the constant instruction prefix across turns.** Prefill the fixed
   instruction once; per turn roll the cache back to the end-of-instruction
   position and prefill only the new transcript. (llama.cpp "system-prompt KV
   reuse" pattern.)

---

## Honest value assessment

- **The reliable win is the persistent warm context + prefix-cached instruction**,
  not the streaming overlap. It removes per-call `new_context()` + full-prompt
  re-decode from *every* extraction, on every ASR path. Low risk, always-on.
- **The streaming-partial overlap is speculative and often small for short turns.**
  The HF project's own finding #3/#7: LLM prefill for short prompts (<200 tokens)
  is only ~16 ms, and ASR–LLM overlap "barely helped" when ASR was fast. The
  vendor numbers that show ~300 ms E2E reduction (NVIDIA Riva) come from
  *response generation* on interims (decode overlap), not pure prefill warming.
- **arXiv 2506.15556** explicitly separates LiveMind's regime (server-side, big
  GPU, many tokens) from **single-user local with small models** — AudioGraph's
  exact regime — and notes that for the local case the benefit is real but
  bounded and demands correct verification when partials change. Translation:
  build it telemetry-gated, prove it beats the foundation, and don't over-invest.
- **Extraction is `Background` priority today**, so the user-perceived effect is
  "graph nodes appear sooner after a turn ends," not a hard real-time latency
  budget. This makes speech-to-notes the **safe place to build and validate** the
  prefill engine before the S2S voice-agent path (a real-time budget) depends on
  it.

---

## Risks / constraints

- `LlamaContext` is `!Send` → a dedicated single-thread actor; extraction, chat,
  and future S2S reasoning contend for that one model and must be scheduled
  (extraction is `Background`).
- KV-rollback correctness: a wrong LCP diff or stale `n_past` silently corrupts
  output. Needs targeted unit tests (prefill → revise → rollback → decode).
- Couples extraction quality/latency to ASR partial **stability** and the
  **turn-final** signal; flaky endpointing causes wasted prefill or split entities
  (AssemblyAI warns low `min_turn_silence` splits phone numbers/emails).
- Two-engine divergence (llama.cpp incremental vs mistral.rs/remote atomic)
  increases the test/behavior surface.

---

## Sources

- llama-cpp-2 KV-cache / batch / decode API — DeepWiki `utilityai/llama-cpp-rs`:
  https://deepwiki.com/search/does-the-llamacpp2-crate-versi_a9f5746e-ca22-4306-971a-54b3411b7881 ;
  docs.rs `llama_cpp_2` https://docs.rs/llama-cpp-2/latest/llama_cpp_2/ ;
  `llama_cpp_sys_2::llama_decode` / `llama_memory_seq_keep` (docs.rs).
- mistral.rs prefill/decode + prefix caching internals — DeepWiki
  `EricLBuehler/mistral.rs`:
  https://deepwiki.com/search/in-the-public-rust-mistralrs-c_4c9edf67-0b57-4820-9e75-be38b33f189a
- LiveMind: Low-Latency LLMs with Simultaneous Inference — https://arxiv.org/html/2406.14319
  (drop the last unstable segment; infer on partial input).
- Stream2LLM: streaming inputs for vLLM, LCP-based cache invalidation —
  https://arxiv.org/abs/2604.16395
- Input-Time Speculation for Real-Time Speech (single-user local regime) —
  https://arxiv.org/html/2506.15556v1
- AssemblyAI Universal-3 Pro streaming partials + turn detection / speculative
  inference — https://www.assemblyai.com/docs/streaming/universal-3-pro/turn-detection-and-partials.md
- NVIDIA voice-agent speculative speech processing (stable interims, ~300 ms) —
  https://github.com/NVIDIA/voice-agent-examples/blob/main/docs/SPECULATIVE_SPEECH_PROCESSING.md
- DDTSR dual-track streaming response (local sherpa-onnx Zipformer + streaming TTS) —
  https://arxiv.org/pdf/2602.23266
- HF reference + prior research: `../../HF/streaming-speech-to-speech/README.md`,
  `docs/research/vllm-rust-frontend.md`, `docs/adr/0003`, `docs/adr/0006`.
- Grounding in code: `src-tauri/src/llm/engine.rs`, `mistralrs_engine.rs`,
  `executor.rs`, `speech/mod.rs:363,406-425`, `asr/mod.rs`.
