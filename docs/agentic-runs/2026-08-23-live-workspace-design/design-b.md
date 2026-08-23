# Design B — contract-first: the living document as an evolution of the notes contract

Designer B, live-workspace epic `audio-graph-a6b5`. Designed bottom-up from the
persisted contract outward. The four ratified decisions in `constraints.md` are
taken as fixed; everything here is below them.

Every claim about current behavior below was read out of the code at HEAD
(commit `070824e`). Line references are to that tree.

---

## 0. The one fact that decides the whole design

`projection_patches` is read by a **strict** canonical reader.
`persistence/canonical_reader.rs:163` → `load_strict_canonical_stream`, and
`commands.rs:12537`
(`strict_reader_corrupt_projection_patches_log_fails_closed_instead_of_reseeding_nothing`)
pins the behavior: a record that fails to deserialize fails the **whole stream**
closed, it is not skipped.

`ProjectionOperation` (`projections.rs:1470`) is an internally-tagged serde enum,
and `ProjectionPatch` (`projections.rs:1237`) / `ProjectionOperation` carry **no
`#[serde(deny_unknown_fields)]`** (verified: the only `deny_unknown_fields` in
this subsystem is on `ProjectionPatchDraft`, `projection_llm.rs:185` — the
*model-facing* type, never persisted).

Those two facts together give an asymmetry that is the spine of this design:

| Wire change | Old build reading a new session's log |
| --- | --- |
| New `ProjectionOperation` **variant** (`upsert_doc_section`) | unknown enum tag → deserialize error → **strict reader fails the whole `projection_patches` stream**. Every patch in that session is lost, not degraded. |
| New `ProjectionKind` **variant** (`kind: "doc"`) | same, and worse: it kills the graph lane's replay in that session too. |
| New **optional field** on an existing variant (`heading_level`) | serde ignores unknown fields → record deserializes, patch applies, session replays. Renders as a note card. **Degraded, never broken.** |

ADR-0045's obligation is "no accepted patch may be silently discarded." A new
variant does not silently discard patches; it **loudly discards all of them.**
That is the e700 precedent read correctly: e700 was blocker-class not because a
type changed but because a *pre-e700 accepted log had to keep replaying*
(`projections.rs:6585`, `:6690`, `:6786`, `:6892` are four separate
replay-tolerance fixtures for exactly that). e700's chosen technique was
never "new op vocabulary" — it was **tolerant resolution inside the existing
vocabulary** plus a carried-forward `node_id_redirects` map on the graph itself
(`projections.rs:1866-1879`).

**Design B applies e700's technique, not a new vocabulary.**

---

## 1. Projection contract: evolve `upsert_note`, do not mint doc ops

### 1.1 Decision

The living document is the **existing notes projection, re-read as an ordered
list of document sections.** No new operation variants. No new
`ProjectionKind`. No new materialized type.

The mapping is already almost complete in the data we have:

| Document concept | Existing contract carrier |
| --- | --- |
| section heading | `UpsertNote.title` |
| section content (paragraph lines + nested bullets) | `UpsertNote.body` |
| section order = document order | `MaterializedNotes.notes: Vec<_>` order, maintained by `ReorderNote` |
| move a section | `ReorderNote { id, after_id }` (`projections.rs:1741`) |
| remove a section | `DeleteNote { id }` |
| topical tags | `UpsertNote.tags` |
| per-section provenance | `MaterializedNote.evidence` / `basis` / `provenance` (already per-note) |
| **heading depth** | **missing — the only gap** |

Exactly one thing is missing, and it is a scalar.

### 1.2 The one additive field

```rust
// src-tauri/src/projections.rs, ProjectionOperation::UpsertNote
UpsertNote {
    id: String,
    title: String,
    body: String,
    tags: Vec<String>,
    #[serde(default)]
    evidence: crate::claim_evidence::EvidenceAnchor,
    /// Document heading depth for `title`, 2..=4 (2 = top-level section).
    ///
    /// `None` is NOT a class-satisfying absence and NOT "level 2" — it means
    /// this operation asserted no document structure at all: a pre-living-
    /// document session's card op, or a model that omitted the field. The
    /// renderer's legacy mode (§4.1) handles `None`; it never fabricates a
    /// depth into the materialized record. Mirrors the posture of
    /// `MaterializedNote::evidence`'s `Option` doc comment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    heading_level: Option<u8>,
}
```

Same treatment on `MaterializedNote` (`projections.rs:1567`), set by
`upsert_note` (`:1712`) exactly like every other field it copies through.

**Why a scalar and not a `DocSectionMeta { heading_level, parent_id }` struct:**
`parent_id` plus `Vec` order is two sources of truth for one tree, and they can
disagree (the classic bug). Markdown itself has no parent pointers — nesting is
*derived* from order + depth. Order + `heading_level` is one source of truth, and
every inconsistency it can produce is renderable rather than erroneous (a level-4
section directly after a level-2 just nests one deeper; the renderer clamps to
`previous + 1`). Cost accepted: moving a subtree is N `reorder_note` ops rather
than one `parent_id` write. §2.1's guidance tells the model to restructure rarely.

### 1.3 Body grammar: a validated subset, not "markdown"

`body` stays a `String` — no contract field changes. What changes is that the
*prompt* and an *ingest normalizer* pin a bounded grammar, and the renderer
parses that grammar directly with **no markdown library and no HTML**:

```
body    := line ( "\n" line )*
line    := bullet | paragraph | ""
bullet  := indent "- " text          ; indent ∈ {"", "  ", "    "} → depth 0/1/2
paragraph := text                    ; any other non-empty line
text    := plain characters, NO inline emphasis / links / code fences
```

Rationale, in priority order:

1. **XSS.** Note bodies are model-derived from arbitrary speech.
   `KnowledgeGraphViewer.tsx:63-71` already carries an `escapeHtml` guard for
   precisely this reason ("entity text is model-derived from arbitrary speech, so
   all interpolated values must be escaped — XSS guard, critique H7"). A markdown
   renderer re-opens that surface. This grammar renders to React text nodes only;
   there is no HTML path at all.
2. **No new dependency.** `package.json:45-57` has no markdown renderer. Adding
   `react-markdown` + `rehype-sanitize` to render model output is a trust-boundary
   dependency for a feature that needs headings and two levels of bullets — which
   is precisely what ratified decision 1 asked for and no more.
3. **Testable in Rust.** The grammar is checked backend-side, so the frontend and
   any future exporter agree by construction.

### 1.4 Where structure is enforced: the ingest normalizer, never the validator

`parse_projection_patch_draft` (`projection_llm.rs:644`) already has exactly the
right seam: `normalize_projection_patch_draft_ontology` (`:682`) runs **after**
structural validation and **only on fresh ingest, never on replay** — its doc
comment (`:669-681`) states the ADR-0045 reasoning verbatim. Add one sibling
called from the same place:

```rust
fn normalize_projection_patch_draft_doc_structure(draft: &mut ProjectionPatchDraft) {
    for op in &mut draft.operations {
        if let ProjectionOperation::UpsertNote { heading_level, body, .. } = op {
            // Clamp instead of refuse: a cosmetic depth must never fail a
            // patch that carries real content.
            *heading_level = heading_level.map(|l| l.clamp(2, 4));
            *body = normalize_doc_body(body); // `*`/`+` → `-`; indent → 0/2/4
                                              // spaces; strip leading `#` markers
                                              // (headings live in `title`);
                                              // strip inline emphasis markers.
        }
    }
}
```

`validate_operation` (`projection_llm.rs:1151`) is **not** touched for structure.
Refusing a draft over a heading level or a stray `**` would discard real content
over formatting — the exact trade ADR-0045 forbids. `normalize_doc_body` is
length-preserving in the sense that matters: it never truncates and never drops a
line, only rewrites markers.

### 1.5 The delta primitive: section granularity + an ingest no-op filter

The field data (93/93 single-utterance verbatim captures, 379 upserts / 0 delete
/ 0 reorder, 72% of repeat upserts carrying zero net information) says the
redundancy is a *mutation-primitive* problem. Design B's answer has two halves,
and neither is a new op:

**(a) Append is emergent from section granularity + `reorder_note`.** Full-replace
upsert is only expensive when a section is large. Under the document model, "add a
bullet about pricing tiers" is either a small upsert of a *short* section or an
upsert of a **new, small subsection** placed with
`reorder_note { id, after_id: <parent's last child> }`. The existing vocabulary
already supports append semantics; the model has simply never been told the unit
is a section and has emitted `reorder_note` **zero times** in the measured
session. That is a prompt failure being misread as a contract gap.

**(b) A no-op filter at draft admission, never at apply.** Add to
`parse_projection_patch_draft`'s caller
(`trusted_projection_patch_from_model_json`, `projection_llm.rs:705`, which
already has the live materialized notes available via the same path
`projection_patch_prompt_messages` uses) a drop of any `UpsertNote` whose
`(title, body, tags, heading_level)` tuple is **byte-identical** to the current
materialized note under that id.

This is ADR-0045-clean because it happens **before the patch is persisted**: the
accepted log never contains the dropped op, so replay is a pure re-derivation of
a log that was always this shape. It is the same seam and the same
"fresh-ingest-only, never replay" contract the ontology normalizer already
occupies. It must **not** move to `MaterializedNotes::apply_patch`, which is on
the replay path.

**(c) When all operations drop, persist the empty patch.** Verified consequence:
`derive_coverage_heads` (`projection_scheduler.rs:1332-1354`) picks the
max-`sequence` patch **per kind, blind to operation count**. So an
`operations: []` patch advances the lane's coverage head correctly, and
`MaterializedNotes::apply_patch` handles it (the `for` loop simply does not run;
`next.last_sequence = patch.sequence` still commits — `projections.rs:1665-1707`).
`validate_projection_patch_draft` accepts an empty list (`operations` is
`#[serde(default)]`, and the validation loop is vacuous — `:1136-1146`). There is
no `operations.is_empty()` early return anywhere in the tree (grepped), so
persist-empty needs **no new code — only the discipline not to add a skip.**

Suppressing the empty patch instead would leave the coverage head un-advanced,
and the next `observe_ledger` would re-queue the same basis forever. Naming this
because it is the trap a "don't write empty patches" instinct walks into.

The record "the model had nothing to add at this basis" is also the honest
ADR-0045 artifact: the ledger keeps proving what happened.

### 1.6 Seed 7462 (notes-lane redundancy): **subsumed — do not double-fix**

§1.5(a)+(b) plus §2's emit-delta prompt rule **are** 7462's fix. Ship no separate
dedupe pass. What should survive from 7462 is its **measurement**: the
byte-identical / tags-only-churn ratio becomes this epic's acceptance metric
(§6). Recommend 7462 be closed as "fixed by the living-document delta contract"
with its metric re-homed, not closed as duplicate-and-forgotten.

### 1.7 Migration table (explicit, per the standing constraint)

| # | Case | Wire shape | New build | Old build (downgrade / mixed install) |
| --- | --- | --- | --- | --- |
| 1 | Pre-doc session, replayed on new build | `upsert_note` with no `heading_level` | `heading_level: None` → renderer legacy mode (§4.1): every note is a depth-3 section in `Vec` order. Materialized state **byte-identical** to today. | n/a |
| 2 | New doc session on new build | `upsert_note` + `heading_level: 2..4` | document renderer | — |
| 3 | New doc session read by an **old** build | one extra JSON key | — | serde ignores unknown fields on `ProjectionOperation`; strict reader passes; renders as note cards with the same title/body/tags. **Degraded, never broken.** |
| 4 | New doc session read by an **old frontend bundle** | one extra key | — | TS `ProjectionOperation` (`types/index.ts:1808`) is structural and `applyProjectionNotesPatch` (`store/index.ts:424-465`) reads only `id`/`title`/`body`/`tags`; the extra key is inert. |
| 5 | No-change tick | `operations: []` | `last_sequence` advances; coverage head advances (§1.5c) | identical — no special case exists on either side |
| 6 | `invalidate_note` in any log | existing variant | Rust: hard delete (`projections.rs:1682`). TS: absent from the union → `default: break` → **ignored**. Pre-existing Rust/TS divergence, out of scope; flagged so it is not mistaken for a regression of this epic. | same |
| 7 | **Rejected:** new op variant | `upsert_doc_section` | — | strict reader fails the entire `projection_patches` stream |
| 8 | **Rejected:** third `ProjectionKind` | `kind: "doc"` | — | same, plus it kills the graph lane's replay in that session |

**Migration action required: none.** No conversion pass, no `schema_version`
bump. Specifically do **not** bump `MaterializedNotes::SCHEMA_VERSION`
(`projections.rs:1630`): verified there is no reader gate on it —
`load_materialized_notes` (`persistence/mod.rs:1407-1414`) is a bare
`load_json`, and the TS side never reads `schema_version` either. Bumping a
version nobody checks is theater; the canonical log is the source of truth and
the snapshot is a rebuildable cache.

### 1.8 Schema surfaces that must move in lockstep

Three schemas describe the same wire shape and are hand-maintained separately:

1. `projection_patch_draft_json_schema` (schemars-derived, `projection_llm.rs:340`)
   — automatic from the Rust type. `require_evidence_on_content_creating_operation_variants`
   (`:363`) is unaffected (it keys on the presence of an `evidence` property).
2. `projection_patch_strict_json_schema` (hand-authored, `:445`) — **must** gain
   `("heading_level", nullable_integer())` on the `upsert_note` variant
   (`:552-561`). Strict mode requires every property in `required` and nullable
   via `["integer","null"]`, matching the existing `nullable_string()` posture
   (`:451`). Omitting this is a silent regression: the model would be *forbidden*
   from emitting the field on the strict OpenRouter route (`additionalProperties:
   false`) while being *told* to emit it by the system prompt.
3. TS `ProjectionOperation` (`types/index.ts:1808`) — add `heading_level?: number | null`
   to the `upsert_note` arm.

A parity test asserting variant-field sets match across (1) and (2) for `Notes`
would have caught this class of drift; recommend adding one with this ticket.

---

## 2. Prompt shape

Current notes prompt (`projection_patch_prompt_messages`, `projection_llm.rs:829`):

```
[0] system   : instructions + operation_guidance + EVIDENCE_GUIDANCE + full schema   ← byte-stable
[1] user     : pinned facts + rolling summary                                        ← byte-stable, append-only
[2] user     : hot-window transcript verbatim
[3] user     : "Current notes state" snapshot (Notes kind, notes.is_some())           ← variable region
[4] user     : job metadata (basis hash, span count, job id)                          ← per-tick volatile
```

`PROJECTION_STABLE_PREFIX_MESSAGE_COUNT = 2` is respected by every change below:
all edits land either inside message `[0]`/`[1]` as *constants* (one-time cache
invalidation at rollout, then stable forever) or in the variable region.

### 2.1 Delta A — `operation_guidance`, Notes arm (`projection_llm.rs:860-869`)

Replaced wholesale. It must remain a single constant with **no runtime branching**
— the existing comment at `:850-859` explains why (branching on `notes.is_some()`
leaks a per-tick variable into the stable prefix), and the replacement keeps the
same conditional phrasing ("if a block appears").

```
Maintain ONE living markdown document for this session. Each upsert_note is one
document SECTION: `title` is the section heading, `heading_level` is its depth
(2 = top-level section, 3 = subsection, 4 = sub-subsection), and `body` is that
section's content as plain lines and `- ` bullets, indenting two spaces per
nesting level, at most two levels deep. Never put headings, links, bold, or code
fences in `body`. Section order IS document order: reorder_note moves a section,
delete_note removes one. A section is a TOPIC that accumulates across many turns
— never one section per utterance and never one section per quote. On each tick
emit an operation ONLY for a section whose title, body, tags, or heading_level
actually changes; re-emitting an unchanged section is a defect. To add one point
to an existing topic, prefer adding a short new subsection under it over
rewriting a long section. If a "Document outline" block appears below, it lists
every existing section id in document order with its heading and depth — reuse
those ids exactly, and mint a new id only for a genuinely new topic. If this turn
adds nothing to the document, return {"operations": []}.
```

Five behavior changes are carried by that text, each traceable to a measured
defect: section-not-card (93/93 utterance captures), topic-not-quote (same),
emit-only-deltas (72% zero-information upserts), prefer-new-subsection-over-rewrite
(body oscillation), and `{"operations": []}` is legal (which nothing in the
current prompt ever says, so a model with nothing to add invents something).

### 2.2 Delta B — `EVIDENCE_GUIDANCE` (`projection_llm.rs:58-66`): re-rank, do not weaken

This constant is shared with the repair prompt by design (`:55-57`), and it is
ADR-0037 machinery. Change is **one appended sentence**, no class list edit, no
judge change:

```
For a synthesized topic section, the right class is normally `grounded_inference`
anchored to the newest span that motivated THIS edit; use `verified_quote` only
when the section body actually contains that span's words verbatim.
```

The measured 93/93 `verified_quote` rate is largely this text's ordering effect:
`verified_quote` is listed first and described most concretely ("literal, verbatim
substring"), so a schema-obeying model optimizes for the class it can most
reliably satisfy — by quoting. Naming `grounded_inference` as the synthesis
default is the minimum intervention.

**Contract friction that must be stated, not designed around:**
`EvidenceAnchor` holds **one** `span_id`, and `MaterializedNote::evidence`
(`projections.rs:1588`) stores one `AdmittedClaimEvidence`. A topic section
spanning twenty turns therefore anchors to *the span that caused this edit*, not
to the topic. That is honest and already admissible (the span must be inside the
basis window — `projection_llm.rs:65-66`). Widening the anchor to a span list
touches a type ADR-0037 validates and would change what the judge proves;
**per-section multi-span provenance is out of scope for this epic and needs its
own ADR.** Flagged, not smuggled.

### 2.3 Delta C — replace `render_notes_snapshot` with a document outline

`render_notes_snapshot` (`projection_llm.rs:1033`) sorts by
`updated_by_sequence` **descending** — recency order. Its own doc comment
(`:1028-1032`) already flags the resulting hazard: "this list is recency-ordered,
which is NOT necessarily the notes' actual display order… a `reorder_note` the
model emits from reading this block targets that real Vec order, not the recency
order shown here." For a card list that was a wart. For a living document it is
disqualifying — the model cannot maintain a structure it is shown scrambled, and
it explains `reorder_note`'s **zero** uses in the field data.

New block, same position (variable region, after the stable prefix — an existing
section's content can be rewritten, so it can never move into `[0]`/`[1]`):

```
Document outline (section ids in document order — reuse exactly; mint a new id
only for a genuinely new topic):
[s-pricing]        h2  Pricing
[s-pricing-tiers]  h3  Tier structure  · 3 bullets · 380 chars
     recent: "- Enterprise tier gates SSO behind the top plan"
[s-pricing-disc]   h3  Discount policy · 1 bullet  · 96 chars
[s-migration]      h2  Migration plan
...
34 sections total; body previews shown for the 8 most recently changed.
```

Budget constants (replacing `NOTES_SNAPSHOT_*`, `projection_llm.rs:78-94`):

| Constant | Value | Note |
| --- | --- | --- |
| `DOC_OUTLINE_MAX_CHARS` | 6000 | hard cap on the whole block |
| `DOC_OUTLINE_ID_MAX_CHARS` | 48 | was 80 (`NOTES_SNAPSHOT_ID_TITLE_MAX_CHARS`) |
| `DOC_OUTLINE_TITLE_MAX_CHARS` | 64 | was 80, same constant |
| `DOC_OUTLINE_PREVIEW_SECTIONS` | 8 | previews only for the most recently changed |
| `DOC_OUTLINE_PREVIEW_MAX_CHARS` | 120 | was 160 (`NOTES_SNAPSHOT_BODY_SUMMARY_MAX_CHARS`) |

Degradation order when the budget binds: **drop previews first** (all of them),
then truncate heading lines from the *least recently changed* end, always
emitting the truthful trailing count line. That preserves 253c's guarantee —
every section id is "shown or counted, never silently dropped"
(`projection_llm.rs:1022-1026`) — while making the structural information the
part that survives longest.

**Token effect: the prompt gets smaller.** Today's worst case is
30 × (80 id + 80 title + 160 body) ≈ **9,600 chars** of un-cacheable
variable-region text every tick. New worst case ≈ **6,000 chars**, and the
typical case is far below it because previews are capped at 8 sections
(≈ 40 × 110 + 8 × 120 ≈ 5,360). Net ≈ **−3,600 chars ≈ −900 prompt tokens per
tick**, while *adding* full document-order id visibility (today only 30 ids are
visible at all) and heading depth.

`ProjectionPromptShape` (`:752`) fields rename `notes_snapshot_chars` /
`notes_snapshot_entries` → keep the names (they feed the data-movement ledger,
ADR-0025 §2g, and they stay content-free counts of the same block). Do not add
fields; the movement facts stay counts-only per ADR-0025.

### 2.4 Completion-token budget and the 4096 floor

`STRUCTURED_OUTPUT_MIN_MAX_TOKENS = 4096` (`llm/openrouter.rs:2068`) is flat and
kind-blind by deliberate choice (`:2059-2067`), and ADR-0038 forbids runtime
escalation after a `Truncated` result. **Therefore the only ADR-0038-compatible
lever on truncation is emitting fewer bytes** — which is exactly what §1.5/§2.1
buy. Stated as a target rather than a hope:

| Metric | Measured today (session c95d21e6) | Target |
| --- | --- | --- |
| upserts / patch | 379 / 83 ≈ 4.6 | ≤ 2.0 |
| byte-identical re-upserts | 37% | 0% (filtered at ingest, §1.5b) |
| tags-only churn | 35% | ≤ 5% |
| ticks emitting `operations: []` | n/a (impossible today) | ≥ 15% |
| notes-lane `Truncated` results | (not currently split by kind) | 0; add the per-kind counter |

Delete `delete_note` from nothing — it stays available and should finally get
used (0 uses measured) for a section the conversation abandons.

---

## 3. Scheduler impact

### 3.1 Lanes stay two. The doc lane **is** the notes lane.

`ProjectionSchedulers` (`projection_scheduler.rs:1045-1057`) is a hardcoded
two-field struct, and `ProjectionKind` has ~257 match/construct sites across 11
files (`projection_llm.rs` 63, `projection_scheduler.rs` 67, `projection_eval.rs`
28, `projections.rs` 22, `speech/mod.rs` 23, `llm/executor.rs` 17, `commands.rs`
14, `persistence/mod.rs` 14, `state.rs` 7, +2). A third variant is a large
mechanical change **and** row 8 of §1.7's migration table (it breaks old readers
at the enum level, taking the graph lane down with it) **and** a third LLM call
per tick against a measured ~650 ms of headroom. Three independent reasons, any
one sufficient.

`ProjectionKind::Notes` keeps its wire value `"notes"` — renaming it to `"doc"`
is exactly the same replay break in a costume.

**Scheduler structural change required: none.** No config change to
`coalesce_span_threshold: 2` or `ttft_estimate_ms: 1200`
(`projection_scheduler.rs:25-26`). New cadence work hooks `ProjectionSchedulers`;
there is none to hook.

### 3.2 A correction the latency budget depends on

The recon calls the pacing "TTFT-aware". Read as code, **`ttft_estimate_ms` does
not gate anything.** In `observe_ledger` (`:478-566`) its only consumer is
`coalescing_reason` (`:931-944`), which returns a *diagnostic label*
(`PendingSpanThreshold` / `InFlightAgeThreshold` / `TtftWindow`) attached to the
`Coalesced` decision. `coalesce_span_threshold` likewise only picks the label. The
actual pacing rule is simpler and stricter:

> **One in-flight job per lane. Any basis change while a job is in flight
> coalesces into the pending slot. Any basis change with the lane idle starts a
> job immediately.**

Two consequences that a naive reading of "650 ms headroom" gets backwards:

1. **Generation latency is self-limiting, not budget-bounded.** A slower lane
   coalesces more and runs fewer, larger ticks; it never queues up. The failure
   mode of a heavy prompt is therefore not saturation — it is **the document
   lagging speech**, which for the notes lane is the product's companion surface
   (ADR-0045 decision 4 explicitly grants the *graph* lane unbounded lag and
   withholds that from notes).
2. **A faster lane raises tick count.** Cutting generation latency shortens the
   in-flight window, so fewer spans coalesce per job and more jobs run per
   minute. The ~650 ms headroom figure (generation ≈ 81% of a ~3.85 s median tick
   interval at 15.4-15.8 ticks/min) is a *coupled* measurement, not an
   independent budget: banked latency partly re-spends itself as tick count.

### 3.3 Stated budget

- p50 notes-lane generation ≤ **3.1 s**, p95 ≤ **3.8 s** (hold at or below the
  measured 81%-of-median position; §2.3 removes ~900 prompt tokens/tick and
  §1.5/§2.1 remove most completion tokens, so this should improve, and the
  ticket must prove it rather than assume it).
- Prompt: variable-region block ≤ 6000 chars (§2.3). Stable prefix grows by the
  one-time constant edits in §2.1/§2.2 — a **single** cache invalidation at
  rollout, then byte-stable forever.
- Watch `notes` lane `coalesced_updates` / accepted-patch count. A rise means the
  doc is lagging speech — the thing to protect. This is a
  `ProjectionSchedulerMetrics` field that already exists (`:40`, incremented at
  `:501`); it needs surfacing, not inventing.
- If tick rate rises past ~16/min after the delta contract lands and that proves
  costly, the lever is a **configured** floor on job start spacing, not a runtime
  escalation and not a third lane. Measure before designing it (ADR-0029 posture).

---

## 4. Frontend: document renderer, KG strip, agent tile

### 4.1 Document renderer

`NotesPanel.tsx` renders `materializedNotes.notes` as `<li class="ag-card">`
cards (`:361-370`, `MaterializedNoteItem` `:436-477`). Replace that section with a
`<article>` document:

- Group into sections by `Vec` order. Each note → `<section>` with
  `<h{clamp(heading_level ?? 3, prevLevel+1, 4)}>{title}</h{...}>` and a parsed
  body (§1.3 grammar → `<p>` / nested `<ul><li>`), all as React text nodes.
- **Legacy mode** (`heading_level == null` on every note, i.e. any pre-doc
  session): every note renders at depth 3 in `Vec` order. Reads as a flat
  document with one heading per former card — which is exactly what a card list
  *was*. No dead-end empty state, no "unsupported session" message.
- Keep `data-note-id={note.id}` on the section element: it is the existing
  cross-component anchor and e2e hook.
- Delete `MaterializedNoteItem`'s per-card revision-count line and
  `notePatchRevisionCounts` (`:416-434`). Under the delta contract a high
  revision count stops meaning "churn defect" and starts meaning "actively
  developed topic"; keeping a yellow warning on it would be actively misleading.
  Move the churn signal where it belongs: the §2.4 acceptance metric.
- **0922 trap:** when replacing the card classes, *delete* the BEM/utility
  properties rather than layering over them, and verify in the production bundle
  (unlayered `styles/index.css` barrel beats `@layer` recipes).

**Two recommendations that touch ADR-0014, flagged rather than assumed:**

1. `NotesPanel`'s client-derived ontology sections (participants / questions /
   tasks / decisions / topics, `:193-218`, `:372-408`) are derived from
   `graphSnapshot`, i.e. they are a *graph* projection wearing notes clothing.
   With a live KG strip directly above the document, keeping them duplicates the
   strip's content and competes with the document for the reader. **Recommend
   removing them from the notes tile in phase 1.**
2. The manual `synthesize_notes` result is rendered *above* the materialized
   notes today (`:303-320`). A one-shot prose summary sitting above a
   continuously-maintained living document is two competing answers to one
   question. **Recommend keeping the command as an export/"prose summary" action
   and not rendering its output inline.**

Both change ADR-0014's ("notes synthesis") two-layer model, which the living
document decision partly supersedes. That deserves an ADR update in this epic —
recording it is cheap and the alternative is an ADR that silently describes a
surface that no longer exists.

### 4.2 KG strip: data source and staleness

**Source: `materializedProjectionGraph`, falling back to `graphSnapshot`.** That
is already the rule `KnowledgeGraphViewer.tsx:96-99` implements
(`materializedGraphToSnapshot(materializedProjectionGraph) ?? graphSnapshot`).
Do not write a second copy of it — **extract it into one shared selector**
(`src/session/useActiveGraphSnapshot.ts`) consumed by the strip, the full canvas,
and the change feed, so the three display modes can never disagree about what the
graph is.

Why the projection graph and not the legacy snapshot: it carries per-node
`updated_by_sequence` / `updated_at_ms` / `valid_until_ms`
(`projections.rs:1772-1791`), which is what makes both "recently mentioned" and
an honest recency line computable at all.

**"Recently mentioned", defined honestly:** the nodes touched by the last **3**
graph patches — take the 3 highest distinct `updated_by_sequence` values present
among nodes with `valid_until_ms == null`, take those nodes (cap 12), then include
edges whose *both* endpoints are in that set, plus one hop. This is recency in
**patch** terms, not speech terms, and the label must say so ("recently updated"
/ "last 3 graph updates"), never "currently being discussed". The graph lane's
lag is unbounded by design (ADR-0045 decision 4); a label claiming live speech
would be false roughly 42% of the time (~82 of ~195 applies carried
`MissingCurrentSpan`).

**Total-size counter** (ratified): node + edge counts from the *full* snapshot,
not the focused subset — that is the counter's whole point.

**Staleness, phased so phase 1 needs zero Rust:**

- *Phase 1 (frontend only).* "Graph as of HH:MM:SS · N entities · M relations",
  where the time is the `created_at_ms` of the last `kind === "graph"` patch in
  `sessionProjectionEvents` (already in the store, `store/index.ts:1719-1722`).
  Neutral tone. **The word "Live" does not appear.** This is an OBSERVED,
  cheap, true claim.
- *Phase 2 (one small Rust change).* `basis_currency_at_apply` exists but never
  leaves the backend — it lives on `ProjectionRuntimeApplyResult`
  (`state.rs:403`) and is only logged (`speech/mod.rs:2905-2915`). Add it as an
  **additive, defaulted, `skip_serializing_if = "Option::is_none"`** field on the
  emitted patch (same forward/backward-compat property as §1.2, and
  `AppliedBasisCurrency` already has a serde round-trip test,
  `projections.rs:7239`). Then the strip earns a real chip, routed through the
  **T2 tone law** (`components/settings/readinessTone.ts`): `data-tone="success"`
  only when the last applied graph patch's basis was `Current`;
  `AppendedTail{..}` demotes to neutral "behind live speech". This is the tone
  law's first non-settings surface, which is precisely what its doc comment
  anticipates ("later surfaces get the same law by construction").

**Rendering under an exploded relation vocabulary (123 types / 73 single-use,
seed 9366):**

- Edges: **one uniform stroke.** No color-by-relation, no legend, no
  filter-by-relation control. Any of those would harden an open vocabulary into
  a UI contract that seed 9366 is still designing, and a 73-entry legend is
  useless anyway.
- `relation_type` appears only in the hover tooltip, through the existing
  `escapeHtml` guard (`KnowledgeGraphViewer.tsx:63-71`) — model-derived text.
- Nodes: color by `entity_type` **is** safe. It has been a closed 10-name
  ontology enum since e700, bound at the schema level
  (`projection_llm.rs:476-484`) with a deterministic ingest-side fallback
  (`ontology::normalize_entity_type`).
- Textual change-feed mode: read `sessionProjectionEvents` filtered to
  `kind === "graph"`, one line per operation. Note that it therefore shows the
  *session's* patches, and `sessionProjectionEvents` is unbounded in the store —
  cap the rendered feed (e.g. last 100 lines), do not cap the store array here
  (that would silently change replay-lens behavior).

### 4.3 Agent tile: what the store already has, and the one thing to add

**No new store slices are needed for phase 1.** Verified inventory:

| Need | Existing state |
| --- | --- |
| actionable queue | `agentProposals` (pending only; cleared on approve/dismiss, `store/index.ts:1851`, `:1884`) |
| activity feed incl. approved/dismissed + outcome | `liveAssistCards` (`status`, `outcome`, `projection_patch_sequence`, `updated_at_ms`) |
| in-flight approve spinner | `approvingAgentProposalIds` |
| "agent is working" | `agentStatus` |
| approve / ask / dismiss / clear | existing actions → `approve_agent_proposal` (ADR-0013, 4b52 timestamp-safe) — **unchanged** |

Two extractions, no new state:

1. Move `mergeLiveAssistCards` / `liveAssistCardFromProposal`
   (`AgentProposalsPanel.tsx:50-85`) into `src/store/selectors/agentQueue.ts` and
   derive **two** lists from the one merge: `queue` = `status === "pending" &&
   agentProposals.has(id)` (the *actionable* set — the existing
   `actionableProposalIds` distinction at `:119-122` is already exactly right and
   must survive), `feed` = everything else, newest first.
2. **The 104f hook.** Define `queue = merged.filter(isActionable).filter(admitToQueue)`
   where `admitToQueue` is a single pure predicate in
   `src/store/selectors/agentQueueAdmission.ts`, shipped in phase 1 as
   `() => true` with a doc comment naming 104f. When 104f's question-fragment
   gating lands, it replaces that one function body and the tile does not change
   — which is the constraint's requirement ("slots in without a tile redesign")
   satisfied by construction rather than by intent.

Two honesty constraints on the tile:

- `agentProposals` is capped at `slice(-49)` (`store/index.ts:1777`) and
  `liveAssistCards` is session-scoped. The feed heading must read "Recent
  activity", never "All activity".
- **Do not reintroduce a global pending-count badge.** Its removal was
  deliberate (`SystemDrawer.tsx:17-23`, reach reduction). A count inside the
  tile's own header is in-tile information; a badge on shell chrome is reach.

Mount: the tile becomes the right sidebar (ratified layout), replacing the
current `hasAgentActivity`-gated full-width strip below notes+transcript
(`App.tsx:506-513`). Keep the `hasAgentActivity` gate (`App.tsx:573-577`) — it
also feeds the get-started fallback exclusion at `:1042-1052`, so removing it has
reach beyond this tile.

### 4.4 Bento phase 1: a grid change, not a layout engine

`.workspace-panel--capture` (`layout.css:116-119`) is a 2-column × 2-row grid
today. Phase 1 is a **fixed** 3-column arrangement — transcript left, KG strip
above notes document center, agent tile right — expressed as
`grid-template-columns` / `grid-template-areas` plus the existing 1120px
single-column reflow (`:143-161`). No drag, no resize, no persistence machinery.

The phase-2 persistence schema may be *designed* now: a `workspaceLayout`
zustand slice of `{ tiles: Record<TileId, { visible: boolean; order: number; size: number }> }`
with `TileId = "transcript" | "graph" | "document" | "agents"`, persisted
alongside existing UI prefs. Ships in phase 2. Phase 1 must not read it — a
phase-1 code path that reads a layout store is drag machinery with the handles
filed off.

### 4.5 Tolerating 64e3 / fa56

The document and the KG can already cite text the transcript tile lacks (64e3
tail loss). Concretely: if a section's evidence span is absent from
`transcriptSegments`, render the section **without** a jump-to-transcript
affordance rather than with one that goes nowhere. No error, no banner — the
tiles disagree quietly until those lanes are fixed. Same rule for the change
feed's span references.

### 4.6 i18n (add-only, en + pt, pt chips ≤ 18 chars)

New keys: `notes.docEmpty`, `notes.docAsOf`, `notes.docSectionCount`;
`graphStrip.title`, `graphStrip.asOf`, `graphStrip.totals`, `graphStrip.expand`,
`graphStrip.modeStrip` / `modeCanvas` / `modeFeed`, `graphStrip.recentlyUpdated`;
`agent.queueTitle`, `agent.queueEmpty`, `agent.feedTitle`, `agent.feedEmpty`.

The three mode values render as chips, so they are pt-budgeted: "Faixa" (5),
"Tela" (4), "Mudanças" (8) — all well inside 18. Add them to the iterated key
list in `src/i18n/settings-chip-length-budget.test.ts` (it reads real `pt.json`
values, so the check stays live), or add a sibling `workspace-chip-length-budget.test.ts`
if keeping that test settings-scoped is preferred. `locale-parity.test.ts` covers
the add-only en/pt parity automatically.

---

## 5. Rust / frontend work split for the one-lane box

Constraint: max **one** Rust-compiling lane at a time. The split below keeps
three frontend tickets runnable in parallel with whichever single Rust ticket is
active, because **F1 is not blocked on R1** — the document renderer's legacy mode
(§4.1) works against today's data, where every note is `heading_level: None`.

| # | Lane | Ticket | Depends on | Notes |
| --- | --- | --- | --- | --- |
| R1 | Rust | `heading_level` on `UpsertNote` + `MaterializedNote`; strict-schema + schemars + TS type parity (§1.8); doc-body normalizer (§1.4); replay fixtures: pre-doc log tolerance, new-log-old-reader byte compat, empty-ops patch advances the coverage head | — | contract only, **no prompt change** — ships dark |
| R2 | Rust | prompt deltas A/B/C (§2.1-2.3); outline renderer replacing `render_notes_snapshot`; ingest no-op filter (§1.5b) | R1 | the behavior change; one prompt-cache invalidation at rollout |
| R3 | Rust | additive `basis_currency_at_apply` on the emitted patch (§4.2 phase 2) | R1 | optional / later; unlocks the strip's tone-law chip |
| F1 | Frontend | document renderer + legacy mode; remove card revision-count line; ADR-0014 recommendations (§4.1) | — | **parallel with R1/R2** |
| F2 | Frontend | shared `useActiveGraphSnapshot` selector; KG strip + display-mode switch; "as of" recency line; uniform edge rendering (§4.2) | — | parallel |
| F3 | Frontend | bento phase-1 fixed grid; agent tile extraction; `agentQueueAdmission` predicate module (§4.3-4.4) | — | parallel |

Serialization requirement: **R1 → R2 → R3, never concurrent.** F1/F2/F3 are
independent of each other and of the Rust lane.

---

## 6. Acceptance metrics (what proves this shipped correctly)

1. **Replay.** A pre-living-document session fixture replays to byte-identical
   `MaterializedNotes` before and after R1. A living-document session's
   `projection_patches` log deserializes under the *pre-R1* `ProjectionOperation`
   type (a serde-level test, not a prose claim).
2. **Redundancy.** Byte-identical re-upserts → 0%; tags-only churn ≤ 5%; upserts
   per patch ≤ 2.0 (§2.4). This is seed 7462's metric, re-homed.
3. **Structure.** `reorder_note` and `delete_note` usage > 0 in a live session
   (both are 0 today; nonzero is the signal that the model is maintaining a
   document rather than appending cards).
4. **Latency.** p50 notes generation ≤ 3.1 s; variable-region prompt block
   ≤ 6000 chars; notes-lane `Truncated` count 0.
5. **Honesty.** No surface in the workspace renders the word "Live" (or a success
   tone) for graph freshness without a `Current` basis behind it.

---

## 7. Open questions and deliberate non-answers

1. **Per-section multi-span provenance.** `EvidenceAnchor` is single-span; a topic
   section spanning twenty turns anchors to the span that caused *this* edit
   (§2.2). Widening it is an ADR-0037-touching change and needs its own ADR.
2. **Outline collapse past ~50 sections.** The 6000-char budget fits ~40-45
   heading lines. Long sessions will exceed that. Deliberately not designed:
   measure real section counts first (ADR-0029 posture — gate the mechanism on a
   measured breach).
3. **`InvalidateNote` Rust/TS divergence.** Rust hard-deletes
   (`projections.rs:1682`); TS ignores it (`default: break`). Pre-existing,
   unrelated to this epic, worth its own seed. `MaterializedNote` still has no
   `valid_until_ms`, which `ProjectionOperation::InvalidateNote`'s own doc
   comment flags as unscoped (`:1490-1511`).
4. **Tick-rate rise after the delta contract.** §3.2 predicts a faster lane runs
   *more* ticks. Measure ticks/min before and after R2 before deciding whether
   any spacing floor is warranted.
5. **586b diarization-fallback banner.** The bento grid gains a natural
   workspace-level notice row above the tiles. Hook noted; not designed here.
6. **Hand-editing the document.** Out of scope per non-goals (phase 1 is
   read-only, model-owned). Worth noting the contract already almost supports it:
   a user edit would be a trusted-code-authored `UpsertNote` needing a
   provenance class distinct from the model's, which is an ADR-0037 question, not
   a UI one.
