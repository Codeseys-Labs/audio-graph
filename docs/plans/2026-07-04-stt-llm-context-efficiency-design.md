# STT → LLM Context-Efficiency & Structured-Update Design

> **ADR:** the architectural decisions in this doc are recorded in
> [ADR-0025](../adr/0025-stt-llm-context-efficiency-and-diff-based-updates.md)
> (proposed; extends ADR-0024). **Epic:** `audio-graph-d7bb` (9 child seeds, one per pillar).

Date: 2026-07-04
Status: proposal / design (not yet a plan of record)
Scope: how live STT becomes coherent LLM-ready units, how those units feed the notes/graph LLM efficiently, and how notes + knowledge-graph updates become diff-based / supersede-aware.
Related ADRs: 0024 (event-sourced notes/graph projections — design of record), 0012 (turn-gated incremental prefill), 0017 (unbounded speaker diarization), 0008 (conversation ontology).

> This document is grounded in the current tree (`fix/gtk-test-harness-65f0`). Every recommendation names the file/seam it touches. The load-bearing thesis: **most of what we need already exists as the diarization-retcon substrate** (one `ProjectionOperation` enum, one append-only patch log, one `ProjectionBasis` staleness gate, one bitemporal `valid_until` model, `supersede_entity`). The work is to (a) stop feeding the LLM the whole transcript every turn, and (b) extend the retcon substrate that already covers the graph to also cover notes and speaker re-attribution inside notes.

---

## 1. Problem & current state

### 1.1 How audio becomes transcript today

Two regimes coexist, selected by provider (`speech/mod.rs` ~2903–3082):

- **Local Whisper = batched.** `AudioAccumulator` (`speech/mod.rs:2711-2805`) buffers 32 ms `ProcessedAudioChunk`s into ~2 s windows (`TARGET_FRAMES = 16_000*2`, `OVERLAP_FRAMES = 16_000/2`, `speech/mod.rs:918-923`) and runs Whisper `full()` once per window (`asr/mod.rs:286-368`). All output is effectively final.
- **Cloud streaming (Deepgram etc.) = word-by-word.** `run_deepgram_speech_processor` forwards each raw 32 ms chunk with **no accumulation** (`speech/mod.rs:4467-4471`, comment "no accumulation needed"). Partial vs final is split in `run_deepgram_event_receiver` (`speech/mod.rs:4585-4711`): `!is_final` → `emit_asr_partial_with_meta` then `continue` (no persist/diarize/extract); `is_final` → `emit_transcript_and_extract_with_meta` (persist + coalesce + graph).

**There is no app-owned VAD/endpointer or sentence segmentation.** `aec_vad/mod.rs` is a fixture-only scaffold explicitly not wired (`mod.rs:1-8`, seed `098b`); `pipeline.rs:141` passes every chunk through. Endpointing/turns are entirely provider-side (Nova `endpointing`/`utterance_end_ms`/`vad_events`; Flux `eot_threshold`/`eager_eot_threshold`/`eot_timeout_ms`, `deepgram.rs:762-803, 1465-1490`) and normalized into a provider-neutral `TurnEventKind` (`events.rs:330-338`; mapped `speech/mod.rs:4720-4744`). Sentence boundaries: none anywhere (grep for `sentence` is empty).

### 1.2 Why feeding every word to the LLM is inefficient

The **full** transcript is re-serialized into every projection LLM call:

- `latest_transcript_events()` returns **all** spans (`projections.rs:137-149`, no window/`take(n)`).
- `basis_events()` → `format_transcript_events_json()` serializes the whole event vector into the user message (`projection_llm.rs:509-540`, inlined at `:228`).
- No windowing, no rolling summary, no delta feed, no transcript token cap. Prior notes are **not** re-injected as text — continuity is via stable ids + returned patch ops (`projection_llm.rs:196-201`).
- **No prompt-caching anywhere.** grep for `cache_control`/`cachePoint` across `llm/*.rs` = zero hits. OpenRouter (`openrouter.rs:335-344`) and `api_client` (`api_client.rs:54-71`) request structs have no cache field; Bedrock sends plain text blocks (`bedrock.rs:551`). `supports_implicit_caching`/`input_cache_read` (`openrouter.rs:262,292`) are catalog display metadata only. Native llama.cpp clears KV every call (`engine.rs:770`); the one KV seam is opt-in local-only `streaming_prefill` (ADR-0012).

Invocation is event-driven per finalized turn (`speech/mod.rs:1640-1683`, gated on `is_final || end_of_turn || Final`), with two independent schedulers (Notes + Graph, `projection_scheduler.rs:485-544`) → up to 2 LLM calls/turn minus coalescing.

**Cost shape:** monotonic full transcript re-sent ~once per turn per kind with no caching → cumulative input tokens ≈ **O(turns × transcript_length) = O(n²)** over a session. This is exactly the failure the research says teams hit first — full-transcript-per-tick is O(N²) and hits the token ceiling first ([gemilab], research-context-efficiency §TL;DR).

### 1.3 How notes / KG update today (the retcon substrate)

This is the strong part of the codebase and the foundation for everything below (ground-graph-retcon):

- **One append-only patch log, one materializer.** Every change is a `ProjectionPatch` appended to durable JSONL (`persistence/mod.rs:1173-1183`); the materialized artifact is a whole-file JSON snapshot rebuilt by replaying the log. `ProjectionOperation` (`projections.rs:1034`) spans **both** notes and graph.
- **Notes are patched-by-id but coarse.** `UpsertNote` does `*existing = next` — a **full body replace**, not a diff (`projections.rs:1203-1227`). `DeleteNote` is a hard filter. No note-level `valid_until`, no sub-note granularity.
- **The graph is temporal / supersede-capable.** `MaterializedGraphNode/Edge` carry `valid_from_ms`/`valid_until_ms`; ops include `InvalidateGraphNode/Edge`, `MergeGraphNodes`, `SplitGraphNode`, `Strengthen/WeakenGraphEdge` (`projections.rs:1048-1090`). The live in-memory graph mirrors this with bitemporal `TemporalEdge` and `supersede_entity()` (`graph/temporal.rs:19-42, 344-512`), which invalidates old incident edges (sets `valid_until`, kept for audit), re-creates equivalent live edges on the canonical node, and re-points `name_index`.
- **Diarization already drives graph retcon live.** `SpeakerTimeline.apply_event` returns a `SpeakerLabelRemap` on a label change (`projections.rs:403-461`), which `dispatch_diarization_span_revision` (`speech/mod.rs:339-376`) feeds straight into `graph.supersede_entity`, then emits `GRAPH_DELTA` + `GRAPH_UPDATE`.
- **Staleness gate.** `ProjectionBasis` (per-span `revision_number` + `transcript_hash`) plus `validate_basis`/`validate_diarization_basis` (`projections.rs:160, 490-544`) reject stale LLM patches before they land.

This is the canonical **supersede-not-delete, bitemporal, provenance-preserving** substrate the diff-knowledge research prescribes (research-diff-knowledge §2, §6) — already built, for the graph. The gaps: notes are a weaker surface (whole-note replace + hard delete, no soft-invalidate/merge, no sub-note atoms, and the `SpeakerLabelRemap` signal is not consumed on the notes side). The diarization revisions are also **not yet persisted** to JSONL on the live path (only a test caller, `commands.rs:10312`) — ground-diarization §(4).6.

---

## 2. Design pillars

### 2(a) STT batching → endpointing/turn-detection → sentence segmentation → coherent units

**Goal:** emit a well-formed unit `{speaker_label, punctuated_sentence(s), turn_id, timestamps, is_final|is_eager}` at *semantic* boundaries, not silence — the research's "unitize at semantic boundaries, not silence" takeaway (research-stt-structuring §Design-takeaways).

**What we keep.** Provider-side turn detection is already good and already normalized. Deepgram Flux's model-integrated EOT state machine (`StartOfTurn`/`EagerEndOfTurn`/`TurnResumed`/`EndOfTurn`, `deepgram.rs:1465-1490`) is exactly the "model-integrated turn detector with an eager/speculative path" the research names as one of the three dominant production systems ([Deepgram Flux docs], research-stt-structuring §2). We should **not** reinvent a turn detector; we should make the app *use the eager path* and add an app-owned fallback where providers give us less.

**Three concrete moves, in leverage order:**

1. **Introduce a provider-neutral "TurnUnit" assembly seam at the event receiver** (the seam the ground note ranks #1 as provider-neutral, ground-stt-pipeline §4.2). Group `AsrSpanRevisionPayload(is_final)` finals under an open `turn_id` opened by `TurnEventKind::SpeechStarted`/`StartOfTurn` and closed by `EndOfTurn`/`UtteranceEnd`/`SpeechFinal`. `AsrSpanRevisionPayload` already carries `turn_id` + `end_of_turn` (`events.rs:261-262`) — this move populates and *consumes* them consistently instead of leaving grouping implicit. New type `TurnUnit { turn_id, speaker_label, text, span_ids, start_ms, end_ms, is_eager }` in `events.rs`.

2. **Wire an app-owned semantic/acoustic endpointer as a *fallback and confirmer* on the `AudioAccumulator` seam** (seam #1, `speech/mod.rs:2711-2805`, the pre-existing `aec_vad` scaffold home). This matters for (i) the Whisper path, which has zero endpointing today (fixed 2 s cuts), and (ii) cloud providers without a Flux-grade turn model. The research's converged pattern is a small (<500 MB, CPU) turn model gated by a VAD with a hard silence-timeout floor — pipecat Smart Turn v2 (waveform, 94.8M params, ~360 MB, ~12 ms/8 s, BSD-2) or LiveKit's audio turn-detector (~400 MB, ~25 ms, CPU) are the candidate models (research-stt-structuring §2). Feature-gate it like the diarization backends (`make_diarization_config` pattern, `speech/mod.rs:943-987`); default off; Flux stays the primary when selected.

3. **Add streaming re-punctuation / sentence sub-segmentation for long turns** so a monologue turn becomes sentence-sized sub-units. Streaming Punctuation with dynamic decoding windows improved segmentation F0.5 +13.9% and downstream MT +0.66 BLEU — better units measurably improve the downstream LLM ([Behre 2023, arXiv:2301.03819]; punctuation-agnostic SaT/WtP [arXiv:2305.18893], research-stt-structuring §3). This is **lowest priority** — it only helps long single-speaker turns and is the only piece with no existing analog.

**Reuse of retcon:** the eager path is retcon-native. Emit an eager `TurnUnit` as a **Provisional** ASR span revision (revision N, `is_eager=true`); on `TurnResumed`, emit a superseding revision (revision N+1) that reopens the turn; on final `EndOfTurn`, emit the **Final** revision (revision N+2, `supersedes` prior). The existing `AsrSpanRevisionPayload` `revision_number`/`supersedes`/`stability` machinery (`events.rs:231-270`) already expresses speculative-then-corrected, so speculative early flushing costs no new contract — it is the ASR-side twin of the diarization Provisional→Final flow.

### 2(b) Diarization integration via speaker-span revisions / retcon

**Goal:** every LLM-ready unit carries a stable speaker label, and a later speaker re-attribution retroactively corrects notes *and* graph without deleting data.

The spine already exists (ground-diarization): revisioned `DiarizationSpanRevisionPayload` (Provisional/Stable/Final) → `SpeakerTimeline` ledger → `SpeakerLabelRemap` → `graph.supersede_entity` → delta/snapshot. Streaming-diarization SOTA supports the arrival-ordered, flexible-count design already chosen (LS-EEND, Streaming Sortformer's Arrival-Order Speaker Cache, research-stt-structuring §1; ADR-0017).

**Three concrete moves:**

1. **Attach the resolved speaker label to the `TurnUnit`** from 2(a) via the existing overlap join (`overlap_speaker_for_segment`, `diarization/mod.rs:843-875`; the frontend already does this in `speakerTimeline.ts`). The label becomes unit metadata, per the research's "diarization labels as unit metadata" (research-stt-structuring §3).

2. **Persist diarization revisions on the live path** — close the confirmed gap (ground-diarization §4.6): call `append_diarization_span_revision` (the trait exists, `persistence/mod.rs:516-533`) from `emit_and_dispatch_diarization_span_revision` (`speech/mod.rs:378-427`). Without this, retcons can't be replayed and the basis gate has no durable ground truth. This unblocks seeds 3588/1fbd's universal "persist and replay revisions" item.

3. **Extend the remap to also retcon notes** (the missing consumer, ground-graph-retcon §4.4). Today `SpeakerLabelRemap` drives only `graph.supersede_entity`. Add a notes-side consumer that emits a note `ProjectionOperation` to re-attribute the affected note(s) — see 2(f) for the exact op. This is the single highest-leverage reuse: the *same* remap signal fans out to graph (already) + notes (new), through the *same* patch log and basis gate.

**Reuse of retcon:** this pillar is almost entirely wiring, not new machinery — `SpeakerLabelRemap`, `supersede_entity`, and the basis gate are the retcon engine; we add one durable-write call and one new fan-out consumer.

### 2(c) STT → LLM context efficiency (rolling summary / delta feed / event-driven invocation)

**Goal:** kill the O(n²) by never re-sending the whole transcript. The research prescribes four complementary levers: tiered hot/warm memory, structured pinned facts, prompt caching, and delta-driven invocation (research-context-efficiency §TL;DR). We already have #4; we add #1–#3.

**Concrete moves, in leverage order (the exact seams the ground note ranks, ground-llm-feed §5):**

1. **Windowed basis + rolling summary (biggest win).** Change `basis_events()` / `format_transcript_events_json()` (`projection_llm.rs:509-540`) to feed only (a) a maintained rolling summary of older turns + (b) the last K unsummarized turns verbatim (the dominant hot-buffer/warm-summary pattern, 5–20 turns hot, [tianpan-gradual][chainofcraft]). Update the summary **incrementally** — fold in only the turn leaving the hot buffer, never re-summarize from scratch (avoids recursive-hallucination/Telephone drift; input settles ~1.5–2.5K tokens, [chainofcraft][gemilab]). Store the current summary text keyed by "summarized-through revision R" on/beside `ProjectionBasis` (not a separate replayable summary artifact — see §3) and put R into `ProjectionBasis` so `validate_basis` keeps coalescing/repair correct (the ground note's exact recommendation, ground-llm-feed §5.1).

2. **Delta-only feed + explicit current-state block.** Send `current notes/graph state + only spans since last patch`, not reconstruct-from-full-transcript. The scheduler *already computes the delta* (`basis_revision_delta_count`/`coalesced_span_delta`, `projection_scheduler.rs:282,454`). Seam: the user-message body in `projection_patch_prompt_messages` (`projection_llm.rs:219-237`) plus a read of the durable projection store to inline current state.

3. **Pin must-never-lose facts as structured typed state** (the single most-cited drift mitigation). The KG *is* our structured state — inject a compact typed-fact/triple view of the current graph (names, decisions, rejected options + why) at the top of the prompt rather than trusting the prose summarizer, which the research shows inverts negations and drops rejection reasons (research-context-efficiency §4). Seam: prepend to the system/first block in `projection_patch_prompt_messages`.

**Reuse of retcon:** the `ProjectionBasis` + `transcript_hash` machinery is the clean seam for defining "summarized-through revision R" as part of the basis, so a slow completion still can't land stale (ground-llm-feed §5.1). The graph snapshot is the pinned-fact source, so the notes prompt reuses the graph projection instead of a second summarizer.

### 2(d) Session / prompt-cache reuse (stable-prefix ordering)

**Goal:** turn the re-sent stable prefix into cache-read tokens (10% Anthropic/Gemini, 50–90% OpenAI, research-context-efficiency §2).

The cache mechanic across all vendors: **longest common prefix; any change after the first differing token misses.** So order the prompt static→dynamic and grow the transcript **append-only** ([anthropic-caching], research-context-efficiency §2).

**Concrete moves:**

1. **Re-order the projection prompt to `[system+schema] → [pinned typed facts] → [rolling summary] → [hot-buffer transcript, append-only] → [per-tick job metadata/timestamp]`.** Today the job metadata (id, basis_hash, span_count) and the growing transcript are interleaved in the user message (`projection_llm.rs:219-237`); the per-call `basis_hash`/`job.id` and any timestamp must move to the **end** or they bust the cache every tick (the documented Anthropic anti-pattern of a changing value near the front, research-context-efficiency §2).

2. **Add a `cache_control` breakpoint on the last stable block** for cache-capable providers. Seams: `ChatCompletionRequest`/`ApiMessage` in `openrouter.rs:335-350` and `api_client.rs:54-77`; Bedrock `build_converse_messages` gets a `cachePoint` content block (`bedrock.rs:551`). Gate on the already-parsed `supports_implicit_caching`/`input_cache_read` catalog fields (`openrouter.rs:262,292`) — the capability signal is already in the tree, just unused. Anthropic min cacheable = 1024 tok (Opus/Sonnet); ≤4 breakpoints; read = 0.1× (research-context-efficiency §2).

3. **Set a `prompt_cache_key` per `(session, resolved-provider)`** (OpenAI) so a session's turns route to the same cache-warm machine (research-context-efficiency §2). Session id is already threaded through the projection job. Scope the key *and* the cache breakpoint to the resolved provider, not the session alone: the executor has a provider fallback chain (`run_projection_patch`/`run_projection_attempts`, gated by `allow_cloud_fallbacks`/`requires_cloud_content_transfer`, `llm/executor.rs`), so a mid-session failover to a different vendor lands a cold cache — and a summary/prefix computed for one vendor's tokenizer is meaningless to another. A fallback hop is an expected cold cache, not a bug; caching is a best-effort per-provider property, not a session-wide guarantee.

**Reuse of retcon:** append-only is *free* here because the transcript ledger is already append-only and monotonic (`projections.rs`); the only discipline needed is prompt ordering. A cache refresh is *not* free — it is a cache write, billed above base input (Anthropic ~1.25×, research-context-efficiency §2). So the cache pays off when turns arrive within the vendor's TTL (Anthropic's 5-min default refreshes the prefix cheaply); a turn gap longer than the TTL (long pauses, screen-shares, breaks — not assumed sub-TTL) forces a full-price re-write of the prefix. Net: a win under active back-and-forth, neutral-to-slight-loss across a long idle gap.

### 2(e) Diff-based note patching (patch-not-append, minimal structured edits)

**Goal:** stop full-body note replacement; emit a minimal, content-anchored edit so a bad edit fails loudly instead of silently clobbering correct prose (research-diff-knowledge §1c, §6 — "a failed patch is a visible signal; a bad regeneration is a silent corruption").

Today `UpsertNote` full-replaces the body (`projections.rs:1203-1227`) — the silent-overwrite vector the research warns against. Notes are the weaker surface (ground-graph-retcon §4).

**Concrete moves:**

1. **Add sub-note (block/claim) granularity** — the one piece with no existing analog (ground-graph-retcon §4.3). Give `MaterializedNote` addressable blocks (`Vec<NoteBlock { block_id, text, valid_from_ms, valid_until_ms }>`), and add ops `UpsertNoteBlock`/`InvalidateNoteBlock`. This makes a note a bitemporal collection of claims — the same shape as graph nodes — so a claim can be superseded (hidden, auditable) rather than overwritten.

2. **Add a SEARCH/REPLACE-style patch op for prose within a block** (`ReplaceNoteText { note_id, block_id, search, replace }`), applied by exact/expanding-unique anchor. Research ranking: search/replace is the frontier default and fails loudly on a bad anchor; line numbers are traps (LLMs are terrible at them); progressively expand the anchor until unique (research-diff-knowledge §1a–1c). The materializer rejects a non-matching anchor → the patch is refused (a signal), not applied blind.

3. **Add note-level bitemporal validity + soft-invalidate** — give `MaterializedNote` `valid_from_ms`/`valid_until_ms` and an `InvalidateNote { id }` op (near-drop-in copy of `InvalidateGraphNode`, `projections.rs:1545`), so a superseded note is hidden-but-auditable instead of hard-`DeleteNote`d (ground-graph-retcon §4.1).

**Reuse of retcon:** every one of these mirrors an *existing* graph op. The materializer, persistence, replay, and basis-validation plumbing already handle bitemporal invalidate + replace-by-id for graph nodes/edges; notes ops slot into the same `ProjectionOperation` enum, the same JSONL log (`state.rs:438-560`), and the same frontend reducer shape (`src/store/index.ts:253-317`). This is additive, not a redesign (ground-graph-retcon §4).

### 2(f) Knowledge-graph retroactive update: patch-in-place vs record-a-supersession

**Goal:** make the LLM (and the diarization remap) choose correctly between *revision* (I was wrong/imprecise) and *update* (the world changed), mapped onto machinery we already have.

The decision reduces to contradiction-type + confidence + fact-type (research-diff-knowledge §5): **patch-in-place** when the new info corrects/refines the same fact with no world change (AGM revision / KM revision); **record-a-supersession** when the new info contradicts the old and both were true at different real-world times (Sarah VP→CEO; KM update). Gate by fact type — static facts never expire; only dynamic/temporal are candidates ([openai-cookbook temporal_agents], research-diff-knowledge §2c).

**The mapping onto existing ops (all already exist, `projections.rs:1048-1090`):**

| Research concept | Existing op / mechanism | When |
| --- | --- | --- |
| Patch-in-place / revision | `UpsertGraphEdge` (replace-by-id), `Strengthen/WeakenGraphEdge` | correction/refinement of the same fact |
| Supersede / update (bitemporal) | `InvalidateGraphEdge` (sets `valid_until_ms`) + new `UpsertGraphEdge` | contradiction across real-world times |
| Entity identity merge | `MergeGraphNodes` (materialized) / `supersede_entity` (live) | two nodes are the same entity (e.g. Speaker 2 = Alice) |
| Entity split | `SplitGraphNode` | one node was actually two entities |
| Hidden-but-auditable | `valid_until_ms` filtered from snapshot/delta (`temporal.rs:725-727`) | all invalidations (soft-delete) |

**Concrete moves:**

1. **Teach the projection prompt the patch-vs-supersede rule + fact typing.** The prompt already says "prefer retcon operations over duplicate nodes" (ADR-0024 §4). Extend the `operation_guidance` (`projection_llm.rs:195-202`) to: (i) classify a fact as STATIC vs DYNAMIC/TEMPORAL; (ii) `Invalidate`+new-edge only DYNAMIC/TEMPORAL facts on a temporally-overlapping contradiction; (iii) `Upsert`-replace for a same-time correction; scope the contradiction check to semantically-similar existing edges only (the Graphiti retrieval-scoped invalidation, research-diff-knowledge §2b, §5).

2. **Add an explicit `superseded_by` acknowledgment (optional, M).** Today supersession is implicit (`valid_until` + re-point), reconstructable only from the log (ground-graph-retcon §3b, §4.5). Adding a `superseded_by` field/edge realizes the research's `invalidated_by` provenance link so a retraction is traversable ([openai-cookbook], research-diff-knowledge §2c). Only worth it if the UI wants a "why was this retired / what replaced it" view.

3. **Validation gate before every write (already present — keep and lean on it).** `validate_basis`/`validate_diarization_basis` is exactly the SSGM "don't let the agent be sole generator+validator" gate (research-diff-knowledge §6). The addition: reject an `Invalidate` on a STATIC-typed fact (over-eager-retraction guard).

**Reuse of retcon:** this pillar is 90% *prompt + guidance* work — the ops, the bitemporal fields, the soft-delete semantics, and the staleness gate are all built and shipping. We are teaching the LLM to use the retcon vocabulary that already exists, and the diarization remap from 2(b) is the non-LLM producer that already exercises `supersede_entity`/`MergeGraphNodes`.

### 2(g) Data-movement ledgering for the new remote-LLM flows

**Goal:** the context-efficiency layer creates *new* transcript-derived artifacts that leave the device — the rolling summary (2c) and the cross-turn cached prefix (2d) — so each must be recorded in the session data-movement ledger, the codebase's first-class privacy substrate (seed 70a3, `crates/ipc-contract/src/session_data_movement.rs` + `persistence/data_movement.rs`). This is not new machinery: the ledger already models exactly this.

**Concrete moves:**

1. **Emit a movement event per projection LLM call.** Every projection call in 2(c)/2(d) is a `ProviderCallStarted`/`ProviderCallSucceeded`/`ProviderCallFailed` (or `Cancelled`) event via `DataMovementLedgerBuilder`, carrying `DataClass::Prompts` (recorded by hash/size only) + `DataClass::TranscriptText`, the remote destination boundary, and the `MovementModel`/`MovementPolicy`/`MovementBasis`. The projection scheduler already has the natural hook points (`ProjectionJobStarted`/`ProjectionPatchAccepted`/`ProjectionPatchRejected` event types exist for this).

2. **Treat the rolling summary and pinned-fact block as ledgered artifacts.** The summary is transcript-derived text (`DataClass::Notes`); the pinned typed-fact block is graph-derived (`DataClass::GraphContext`). Both are new derived artifacts that get transmitted to remote vendors; record their movement, not just the raw-transcript spans.

3. **Ledger the vendor-side persistence of the cached prefix, and gate it.** `cache_control` (2d.2) persists the cached prefix on the vendor's servers for the cache TTL — a durable off-device copy, not just an in-flight transfer. Note this in the event and gate the whole context-efficiency path behind the same `MovementPolicy` that governs cloud transfer, so a session pinned to local-only providers never writes a summary/prefix to a remote cache.

**Reuse:** the ledger seam (`persistence/data_movement.rs`, `DataMovementLedgerBuilder`) already stamps `event_id`/`schema_version`/`created_at_ms` and is redaction-safe by construction (no field can carry raw prompt bodies). Every ASR provider already records movement events on this seam; the projection LLM path is exactly where transcript-derived text leaves the device, so it is the one seam that must not stay unledgered.

---

## 3. Concrete change set (files / types / new events)

**New / changed types (`events.rs`, `projections.rs`):**
- `events.rs`: `TurnUnit { turn_id, speaker_label, text, span_ids, start_ms, end_ms, is_eager }`; extend `AsrSpanRevisionPayload` usage so eager/final turns are revisions (no new field needed — `is_eager` can ride `stability=Provisional`).
- `projections.rs`: `MaterializedNote` gains `valid_from_ms`/`valid_until_ms` + `blocks: Vec<NoteBlock>`; new `ProjectionOperation` variants `UpsertNoteBlock`, `InvalidateNoteBlock`, `ReplaceNoteText`, `InvalidateNote`, `ReattributeNoteSpeaker { note_id, superseded_label, canonical_label }`; optional `superseded_by` on graph edge/node.
- `ProjectionBasis` gains `summarized_through_revision: Option<u64>` for the rolling-summary seam.

**Rolling-summary storage (lazier form first):** store the current summary text keyed by `summarized_through_revision` on/beside `ProjectionBasis` (the `ProjectionBasis` field above) and regenerate-forward — the ground note's actual recommendation (ground-llm-feed §5.1). Do **not** stand up a separate replayable `MaterializedSummary` artifact with its own fold-patch stream in phase one: that is a third materialized artifact whose durable replay the ground note does not call for and whose incremental-fold summarizer is the drift risk this doc flags (Open Q1). Defer the durable-replay summary artifact (mirroring `MaterializedNotes`/`MaterializedGraph`, `persistence/mod.rs:1233-1242` pattern) to a later phase only if replay determinism demands it.

**STT / turn layer (`speech/mod.rs`, `asr/*`):**
- `run_deepgram_event_receiver` / per-provider receivers: assemble `TurnUnit`s keyed on `TurnEventKind` + `turn_id`; emit eager units on `EagerEndOfTurn`, superseding revision on `TurnResumed`, final on `EndOfTurn`.
- `AudioAccumulator` seam / `aec_vad`: optional app-owned endpointer (Smart Turn / LiveKit model), feature-gated like `make_diarization_config`.
- `emit_and_dispatch_diarization_span_revision`: add `append_diarization_span_revision` durable write; fan the `SpeakerLabelRemap` to a new notes consumer emitting `ReattributeNoteSpeaker`.

**LLM feed (`projection_llm.rs`, `projection_scheduler.rs`, `llm/*`):**
- `basis_events`/`format_transcript_events_json`: windowed basis + rolling summary + delta feed.
- `projection_patch_prompt_messages`: re-order static→dynamic; prepend pinned typed-fact block (from graph snapshot); extend `operation_guidance` with patch-vs-supersede + fact-typing rules and the new note-block ops.
- `openrouter.rs`/`api_client.rs`/`bedrock.rs`: `cache_control`/`cachePoint` on the stable prefix, gated on catalog capability; `prompt_cache_key` per `(session, resolved-provider)`.

**Privacy / data-movement (`persistence/data_movement.rs`, `crates/ipc-contract/src/session_data_movement.rs`):**
- Emit `DataMovementEvent`s for every projection LLM call (`ProviderCallStarted/Succeeded/Failed`) via `DataMovementLedgerBuilder`, tagging `DataClass::Prompts` + `TranscriptText`, remote destination, and `MovementModel`/`MovementPolicy`/`MovementBasis`; ledger the rolling summary (`DataClass::Notes`) and pinned-fact block (`DataClass::GraphContext`) as derived artifacts; record that `cache_control` persists the prefix on the vendor for the TTL and gate the whole path behind the cloud-transfer `MovementPolicy`.

**Frontend (`src/store/index.ts`, `src/components/NotesPanel.tsx`, `speakerTimeline.ts`):** mirror the new note-block / invalidate / reattribute reducers (same shape as existing note/graph reducers `index.ts:253-317`).

---

## 4. Phased build plan

**Vertical slice first (proves the whole spine end-to-end):** *Rolling-summary + delta feed + stable-prefix caching for the Notes scheduler only.* It touches `projection_llm.rs` + one request builder + `ProjectionBasis`, needs no new STT work, and immediately kills the O(n²) cost on the highest-frequency call path. It must ship *with* 2(g) ledgering — the summary and cached prefix are the slice's new off-device artifacts, so the slice is not shippable until they are ledgered and gated behind the cloud-transfer policy. Ship, measure `tokens_used` (`executor.rs:638,663`) before/after.

| Pillar | Effort | Depends on | Notes |
| --- | --- | --- | --- |
| 2(d) stable-prefix caching | **S** | none | mechanical; prompt re-order + `cache_control`; biggest cost win for least code |
| 2(c) rolling summary + delta feed | **M** | 2(d) ordering | the O(n²) kill; incremental-fold summarizer is the risk (drift) |
| 2(b) persist diarization revisions | **S** | none | one durable-write call; unblocks replay + seeds 3588/1fbd |
| 2(f) patch-vs-supersede prompt + fact-typing | **M** | none (ops exist) | mostly prompt/guidance; `superseded_by` is an optional +M |
| 2(e) note-block granularity + search/replace | **L** | 2(f) validation-gate reuse | new sub-note atom + reducers FE+BE; only non-analog piece |
| 2(b→e) remap → `ReattributeNoteSpeaker` | **M** | 2(e) note ops, 2(b) persist | fans the existing remap to notes |
| 2(a) TurnUnit assembly + eager path | **M** | none | consumes existing `turn_id`/`end_of_turn`; retcon-native eager |
| 2(a) app-owned endpointer model | **L** | TurnUnit seam | feature-gated; ships a model; Whisper-path win |
| 2(a) streaming re-punctuation | **XL** | endpointer | lowest ROI; long-turn only; new model |
| 2(g) data-movement ledgering for new LLM flows | **S** | 2(c)/2(d) | ships *with* the vertical slice; summary + cached prefix are new off-device artifacts, gate behind cloud-transfer policy |

**Dependency notes:** 2(d) before 2(c) (ordering must be stable before caching pays). 2(f) before 2(e) (reuse the validation-gate + supersede semantics). The diarization-persist (2b) is independent and cheap — do it early to unblock replay tests. **Watch the `1534` ipc::Channel migration** (in-flight, Wave 7 task #50, touches `events.rs`/`speech/mod.rs`) — the new streaming payloads (`TurnUnit`) should land in the `ipc-contract` crate to ride that migration, and must be serialized after or kept disjoint from 1534's edits (ground-stt-pipeline §3).

### Open questions
1. **Rolling-summary drift vs the pinned-graph fallback.** If the graph snapshot already pins the must-never-lose facts (2c.3), how lossy can the prose warm-summary be before it hurts notes quality? Needs the weekly audit-loop the research prescribes ([gemilab], research-context-efficiency §4).
2. **Eager-EOT redundancy budget.** Speculative early flushing wastes LLM work on resumed turns (metered in [arXiv:2606.13450]). Do we gate eager flushing behind cost, or only enable it for the (cheap, cached) Notes call and not Graph?
3. **Note-block identity across LLM calls.** Search/replace needs stable `block_id`s the LLM reuses; how do we keep block ids stable when the LLM rewrites prose, given LLMs are bad at ids (the same reason we avoid line numbers)? Possibly deterministic content-hash block ids assigned by the materializer, not the model.
4. **Fact typing source.** Who classifies STATIC vs DYNAMIC/TEMPORAL — the extraction LLM inline, or a cheap deterministic pre-pass? Over-eager retraction is the adversarial failure (research-diff-knowledge §6).
5. **Which providers get the app-owned endpointer.** Only non-Flux cloud + Whisper, or also as a confirmer over Flux eager events to cut false starts (~6% under noise, [LiveTurn 2026])?

---

## 5. Research citations (inline sources)

- STT structuring / turn detection / diarization SOTA — `research-stt-structuring.md`: LS-EEND (arXiv:2410.06670), Streaming Sortformer AOSC (arXiv:2507.18446), EEND-EDA flexible count (arXiv:2005.09921), VAP (arXiv:2205.09812 / 2401.04868), Endpoint Anticipation (arXiv:2606.13450), Deepgram Flux docs, LiveKit turn-detector, pipecat Smart Turn v2, Streaming Punctuation (arXiv:2301.03819), SaT/WtP (arXiv:2305.18893), SID-Bench/APT (arXiv:2603.24144), LiveTurn (OpenReview JIaOGuEMET).
- Context efficiency / caching / drift — `research-context-efficiency.md`: hot/warm tiering [tianpan-gradual][chainofcraft], incremental rolling summary [gemilab], structured pinned facts [gemilab][tianpan-artifacts], Anthropic/OpenAI/Gemini prompt-cache mechanics + stable-prefix ordering [anthropic-caching][openai-cookbook-201][vertex-caching], delta/event-driven invocation [eridanus][pipecat], drift failure taxonomy incl. negation inversion [tianpan-artifacts]; academic anchors MemGPT (2310.08560), Recursive-Summarizing (2308.15022), Generative Agents (2304.03442).
- Diff / supersede / bitemporal — `research-diff-knowledge.md`: search/replace + fail-loud safety [aider][tsukino], RFC-6902 for structured KG [json-correction-loop], Graphiti/Zep bitemporal + edge invalidation (arXiv:2501.13956), `invalidated_by` + statement typing [openai-cookbook temporal_agents], AGM revision vs KM update (arXiv:2104.14512 / 2602.23302), TMS/JTMS provenance, SSGM validation gate (arXiv:2603.11768), RoundEdit irreversibility (arXiv:2310.02129).
