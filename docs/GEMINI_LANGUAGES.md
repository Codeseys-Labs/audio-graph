# Gemini Live: Multi-Language Support

Brief user-facing reference for what works, what doesn't, and where quality
drops off when using Gemini Live (`gemini-3.1-flash-live-preview`) for
transcription in AudioGraph.

---

## 1. Supported languages

Gemini 3.1 Flash Live supports a broad set of spoken languages for audio
input. The list changes as Google ships updates, so rather than duplicating
it here (and going stale), consult Google's official documentation:

- **Model page:** <https://ai.google.dev/gemini-api/docs/models>
- **Live API supported languages:**
  <https://ai.google.dev/gemini-api/docs/live>

Any language Google lists as supported for the Live API should work as an
audio input to AudioGraph.

---

## 2. Can I hint or pin a language?

**No — not today.** AudioGraph does not expose a language hint for Gemini.
Audio is streamed in and Gemini auto-detects the language.

This is visible in the code:

- `src-tauri/src/settings/mod.rs` — `GeminiSettings` has exactly two fields:
  `auth` (API key or Vertex) and `model`. No `language_code`, no `hint`.
- `src-tauri/src/gemini/mod.rs` — the `BidiGenerateContentSetup` message
  sent on connect (see `build_setup_message`) contains:
  ```json
  {
    "setup": {
      "model": "models/gemini-3.1-flash-live-preview",
      "generationConfig": {
        "responseModalities": ["TEXT"],
        "inputAudioTranscription": {}
      }
    }
  }
  ```
  `inputAudioTranscription` is passed as an empty object, which tells
  Gemini to transcribe with its own defaults. No language code is plumbed
  through.

For comparison, AudioGraph **does** plumb a language code for AWS
Transcribe (see `AsrProvider::AwsTranscribe { language_code, … }` in
`settings/mod.rs`, which defaults to `en-US`) — Gemini just hasn't been
wired up yet.

**Future work.** A `language_code` field on `GeminiSettings` that gets
injected into `inputAudioTranscription.languageCode` at setup is the
expected extension point. Track this against the open items in
`docs/reviews/gap-analysis.md` before starting work.

---

## 3. Quality caveats on non-English input

Even though Gemini itself handles many languages, the rest of the
AudioGraph pipeline is partially English-tuned. Expect the following
degradations on non-English audio:

### Transcription quality: good

Gemini Live does the transcription directly, so quality tracks Google's
model quality for that language. No AudioGraph-side issue.

### Diarization: mixed

AudioGraph's speaker diarization is driven by acoustic features (not
language), so it is mostly language-agnostic. Cloud ASR providers
(Deepgram, AssemblyAI, AWS) may have language-specific diarization quality;
Gemini Live does not currently expose diarization labels, so with Gemini
you get speaker-agnostic transcript only.

### Entity extraction: English-biased

The extraction layer is where non-English falls off most visibly:

- **Rule-based fallback** (`src-tauri/src/graph/extraction.rs`,
  `RuleBasedExtractor`) uses English-only heuristics:
  - Capitalized-word sequences to detect names (doesn't match scripts
    without case: Arabic, Chinese, Japanese, Korean, Thai, …).
  - English company suffixes (`Inc`, `Corp`, `Ltd`, `LLC`, `Company`,
    `Technologies`, …).
  - English prepositions for locations (`in`, `at`, `from`, `near`,
    `based in`).
  - Will largely return empty on non-English transcripts.
- **LLM-based extraction** (`src-tauri/src/llm/engine.rs`,
  `src-tauri/src/llm/mistralrs_engine.rs`) sends the transcript to the
  configured LLM with an English instruction prompt (`"Extract entities
  and relationships from this conversation segment."`). A capable
  multilingual LLM (e.g. a recent Llama 3 or an API model like GPT-4 /
  Claude) will still produce sensible entities in any language it knows,
  but:
  - Small local models (the default `ggml-small-extract.gguf`) may be
    English-tuned and underperform.
  - The prompt itself is in English, which can bias the model to translate
    or under-extract non-Latin-script content.

**Workaround for now:** use an API-based LLM provider
(`LlmProvider::Api` pointing at an OpenAI-compatible endpoint, or
`LlmProvider::AwsBedrock`) with a strong multilingual model when working
primarily in a non-English language.

---

## 4. How to verify multilingual behavior

Manual smoke test with a Portuguese (or other non-English) audio sample:

1. Configure Gemini Live in Settings → enter your API key → select
   `gemini-3.1-flash-live-preview` as the model.
2. Pick an audio source. For a reproducible test on macOS/Linux/Windows,
   use a virtual loopback device (e.g. VB-CABLE on Windows, BlackHole on
   macOS, a `pw-loopback` node on Linux) so you can play a known file into
   AudioGraph.
3. Play a Portuguese sample (any clear speech clip works — news broadcast,
   podcast excerpt).
4. Observe in the UI:
   - **Live transcript** — should show Portuguese text, not translated.
     Confirms Gemini auto-detected the language correctly.
   - **Knowledge graph** — expect sparse entities if the LLM provider is
     a small local English-tuned model; expect reasonable entities if the
     LLM provider is a capable multilingual API model.
   - **Speaker labels** — Gemini doesn't emit diarization, so all
     segments will share a speaker label (or none).
5. Repeat with English as a control to compare extraction density.

To inspect what Gemini actually sent over the wire, raise the log level:

```bash
RUST_LOG=audio_graph::gemini=debug bun run tauri dev
```

The `wait_for_setup_complete` path logs pre-setup messages at `debug`, and
the session task logs transcription frames. That will confirm the absence
of any language field in the setup message.
