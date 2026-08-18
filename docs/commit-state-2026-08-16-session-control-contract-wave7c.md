# Session Control Contract Wave 7C Commit State

Date: 2026-08-16

## Fixed custody

- Seed: `audio-graph-3b53`, first serial implementation child of
  `audio-graph-7e81` under accepted ADR-0038.
- Exact base: `b5145b2b630a38df7065905263139575b44ead7e`.
- Branch: `work/audio-graph-3b53-session-control-contract-wave7c`.
- Worktree:
  `/home/codeseys/DevBox/audio-graph/.worktrees/3b53-session-control-contract-wave7c`.
- Initial state: clean; no staged, unstaged, or untracked paths.
- One implementer owns this worktree. The conductor retains Seeds, review,
  integration, push, merge, and worktree-cleanup authority.

## Accepted authority and current evidence

- ADR-0038 is accepted at this base and keeps Session control artifacts in the
  qualified flat root under one store-owned global lock.
- `sessions::session_id_is_valid` is the production addressability authority:
  non-empty, at most 128 bytes, ASCII alphanumeric/hyphen/underscore.
- The dormant manifest wire intentionally retains a broader, at-most-255-byte
  UTF-8 Session-id validator. Wire validity alone grants no control path.
- The existing manifest store is explicit-root and dormant, but its manifest
  and temporary identities are root-wide constants. It has no production
  Session control caller or persisted migration.
- The durability substrate already owns qualified exact-root shared/exclusive
  coordination and atomic snapshots. Shared acquisition does not create a
  missing lock; exclusive acquisition establishes the one global entry.
- Windows production qualification refuses namespace mutation before
  coordination-file creation. Unqualified production manifest write already
  refuses.

These are exact-base code and accepted-ADR facts. Prior Linux/macOS/Windows
protocol evidence validates inherited primitives only; it does not establish
the new per-Session address, checked-read, proof, or transition operations.

## Owned footprint

Production and inline-test ownership is limited to:

- `src-tauri/src/persistence/session_artifact_manifest.rs`;
- `src-tauri/src/persistence/canonical_durability.rs` only for a narrow
  shared-guard or immutable exact-create seam that the manifest module cannot
  implement honestly;
- `src-tauri/src/persistence/mod.rs` only if a narrow module/export change is
  required;
- this commit-state document;
- `docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-3b53-plan.md`;
  and
- `docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-3b53-report.md`.

No Seed, workflow, dependency/lockfile, frontend, writer, projection, consumer,
lifecycle-parity, generated contract, or other path is in scope. Inherited
test-only root-store helpers may remain for regression compatibility; they do
not become a production root-wide constructor.

## TDD, commits, and stop lines

Implementation follows the public/crate-public seams and vertical RED/GREEN
slices in the 3b53 plan. Each logical slice records its exact RED and GREEN and
commits separately with `audio-graph-3b53` in the message. Planning lands first;
the final report lands after all source gates.

Stop and report rather than widen scope if the contract requires a consumer or
lifecycle file, a new dependency/workflow, a fifth canonical stream, a
per-Session directory or lock, Windows mutation acceptance, or an ADR change.
No `sd sync`, push, merge, workflow dispatch, release, or worktree cleanup is
authorized.

## Rollback and verification

The branch is runtime-dark. Before a production caller adopts it, rollback is
the ordered planning, addressing/read, proof, transition, and report commits;
there is no persisted migration to reverse.

Final verification includes focused manifest/canonical tests, locked cloud
check, strict Clippy and rustfmt, one full serialized cloud library suite,
Windows production and cfg(test) compile/probes, `bun run verify:fast`, all
contracts, Betterleaks, docs/Seeds secret hygiene, exact base-range diff and
footprint, and runtime-dark searches. The report records real output and any
out-of-scope finding.

## Round-2 durable handoff — 2026-08-17

### Exact state

- Source checkpoint HEAD before this docs-only handoff:
  `3ddeb0afbb602a533295fcb06910d0bd53464ff1`.
- Starting round-2 base: `7c5ef306914a9007cf362448bf333ede6b4ee569`.
- Branch: `work/audio-graph-3b53-session-control-contract-wave7c`.
- Worktree:
  `/home/codeseys/DevBox/audio-graph/.worktrees/3b53-session-control-contract-wave7c`.
- Source commit:
  `3ddeb0a fix(audio-graph-3b53): authenticate partial proof recovery`.
- Source footprint remains exactly the two authorized persistence modules.
  Documentation changes remain within this commit-state, the 3b53 plan, and
  the 3b53 report. No Seed, workflow, dependency, consumer, integration, or
  accepted-ADR path changed.

### Round-2 contract and order

The round-1 proof-prefix threshold was not crash-complete: create-before-write
can leave an empty fixed proof final, and a short write can stop before any
proof-internal identity boundary. Round 2 uses the already-owned per-Session
manifest temporary as the durable authentication record and adds no path.

That replacement is complete only on the `Install` preparation path, which is the
one this order describes. The `AlreadyCompleted` retry branch — the
`cfg(not(test))` copy at `session_artifact_manifest.rs:1262-1270`, not the
`cfg(test)` twin above it — still calls
`create_or_reconcile_immutable_exact_with_identity_prefix` with
`recovery_identity_prefix_len`, so the round-1 prefix-length threshold still runs
in production there. This document previously implied the threshold was gone
everywhere; it is not. The 3b53 plan's round-2 section, which previously denied
a prefix-length heuristic outright, now carries the same `Install`-scoped
wording after the docs-only correction under Seed `audio-graph-bd54`. The 3b53
report names that second entry point and leaves its disposition to Spec review.

The exact order at the public transition transaction is:

1. pure candidate/head/generation/idempotency preparation;
2. non-mutating proof collision preflight — it creates, writes, truncates, and
   syncs nothing, but it is not a read-only open: `preflight_immutable_exact`
   reaches `open_existing_regular`, which opens the existing entry
   `.read(true).write(true)` to revalidate handle identity, so an entry that
   cannot be opened for writing is refused at this step. An earlier revision of
   this document called it a "read-only proof collision preflight", which
   misdescribed the open mode;
3. exact manifest-temporary stage with file and parent barriers;
4. exact temporary byte and open-handle revalidation under the same exclusive
   guard;
5. empty/strict-prefix proof create or reconcile;
6. proof file and parent barriers;
7. existing snapshot recovery consuming the same temporary; and
8. manifest reopen before `Accepted`, or exact `AlreadyCompleted` after a
   post-rename uncertain retry.

The dead `_after_snapshot` checked-read test callback was removed. No new proof
temporary, per-Session lock/directory, fifth stream, or Windows mutation path
exists.

### RED and GREEN evidence

Public-seam REDs:

```text
exact_transition_restarts_after_post_create_empty_proof_final
  retry not Accepted; 0 passed, 1 failed, 1726 filtered, exit 101
exact_transition_restarts_after_one_byte_proof_final
  observed 78-byte forced prefix versus expected 1 byte; exit 101
transition_intent_stage_faults_are_honest_and_exact_retry_converges
  CreateNew injection ignored; exit 101
manifest_cas_consumes_staged_intent_and_restarts_every_install_cut
  ParentSync injection ignored after FileSync/Rename cases; exit 101
```

Focused GREEN at `3ddeb0a`:

```text
cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud persistence::session_artifact_manifest::tests -- --test-threads=1
  49 passed; 0 failed; 1682 filtered; 8.99s
cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud persistence::canonical_durability::tests -- --test-threads=1
  50 passed; 0 failed; 1681 filtered; 3.25s
cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --all -- --check
  passed after formatting
git diff --check
  passed
```

Cross-proof coverage uses empty, one-byte, one byte below the former identity
boundary, and one byte below the complete record. Proof B cannot mutate either
proof A's prefix or its exact staged intent; a fresh exact-A transaction
converges. CAS cut coverage proves FileSync, Rename, and post-rename ParentSync
remain indeterminate on the first attempt, with exact fresh retry returning
only `Accepted` or authoritative `AlreadyCompleted` as appropriate.

### Broad gates not yet rerun

The locked production/test checks, strict Clippy, full serialized cloud suite,
Windows actual-module probes, contracts, aggregate `verify:fast` plus its
repo-pinned Seeds fallback, Betterleaks, secrets, exact footprint, and
runtime-dark searches have not run after round 2. Earlier report results do not
clear `3ddeb0a`.

Update after the in-footprint Clippy correction: strict Clippy failed once on
`3ddeb0a` for `too_many_arguments` at
`canonical_durability.rs:1496` and is green after adopting the file's existing
`#[allow(clippy::too_many_arguments)]` idiom. Strict Clippy, the focused
`--lib persistence` suite (233 passed, 0 failed), and `cargo fmt --all -- --check`
are the only gates run since; their real output is in the 3b53 report. Every other
gate in the paragraph above is still outstanding, and the footprint did not widen.

Update after the round-3 classification correction (uncommitted at this
checkpoint): the checklist's "proof CreateNew classification after intent
staging" item was a real blocking defect and is now fixed under TDD.
`advance_session_semantics_v1_to_v2_inner` forwarded a refused provenance
`create_new` as `Durability(IoFailedBeforeMutation)` — "nothing was staged" —
after it had already accepted a file- and parent-synced intent temporary
(`session_artifact_manifest.rs:1130,1176`), so one EMFILE/ENOSPC/EDQUOT/EACCES
wedged every later `compare_and_swap` (`:975`) with `SnapshotTempAlreadyExists`
and returned no recovery key. `ManifestCasRejection::TransitionProofRefusedAfterIntentStaged`
(`:529`) now reports the inner refusal verbatim plus the staged intent's recovery
key, and only when a staged intent really exists (`:1271`); `Durability` states
its own no-survivor contract (`:518`) and the durability enum's doc scopes the
pre-mutation claim to one operation (`canonical_durability.rs:404`). Two new
tests (`:3627`, `:3708`) pin the truthful variant, prove the wedge and the
byte-exact public resume, and fill the entirely empty proof-stage fault table
across `CreateNew`, `Flush`, `ProtectTemp`, `FileSync`, and `ParentSync`. Strict
Clippy (exit 0) and `--lib persistence` (235 passed, 0 failed) are green on this
state; the footprint stayed at exactly 5. Real output is in the 3b53 report's
correction round 3 section.

Update after round 4 (commit `58b1e3a`): B4 was implemented and then REVERTED.
The transition-proof gate in `prepare_compare_and_swap` is still keyed on the
candidate's floor rather than on whether the call performs the advance, so on an
addressed store every V2 candidate without owned proof bytes is refused
`TransitionProofRequired`. Once a Session commits a V2 head — exactly the state
rounds 2 and 3 left their fixtures in — all four write paths remain closed: a V1
candidate to `SessionSemanticsFloorRegression`, a V2 candidate to
`TransitionProofRequired`, a fresh-id `advance_session_semantics_v1_to_v2` to
`ImmutableExactConflict` on the existing proof, and the same id with changed
artifacts to `TransitionConflict`. `compare_and_swap_recovery` shares the gate and
`SessionArtifactManifestV1::candidate` hardcodes `V1`, so quarantine recovery on
an advanced Session has no admissible floor either — it is fully closed for every
candidate shape, before and after this round.

Re-keying the gate on "this call advances the head" WAS written, and was reverted
after adversarial review: relaxing it for an already-advanced head let the generic
`compare_and_swap` install a V2 candidate carrying a FORGED provenance entry —
generation 2 with a fabricated `transition.fingerprint` and a `managed_identity`
naming something other than the real proof, which stayed untouched at the control
identity — and the forged manifest reloaded cleanly. That path was unreachable
before the relaxation. The re-key's stated justification, that
`validate_v2_session_provenance` binds a candidate's fingerprint to the durable
proof digest, was FALSE: that function only compares the candidate against its own
provenance entry, with no reference to any durable proof record.

So B4 is OPEN BY CHOICE. Relaxing this gate without a proof binding trades a
liveness wedge for a forgery hole, which is strictly worse. Seed
`audio-graph-68a1` owns the missing binding as B4's prerequisite; do not re-attempt
the re-key before it lands. The wedge is pinned deliberately by
`committed_v2_head_refuses_later_generations_until_proof_binding_exists`, which
asserts the REFUSAL and records why; when the binding exists, invert that test
rather than deleting it.

Strict Clippy (exit 0), `--lib persistence` (236 passed, 0 failed), `cargo fmt
--all -- --check`, and `git diff --check` are green on this state, and the
footprint stayed at exactly 5 files. Real output and the full revert rationale are
in the 3b53 report's correction round 4 section, which is authoritative for this
question — this file and that report must agree, and did not before `58b1e3a` was
corrected.

Update after round 5 (fixing `9e8b9a9`): review confirmed the production
behaviour, the gates, and the B4 revert clean, and blocked on the round-4 anchor
guarantee being false for one anchor in its own set — the round-4 comment
deletion had shifted the fingerprint comparison three lines up. That anchor, the
B4 gate comment's copy of it, the `Durability` rustdoc's return-site anchor, and
the rustdoc pointer that routed B3's install-stage residual to the round-3
review ticket instead of `audio-graph-3cf2` — which owns B3's remaining
substrate residuals, the orphaned-intent abandon path and the install-stage
misclassification — were all fixed intra-line, so no source line moved. Hand
verification of anchors is retired: the report's round-5 section carries a
machine-checked table enumerating every line anchor in its round-3 through
round-5 sections, and the check fails on any anchor it does not enumerate. A
mechanical sweep during that fix caught three more stale round-3 anchors that
five review rounds had not. Line anchors in the round-3 paragraph above date
from that round's commit and are not maintained; the report's table is current.
On this state: strict Clippy exit 0, `--lib persistence` serial 236 passed and
0 failed, `cargo fmt --all -- --check` clean, `git diff --check` clean, and the
footprint stayed at exactly 5 files.

Immutable continuation checklist:

- run the exact next command below;
- complete every broad gate above without widening the footprint;
- adversarially inspect incomplete manifest-intent recovery, special-entry
  authentication, and the new canonical surface;
- decide the round-4 residue: re-running `advance_session_semantics_v1_to_v2` on
  an already-advanced head under a different idempotency id is still refused
  `ImmutableExactConflict`, which is the right outcome reported in durability
  rather than manifest vocabulary;
- decide the two round-3 residues: there is still no reachable path to abandon
  an orphaned intent temporary (resume is public, removal exists nowhere in the
  durability substrate), and a `manifest_fault` of `CreateNew` still returns
  `Durability(IoFailedBeforeMutation)` from the install after both the intent
  temporary and the proof are durable, which is the same misclassification one
  step later and is uncovered by the install-cut table;
- update the report with real broad outputs; and
- obtain fresh Standards and Spec reviews. Both are pending.

Two open items are now named in the 3b53 report and are not fixed at this
checkpoint: addressed per-Session control identities are reserved nowhere, whose
fix seam is the out-of-footprint `canonical_log.rs`; and a same-length
zero-filled proof residue would be an unreclaimable `ImmutableExactConflict` if it
is reachable, which is a PLAUSIBLE code-shape argument with no fixture. The 3b53
plan's round-2 wording that denies a prefix-length heuristic still overstates the
replacement and was left unedited here.

Exact next command. The strict-Clippy command previously recorded here has run and
is green, so the next outstanding broad gate is the full serialized cloud library
suite:

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud -- --test-threads=1
```

This is a no-SHIP, no-integration, runtime-dark handoff. Do not merge, push,
dispatch, update Seeds, or enable any consumer from this checkpoint.

## Update after integration (2026-08-18, integration branch)

The paragraph above is the checkpoint-era constraint and is superseded: round-6
fresh reviews of candidate `8dd3d35` returned SHIP on both required axes plus
the B4-revert audit (zero blocking findings, residuals recorded honestly), and
the maintainer's standing landing instruction applied. The candidate was
squash-integrated onto `integration/session-memory-wave-20260814` as `f27e322`,
re-gated there (strict Clippy exit 0, serial persistence 236 passed and 0
failed, fmt and diff checks clean), and the Seeds tracker closed
`audio-graph-3b53` and `audio-graph-7e18` with reasons. `bun run verify:fast`
now completes (exit 0) using the repo-pinned pipe-safe Seeds CLI — the B6
environment gap is closed — and Betterleaks over the footprint reported no
leaks.

OS probes, previously "not cleared", now have first real evidence
(os-native-test-binaries runs 32174122813 macOS and 32174795688 Windows, both
building integration tip `3ab13fe`):

- macOS 15: full compile, the binary starts and lists its tests, and the
  keychain smoke `os_keychain_smoke_save_import_delete_tombstone_and_redaction`
  passed on the runner.
- Windows Server 2025 (MSVC): full compile and binary start — after fixing the
  dispatch workflow's Git Bash/MSVC linker collision on master (`10c4f44`).
  Rotation probes on the runner:
  `rotate_session_writer_open_failure_preserves_current_session` passed;
  `rotate_session_respawns_transcript_writer_to_new_file` failed because
  HomeGuard's HOME/USERPROFILE override cannot redirect Windows known-folder
  resolution — test infrastructure only, production resolution is correct
  (seed `audio-graph-0641`).
- Real Windows 11 NTFS host (the CI-built binary executed via WSL interop,
  cwd `C:\Temp`): persistence suite 163 passed, 6 failed. All six are test
  cfg-gating defects against a substrate that is Linux/macOS-only by design
  (`namespace_supported_for` excludes Windows and `filesystem_identity`
  carries no identity under `cfg(not(unix))`): four real-filesystem manifest
  fixtures panic `Coordination(IdentityUnavailable)`, and the two
  crash-harness tests additionally require fsutil elevation. Production
  Windows correctly reports `NamespaceDurabilityUnsupported` at the platform
  gate. Seed `audio-graph-a58b` owns the test gating.

Line anchors and gate outputs in earlier sections describe their own rounds'
commits; this section is the current state record.

## Continuation 2026-08-18 — audio-graph-3cf2 closes the install-stage residual

Seed `audio-graph-3cf2` landed on `work/audio-graph-3cf2-refused-install-survivors`
(base `27be43a`) and changes two statements this document makes above.

- The install-stage misclassification that the round-4 section left open is
  fixed. `commit_prepared_compare_and_swap` no longer widens an install refusal
  into `ManifestCasRejection::Durability` when this transaction had already made
  its intent temporary and immutable proof durable; that case is
  `ManifestCasRejection::ManifestInstallRefusedAfterProofAndIntentDurable` and
  carries the intent temporary's recovery key.
- The wedge described in the round-3 section — a surviving intent temporary
  refusing every later `compare_and_swap` with `SnapshotTempAlreadyExists` — now
  has a second escape besides a byte-exact replay:
  `ManifestWriteTransaction::abandon_staged_transition` durably unlinks the
  Session's own temporary. Any earlier sentence in this document describing the
  replay as the only way out is a record of its own round, not current state.
- Still open, unchanged, and not owned by `audio-graph-3cf2`: the B4 proof-binding
  gate in `prepare_compare_and_swap` (`audio-graph-68a1`), the Windows test
  cfg-gating defects (`audio-graph-a58b`), the rotation HomeGuard gap
  (`audio-graph-0641`), and consumer activation of the dormant kernel. Abandon
  does not remove or re-key the immutable proof, so a durable v2 proof still
  refuses a different transition id with `ImmutableExactConflict`.

Every line anchor in the sections above predates this wave and its diff moved
lines in both persistence sources; prefer the symbol names. This section cites no
line numbers.

## Update after audio-graph-68a1 (2026-08-18, `work/audio-graph-68a1-proof-binding`)

<!-- audio-graph-68a1 anchors: begin -->

This section supersedes the round-4 block above, which is retained as history and
must no longer be read as current state. Specifically, these round-4 statements
are now FALSE:

- "So B4 is OPEN BY CHOICE." B4 is CLOSED. An addressed Session with a committed
  V2 head records later generations.
- "The transition-proof gate in `prepare_compare_and_swap` is still keyed on the
  candidate's floor rather than on whether the call performs the advance." It is
  now keyed on whether this call performs the transition, in
  `refuse_unproven_v2_candidate`
  (`src-tauri/src/persistence/session_artifact_manifest.rs:1530`).
- "do not re-attempt the re-key before it lands" and "The wedge is pinned
  deliberately by `committed_v2_head_refuses_later_generations_until_proof_binding_exists`".
  The binding landed first, in the same commit as the re-key, and the pinning test
  was INVERTED, not deleted, into
  `committed_v2_head_records_later_generations_only_against_the_durable_proof`
  (`src-tauri/src/persistence/session_artifact_manifest.rs:3712`).

What replaced the wedge: `bind_v2_provenance_to_durable_proof`
(`src-tauri/src/persistence/session_artifact_manifest.rs:2164`) reads the durable
v1-to-v2 proof record at the DERIVED control provenance identity, under the
exclusive guard, before anything is staged, and refuses a V2 candidate whose
`SessionProvenanceEvents` entry is not exactly that record — absent, non-regular,
over-bound, unreadable, non-canonical, foreign Session, wrong identity, or wrong
digest/length, each its own classified refusal. `proof_owned` calls skip it,
because the advance runs preparation before the record exists and binds by
full-byte equality afterwards.

The round-4 four-closed-paths list is now conditional, and reads as follows on a
committed V2 head:

1. a V1 candidate is still `SessionSemanticsFloorRegression`;
2. a V2 candidate without owned proof bytes is `TransitionProofRequired` ONLY
   while the head is absent or below V2 — against an already-V2 head it is
   admitted if, and only if, its provenance entry binds to the durable record,
   and otherwise refused `V2ProvenanceProofBinding(..)`;
3. a fresh-id `advance_session_semantics_v1_to_v2` is still
   `Durability(ImmutableExactConflict)` on the existing proof, unchanged, because
   the binding deliberately does not run on the proof-owning path;
4. the same idempotency id with changed artifacts is `TransitionConflict` —
   *newly* reachable, not "still": before the re-key that path was refused
   `TransitionProofRequired` like every other V2 candidate, so round 4's
   four-path list was internally inconsistent in claiming both. The branch's RED
   transcript records the pre-change behaviour.

Quarantine recovery on an advanced Session is still fully closed for every
candidate shape, and now has executable evidence in
`quarantine_recovery_remains_closed_on_a_v2_session`
(`src-tauri/src/persistence/session_artifact_manifest.rs:3830`); the
`SessionArtifactManifestV1::candidate` V1 hardcode remains the named cause and is
deliberately not fixed.

Gates on this state: strict Clippy exit 0, serial `--lib persistence` 240 passed
and 0 failed (236 at base `27be43a`; net plus four tests), `cargo fmt --all --
--check` clean, `git diff --check` clean. Every line anchor in this section is
machine-checked by
`docs/agentic-runs/2026-08-18-audio-graph-68a1/check-anchors.py`, which fails on
any anchor it does not enumerate. Full detail, verbatim RED and gate output, and
ten recorded residuals with owners are in
`docs/agentic-runs/2026-08-18-audio-graph-68a1/report.md`. The install-stage
`Durability` classification and the missing abandon primitive remain
audio-graph-3cf2's, untouched here; this branch rebases onto that result.

<!-- audio-graph-68a1 anchors: end -->
