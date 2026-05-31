# Research: OpenAI Realtime API (B15 / ADR-0002) — 2026-05-30

Implementation-ready findings for a Rust (Tauri) STT provider, then voice agent.
Sources: developers.openai.com (platform.openai.com now 301s here), API reference,
changelog. All fetched 2026-05-30.

## Critical
- Realtime **Beta removed from the API 2026-05-12**. Implement **GA** shape only.
- Do **NOT** send `OpenAI-Beta: realtime=v1` (that is the removed beta).
- GA went live 2025-08-28; "Realtime 2" models added 2026-05-07.

## Transport / connection
- For a Rust desktop/server client receiving raw PCM: use **WebSocket** (not WebRTC).
- URL: `wss://api.openai.com/v1/realtime?model=<MODEL>`
- Headers: `Authorization: Bearer <KEY>` (+ optional `OpenAI-Safety-Identifier`). No beta header.
- Transcription session: connect then send `session.update` with `session.type="transcription"`.

## Models (exact current ids)
- STT streaming (recommended): `gpt-realtime-whisper` (2026-05-07; natively streaming;
  requires `turn_detection:null` + manual commit; no `prompt`).
- STT stable: `gpt-4o-transcribe`, `gpt-4o-mini-transcribe`, `whisper-1`.
- Voice s2s: `gpt-realtime-2` (newest, configurable reasoning), `gpt-realtime` (first GA),
  `gpt-realtime-1.5`. Voices incl. `marin`, `cedar` (recommended).
- `gpt-4o-transcribe-diarize` is REST `/v1/audio/transcriptions` ONLY — not Realtime.

## Transcription-only session.update (GA, gpt-realtime-whisper)
```json
{"type":"session.update","session":{"type":"transcription","audio":{"input":{
  "format":{"type":"audio/pcm","rate":24000},
  "transcription":{"model":"gpt-realtime-whisper","language":"en"}}}}}
```
- `audio.input.transcription.delay`: minimal|low|medium|high|xhigh (whisper only).
- With `gpt-4o-transcribe` you may add `turn_detection:{type:"server_vad",threshold:0.5,
  prefix_padding_ms:300,silence_duration_ms:200}` and `include:["item.input_audio_transcription.logprobs"]`.
- GA format gotcha: newer models use **object** `{type:"audio/pcm",rate:24000}`; older GA
  (`gpt-realtime`, transcription_sessions REST) use **string** `"pcm16"`. Make it configurable.
- Transcription is OFF unless `transcription.model` is set.

## Events
Client->server: `session.update`, `input_audio_buffer.append` `{audio:"<b64 pcm16>"}`,
`input_audio_buffer.commit`, `input_audio_buffer.clear`, (voice) `response.create`,
`response.cancel`, `conversation.item.truncate`, `conversation.item.create`.

Server->client (transcription): `session.created/updated`,
`input_audio_buffer.speech_started/stopped/committed/cleared`,
`conversation.item.added/done`,
`conversation.item.input_audio_transcription.delta` `{item_id,content_index,delta}`,
`...completed` `{item_id,content_index,transcript}`,
`...segment`, `...failed` `{item_id,content_index,error{type,code,message,param}}`,
generic `error` `{type,code,message,param,event_id}`.
**Correlate by `item_id` (+content_index).** Ordering across turns not guaranteed.

Server->client (voice GA, renamed from beta): `response.output_audio.delta` (the bytes),
`response.output_audio_transcript.delta/done`, `response.output_text.delta/done`,
`response.created/done`, `rate_limits.updated`.

## Audio format
- PCM16 LE, **24000 Hz only**, **mono**. base64 per `input_audio_buffer.append`.
- append payload <=15 MB; send ~20-100ms/chunk (24kHz mono pcm16 = 48000 B/s).
- `audio/pcmu`/`audio/pcma` for telephony only.

## Reconnect / limits
- Max session **60 min**; no resume — reconnect + re-send `session.update`, treat as new item namespace.
- Reconnect exponential backoff. 429 surfaces inside `...input_audio_transcription.failed`.
- Listen for `rate_limits.updated`. Set client `event_id` to correlate server `error`.

## Rust
- `async-openai` (feat `realtime`/`realtime-types`) has **GA** types
  (`RealtimeSessionCreateRequestGA`, transcription delta/completed/failed events). Bundles
  tokio-tungstenite ^0.28, base64 ^0.22. Verify serde renames vs GA names; pin + round-trip test.
- Reference (pre/early-GA names, use for cpal/WS plumbing only): raja-patnaik/openai-realtime-rust,
  lukacf/oai-rt-rs.
- Suggested stack: tokio, tokio-tungstenite 0.28, serde/serde_json (tag="type" enum),
  base64 0.22, backoff; build handshake request manually to attach Authorization.

## Build order
1. Transcription provider (whisper model, manual commit) keyed on item_id.
2. Add VAD via gpt-4o-transcribe + server_vad.
3. Voice agent (session.type=realtime, gpt-realtime-2, output audio + truncate/cancel barge-in).
