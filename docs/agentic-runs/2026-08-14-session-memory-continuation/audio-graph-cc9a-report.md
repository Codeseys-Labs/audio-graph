# audio-graph-cc9a implementation report

## Assignment and custody

- Seed: `audio-graph-cc9a` — authorize production namespace qualification for
  Session semantics first-create.
- Parent: `audio-graph-7e81`; the dormant semantics kernel remains runtime-dark.
- Exact base: `d31b5f9695164452a6c353b8230097fd8f661119`.
- Worktree:
  `/home/codeseys/DevBox/audio-graph/.worktrees/cc9a-production-qualification-wave7c`.
- Branch: `work/audio-graph-cc9a-production-qualification-wave7c`.
- Initial state: clean at the exact base.
- Plan commit: `e682f0713bc6638c23ad71937705abaf66a5491e`.
- Implementation commit: `0edc8a08c98a2dda8f7954cd9fd133deb42ebdeb`.
- Report artifact:
  `docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-cc9a-report.md`.

## Outcome

`CanonicalFilesystemQualification::for_existing_managed_root` now returns one
opaque production qualification and its paired `CanonicalDurability` factory.
It accepts only an existing exact root and binds its canonical path, native
volume/object identity, longest matching live sysinfo mount, stable filesystem
class, and a private in-process token. The production constructor has no
caller-provided policy boolean or filesystem string.

The policy admits only writable, non-removable ext4 on Linux and APFS on
macOS. Windows and Other return typed namespace-unsupported errors before root
or coordination mutation. No matching mount, unknown/local-unclassified,
remote, FUSE, tmpfs, read-only, removable, and unavailable-identity cases
return content-free typed refusals. Mount volume metadata must match the root
volume. Raw filesystem strings, mount sources, roots, object ids, and user
bytes do not cross the error or `Debug` boundary.

Production writer and reader guard acquisition require the paired opaque
qualification. Before opening or creating the coordination entry they reload
the exact root identity, take a fresh sysinfo inventory, reselect the longest
mount, revalidate its volume and policy, and compare the private token. A
foreign token, cross-root use, ancestor/nested mismatch, and moved/recreated
root refuse without a coordination-file mutation. Existing cfg(test)
qualification and the synthetic algorithm environment remain separate; a
production token cannot be consumed by an unpaired `CanonicalDurability`.

`SessionArtifactManifestStore::qualified_existing_root` consumes the
production pair. Qualified initial CAS reports `Accepted` only with
`InitialSnapshotInstall + FileAndParentNamespace`; replacement reports
`SnapshotReplacement + FileAndParentNamespace` and retains the exact open head
used for validation. A byte-identical foreign replacement object is refused as
`IdentityChanged` before temporary-file creation. The unqualified constructor
now refuses `begin_write` with the content-free typed
`NamespaceQualificationRequired` error before guard acquisition or namespace
mutation. Strict non-mutating load through an unqualified store remains
available.

No unsafe code, fifth stream, b887 runtime call site, Session writer/consumer,
workflow, dependency/lockfile, generated contract, frontend, canonical-log
production code, or crash-harness change was added. Correction round 1 changes
only one canonical-log test assertion outside the manifest module.

## TDD evidence

### Slice 1: live constructor and closed filesystem policy

RED command:

```text
cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml \
  --lib --no-default-features --features cloud canonical_durability -- --nocapture
```

RED result: exit 101 with 36 expected missing-seam compile errors, including
`E0599` for absent
`CanonicalFilesystemQualification::for_existing_managed_root`, `E0425` for
absent `assess_filesystem_policy`, and `E0422`/`E0433` for the absent stable
filesystem class, observation, qualified-mount, and qualification-error types.

GREEN result after the minimum production policy/pair implementation: 44
passed, 0 failed. The final focused rerun after all slices passed 46, 0 failed,
with the live test explicitly printing:

```text
live Linux ext4 qualification admitted
```

The deterministic inventory test covers longest-mount selection and the
Linux/macOS allowlist plus Windows, Other, no-match, remote, FUSE, tmpfs,
read-only, removable, and wrong-platform refusal. The test inventory exercises
policy only and cannot create a production token or qualification authority.

### Slice 2: pre-coordination binding validation

RED command targeted
`qualified_guard_refuses_foreign_or_changed_binding_before_coordination_mutation`.
RED result: exit 101 with 12 expected `E0599` errors for the absent
`try_lock_exclusive_qualified` method and absent
`CanonicalCoordinationError::QualificationBindingMismatch` variant. A test
assertion initially used `expect_err`, which required `Debug` on the successful
guard type; the assertion was corrected to pattern-match the error without
weakening or exposing guard internals.

GREEN result: 1 passed, 0 failed. Cross-root, foreign-token, ancestor-to-nested,
nested-to-ancestor, and moved/recreated-root attempts all returned the exact
binding mismatch before a coordination entry appeared. The exact pair acquired
the guard normally.

### Slice 3: production manifest CAS

RED command targeted
`production_qualified_existing_root_initial_cas_has_parent_barrier`. RED
result: exit 101; `E0599` named the absent
`SessionArtifactManifestStore::qualified_existing_root` constructor. Two test
imports for the exact receipt enums were also absent and corrected.

Final GREEN command targeted both `production_qualified_` tests. Result: 2
passed, 0 failed. It proves the exact initial and replacement receipts and the
open-head identity refusal. The complete manifest focused rerun passed 25, 0
failed.

### Regression seam

The focused `session_semantics` suite passed 11, 0 failed. Accepted or exact
already-completed manifest evidence remains the only logical floor admission;
the cc9a production store is not called by the dormant b887 kernel.

## Final gates and real results

### Focused Rust

```text
cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml \
  --lib --no-default-features --features cloud canonical_durability -- --nocapture
```

Result: 46 passed, 0 failed, 1,657 filtered out; live Linux ext4 admitted.

```text
cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml \
  --lib --no-default-features --features cloud session_artifact_manifest -- --nocapture
```

Result: 25 passed, 0 failed, 1,678 filtered out.

```text
cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml \
  --lib --no-default-features --features cloud session_semantics -- --nocapture
```

Result: 11 passed, 0 failed, 1,692 filtered out.

### Locked check, serialized library, Clippy, formatting, and typecheck

```text
cargo +1.95.0 check --locked --manifest-path src-tauri/Cargo.toml \
  --lib --no-default-features --features cloud
```

Result: pass; dev profile completed in 1m 47s.

```text
cargo +1.95.0 test --quiet --locked --manifest-path src-tauri/Cargo.toml \
  --lib --no-default-features --features cloud -- --test-threads=1
```

Result: pass; 1,695 passed, 0 failed, 8 ignored, 57.31s. PipeWire and
ALSA emitted the expected host-without-audio-device diagnostics without a test
failure.

```text
cargo +1.95.0 clippy --locked --manifest-path src-tauri/Cargo.toml \
  --lib --no-default-features --features cloud -- -D warnings
cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml -- --check
bun run typecheck
```

Result: all pass. Clippy emitted no warning, rustfmt had no diff, and TypeScript
emitted no diagnostic.

### Repository gates and hygiene

```text
SEEDS_CLI_ROOT=/home/codeseys/DevBox/audio-graph/.worktrees/cc9a-production-qualification-wave7c/node_modules/@os-eco/seeds-cli \
  bun run verify:fast
bun run verify:contracts
```

Result: pass. `verify:fast` checked Biome over 174 files, typecheck, all five
generated contracts, the direct LABSN action contract and 18 mutations, Seeds
JSON output (`ready` 50, `blocked` 94, `list` 50), docs/Seeds secret hygiene
with 0 findings, and diff hygiene. The explicit contract rerun confirmed the
audio-source, provider-registry, session-data-movement,
endpoint-credential-routing, and speech-span artifacts are current.

```text
betterleaks dir --no-banner --redact \
  src-tauri/src/persistence/canonical_durability.rs \
  src-tauri/src/persistence/session_artifact_manifest.rs \
  docs/commit-state-2026-08-16-session-memory-wave7c.md \
  docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-cc9a-plan.md
```

Pre-report result: pass; approximately 296,519 bytes scanned, no leaks. The
post-report final hygiene rerun is recorded by the handoff result.

For the initial candidate, `git diff --check` passed and the exact base
footprint before this report was the
two owned Rust modules plus the two owned planning documents. The forbidden
diff over Seeds, workflows, package/dependency/lockfiles, generated contracts,
frontend, `canonical_log.rs`, crash harness, commands, and Session runtime was
empty. A repository search found no production call site for the new
constructor or qualified guard methods outside the two owned persistence
modules; b887 remains runtime-dark.

## Correction round 1: unqualified preflight refusal

### Review disposition

The stable-snapshot Standards and Spec reviewers both returned **BLOCK** on
the same P1 issue: public
`SessionArtifactManifestStore::new(root).begin_write()` created
`.audio-graph-canonical.lock` before the later unqualified CAS refusal. The
finding was accepted. An unqualified store cannot produce namespace
`Accepted`, so the narrow contract is now refusal at `begin_write` on every
platform rather than a post-lock Windows/Other distinction.

The production correction is confined to
`session_artifact_manifest.rs`: `begin_write` requires the existing
qualification before calling `try_lock_exclusive_qualified`. The new
`ManifestStoreError::NamespaceQualificationRequired` carries no path, bytes,
id, or filesystem content. Qualified production initial/replacement CAS and
strict unqualified load are unchanged.

The conductor authorized a test-only footprint exception for the two old
post-lock assertions in `session_semantics.rs` and `canonical_log.rs`. No
production code, recovery behavior, or module interface changed in either
file.

### Correction RED / GREEN evidence

Primary RED command targeted
`unqualified_begin_write_refuses_before_coordination_mutation`. Result: exit
101 with one `E0599` because
`ManifestStoreError::NamespaceQualificationRequired` did not exist.

Primary GREEN: 1 passed, 0 failed. The test snapshots an existing empty root,
attempts unqualified `begin_write`, receives the exact content-free typed
error, and proves the root remains entry-identical with no coordination,
temporary, or manifest entry.

Before the authorized fixture updates, the focused dependent REDs were:

- `session_semantics`: 10 passed, 1 failed. The old unqualified test panicked
  at `.begin_write().expect("transaction")` with
  `NamespaceQualificationRequired`.
- canonical-log recovery target: 0 passed, 1 failed because the old assertion
  expected a post-lock `NamespaceDurabilityUnsupported` rejection rather than
  the new manifest preflight error.

After changing only those test assertions, the dependent GREEN results were
`session_semantics` 11/11 and canonical-log recovery 1/1. Final focused results
after correction are manifest 26/26, canonical durability 46/46, and Session
semantics 11/11. The production-qualified initial and replacement tests remain
green.

### Correction gates and final footprint

Locked cloud check passed in 6.94s. Strict Clippy with `-D warnings` passed in
11.13s, and rustfmt `--check` passed with no diff. The serialized cloud library
suite passed 1,696 tests with 0 failures and 8 ignored in 59.48s. PipeWire and
ALSA emitted the expected no-audio-device diagnostics without a test failure.
Repository-authoritative `verify:fast` also passed: Biome checked 174 files,
typecheck and all five contracts were current, the direct LABSN action and 18
mutations passed, Seeds JSON parsed 50 ready / 94 blocked / 50 listed rows,
docs/Seeds secret hygiene found 0 findings, and diff hygiene passed. Betterleaks
scanned all seven final-footprint paths with no leaks found.

The correction changes four paths: production and tests in
`session_artifact_manifest.rs`, test-only assertions in
`session_semantics.rs` and `canonical_log.rs`, and this report. The final
branch footprint from the exact base is therefore the original five assigned
paths plus the two explicitly authorized test-only files. Seeds, workflows,
dependencies/lockfiles, frontend, generated contracts, runtime callers,
canonical durability, crash harness, and all other paths remain unchanged.

## Native evidence boundary

The local worktree is on `/dev/sdd`, writable ext4. The new code-specific live
test exercised the actual sysinfo inventory and production constructor and
admitted that root; this is native Linux/ext4 evidence for qualification,
guard acquisition, first CAS, replacement CAS, and substituted-head refusal.

Accepted prior protocol evidence remains relevant but is not reclassified as
new-constructor evidence: Linux/ext4 previously passed 42/42 durability and
11/11 crash tests; macOS/APFS passed 13/13 durability and 11/11 crash tests.
No native macOS run occurred in this worktree. Windows NTFS probing previously
ran, but the trusted-helper predicate stopped before install and no Windows
durability/crash test executed. This implementation keeps Windows typed
refusal and does not claim native Windows execution.

## Findings and open questions

- No implementation or local-Linux blocker remains.
- Code-specific native macOS/APFS and Windows refusal execution remain external
  evidence gaps for the conductor. This worker was explicitly forbidden from
  workflow mutation or dispatch, so neither platform was claimed locally.
- The conductor owns stable-snapshot review, Seed reconciliation/closure,
  integration, push/merge, and worktree cleanup.
