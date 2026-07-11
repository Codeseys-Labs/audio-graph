# Architecture Decision Records

This index is regenerated from ADR files. New decisions use MADR 3.0; accepted records are immutable and are replaced only by superseding ADRs.

| # | Title | Status | Date |
|---|---|---|---|
| [ADR-0001](0001-parallel-realtime-pipeline.md) | Parallel Realtime Pipeline | accepted | (initial) |
| [ADR-0002](0002-openai-realtime-provider.md) | OpenAI Realtime Provider Family | accepted | (initial) |
| [ADR-0003](0003-speech-to-speech-agent-provider-matrix.md) | Speech-to-Speech Agent Provider Matrix | superseded by ADR-0006 | (initial) |
| [ADR-0004](0004-tts-provider-trait-and-deepgram-aura.md) | TtsProvider Trait + Deepgram Aura as Default Cloud TTS | accepted | 2026-05-19 |
| [ADR-0005](0005-openrouter-as-recommended-llm-endpoint.md) | OpenRouter as Recommended Cloud LLM Endpoint | accepted | 2026-05-19 |
| [ADR-0006](0006-streaming-chat-and-native-s2s-separation.md) | Streaming Chat with Token Deltas; Native-S2S Agents Are Sibling Surfaces | accepted | 2026-05-19 |
| [ADR-0007](0007-feature-gate-local-ml.md) | Gate local ML inference behind cargo feature flags | accepted | 2026-05-28 |
| [ADR-0008](0008-conversation-ontology.md) | Built-in conversation ontology for entity/relation extraction | accepted | 2026-05-28 |
| [ADR-0009](0009-design-token-system-and-theming.md) | Layered design-token system + theming | accepted | 2026-05-29 |
| [ADR-0010](0010-icon-system.md) | Icon system (lucide-react) replacing emoji iconography | accepted | 2026-05-29 |
| [ADR-0011](0011-unified-feedback-system.md) | Unified feedback / notification system | accepted | 2026-05-29 |
| [ADR-0012](0012-turn-gated-incremental-prefill-llama-cpp.md) | Adopt turn-gated incremental prefill on the local llama.cpp engine for entity extraction | accepted | 2026-05-29 |
| [ADR-0013](0013-conversation-modes.md) | Conversation modes — Notes/Graph vs Converse (native + pipelined S2S) | accepted | 2026-05-29 |
| [ADR-0014](0014-notes-synthesis.md) | On-demand notes synthesis (narrative parallel to the graph) | superseded by ADR-0024 | 2026-05-29 |
| [ADR-0015](0015-modularize-css-defer-tailwind.md) | Modularize App.css into per-component stylesheets; keep vanilla CSS (defer Tailwind/shadcn) | superseded by ADR-0016 | 2026-05-29 |
| [ADR-0016](0016-adopt-tailwind-v4-incremental.md) | Adopt Tailwind v4 (token-bridged, no Preflight) and migrate components incrementally | accepted | 2026-05-29 |
| [ADR-0017](0017-unbounded-speaker-diarization.md) | Unbounded speaker diarization via sherpa-onnx embedding + clustering | accepted | 2026-05-30 |
| [ADR-0018](0018-converse-turn-state-machine-and-half-duplex.md) | Provider-agnostic converse turn-state machine + backend-side half-duplex/AEC | accepted | 2026-05-30 |
| [ADR-0019](0019-credential-and-config-storage.md) | Credential And Config Storage Migration | proposed | 2026-06-25 |
| [ADR-0020](0020-processed-pcm-contract.md) | Adopt a Source-Aware Processed PCM Contract | accepted | 2026-07-09 |
| [ADR-0021](0021-storage-architecture.md) | Storage Architecture — File-Canonical Event Logs, DB Gated on Evidence | superseded by ADR-0027 | 2026-06-27 |
| [ADR-0022](0022-codec-decode-boundary.md) | Codec/Decode Boundary — Keep Realtime PCM Codec-Free; Adopt symphonia Only at the Fixture/Import Edge | accepted | 2026-06-28 |
| [ADR-0023](0023-anonymous-analytics-sentry-integration.md) | Anonymous Analytics — Raw Sentry Rust SDK over tauri-plugin-sentry | accepted | 2026-06-28 |
| [ADR-0024](0024-event-sourced-notes-graph-projections.md) | Event-sourced transcript → notes/graph projections (supersedes ADR-0014) | accepted | 2026-06-30 |
| [ADR-0025](0025-stt-llm-context-efficiency-and-diff-based-updates.md) | STT→LLM context efficiency + diff-based note/graph retroactive updates (extends ADR-0024) | proposed | 2026-07-04 |
| [ADR-0026](0026-session-timeline-who-said-what-when.md) | Session Timeline — "who said what when, in relation to what" (extends ADR-0024) | proposed | 2026-07-04 |
| [ADR-0027](0027-file-canonical-durable-session-store.md) | Adopt File-Canonical Durable Session Storage | accepted | 2026-07-09 |
| [ADR-0028](0028-separate-capture-lifecycle-from-foreground-workspace.md) | Separate Capture Lifecycle from Foreground Workspace | accepted | 2026-07-09 |
| [ADR-0029](0029-gate-rebuildable-query-indexes-on-measured-demand.md) | Gate Rebuildable Query Indexes on Measured Demand | accepted | 2026-07-09 |
| [ADR-0030](0030-organize-mvp-shell-around-ready-livenow-review-inspect.md) | Organize the MVP Shell Around Ready, LiveNow, Review, and Inspect | accepted | 2026-07-09 |
| [ADR-0031](0031-classify-projection-bases-as-current-append-only-or-revised.md) | Classify Projection Bases as Current, Append-Only, or Revised | accepted | 2026-07-09 |
| [ADR-0032](0032-layer-validation-evidence-by-claim.md) | Layer Validation Evidence by Claim | accepted | 2026-07-09 |
| [ADR-0033](0033-enforce-mvp-provider-enablement-at-content-start.md) | Enforce MVP Provider Enablement at Every Content-Bearing Start | accepted | 2026-07-09 |
| [ADR-0034](0034-require-exhaustive-evidence-for-negative-data-egress-claims.md) | Require Exhaustive Evidence for Negative Data-Egress Claims | accepted | 2026-07-10 |
| [ADR-0035](0035-define-canonical-log-v1-payload-commitments.md) | Define Canonical Log V1 Payload Commitments with Key-Canonical JSON | accepted | 2026-07-10 |
| [ADR-0036](0036-bind-uncertain-canonical-recovery-to-append-identity.md) | Bind Uncertain Canonical Recovery to the Expected Append Identity | accepted | 2026-07-10 |

## Status vocabulary

- `proposed`
- `accepted`
- `rejected`
- `deprecated`
- `superseded by ADR-NNNN`

## Adding a decision

Use MADR 3.0, record at least two considered options and one negative consequence, assign the next four-digit number, regenerate this index, and commit the ADR plus index together.
