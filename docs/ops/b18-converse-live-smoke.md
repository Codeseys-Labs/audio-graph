# B18 native-S2S — live runtime smoke checklist (#46)

**Status: ready to run (needs hardware).** Everything code-side is implemented,
unit-tested, and CI-green; this checklist is the one remaining step — a human
running the app on a real machine (audio output + mic + a live `gemini_api_key`)
to confirm end-to-end behavior that cannot be unit-tested ("audio comes out of
the speaker, and talking over it cuts the reply").

## Prerequisites

- A real audio **output device** (speakers/headphones) and a **microphone**.
- A Gemini credential configured through Express Setup or the local credential
  backend. Do not record whether a specific workstation has one.
- A build to run. Either:
  - **Release:** `cargo tauri build` → run the produced
    `AudioGraph_*_x64-setup.exe` / app bundle, or
  - **Dev:** `bun run tauri dev` (hot-reload; easiest for iterating on findings).

## Procedure

1. **Launch** the app. Confirm no error banner on start.
2. **Select converse mode + native engine.** In the UI: set conversation mode to
   **Converse** and converse engine to **Native (S2S)** (Settings → "native
   speech-to-speech", or the ConversationModeControl in the ControlBar). This is
   what makes the store invoke `start_converse` instead of `start_gemini` (the
   #46 FE wiring).
3. **Start capture** (pick a mic source → Start). Confirm the capture status dot
   goes green.
4. **Start the converse session.** Confirm:
   - No `gemini_api_key` / auth error toast.
   - The backend log shows `converse session started (Gemini AUDIO)` and
     `converse driver: starting`.
5. **Speak a short turn** ("Hello, can you hear me?"). Confirm — **the core
   acceptance**:
   - An **audible spoken reply** plays from the output device (validates
     `GeminiConfig::audio` → server emits `AudioChunk` → `PlayAudio` byte→i16
     decode → `AudioPlayer::push_samples` on the 24 kHz stream).
   - The assistant transcript appears in the live panel (the `EmitTranscript`
     path / `gemini-response` event).
6. **Barge-in:** while the assistant is still speaking, **start talking over
   it.** Confirm:
   - The assistant audio **cuts off promptly** (validates the engine
     `interrupted` event → `Interrupted` signal → `StopPlayback` →
     `audio_player.cancel()`).
   - After you stop, the next turn proceeds normally (FSM returns to
     `Listening`, re-`StartCapture`).
7. **Stop the converse session.** Confirm:
   - No lingering/looping audio.
   - The backend log shows `converse session stopped` and `converse driver:
     exiting` (threads joined, not detached-on-timeout).
8. **Restart** the converse session once (start → stop → start) to shake out the
   double-start / thread-handle reuse path (see the known-risk note below).

## What to watch for (known audited risks)

These were flagged by the concurrent audit (tasks #48/#49) and are the most
likely places a live run surfaces a problem:

- **#48 (P1):** converse shares the `gemini_audio_thread` slot with notes mode.
  If a stop→start cycle is fast, or if you switch between notes and converse
  without a clean stop, watch for: capture producing no audio (sender thread not
  spawned), or audio "stealing" between modes. If you see this, it confirms #48.
- **#49 (P3):** if the session dies on a bad/expired key but the UI stays
  "active" forever (driver thread never exits), that confirms #49 (driver loop
  doesn't break on fatal auth without a `Disconnected`).
- Barge-in not cutting: on the Gemini path the `InterruptionGate` is **disabled**
  by design (server-VAD, no client AEC), so barge-in rides the engine's own
  `interrupted` event. If the engine doesn't emit it, barge-in won't fire — that
  is a Gemini-config/server-VAD question, not an FSM bug.

## Recording results

Capture the backend log (`%APPDATA%\audio-graph\logs`) for the session and note
pass/fail per step. File any runtime finding as a new backlog task (link this
doc). Once steps 5 + 6 pass, #46 is **done** and native S2S is shippable.
