# Speaker-label flow trace for seed audio-graph-1586

Read-only source trace, current `master`. No transcript content quoted
anywhere below — only label strings, ids, and counts, per the round-5 rules.
This trace does not touch `src-tauri/src/commands.rs`,
`src/components/SessionsBrowser.tsx`, or `ErrorBoundary` — no overlap with
the two mid-flight fix lanes (4fa5, 16e2).

## The question

Both field sessions' `speaker.jsonl` streams carry 6 distinct labels with
real per-label counts (session `57cfc64e`: Speaker 0-5, counts
56/160/32/20/46/2; session `d97bfcc3`: 34/778/1036/16/4/10), but the
knowledge graph only ever references `Speaker 1` (the two `MissingGraphNode`
WARNs at active-log lines 2586/2900). Where does the rest drop, between
transcript ingestion, the projection LLM prompt, and graph-entity creation?

## Hop 1: two independent speaker-label producers exist, and only one is
## trusted

`TranscriptEvent`/`TranscriptSegment.speaker_label` (the "inline" label) and
the persisted `SpeakerTimeline`/`speaker.jsonl` stream (the "trusted,
latest-wins" label) are **architecturally distinct** from the start. The
seam is named explicitly in `SessionViewProvider`'s doc comment
(src/session/SessionViewProvider.tsx — referenced from the earlier round-5
UI trace) and in `resolve_claim_evidence_basis_events`'s doc comment
(src-tauri/src/projections.rs:3046-3057):

> "the persisted speaker log the live path now writes... lets the frontend
> resolve trusted latest-wins speaker attribution on reload RATHER than
> trusting the inline ASR labels" — and `resolve_claim_evidence_basis_events`
> exists precisely because, without it, evidence resolution "would read
> straight off the untrusted inline ASR label the diarization override
> exists precisely to supersede (ADR-0026 §3/§4)."

**The diarization-attribution override (`SpeakerTimeline` latest-wins) is
only ever joined onto transcript data at two call sites in the whole
codebase**:
1. `crate::timeline::build_session_timeline` (src-tauri/src/timeline.rs:119-186) — the frontend's after-the-fact seek-timeline fold, replay-only.
2. `resolve_claim_evidence_basis_events` (src-tauri/src/projections.rs:3058-3094) — apply-time evidence re-judging for a patch the model has *already generated* (ADR-0037), not generation itself.

## Hop 2: the projection LLM prompt reads the untrusted inline label directly

`pinned_typed_facts` (src-tauri/src/projection_llm.rs:1723-1739) is what
builds the `speaker: {label}` fact lines the model sees in every projection
prompt (notes and graph alike — ADR-0025 §2c.3, "pinned must-never-lose
facts... which speaker said something"):

```rust
fn pinned_typed_facts(events: &[TranscriptEvent]) -> Vec<String> {
    let ordered = ordered_for_window(events);
    let mut seen = BTreeSet::new();
    let mut facts = Vec::new();
    for event in ordered {
        if let Some(speaker) = event.speaker_label.as_deref()
            && seen.insert(speaker.to_string())
        {
            facts.push(format!("speaker: {speaker}"));
        }
    }
    facts
}
```

This reads `event.speaker_label` — the **inline** field — straight off
`&[TranscriptEvent]`, with no `SpeakerTimeline`/attribution join anywhere in
this function or its caller chain. The rolling-summary digest line used for
older turns (`digest_line`, src-tauri/src/projection_llm.rs:288-300) does the
same: `event.speaker_label.as_deref().unwrap_or("Unknown")`. **Neither of the
two override call sites from Hop 1 is in this function's call graph.** The
model never sees the corrected/richer diarization attribution — only
whatever label got stamped into the ledger at ASR-ingestion time, frozen for
the life of the session (nothing rewrites `TranscriptEvent.speaker_label` in
the ledger itself; the only two overrides are read-time joins external to
the ledger, per Hop 1).

## Hop 3: graph speaker nodes are entirely model-minted, not deterministic

No deterministic Rust code constructs a `Speaker N` graph node — grepping
`src-tauri/src/graph/*.rs` and `ontology.rs` for a literal `"Speaker"` node
construction returns nothing outside test fixtures and the prompt-building
code already covered in Hop 2. The only production writer of graph
nodes/edges is the projection LLM's `upsert_node`/`upsert_edge` patch
operations, generated from the prompt `pinned_typed_facts` feeds. **Whatever
`speaker: X` facts the model sees in Hop 2 is the entire vocabulary it has to
mint or reference a speaker entity from.** This confirms the
`MissingGraphNode` WARNs are a downstream symptom of Hop 2, not a separate
bug: an edge citing a `Speaker 1` node id failed because the referenced node
row wasn't present (or had been invalidated — see Hop 4) at apply time, not
because the model was confused about which speaker said what.

## Hop 4: the "inline" label itself is not one stable source — ranked
## root-cause hypotheses for why it collapses to `Speaker 1` specifically

The inline `speaker_label` is set at exactly one of two places per segment
(src-tauri/src/speech/mod.rs, e.g. lines 6567-6577): if the ASR provider
(Deepgram) supplies its own per-word speaker index, `remap_deepgram_speaker`
formats it as `Speaker {id}` and that value is used directly, bypassing the
app's own diarizer entirely. If the provider gives nothing, the segment
falls through to the app's local `DiarizationWorker` (Simple backend — the
one active in both field sessions per the honest-degradation log lines,
`586b`) via `diarization_worker.process_input(input)`
(src-tauri/src/speech/mod.rs:6594).

### Hypothesis 1 (highest confidence): the Simple backend's distance match is fed empty audio on nearly every Deepgram call site, systematically biasing every match toward the first-ever-created profile

Of the 8 `DiarizationInput { .. }` construction sites in
`src-tauri/src/speech/mod.rs`, **7 pass `speech_audio: vec![]`** (lines 6590,
6697, 7031, 7180, 8302, 8546, 8785 — all in Deepgram-path functions) and only
**1 passes real audio**, `speech_audio: segment.audio`
(src-tauri/src/speech/mod.rs:5604, inside a VAD-placeholder-transcript flow
whose surrounding code — `text: "[speech]"`, `confidence: 0.0`, no ASR text
yet — reads as a different, non-Deepgram capture path, consistent with the
round-5 logs finding that the local-ASR-worker diarization call sites are
disjoint from the Deepgram-streaming path this session used).

Tracing what an empty buffer does:
- `AudioFeatures::extract_features(&[])` returns the degenerate all-zero
  vector unconditionally (src-tauri/src/diarization/mod.rs:642-648:
  `if audio.is_empty() { return AudioFeatures { rms_energy: 0.0, ... } }`).
- `feature_distance` between two identical zero vectors is exactly `0.0`
  (src-tauri/src/diarization/mod.rs:760-764 — each term's numerator is `0.0
  - 0.0`).
- `find_or_create_speaker_simple` (src-tauri/src/diarization/mod.rs:682-728)
  picks the existing profile with the smallest distance via `.min_by(...)`
  over `self.speakers.iter().enumerate()` (src-tauri/src/diarization/mod.rs:684-700)
  — ties resolve to the **first** minimal element in iteration order, i.e.
  the lowest-index / earliest-created profile. Since `best_dist == 0.0` is
  always `< effective_threshold` (a positive number, even after the
  `time_gap` gap-penalty halves it — src-tauri/src/diarization/mod.rs:702-717),
  **every subsequent empty-audio segment matches the very first zero-feature
  profile the worker ever created**, which is always named `Speaker 1`
  (`next_speaker_num` starts at 1, src-tauri/src/diarization/mod.rs:732-739).

Taken alone, this mechanism predicts a Simple-backend worker should never
create a *second* profile once one zero-feature profile exists (0.0 distance
never clears any positive threshold), which does not, by itself, explain
`speaker.jsonl`'s 6 distinct labels with real counts — something else must
also be creating those. That "something else" is most likely Deepgram's own
occasional per-word speaker hints (bypassing the Simple-backend match
entirely per the `segment.speaker_label.is_some()` branch,
src-tauri/src/speech/mod.rs, immediately above line 6594) or multiple
independent worker instances (Hypothesis 2). **What would confirm this
hypothesis on its own**: instrument (or replay-test) `find_or_create_speaker_simple`
against this session's actual `DiarizationInput` sequence and confirm the
empty-audio branches never diverge from index 0 — the code-level mechanism
above is deterministic and doesn't require guessing once you know
which segments hit which branch.

### Hypothesis 2 (high confidence, orthogonal to #1): multiple independent `DiarizationWorker` instances each restart their own "Speaker 1" numbering

`DiarizationWorker::new(...)` is constructed at 7 separate call sites in
`src-tauri/src/speech/mod.rs` (lines 5228, 5519, 5916, 6775, 8183, 8437,
8700) — distinct functions, plausibly one per capture-loop variant (this
session ran two simultaneous audio sources, "device" and "app", per the
active log's `stop_capture` sequence in the round-5 crash trace). Each
constructed instance owns its own `next_speaker_num` counter
(src-tauri/src/diarization/mod.rs:732 — a field on the `DiarizationWorker`
struct, not shared/global state), so **a fresh instance's first detected
speaker is always named `Speaker 1`, regardless of what any other instance
has already labeled**. Two unrelated audio streams (e.g. the "device"
microphone and the "app"/system-audio loopback) each producing their own
"Speaker 1" would be textually identical labels naming two different
real-world sources.

Graph-side, entity resolution is name-based
(`TemporalKnowledgeGraph::resolve_entity(&self, name: &str, threshold: f64)`,
src-tauri/src/graph/temporal.rs:342) — the model upserting a node for
`Speaker 1` from either source's fact block would resolve/reuse the *same*
graph entity, since nothing downstream distinguishes "device's Speaker 1"
from "app's Speaker 1." This predicts exactly the observed pattern: a single
graph-side `Speaker 1` node absorbing references from what were, on the
diarization side, two-plus independent identity spaces, while the *other*
labels in `speaker.jsonl` (Speaker 2 through 5, low counts) are each
instance's own later, more sporadic Simple-backend creations (each
constrained by Hypothesis 1's zero-feature/tie-break behavior within its own
instance) that never separately accumulate enough graph-side references to
survive as distinct entities. **What would confirm this**: check how many of
the 7 `DiarizationWorker::new` call sites are actually reached in a
two-source Deepgram session (vs. dead code for other providers/configs), and
whether `speaker.jsonl`'s label-to-timestamp pattern shows two interleaved
"Speaker 1" numbering sequences (one per source) rather than one continuous
one — the per-label counts already look like two dominant labels plus
several low-count strays in both sessions (e.g. `d97bfcc3`: 778 and 1036 are
the two large counts; 34/16/4/10 are strays), consistent with two
independent per-source identity spaces plus scattered mis-clusters, though
this pattern alone doesn't rule out Hypothesis 3.

### Hypothesis 3 (plausible, unconfirmed): per-span diarization retcons (`SpeakerLabelRemap`) fire frequently enough, span-by-span, to explain some of the label churn, but retcon results never reach the projection prompt either way

`SpeakerTimeline::apply_event` (src-tauri/src/projections.rs:528-571) DOES
support retroactive correction — but only when a **later revision of the
same `span_id`** carries a different `speaker_label` than the one currently
recorded for that span (`detect_label_remap`,
src-tauri/src/projections.rs:573-586: matched by `span_id`, not by
cross-speaker similarity). When that fires, `graph.supersede_entity` folds
the superseded label's relations onto the canonical label
(src-tauri/src/speech/mod.rs:485-490, inside
`dispatch_diarization_span_revision`) — this is the one place a diarization
signal *does* reach the graph directly, independent of the LLM prompt. Since
Deepgram interim→final promotion re-runs the diarization/labeling path per
revision of the same span (src-tauri/src/speech/mod.rs's `next_span_revision`/
`final_span_revision` bookkeeping, lines 347-366), a span whose early
(interim, possibly shorter/noisier) revision matched one profile and whose
later (final) revision matched a different one would trigger exactly this
remap. This is plausible churn but does **not** independently explain the
`Speaker 1`-only graph outcome — if anything it would tend to reduce distinct
graph-side labels over time (superseded labels get folded away), which is
directionally consistent with, but not sufficient on its own to prove,
"only Speaker 1 survives." **What would confirm/deny this**: grep the
archived session's diarization-related log lines (if retconning fired,
`dispatch_diarization_span_revision`'s `retcon_fired`/`edges_retconned`
counters would be observable if logged — check whether this path has any
`log::` instrumentation at all, since if it's silent the way `load_session_impl`
was found to be silent in the crash trace, this hypothesis may be
just as hard to confirm from logs alone as that one was.)

## Ranking and recommended next step

1. **Hypothesis 1** (empty-audio-fed Simple-backend matching) is the most
   directly confirmed by code — 7 of 8 call sites measurably pass no audio,
   and the resulting always-ties-to-first-profile behavior follows
   deterministically from the distance formula. It is necessary but not
   sufficient alone to explain the full `speaker.jsonl` label diversity.
2. **Hypothesis 2** (per-instance speaker numbering colliding at the graph
   via name-based entity resolution) is the best fit for *why the graph
   specifically collapses to one name* while the persisted stream shows
   several distinct labels — it composes with Hypothesis 1 rather than
   competing with it (each instance's own numbering is itself shaped by
   Hypothesis 1's tie-break behavior).
3. **Hypothesis 3** (per-span retcon folding) is a real, confirmed mechanism
   in the code but the weakest fit for the specific "graph only ever
   references Speaker 1" symptom, and is the hardest to confirm without
   either new logging or a repro.

None of these requires disputing Hop 1-3 (the prompt reads the untrusted
inline label, and graph nodes are model-minted from it) — they're competing
explanations for *why the inline label itself* is so thin, which is the part
Hop 1-3 alone can't answer. A repro with per-source logging of
`DiarizationWorker` instance identity + `find_or_create_speaker_simple`'s
`best_dist` values, against this session's actual two-source audio, would
distinguish all three cleanly without needing new production code.
