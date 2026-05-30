# ADR-0013: Conversation modes — Notes/Graph vs Converse (native + pipelined S2S)

## Status

Accepted 2026-05-29.

## Context

The app can both **build** a knowledgebase from speech (transcribe → temporal
graph) and, increasingly, let a user **talk to** that knowledgebase. Today the
only user-facing control for the second capability is a hidden boolean
(`nativeS2sEnabled`) buried in Settings → Gemini that merely reveals a "Gemini"
button in the top bar (`ControlBar.tsx`). This is undiscoverable (W3.2 in
`docs/reviews/2026-05-29-uiux-deep-dive.md`) and conflates two different ideas:
*which engine* vs *what the user is trying to do*.

A canvas of the code (2026-05-29) established what exists:

- **Pipelined S2S is ~70% built.** STT → graph and LLM(graph-grounded,
  streaming) → TTS(Deepgram Aura) → playback (with barge-in) both work
  (`speak_aloud.rs`, `start_streaming_chat`). The missing leg is **user speech
  → STT-final → chat input**, plus a turn/endpointing loop.
- **Native S2S (Gemini Live) is audio-in → TEXT only.** `responseModalities`
  is hardcoded to `["TEXT"]` (`gemini/mod.rs:621`); there is no audio-out decode,
  and Gemini text feeds graph extraction, not a grounded reply. OpenAI Realtime
  is not implemented.

ADR-0006 already separates the *composed pipeline* (STT→LLM→TTS) from
*native-S2S agents* as sibling surfaces. This ADR decides how the **user**
chooses between them.

## Decision Drivers

- Make the capability discoverable and intent-first ("what do you want to do?"),
  not engine-first or hidden behind a flag.
- Honestly represent availability: don't offer controls that silently no-op.
- Reuse the working pieces (graph-grounded streaming chat + speak-aloud) rather
  than waiting on native audio-out.
- Leave a clean seat for OpenAI Realtime and local/hybrid TTS later.

## Considered Options

- **Option A — A `conversationMode` selector with an engine sub-choice.**
  Top-level mode: `notes` (transcribe → build graph, the default) vs `converse`
  (talk to the knowledgebase). When `converse`, an engine sub-choice:
  `pipelined` (selected STT → graph-grounded LLM → TTS) or `native`
  (Gemini Live; OpenAI Realtime later). Availability is computed from settings
  (keys present, provider support) and shown truthfully (enabled / "needs key" /
  "coming soon"). Replaces `nativeS2sEnabled`.
- **Option B — Keep the boolean**, just surface it better (a visible toggle).
  Smaller change, but still engine-first and can't express the pipelined route.
- **Option C — Auto-pick the engine** based on configured providers, no user
  choice. Less control; surprising when multiple engines are configured.

## Decision Outcome

Chosen: **Option A**. It matches the user's mental model (notes vs converse),
exposes both the pipelined and native engines, and is honest about what each can
do today. It replaces the hidden flag and makes the previously-buried Gemini
path discoverable.

Rollout is staged to match backend readiness:

1. **Now:** the mode selector UI + store state; `notes` default; `converse`
   exposes `pipelined` and `native` with availability badges. Native maps to the
   existing Gemini start path. Selecting an engine that needs a key routes the
   user to Settings.
2. **Next (pipelined front-leg):** wire STT-final → `start_streaming_chat`
   (which already grounds in the graph and drives speak-aloud) behind a
   push-to-talk / endpointed turn, reusing the working LLM→TTS leg.
3. **Later:** native audio-out + graph grounding for Gemini (`responseModalities`
   AUDIO + decode/play + context injection); OpenAI Realtime; full barge-in turn
   orchestrator (ADR-0003 turn protocol); local TTS.

### Consequences

- **Positive:** Discoverable, intent-first control; both engines representable;
  honest availability; hidden flag retired.
- **Positive:** Fastest functional converse path (pipelined) reuses built code.
- **Negative:** UI exposes a `native` engine whose spoken/grounded mode is still
  partial — mitigated by explicit availability/"transcription-only today"
  labeling so we don't over-promise.
- **Neutral:** `conversationMode` supersedes `nativeS2sEnabled` in the store;
  the boolean is migrated, not multiplied.

## Implementation (intended)

- Store: `conversationMode: "notes" | "converse"`, `converseEngine:
  "pipelined" | "native"`, derived `converseAvailability`; migrate
  `nativeS2sEnabled` (true ⇒ converse/native).
- UI: a mode control in the ControlBar (and/or a small popover) replacing the
  hidden Gemini gating; availability from settings (Gemini key, STT+TTS+LLM for
  pipelined). Settings → Gemini "Conversation mode" checkbox is replaced by this.
- Backend (staged): STT-final → chat trigger; later Gemini audio-out + OpenAI
  Realtime.

## References

- `docs/reviews/2026-05-29-uiux-deep-dive.md` (W3.2)
- ADR-0006 (streaming chat vs native-S2S separation), ADR-0003 (S2S provider
  matrix + turn protocol), ADR-0002 (OpenAI Realtime), ADR-0004 (TTS/Aura).
