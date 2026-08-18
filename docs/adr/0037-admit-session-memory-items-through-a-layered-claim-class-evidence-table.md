---
status: accepted
date: 2026-08-18
deciders: [maintainer]
consulted: [wayfinder 8873 frontier decision packet, docs/agentic-runs/2026-08-17-wayfinder-8873-frontier/]
extends: ADR-0034
---

# ADR-0037: Admit Session Memory Items Through a Layered Claim-Class Evidence Table

> **Provenance.** The maintainer decided this on 2026-08-18 during the wayfinder
> grilling of ticket `audio-graph-a668`, choosing among options prepared by
> agents in
> [`decision-packet.md`](../agentic-runs/2026-08-17-wayfinder-8873-frontier/decision-packet.md)
> §2 and
> [`reconciliation.md`](../agentic-runs/2026-08-17-wayfinder-8873-frontier/reconciliation.md).
> The reasoning below restates the decision packet's case for that option; the
> maintainer reviewed the distilled trade-offs and caveats recorded here, not the
> full packet. See **More Information** for how to reverse it.

## Context and Problem Statement

Nothing in AudioGraph today carries per-item evidence. Provenance is per **patch**
— a `basis`, a `ProjectionProvenance { provider, model, prompt_id }`, and one
`confidence` for the whole patch (`src-tauri/src/projections.rs:1246-1250,
1262`). A Session Memory item that says "Ana owns the migration" and an item that
says "the team never discussed rollback" are, as persisted, evidentially
indistinguishable: both inherit one patch-level confidence and one basis.

The wave that produces `Finalized Session Memory` needs to know what evidence
every admitted unit must carry, who produces it, and whether the requirement
differentiates by the *kind* of claim being made. Three facts make this a real
decision rather than a schema detail:

1. **Layered per-item citation is already shipped for exactly one class.** A live
   assist card is admitted if it cites transcript spans **or** graph context
   (`src-tauri/src/persistence/mod.rs:345-349`). Layering is precedent here, not
   novelty — but it exists for one surface, not as a rule.
2. **The only graph citation that exists is untyped and revision-less** — a bare
   `source_segment_id` string (`src-tauri/src/graph/temporal.rs:38`,
   `src-tauri/src/graph/entities.rs:48`). It cannot survive
   [ADR-0031](0031-classify-projection-bases-as-current-append-only-or-revised.md)'s
   `Revised` classification, which needs a pinned revision to detect that a
   covered span moved underneath an item.
3. **Some true statements have no citation at all.** "Nothing was said about the
   security review" is an absence-shaped claim. It is exactly the kind of
   statement a Finalized Session must be able to make honestly, and it is the one
   kind that a citation-per-item rule cannot express without inventing a
   citation.

The strict structured-output subset constrains any answer. Cerebras strict mode
requires an object root and `additionalProperties: false` everywhere, caps a
schema at **5,000 characters**, and supports **no external `$ref`** and **no
array `minItems`**. The runtime validator — not the schema — is already the
admission authority; `src-tauri/src/projection_llm.rs:281-287` documents the
schema as deliberately "marginally looser … on ranges only — never on structure
or kind", and `validate_projection_patch_draft` (`:644`) is the gate.

Ticket `audio-graph-a668` cannot be specified without settling this, and
`audio-graph-8873`, `fbca`, `1d92`, and `44c1` all consume the answer.

## Decision Drivers

- A Finalized Session must be able to report what it *lacks* without fabricating
  evidence for the absence. Under the accepted answer to Q0.2 (unmet obligations
  and unconfirmed proposals do **not** hold the `Finalized` boundary), typed gaps
  are the only mechanism left for honesty, so they must be expressible.
- Trusted metadata is stamped by trusted code, never by the model
  ([ADR-0024](0024-event-sourced-notes-graph-projections.md) §3;
  `projections.rs:1246`). Whatever the model supplies must be *checkable* rather
  than *believed*.
- [ADR-0032](0032-layer-validation-evidence-by-claim.md)'s "a command may claim
  only what it asserts" is the same principle one layer up: an item should assert
  what its evidence supports, no more.
- ADR-0031 forbids a `Revised` span from mutating notes or graph state, so an
  item's citation must pin a revision or the classifier has nothing to compare.
- The strict schema budget is hard, uncompressible (no `$ref`), and shared with
  13 existing `ProjectionOperation` variants inlined across 14 `variant(...)`
  constructions (`projection_llm.rs:287-415`).
- Cheap items must stay cheap. A one-line note should not pay a whole-session
  summary's evidence cost.

## Considered Options

- **A. Uniform span-citation floor** — every admitted item carries exactly one
  `{span_id, revision_number}`.
- **B. Layered claim-class table** — a table of claim classes with per-class
  evidence minimums; the model supplies anchors only, the backend derives
  everything trusted and judges class satisfaction.
- **C. Verified verbatim quote on every item** — every item carries a
  transcript substring that the backend verifies by containment.
- **D. Defer all per-item evidence to final refinement** — the live path stays
  as it is, and refinement reconstructs attribution at one checkpoint.

## Decision Outcome

Chosen option: **"B. Layered claim-class table"**, because it is the only
considered option that can admit an absence-shaped gap without fabricating a
citation, and because the evidence it asks the model for is checkable rather than
trusted.

The decision has four parts:

1. **A layered claim-class table with per-class evidence minimums.** Different
   claim shapes — a quoted verbatim assertion, an aggregate or inferential
   statement, an absence-shaped gap, an item whose supporting evidence is
   unavailable — carry different minimum evidence, defined per class rather than
   uniformly.
2. **The model supplies anchors only**: span ids, plus the verbatim substring for
   quoted assertions, which the backend verifies by containment. Nothing the
   model emits is trusted on its own word.
3. **The trusted backend derives everything else** — revisions, offsets, hashes,
   speaker references — and is the sole judge of whether an item satisfies its
   class. Because `minItems` is unavailable in the strict subset, every
   per-class "at least one" floor is a backend check by construction.
4. **Corrections and retractions are derived, not model-authored.** ADR-0031
   already supplies the mechanical trigger: when a pinned revision advances, the
   item's support has moved. This requires adding **`InvalidateNote`** for parity
   with the graph operations, which have `InvalidateGraphNode` and
   `InvalidateGraphEdge` while notes have only `DeleteNote`
   (`projections.rs:1269, 1285, 1299`).

Two accepted cross-cutting answers from the same sitting bind this decision and
are part of it, not context around it. **Q0.3 is NO** — Original Session Audio is
not retained in this slice — so retained-audio-range annotations are an optional
enrichment that degrades to `Unavailable Evidence`, while the annotation *shape*
must stay expressible so retention can be added later without a schema
migration. **Q0.2 is NO** — unmet obligations and unconfirmed High-Impact
Inferences do not block `Finalized` — which obliges the typed-gap classes to be
expressive enough that a Finalized Session honestly reports what it lacks. Absent
that expressiveness, Q0.2's answer degrades into silently dropping obligations.

**Recommendation confidence was medium-to-low.** This record accepts the option
at that confidence deliberately, with the caveats below stated rather than
resolved.

### Consequences

- **Positive**: an absence claim can be admitted as an absence claim. No item has
  to invent a citation to get through the gate.
- **Positive**: the model's contribution is verifiable. A claimed verbatim
  substring either is or is not contained in the transcript; a span id either
  does or does not resolve. Neither requires trusting the model.
- **Positive**: trusted metadata stays trusted-code-stamped, consistent with
  ADR-0024 §3, and pinned revisions give ADR-0031's classifier something to
  compare against — which the current untyped `source_segment_id` cannot provide.
- **Positive**: cheap notes stay cheap. The measured Notes strict schema uses 837
  of 5,000 characters across 3 variants (~1,387 characters of headroom per
  variant), so the class that needs the least evidence also has the most room.
- **Positive**: promotion gets the typed item kind it already demands
  (`promotion.rs:48-56`), and `InvalidateNote` closes a real asymmetry between
  the notes and graph operation sets.
- **Negative — this record extends ADR-0034 to new territory, and that extension
  is itself a decision made here.** ADR-0034's five conditions were written for
  **data-egress** claims and turn on "every content-bearing **producer** enabled
  in the build". Extending its "positive evidence and negative evidence have
  different logic" principle from egress claims to **knowledge** claims is
  correct in the maintainer's judgement, but the `audio-graph-a668` brief had
  claimed "two accepted ADRs jointly rule out the uniform options", treating the
  extension as settled ground that foreclosed options A and C. It did not — the
  decision packet flagged that over-claim as a correction to the brief. This ADR
  is where the extension actually becomes a decision, and it should be read as a
  choice, not as compelled by ADR-0034's text.
- **Negative — the character budget is measured and it constrains this option.**
  The Graph strict schema serializes at **2,628 of the hard 5,000-character
  ceiling** (measured 2026-08-17/18 via a throwaway probe over
  `projection_patch_strict_json_schema`, reverted after; recorded in
  [`README.md`](../agentic-runs/2026-08-17-wayfinder-8873-frontier/README.md)).
  With no external `$ref`, a per-class evidence shape must be inlined at every
  variant, leaving 2,372 characters across 10 Graph variants — approximately
  **237 characters of headroom per variant**. The budget constrains the design
  but does not void the option. Any per-class shape that cannot fit 237
  characters inlined has to be redesigned, and a future `ProjectionOperation`
  variant eats budget that this decision has already spent.
- **Negative — `minItems` is unsupported in the strict subset, so no per-class
  floor can ever be schema-enforced.** "At least one Evidence Annotation" and "at
  least one plausible alternative" are validator-only, permanently. That is
  consistent with the existing posture at `projection_llm.rs:281-287`, but it
  means the strict schema will never reject an item that violates its class
  minimum — the rejection always costs a round trip.
- **Negative — class assignment is a failure mode structural validation cannot
  catch.** A model can emit a structurally perfect item in the wrong class: an
  inference dressed as a quoted assertion, or an aggregate dressed as an absence
  claim. Every other option in this list has a smaller space of undetectable
  errors.
- **Negative — nothing in this repository measures how reliably a Cerebras-class
  model attaches correct per-item anchors under a strict schema.** This is the
  cap on the medium-to-low confidence. The fix is a deterministic offline fixture
  built **before** the schema is implemented, not after.
- **Negative — the absence class still has no coverage marker.** ADR-0034's
  named, versioned marker is a *producer inventory for egress*; it is not and
  cannot be a statement about transcript coverage.
  [ADR-0036](0036-derive-session-finalization-state-from-durable-barriers.md)
  deliberately refuses to persist or version lane coverage at all. So the absence
  class needs its **own** separately named and versioned transcript-coverage
  marker, which nobody has specified and which cannot be derived from a
  disposable per-lane predicate. Until that exists, the absence class is
  specified but not admissible.
- **Negative — per-item provenance inherits a wrong label today.**
  `chat_completion_with_schema_cached` discards `selected_provider` and
  `served_model` (`src-tauri/src/llm/openrouter.rs:1615-1616`) and the patch
  records the **requested** model (`src-tauri/src/llm/executor.rs:967`). Every
  per-item provenance claim built on this decision is mislabelled until the route
  contract ticket stamps the served route. This is a requirement this record
  imposes on `audio-graph-21e9`, not an existing behaviour it relies on.
- **Negative — re-deriving a Blocked or Finalized view becomes O(items ×
  annotations), not O(streams).** ADR-0036's best property is that a Blocked
  reason is re-derived before it is shown or retried; a per-item,
  revision-resolving evidence gate makes that re-derivation proportional to
  annotation count, and a cached head vector does not cache item-level
  annotation resolution.
- **Negative — evidence-annotated items cost materially more output tokens
  each**, so the pinned per-endpoint completion cap (40,960 for the selected
  endpoint) silently sets an items-per-partition ceiling that
  `audio-graph-fbca`'s safe-fit calculation must respect.
- **Negative — the quoted-assertion class widens the redaction and privacy
  surface, at lower volume than option C but for the same reason.** Requiring the
  model to emit the verbatim supporting substring duplicates transcript text into
  durable items, which changes the content class of `MaterializedNotes` and
  `MaterializedGraph` — the artifacts the deletion inventory and ADR-0034's
  redaction rules govern. This looks like a schema choice inside `a668` and is
  not: it moves a privacy classification that another ticket owns. The chosen
  option does not escape this cost, it only reduces its volume.
- **Negative — this is the largest spec surface of the four options**, and it
  lands on durable canonical artifacts, which is what makes it expensive to
  reverse (see More Information).
- **Neutral**: the runtime validator remains the sole admission authority. This
  decision adds obligations to the validator, not to the schema's authority.

## Pros and Cons of the Options

### A. Uniform span-citation floor

- Good, because it is the smallest schema delta of any option that adds per-item
  evidence at all — one `{span_id, revision_number}` per item.
- Good, because it generalizes a rule that already ships
  (`persistence/mod.rs:345-349`) rather than inventing one.
- Good, because exhaustive fixtures are trivial: one shape, one check.
- Good, because it fits the character budget with room to spare.
- Bad, because it **cannot represent an absence-shaped gap without a fabricated
  citation**. "Nothing was said about X" would have to point at a span that does
  not support it, which is worse than admitting nothing.
- Bad, because it treats a verbatim decision and a whole-session summary as
  evidentially identical, discarding exactly the distinction ADR-0032 draws one
  layer down.

### B. Layered claim-class table

- Good, because it is the only option that admits absence claims without
  fabricating evidence.
- Good, because verified-substring evidence is checkable rather than trusted, so
  the model's role shrinks to anchoring.
- Good, because per-class minimums keep cheap items cheap and expensive items
  honest.
- Good, because it gives promotion the typed item kind it already demands.
- Bad, because it has the largest spec surface — a class table, per-class
  minimums, per-class shapes, and a class-satisfaction judge.
- Bad, because it adds a class-assignment failure mode that structural
  validation cannot detect.
- Bad, because it spends scarce, uncompressible strict-schema budget (~237
  characters per Graph variant) that later variants will want.

### C. Verified verbatim quote on every item

- Good, because it is the most precise option: every item points at text a human
  can read.
- Good, because verification is purely mechanical — containment, nothing else.
- Good, because it is one rule with no class taxonomy to specify or get wrong.
- Bad, because it **structurally excludes aggregate and inferential support**. A
  statement supported by five scattered turns has no single quotable substring,
  so the true statement is inadmissible.
- Bad, because it excludes absence claims for the same reason as option A.
- Bad, because it duplicates transcript text into notes and graph artifacts,
  widening the redaction surface that `projections.rs:1131-1200` keeps narrow,
  and changing the content class of materialized artifacts.
- Bad, because it carries the highest token cost and the highest rejection rate,
  and every rejection today is also an egress event (see the sequencing
  constraint below).

### D. Defer all per-item evidence to final refinement

- Good, because it requires zero change to the live incremental path.
- Good, because there is exactly one checkpoint to specify, test, and observe.
- Good, because it spends no strict-schema budget on the incremental lanes.
- Bad, because it contradicts `audio-graph-1d92`'s evidence inspection, which
  needs per-item evidence to exist before refinement runs.
- Bad, because it concentrates every failure at the most expensive call in the
  system — the one whose failure is hardest to retry and most expensive to
  repeat.
- Bad, because refinement would have to reconstruct attribution after the fact,
  from items whose support was never recorded.
- Bad, because it defers rather than decides: the same question returns at
  refinement time with less information and more sunk cost.

## More Information

- **Relationship to [ADR-0034](0034-require-exhaustive-evidence-for-negative-data-egress-claims.md)**:
  this record **explicitly extends** ADR-0034's evidence-classing discipline —
  written for data-egress claims — to knowledge claims. ADR-0034 keeps its
  `accepted` status and its egress scope unchanged; nothing here alters its five
  conditions or its producer inventory. The extension is additive and is the
  decision recorded here, not a restatement of ADR-0034's text.
- **Relationship to [ADR-0032](0032-layer-validation-evidence-by-claim.md)**:
  this is the same principle at the item layer that ADR-0032 applies at the
  command layer. No conflict; ADR-0032 remains in force as written, and its tier
  3 release-blocking offline fixtures are where the anchor-reliability fixture
  named above belongs.
- **Relationship to [ADR-0031](0031-classify-projection-bases-as-current-append-only-or-revised.md)**:
  derived corrections and retractions depend on ADR-0031's classifier. The pinned
  revision this decision requires is what makes ADR-0031's `Revised` case
  detectable per item rather than per patch.
- **Relationship to [ADR-0024](0024-event-sourced-notes-graph-projections.md)**:
  the anchors-only authority split is ADR-0024 §3's "trusted metadata is stamped
  by trusted code" applied to per-item evidence.
- **Relationship to [ADR-0027](0027-file-canonical-durable-session-store.md)**:
  the evidence shape is persisted on durable canonical artifacts, so any later
  change to it is an ADR-0027 migration. That is the dominant reversal cost.
- **Relationship to [ADR-0035](0035-record-post-stop-finalization-failure-as-per-session-finalization-blocked.md)**:
  an unmet evidence obligation is **not** a `Finalization Blocked` reason.
  `Finalization Blocked` is for failures with a retry path; unconfirmed proposals
  and unresolved gaps are recorded inside a Finalized Session as typed gaps.
- **Relationship to [ADR-0036](0036-derive-session-finalization-state-from-durable-barriers.md)**:
  the most tightly coupled record. ADR-0036 derives finalization state from a
  per-lane coverage predicate that it deliberately refuses to persist or version;
  this record's absence class needs a *persisted, versioned* transcript-coverage
  marker, so the two cannot share one artifact. ADR-0036's first sub-question
  default ("notes required, graph recorded-but-not-required") makes the absence
  class inert for graph facts, and ADR-0036 already prices the O(items ×
  annotations) re-derivation cost that this decision imposes on its Blocked and
  Finalized views. Neither record supersedes the other; both must be read
  together when specifying `audio-graph-a668`.
- **Relationship to [ADR-0033](0033-enforce-mvp-provider-enablement-at-content-start.md)**:
  ADR-0033 gates provider enablement at every content-bearing start, and
  `executor.rs` contains no such gate. That gap is what makes the sequencing
  constraint below load-bearing rather than advisory.
- **Sequencing (hard constraint)**: the stricter validator this decision implies
  must be **implemented after `audio-graph-21e9`'s fallback removal**, even
  though this decision was made before `21e9`'s. Today every validator rejection
  escalates the repair prompt to the **next provider in the fallback chain**
  (`src-tauri/src/llm/executor.rs:774-780`), authorized only by a privacy boolean
  (`src-tauri/src/commands.rs:2026-2028`), with **no ADR-0033 gate anywhere in
  `executor.rs`**. Tightening evidence first would raise the rejection rate and
  therefore amplify unauthorized cross-provider egress. Decision order and
  implementation order deliberately disagree here.
- **Downstream ownership**: this decision unblocks `audio-graph-8873`,
  `audio-graph-fbca`, `audio-graph-1d92`, and `audio-graph-44c1`. `fbca` owns
  safe-fit and partition budgets under the per-endpoint completion cap; `1d92`
  owns evidence inspection over the per-item record; `44c1` and `8873` consume
  the class table. The separately named, versioned **transcript**-coverage marker
  that the absence class requires is unowned today and must be assigned before
  the absence class can be admitted. `audio-graph-21e9` owns stamping the
  **served** route into provenance, without which per-item provenance is
  mislabelled.
- **How to reverse**: expensive. Reversal changes the evidence shape persisted on
  durable canonical artifacts, so it is an ADR-0027 migration, and it consumes
  hard strict-schema budget that a successor would have to reclaim variant by
  variant. Worse, items admitted under an old floor **stay admitted**: a stricter
  successor either grandfathers them (leaving a silent two-tier corpus) or
  re-derives evidence for historical items, which needs the transcript revisions
  they pinned to still resolve. Reverse by superseding this ADR with one choosing
  option A, C, or D, and reopening `audio-graph-a668`. The cost is lowest before
  the schema and validator ship and rises steeply once Sessions carry admitted
  items.
- **Not decided here**: (a) whether an evidence-repair LLM call may be spent on a
  deterministic validation failure, and whether it must stay pinned to the
  producing route — that is two questions, and the route-pinning half depends on
  `audio-graph-21e9`; (b) whether `CONTEXT.md`'s `Knowledge Gap` definition is
  amended to admit a session-scoped sense or a second term is introduced —
  `CONTEXT.md:93-94` currently scopes it to the User World, which this map
  excludes. Both are recorded in
  `docs/agentic-runs/2026-08-17-wayfinder-8873-frontier/`.
- **Already decided elsewhere, with a consequence for this record**: which
  Projection Lanes are required for `Finalized` is **not** open —
  [ADR-0036](0036-derive-session-finalization-state-from-durable-barriers.md)'s
  first sub-question default sets "notes required, graph
  recorded-but-not-required". A graph-lane absence claim therefore has **no
  coverage basis over the final transcript**, so the absence class specified here
  is inert for graph facts unless that default is explicitly reopened. This record
  does not reopen it.
