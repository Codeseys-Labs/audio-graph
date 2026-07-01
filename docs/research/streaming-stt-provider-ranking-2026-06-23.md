# Streaming STT Provider Ranking - 2026-06-23

Fetched 2026-06-23. Scope: turn the Artificial Analysis streaming STT list and
official provider docs into an AudioGraph integration order. This is not a
generic benchmark mirror; it ranks providers by fit for AudioGraph's saved
transcript, revisioned notes, temporal graph, local credentials, and
cross-platform desktop constraints.

Sources:

- Artificial Analysis streaming STT leaderboard:
  <https://artificialanalysis.ai/speech-to-text/streaming>
- Soniox STT realtime docs:
  <https://soniox.com/docs/stt/rt/real-time-transcription>
- Soniox STT WebSocket API reference:
  <https://soniox.com/docs/api-reference/stt/websocket-api>
- Soniox data residency:
  <https://soniox.com/docs/data-residency>
- AssemblyAI Universal-Streaming quickstart:
  <https://www.assemblyai.com/docs/streaming/getting-started/transcribe-streaming-audio>
- Gladia live transcription quickstart:
  <https://docs.gladia.io/chapters/live-stt/quickstart>
- Speechmatics realtime API reference:
  <https://docs.speechmatics.com/api-ref/realtime-transcription-websocket>
- ElevenLabs Scribe realtime API reference:
  <https://elevenlabs.io/docs/api-reference/speech-to-text/v-1-speech-to-text-realtime>

## Short Recommendation

1. Keep OpenAI Realtime STT as a first-class current provider. The backend path
   already exists and the settings UI now exposes it. Treat this as the P0
   drift fix, not a net-new provider project.
2. Implement Soniox v5 Real-Time as the first net-new provider after the
   provider registry and normalized ASR event contract land.
3. Upgrade AssemblyAI from the current v2 realtime socket to v3
   Universal-Streaming in the same wave as Soniox or immediately after it.
4. Spike Gladia Solaria-1 and Speechmatics Enhanced next. Both are credible,
   but they introduce enough lifecycle/config surface that they should follow
   the registry instead of being added as one-off UI fields.
5. Keep Google Chirp, Azure Speech, ElevenLabs Scribe v2 Realtime, RevAI, xAI,
   Mistral Voxtral, NVIDIA/Together, Cartesia, Qwen/Alibaba, Inworld,
   Smallest.ai, and Gradium behind research/watch items until the registry can
   represent enterprise auth, model availability, retention controls, and
   partial/final semantics without additional TS/Rust drift.
   The 2026-06-25 Artificial Analysis tail-provider backfill is recorded in
   `docs/research/artificial-analysis-streaming-stt-watchlist-2026-06-25.md`.

Artificial Analysis compares streaming STT providers on final WER, first-partial
WER, latency, and price. Its fetched page lists the current compared models and
providers, including xAI, AssemblyAI, ElevenLabs, Gladia, Deepgram, Speechmatics,
NVIDIA/Together, Soniox, Inworld, OpenAI, Google, Mistral, Smallest.ai, Gradium,
Alibaba/Qwen, Amazon, Azure, Cartesia, and RevAI. The chart values are rendered
client-side, so this note uses the provider/model set and official protocol docs,
not scraped numeric benchmark rankings.

## AudioGraph Fit Criteria

- Backend-owned provider registry can declare the provider without duplicating
  Rust settings enums, TS unions, Settings UI strings, credential keys, model
  catalog logic, and health commands.
- Provider can emit stable span identity and revision metadata for partials,
  finals, turns, speakers, timestamps, source id, provider id, and raw event
  provenance.
- Provider health/model discovery can run from saved `credentials.yaml` entries
  without echoing plaintext keys into normal UI state.
- Audio transport fits current processed-audio frames: preferably mono PCM16
  16 kHz or 24 kHz, binary WebSocket frames, and bounded queue/backpressure.
- Provider-specific advanced controls can be hidden until needed: endpointing,
  diarization, language hints, data residency, retention/logging, custom terms,
  translation, and channel handling.

## Ranking

| Tier | Provider | Why | Main risks | Seed |
| --- | --- | --- | --- | --- |
| P0 done | OpenAI Realtime STT | Existing backend implementation; OpenAI key already exists; useful baseline for realtime transcription. | Requires 24 kHz mono PCM and item-id correlation; Realtime voice-agent path must stay separate from STT. | `audio-graph-5749` |
| P1 | Soniox v5 Real-Time | Clean direct WebSocket provider; model `stt-rt-v5`; token-level final/non-final flags; timestamps, confidence, speaker, language, final/total audio progress; REST model listing; US/EU/JP domains. | Token stream needs an adapter to construct revisioned spans and turn boundaries; endpointing/manual finalization must be represented in the normalized ASR contract. | `audio-graph-e35f` |
| P1 | AssemblyAI v3 Universal-Streaming | We already have an AssemblyAI provider, so this is an upgrade rather than a new product surface; v3 has Begin/Turn/Termination events, `turn_order`, `end_of_turn`, word timing, and direct WebSocket docs. | Current code uses the older v2 endpoint and message shape; migration needs parser fixtures and a saved-settings compatibility story. | `audio-graph-f0a3` |
| P2 | Gladia Solaria-1 live | Strong live product fit; init call returns a resumable signed WebSocket URL; supports binary or JSON audio chunks, partial transcripts, endpointing, speech start/end, multi-channel attribution. | Two-step lifecycle, message filtering, cleanup, and result retrieval are heavier than Soniox/AssemblyAI; should not be added before the registry can encode lifecycle shape. | `audio-graph-228b` |
| P2 | Speechmatics Realtime Enhanced | Mature enterprise API; StartRecognition/AddAudio/AddTranscript/AddPartialTranscript/EndOfUtterance map well to a normalized event adapter; strong diarization and long-session story. | Enterprise configuration surface is large: language/model quality, JWT/bearer auth, partial toggles, max delay, speaker labels, multi-channel, and errors. | `audio-graph-1476` |
| P3 | Google Chirp / Azure Speech | Enterprise demand, compliance, and familiar procurement. | SDK/gRPC/auth/packaging complexity is higher than WebSocket-first providers; assess after P1/P2 registry work. | `audio-graph-473b` |
| P3 watch | ElevenLabs Scribe v2 Realtime | Realtime WebSocket exposes `partial_transcript`, `committed_transcript`, timestamps, language detection, keyterms, manual/VAD commit strategy, and single-use token auth. | Need to prove transcript/graph-quality value over Soniox/AssemblyAI and clarify retention/logging/cost knobs before UI exposure. | `audio-graph-7e92` |

## Registry Implications

The provider registry should model these differences before more providers are
implemented:

- `provider_id`, display name, stage (`asr`, `llm`, `tts`, `realtime_agent`).
- Credential key(s), credential mode, region/data-residency fields, and whether
  the UI can use a temporary client token.
- Transport: WebSocket direct, WebSocket after REST init, AWS eventstream, SDK,
  local process/FFI.
- Required audio format: sample rate, channels, encoding, frame type, max frame
  size, provider pacing/keepalive rules.
- Event semantics: token stream, partial transcript, committed transcript,
  turn message, endpoint event, provider item id, timestamps, confidence,
  speaker/channel/language metadata, finality model, and revision strategy.
- Capability flags: interim results, explicit endpointing, manual finalize,
  diarization, channel separation, language identification, translation,
  custom vocabulary/keyterms, retention/zero-data controls, model listing,
  health check.
- Settings schema groups: basic setup, model picker, language, speaker/turn,
  privacy/data residency, advanced latency/endpointing, diagnostics.

Without this registry, every added provider repeats the current drift pattern:
Rust enum variant, TS union, Settings state, i18n label, credential routing,
first-run detection, model loading, health testing, and transcript adapter all
change independently.

## Normalized ASR Event Requirements

The P1 provider work should not emit provider-specific transcript strings
directly into notes/graph. Each parser should map into a common event shape:

- `span_id`: stable local id for the transcript span being revised.
- `provider_item_id`: provider-native id where available.
- `provider`: canonical provider id.
- `source_id`: AudioGraph source descriptor id.
- `speaker_id` and/or `channel`: nullable but explicit.
- `start_ms`, `end_ms`, `received_at_ms`.
- `text`, `is_final`, `stability`, `confidence`.
- `revision_number`, `supersedes`, and `turn_id`.
- `end_of_turn`: boolean or enum when the provider can distinguish it.
- `raw_event_ref`: pointer/hash for audit replay without storing secrets.

Soniox maps final/non-final tokens into rolling span revisions. AssemblyAI maps
Turn messages into span revisions keyed by `turn_order`. Gladia maps transcript
messages by `data.id` and uses `is_final`; speech start/end events can set turn
state. Speechmatics maps AddPartialTranscript/AddTranscript/EndOfUtterance into
partial/final/turn events with word-level result metadata.

## UI and Config Implications

- Basic Settings should expose provider, credential presence, model, and
  language first. Endpointing/diarization/keyterms/regions belong under
  Advanced until the user opts in.
- Saved credentials must unlock model catalogs and health checks without
  requiring key re-entry. The next architecture slice should replace plaintext
  `load_all_credentials_cmd` flows with redacted provider readiness.
- Model catalogs should be provider-specific but rendered through one searchable
  combobox. Providers without catalog APIs need pinned presets plus a custom
  model override.
- Health should distinguish `missing key`, `saved key untested`, `auth failed`,
  `region mismatch`, `quota/rate limited`, `provider unavailable`, and
  `catalog stale`.
- Every provider should show a "why disabled" reason when blocked by feature
  flags, missing credentials, unsupported platform capture, or registry policy.

## Backlog Mapping

- `audio-graph-80ed`: provider capability registry with TS schema generation.
- `audio-graph-3709`: normalized ASR partial/final events with span revisions.
- `audio-graph-e35f`: Soniox v5 Real-Time implementation.
- `audio-graph-f0a3`: AssemblyAI v3 Universal-Streaming upgrade.
- `audio-graph-02da`: reusable streaming WebSocket ASR harness.
- `audio-graph-228b`: Gladia Solaria-1 spike.
- `audio-graph-1476`: Speechmatics Enhanced spike.
- `audio-graph-473b`: Google Chirp/Azure enterprise evaluation.
- `audio-graph-7e92`: ElevenLabs Scribe v2 Realtime watch/spike.

Close `audio-graph-91ca` once this note is accepted as the provider ranking
source of truth.
