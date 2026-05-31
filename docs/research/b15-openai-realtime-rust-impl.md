# Research: OpenAI Realtime — Rust Implementation Guide (B15 / ADR-0002) — 2026-05-30

Verifies and extends `docs/research/openai-realtime-2026-05.md` (the GA wire-protocol doc).
That doc is **confirmed accurate** against current reality. This doc fills the Rust gaps:
exact crate versions, feature flags, serde-tag verification, WebSocket handshake code, and
the minimal STT-only happy path with verbatim JSON.

Primary sources (all fetched 2026-05-30): docs.rs/async-openai, crates.io/async-openai,
github.com/64bit/async-openai (via DeepWiki against `main`), developers.openai.com
(realtime-transcription, realtime guides, API reference), docs.rs/tungstenite + tokio-tungstenite,
github reference repos.

---

## 0. Verification verdict on the existing doc

| Claim in `openai-realtime-2026-05.md` | Status |
|---|---|
| Beta removed; implement GA only; no `OpenAI-Beta: realtime=v1` | CONFIRMED (migration guide + GA-only SDKs) |
| URL `wss://api.openai.com/v1/realtime?model=<MODEL>`, header `Authorization: Bearer` | CONFIRMED |
| Transcription `session.update` with `session.type="transcription"` + `audio.input.format` **object** `{type:"audio/pcm",rate:24000}` | CONFIRMED verbatim from realtime-transcription guide |
| Format **object vs string** gotcha (string -> "expected an object, but got a string") | CONFIRMED (community thread #1355366) |
| `gpt-realtime-whisper` requires `turn_detection:null`, manual commit, no prompt | CONFIRMED (API reference) |
| Transcription server events keyed by `item_id` (+`content_index`); cross-turn order not guaranteed | CONFIRMED (production checklist: "Use item_id to order and reconcile") |
| `async-openai` (`realtime`/`realtime-types`) exposes GA types, tokio-tungstenite ^0.28, base64 ^0.22 | CONFIRMED, with version correction below |

**One correction:** DeepWiki's index reported latest async-openai `0.32.4`. crates.io and docs.rs
both show **`0.40.x`** is current (crates.io `0.40.1`, docs.rs latest `0.40.2`). Pin against 0.40.x.

---

## 1. async-openai crate — verified facts (as of 2026-05-30)

- **Latest version:** `0.40.2` (docs.rs) / `0.40.1` (crates.io published). MSRV 1.75.0. License MIT.
- **Feature flags** (from `async-openai/Cargo.toml` on `main`):
  - `realtime` = `["realtime-types", "_api", "dep:tokio-tungstenite"]`
  - `realtime-types` = `["dep:derive_builder", "dep:bytes", "response-types"]`
  - `_api` (internal) pulls `base64`, `tokio`, `reqwest`, `futures`, `thiserror`, etc.
  - **Gotcha:** `realtime-types` alone does **NOT** bring in `tokio-tungstenite`. Only the full
    `realtime` feature adds `dep:tokio-tungstenite`. If you only want the typed events and supply
    your own WS transport, use `realtime-types` and add `tokio-tungstenite` yourself; if you want
    async-openai's `tungstenite::Message` `From` impls, that's fine — they live in `realtime-types`.
- **Bundled deps:** `tokio-tungstenite = "0.28"` (optional, `default-features = false`, non-WASM),
  `base64 = "0.22"` (optional), `tokio = "1"` (`["fs","macros"]`), `serde = "1"` (`["derive","rc"]`),
  `serde_json = "1"`.

### GA types present (module `async_openai::types::realtime`)
- `RealtimeSession` — docs say *"openapi spec type: RealtimeSessionCreateRequestGA"*.
- `RealtimeTranscriptionSession` — *"openapi spec type: RealtimeTranscriptionSessionCreateRequestGA"*.
- Client event wrappers: `RealtimeClientEventSessionUpdate`, and the `RealtimeClientEvent` enum.
- Server transcription events:
  - `RealtimeServerEventConversationItemInputAudioTranscriptionDelta`
  - `...Completed`
  - `...Failed`
  - `...Segment`
  - all variants of the `RealtimeServerEvent` enum.

### serde tags — VERIFIED to match GA event names
Both enums are **internally tagged on `"type"`** (`#[serde(tag = "type")]`). Exact `rename` strings
(from source on `main`):

| Rust variant | serde tag |
|---|---|
| `RealtimeServerEvent::ConversationItemInputAudioTranscriptionDelta` | `conversation.item.input_audio_transcription.delta` |
| `...Completed` | `conversation.item.input_audio_transcription.completed` |
| `...Failed` | `conversation.item.input_audio_transcription.failed` |
| `...Segment` | `conversation.item.input_audio_transcription.segment` |
| `RealtimeClientEvent::SessionUpdate` | `session.update` |
| `RealtimeClientEvent::InputAudioBufferAppend` | `input_audio_buffer.append` |
| `RealtimeClientEvent::InputAudioBufferCommit` | `input_audio_buffer.commit` |

These are exact GA wire names. **Still pin + round-trip test** (serialize a known struct, assert the
JSON; deserialize a captured server frame). The format nesting is
`RealtimeTranscriptionSession { audio: TranscriptionAudio { input: AudioInput { format: RealtimeAudioFormats, transcription: Option<AudioTranscription> } }, include: Option<Vec<String>> }`;
`RealtimeAudioFormats::PCMAudioFormat` renames to `"audio/pcm"` (the object form). That confirms
async-openai already models the object-vs-string gotcha correctly via an enum, so you don't have to
hand-roll it — but verify the older string form isn't needed for your target model.

### No built-in transcription WS connector
`Client::realtime()` returns the `Realtime<'c, C>` struct, which covers **session creation, calls,
and WebRTC** — it is *not* a ready-made "open the transcription WebSocket and stream" helper. What
async-openai gives you for raw WS is the typed enums plus `From<RealtimeClientEvent*> for
tokio_tungstenite::tungstenite::Message` (serializes to a **Text** frame). So the connection itself
(URL + auth header + read/write loop) is **yours to build** — exactly as the prior doc warned.

---

## 2. WebSocket client patterns (verified)

### tokio-tungstenite / tungstenite
- Use `tokio-tungstenite = "0.28"` (matches async-openai). `connect_async(request)` signature:
  ```rust
  pub async fn connect_async<R>(request: R)
      -> Result<(WebSocketStream<MaybeTlsStream<TcpStream>>, Response), Error>
  where R: IntoClientRequest + Unpin;
  ```
- TLS: enable a TLS feature on tokio-tungstenite (e.g. `rustls-tls-webpki-roots` or
  `native-tls`) so `wss://` works; otherwise `connect_async` errors on the TLS scheme.

### Building the handshake request (the clean GA way)
Because the URL carries the `?model=` query param and we must add `Authorization`, build a custom
request. Two equivalent patterns:

**A. `ClientRequestBuilder` (preferred — typed, no manual header parsing):**
```rust
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::Uri;
use tokio_tungstenite::tungstenite::ClientRequestBuilder;
use tokio_tungstenite::connect_async;

// signatures: new(uri: Uri); with_header(K: Into<String>, V: Into<String>); with_sub_protocol(..)
let uri: Uri = "wss://api.openai.com/v1/realtime?model=gpt-realtime-whisper"
    .parse()?;
let request = ClientRequestBuilder::new(uri)
    .with_header("Authorization", format!("Bearer {api_key}"))
    // optional: .with_header("OpenAI-Safety-Identifier", safety_id)
    ;                                  // implements IntoClientRequest
let (ws_stream, _resp) = connect_async(request).await?;
let (mut write, mut read) = ws_stream.split();
```

**B. `url.into_client_request()` + insert headers** (what the reference repos use):
```rust
let mut request =
    "wss://api.openai.com/v1/realtime?model=gpt-realtime-whisper".into_client_request()?;
request.headers_mut()
    .insert("Authorization", format!("Bearer {api_key}").parse()?);
// DO NOT add OpenAI-Beta: realtime=v1 (removed beta). The ref repos still do — strip it.
let (ws_stream, _resp) = connect_async(request).await?;
```
> Known footgun (snapview/tokio-tungstenite issues #92, #217, #327): passing a bare URL string and
> *then* expecting headers to stick, or hand-building an `http::Request` without the mandatory
> WebSocket upgrade headers, causes hangs/handshake failures. Use `IntoClientRequest`
> (`ClientRequestBuilder` or `.into_client_request()`) so the upgrade headers are filled in for you;
> only add `Authorization` on top.

### Reconnect with exponential backoff (60-min cap, no resume)
- Realtime sessions are capped at **60 minutes**, there is **no resume**: on disconnect (or proactive
  pre-expiry rotation) you must open a fresh socket and **re-send `session.update`**, and treat all
  `item_id`s as a new namespace (do not assume continuity across reconnects).
- Recommended loop (use `tokio` + a backoff helper, e.g. the `backoff` crate, or hand-rolled):
  1. connect -> on success reset backoff to base.
  2. immediately send `session.update` (transcription config) and wait for `session.updated`.
  3. stream audio; pump server events.
  4. on WS error / close / `error` frame indicating fatal: compute next delay
     `min(base * 2^n, cap)` with jitter (e.g. base 250 ms, cap 30 s), sleep, retry.
  5. proactively reconnect a bit before the 60-min cap to avoid a mid-utterance drop.
- 429 / rate limiting surfaces inside `conversation.item.input_audio_transcription.failed`
  (`error.{type,code,message,param}`) and/or a top-level `error` frame — back off on those too.
  Listen for `rate_limits.updated`. Set a client `event_id` on outbound events so a server `error`
  can be correlated to the offending event.

### Reference repos (verified)
- **`lukacf/oai-rt-rs`** — *recommended primary reference now.* Explicitly **GA-only**
  ("beta headers/events are not supported"), active in 2026 (~28 commits, © 2026). Uses
  `tokio` + `tokio-tungstenite`, strongly typed `ClientEvent`/`ServerEvent`, and has dedicated
  **transcription sessions** via `/v1/realtime?intent=transcription` with `transcription_session.update`.
  Note: it does **not** implement reconnect/backoff (you add that). Also note its transcription
  intent uses the `?intent=transcription` query form — an alternative to the `session.type` path;
  the `session.type="transcription"` via `?model=` form (per OpenAI's current guide) is what the
  prior doc specifies and is the path to follow.
- **`raja-patnaik/openai-realtime-rust`** — minimal, good cpal/WS plumbing reference, but **pre/early
  GA**: it still sends `OpenAI-Beta: realtime=v1` and relies on `server_vad` with no manual
  `input_audio_buffer.commit`. Use only for transport scaffolding; strip the beta header and the VAD
  assumption.
- Others seen: `scalarian/openai-rust` (realtime sessions), `goo-yyh/openai-rs`
  (`examples/realtime_ws.rs`), `m1guelpf/openai-realtime-proxy` (proxy, not a client).

---

## 3. 2026 protocol deltas confirmed since the prior doc

- **Audio format is an object, not a string** (GA). `{"type":"audio/pcm","rate":24000}`. Sending
  the legacy string (`"pcm16"`, `"pcm_s16le_16000"`) yields
  `Invalid type for 'session.audio.input.format': expected an object, but got a string instead.`
  (community #1355366). `rate` is required inside the object. Telephony uses
  `{"type":"audio/pcmu"}` / `{"type":"audio/pcma"}`.
- **Event renames (beta -> GA)** still in force: `response.text.delta -> response.output_text.delta`,
  `response.audio.delta -> response.output_audio.delta`,
  `response.audio_transcript.delta -> response.output_audio_transcript.delta`; assistant content
  types `text -> output_text`, `audio -> output_audio`; conversation items now include
  `object: "realtime.item"`. (Not on the STT-only path, but relevant for B18 voice s2s.)
- **`session.type` is required** in `session.update` (`"transcription"` or `"realtime"`); omitting it
  can cause the server to reject and terminate.
- **`temperature` removed** as a model param in the GA interface (voice path).
- **Transcription models currently listed** (OpenAI realtime-transcription guide + sessions ref):
  `gpt-realtime-whisper` (native streaming; `turn_detection:null`, manual commit),
  `gpt-4o-transcribe`, `gpt-4o-mini-transcribe` (+ dated `-2025-12-15`), `whisper-1`,
  `gpt-4o-transcribe-diarize` (REST-only, **not** realtime).
- `gpt-realtime-whisper` `audio.input.transcription.delay`: `minimal|low|medium|high|xhigh`.
- Optional logprobs: add `"include": ["item.input_audio_transcription.logprobs"]`.

---

## 4. Minimal STT-only happy path (verbatim client->server JSON)

Flow: **connect** (with `?model=gpt-realtime-whisper` + Bearer) -> **session.update** ->
loop[ **append** PCM16 base64 ] -> **commit** -> read **delta** / **completed** keyed by `item_id`.

**1) session.update (transcription) — send immediately after connect:**
```json
{
  "type": "session.update",
  "session": {
    "type": "transcription",
    "audio": {
      "input": {
        "format": { "type": "audio/pcm", "rate": 24000 },
        "transcription": { "model": "gpt-realtime-whisper", "language": "en" }
      }
    }
  }
}
```
> For `gpt-realtime-whisper` do **not** set `turn_detection` (or set it `null`) — VAD unsupported, so
> you drive turns with manual `commit`. To add logprobs, add `"include":
> ["item.input_audio_transcription.logprobs"]` as a sibling of `"audio"` inside `session`.
> Wait for the server `session.updated` before streaming.

**2) input_audio_buffer.append (one per audio chunk; `audio` = base64 of PCM16 LE, 24 kHz mono):**
```json
{ "type": "input_audio_buffer.append", "audio": "<BASE64_PCM16_24K_MONO>" }
```
> Send ~20–100 ms per chunk (24 kHz mono PCM16 = 48000 bytes/s). Single append payload <= 15 MB.
> Encode with `base64` 0.22: `base64::engine::general_purpose::STANDARD.encode(&pcm_bytes)`.

**3) input_audio_buffer.commit (end of an utterance — triggers transcription of buffered audio):**
```json
{ "type": "input_audio_buffer.commit" }
```
> Optionally `{ "type": "input_audio_buffer.clear" }` to discard uncommitted audio.

**4) Server -> client transcript events (read loop): correlate by `item_id` (+`content_index`):**
```json
{ "type": "conversation.item.input_audio_transcription.delta",
  "item_id": "item_003", "content_index": 0, "delta": "Hello," }
```
```json
{ "type": "conversation.item.input_audio_transcription.completed",
  "item_id": "item_003", "content_index": 0, "transcript": "Hello, how are you?" }
```
Failure variant:
```json
{ "type": "conversation.item.input_audio_transcription.failed",
  "item_id": "item_003", "content_index": 0,
  "error": { "type": "...", "code": "...", "message": "...", "param": "..." } }
```
> Accumulate `delta`s per `item_id` for live display; replace with `transcript` on `completed`.
> Cross-turn `completed` ordering is **not guaranteed** — order/reconcile by `item_id`. Also handle
> the top-level `error` frame `{type,code,message,param,event_id}` (connection stays open).

---

## 5. Suggested Rust stack (concrete)

```toml
[dependencies]
# Option A: lean on async-openai's verified GA enums (no WS connector — you write the loop)
async-openai = { version = "0.40", default-features = false, features = ["realtime-types"] }
tokio = { version = "1", features = ["rt-multi-thread", "macros", "sync", "time"] }
tokio-tungstenite = { version = "0.28", features = ["rustls-tls-webpki-roots"] }
futures-util = "0.3"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
base64 = "0.22"
# backoff loop: either the `backoff` crate or hand-rolled min(base*2^n, cap)+jitter
```
> If you prefer fully owning the type definitions (and the object-vs-string format toggle), define
> your own `#[serde(tag = "type")]` enums mirroring the table in §1 instead of `async-openai`.

## Build order (STT leg = B15 scope)
1. WS connect helper (`ClientRequestBuilder` + Bearer; `?model=gpt-realtime-whisper`).
2. Send `session.update` (transcription, object PCM format), await `session.updated`.
3. cpal/capture -> resample to 24 kHz mono PCM16 -> base64 -> `input_audio_buffer.append`.
4. `input_audio_buffer.commit` per utterance; read `.delta`/`.completed` keyed by `item_id`.
5. Reconnect+backoff wrapper (60-min cap, re-send `session.update`, new item namespace).
6. Round-trip serde test against `async-openai` 0.40 enums (or your own).
7. (Later, B16/B18) VAD via `gpt-4o-transcribe` + `server_vad`; voice s2s via `session.type="realtime"`.
