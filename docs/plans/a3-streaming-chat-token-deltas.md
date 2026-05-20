# Plan A3: Streaming chat with token-delta IPC events

**Goal:** Replace today's blocking `send_chat_message` with a streaming
path that emits `chat-token-delta` events to the frontend as tokens arrive
from the LLM. Keep the blocking command as a backward-compatible shim.

**ADR:** [0006](../adr/0006-streaming-chat-and-native-s2s-separation.md) (accepted, sub-decision A).

**Backlog:** new — file as `audio-graph-stream` once committed; this plan's
acceptance creates the seam Wave C's speak-aloud loop will plug into.

## Why this plan exists

Per `docs/research/chat-tts-integration-map.md`, today's chat path is fully
blocking. Token deltas don't exist. The speak-aloud loop in Wave C cannot
hit the ≤500 ms first-audio target without streaming. This plan introduces
streaming infrastructure once, so all four LLM provider impls (Api,
OpenRouter, LocalLlama, MistralRs, AwsBedrock) can adopt it.

## Acceptance criteria

- [ ] New event: `chat-token-delta` with payload `ChatTokenDelta { request_id: String, delta: String, finish_reason: Option<String>, usage: Option<UsageStats> }`.
- [ ] New event: `chat-token-done` with payload `ChatTokenDone { request_id: String, full_text: String, usage: UsageStats }`. Fired exactly once per request.
- [ ] New Tauri command: `start_streaming_chat(message: String) -> Result<String, String>` returns `request_id` immediately. Real work happens on a tokio task.
- [ ] Existing `send_chat_message` becomes a thin shim: calls `start_streaming_chat`, subscribes to its events internally, accumulates the full text, returns the same `ChatResponse` shape on `chat-token-done`. Blocking callers see no change.
- [ ] `src-tauri/src/llm/api_client.rs`: new `chat_completion_stream` function. Uses `reqwest::Client` (NOT `blocking::Client`) with `stream: true` in the request body, parses SSE chunks via `eventsource-stream` or hand-rolled SSE parser.
- [ ] `src-tauri/src/llm/openrouter.rs` (from plan A2): same streaming path; reuses the api_client helper if shapes match exactly, otherwise has its own thin layer.
- [ ] LocalLlama (`src-tauri/src/llm/engine.rs`): uses llama.cpp's existing token-callback API to emit deltas.
- [ ] mistralrs (`src-tauri/src/llm/mistralrs_engine.rs`): uses its existing streaming token API.
- [ ] AwsBedrock: stream chat via Bedrock `ConverseStream` API. If complexity is high, scope this provider's streaming to a follow-up plan and have it short-circuit to blocking-and-synthesize-batch for the speak-aloud loop. Document the gap in the bedrock module.
- [ ] Cancellation: `cancel_streaming_chat(request_id) -> Result<(), String>` that aborts the in-flight stream + emits `chat-token-done` with empty text + `finish_reason: "cancelled"`.
- [ ] Frontend (`src/hooks/useTauriEvents.ts`): subscribe to `chat-token-delta` + `chat-token-done`. Update store with each delta (immutable append), finalize on done.
- [ ] Frontend coalescing: avoid React re-render thrashing on token deltas. Implement a 33ms throttle (one frame at 30fps) for delta-driven re-renders, or batch deltas into store via `unstable_batchedUpdates`.

## Files

**New:**
- (Optional) `src-tauri/src/llm/sse.rs` — small SSE parser if `eventsource-stream` is rejected for dependency-bloat reasons.

**Modified:**
- `src-tauri/src/events.rs` — declare `chat-token-delta`, `chat-token-done` event names + payload types
- `src-tauri/src/llm/executor.rs:117` — accept a `tx: tokio::sync::mpsc::Sender<TokenDelta>` parameter; emit deltas as they arrive
- `src-tauri/src/llm/api_client.rs:145` — switch the `Api` provider's chat path from `blocking::Client` + `.json()` to `reqwest::Client` + SSE streaming
- `src-tauri/src/llm/openrouter.rs` (from plan A2) — same SSE path
- `src-tauri/src/llm/engine.rs` — local llama streaming
- `src-tauri/src/llm/mistralrs_engine.rs` — mistralrs streaming
- `src-tauri/src/commands.rs:1088` — new `start_streaming_chat` command + cancellation command + `send_chat_message` shim
- `src-tauri/src/state.rs` — track in-flight request IDs for cancellation
- `src/hooks/useTauriEvents.ts:155` — subscribe to new events
- `src/store/index.ts` — chat slice gains a `streamingMessageId` field + delta append actions
- `src/types/index.ts` — TS types

**Cargo.toml:**
- Add `eventsource-stream` (lightweight, no_std-friendly SSE parser) OR
  hand-roll a small parser. Decide at implementation time based on size.
- Confirm `reqwest` is built with `stream` feature (currently it is, used
  for downloads).

## Steps

1. Read `docs/research/chat-tts-integration-map.md` end-to-end. The 5
   integration points listed there are this plan's roadmap.
2. Read `src-tauri/src/gemini/mod.rs` for the existing
   token-deltas-via-event pattern. Mirror its `event_tx` channel shape.
3. Define the event payloads in `events.rs` first; commit just that.
4. Update `executor.rs` to accept an `event_tx`. Make all callers pass
   one (the legacy blocking command passes a no-op channel that just
   accumulates).
5. Switch `api_client.rs` chat path to streaming. Test it works against
   the existing OpenAI-compat blocking provider before adding OpenRouter.
6. Add the new commands. Wire frontend.
7. Convert local engines (engine.rs, mistralrs_engine.rs).
8. Stub bedrock streaming with a TODO that delegates to blocking + emits
   one big delta. Open a follow-up issue.
9. Stress-test: 1000 deltas in 5s, verify frontend doesn't lock up.
10. Run all checks.

## Tests

Unit tests:

- `executor_emits_deltas_through_tx_channel` — fake provider sends 5
  tokens; assert receiver gets 5 deltas + 1 done.
- `cancel_aborts_in_flight_stream` — start a stream, cancel mid-way,
  verify `finish_reason: "cancelled"` arrives and no further deltas.
- `send_chat_message_shim_returns_full_text` — call the legacy command,
  assert the response has the full text accumulated from deltas.

Frontend tests:

- `chat store appends deltas immutably` — vitest test on the store slice.
- `useTauriEvents subscribes and unsubscribes on unmount` — already
  covered by the hook test gap from loop23 review; this plan can land
  the new hook test as collateral.

## Dependencies on other plans / ADRs

- ADR-0006 (this plan's spec, sub-decision A).
- Soft dependency on A2 (OpenRouter streaming wires through this path,
  but A2's blocking path lands first).
- Hard dependency for Wave C (audio-graph-92c7) speak-aloud loop.

## Rollback

`send_chat_message` shim is the legacy command; it works whether or not
the streaming path is enabled. To roll back the visible change, gate the
streaming command behind a feature flag in settings (`chat_streaming_enabled`,
default `true`); flip to `false` to fall back to the old blocking path
inside the shim. Keep the events fired but not consumed by frontend if
the flag is off.

## Out-of-scope

- TTS integration — Wave C, audio-graph-92c7.
- AwsBedrock full streaming — follow-up issue if bedrock proves heavier
  than expected.
- Per-token rate limiting (let the provider enforce).
- Cost-tracking integration — audio-graph-2e40.
