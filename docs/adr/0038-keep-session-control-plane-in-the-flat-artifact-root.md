---
status: accepted
date: 2026-08-16
deciders:
  - "AudioGraph user and product owner (human acceptance required)"
drafter: "Codex agent (non-decider)"
---

# ADR-0038: Keep the Session Control Plane in the Flat Artifact Root

## Context and Problem Statement

ADR-0027 makes one typed Session Artifact manifest authoritative for load,
export, backup, delete, purge, recovery, retention, and usage. ADR-0035 then
requires one monotonic `session_semantics_version` floor to become durably
Accepted before any v2 transcript, Projection Basis, or projection patch can
become authoritative. The dormant manifest kernel currently stores one
root-wide manifest and temporary name even though the qualified application
data root contains artifacts for many Sessions. Activating that shape would
let one Session overwrite or be mistaken for another.

The existing durability substrate already coordinates the qualified data root
through one stable global lock. It supports shared strict readers and exclusive
mutators, and qualified Linux/ext4 and macOS/APFS namespace mutation can cross
the required file and parent-directory barriers. Windows can perform strict
non-mutating reads, but its namespace-changing manifest snapshot operation is
deliberately unsupported. A new layout must retain those proven boundaries
rather than introduce a second directory-provisioning protocol.

Historical Sessions add two ambiguity traps. An absent manifest must mean v1
only at a race-safe checked-open point, not from an unlocked path preflight.
Also, every manifest requires one Original Session Audio entry, but historical
inventory can prove only whether original audio bytes exist; it cannot prove
why they do not. The bootstrap must not invent `retention_disabled`,
`never_captured`, or another policy history.

The active Sessions boundary and dormant manifest wire do not currently accept
the same Session-id language. `sessions::session_id_is_valid` admits only
non-empty, at-most-128-byte ASCII alphanumeric, hyphen, or underscore ids. The
dormant manifest validator admits a broader, at-most-255-byte UTF-8 value that
excludes control characters and path separators. A control-address decision
must therefore name which contract may derive a path without retroactively
changing which historical manifest bytes are structurally valid.

Seed `audio-graph-67a1` blocks production work under `audio-graph-7e81` until
this control-plane layout is reviewed and accepted. This proposed record is
decision evidence only. It authorizes no code, Seed, workflow, or release
change.

## Decision Drivers

- Give every valid Session exactly one collision-free manifest, transition
  proof, and temporary identity inside the already-qualified data root.
- Preserve the existing qualified-root durability and one stable global
  coordination lock.
- Make the v1-to-v2 floor transition provable from immutable persisted bytes,
  not from manifest metadata alone.
- Keep ADR-0037's four canonical event streams frozen; this decision must not
  create a fifth stream by implication.
- Linearize floor admission with cooperating manifest mutation and prevent an
  absent-manifest preflight race.
- Bootstrap historical Original Session Audio from observed bytes without
  fabricating a retention or capture reason.
- Let Windows read compatible Sessions while refusing v2 mutation before any
  manifest, proof, temporary, or coordination side effect.
- Give inventory, export, delete, and recovery one explicit ownership rule for
  Session control artifacts without deleting the store-owned global lock.

The decisive driver is collision-free durable authority: a v2 admission proof
is trustworthy only when its path, bytes, lock domain, and lifecycle ownership
all resolve to the same Session.

## Considered Options

### Parameterize Session control files in the existing flat root

Encode the already-bounded Session id into a portable Session key and use it in
role-specific manifest, proof, and temporary filenames. Continue to coordinate
all Sessions with the existing root-wide lock. This preserves the qualified
namespace and keeps the new surface to one addressing/admission contract. This
is the chosen option.

### Put every Session in a directory capsule

A per-Session directory would make ownership visually obvious and could reduce
filename length pressure. It also adds directory creation, replacement,
qualification, parent-entry durability, partial-provisioning recovery, and
legacy migration to a protocol whose current evidence covers one existing
managed root. That is a new cross-platform durability problem, not a free
organizational improvement, so it is rejected for this bounded activation.

### Record provenance as a fifth canonical event stream

An append-only lifecycle stream could eventually represent every source,
clock, route, discontinuity, and lifecycle transition promised by ADR-0027.
Adding it now would change ADR-0037's frozen four-stream registry and broaden a
single v1-to-v2 guard into a general provenance subsystem. It is rejected
without separate ADR-0037 backflow and the richer lifecycle design.

### Keep the transition only as manifest metadata

The manifest already carries a floor, transition identity, and fingerprint.
Without separately persisted bytes, however, that metadata can describe a
proof that never existed and cannot be independently hashed, exported, or
deleted. Metadata-only provenance is rejected because it does not prove the
irreversible compatibility transition.

## Decision Outcome

Chosen option: parameterize per-Session control files inside the existing
qualified flat artifact root and retain the one store-wide coordination lock.

1. The production control-address constructor first requires
   `sessions::session_id_is_valid` and only then derives any control path. Its
   accepted id is non-empty, at most 128 bytes, and contains only ASCII
   alphanumeric characters, hyphen, or underscore. This is the authoritative
   production addressability seam, not a narrowing of the dormant manifest
   wire validator. A manifest id that is structurally valid under the broader
   at-most-255-byte UTF-8 wire but ineligible under the Sessions validator is
   refused with a content-free error before control-path resolution, I/O, or
   creation. A manifest loaded from a derived control path must contain the
   exact requested validated Session id or fail closed.

   After that validation, the Session control key is the lowercase RFC 4648
   Base32 encoding, without padding, of the exact ASCII Session-id bytes. The
   encoding performs no case folding or normalization, is injective, is
   bounded to 205 characters, and is never truncated or replaced by a
   collision-prone hash.
2. The role-specific control identities are
   `.audio-graph-session-<key>-artifacts.v1.json` for the manifest,
   `.audio-graph-session-<key>-artifacts.v1.tmp` for its temporary, and
   `.audio-graph-session-<key>-v1-v2.provenance` for the immutable transition
   proof. All remain immediate children of the existing qualified data root.
   Their maximum basenames remain below the 255-byte portable component
   ceiling. ASCII-case-equivalent aliases are reserved before filesystem
   access.
3. The manifest, its temporary, and the provenance proof are Session-owned.
   The global `.audio-graph-canonical.lock` remains store-owned and is shared
   by every Session. Per-Session inventory/export/delete/recovery derives and
   owns its three control identities in addition to the manifest's managed
   artifact inventory; it never inventories, exports as Session data, or
   deletes the global lock. A successful export contains the manifest and an
   available proof. Delete owns any retained Session temporary as recovery
   residue. No manifest inventories itself.
4. The existing global lock orders control-plane activity. A checked Session
   open holds a shared guard across manifest selection, strict manifest/proof
   validation, compatibility-floor admission, and creation of the content
   reader's admitted snapshot. A mutator holds the exclusive guard across
   proof creation/revalidation and manifest compare-and-swap. The bounded
   v1-to-v2 activation accepts global cross-Session lock contention rather than
   introducing unproved per-Session lock identities.
5. A checked open may classify an absent Session manifest as historical v1
   only through the capability-specific shared persistence contract. When the
   qualified Linux/ext4 or macOS/APFS store lock is initially absent, checked
   open establishes the qualified global coordination entry, acquires its
   shared guard, and revalidates the selected Session manifest's absence under
   that guard before admitting v1. A concurrent mutator that wins between lock
   establishment and shared acquisition is observed by that guarded
   revalidation; it cannot preserve a stale absent result.

   On the unqualified Windows/Other read-only path, checked open creates
   nothing. It may admit an absent manifest as v1 only because the production
   namespace policy makes manifest/proof mutation unavailable there. It checks
   the Session manifest and global coordination entry before reading, builds
   the complete content snapshot without releasing bytes to the caller, then
   checks both identities again immediately before that snapshot escapes. Any
   appearance or change returns typed retry/refusal rather than v1. An unlocked
   preflight result alone is never an admitted floor. These branches are fixed
   by capability; an implementer does not choose between them.
6. The v1-to-v2 provenance file contains exactly one immutable, versioned,
   content-free transition proof. Its canonical serialization is digest-free:
   no proof field is derived from the proof's own bytes. Construction is
   acyclic: serialize the canonical proof first, hash those exact bytes, then
   use that digest for both the manifest transition fingerprint and the proof
   artifact content hash. The manifest inventories the proof as the existing
   `SessionProvenanceEvents` kind and records its exact byte length. The proof
   bytes are durable before the manifest can Accept floor v2. Exact retry
   reuses identical bytes. A missing, duplicate, altered, unavailable,
   residual, mismatched, or self-hashing proof refuses admission. The file is
   not an ADR-0037 canonical event stream, and its one-record form must not be
   appended to.

   The exact proof wire, canonical serialization, and golden fixture belong to
   the sibling plan's
   [Versioned transition proof wire specification](../agentic-runs/2026-08-14-session-memory-continuation/audio-graph-67a1-session-control-plane-plan.md#versioned-transition-proof-wire-specification),
   not to this decision record.

   Logical admission consumes the actual `ManifestCasOutcome`: authoritative
   `Accepted` or exact `AlreadyCompleted` is passed to
   `admitted_session_semantics_floor`. A generic durability receipt, proof-only
   result, boolean, or caller-asserted success cannot admit or preserve v2.
7. This proof establishes only the v1-to-v2 compatibility floor. Rich source,
   clock, discontinuity, route, and lifecycle provenance promised by ADR-0027
   remains deferred to a separate decision and may not silently accumulate in
   this file.
8. Historical bootstrap inventories Original Session Audio truthfully. If
   observable managed audio bytes exist, it records them as Present with exact
   content identity. If none are observable, bootstrap records a distinct
   historical-unknown unavailable classification; it does not infer
   `retention_disabled`, `never_captured`, expiry, or user deletion. If the v1
   manifest wire cannot represent that distinction, implementation stops for
   a reviewed schema refinement rather than choosing a plausible reason.
9. Windows may perform strict v1 reads and may read a present compatible
   manifest/proof under the shared guard. It refuses v2 advancement before
   creating or changing the global lock, provenance proof, manifest temporary,
   or manifest. A Windows read never upgrades the floor, synthesizes proof
   bytes, or converts a namespace-durability refusal into Accepted.
10. Production implementation remains blocked until a human decider accepts
    the decision after the confirmation review. That lifecycle transition is
    one separate docs-only commit that changes this record's status to
    `accepted`, changes its date to the actual acceptance date, and changes the
    ADR-0038 README index status/date in the same commit. It changes no other
    claim, production path, or Seed. Acceptance is decision evidence, not
    implementation authorization; implementation and queue mutations still
    require their own scoped work.
11. The manifest and semantics modules named here remain dormant at this
    decision base: there is no production control-address caller and no
    persisted control-plane migration to preserve or execute. This record
    chooses the future production boundary; it does not claim that any Session
    has already been addressed, migrated, or advanced through it.

## Consequences

- **Positive:** Multiple Sessions can share one qualified flat root without a
  manifest, proof, or temporary collision.
- **Positive:** The v2 floor has independently persisted exact evidence while
  ADR-0037's canonical stream registry remains unchanged.
- **Positive:** Checked open and mutation share one proven coordination domain,
  and Windows remains useful for compatible reads without overstating
  namespace durability.
- **Positive:** Session deletion can remove every Session-owned control
  artifact while leaving other Sessions and the store-owned lock intact.
- **Negative:** One global lock serializes manifest/provenance mutation across
  unrelated Sessions and can delay opens during a transition.
- **Negative:** Flat role-specific filenames are less visually grouped than a
  Session directory and require a stable encoding contract forever.
- **Negative:** Some ids remain structurally valid manifest wire values but are
  deliberately ineligible for production control addressing; callers must
  distinguish wire validity from addressability.
- **Negative:** The historical-unknown Original Audio state requires an
  explicit wire representation before broad bootstrap; current unavailable
  reasons are not sufficient.
- **Negative:** Windows cannot advance a Session to v2 under the present
  namespace policy even though it can read v1 and compatible persisted state.
- **Negative:** Rich lifecycle provenance remains incomplete; this record
  proves only one compatibility transition.

### Confirmation

Before human acceptance, review the ADR and scoped plan together and confirm
that no production or Seeds diff is present:

```text
git diff --check e64aa4a...HEAD
git diff --name-only e64aa4a...HEAD
bun run verify:contracts
```

The human wire-acyclicity review confirms the one-way order
`canonical bytes -> content digest -> both manifest digest references` and
reviews the cited sibling wire-specification section. The implementation review
must include that plan's golden exact-bytes/hash fixture and negative fixture
that rejects any self-hash field before writer activation.

The addressability review compares `sessions::session_id_is_valid` with the
dormant manifest validator and confirms that control paths are derived only
after the narrower Sessions check. It also confirms content-free, pre-I/O
refusal for broader wire-only ids and exact requested/loaded Session-id match.

The acceptance review also verifies the exact evidence class: current local
documentation gates establish record/index/link consistency only; they do not
establish cross-platform implementation behavior. If accepted, status, actual
acceptance date, and README index status/date move atomically in their own
docs-only commit.

After acceptance, each implementation workstream must run its focused locked
Rust tests first, then locked cloud check, strict Clippy, rustfmt, the full
serialized cloud library suite, `bun run verify:fast`,
`bun run verify:contracts`, docs/Seeds secret hygiene, Betterleaks, and exact
base-range footprint checks. Confirmation is conformance evidence only.

## Relationships

| Relationship | ADR | Note |
| --- | --- | --- |
| Refines | [ADR-0027](0027-file-canonical-durable-session-store.md) | Narrows its typed manifest and provenance authority to collision-free per-Session control files in the existing qualified flat root. |
| Refines | [ADR-0035](0035-keep-versioned-speech-revisions-in-one-canonical-stream.md) | Narrows the durable v1-to-v2 floor evidence to one immutable exact proof plus a manifest CAS under one guard. |
| Relates-To | [ADR-0037](0037-freeze-canonical-event-stream-registry.md) | Preserves the four-stream registry by classifying the transition proof as a bounded control artifact, not a fifth event stream. |

This record supersedes no ADR. ADR-0027 and ADR-0035 remain accepted in all
other respects.

## Compliance

- Only an id accepted by `sessions::session_id_is_valid` may enter production
  control-address derivation; broader manifest-wire validity grants no path
  eligibility.
- Every production-addressable Session id maps injectively to one bounded
  lowercase Base32 key; the implementation neither normalizes, truncates, nor
  hashes it.
- Addressability refusal occurs before control-path resolution or I/O, and a
  loaded manifest Session id exactly equals the requested validated id.
- Manifest, provenance, and temporary basenames contain that key and remain
  immediate children of the already-qualified data root.
- The manifest, proof, and temporary are Session-owned; the one global lock is
  store-owned and never removed by a Session lifecycle operation.
- No v2 floor is Accepted without one exact immutable digest-free proof whose
  persisted bytes, derived hash, length, Session, and transition agree.
- Canonical proof bytes contain no digest-derived field; their SHA-256 is
  computed only after serialization and then supplies both manifest digest
  references.
- Logical admission passes the actual authoritative `ManifestCasOutcome` to
  `admitted_session_semantics_floor` and accepts only `Accepted` or exact
  `AlreadyCompleted`, never a generic receipt or proof-only result.
- The proof is one record and is not registered or replayed as a fifth
  ADR-0037 canonical stream.
- Qualified Linux/macOS checked open establishes/acquires the global
  coordination entry and revalidates absent manifest under its shared guard;
  unqualified Windows/Other creates nothing and requires exact pre/post
  manifest-and-lock absence around the unreleased content snapshot.
- Historical Original Session Audio reflects observed bytes or explicit
  historical unknown; it never fabricates a reason.
- Windows v2 mutation refuses before every coordination, proof, temporary, or
  manifest side effect while compatible reads remain available.
- Inventory, export, delete, and recovery treat all three per-Session control
  identities consistently and exclude the global lock.
- This proposed ADR does not authorize implementation or change any Seed.
- The dormant kernel has no production caller or persisted migration claim.

## Reversal Condition

Re-examine this decision if a cross-platform race or crash fixture demonstrates
that the existing single root-wide lock cannot give a checked Session open and
v1-to-v2 mutation one linearizable boundary without blocking ordinary Session
work beyond the product's measured latency budget. AudioGraph maintainers and
the product owner would observe that event in the required contention/crash
matrix or production latency telemetry; a replacement ADR would then compare
per-Session locks or directory capsules with new durability evidence.

## More Information

- Seed: `audio-graph-67a1`, prerequisite of `audio-graph-7e81`.
- Implementation plan:
  [`audio-graph-67a1-session-control-plane-plan.md`](../agentic-runs/2026-08-14-session-memory-continuation/audio-graph-67a1-session-control-plane-plan.md).
- Current dormant seams:
  `src-tauri/src/persistence/session_artifact_manifest.rs`,
  `src-tauri/src/persistence/session_semantics.rs`, and
  `src-tauri/src/persistence/canonical_durability.rs`.
