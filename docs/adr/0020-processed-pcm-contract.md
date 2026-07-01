# ADR-0020: Processed PCM And Timing Contract

## Status

proposed

## Context

AudioGraph captures audio through `rsac`, then fans processed audio out to ASR,
diarization, realtime agents, playback-adjacent voice flows, and projection
metadata. Provider expansion makes the audio boundary a product contract rather
than an implementation detail: Soniox, Deepgram, AssemblyAI, AWS Transcribe,
OpenAI Realtime, Speechmatics, Gladia, Google/Azure enterprise adapters, and
local runtimes all need to agree on sample format, source identity, and timing.

## Decision

The backend processed-audio bus emits one canonical shape:

- sample type: normalized mono `f32`, finite samples only, nominal range
  `[-1.0, 1.0]`
- sample rate: `16_000` Hz
- channels: `1`
- full chunk size: `512` frames, which is `32 ms`
- final flush chunk: may contain fewer than `512` frames
- source identity: `source_id` is the stable capture-source id for per-source
  streams; explicitly mixed streams use the mixer-owned synthetic source id
- timestamp: elapsed time from the capture session start, attached to the start
  of the current processed window. The pipeline advances each emitted chunk's
  timestamp by `num_frames / sample_rate`; transcript/projection layers should
  treat provider timestamps as transcript-relative and keep this capture
  timestamp as provenance, not as a replacement for provider word timings

The Rust owner for this contract is
`src-tauri/src/audio/pipeline.rs`:

- `PROCESSED_AUDIO_SAMPLE_RATE_HZ`
- `PROCESSED_AUDIO_CHANNELS`
- `PROCESSED_AUDIO_CHUNK_FRAMES`
- `PROCESSED_AUDIO_CHUNK_DURATION_MS`
- `ProcessedAudioChunk::matches_processed_audio_contract`
- `processed_audio_duration`

Provider adapters convert from this bus format into their required wire/runtime
formats. Headerless signed 16-bit little-endian PCM conversion is owned by
`src-tauri/src/audio/pcm.rs`; provider modules should call that helper instead
of copying their own scaling and clamping logic.

The provider registry must declare both sides of the boundary:

- `pipeline_format`: the canonical processed-audio bus format
- `provider_format`: the provider/runtime format after adapter conversion
- `transport_encoding`: binary WebSocket, JSON/base64, AWS event stream, local
  buffer, gRPC, SDK-native, multipart WAV, etc.
- `adapter_resamples`: true only when the provider format changes sample rate
- `supports_multichannel`: false until the adapter actually preserves channel
  semantics

Playback is intentionally outside the ASR processed-audio bus. TTS providers
emit playback-oriented `i16` PCM at the provider voice sample rate, and the
playback subsystem owns device-format negotiation and output resampling work.

## Consequences

Positive:

- provider adapters no longer invent ad hoc PCM scaling or sample-rate claims.
- registry metadata can drive provider UI and CI without loading the full Tauri
  app.
- diarization and projections have a stable source/timing basis while provider
  word timings remain provider-owned.
- local and cloud providers can be tested against the same chunk shape.

Negative:

- providers that prefer 24 kHz or compressed formats need an explicit adapter
  stage.
- multi-channel diarization/channel attribution is not represented by the
  current processed-audio bus; it needs a separate explicit contract before
  `supports_multichannel` can become true.
- current chunk timestamps describe the accumulation start and do not encode
  every sample's absolute time. Consumers that need exact sample clocks must
  derive them from `num_frames`, `sample_rate`, and per-source ordering.

## Acceptance Tests

- pipeline tests assert sample rate, channel count, chunk duration, source id,
  timestamp, finite samples, and final-remainder behavior.
- PCM helper tests assert full-scale mapping, clamping, NaN/inf handling, and
  little-endian output.
- provider registry tests assert every audio-capable descriptor uses the
  canonical pipeline format and correctly marks adapter resampling.
- provider-specific PCM tests continue to exercise the shared conversion helper
  through adapter-local call sites.

## References

- `src-tauri/src/audio/pipeline.rs`
- `src-tauri/src/audio/pcm.rs`
- `src-tauri/src/audio/consumer.rs`
- `src-tauri/src/audio/mixer.rs`
- `src-tauri/crates/provider-registry/src/lib.rs`
- `src-tauri/src/asr/deepgram.rs`
- `src-tauri/src/asr/assemblyai.rs`
- `src-tauri/src/asr/aws_transcribe.rs`
- `src-tauri/src/asr/openai_realtime.rs`
