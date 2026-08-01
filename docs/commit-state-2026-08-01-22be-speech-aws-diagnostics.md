# Typed speech and AWS diagnostic commit state

- Date: 2026-08-01
- Seed: `audio-graph-22be`
- Worktree: `/home/codeseys/DevBox/audio-graph/.worktrees/22be-speech-aws-diagnostics`
- Branch: `work/audio-graph-22be-speech-aws-diagnostics`
- Base and intake HEAD: `7bbcb5818754b6d70f58455d00fcfb7efb741d66`

## Scope and custody

The worktree was clean at intake. This slice owns the `StageStatus::Error`
wire shape, shared speech-stage error sinks, AWS Transcribe SDK error
classification, the frontend status-bar rendering boundary, matching locale
keys and focused tests. The generated `RuntimeDiagnostic` contract already
present at the base commit remains the source vocabulary.

WebSocket ASR adapters, readiness commands, realtime-agent events, TTS, LLM,
credential adapters, workflows, dependency manifests, generated files, Seeds,
the main checkout, integration and push are outside this worktree's custody.

## Accepted boundary

- `StageStatus::Error` carries only a `RuntimeDiagnostic`; there is no message
  compatibility field.
- Provider and native prose may be inspected privately only where an adapter
  exposes a structural variant. It cannot be retained in status events, logs,
  returned errors or rendered copy.
- AWS outer SDK failures and generated Transcribe service variants map to the
  closed diagnostic vocabulary. Unrecognized input maps conservatively to the
  fixed internal diagnostic.
- The status bar derives user-visible copy from locale keys selected by the
  closed diagnostic code.

## Verification target

Focused Rust and PipelineStatusBar tests, TypeScript typecheck, scoped Biome,
Rust formatting, strict Clippy, generated-runtime-diagnostic drift and
redaction/hygiene canaries must pass before handoff. Exact commands and results
are recorded in the implementation artifact.
