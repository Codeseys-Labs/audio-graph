# Provider Connections — Adversarial Review (Fable)

Date: 2026-07-05
Scope: every external provider connection path in `audio-graph` —
ASR streaming WebSocket clients (`src-tauri/src/asr/`), the Gemini Live S2S
client (`src-tauri/src/gemini/mod.rs`), the LLM providers
(`src-tauri/src/llm/`, OpenRouter blocking + SSE streaming, generic API,
Bedrock), and the Deepgram Aura TTS client (`src-tauri/src/tts/deepgram_aura.rs`).
Read-only review; no code, `.seeds/`, or config was modified.

Method: read production code incrementally, compared sibling providers
side-by-side, and verified the highest-severity structural claims empirically
against the pinned dependency versions (`tokio-tungstenite = 0.29`,
`tungstenite = 0.29.0`) with a standalone reproduction.

---

## Severity summary

| Severity | Count | IDs |
|----------|-------|-----|
| Blocker  | 2 | B1 (AssemblyAI connect never handshakes), B2 (Gemini ApiKey connect never handshakes) |
| Major    | 4 | M1 (AWS Transcribe has no reconnect), M2 (no keepalive on 3 idle-capable ASR clients), M3 (Soniox reconnect resends session config but abandons turn silently → duplicate audio window), M4 (OpenRouter blocking client has no HTTP-layer retry while streaming/others differ) |
| Minor    | 5 | see below |
| Nit      | 4 | see below |

Blocker one-liners:
- **B1** `src-tauri/src/asr/assemblyai.rs:754` — `open_ws` hand-builds the upgrade `Request` with only `Authorization`; tungstenite 0.29 rejects it with `Protocol(InvalidHeader("sec-websocket-key"))` before any network I/O, so AssemblyAI ASR can never connect in production.
- **B2** `src-tauri/src/gemini/mod.rs:991` — the Gemini `ApiKey` connect path hand-builds the request with only `x-goog-api-key` + `Content-Type` and the same missing 5 mandatory WS headers, so API-key Gemini Live can never connect (the Vertex path at :1066 has the identical defect).

---

## Blockers

### B1 — AssemblyAI streaming can never establish a WebSocket (dead provider)
`src-tauri/src/asr/assemblyai.rs:751-768` (`open_ws`).

The request is built as:
```rust
let request = tungstenite::http::Request::builder()
    .uri(url.as_str())
    .header("Authorization", &config.api_key)
    .body(())?;
let (ws_stream, _response) = connect_async(request).await ...
```
When you pass a `&str`/`Uri` to `connect_async`, tungstenite's
`IntoClientRequest for Uri` injects the five mandatory upgrade headers
(`Host`, `Connection`, `Upgrade`, `Sec-WebSocket-Version`,
`Sec-WebSocket-Key`). When you pass an already-built `http::Request<()>`, the
`IntoClientRequest for Request` impl is the identity function — it adds
nothing — and `tungstenite::handshake::client::generate_request` then
**hard-requires** all five headers and errors if any is absent
(`~/.cargo/.../tungstenite-0.29.0/src/handshake/client.rs`, `WEBSOCKET_HEADERS`
loop; `client.rs` `IntoClientRequest for Uri`).

Deepgram's `open_ws` (`asr/deepgram.rs:520-532`) sets all five explicitly; the
Aura TTS `open_ws` (`tts/deepgram_aura.rs:424-436`) also sets all five;
AssemblyAI sets only `Authorization`.

Verified empirically against `tokio-tungstenite 0.29` / `tungstenite 0.29.0`
with a standalone repro that mirrors both styles:
```
assemblyai_style: ERR kind=PROTOCOL/HEADER => Protocol(InvalidHeader("sec-websocket-key"))
deepgram_style:   ERR ... => Http(Response { status: 401, ... "dg-error": "Invalid credentials." ... })
```
The AssemblyAI-shaped request fails at header generation **before any TCP/TLS**;
the Deepgram-shaped request reaches a real server 401. So every
`AssemblyAIClient::connect()` returns
`WebSocket connect failed: Protocol(InvalidHeader("sec-websocket-key"))`, the
speech processor logs "AssemblyAI connect failed" and returns
(`speech/mod.rs:4895-4908`), and the provider is 100% non-functional.

Why the tests miss it: the only coverage of `open_ws` is the `#[ignore]` live
smoke (`assemblyai.rs:1734`); all fast tests drive `run_io`/parsers against a
`ws_fixture` server they connect to with the library's `connect_async(&url)`
string path (`assemblyai.rs:1559` via `ws_fixture::connect_client`), which
*does* inject the headers — so the production `open_ws` request shape is never
exercised.

Fix direction: build the request the way Deepgram/Aura do — add
`Sec-WebSocket-Key` (`tungstenite::handshake::client::generate_key()`),
`Sec-WebSocket-Version: 13`, `Connection: Upgrade`, `Upgrade: websocket`, and
`Host` — or, simpler and less error-prone, use the `IntoClientRequest`
string/`Uri` path plus `request.headers_mut().insert("Authorization", …)`
exactly as `openai_realtime.rs:568-576` already does (that client is correct
because it starts from `url.into_client_request()`). Add a unit test that calls
`open_ws` against the `ws_fixture` server (not `connect_client`) so the
production request shape is exercised without a live key.

### B2 — Gemini Live (API-key and Vertex) can never establish a WebSocket
`src-tauri/src/gemini/mod.rs:991-1004` (ApiKey) and `:1066-1079` (VertexAI).

Same root cause as B1. The ApiKey path builds:
```rust
tungstenite::http::Request::builder()
    .uri(url_str)
    .header("x-goog-api-key", api_key)
    .header("Content-Type", "application/json")
    .body(())?
```
— missing all five mandatory WS headers. The Vertex path adds only
`Authorization` + `Content-Type`, also missing the five. Both feed
`connect_async(request)`, so both fail with
`Protocol(InvalidHeader("sec-websocket-key"))` before touching the network,
exactly as the empirical repro shows. `GeminiLiveClient` is instantiated from
`commands.rs:3722` and `:4438` (converse S2S / front-leg), so this is a live,
user-reachable path, not dead code.

Coverage gap mirrors B1: the fast Gemini tests drive `run_io`/parse paths
(`gemini/mod.rs:2243` `run_io_blocked_policy_...`), and `open_ws` has no
non-ignored coverage.

Fix direction: identical to B1 — set the five headers explicitly on both auth
branches, or switch to the `Uri`/string `IntoClientRequest` path and only
`insert` the provider auth header on top. One shared `open_ws` request-builder
helper for all hand-rolled clients would prevent the divergence recurring.

Note for the fix author: Deepgram and Aura are correct precisely because they
set the five headers by hand; OpenAI-realtime is correct because it uses
`into_client_request()`. Soniox is correct because it passes the URL string to
`connect_async` (`asr/soniox.rs:515`). AssemblyAI and Gemini are the only two
that hand-build a `Request` *and* omit the headers — the classic
sibling-asymmetry bug class.

---

## Major

### M1 — AWS Transcribe streaming has no reconnect; a mid-stream drop kills ASR for the session
`src-tauri/src/asr/aws_transcribe.rs:493-614`, dispatched from
`speech/mod.rs:5916`.

Every other streaming provider (Deepgram, AssemblyAI, Soniox, OpenAI-realtime,
Gemini, Aura) runs a reconnect ladder (`asr/reconnect.rs`, or per-file
`backoff_for_attempt`). AWS Transcribe does not: `run_streaming_session` opens
one `start_stream_transcription` stream and drives
`output.transcript_result_stream.recv()` in a single `while let` loop
(`aws_transcribe.rs:582-610`). On any transport error the `map_err` propagates
out of `run_aws_transcribe_session`, `run_aws_transcribe_speech_processor`
emits a one-shot `AWS_ERROR` + `StageStatus::Error` (`speech/mod.rs:6061-6087`)
and the session is over — no retry, no backoff. `grep -c "backoff\|reconnect\|retry"`
in `aws_transcribe.rs` is `0`.

Impact: a single TCP reset / Wi-Fi blip permanently ends AWS transcription for
the recording session while the app still looks like it is capturing. AWS
Transcribe streaming sessions also have their own idle/duration limits that
would benefit from re-establishment.

Fix direction: wrap the session open + drive loop in the same
attempt/backoff ladder the siblings use (`reconnect::next_reconnect_step`),
re-`start_stream_transcription` on a recoverable `SdkError`
(dispatch_failure/timeout/response), and emit `Reconnecting`/`Reconnected`
status so the UI parity matches the other providers. Preserve the
`is_transcribing` cancellation semantics inside the retry sleep.

### M2 — Three idle-capable ASR clients send no KeepAlive; Deepgram is the only one that does
`asr/deepgram.rs:1082-1106` sends `{"type":"KeepAlive"}` every
`KEEPALIVE_INTERVAL_SECS = 4`. `grep -c KeepAlive` is `11` for deepgram and
`0` for `assemblyai.rs`, `soniox.rs`, `openai_realtime.rs`, and
`gemini/mod.rs`.

Deepgram's own comment (`asr/deepgram.rs:174-176`) documents that the listen
socket idle-closes after ~10s without audio or a KeepAlive. The mixer feeds a
**continuous** silence-padded stream while transcribing
(`audio/mixer.rs:120-166`, "flush a (silence-padded) mixed frame"), so in the
common case audio never truly stops and the sockets stay warm — which is why
this hasn't surfaced as an outright outage. But the moment the mixer stalls,
`is_transcribing` pauses, or a provider's own idle window is shorter than the
inter-chunk gap during a quiet passage, the non-Deepgram clients will silently
idle-disconnect and then burn a reconnect-ladder attempt to recover
(AssemblyAI/Soniox/OpenAI). This is latent divergence: the resilience posture
differs per provider for no principled reason.

Severity is Major (not Blocker) precisely because the continuous silence-fill
usually masks it; treat it as "works until the audio cadence changes."

Fix direction: AssemblyAI v3, Soniox, and OpenAI-realtime all support an idle
keepalive/no-op; add the same `tokio::time::interval` keepalive branch
Deepgram has to each `run_io` (guarded by `last_outbound.elapsed()`), sending
the provider-appropriate idle frame. OpenAI-realtime can send nothing on the
socket but should at least not assume the server holds an idle transcription
session open indefinitely.

### M3 — Soniox reconnect abandons the active turn but keeps streaming buffered audio into a fresh turn namespace
`asr/soniox.rs:685-694` (reconnect success) + `:180-182`
(`abandon_active_turn`).

On reconnect, Soniox calls `parser.abandon_active_turn()` and resends the full
session-config payload via `open_ws` (`asr/soniox.rs:507-536`). But
`pending_chunks`/the unbounded audio channel are preserved across the
reconnect (by design, matching the other clients), so audio buffered during
the outage is flushed into the new socket. Because the parser's turn index is
per-`SonioxRealtimeParser` instance and only the *active turn* is abandoned
(not reset), the post-reconnect audio resumes under a **new** `turn-{n}` while
the pre-drop partials for the abandoned turn were already emitted downstream
with `is_final=false` and never supersed­ed/finalized. Downstream
(`speech/mod.rs:5375-5418`) keys spans off `revision.payload.span_id`
(`soniox:{source}:turn-{n}`), so the abandoned turn's provisional span is
orphaned — it can linger in the transcript ledger as a never-finalized partial.

Contrast: OpenAI-realtime explicitly preserves and **replays** the in-flight
command across reconnect (`asr/openai_realtime.rs:686`, `pending_cmd` +
`write_audio_cmd` replay) and resets its accumulator because item_ids are a new
namespace; Deepgram keys spans off start-time so a resumed stream re-collides
cleanly. Soniox is the odd one out: it drops turn state without emitting a
terminal/superseding revision for the abandoned span.

Fix direction: when abandoning the active turn on reconnect, emit a final (or
explicitly-superseded) revision for the abandoned `span_id` so the ledger
doesn't keep an orphaned provisional, mirroring how the OpenAI path finalizes.
At minimum document that a reconnect can strand one partial and have the
receiver reconcile it on the next final.

### M4 — OpenRouter blocking chat has no HTTP retry; timeout/5xx surfaces as a hard error while other paths retry or stream-recover
`src-tauri/src/llm/openrouter.rs:1378-1464`
(`chat_completion_with_routing_telemetry`).

The blocking OpenRouter client issues a single `client.post(...).send()` with a
60s request timeout (`HTTP_REQUEST_TIMEOUT`, :23) and no retry: a transient 429/
502/503 or a timeout returns `Err` straight to the caller. The streaming SSE
path (`llm/streaming.rs:772`) at least degrades gracefully with a partial
`full_text` on mid-stream drop, and the WS providers reconnect. The blocking
extraction path (used by `extract_entities`, :1468, on the hot transcript path)
therefore has the weakest resilience of any provider connection and will drop
an extraction on any blip. OpenRouter explicitly recommends retrying 429/5xx.

Fix direction: add a small bounded retry (2-3 attempts, jittered backoff) around
the blocking `send()` for idempotent completions on 408/409/429/5xx and
`is_timeout()`/`is_connect()` reqwest errors, mirroring the reconnect-ladder
budget the streaming clients use. Keep it off for 4xx auth/validation errors.

---

## Minor

- **m1 — AssemblyAI/Soniox/OpenAI-realtime give up permanently after 4 reconnect attempts with no re-arm.**
  `asr/reconnect.rs:4` caps the ladder at `[1,2,5,10]` then `GiveUp`. On
  exhaustion the session task emits a fatal `Error` and exits
  (`deepgram.rs:943-951`, `assemblyai.rs:892-901`, etc.). There is no outer
  "cold restart" — a network partition longer than ~18s ends the provider for
  the whole recording with only a toast. Consider a longer/again-resettable
  ladder for long-lived capture sessions, or a user-visible "reconnect" action.
  (Deepgram resets `reconnect_attempts = 0` on success, which is correct; the
  gap is only total-give-up behaviour.)

- **m2 — `AUDIO_BUFFER_MAX_CHUNKS` back-pressure semantics differ across clients.**
  Deepgram/AssemblyAI/Soniox/OpenAI-realtime use a hard cap of 200 chunks and
  flip `user_disconnected` + return an error when exceeded
  (`deepgram.rs:390-396`), i.e. a stuck reconnect *kills* the session. Gemini
  instead uses a bounded `tokio::mpsc` of `GEMINI_AUDIO_QUEUE_CAP = 1000` and
  **drops newest** on full while keeping the session alive
  (`gemini/mod.rs:625-642`). Two different overflow policies (fail-fast vs
  lossy-continue) for the same "reconnect is stuck" condition. Pick one policy
  intentionally; the divergence is not documented as deliberate.

- **m3 — `pending_chunks` decrement-before-send can under-count on write error.**
  In Deepgram/AssemblyAI/Soniox `run_io`, the `Chunk` arm does
  `pending_chunks.fetch_sub(1)` *before* the `send_binary` await
  (`deepgram.rs:1115-1117`, `assemblyai.rs:1001-1003`, `soniox.rs:742-744`).
  On a send error the chunk is dropped and the counter has already been
  decremented, which is consistent — but OpenAI-realtime deliberately does the
  opposite (holds the decrement so a replayed chunk still counts,
  `openai_realtime.rs:874-876,905-921`). The three ASR clients cannot replay,
  so they can't over-count, but the asymmetry means a future "add replay to
  Deepgram" change would silently double-decrement. Worth a comment noting the
  invariant per client.

- **m4 — AWS Transcribe `language_code` parse failure silently falls back to `en-US`.**
  `asr/aws_transcribe.rs:519-522`: `.parse::<LanguageCode>().unwrap_or(EnUs)`.
  A typo'd or unsupported language code is silently coerced to US English with
  no log/warn, so a user configured for e.g. `de-DE` who mistypes gets English
  transcripts with no signal. Log a warning on the fallback (the other
  providers surface config problems as errors). Non-secret value, safe to log.

- **m5 — AssemblyAI hardcodes `speech_model=universal-3-5-pro` and ignores any configured model.**
  `asr/assemblyai.rs:775` always appends `DEFAULT_MODEL`; unlike Deepgram
  (`enable_diarization` + model threaded from settings) and Soniox
  (`config.model`), the `AssemblyAIConfig` has no `model` field at all
  (`assemblyai.rs:119-127`). If AssemblyAI ships a new tier the app can't select
  it without a code change, and the settings/model picker (if any) is a no-op
  for this provider. Confirm this is intended (v3 may only expose one streaming
  model) or thread the model through like the siblings.

## Nits

- **n1 — Duplicated `response_request_id` + `diagnostic_path` helpers.**
  `llm/openrouter.rs:1598` and `llm/streaming.rs:1050` define byte-identical
  `response_request_id` (same header list, same sanitizer) and `diagnostic_path`.
  Fold into one shared helper to keep the header allow-list from drifting.

- **n2 — Backoff ladder is copy-pasted four times.**
  `asr/reconnect.rs` (shared, used by the ASR trio), plus independent
  `backoff_for_attempt` in `gemini/mod.rs:1203`, `tts/deepgram_aura.rs:546`,
  and `openai_realtime/mod.rs`. Aura adds jitter, the ASR shared one doesn't.
  Consolidating (with jitter as an option) would remove the risk of the
  schedules silently diverging.

- **n3 — Aura keepalive comment says 8s honouring "ADR", Deepgram listen says 4s; both cite ~10s idle.**
  `tts/deepgram_aura.rs:69` uses 8s; `asr/deepgram.rs:176` uses 4s. Same vendor,
  same ~10s idle window, two different safety margins. Harmless but worth a note
  on why TTS gets a looser margin.

- **n4 — `emit_disconnected_once` exists for Deepgram + OpenAI-realtime but not Soniox/AssemblyAI.**
  Deepgram (`deepgram.rs:826`) and OpenAI-realtime (`openai_realtime.rs:645`)
  route `Disconnected` through a one-shot atomic guard so teardown never
  double-emits; Soniox and AssemblyAI emit `Disconnected`/`SessionTerminated`
  directly from multiple arms (`soniox.rs:614,620,625,646,667,675`) and can
  double-emit on a race between `disconnect()` and the session task. Downstream
  is idempotent enough today (status re-set), so it's cosmetic — but the guard
  pattern should be applied uniformly.

---

## Provider parity matrix

Legend: Y = present/correct, N = absent, — = N/A, ! = present-but-defective.

| Provider | Connect works | Reconnect ladder | KeepAlive | Backpressure cap | Teardown (Drop) | Errors surface to user | Key redaction on connect err |
|----------|:-------------:|:----------------:|:---------:|:----------------:|:---------------:|:----------------------:|:----------------------------:|
| Deepgram ASR | Y | Y (1/2/5/10) | Y (4s) | Y (200, fail-fast) | Y (rt shutdown) | Y (status+events) | Y |
| AssemblyAI ASR | **N (B1)** | Y (1/2/5/10) | N (M2) | Y (200, fail-fast) | Y | Y | Y |
| Soniox ASR | Y | Y (1/2/5/10) | N (M2) | Y (200, fail-fast) | Y | Y | Y |
| OpenAI-realtime ASR | Y | Y (1/2/5/10, replays in-flight cmd) | N (M2) | Y (200, fail-fast) | Y | Y | Y |
| AWS Transcribe ASR | Y | **N (M1)** | — (SDK stream) | — (SDK bounded chan 16) | Y (rt drop) | Y (AWS_ERROR) | Y (metadata-only) |
| Gemini Live S2S | **N (B2)** | Y (+session resume) | N (M2) | Y (1000, lossy-drop) | Y | Y | Y |
| Deepgram Aura TTS | Y | Y (1/2/5/10 + jitter) | Y (8s) | — (unbounded cmd) | Y (task winddown) | Y (TtsEvent) | Y |
| OpenRouter LLM (blocking) | Y | **N retry (M4)** | — | — | — (per-request) | Y | Y (redacted diag) |
| OpenRouter/API LLM (SSE) | Y | N (partial-text on drop) | — | — | Y (cancel token) | Y | Y (redacted excerpt) |
| Bedrock LLM (Converse) | Y | N (single stream) | — | — | Y | Y (classified) | Y |

Credential handling: no raw key reaches any log or UI-visible string on the
paths reviewed. Every `Debug` impl redacts the key
(`redacted_secret_presence`), every connect/reconnect/read error string is run
through `redacted_provider_diagnostic` / `redacted_error_excerpt` with the
key(s) registered as secrets, keys are sent as headers not URL query strings
(Gemini ApiKey comment at `gemini/mod.rs:979-981` is explicit about this), and
OpenRouter routing telemetry sanitizes provider/model metadata and rejects
credential-shaped tokens (`openrouter.rs:986-1016`). The
`ProviderContentEgressPolicy` write-guard at the transport layer
(`asr/transport.rs`) is a genuine defense-in-depth win and is exercised by a
non-vacuous socket-level test. No credential findings.
