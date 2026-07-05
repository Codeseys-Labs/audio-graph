# Session Timeline — "Who Said What When, In Relation To What"

Status: proposed (design; pairs with ADR-0026)

Date: 2026-07-04

Related: [ADR-0026](../adr/0026-session-timeline-who-said-what-when.md),
[ADR-0024](../adr/0024-event-sourced-notes-graph-projections.md),
[ADR-0025](../adr/0025-stt-llm-context-efficiency-and-diff-based-updates.md),
[ADR-0017](../adr/0017-unbounded-speaker-diarization.md),
shell design `docs/designs/2026-07-04-during-after-shell.md`.

Research base (this repo, `/tmp/timeline-research/` ground + external notes): three
code-grounded inventories (`ground-temporal-data.md`, `ground-relates-to.md`,
`ground-surface.md`) and two literature notes (`research-timeline-ux.md`,
`research-temporal-provenance.md`). Cited inline as [GROUND-T], [GROUND-R],
[GROUND-S], [UX], [ARCH].

---

## 1. Problem — the product core

The product concept is one sentence: **who said what, when, and in relation to
what.** A session is a stream of utterances; each utterance is spoken by a
speaker, positioned in time, and (often) the reason a note, a graph entity, or a
decision exists. Today the app can show *what was said* (a scrolling transcript
log) and *what was derived* (notes panel, force-graph), but it cannot show the
**join**: an ordered, speaker-attributed sequence where each moment links forward
to the artifacts it produced and backward to the utterance that produced them.

The market has converged hard on this join. Every serious meeting tool now ships
**click-a-derived-claim → jump-to-the-source-moment** (Granola's bullet
magnifier, Notion citation→line, Fireflies "click any bullet", Agency Hero
timestamp badges that survive promotion to tasks) [UX §3]. The winning surface
pairs a document-shaped primary view with a time-shaped verification layer;
Descript's lesson is that text and timeline are *projections of one time-indexed
object*, not two features [UX §2]. The differentiation gap the research flags is
**reverse provenance** — select a time range, highlight the derived artifacts —
which ships almost nowhere except MeetMap's research prototype (topic-block →
node highlighting) [UX §3, §1.3].

The concept is not a new feature bolted on; it is the spine that ties the three
existing surfaces (transcript, notes, graph) into one navigable artifact.

## 2. Current-state inventory — what already exists

The decisive finding [GROUND-T]: **every field a timeline needs is already
modeled and event-sourced. A Session Timeline is a pure fold over existing data,
not new source data.** What is missing is the *joined ordered projection* and, on
the "relates to" axis, the *per-utterance provenance link on durable artifacts*.

### 2.1 Temporal + attribution data (present today)

- **The utterance unit is the span.** `TranscriptEvent` (projections.rs:36-69)
  carries `start_time`/`end_time` (f64 sec, media clock), `received_at_ms` (u64
  unix ms), `turn_id`, `end_of_turn`, and inline `speaker_id`/`speaker_label`.
  There is no per-word timestamp; the span is the finest grain [GROUND-T §1].
- **Two clocks coexist everywhere:** f64 seconds-since-capture (position in the
  recording) vs u64 unix-ms (when the system learned it) [GROUND-T §1]. This is
  exactly the bitemporal valid-time / transaction-time split the literature calls
  load-bearing [ARCH §1.2].
- **Speaker attribution has three paths** [GROUND-T §2]: (a) inline
  `TranscriptEvent.speaker_id`; (b) the provider-neutral `SpeakerTimeline` ledger
  joined via `basis_asr_span_ids`/`basis_transcript_segment_ids`
  (projections.rs:261-262, 403-446); (c) time-overlap fallback
  `overlap_speaker_for_segment` (diarization/mod.rs:843-875). **The frontend JOIN
  is already written** — `speakerTimeline.ts`: `materializeSpeakerTimeline` (l123),
  `speakerAttributionIndex` (l158), `joinSpeakerTimelineToTranscript` (l225)
  mirror the backend latest-revision-wins rule exactly.
- **`derive_legacy_transcript_segments`** (projections.rs:749-769) already yields
  a duplicate-free, start-time-ordered `Vec<TranscriptSegment>` whose ids match
  diarization basis ids and the graph `source_segment_id` — the closest existing
  timeline backbone [GROUND-T §4].
- **Turn grouping exists as data** (`turn_id`/`end_of_turn` per span,
  `TurnEventPayload.turn_index`) but is never aggregated into a turn-grouped view;
  the frontend keeps only the last 100 turn events and shows only the latest
  (store/index.ts:1383, PipelineStatusBar.tsx:189) [GROUND-T §4].

### 2.2 The "in relation to what" axis (partial today)

This axis is where the real gap lives [GROUND-R]:

- **The only true single-utterance links** are (a) live-assist
  `AgentProposalPayload.source_segment_id` (required; events.rs:412; set from
  `segment.id`, speech/mod.rs:830) and (b) the live `TemporalEdge.source_segment_id`
  (temporal.rs:38, set in `add_relation`). The ingest path threads
  `(text, speaker, segment_id, timestamp)` together via
  `process_extraction_and_emit` → `process_extraction` [GROUND-T §3, GROUND-R §2].
- **But both links leak or are stripped.** The live `TemporalEdge.source_segment_id`
  is **dropped** from the frontend `GraphLink`/`GraphEdge` render types
  (entities.rs:57-152), so the UI cannot click through from a relation to its
  utterance [GROUND-R §2]. And on card approval the durable patch re-attaches the
  **full-window** `ProjectionBasis` (commands.rs:2708 from `current_basis()`), so
  the precise single-utterance link the card carried is **lost** when it becomes a
  durable note/node [GROUND-R §3].
- **Durable notes/nodes/edges carry only a whole-window `ProjectionBasis`**
  (projections.rs:1218, 1500, 1534) — you can attribute a note to a *session
  window of N spans*, never to the one utterance. `ProjectionOperation` has no
  per-op source-span field and the LLM prompt forbids emitting provenance
  (projection_llm.rs:215) [GROUND-R §1].
- **A complete per-artifact provenance schema already exists but is unwired:**
  `PromotionSourceReference { source_span_ids, source_basis, source_projection_sequence }`
  + `PromotionSourceProvenance { span_revisions }` (promotion.rs:210-223, 189-206)
  is only ever built in `#[cfg(test)]` helpers — zero production constructors, no
  Tauri command [GROUND-R "unwired schema"].
- **Id-spaces reconcile**, so linkage is *feasible, not built*: proposal
  `source_segment_id` = `TranscriptSegment.id` = `"{span_id}@final"` =
  `ProjectionBasisSpan.span_id` [GROUND-R cross-cutting]. A basis *can* be matched
  to an utterance; no code does it.
- **`graph_context_ids`** — the intended "relates to which existing entities"
  field — is never populated (always `Vec::new()`) [GROUND-R §3].

### 2.3 Retcon substrate (present today)

The temporal graph already models validity + retcon: `TemporalEdge.valid_from`/
`valid_until` (temporal.rs:19-42) and `supersede_entity` (temporal.rs:344-512)
invalidate + re-point edges on speaker/entity merge, driven by `SpeakerLabelRemap`
(projections.rs:349-361) [GROUND-T §3]. ADR-0024's basis-checked patch log and
ADR-0025's proposed notes-side supersede ops extend the same substrate. The
literature confirms this stack is independently convergent with consensus:
event-sourced projection + bitemporal facts + retraction propagation, corrections
as new events never in-place edits [ARCH §1, §6]. The proposed timeline sits
*on top* of this — it never needs its own store.

### 2.4 Shell surface (present today)

The During/After/Analysis shell already answers most of the placement question
[GROUND-S]. `WORKSPACE_VIEWS = ["during","after","analysis"]` (App.tsx:141).
"Timeline" is actually **two surfaces** that split cleanly:

- **During** must stay timeline-free — design intent is live-present + first-note;
  it "does not render the graph or projection diagnostics by default"
  (design:21,40). Ceiling: a passive position cue inside `LiveTranscript`.
- **After** (review) is the natural home for a **transcript/turn seek-timeline**
  that scrolls `LiveTranscript` to a clicked moment — an affordance it lacks today
  (design:22,39).
- **Analysis** owns graph + provenance; it is the home for a graph **as-of/replay
  scrubber**. `KnowledgeGraphViewer` is *already bitemporal* but hardcodes the
  "now" cut (`materializedGraphToSnapshot` filters `valid_until_ms==null`,
  KGV.tsx:85-91). An as-of scrubber is a small extension of that filter, not a new
  surface [GROUND-S §4].

No new IPC / backend event is needed for the view — every temporal signal already
flows into the Zustand store via `useTauriEvents.ts` [GROUND-S §3].

## 3. Decision

**Adopt Option C — a hybrid: a new lightweight, read-only Session Timeline
projection over the existing event log, surfaced as two views integrated into the
stages that own their data, reusing the ADR-0024/0025 retcon + basis substrate.**

Why not the two alternatives:

- **Option A (integrate only — extend `LiveTranscript`/`KnowledgeGraphViewer` with
  no new model)** is insufficient for the *product core*. The "in relation to
  what" axis has no per-utterance backbone on durable artifacts [GROUND-R]; a pure
  view extension can render who-said-what-when but cannot answer
  "this utterance caused this note/decision" because the data is not joined and
  the provenance is stripped/widened. It would ship half the concept.
- **Option B (a full parallel analysis agent — a separate projection + view like
  notes/graph, with its own scheduler and LLM pass)** is over-engineered. The
  timeline is a **pure fold**, not an inference task [GROUND-T TL;DR]; it needs no
  LLM, no scheduler, no new event stream. A parallel agent would duplicate the
  retcon/basis machinery ADR-0024 already ships and re-introduce the O(n²) and
  provenance-bloat failure modes the literature warns about [ARCH §5]. The
  research is explicit that provenance must be span-granular, final-stability-only,
  support-not-context, and GC'd — a heavyweight always-on agent fights all four.

Option C is the **laziest sufficient design**: it adds one read-model fold, one
small provenance surfacing, and two integrated views — and it inherits retcon,
bitemporality, and staleness rejection for free from the substrate that already
exists. It also matches the shell's deliberate 3-phase simplification: **do not
add a 4th top-level stage** [GROUND-S §recommendation].

## 4. Architecture

### 4.1 The Session Timeline read-model (the fold)

A read-only derived projection — **no new events, no persistence, no schema
migration** [GROUND-T §5]. Preferred home: a backend `fn build_session_timeline`
folding the three in-memory structures, exposed via one Tauri command; a
frontend-only `useMemo` selector is an even smaller fallback for the After view.

```
build_session_timeline(&TranscriptLedger, &SpeakerTimeline,
    &TemporalKnowledgeGraph, &MaterializedGraph) -> Vec<TimelineEntry>

TimelineEntry {
    span_id,                    // stable join key
    start_ms, end_ms,           // media clock (f64→ms), the timeline axis
    received_at_ms,             // wall clock, for as-at ordering when needed
    turn_id, end_of_turn,       // turn grouping
    speaker_id, speaker_label,  // resolved via SpeakerTimeline latest-wins
    text,
    related_edge_ids: Vec<..>,  // live TemporalEdges whose source_segment_id == span_id
    related_artifact_refs,      // notes/decisions once §4.2 wires provenance
}
```

**Two distinct graph inputs, deliberately.** `related_edge_ids` must fold over the
**live `TemporalKnowledgeGraph`**, whose `TemporalEdge.source_segment_id`
(temporal.rs:38) carries the per-utterance link. `MaterializedGraphEdge`
(projections.rs:1300-1317) has **no** `source_segment_id` — its only source link is
the whole-window `basis` — so folding the per-utterance join over `&MaterializedGraph`
would yield an always-empty `related_edge_ids` and silently break the forward
"relates to" link (Phases 1+3) [GROUND-R §2]. `&MaterializedGraph` is passed only
for the Analysis as-of scrubber (§4.4), which needs the bitemporal
`valid_from_ms`/`valid_until_ms` fields the live graph lacks. (Adding
`source_segment_id` to `MaterializedGraphEdge` is the alternative — a real schema
change — but the live-graph fold avoids it.)

Construction reuses what exists:
`derive_legacy_transcript_segments` (order + text + inline speaker) ⨝
`speakerAttributionIndex` (diarization override, the ready-made "who said what"
resolver) ⨝ **live** `TemporalKnowledgeGraph` edges grouped by `source_segment_id`
(the "relates to" links) [GROUND-T §5, GROUND-R §2]. **Media-time sort** (by
`start_time`) is the backbone — it sidesteps
the fact that transcript and diarization persist to separate JSONL logs and there
is no merged wall-clock stream on disk [GROUND-T §4]. The build is O(spans +
edges); at this app's scale (thousands of spans/session) a sorted array + binary
search over `latest_spans` is adequate — no interval tree needed unless stabbing
queries get hot [ARCH §1.4].

**Privacy substrate (required, not optional).** `TimelineEntry` carries raw `text`
+ `speaker_label`, so it must obey the enforced convention that no who-said-what
reaches logs/telemetry: **do NOT `derive(Debug)`** — hand-impl `fmt::Debug`
substituting `REDACTED_DEBUG_VALUE` for `text`/`speaker_label`, exactly as sibling
types do (`TranscriptEvent`, `MaterializedGraphEdge`, `ProjectionPatch` — 22 such
usages in projections.rs). And because the fold is exposed as a command and is
"reusable (export, eval)" (Q1), any command/export path that materializes it must
emit a data-movement ledger event (`sessionDataMovement.ts`:
`artifact_loaded`/`artifact_exported`, `DataClass` = `transcript_text` +
`speaker_labels`, `DestinationBoundary` local/export) so the join is auditable in
the privacy report.

### 4.2 The "relates to" provenance links (utterance → note / entity / decision)

Three targeted moves, in increasing depth, all reusing existing schema:

1. **Surface `source_segment_id` on emitted graph edges** (the one small backend
   change [GROUND-T gap #4]). Add the field the backend `TemporalEdge` already
   has to the frontend `GraphLink`/`GraphEdge` payload (entities.rs:74,137). This
   alone lets the timeline draw "this utterance created these relations" and lets
   the graph click through to the source moment.
2. **Wire the unwired `PromotionSourceReference`** (promotion.rs) into production.
   When a live-assist card is approved into a durable patch, attach the card's
   single-utterance `source_span_ids` as an **additive** citation *alongside* the
   unchanged whole-window `ProjectionBasis` [GROUND-R §3, GROUND-R §1]. Critically,
   **do not narrow the basis**: the `ProjectionBasis` (commands.rs:2708, from
   `ledger.current_basis()`) is a staleness/consistency token consumed by
   `validate_basis` (projections.rs:646), not a citation — narrowing it to one span
   would make the derived note re-evaluate stale only when that single span changes,
   not when the window it was built over changes, regressing consistency checking.
   `PromotionSourceReference` (promotion.rs:210-223) is already a separate structure
   from basis, so provenance is added without touching the staleness token. The
   id-spaces already reconcile [GROUND-R cross-cutting]; this is wiring, not new
   design. This is the W3C-PROV `wasDerivedFrom(note, utterance)` edge the
   literature names as the exact vocabulary [ARCH §2.1], stored as a compact
   app-native edge (PROV-mappable, not PROV-serialized).
3. **Reverse provenance (the differentiator)** [UX §3]. With (1)+(2) in place, the
   timeline can invert the index: select a time range → highlight the notes/nodes
   derived from spans in that range (MeetMap's moment→artifact pattern, which
   ships almost nowhere [UX §1.3]). This is a frontend index over
   `related_artifact_refs`, not new backend work.

Provenance discipline follows the research to avoid bloat [ARCH §5]: record
derivation edges only from **stabilized/final** spans, at **span granularity**
(never token, never "whole session"), for spans that **support** a claim
(corroborative) not merely spans in LLM context (contributive), and GC provenance
to superseded partials once the retcon window closes.

### 4.3 How retcon flows into the timeline

The timeline is a projection, so corrections are just new events that re-fold — no
special path [ARCH §1.1]. **Scope of "for free":** this is automatic for **live
(in-memory) sessions** only. `emit_and_dispatch_diarization_span_revision`
(speech/mod.rs:378) applies revisions to `SpeakerTimeline` + `supersede_entity` in
memory but **never persists them** — `append_diarization_span_revision` has zero
non-test callers (the only call, commands.rs:10312, is under `#[cfg(test)]`), so
there is no `{session}.speaker.jsonl` retcon log to re-fold on reload and a speaker
relabel is **lost for any loaded/After session**. Cross-reload retcon therefore
requires diarization span-revision persistence (Phase 6), which is a **hard
prerequisite**, not an optional enabler:

- **Speaker relabel:** a `SpeakerLabelRemap` (projections.rs:349-461) updates the
  `SpeakerTimeline` latest-revision-wins state and drives `supersede_entity` on
  the graph. The next `build_session_timeline` fold picks up the new attribution
  automatically because it reads `speakerAttributionIndex`, which mirrors the
  latest-wins rule [GROUND-T §2, §3]. ADR-0025's proposed
  `ReattributeNoteSpeaker` fan-out extends the same signal to notes.
- **Late final / span revision:** `TranscriptLedger` replay already collapses
  partials into their final and rejects stale revisions; the fold re-runs over
  `latest_spans`, so a corrected utterance retro-updates its timeline row and its
  `related_edge_ids` [GROUND-T §4, ARCH §3].
- **Graph retcon:** edges carry `valid_from`/`valid_until`; the Analysis as-of
  scrubber reads exactly these fields (§4.4). An invalidated edge shows as ended,
  not deleted — the honest "why did this note say X until 14:32" answer the
  literature calls a trust requirement [ARCH §2.2, §5]. Retcons should be
  **visibly marked** in the UI, not silently rewritten [ARCH §5].

Online diarization labels are provisional by design [ARCH §3, ADR-0017], so
speaker identity is a *revisable attribution edge*, never baked into derived text
— which is precisely why the timeline resolves speaker at fold time from the
ledger rather than trusting the inline label.

### 4.4 UI surface + shell stage

Two views, integrated into the stage that owns each one's data [GROUND-S]:

- **After — transcript/turn seek-timeline (primary).** A NEW slim component,
  sibling of `LiveTranscript` in the After grid, rendered as **DOM/SVG lanes**
  (not force-graph; that lib is 2-D spatial, transcript time is 1-D). Speaker lanes
  / turns positioned by `start_time`, styled with existing CSS tokens; bounded by
  `LiveTranscript`'s ~200-segment cap. Click a moment → scroll `LiveTranscript` to
  it (bidirectional sync, the Ferret/Otter pattern [UX §1.1,§1.3]). Reuses
  `materializeSpeakerTimeline` for lane data. **Constraint:** a frontend-only
  selector cannot resolve trustworthy speakers on a *loaded* After session —
  `LoadedSession` (types/index.ts:2171-2180) has no `diarization_events` and
  `loadSession` (store/index.ts:2545-2568) never populates
  `diarizationSpanRevisions`, so `joinSpeakerTimelineToTranscript` is a documented
  no-op on reload (LiveTranscript.tsx:56-64) and attribution silently falls back to
  the untrusted inline ASR `speaker_id`. The export bundle already carries
  `diarization_events` (types:2196), so the data exists but is unwired. **Do not
  ship the frontend-selector-only After slice** until one of two lands: (a) add
  `diarization_events` to `LoadedSession` and populate `diarizationSpanRevisions` in
  `loadSession`, or (b) do the fold in the backend command, which reads
  `SpeakerTimeline` directly and sidesteps the store gap.
- **Analysis — graph as-of / replay scrubber.** EXTEND `KnowledgeGraphViewer`:
  parameterize `isActiveMaterializedNode`/`Edge` (KGV.tsx:85-91) by an as-of
  `sequence`/`updated_at_ms` from a DOM slider over `sessionProjectionEvents[].sequence`.
  Keep the canvas graph; add only the DOM scrubber. **Scope the query modes
  honestly:** a `valid_until_ms` filter over the *final* `MaterializedGraph`
  delivers **as-of only** — because `upsert_node`/`upsert_edge` overwrite in place
  (`*existing = next`, projections.rs:1504,1538), the final graph cannot reconstruct
  a node's superseded attribute values (e.g. an old name before a rename). True
  **as-at / as-of-until** requires **replaying `sessionProjectionEvents` up to a
  sequence cut**, not filtering `valid_until_ms`; implement those via replay if the
  UI needs them [ARCH §1.2].
- **During — no timeline.** Ceiling is a passive elapsed/position cue inside
  `LiveTranscript`; a scrubber there fights the shell's first-note intent
  [GROUND-S, design:21,40]. Consistent with the UX finding that live transcript is
  a distraction-recovery rail, not a reading/scrubbing surface [UX §4].

Clutter guards from the research [UX §5]: keep the marker vocabulary tiny (Teams
ships only mentions/shares/joins); aggregate-then-zoom for dense stretches; group
citations so users don't go marker-blind; offer bird's-eye + zoom; render as an
accessible ordered list with time anchors, not a bare canvas.

## 5. Phased build plan

Effort S/M/L/XL. "Executable now" = no upstream blocker in this plan.

| # | Pillar | Area | Effort | Executable | Depends on |
|---|---|---|---|---|---|
| 1 | `build_session_timeline` read-model fold + Tauri command (ordered `TimelineEntry`, media-time sort, speaker + edge join) | timeline-projection | M | yes | — |
| 2 | Surface `source_segment_id` on emitted `GraphLink`/`GraphEdge` (the one small backend change) | provenance-linkage | S | yes | — |
| 3 | After transcript/turn seek-timeline view (DOM/SVG lanes, click→scroll `LiveTranscript`) | timeline-ui | M | yes | 1 |
| 4 | Analysis graph as-of/replay scrubber (parameterize KGV bitemporal filter) | timeline-ui | M | yes | — |
| 5 | Wire `PromotionSourceReference` so approved cards add span-granular provenance next to the whole-window basis (basis stays the staleness token; provenance is additive) | provenance-linkage | L | yes | 2 |
| 6 | Diarization span-revision persistence on the live path (`append_diarization_span_revision` is test-only today) so speaker retcons replay — **hard prerequisite for any cross-reload retcon (Phase 7), not an optional enabler** | diarization | S | yes | — |
| 7 | Retcon reflection into the timeline (speaker relabel + late-final re-fold; visibly mark superseded rows/edges) | retcon-integration | M | no | 1, 6 |
| 8 | Reverse provenance: select time range → highlight derived notes/nodes (MeetMap differentiator) | notes-linkage | L | no | 1, 2, 5 |

Sequencing: 1+2+4+6 are independent and can land first (2 is a small, low-risk
enabler; 6 is small in effort but a hard prerequisite — no cross-reload retcon
works without it). 3 builds on 1. 5 depends on 2. 7 depends on 1+6 (6 must land
before any "retcon works automatically" claim holds on reload). 8 is the
capstone differentiator depending on 1+2+5. The first shippable slice is **1 → 3**
(a read-only After timeline with speaker attribution and forward edge links),
which delivers the who-said-what-when core with zero new persistence.

## 6. Open questions

1. **Backend fold vs frontend selector for the read-model.** Backend
   `build_session_timeline` is reusable (export, eval, future cross-session) and
   keeps the join authoritative; a frontend `useMemo` is smaller and ships the
   After view faster. Recommendation: backend command for the authoritative join.
   A frontend selector is acceptable for the first After slice **only for live
   sessions** — on a loaded/After session it resolves untrusted inline labels
   until the `loadSession` diarization gap (§4.4) is closed, so the backend fold is
   the safer first slice for reload. Which first?
2. **Turn-grouped vs per-utterance granularity for the After lanes.** Turn data
   exists but is unaggregated [GROUND-T §4]. Per-utterance is simpler; turn charts
   [UX §1.3] read better for participation. Start per-utterance, add turn grouping
   as a zoom level?
3. **Explicit per-session watermark.** The literature recommends promoting the
   implicit Partial→Final stability into an explicit queryable "timeline complete
   through t" value [ARCH §1.3] so the timeline can visibly distinguish settled
   vs still-forming regions. In scope for the timeline, or a separate concern?
4. **Provenance for LLM-projection notes (not just approved cards).** §4.2 wires
   card-derived provenance; durable projection notes still carry only whole-window
   basis and the LLM is told not to emit per-op provenance [GROUND-R §1]. Do we add
   a per-operation `source_span_ids` to `ProjectionOperation` (a real contract
   change), or accept window-granular provenance for projection notes and
   span-granular only for card-derived ones?
5. **Reverse-provenance highlight scope.** Corroborative (spans that support the
   claim) vs contributive (spans merely in context) [ARCH §5]. We can only compute
   contributive today (whole-window basis); corroborative needs §4.2/Q4. Ship
   contributive with a clear "in context" label, or wait for corroborative?
6. **As-of scrubber time axis:** `ProjectionPatch.sequence` vs `valid_from_ms`.
   Note both are **learned-time**: `MaterializedGraphNode`/`Edge` set
   `valid_from_ms = patch.created_at_ms` (projections.rs:1496,1530), u64 wall-clock,
   the same clock as `sequence`. There is **no audio-time axis on the materialized
   graph at all** — audio-time seconds live only on the live `TemporalEdge.valid_from`,
   a different structure. So an audio-time graph scrubber would require the live
   graph, not a filter over the materialized one. `sequence` is simpler and matches
   "replay how the graph was built". Consistent axis across both views, or a live-graph
   axis when audio-time is wanted?
