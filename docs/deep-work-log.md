# Deep Work Log

Chronological record of deep-work-loop runs against the audio-graph backlog.

## Run 2026-05-19 16:45 — corrected goal: Deepgram + OpenRouter pipeline

**Started at:** 34e1b1ba4544a074e814969db2c0cd7c815dc92d

**Goal:** Complete the application with a real Deepgram STT → OpenRouter LLM →
Deepgram Aura TTS pipeline (with optional graph/notes branch). Native S2S
agents (Gemini Live, gpt-realtime-2) are sibling parallel agents, not
pipeline stages. Linux + Windows are the priority CI surface; macOS deferred
until L+W are solid.

**Carry-forward research:**
- `docs/research/deepgram-aura-streaming-tts.md` — produced in a prior loop
  attempt. WebFetch/context7 were denied in that run, so URLs need re-verification
  before protocol-sensitive code is shipped.

