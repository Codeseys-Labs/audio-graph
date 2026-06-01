# B18 — Native S2S runtime driver wiring plan (FA-8)

**Status: WIRED 2026-06-01 — pending live smoke.** All six build steps below are
implemented and verified (clippy cloud + default `--all-targets -D warnings`
clean; WSL cloud tests 484/0; 12 converse unit tests). The pure FSM now has a
production driver: `ConverseDriver` + `ConverseSink` (converse/mod.rs),
`GeminiLiveClient::end_user_turn()` (gemini/mod.rs), and the
`start_converse`/`stop_converse` commands + `GeminiConverseSink` (commands.rs,
registered in lib.rs). The OpenAI voice seam (`openai_event_to_signal`) is stubbed
to the current STT enum. The ONE remaining item is a **live runtime smoke** on
real hardware (audio device + mic + the present `gemini_api_key`): a real spoken
turn must produce an audible reply and an engine-`interrupted` barge-in must cut
it. No new ADR — this is the implementation of accepted ADR-0018.

Step status: 1 ✅ `GeminiConfig::audio` in start_converse · 2 ✅ `end_user_turn()`
· 3 ✅ converse-event driver loop · 4 ✅ `PlayAudio` byte→i16 · 5 ✅ capture gate
(`converse_capture_gate`) · 6 ✅ `SignalContext` clock (`ConverseDriver` tracks
Speaking-entry; gate disabled on the Gemini server-VAD path, so barge-in rides the
engine's `interrupted`). Below is the original plan, retained for the record.

---

## Problem (verified against current code)

Today `start_gemini` (commands.rs:2220) is a **notes-mode** path:

- It builds `GeminiConfig::text(...)` (commands.rs:2283) — TEXT modality only.
  `GeminiConfig::audio(auth, model, voice)` (gemini/mod.rs:338) has **zero
  non-test callers**, so the server never emits `AudioChunk` and the FSM's
  `Thinking → Speaking` edge can never fire from live data.
- The event-receiver thread (commands.rs:2384+) routes `Transcription` to the
  graph and **logs-and-ignores** `AudioChunk` / `OutputTranscription` /
  `Interrupted` / `GenerationComplete` (commands.rs:2552+) because nothing
  consumes them — there is no `TurnMachine` in the loop.
- The audio-sender thread (commands.rs:2320) streams captured audio to Gemini
  **unconditionally** while `is_gemini_active`, so a barge-in cannot stop the
  mic and `CancelToken` has no token to trip.

Net: native speech-to-speech is **non-functional end-to-end** despite the FSM
core being done. The pieces are all present but unconnected.

## Decision drivers

- Reuse the proven pure FSM verbatim — the driver is the *only* new logic, and
  it must stay thin (I/O + clock + dispatch), so the FSM's test coverage keeps
  its value.
- Do not regress the notes-mode `start_gemini` path (graph/notes projection is a
  separate, shipping feature). Converse mode is **additive**: a new command +
  config, selected by the caller, not a rewrite of notes mode.
- Half-duplex fallback must work with **no AEC** (ADR-0018): when the gate is
  disabled, only an engine-confirmed `Interrupted` breaks a reply. The clock/VAD
  wiring (step 6) only *upgrades* barge-in to full-duplex; its absence degrades
  gracefully, it does not block the feature.

## Build sequence (6 steps, each independently testable)

Ordered so each step compiles + tests green on its own; later steps light up
capability the earlier ones scaffold. Estimated as one focused wave (the FSM and
event map — the hard part — are done).

### Step 1 — `GeminiConfig::audio` in a converse-start path *(P1, gemini/mod.rs caller in commands.rs)*

Add a `start_converse` command (or a `mode: ConverseMode` arg on a unified start
path) that builds `GeminiConfig::audio(auth, model, voice)` with the
user-configured voice instead of `::text(...)`. Store the client the same way
notes mode does (`state.gemini_client`). This is the precondition for the server
to emit `AudioChunk` at all.

- **Files:** commands.rs (new command), lib.rs (register in
  `tauri::generate_handler!`).
- **Test:** a unit test asserting the converse path selects `GeminiConfig::audio`
  with the configured voice (config-construction level, no socket).

### Step 2 — `GeminiLiveClient::end_user_turn()` for `TurnAction::EndUserTurn` *(P1, gemini/mod.rs)*

`audioStreamEnd` is currently sent **only** inside `disconnect()`
(gemini/mod.rs:645) — i.e. full teardown. Add
`GeminiLiveClient::end_user_turn()` that sends
`{realtimeInput:{audioStreamEnd:true}}` **without** closing the socket, so the
`Listening → Thinking` edge has an engine binding. (With Gemini server-VAD this
may be implicit, but the action must map to *something* or it is a silent no-op.)

- **Files:** gemini/mod.rs (new method + a unit test on the serialized frame).
- **Test:** assert the emitted JSON frame shape; assert the socket stays open.

### Step 3 — the converse-event worker loop (the driver) *(P1, commands.rs + lib.rs)*

A dedicated thread (sibling of `gemini-event-receiver`) that:

1. Holds a `TurnMachine::new(gate)` where `gate` comes from settings (enabled +
   `aec_warmup_ms` + `min_interruption_duration_ms`).
2. For each `GeminiEvent`, calls `gemini_event_to_signal(ev)`; `None` → handle
   at the transport layer exactly as notes mode does (Connected/Disconnected/
   Reconnecting/Reconnected), `Some(sig)` → `machine.on_signal_ctx(sig, ctx)`.
3. Dispatches each returned `TurnAction` (step 4/5 bindings).

Threads the `SignalContext` (step 6). Registers in lib.rs.

- **Files:** commands.rs (the loop), lib.rs (start/stop wiring).
- **Test:** an integration-style test feeding a scripted `GeminiEvent` sequence
  through `gemini_event_to_signal` + `on_signal_ctx` and asserting the dispatched
  action order (this largely reuses the FSM tests; the new coverage is the
  event→signal→dispatch glue).

### Step 4 — `PlayAudio` / `StopPlayback` binding *(P2, commands.rs ↔ playback/mod.rs)*

`TurnAction::PlayAudio { pcm24: Vec<u8> }` carries **PCM16-LE bytes**;
`AudioPlayer::push_samples(&[i16])` (playback/mod.rs:261) wants i16 samples.
The dispatcher decodes: `pcm24.chunks_exact(2).map(|b| i16::from_le_bytes([b[0],
b[1]])).collect()` then `push_samples`. Ensure a 24 kHz playback stream is opened
by the converse-start path first. `StopPlayback` → `audio_player.cancel()`.

- **Files:** commands.rs (dispatcher), reuse playback/mod.rs as-is.
- **Test:** a byte→i16 decode unit test (odd-length truncation, endianness).

### Step 5 — capture gating + per-turn `CancelToken` *(P2, commands.rs)*

Gate the `gemini-audio-sender` thread on a per-turn `AtomicBool` toggled by
`StartCapture`/`StopCapture` (today it streams unconditionally while
`is_gemini_active`). Wire `TurnAction::CancelToken` to a
`tokio_util::CancellationToken` (per ADR-0003) created per turn, so in-flight
async work for the turn aborts at its next await. This is what makes barge-in
actually **stop the mic** instead of only flushing playback.

- **Files:** commands.rs (the AtomicBool + token), AppState (hold the token).
- **Test:** assert the sender skips sends when the gate bool is false.

### Step 6 — `SignalContext` clock + VAD source *(P3, commands.rs)*

The FSM is clock-free by design; nothing currently populates
`SignalContext { ms_since_speaking_started, user_speech_ms }`, so both are 0 and
the gate suppresses **every** audio-activity barge-in as `AecWarmup`. The driver
must record an `Instant` on `Speaking`-entry and track VAD-speech duration to
populate the context on each `UserSpeechStarted` while `Speaking`. **Without this
step, only engine-`Interrupted` barge-in works** — which is the documented
half-duplex fallback, so step 6 is a correctness *upgrade*, not a blocker.

- **Files:** commands.rs (the clock + VAD-duration tracking in the driver).
- **Test:** drive the FSM with a synthetic clock crossing the warmup boundary and
  assert suppress→honor flips (the gate math is already unit-tested; this asserts
  the *driver* populates the context correctly).

## Acceptance

- `start_converse` builds `GeminiConfig::audio`; `end_user_turn()` exists and is
  dispatched for `EndUserTurn`; the converse-event loop drives a `TurnMachine`
  and dispatches all `TurnAction`s; `PlayAudio` decodes bytes→i16 to a 24 kHz
  stream; capture is gated + `CancelToken` is real; `SignalContext` is populated
  from a live clock/VAD.
- `cargo clippy --features cloud --all-targets -D warnings` clean; WSL `cargo
  test cloud` green; **plus a live runtime smoke** with the gemini key in
  credentials.yaml (B15/B18 live-smoke item) — a real spoken turn produces
  audible reply + a barge-in cuts it.
- Notes-mode `start_gemini` path unchanged and still green.

## Sequencing note

Steps 1–3 are the critical path (config + end-turn + driver loop) and unblock a
basic working turn (no barge-in). Steps 4–5 make it audible + interruptible.
Step 6 upgrades barge-in to full-duplex. A reviewable PR can land 1–3 first, then
4–6, or all six as one converse-driver commit. This is **not** a worktree-parallel
wave — all six steps touch commands.rs (the driver), so they are sequential
within one agent/worktree.
