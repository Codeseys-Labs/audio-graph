# AudioGraph Pipeline Modernization Backlog

This backlog is the working ledger for the 2026-05-17 deep-work loop. Items are
updated as waves land. "Blocked" means the work depends on unavailable
credentials, certificates, external hardware, or a product decision.

## P0 — Correctness / User-Visible Breakage

| ID | Status | Item | Notes |
|---|---|---|---|
| AG-P0-001 | Done | Sherpa ONNX model packaging mismatch | Downloader now treats Sherpa as a `.tar.bz2` archive-backed directory model, extracts it, and validates encoder/decoder/joiner/tokens before marking it usable. |
| AG-P0-002 | Done | Secrets can be serialized in provider settings | Settings save/load now migrates inline provider secrets into `credentials.yaml`, redacts `settings.json`/IPC payloads, and hydrates runtime-only provider configs from the credential store. |
| AG-P0-003 | Done | Session restore loads transcript only | Added `load_session` so UI loads transcript plus graph snapshot together. |
| AG-P0-004 | Done | Start flow is ambiguous | README now describes Start as capture and Transcribe/Gemini as the graph-producing processing paths. |
| AG-P0-005 | Done | Multi-source capture can corrupt source attribution | `AudioPipeline` now keeps independent resample/accumulation state per `source_id`, with a regression test for interleaved sources. |
| AG-P0-006 | Done | Batch/streaming ASR can mix concurrent sources | Batch local/cloud paths now keep independent speech accumulators per `source_id`; single-session streaming providers now reject multi-source transcription until per-source streaming fanout lands. |
| AG-P0-007 | Done | CI/release rsac path dependency was staged in the wrong place | GitHub Actions now checks out `audio-graph/` and clones `rsac/` beside it before Cargo runs, matching the `src-tauri` `../../rsac` path dependency on Linux, macOS, and Windows. |
| AG-P0-008 | Done | Gemini Live could fail frontend gating after credential redaction | The React control now treats configured Gemini auth mode as sufficient and lets the backend validate the secure credential store instead of requiring a redacted API key value in settings IPC. |
| AG-P0-009 | Done | Gemini Live startup race after connection | `start_gemini` now marks the runtime active before worker/event threads process the queued connected status, avoiding an immediate stale-active shutdown. |
| AG-P0-010 | Done | Default capture sample rate was incompatible with rsac | Defaults and UI options now use rsac-supported rates, with 48 kHz as the default and 16 kHz removed from capture settings. The audio pipeline still resamples processed chunks to 16 kHz for ASR. |

## P1 — Requested Pipeline Architecture

| ID | Status | Item | Notes |
|---|---|---|---|
| AG-P1-001 | Done | ADR for parallel realtime pipeline | `docs/adr/0001-parallel-realtime-pipeline.md` captures the split realtime topology, provider-specific cloud routing rules, vLLM/OpenAI-compatible backend approach, implementation waves, and rollback criteria. |
| AG-P1-002 | Done | Pipeline latency event contract + UI display | Backend emits current ASR/diarization/extraction/graph samples and the frontend status bar shows the latest per-stage timings. |
| AG-P1-003 | Done | Parallel diarization/extraction + agent loop design | Transcript finalization now emits non-blocking agent status/proposal events beside the existing diarization/extraction path, with frontend listener/store wiring and proposal toasts. |
| AG-P1-004 | Done | ASR provider contract cleanup | Keep cloud STT in Rust for `rsac` audio. Deepgram, AssemblyAI, Sherpa streaming, and AWS Transcribe now emit normalized interim `asr-partial` events, and streaming finals preserve source attribution. |
| AG-P1-005 | Done | Graph delta frontend consumption | Frontend now subscribes to `graph-delta`, applies node/edge deltas in the store, and full snapshots include edge IDs. |
| AG-P1-006 | Done | Agent/react loop skeleton | vLLM is documented/configured through the OpenAI-compatible LLM provider, API calls now have finite timeouts, chat/ER LLM work is routed through a priority executor, the OpenAI-compatible API client is synced from runtime-hydrated settings on load/save plus chat/transcription entrypoints, transcript-bound proposal/status events reach the UI, and backend-owned approved proposals can now update chat history or apply graph-suggestion actions to the temporal graph without replaying stale frontend payloads. |
| AG-P1-007 | Open | OpenAI Realtime provider family | ADR added in `docs/adr/0002-openai-realtime-provider.md`. Documentation now separates STT for the speech-to-notes/temporal-graph product from `gpt-realtime-2` for the parallel speech-to-speech agent. Implementation remains open: add backend-owned OpenAI Realtime support with `gpt-realtime-whisper` as a streaming STT ASR provider and `gpt-realtime-2` as a Gemini-like speech-to-speech voice-agent path. Needs settings/provider enums, Rust WebSocket client, `openai_api_key` hydration, OpenAI audio-format/resampling decisions, provider item-id correlation for deltas/finals, normalized transcript events, tool/action hooks, graph updates, latency telemetry, and tests. |
| AG-P1-008 | Open | Speech-to-speech agent provider matrix | ADR added in `docs/adr/0003-speech-to-speech-agent-provider-matrix.md`. Implementation remains open: introduce a first-class S2S agent provider surface for Gemini Live, OpenAI Realtime `gpt-realtime-2`, and local/hybrid STT -> vLLM -> TTS chains. Needs turn-state orchestration, local/cloud STT and TTS provider contracts, vLLM reasoning via OpenAI-compatible HTTP, cancellation/barge-in semantics, playback events, provider latency telemetry, and proposal-safe tool routing. |
| AG-P1-009 | Partial | Deepgram/local shared turn detector | Deepgram settings now expose Nova endpointing, `UtteranceEnd`, VAD events, and Flux EOT controls; the Rust Deepgram client routes Nova `/v1/listen` vs Flux `/v2/listen`, parses `speech_final`, `SpeechStarted`, `UtteranceEnd`, `TurnInfo`/`EagerEndOfTurn`/`EndOfTurn`/`TurnResumed`, and emits normalized `turn-event` payloads. The UI listens to `turn-event` and shows the latest turn state in the pipeline bar. Local Whisper/diarization fallback emits `local_window` turn events. Remaining work: consume this lifecycle in the first-class S2S orchestrator, add dedicated local VAD instead of fixed-window fallback, add false-start/cancel telemetry, and test S2S cancellation once TTS exists. |

## P2 — Capture UX / rsac Integration

| ID | Status | Item | Notes |
|---|---|---|---|
| AG-P2-001 | Done | Source selector should expose target semantics | Frontend now centralizes capture-target parsing/labels, distinguishes system/device/application/process/process-tree modes, and keeps process vs process-tree selections mutually exclusive before invoking the existing backend target parser. |
| AG-P2-002 | Done | Process-tree source IDs | Backend parses `process-tree:<pid>` and frontend exposes a per-process Tree mode. |
| AG-P2-003 | Done | Source empty-state remediation | Empty source states now include OS-specific permission/PipeWire/app-audio hints and use the existing retry styling. |
| AG-P2-004 | Done | Mid-session source changes | Selector rows now explicitly explain that capture must stop before changing sources; mutable mid-session source changes remain behind a future active-source tracking contract rather than risking orphaned backend captures. |

## P3 — Observability / Quality

| ID | Status | Item | Notes |
|---|---|---|---|
| AG-P3-001 | Done | Coverage reporter / thresholds | Added a dedicated Vitest V8 coverage script with HTML/text/json-summary reporters and modest global thresholds. |
| AG-P3-002 | Done | Full speech orchestration integration test | Added a deterministic `run_speech_processor` integration path that drives missing local Whisper through the real diarization-only fallback without downloaded models or cloud credentials. |
| AG-P3-003 | Done | Gemini reconnect scenario test | Added a backend async test that drives the real session task through disconnect, reconnect backoff, and user cancellation before any real Gemini endpoint is contacted. |
| AG-P3-004 | Done | Structured errors across all commands | Fallible registered Tauri commands now return `AppResult`, legacy helper strings are wrapped as `{ code: "unknown" }`, and user-visible frontend catches route through `errorToMessage`. Deeper taxonomy/i18n mapping remains future refinement. |
| AG-P3-005 | Done | WCAG/contrast audit | Static palette/control audit completed in `docs/reviews/wcag-contrast-audit.md`; muted text, filled accent controls, and toast variants now meet AA contrast for audited normal-text pairs. |
| AG-P3-006 | Open | General LLM/chat token accounting parity | Gemini token usage is surfaced in the UI, but OpenAI-compatible, Bedrock, llama.cpp, and mistral.rs chat/extraction paths still need normalized usage events where providers return counts and explicit "not available" handling where they do not. |
| AG-P3-007 | Open | Cross-platform live capture smoke tests | Unit/integration tests cover parser, settings, store, and orchestration paths, but verified live capture smoke tests still require Windows and Linux machines with real audio devices/app streams. |

## P4 — Persistence / Configuration / Release

| ID | Status | Item | Notes |
|---|---|---|---|
| AG-P4-001 | Done | Load `src-tauri/config/default.toml` | Bundled TOML is parsed into typed defaults; audio sample-rate/channels and the ASR model filename now seed `AppSettings`. Remaining TOML sections are parsed but stay unwired until their runtime owners are ready. |
| AG-P4-002 | Done | Reconcile user data roots | Added a shared non-secret user-data resolver for session artifacts, usage, crashes, and the sessions index while intentionally keeping credentials in `~/.config/audio-graph` and settings/models in Tauri app data. |
| AG-P4-003 | Done | Rebuild sessions index from orphaned files | Backend can scan transcript/graph/usage artifacts, reconstruct missing metadata, preserve metadata paths during load/delete, and expose recovery through the Sessions UI. |
| AG-P4-004 | Blocked | Apple notarization/signing | Requires Developer ID and secrets. |
| AG-P4-005 | Blocked | Windows Authenticode signing | Requires certificate and secrets. |
| AG-P4-006 | Blocked | README screenshots/GIFs | Requires truthful screenshots/GIFs captured from a running desktop app; this environment cannot launch Tauri because the Linux GLib/GObject pkg-config packages are missing. |
| AG-P4-007 | Done | Docs drift cleanup | Refreshed stale README, architecture, contributing, provider, settings, model-management, session-management, Gemini reconnect, and product-mode language while preserving the existing design detail. |
| AG-P4-008 | Blocked | Encrypted credential storage | Requires OS keychain decision and migration plan. |
| AG-P4-009 | Done | LFM2 model filename and URL drift | Rust model catalog, settings defaults, config TOML, shell/PowerShell download helpers, and frontend model defaults now use the canonical LiquidAI Q4_K_M `lfm2-350m-extract-q4_k_m.gguf` filename. Scripts keep the filename in variables instead of repeating the literal at every URL/file reference. |

## External Research Notes

- Deepgram realtime STT remains backend-direct for `rsac` audio. Nova Listen
  uses `/v1/listen`; Flux turn-taking uses `/v2/listen`. Near-term Deepgram
  work now surfaces endpointing, `speech_final`, `SpeechStarted`,
  `UtteranceEnd`, and Flux turn events as a shared turn lifecycle for both
  graph/notes and future voice-agent use cases.
- AssemblyAI Universal Streaming supports one-use temporary tokens and optional
  streaming speaker labels. Backend-direct remains preferred for `rsac` audio;
  browser-origin token use is a future special mode, not the default pipeline.
- AWS Transcribe streaming requires SigV4-style authentication and should stay
  backend-first for credential handling and SDK integration.
- OpenAI Realtime should stay backend-first for the default `rsac` pipeline.
  Use `gpt-realtime-whisper` for transcription-only streaming and
  `gpt-realtime-2` for speech-to-speech voice-agent workflows; browser WebRTC
  is a future browser-origin-audio mode, not the default desktop pipeline.
  The implementation must explicitly handle OpenAI audio input format,
  sample-rate conversion, Base64 append framing, and item-id correlation for
  partial/final transcript events before emitting AudioGraph events.
- The speech-to-speech agent should support three provider families: Gemini
  Live, OpenAI Realtime `gpt-realtime-2`, and a local/hybrid STT -> vLLM -> TTS
  chain. The HF streaming S2S project should inform turn-state, cancel,
  aggressive TTS flush, and latency instrumentation, but AudioGraph should keep
  orchestration in the Rust backend and route provider credentials through the
  existing credential store.
- HF streaming-speech-to-speech uses bounded turn state, explicit cancel ack,
  aggressive streaming flush, and latency charts; those patterns should be
  mirrored in AudioGraph rather than porting the Python stack wholesale.
