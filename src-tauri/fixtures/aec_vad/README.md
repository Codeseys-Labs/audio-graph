# AEC/VAD Playback-Reference Fixture Set

Seed: `audio-graph-098b` (evidence the parent VAD/AEC bakeoff seed
`audio-graph-0bdc` is blocked on).

This directory contains tiny, **synthesized**, non-secret, deterministic speech
fixtures for offline VAD/AEC bakeoffs. Each fixture class carries an aligned
**mic/system capture** track AND an **assistant render-reference** track so a
pre-bus AEC stage can subtract playback echo before the canonical 16 kHz
`ProcessedAudioChunk` bus (`src-tauri/src/audio/pipeline.rs`).

## Fixture classes

| class | capture | render reference | overlap |
| --- | --- | --- | --- |
| `echo-only` | assistant echo, no user | assistant tone | none |
| `user-barge-in-over-assistant` | assistant echo + user speech | assistant tone | user over assistant (600–1600 ms) |
| `keyboard-noise` | seeded broadband noise | silent | none |
| `quiet-room` | low noise floor | silent | none |
| `overlapped-speech` | two user voices | silent | user_a/user_b (700–1400 ms) |

## Provenance

The WAV files are **not recorded** — they are synthesized from sine tones and a
seeded `xorshift64*` noise source by
`src-tauri/src/aec_vad/mod.rs::synthesize_fixture`. They carry no secrets and no
third-party dataset. The offline validator
(`src-tauri/src/aec_vad_fixtures.rs`) regenerates any missing WAV from the
synthesizer, so a fresh checkout is self-healing and the bytes are byte-for-byte
reproducible.

## What this harness is NOT

It does **not** pull a real AEC candidate (`sonora`,
`webrtc-audio-processing`) and does **not** wire a runtime. Selecting and wiring
a real candidate — which pulls a heavy native dependency under its own guardrail
— is owned by seed `audio-graph-0bdc`. The metrics in the manifest stay
`pending_real_candidate` until that runtime decision lands. The AEC stage must
never mutate already-emitted ASR `ProcessedAudioChunk`s to simulate echo
cancellation; it is an upstream producer of clean audio.
