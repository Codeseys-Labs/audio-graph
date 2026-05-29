# AudioGraph — Documentation Index

Entry point for all AudioGraph documentation. See the main [`README`](../README.md) for setup and quick start.

## Architecture and design

- [`ARCHITECTURE.md`](ARCHITECTURE.md) — current fan-out realtime pipeline, product-mode split, provider abstraction, event flow, data roots, and module map.
- [`DATA_FLOW.md`](DATA_FLOW.md) — code-grounded thread/channel inventory and the exact sequential-vs-parallel boundaries across the capture spine and the three processing tracks (speech-to-graph, Gemini Live, TTS/playback).
- [`adr/0001-parallel-realtime-pipeline.md`](adr/0001-parallel-realtime-pipeline.md) — accepted ADR for the split realtime topology and provider routing rules.
- [`adr/0002-openai-realtime-provider.md`](adr/0002-openai-realtime-provider.md) — proposed ADR for `gpt-realtime-whisper` STT and `gpt-realtime-2` speech-to-speech support.
- [`adr/0003-speech-to-speech-agent-provider-matrix.md`](adr/0003-speech-to-speech-agent-provider-matrix.md) — proposed ADR for Gemini, OpenAI Realtime, and local/hybrid vLLM speech-to-speech agents.
- [`designs/provider-architecture.md`](designs/provider-architecture.md) — implemented provider-abstraction design across product modes and pipeline stages.
- [`designs/provider-refactor.md`](designs/provider-refactor.md) — refactor plan toward that target.
- [`designs/session-management.md`](designs/session-management.md) — session lifecycle, persistence, recovery.
- [`MODEL_MANAGEMENT_DESIGN.md`](MODEL_MANAGEMENT_DESIGN.md) — model download, caching, and in-app management.
- [`SETTINGS_DESIGN.md`](SETTINGS_DESIGN.md) — Settings page architecture and credential storage.
- [`SYSTEM_TRAY_WIDGET_PROPOSAL.md`](SYSTEM_TRAY_WIDGET_PROPOSAL.md) — proposed system tray widget.
- [`GEMINI_LANGUAGES.md`](GEMINI_LANGUAGES.md) — Gemini Live language coverage.

## Operations

- [`ops/gemini-reconnect-runbook.md`](ops/gemini-reconnect-runbook.md) — Gemini Live reconnect / recovery runbook.
- [`ops/vllm-backend.md`](ops/vllm-backend.md) — vLLM OpenAI-compatible backend setup for agent and extraction work.

## Reviews and retrospectives

- [`reviews/`](reviews/) — loop-by-loop code review notes
  (`audio-graph-review-loop10.md` through `audio-graph-review-loop23.md`)
  plus the wave-based follow-ups (`audio-graph-wave-a-review.md`,
  `audio-graph-wave-b-review.md`).
- [`reviews/gap-analysis.md`](reviews/gap-analysis.md) — outstanding gaps across the product.
- [`reviews/ux-first-run-review.md`](reviews/ux-first-run-review.md) — first-run UX audit.
- [`reviews/2026-05-29-deep-critique/synthesis.md`](reviews/2026-05-29-deep-critique/synthesis.md) — 6-facet parallel adversarial critique (concurrency, architecture, security, frontend, performance, executive) with a prioritized improvement plan.

## Documentation audits

- [`audit/docs-queue.md`](audit/docs-queue.md) — recursive doc-gap queue
  driving the latest sweep. Records what was queued, in progress, and
  done.

## Release and contributing

- [`RELEASE.md`](RELEASE.md) — release and versioning process.
- [`CONTRIBUTING.md`](CONTRIBUTING.md) — branch workflow, commit conventions, pre-submit checklist.
