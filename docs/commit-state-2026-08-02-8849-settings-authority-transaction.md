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

## Verification

All Cargo commands used Rust 1.95.0 and
`CARGO_TARGET_DIR=/tmp/audio-graph-target-8849`.

- Focused domain tests: `16 passed; 0 failed`.
- Focused authority-journal tests: `11 passed; 0 failed`.
- Focused native-keyring tests: `25 passed; 0 failed`.
- Focused service tests: `54 passed; 0 failed`.
- Full credential slice:

  ```text
  test result: ok. 235 passed; 0 failed; 1 ignored; 0 measured; 1423 filtered out
  ```

- Credential logging canary:

  ```text
  test logging::tests::credential_provider_debug_and_trace_never_reach_the_file_sink ... ok
  test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1658 filtered out
  ```

- Locked cloud library check:

  ```text
  Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 29s
  ```

- Strict all-target Clippy with `-D warnings`:

  ```text
  Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 22s
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

## Queue and integration notes

- No unrelated problem was fixed and no out-of-scope file was changed.
- No blocker or unresolved implementation question remains in this workstream.
- `audio-graph-c2be` can consume this seam only after immutable review and
  integrator fan-in of the candidate; this worker does not claim that downstream
  Seed is complete.
- Seeds were not edited or synced here because queue ownership and final Seed
  hygiene remain with the root conductor/integrator.
- No push, merge, PR, or workflow mutation was performed.
