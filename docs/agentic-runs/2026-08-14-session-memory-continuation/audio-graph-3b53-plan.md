# audio-graph-3b53 Session Control Contract Plan

Date: 2026-08-16

## Assignment and acceptance

Seed `audio-graph-3b53` implements only the first serial shared-persistence
workstream under accepted
[ADR-0038](../../adr/0038-keep-session-control-plane-in-the-flat-artifact-root.md).
Execution is fixed to base `b5145b2b630a38df7065905263139575b44ead7e` on
branch `work/audio-graph-3b53-session-control-contract-wave7c` in clean
worktree
`/home/codeseys/DevBox/audio-graph/.worktrees/3b53-session-control-contract-wave7c`.

Done means the dormant manifest kernel exposes collision-free production
per-Session control addressing, a guard-owning checked-read transaction, an
immutable exact transition-proof operation, and one proof-before-manifest
transition operation. Actual `ManifestCasOutcome::Accepted` and exact
`AlreadyCompleted` remain available for the next serial admission layer; this
workstream does not call `admitted_session_semantics_floor` or activate any v2
writer/consumer.

## Agreed public and crate-public seams

Tests observe behavior through these pre-agreed seams rather than private
filesystem helpers:

1. A production per-Session store constructor validates with
   `sessions::session_id_is_valid` before deriving `SessionControlIdentities`.
   Qualified construction binds the same Session address to live filesystem
   qualification. The legacy explicit-root constructor remains test-only for
   inherited dormant regression fixtures.
2. `SessionArtifactManifestStore::checked_read` (or equivalently named public
   closure seam) owns the shared guard and returns only the closure's complete
   snapshot. Qualified Linux/macOS may establish the one global coordination
   entry, then acquire shared and revalidate. Unqualified Windows/Other creates
   nothing. When both the Session manifest and global coordination entry are
   absent, transient appearance cannot be excluded through stable handles, so
   the operation fails closed before invoking the snapshot closure. Existing
   state is read only while the pre-existing global coordination entry is held
   shared.
3. A crate-public exclusive-guard immutable exact-create/reconcile operation in
   `canonical_durability` owns create-new, complete write, flush, owner-only
   protection, file sync, parent sync, exact-existing reconciliation, typed
   refusal, and indeterminate outcomes. It exists only if the manifest module
   cannot express those barriers through an existing guard seam.
4. A public manifest transition operation accepts the expected generation,
   candidate manifest, and versioned digest-free proof input. While holding the
   exact exclusive transaction, it durably ensures the proof first, derives and
   installs both digest references, then performs manifest CAS. It returns the
   actual `ManifestCasOutcome`, including `Accepted`, exact
   `AlreadyCompleted`, typed rejection, or durability indeterminate.

No generic receipt, caller boolean, proof-only success, or detached manifest
can stand in for the actual transition outcome.

## Slice 1: production Session control addressing

### RED

Add one failing public-constructor table test for:

- a 128-byte ASCII-safe id accepted with exact lowercase unpadded RFC 4648
  Base32 identities;
- a 129-byte ASCII id rejected content-free before path/I/O;
- non-ASCII and 255-byte inputs that remain valid under the dormant manifest
  wire but are ineligible for production addressing;
- case-distinct ids mapping to distinct keys;
- exact manifest, temporary, and proof basenames at the qualified flat root;
- requested validated Session id versus loaded manifest id mismatch; and
- refusal probes proving no root lookup, lock/control creation, or file I/O.

Add a two-Session test proving independent manifest/temp/proof identities share
only `.audio-graph-canonical.lock`. Preserve a direct dormant broad-wire
validation fixture so production addressability cannot silently narrow the
manifest wire.

### GREEN

Add the smallest opaque Session address/control-identity types and production
store constructor. Validation precedes `PathBuf::join`, filesystem metadata,
qualification, or any guard open. Strict load rechecks that a persisted
manifest's Session id exactly equals the requested address.

Commit this slice separately after the focused test and inherited manifest
tests pass.

## Slice 2: one global-lock checked read

### RED

Add public closure-seam tests for:

- present manifest held under shared guard while an exclusive writer contends;
- initially absent qualified Linux/macOS lock establishment followed by shared
  acquisition and Session-manifest absence revalidation;
- a writer winning between qualified establishment and shared acquisition;
- initially absent and present unqualified Windows/Other reads that create
  nothing;
- an absent unqualified namespace refusing before the closure, even when a
  manifest or lock appeared and disappeared before the call; and
- no uncoordinated closure result escaping when ABA cannot be observed.

### GREEN

The qualified branch establishes the existing store-owned coordination entry
through the qualification-bound durability pair, releases that establishment
guard, acquires shared, then reloads/revalidates the selected manifest under the
shared guard. The unqualified Windows/Other branch never opens with create. If
both manifest and lock are absent, it returns a content-free typed refusal
before invoking the reader because two pathname observations cannot exclude an
appearance-and-removal ABA. A pre-existing coordination entry permits the
normal shared-guard read.

No per-Session lock, lock rename, or Windows namespace mutation is introduced.

## Slice 3: versioned immutable transition proof

### Wire fixture

The proof's compact UTF-8 JSON field order is `schema_version`, `session_id`,
`from`, `to`, `idempotency_id`, and `transition_kind`; floors are 1 and 2 and
the kind is `session_semantics_advance`. It ends at the closing object delimiter
without a newline and contains no digest-derived field.

For Session `session-1` and idempotency id `advance-floor-v2`, the exact 143
bytes are:

```json
{"schema_version":1,"session_id":"session-1","from":1,"to":2,"idempotency_id":"advance-floor-v2","transition_kind":"session_semantics_advance"}
```

The independent golden digest is
`sha256:1d796c12d556471fbfca4381957a80e82f3139259c6c974a190730fa69c6b0e6`.

### RED

Start with exact byte/length/hash and unknown/self-hash member rejection. Then
add one public exclusive-transaction test per create, write, flush, protection,
file-sync, parent-sync, and fault cut; exact orphan reuse after every
indeterminate cut; different-byte conflict; regular/special/symlink collision;
ASCII-case alias refusal; and repeated exact reconcile proving no append or
duplicate record.

### GREEN

Serialize first, hash the exact complete bytes second, and pass the same digest
to the manifest transition fingerprint and proof artifact content identity.
The narrow durability seam creates a missing proof exclusively and durably, or
reopens one regular exact proof and re-establishes its file/parent barriers.
Because a true partial write can leave the final proof identity as a strict
prefix, the operation first durably stages the exact prepared manifest bytes at
the already-owned per-Session manifest temporary. That intent contains the
proof digest, length, and exact managed identity. After path/handle
revalidation under the same exclusive guard, those exact intent bytes
authenticate recovery of the same proof from an empty file or any regular
strict prefix. A different proof cannot reuse that intent. Exact bytes
reconcile without append; different bytes, a non-prefix or longer regular file,
and unsafe entry types reject without replacement. The existing recovery CAS
consumes the same exact manifest temporary after proof durability. This adds no
path identity or proof temporary. Once bytes may be visible, an unproved
barrier returns durability indeterminate with the opaque recovery key.

Windows and unqualified namespaces refuse before proof or coordination
mutation.

## Slice 4: proof-before-manifest transition

### RED

At the public manifest transaction seam, prove:

- proof Accepted precedes manifest CAS and both manifest digest references equal
  the independently derived proof digest and length;
- exact proof orphan retry can proceed to manifest `Accepted`;
- an exact completed manifest retry returns the actual
  `ManifestCasOutcome::AlreadyCompleted` without rewriting proof or generation;
- proof conflict/rejection prevents manifest/temp mutation;
- proof durability indeterminate prevents manifest CAS and remains
  indeterminate; and
- manifest CAS rejection or indeterminate after a durable exact proof remains
  honest and retryable, never converted to success.
- the persisted `SessionProvenanceEvents` identity exactly equals the derived
  per-Session proof identity;
- generic addressed-store CAS cannot install v2 without owning proof bytes;
- stale generation, head, idempotency, and candidate checks finish before any
  proof, manifest, or temporary mutation; and
- a partial proof orphan cannot be claimed by a different proof that shares
  only an earlier canonical prefix, while the exact proof still recovers after
  restart.

### GREEN

Add one operation on the guard-owning `ManifestWriteTransaction`. It binds the
candidate's provenance identity to the exact derived per-Session proof
identity, serializes and hashes the proof in memory, installs its hash/length in
the candidate, and validates the requested Session address, candidate, head,
generation, and idempotency state. Only after that pure preflight succeeds may
it durably create or reconcile the proof and commit the prepared generation
CAS. Addressed generic CAS cannot install v2. Strict-prefix recovery requires
the observed bytes to reach the complete proof-specific idempotency boundary,
so a different proof cannot complete an orphan that merely shares the
schema/session prefix. The returned value is the existing actual
`ManifestCasOutcome`; no semantics-floor admission occurs here.

### Round-2 crash-recovery correction

The fixed proof final can be empty after create-before-write or shorter than
any proof-internal identity boundary after a real short write. Proof bytes alone
cannot distinguish their intended completion. The manifest temporary is
therefore the durable authentication record, not a prefix-length heuristic.

The exact order is: pure transition/CAS preparation; read-only proof collision
preflight; durable exact manifest-temporary stage; exact intent validation;
proof create/reconcile; proof file and parent barriers; recovery CAS consuming
that same temporary; manifest reopen; authoritative `Accepted` or
`AlreadyCompleted`. Tests cover empty, one-byte, sub-identity, and near-complete
proof prefixes; a different proof preserving both the first proof and intent;
intent CreateNew/PostCreate/Write/Flush/Protect/FileSync/ParentSync cuts; and
manifest FileSync/Rename/ParentSync cuts. The unused post-snapshot read callback
test seam is removed.

The source checkpoint is focused-green but not release-gated. Strict Clippy,
locked broad checks, the serialized full suite, Windows probes, contracts and
aggregate verification, Betterleaks, secrets, runtime-dark checks, and fresh
Standards/Spec review remain required before any SHIP or integration decision.

## Ownership and hard stops

Owned source is `session_artifact_manifest.rs`; `canonical_durability.rs` is
owned only for the narrow guard/exact-create seam; `persistence/mod.rs` only for
a required declaration/export. Inline tests stay with those modules. Planning,
commit-state, and final report are the only owned docs.

Stop if implementation needs consumer/lifecycle files, a new dependency or
workflow, a fifth stream, a per-Session directory/lock, Windows mutation
acceptance, or an ADR change. Do not touch Seeds, frontend, writers,
projections, generated contracts, main/integration, or other worktrees.

## Commits, rollback, and report

Planning commits first. Each vertical slice records exact RED/GREEN output in
`audio-graph-3b53-report.md` and commits separately with the Seed id. The final
report commit contains final gates, findings, open questions, exact footprint,
and runtime-dark proof.

Before production adoption, rollback reverts the addressing/read, proof, and
transition slices in reverse order. There is no runtime caller or persisted
migration to unwind.

## Gates

Focused tests throughout:

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked \
  --manifest-path src-tauri/Cargo.toml --lib --no-default-features \
  --features cloud session_artifact_manifest -- --nocapture --test-threads=1
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked \
  --manifest-path src-tauri/Cargo.toml --lib --no-default-features \
  --features cloud canonical_durability -- --nocapture --test-threads=1
```

Final gates:

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 check --locked \
  --manifest-path src-tauri/Cargo.toml --lib --tests \
  --no-default-features --features cloud
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked \
  --manifest-path src-tauri/Cargo.toml --lib --no-default-features \
  --features cloud -- --test-threads=1
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 clippy --locked \
  --manifest-path src-tauri/Cargo.toml --lib --tests \
  --no-default-features --features cloud -- -D warnings
cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --all -- --check
bun run verify:fast
bun run verify:contracts
bun scripts/check-docs-secret-hygiene.mjs
git diff --check b5145b2...HEAD
```

Also compile/probe Windows production and cfg(test) surfaces without simulating
namespace acceptance, run Betterleaks with redaction over the exact footprint,
prove only owned paths changed, and search for runtime callers, fifth-stream
registration, per-Session locks/directories, and Windows acceptance. Native
platform evidence outside this local worktree remains conductor-owned.
