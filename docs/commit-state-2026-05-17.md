# Commit State Snapshot — 2026-05-17

## Baseline

- Branch: `master`
- HEAD: `ebd78f23a2b002e2b15bb22fea0fcdc9494c17fc`
- HEAD subject: `docs: recursive audit — component JSDoc, module headers, rustdoc hygiene`
- Capture time: 2026-05-17, local workspace `/mnt/e/CS/github/audio-graph`

## Working Tree

`git status --short` reports tracked modifications across nearly every file.
`git diff --ignore-space-at-eol --stat` reports no changes.

Interpretation: the dirty tree is line-ending churn, not semantic code drift.
Do not normalize or revert this churn as part of pipeline work unless that is
made an explicit cleanup task.

## Current Product State

AudioGraph is a Tauri v2 + React desktop app with a Rust backend that captures
system, device, application, and process audio through `rsac`, resamples audio
to 16 kHz mono, streams it into ASR/Gemini pipelines, emits transcripts and
speaker updates, extracts entities/relations, mutates a temporal knowledge
graph, and persists transcripts/graphs/sessions/usage to disk.

The codebase already has substantial hardening around:

- Provider settings and first-run setup.
- Local and cloud ASR providers.
- Gemini Live reconnect/error classification.
- Session rotation and transcript writer shutdown.
- Model download management.
- Storage-full UX.
- Frontend hook and settings tests.
- Documentation/rustdoc hygiene.

## Constraints

- The requested `deep-work-loop` skill is not installed in this Codex session.
- Tavily, Exa, and DeepWiki tools are not exposed in this session. Research is
  performed with available local repository inspection and standard web access.
- Apple/Windows signing, paid cloud provider verification, and any operation
  requiring private credentials cannot be completed locally without external
  secrets or certificates.

