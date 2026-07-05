# ADR-0026: Session Timeline — "who said what when, in relation to what" (extends ADR-0024)

## Status

Proposed 2026-07-04. **Extends [ADR-0024](0024-event-sourced-notes-graph-projections.md)**
(event-sourced notes/graph projections) and relates to
[ADR-0025](0025-stt-llm-context-efficiency-and-diff-based-updates.md) (context
efficiency + diff-based retcon) and [ADR-0017](0017-unbounded-speaker-diarization.md)
(unbounded diarization). ADR-0024 established the immutable transcript log,
basis-checked `ProjectionPatch`, and the graph retcon engine; this ADR records
that a **Session Timeline** — an ordered, speaker-attributed, provenance-linked
view of the session — is a *new read-only projection over that same log plus two
integrated views*, not a new subsystem.

This ADR is **Proposed**, not Accepted — it is filed for review alongside the
design doc; no code has changed. It becomes Accepted when the first vertical slice
(read-model fold + After seek-timeline) lands. Full design + citations:
`docs/plans/2026-07-04-session-timeline-design.md`.

## Context

The product concept is "who said what, when, and in relation to what." The app
already renders *what was said* (`LiveTranscript`, a scrolling log) and *what was
derived* (`NotesPanel`, `KnowledgeGraphViewer`), but not the **join**: an ordered,
speaker-attributed sequence where each moment links forward to the artifacts it
produced and backward to the utterance that produced them. A code-grounded
inventory (`docs/plans/2026-07-04-session-timeline-design.md` §2) established the
decisive facts:

1. **Every temporal + attribution field already exists and is event-sourced.** The
   utterance unit is the span (`TranscriptEvent`, `projections.rs:36-69`, with
   `start_time`/`end_time` media-clock, `received_at_ms` wall-clock, `turn_id`,
   inline speaker). Speaker attribution is resolvable three ways, and the frontend
   JOIN is *already written* (`speakerTimeline.ts`:
   `joinSpeakerTimelineToTranscript` l225). `derive_legacy_transcript_segments`
   (`projections.rs:749-769`) already yields a duplicate-free, start-time-ordered
   segment list. **No joined ordered `(speaker, utterance, time, →relates-to)`
   projection is returned by any type or command today** — but building it is a
   pure fold.

2. **The "in relation to what" axis is only half-wired.** The only true
   single-utterance links are live-assist `AgentProposalPayload.source_segment_id`
   (`events.rs:412`) and the live `TemporalEdge.source_segment_id`
   (`temporal.rs:38`). The edge link is **stripped** before it reaches the
   frontend `GraphLink`/`GraphEdge` (`entities.rs:57-152`); the card link is
   **widened** back to a whole-window `ProjectionBasis` on approval
   (`commands.rs:2708`). Durable notes/nodes/edges carry only the whole-window
   basis (`projections.rs:1218,1500,1534`). A complete per-artifact provenance
   schema (`PromotionSourceReference`, `promotion.rs:210-223`) exists but is built
   only in `#[cfg(test)]` — zero production constructors.

3. **The retcon substrate already exists.** Bitemporal edges
   (`valid_from`/`valid_until`, `temporal.rs:19-42`), `supersede_entity`
   (`temporal.rs:344-512`), and `SpeakerLabelRemap` (`projections.rs:349-461`)
   already invalidate + re-point on speaker/entity merge. ADR-0024's basis-checked
   patch log and ADR-0025's proposed notes-side supersede ops extend the same
   substrate.

4. **The shell already answers placement.** `WORKSPACE_VIEWS =
   ["during","after","analysis"]` (`App.tsx:141`, shell design
   `docs/designs/2026-07-04-during-after-shell.md`). "Timeline" splits into a
   transcript seek-timeline (belongs in After) and a graph as-of/replay scrubber
   (belongs in Analysis); During must stay timeline-free by design.
   `KnowledgeGraphViewer` is already bitemporal but hardcodes the "now" cut
   (`materializedGraphToSnapshot` filters `valid_until_ms==null`, KGV.tsx:85-91).

The external literature confirms the shape: the timeline is a rebuildable CQRS
projection over an append-only log, corrections are new events; provenance is the
W3C-PROV `wasDerivedFrom` edge; provenance must be span-granular /
final-stability-only / support-not-context or it grows super-linearly. Every
shipping meeting tool now pairs a document surface with a time-shaped verification
layer and click-to-source provenance; reverse provenance (time-range → derived
artifacts) is the open differentiation gap.

## Decision Drivers

- The timeline is a **pure fold** over data that already round-trips through JSONL
  — it must add **no new events, no new persistence, no schema migration** for the
  read-model itself.
- The concept requires the **"in relation to what" axis**, so the design must
  close the per-utterance provenance gap (surface the stripped edge link; stop
  widening the card link) — a view-only extension is insufficient.
- Corrections (speaker relabel, late final, graph retcon) must **flow into the
  timeline for free** via re-fold, reusing ADR-0024/0025's basis + retcon
  machinery — no parallel retcon path.
- Provenance must not bloat: **span granularity, final-stability-only,
  support-not-context, GC superseded partials.**
- Placement must respect the shell's deliberate 3-phase simplification: **no 4th
  top-level stage**; integrate each timeline into the stage that owns its data.
- Prefer the **laziest sufficient mechanism**: reuse `SpeakerTimeline`,
  `derive_legacy_transcript_segments`, the temporal graph, and the bitemporal KGV
  filter; add no LLM pass, no scheduler, no store.

## Considered Options

- **Option A — Integrate into existing surfaces only.** Extend `LiveTranscript`
  with time positioning and `KnowledgeGraphViewer` with a scrubber; add no new
  model. Cheapest, but cannot deliver the "in relation to what" core: durable
  artifacts have no per-utterance backbone and the edge→utterance link is stripped
  before the UI, so it can render who-said-what-when but not
  "this utterance caused this note/decision." Ships half the concept.

- **Option B — A parallel analysis agent.** A separate projection + view with its
  own scheduler and LLM pass, like notes/graph. Maximally general, but the
  timeline is a *fold, not an inference task* — it needs no LLM, no scheduler, no
  new event stream. Duplicates the retcon/basis machinery ADR-0024 already ships
  and re-introduces the O(n²) and provenance-bloat failure modes the research
  warns against. Over-engineering for a single-writer read-model.

- **Option C — Hybrid: a new lightweight read-only timeline projection over the
  existing log + two integrated views, reusing the retcon/basis substrate
  (chosen).** One `build_session_timeline` fold; one small provenance surfacing
  (ship the stripped `source_segment_id`, wire `PromotionSourceReference`); an
  After DOM/SVG seek-timeline and an Analysis as-of scrubber that extends the
  already-bitemporal KGV filter. Inherits retcon, bitemporality, and staleness
  rejection from the existing substrate.

## Decision Outcome

Chosen: **Option C.** It is the only option that delivers the full concept (both
the temporal axis *and* the "in relation to what" axis) while remaining a pure
fold — no LLM, no scheduler, no new store, no new event, no migration for the
read-model. It reuses `SpeakerTimeline`, `derive_legacy_transcript_segments`, the
temporal graph's `valid_from`/`valid_until`, `SpeakerLabelRemap`, and the
already-bitemporal `KnowledgeGraphViewer` filter, so retcon and bitemporality come
for free. Option A cannot express provenance and ships half the product. Option B
rebuilds machinery that already exists and fights the provenance-discipline the
research prescribes. Option C also honors the shell's 3-phase simplification by
adding no fourth stage.

### Architecture (grounded in code)

#### 1. The Session Timeline read-model (the fold)

A read-only derived projection — no new events, no persistence, no migration.
Preferred: a backend `fn build_session_timeline(&TranscriptLedger,
&SpeakerTimeline, &TemporalKnowledgeGraph, &MaterializedGraph) ->
Vec<TimelineEntry>` exposed via one Tauri command (a frontend `useMemo` selector over already-loaded store slices is a
smaller fallback for the After view only). `TimelineEntry` carries `span_id`,
`start_ms`/`end_ms` (media clock, the axis), `received_at_ms` (wall clock,
as-at ordering), `turn_id`/`end_of_turn`, resolved `speaker_id`/`speaker_label`,
`text`, and `related_edge_ids`/`related_artifact_refs`. Construction reuses
`derive_legacy_transcript_segments` (order + text) ⨝ `speakerAttributionIndex`
(diarization latest-wins override) ⨝ **live `TemporalKnowledgeGraph`** edges grouped
by `source_segment_id`. The per-utterance join must fold over the *live* graph:
only `TemporalEdge` (temporal.rs:38) carries `source_segment_id`;
`MaterializedGraphEdge` (projections.rs:1300-1317) carries only the whole-window
`basis`, so folding it would leave `related_edge_ids` always empty.
`&MaterializedGraph` is passed only for the Analysis as-of scrubber (needs the
bitemporal `valid_from_ms`/`valid_until_ms` the live graph lacks). **Media-time
sort** is the backbone — it sidesteps the absence of a merged
wall-clock stream (transcript and diarization persist to separate JSONL logs). The
build is O(spans + edges); a sorted array over `latest_spans` suffices at this
app's scale (no interval tree). Because `TimelineEntry` carries raw `text` +
`speaker_label`, it must obey the enforced privacy convention: **hand-impl a
redacted `fmt::Debug`** (`REDACTED_DEBUG_VALUE` for `text`/`speaker_label`, like
`TranscriptEvent`/`MaterializedGraphEdge`), never `derive(Debug)`; and the command /
export path must emit a `sessionDataMovement` event (`DataClass` `transcript_text` +
`speaker_labels`) so the join is auditable.

#### 2. The "relates to" provenance links (extends ADR-0024 §3)

Three moves, all reusing existing schema:

- **Surface `source_segment_id` on emitted graph edges** — add the field the
  backend `TemporalEdge` already has to the frontend `GraphLink`/`GraphEdge`
  (`entities.rs:74,137`). This alone lets the timeline draw "this utterance created
  these relations" and lets the graph click through to the source moment.
- **Wire `PromotionSourceReference`** (`promotion.rs`, currently test-only) into
  the card-approval path so an approved card attaches its single-utterance
  `source_span_ids` as an *additive* citation alongside the unchanged whole-window
  basis. Do **not** narrow the basis: `current_basis()` (`commands.rs:2708`) is a
  staleness/consistency token read by `validate_basis` (projections.rs:646), not a
  citation — narrowing it would make the note re-evaluate stale only on that one
  span, regressing consistency checking. `PromotionSourceReference` is already
  separate from basis, so provenance is added without touching the staleness token.
  Id-spaces already reconcile (`source_segment_id` = `TranscriptSegment.id` =
  `"{span_id}@final"` = `ProjectionBasisSpan.span_id`), so this is wiring, not new
  design. This is the W3C-PROV `wasDerivedFrom(note, utterance)` edge, stored as a
  compact app-native (PROV-mappable, not PROV-serialized) edge.
- **Reverse provenance** — with the above, the timeline inverts the index: select
  a time range → highlight the notes/nodes derived from spans in that range (the
  MeetMap moment→artifact pattern that ships almost nowhere). A frontend index over
  `related_artifact_refs`, no new backend work.

Provenance discipline: derivation edges only from **stabilized/final** spans, at
**span granularity**, for spans that **support** a claim (corroborative, not merely
in-context/contributive), GC'd to superseded partials once the retcon window
closes — the mitigations the streaming-provenance literature requires.

#### 3. Retcon flows into the timeline (reuses ADR-0024 §4 / ADR-0025 §3)

Because the timeline is a projection, corrections are new events that re-fold; no
special path. A `SpeakerLabelRemap` updates `SpeakerTimeline` latest-wins state and
drives `supersede_entity`; the next fold reads `speakerAttributionIndex` and picks
up the new attribution automatically (ADR-0025's `ReattributeNoteSpeaker` extends
this to notes). A late final / span revision is collapsed by `TranscriptLedger`
replay, so the fold retro-updates the row and its `related_edge_ids`. Graph retcon
uses `valid_from`/`valid_until`, which the Analysis as-of scrubber reads directly:
an invalidated edge shows as *ended, not deleted* — the honest "why did this note
say X until 14:32" answer. Retcons are visibly marked, never silently rewritten.
Online diarization labels are provisional by design (ADR-0017), so the timeline
resolves speaker at fold time from the ledger rather than trusting the inline
label. **Scope:** retcon flows in for free only for **live (in-memory)** sessions.
`emit_and_dispatch_diarization_span_revision` (`speech/mod.rs:378`) applies
revisions in memory but never persists them —
`append_diarization_span_revision` has zero non-test callers, so there is no
`{session}.speaker.jsonl` to re-fold and a relabel is lost on reload. Diarization
span-revision persistence on the live path (currently test-only) is therefore a
**hard prerequisite** for cross-reload speaker retcon, not an optional enabler.

#### 4. UI surface + shell stage

- **After — transcript/turn seek-timeline (primary):** a NEW slim component,
  sibling of `LiveTranscript`, rendered as **DOM/SVG lanes** (not force-graph),
  speaker lanes positioned by `start_time` with existing CSS tokens, bounded by the
  ~200-segment cap; click a moment → scroll `LiveTranscript` (bidirectional sync).
  Reuses `materializeSpeakerTimeline`. A frontend-only selector cannot resolve
  trustworthy speakers on a *loaded* session — `LoadedSession` has no
  `diarization_events` and `loadSession` never populates `diarizationSpanRevisions`,
  so attribution falls back to untrusted inline labels; the backend fold (which reads
  `SpeakerTimeline` directly) or wiring `diarization_events` into `loadSession` is
  required before the After slice ships on reload.
- **Analysis — graph as-of/replay scrubber:** EXTEND `KnowledgeGraphViewer` by
  parameterizing `isActiveMaterializedNode`/`Edge` (KGV.tsx:85-91) by an as-of
  `sequence`/`updated_at_ms` from a DOM slider; keep the canvas graph, add only the
  scrubber. A `valid_until_ms` filter over the final graph delivers **as-of only** —
  `upsert_node`/`upsert_edge` overwrite in place (projections.rs:1504,1538), so
  superseded attribute values cannot be reconstructed by filtering. **As-at /
  as-of-until require replaying `sessionProjectionEvents` up to a sequence cut**, not
  a `valid_until_ms` filter. Both materialized-graph axes are learned-time
  (`valid_from_ms = patch.created_at_ms`); an audio-time axis lives only on the live
  `TemporalEdge`.
- **During — no timeline:** ceiling is a passive position cue inside
  `LiveTranscript`; a scrubber there fights the shell's first-note intent.

Clutter guards: tiny marker vocabulary, aggregate-then-zoom for dense stretches,
grouped citations, bird's-eye + zoom, and an accessible ordered list with time
anchors (not a bare canvas).

### Consequences

- **Positive:** The full product concept — who / what / when / in-relation-to-what
  — becomes one navigable artifact, built as a pure fold with no new events,
  persistence, or migration for the read-model.
- **Positive:** Retcon, bitemporality, and staleness rejection are inherited from
  the ADR-0024/0025 substrate; for **live sessions** a speaker relabel or late final
  retro-updates the timeline automatically. Cross-reload retcon additionally
  requires diarization span-revision persistence (a hard prerequisite, currently
  test-only), without which a relabel is lost on reload.
- **Positive:** Reverse provenance (time-range → derived artifacts) is a
  differentiator that ships almost nowhere else, reachable once the two small
  provenance wirings land.
- **Positive:** Honors the shell's 3-phase simplification — no 4th stage; each
  view lives in the stage that owns its data.
- **Negative:** Two genuine (if small) backend changes are required for the full
  "relates to" axis — surfacing `source_segment_id` on emitted edges and wiring
  `PromotionSourceReference` — so it is not purely additive view code.
- **Negative:** Durable projection-note provenance stays window-granular until/if a
  per-operation `source_span_ids` is added to `ProjectionOperation` (open question);
  card-derived provenance is span-granular first.
- **Neutral:** The backend-fold-vs-frontend-selector and scrubber-axis choices
  (both materialized-graph axes are learned-time; an audio-time axis needs the live
  graph) are deferred to the design doc's open questions; either can ship the first
  slice.

## References

- Extends: [ADR-0024](0024-event-sourced-notes-graph-projections.md) (event-sourced
  projections — the ledger/basis/patch-log/retcon substrate this folds over).
- Relates to: [ADR-0025](0025-stt-llm-context-efficiency-and-diff-based-updates.md)
  (diff-based note/graph retcon, `SpeakerLabelRemap` notes fan-out, diarization
  span-revision persistence), [ADR-0017](0017-unbounded-speaker-diarization.md)
  (provisional online diarization → revisable attribution),
  [ADR-0008](0008-conversation-ontology.md) (typed nodes/relations; temporal
  retcon), shell design `docs/designs/2026-07-04-during-after-shell.md`
  (During/After/Analysis placement).
- Design + citations: `docs/plans/2026-07-04-session-timeline-design.md` (§2
  current-state inventory, §4 architecture, §5 phased plan, §6 open questions) and
  the research ground/notes it cites.
- Code seams: read-model fold `src-tauri/src/projections.rs`
  (`TranscriptLedger`, `SpeakerTimeline`, `derive_legacy_transcript_segments`,
  `MaterializedGraph`); frontend join `src/utils/speakerTimeline.ts`
  (`materializeSpeakerTimeline`, `speakerAttributionIndex`,
  `joinSpeakerTimelineToTranscript`); edge provenance surfacing
  `src-tauri/src/graph/entities.rs` (`GraphLink`/`GraphEdge`),
  `src-tauri/src/graph/temporal.rs` (`TemporalEdge.source_segment_id`,
  `valid_from`/`valid_until`, `supersede_entity`); card provenance wiring
  `src-tauri/src/promotion.rs` (`PromotionSourceReference`),
  `src-tauri/src/commands.rs` (card approval, `current_basis()` widening);
  diarization persistence `src-tauri/src/speech/mod.rs`
  (`append_diarization_span_revision`); views `src/components/LiveTranscript.tsx`,
  `src/components/KnowledgeGraphViewer.tsx` (`materializedGraphToSnapshot`
  bitemporal filter), `src/App.tsx` (`WORKSPACE_VIEWS`); store/events
  `src/store/index.ts`, `src/hooks/useTauriEvents.ts`.
