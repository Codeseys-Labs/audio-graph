---
status: accepted
date: 2026-08-18
deciders:
  - "AudioGraph maintainer (accepted 2026-08-18)"
drafter: "Claude agent (non-decider)"
---

# ADR-0039: Let a Session Abandon Its Own Staged Manifest Temporary

## Context and Problem Statement

ADR-0038 decision item 3 assigns per-Session ownership of the manifest, its
intent temporary, and the immutable transition proof, keeps the one
`.audio-graph-canonical.lock` store-owned, and closes with "Delete owns any
retained Session temporary as recovery residue". Its drivers ask for one
explicit ownership rule per control file. That record is accepted and, per
`docs/adr/README.md`, immutable; changing it requires a new record.

Seed `audio-graph-3b53` round 4 exposed a wedge that item 3 does not resolve.
A manifest compare-and-swap that refuses after its intent temporary is already
durable leaves that temporary behind, and every later `compare_and_swap` on the
same Session is refused `SnapshotTempAlreadyExists`. Before this record's
implementation branch, the only escape was a byte-exact replay of the same
candidate through `advance_session_semantics_v1_to_v2` — a candidate the
refusal outcome never named or told the caller to retain — because
`compare_and_swap_recovery` is `pub(crate)` and the durability substrate had no
removal primitive at all. A caller that wants to give up on a transition and
install a different candidate therefore stayed wedged forever.

Seed `audio-graph-3cf2` adds the missing capability on
`work/audio-graph-3cf2-refused-install-survivors`: a namespace-qualified
durable unlink primitive in `canonical_durability.rs`
(`CanonicalExclusiveGuard::unlink_canonical_entry`) and one public entry point,
`ManifestWriteTransaction::abandon_staged_transition`. Abandon is a second,
non-delete remover of a Session temporary, which is a scope change to ADR-0038
item 3's ownership sentence rather than a fact that sentence already covers.
That change is recorded here. It was briefly recorded instead as an in-place
dated amendment inside accepted ADR-0038 (commit `2ae6435` on the same branch);
that edit is reverted, because `docs/adr/README.md` makes accepted records
immutable and ADR-0038 carries no self-amendment clause.

The implementation exists but is dormant: `abandon_staged_transition` has no
production caller, only unit tests, so no shipped behavior depends on this
refinement yet.

## Decision Drivers

- Keep exactly one explicit ownership rule for each control file, including the
  temporary, instead of leaving a second remover undocumented.
- Give a refusal that leaves a durable temporary an escape that does not require
  replaying a candidate the outcome never named.
- Never let a Session remove the store-owned coordination lock, whatever the
  caller passes.
- Keep a removal capability from becoming a general purge: it must reach only
  immediate children of the already-qualified data root.
- Keep the immutable transition proof out of the abandon path; the proof-binding
  gate is a separate, unresolved decision (`audio-graph-68a1`).
- Classify an abandon truthfully: an exact rerun must be an assessment, not a
  fabricated success and not an error.

The decisive driver is that a caller must be able to stop retrying one
transition and install a different candidate without deleting the Session.

## Considered Options

### Keep delete as the sole remover and rely on byte-exact replay

This preserves ADR-0038 item 3 verbatim, but it leaves the wedge: the refusal
outcome names no candidate to replay, so a caller that discarded the candidate
has no reachable escape short of deleting the Session and its content. Rejected
because it makes an install refusal permanently unrecoverable for that Session.

### Make `compare_and_swap_recovery` public

The existing recovery path already consumes an orphaned temporary, so exposing
it looks cheaper than a new primitive. But it consumes the temporary by
installing it, which needs the same candidate bytes the caller no longer has,
and it publishes a transition the caller is trying to abandon. Rejected: it
answers a different question.

### Add a general delete or purge facility to the durability substrate

A recursive removal helper would also serve future retention work, but it makes
every caller a potential destroyer of the canonical head, the proof, or the
store lock, and nothing in the substrate could prove such a call wrong.
Rejected on blast radius.

### Add a bound durable-unlink primitive plus a Session-scoped abandon entry point

This adds one substrate primitive with its own preflight, namespace
qualification, parent sync, and fault classification at every cut, and one
public entry point that can only name the Session's own derived temporary. It
costs a new fault table and a new trust boundary to document. This is the
chosen option.

## Decision Outcome

Chosen option: scope ADR-0038 item 3's temporary-removal ownership rule and
add a bound durable-unlink primitive with a Session-scoped abandon entry point.

1. **Ownership, refined.** Delete remains the sole owner of Session-temporary
   removal among lifecycle operations acting on a Session from outside a write
   transaction. A Session holding its own exclusive-guard manifest write
   transaction may additionally abandon its own retained manifest intent
   temporary. Every other ownership assignment in ADR-0038 item 3 — the
   Session-owned manifest, temporary, and proof; the store-owned global lock;
   what export contains; that no manifest inventories itself — is unchanged.
2. **The store-owned lock stays store-owned.** The removal primitive refuses
   the reserved coordination basename under every ASCII-case-equivalent
   spelling in its preflight, before any filesystem access, independently of
   which caller asked.
3. **The primitive is bound, not general.** It binds the target's parent to the
   managed root, so it reaches only immediate children of the already-qualified
   data root and can never become a subtree purge. It refuses a non-regular
   entry, a target whose canonical parent is not exactly that root, and a
   pathname that no longer names the object the guard opened. It crosses the
   parent-namespace barrier before reporting removal, classifies every cut
   (`InspectEntry`, `OpenExisting`, `Unlink`, `ParentSync`) on its own, and
   reports the unsupported-namespace platform gate rather than pretending to
   remove.
4. **Its trust boundary is explicit and caller-enforced.** The substrate refuses
   only what it can prove wrong without knowing caller intent. It does not know
   which of the caller's records are immutable, so keeping the canonical
   manifest head and the immutable proof out of the target path is the caller's
   obligation. `abandon_staged_transition` discharges that obligation by
   passing only the Session's own derived temporary identity, never a
   caller-supplied path.
5. **Abandon removes only the temporary.** Neither the canonical manifest, its
   generation, nor the immutable transition proof is written or removed, so a
   durable v2 proof still refuses a different transition id with
   `ImmutableExactConflict`. Removing or re-keying that proof is out of scope
   here and belongs to the proof-binding decision.
6. **An exact rerun is an assessment.** Nothing staged yields `AlreadyAbsent`
   after crossing the same parent barrier and creates nothing. `AlreadyAbsent`
   asserts only that this call removed nothing; it does not assert that the
   manifest head is unchanged, because an install left indeterminate at its
   rename may already have consumed the temporary. Abandon is therefore not the
   reconciliation for a `DurabilityIndeterminate` outcome; that reconciliation
   remains the exact rerun keyed by the outcome's own recovery key.
7. **This record is `proposed`.** Until a human decider accepts it, ADR-0038
   item 3 remains the in-force ownership rule, and the landed primitive and
   entry point stay without any production caller. Acceptance is evidence only:
   it authorizes no consumer activation, closes no Seed, and does not itself
   change queue state.

## Consequences

- **Positive:** An install refusal that leaves a durable temporary has a
  reachable, documented escape, so a caller can install a different candidate
  without deleting the Session.
- **Positive:** The substrate gains its first removal capability with the same
  qualification, barrier, and fault discipline as its other mutations rather
  than an ad-hoc `remove_file` at a call site.
- **Positive:** The ownership rule for the temporary is stated once, in force,
  and matches the code that ships.
- **Negative:** The substrate now contains a removal capability whose safety
  against removing an immutable record depends on its caller, which is a new
  trust boundary to review on every future caller.
- **Negative:** Abandon cannot retract an indeterminate install; a caller that
  reaches for it after an indeterminate may observe the head advanced to the
  candidate it tried to abandon.
- **Negative:** Abandon does not authenticate the temporary's bytes, by design,
  so it cannot distinguish this transaction's own stale temporary from one left
  by an earlier transaction of the same Session.
- **Negative:** A second remover exists while ADR-0038's accepted text still
  names delete, until this record is accepted or rejected.

### Confirmation

Current evidence is the implementation branch's locked Rust gates, not any
cross-platform or activation evidence. The four behaviors this record adds are
pinned by these tests in `src-tauri/src/persistence/session_artifact_manifest.rs`:

```text
refused_manifest_install_after_durable_proof_is_never_reported_as_unstaged
abandoned_transition_unwedges_a_different_candidate
exact_rerun_of_abandon_is_a_no_effect_assessment
abandon_after_an_indeterminate_install_publishes_rather_than_retracts
```

The substrate primitive's own fences and cuts are pinned by these tests in
`src-tauri/src/persistence/canonical_durability.rs`:

```text
unlink_refuses_reserved_windows_unqualified_and_non_regular_entries_before_mutation
unlink_refuses_a_replaced_managed_root_before_mutation
unlink_fault_cuts_are_honest_and_exact_rerun_is_a_no_effect_assessment
```

Review runs the repository's authoritative gates and confirms that accepted
ADR-0038 is byte-identical to its acceptance commit:

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 clippy --locked \
  --manifest-path src-tauri/Cargo.toml --lib --tests \
  --no-default-features --features cloud -- -D warnings
cargo +1.95.0 fmt --all -- --check
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked \
  --manifest-path src-tauri/Cargo.toml --lib --no-default-features \
  --features cloud persistence -- --test-threads=1
git diff --exit-code fcd5d10 -- \
  docs/adr/0038-keep-session-control-plane-in-the-flat-artifact-root.md
git diff --check
```

Those gates establish local Linux behavior only. They do not establish
cross-platform removal behavior, and no non-Linux probe of the unlink primitive
has been run. Acceptance of this record does not authorize a production caller;
a consumer activation would need its own evidence.

## Relationships

| Relationship | ADR | Note |
| --- | --- | --- |
| Refines | [ADR-0038](0038-keep-session-control-plane-in-the-flat-artifact-root.md) | Scopes only decision item 3's temporary-removal ownership sentence to lifecycle operations outside a write transaction and adds the removal primitive's fences; ADR-0038 remains accepted and unedited in every other respect. |
| Relates-To | [ADR-0027](0027-file-canonical-durable-session-store.md) | Keeps the typed manifest authoritative for delete, recovery, and retention; abandon adds no second inventory. |

This record supersedes no ADR.

## Compliance

- Removal of a Session manifest temporary happens only through Session delete,
  acting from outside a write transaction, or through that Session's own
  exclusive-guard write transaction abandoning its own derived temporary.
- The removal primitive refuses the reserved coordination basename under every
  ASCII-case spelling before any filesystem access.
- The removal primitive refuses non-regular targets, targets outside the
  qualified root, and non-immediate children; it reaches only immediate
  children of that root.
- A reported removal has crossed the qualified parent-namespace barrier; an
  unacknowledged barrier is reported as indeterminate with a recovery key that
  names the pathname's unlink.
- An exact rerun of an abandon with nothing staged reports `AlreadyAbsent` and
  creates nothing.
- Abandon never writes or removes the canonical manifest, its generation, or the
  immutable transition proof.
- While this record is `proposed`, no production path calls the abandon entry
  point.
- Acceptance of this record does not itself close or unblock any Seed.

## Reversal Condition

Re-examine this decision if a reviewer or fixture shows that a caller of the
removal primitive can destroy a record the project treats as immutable — the
canonical manifest head, a transition proof, or the store lock — or if the
proof-binding work (`audio-graph-68a1`) makes proof retraction, not temporary
removal, the operation a wedged caller actually needs. If the decider rejects
this record, the reversal is removing the public abandon entry point and its
substrate primitive; nothing in production depends on either.

## More Information

The implementation plan and result for the branch that landed the primitive are
in `docs/agentic-runs/2026-08-18-audio-graph-3cf2/plan.md` and
`docs/agentic-runs/2026-08-18-audio-graph-3cf2/report.md`. The wedge this record
escapes is recorded in
`docs/commit-state-2026-08-16-session-control-contract-wave7c.md`. This proposed
record authorizes no code, Seed, provider, or workflow change.
