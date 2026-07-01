# Speaker channel routing and diarization join decision

Date: 2026-06-26
Seed: `audio-graph-56da`
Status: design recommendation for implementation Seeds

## Decision

AudioGraph should default to **mono processed audio plus revisioned speaker-timeline
metadata joins** for speaker-attributed transcription, notes, and temporal-graph
projection.

Do not synthesize one provider audio channel per diarized speaker. Diarization
answers "who spoke when"; it does not turn one mixed signal into clean,
provider-ready speaker channels. Speaker-separated PCM lanes can exist only as
an explicit experimental source-separation mode after fixtures prove latency,
overlap behavior, and artifact handling.

Source-native multichannel ASR is allowed only when both sides prove semantics:

1. The capture/source descriptor proves that channels are real, stable source
   lanes, not a downmixed desktop/system mix.
2. The provider descriptor proves that the adapter preserves channel order,
   channel count, and provider channel-diarization semantics.
3. The session artifact stores the channel map used for that run so replay can
   reconstruct why a word/span was attributed to a source or channel.

Until those conditions are met, keep `supports_multichannel = false` in the
provider registry and route speaker attribution through `DiarizationSpanRevision`
events joined to ASR spans by time/source.

## Current code evidence

- `src-tauri/src/audio/pipeline.rs` defines the processed-audio bus as 16 kHz
  mono and converts interleaved input to mono before fan-out. It preserves
  `source_id`, but not per-channel PCM.
- `src-tauri/src/audio/consumer.rs` can keep per-source chunks independent and
  records a `ProcessedAudioMixingMode`, but dispatched chunks are still mono.
- `src-tauri/crates/provider-registry/src/lib.rs` has
  `ProviderAudioInputDescriptor.supports_multichannel`; every current ASR and
  diarization descriptor uses the canonical mono pipeline format.
- `src-tauri/src/events.rs` already carries `channel: Option<String>` on ASR and
  diarization span revisions, so channel-aware providers can be normalized later
  without replacing the public event family.
- `src-tauri/src/speech/mod.rs` local clustering emits a session-level speaker
  timeline with `source_id: None` and `channel: None`, then labels transcript
  segments by overlap. That is a metadata join, not an audio-channel split.

## Provider evidence

Primary docs show that channel diarization and speaker diarization are separate
features:

- Deepgram says diarization identifies speakers while multichannel identifies
  audio channels; its multichannel mode transcribes each submitted channel
  independently and supports up to 20 channels.
  https://developers.deepgram.com/docs/multichannel-vs-diarization
  https://developers.deepgram.com/docs/multichannel
- AWS Transcribe speaker diarization labels speakers, while channel
  identification is a separate two-channel feature with `ch_0` and `ch_1`
  labels.
  https://docs.aws.amazon.com/transcribe/latest/dg/diarization.html
  https://docs.aws.amazon.com/transcribe/latest/dg/channel-id.html
- Speechmatics realtime diarization explicitly distinguishes speaker
  diarization, channel diarization, and combined channel plus speaker
  diarization.
  https://docs.speechmatics.com/speech-to-text/realtime/realtime-diarization
- Soniox realtime returns token streams and supports speaker diarization via
  token-level `speaker` fields. Its model page currently emphasizes improved
  speaker separation for `stt-rt-v5`; it does not prove AudioGraph can send
  source-native multichannel audio through the current adapter.
  https://soniox.com/docs/stt/rt/real-time-transcription
  https://soniox.com/docs/stt/concepts/speaker-diarization
  https://soniox.com/docs/stt/models
- Google Cloud STT v2 supports speaker diarization for streaming and non-
  streaming recognition, with word speaker tags and required speaker-count
  configuration.
  https://docs.cloud.google.com/speech-to-text/docs/multiple-voices
  https://docs.cloud.google.com/speech-to-text/docs/reference/rpc/google.cloud.speech.v2
- Azure Speech supports real-time diarization and describes diarization as
  speaker identification over mono-channel recordings.
  https://learn.microsoft.com/en-us/azure/ai-services/speech-service/get-started-stt-diarization
  https://learn.microsoft.com/en-us/azure/foundry/responsible-ai/speech-service/speech-to-text/transparency-note

Artificial Analysis tracks current streaming STT providers, WER, latency, and
price, including xAI, AssemblyAI, ElevenLabs, Gladia, Deepgram, Speechmatics,
NVIDIA, OpenAI, Google, Amazon, Azure, and Soniox. This supports keeping the
provider registry extensible, but not enabling multichannel behavior without
provider-specific adapter proof.
https://artificialanalysis.ai/speech-to-text/streaming

Local/source-separation evidence is promising but not alpha-safe:

- `sherpa-onnx` Rust bindings cover local ASR, VAD, speaker embeddings,
  diarization, denoising, and other speech features; AudioGraph already uses
  sherpa for local diarization paths.
  https://docs.rs/sherpa-onnx
- `charon-audio` is a Rust source-separation crate with ONNX/Candle backends and
  CPAL-oriented real-time support, but it is music/source separation oriented and
  must be proven on meeting speech before routing STT through it.
  https://docs.rs/charon-audio
- `pyannote/speech-separation-ami-1.0` can output diarization plus separated
  speaker streams from 16 kHz mono audio, but it is a Python/Hugging Face
  pipeline with user conditions and is not a drop-in Rust desktop runtime.
  https://huggingface.co/pyannote/speech-separation-ami-1.0

## Mode matrix

| Source/provider mode | Default decision | Why | Required proof before enabling |
|---|---|---|---|
| System mix, device mix, application/process capture | Mono + speaker-timeline metadata join | Current bus downmixes to mono; source channels are not stable speaker lanes | Existing ASR revision fixtures plus diarization overlap/revision fixtures |
| Provider speaker diarization on mono input | Mono ASR plus provider speaker-span revisions | Providers can label speakers without channel semantics | Parser fixtures for partial/final speaker labels, unknown speakers, label remaps |
| Provider channel diarization on source-native channels | Future opt-in | Useful when each participant/source is already on its own channel | `AudioSourceInfo` channel map, provider `max_channels`, adapter fixture preserving channel order |
| Provider channel plus speaker diarization | Future opt-in for Speechmatics-like providers | Needed when channels contain multiple speakers | Combined channel+speaker parser fixtures and conflict policy with local timeline |
| Local clustering diarization | Mono + local provisional speaker timeline | Already wired as rolling-window metadata with retcon risk | Stale-span, overlap, flexible speaker-count, and replay fixtures |
| Source-separated speaker PCM lanes | Experimental only | Separation artifacts can hurt ASR and create false confidence | Golden mixed/overlap clips, separated PCM quality metrics, artifact rollback path, feature gate |

## Architecture shape

1. Keep the processed-audio bus mono and per-source for alpha. This is the
   invariant all current adapters and tests assume.
2. Treat speaker attribution as its own event-sourced timeline:
   `AsrSpanRevision` carries text and timing; `DiarizationSpanRevision` carries
   speaker/channel attribution and basis links. Notes and graph projection read
   a materialized join, not append-only transcript rows.
3. Add provider descriptor fields before enabling channel routes:
   `speaker_attribution_mode`, `max_channels`, `channel_label_semantics`, and
   `requires_source_native_channels`. `supports_multichannel` is too coarse on
   its own.
4. Add source descriptor/channel provenance before sending multichannel audio:
   channel count, channel labels when known, source ids per channel, and whether
   channels are physical, app/process-derived, virtual meeting lanes, or
   generated by source separation.
5. Make the routing rule explicit: `supports_multichannel` must remain false
   unless source provenance and provider capability are both true for the active
   session.
6. Persist the channel/speaker routing plan with session artifacts so replay can
   explain and reconstruct speaker attribution after provider/local retcons.

## Failure modes to design for

- Overlapping speech: channel diarization can preserve simultaneous timestamps
  when real channels exist; speaker diarization on mono may emit unstable or
  overlapping spans; separated-speaker lanes may hallucinate artifacts.
- Speaker-count changes: local clustering and provider diarization can introduce
  new labels mid-session; downstream notes/graph must accept retcons.
- Label churn: provider speaker ids are generic labels, not durable people.
- Source drift: app/process capture by pid can point to a different process after
  restart; source-native channel plans must be session-scoped.
- Backpressure and latency: local diarization windows lag ASR; source separation
  adds compute and may miss real-time budgets.
- Mixed semantics: a "stereo" capture can mean left/right channels, not speaker A
  and speaker B. Do not infer people from channel number without metadata.

## Fixtures and tests required

- ASR span + diarization span replay where a provisional speaker label is later
  superseded and the materialized transcript/notes/graph join updates by diff.
- Two speakers overlapping on mono audio: expected timeline emits either
  overlapping diarization spans or conservative unknown/provisional spans.
- Source-native stereo fixture with explicit channel labels: provider adapter
  preserves `channel` on ASR and diarization revisions.
- Misleading stereo fixture without channel provenance: router refuses
  channel-diarization mode and falls back to mono+timeline.
- Speaker-count growth fixture: labels S1/S2 first, then S3 later, without
  rewriting stable earlier spans unless a superseding revision exists.
- Source-separation artifact fixture: separated lane confidence failure keeps
  original mono ASR as source of truth and records separation as experimental
  metadata only.

## Seed follow-ups

Close `audio-graph-56da` after this recommendation is accepted into the queue.
The implementation work belongs in focused Seeds:

- Provider attribution capability descriptor: add speaker/channel attribution
  mode, max channel count, and source-native-channel requirement to the provider
  registry and generated TS.
- Source channel provenance contract: extend rsac/source descriptors with
  channel-map metadata before any provider can request multichannel audio.
- Speaker timeline replay fixtures: prove ASR plus diarization span revisions
  materialize transcript/notes/graph diffs instead of append-only labels.
- Experimental source-separation bakeoff: evaluate `sherpa-onnx` capabilities,
  `charon-audio`, and pyannote separation against meeting speech before any
  separated-speaker PCM lane reaches default UX.
