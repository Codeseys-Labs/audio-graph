# Commit state: 8849 settings authority transaction

Date: 2026-08-02

## Work item and checkout

- Seed: `audio-graph-8849`
- Acceptance: bind one validated non-secret settings draft to the service-generated
  credential operation and the atomically loaded native authority; pass one closed
  transaction to settings persistence and the same closed identity through verify,
  restore, clear, and restart recovery.
- Worktree:
  `/home/codeseys/DevBox/audio-graph/.worktrees/8849-settings-authority-transaction`
- Branch: `work/audio-graph-8849-settings-authority-transaction`
- Accepted base: `8be073fecd8db650548c3d28734ebdebae26e379`
- Base subject: `feat(credentials): seal native interaction gate (audio-graph-0350)`
- Starting status: clean.
- Dirty-tree caveat: only the five owned credential Rust files and this unique
  commit-state document are changed. No Seeds, Cargo, generated contract, IPC,
  frontend, platform, runtime, CI, or native-interaction files are modified.

## Implemented boundary

- `CredentialMutationSession::load_journal` now returns one
  `LoadedAuthorityJournal`, pairing the validated journal with a typed opaque
  `CredentialAuthorityInstanceId` from the same authority-pair read.
- Raw authority UUID text remains private to `authority_journal` wire envelopes.
  The core authority identity is cloneable and comparable but has no serde,
  display, string, path, target, provider, or secret projection.
- `PrepareCredentialActivation` owns a `ValidatedNonSecretSettingsDraft`.
  After generating the operation and credential revision and loading authority,
  the service freezes all activation identity fields plus that exact draft into
  one `SettingsActivationTransaction` owned by the prepared state.
- `CredentialSettingsActivationPort::persist_pending_settings` receives the full
  transaction. Verify, committed verification, restore, and clear receive its
  identity projection rather than loose scalars or ambient lookup state.
- Restart recovery builds a separate identity-only recovery context by combining
  durable `PendingSettingsActivation` with the authority returned by the same
  authoritative load. It never fabricates or defaults a settings draft.
- Fake settings persistence models exact transaction replay, closed-identity
  verification, revision-fenced restore/clear, and concurrent CAS behavior.
- Manual diagnostics for authority, draft, loaded pair, identity, transaction,
  prepared state, and restart state are content-free. Renderer-safe status and
  journal serialization contain neither transaction fields nor canaries.

## Immutable-review correction

The first candidate (`90bd0e62dbd5dbc72a5903c0b7c9fe0c4b72546b`) was
preserved at `review-blocked/audio-graph-8849-service-replay-90bd0e6` after the
immutable review artifact
`/tmp/audio-graph-artifacts/2026-08-02/audio-graph-8849-review.md`
(`sha256:1938d23ba219f2e3e9402743bb703dc4a06a77d640f7e4cd690f9f31e1d55d24`)
identified an exact-prepared-clone replay race.

- Normal service stage movement is now an expected-stage compare-and-set. The
  complete normal vocabulary is only `Staged -> SettingsPending` and
  `SettingsPending -> CredentialPending`; same-stage, backward, cleanup, and
  recovery transitions are ineligible.
- Stage-transition failures distinguish an ineligible/stale caller from an
  operational failure. A stale, backward, or advanced caller returns before
  settings persistence, rollback, staging deletion, recovery marking, or event
  effects.
- Definite rollback first proves the durable stage is `SettingsPending` or
  `CredentialPending` and proves the active record plus journal still name the
  expected credential generation. It then persists `RecoveryRequired` as the
  rollback ownership claim.
- Worker admission and the credential mutation session remain held from that
  eligibility check and ownership claim through revision-fenced settings
  restore, staging deletion, and the final journal abort. A later winner cannot
  publish between rollback verification and restore.
- Recovery marking is monotonic: `CleanupPending` and `RecoveryRequired` are
  preserved without another journal write, so a stale error path cannot regress
  or replace the winner's durable recovery authority.

## Second immutable-review correction

The corrected candidate (`857ca6eb88aee1bbf4042cd57a219899539c13f9`, tree
`92c16dbbe349fd99e53496f755c2b6850f02367d`) was preserved at
`review-blocked/audio-graph-8849-rollback-stall-857ca6e` after the second
immutable review artifact
`/tmp/audio-graph-artifacts/2026-08-02/audio-graph-8849-second-review.md`
(`sha256:fd66c9dbfb3e7a643489b94863c88c731eb450cf39356b2c03358a6705b759cc`)
identified one final rollback cut that failed to latch mutation admission.

- The rollback already held one worker permit and mutation session while it
  durably claimed `RecoveryRequired`, restored the settings backup, deleted
  credential staging, and attempted the final journal abort.
- A `StalledWorker` from that final journal commit was collapsed directly to
  public `RecoveryRequired`. Because the underlying failure never passed
  through the service failure mapper, permit drop reset the cached worker from
  `Busy` to `Idle` and admitted another mutation after an indeterminate native
  operation.
- The final commit failure now passes through `map_store_failure` so
  `StalledWorker` latches the cached worker and mutation admission closed. Its
  mapped public error is intentionally discarded and the operation still
  returns `RecoveryRequired`, because the rollback claim, settings restore, and
  staging deletion are already partially applied.
- The correction does not change rollback eligibility, the fused ownership
  critical section, expected-stage transitions, or monotonic recovery marking
  established by the first correction.

## TDD evidence

The first accepted test was written before the production seam existed:

```text
credentials::service::tests::prepare_freezes_generated_operation_authority_and_exact_same_revision_draft_into_one_transaction
```

Initial compile result: exit 101. The compiler reported the missing
`SettingsActivationIdentity`, `SettingsActivationTransaction`,
`CredentialAuthorityInstanceId`, and `ValidatedNonSecretSettingsDraft` types;
the missing `settings_draft` request field and prepared transaction accessor;
the loose scalar settings-port signature; and the absence of an authority value
from the fake mutation session. This was the expected RED for the previously
inexpressible contract.

Final exact GREEN output:

```text
running 1 test
test credentials::service::tests::prepare_freezes_generated_operation_authority_and_exact_same_revision_draft_into_one_transaction ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1658 filtered out
```

Additional focused tests cover a real two-thread same-revision draft race,
exact/stale replay, foreign-authority rejection before settings or entry I/O,
identity-only restart reconstruction across prepare cut points, stable opaque
identity across native sessions, and draft/authority/path/provider canaries.

The immutable-review correction also followed red-before-green:

- `stale_clone_rollback_race_preserves_the_winner_cleanup_authority` ran twice
  against the blocked candidate and failed deterministically with final settings
  revision `7` instead of the winner's revision `8` (exit 101 both runs).
- `backward_activation_stage_transition_is_rejected_without_effects` failed
  because the old transition returned `Ok(())` and durably moved backward.
- `rollback_after_cleanup_pending_preserves_winner_state_without_effects`
  failed because the old rollback returned `Ok(())`, restored old settings,
  deleted staging, and cleared the advanced pending authority.

The final tests make those failures mutation-sensitive: they require one active
write, proposed settings plus marker, retained staging, retained
`CleanupPending`, zero event/epoch publication, no restore/delete effect, an
exact settings-call trace, and exactly the two allowed normal transition edges.

The second correction also followed red-before-green. The durable colocated
regression `rollback_final_stalled_journal_cut_latches_worker_status` was added
before the production edit and ran twice against `857ca6e`. Both runs exited
101 with the same behavioral failure:

```text
assertion `left == right` failed
  left: Idle
 right: Stalled

test result: FAILED. 0 passed; 1 failed; 0 ignored; 1663 filtered out
```

The test injects `StalledWorker` at the third commit after activation prepare:
the final rollback journal commit after the durable recovery claim. It proves
the public result remains `RecoveryRequired`, settings are restored, staging is
deleted, the authoritative journal retains `RecoveryRequired`, the cached
worker is `Stalled`, and a competing mutation returns `StalledWorker` without
adding a store call.

Final exact GREEN output:

```text
running 1 test
test credentials::service::tests::rollback_final_stalled_journal_cut_latches_worker_status ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1663 filtered out
```

## Verification

All correction Cargo commands used Rust 1.95.0 and
`CARGO_TARGET_DIR=/tmp/audio-graph-target-8849-correction`.

- Focused domain tests: `16 passed; 0 failed`.
- Focused authority-journal tests: `11 passed; 0 failed`.
- Focused native-keyring tests: `25 passed; 0 failed`.
- Focused service tests: `58 passed; 0 failed`.
- Full credential slice:

  ```text
  test result: ok. 239 passed; 0 failed; 1 ignored; 0 measured; 1423 filtered out
  ```

- Credential logging canary:

  ```text
  test logging::tests::credential_provider_debug_and_trace_never_reach_the_file_sink ... ok
  test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1658 filtered out
  ```

- Locked cloud library check:

  ```text
  Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 49s
  ```

- Strict all-target Clippy with `-D warnings`:

  ```text
  Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 24s
  ```

- `cargo +1.95.0 fmt --all --manifest-path src-tauri/Cargo.toml -- --check`:
  exit 0, no output.
- Locked no-dependency metadata:

  ```text
  workspace_root=/home/codeseys/DevBox/audio-graph/.worktrees/8849-settings-authority-transaction/src-tauri
  packages=3
  ```

- Credential contract check:

  ```text
  credential contract is current: /home/codeseys/DevBox/audio-graph/.worktrees/8849-settings-authority-transaction/src/generated/credentialContract.ts
  ```

- `git diff --check`: exit 0, no output.

### Third-correction verification

All third-correction Cargo build and test commands used Rust 1.95.0 and
`CARGO_TARGET_DIR=/tmp/audio-graph-target-8849-third-correction`.

- Exact final-stall regression: `1 passed; 0 failed`.
- Exact stale-clone rollback race: `1 passed; 0 failed`.
- Credential service suite: `59 passed; 0 failed`.
- Full credential slice:

  ```text
  test result: ok. 240 passed; 0 failed; 1 ignored; 0 measured; 1423 filtered out
  ```

- Credential logging canary: `1 passed; 0 failed`.
- Locked cloud library check:

  ```text
  Finished `dev` profile [unoptimized + debuginfo] target(s) in 3m 02s
  ```

- Strict all-target cloud Clippy with `-D warnings`:

  ```text
  Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 23s
  ```

- `cargo +1.95.0 fmt --all --manifest-path src-tauri/Cargo.toml -- --check`:
  exit 0, no output.
- Locked metadata reported the exact worktree `src-tauri` workspace, exact
  isolated target, and three packages.
- Credential contract generation check reported the generated contract is
  current.
- `cargo +1.95.0 audit --file src-tauri/Cargo.lock --deny warnings` reproduced
  the unchanged dependency baseline: exit 1 with four vulnerabilities
  (`RUSTSEC-2023-0071`, `RUSTSEC-2026-0098`, `RUSTSEC-2026-0099`, and
  `RUSTSEC-2026-0104`) and 22 denied warnings. Neither Cargo manifest nor lockfile
  differs from the accepted base.

## Queue and integration notes

- No unrelated problem was fixed and no out-of-scope file was changed.
- No blocker or unresolved implementation question remains in this workstream.
- Both blocked candidates remain preserved only as immutable review evidence;
  the worktree branch contains one amended replacement candidate over the same
  accepted base.
- `audio-graph-c2be` can consume this seam only after immutable review and
  integrator fan-in of the candidate; this worker does not claim that downstream
  Seed is complete.
- Seeds were not edited or synced here because queue ownership and final Seed
  hygiene remain with the root conductor/integrator.
- No push, merge, PR, or workflow mutation was performed.

## Fourth correction: phase-specific stalled-worker observation

The third immutable review found a finite class of seven activation paths that
preserved the correct public `CommitUnknown` or `RecoveryRequired` result but
discarded the underlying `StalledWorker` before the admission state observed
it. The same class also affected ordinary replace/delete final-journal remaps
tracked by `audio-graph-7cc6`.

The correction introduces one content-free `observe_store_failure` seam.
Normal mappings and every phase-specific remap now invoke that seam before
choosing their public semantic. No failure content, provider name, target,
path, operation identifier, or credential material is retained or logged.

The deterministic matrix covers:

- activation staging readback;
- active activation final journal;
- cleanup settings verification;
- cleanup settings-marker clear;
- cleanup staging deletion;
- cleanup final journal;
- restart pending-settings verification;
- ordinary replace and delete final journals.

For each selected `StalledWorker` cut, the test preserves the intended public
error, requires cached worker state `Stalled`, and proves a subsequent mutation
or recovery attempt adds zero store calls.

The first exact matrix test was run twice before the production change. Both
runs exited 101 at the same assertion (`Idle` versus required `Stalled`). The
first run populated the stable lane target in 4m21s; the second reused it and
reached the same RED in 12s. After the production change, the exact matrix was
GREEN and the complete service suite passed 62 of 62 tests. All fourth-wave
commands reuse
`CARGO_TARGET_DIR=/tmp/audio-graph-target-8849-fourth-correction` and use
`--jobs 2` while other Cargo lanes are active; a fresh target is reserved for
the final immutable clean-room review.

Fourth-correction verification on that stable lane:

- exact six-cut activation matrix: `1 passed; 0 failed`;
- complete service suite: `62 passed; 0 failed`;
- complete credential slice: `243 passed; 0 failed; 1 ignored` (the existing
  OS-keychain smoke);
- credential logging canary: `1 passed; 0 failed`;
- locked cloud library/tests check: GREEN in 4m06s on its first check-profile
  population;
- strict all-target cloud Clippy with `-D warnings`: GREEN in 1m10s;
- formatting, locked no-dependency metadata, generated credential contract,
  and `git diff --check`: GREEN;
- production-source inventory contains no discarded `map_err(|_| ...)` or
  `.is_err()` store/settings remap; all eleven mapping/semantic-remap sites
  reach the single observer directly or through `map_store_failure`;
- Cargo manifest and lockfile are byte-identical to the accepted base;
- configured `cargo audit` reproduces the unchanged lock baseline: four
  documented advisories and 22 allowed warnings. This correction changes no
  dependency input.

## Fifth correction: completable post-permit stalls

The fourth immutable review blocked candidate `f7ac3bd` because three settings
phases run after the preceding `WorkerPermit` has dropped. A
`StalledWorker` from pending-settings persistence, live pending-settings
verification, or restarted pending-settings verification therefore changed an
`Idle` status to `Stalled` without retaining the authoritative operation or
credential-set identity. Admission correctly failed closed, but
`complete_stalled_operation` could never match that anonymous stall; only a
process restart could clear it.

The fifth correction adds an identity-aware, content-free stalled observation
for those post-permit phases. It stores only the already-public typed operation
and set identifiers, never a secret, draft, path, provider endpoint, or native
failure payload. An existing stalled identity is not overwritten. All
permit-owned paths continue using the original observer because their busy
status already contains the authoritative identity.

The activation failure matrix now includes pending-settings persistence and
live pending-settings verification in addition to its prior six live cuts.
The shared assertion requires every stall to retain a set and operation,
rejects a follow-up mutation with zero store I/O, proves an unrelated operation
cannot clear the gate, proves the exact operation can clear it, and verifies
the resulting status is `Idle`. Restarted verification separately proves that
the recovered operation identity is retained and completable. Ordinary
replace/delete final-journal cuts use the same strengthened assertion.

TDD and stable-lane evidence:

- Expanded exact activation matrix before the production correction: RED,
  `0 passed; 1 failed`, with cached `set_id` `None` instead of the expected
  Deepgram set; exit 101 after 29.48s.
- Expanded exact eight-cut activation matrix after correction: GREEN,
  `1 passed; 0 failed`; 18.83s compile plus a zero-duration test body.
- Activation and restart remap filter: `2 passed; 0 failed`.
- Ordinary replace/delete final-journal identity/completion test:
  `1 passed; 0 failed`.
- Complete service suite: `62 passed; 0 failed`.
- Complete credential slice: `243 passed; 0 failed; 1 ignored`.
- Credential logging canary: `1 passed; 0 failed`.
- Locked cloud library fast gate: GREEN in 58.76s using
  `-p audio-graph --lib --no-default-features --features cloud --jobs 2`.
- Strict all-target cloud Clippy with `-D warnings`: GREEN in 16.04s.
- Formatting, locked metadata (three packages), generated credential contract,
  `git diff --check`, and Cargo-input identity against the accepted base:
  GREEN.

All compile-bearing correction commands reused
`/tmp/audio-graph-target-8849-fourth-correction`; no additional clean target was
created. The fourth reviewer independently kept its review pinned to the
immutable `f7ac3bd` archive and recorded the concurrent fifth-correction work as
external drift. Its official BLOCK artifact is
`/tmp/audio-graph-artifacts/2026-08-02/audio-graph-8849-fourth-review.md`
(SHA-256
`5a503e340e3740994c160050eb92809ae3156b135e88d17521b72cb2bb101abc`).

## Sixth correction: retain admission across pending-settings calls

The fifth immutable review blocked candidate `718433c` because assigning an
identity after a post-permit settings call is not equivalent to retaining the
serialized worker lease. While operation A was inside pending-settings
persistence or verification, an explicitly allowed unrelated-set resolution B
could acquire the worker and install `Busy(B)`. If either or both calls then
stalled, the single worker-status slot could lose one indeterminate operation's
identity and later reopen admission too early.

The sixth correction removes the identity-overwrite seam. Live persistence and
verification now execute under one `WorkerPermit` for the prepared operation,
and restarted pending-settings verification reacquires a permit for the
durable operation before entering the settings port. `StalledWorker` is
observed while that permit is still live, so its existing busy operation and
set become the completable stalled identity. The permit is released before any
rollback or recovery-journal I/O, avoiding reentrant acquisition. No new
identity, secret, draft, path, provider, or native payload is cached or logged.

Two deterministic test ports inspect both the cached worker identity and the
underlying serial mutex from inside the settings boundary. Before the
production change, the live and restarted tests each failed with `Idle` versus
required `Busy`. After the change, both prove the authoritative operation/set
is `Busy` and the serial lock is unavailable for a competing operation during
the call.

Sixth-correction verification reused the established stable lane
`/tmp/audio-graph-target-8849-fourth-correction` with two Cargo jobs:

- live worker-permit test before production: RED, `0 passed; 1 failed`, cached
  `Idle` versus `Busy`;
- restarted worker-permit test before production: the same deterministic RED;
- both worker-permit tests after production: `2 passed; 0 failed`;
- complete service suite: `64 passed; 0 failed`;
- complete credential slice: `245 passed; 0 failed; 1 ignored`;
- credential logging canary: `1 passed; 0 failed`;
- locked cloud library check: GREEN;
- strict all-target cloud Clippy with `-D warnings`: GREEN;
- formatting, locked no-dependency metadata, generated credential contract,
  `git diff --check`, and Cargo-input identity against the accepted base:
  GREEN.

The fifth review artifact is
`/tmp/audio-graph-artifacts/2026-08-02/audio-graph-8849-fifth-review.md`
(SHA-256
`44b17fa10bdaa488d166a38bb60522ddd188a8aecf20a03674d32a7044b5b14d`).
The corrected tree still owns only the original five Rust files and this
commit-state document; this correction itself changes only `service.rs` and
this section.
