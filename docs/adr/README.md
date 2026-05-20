# Architecture Decision Records

ADRs in this directory follow a lightweight MADR-inspired format. Each
records one architectural decision with its drivers, considered
alternatives, outcome, and consequences. ADRs are immutable after
acceptance — to change one, write a new ADR that supersedes it.

| #         | Title                                                                                       | Status                              | Date       |
| --------- | ------------------------------------------------------------------------------------------- | ----------------------------------- | ---------- |
| [0001]    | Parallel Realtime Pipeline                                                                  | accepted                            | (initial)  |
| [0002]    | OpenAI Realtime Provider Family                                                             | proposed                            | (initial)  |
| [0003]    | Speech-to-Speech Agent Provider Matrix                                                      | superseded in part by ADR-0006      | (initial)  |
| [0004]    | TtsProvider Trait + Deepgram Aura as Default Cloud TTS                                      | proposed                            | 2026-05-19 |
| [0005]    | OpenRouter as Recommended Cloud LLM Endpoint                                                | proposed                            | 2026-05-19 |
| [0006]    | Streaming Chat with Token Deltas; Native-S2S Agents Are Sibling Surfaces                    | proposed                            | 2026-05-19 |

[0001]: 0001-parallel-realtime-pipeline.md
[0002]: 0002-openai-realtime-provider.md
[0003]: 0003-speech-to-speech-agent-provider-matrix.md
[0004]: 0004-tts-provider-trait-and-deepgram-aura.md
[0005]: 0005-openrouter-as-recommended-llm-endpoint.md
[0006]: 0006-streaming-chat-and-native-s2s-separation.md

## Status legend

- `proposed` — recorded; awaiting team / user sign-off before implementation work begins.
- `accepted` — in force; implementations should follow it.
- `rejected` — considered and ruled out; kept for historical context.
- `deprecated` — no longer guides new work; not yet replaced.
- `superseded by ADR-NNNN` — replaced. Read the successor.

## Concept map

```
┌─────────────────────────────────────────────────────────────────┐
│  COMPOSED PIPELINE (audio in → graph/notes + chatbot replies)   │
│                                                                 │
│  STT (ADR-0001)  →  LLM (ADR-0005)  →  TTS (ADR-0004)           │
│                                          ↘ Audio playback       │
│                                          ↘ Graph annotator      │
│                                                                 │
│  Streaming events: ADR-0006                                     │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────┐
│  NATIVE-S2S AGENTS (audio in → audio out, single model)         │
│                                                                 │
│  Gemini Live  ·  OpenAI Realtime gpt-realtime-2 (ADR-0002)      │
│                                                                 │
│  Boundary against composed pipeline: ADR-0006                   │
│  Provider matrix: ADR-0003 (superseded in part by ADR-0006)     │
└─────────────────────────────────────────────────────────────────┘
```

## Adding a new ADR

1. Pick the next number from this index.
2. Copy the structure from a recent file.
3. Write status as `proposed`.
4. Update this README with the new entry (alphabetic / numeric order).
5. Commit the ADR + the README update in one commit:
   `docs(adr): add ADR-NNNN <title>`.
