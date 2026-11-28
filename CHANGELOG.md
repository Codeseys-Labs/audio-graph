# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Open a completed session's transcript, speaker labels, and speaker timeline from the Sessions browser while capture is running (#140). Review reads a query-backed snapshot (`load_session`) in a separate store, refreshes on open and window focus, and never writes the live session's state; Notes, Timeline, and Graph stay gated until they migrate.
- rsac 0.5.0 `hotplug-recovery` auto-recovery for `SystemDefault` capture on
  Windows and Linux (withheld on macOS per rsac ADR-0021): a default-device
  change or stream invalidation now reopens the stream — tracked via rsac
  stream generations — instead of ending the session; recovery transitions are
  logged and reported as anonymous capture telemetry.
- Name, merge, or unbind session speaker clusters with durable atomic relabels; local clustering now distinguishes capture sources.
- Speaker identity panel (#340): name, claim ("This is me"), merge, or unbind session speaker clusters from the live/review workspace, plus a `self_display_name` setting backing the self-claim.
- Backend-only decision-engine registry and opt-in Laya ONNX inference using the
  existing app-data model cache. Includes calibrated probabilities, state
  windowing, Unicode-script routing, Python-reference fixtures, and a native
  parity/latency runner. CUDA disables TF32 for probability parity. The
  multilingual development exporter pins source/toolchain versions and verifies
  input and output SHA-256 hashes; model binaries remain outside Git.
- Hosted multilingual Laya FP32 downloads from a pinned Hugging Face revision
  through the existing model manager, with SHA-256 checks for every component.
  The native loader accepts the hosted file layout and checks ONNX input metadata.
- Opt-in (`decision-d3`) native Decision 3.0 backend (`d3-edge`, `d3-lite`,
  `d3-nano`, `d3-mini`) on candle: Qwen3.5 encoder with bidirectional full
  attention, fused Gated DeltaNet kernels (CPU, CUDA via NVRTC), 255-code
  readout, model-manager entries pinned to the parity-fixture revisions. FP32
  answers match the Hugging Face reference to within 5e-6 per probability.
- Opt-in (`decision-clef`) native Clef-flash engine (`Cloudflare/clef-flash`, 9B)
  on the same candle Qwen3.5 encoder: the checkpoint's joint schema prompt and
  head, one forward pass per request, BF16 on GPU (14 GB) with an FP32-compute
  option, model-manager entry pinned to the parity-fixture revision (listed only
  in `decision-clef` builds). FP32 compute matched the reference to within 1e-6
  per probability before the per-norm cosine clamp fix; parity has not been
  re-measured since (audio-graph-a1c7).
- Opt-in, shadow-only named decision routes for live-assist proposal kind and
  answerable-question observations. Heuristics retain all mint and spend authority.
  Records carry raw/calibrated probabilities, model-pin metadata, canonical state
  digests, and movement-ledger correlation ids. Hosted routes remain deferred;
  native cache selection never falls back to cloud.

### Changed

- Live diarization now chooses the highest-scoring one-to-one speaker assignment
  across each window, avoiding extra speaker IDs caused by greedy matches.
- Native decision backends now re-export the shared decision contract directly;
  the obsolete vendored module and unused direct dependencies are removed.
- Upgraded audio capture to `rsac` 0.5.0 (`wasapi` 0.27.0), with Windows
  STA/COM and `SILENT` buffer fixes, PipeWire `DONT_RECONNECT` on Linux,
  and system-capture capability reporting for macOS 14.4 and later.

### Fixed

- A panic inside one LLM executor job no longer leaves other jobs waiting
  forever. That job and the jobs queued behind it end at once (notes synthesis
  and projection with an error, entity extraction with its rule-based
  fallback), and the next job starts a new worker (#539).
- Soniox duration rollovers emit the standard reconnect events without a
  separate adapter error event. The ASR stage still uses its standard reconnect
  display. Deepgram, AssemblyAI, Soniox, and OpenAI report a stalled audio
  backlog as a timeout. Aura uses request and protocol diagnostics for text and
  WebSocket size limits.
- ASR sessions that fail soon after readiness now consume the reconnect budget.
  Deepgram preserves an intentional stop during final transcript drain. Soniox
  stops replaced connections and opens a fresh session after a duration limit.
  WebSocket size and buffer errors no longer report account capacity exhaustion.
- Speaker revisions now correct only the affected turn in the graph, preserving
  other turns and their timeline links. Whole-speaker renames keep the original
  graph node when the new name is unused.
- Notes and graph items no longer record the file path of a local model. A
  self-hosted server that names its model by path (for example
  `/home/name/private.gguf`) is now recorded by the file name alone. Sessions
  saved before this fix are read and exported the same way; their files are
  not rewritten (#540).
- vLLM's documented `yes`/`no` noul probability keys are now normalized to the
  canonical `true`/`false` pair at the parse boundary, so documented responses
  validate; mixed, partial, or overlong key pairs are rejected, and the raw
  `WireResponse` bytes plus vendor confidence stay untouched.
- Updated `cookie` to 0.18.3 to prevent private-cookie nonce reuse after a fork
  ([RUSTSEC-2026-0594](https://rustsec.org/advisories/RUSTSEC-2026-0594)).
- An OpenRouter chat or question-card answer keeps the routing restrictions it
  was started under (zero data retention, deny data collection, provider
  pinning) when settings are saved while it starts. A provider switch at that
  moment could send the prompt to OpenRouter without them (#519).

## [0.1.0-rc.1] - 2026-05-20

First release candidate. The full STT → LLM → TTS pipeline is wired
end-to-end and CI-validated on Linux, Windows, and macOS. With API keys
pasted, ExpressSetup gets a fresh user from "downloaded" to "speech →
graph + chatbot replies + optional spoken-aloud audio" without code edits.

### Added

- **TtsProvider trait + Deepgram Aura streaming TTS**
  - New `src-tauri/src/tts/` module with an async-trait `TtsProvider` +
    `TtsSession` surface. Each provider lives in its own file (mirrors the
    ASR module shape).
  - `DeepgramAuraProvider` implementation: WebSocket client to
    `wss://api.deepgram.com/v1/speak`, exponential reconnect-with-jitter,
    `Speak`/`Flush`/`Clear`/`Close`/`KeepAlive` lifecycle, normalized
    `TtsEvent` stream.
  - Session-layer barge-in suppression: AudioChunk frames received between
    client-side `Clear` and server's `Cleared` ack are dropped at the
    session layer (audio-graph-7107) — consumers don't have to filter.
  - See [ADR-0004](docs/adr/0004-tts-provider-trait-and-deepgram-aura.md).

- **OpenRouter as a first-class LLM provider**
  - New `LlmProvider::OpenRouter` settings variant alongside the generic
    `Api` variant. Dedicated `test_openrouter_connection_cmd` (hits
    `/api/v1/models` — fast, free) and `list_openrouter_models_cmd` (live
    catalog for the model picker). Default attribution headers
    (`HTTP-Referer`, `X-OpenRouter-Title`).
  - SettingsPage gets a labeled OpenRouter dropdown + model picker.
  - ExpressSetup routes the "openrouter" choice through the first-class
    variant (was generic Api before this release) and saves the key under
    `openrouter_api_key`.
  - See [ADR-0005](docs/adr/0005-openrouter-as-recommended-llm-endpoint.md).

- **Streaming chat with token-delta IPC events**
  - `chat-token-delta` and `chat-token-done` Tauri events emitted as the
    LLM produces output. Replaces the prior block-and-batch `send_chat_message`
    semantics; the legacy command becomes a backward-compatible shim.
  - Frontend coalesces deltas through a 33ms throttle keyed by `request_id`
    so React doesn't re-render on every token.
  - `cancel_streaming_chat` aborts the in-flight HTTP body stream + emits
    a terminal `chat-token-done` with `finish_reason = "cancelled"`.
  - Provider's actual `finish_reason` (e.g. `"length"`, `"content_filter"`)
    is propagated through `TokenDelta::Done` instead of hardcoded `"stop"`.
  - Initial impl: `LlmProvider::Api` + `LlmProvider::OpenRouter`. Local
    engines and Bedrock continue to use the legacy blocking path; tracked
    as audio-graph-b373.
  - See [ADR-0006](docs/adr/0006-streaming-chat-and-native-s2s-separation.md).

- **Cross-platform audio playback subsystem**
  - New `src-tauri/src/playback/` module with `AudioPlayer` actor.
    Dedicated `std::thread` owns the cpal `Stream` (which is `!Send` on
    Windows). SPSC `ringbuf::HeapRb<i16>` buffers samples between the
    producer task and the realtime callback. `Arc<AtomicBool>` cancel
    drains the ringbuf and emits silence within ~10–20ms (one callback
    period) — the barge-in budget.
  - Tauri commands: `list_audio_output_devices_cmd`,
    `start_audio_playback_cmd`, `stop_audio_playback_cmd`.

- **Speak-aloud loop**
  - New `src-tauri/src/speak_aloud.rs` module with `SpeakAloudPipe`. When
    `AppSettings.speak_aloud == true` and `tts_provider` is `DeepgramAura`,
    each streaming chat reply is also piped to TTS (clause-boundary
    flushing for low first-audio latency) and out the playback subsystem.
  - Cancellation propagates: `cancel_streaming_chat` → `TtsSession::clear`
    → `AudioPlayer::cancel`.
  - SettingsPage gets a TTS section: provider dropdown, voice picker,
    speed slider, speak-aloud toggle, test-connection button.
  - ExpressSetup gets a "Also enable speak-aloud" checkbox when ASR is
    Deepgram (the same key authorises Aura).

- **CI hardening**
  - Pin third-party actions to commit SHAs.
  - Pin rsac sibling clone to a master SHA.
  - Add `lints` job with `cargo fmt --check` + `cargo clippy --all-targets`.
  - Add `cargo audit` job that runs in parallel with platform builds.
  - Workflow-level `permissions: contents: read` minimal token scope.
  - All six jobs (Linux + Windows + macOS Rust, Lints, cargo audit,
    Frontend) green on the release-candidate commit.

- **Architecture decision records**
  - ADR-0004 (TTS provider trait + Deepgram Aura)
  - ADR-0005 (OpenRouter as recommended cloud LLM)
  - ADR-0006 (streaming chat + native-S2S boundary; supersedes part of
    ADR-0003)

### Changed

- ADR-0003's matrix reframed: native S2S agents (Gemini Live,
  gpt-realtime-2) are sibling agents, not pipeline stages. The composed
  pipeline (STT → LLM → TTS, governed by ADRs 0001/0004/0005/0006) is the
  primary user-facing path. Native S2S provider implementations are
  separately tracked.

### Fixed

- Aura `clear_drops_in_flight_audio_frames` test: switched from
  current_thread to multi-thread runtime + `recv()` instead of
  `try_recv()` polling so the session task gets scheduled to fire its
  keepalive timer (the previous setup was racy on Windows + macOS).
- Aura `AudioChunk.sample_rate` was hardcoded to 24_000 regardless of
  `TtsConfig::sample_rate`. Now correctly threaded through.
- OpenRouter chat path attribution headers (`HTTP-Referer`,
  `X-OpenRouter-Title`) now set explicitly per-request — `default_headers`
  on `reqwest::blocking::Client` proved unreliable on Windows.
- ExpressSetup OpenRouter routing: was using the generic `LlmProvider::Api`
  variant + saving the key under `openai_api_key`. Now routes through the
  first-class `LlmProvider::OpenRouter` with the correct credential slot.

### Known limitations (tracked in `.seeds/`)

- Streaming chat on `LocalLlama` / `MistralRs` / `AwsBedrock` providers
  still uses the legacy blocking path (audio-graph-b373).
- No automatic resampling on the playback path; Aura at 24 kHz drives the
  device at 24 kHz. Some devices may reject and force the build to error.
- OpenAI Realtime gpt-realtime-2 native-S2S provider is ADR-recorded but
  not yet implemented (audio-graph-396f).
- Local TTS engines (Kokoro, Piper, Coqui) are not implemented
  (audio-graph-1a8c).
- Blacksmith Windows CI runners ship without an audio service; the
  `playback::tests::open_default_handles_missing_device_gracefully` test
  is `#[cfg(not(target_os = "windows"))]` to avoid a STATUS_ACCESS_VIOLATION
  crash inside cpal's WASAPI probe. Real Windows users see no impact.

[Unreleased]: https://github.com/Codeseys-Labs/audio-graph/compare/v0.1.0-rc.1...HEAD
[0.1.0-rc.1]: https://github.com/Codeseys-Labs/audio-graph/releases/tag/v0.1.0-rc.1
