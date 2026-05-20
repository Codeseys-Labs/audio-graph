# ADR-0004: TtsProvider Trait + Deepgram Aura as Default Cloud TTS

## Status

Accepted 2026-05-19 for phased implementation.

## Context

ADR-0003 names TTS as a first-class provider category for the speech-to-
speech agent personality, but no implementation exists. The chosen primary
pipeline (per user goal alignment 2026-05-19) is:

```
audio → Deepgram STT → OpenRouter LLM → Deepgram Aura TTS → playback
                          ↘ graph/notes branch
```

Deepgram Aura is the cloud TTS that pairs naturally with Deepgram STT — same
account, same auth, same WebSocket protocol style. The decision here is
not which TTS provider to ship first (Aura is the user's stated choice) but
**how to structure the trait** so future providers (Kokoro, Piper, Coqui,
OpenAI TTS, ElevenLabs, future Google native audio) plug in cleanly without
forcing each call site to know about provider-specific quirks.

The existing parallel is `src-tauri/src/asr/`, which has a `transcribe`
trait surface (different concrete files per provider — `deepgram.rs`,
`assemblyai.rs`, `aws_transcribe.rs`, `sherpa_streaming.rs`). The Aura
client should mirror Deepgram STT's shape: WebSocket session, reconnect
with backoff, normalized event emission.

## Decision Drivers

- Pipeline must support cancellation within 50ms (barge-in goal from
  audio-graph-8d75; Aura's `Clear` frame is the primary mechanism).
- Streaming, not request-response: TTS providers vary on whether they
  yield audio before the input text is complete. Aura does. Local engines
  (Piper, Coqui) typically do not. The trait must accept "buffered local"
  and "incremental cloud" as equally valid implementations.
- Cross-platform: Linux + Windows are the priority targets per goal;
  macOS is deferred but must not be excluded by the trait shape.
- Reuse credential infrastructure: Deepgram API keys are already stored
  in `credentials.yaml` and accessed by `asr/deepgram.rs`; TTS should
  share storage and auth, not introduce a separate keying surface.
- The trait must compose with the audio playback subsystem (audio-graph-
  8d75) without the caller threading PCM samples manually. PCM emission
  is the trait's job, not the caller's.

## Considered Options

- **Option A**: Single async-trait `TtsProvider` with `synthesize_stream`
  returning a `Stream<Item = TtsEvent>` where events include `AudioChunk`,
  `Status`, `Error`.
- **Option B**: Provider-specific actors with no shared trait — each
  provider gets its own Tauri command surface (`start_aura`, `start_kokoro`,
  etc.), no abstraction.
- **Option C**: Generic enum `TtsProviderConfig` that dispatches at runtime
  through a single concrete `TtsClient` struct that internally branches.
  No trait at all.

## Decision Outcome

Chosen option: **Option A** (`async_trait`-based `TtsProvider` trait).
Rationale: it matches the existing ASR provider shape, keeps each provider
in its own file (so a Piper rewrite doesn't risk Aura regressions), and
exposes a clean stream-of-events surface that the audio playback subsystem
can consume without provider-specific glue code.

### Consequences

- **Positive**: Each provider lives in its own file (`tts/deepgram_aura.rs`,
  later `tts/kokoro.rs`), unit-testable in isolation.
- **Positive**: Adding ElevenLabs, OpenAI TTS, Google native-audio TTS in
  the future is a new file + a Settings enum entry, nothing else.
- **Positive**: The audio playback subsystem (audio-graph-8d75) consumes
  `Stream<Item = TtsEvent>` — provider-agnostic.
- **Negative**: `async_trait` adds runtime dyn-dispatch overhead. Acceptable
  for our stream rate (~24 kHz audio, one event per ~10ms callback period).
- **Negative**: Trait surface ossifies once consumers depend on it. If a
  future provider needs a fundamentally different lifecycle (e.g.,
  pre-warmup with a model file), we'll need to extend the trait carefully
  to avoid breaking existing impls.
- **Neutral**: Settings enum gets a new `TtsProvider` variant alongside
  `AsrProvider` and `LlmProvider`; UI grows a TTS section in SettingsPage.

## Pros and Cons of the Options

### Option A: async-trait `TtsProvider`

- Good, because: matches existing ASR pattern (consistent codebase shape).
- Good, because: each provider is independently testable + replaceable.
- Good, because: the playback subsystem consumes `Stream<Item = TtsEvent>`
  without knowing the provider.
- Bad, because: `async_trait` dyn-dispatch overhead (negligible at our
  audio rate, but real).
- Bad, because: the trait shape locks in once shipped — hard to evolve
  without breaking changes.

### Option B: Provider-specific actors, no shared trait

- Good, because: each provider can have its absolutely-best-fit surface.
- Bad, because: every consumer (chat reply path, future S2S orchestrator,
  graph annotator) duplicates provider-switching logic.
- Bad, because: the tooling for "how do I add a new TTS provider?" becomes
  "rewrite N call sites".

### Option C: Generic enum `TtsProviderConfig` with internal branching

- Good, because: simplest possible code at the call site.
- Bad, because: `tts/mod.rs` becomes a god-module with all providers mixed
  in. Diff hygiene degrades.
- Bad, because: a Kokoro implementer would need to read the entire shared
  state machine to land their changes — high contributor friction.

## Implementation outline (informational; not part of the decision)

```rust
// src-tauri/src/tts/mod.rs
#[async_trait]
pub trait TtsProvider: Send + Sync {
    async fn open(&self, voice: &str, config: TtsConfig) -> Result<TtsSession, TtsError>;
}

pub trait TtsSession: Send {
    fn speak(&self, text: &str) -> Result<(), TtsError>;
    fn flush(&self) -> Result<(), TtsError>;
    fn clear(&self) -> Result<(), TtsError>;  // barge-in
    fn close(self) -> Result<(), TtsError>;
    fn events(&self) -> &mut dyn Stream<Item = TtsEvent>;
}

pub enum TtsEvent {
    AudioChunk { samples: Vec<i16>, sample_rate: u32 },
    Status(TtsStatus),
    Error(TtsError),
}
```

Concrete: `src-tauri/src/tts/deepgram_aura.rs` connects to
`wss://api.deepgram.com/v1/speak` with `Authorization: Token <key>`,
default voice `aura-asteria-en`, default `encoding=linear16`,
`sample_rate=24000`. (Per `docs/research/verified-2026-05-19.md`.)

## References

- `docs/research/verified-2026-05-19.md` — Aura protocol facts
- `docs/research/deepgram-aura-streaming-tts.md` — fuller protocol
  description (caveat: protocol details at Jan-2026-cutoff confidence)
- `docs/adr/0003-speech-to-speech-agent-provider-matrix.md` — names TTS
  as a first-class category but doesn't define the trait
- `src-tauri/src/asr/deepgram.rs` — sister module (54KB) whose shape this
  ADR follows
- audio-graph-3132 (seeds issue)
