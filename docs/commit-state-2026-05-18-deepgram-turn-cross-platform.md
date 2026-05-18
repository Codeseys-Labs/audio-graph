# Commit State: 2026-05-18 Deepgram Turn Lifecycle and Cross-Platform Pass

## Repository State

- Branch: `master`
- Starting HEAD: `63f5b8f docs: add logical pipeline diagram and turn strategy`
- Workspace shape: standalone `audio-graph/` checkout with sibling `../rsac`
  path dependency expected by `src-tauri/Cargo.toml`.
- Scope of this wave: runtime Deepgram/local turn lifecycle wiring,
  Windows/Linux-friendly capture defaults, CI/release rsac staging, Gemini
  startup/gating fixes, model filename alignment, and documentation/backlog
  reconciliation.

## Rationale

The previous documentation wave established two product personalities:

1. **Speech-to-notes / speech-to-temporal-graph** for durable transcript,
   entity extraction, temporal graph updates, and chatbot recall.
2. **Parallel speech-to-speech agent** for realtime voice collaboration that
   listens beside the graph path.

This implementation wave focuses on the shared substrate both modes need:
provider-normalized turn signals, stable capture defaults across Windows and
Linux, and a less brittle Deepgram/local provider surface.

## Changes Captured

- Deepgram ASR settings now expose endpointing, `UtteranceEnd`, VAD events,
  and Flux EOT thresholds/timeouts.
- The Rust Deepgram client now routes Nova models to `/v1/listen` and Flux
  models to `/v2/listen`, adding only the query parameters supported by that
  route.
- Deepgram server messages now normalize `speech_final`, `SpeechStarted`,
  `UtteranceEnd`, Flux `TurnInfo`, `EagerEndOfTurn`, `EndOfTurn`, and
  `TurnResumed` into backend `turn-event` payloads.
- Local Whisper and diarization fallback paths now emit conservative
  `local_window` turn events so local-only operation has the same frontend
  event contract.
- React listens for `turn-event`, stores the latest turn lifecycle history,
  clears it with transcripts, and surfaces the latest turn state in the
  pipeline status bar.
- Capture sample-rate defaults now use 48 kHz and the settings UI only exposes
  rsac-supported capture rates.
- Multi-source start now rolls back already-started sources if a later selected
  source fails.
- Gemini Live startup now marks the runtime active before event workers handle
  the queued connected status, and the React control no longer blocks Gemini
  startup solely because IPC redacts the API key.
- GitHub CI/release jobs now stage `audio-graph/` and sibling `rsac/` paths so
  `src-tauri` resolves `../../rsac` on Linux, macOS, and Windows runners.
- LFM2 model references now align on the LiquidAI Q4_K_M filename across Rust,
  frontend defaults, config TOML, and shell/PowerShell download helpers. The
  script filenames are held in variables instead of repeated string literals.

## Backlog State

Closed in this wave:

- `AG-P0-007` CI/release rsac path staging.
- `AG-P0-008` Gemini frontend credential-redaction gating.
- `AG-P0-009` Gemini startup active-state race.
- `AG-P0-010` rsac-incompatible 16 kHz capture default.
- `AG-P4-009` LFM2 model filename and URL drift.

Advanced in this wave:

- `AG-P1-009` Deepgram/local shared turn detector moved from open to partial.
  The normalized turn event contract and Deepgram/local producer side now
  exist; S2S orchestration, dedicated local VAD, false-start telemetry, and
  TTS cancellation tests remain open.

Still open by scope:

- `AG-P1-007` OpenAI Realtime provider implementation.
- `AG-P1-008` first-class speech-to-speech provider surface.
- `AG-P3-006` generalized chat/extraction token usage events beyond Gemini.
- `AG-P3-007` live Windows/Linux capture smoke tests on real hardware.

Externally or decision blocked:

- Apple notarization/signing needs Developer ID and secrets.
- Windows Authenticode signing needs certificate and secrets.
- README screenshots/GIFs need a runnable desktop capture environment.
- Encrypted credential storage needs an OS keychain decision and migration
  plan.

## Research Notes

Tavily, Exa, DeepWiki, and the named `deep-work-loop` skill were unavailable in
this environment, so the turn implementation was checked against official
Deepgram documentation and the existing codebase instead. The applied routing
matches Deepgram's split between Nova Listen endpointing/utterance events and
Flux turn-taking events. OpenAI Realtime and local/hybrid S2S remain documented
in ADRs but were not implemented in this wave.
