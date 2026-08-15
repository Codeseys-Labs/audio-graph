# audio-graph-c2e3 canonical durability report

Date: 2026-08-15

Seed: `audio-graph-c2e3`

Branch: `work/c2e3-canonical-durability-wave7b`

Exact base: `d2591059f70247c41f054880682560fc18528622`

Worktree:
`/home/codeseys/DevBox/audio-graph/.worktrees/c2e3-canonical-durability-wave7b`

## Outcome

Implemented a dormant, provider-neutral canonical durability module with one
small public interface for stable cooperative locks, durable append, and
qualified durable rename. No runtime caller was added.

The implementation:

- classifies first-create versus existing files from `create_new` and
  `AlreadyExists`, never from a `Path::exists` preflight;
- accepts an existing append only after buffered write, userspace flush, and
  file `sync_all`;
- accepts a qualified Linux first-create only after the file barrier and parent
  directory `sync_all`;
- leaves macOS namespace acceptance conditional on an externally proven APFS
  directory-sync qualification;
- refuses Windows first-create and rename before mutation as
  `NamespaceDurabilityUnsupported`;
- refuses an absent canonical parent as `ParentProvisioningRequired` rather
  than creating it;
- maps write, flush, file-sync, rename, and parent-sync uncertainty after a
  visible or possibly visible mutation to `DurabilityIndeterminate` with an
  opaque redacted recovery key; and
- exposes one stable shared/exclusive coordination-file lock while ensuring a
  missing strict-reader lock acquisition creates neither the file nor parent.

The module is declared from `persistence::canonical_durability`. Existing
`canonical_log` and strict-reader implementations are unchanged, so this work
does not activate a writer or make strict reads mutate.

## Public interface and deep-module seam

`CanonicalDurability` is the only behavior-bearing public module type. Its
interface consists of:

- `try_lock_exclusive`: writer-side stable coordination-file acquisition;
- `try_lock_shared`: open-only strict-reader acquisition;
- `append`: existing append or atomically classified first-create with all
  applicable barriers internal to the module; and
- `rename`: unique-destination publication with source file sync, rename, and
  each changed parent barrier internal to the module.

Callers receive typed `Accepted`, `Rejected`, or
`DurabilityIndeterminate` outcomes. Paths, payload bytes, OS error messages,
and opaque recovery bytes are absent from `Debug`. The interface accepts a
filesystem qualification token rather than embedding platform probes or
claiming that the local test filesystem is automatically qualified.

## Invariants

1. `Accepted(ExistingAppend)` follows write, flush, and file `sync_all`.
2. `Accepted(FirstCreate)` follows `create_new`, write, flush, file `sync_all`,
   and qualified parent-directory `sync_all`.
3. `Accepted(Rename)` follows source file `sync_all`, rename, and every changed
   parent-directory `sync_all`.
4. A first-created name is never inferred from a preflight existence check.
5. A canonical parent directory is never provisioned by this module.
6. Windows namespace mutation is refused before mutation regardless of a
   supplied qualification token.
7. macOS namespace mutation is accepted only with the explicit
   `MacOsApfsDirectorySyncProven` token and successful runtime barriers.
8. Any failed visible or possibly visible write/flush/sync/rename barrier is
   indeterminate and carries the caller-supplied recovery identity.
9. The recovery identity has no diagnostic byte accessor and formats only as
   `[REDACTED]`.
10. The coordination file is never renamed or removed by the module. Shared
    acquisition opens existing state only. Locks remain cooperative/advisory;
    uncooperative pathname mutation is outside this contract.

## TDD evidence

The agreed test seam was the public `persistence::canonical_durability` module
interface.

### RED 1: append classification and barriers

Command:

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud public_append_classifies_existing_and_first_created_files_atomically -- --nocapture --test-threads=1
```

Real result: exit 101. The public test failed to compile with 12 unresolved
interface errors, beginning with:

```text
error[E0433]: cannot find type `CanonicalDurability` in this scope
error[E0422]: cannot find struct, variant or union type `CanonicalDurabilityReceipt` in this scope
error[E0433]: cannot find type `CanonicalRecoveryKey` in this scope
```

### GREEN 1

After implementing stable exclusive locking, atomic classification, buffered
write/flush/file sync, and the qualified Linux parent barrier:

```text
running 1 test
test persistence::canonical_durability::tests::public_append_classifies_existing_and_first_created_files_atomically ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1588 filtered out
```

### RED 2: qualified rename

Command:

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud public_rename_syncs_the_file_and_changed_parent_namespace -- --nocapture --test-threads=1
```

Real result: exit 101.

```text
error[E0599]: no method named `rename` found for struct `canonical_durability::CanonicalDurability` in the current scope
```

### GREEN 2

After implementing qualified source sync, rename, and changed-parent barriers:

```text
running 1 test
test persistence::canonical_durability::tests::public_rename_syncs_the_file_and_changed_parent_namespace ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1589 filtered out
```

### Final focused GREEN

Command:

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud canonical_durability -- --nocapture --test-threads=1
```

Real result:

```text
running 13 tests
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 1588 filtered out; finished in 0.04s
```

The 13 tests cover public existing/first-create classification, public rename,
parent refusal, unqualified namespace refusal, shared/exclusive contention and
release, missing shared-lock non-mutation, writer-parent non-provisioning,
Linux/macOS/Windows qualification policy, every append barrier failure, the
first-create parent barrier failure, rename barrier failures, destination
collision, and diagnostic redaction.

## Rust gates

All Rust commands used the one stable worktree-local target directory
`src-tauri/target` and Rust/Cargo 1.95.0.

### Locked cloud check

Command:

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 check --locked --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud
```

Final real output:

```text
Checking audio-graph v0.1.0-rc.1 (.../src-tauri)
Finished `dev` profile [unoptimized + debuginfo] target(s) in 14.36s
```

An earlier check correctly exposed one non-test-build unused-variable warning
in the test fault seam. The parameter was made intentionally underscore-prefixed
and the final check above is clean.

### Strict Clippy

Command:

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 clippy --locked --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud -- -D warnings
```

Real output:

```text
Finished `dev` profile [unoptimized + debuginfo] target(s) in 56.90s
```

### Rustfmt

Command:

```text
cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --check
```

Real result: exit 0, no output.

### One full serialized locked cloud library suite

Command:

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud -- --test-threads=1
```

Real result:

```text
running 1601 tests
test result: ok. 1593 passed; 0 failed; 8 ignored; 0 measured; 0 filtered out; finished in 38.56s
```

## Repository and security gates

### Pinned fast verification

The worktree-local package root was absent, so the plan-authorized repository
fallback resolved to:

```text
/home/codeseys/DevBox/audio-graph/node_modules/@os-eco/seeds-cli
```

Its package identity was exactly `@os-eco/seeds-cli@0.4.5`, and both
`package.json` and `src/output.ts` were present. No package was installed and no
worktree symlink was created.

Command:

```text
SEEDS_CLI_ROOT="$seeds_cli_root" bun run verify:fast
```

Real result: exit 0.

```text
Checked 174 files in 280ms. No fixes applied.
audio source contract is current
provider registry is current
session data movement contract is current
endpoint credential routing contract is current
speech span revision contract is current
sd ready --format json: parsed (50)
sd blocked --format json: parsed (101)
sd list --format json: parsed (50)
docs/Seeds secret hygiene scan passed: 0 findings
```

The command also completed `tsc --noEmit` and `git diff --check` without output.

### Explicit generated-contract verification

Command: `bun run verify:contracts`

Real result: exit 0; all five generated contracts reported current.

### Secret hygiene

Command: `bun scripts/check-docs-secret-hygiene.mjs`

Real output:

```text
docs/Seeds secret hygiene scan passed: 0 findings
```

### Betterleaks

Command:

```text
betterleaks dir --no-banner --redact src-tauri/src/persistence/canonical_durability.rs src-tauri/src/persistence/mod.rs docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-c2e3-report.md
```

Real output from the complete implementation and evidence artifact set:

```text
scanned ~287969 bytes (287.97 KB)
no leaks found
```

The final post-report scan, exact range diff, and footprint are recorded in the
handoff after commit.

## Scope and rollback

Owned implementation paths are:

- `src-tauri/src/persistence/canonical_durability.rs`;
- `src-tauri/src/persistence/mod.rs`; and
- this report.

No `canonical_log` edit was required because runtime adoption is explicitly
out of scope. There is no Cargo dependency/feature expansion, unsafe code,
manifest or recovery transaction, Session semantics floor, prompt, adapter,
UI, workflow, generated artifact, or Seeds mutation.

The module is dormant and writes no production data. Before adoption, rollback
is reversal of the isolated c2e3 commit or branch disposal. No on-disk migration
or runtime reconciliation is required.

## Findings and open questions

- No unrelated problem was changed.
- macOS APFS and Windows NTFS execution remain owned by the later platform
  qualification workstream. This implementation preserves the conditional and
  refusal contracts without simulating those platforms locally.
- Stable locks are cooperative OS file locks. An uncooperative process can
  still mutate names on Unix, and Windows rename/delete sharing is a distinct
  policy; the module does not overstate the lock as a namespace lease.
- No Docker, Blacksmith, workflow, push, merge, deployment, or release action
  was run.
