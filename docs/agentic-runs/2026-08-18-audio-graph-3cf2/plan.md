# Plan — audio-graph-3cf2: honest install-stage refusals + a durable abandon path

Date: 2026-08-18. Seed: `audio-graph-3cf2` (substrate residual of
`audio-graph-3b53` round 4, tracked under `audio-graph-7e18`). Worktree
`.worktrees/3cf2-refused-install-survivors`, branch
`work/audio-graph-3cf2-refused-install-survivors`, base `27be43a`.

This is the design brief as amended by every required change from the adversarial
critique. It cites **symbol names only, no file:line anchors**: the diff moves
lines in both persistence sources and this repo has no committed anchor-sweep
script.

## 1. The two coupled defects

**Defect 1 — misclassification one stage later than the one 3b53 fixed.** On the
v1-to-v2 transition path, `install_snapshot_inner` can refuse at `CreateNew`
*after* `advance_session_semantics_v1_to_v2_inner` has already accepted both the
intent temporary (`stage_snapshot_temporary`, file- and parent-synced) and the
immutable transition proof (`create_or_reconcile_immutable_exact_with_authentication`,
whose create authenticates the staged temporary byte-exactly first, so the
temporary's existence is proven rather than inferred).
`commit_prepared_compare_and_swap` forwarded every such refusal as plain
`ManifestCasRejection::Durability`, whose contract is that the refused call
staged nothing that survives. That is worse than the case 3b53 closed: a canonical
proof record exists on disk while the outcome claims no mutation. The stage was
untested — `manifest_cas_consumes_staged_intent_and_restarts_every_install_cut`
looped only `FileSync`, `Rename`, `ParentSync`.

**Defect 2 — no abandon path.** 3b53 proved RESUME is reachable. Nothing could
ABANDON: `compare_and_swap_recovery` is `pub(crate)`, and the durability
substrate had no removal primitive at all — the only `remove_file` in either
source lived inside a test. A caller that wanted to give up on a transition and
install a different candidate stayed wedged on `SnapshotTempAlreadyExists`
forever.

## 2. Defect 1 — `session_artifact_manifest.rs`

1. New private `ManifestInstallDisposition { Fresh, ResumeUnstaged,
   ResumeStagedTransition { intent_recovery_key } }` beside
   `ManifestCasPreparationResult`. Three states, not the existing
   `resume_temporary` bool: widening the fix to `resume_temporary == true` would
   mislabel `compare_and_swap_recovery` refusals, where a surviving temporary
   pre-dates the transaction and the transaction owns no key for it.
2. `commit_prepared_compare_and_swap` and `compare_and_swap_inner` take the
   disposition instead of the bool. Install selection is unchanged
   (`resume_temporary = !matches!(disposition, Fresh)`); only the `Rejected` arm
   changes. `Accepted` and `DurabilityIndeterminate` are untouched — indeterminate
   already carries the key and already claims nothing.
3. `advance_session_semantics_v1_to_v2_inner` derives the disposition from
   `staged_intent_recovery_key`, the variable that already means "this
   transaction owns a durable intent temporary that outlives any later refusal".
   `AlreadyCompleted` preparations return before the install and never reach the
   arm.
4. New `ManifestCasRejection::ManifestInstallRefusedAfterProofAndIntentDurable {
   rejection, recovery_key }`, placed directly after
   `TransitionProofRefusedAfterIntentStaged` so the pair stays adjacent.
5. `Durability`'s rustdoc is rewritten to assert a no-survivor contract scoped to
   the one refused call — matching the substrate scoping, not transaction-wide —
   instead of warning about this stage, and to state what it still cannot claim,
   including that earlier accepted work on the same transaction survives.

### Rustdoc scope decisions forced by the critique

- **The new variant's survivor claim is scoped to provenance, not location.**
  Every install-path `Rejected` is pre-mutation, so "this transaction made both
  records durable and this refusal did not consume them" is true for all of them.
  The *location* claim is not: under a replaced managed root the refusal is
  `IdentityChanged` and the records sit in a displaced tree. The rustdoc
  therefore asserts provenance and explicitly declines to re-verify the current
  pathnames for `IdentityChanged`-class rejections. Only `CreateNew` is injected
  on this arm; the report says so.
- **`Durability` also documents the fresh path.** A foreign orphan temporary is
  *evidenced* through this variant on the fresh path (`SnapshotTempAlreadyExists`
  from a plain `compare_and_swap` is exactly that evidence), not only tolerated
  on the recovery path.
- **`TransitionProofRefusedAfterIntentStaged`'s rustdoc and the in-test comment
  in `refused_proof_create_after_durable_intent_is_never_reported_as_unstaged`
  both claimed the byte-exact replay was the ONLY escape.** Both become false the
  moment abandon lands, so both are edited in this diff. The same "only escape"
  phrasing in the historical 3b53 report is scoped to its own round with a dated
  parenthetical; the live commit-state document gets a continuation entry that
  does not restate it as current.

## 3. Defect 2 — `canonical_durability.rs`

**Placement constraint.** Three tests slice this file as text:
`algorithm_snapshot_temp_revalidation_stays_on_the_guard_identity_seam` slices
`install_snapshot_inner` → `checked_snapshot`, and `canonical_crash_harness.rs`
slices `rename_inner` → the `install_snapshot` doc line and `append_opened` →
`open_existing_regular`. Everything new goes **between `sync_recovery_namespace_entry`
and `preflight_immutable_exact`'s doc comment**, with the free helpers after
`immutable_namespace_unsupported`. Nothing lands between any boundary pair.

1. Additive pub-enum variants (no exhaustive match over any of them exists
   outside the module): `CanonicalDurabilityStage::Unlink`,
   `CanonicalNamespaceOperation::Unlink`, `CanonicalMutation::Unlink`, and
   `CanonicalDurabilityBarrier::ParentNamespace`. `ParentNamespace` is
   honesty-forced: an unlink crosses no file barrier, so returning
   `FileAndParentNamespace` would be a false durability claim. A stage named
   `Rename` inside a removal would be a doc/code disagreement.
2. New `pub enum CanonicalUnlinkOutcome { Unlinked(receipt),
   AlreadyAbsent(barrier), Rejected(rejection), DurabilityIndeterminate(_) }`,
   `#[must_use]`. A separate type, not a fourth `CanonicalDurabilityOutcome`
   variant: that enum is matched exhaustively across the crate and a removal has
   an absence assessment no other mutation has.
3. `unlink_canonical_entry` / `unlink_canonical_entry_with_fault` (cfg(test)) →
   `unlink_canonical_entry_inner`, ordered exactly like the module's other
   mutations:
   1. `preflight_mutation_targets([path])` → `ReservedCoordinationEntry`, which
      is what makes the store-owned lock unremovable including every ASCII-case
      alias.
   2. `operation_lock` → `CoordinationPoisoned`.
   3. platform gate → `unlink_namespace_unsupported`.
   4. `qualification_status` → `Rejected` / `unlink_namespace_unsupported`.
   5. `bind_parent`, **never** `bind_descendant_parent`: immediate children of
      the managed root only, so this can never become ADR-0027's unimplemented
      subtree delete/purge.
   6. parent barrier availability → `unlink_namespace_unsupported`.
   7. injected `InspectEntry` cut; then `symlink_metadata`: `NotFound` → absent
      arm, non-regular → `NonRegularCanonicalEntry`, other error →
      `IoFailedBeforeMutation { stage: InspectEntry }`.
   8. injected `OpenExisting` cut; then `open_existing_regular` (the
      `IdentityChanged` fence) and `validate_snapshot_destination(path,
      Existing(&file))` — the cfg(test)-aware substitution fence proving the
      pathname still names the exact object being removed.
   9. injected `Unlink` cut **before invoking**, plus `injected_error(Unlink)` so
      `CanonicalDurability::failing_at(Unlink)` lands on the same arm; the entry
      is provably intact.
   10. `remove_file`: error → `DurabilityIndeterminate { stage: Unlink }`. A raced
       `NotFound` folds into indeterminate deliberately — the barrier has not
       been crossed, so absence is not durable yet and the exact rerun resolves
       it.
   11. absent arm: parent barrier, then `AlreadyAbsent(ParentNamespace)`. Without
       that barrier a lost `ParentSync` would have no reconciliation and an
       unpublished removal would be called durable.
   12. removed arm: parent barrier, then `Unlinked(receipt{ Unlink,
       ParentNamespace })`.
4. **`unlink_canonical_entry` carries a stated trust boundary** (critique
   requirement). This is the crate's first removal capability; until now
   immutability of the proof record was enforced by the *absence* of the
   capability, and tests such as the foreign-proof `ImmutableExactConflict` case
   rely on that. The rustdoc states that the substrate refuses only the reserved
   lock, non-regular entries, out-of-root targets, and a pathname that no longer
   names the opened object — and that identity discipline (never the manifest,
   never an immutable proof) is the caller's obligation, its only production
   caller being the Session's derived temporary. The report names this
   capability-weakening so `audio-graph-68a1`'s proof-binding work knows the
   immutability convention is now caller-enforced.
5. `install_snapshot_recovery` gets the rustdoc it never had: on the resume
   disposition a `Rejected` means *this operation* mutated nothing and is not a
   claim that the temporary pathname is empty. Behaviour-neutral, and it is the
   doc whose absence produced this seed.

## 4. `abandon_staged_transition`

`pub fn abandon_staged_transition(&self) -> CanonicalUnlinkOutcome` on
`ManifestWriteTransaction`, which already owns the exclusive guard, the
qualification, and both derived paths. `&self`, not `&mut self`, because no
cached field changes — the type shows it rather than a comment claiming it. No
`SessionArtifactManifestStore` wrapper: `begin_write()` is already `pub` and both
persistence modules are `pub mod`, so
`store.begin_write()?.abandon_staged_transition()` is reachable from outside the
crate; a second entry point would be surface with no caller in a dormant kernel.

**Recovery key (pinned, not left to the implementer).** The brief offered a
shared constant seed and a head-derived key; both violate
`CanonicalRecoveryKey`'s contract — the constant makes two different Sessions'
abandons carry the same key, and the head-derived one changes across reruns once
the head moves. The key is instead `SHA-256(domain separator ‖ temporary
pathname)[..16]` via `temporary_abandon_recovery_key`: distinct across Sessions
and roots, identical on every rerun by any transaction of a store built from the
same root spelling (the root is not canonicalized, so equivalent spellings
derive different keys — the rustdoc carries the caveat), and
independent of the head and of any candidate. The domain separator makes it
unequal to a candidate-fingerprint key for every input. The rustdoc states that
the key names *the unlink to reconcile*, not the abandoned candidate.

**Rustdoc scope, forced by the critique.** Abandon is the escape for the two
named `Rejected` variants only. After any `DurabilityIndeterminate` the
reconciliation is the exact rerun keyed by that outcome's recovery key: an
indeterminate install whose rename was invoked but unacknowledged has already
consumed the temporary, so abandon finds it absent and its own parent sync can
become the durability point of that unacknowledged install. `AlreadyAbsent`
therefore asserts only that this call removed nothing — never that the manifest
head is unchanged.

Abandon removes ONLY the temporary. The immutable proof stays, so a durable v2
proof still refuses a DIFFERENT transition id with `ImmutableExactConflict`.
The temporary's content is deliberately not authenticated: requiring the bytes
would reinstate the wedge abandon removes. Authorization is the exclusive guard,
the derived Session-owned temporary identity (never caller-supplied, never equal
to the manifest identity), and the substrate's fences.

## 5. Tests, mapped to acceptance criteria

Grouping rule for the concurrent `audio-graph-68a1` rebase: every new manifest
test sits in one contiguous block at the **end** of `session_artifact_manifest.rs`'s
`mod tests`; the only in-place edits to existing tests are the loop header plus
one `if` branch in `manifest_cas_consumes_staged_intent_and_restarts_every_install_cut`
and one comment in `refused_proof_create_after_durable_intent_is_never_reported_as_unstaged`,
both far from 68a1's sites.

1. **Criterion 1, RED first.**
   `refused_manifest_install_after_durable_proof_is_never_reported_as_unstaged`
   drives a `CreateNew` manifest fault and asserts, in order: which files survive
   (temporary exists; provenance bytes byte-exact; manifest absent), then the
   honest classification with `recovery_key == recovery_key(&proof_digest)`, then
   the consequence end to end (a plain `compare_and_swap` refused
   `SnapshotTempAlreadyExists`, and a byte-exact replay `Accepted` with the
   temporary gone). Also extends the install-cut loop to `[CreateNew, Flush,
   ProtectTemp, FileSync, Rename, ParentSync]` with a `CreateNew`/indeterminate
   split. `Write` is deliberately excluded: on a resumed complete temporary
   `already_written == bytes.len()`, so the injected write arm is skipped and
   asserting a non-cut would be theatre.
2. **Criterion 2, substrate.** Three tests in `canonical_durability.rs`:
   reserved-name (including an ASCII-case alias), unqualified, foreign
   qualification, out-of-root, nested-descendant, Windows/Other, and
   non-regular refusals with the entry surviving each time; a real replaced
   managed root → `IdentityChanged`; and the fault table
   `[InspectEntry, OpenExisting, Unlink]` → `IoFailedBeforeMutation` with the
   bytes intact, `failing_at(Unlink)` on the same arm, `ParentSync` →
   `DurabilityIndeterminate` with the entry gone then an exact rerun →
   `AlreadyAbsent(ParentNamespace)`, and a clean run →
   `Unlinked(Unlink, ParentNamespace)` then `AlreadyAbsent`.
3. **Criterion 3.** `abandoned_transition_unwedges_a_different_candidate`, two
   arms in one test so the boundary of abandon is pinned, not implied: proof
   absent (fault in the proof slot) → abandon → a DIFFERENT transition installs;
   proof durable (fault in the manifest slot) → abandon → a different v1
   candidate installs through the public CAS, **and** a different transition id
   is still `Durability(ImmutableExactConflict)`.
4. **Criterion 4.** `exact_rerun_of_abandon_is_a_no_effect_assessment`: abandon
   with nothing staged → `AlreadyAbsent` and nothing created (scoped to exclude
   the coordination entry `begin_write` establishes); then `Unlinked` followed by
   two `AlreadyAbsent` reruns with the v1 head bytes, its generation, and the
   proof bytes byte-identical across all of them.
5. **Critique-forced.** `abandon_after_an_indeterminate_install_publishes_rather_than_retracts`
   pins the documented caveat: an install indeterminate at `ParentSync` renamed
   but lost its barrier, abandon returns `AlreadyAbsent`, and a fresh
   `begin_write` observes the head advanced to the candidate the caller tried to
   abandon.

## 6. ADR-0044 and the new proposed ADR-0039

ADR-0044 §3 says "Delete owns any retained Session temporary as recovery
residue" and its drivers ask for **one explicit ownership rule**;
`abandon_staged_transition` is a second, non-delete owner of temporary removal,
so that sentence does not describe shipped code. ADR-0044 is `accepted`, and
`docs/adr/README.md` states that accepted ADRs are immutable — to change one,
write a new ADR. It carries no self-amendment clause, and its deciders line
requires human acceptance.

So the scope change is a NEW record, not an edit:
`docs/adr/0039-let-a-session-abandon-its-own-staged-manifest-temporary.md`,
status `proposed`, refining ADR-0044 decision item 3 in part and superseding
nothing. It scopes the single-owner rule to lifecycle operations acting from
outside a write transaction, allows a Session to abandon its own retained
temporary from inside its exclusive-guard write transaction, and records the
removal primitive's fences (store-owned lock refused under every ASCII-case
spelling before any I/O, non-regular and out-of-root targets refused, immediate
children of the qualified root only). ADR-0044's own bytes stay identical to its
acceptance commit `fcd5d10`; the README index gains an 0039 row and link and
ADR-0044's row is untouched, matching how ADR-0042 records "Refines ADR-0031"
without editing ADR-0031's row. (Both records were numbered 0038 and 0036 while
this wave ran; seed `audio-graph-c306` later renumbered them, which is the only
subsequent change to ADR-0044's bytes.)

Because ADR-0039 is `proposed`, ADR-0044 item 3 remains the in-force rule and
the landed primitive keeps having no production caller. A later review round of
this wave reverted an earlier in-place amendment of ADR-0044 (commit `2ae6435`)
in favour of this arrangement.

## 7. Out of scope

- Re-keying the transition-proof gate in `prepare_compare_and_swap`,
  `validate_v2_session_provenance`, or `SessionProvenanceEvents` validation:
  untouched. `audio-graph-68a1` owns them and this branch lands first. The B4
  comment block is not edited.
- Removing or re-keying the durable proof record. Abandon unlinks the temporary
  only.
- Any consumer activation: no production caller for `for_session`, no
  `admitted_session_semantics_floor` call, no `canonical_log.rs` change.
- Making `compare_and_swap_recovery` public, or giving its refusal a key for a
  foreign orphan: it has none to give.
- A general delete/purge facility: `bind_parent` restricts the primitive to
  immediate children of the managed root.

## 8. Failure modes considered and rejected

- Reusing `FileAndParentNamespace` or `Rename` for a removal: false claim and
  doc/code disagreement.
- `AlreadyAbsent` without the parent barrier: no reconciliation for a lost
  `ParentSync`.
- A fourth `CanonicalDurabilityOutcome` variant: breaks exhaustive matches for no
  gain.
- Authenticating the temporary's bytes before abandoning: reinstates the wedge.
- `ensure_no_ascii_case_alias` in the unlink path: its only refusal is
  `ImmutableExactConflict`, a misleading name for an unlink, and
  `install_snapshot_inner` — which *creates* this temporary — does not
  alias-reserve either. The safety-critical alias case, the coordination lock, is
  already covered case-insensitively by `preflight_mutation_targets`. The
  omission is a decision, not an oversight.
- Abandoning a live cooperating process's staged intent: under the exclusive
  guard no other process is mid-transaction, and a process that lost the guard
  race while holding a byte-exact resume candidate is legitimately linearized
  behind the abandon. The cooperative-lock caveat is the module's existing one.
- A v1 head installed while an orphan v2 proof persists (criterion 3, arm 2): no
  floor is claimed — the floor derives from the head and
  `admitted_session_semantics_floor` consumes the CAS outcome — and a later
  byte-exact `advance` reconciles the proof as `Exact`.
- Post-invocation `remove_file` failure is classified `DurabilityIndeterminate`
  and is *not* injectable; the fault table must not claim to cover it, and the
  report says which cuts are injected and which arm is classified-but-unexercised.
