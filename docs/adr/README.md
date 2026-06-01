# Architecture Decision Records

ADRs in this directory follow a lightweight MADR-inspired format. Each
records one architectural decision with its drivers, considered
alternatives, outcome, and consequences. ADRs are immutable after
acceptance — to change one, write a new ADR that supersedes it.

| #         | Title                                                                                       | Status                              | Date       |
| --------- | ------------------------------------------------------------------------------------------- | ----------------------------------- | ---------- |
| [0001]    | Parallel Realtime Pipeline                                                                  | accepted                            | (initial)  |
| [0002]    | OpenAI Realtime Provider Family                                                             | accepted; partial (Wave A STT landed) | (initial)  |
| [0003]    | Speech-to-Speech Agent Provider Matrix                                                      | superseded in part by ADR-0006      | (initial)  |
| [0004]    | TtsProvider Trait + Deepgram Aura as Default Cloud TTS                                      | accepted                            | 2026-05-19 |
| [0005]    | OpenRouter as Recommended Cloud LLM Endpoint                                                | accepted                            | 2026-05-19 |
| [0006]    | Streaming Chat with Token Deltas; Native-S2S Agents Are Sibling Surfaces                    | accepted                            | 2026-05-19 |
| [0007]    | Gate Local ML Inference Behind Cargo Feature Flags                                          | accepted                             | 2026-05-28 |
| [0008]    | Built-in Conversation Ontology for Entity/Relation Extraction                               | accepted; partial (cloud only)      | 2026-05-28 |
| [0009]    | Layered Design-Token System + Theming                                                       | accepted                            | 2026-05-29 |
| [0010]    | Icon System (lucide-react) Replacing Emoji Iconography                                       | accepted                            | 2026-05-29 |
| [0011]    | Unified Feedback / Notification System                                                       | accepted                            | 2026-05-29 |
| [0012]    | Turn-Gated Incremental Prefill on the Local llama.cpp Engine for Entity Extraction           | accepted (Phase 0a done)            | 2026-05-29 |
| [0013]    | Conversation Modes — Notes/Graph vs Converse (native + pipelined S2S)                        | accepted                            | 2026-05-29 |
| [0014]    | On-demand Notes Synthesis (narrative parallel to the graph)                                  | accepted                            | 2026-05-29 |
| [0015]    | Modularize App.css into per-component stylesheets; defer Tailwind/shadcn                     | superseded by ADR-0016              | 2026-05-29 |
| [0016]    | Adopt Tailwind v4 (token-bridged, no Preflight) and migrate components incrementally         | accepted                            | 2026-05-29 |
| [0017]    | Unbounded Speaker Diarization via sherpa-onnx Embedding + Clustering                          | accepted; engine+worker+downloads+pipeline-wiring landed and model-validated, multi-speaker accuracy gate pending | 2026-05-30 |
| [0018]    | Provider-agnostic Converse Turn-State Machine + Backend-side Half-duplex/AEC                  | accepted; supersedes the interim echo guard (172edbf) | 2026-05-30 |

[0001]: 0001-parallel-realtime-pipeline.md
[0002]: 0002-openai-realtime-provider.md
[0003]: 0003-speech-to-speech-agent-provider-matrix.md
[0004]: 0004-tts-provider-trait-and-deepgram-aura.md
[0005]: 0005-openrouter-as-recommended-llm-endpoint.md
[0006]: 0006-streaming-chat-and-native-s2s-separation.md
[0007]: 0007-feature-gate-local-ml.md
[0008]: 0008-conversation-ontology.md
[0009]: 0009-design-token-system-and-theming.md
[0010]: 0010-icon-system.md
[0011]: 0011-unified-feedback-system.md
[0012]: 0012-turn-gated-incremental-prefill-llama-cpp.md
[0013]: 0013-conversation-modes.md
[0014]: 0014-notes-synthesis.md
[0015]: 0015-modularize-css-defer-tailwind.md
[0016]: 0016-adopt-tailwind-v4-incremental.md
[0017]: 0017-unbounded-speaker-diarization.md
[0018]: 0018-converse-turn-state-machine-and-half-duplex.md

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
