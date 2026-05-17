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

## P1 — Requested Pipeline Architecture

| ID | Status | Item | Notes |
|---|---|---|---|
| AG-P1-001 | In progress | ADR for parallel realtime pipeline | See `docs/adr/0001-parallel-realtime-pipeline.md`; now includes provider-specific cloud routing rules. |
| AG-P1-002 | Done | Pipeline latency event contract + UI display | Backend emits current ASR/diarization/extraction/graph samples and the frontend status bar shows the latest per-stage timings. |
| AG-P1-003 | Open | Parallel diarization/extraction + agent loop design | Needs event bus and action proposal contract. |
| AG-P1-004 | Done | ASR provider contract cleanup | Keep cloud STT in Rust for `rsac` audio. Deepgram, AssemblyAI, Sherpa streaming, and AWS Transcribe now emit normalized interim `asr-partial` events, and streaming finals preserve source attribution. |
| AG-P1-005 | Done | Graph delta frontend consumption | Frontend now subscribes to `graph-delta`, applies node/edge deltas in the store, and full snapshots include edge IDs. |
| AG-P1-006 | In progress | Agent/react loop skeleton | vLLM is documented/configured through the OpenAI-compatible LLM provider, API calls now have finite timeouts, chat/ER LLM work is routed through a priority executor, and the OpenAI-compatible API client is synced from runtime-hydrated settings on load/save plus chat/transcription entrypoints. Remaining work is the explicit action/react loop contract. |

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
| AG-P3-001 | Open | Coverage reporter / thresholds | Gap analysis calls out unknown test coverage. |
| AG-P3-002 | Open | Full speech orchestration integration test | Existing tests are mostly narrow unit/integration baselines. |
| AG-P3-003 | Open | Gemini reconnect scenario test | Manual only today. |
| AG-P3-004 | Open | Structured errors across all commands | Only pilot paths use `AppError`; many commands return strings. |
| AG-P3-005 | Open | WCAG/contrast audit | Requires design/a11y pass. |

## P4 — Persistence / Configuration / Release

| ID | Status | Item | Notes |
|---|---|---|---|
| AG-P4-001 | Open | Load `src-tauri/config/default.toml` | Runtime currently uses hardcoded defaults. |
| AG-P4-002 | Open | Reconcile user data roots | Data is split between `~/.audiograph`, Tauri app data, and `~/.config/audio-graph`. |
| AG-P4-003 | Open | Rebuild sessions index from orphaned files | Documented as recovery path but not implemented. |
| AG-P4-004 | Blocked | Apple notarization/signing | Requires Developer ID and secrets. |
| AG-P4-005 | Blocked | Windows Authenticode signing | Requires certificate and secrets. |
| AG-P4-006 | Open | README screenshots/GIFs | Requires capture assets. |
| AG-P4-007 | Open | Docs drift cleanup | Some design/runbook docs still describe already-landed work as pending. |
| AG-P4-008 | Blocked | Encrypted credential storage | Requires OS keychain decision and migration plan. |

## External Research Notes

- Deepgram realtime STT can be handled by the existing internal Rust
  WebSocket client. It should remain backend-direct for `rsac` audio, with
  KeepAlive text frames during idle periods and normalized partial/final
  transcript events.
- AssemblyAI Universal Streaming supports one-use temporary tokens and optional
  streaming speaker labels. Backend-direct remains preferred for `rsac` audio;
  browser-origin token use is a future special mode, not the default pipeline.
- AWS Transcribe streaming requires SigV4-style authentication and should stay
  backend-first for credential handling and SDK integration.
- HF streaming-speech-to-speech uses bounded turn state, explicit cancel ack,
  aggressive streaming flush, and latency charts; those patterns should be
  mirrored in AudioGraph rather than porting the Python stack wholesale.
