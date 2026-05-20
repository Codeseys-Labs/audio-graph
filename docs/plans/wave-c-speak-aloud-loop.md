# Plan C1: Speak-aloud loop — wire chat token deltas to TTS

**Goal:** When the user enables "speak aloud" in settings, route the
`chat-token-delta` events from plan A3 into the TtsProvider from plan A1,
flushed through the audio playback subsystem from plan B1.

**ADR:** [0006](../adr/0006-streaming-chat-and-native-s2s-separation.md) (sub-decision A).

**Backlog:** audio-graph-92c7. Blocked by A1 + A3 + B1.

**Status:** plan PROPOSED — full elaboration after Waves A and B merge.

## Acceptance criteria (provisional)

- [ ] Settings flag `chat.speak_aloud: bool` (default `false`).
- [ ] When enabled and a chat reply is in flight: delta events accumulate
  in a clause-boundary buffer; on `,` `;` `—` or `.`, flush the buffer to
  TtsProvider via Speak + Flush.
- [ ] On `chat-token-done`: final flush + Close.
- [ ] On `cancel_streaming_chat`: also call TtsSession::Clear.
- [ ] UI: speaker icon next to chatbot replies; click to replay last
  reply (re-feeds the recorded transcript through TTS — separate request,
  not the original one).
- [ ] Latency goal: first TTS audio out within 500ms of chat command
  invocation, measured end-to-end on Linux + Windows. Document actual
  numbers in the PR.

## References

- All three Wave A plans
- ADR-0006
- `docs/research/chat-tts-integration-map.md`
