# Plan A1: TtsProvider trait + Deepgram Aura skeleton

**Goal:** Land a `TtsProvider` trait + a working `DeepgramAura` impl that
streams PCM audio from `wss://api.deepgram.com/v1/speak` over a WebSocket,
exposes start/flush/clear/close lifecycle, and emits a normalized event
stream the audio playback subsystem (Wave B) will consume.

**ADR:** [0004](../adr/0004-tts-provider-trait-and-deepgram-aura.md) (accepted).

**Backlog:** audio-graph-3132.

## Acceptance criteria

- [ ] `src-tauri/src/tts/mod.rs` exists with `TtsProvider` trait (async-trait,
  Send + Sync) defining: `open(voice, config) -> TtsSession`,
  `TtsSession::speak(text)`, `flush()`, `clear()`, `close()`, `events()`.
- [ ] `src-tauri/src/tts/deepgram_aura.rs` implements `TtsProvider`,
  connecting to `wss://api.deepgram.com/v1/speak` with `Authorization: Token <key>`
  via `tokio-tungstenite`. Default voice `aura-asteria-en`,
  `encoding=linear16`, `sample_rate=24000`.
- [ ] On `speak(text)`: send `{"type":"Speak","text":"<text>"}`. On `flush()`:
  send `{"type":"Flush"}`. On `clear()`: send `{"type":"Clear"}`. On
  `close()`: send `{"type":"Close"}`. On idle (no traffic for 8s): send
  `{"type":"KeepAlive"}` automatically.
- [ ] Reads server frames: binary frames → `TtsEvent::AudioChunk { samples: Vec<i16>, sample_rate: 24000 }`;
  JSON frames decoded into `TtsEvent::Status(...)` or `TtsEvent::Error(...)`.
- [ ] `TtsEvent` enum covers: `AudioChunk { samples, sample_rate }`,
  `Status(TtsStatus)` where TtsStatus has variants `Connected`,
  `Flushed { sequence: u64 }`, `Cleared`, `Disconnected`,
  `Reconnecting { attempt, backoff_secs }`. `Error { kind, message }`.
- [ ] `TtsConfig` struct: `voice: String`, `sample_rate: u32`,
  `encoding: TtsEncoding` (enum: Linear16, Mulaw, Alaw — only the
  streaming-compatible set per ADR-0004), `speed: f32` (range 0.7–1.5).
- [ ] Reconnect with exponential backoff (start 1s, cap 30s, jitter ±20%) —
  mirror `gemini/mod.rs::session_task` shape but for TTS.
- [ ] Unit tests in `#[cfg(test)] mod tests` covering: connect-disconnect
  cycle (mock WS server), `Clear` cancellation drops in-flight audio
  frames received after the Clear was sent, KeepAlive cadence, error
  classification.
- [ ] `Cargo.toml`: add `tokio-tungstenite` (already present), confirm
  `async-trait`, `futures-util` deps. No new heavy deps.
- [ ] `src-tauri/src/lib.rs` (or wherever modules are declared): add
  `pub mod tts;`.
- [ ] Settings enum entry: in `src-tauri/src/settings/mod.rs`, add
  `TtsProvider` enum with variants `None`, `DeepgramAura`. (Local providers
  Kokoro/Piper/Coqui are out-of-scope for this plan.)
- [ ] Tauri commands: `test_tts_connection_cmd(provider, key) -> Result<(), String>`.
  No `list_tts_voices_cmd` for v1 — voices are a fixed list per Aura docs;
  expose them as a TS constant in `src/types/index.ts`.
- [ ] Credentials allowlist: add `deepgram_tts_api_key` (or just reuse
  `deepgram_api_key` since the same key works for STT + TTS — RECOMMENDED
  to reuse and document the assumption in code comment).
- [ ] Frontend types in `src/types/index.ts`: `TtsProviderConfig`,
  `TtsEvent`, mirroring backend serde.

## Files

**New:**
- `src-tauri/src/tts/mod.rs` (~150 LOC: trait + types)
- `src-tauri/src/tts/deepgram_aura.rs` (~600–800 LOC: WebSocket client,
  reconnect, tests). Use `asr/deepgram.rs` as a structural reference, not
  a copy-paste source.

**Modified:**
- `src-tauri/src/lib.rs` — module declaration
- `src-tauri/src/settings/mod.rs` — TtsProvider enum + persistence
- `src-tauri/src/commands.rs` — `test_tts_connection_cmd`
- `src-tauri/src/credentials/mod.rs` — confirm `deepgram_api_key` covers
  both STT and TTS (no change needed if so)
- `src/types/index.ts` — TS types
- `src-tauri/Cargo.toml` — only if a missing crate is needed (probably not)

## Steps

1. Read `docs/research/verified-2026-05-19.md` and
   `docs/research/deepgram-aura-streaming-tts.md`. The verified doc
   wins where they disagree.
2. Read `src-tauri/src/asr/deepgram.rs` for shape, especially the session
   task pattern and event-emission style.
3. Write `tts/mod.rs` with trait + types. Land first as scaffolding
   (no impl, just types compiling).
4. Write `tts/deepgram_aura.rs` minimal version: connect, send Speak +
   Close, receive binary frames, no reconnect.
5. Add unit test using a `tokio-tungstenite` mock server (see
   `gemini/mod.rs:1500+` for a working pattern of an in-process WS server
   for tests).
6. Add reconnect + backoff loop. Add Cancel/Clear semantics. Verify
   Cleared ack arrives + caller drops trailing frames.
7. Wire settings + Tauri command + frontend types.
8. Run `cargo fmt`, `cargo clippy --all-targets`, `cargo test --lib tts`.
9. Stop at the trait + Aura. Audio playback (Wave B) consumes the
   `TtsEvent::AudioChunk` stream — that's the next plan.

## Tests

Unit tests in `tts/deepgram_aura.rs`:

- `connect_emits_connected_status_then_audio_chunk` — mock server, send
  binary frame, verify `TtsEvent::AudioChunk` arrives.
- `clear_drops_in_flight_audio_frames` — buffer 5 audio frames, send
  Clear, verify only frames received before the Clear-ack are kept.
- `keepalive_sent_after_idle_timeout` — mock server, no Speak for 8s
  simulated time, verify KeepAlive frame sent.
- `reconnect_after_disconnect_with_backoff` — mock server drops, client
  reconnects with backoff jitter, eventually emits Reconnected status.
- `error_classification_for_4xx_vs_5xx` — auth error vs server error
  shows up as distinct TtsError kinds.

## Dependencies on other plans / ADRs

- ADR-0004 (this plan's spec)
- audio-graph-3132 (this plan's tracker)
- Blocks: A1 enables Wave B (audio playback) and the speak-aloud loop.

## Rollback

Revert the new files + the settings + commands additions. Settings
migration drops the `tts` field if present (graceful default). No
runtime regression possible — the module is unused until Wave B
consumes it.

## Out-of-scope (defer to later plans)

- Audio playback subsystem — Wave B (audio-graph-8d75).
- Speak-aloud chat→TTS wiring — Wave C (audio-graph-92c7).
- Local TTS engines (Kokoro/Piper/Coqui) — audio-graph-1a8c.
- Voice picker UI beyond a fixed dropdown.
