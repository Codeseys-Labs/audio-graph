# audio-graph-68a1 — PLAN (amended design): provenance-to-durable-proof binding, then the B4 re-key

Seed: `audio-graph-68a1`. Branch `work/audio-graph-68a1-proof-binding`, worktree
`.worktrees/68a1-proof-binding`, base `27be43a`.

This is the design brief as amended by the adversarial critique. Every required
change from that critique is folded in and marked **[critique N]**. Deviations
from the brief that the critique did not ask for are marked **[deviation]** with
the reason. What shipped, the gates, and the residuals are in `report.md`.

Anchor honesty: every `path:line` in this file and in `report.md` is checked by
`docs/agentic-runs/2026-08-18-audio-graph-68a1/check-anchors.py`, which enumerates
all anchors in both documents, asserts an expected substring at each cited line,
fails on any anchor it does not enumerate, fails on any enumerated anchor no
document cites, and fails on a bare line-number anchor (a colon followed by
digits, naming no file). Symbol names are authoritative; the numbers are
navigation.

---

## 1. The defect, as verified in this tree

`validate_v2_session_provenance`
(`src-tauri/src/persistence/session_artifact_manifest.rs:2329`) ends with two
checks against the candidate's own fields — the transition must be `Completed`,
and `manifest.transition.fingerprint` must equal the candidate's own provenance
entry's `content.sha256`
(`src-tauri/src/persistence/session_artifact_manifest.rs:2357`). That is internal
self-consistency: no parameter of the function names a path, a root, a guard, or
a Session address, so nothing it computes can reference the durable proof record,
and a caller controlling both fields satisfies it trivially. Its callers are
`validate_and_normalize` in the same file and
`src-tauri/src/persistence/session_semantics.rs:194`; neither has filesystem
context either.

The only thing that stood between a caller and a forged V2 head was the
floor-keyed transition-proof gate in `prepare_compare_and_swap`. Two facts force
the shape of the fix:

1. **The durable proof does not exist yet when `prepare_compare_and_swap` runs on
   the transition path.** `advance_session_semantics_v1_to_v2_inner` calls
   `prepare_compare_and_swap(.., true)` at
   `src-tauri/src/persistence/session_artifact_manifest.rs:1346` and only then
   proves the durable record equals the proof bytes at
   `src-tauri/src/persistence/session_artifact_manifest.rs:1356`. So the binding
   must NOT run when `proof_owned == true`, or the first advance refuses itself
   for want of the record it is about to create.
2. **On the proof-owning path a stronger binding already exists.** The advance
   overwrites the candidate's transition fingerprint, provenance identity, and
   provenance content from the proof bytes it is about to make durable, then
   proves full-byte equality against the durable record through
   `preflight_immutable_exact`. Full-byte equality beats a digest comparison, so
   `proof_owned` calls need nothing new, including the idempotent retry against
   an already-V2 head.

So the binding is scoped to `!proof_owned` V2 candidates on an addressed store
whose head is already V2 — precisely the set the re-key admits, and precisely
where the forgery lives.

**Governing authority.** ADR-0038
(`docs/adr/0038-keep-session-control-plane-in-the-flat-artifact-root.md:2` is
`status: accepted`) decision outcome point 4
(`docs/adr/0038-keep-session-control-plane-in-the-flat-artifact-root.md:158`):
a mutator holds the exclusive guard across proof creation/revalidation and
manifest compare-and-swap. Point 6
(`docs/adr/0038-keep-session-control-plane-in-the-flat-artifact-root.md:188`): a
missing, duplicate, altered, unavailable, residual, mismatched, or self-hashing
proof refuses admission. Reading and revalidating the proof inside a guard-owned
CAS is authorized by the accepted decision; the code satisfied point 6 only for
defects visible inside the candidate's own inventory. **No ADR change and no new
ADR — this closes an implementation gap against point 6.**

---

## 2. Production design, by symbol

All in `src-tauri/src/persistence/session_artifact_manifest.rs`, all outside
`commit_prepared_compare_and_swap`
(`src-tauri/src/persistence/session_artifact_manifest.rs:1789`) and the
install-stage classification that audio-graph-3cf2 owns.

### 2.1 `V2ProvenanceProofBindingError`

New public enum immediately after `V2SessionProvenanceError`
(`src-tauri/src/persistence/session_artifact_manifest.rs:442`), with a per-variant
contract in rustdoc:

| Variant | Contract |
| --- | --- |
| `DurableProofAbsent` | Nothing exists at the derived provenance identity. A V2 head with no proof record is refused, never repaired, never treated as V1. |
| `NonRegularDurableProof` | The identity resolves to a non-regular entry (directory, symlink, device), before or after this call opened it. |
| `DurableProofExceedsCanonicalBound` | Longer than any canonical v1-to-v2 proof, so it is refused **without** classifying a truncated view of it. |
| `DurableProofUnreadable { kind, raw_os_error }` | Any other I/O failure, reported verbatim and never widened into `DurableProofAbsent`. |
| `NotCanonicalDurableProof(..)` | Not exactly one canonical proof; the inner error is forwarded verbatim from `from_canonical_bytes`, which re-serializes and byte-compares, so a re-ordered or padded record is `NonCanonical`. |
| `DurableProofSessionMismatch` | A canonical proof for a different `session_id` than this store's address. |
| `ProvenanceIdentityMismatch` | The entry names something other than the derived control provenance identity. Exact, case-sensitive equality: a case variant is refused, stricter than a case-insensitive volume, which is the fail-closed direction. |
| `ProvenanceContentMismatch` | The entry's `sha256` or `byte_length` is not the digest/length of the record's actual bytes. |

The enum rustdoc states the guarantee and its limits: the binding proves the
Session's floor is backed by a real, canonical, session-matching proof record
whose bytes are what the candidate's inventory claims; it proves nothing about
the candidate's own transition; it deliberately does not compare the record's
`idempotency_id` to the candidate's, because every generation after the advance
legitimately carries a different one; and it closes the API-level forgery, not an
adversary with raw write access to the managed root.

### 2.2 `ManifestCasRejection::V2ProvenanceProofBinding`

Appended at the **end** of the enum
(`src-tauri/src/persistence/session_artifact_manifest.rs:673`), after
`TransitionProofRefusedAfterIntentStaged`, so the diff stays off the `Durability`
and `TransitionProofRefusedAfterIntentStaged` rustdoc that audio-graph-3cf2 will
edit. Blast radius outside the file is nil: `ManifestCasRejection` is only ever
wrapped, never exhaustively matched — the sole external use is
`src-tauri/src/persistence/canonical_log.rs:349`.

### 2.3 `validate_v2_session_provenance` returns the validated pieces **[critique 1]**

The brief proposed returning `&SessionArtifactEntry` so the binding needed no
unreachable "entry missing" variant — and then reintroduced exactly that pattern
one field deeper, by reading `entry.availability.content()` and classifying the
impossible `None` as `ProvenanceContentMismatch`. That is a silent
misclassification arm in a repo whose rule is that refusals are classified
truthfully. Amended: the validator returns

```rust
pub(crate) struct V2SessionProvenanceEntry<'manifest> {
    managed_identity: &'manifest ManagedArtifactIdentity,
    content: &'manifest ArtifactContentIdentity,
}
```

so the unreachable state is unrepresentable and the binding has no `None` arm at
all. The two self-consistency checks stay exactly as they are: they are load-path
and floor-admission checks, and `session_semantics.rs`'s
`forged_already_completed_manifest_proof_cannot_preserve_the_logical_floor`
depends on `TransitionFingerprintMismatch`. Both existing call sites are
`…map_err(..)?;` statements and keep compiling.

### 2.4 `bind_v2_provenance_to_durable_proof`

Free function immediately after the validator
(`src-tauri/src/persistence/session_artifact_manifest.rs:2392`), so the diff does
not abut `commit_prepared_compare_and_swap`. Bound constant at
`src-tauri/src/persistence/session_artifact_manifest.rs:2374` (u64 **[deviation]**,
so it compares directly against `Metadata::len` and feeds `Read::take`); its
rustdoc carries the 143-byte golden wire, the 118-byte template, the 255-byte id
caps, the "escaping at most doubles" premise, and the resulting 1138-byte true
ceiling under the rounded 4096 bound.

Steps, in order:

1. `entry.managed_identity != *provenance_identity` → `ProvenanceIdentityMismatch`.
   Cheapest, and the shape the audio-graph-3b53 probe used.
2. `symlink_metadata`; non-file → `NonRegularDurableProof`.
3. Open read-only, **then re-check the opened handle's metadata** — `is_file`,
   and its length against the bound — before reading **[critique 2]**. The brief
   cited `load_manifest_file`
   (`src-tauri/src/persistence/session_artifact_manifest.rs:1989`) but mirrored
   only its first half; the precedent continues at
   `src-tauri/src/persistence/session_artifact_manifest.rs:2005` with exactly this
   re-check, because `open()` follows a symlink planted between the metadata check
   and the open. Without it, `NonRegularDurableProof`'s contract would be false
   under a swap in that window. With it, the contract holds for both observations
   and the variant rustdoc says "before or after this call opened it".
4. `take(MAX + 1).read_to_end`; observed length over the bound →
   `DurableProofExceedsCanonicalBound`.
5. `from_canonical_bytes` → `NotCanonicalDurableProof(..)`;
   `proof.session_id() != expected_session_id` → `DurableProofSessionMismatch`.
6. `canonical_bytes_and_digest()`; entry digest or entry byte length not equal to
   the record's → `ProvenanceContentMismatch`.

I/O classification is centralized in `binding_io`
(`src-tauri/src/persistence/session_artifact_manifest.rs:2444`): `NotFound` from
any of the three I/O sites is `DurableProofAbsent`, everything else is
`DurableProofUnreadable { kind, raw_os_error }`. **[deviation]** The brief mapped
`NotFound` only at `symlink_metadata`; a `NotFound` at open or read means the
record vanished between two syscalls of this same call, and "absent" is the
truthful classification for that, while "unreadable" would be a widening in the
wrong direction. No condition collapses into `DurableProofAbsent` other than
`NotFound`.

**Why plain `std::fs`, not `preflight_immutable_exact`.** That primitive needs the
expected bytes this call is trying to discover, is not a read-only open, and would
import `NamespaceDurabilityUnsupported` semantics and per-platform refusal
ordering into a validation check. This module already reads its own authoritative
head with plain `std::fs` under a guard; this is the same discipline against a
record whose identity is derived, never caller-supplied. We hold the exclusive
guard for the whole transaction, which is the only guarantee the rest of this
module claims.

### 2.5 The re-key

`refuse_unproven_v2_candidate`
(`src-tauri/src/persistence/session_artifact_manifest.rs:1631`) is called from
`prepare_compare_and_swap` at
`src-tauri/src/persistence/session_artifact_manifest.rs:1697`, replacing the
floor-keyed gate and its `KNOWN OPEN` comment block. **[deviation]** The brief
wrote the gate as a nested `if` block inline; it ships as one named method with
guard clauses, which keeps the `prepare_compare_and_swap` diff at seven lines,
keeps the rationale in rustdoc next to the predicate it explains, and avoids
`clippy::collapsible_if` on the `if cond { if let Some(..) }` shape. Semantics and
ordering are identical to the brief's version:

* skip when `proof_owned`, or when the candidate is not V2;
* skip when the store is unaddressed — unchanged behaviour, sound only because
  every unaddressed constructor is `#[cfg(test)]` while the two non-test
  constructors both derive an address (standing constraint, R7 in `report.md`);
* head absent or below V2 → `TransitionProofRequired`, unchanged classification;
* provenance path/identity missing on an addressed store → `SessionAddressRequired`,
  destructured the way the advance path destructures it rather than assuming
  `expected_session_id.is_some()` implies the other two;
* `validate_v2_session_provenance` as an accessor — it cannot fail here, since
  `validate_candidate_and_normalize` already ran it, and its error arm reproduces
  that exact classification rather than inventing one;
* then the binding, mapped to `V2ProvenanceProofBinding`.

Placement is unchanged: after `validate_candidate_and_normalize`, so the candidate
is normalized and its V2 structure already proved, and before the `if let
Some(head)` block, so floor regression, prepared/completed, idempotency,
generation, and size checks keep their current order and classifications. Nothing
is staged at that point, so every refusal leaves the store byte-identical.

### 2.6 Derived consequences, both recorded rather than fixed

The binding composes transitively with the surviving self-consistency check:
`transition.fingerprint == content.sha256` plus
`content.sha256 == digest(durable proof)` means **every admissible V2 generation
of a Session carries the durable proof digest as its transition fingerprint.**

1. `recovery_key(&candidate.transition.fingerprint)`
   (`src-tauri/src/persistence/session_artifact_manifest.rs:1781`) is therefore
   identical for every V2 generation of a Session. `CanonicalRecoveryKey` is an
   opaque correlation token, never a path component or a comparison key, so this
   costs diagnosability, not correctness, and it was already the status quo for
   the transition path.
2. **[critique 3]** `IdempotencyConflict`
   (`src-tauri/src/persistence/session_artifact_manifest.rs:1730`) is unreachable
   for a bound V2 candidate. A V2 head carries the proof digest as its
   fingerprint, and so does any candidate that survives the binding, so the
   same-id arm's fingerprint comparison can never fire. The acceptance property
   "the same idempotency id with changed artifacts still conflicts" is therefore
   carried solely by the byte-equality fallthrough to `TransitionConflict`
   (`src-tauri/src/persistence/session_artifact_manifest.rs:1743`). The refusal
   holds, but the classification differs in shape from V1's, and the load-bearing
   check is not the one the variant name suggests. T5 asserts the refusal and its
   rustdoc records why `IdempotencyConflict` is not reachable.
3. A V2 quarantine transaction would have to carry the proof digest as its own
   fingerprint. It can, and it still cannot land — see §5.

---

## 3. Test plan, mapped to the acceptance criteria

All new tests are one contiguous block starting at the (renamed) pinning test, so
the rebase onto audio-graph-3cf2 sees a single test hunk. All are `#[cfg(unix)]`,
like every sibling CAS test. Later-generation fixtures clone the **accepted**
manifest, never `v2_candidate()`, whose entry names
`streams/session-provenance.jsonl`.

**RED discipline.** T1, T5 and T6 were written first against the unchanged API and
watched fail; then the error type and the rejection variant were added so T2 and
T3 could compile, and they were watched fail; only then did the binding and the
re-key land. The T4 case was written in the same first pass but is NOT RED
evidence: the pre-change gate already refused every V2 candidate whose call did not
own proof bytes, so that case passed from the start and stands as a green pin on
behaviour the re-key had to preserve. Binding, re-key, inverted pin and all tests are in
ONE commit, so no commit in this branch's history has the re-key without the
binding. Verbatim RED output is in `report.md`.

| # | Test | Acceptance criterion |
| --- | --- | --- |
| T1 | `committed_v2_head_records_later_generations_only_against_the_durable_proof` (`src-tauri/src/persistence/session_artifact_manifest.rs:4146`) | invert the pinning test rather than deleting it; B4 liveness closed |
| T2 | `forged_v2_provenance_is_refused_on_an_advanced_head` (`src-tauri/src/persistence/session_artifact_manifest.rs:4356`) | the seed's required RED: forged identity / fabricated fingerprint refused **after** the head advanced |
| T3 | `advanced_head_refuses_later_generations_when_the_durable_proof_is_not_intact` (`src-tauri/src/persistence/session_artifact_manifest.rs:4465`) | provably tied to the durable record, not merely self-consistent |
| T4 | `addressed_generic_cas_cannot_install_v2_without_owning_proof_bytes` (`src-tauri/src/persistence/session_artifact_manifest.rs:3967`), one case added | a real transition still requires its proof |
| T5 | `advanced_head_conflicts_on_the_same_idempotency_id_with_changed_artifacts` (`src-tauri/src/persistence/session_artifact_manifest.rs:4216`) | same idempotency id with changed artifacts still conflicts |
| T6 | `quarantine_recovery_remains_closed_on_a_v2_session` (`src-tauri/src/persistence/session_artifact_manifest.rs:4273`) | the "also record while here" closure, as executable evidence |
| — | `accepted_v2_manifest_cannot_regress_to_v1` (`src-tauri/src/persistence/session_artifact_manifest.rs:3866`) plus T1's third assertion | a V1 candidate against a V2 head is still a floor regression |

**T1** — advance to generation 1, then a genuine later generation (clone of the
accepted manifest, new idempotency id, one extra `exports/notes.md` artifact) is
`Accepted` at generation 2; the V1-regression assertion is kept verbatim,
including its now-stale `expected_generation` of 1, because floor regression is
classified before the generation comparison and keeping it proves that ordering;
then the store is reopened through `load()` and the persisted provenance entry
must still equal `control_identities().provenance` with the proof's digest and
length, and the proof file's bytes must be byte-identical to before. Rename
recorded in `report.md`: the old name asserts "until proof binding exists", which
becomes a false statement about shipped code, and a test name that misdescribes
the code is the same defect class as a comment that does.

**T2** — four forged shapes, each submitted with `compare_and_swap(1, forged)`
against the generation-1 V2 head: (1) `managed_identity =
identity("attacker/not-the-proof.jsonl")` with genuine content →
`ProvenanceIdentityMismatch`; (2) `content.sha256` and `transition.fingerprint`
both set to `digest('f')` so self-consistency passes →
`ProvenanceContentMismatch`, the seed's `sha256:ffff…` forgery; (3) genuine
identity and digest, `byte_length + 1` → `ProvenanceContentMismatch`, proving the
exact byte length is bound; (4) only `transition.fingerprint` forged →
`Validation(InvalidV2SessionProvenance(TransitionFingerprintMismatch))`, proving
the pre-existing self-consistency check still fires first. After every case:
manifest bytes unchanged, temporary absent, proof bytes unchanged; and after all
of them, the reopened head is still generation 1 naming the control identity.

**T3** — one durable-record shape per refusal, each with a genuine candidate:
delete the record → `DurableProofAbsent`; truncate to 100 bytes →
`NotCanonicalDurableProof(Malformed)`; write `MAX + 1` bytes →
`DurableProofExceedsCanonicalBound`; write the canonical proof for `"session-2"`
at session-1's identity → `DurableProofSessionMismatch`; **[deviation]** write
another genuine canonical proof for session-1 under a different idempotency id →
`ProvenanceContentMismatch` (a sixth shape the brief did not list: it proves a
swapped-in *valid* record is refused, which is the closest thing to a real
attack); `remove_file` + `create_dir` → `NonRegularDurableProof`.
`DurableProofUnreadable` gets **no** automated test: forcing a read error at that
exact path needs root-fragile permission manipulation or a fault seam this module
does not have. Precedent: `ManifestLoadError::Io` is constructed only by `load_io`
and is asserted by no test in this module either. Coverage is not faked for it.

**T4** — a case added to the existing test: seed a **V1 head**, then a generic V2
candidate at generation 1 → still `TransitionProofRequired`. The pre-existing
version only exercised an absent head, so this is the assertion proving the
re-key keys on the head's *floor*, not on the head's *absence*. **[deviation]**
The added case calls `self::root(..)` because the test's own local binding shadows
the fixture-root helper's name.

**T5** — after the advance: the same `idempotency_id` with one extra artifact →
`TransitionConflict`; the byte-identical clone → `AlreadyCompleted`. Both
unreachable before the re-key. The rustdoc records §2.6's consequence 2.

**T6** — four shapes against a generation-1 V2 head, each carrying genuine
provenance so the closure's real cause is what refuses: V1 prepared and V1
completed quarantine candidates through `compare_and_swap_recovery` →
`SessionSemanticsFloorRegression`; a V2 **Prepared** quarantine candidate →
`Validation(InvalidV2SessionProvenance(TransitionNotCompleted))`, refused by
`validate_candidate_and_normalize` before gate and binding; a V2 **Completed**
quarantine candidate whose transaction fingerprint is the proof digest, so it
passes both `validate_quarantine_transaction` and the binding →
`CompletionRequiresPrepared`
(`src-tauri/src/persistence/session_artifact_manifest.rs:1749`). The test comment
names the cause the seed names: `SessionArtifactManifestV1::candidate`
(`src-tauri/src/persistence/session_artifact_manifest.rs:256`) hardcodes
`SessionSemanticsVersion::V1`, and the production quarantine candidate is built
through exactly that constructor
(`src-tauri/src/persistence/canonical_log.rs:973` →
`src-tauri/src/persistence/canonical_log.rs:1032`) and reaches this CAS through
`compare_and_swap_recovery` (`src-tauri/src/persistence/canonical_log.rs:1107`)
with `proof_owned = false`. **The re-key does not open quarantine recovery**, and
T6 is what keeps that statement true.

**Regression set re-run by name:**
`proof_before_manifest_transition_returns_actual_accepted_and_already_completed`
(`src-tauri/src/persistence/session_artifact_manifest.rs:3894` — its second call is
the advance retry against a V2 head, the proof-owning path the binding
deliberately skips),
`proof_conflict_and_indeterminate_prevent_manifest_mutation_then_retry_converges`
(`src-tauri/src/persistence/session_artifact_manifest.rs:4562`),
`stale_existing_head_rejects_different_transition_before_proof_mutation`
(`src-tauri/src/persistence/session_artifact_manifest.rs:4647`),
`v2_candidate_requires_exact_bound_session_provenance_proof`
(`src-tauri/src/persistence/session_artifact_manifest.rs:3722`),
`accepted_v2_manifest_cannot_regress_to_v1`, and all of
`persistence::session_semantics::tests`.

---

## 4. Documentation deliverables

1. This plan and `report.md`, in
   `docs/agentic-runs/2026-08-18-audio-graph-68a1/`, plus `check-anchors.py`.
   **[deviation]** The brief put the report at the wave-7c path
   `docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-68a1-report.md`;
   the implementer instruction requires `plan.md` and `report.md` under the
   dated run directory. Both are satisfied: the canonical documents live in the
   run directory, and the wave path holds a short pointer so the wave index stays
   complete. No content is duplicated.
2. `docs/commit-state-2026-08-16-session-control-contract-wave7c.md` gains an
   "Update after audio-graph-68a1" block that explicitly supersedes its round-4
   claims that "B4 is OPEN BY CHOICE", that the wedge stays, and that the re-key
   must not be re-attempted, including the four-closed-paths list, whose "V2
   candidate → `TransitionProofRequired`" entry is now conditional on the head's
   floor.
3. `docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-3b53-report.md`
   gains a short superseding note. Its round-4 history is NOT rewritten. Its
   instruction "when the binding exists, that test should be inverted rather than
   deleted" is discharged by T1 and the note points at T1's name.
4. No ADR change and no new ADR; ADR-0038 outcome points 4 and 6 are the
   authority (§1).
5. `.seeds/` untouched.

---

## 5. Out of scope, stated as not done

* **The install-stage `Durability` classification** in
  `commit_prepared_compare_and_swap` / `install_snapshot_inner`, and any
  removal/abandon primitive — audio-graph-3cf2, which lands first; this branch
  rebases onto it. These edits deliberately avoid that region and the
  `Durability` / `TransitionProofRefusedAfterIntentStaged` rustdoc.
* **Load-path binding.** `load`, `load_selected_manifest`, and `load_manifest_file`
  still accept a persisted V2 manifest whose durable proof is absent or altered,
  because they run only the structural validator. Deliberate: a damaged Session
  must stay loadable for diagnosis, and floor *admission* is CAS-gated through
  `admitted_session_semantics_floor`
  (`src-tauri/src/persistence/session_semantics.rs:167`). Consequence stated
  plainly: this binding closes the **API-level** forgery, not an adversary with
  raw write access to the managed root — nothing in this kernel can close that.
  `checked_session_open`
  (`src-tauri/src/persistence/session_semantics.rs:113`) admitting a V2 floor from
  manifest bytes alone is a named residual for a follow-up seed.
* **Relaxing the self-consistency check.** It stays. Removing it would weaken the
  load path and `admitted_session_semantics_floor` and would break
  `forged_already_completed_manifest_proof_cannot_preserve_the_logical_floor`.
* **The shared V2 recovery key** (§2.6.1) — recorded, not fixed.
* **`IdempotencyConflict` unreachable for bound V2 candidates** (§2.6.2) —
  recorded, not fixed.
* **`SessionArtifactManifestV1::candidate`'s V1 hardcode** and therefore the
  quarantine-recovery closure — recorded (T6), not fixed. Fixing it means letting
  recovery build V2 candidates, which collides with `TransitionNotCompleted`
  versus `CompletionRequiresPrepared`; that is a design question about
  quarantine-under-V2, not a binding question.
* **A `ChangedDuringRead` analogue.** `load_manifest_file` re-checks metadata
  *after* its read; the binding does not, because it does not need to: the record
  is accepted only if its bytes hash and measure to exactly what the candidate
  claims, so a torn or swapped read is refused as `ProvenanceContentMismatch` or
  `NotCanonicalDurableProof` rather than accepted. Adding a fourth metadata check
  would buy a different refusal name for an already-refused state.

---

## 6. Failure modes considered

1. **Binding on `proof_owned` calls** would deadlock the first advance against
   itself. Rejected; keyed on `!proof_owned` (§1).
2. **Binding inside `validate_v2_session_provenance`** would force I/O into a
   function called from `load_manifest_file` and `session_semantics.rs`, neither
   of which has a root, a guard, or an address, and would make a damaged Session
   unloadable. Rejected.
3. **Reconstructing the proof bytes from the candidate** to reuse
   `preflight_immutable_exact` is impossible: the proof's `idempotency_id` is only
   recoverable from the manifest at the advance generation, by later generations
   the head carries a different one, and sha256 is not invertible. Persisting it
   would be a wire change against a golden-tested wire. Rejected.
4. **Requiring callers to present the proof on every V2 CAS** contradicts the
   seed's re-key, forces every caller to retain proof bytes forever, and
   re-creates the read anyway. Rejected.
5. **Refusal-ordering churn on the fresh-id advance.** If the binding also ran for
   `proof_owned`, a fresh-id advance against a V2 head would move from the
   documented `Durability(ImmutableExactConflict)` to a binding variant.
   `!proof_owned` keeps that classification untouched, which is why the advance
   path needed no edit at all.
6. **Silent collapse of I/O into "absent"** is the exact failure this repo blocks
   on. Hence separate `DurableProofAbsent` / `DurableProofUnreadable` /
   `NonRegularDurableProof` / `DurableProofExceedsCanonicalBound`, hence refusing
   an over-long record instead of classifying a truncated read of it, and hence
   `NotFound` being the only condition that maps to absence.
7. **[critique 4] A partially-written proof.** A strict prefix on disk coexists
   only with a sub-V2 head, so `refuse_unproven_v2_candidate` takes its
   head-floor arm and refuses a generic V2 candidate with
   `TransitionProofRequired` without reading anything, while the advance *retry*
   that converges is `proof_owned` and skips the binding entirely. (The brief
   described this in terms of an `advances` flag, which no shipped predicate
   contains; the shipped predicate is `proof_owned` plus the head's floor.) If a
   strict prefix ever did coexist with a V2 head, T3's truncation case is the
   refusal.
8. **Overclaiming the guarantee.** The binding proves the floor is backed by a
   real, canonical, session-matching record whose bytes are exactly what the
   inventory claims. It proves nothing about the later generation's own
   transition, and it must not compare the record's `idempotency_id` to the
   candidate's — that would refuse every legitimate later generation. Both the
   rustdoc and `report.md` say this.
9. **Unaddressed-store asymmetry.** The binding, like the gate, applies only to
   addressed stores; that is sound only because every unaddressed constructor is
   `#[cfg(test)]`. If a future production caller ever constructs an unaddressed
   store, this binding silently does not apply. Standing constraint, R7.
10. **[critique 2] The metadata-then-open window.** Closed for the observations
    the variants name by re-checking the opened handle, matching
    `load_manifest_file`'s discipline rather than half of it.
