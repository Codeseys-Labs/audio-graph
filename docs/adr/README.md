# Architecture Decision Records

New ADRs in this directory follow MADR 3.0 and record one architectural
decision with its drivers, considered alternatives, outcome, and consequences.
Earlier accepted records retain their historical formats. ADRs are immutable
after acceptance — to change one, write a new ADR that supersedes it.

| # | Title | Status | Rollout / relationship note | Date |
| --- | --- | --- | --- | --- |
| [0001] | Parallel Realtime Pipeline | accepted | — | (initial) |
| [0002] | OpenAI Realtime Provider Family | accepted | Wave A STT landed; broader family partial | (initial) |
| [0003] | Speech-to-Speech Agent Provider Matrix | superseded | Superseded in part by ADR-0006 | (initial) |
| [0004] | TtsProvider Trait + Deepgram Aura as Default Cloud TTS | accepted | — | 2026-05-19 |
| [0005] | OpenRouter as Recommended Cloud LLM Endpoint | accepted | — | 2026-05-19 |
| [0006] | Streaming Chat with Token Deltas; Native-S2S Agents Are Sibling Surfaces | accepted | Partially supersedes ADR-0003 | 2026-05-19 |
| [0007] | Gate Local ML Inference Behind Cargo Feature Flags | accepted | — | 2026-05-28 |
| [0008] | Built-in Conversation Ontology for Entity/Relation Extraction | accepted | Cloud path landed; broader rollout partial | 2026-05-28 |
| [0009] | Layered Design-Token System + Theming | accepted | — | 2026-05-29 |
| [0010] | Icon System (lucide-react) Replacing Emoji Iconography | accepted | — | 2026-05-29 |
| [0011] | Unified Feedback / Notification System | accepted | — | 2026-05-29 |
| [0012] | Turn-Gated Incremental Prefill on the Local llama.cpp Engine for Entity Extraction | accepted | Phase 0a complete | 2026-05-29 |
| [0013] | Conversation Modes — Notes/Graph vs Converse (native + pipelined S2S) | accepted | — | 2026-05-29 |
| [0014] | On-demand Notes Synthesis (narrative parallel to the graph) | superseded | Superseded by ADR-0024; further narrowed 2026-08-23 (bento document tile drops ontology sections + inline synthesize output, ticket W5) | 2026-05-29 |
| [0015] | Modularize App.css into per-component stylesheets; defer Tailwind/shadcn | superseded | Superseded by ADR-0016 | 2026-05-29 |
| [0016] | Adopt Tailwind v4 (token-bridged, no Preflight) and migrate components incrementally | accepted | 2026-06-29 "two channels only" clause amended by ADR-0047 (third channel: `.ag-*` recipes) | 2026-05-29 |
| [0017] | Unbounded Speaker Diarization via sherpa-onnx Embedding + Clustering | accepted | Engine, worker, downloads, and pipeline wiring landed; multi-speaker accuracy gate pending | 2026-05-30 |
| [0018] | Provider-agnostic Converse Turn-State Machine + Backend-side Half-duplex/AEC | accepted | Supersedes interim echo guard `172edbf` | 2026-05-30 |
| [0019] | Credential And Config Storage Migration | proposed | — | 2026-06-25 |
| [0020] | Adopt a Source-Aware Processed PCM Contract | accepted | — | 2026-07-09 |
| [0021] | Storage Architecture — File-Canonical Event Logs, DB Gated on Evidence | superseded | Superseded by ADR-0027 | 2026-06-27 |
| [0022] | Codec/Decode Boundary — Keep Realtime PCM Codec-Free; symphonia Only at the Fixture/Import Edge | accepted | symphonia adoption gated on first import consumer; relates to ADR-0020, ADR-0004, ADR-0007 | 2026-06-28 |
| [0023] | Anonymous Analytics — Raw Sentry Rust SDK over tauri-plugin-sentry | accepted | Opt-in and PII-off; WebView JS capture and sourcemap upload deferred; relates to ADR-0019 | 2026-06-28 |
| [0024] | Event-sourced transcript → notes/graph projections | accepted | Supersedes ADR-0014; relates to ADR-0021, ADR-0008, ADR-0012; live document tile (ticket W5, 2026-08-23) renders `LiveDocument` over this projection with no ontology sections | 2026-06-30 |
| [0025] | STT→LLM context efficiency + diff-based note/graph retroactive updates | proposed | Extends ADR-0024; relates to ADR-0023 and ADR-0017; epic `d7bb` | 2026-07-04 |
| [0026] | Session timeline — who said what when, in relation to what | proposed | Extends ADR-0024; relates to ADR-0025 and ADR-0017; epic `0d72` | 2026-07-04 |
| [0027] | Adopt File-Canonical Durable Session Storage | accepted | Supersedes ADR-0021 | 2026-07-09 |
| [0028] | Separate Capture Lifecycle from Foreground Workspace | accepted | Finalization arm narrowed by ADR-0035 | 2026-07-09 |
| [0029] | Gate Rebuildable Query Indexes on Measured Demand | accepted | — | 2026-07-09 |
| [0030] | Organize the MVP Shell Around Ready, LiveNow, Review, and Inspect | accepted | Structure amended by ADR-0046 | 2026-07-09 |
| [0031] | Classify Projection Bases as Current, Append-Only, or Revised | accepted | — | 2026-07-09 |
| [0032] | Layer Validation Evidence by Claim | accepted | — | 2026-07-09 |
| [0033] | Enforce MVP Provider Enablement at Every Content-Bearing Start | accepted | — | 2026-07-09 |
| [0034] | Require Exhaustive Evidence for Negative Data-Egress Claims | accepted | — | 2026-07-10 |
| [0035] | Record Post-Stop Finalization Failure as Per-Session Finalization Blocked | accepted | Narrows ADR-0028; decided under maintainer delegation | 2026-08-17 |
| [0036] | Derive Session Finalization State from Durable Barriers | accepted | Implementation blocked on 90f3/8e73; maintainer-decided via wayfinder grilling | 2026-08-18 |
| [0037] | Admit Session Memory Items Through a Layered Claim-Class Evidence Table | accepted | Extends ADR-0034 discipline to knowledge claims; validator ships after ADR-0038's fallback removal | 2026-08-18 |
| [0038] | Route LLM Operations Through a Single-Skin Named Route Table | accepted | Fallback removal precedes ADR-0037's validator | 2026-08-18 |
| [0039] | Let a Session Abandon Its Own Staged Manifest Temporary | accepted | Refines ADR-0044 decision item 3's temporary-removal ownership scope; supersedes none; seed `audio-graph-3cf2` | 2026-08-18 |
| [0040] | Select the Checked-Open Branch from Observed Control-Plane Absence | accepted | Narrows ADR-0044 item 5's branch selection; **expires when a v2 writer is activated**; seed `audio-graph-e8e7` | 2026-08-19 |
| [0041] | Keep Versioned Speech Revisions in One Canonical Transcript Stream | accepted | Refines ADR-0024 and ADR-0027; relates to ADR-0031 and ADR-0042 | 2026-08-14 |
| [0042] | Version Projection Basis Hashes by Speech-Revision Semantics | accepted | Refines ADR-0031; depends on ADR-0024, ADR-0027, and accepted ADR-0041 | 2026-08-14 |
| [0043] | Freeze the Canonical Event Stream Registry | accepted | Refines ADR-0027; freezes canonical event-domain IDs and outer schema v1 | 2026-07-10 |
| [0044] | Keep the Session Control Plane in the Flat Artifact Root | accepted | Refines ADR-0027 and ADR-0041; relates to ADR-0043; item 3 scoped by ADR-0039 and item 5 narrowed by ADR-0040; supersedes none | 2026-08-16 |
| [0045] | Schedule Projections on Per-Kind Coalescing Slots with Derived Coverage Heads | accepted | Refines ADR-0024, ADR-0027, ADR-0031; relates to ADR-0029/0035/0036/0042; maintainer-decided via grilling; seed `audio-graph-3b48` | 2026-08-20 |
| [0046] | Collapse the Shell to Capture and Sessions with a Persistent Active-Session Strip | accepted | Amends ADR-0030's workspace structure; preserves ADR-0028/0033/0034; ratified 2026-08-20 via the design panel; seed `audio-graph-19c7` | 2026-08-20 |
| [0047] | A Tier-3 `.ag-*` Recipe Layer in `@layer components`, and `--edge`/`--edge-subtle` Non-Text Contrast Tokens | accepted | Amends ADR-0016's 2026-06-29 "two channels only" convention clause; extends ADR-0009's surface/border tokens; seed `audio-graph-b9dc` | 2026-08-20 |

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
[0019]: 0019-credential-and-config-storage.md
[0020]: 0020-processed-pcm-contract.md
[0021]: 0021-storage-architecture.md
[0022]: 0022-codec-decode-boundary.md
[0023]: 0023-anonymous-analytics-sentry-integration.md
[0024]: 0024-event-sourced-notes-graph-projections.md
[0025]: 0025-stt-llm-context-efficiency-and-diff-based-updates.md
[0026]: 0026-session-timeline-who-said-what-when.md
[0027]: 0027-file-canonical-durable-session-store.md
[0028]: 0028-separate-capture-lifecycle-from-foreground-workspace.md
[0029]: 0029-gate-rebuildable-query-indexes-on-measured-demand.md
[0030]: 0030-organize-mvp-shell-around-ready-livenow-review-inspect.md
[0031]: 0031-classify-projection-bases-as-current-append-only-or-revised.md
[0032]: 0032-layer-validation-evidence-by-claim.md
[0033]: 0033-enforce-mvp-provider-enablement-at-content-start.md
[0034]: 0034-require-exhaustive-evidence-for-negative-data-egress-claims.md
[0035]: 0035-record-post-stop-finalization-failure-as-per-session-finalization-blocked.md
[0036]: 0036-derive-session-finalization-state-from-durable-barriers.md
[0037]: 0037-admit-session-memory-items-through-a-layered-claim-class-evidence-table.md
[0038]: 0038-route-llm-operations-through-a-single-skin-named-route-table.md
[0039]: 0039-let-a-session-abandon-its-own-staged-manifest-temporary.md
[0040]: 0040-select-the-checked-open-branch-from-observed-control-plane-absence.md
[0041]: 0041-keep-versioned-speech-revisions-in-one-canonical-stream.md
[0042]: 0042-version-projection-basis-hashes-by-speech-semantics.md
[0043]: 0043-freeze-canonical-event-stream-registry.md
[0044]: 0044-keep-session-control-plane-in-the-flat-artifact-root.md
[0045]: 0045-schedule-projections-on-per-kind-coalescing-slots-with-derived-coverage-heads.md
[0046]: 0046-collapse-shell-to-capture-and-sessions.md
[0047]: 0047-tier-3-recipe-layer-and-non-text-contrast-edge-tokens.md

> **ADR-0043 lineage warning:** The immutable restored ADR was authored in a
> non-ancestor canonical-log lineage. Its internal ADR-0035 and ADR-0036
> references are historical and do not name the current speech-semantics
> records, which are ADR-0041 and ADR-0042 (numbered 0035 and 0036 before the
> renumbering below). See the
> [Wave 7B durability plan](../agentic-runs/2026-08-14-session-memory-continuation/audio-graph-8e73-wave7b-plan.md)
> for the exact blob lineage and implementation boundary.

> **Renumbering note (seed `audio-graph-c306`, 2026-08-18):** this branch and
> master independently issued numbers 0035-0038 to four entirely different
> records. Per the convention that the later-merged branch renumbers, and
> because master is trunk and merges first, this branch's four records moved:
>
> | Was | Now | Record |
> | --- | --- | --- |
> | 0035 | [0041] | Keep Versioned Speech Revisions in One Canonical Transcript Stream |
> | 0036 | [0042] | Version Projection Basis Hashes by Speech-Revision Semantics |
> | 0037 | [0043] | Freeze the Canonical Event Stream Registry |
> | 0038 | [0044] | Keep the Session Control Plane in the Flat Artifact Root |
>
> ADR-0039 and ADR-0040 never collided and kept their numbers. Renumbering
> changed each file's name and its `# ADR-NNNN:` heading, and therefore its Git
> blob hash. Two consequences for readers of older documents:
>
> - Run records under `docs/agentic-runs/` keep their recorded blob hashes and
>   the pre-renumber paths inside fenced evidence blocks, because those blocks
>   reproduce commands and output as they actually ran. Their prose names each
>   record by its current number.
> - ADR-0043 is immutable archival content restored from a non-ancestor
>   canonical-log lineage. Its internal ADR-0035 and ADR-0036 references were
>   deliberately left alone; see the lineage warning above.

## Status legend

- `proposed` — recorded; awaiting team / user sign-off before implementation work begins.
- `accepted` — in force; implementations should follow it.
- `rejected` — considered and ruled out; kept for historical context.
- `deprecated` — no longer guides new work; not yet replaced.
- `superseded` — replaced. Read the successor named in the rollout / relationship note.

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
