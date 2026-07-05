# Ground: How transcript text is fed to the LLM for notes/summaries (audio-graph, TODAY)

READ-ONLY codebase map. Repo: `/mnt/e/CS/github/audio-graph`, branch `fix/gtk-test-harness-65f0`.
All paths under `src-tauri/src/`.

## TL;DR

There are **two distinct LLM surfaces**:

1. **Projection path (the "notes/summaries + graph" generator)** — the primary automated
   surface. On each finalized transcript span, a scheduler enqueues a "projection job" and an
   executor calls the LLM to produce a **structured JSON patch** (`upsert_note`, `delete_note`,
   graph ops). This is how notes and the knowledge graph are built.
2. **Interactive chat / converse path** — a user-driven "ask about the conversation" chat that
   streams replies. It does NOT send the raw transcript; it sends chat `history` + a synthesized
   **graph-context** system prompt.

Neither path sends the transcript incrementally or with a rolling summary. Neither path uses
Anthropic/OpenAI prompt-caching or any KV/session-cache reuse over the wire. The only cache
mechanism in the tree is an opt-in **local llama.cpp** KV-prefill flag (`streaming_prefill`,
ADR-0012, default off), and the native engine otherwise **clears its KV cache every call**.

---

## (1) Full transcript vs. windowing/summarization/incremental context

**The FULL current transcript is sent on every projection LLM call. No windowing, no rolling
summary, no delta-only feed, no token cap on the transcript body.**

- The prompt basis is the *entire* set of latest span revisions:
  - `latest_transcript_events()` — `projections.rs:137-149` — folds all events into a
    `BTreeMap<span_id, latest TranscriptEvent>` and returns **every** span's latest revision.
    There is no `.take(n)`, no time-window filter, no truncation.
  - `ProjectionBasis::from_transcript_events_and_speaker_spans()` — `projections.rs:176-193` —
    turns *all* latest events into `span_revisions` + a `transcript_hash` over all of them.
  - `TranscriptLedger::current_basis()` — `projections.rs:642-644` — basis = all `latest_spans`.
- The prompt body serializes those events verbatim:
  - `projection_patch_prompt_messages()` — `projection_llm.rs:185-239`. It calls
    `basis_events()` (`projection_llm.rs:509-532`) to gather every span in the basis, then
    `format_transcript_events_json()` (`projection_llm.rs:534-540`) `serde_json::to_string_pretty`
    of the **full** event vector, and inlines it into the user message at line 228
    (`"Current transcript basis:\n{transcript}\n\n"`).
- So as a session grows, the transcript in every projection prompt grows unbounded. Notes are
  "refined in place" by reusing stable note ids (system guidance at `projection_llm.rs:196-201`),
  but the *input* is always the whole transcript, re-sent each time — there is no summarize-then-drop.
- Repair retry re-sends the same full prompt plus the invalid output + error:
  `projection_patch_repair_prompt_messages()` — `projection_llm.rs:241-273` (calls
  `projection_patch_prompt_messages` first, then appends).
- The one bound anywhere is on the *model's prior output* in the repair message, not the
  transcript: `compact_model_output()` truncates to 2000 chars — `projection_llm.rs:542-549`.

**Chat/converse path** does NOT send the transcript at all — it sends chat history plus a
"knowledge graph context" string (`graph_context`), so the graph is the compression layer there:
- `build_messages()` — `streaming.rs:119-139` (system prompt embeds `graph_context`, then all of `history`).
- Bedrock equivalent: `build_system_prompt()` — `bedrock.rs:536-544`; `build_converse_messages()` maps the full `history` — `bedrock.rs:551`.

## (2) How the LLM prompt is assembled (system + transcript + prior notes)

**Projection path** — two-message chat, no prior notes are fed back as text:
- `projection_llm.rs:210-238` builds exactly:
  - `system`: "You generate AudioGraph projection patch drafts. Return strict JSON only... Do not
    include trusted metadata (sequence, basis, provenance, session_id, llm_request_id)..." + a
    per-kind `operation_guidance` (Notes vs Graph) — `projection_llm.rs:195-202,213-217`.
  - `user`: job metadata (id, session_id, kind, basis_hash, span_count) + the **full transcript
    JSON** + the JSON output schema (from `projection_patch_draft_json_schema()`,
    `projection_llm.rs:143-146`) + "Return a compact patch draft as JSON: {operations, confidence}".
- **Prior notes are NOT re-injected as prose.** State is carried by (a) **stable ids** — the model
  is told to keep note/graph ids stable so an `upsert` refines the existing note (guidance
  `projection_llm.rs:197,200`), and (b) the durable projection store applies the returned patch
  ops. The model reconstructs notes from the transcript each call; continuity is via id reuse +
  patch semantics, not by pasting current notes into the prompt.
- Model output is a strict-JSON `ProjectionPatchDraft` (`projection_llm.rs:22-29`) parsed +
  validated (`parse_projection_patch_draft` `:148-158`; `validate_*` `:275-437`); trusted metadata
  (sequence/basis/provenance) is stamped by the backend, never by the model
  (`trusted_projection_patch_from_model_json` `:160-183`).
- Wire assembly per backend (executor `run_projection_patch` `executor.rs:561-617`, attempts
  `:742-860`): OpenAI-compatible `ChatCompletionRequest` = model + messages + max_tokens/
  max_completion_tokens + optional `response_format:{type:json_object}` (api_client `:54-71`,
  `:291-314`); OpenRouter `ChatCompletionRequest` = model + messages + max_tokens + temperature +
  optional response_format + provider routing (`openrouter.rs:335-371`). JSON mode / vLLM
  structured outputs / mistral.rs JSON-schema are used for the structured constraint.

**Chat/converse path** — system(graph_context) + full history:
- `build_messages()` `streaming.rs:119-139`; `build_api_request` `:144-178`, `build_openrouter_request`
  `:183-219`; Bedrock carries graph context in its dedicated `system` slot (`bedrock.rs:536-549`).

## (3) Prompt-caching / KV / session-cache reuse — NONE over the wire

**No Anthropic `cache_control`, no OpenAI/Bedrock prompt-cache markers are emitted anywhere.**
- Sweep: `grep -rn 'cache_control|cacheControl|cachePoint|CachePoint|promptCache|cache_read_input|cache_creation'`
  over `llm/*.rs` returns **no matches** (exit 1).
- OpenRouter request struct `ChatCompletionRequest` (`openrouter.rs:335-344`) has fields:
  model, messages, max_tokens, temperature, response_format, provider — **no cache field**.
- api_client request struct `ChatCompletionRequest` (`api_client.rs:54-71`) — model, messages,
  max_tokens/max_completion_tokens, response_format — **no cache field**.
- Streaming chat bodies (`streaming.rs:159-166` API, `:199-208` OpenRouter) — plain
  model/messages/max_tokens/temperature/stream — **no cache field**.
- Bedrock Converse messages (`bedrock.rs:551`) — plain text content blocks, **no `cachePoint`**.
- The OpenRouter model **catalog** carries `supports_implicit_caching` and `input_cache_read`
  fields (`openrouter.rs:262,292`), but these are only *display/metadata parsed from OpenRouter's
  model list* — the app never sets a cache directive on a request based on them.

**Local KV cache behavior:**
- Native llama.cpp engine **clears the KV cache on every call**: `engine.rs` header comment
  `:12-18`, and `ctx.clear_kv_cache()` at `engine.rs:770` (tests at `:879-952` assert per-call reset,
  i.e. no cross-call reuse).
- The one KV-warming seam is opt-in and local-only: `AppSettings::streaming_prefill`
  (`settings/mod.rs:1188-1197`, default `false`), gated by `LlmProvider::supports_streaming_prefill()`
  which is **`LocalLlama` only** (`settings/mod.rs:598-607`). It warms the KV cache from streaming
  transcript and defers decode to the turn boundary (ADR-0012) to lower post-turn latency — it is a
  latency optimization for the in-process model, **not** a cross-call/session cache and **not**
  prompt-caching for any remote provider (mistral.rs + all remote/API ignore the flag).

## (4) Invocation cadence + token-cost shape

**Event-driven, per finalized transcript span/turn, with in-flight coalescing. Not per-utterance-
partial, not a fixed timer, not purely on-demand.**

- Live trigger: `observe_projection_schedulers_for_asr_revision()` — `speech/mod.rs:1640-1683`.
  It **only observes when the span is final / end_of_turn / Final stability** (`speech/mod.rs:1646-1651`);
  partial/interim ASR revisions do NOT trigger an LLM call.
- `ProjectionScheduler::observe_ledger()` — `projection_scheduler.rs:267-310`:
  - If basis unchanged since last completed/failed → `Idle` (no call).
  - If a job is already in-flight → `Coalesced` (fold the new spans into `pending_basis`; **no new
    call** until the current one completes) — `:277-299`.
  - Else → `StartJob` (one LLM call) — `:308-309`.
- On completion, if the ledger moved on, it starts one **follow-up** job for the newer basis
  (`complete_in_flight` `:312-364`, `CompletedAndStartedFollowUp`). Stale/failed bases spawn a
  repair/replay job.
- There are **two independent schedulers per session** — one for `Notes`, one for `Graph`
  (`ProjectionSchedulers` `projection_scheduler.rs:485-544`; both observed per revision via
  `dispatch_projection_observation` `speech/mod.rs:1685-1691`) → up to **2 LLM calls per
  finalized turn** (notes + graph), minus coalescing.
- Coalescing pressure knob: `coalesce_span_threshold` default `2` (`projection_scheduler.rs:18-27`),
  plus a TTFT-estimate age window (`coalescing_reason` `:439-451`). So under load, multiple new
  spans collapse into one next call rather than one-call-per-span.
- Interactive chat is purely **on-demand** (user sends a message; streaming reply).

**Token-cost shape:** because the *entire* transcript is re-sent every projection call and the
transcript grows monotonically, **input tokens per call grow ~linearly with session length**, and
calls fire roughly once per finalized turn per kind (notes+graph). Net cumulative input-token cost
over a session is therefore ~**O(turns × transcript_length) ≈ O(n²)** in the number of turns.
Coalescing reduces the call *count* under burst but not the per-call transcript size. With no
prompt-caching, every one of those tokens is billed fresh each call (worst case for cost on a
long meeting). `tokens_used` is tracked per outcome (executor `:638,663`; scheduler
`record_generation_result` `:203`) and drives the TTFT estimate, but there is no input-token
budget/cap on the transcript.

## (5) Where a "contextually-efficient" seam would live

Ranked by leverage, with the exact insertion points already present in the code:

1. **Rolling-summary / windowed basis (biggest win, kills the O(n²)).**
   Insert between the ledger and the prompt: change what `basis_events()` returns and what
   `format_transcript_events_json()` emits — `projection_llm.rs:509-540`. Feed only (a) a
   maintained running summary + (b) the last K unsummarized spans, instead of all spans. The
   `ProjectionBasis`/`transcript_hash` machinery (`projections.rs:166-193`) and the staleness
   validation (`validate_basis`) already give you a clean seam to define "summarized-through
   revision R" as part of the basis so coalescing/repair semantics stay correct.

2. **Delta-only feed + explicit "current notes/graph" state block.**
   Today prior notes are reconstructed from the transcript each call (id reuse only). Change the
   prompt to send `current notes/graph state + only the new spans since last patch`. Seam: the
   `user` message body in `projection_patch_prompt_messages` — `projection_llm.rs:219-237` — plus a
   read of the durable projection store to inline current state. The scheduler already knows the
   delta: `basis_revision_delta_count()` / `coalesced_span_delta` (`projection_scheduler.rs:282,454`)
   identify exactly which spans are new since the last basis.

3. **Provider prompt-caching (cheap, mechanical, no algorithm change).**
   Add `cache_control` to the *stable prefix* (system prompt + already-seen transcript prefix) on
   the request builders. Seams: `ChatCompletionRequest` + `ApiMessage` in `openrouter.rs:335-350`
   and `api_client.rs:54-77`; Bedrock `build_converse_messages` `bedrock.rs:551` (add a
   `cachePoint` content block). The model catalog already parses `supports_implicit_caching` /
   `input_cache_read` (`openrouter.rs:262,292`) so gating on capability is straightforward. This
   turns the repeated transcript prefix into cache-read tokens instead of full-price input tokens.

4. **Semantic segmentation / topic-scoped jobs.**
   Instead of one whole-transcript basis, partition spans into topic segments and run projections
   per active segment. Seam: `latest_transcript_events` → basis construction (`projections.rs:137-193`);
   would require the scheduler to key jobs by segment as well as kind
   (`ProjectionScheduler`/`ProjectionSchedulers` `projection_scheduler.rs`).

5. **Local KV-prefill is already the latency seam (not a cost seam).**
   `streaming_prefill` (ADR-0012, `settings/mod.rs:1188-1197`) warms the local llama.cpp KV cache
   from streaming transcript; it's the natural home for "incremental context" on the in-process
   model, but note the engine currently `clear_kv_cache()`s per call (`engine.rs:770`), so any
   cross-turn KV reuse would need that reset relaxed for the prefill mode.

## Key file:line anchors
- `projections.rs:137-149` — `latest_transcript_events` (ALL spans, no window)
- `projections.rs:166-193`, `:642-644` — basis = full transcript + hash
- `projection_llm.rs:185-239` — projection prompt (system + FULL transcript JSON + schema)
- `projection_llm.rs:509-540` — basis_events + full-transcript JSON serialization (the summarize/window seam)
- `projection_llm.rs:241-273` — repair prompt (re-sends full prompt); `:542-549` truncates only model output
- `projection_scheduler.rs:267-310` — observe_ledger: StartJob / Coalesced / Idle; `:18-27` coalesce threshold=2
- `projection_scheduler.rs:485-544` — separate Notes + Graph schedulers (≤2 calls/turn)
- `speech/mod.rs:1640-1683` — live trigger, final/end_of_turn gated; `:1685-1691` dispatch both kinds
- `llm/executor.rs:561-617`, `:742-860` — backend fan-out (native/openrouter/api/mistralrs) + fallback chain
- `llm/openrouter.rs:335-371` — OpenRouter request struct/builder (NO cache_control)
- `llm/api_client.rs:54-71`, `:291-314` — API request struct/builder (NO cache_control)
- `llm/streaming.rs:105-219` — interactive chat wire bodies (history + graph_context, NO cache_control)
- `llm/bedrock.rs:536-551` — Bedrock system prompt + converse messages (NO cachePoint)
- `llm/engine.rs:12-18`, `:770` — native KV cache cleared per call
- `settings/mod.rs:598-607`, `:1188-1197` — streaming_prefill (local-only, opt-in KV warm, ADR-0012)
- `llm/openrouter.rs:262,292` — catalog-only `supports_implicit_caching` / `input_cache_read` (display metadata, not used to set cache directives)
