# Commit State: 2026-05-18 Documentation and ADR Refresh

## Repository State

- Branch: `master`
- Starting HEAD: `aa1811a docs(architecture): update ARCHITECTURE.md with executor/agent nodes and add backup`
- Workspace shape: standalone `audio-graph/` checkout with sibling `../rsac`
  path dependency.
- Scope of this wave: documentation, backlog, and ADR refresh only. No Rust or
  React runtime code was changed in this wave.

## Rationale

The project has evolved from a single speech pipeline into two related product
personalities:

1. **Speech-to-notes / speech-to-temporal-graph** — durable transcript,
   extracted entities/relations, temporal graph, and chatbot recall.
2. **Parallel speech-to-speech agent** — realtime voice-agent behavior that
   listens beside the graph pipeline and routes actions through backend-owned
   proposals.

The docs now make this split explicit and map local vs cloud options for each
phase of each product mode.

## Changes Captured

- `README.md` now describes the two product modes and fixes stale Gemini
  reconnect event names.
- `docs/ARCHITECTURE.md` now includes product-personality tables, updated
  Mermaid topology, standalone repo placement, OpenAI Realtime notes, and
  explicit local/cloud phase options.
- `docs/designs/provider-architecture.md` now separates provider choices by
  product mode and marks OpenAI Realtime/local S2S as planned rather than
  implemented.
- `docs/adr/0001-parallel-realtime-pipeline.md` now names the durable graph
  mode and parallel speech-to-speech mode as separate product targets.
- `docs/adr/0002-openai-realtime-provider.md` now captures the OpenAI Realtime
  decision, including STT vs voice-agent split, audio-format handling,
  Base64 append framing, provider item-id transcript correlation, session
  settings, and diarization fallback.
- `docs/adr/0003-speech-to-speech-agent-provider-matrix.md` captures the
  speech-to-speech agent design for Gemini Live, OpenAI Realtime
  `gpt-realtime-2`, and local/hybrid STT -> vLLM -> TTS pipelines.
- `docs/backlog/pipeline-modernization.md` now records OpenAI Realtime
  and speech-to-speech provider-matrix implementation details still required
  after the ADRs.
- `docs/ops/gemini-reconnect-runbook.md` and `docs/reviews/gap-analysis.md`
  were corrected for the implemented `gemini-status` reconnect surface.
- `docs/audit/docs-queue.md` records the latest product-mode and realtime
  provider documentation wave.

## Backlog State

Closed or refreshed in this wave:

- Product personality documentation: completed.
- Local vs cloud phase matrix for both product modes: completed.
- OpenAI Realtime ADR documentation: completed.
- Speech-to-speech provider-matrix ADR documentation: completed.
- Gemini reconnect stale documentation: completed.

Still open:

- `AG-P1-007` OpenAI Realtime implementation remains open. Recommended first
  implementation wave is STT-only: backend-owned Rust WebSocket client,
  `gpt-realtime-whisper` settings, audio-format conversion, parser fixtures,
  partial/final transcript mapping, source attribution, latency telemetry, and
  redaction/hydration tests.
- `AG-P1-008` speech-to-speech agent provider matrix implementation remains
  open. Recommended first implementation wave is the provider/settings/event
  surface plus bounded turn-state orchestration, keeping Gemini as the
  implemented cloud-native path and adding local/hybrid vLLM in a later
  runtime wave.

Externally or product-decision blocked:

- Apple notarization/signing requires Developer ID and secrets.
- Windows Authenticode signing requires certificate and secrets.
- README screenshots/GIFs require a runnable desktop capture environment.
- Encrypted credential storage requires an OS keychain decision and migration
  plan.

## Research Notes

Tavily, Exa, DeepWiki, and the named `deep-work-loop` skill were unavailable in
this environment. The OpenAI Realtime ADR was cross-checked against official
OpenAI documentation instead:

- Realtime overview
- Realtime WebSocket guide
- Realtime transcription guide
- `gpt-realtime-2` model page
- `gpt-realtime-whisper` model page
- Gemini Live API capabilities documentation
- Deepgram streaming TTS documentation
- vLLM OpenAI-compatible server documentation

The resulting implementation guidance is backend-first for the default `rsac`
desktop audio path. Browser WebRTC remains a future browser-origin mode.
