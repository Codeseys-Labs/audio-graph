# Plan B1: Cross-platform audio playback subsystem

**Goal:** Take the `TtsEvent::AudioChunk` stream from any `TtsProvider` and
play it through the user's selected output device on Linux + Windows
(macOS deferred). Cancel within 50ms on barge-in.

**ADR:** [0004](../adr/0004-tts-provider-trait-and-deepgram-aura.md) (this is the consumer side of the trait).

**Backlog:** audio-graph-8d75. Blocked by Wave A's plan A1 (TtsProvider trait must exist + Aura impl must produce real chunks).

**Status:** plan PROPOSED — full elaboration after Wave A merges and the
TtsEvent surface is empirically shaped (the trait may evolve in Wave A
review).

## Acceptance criteria (provisional)

- [ ] New module `src-tauri/src/playback/` with cpal-based player.
- [ ] Actor pattern: dedicated `std::thread` owns the cpal Stream;
  `tauri::State<AudioPlayerHandle>` holds a crossbeam-channel sender +
  AtomicBool cancel flag.
- [ ] Producer task (consumes TtsEvent::AudioChunk) writes to a SPSC
  ringbuf::Producer. Realtime callback on the cpal thread reads from
  ringbuf::Consumer.
- [ ] Cancel: AtomicBool flips → callback drains ring buffer → end-to-end
  audible cut ≤ 50ms.
- [ ] Resampler: rubato `FftFixedIn` in producer task, never in callback.
  24kHz (Aura default) → device's preferred rate (often 48kHz).
- [ ] Tauri commands: `start_playback`, `stop_playback`,
  `list_output_devices`, `set_output_device`.
- [ ] Tests: integration test pushes a known sine-wave PCM stream, verifies
  the realtime callback receives correct samples (use cpal's
  `assert_stream_send!` macro and a custom test backend).

## References

- `docs/research/audio-playback.md`
- `docs/research/verified-2026-05-19.md` (cpal section)
