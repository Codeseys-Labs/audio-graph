# Report — audio-graph-3cf2: honest install-stage refusals + a durable abandon path

Date: 2026-08-18. Branch `work/audio-graph-3cf2-refused-install-survivors`,
worktree `.worktrees/3cf2-refused-install-survivors`, base `27be43a`.

Citation policy for this document: **symbol names only, no file:line anchors.**
This wave's diff moves lines in both persistence sources and the repo has no
committed anchor-sweep script, so any new anchor would be stale on the next edit.
The one machine-check that was run is reported in section 7 with exactly what it
covered.

## 1. Commits

| sha | unit |
| --- | --- |
| `f13306f` | `fix(audio-graph-3cf2): classify install refusals that leave a durable proof` |
| `534745a` | `feat(audio-graph-3cf2): add a durable unlink primitive and a public abandon path` |
| `2ae6435` | `docs(audio-graph-3cf2): amend ADR-0044 ownership, record the wave, scope stale claims` |
| review-round `docs` commit | reverts that in-place ADR-0044 amendment; adds proposed ADR-0039 + README index row instead |
| review-round-1 fix commit | rescopes `Durability`'s rustdoc from the transaction to the one refused call, and corrects ADR-0039's unlink receiver to `CanonicalExclusiveGuard` |

Files touched: `src-tauri/src/persistence/session_artifact_manifest.rs`,
`src-tauri/src/persistence/canonical_durability.rs`,
`docs/adr/0039-let-a-session-abandon-its-own-staged-manifest-temporary.md`,
`docs/adr/README.md`,
`docs/commit-state-2026-08-16-session-control-contract-wave7c.md`,
`docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-3b53-report.md`,
`docs/agentic-runs/2026-08-18-audio-graph-3cf2/plan.md`,
`docs/agentic-runs/2026-08-18-audio-graph-3cf2/report.md`.

## 2. What shipped — defect 1

`ManifestInstallDisposition` (private, three states) replaces the
`resume_temporary` bool on `commit_prepared_compare_and_swap` and
`compare_and_swap_inner`. `compare_and_swap` passes `Fresh`,
`compare_and_swap_recovery` and its fault variant pass `ResumeUnstaged`, and
`advance_session_semantics_v1_to_v2_inner` derives
`ResumeStagedTransition { intent_recovery_key }` from `staged_intent_recovery_key`
— the variable that already means "this transaction owns a durable intent
temporary that outlives any later refusal". Install selection is unchanged
(`resume_temporary = !matches!(disposition, Fresh)`); only the `Rejected` arm
changed. `Accepted` and `DurabilityIndeterminate` keep exactly their previous
behaviour.

An install refusal on the transition path is now
`ManifestCasRejection::ManifestInstallRefusedAfterProofAndIntentDurable {
rejection, recovery_key }`: the inner refusal verbatim plus the intent temporary's
key (`recovery_key(&candidate.transition.fingerprint)`, which after fingerprint
assignment equals `recovery_key(&proof_digest)` — the same key
`TransitionProofRefusedAfterIntentStaged` reports for the same temporary).

Three rustdoc contracts changed with it:

- `Durability` now asserts a no-survivor claim scoped to the ONE refused call —
  the same scoping the substrate's `CanonicalDurabilityRejection` already states,
  never transaction-wide — instead of warning about this stage, and states what
  it cannot claim: earlier accepted work on the same `ManifestWriteTransaction`
  (an installed head, a durable proof) survives a later `Durability`, which
  `abandoned_transition_unwedges_a_different_candidate` exercises directly; and
  an orphan temporary the refused call did not stage outlives the refusal with no
  key — *evidenced* on the fresh path (`SnapshotTempAlreadyExists` from a plain
  `compare_and_swap`) and adoptable on the recovery path.
- `TransitionProofRefusedAfterIntentStaged` no longer calls the byte-exact replay
  the only escape; it names both.
- `install_snapshot_recovery` gained the rustdoc it never had — the doc whose
  absence produced this seed.

**Scope of the new variant's claim (critique item 5).** It asserts *provenance*:
this transaction made both records durable and this refusal did not consume them.
That holds for every install-path `Rejected`, all of which are pre-mutation. It
deliberately does **not** re-verify the records' current pathnames, because for an
`IdentityChanged`-class rejection the refusal itself reports that the namespace
moved. The rustdoc says exactly that.

## 3. What shipped — defect 2

`CanonicalExclusiveGuard::unlink_canonical_entry` (plus a cfg(test) fault
variant, both delegating to `unlink_canonical_entry_inner`) is the substrate's
first removal primitive. Order: reserved-name preflight, operation lock, platform
gate, qualification, `bind_parent`, parent-barrier availability, `InspectEntry`
cut then `symlink_metadata`, `OpenExisting` cut then `open_existing_regular` plus
the `validate_snapshot_destination` identity fence, `Unlink` cut before invoking,
`remove_file`, parent barrier. New additive pub variants:
`CanonicalDurabilityStage::Unlink`, `CanonicalNamespaceOperation::Unlink`,
`CanonicalMutation::Unlink`, `CanonicalDurabilityBarrier::ParentNamespace`. New
`#[must_use] pub enum CanonicalUnlinkOutcome`.

`ManifestWriteTransaction::abandon_staged_transition(&self)` is the public entry
point. Recovery key: `SHA-256(domain separator || temporary pathname)[..16]` via
`temporary_abandon_recovery_key` — rerun-stable for a store built from the same
root spelling (the root is not canonicalized, so a trailing separator, a `..`
segment, or a symlinked data dir derives a different key for the same logical
unlink; review finding, caveat now in the rustdoc), distinct across Sessions and
roots, independent of head and candidate, and unequal to a candidate-fingerprint
key for every input by domain separation. The rustdoc states that the key names
the unlink to reconcile, not the abandoned candidate.

**External reachability** is by construction: `pub fn` on a `pub struct` in a
`pub mod`, returning `pub` types, so
`store.begin_write()?.abandon_staged_transition()` compiles outside the crate.
Strict Clippy with `-D warnings` would have failed on `private_interfaces` had any
part of that signature been non-public. No integration test exercises the
out-of-crate call in this wave; the kernel stays dormant with no production caller.

**Capability weakening, stated for `audio-graph-68a1`.** Until this commit,
immutability of the provenance proof was enforced by the *absence* of any removal
capability in the crate — a grep for `remove_file`/`unlink` over both persistence
sources returned exactly one hit, inside a test. That is no longer true. The
substrate refuses only the reserved lock (ASCII-case-insensitively), non-regular
entries, out-of-root and nested targets, and a pathname that no longer names the
opened object; it does **not** know which of the caller's records are meant to be
immutable. Keeping the manifest head and the proof record out of the primitive is
now a caller obligation, stated in `unlink_canonical_entry`'s rustdoc, with the
Session's derived temporary as its only production caller. The B4 assumption that
"the real proof sits untouched at the control identity" is caller-enforced from
here on, which 68a1's proof-binding work must account for.

**Abandon is not a reconciliation for indeterminate outcomes (critique item 2).**
An install that renamed but lost its parent barrier has already consumed the
temporary; abandon then finds it absent and its own parent sync can be what makes
that unacknowledged install durable. The rustdoc scopes abandon to its two named
`Rejected` variants, says the exact rerun keyed by the indeterminate's recovery
key is the reconciliation, and says `AlreadyAbsent` asserts only that this call
removed nothing — never that the head is unchanged.
`abandon_after_an_indeterminate_install_publishes_rather_than_retracts` pins it:
after a `ParentSync`-cut install the manifest exists, abandon returns
`AlreadyAbsent`, and a fresh `begin_write` observes generation 1 at the V2
candidate the caller tried to abandon.

## 4. RED first — verbatim

The criterion-1 test was written before the fix, in a form that compiles against
the pre-fix API (a panic arm on `Durability` instead of a match on the
not-yet-existing variant), so the recorded failure is the misclassification itself
rather than a compile error. Verbatim output of
`cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib
--no-default-features --features cloud refused_manifest_install_after_durable_proof
-- --test-threads=1 --nocapture` at that state:

```text
running 1 test
test persistence::session_artifact_manifest::tests::refused_manifest_install_after_durable_proof_is_never_reported_as_unstaged ...
thread 'persistence::session_artifact_manifest::tests::refused_manifest_install_after_durable_proof_is_never_reported_as_unstaged' (191478) panicked at src/persistence/session_artifact_manifest.rs:4848:90:
install refusal after durable proof misclassified as Durability(IoFailedBeforeMutation { stage: CreateNew, kind: Other, raw_os_error: None }); that variant claims nothing this transaction staged survives, but the staged intent temporary named by CanonicalRecoveryKey([REDACTED]) and the durable proof both do
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
FAILED

failures:

failures:
    persistence::session_artifact_manifest::tests::refused_manifest_install_after_durable_proof_is_never_reported_as_unstaged

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1734 filtered out; finished in 0.02s
```

Every assertion *above* the panic already passed pre-fix: the temporary existed,
the provenance bytes were byte-exact against
`proof.canonical_bytes_and_digest()`, and the manifest was absent. The RED is the
classification alone — the same shape 3b53 recorded. The panic arm was then
replaced by the match on `ManifestInstallRefusedAfterProofAndIntentDurable` that
ships.

## 5. Tests

Seven new tests, plus two in-place edits to existing tests.

New in `canonical_durability.rs`:

- `unlink_refuses_reserved_windows_unqualified_and_non_regular_entries_before_mutation`
  — the coordination basename and a mixed-case alias give
  `ReservedCoordinationEntry` with the lock intact; `qualification = None` gives
  `NamespaceDurabilityUnsupported { platform: current_platform(), operation: Unlink }`;
  a qualification bound to another root gives `QualificationBindingMismatch`; a
  target in a foreign root and a nested descendant of the managed root give
  `TargetOutsideManagedNamespace` (the nested entry's bytes verified intact, which
  is the `bind_parent`-only restriction under test); symlink, directory, fifo, and
  socket give `NonRegularCanonicalEntry`, each surviving; simulated `Windows` and
  `Other` give `NamespaceDurabilityUnsupported { operation: Unlink }` with the
  entry intact.
- `unlink_refuses_a_replaced_managed_root_before_mutation` — a real root
  replacement (rename away, recreate the path) gives `IdentityChanged`, with the
  displaced entry's bytes intact. This is the `ValidateNamespace` preflight
  exercised for real rather than injected.
- `unlink_fault_cuts_are_honest_and_exact_rerun_is_a_no_effect_assessment` —
  `[InspectEntry, OpenExisting, Unlink]` give `IoFailedBeforeMutation { stage }`
  with the bytes unchanged; `CanonicalDurability::failing_at(Unlink)` lands on the
  same pre-invocation arm; `ParentSync` gives `DurabilityIndeterminate` with the
  entry gone, then an exact rerun gives `AlreadyAbsent(ParentNamespace)`; a clean
  run gives
  `Unlinked(CanonicalDurabilityReceipt { mutation: Unlink, barrier: ParentNamespace })`,
  then a rerun gives `AlreadyAbsent(ParentNamespace)`.

New in `session_artifact_manifest.rs`, in one contiguous trailing block:

- `refused_manifest_install_after_durable_proof_is_never_reported_as_unstaged`
  (criterion 1): survivors asserted first (temporary present, provenance bytes
  byte-exact, manifest absent), then the honest classification with
  `recovery_key == recovery_key(&proof_digest)`, then the consequence end to end.
- `abandoned_transition_unwedges_a_different_candidate` (criterion 3), two arms:
  proof absent, abandon, a DIFFERENT transition installs; proof durable, abandon,
  a different v1 candidate installs through the public `compare_and_swap`, and a
  different transition *id* is still `Durability(ImmutableExactConflict)` with the
  abandoned proof's bytes unchanged.
- `exact_rerun_of_abandon_is_a_no_effect_assessment` (criterion 4): nothing staged
  gives `AlreadyAbsent` with nothing created — the coordination entry
  `begin_write` establishes is explicitly excluded and asserted present, not
  claimed absent; then `Unlinked` followed by two `AlreadyAbsent` reruns with the
  v1 head bytes, `head()`'s generation, and the proof bytes byte-identical across
  all three calls.
- `abandon_after_an_indeterminate_install_publishes_rather_than_retracts`
  (critique item 2).

Edited in place:

- `manifest_cas_consumes_staged_intent_and_restarts_every_install_cut` now loops
  `[CreateNew, Flush, ProtectTemp, FileSync, Rename, ParentSync]` with a
  `CreateNew`/indeterminate split. **`Write` is excluded on purpose**: the install
  resumes a complete temporary, so `already_written == bytes.len()`, `remaining` is
  empty, and `install_snapshot_inner`'s injected `Write` arm is skipped. There is
  no byte left to cut, and asserting a non-cut would be theatre.
- The "one reachable escape" comment in
  `refused_proof_create_after_durable_intent_is_never_reported_as_unstaged` now
  names both escapes and points at the abandon test. It would otherwise be a
  comment that misdescribes shipped code.

**Which cuts are injected, and which arm is classified but unexercised.** The
unlink fault table injects `InspectEntry`, `OpenExisting`, `Unlink`
(pre-invocation), and `ParentSync` (post-removal). A *post-invocation*
`remove_file` failure is classified `DurabilityIndeterminate { stage: Unlink }` and
is **not injectable** — the injection points sit before the call so the "entry is
provably intact" claim holds. That arm ships classified but unexercised, and the
fault table does not claim to cover it. Likewise, on the manifest install arm only
`CreateNew` is injected; the other install-path refusals (`IdentityChanged`,
`CoordinationPoisoned`, `SnapshotPathOverlap`, and so on) reach the new variant by
the same code path but are not injected there.

**Consequence recorded, not left for a reviewer to discover.** In criterion 3's
second arm a v1 head is installed while an orphan v2 proof record persists. No
floor is claimed by that: the logical floor derives from the head and
`admitted_session_semantics_floor` consumes the CAS outcome, and a later
byte-exact `advance_session_semantics_v1_to_v2` reconciles the orphan proof as
`Exact`. What it does mean is that a DIFFERENT transition id stays refused with
`ImmutableExactConflict` until that happens — the already-recorded round-4
residue, owned by `audio-graph-68a1`.

## 6. Gates — verbatim

All four gate commands were run from the worktree root at the shipped state with
`CARGO_TARGET_DIR="$PWD/src-tauri/target"`. Rust 1.95.0, `--features cloud`,
serial persistence suite.

All four were re-run unchanged after the review-round-1 fix commit (which touches
only rustdoc, one code comment, and three documents — no executable code) and are
still green: clippy exit 0, fmt exit 0, `243 passed; 0 failed; 1498 filtered out`,
`diff --check` exit 0.

```text
### GATE 1 clippy
    Checking audio-graph v0.1.0-rc.1 (/home/codeseys/DevBox/audio-graph/.worktrees/3cf2-refused-install-survivors/src-tauri)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 15.67s
exit=0
```

```text
### GATE 2 fmt
exit=0
```

```text
### GATE 3 persistence suite
running 243 tests
test result: ok. 243 passed; 0 failed; 0 ignored; 0 measured; 1498 filtered out; finished in 19.49s
exit=0
```

```text
### GATE 4 diff --check
exit=0
```

Rustfmt failed once mid-implementation (three long-line wrappings in the new
unlink code and one test statement); `cargo +1.95.0 fmt --all` fixed it and the
recheck is the exit 0 above. Clippy failed once, before the abandon entry point
landed, with `methods ... are never used` for the then-uncalled unlink functions;
it has been exit 0 since.

The three source-text slice tests that constrain where code may land in
`canonical_durability.rs` all pass, which is the check that the placement
constraint was honoured:

```text
test persistence::canonical_durability::tests::algorithm_snapshot_temp_revalidation_stays_on_the_guard_identity_seam ... ok
test persistence::canonical_crash_harness::tests::every_crash_checkpoint_pair_brackets_its_unique_operation ... ok
test persistence::canonical_crash_harness::tests::first_create_distinguishes_file_sync_from_parent_namespace_sync ... ok
```

The seven new tests:

```text
test persistence::canonical_durability::tests::unlink_fault_cuts_are_honest_and_exact_rerun_is_a_no_effect_assessment ... ok
test persistence::canonical_durability::tests::unlink_refuses_a_replaced_managed_root_before_mutation ... ok
test persistence::canonical_durability::tests::unlink_refuses_reserved_windows_unqualified_and_non_regular_entries_before_mutation ... ok
test persistence::session_artifact_manifest::tests::abandon_after_an_indeterminate_install_publishes_rather_than_retracts ... ok
test persistence::session_artifact_manifest::tests::abandoned_transition_unwedges_a_different_candidate ... ok
test persistence::session_artifact_manifest::tests::exact_rerun_of_abandon_is_a_no_effect_assessment ... ok
test persistence::session_artifact_manifest::tests::refused_manifest_install_after_durable_proof_is_never_reported_as_unstaged ... ok
```

Not run in this wave, and not claimed: the full library test suite beyond the
`persistence` filter, any non-Linux OS probe, any integration, e2e, or frontend
gate, and Betterleaks.

## 7. Documents changed, and the checks that were run

- **ADR-0044** (accepted): **not changed by this branch.** Commit `2ae6435`
  did add a dated in-place amendment paragraph to decision item 3 and rewrite one
  Compliance bullet; review found that this violates `docs/adr/README.md`, which
  states that accepted ADRs are immutable and that changing one requires a new
  superseding record, and ADR-0044 has no self-amendment clause and a
  human-acceptance deciders line. A later review round reverted both hunks, so
  ADR-0044 is byte-identical to its acceptance commit `fcd5d10` and the README
  index row still truthfully presents it as unchanged since 2026-08-16.
  Verified with `git diff --exit-code fcd5d10 -- docs/adr/0038-...md` (exit 0).
  That gate ran on this wave's tip, when the record was numbered `0038`. Seed
  `audio-graph-c306` has since renumbered it to `0044` to resolve a collision
  with master, changing its filename and H1 line, so the command above no
  longer reproduces on later tips and the claim is scoped to this wave.
- **ADR-0039** (new, `proposed`):
  `docs/adr/0039-let-a-session-abandon-its-own-staged-manifest-temporary.md`
  carries the ownership scope change instead. ADR-0044 item 3's "Delete owns any
  retained Session temporary as recovery residue" sentence and its "one explicit
  ownership rule" driver do not describe shipped code, because
  `abandon_staged_transition` is a second, non-delete remover of a Session
  temporary. ADR-0039 refines that item in part: the single-owner rule is scoped
  to lifecycle operations acting from outside a write transaction, a Session may
  abandon its own retained temporary from inside its exclusive-guard write
  transaction, and the removal primitive's fences are recorded (store-owned lock
  refused under every ASCII-case spelling before any filesystem access,
  non-regular and out-of-root targets refused, immediate children only). It
  supersedes nothing. Because it is `proposed`, ADR-0044 item 3 remains in force
  and the landed entry point still has no production caller; acceptance is the
  human decider's, and nothing here claims it.
- **`docs/adr/README.md`**: one index row and one link definition for ADR-0039,
  in the same style as ADR-0042's "Refines ADR-0031" row. ADR-0044's row is not
  touched, matching the existing convention that the refining record carries the
  relationship note.
- **`docs/commit-state-2026-08-16-session-control-contract-wave7c.md`**: a
  continuation section dated 2026-08-18 recording that the round-4 install-stage
  residual is closed, that the wedge now has a second escape, and that any earlier
  sentence describing the replay as the only way out is a record of its own round.
  It cites no line numbers.
- **`docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-3b53-report.md`**:
  one dated line added to the round-5 anchor table's preamble, and a parenthetical
  scoping the round-3 narrative's "one reachable escape" sentence to that commit.

**Machine-check performed.** A script enumerated all 15 rows of that report's
round-5 anchor table by regex over the table rows, read each cited file at
`f27e322` via `git show`, and asserted that a backticked fragment of each row's
expectation appears on the cited line. Result: **15 rows enumerated, 15 held at
`f27e322`, 0 failures.** The same script re-run against this worktree's shipped
sources reports **15 of 15 rows now stale**, because this wave's enum-variant,
rustdoc, and function additions sit above every cited line in both files. That is
exactly the fact the added preamble line records. No other document's anchors were
checked, and neither this report nor the plan adds any.

**Second check performed.**
`git diff --exit-code fcd5d10 -- docs/adr/0038-keep-session-control-plane-in-the-flat-artifact-root.md`
exited 0 on this wave's branch tip, which is the assertion that accepted
ADR-0044 — numbered `0038` when this check ran — is byte-identical to its
acceptance commit after the amendment revert. Seed `audio-graph-c306` later
renumbered that record to `0044`, so the path above is the one the check
actually used and the result is scoped to this wave's tip. ADR-0039
cites no file line numbers; its two test-name blocks were checked by grepping
each name in the named source file.

**Third check performed (added in review round 1).** The second check above did
NOT cover ADR-0039's non-test symbol citations, and one of them was wrong: the
unlink primitive was cited as `CanonicalDurabilityGuard::unlink_canonical_entry`,
a receiver type that exists nowhere in the crate. It is now
`CanonicalExclusiveGuard::unlink_canonical_entry`. A script closes the gap by
enumerating every backticked Rust-path-shaped citation in ADR-0039, this plan,
and this report, and requiring each `::`-separated segment to appear as an
identifier token somewhere under `src-tauri/src`. Result: **ADR-0039 17 paths
enumerated / 16 resolve, plan 92 / 90, report 90 / 84.** Every unresolved token
was inspected and is not a Rust symbol: the ADR status word `proposed`, the
commit shorthands `f13306f` / `f27e322` / `fcd5d10`, the commit type `feat`, and
the rustc lint name `private_interfaces`. The receiver of the corrected ADR
citation was checked more strictly than token presence: `impl
CanonicalExclusiveGuard` opens the only impl block enclosing
`unlink_canonical_entry`'s definition, with no intervening block close.

## 8. Residuals, with owners

| residual | owner |
| --- | --- |
| Re-keying the transition-proof gate in `prepare_compare_and_swap` / `validate_v2_session_provenance` / `SessionProvenanceEvents` validation; the B4 wedge stays until the candidate's provenance entry can be proven to reference the durable proof record | `audio-graph-68a1` (untouched here, blocked on the proof binding) |
| Removing or re-keying the durable proof record; a durable v2 proof still refuses a different transition id with `ImmutableExactConflict` after an abandon | `audio-graph-68a1` |
| Immutability of the proof record is now caller-enforced rather than guaranteed by the absence of a removal capability | `audio-graph-68a1` must account for it; the trust boundary is stated in `unlink_canonical_entry`'s rustdoc |
| Post-invocation `remove_file` failure: classified `DurabilityIndeterminate { stage: Unlink }`, not injectable, ships unexercised | open, this seed's substrate; a future fault-injection seam would be needed |
| Install-path refusals other than `CreateNew` reach the new variant uninjected (`IdentityChanged`, `CoordinationPoisoned`, `SnapshotPathOverlap`) | open, low value: they share one code path with the injected case |
| Consumer activation: no production caller for `for_session`, no `admitted_session_semantics_floor` call, no `canonical_log.rs` change; the addressed-identity reservation gap named in the 3b53 report stays open | outside this seed |
| Windows test cfg-gating defects on a real NTFS host | `audio-graph-a58b` |
| Rotation HomeGuard known-folder override gap | `audio-graph-0641` |
| A general delete/purge facility for nested Session artifacts (ADR-0027) | unimplemented; `bind_parent` deliberately keeps this primitive to immediate children |
| ADR-0039 is `proposed`: accepted ADR-0044 item 3 still names delete as the owner of a retained temporary, so the shipped second remover has no accepted decision behind it until a human decides | the human decider; nothing in production calls the abandon entry point meanwhile |

## 9. Deliberate deviations from the brief

1. **The abandon recovery key was pinned, not chosen from the brief's menu.** The
   brief left the derivation to the implementer with two candidates on offer; both
   break `CanonicalRecoveryKey`'s "same value for the same logical mutation"
   contract — a shared constant collides across Sessions, and a head-derived key
   changes across reruns once the head moves. Shipped instead:
   `SHA-256(domain separator || temporary pathname)[..16]`.
2. **The critique's indeterminate interleaving is reproduced with a `ParentSync`
   cut, not a `Rename` one.** An injected `Rename` fault cuts *before* invoking the
   rename, so it cannot produce "rename invoked, outcome lost". The `ParentSync`
   cut does exactly that — the rename ran and only its namespace barrier was lost
   — so the test drives the real hazard instead of a fault that cannot express it.
3. **Two extra refusal cases were added to the substrate test beyond the brief's
   list**: a nested descendant of the managed root (proving the `bind_parent`
   restriction rather than only asserting it), and a socket alongside
   symlink/directory/fifo, matching the existing collision test's idiom.
4. **The RED was captured with a temporary panic arm** rather than by compiling
   against a variant that did not yet exist, so the recorded failure is the
   misclassification the seed is about rather than a type error. The final test
   matches the shipped variant.
