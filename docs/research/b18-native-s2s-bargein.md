# Research: B18 — Native Speech-to-Speech with Barge-in (ADR-0013 step 3 / ADR-0003 turn protocol)

Implementation-ready findings for native **audio-out** + a **barge-in-capable turn
orchestrator** for BOTH Gemini Live and OpenAI Realtime voice. Sources are primary
(ai.google.dev Live docs + `/api/live` reference; developers.openai.com Realtime
reference) plus production-pattern repos (LiveKit Agents, Pipecat) via DeepWiki.
All fetched 2026-05-30. Cross-checked against existing `src-tauri/src/gemini/mod.rs`
and `docs/research/openai-realtime-2026-05.md`.

> TL;DR for the implementer: the change from today's `responseModalities:["TEXT"]`
> is a **config + decode + playback + interruption-routing** change, not a new
> transport. Both engines emit **base64 PCM16 LE @ 24 kHz mono** audio deltas, both
> have a server-side interruption signal, and both need the client to (a) stop local
> playback and (b) tell the server how much audio was actually heard. The hard part
> is **echo suppression** (so the assistant doesn't transcribe/interrupt itself) and
> a **provider-agnostic turn state machine**.

---

## 1. Gemini Live — enabling AUDIO output + interruption

### 1.1 Model IDs (current Live family, 2026-05)
The code's tests already use `gemini-3.1-flash-live-preview` — that is current.
Confirmed Live-capable IDs from ai.google.dev models/capabilities pages:

| Model ID | Class | Notes |
|---|---|---|
| `gemini-3.1-flash-live-preview` | newest A2A / native audio | low-latency dialogue; `send_client_content` only seeds initial context; no affective/proactive; **sequential** function calls only |
| `gemini-2.5-flash-native-audio-preview-12-2025` | native audio output | "flagship" native-audio voice/video agent; 128k context |
| `gemini-2.5-flash-live-preview` | half-cascade / live | supports `enable_affective_dialog` + `proactive_audio` (v1alpha), `NON_BLOCKING` async function calls, `send_client_content` throughout |

- **Native-audio** models generate speech directly from the model (more expressive,
  affective/proactive features). **Half-cascade** = model text → internal TTS.
  For AudioGraph's voice agent, default to a native-audio model
  (`gemini-3.1-flash-live-preview` is the safe current pick; it's what tests assume).
- Context window: native-audio output models **128k tokens**; other Live models **32k**.
- Transport is unchanged: the same `BidiGenerateContent` WebSocket already wired in
  `gemini/mod.rs`. Only `setup` and message handling change.

### 1.2 Enabling AUDIO in `build_setup_message` (the one-line-ish change)
Today (`gemini/mod.rs:621`):
```json
"generationConfig": { "responseModalities": ["TEXT"], "inputAudioTranscription": {} }
```
Native audio out:
```json
"generationConfig": {
  "responseModalities": ["AUDIO"],
  "speechConfig": {
    "voiceConfig": { "prebuiltVoiceConfig": { "voiceName": "Kore" } }
  }
},
"outputAudioTranscription": {},
"inputAudioTranscription": {}
```
Important constraints (verified):
- `responseModalities` accepts **exactly one** modality per session — you get **AUDIO
  XOR TEXT**, *not both*. You cannot ask for `["TEXT","AUDIO"]`. To still drive graph
  extraction from the spoken reply, enable `outputAudioTranscription: {}` — the server
  then sends `serverContent.outputTranscription.text` *alongside* the audio. So: audio
  for the user, transcript text for graph proposals. (This is why AudioGraph needs the
  output transcription path even in AUDIO mode.)
- `speechConfig.voiceConfig.prebuiltVoiceConfig.voiceName` selects the voice (e.g.
  `Kore`, `Puck`, `Charon`, `Aoede`, `Fenrir` — Gemini's prebuilt set). Optional
  `speechConfig.languageCode`.
- `inputAudioTranscription: {}` stays (already present) so user speech still feeds the graph.

### 1.3 Output-audio message shape
Audio arrives inside the existing `serverContent` envelope this code already parses:
```
serverContent.modelTurn.parts[].inlineData.data       // base64 bytes
serverContent.modelTurn.parts[].inlineData.mimeType    // e.g. "audio/pcm;rate=24000"
```
- **Format: raw 16-bit PCM, little-endian, mono, 24 kHz output** (input stays 16 kHz —
  matches `send_audio`'s existing 16 kHz path). Audio output sample rate is *always*
  24 kHz per the docs.
- So `handle_server_message` must, inside the `modelTurn.parts` loop, additionally
  branch on `part.inlineData`: base64-decode → emit a new `GeminiEvent::AudioChunk`
  (bytes + sample_rate=24000) instead of only handling `part.text`. The existing
  `Message::Binary` warning path is unused (frames are JSON text).

### 1.4 Turn / interruption signals (`BidiGenerateContentServerContent`)
Exact fields from `/api/live`:
| Field | Type | Meaning |
|---|---|---|
| `modelTurn` | `Content` | carries `parts[].inlineData` (audio) / `parts[].text` |
| `turnComplete` | bool | model finished this turn (already handled) |
| `generationComplete` | bool | generation done (precedes turnComplete) — **new, surface it** |
| `interrupted` | **bool** | **the barge-in signal** — VAD detected user speech, server canceled+discarded the in-flight generation |
| `inputTranscription` | `{text}` | user speech (already handled) |
| `outputTranscription` | `{text}` | assistant speech-as-text (new; for graph in AUDIO mode) |

- **Interruption is automatic by default.** With server VAD on, when the user speaks,
  the server cancels the ongoing generation, discards pending function calls, and sends
  a `serverContent.interrupted == true` frame. Only audio *already sent to the client* is
  retained in session history. The client MUST react by **flushing any queued/unplayed
  audio** locally (the server stops producing, but you may have buffered chunks).
- New events to add: `GeminiEvent::Interrupted` and `GeminiEvent::AudioChunk { data, sample_rate }`
  (and optionally `GenerationComplete`).

### 1.5 VAD: server (automatic) vs manual (`realtimeInputConfig`)
`setup.realtimeInputConfig`:
```json
"realtimeInputConfig": {
  "automaticActivityDetection": {
    "disabled": false,
    "startOfSpeechSensitivity": "START_SENSITIVITY_LOW",
    "endOfSpeechSensitivity": "END_SENSITIVITY_LOW",
    "prefixPaddingMs": 20,
    "silenceDurationMs": 100
  },
  "activityHandling": "...",   // enum: how to treat activity during generation
  "turnCoverage": "..."        // enum: which audio counts toward the turn
}
```
- **Automatic VAD (default, recommended for v1):** server detects start/end of user
  speech and auto-interrupts. Just send `realtimeInput.audio` chunks as today.
- **Manual VAD:** set `automaticActivityDetection.disabled = true`, then on the
  `BidiGenerateContentRealtimeInput` send `{ activityStart: {} }` / `{ activityEnd: {} }`
  around speech (instead of `audioStreamEnd`). Use only if AudioGraph's own VAD is
  better than the server's. Docs warn manual end-of-speech threshold should be **≥500 ms**
  to avoid fragmenting audio. **Recommendation: stick with automatic VAD for B18.**
- `BidiGenerateContentRealtimeInput` fields: `audio: Blob`, `audioStreamEnd: bool`,
  `activityStart`, `activityEnd`.

---

## 2. OpenAI Realtime — VOICE (speech-to-speech), GA shape

Cross-checks and extends `docs/research/openai-realtime-2026-05.md` §"Voice s2s".
GA only (no `OpenAI-Beta` header). URL `wss://api.openai.com/v1/realtime?model=gpt-realtime-2`,
`Authorization: Bearer <KEY>`. Model `gpt-realtime-2` (newest); voices incl. `marin`, `cedar`.

### 2.1 Voice session config (`session.type="realtime"`)
```json
{ "type": "session.update", "session": {
  "type": "realtime",
  "model": "gpt-realtime-2",
  "output_modalities": ["audio"],
  "instructions": "...system prompt + graph context...",
  "audio": {
    "input": {
      "format": { "type": "audio/pcm", "rate": 24000 },
      "turn_detection": { "type": "semantic_vad" }
    },
    "output": {
      "format": { "type": "audio/pcm" },   // pcm16 @ 24 kHz
      "voice": "marin"
    }
  }
}}
```
- **Output audio is PCM16 LE, 24 kHz, mono** — same rate as Gemini output. Convenient:
  one playback path serves both engines.
- `turn_detection`: `server_vad` (energy-based, with `threshold`/`prefix_padding_ms`/
  `silence_duration_ms`) or `semantic_vad` (model decides end-of-turn — better for barge-in
  / fewer false cuts). Set `null` for fully manual (push-to-talk).
- Voice **cannot change after the model has emitted audio** in a session.
- GA format gotcha (from prior research): newest models use the **object** form
  `{type:"audio/pcm",rate:24000}`; older GA used the string `"pcm16"`. Keep configurable.

### 2.2 Driving a response
- Auto (server VAD): on `input_audio_buffer.speech_stopped` + commit, the server creates a
  response automatically. To control it, send `response.create` yourself:
  ```json
  { "type": "response.create", "response": { "output_modalities": ["audio"] } }
  ```
- Manual input: `input_audio_buffer.append {audio:"<b64 pcm16>"}` → `input_audio_buffer.commit`
  → `response.create`.

### 2.3 Server audio-output events (the bytes)
| Event | Carries |
|---|---|
| `response.output_audio.delta` | **base64 audio chunk** (`delta`) — the bytes to play |
| `response.output_audio.done` | end of audio for an item (NO bytes — transcript only) |
| `response.output_audio_transcript.delta` / `.done` | streaming transcript of the spoken reply (use for graph proposals) |
| `response.created` / `response.done` | response lifecycle; `response.done.status` can be `cancelled` |
| `rate_limits.updated` | quota; listen for it |
- Note the GA rename: it's `response.output_audio.delta` (older beta was `response.audio.delta`).
  Some community snippets still show the old name; emit/handle the GA name.

### 2.4 Barge-in / interruption — the exact WebSocket sequence
Two signals + a 3-step client reaction:

**Detect (server VAD on):**
- `input_audio_buffer.speech_started` — user began speaking (the barge-in trigger).
- `input_audio_buffer.speech_stopped` — user finished.

**React (client, in order):**
1. **Stop local playback immediately** and **measure** how many ms of assistant audio
   were *actually played* (`audio_end_ms`). This is the load-bearing number.
2. `response.cancel` — stops the server generating more of the current response. Server
   replies `response.done` with `status=cancelled`.
   ```json
   { "type": "response.cancel" }          // or {"type":"response.cancel","response_id":"resp_..."}
   ```
3. `conversation.item.truncate` — tells the server to **discard the audio (and its text
   transcript) past what the user heard**, so the model's context matches reality:
   ```json
   { "type": "conversation.item.truncate",
     "item_id": "item_1234",      // the assistant item being spoken
     "content_index": 0,
     "audio_end_ms": 1500 }       // ms actually played; >actual duration → server error
   ```
   Server confirms with `conversation.item.truncated`. Truncating **deletes the unheard
   text transcript** so the model doesn't "remember" saying something the user never heard.

**Gotchas (from OpenAI community / production reports):**
- `output_audio_buffer.clear` / `.cleared` / `.audio_started` / `.audio_stopped` are
  **WebRTC/SIP only** — they do NOT apply to the WebSocket transport AudioGraph uses.
  On WebSocket, the server produces audio *faster than real-time*, so chunks are already
  buffered client-side; **the client owns stopping playback**. `response.cancel` alone
  does not silence already-delivered audio — you must drop your local buffer.
- Therefore the correct WebSocket barge-in is: **(local) flush playback buffer →
  `response.cancel` → `conversation.item.truncate(audio_end_ms = ms_played)`**.
- `audio_end_ms` must be ≤ actual audio duration or the server errors — track played ms
  conservatively (count samples popped from the playback ring buffer / fed to the sink).

### 2.5 Reconnect/limits (unchanged from prior research)
Max session 60 min, **no resume** — reconnect + re-send `session.update`, treat as a new
item namespace. (Contrast Gemini, which *does* support `sessionResumption` — already wired.)

---

## 3. Barge-in / half-duplex pipeline design (production patterns)

The current frontend guard (`useConverseFrontLeg.ts` `onSegmentText`: "while
`isChatLoading`, ignore incoming transcripts") is a **coarse half-duplex mute**: it
prevents the echo loop but also **drops genuine user barge-in** during a reply. True
barge-in needs pipeline-side design. Synthesis of LiveKit Agents + Pipecat + Deepgram:

### 3.1 The self-interruption / echo problem (root cause)
With loopback/system-audio capture, the mic stream contains the assistant's own TTS.
Naive VAD then fires on the assistant's voice → **false barge-in** (assistant interrupts
itself) and, worse, the assistant's words get transcribed and fed back as a new "user
turn" → **self-sustaining echo loop**. Production reports (Asterisk/Twilio, Deepgram)
confirm this is *the* dominant failure mode.

### 3.2 The layered mitigation stack (apply in this order)
1. **Acoustic Echo Cancellation (AEC) — closest to the source, "essentially free."**
   Prefer **platform-native AEC** (OS/WebRTC APM `EchoCanceller`). For AudioGraph this
   means capturing the mic with echo cancellation enabled rather than raw loopback when
   in converse mode, or running a WebRTC-style AEC (reference = TTS playback stream,
   time-aligned with mic) in the pipeline. Without time-aligned AEC, server VADs treat
   echoed TTS as speech.
2. **Prefer model-level speech detection over raw energy VAD.** Deepgram and OpenAI
   `semantic_vad` operate on *speech content*, not energy, so background noise / TV /
   the assistant's own residual echo cause far fewer false interrupts than a client-side
   energy VAD (Silero/WebRTC VAD). For native engines, **let the provider's server VAD
   do detection** (Gemini auto-VAD, OpenAI `semantic_vad`).
3. **AEC warmup window (LiveKit's trick).** When the agent enters `speaking`, start an
   `aec_warmup` timer (a few hundred ms) during which **audio-activity interruptions are
   disabled** — gives the AEC adaptive filter time to converge before allowing barge-in.
   Deepgram makes the same point: an opening greeting helps AEC calibrate.
4. **Minimum-interruption gates (don't cut on a cough).** LiveKit: `min_interruption_duration`
   (min speech length, e.g. 500 ms) and `min_interruption_words` (STT-only). Pipecat: VAD
   `start_secs` / `stop_secs` debounce. Only treat sustained speech as a real barge-in.
5. **False-interruption recovery (LiveKit `resume_false_interruption`).** If you paused
   the agent on a suspected barge-in but no real user speech follows within
   `false_interruption_timeout`, **resume** the paused TTS instead of dropping the turn.
6. **Backchannel suppression.** Near turn boundaries, brief overlaps ("uh-huh", "yeah")
   are backchannels, not interruptions — suppress them in a small boundary window
   (LiveKit `backchannel_boundary`).

### 3.3 How the two frameworks signal it (for our event model)
- **Pipecat:** VAD (`SileroVADAnalyzer`) → `VADUserStartedSpeakingFrame` →
  `LLMUserAggregator.broadcast_interruption()` pushes an `InterruptionFrame` up+down the
  pipeline. On that frame: `TTSService` clears its audio queue, `BaseOutputTransport`/
  `MediaSender` cancels audio tasks + clears buffers (immediate silence) and emits
  `BotStoppedSpeakingFrame`; the realtime LLM service truncates the current audio response.
  `BotStartedSpeakingFrame`/`BotStoppedSpeakingFrame` bracket assistant speech.
- **LiveKit Agents:** `AgentSession` tracks `user_state` + `agent_state`
  (`initializing → listening → thinking → speaking → listening`). EOU via VAD / STT /
  `realtime_llm` / custom turn-detector model. `InterruptionOptions` (`enabled`,
  `min_duration`, `min_words`, `resume_false_interruption`, `false_interruption_timeout`,
  `backchannel_boundary`, `aec_warmup_duration`). User speech during `speaking` →
  agent → `listening`/`thinking`; agent emits `AgentStateChangedEvent`.

### 3.4 AudioGraph recommendation (pipeline-side, replaces the coarse guard)
- Keep converse capture **mic-with-AEC**, not raw loopback, while speaking (or run a
  reference-aligned AEC). This is the single highest-leverage fix for the echo loop.
- Drive barge-in detection from the **provider's server VAD** (Gemini auto / OpenAI
  `semantic_vad`) for native engines — they already gate on speech content. The backend
  turn orchestrator (not the React hook) owns the reaction.
- On a confirmed barge-in: **stop playback first**, then run the per-provider cancel
  sequence (§1.4 react / §2.4 react), then transition to `listening`.
- Add `min_interruption_duration` + an AEC warmup window so the assistant can't cut
  itself off in the first moments of speaking.
- The frontend `isChatLoading` mute can stay as a **pipelined-engine fallback** when no
  AEC is available, but native engines should use real barge-in.

---

## 4. Provider-agnostic turn-state machine

A single FSM both engines (and the pipelined STT→LLM→TTS path) implement. This realizes
ADR-0003's `turn_start / audio append / turn_end / cancel / barge_in` protocol and the
`S2STurnState` bounded-buffer idea, in Rust terms in the backend orchestrator.

### 4.1 States
| State | Meaning |
|---|---|
| `Idle` | session open, not in a turn; no audio flowing to model |
| `Listening` | capturing user audio, streaming to provider; waiting for end-of-speech |
| `Thinking` | end-of-user-turn detected; model generating, no audio out yet |
| `Speaking` | assistant audio chunks arriving + being played; (AEC-warmup sub-phase blocks barge-in) |
| `Interrupted` | barge-in confirmed during `Speaking`; running cancel/truncate; transient → `Listening` |

### 4.2 Transitions + the provider events that drive each
```
Idle ──(session ready / user mic on)──────────────► Listening

Listening ──(user end-of-speech)──────────────────► Thinking
   Gemini : automatic VAD endpoint (server-side)
   OpenAI : input_audio_buffer.speech_stopped (+ commit)
   Pipe.  : ASR endpoint / utterance_end / silence timeout

Thinking ──(first assistant audio chunk)──────────► Speaking
   Gemini : serverContent.modelTurn.parts[].inlineData (first)
   OpenAI : response.output_audio.delta (first)

Speaking ──(turn finished, playback drained)──────► Listening
   Gemini : serverContent.generationComplete → turnComplete
   OpenAI : response.output_audio.done → response.done

Speaking ──(barge-in confirmed)───────────────────► Interrupted
   Gemini : serverContent.interrupted == true   (server auto-fires on VAD)
   OpenAI : input_audio_buffer.speech_started    (client must react)
   gate   : only if past aec_warmup AND speech ≥ min_interruption_duration

Interrupted ──(cancel/truncate done)──────────────► Listening
   action(Gemini): drop local audio buffer; (auto-canceled server-side)
   action(OpenAI): stop playback → response.cancel → conversation.item.truncate(audio_end_ms)
   action(Pipe.) : InterruptionFrame → clear TTS queue + output buffers

any ──(user stop / session close / error)─────────► Idle
any ──(false interruption timeout, no real speech)► resume Speaking   (optional, LiveKit-style)
```

### 4.3 The provider-agnostic trait surface (sketch)
A `RealtimeAgent` trait the orchestrator drives, with engine-specific impls. Each impl
maps its raw events onto a normalized `TurnSignal` enum the FSM consumes:
```text
enum TurnSignal {
    UserSpeechStarted,          // → maybe Interrupted (if Speaking) else stay Listening
    UserSpeechEnded,            // Listening → Thinking
    AssistantAudio { pcm24: Vec<u8> },   // Thinking/Speaking; feed playback
    AssistantTranscript { text, final_ },// → graph proposal queue
    GenerationComplete,         // Speaking bookkeeping
    TurnComplete,               // Speaking → Listening (after drain)
    Interrupted,                // Speaking → Interrupted
    Error { category, message },
}
```
- **Outbound ops** the FSM calls on the active impl: `append_audio(pcm16_16k)`,
  `end_user_turn()`, `cancel()` (barge-in: Gemini = local flush; OpenAI = cancel+truncate
  with `audio_end_ms`), `set_voice/config`.
- **Bounded buffers + cancellation token per turn** (ADR-0003): the playback ring buffer
  feeds `audio_end_ms`; the cancellation token is tripped on `Interrupted`/`cancel` and
  checked at every async await boundary (the HF streaming-S2S pattern ADR-0003 cites).
- **Latency milestones** to emit (ADR-0003): turn_start, user end-of-speech, first
  assistant audio (`Thinking→Speaking`), final assistant audio, cancel-ack.

### 4.4 Event-model deltas needed in `gemini/mod.rs` (no code change here — just the map)
- Add `GeminiEvent::AudioChunk { data: Vec<u8>, sample_rate: u32 }` (24000),
  `GeminiEvent::Interrupted`, `GeminiEvent::OutputTranscription { text }`, optionally
  `GenerationComplete`. Extend `handle_server_message` `modelTurn.parts` loop to decode
  `inlineData`, and add `serverContent.interrupted` / `outputTranscription` /
  `generationComplete` branches. Make `responseModalities` + `speechConfig` configurable
  in `GeminiConfig` (so notes-mode keeps TEXT, converse-mode uses AUDIO).
- These feed the same `TurnSignal` map as the OpenAI impl, so the FSM is engine-agnostic.

---

## Sources (primary unless noted)
- Gemini Live audio/interruption: <https://ai.google.dev/gemini-api/docs/live-guide>,
  <https://ai.google.dev/gemini-api/docs/live>, <https://ai.google.dev/gemini-api/docs/live-api/capabilities>
- Gemini Live field reference (`responseModalities`, `serverContent.interrupted`,
  `realtimeInputConfig.automaticActivityDetection`, `activityStart/End`): <https://ai.google.dev/api/live>
- Gemini model IDs: <https://ai.google.dev/gemini-api/docs/models>
- OpenAI Realtime client/server events (`response.cancel`, `conversation.item.truncate`,
  `output_audio_buffer.*` = WebRTC/SIP only): <https://developers.openai.com/api/reference/resources/realtime/client-events>
- OpenAI Realtime conversations guide (voice session, `response.output_audio.delta`,
  speech_started/stopped): <https://developers.openai.com/api/docs/guides/realtime-conversations>
- OpenAI community (WebSocket barge-in: cancel + clear local buffer; `audio_stopped` undocumented):
  <https://community.openai.com/t/need-help-being-able-to-interrupt-the-realtime-api-response/972589>
- Barge-in / AEC patterns: Deepgram <https://developers.deepgram.com/guides/deep-dives/audio-preprocessing-barge-in>;
  LiveKit Agents (DeepWiki: AgentSession states, InterruptionOptions, aec_warmup,
  resume_false_interruption); Pipecat (DeepWiki: InterruptionFrame, VAD frames,
  BotStarted/StoppedSpeakingFrame)
- Internal: `docs/research/openai-realtime-2026-05.md`, ADR-0003, ADR-0006, ADR-0013,
  `src-tauri/src/gemini/mod.rs`, `src/hooks/useConverseFrontLeg.ts`
