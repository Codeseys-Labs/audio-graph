# ADR-0018: Provider-agnostic converse turn-state machine + backend-side half-duplex/AEC

## Status

Accepted (2026-05-30). Records the orchestration architecture for native and
pipelined converse **before** the B18 implementation, backed by a primary-source
barge-in study (`docs/research/b18-native-s2s-bargein.md`). **Supersedes the
interim frontend echo guard** added in `172edbf`
(`useConverseFrontLeg` `isChatLoading` mute) — that guard remains a documented
*fallback* for the pipelined engine when no AEC is available, but it is no longer
the architecture of record for half-duplex/barge-in. Realizes ADR-0003's
`turn_start / audio-append / turn_end / cancel / barge_in` protocol in concrete
Rust terms and supplies ADR-0013 step 3's "full barge-in turn orchestrator".

## Context

ADR-0013 shipped the converse **pipelined front-leg** (STT-final → graph-grounded
streaming chat → speak-aloud). Concurrent review of that work caught a **P1 echo
loop**: with loopback/system-audio capture the assistant's own TTS is re-captured,
re-transcribed, and fed back as a new "user turn" — a self-sustaining loop. The
interim fix (`172edbf`) was a coarse **half-duplex mute**: while a reply is
streaming (`isChatLoading`), incoming transcripts are ignored. It stops the loop
but also **drops genuine user barge-in** during a reply, and it lives in the React
hook rather than the pipeline that owns the audio.

B18 (native speech-to-speech) makes this load-bearing. Enabling audio-out on
Gemini Live and OpenAI Realtime means the assistant *speaks*, so:

- The same echo/self-interruption hazard now applies to two more engines, each
  with its **own** interruption protocol (Gemini auto-fires `serverContent.
  interrupted`; OpenAI requires the client to stop playback → `response.cancel`
  → `conversation.item.truncate{audio_end_ms}`; the pipelined path clears the TTS
  queue). See research §1.4, §2.4, §3.
- Without a single orchestration model, each engine would re-implement turn
  bookkeeping, barge-in, and graph-grounding routing, diverging immediately.

Research (LiveKit Agents, Pipecat, Deepgram, OpenAI/Gemini primary docs)
converges on two ideas: a **provider-agnostic turn-state machine**
(`Idle→Listening→Thinking→Speaking→Interrupted`) and a **layered echo-mitigation
stack** rooted in acoustic echo cancellation (AEC), not a duplex mute.

## Decision Drivers

- **One orchestration model for all three converse engines** (Gemini Live,
  OpenAI Realtime voice, pipelined STT→LLM→TTS) so barge-in, turn bookkeeping,
  and graph routing are written once.
- **True barge-in** — the user can interrupt the assistant mid-sentence — not
  just loop suppression. The interim guard fails this.
- **Echo loop must not regress** as audio-out lands on two more engines.
- **Backend owns audio**, so the half-duplex/barge-in reaction belongs in the
  Rust orchestrator (next to the capture/playback rings), not in a React hook.
- **Honest availability** (ADR-0013): degrade gracefully when AEC is unavailable
  rather than silently shipping a broken full-duplex experience.
- Keep the per-engine surface thin: each engine maps its raw events to a
  normalized signal the FSM consumes.

## Considered Options

- **Option A — Provider-agnostic turn-state FSM in the backend + layered AEC
  echo mitigation (chosen).** A single `Idle→Listening→Thinking→Speaking→
  Interrupted` state machine in the Rust converse orchestrator. Each engine
  implements a `RealtimeAgent` trait that normalizes its native events into a
  `TurnSignal` enum (`UserSpeechStarted/Ended`, `AssistantAudio`,
  `AssistantTranscript`, `GenerationComplete`, `TurnComplete`, `Interrupted`,
  `Error`) and exposes outbound ops (`append_audio`, `end_user_turn`, `cancel`,
  `set_config`). Echo is fought with a layered stack: mic-with-AEC (or
  reference-aligned AEC) instead of raw loopback while speaking; provider
  server-VAD (Gemini auto / OpenAI `semantic_vad`) for detection; an AEC-warmup
  window + `min_interruption_duration` gate; optional false-interruption resume.
- **Option B — Keep the frontend `isChatLoading` half-duplex mute, generalize it
  to native engines.** Extend the existing coarse guard to also gate Gemini/
  OpenAI audio. Smallest change. But it is *fundamentally half-duplex*: it
  cannot express real barge-in (it drops user speech during replies), it sits in
  the wrong layer (React, not the audio pipeline), and it cannot drive the
  per-engine cancel/truncate sequences OpenAI requires.
- **Option C — Per-engine bespoke turn handling (no shared FSM).** Let each
  engine's module own its own turn/barge-in logic against its native events.
  Least abstraction up front. But three diverging implementations of the same
  hard problem (echo, truncation accounting, graph routing), no shared latency
  milestones, and the pipelined path can't reuse any of it.
- **Option D — Adopt an external voice-agent framework** (LiveKit Agents /
  Pipecat) for the turn orchestration. Most batteries-included. But both are
  Python pipelines; AudioGraph's orchestrator is in-process Rust owning rsac
  capture and the temporal graph — wrapping a Python agent runtime would add a
  process boundary, a second audio path, and a heavy dependency contrary to the
  in-process design (ADR-0001/0012). We borrow their *patterns*, not their code.

## Decision Outcome

Proposed: **Option A**. A single backend FSM with a normalized `TurnSignal`
surface is the only option that serves all three engines without divergence,
expresses genuine barge-in (which B/C cannot), and keeps the half-duplex/AEC
reaction in the layer that owns the audio. The interim frontend guard is demoted
to a documented pipelined-engine fallback for the no-AEC case. We adopt the
production patterns from LiveKit/Pipecat/Deepgram (AEC-first, server-VAD,
warmup window, minimum-interruption gate, false-interruption resume) but
implement them in Rust rather than importing a framework (rules out D).

This is **not yet implemented** — it is the architecture B18 will build against.
The ADR records the FSM, the trait surface, and the per-engine event maps so the
implementation is a focused effort (see `docs/research/b18-native-s2s-bargein.md`
§4 for the verbatim state table and event mapping).

### Consequences

- **Positive:** One turn orchestrator for Gemini Live, OpenAI Realtime voice, and
  the pipelined path; barge-in, truncation accounting, and graph routing written
  once. New engines implement one trait.
- **Positive:** True barge-in replaces loop-only suppression; the user can
  interrupt mid-reply. Latency milestones (turn_start, end-of-speech, first
  assistant audio, final audio, cancel-ack) are emitted from one place.
- **Positive:** The echo loop is fought at the source (AEC) with content-aware
  server VAD, which is robust to the assistant's own residual audio in a way the
  energy/duplex approach is not.
- **Negative (required):** Real AEC is **hard and platform-dependent**. Without a
  time-aligned reference-cancelling AEC, server VADs still treat echoed TTS as
  speech — so the highest-leverage piece (capture mic-with-AEC vs raw loopback,
  or a reference-aligned APM) is also the most fragile across OS/hardware, and
  ships behind capability detection with the duplex-mute fallback.
- **Negative:** Net-new backend complexity — an FSM, a per-turn cancellation
  token, per-engine event maps, and an `audio_end_ms` played-samples accountant
  (OpenAI errors if `audio_end_ms` exceeds the real duration). More moving parts
  than the one-line mute it replaces.
- **Negative:** Two interruption protocols to keep correct (Gemini server-auto
  vs OpenAI client-driven cancel+truncate); a provider protocol change breaks
  one engine's barge-in without touching the other.
- **Neutral:** `responseModalities`/`speechConfig` (Gemini) and
  `output_modalities`/`audio.output` (OpenAI) become configurable so notes-mode
  keeps TEXT and converse-mode uses AUDIO + output transcription.
- **Neutral:** Both engines emit PCM16 LE mono **24 kHz** audio, so one playback
  path serves both.

## Implementation outline (informational, non-binding)

1. **Event-model deltas (no FSM yet):** add `GeminiEvent::AudioChunk{data,
   sample_rate=24000}`, `Interrupted`, `OutputTranscription{text}`,
   (optional) `GenerationComplete`; decode `serverContent.modelTurn.parts[].
   inlineData` and branch on `serverContent.interrupted` / `outputTranscription`
   / `generationComplete`. Make `responseModalities` + `speechConfig`
   configurable in `GeminiConfig`. (Mirror on the OpenAI voice client when B15's
   transport lands.)
2. **Turn FSM + trait:** a `RealtimeAgent` trait (`append_audio`,
   `end_user_turn`, `cancel`, `set_config`) and a normalized `TurnSignal` enum;
   a backend orchestrator driving `Idle→Listening→Thinking→Speaking→Interrupted`
   with a per-turn cancellation token tripped on `Interrupted`/`cancel`.
3. **Barge-in reaction (per engine):** Gemini = drop local playback buffer on
   `interrupted` (server auto-cancels); OpenAI = stop playback → `response.cancel`
   → `conversation.item.truncate{audio_end_ms = ms_played}`; pipelined = clear
   the TTS queue + output buffer.
4. **Echo stack:** capture mic-with-AEC (or reference-aligned AEC) in converse
   mode; drive detection from provider server VAD; add an AEC-warmup window +
   `min_interruption_duration` gate; optional false-interruption resume. Keep the
   `isChatLoading` mute as the pipelined no-AEC fallback.
5. **Played-samples accountant:** track ms actually fed to the sink from the
   playback ring buffer to supply OpenAI's `audio_end_ms` conservatively.

## Rollback

Each layer is independently reversible behind converse-engine selection. If the
FSM destabilizes converse, fall back to the pipelined front-leg + the
`isChatLoading` mute (the pre-0018 behavior) by not enabling native audio-out;
remove only the orchestrator + native-audio event branches. The notes/graph
pipeline, the pipelined front-leg, and all non-converse paths are untouched by
this ADR.

## References

- Research: `docs/research/b18-native-s2s-bargein.md` (FSM §4, echo stack §3,
  per-engine barge-in §1.4/§2.4), `docs/research/openai-realtime-2026-05.md`,
  `docs/research/b15-openai-realtime-rust-impl.md`.
- Supersedes the interim guard in commit `172edbf`
  (`src/hooks/useConverseFrontLeg.ts`).
- Related ADRs: ADR-0003 (S2S provider matrix + turn protocol), ADR-0006
  (streaming chat vs native-S2S separation), ADR-0013 (conversation modes —
  this is its step-3 orchestrator), ADR-0002 (OpenAI Realtime provider family),
  ADR-0004 (TtsProvider/Aura), ADR-0001 (parallel realtime pipeline).
- External patterns (borrowed, not imported): LiveKit Agents `AgentSession`
  states + `InterruptionOptions` (aec_warmup, min_duration,
  resume_false_interruption); Pipecat `InterruptionFrame` / VAD frames; Deepgram
  barge-in/AEC deep-dive.
