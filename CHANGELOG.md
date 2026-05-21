# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

### Changed

### Fixed

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
