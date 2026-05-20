# ADR-0006: Streaming Chat with Token Deltas; Native-S2S Agents Are Sibling Surfaces

## Status

Accepted 2026-05-19 for phased implementation.

## Context

Two related decisions, captured together because they share a single
boundary: **what is in the composed pipeline vs what is a sibling agent**.

### The streaming chat decision

Today, `send_chat_message` (commands.rs:995–1119) is fully blocking. The
LLM provider is invoked, the response is fully accumulated in memory, and
the frontend sees one IPC call resolve with a complete string. There are
no token-delta events.

The speak-aloud loop (audio-graph-92c7) needs token-level streaming so
TTS can begin synthesis on the first clause boundary, not wait for the
final period. ADR-0003's "turn protocol" section calls this out as
"aggressive token-to-TTS flushing". Without streaming chat, the whole
speak-aloud loop's latency is bounded by full-completion latency — usually
1–3 seconds for a chat reply. With streaming + clause-boundary flushing
to Aura, first-audio latency drops to 200–400 ms.

### The native-S2S separation decision

The user's goal correction (2026-05-19) explicitly distinguishes:

1. **The composed pipeline**: STT → LLM → TTS. Each stage is independently
   chosen (Deepgram STT + OpenRouter LLM + Aura TTS, or local equivalents).
   Drives speech-to-graph/notes + chatbot replies + optional speak-aloud.
2. **Native-S2S agents**: Gemini Live, OpenAI Realtime gpt-realtime-2.
   Audio-in → audio-out as a single model. **Bypass the composed pipeline
   entirely.** They're sibling surfaces, not pipeline stages.

ADR-0003 (proposed) framed all three providers as a single "S2S agent
provider matrix". That framing accidentally implies the composed pipeline
and the native-S2S agents share orchestration. They don't. They share UI
gestures (a button to start an agent, a panel to see its output) but not
internals.

## Decision Drivers

- Speak-aloud first-audio latency must be ≤ 500 ms for the conversation
  to feel responsive. Without streaming chat, it can't be.
- The codebase needs a clear answer to: "where do I add a new LLM
  provider?" vs "where do I add a new realtime voice agent?". Today the
  answer is muddled by ADR-0003's matrix.
- Existing Gemini Live module (`src-tauri/src/gemini/mod.rs`, 90KB) is
  already structurally a "sibling agent" — it has its own session task,
  its own events, its own UI hook. The future OpenAI Realtime module
  (audio-graph-396f) will follow that shape. Neither composes with the
  pipeline orchestrator.
- Streaming chat must work for *all* LLM providers (OpenRouter, vLLM,
  AWS Bedrock, local llama), not just OpenRouter. The transport layer
  (SSE-over-HTTP) is OpenAI-compatible across our cloud providers and
  llama.cpp servers; native engines (mistralrs) need their own streaming
  shim but the trait surface upstream is the same.

## Considered Options

### For streaming chat (sub-decision A)

- **A1**: Introduce streaming events. Add `chat-token-delta` event type
  emitted from a new background worker; chat command starts streaming and
  returns immediately. Frontend subscribes to deltas and reassembles +
  feeds to TTS.
- **A2**: Block-and-batch. Keep the blocking path; add a separate
  "synthesize this complete reply now" command after each completion.
  Sacrifices the latency benefit but minimizes code churn.
- **A3**: Per-provider streaming abstraction at the bottom of the LLM
  trait, but keep `send_chat_message` blocking and add a new
  `start_streaming_chat` command alongside. No backward-compat break.

### For native-S2S separation (sub-decision B)

- **B1**: Two distinct concept names in code + UI. `Pipeline` = composed
  STT/LLM/TTS path. `RealtimeAgent` = native-S2S surface. ADR-0003's
  matrix narrows to "RealtimeAgent providers" only; pipeline providers
  are in their own ADRs (0001 already does this for STT, 0005 for LLM,
  0004 for TTS).
- **B2**: Keep the unified ADR-0003 matrix; add prose clarification that
  the local/hybrid row is a different code path from the cloud-native
  rows. No structural change.

## Decision Outcome

**A1 + B1.** Chosen because:

- A1 is the only option that delivers the speak-aloud latency target.
  A2 sacrifices the goal; A3 introduces a permanent fork in the chat path
  that we'd later have to consolidate.
- B1 lets the codebase grow. Adding a Whisper-streaming local STT (audio-
  graph-14e0) is clearly a Pipeline change. Adding gpt-realtime-2 (audio-
  graph-396f) is clearly a RealtimeAgent change. The two surfaces never
  share state machines or settings panes.

### Consequences

- **Positive (A1)**: Speak-aloud first-audio latency target becomes
  achievable. Streaming chat also benefits the chat UI on its own
  (typewriter effect, "stop generating" responsiveness).
- **Positive (A1)**: All cloud LLM providers (OpenRouter, vLLM, Bedrock)
  share the streaming infrastructure. One implementation, many providers.
- **Positive (B1)**: Settings UI splits cleanly: "STT / LLM / TTS"
  (pipeline) and "Realtime Voice Agent" (RealtimeAgent). No more
  "where does Gemini Live live?" UI confusion.
- **Positive (B1)**: ADR-0003's local/hybrid row gets re-homed in
  ADR-0004 (TTS) + ADR-0005 (LLM) + the new pipeline orchestrator
  (audio-graph-eee3) which becomes a "Pipeline conductor" not an
  "S2S provider".
- **Negative (A1)**: All four LLM providers (`Api`, `LocalLlama`,
  `MistralRs`, `AwsBedrock`) need streaming support added. Bedrock's
  ConverseStream API differs from OpenAI SSE; needs an adapter. Local
  llama.cpp and mistralrs already have token-callback APIs but our
  current code ignores them.
- **Negative (A1)**: Settings hydration order matters more — the
  `tts_enabled` flag has to be checked before the streaming worker
  starts, otherwise the TTS channel sits open for nothing.
- **Negative (B1)**: ADR-0003 is now partially superseded. We renumber
  the surface it owns: STT providers → ADR-0001, LLM providers → ADR-0005,
  TTS providers → ADR-0004, RealtimeAgent providers → ADR-0002 (gpt-
  realtime-2) + an existing implicit "ADR-pre-history" for Gemini Live.
  This ADR (0006) is a meta-clarification.
- **Negative (A1)**: Token-delta event spam on the IPC bus is real; we
  need to coalesce deltas at the frontend (e.g. flush every 33 ms or
  every clause boundary, whichever first) to avoid React re-render
  thrashing.
- **Neutral**: `send_chat_message` becomes a thin shim over
  `start_streaming_chat` + collect-to-end. Backward compatible.

## Pros and Cons of the Options

### A1: Streaming events with token-delta IPC

- Good, because: enables speak-aloud latency target; also improves chat UX.
- Good, because: shared infra across all LLM providers.
- Bad, because: requires changes in 4 LLM provider modules + IPC schema +
  frontend store. Largest scope change in this loop.

### A2: Block-and-batch with post-hoc TTS

- Good, because: minimal code churn — just add a new TTS command.
- Bad, because: first-audio latency is bounded by full chat completion,
  often 2–3 s. Conversational feel is lost.

### A3: Two parallel chat paths (blocking + streaming)

- Good, because: no risk to existing code.
- Bad, because: permanent fork; future maintainers must update both.
- Bad, because: settings have to track which path is active per LLM
  provider — combinatorial mess.

### B1: Pipeline vs RealtimeAgent as distinct surfaces

- Good, because: settings UI organizes cleanly, code grows orthogonally.
- Good, because: ADRs map 1:1 to the surfaces (0001 STT, 0004 TTS,
  0005 LLM, 0006 boundary, 0002 + future for RealtimeAgent).
- Bad, because: requires a small ADR-0003 deprecation note + backlog
  retitling.

### B2: Keep ADR-0003 unified

- Good, because: fewer files.
- Bad, because: doesn't capture the architectural split that already
  exists in code (gemini/ vs the proposed pipeline orchestrator).
- Bad, because: future contributors still have to read ADR-0003 in full
  to know where their feature goes.

## Implementation outline (informational)

### Streaming chat (A1)

Per the integration map (`docs/research/chat-tts-integration-map.md`),
the seam points are:

1. `src-tauri/src/llm/executor.rs:117` — add a `tx` parameter so the
   worker can emit deltas alongside the final blocking response.
2. `src-tauri/src/llm/api_client.rs:145` — switch `.json()` for SSE
   streaming with `eventsource-stream` or hand-rolled SSE parser. Pass
   each delta through to `tx`.
3. `src-tauri/src/commands.rs:1088` — start a streaming task instead of
   awaiting a blocking call; the existing command can stay (returns final
   string) but emit deltas via `app.emit("chat-token-delta", ...)`.
4. `src-tauri/src/settings/mod.rs` — add `tts_enabled: bool` to ChatSettings.
   When false, the streaming task short-circuits the TTS branch.
5. `src/hooks/useTauriEvents.ts:155` — subscribe to `chat-token-delta`,
   feed Zustand store + (if speak-aloud enabled) the TTS channel.

### Native-S2S separation (B1)

- Rename `audio-graph-eee3` to "Pipeline orchestrator" (already updated
  per goal correction).
- Update ADR-0003 status to `superseded by ADR-0006` (per the
  one-allowed-edit rule).
- Update `docs/ARCHITECTURE.md` to draw the boundary line in its diagram.

## References

- `docs/research/chat-tts-integration-map.md` — the 5 streaming seam
  points
- `docs/research/verified-2026-05-19.md` — OpenRouter SSE shape +
  Aura barge-in protocol
- `docs/adr/0001-parallel-realtime-pipeline.md` — earlier statement of
  the Pipeline concept
- `docs/adr/0002-openai-realtime-provider.md` — RealtimeAgent
- `docs/adr/0003-speech-to-speech-agent-provider-matrix.md` — superseded
  in part by this ADR
- `docs/adr/0004-tts-provider-trait-and-deepgram-aura.md` — TTS as a
  Pipeline provider
- `docs/adr/0005-openrouter-as-recommended-llm-endpoint.md` — LLM as a
  Pipeline provider
