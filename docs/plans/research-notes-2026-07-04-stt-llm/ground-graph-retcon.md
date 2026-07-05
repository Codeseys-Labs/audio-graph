# Ground Truth: Diarization Retcon Machinery + Notes/Knowledge-Graph Model

Repo: `/mnt/e/CS/github/audio-graph` (Tauri app; Rust backend in `src-tauri/src`, React/TS frontend in `src`).
Analysis is READ-ONLY. All the "diarization retcon" work named in the task is already MERGED into the working tree (HEAD `9429aad`, master). The commits are visible on branch `work/2885-diarization-retcon-live-producer` (e.g. `b3059ab` wire live producer for invalidate_edge retcon `0966`; `e9e6d08` wire diarization speaker-resolution to entity retcon dispatcher `2113`; `b55501d` re-point name_index on supersede `78d0`; `3979647`/`b0f62bd` dispatch live diarization retcons; `01b0a61` promote Deepgram diarization normalizer). ADR-0024 (`docs/adr/0024-*.md`) documents the whole event-sourced design and supersedes ADR-0014.

---

## (1) What a "retcon" is here, and the event/revision shape that enables it

A **retcon = retroactively correcting already-emitted speaker attribution** without mutating or deleting the original data. Two coupled layers:

### 1a. The live in-memory graph retcon (`graph/temporal.rs`)

`TemporalKnowledgeGraph` is a petgraph `StableGraph<GraphEntity, TemporalEdge>` (`src-tauri/src/graph/temporal.rs:51`). Edges carry **Graphiti-style bitemporal validity**: `TemporalEdge { valid_from: f64, valid_until: Option<f64>, ... }` (`temporal.rs:19-42`). An edge with `valid_until = None` is LIVE; setting `valid_until = Some(ts)` HIDES it from views but keeps it in the graph and the persisted file (audit/replay).

- `invalidate_edge(edge_idx, timestamp)` (`temporal.rs:305-309`) is the SOLE producer of `valid_until`. It just sets the timestamp.
- `snapshot()` (`temporal.rs:696-753`) and the delta builder `build_delta_edge` (`temporal.rs:833-859`) FILTER OUT any edge with `valid_until.is_some()` (`temporal.rs:725-727`, `843-845`). So invalidation = "hidden, not deleted."
- `supersede_entity(superseded_name, canonical_name, timestamp, threshold) -> usize` (`temporal.rs:344-512`) is the retcon ENGINE. When a provisional speaker (`"Speaker 2"`) is resolved to a stable identity (`"Alice"`):
  1. Resolve both names via `resolve_entity` (exact + jaro_winkler fuzzy, `temporal.rs:273-293`).
  2. For every LIVE incident edge on the superseded node (both directions, `temporal.rs:375-408`): `invalidate_edge` it (sets `valid_until`), push its id to `delta_removed_edge_ids`, then **re-create an equivalent LIVE edge** between the canonical node and the original other endpoint (`temporal.rs:415-463`), folding weight into an existing same-type edge if one exists.
  3. Fold the superseded node's mention_count / first_seen / last_seen / speakers into the canonical node (`temporal.rs:469-493`). The superseded node is deliberately KEPT (not deleted) with only invalidated edges.
  4. **Re-point the `name_index`** so every key aiming at the superseded node now resolves to canonical (`temporal.rs:505-509`, seed `78d0`) — otherwise a later `add_relation("Speaker 2", ...)` would resurrect the retired node.
- Edge ids are `edge-{seq_id}` from a monotonic never-reused `seq_id` (`temporal.rs:110-112`, `33`), NOT the recyclable petgraph EdgeIndex — so removals/updates match across delta windows.

### 1b. The durable event/revision shape (`events.rs` + `projections.rs`)

The wire event is `DiarizationSpanRevisionPayload` (`src-tauri/src/events.rs:287-325`), emitted on the `"diarization-span-revision"` event (`events.rs:21`). Key fields:
- `span_id` (provider-neutral logical span identity — NOT a provider speaker id),
- `speaker_label: Option<String>` (the human-facing label that drives the retcon),
- `stability: DiarizationSpanStability` = `Provisional | Stable | Final` (`events.rs:273-283`) — Provisional "may be remapped", Stable "can still be retconned by later full-session/provider revisions", Final "complete",
- `revision_number: u64` + `supersedes: Option<String>`,
- `basis_asr_span_ids` / `basis_transcript_segment_ids` (what this attribution was computed from).

The durable form is `DiarizationSpanRevision` (`projections.rs:226`, `From<DiarizationSpanRevisionPayload>` at `projections.rs:309`), persisted append-only as JSONL. There is a parallel transcript event: `TranscriptEvent` (`projections.rs:36`, from `AsrSpanRevisionPayload`) on event `"asr-span-revision"` (`events.rs:16`). Both use identical revision semantics: monotonic `revision_number`, `supersedes`, partial→final `stability`.

### 1c. How the two layers connect (the live producer)

`SpeakerTimeline` (`projections.rs:373`) is the diarization ledger. `apply_event` (`projections.rs:403-446`) replaces a span's earlier revision with a newer one (rejecting stale/conflicting) and, when the `speaker_label` changed from one non-empty value to a different one, returns a `SpeakerLabelRemap { superseded_label, canonical_label }` via `detect_label_remap` (`projections.rs:448-461`). `SpeakerLabelRemap` (`projections.rs:349-353`) is explicitly "the durable signal that drives the knowledge-graph entity retcon."

The LIVE wiring: `speech/mod.rs:339-376` `dispatch_diarization_span_revision`:
```
let remap = timeline.apply_event(revision)?;   // speech/mod.rs:345
let Some(remap) = remap else { return not-fired };  // 357
let invalidated = graph.supersede_entity(&remap.superseded_label, &remap.canonical_label, timestamp, 1.0);  // 365-370
```
Wrapper `emit_and_dispatch_diarization_span_revision` (`speech/mod.rs:378-427`) locks the timeline + graph, calls the dispatch, and if `retcon_fired` emits a `graph-delta` + full `graph-update` snapshot so the UI drops the stale link and shows the re-pointed one immediately. Test proof: `speech/mod.rs:6881` `assemblyai_speaker_revision_emission_retcons_graph_on_label_remap`, and `temporal.rs:1229` `supersede_entity_invalidates_old_edge_and_repoints_to_canonical`.

---

## (2) Notes: APPENDED or PATCHED?

**BOTH, at different layers — this is the crux.** Notes are an event-sourced projection:

- **The patch LOG is append-only.** Each note change is a `ProjectionPatch` (`projections.rs:857`) appended to a durable JSONL event log via `PersistenceRepository::append_projection_patch` → `append_jsonl` (`persistence/mod.rs:1173-1183`, `178-183` uses `OpenOptions::new().append(true)`).
- **The MATERIALIZED note is patched in place (replace-by-id).** `ProjectionOperation` (`projections.rs:1034`) has notes ops `UpsertNote { id, title, body, tags }`, `DeleteNote { id }`, `ReorderNote { id, after_id }`. `MaterializedNotes::apply_patch` → `upsert_note` (`projections.rs:1203-1227`): if a note with the same `id` exists it does `*existing = next` (FULL REPLACE of title/body/tags — **not** an append-to-body, no diff/patch of prose), else it pushes a new note. `DeleteNote` filters by id (`projections.rs:1176`).
- The materialized artifact itself is a whole-file JSON SNAPSHOT (`notes/<id>.json`), rewritten each time via `save_materialized_notes` → `save_json` (`persistence/mod.rs:1233-1242`), rebuilt deterministically by replaying the patch log (`replay_accepted_patches`).
- Stable note ids are what let a later patch "refine an earlier note in place" (ADR-0024 §3). The LLM is instructed to reuse ids, and `sequence`/`basis`/`provenance` are stamped by the trusted path, not the model.

Frontend mirror (`src/store/index.ts:253-317`): the TS reducer applies the same UpsertNote replace-in-place (`index.ts:283-285`: `if (index >= 0) notes.notes[index] = next; else notes.notes.push(next)`), DeleteNote filter, ReorderNote splice — guarded by `patch.sequence <= current.last_sequence` (`index.ts:256`). UI in `src/components/NotesPanel.tsx`.

Write path end-to-end: `state.rs:438` `apply_runtime_projection_patch` → `apply_runtime_projection_patch_with_savers` (`state.rs:454-560+`): checks session match, checks `patch.basis == expected_basis` (`state.rs:492`), calls `apply_validated_patch(&ledger, &patch)` (basis-staleness gate, `state.rs:516`), APPENDS the patch to the JSONL writer (`state.rs:538`), then SAVES the materialized notes/graph snapshot (`state.rs:546-557`).

NOTE: an older `synthesize_notes` (ADR-0014) whole-conversation prose path still exists in `commands.rs`/`lib.rs` as a manual escape hatch, but is no longer the live surface.

---

## (3) Is the knowledge graph append-only, or can facts be superseded/revised?

**It is NOT append-only — it is explicitly temporal/supersede-capable, in TWO parallel graph representations that share the same design:**

### 3a. Live in-memory graph (`graph/temporal.rs`) — covered in (1a).
Bitemporal edges (`valid_from`/`valid_until`). Facts are superseded by `invalidate_edge` (hide via `valid_until`) + re-point via `supersede_entity`. The superseded node/edges LINGER for audit. This is the Graphiti temporal-KG concept (ADR-0008 referenced by ADR-0024).

### 3b. Materialized projection graph (`projections.rs` `MaterializedGraph`).
`MaterializedGraphNode`/`Edge` (`projections.rs:1261`, `1300`) ALSO carry `valid_from_ms` / `valid_until_ms: Option<u64>` (bitemporal, mirroring the live graph). Graph `ProjectionOperation`s (`projections.rs:1048-1090`) are a rich supersede/revise vocabulary:
- `UpsertGraphNode` / `UpsertGraphEdge` (replace-by-id, `projections.rs:1482-1544`),
- `InvalidateGraphNode` (sets `valid_until_ms`, cascades to incident edges, `projections.rs:1545-1565`) / `InvalidateGraphEdge` (`1567-1581`),
- `RemoveGraphNode` / `RemoveGraphEdge` (hard remove),
- `StrengthenGraphEdge` / `WeakenGraphEdge` (weight deltas, `1583-1612`),
- **`MergeGraphNodes { source_id, target_id }`** (`1614-1663`): invalidates source, rewrites its edges to target, then `invalidate_duplicate_active_edges` (`1745-1777`) — this is the projection-layer analog of `supersede_entity`,
- **`SplitGraphNode { id, replacement_nodes }`** (`1666-1712`): invalidates source + edges, upserts ≥2 replacements.

**Temporal/bitemporal concept: YES, explicit and pervasive** — `valid_from`/`valid_until` on edges (live) and `valid_from_ms`/`valid_until_ms` on nodes AND edges (materialized). **Supersede-edge concept: YES** — invalidation-then-repoint (live `supersede_entity`; materialized `MergeGraphNodes`/`InvalidateGraph*`). Invalidated rows are retained (hidden), which is a soft-delete / logical-tombstone form of supersession rather than a true separate "supersede edge" pointer. ADR-0024 §4 explicitly: "The materialized graph reuses the temporal model … Prefer retcon operations over duplicate nodes when later transcript context corrects earlier assumptions" (from `projection_llm::projection_patch_prompt_messages`).

**Basis/staleness (the bitemporal-validity gate on the projection side):** `ProjectionBasis` (`projections.rs:160`) records the exact `span_revisions` (per-span revision_number), `diarization_span_revisions`, and a `transcript_hash`. `TranscriptLedger::validate_basis*` + `SpeakerTimeline::validate_diarization_basis` (`projections.rs:490-544`) reject a patch whose basis no longer matches the current ledger (`StaleSpanRevision`, `MissingCurrentSpan`, `TranscriptHashMismatch`, `StaleDiarizationSpanRevision`, …). This is how a slow LLM completion can't land stale facts over fresher state.

---

## (4) Could the retcon machinery GENERALIZE to note-patching + KG fact-superseding? (load-bearing)

**It already substantially generalizes — the two surfaces (notes + graph) SHARE the event-sourced retcon substrate.** ADR-0024's stated goal was exactly this: "The graph's existing temporal retcon semantics must extend to the new patch contract rather than being a separate code path," and "Notes and graph share the basis, the patch log, and replay semantics."

What is ALREADY shared / generalized today:
- One `ProjectionPatch` + one `ProjectionOperation` enum spans BOTH notes and graph (`projections.rs:1034`). One append-only JSONL patch log. One `ProjectionBasis` staleness gate. One `MaterializedProjectionState { notes, graph }` (`projections.rs:1779`) with unified `apply_validated_patch` (live) / `apply_replayed_patch` (replay). One scheduler (`projection_scheduler.rs`).
- KG fact-superseding is FULLY realized: `MergeGraphNodes`, `SplitGraphNode`, `InvalidateGraphNode/Edge`, `valid_until_ms`. Diarization remaps flow into it live (`supersede_entity`) and can flow into the materialized graph via `MergeGraphNodes`.

What note-patching does today vs. what fact-superseding does:
- Notes ops are coarse: `UpsertNote` REPLACES the entire body by id; there is no field-level/prose-level diff, no note-level `valid_until` (a superseded note is just overwritten or `DeleteNote`d — the prior body survives ONLY in the append-only patch log, not as a queryable live tombstone the way an invalidated edge is). `DeleteNote` is a hard filter, not a soft `valid_until` hide.

What would need to change to bring notes up to true "fact-superseding" parity:
1. **Add bitemporal validity to notes.** Give `MaterializedNote` `valid_from_ms`/`valid_until_ms` (mirror `MaterializedGraphNode`) and add an `InvalidateNote { id }` op so a note can be SOFT-superseded (hidden but auditable) instead of hard `DeleteNote`/overwrite. The invalidate *logic* copies the graph node's path (`projections.rs:1545`, `1725`), but the full change is broader than a drop-in: it also needs the `ProjectionOperation` schema addition, replay/materializer handling, the frontend reducer, and migration coverage for existing notes without the new fields — sized **L**, not trivial (see ADR-0025 §2, which scopes note-patching as additive-but-nontrivial).
2. **Add note supersede/merge/split analogs.** A `SupersedeNote { superseded_id, canonical_id }` (fold provenance/basis, invalidate old) mirroring `MergeGraphNodes` (`projections.rs:1614`); optionally `SplitNote`. The graph merge/split code is a near-drop-in template.
3. **Sub-note (block/claim) granularity** if patch-level prose editing is wanted: today the atom is a whole note (title/body/tags). To "patch" a note the way an edge weight is nudged, notes would need addressable sub-elements (e.g. claim/bullet ids) so an `UpsertNoteBlock`/`InvalidateNoteBlock` can retcon one claim. This is the only piece with no existing analog.
4. **Wire diarization remaps into notes.** The `SpeakerLabelRemap` (`projections.rs:349`) currently drives ONLY `graph.supersede_entity` (`speech/mod.rs:365`). To retcon speaker attribution INSIDE materialized notes, the same remap would need to emit note ops (e.g. re-attribute a note's speaker, or invalidate+re-issue). Nothing consumes the remap on the notes side yet.
5. **True supersede-edge (vs soft-delete).** If the design wants an explicit "X was superseded BY Y" edge/pointer (provenance chain) rather than the current hide-and-recreate, both graph and notes would need a `superseded_by` field. Today supersession is implicit (valid_until + re-point), reconstructable only by reading the patch/edge log.

Bottom line: the **retcon substrate is already generic (shared patch/basis/replay/temporal model)**; graph fact-superseding is complete; notes are the WEAKER surface (whole-note replace + hard delete, no soft-invalidate, no merge/split, no sub-note granularity, and the diarization remap signal isn't consumed on the notes side). Bringing notes to parity is additive (new ops + a `valid_until_ms` field), not a redesign — the materializer, persistence, and validation plumbing already handle it for graph.

---

## Key files
- `src-tauri/src/graph/temporal.rs` — live temporal KG, `invalidate_edge`, `supersede_entity`, `snapshot`, delta.
- `src-tauri/src/graph/entities.rs` — `GraphEntity`/`GraphEdge`/`GraphDelta`/`ExtractionResult` types.
- `src-tauri/src/events.rs` — `DiarizationSpanRevisionPayload`, `AsrSpanRevisionPayload`, stability enums, event-name constants.
- `src-tauri/src/projections.rs` — `TranscriptEvent`, `TranscriptLedger`, `SpeakerTimeline`, `SpeakerLabelRemap`, `ProjectionBasis`, `ProjectionPatch`, `ProjectionOperation`, `MaterializedNotes`/`MaterializedNote`/`MaterializedGraph`(+Node/Edge), `MaterializedProjectionState`.
- `src-tauri/src/speech/mod.rs:339-427` — LIVE retcon producer (timeline.apply_event → supersede_entity → emit delta/snapshot).
- `src-tauri/src/state.rs:438-560+` — `apply_runtime_projection_patch` (validate → append JSONL → save materialized).
- `src-tauri/src/persistence/mod.rs:1173-1270` — append_projection_patch (JSONL) + save_materialized_notes/graph (whole-file JSON).
- `src/store/index.ts:233-321` — frontend note/graph patch reducers (mirror backend semantics).
- `docs/adr/0024-*.md` — the design of record (supersedes ADR-0014).
