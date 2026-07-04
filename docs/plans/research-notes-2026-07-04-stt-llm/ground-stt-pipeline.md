# Ground truth: audio → transcript pipeline (batching & buffering focus)

Repo: `/mnt/e/CS/github/audio-graph` (Tauri v2 app; Rust backend under `src-tauri/`, React/TS frontend under `src/`).
Scope: READ-ONLY analysis. Focus on how audio becomes transcript TODAY, esp. batching/buffering, endpointing/VAD, and delivery to the frontend/graph. Deepgram is the reference provider.

---

## 0. Top-level dataflow

```
capture (cpal, mic/system, 48k stereo)
  → audio::pipeline::AudioPipeline  (mixdown → rubato resample 48k→16k mono → fixed 32ms chunks)
  → ProcessedAudioChunk bus (crossbeam channel, `processed_rx`)   [16 kHz mono f32, ~512 frames = 32 ms]
  → per-provider speech processor (speech/mod.rs)
       ├─ LOCAL whisper path:  AudioAccumulator → ~2 s AccumulatedSegment → AsrWorker::transcribe_segment (batch)
       └─ CLOUD streaming path (Deepgram/AssemblyAI/Soniox/OpenAI-realtime):
              audio sender loop → client.send_audio(&chunk.data)   [forwards raw 32ms chunks, NO accumulation]
              WebSocket ⇄ provider
              provider JSON → DeepgramEvent (crossbeam) → run_deepgram_event_receiver
                → emit_asr_partial_with_meta (interim)  /  emit_transcript_and_extract_with_meta (final)
                → AppHandle::emit(ASR_PARTIAL / ASR_SPAN_REVISION / TRANSCRIPT_UPDATE / TURN_EVENT)
                → coalesce_submit → LLM entity extraction → GRAPH_DELTA / GRAPH_UPDATE
```

Chunk granularity is defined in `src-tauri/src/audio/pipeline.rs:42-46`:
`PROCESSED_AUDIO_CHUNK_DURATION_MS = 32` (`~32ms at 16kHz, suitable for streaming ASR`). `pipeline.rs` only downmixes + resamples; it does **not** VAD-gate — every chunk is emitted (`pipeline.rs:141` `Process a single audio chunk: mixdown → resample → accumulate → emit`).

---

## 1. Streamed word-by-word or batched? Partial vs final handling.

**Two distinct regimes coexist, selected by ASR provider (`AsrProvider` enum matched in `speech/mod.rs` ~2903–3082):**

### (a) Local Whisper = BATCHED (not streaming)
- `asr/mod.rs:190` `SpeechSegment` (~2 s of 16k mono audio) is the ASR input unit.
- `speech/mod.rs:918-923`: `TARGET_FRAMES = 16_000 * 2` (2 s), `OVERLAP_FRAMES = 16_000/2` (0.5 s overlap between segments).
- `AudioAccumulator` (`speech/mod.rs:2711-2786`) buffers 32ms `ProcessedAudioChunk`s until ≥2 s, emits an `AccumulatedSegment`, retains a 0.5 s tail as overlap (`take()` at 2753). Doc at `speech/mod.rs:901-903` says individual 32ms chunks are "too short for coherent speech recognition."
- `AsrWorker::transcribe_segment` (`asr/mod.rs:286-368`) runs whisper `full()` once per 2 s segment → 0+ `TranscriptSegment`s. All whisper output is effectively **final** (no interim concept).

### (b) Cloud streaming (Deepgram etc.) = STREAMED with partial + final
- Deepgram audio sender loop `run_deepgram_speech_processor` (`speech/mod.rs:4367-4489`) forwards **each raw 32ms chunk directly**: line 4467 `// Send audio directly to Deepgram (no accumulation needed).` → `client.send_audio(&chunk.data)` (4471). No local re-batching for cloud.
- `DeepgramStreamingClient::send_audio` (`asr/deepgram.rs:363-410`) converts f32→i16 LE PCM, queues on an **unbounded** tokio mpsc; a writer task drains to binary WS frames. Backpressure cap: `AUDIO_BUFFER_MAX_CHUNKS = 200` (`deepgram.rs:173`, ~10 s of audio) — over the cap it flips `user_disconnected` and errors (4390-396). Keepalive text frame every 4 s idle (`KEEPALIVE_INTERVAL_SECS = 4`, `deepgram.rs:176-177`).
- Deepgram v1/listen URL sets `interim_results=true&punctuate=true` (`deepgram.rs:775`).
- Server JSON parsed in `handle_server_message_with_key` (`deepgram.rs:1216-1410`). `"Results"` messages (1247-1339): read `is_final`, `speech_final`, `start`, `duration`, `channel.alternatives[0].transcript`, word-level `words[]` (each `word/start/end/confidence/speaker`). Emits `DeepgramEvent::Transcript { text, confidence, is_final, speech_final, start, duration, words }` only when transcript text is non-empty (1313). If `speech_final` also emits a `Turn{SpeechFinal}` (1325-1337).

**Partial vs final split lives in `run_deepgram_event_receiver` (`speech/mod.rs:4525-4813`):**
- `!is_final` (interim) → `emit_asr_partial_with_meta` (4610) then `continue` — interim results do **not** hit persistence/diarization/extraction; they only surface a live hypothesis. Revision numbering per `span_id` via `next_span_revision` (span id = `provider_start_span_id("deepgram", source_id, start)`, 4596). `raw_event_ref = "deepgram.results.interim"`.
- `is_final` → speaker remap (`remap_deepgram_speaker`, 4499-4520, caps over-segmented speaker ids to `max_speakers`), build `TranscriptSegment`, optional local diarization if Deepgram gave no speaker (4659-4681, note: event path has **no audio**, so Simple backend assigns default speaker), then `emit_transcript_and_extract_with_meta` (4695). `raw_event_ref = "deepgram.results.final"`.

**Emission internals:**
- `emit_asr_partial_with_meta` (`speech/mod.rs:601-669`): records an `AsrSpanRevisionPayload{ is_final:false, stability:Partial, end_of_turn:false }` into the transcript ledger + writer, then emits `ASR_SPAN_REVISION` and legacy `ASR_PARTIAL` (`AsrPartialPayload`). Empty text is dropped (612).
- `emit_transcript_and_extract_with_meta` (`speech/mod.rs:1967+`): builds `AsrSpanRevisionPayload{ is_final:true, stability:Final, end_of_turn:true }` (2008-2013), records to ledger/persistence, pushes into a 500-item ring transcript buffer (2030-2035), persists the segment (2037-2041), emits `ASR_SPAN_REVISION` + diarization span revision + `TRANSCRIPT_UPDATE` (2062), spawns an agent-proposal task, then submits to LLM extraction via `coalesce_submit` (see §4).
- Provider-neutral typed payloads live in `events.rs`: `AsrPartialPayload` (211), `AsrSpanStability{Partial,Final}` (224), `AsrSpanRevisionPayload` (231-270, carries `span_id/revision_number/supersedes/turn_id/end_of_turn/*_latency_ms`), `TurnEventKind` (330-338), `TurnEventPayload` (342-357). This is the revision/retcon contract: partials and finals for the same `span_id` are ordered by `revision_number` + `supersedes`.

---

## 2. Endpointing / VAD / turn-boundary detection today

**There is NO local VAD in the live path.** `src-tauri/src/aec_vad/mod.rs` is an explicit fixture-only scaffold (module doc lines 1-8: "does NOT wire a runtime", seed `audio-graph-098b`, blocked-on `0bdc`); no `aec_vad`/`Vad`/`is_speech` references exist in `audio/*.rs` or `speech/mod.rs` (grep empty). `pipeline.rs` passes every chunk through (no energy/silence gate). Whisper path has no endpointing at all — it just cuts fixed 2 s windows.

**Endpointing/turn detection is entirely PROVIDER-SIDE for the cloud path**, normalized into a provider-neutral `TURN_EVENT`:

- **Deepgram Nova (v1/listen)** — request params built in `deepgram_listen_url` (`deepgram.rs:762-803`) for non-flux models: optional `&endpointing={ms}` (`endpointing_ms`), `&utterance_end_ms={ms}` (`utterance_end_ms`), `&vad_events=true` (`vad_events`). Config fields at `deepgram.rs:126-132`.
  - `SpeechStarted` message → `Turn{SpeechStarted}` (`deepgram.rs:1384-1400`).
  - `UtteranceEnd` message → `Turn{UtteranceEnd}` (1363-1383); guards `last_word_end < 0` (the `-1` sentinel) and drops it (1368-1371).
  - `speech_final:true` on a Results frame → `Turn{SpeechFinal}` (1325-1337).

- **Deepgram Flux (v2/listen)** — turn-based conversational model, routed by the `flux-` prefix (`DEEPGRAM_FLUX_MODEL_PREFIX`, `deepgram.rs:643`; closed enum `DEEPGRAM_FLUX_MODELS = [flux-general-en, flux-general-multi]`, 655). Flux URL uses `&eot_threshold`, `&eager_eot_threshold`, `&eot_timeout_ms` instead of endpointing (781-789; config 133-139).
  - `TurnInfo` messages parsed by `handle_flux_turn_info` (`deepgram.rs:1465-1490`): reads `event`/`turn_event`/`state` → `StartOfTurn`, `EagerEndOfTurn`, `EndOfTurn`, `TurnResumed`. Also top-level message types `StartOfTurn`/`EagerEndOfTurn`/`EndOfTurn`/`TurnResumed` handled directly (1343-1354). `DeepgramTurnKind` enum at 97-105.

- **Normalization**: `DeepgramEvent::Turn{kind,...}` → `run_deepgram_event_receiver` maps `DeepgramTurnKind` → `events::TurnEventKind` (`speech/mod.rs:4720-4728`; note SpeechStarted **and** StartOfTurn both collapse to `TurnEventKind::SpeechStarted`) → `emit_turn_event` → `AppHandle::emit(TURN_EVENT, TurnEventPayload)` (`speech/mod.rs:684-700`). `TurnEventKind` also has a `LocalWindow` variant (`events.rs:337`) for local diarization windows.

- **Silence timers**: the only backend timers are (a) the 4 s Deepgram **keepalive** (not endpointing), and (b) the downstream extraction **coalescing** idle/age flush (§4) — a batching timer, not speech-boundary detection. The 500 ms `recv_timeout` heartbeat in the receiver loops (`speech/mod.rs:4561`) only drives coalesce flush + exit checks.

So today: turn boundaries come from the provider; the app trusts Deepgram Nova endpointing/UtteranceEnd or Flux EOT events and re-emits them as `TURN_EVENT`. No app-owned VAD/endpointer exists yet.

---

## 3. Delivery to frontend/graph: AppHandle::emit vs ipc::Channel (1534 migration)

**TODAY: everything is `AppHandle::emit` (broadcast events).** No `ipc::Channel<T>` usage exists anywhere in `src-tauri/src` (grep for `ipc::Channel`/`tauri::ipc`/`Channel<` returned nothing). The central helper is `events::emit_or_log` (`events.rs:524-532`) → `app.emit(event, payload)` with error logging. Some sites call `app_handle.emit(...)` directly (e.g. `TRANSCRIPT_UPDATE` at `speech/mod.rs:2062`, `PIPELINE_STATUS_EVENT` at 576).

Event name constants (`events.rs`): `TRANSCRIPT_UPDATE`=`"transcript-update"` (7), `ASR_PARTIAL`=`"asr-partial"` (10), `ASR_SPAN_REVISION`=`"asr-span-revision"` (16), `DIARIZATION_SPAN_REVISION` (21), `TURN_EVENT`=`"turn-event"` (27), `GRAPH_UPDATE` (41), `GRAPH_DELTA` (53), `PROJECTION_PATCH` (57), `PIPELINE_STATUS_EVENT` (68), `SPEAKER_DETECTED` (84).

**IN FLIGHT — seed `audio-graph-1534` "Migrate streaming hot paths to `ipc::Channel<T>`"** (size L). Per `docs/plans/2026-07-03-backlog-zero-plan.md:152`, target files: `commands.rs, events.rs, speech/mod.rs, ipc-contract/, useTauriEvents.ts`. Conflict note (line 157): 1534 touches `commands.rs`/`speech/mod.rs` shared with other seeds — "keep functions disjoint or serialize `1534` last." It is currently the Rust lane of Wave 7 (task #50, in_progress) but **not landed** — the hot paths (ASR partial/final/turn, graph deltas) still go over `AppHandle::emit`.

Rationale for the migration (typical Tauri): `AppHandle::emit` is a fan-out/broadcast to all windows with per-event JSON serialization; `ipc::Channel<T>` is a per-invocation ordered typed stream — better for high-rate ordered streams like interim ASR hypotheses and graph deltas. The revision/`supersedes` machinery in `AsrSpanRevisionPayload` currently compensates for emit's lack of ordering guarantees.

**ipc-contract crate** (`src-tauri/crates/ipc-contract/`, workspace member per `Cargo.toml:9`): today it owns Rust-authored types exported to TS (`AudioSourceInfo` + audio source/channel provenance enums, `lib.rs`; `session_data_movement.rs`). Export bins under `crates/ipc-contract/src/bin/`. This is the crate the 1534 typed-channel contracts would land in. The streaming payloads (`AsrSpanRevisionPayload`, `TurnEventPayload`, etc.) currently live in `src-tauri/src/events.rs`, NOT yet in the ipc-contract crate.

---

## 4. Where a batching/segmentation seam naturally lives

Three existing seams, in increasing downstream order — a new batching/segmentation layer would slot cleanly at any of them:

1. **Audio-in accumulation seam (`AudioAccumulator`, `speech/mod.rs:2711-2805` + `feed_source_accumulator`/`flush_source_accumulators`).** Already the batching point for the whisper path (2 s windows + 0.5 s overlap, per-source keyed HashMap). The cloud path deliberately BYPASSES it (`run_deepgram_speech_processor` sends raw 32ms chunks, comment at 4467). **This is the natural home for an app-owned VAD/endpointer or a re-segmentation buffer** that would feed batch/near-real-time ASR (e.g. Moonshine, or batching cloud calls). It sits right on the `ProcessedAudioChunk` bus before provider handoff.

2. **Turn/event boundary seam (`run_deepgram_event_receiver` + `emit_turn_event`, `speech/mod.rs:4712-4744`; per-provider receivers 4525/4947/5275/5616).** Provider turn signals are already normalized to `TurnEventKind` here. A segmentation layer that groups partials/finals into "turns" using `TURN_EVENT` (Deepgram SpeechFinal/UtteranceEnd/Flux EndOfTurn) would live in these receiver loops. The `span_id` + `revision_number` + `supersedes` fields on `AsrSpanRevisionPayload` are the existing primitives for stitching interim→final and retconning turns.

3. **LLM-extraction coalescing seam (`coalesce_submit` / `flush_batch` / `flush_pending_if_due` / `flush_pending_now`, `speech/mod.rs:2461-2632`).** THE existing batching layer for LLM work. Finals are coalesced into `PendingBatch` by same-speaker runs and flushed on: speaker change, `COALESCE_MAX_SEGS=3` segments, `COALESCE_MAX_CHARS=500`, idle `COALESCE_IDLE_MS=1000`, or age `COALESCE_MAX_AGE_MS=3500` (constants 2473-2479). Idle/age flush is driven by the receiver loop's 500ms heartbeat (`flush_pending_if_due`) and shutdown (`flush_pending_now`). Sliding-window context of 6 prior transcript segments is attached per batch (`CONTEXT_WINDOW=6`, 2091). **This is where "batch N turns before an LLM call" already happens** — an LLM-driven segmentation/summarization stage would extend or parallel this.

**Recommendation for a new STT→LLM batching/segmentation stage:** the cleanest seam is (2) the per-provider event receiver, keyed on the already-normalized `TURN_EVENT` + `AsrSpanRevisionPayload(is_final/end_of_turn)` stream — it is provider-neutral, carries revision/supersedes identity, and is upstream of the existing LLM coalescer (3) which can be reused/retuned. If the goal is audio-domain segmentation (VAD/endpointing before ASR), seam (1) `AudioAccumulator` on the `ProcessedAudioChunk` bus is the home, and `aec_vad/mod.rs` is the pre-existing (unwired) scaffold for that stage.

---

## Key file:line index
- `src-tauri/src/audio/pipeline.rs:22` `ProcessedAudioChunk`; `:42-46` 32ms chunk const; `:141` process() no VAD gate.
- `src-tauri/src/asr/mod.rs:190` `SpeechSegment`; `:286-368` whisper `transcribe_segment` (batch).
- `src-tauri/src/asr/deepgram.rs:50-91` `DeepgramEvent`; `:97-105` `DeepgramTurnKind`; `:119-142` `DeepgramConfig`; `:363-410` `send_audio`; `:762-803` `deepgram_listen_url` (v1 nova vs v2 flux + endpointing params); `:1216-1410` `handle_server_message_with_key` (Results/interim/final/turn parse); `:1325-1337` speech_final→Turn; `:1363-1400` UtteranceEnd/SpeechStarted; `:1465-1490` `handle_flux_turn_info`.
- `src-tauri/src/speech/mod.rs:601-669` `emit_asr_partial_with_meta`; `:684-700` `emit_turn_event`; `:918-923` TARGET/OVERLAP frames; `:1967+` `emit_transcript_and_extract_with_meta`; `:2461-2632` coalescing (constants 2473-2479); `:2711-2805` `AudioAccumulator`; `:4367-4489` `run_deepgram_speech_processor` (raw-chunk sender, no accumulation @4467); `:4499-4520` `remap_deepgram_speaker`; `:4525-4813` `run_deepgram_event_receiver`.
- `src-tauri/src/events.rs:211` `AsrPartialPayload`; `:224` `AsrSpanStability`; `:231-270` `AsrSpanRevisionPayload`; `:330-357` `TurnEventKind`/`TurnEventPayload`; `:524-532` `emit_or_log`.
- `src-tauri/crates/ipc-contract/src/lib.rs` (typed export crate, currently audio-source types only); `Cargo.toml:9` workspace member.
- `src-tauri/src/aec_vad/mod.rs:1-27` fixture-only AEC/VAD scaffold (NOT wired).
- 1534 migration plan: `docs/plans/2026-07-03-backlog-zero-plan.md:152,157`.
