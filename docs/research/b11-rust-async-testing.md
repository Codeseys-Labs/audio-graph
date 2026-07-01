# B11 — Rust testing patterns for async/network modules (no live services)

Research for unit-testing `llm/executor.rs`, `llm/api_client.rs`, `speak_aloud.rs`,
`asr/cloud.rs`, `asr/mod.rs`, `speech/context.rs` without hitting real HTTP / audio
hardware.

Date: 2026-05-30. Sources: docs.rs (wiremock 0.6.5, tokio 1.50), tokio.rs/topics/testing,
LukeMathWalker/wiremock-rs, repo source.

---

## 0. What the repo already does (skim findings — reuse these patterns)

- **No mocking crate is in `Cargo.toml`.** `[dev-dependencies]` has only
  `tauri = { features = ["test"] }` (src-tauri/Cargo.toml:200). `tokio` is a normal
  dep with `features=["full"]` (so `test-util`/`time::pause` are already compiled in;
  `#[tokio::test]` works today). `reqwest = "0.13.2"` with `["blocking","json","multipart","stream"]`.
- **The clients are BLOCKING.** `ApiClient` (api_client.rs:95), `OpenRouterClient`
  (openrouter.rs:272) and `cloud::transcribe_segment` (cloud.rs:132) all use
  `reqwest::blocking::Client`. The async `reqwest::Client` is used ONLY for
  `openrouter::test_connection` / `list_models` / streaming. **This is the single most
  important constraint for tool choice** (see §1).
- **Existing HTTP-test idiom = hand-rolled `TcpListener` mock**, not a crate
  (openrouter.rs:464-509 `spawn_mock`). It binds `127.0.0.1:0`, reads one request,
  captures raw bytes into `Arc<Mutex<String>>`, returns a canned
  `HTTP/1.1 … Connection: close` response. Async clients are tested with
  `#[tokio::test]`; the **blocking** client is tested by running the mock on an
  `rt.block_on` runtime and issuing the request from `std::thread::spawn` because
  *"`reqwest::blocking` cannot run inside an active tokio runtime"* (openrouter.rs:609,633).
- Pure-logic tests already exist and are the model to copy: `llm/sse.rs` (12 tests,
  decoder + chunk parse), `tts/mod.rs` (config clamp, error-status mapping, event
  round-trip), `playback/tests.rs` (device-absent graceful path).
- **Seams that already exist:** `TtsSession` is an object-safe `#[async_trait]` trait
  (tts/mod.rs:295) → speak_aloud can take a fake session. `TtsEventStream =
  Pin<Box<dyn Stream<Item=TtsEvent> + Send>>` (tts/mod.rs:286) → the pump is testable
  with an in-memory stream. `AudioPlayer::new()` with no `open_default()` is a working
  no-device stub (playback/tests.rs:57). `CloudAsrConfig`/`ApiConfig` take an `endpoint`
  string → point at a mock URL with no code change.

---

## 1. HTTP mocking for reqwest: wiremock vs mockito vs httpmock

### Latest versions / status
| crate | latest | runtime model | request matching | notes |
|---|---|---|---|---|
| **wiremock** | **0.6.5** | async-only (`MockServer::start().await`); runtime-agnostic (tokio/async-std) | rich matcher trait + verification (`.expect(1)`, `mount_as_scoped`) | the de-facto modern choice; 780★ |
| **mockito** | 1.7.x | spins its own server; `Server::new()` (sync) or `new_async().await` | builder `mock(method,path).with_*` | older API; global-state footguns historically |
| **httpmock** | 0.7.x | sync `MockServer::start()` + `.assert()`; async variants exist | `when/then` closures, standalone-binary mode | sync-first ergonomics fit blocking clients |

### Decision for THIS repo
Three independent considerations push to a hybrid answer:

1. **The chat/extraction clients are `reqwest::blocking`.** wiremock is async-only and
   `MockServer::start().await` must run on a tokio runtime; a `blocking` request issued
   on that same runtime thread panics. You'd have to replicate the existing
   `rt.block_on` + `std::thread::spawn` dance (openrouter.rs:607-638) around every
   wiremock test — at which point wiremock buys little over the existing `spawn_mock`.
   For the **blocking** clients, **httpmock (sync mode)** is the most ergonomic external
   option, OR just **reuse the existing `spawn_mock` helper** (zero new deps).
2. **The streaming SSE client needs a *chunked* body delivered over time.** This is the
   sharp edge: wiremock's `ResponseTemplate` sends a **single complete body**
   (`set_body_raw(bytes, "text/event-stream")`, docs.rs wiremock 0.6.5
   `ResponseTemplate`); it has `set_delay` (oneuptime example) but **no API to emit
   multiple SSE frames with gaps**. reqwest's `bytes_stream()` will still see the whole
   payload, so you can assert the decoder concatenates deltas, but you **cannot** test
   true incremental arrival, mid-stream cancel timing, or keepalive interleaving through
   wiremock. mockito/httpmock have the same single-body limitation.
   → **True streaming/cancel behaviour should be tested at the `SseDecoder` layer
   (already done, sse.rs) + a raw `TcpListener` that writes frames with `flush()`/sleep
   between them** (extend `spawn_mock` to take a `Vec<&[u8]>` and write+flush each).
3. **Don't add a dep unless it earns its place.** The repo already mocks HTTP with ~45
   lines of std/tokio. Adding wiremock (+ its tide/hyper transitive tree) for parity is
   low ROI. **Recommendation: keep `spawn_mock` as the house pattern; reach for
   `httpmock` only if matcher/verification ergonomics on a specific test become painful.**

### wiremock reference (if adopted for the async leg only)
```rust
// dev-dependency: wiremock = "0.6"
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::{method, path, header}};

#[tokio::test]
async fn list_models_parses_data_array_wiremock() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/models"))
        .and(header("authorization", "Bearer sk-test"))
        .respond_with(ResponseTemplate::new(200)
            .set_body_json(serde_json::json!({ "data": [ /* … */ ] })))
        .expect(1)                       // built-in call-count verification
        .mount(&server).await;
    let models = list_models("sk-test", &server.uri()).await.unwrap();
    assert_eq!(models.len(), /* … */);
}   // server (and the .expect(1) assertion) drop here
```
For a 4xx/timeout: `ResponseTemplate::new(429)` / `.set_delay(Duration::from_secs(30))`
against a client with a short `.timeout(...)`.

### Bottom line
- **Async OpenRouter helpers (`test_connection`, `list_models`):** wiremock is clean and
  gives free verification — adopt if you want matcher ergonomics; otherwise the existing
  `spawn_mock` already covers them.
- **Blocking `ApiClient` / `OpenRouterClient::chat_completion` / `cloud::transcribe_segment`:**
  prefer the existing `spawn_mock` + `std::thread::spawn` idiom (or httpmock sync). Avoid
  wiremock here — the blocking-on-runtime panic makes it strictly more boilerplate.
- **Streaming SSE / cancel:** no mock server tests this faithfully; test `SseDecoder`
  (pure) + a `TcpListener` that flushes frames with delays.

---

## 2. Deterministic tokio / scheduler / fallback-chain testing

### 2a. `executor.rs` is NOT tokio — it's a `std::thread` + `Condvar` actor

`LlmExecutor::new` spawns a named OS thread running `worker_loop` over a
`Arc<(Mutex<QueueState>, Condvar)>` (executor.rs:144). Jobs reply over a
`std::sync::mpsc` channel. So `#[tokio::test]` / `time::pause` are **irrelevant to the
executor itself** — it's classic threaded-actor testing. The hard part: most logic is
private (`enqueue`, `run_chat`, `run_extraction`, `worker_loop`) and the backends are
concrete types behind `Arc<Mutex<Option<…>>>` with no trait seam.

**High-value, NO-seam-needed tests (make these `fn`s testable):**
- **`run_chat` fallback ordering is pure given fake attempt fns.** `ChatAttemptFn` is a
  `fn(&BackendHandles, &[ChatMessage], &str) -> Result<String,String>` (executor.rs:318)
  and the per-provider order is a hard-coded `&[ChatAttemptFn]` slice (executor.rs:326-333).
  The cleanest refactor-for-test: extract the slice-walking loop (executor.rs:335-343)
  into `fn run_attempts(attempts: &[F], …)` and unit-test it with closures that record
  call order into a `Vec` and return `Err` until the Nth — asserts "tries in order, stops
  at first `Ok`, returns last `Err` when all fail". No network, no mutexes.
- **Cooldown / rate-limit pure logic (already free functions):** `is_rate_limited`
  (executor.rs:60), `note_extraction_error` + `extraction_in_cooldown`
  (executor.rs:56-76). These touch a `static AtomicU64` — testable but **shared global
  state across tests** → run them serially or read/restore the atomic. Assert `"429"`,
  `"Too Many Requests"`, `"rate limit"` (case-insensitive) trip the cooldown and a plain
  error does not.

### 2b. Priority + drop-oldest queue: test via the *real* worker with fake backends
The interactive-before-background ordering (`worker_loop` pops `interactive` first,
executor.rs:249-251) and the `MAX_BACKGROUND_QUEUE`=32 drop-oldest bound
(executor.rs:220-233) are the two behaviours worth asserting. Two routes:

- **(preferred) Pure queue test.** `QueueState` + `enqueue`'s drop-oldest body is pure
  data-structure logic. Lift it to a `fn push(state:&mut QueueState, prio, job)` and test:
  push 33 background jobs → `len()==32` and the oldest is gone; push interactive after
  background → interactive pops first. Asserting "the dropped job's `response_tx` is
  dropped so the caller's `recv()` returns `Err`" is the key correctness property
  (executor.rs:218-219 comment).
- **(integration) Drive the live executor with stub backends.** Requires a seam: today
  `extract_native`/`chat_native` etc. lock concrete engines. To test ordering
  deterministically without a real LLM you must inject backends — either (a) add a
  `#[cfg(test)]` constructor that accepts a `Box<dyn ChatBackend>` slice, or (b) gate the
  worker on a trait. Given the concrete-type coupling, **(a) the pure-`run_attempts` +
  pure-queue tests give ~90% of the value for ~10% of the effort.** Recommend NOT
  refactoring the whole backend storage for B11.

**Asserting ordering deterministically:** the canonical tokio/thread idiom is a shared
`Arc<Mutex<Vec<&'static str>>>` (or `mpsc`) that each fake records into; after the work
completes, assert the recorded sequence. For the threaded executor, signal completion via
the existing `response_rx.recv()` (it blocks until the worker replied), then inspect the
recorder — no sleeps, fully deterministic.

### 2c. `time::pause`/`advance` — where it DOES apply
`tokio::time::pause()` + `advance(Duration)` (docs.rs `tokio::time`, tokio.rs/topics/testing)
make `sleep`/`interval`-based code deterministic: time is virtual, and a paused runtime
**auto-advances to the next timer when all tasks are idle** (so a `sleep(30s)` resolves
instantly). Requires `tokio` `test-util` (already on via `features=["full"]`) and
`#[tokio::test(start_paused = true)]`. **Relevant to:** any reconnect-backoff ladder in
TTS/ASR streaming (`TtsStatus::Reconnecting { backoff_secs }`, tts/mod.rs:172) — pause
time, drive the backoff, assert attempts/backoff sequence without real waits.
**Not relevant to** `executor.rs` (no async timers; the cooldown uses wall-clock
`SystemTime` via `now_ms()` — that needs a clock seam or tolerance assertion, not
`time::advance`).

Channel test idiom (tokio mpsc, for `stream_chat`'s `TokenDelta` receiver,
streaming.rs:248): `let (tx,rx)=mpsc::channel(64); … assert_eq!(rx.recv().await, Some(TokenDelta::Delta{..}))`
then assert exactly one terminal `Done`/`Error`/`Cancelled` and then `None`.

---

## 3. Testing `speak_aloud.rs` (TTS pipe + barge-in) without an audio device

Seams already present — good news:
- `SpeakAloudPipe::maybe_new` returns `Ok(None)` when `speak_aloud=false` or provider is
  `TtsProvider::None` (speak_aloud.rs:69-73) — **pure, test it directly** (no creds, no
  device).
- The pipe holds `Box<dyn TtsSession>` (speak_aloud.rs:50) — **inject a fake session**.
  But: `maybe_new` is the only constructor and it's hard-wired to `DeepgramAuraProvider`
  + `player.open_default()` (speak_aloud.rs:79-103), so the struct can't be built in a
  test as-is. Minimal seam: add a `#[cfg(test)] fn from_parts(session, player, cancel)`
  constructor, or make the provider-open step a parameter. Then:
  - **`append_delta` clause-buffering is the highest-value pure-ish test.** `is_clause_boundary`
    (speak_aloud.rs:42) is a free fn — test it directly. With a fake `TtsSession` that
    records every `speak(text)` into `Arc<Mutex<Vec<String>>>`, assert: a delta without
    a boundary buffers (no `speak` call); `"Hello, "` flushes `"Hello,"` and keeps `" "`;
    multiple boundaries flush up to the **last** boundary (speak_aloud.rs:135-141);
    whitespace-only chunks don't call `speak` (speak_aloud.rs:144).
  - **`finish` flushes the pending tail then calls `flush()`+`close()`** (speak_aloud.rs:155-176)
    — assert the fake recorded the trailing fragment + a `flush` + a `close`.
  - **`cancel` (barge-in) ordering** (speak_aloud.rs:183-198): assert the fake session got
    `clear()` then `close()`, that `player.cancel()` was called, and that
    `audio_pump_cancel.cancel()` fired. With a fake `AudioPlayer` (or assert on real
    `AudioPlayer::new()` which has a no-op `cancel`, playback/tests.rs:66) and a
    `CancellationToken` you own, `assert!(token.is_cancelled())`.

**Barge-in cancel path is fully unit-testable without injecting the whole pipe** by
testing `pump_audio` directly (speak_aloud.rs:203) — it's a free async fn taking
`(TtsEventStream, AudioPlayer, CancellationToken)`:
```rust
#[tokio::test]
async fn pump_stops_on_cancel() {
    let cancel = CancellationToken::new();
    // in-memory stream of events; never-ending so only cancel can stop it
    let stream = futures_util::stream::iter(vec![TtsEvent::AudioChunk{samples:vec![1,2], sample_rate:24000}])
        .chain(futures_util::stream::pending());
    let player = AudioPlayer::new();          // no device opened ⇒ push_samples is a no-op
    let token = cancel.clone();
    let h = tokio::spawn(pump_audio(Box::pin(stream), player, cancel));
    token.cancel();
    h.await.unwrap();                          // returns promptly via tokio::select! cancel arm
}
```
Also assert the **error arm**: a stream yielding `TtsEvent::Error{..}` makes `pump_audio`
call `player.cancel()` and return (speak_aloud.rs:224-228); and the **stream-end arm**:
`None` ⇒ clean return (speak_aloud.rs:216). `TtsEventStream` is exactly
`Pin<Box<dyn Stream<Item=TtsEvent>+Send>>`, so `Box::pin(futures_util::stream::iter(..))`
is a drop-in fake — **no mock crate, no device, fully deterministic.**

---

## 4. Per-module: pure-logic tests (NO network/hardware) — concrete list

### `llm/api_client.rs`
1. **`ApiConfig` → request shape** is testable via `spawn_mock`: assert the POSTed JSON
   has `response_format:{type:"json_object"}` only when `json_mode` (api_client.rs:160-166)
   and that `Authorization: Bearer …` is present iff `api_key` is `Some`/non-empty
   (api_client.rs:184-188). (Capture request body in the mock.)
2. **`prefers_vllm_structured_outputs` is pure** (api_client.rs:273-279): `localhost:8000`,
   `127.0.0.1:8000`, `0.0.0.0:8000`, `…vllm…` → true; `api.openai.com` → false.
3. **`is_configured`** (api_client.rs:117): empty endpoint/model ⇒ false.
4. **Response parsing**: feed a canned `{"choices":[{"message":{"content":"hi"}}]}` (mock)
   and a non-2xx body → assert `Err` carries status+body (api_client.rs:194-198); empty
   choices ⇒ `"No response choices"` (api_client.rs:208). URL is `endpoint` with trailing
   `/` trimmed + `/chat/completions` (api_client.rs:177-180) — pure to assert.
5. **`extract_entities` JSON-parse failure** surfaces the raw text in the error
   (api_client.rs:260-265) — feed malformed JSON via mock.

### `asr/cloud.rs`
1. **`encode_wav` is pure & high-value** (cloud.rs:54): assert 44-byte header, `RIFF`/`WAVE`/
   `fmt `/`data` tags, little-endian sample-rate/byte-rate fields, and that f32 samples are
   clamped to ±1.0 then scaled by 32767 (cloud.rs:83-85). Round-trip a known buffer.
2. **WhisperResponse → TranscriptSegment mapping** (cloud.rs:181-215): with a canned
   `verbose_json` body via mock, assert empty-text segments are filtered (cloud.rs:184),
   `confidence = 1.0 - no_speech_prob` else 0.9 (cloud.rs:186), and `start/end_time` are
   offset by `segment.start_time` (cloud.rs:193-194).
3. **No-segments fallback** (cloud.rs:200-214): body with only top-level `text` ⇒ one
   segment spanning `start_time..end_time`; empty `text` ⇒ `Ok(vec![])`.
4. **URL build + auth**: `endpoint` trailing-slash trim + `/audio/transcriptions`
   (cloud.rs:116-119); `bearer_auth` only when key non-empty (cloud.rs:138). Assert via mock.
5. **Non-2xx → `Err` with status+body** (cloud.rs:147-151).

### `asr/mod.rs`
1. **`AsrConfig` constructors are pure** (mod.rs:60-94): `with_models_dir` joins
   `ggml-small.en.bin`; `with_models_dir_and_model` joins the given filename; `Default`
   values (`n_threads=4`, `temperature=0.0`, `beam_size=5`, lang `"en"`).
2. **`SpeechSegment` invariant**: `num_frames == audio.len()` (mod.rs:42-43) — assert on
   constructed values.
3. **Cloud-only build stub**: under `not(feature="asr-whisper")`, `AsrWorker::run` logs &
   returns without panic (mod.rs:221-228) — testable in the default (cloud-only) build.
4. **`segments_processed()` accessor** starts at 0 (mod.rs:316). NB: `transcribe_segment`
   needs a real `WhisperState` + model file (feature-gated, mod.rs:235) → **not**
   unit-testable without the model; skip it for B11.

### `speech/context.rs`
Pure type-aggregation module — `SpeechChannels` / `SpeechShared` / `SpeechConfig` /
`ExtractionDeps` are field bundles with no behaviour (context.rs:30-83). **Almost nothing
to unit-test here** beyond a `Clone`-is-cheap smoke test on `SpeechShared`/`SpeechConfig`
(both derive `Clone`, all `Arc`-wrapped). The testable *logic* that consumes these structs
lives in `speech/mod.rs` (the worker fns) and is better covered there (see existing
`speech/tests_integration.rs`, `tests_audio_accumulator.rs`). **Recommend: no new B11
tests targeting context.rs itself.**

### `llm/executor.rs` (recap from §2)
1. `is_rate_limited` matches `429` / `Too Many Requests` / `rate limit` (case-insensitive),
   rejects plain errors (executor.rs:60-64).
2. `note_extraction_error` + `extraction_in_cooldown` set/observe the cooldown window
   (executor.rs:56-76) — serialize these (shared static).
3. Extracted `run_attempts` fallback loop: in-order, stop-on-first-`Ok`, last-`Err`-on-all-fail.
4. Extracted `QueueState` push: drop-oldest at `MAX_BACKGROUND_QUEUE`, interactive-before-background pop.
5. `LlmPriority` / `LlmJobResult` mismatch handling (executor.rs:174-176, 204-205) — chat
   request that receives an Extraction result returns the right `Err` string.

### `speak_aloud.rs` (recap from §3)
1. `is_clause_boundary` truth table (speak_aloud.rs:42).
2. `append_delta` buffer/flush at last boundary + whitespace-skip (needs fake session).
3. `cancel` ⇒ `clear`→player.cancel→token.cancel→`close` ordering.
4. `pump_audio` cancel-arm / error-arm / stream-end-arm (in-memory stream, `AudioPlayer::new()`).
5. `maybe_new` returns `Ok(None)` for disabled / `TtsProvider::None` (pure).

---

## Recommendations summary
- **Don't add wiremock as the primary tool.** The blocking clients + the existing
  `spawn_mock` helper make it net-negative boilerplate here. Promote `spawn_mock` to a
  shared `#[cfg(test)]` test util (e.g. `src/test_support.rs`) and reuse across
  api_client / cloud / openrouter. Add `httpmock` (sync) only if a specific test needs
  rich matchers/verification.
- **Most B11 value is pure-logic + tiny seams**, not HTTP mocks: `encode_wav`, SSE (done),
  clause-buffering, config constructors, fallback-order loop, queue drop-oldest, error→kind
  mapping. These need zero network and zero hardware.
- **For streaming/cancel correctness**, test `SseDecoder` (done) + `pump_audio` (in-memory
  `Stream` + `CancellationToken`) + a frame-flushing `TcpListener`; mock servers can't
  model incremental SSE arrival.
- **`time::pause`/`advance`** (already compiled in via `tokio/full`) is for future
  reconnect-backoff tests, not the threaded executor.
- Two small, low-risk refactors unlock the best executor tests: extract `run_attempts`
  (fallback loop) and the `QueueState` push body into free functions.
