---
status: accepted
date: 2026-08-14
deciders:
  - "AudioGraph user and product owner (human decider)"
drafter: "Codex agent (non-decider)"
---

# ADR-0042: Version Projection Basis Hashes by Speech-Revision Semantics

## Context and Problem Statement

ADR-0024 binds every notes or graph projection to an exact Projection Basis.
ADR-0031 requires the basis classifier to hash the covered transcript subset
before it classifies later growth as append-only. The current
`TranscriptHashVersion` contains only v1, defaults a missing version to v1, and
uses a frozen FNV-1a sequence over selected legacy `TranscriptEvent` scalars.
That byte sequence deliberately excludes the nested fidelity semantics added
by Speech Span Revision v2.

Reusing hash v1 for v2 would collapse distinctions the new contract is meant
to preserve: provider-exact versus app-estimated versus unavailable timing;
provider, app, or unavailable confidence/turn/speaker/channel evidence; v2
source order; and exact supersession. It would also force timing-unavailable
spans through an ordering function that requires legacy scalar times.

ADR-0041 establishes one canonical stream containing legacy-v1 and v2 payloads.
A mixed Projection Basis therefore needs one declared semantic interpretation,
not a hash of whichever Rust or JSON representation a caller happens to hold.
The exact implementation surface and blocked vertical tracer are recorded in
`docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-48de-report.md`.
The normative hash-v2 encoder, ordering fold, and golden fixture catalog are
specified separately in
[Projection Basis Hash v2 Canonical Encoding](../designs/projection-basis-hash-v2-encoding.md)
so this record remains one decision rather than an implementation schema.

## Decision Drivers

- Preserve every historical basis and accepted patch byte-for-byte under hash
  v1.
- Make v2 and mixed bases commit to all transcript semantics that can affect a
  notes or graph projection.
- Keep basis currency classification deterministic across restart and input
  reorder.
- Order multiple sources without pretending their source-local ordinals form a
  global clock.
- Normalize legacy rows honestly without inventing v2 app identities, source
  ordinals, or evidence origins.
- Exclude operational and storage metadata that cannot change projection
  meaning.
- Dispatch validation by the basis's declared version and fail closed on an
  unsupported version.
- Select the hash version from the one durable Session semantics floor rather
  than from whichever payload variants happen to be covered by one basis.

The decisive driver is stale-output safety: two covered revision sets that can
produce meaningfully different notes or graph state must not share a basis
hash merely because one arrived through the v1 representation.

## Considered Options

### Reuse hash v1 for all speech revisions

This preserves one implementation, but hash v1 cannot represent unavailable
timing or origin-qualified fidelity and sorts by mandatory legacy time. It
would either omit v2 semantics or fabricate legacy values. That violates the
accepted hash-version boundary and ADR-0031's exact-covered-subset rule.

### Hash serialized v2 or mixed JSON payloads directly

This appears comprehensive, but it couples basis identity to wire field order,
optional-field serialization, representation-only identifiers, and later
schema additions. Semantically equivalent rows could hash differently, while
an innocuous serialization refactor would stale every basis. Canonical-log
payload hashes already protect stored JSON integrity; Projection Basis needs a
separate semantic commitment.

### Define an explicit semantic hash v2 and retain hash v1 for history

This keeps historical replay fixed while giving mixed/v2 bases a complete,
domain-separated commitment independent of Rust struct or JSON layout. It
requires a new canonical semantic encoder and golden fixtures. This is the
chosen option.

### Start a separate projection ledger for v2 revisions

This avoids changing the existing classifier, but duplicates scheduler,
currency, patch, and replay semantics and leaves notes/graph with two possible
sources of truth. It conflicts with ADR-0024's shared Projection Basis and is
rejected.

## Decision Outcome

Chosen option: version Projection Basis transcript hashes by speech-revision
semantics.

1. Hash v1 is frozen. A missing `hash_version` continues to mean v1, and every
   historical v1 basis or accepted patch is serialized, replayed, and validated
   with the exact existing FNV-1a field sequence. No migration rewrites it.
2. Every historical deserialized basis with absent `hash_version` or explicit
   v1 validates with hash v1 forever, including after its Session semantics
   floor advances. An absent `session_semantics_version` on a historical
   Session means v1. While the accepted floor is v1, newly created bases use
   v1. After the same floor is durably Accepted at v2, every newly created
   basis uses explicit v2 even when its covered set is pure legacy. Basis
   creation, normalized prompt construction, currency classification,
   accepted-patch validation, and replay share this rule; they do not infer the
   version from covered payload variants.
3. Hash v2 is `sha256:` followed by lowercase hexadecimal SHA-256 over a
   domain-separated, length-delimited sequence of typed semantic values. The
   domain separator is `audio-graph:projection-basis:v2`. Golden fixtures
   freeze the concrete byte encoding before writer activation; the field-level
   schema belongs in the implementation design, not this decision record.
4. Each logical span receives one immutable basis position from its first
   canonical Accepted record. A later revision replaces that span's semantic
   content at the same position; it never moves the span to the later record's
   position. Within one v2 source, `source_order.ordinal` must remain strictly
   increasing in this logical-span order and can never reverse. Across sources,
   the global order is the first Accepted canonical sequence because
   source-local ordinals are incomparable. Conforming canonical sequences are
   unique; only a compatibility import that cannot provide a unique position
   may use span id as a deterministic tie-breaker, and that case must be
   explicit in its fixture. The ordering coordinate determines position but
   its numeric value is not itself a semantic hash field.
5. The v2 semantic representation includes the normalized payload variant,
   stable span identity, source identity, v2 source ordinal when present,
   provider identity, text, stability/finality, revision number, exact
   supersession, and timing/confidence/turn/speaker/channel evidence including
   origin, precision, and value where present.
6. The v2 semantic representation excludes provider item id and transcript
   segment id hints, provider/raw event references, capture and ASR latency,
   received-at time, the numeric canonical sequence, canonical event ids,
   causal ids, basis heads, payload/record hashes, and framing metadata. Those
   values support idempotency, operations, or storage integrity but do not
   change the speech meaning supplied to a projection. The same normalized
   semantic sequence, in the same canonical admission order, is the transcript
   view supplied to new v2 projection prompts; excluded fields are not sent to
   the model where they could affect output without affecting the basis hash.
7. In a mixed v2 hash, a legacy row is normalized without down-conversion or
   invented provenance. Its existing span/provider/source/text/revision,
   stability, timing, confidence, turn, speaker, and channel values are
   preserved. Present legacy evidence is tagged `legacy_unspecified`; absent
   optional evidence is tagged unavailable. V2 source order and app-owned span
   identity remain explicitly absent for that legacy row rather than being
   synthesized.
8. The `session_semantics_version` defined by ADR-0041 is the sole durable
   cutover guard for transcript, basis, and patch semantics. Its v1-to-v2
   transition must be durably Accepted under ADR-0027 before a hash-v2 basis or
   hash-v2 patch is created or applied. Basis creation, patch creation, and
   patch apply refuse v2 while the floor is v1. A v2 basis or patch observed
   ahead of its floor is typed corruption and does not promote or repair the
   Session. A v2 floor with no v2 artifact yet is a safe guard-ahead state, and
   retrying that same transition is idempotent.
9. Basis creation, covered-subset hashing, normalized prompt construction,
   currency classification, accepted-patch validation, and replay dispatch on
   the declared hash version. An unsupported version, a version/algorithm
   mismatch, or a v2 basis that cannot recover canonical admission order is a
   typed failure, never a fallback to v1.
10. Unavailable v2 fields are hashed as unavailable semantic variants and remain
   unavailable downstream. They are never replaced with zero timing, default
   confidence, a synthetic turn, or another legacy scalar.

This record is accepted and constrains implementation through its Decision
Outcome and Compliance sections. Acceptance is evidence only: it does not
close or unblock a Seed, and queue changes remain conductor-owned.

## Consequences

- **Positive:** Historical Projection Bases and patches retain byte-for-byte
  hash-v1 replay compatibility.
- **Positive:** Mixed/v2 bases commit to evidence fidelity and unavailable
  states, so a semantic change cannot hide behind equal span ids and revision
  numbers.
- **Positive:** Canonical admission order gives multiple sources one durable,
  deterministic total order without treating source-local ordinals or provider
  timestamps as globally comparable.
- **Positive:** Revising an early same-source span does not move it after later
  spans, so prompt order and hash order retain the original logical speech
  sequence.
- **Negative:** The ledger must retain or recover canonical admission order,
  and live scheduling cannot claim an accepted v2 basis before ADR-0027's
  durable admission point.
- **Negative:** Replay must retain each span's first Accepted position across
  compaction and late revisions; reconstructing order only from latest events
  is no longer sufficient.
- **Negative:** The project must maintain two hash algorithms indefinitely for
  replay and validate both throughout load, scheduling, apply, and evaluation.
- **Negative:** A crash immediately after the Session floor advances makes new
  bases v2 even if no v2 transcript exists yet, and the immediate predecessor
  must refuse that guard-ahead Session.
- **Negative:** Semantically similar legacy and v2 rows intentionally hash
  differently because legacy-unspecified evidence is not equivalent to
  provider/app-qualified v2 evidence.
- **Negative:** Adding a future projection-relevant speech field requires hash
  v3 or another explicit compatibility decision; it cannot silently enter v2.

### Confirmation

#### Current review evidence

Current confirmation is the independent ADR review plus existing hash-v1,
reader, and scheduler proofs. It does not establish hash-v2 conformance or
writer activation readiness.

Current hash-v1 and compatibility evidence is checked with:

```text
cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml \
  --lib --no-default-features --features cloud \
  projection_basis_defaults_and_serializes_explicit_transcript_hash_v1 \
  -- --nocapture
cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml \
  --lib --no-default-features --features cloud canonical_reader -- --nocapture
cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml \
  --lib --no-default-features --features cloud projection_scheduler -- --nocapture
```

Review must inspect the semantic include/exclude list against the actual prompt
inputs and verify accepted ADR immutability:

```text
git diff --exit-code 373d2c88de49818a8b10eb6fd7b02ccd4904e000 -- \
  docs/adr/0024-event-sourced-notes-graph-projections.md \
  docs/adr/0027-file-canonical-durable-session-store.md \
  docs/adr/0031-classify-projection-bases-as-current-append-only-or-revised.md
git diff --check
```

The current commands preserve existing evidence only. Passing them does not
accept this record or authorize activation.

#### Pre-activation executable gates

Activation requires named executable gates, not only this decision text, for:

- the hash-v2 encoder golden catalog in
  [Projection Basis Hash v2 Canonical Encoding](../designs/projection-basis-hash-v2-encoding.md),
  including an independent implementation or cross-version decoder check;
- mixed v1/v2 append, reopen, recovery, and deterministic basis replay;
- normalized prompt order matching hash order across multiple sources;
- a same-source late revision retaining the span's first position while
  replacing its content and advancing its revision;
- strict rejection of reversed same-source ordinals and deterministic
  cross-source input reorder;
- a historical absent/v1 basis and accepted patch replaying with hash v1 after
  the Session semantics floor advances to v2;
- a crash after durable acceptance of the v2 floor but before any v2 artifact,
  followed by reopen, idempotent guard retry, and new pure-legacy basis
  creation with hash v2;
- independent refusal fixtures for a hash-v2 basis and a hash-v2 patch found
  while the accepted floor remains v1, with no floor promotion or v1 fallback;
- the predecessor-binary compatibility canary required by ADR-0041.

These v2 gates do not yet exist as current passing commands. `audio-graph-4249`
and `audio-graph-48de` remain blocked until they exist and pass.

## Relationships

| Relationship | ADR | Note |
| --- | --- | --- |
| Depends-On | [ADR-0024](0024-event-sourced-notes-graph-projections.md) | Projection Basis exists to bind both notes and graph work to the same immutable transcript evidence. |
| Depends-On | [ADR-0027](0027-file-canonical-durable-session-store.md) | Canonical Accepted record order supplies the deterministic cross-source admission order. |
| Refines | [ADR-0031](0031-classify-projection-bases-as-current-append-only-or-revised.md) | Specializes covered-subset hashing and currency dispatch for declared v1 versus v2 semantics. |
| Depends-On | [ADR-0041](0041-keep-versioned-speech-revisions-in-one-canonical-stream.md) | Mixed normalization and admission order assume one canonical transcript revision stream. |

## Compliance

- Missing `hash_version` and every historical v1 basis select only the frozen
  hash-v1 algorithm.
- While the accepted `session_semantics_version` floor is v1, every new basis
  uses v1; after the same floor is durably Accepted at v2, every new basis uses
  v2, including a pure-legacy covered set.
- Hash-v2 basis creation and hash-v2 patch creation or apply refuse a v1 floor;
  an observed v2 basis or patch ahead of the floor is typed corruption and
  never promotes the Session.
- A v2 guard ahead of v2 artifacts is safe and idempotent, while every
  immediate predecessor must refuse the v2-floor Session.
- Each logical span keeps its first canonical Accepted position across every
  later revision.
- Same-source v2 ordinals never reverse; cross-source order uses first Accepted
  canonical sequence and only an explicit compatibility fixture may require a
  span-id tie-breaker.
- Hash v2 includes every field named as semantic and excludes every field named
  as operational or storage metadata in this record.
- Mixed normalization marks legacy evidence as legacy-unspecified or
  unavailable and never invents v2 source order, app identity, or provenance.
- Currency classification, prompt lookup, apply validation, and replay all
  dispatch on the basis's declared hash version.
- Unsupported or unrecoverable hash semantics fail with a typed error and do
  not fall back to v1.
- Acceptance of this record does not itself close or unblock any Seed.

## Reversal Condition

Re-examine this decision if a deterministic projection fixture demonstrates
that changing a speech attribute excluded from hash v2 changes the notes or
graph prompt or accepted output while the covered basis still classifies as
Current. AudioGraph maintainers and the product owner would observe that event
in the required mixed/v2 projection gate before writer activation or in a
later regression report.

## More Information

The normative encoder and golden fixture contract is in
[Projection Basis Hash v2 Canonical Encoding](../designs/projection-basis-hash-v2-encoding.md).
The consumer inventory,
migration boundary, error cases, and cheapest vertical fixtures are in
`docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-48de-report.md`.
This accepted record does not authorize code, Seed, provider, or workflow
changes; those effects require separately authorized work.
