# Chat/LLM Token-Delta Flow & TTS Integration Map

## 1. LLM Token Deltas — Current Path

**NO STREAMING.** The chat pipeline is fully aggregated:

- **Entry**: `send_chat_message()` @ src-tauri/src/commands.rs:995
  - Builds graph context snapshot (lines 1007–1055)
  - Calls `state.llm_executor.chat_with_history()` (line 1088)
  
- **Executor Layer**: src-tauri/src/llm/executor.rs:117
  - Enqueues chat job with `LlmPriority::Interactive` (line 125)
  - **Blocks on response** via `mpsc::channel()` (lines 123, 134–140)
  - No intermediate token events; only final `Result<String, String>` returned

- **Provider Invocation**:
  - **API**: src-tauri/src/llm/api_client.rs:287 (`chat_with_history`) → line 305 calls `chat_completion()`
    - Uses `reqwest::blocking::Client` (line 98) — synchronous HTTP POST
    - Returns complete response string (line 196–200)
  - **Native LLM** (Llama/mistral.rs): equivalent blocking pattern
  
- **Return to UI**: src-tauri/src/commands.rs:1119
  - `ChatResponse { message, tokens_used: 0 }` — `tokens_used` hardcoded TODO (line 1121)
  - Frontend updates store **once** with final assistant message (src/store/index.ts:469)

**Verdict**: Token deltas do not exist in the current chat path. Everything is collected server-side and returned as a single block.

---

## 2. Backend Events During Chat

**Chat generates NO events currently.** Only response payload is returned via command response:

```
send_chat_message() 
  → AppState serializes ChatResponse 
  → Tauri marshals to IPC 
  → Frontend's sendChatMessage() promise resolves
```

**Events emitted in the broader pipeline** (from events.rs):
- `TRANSCRIPT_UPDATE` — ASR segments (not chat)
- `GRAPH_DELTA` / `GRAPH_UPDATE` — knowledge graph changes
- `PIPELINE_STATUS` — capture/processing status
- **NO chat-specific events exist**

**Gemini Live (contrast)** @ src-tauri/src/commands.rs:1625–1661:
- `GEMINI_TRANSCRIPTION` — emitted per server message
- `GEMINI_RESPONSE` — emitted per model response chunk
- `GEMINI_STATUS` — connection state + errors
- These flow through `event_rx` channel (line 1514) → Tauri emit (lines 1627, 1661)

---

## 3. Frontend Chat Subscription Model

**No event-based model.** Frontend uses **promise-based** chat (src/store/index.ts:458–482):

```typescript
sendChatMessage: async (message: string) => {
  set({ isChatLoading: true });  // Optimistic UI
  const response = await invoke<ChatResponse>("send_chat_message", { message });
  set({ chatMessages: [...state.chatMessages, response.message], isChatLoading: false });
}
```

- **UI knows reply is complete** when promise resolves
- **No interim updates** — frontend sees "loading" or "done", never progressive tokens
- Zustand store slices: `chatMessages: []`, `isChatLoading: boolean` (lines 454–455)
- **No useTauriEvents subscription for chat** (useTauriEvents.ts:99–116 has no chat listener)

---

## 4. Stream Abstraction

**None.** The entire response is accumulated server-side:

- `LlmExecutor::chat_with_history()` (executor.rs:117) **blocks** until full response
- `ApiClient::chat_completion()` (api_client.rs:129) collects entire HTTP response body
- Native engines (Llama, mistral.rs) return strings, not streams

**Contrast: Gemini Live** has a true event stream:
- `GeminiLiveClient::event_rx()` (gemini/mod.rs:1514) is a crossbeam channel
- Events flow continuously during a turn (Transcription → ModelResponse → TurnComplete)
- Frontend subscribed via `useTauriEvents()` so each event updates the UI in real-time

---

## 5. LLM Provider Settings & Feature Flags

**Location**: src-tauri/src/settings/mod.rs:359

```rust
pub struct AppSettings {
    pub asr_provider: AsrProvider,
    pub llm_provider: LlmProvider,        // enum: LocalLlama | Api | AwsBedrock | MistralRs
    pub llm_api_config: Option<LlmApiConfig>,
    pub audio_settings: AudioSettings,
    pub gemini: GeminiSettings,           // nested Gemini config
    pub log_level: Option<String>,
    pub demo_mode: Option<bool>,
}
```

**Hydration at runtime** (commands.rs:1080–1084):
```rust
let llm_provider = state
    .app_settings
    .read()
    .map(|s| s.llm_provider.clone())
    .unwrap_or_default();
```

**Where to plug TTS flag**:
- Add `tts_enabled: bool` to `AppSettings` (settings/mod.rs)
- Read during `send_chat_message` (commands.rs:1080 area) **before** invoking executor
- Short-circuit: if `!tts_enabled`, skip TTS sink registration

---

## 6. Gemini Live Token Flow for Contrast

**Event shape** (gemini/mod.rs:104–166, commands.rs:1673–1685):

```rust
GeminiEvent::TurnComplete { usage: Option<UsageMetadata> }
  where UsageMetadata {
    prompt_token_count: Option<u32>,
    response_token_count: Option<u32>,
    total_token_count: Option<u32>,
    ...
  }
```

**Emission** (commands.rs:1673):
```rust
if let Some(u) = usage {
    log::debug!("Gemini: turn complete (tokens total={:?})", u.total_token_count);
}
```

**Frontend subscription** (useTauriEvents.ts:257–295):
```typescript
safeListen<GeminiStatusEvent>(GEMINI_STATUS, (event) => {
    // Routes connection state, errors, usage
});
```

**Key insight**: Each `ModelResponse` event is a *discrete chunk*. TTS should subscribe to the same event, decode tokens incrementally, and enqueue audio synthesis per-chunk. **Not** the final aggregated response.

---

## 7. Cancellation Propagation

**Current state**: No cancel mechanism in chat pipeline.

- `isChatLoading` flag (store:455) is UI-only; no backend cancellation token
- `LlmExecutor` has no stop/abort API
- Executor thread runs until completion or error (executor.rs:154–193)

**Gemini has partial cancellation** (commands.rs:1550–1554):
```rust
while let Ok(chunk) = gemini_rx.recv() {
    let active = is_active.read().map(|a| *a).unwrap_or(false);
    if !active { break; }  // <- checks is_gemini_active flag
}
```

**For TTS**: would need similar `stop_chat` command + cancellation token threaded through executor.

---

## INTEGRATION POINTS

**Exact 5 locations to wire token deltas into a TtsProvider**:

### 1. **Streaming enablement in executor** (src-tauri/src/llm/executor.rs:117–141)
   - Replace blocking `mpsc::channel()` with streaming variant
   - Thread a `tx: Sender<TokenDelta>` alongside response channel
   - Modify `chat_api()`, `chat_native()`, `chat_mistralrs()` to emit deltas during iteration

### 2. **API client token callback** (src-tauri/src/llm/api_client.rs:145–200)
   - Intercept response stream before `.json()` parsing
   - For each chunk, extract token boundaries (via SSE or chunked encoding)
   - Invoke token delta callback: `on_token(delta: &str)` if provided
   - Aggregate for final return as today

### 3. **Event emission in send_chat_message** (src-tauri/src/commands.rs:1088–1103)
   - After `llm_executor.chat_with_history()`, receive token-delta channel
   - Spawn a listener thread (like Gemini event receiver, commands.rs:1609)
   - For each delta, emit: `CHAT_TOKEN_DELTA` event (add to events.rs)
   - Payload: `{ delta: string, cumulative_tokens: u32, timestamp_ms: u64 }`

### 4. **Feature flag check in send_chat_message** (src-tauri/src/commands.rs:1080–1091)
   - Read `state.app_settings.read().map(|s| s.tts_enabled)` (after adding to AppSettings)
   - If `false`, skip event emission for token deltas (or don't spawn TTS sink)
   - Log: `"TTS disabled; chat reply will not stream audio"`

### 5. **Frontend event listener** (src/hooks/useTauriEvents.ts:155–330)
   - Add `safeListen<ChatTokenDelta>(CHAT_TOKEN_DELTA, ...)` in setup()
   - Payload type: `{ delta: string, cumulative_tokens: u32, timestamp_ms: u64 }`
   - Call new store method: `addChatTokenDelta(delta)` (mirrors Gemini transcription pattern)
   - Update store: new field `chatTokens: TokenDeltaEvent[] = []`
   - Frontend can render live token count, enqueue TTS synthesis per delta, etc.

---

**Total scope**: ~200 LOC across 5 files. Pattern mirrors Gemini Live's event-per-chunk model exactly.
