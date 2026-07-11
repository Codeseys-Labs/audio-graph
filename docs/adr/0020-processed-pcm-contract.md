---
status: accepted
date: 2026-07-09
deciders: [AudioGraph maintainers]
---

# ADR-0020: Adopt a Source-Aware Processed PCM Contract

## Context and Problem Statement

AudioGraph captures audio through `rsac` and fans processed samples out to
ASR, diarization, realtime agents, and projection provenance. Provider
expansion makes this boundary a product contract: every consumer needs a
stable sample shape, source identity, and timing model.

A format-only chunk with one elapsed-time field cannot explain producer loss,
subscriber overflow, device reset, sample-rate changes, or independently
clocked sources. Manufacturing a contiguous timeline after any of those events
silently shifts later speech earlier and makes restored transcripts and graph
evidence untrustworthy.

## Decision Drivers

- Give every audio-capable provider one testable backend-owned input contract.
- Preserve source time and loss instead of inferring continuity from arrival.
- Map multiple independent source clocks deterministically into one session.
- Keep provider word timing distinct from capture provenance.
- Keep cloud and local adapter conversion out of capture ownership.
- Avoid channel or timing claims the current pipeline cannot actually preserve.

## Considered Options

- Source-aware normalized processed PCM with explicit clock mappings
- Format-only processed PCM with a contiguous elapsed-time accumulator
- Preserve each provider's preferred native format through the whole pipeline

## Decision Outcome

Chosen option: "Source-aware normalized processed PCM with explicit clock
mappings", because it provides one portable processing boundary while retaining
the provenance needed to detect loss, resets, and multi-source alignment.

The backend processed-audio bus emits:

- normalized, finite, mono `f32` samples in the nominal range `[-1.0, 1.0]`
- `16_000` Hz, one channel
- full chunks of `512` frames (`32 ms`) and a shorter final flush chunk
- a stable capture `source_id`, or a mixer-owned synthetic id for an explicit
  mixed stream
- a stable `source_clock_id` for each source generation and the rsac
  source-position timestamp of the first source frame represented by the chunk
- an explicit mapping from each source generation to monotonic session time,
  with both source position and mapped session position on every chunk
- a content-free discontinuity before a chunk when producer, subscriber, queue,
  device-reset, or rate-change loss breaks continuity; the record includes its
  reason, generation, and a known or unknown dropped-frame count

Rate changes create a new source-to-session mapping and preserve or explicitly
account for the pending resampler tail. They never rescale earlier chunks.
Independent sources retain independent clocks and mappings; arrival order does
not become a synthetic shared source clock.

Provider word timestamps remain transcript-relative. Source and mapped session
time are capture provenance and never replace provider word timing.

`src-tauri/src/audio/pipeline.rs` owns the contract constants and
`ProcessedAudioChunk` schema. Provider adapters convert from this bus into
their wire or runtime formats and use `src-tauri/src/audio/pcm.rs` for signed
16-bit little-endian PCM conversion. Provider registry descriptors declare the
pipeline format, provider format, transport encoding, resampling, and proven
channel support.

Playback stays outside the ASR processed-audio bus. TTS output keeps its
playback-oriented sample rate, and the playback subsystem owns device
negotiation and output resampling.

### Consequences

- **Positive**: Capture loss and source resets remain visible instead of
  time-compressing later speech.
- **Positive**: Every provider can be tested against the same processed chunk
  shape and shared PCM conversion.
- **Positive**: Independently clocked sources can be joined through explicit
  mappings rather than assumed arrival order.
- **Negative**: The processed chunk schema grows clock, generation, mapping,
  and discontinuity metadata that every consumer must handle.
- **Negative**: Providers that prefer 24 kHz, compressed audio, or multiple
  channels need explicit adapters or a later contract.
- **Negative**: Multi-source alignment and rate-transition tests add
  cross-platform capture-test cost.
- **Neutral**: Provider word timestamps and capture provenance remain separate
  timing domains.

## Pros and Cons of the Options

### Source-aware normalized processed PCM with explicit clock mappings

- Good, because one backend-owned format prevents adapter-specific capture
  assumptions.
- Good, because source position, session mapping, and discontinuities survive
  resampling and replay.
- Good, because independent sources do not pretend to share one hardware clock.
- Bad, because all consumers must migrate from the current single timestamp.
- Bad, because the app owns mapping and discontinuity semantics.

### Format-only processed PCM with a contiguous elapsed-time accumulator

- Good, because it is the smallest payload and matches the current accumulator.
- Good, because consumers can treat chunks as a simple continuous stream.
- Bad, because any dropped or reset frames silently alter the meaning of time.
- Bad, because multiple independent sources cannot be aligned honestly.

### Preserve each provider's preferred native format through the whole pipeline

- Good, because some providers avoid an adapter resample.
- Good, because provider-native features could remain available.
- Bad, because capture, diarization, and projection consumers lose one shared
  contract.
- Bad, because format and timing behavior fragment across providers and make
  provider fallback unsafe.

## More Information

Implementation is tracked by `audio-graph-b718` and `audio-graph-99ed`.
Validation must cover source and mapped-session monotonicity, explicit induced
loss, final remainders, 48 kHz to 44.1 kHz to 48 kHz transitions, two-source
mapping, finite-sample enforcement, full-scale PCM conversion, and every
audio-capable provider descriptor.

Relevant code:

- `src-tauri/src/audio/pipeline.rs`
- `src-tauri/src/audio/consumer.rs`
- `src-tauri/src/audio/mixer.rs`
- `src-tauri/src/audio/pcm.rs`
- `src-tauri/crates/provider-registry/src/lib.rs`
