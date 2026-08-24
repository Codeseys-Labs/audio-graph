---
status: accepted
date: 2026-08-24
deciders: [AudioGraph maintainers]
---

# ADR-0048: Compact the Projection-Basis Summarized Prefix Behind a Content-Hash Digest (amends ADR-0031)

## Context and Problem Statement

`ProjectionBasis::span_revisions` (ADR-0031) named every transcript span a
basis covered, individually, forever. Field session `d97bfcc3` showed one
materialized graph node accumulating 933 `span_revisions` entries despite
`summarized_through_revision: 12` — growth is `O(items touched × live
spans)`, superlinear per session hour, and a single 9.5-minute session
already produced a 158-entry basis with 146 of those entries already folded
into the rolling summary (ADR-0025 §2c) and never sent to the projection LLM
again. A 3-hour session projects a persisted artifact over 1GB.

ADR-0031 step 1's Decision Outcome states that "every completion basis
records the ordered covered transcript identities and revisions, covered
count, and hash," and its classifier proves currency by "compar[ing] the
subset hash and ordered identities/revisions with the basis." That
per-identity list is exactly what grows without bound. Truncating it at the
embedding sites (`ProjectionPatch`, `MaterializedNote`,
`MaterializedGraphNode`, `MaterializedGraphEdge`) would require four
independent truncation calls with no single place to enforce or audit the
invariant, and ADR-0042 requires every historical v1 basis to stay
byte-for-byte replayable — compaction cannot be a mutation of the frozen
hash-v1 serialization.

## Decision Drivers

- Persisted artifact size must stay bounded per item, not grow with session
  length or how many times an item is re-touched.
- Compaction must be an invariant the basis type enforces at construction,
  not a convention scattered across every embedding site.
- ADR-0042 byte-stability: a historical v1 basis's on-disk shape and
  `transcript_hash` value must never change.
- ADR-0031's detection power (revision, deletion, reorder, and hash
  corruption inside the covered set all classify `Revised`) must not
  silently weaken for a compacted basis.
- The rolling-summary window (ADR-0025 §2c / seed `audio-graph-18ee`) already
  defines exactly which turns stop being sent to the LLM verbatim
  (`ROLLING_SUMMARY_HOT_WINDOW_TURNS`) — the basis's own compaction boundary
  should be the same boundary, not an independently tuned one.

## Considered Options

### Truncate `span_revisions` at each embedding site, keep only a count

Cheapest to write, but every one of the four embedding sites (plus any
future one) has to remember to call it, and a truncated basis with no
covered-count/hash proof for the dropped spans cannot be re-verified — a
revision inside the truncated-away region would go undetected. Rejected:
this is exactly the "scattered enforcement" ADR-0042/this ticket's
architectural shaping explicitly ruled out.

### Store the full prefix identity list, just move it out of the hot embedding path (e.g. a side table keyed by basis hash)

Preserves ADR-0031's ordered-identity proof for the whole covered set, but
adds a second durable structure with its own lifecycle, GC, and replay
semantics, and does not shrink the embedded artifact — the ticket's field
evidence is about the size of the artifact ITSELF (78% of a 156.6MB
projection graph), not about where the identity list lives. Rejected.

### Fold the covered-but-summarized prefix into a `(span_count, content_hash)` digest, verified by hash reconstruction against the ledger

Bounds `span_revisions` to at most `ROLLING_SUMMARY_HOT_WINDOW_TURNS` entries
regardless of session length, matches the rolling-summary boundary that
already exists for an unrelated reason (so no new tuning knob), and keeps
detection power by re-deriving the prefix's exact event set from the ledger
and comparing its hash to the recorded digest before trusting it — a
revision, deletion, or content change anywhere in the prefix changes the
digest. Chosen option.

### Bump `hash_version` to a new value for a compacted basis

Would let a compacted and uncompacted basis dispatch on different code
paths explicitly, but ADR-0042 reserves `hash_version` exclusively for the
`session_semantics_version`-floor-gated transcript-hash *algorithm*; while
that floor is v1, ADR-0042 rule 2 requires every new basis to use v1.
Compaction is a storage-representation concern, not an algorithm change —
`transcript_hash` is computed identically (frozen `transcript_events_hash_v1`
over the whole covered set) regardless of how `span_revisions` represents
that set. Riding `hash_version` would conflate two independent axes and
block compaction from shipping before a hypothetical hash v2 exists.
Rejected.

## Decision Outcome

Chosen option: fold the covered-but-summarized prefix into an opaque
`CoveredPrefixDigest { span_count, content_hash }`, verified by hash
reconstruction, as a field orthogonal to `hash_version`.

1. `ProjectionBasis` gains `covered_prefix: Option<CoveredPrefixDigest>`.
   `None` means "every basis this crate wrote before `audio-graph-cfa1`,
   and any basis old or new whose covered set still fits inside
   `ROLLING_SUMMARY_HOT_WINDOW_TURNS`" — those keep the exact pre-compaction
   `span_revisions` shape, with every covered span named individually.
   `Some` means `span_revisions` carries only the verbatim hot-window tail;
   everything chronologically before it is folded into the digest.
2. The ONE canonical constructor
   (`ProjectionBasis::from_transcript_events_and_speaker_spans`, used by
   `TranscriptLedger::current_basis`/`current_projection_basis` and therefore
   every production embedding site) performs the split unconditionally.
   Compaction is a property of construction, not a per-caller decision.
3. `content_hash` is computed by the exact same frozen
   `transcript_events_hash_v1` function `transcript_hash` itself uses,
   applied to just the prefix's events in canonical chronological order.
   Reconstruction (`reconstruct_verified_covered_prefix`) rebuilds the
   candidate prefix from a ledger snapshot in that same canonical order and
   REJECTS it (returns nothing, never a partial/unverified result) unless
   the reconstructed hash matches exactly.
4. `transcript_hash` itself is unchanged: always computed over the FULL
   covered set (prefix + tail), by the same algorithm, regardless of how
   `span_revisions` represents that set. This is the outer safety net: even
   if the inner digest layer were ever bypassed, a revision anywhere in the
   covered set still fails the outer whole-set hash comparison.
5. `covered_prefix` does NOT carry its own `hash_version`-style tag. There
   has only ever been one algorithm for it (the same one `transcript_hash`
   uses at whatever `hash_version` the basis itself declares), so tagging it
   now would version a field with no second variant to dispatch on. When a
   future hash v2 exists, `ProjectionBasis`'s own exhaustive
   `Deserialize` match on `hash_version` already forces every construction
   and verification site — including this one — to be revisited; that is
   the point to decide whether the digest needs an explicit tag of its own,
   not before v2 exists.
6. **Amends ADR-0031's ordered-identity requirement for the covered
   summarized prefix only.** ADR-0031 step 3 requires the classifier to
   "compare the subset hash and ordered identities/revisions with the
   basis." For the verbatim tail this is unchanged and, as of this
   record, uniformly enforced whether or not the basis is compacted — a
   permutation of the persisted tail vector still classifies `Revised` via
   `CoveredSpanOrderMismatch`. For the summarized-away prefix, there is no
   per-span identity list to compare positions against; the prefix's
   content — and therefore its order, since `transcript_events_hash_v1`
   canonicalizes by chronological order internally — is proven by the
   digest hash instead. A basis whose prefix cannot be reconstructed and
   verified against the current ledger is treated as if the prefix were
   simply absent, which always shows up as a covered-count mismatch before
   any ordering or hash comparison runs. Detection power for the prefix is
   unchanged in kind (revision, deletion, and content corruption all still
   classify `Revised`) but changes MECHANISM (hash reconstruction, not
   position-by-position identity comparison).
7. Deserialization is a plain pass-through: the manual `Deserialize` impl
   copies `span_revisions` verbatim regardless of length, so a historical
   basis with a long, uncompacted `span_revisions` list round-trips
   byte-identically and is never retroactively re-compacted on load.
   Compaction is enforced by construction, not by field privacy or a
   deserialize-time invariant check — a blanket length assertion at
   deserialize time would itself violate ADR-0042's byte-stability
   requirement for historical bases.
8. Every basis-derived span-count reader that previously treated
   `span_revisions.len()` as "the whole covered set" — the classifier's own
   covered-set reconstruction, the projection LLM's rolling-summary window
   feed, ADR-0037 claim-evidence resolution, and the scheduler's/eval
   harness's span-count and latency telemetry — reads
   `covered_span_count()`/`resolve_covered_events()` instead, so none of
   them silently narrow to the hot-window tail once a session compacts.

### Predecessor-binary downgrade compatibility

`covered_prefix` ships with no `deny_unknown_fields` guard anywhere in this
crate's basis deserializer, so an OLDER binary (predating this record)
reading a session written by a newer binary silently discards the
`covered_prefix` field it doesn't know about. That older binary then sees a
tail-only `span_revisions` paired with a `transcript_hash` computed over the
full (prefix + tail) covered set — a guaranteed hash mismatch — and
classifies every such basis `Revised`, triggering ADR-0031's Replay-repair
path to regenerate notes/graph projections. This is fail-safe in direction
(stale-and-repair, never a false `Current` or silent corruption), and
forward replay (new code reading old data) is unaffected because `None`
round-trips exactly as it always has. If the older binary then REWRITES the
artifact, the digest is lost permanently for that item — it classifies
`Revised` forever, even after a later upgrade back to a compaction-aware
binary, until the next successful regeneration. Unlike ADR-0042's explicit
predecessor-binary compatibility canary for hash-version semantics, this
record does not add a new such canary: compaction never changes
`hash_version` or `transcript_hash`'s value, so the failure mode is
"redundant repair work," not "silently wrong output." Noted here so a
future downgrade incident is diagnosable rather than mysterious.

### Disclosed scope beyond the ticket's named file set

The ticket (`audio-graph-cfa1`) named the owning module, the four embedding
sites, the two DEBUG log sites, `projection_eval.rs`, and tests as the
allowed diff surface, with an explicit STOP condition if the change
cascaded further. Implementing deliverable (a) surfaced three call sites
outside that list that shared the exact same defect — reading
`span_revisions` directly as "the whole covered set," which silently
became false the moment compaction shipped:

- `projection_llm.rs`'s `basis_events` fed the projection LLM's prompt
  builder only the tail, which would silently DROP the rolling summary
  (and every claim/fact only present in it) from the prompt for any session
  past the hot window — regressing seed `audio-graph-18ee`'s fix for the
  same O(n²) full-transcript re-feed problem, in a different shape.
- `resolve_claim_evidence_basis_events` (ADR-0037 claim-evidence
  resolution) had the same assumption, which would silently downgrade valid
  evidence anchored to a summarized-away span to `unsatisfied`.
- `projection_scheduler.rs`'s span-count telemetry
  (`in_flight_span_count`/`pending_span_count`/`queued_span_count`, the last
  of which gates the `PendingSpanThreshold` coalescing decision) and
  `projection_eval.rs`'s `basis_span_count` diagnostic had the same
  assumption baked into scheduler-visible behavior, not just telemetry.

Each was judged a required fix for the SAME root cause as deliverable (a)
itself, rather than a separate cascade to stop and report: shipping the
compaction encoding without them would ship a silent, severe regression in
every long session's projection quality and coalescing behavior. A fourth,
narrower change — enabling serde's `rc` Cargo feature — was required for
deliverable (d) (`Arc<ProjectionBasis>` sharing across items one patch
touches) and is a build-time dependency-surface change, not a runtime
behavior change. This record exists in part to make that judgment call a
recorded decision rather than code-only, per the retrospective finding that
prompted it.

### Consequences

- **Positive**: Per-item persisted basis size is bounded by
  `ROLLING_SUMMARY_HOT_WINDOW_TURNS` regardless of session length or how many
  times an item is re-touched, closing the field-observed unbounded growth.
- **Positive**: Historical v1 bases remain byte-for-byte replayable; `None`
  is a permanent, correct marker, never retroactively upgraded.
- **Positive**: Detection power for revision/deletion/corruption inside the
  covered set is preserved for both the tail (ordered identity) and the
  prefix (hash reconstruction), with the whole-set `transcript_hash` as an
  independent outer check in both cases.
- **Negative**: The prefix's proof mechanism (hash reconstruction) is
  weaker than ordered identity comparison in one narrow, corruption-only
  sense: it detects membership/content changes but has no per-span position
  to point at in its error, unlike `CoveredSpanOrderMismatch` for the tail.
- **Negative**: A predecessor-binary downgrade that rewrites a compacted
  artifact loses that item's digest permanently, forcing regeneration on
  the next upgrade (see above) — no new canary test guards this today.
- **Negative**: `diarization_span_revisions` is NOT compacted by this
  record — out of the ticket's disclosed scope (field evidence and the
  deliverable list named only transcript `span_revisions`) — so a
  diarization-heavy long session can still partially reproduce the
  artifact-growth pattern this record otherwise closes. Candidate follow-up
  seed, not a blocker for this record.

## Relationships

| Relationship | ADR | Note |
| --- | --- | --- |
| Amends | [ADR-0031](0031-classify-projection-bases-as-current-append-only-or-revised.md) | Narrows step 3/4's ordered-identity comparison to the exposed verbatim tail; the summarized prefix is proven by hash reconstruction instead. |
| Depends-On | [ADR-0025](0025-stt-llm-context-efficiency-and-diff-based-updates.md) | Reuses the rolling-summary hot-window boundary (§2c) as the compaction boundary, rather than an independently tuned one. |
| Relates-To | [ADR-0042](0042-version-projection-basis-hashes-by-speech-semantics.md) | `covered_prefix` is deliberately orthogonal to `hash_version`; `transcript_hash`'s frozen v1 algorithm and byte-stability requirement are unchanged. |
| Relates-To | [ADR-0037](0037-admit-session-memory-items-through-a-layered-claim-class-evidence-table.md) | `resolve_claim_evidence_basis_events` must resolve the full covered set, including a compacted prefix, or valid evidence silently downgrades to unsatisfied. |

## Compliance

- `ProjectionBasis::from_transcript_events_and_speaker_spans` is the only
  production constructor and folds the pre-hot-window prefix into
  `covered_prefix` unconditionally.
- `span_revisions` never exceeds `ROLLING_SUMMARY_HOT_WINDOW_TURNS` entries
  for any basis whose covered set has grown past the hot window.
- A historical basis with no `covered_prefix` on disk deserializes to
  `covered_prefix: None` and keeps its original `span_revisions` shape
  forever; it is never retroactively re-compacted.
- `classify_basis_currency` classifies a revised, deleted, or corrupted span
  inside either the tail or the summarized prefix as `Revised`, never
  `Current` or `AppendOnlyStale`.
- A permutation of the persisted tail `span_revisions` vector classifies
  `Revised` via `CoveredSpanOrderMismatch`, whether or not the basis is
  compacted.
- Every basis-derived span-count reader in this crate uses
  `covered_span_count()`/`resolve_covered_events()`, not
  `span_revisions.len()`, as "the whole covered set."
- Neither production DEBUG log site (`speech/mod.rs`'s
  `observe_asr_revision` and the scheduler-completion log) dumps a raw span
  identity or the prefix digest's own content; both stay bounded regardless
  of session length.

## More Information

Field evidence, growth-rate analysis, and the full deliverable list are
recorded in the `audio-graph-cfa1` ticket. Reversal condition: re-examine
this decision if a deterministic fixture demonstrates that a corrupted or
forged compacted basis passes `classify_basis_currency` as `Current` or
`AppendOnlyStale` when its true covered content differs from what it
records — the hash-reconstruction proof mechanism would need strengthening,
not just this record's disclosure updated.
