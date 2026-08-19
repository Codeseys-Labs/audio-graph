---
status: accepted
date: 2026-08-14
deciders:
  - "AudioGraph user and product owner (human decider)"
drafter: "Codex agent (non-decider)"
---

# ADR-0035: Keep Versioned Speech Revisions in One Canonical Transcript Stream

## Context and Problem Statement

ADR-0024 makes transcript revisions the immutable source for notes and graph
projections, while ADR-0027 makes versioned file streams the canonical durable
authority. The current `transcript_revisions` stream stores legacy-v1
`TranscriptEvent` payloads. The Speech Span Revision core now also defines a
strict v2 payload with honest timing, confidence, turn, speaker, and channel
evidence, including unavailable evidence that cannot be down-converted into
mandatory legacy scalars without fabrication.

The outer canonical format and the inner speech-revision payload are distinct
versioning concerns. `canonical_log.rs` frames accepted records as `AGCL1` and
strictly validates one stream id and one `domain_schema_version` across the
file. `canonical_reader.rs` currently fixes `transcript_revisions` to domain
schema version 1. Its reader-first `load_speech_span_revisions` seam already
decodes both absent-`contract_version` legacy v1 and explicit
`contract_version: 2` payloads through `CompatibleSpeechSpanRevision`, without
rewriting the framed v1 bytes. By contrast, changing the outer domain schema
version within the existing file would trigger
`DomainSchemaVersionMismatch` before inner payload decoding.

The downgrade boundary is narrower than “all old binaries fail closed.” Current
supported readers that open the canonical stream strictly fail when the
requested payload type cannot decode a v2 row. Older pre-canonical binaries may
ignore the canonical artifact and display a stale legacy transcript derivative
instead. Writer activation therefore needs one monotonic durable Session
compatibility floor, named `session_semantics_version`, plus a canary against
the minimum supported predecessor binary. The floor states the minimum reader
and Session semantics required to open the Session; it is not a
maximum-payload-observed counter. Strict behavior in the current reader cannot
be generalized to every historical build.

Seed `audio-graph-48de` cannot activate v2 adapter writes until this storage
choice and the related Projection Basis hash choice are reviewed. The exact
blocked code paths and implementation probes are recorded in
`docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-48de-report.md`.

## Decision Drivers

- Preserve one canonical transcript revision history for replay, projection,
  review, export, recovery, and deletion.
- Preserve every existing v1 payload byte and accepted framing commitment.
- Admit v2 unavailable evidence without fabricating a legacy scalar or writing
  a lossy compatibility copy.
- Make unsupported payload contracts fail closed at a precise record rather
  than silently skip history.
- Complete a reader-first cutover before the first production v2 append.
- Advance one durable compatibility floor before any v2 transcript, basis, or
  patch can become authoritative.
- Keep rollback behavior explicit once the Session semantics floor advances.
- Make the minimum supported downgrade refuse an incompatible Session before
  consulting a stale legacy derivative.
- Avoid dual-write reconciliation and a second transcript source of truth.

The decisive driver is canonical authority: a Session must have one ordered
speech-revision history even while its inner payload contract evolves.

## Considered Options

### Keep v1 and v2 payloads in one stream, discriminated inside the payload

This keeps one canonical order and lets the strict compatibility reader decode
each inner payload according to its own contract. It preserves the existing
stream identity and framing while making the reader upgrade the prerequisite
for writer activation. This is the chosen option.

### Quiesce v1 and continue in one sole v2 successor stream segment

This is not a dual write: the v1 file would become a closed immutable prefix
and one v2 successor file or segment would become the sole writer. It preserves
both payload contracts without an inner union, but every read must compose two
files atomically and prove their handoff order. The Session manifest, artifact
inventory, export, delete, recovery, quarantine, backup, and retention paths
would all need cross-file completeness rules. A crash between quiescing v1 and
registering v2 would leave an ambiguous active head. Those composition and
atomicity costs are broader than strict inner-payload dispatch in one existing
canonical stream, so this genuine successor-stream option is rejected.

### Dual-write v1 and v2 payloads

This would preserve an old-reader view only by fabricating or dropping v2
unavailable evidence during v1 projection. It also introduces partial-write
states and requires a permanent winner rule between two representations. It is
rejected because the compatibility copy would be both lossy and independently
fallible.

### Advance the outer `domain_schema_version` to 2 in the existing stream

The current canonical loader validates one expected domain schema version for
every framed record. A mid-file change would make either the old prefix or the
new suffix structurally invalid to a single strict load. Rewriting all prior
frames would violate immutable v1 bytes and their integrity chain. A new outer
version therefore buys no safe mixed-history migration under the current
canonical-log contract.

## Decision Outcome

Chosen option: keep legacy-v1 and Speech Span Revision v2 payloads in the one
canonical `transcript_revisions` stream.

1. The outer stream id remains `transcript_revisions`, the canonical frame
   remains `AGCL1`, and `domain_schema_version` remains 1. Domain schema 1 means
   “an ordered canonical speech-revision payload”; the inner payload contract
   determines how an individual revision is decoded.
2. Inner payload discrimination is strict. Absence of `contract_version`
   selects legacy v1, numeric `contract_version: 2` selects Speech Span
   Revision v2, and any other present value fails closed at that record.
3. The mixed reader lands and is used by every replay/load/export consumer
   before production activation. Writer cutover is single-path: it appends the
   admitted payload once and does not emit a v1 shadow.
4. Existing v1 bytes are immutable. Reader-first activation does not rewrite,
   normalize in place, migrate, or reframe the legacy prefix.
5. Current supported strict canonical readers fail the stream read at the first
   v2 payload when they request the legacy payload type; they may not skip that
   record or return a valid-looking canonical prefix. This guarantee does not
   cover pre-canonical binaries that can ignore the stream and read a stale
   legacy derivative.
6. The Session owns one `session_semantics_version` compatibility floor. An
   absent field on a historical Session means v1; new state writes it
   explicitly. The floor may advance from v1 to v2 and never decrease. Its
   value is derived from a canonical Session-provenance transition that is
   durably Accepted under ADR-0027, and the Session manifest and typed artifact
   inventory expose that accepted value. It denotes the minimum reader and
   Session semantics required across transcript payloads, Projection Bases,
   and projection patches; it does not summarize the maximum payload contract
   observed in any one stream.
7. The v1-to-v2 floor transition must be durably Accepted before the first v2
   transcript append, the first hash-v2 basis creation, or the first hash-v2
   patch creation or apply. Transcript writers, basis creators, patch writers,
   and patch appliers refuse v2 work while the accepted floor is v1. A v2
   payload, basis, or patch observed ahead of the v2 floor is forbidden state
   and is reported as corruption; no reader infers or repairs the floor from
   that artifact.
8. A crash after the v2 floor is Accepted but before any v2 artifact is safe.
   This guard-ahead state is intentional, and retrying the same monotonic
   transition is idempotent. A v2-capable release reopens the Session at floor
   v2 and may resume activation without lowering the floor.
9. The minimum supported downgrade is the immediate predecessor release to the
   writer-activation release, after that predecessor contains the reader-first
   mixed contract and understands `session_semantics_version`. It may open only
   Sessions whose floor remains v1. It must explicitly refuse a v2-floor
   Session before reading canonical transcript data or consulting a stale
   legacy derivative, including guard-ahead Sessions with no v2 payload yet.
   Writer activation requires an executed canary proving that behavior. Older
   builds are outside the supported downgrade boundary.
10. Rollback after the floor advances retains a release that supports v2
    Session semantics or restores a complete pre-transition Session boundary;
    the immediate predecessor is refusal-only for that Session. No rollback
    lowers the floor or promises universal historical-binary compatibility.
11. This record changes neither provider selectability nor content-egress
    policy. It chooses storage authority only.

This record is accepted and constrains implementation through its Decision
Outcome and Compliance sections. Acceptance is evidence only: it does not
close or unblock a Seed, and queue changes remain conductor-owned.

## Consequences

- **Positive:** One canonical sequence orders all legacy and v2 revisions, so
  projection, replay, export, recovery, and deletion do not reconcile two
  transcript authorities.
- **Positive:** V2 can retain unavailable and origin-qualified evidence without
  a fabricated legacy projection.
- **Positive:** Reader-first rollout preserves current v1 artifacts and makes
  the compatibility boundary testable before any irreversible append.
- **Negative:** After the v2 compatibility floor is Accepted, the immediate
  predecessor must refuse the Session even if the process crashed before a v2
  artifact was written; rollback requires a v2-capable release or a complete
  pre-transition restore.
- **Negative:** Every strict transcript consumer must move together to the
  compatibility reader before writer activation; one missed v1-only reader
  becomes a reload/export failure.
- **Negative:** Session provenance, manifests, and the typed artifact inventory
  must carry and enforce one monotonic semantics floor across transcript write,
  basis creation, patch creation/apply, load, review, export, and recovery.
- **Negative:** Keeping outer domain schema version 1 means inner contract
  support must remain explicit and exhaustive; the outer schema number alone
  cannot advertise the newest payload present.
- **Neutral:** Plain legacy JSONL prefixes and framed v1 records remain readable
  under the existing canonical-log compatibility rules.

### Confirmation

#### Current review evidence

Current confirmation is the independent ADR review plus the existing v1 and
reader-first proofs. It does not establish activation readiness.

Current reader/framing evidence is checked with:

```text
cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml \
  --lib --no-default-features --features cloud canonical_reader -- --nocapture
cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml \
  --lib --no-default-features --features cloud speech_span_revision -- --nocapture
```

Review must also confirm accepted ADR immutability and the scoped diff:

```text
git diff --exit-code 373d2c88de49818a8b10eb6fd7b02ccd4904e000 -- \
  docs/adr/0024-event-sourced-notes-graph-projections.md \
  docs/adr/0027-file-canonical-durable-session-store.md \
  docs/adr/0031-classify-projection-bases-as-current-append-only-or-revised.md
git diff --check
```

Passing these commands is evidence only; it neither accepts this record nor
authorizes writer activation.

#### Pre-activation executable gates

Writer activation additionally requires executable evidence for all of the
following:

- durably Accept the v1-to-v2 `session_semantics_version` transition, crash
  before the first v2 artifact, reopen at floor v2, retry the transition
  idempotently, and resume without changing the accepted floor;
- append legacy v1 then v2 through the production canonical appender, close,
  reopen, strictly decode, recover a torn tail, and replay the same heads;
- load, review, export, delete, inventory, and recovery over the mixed stream;
- preserve framed and raw legacy-v1 bytes exactly;
- reject unknown inner contracts at the exact record without legacy fallback;
- run crash/recovery fixtures that present a v2 transcript payload, hash-v2
  basis, and hash-v2 patch separately while the accepted floor remains v1, and
  observe strict corruption/refusal before projection or fallback;
- launch the designated predecessor binary against both a v1-floor Session and
  a guard-ahead v2-floor Session; observe the first open and the second refuse
  before canonical transcript read or legacy fallback.

The mixed append/reopen/recovery, semantics-floor crash fixtures, and
predecessor-binary canary do not exist as current passing gates.
`audio-graph-4249` and `audio-graph-48de` stay blocked until named commands for
them exist and pass; prose descriptions are not substitutes for executable
evidence.

## Relationships

| Relationship | ADR | Note |
| --- | --- | --- |
| Refines | [ADR-0024](0024-event-sourced-notes-graph-projections.md) | Narrows the immutable transcript source to one stream whose inner revision payloads are versioned. |
| Refines | [ADR-0027](0027-file-canonical-durable-session-store.md) | Specializes file-canonical storage for mixed legacy-v1 and v2 transcript payloads without dual authority. |
| Relates-To | [ADR-0031](0031-classify-projection-bases-as-current-append-only-or-revised.md) | Basis currency remains separate and is versioned by ADR-0042. |
| Relates-To | [ADR-0042](0042-version-projection-basis-hashes-by-speech-semantics.md) | The mixed canonical stream supplies the revisions whose semantic hashes ADR-0042 defines. |

## Compliance

- Every new speech revision is appended exactly once to
  `transcript_revisions`; no v1 shadow or second canonical transcript stream is
  written.
- The outer frame is `AGCL1` with stream id `transcript_revisions` and domain
  schema version 1.
- A production v2 writer is unreachable until all production transcript reads
  use the strict compatible payload reader.
- An unknown present inner `contract_version` fails the strict stream read at
  its record index.
- The Session manifest and typed artifact inventory expose the monotonic
  `session_semantics_version` derived from its durably Accepted provenance
  transition; they do not derive it from the maximum payload observed.
- A v2 transcript append, hash-v2 basis creation, or hash-v2 patch creation or
  apply is refused unless the Session floor is already durably Accepted at v2.
- Guard-ahead is safe and idempotent; payload-, basis-, or patch-ahead is typed
  corruption and never advances the floor implicitly.
- The minimum supported predecessor binary is canaried to open v1-floor
  Sessions and refuse every v2-floor Session before it can read canonical data
  or display a stale legacy derivative.
- Existing v1 payload and frame bytes are never rewritten during cutover.
- No unavailable v2 field is down-converted into a mandatory legacy scalar.
- Acceptance of this record does not itself close or unblock any Seed.

## Reversal Condition

Re-examine this decision if the required mixed-stream recovery fixture shows
that a valid v1 prefix and a supported v2 suffix cannot be loaded, quarantined,
and replayed under one domain schema version without rewriting an accepted
frame. AudioGraph maintainers would observe that event in the required
canonical-reader/recovery gate before v2 writer activation.

## More Information

Implementation contracts, consumer inventory, old-binary limitations, and the
cheapest vertical probes are in
`docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-48de-report.md`.
This accepted record does not authorize code, Seed, provider, or workflow
changes; those effects require separately authorized work.
