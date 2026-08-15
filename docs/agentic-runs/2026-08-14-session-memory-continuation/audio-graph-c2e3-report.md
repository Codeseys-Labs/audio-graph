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

Review/fix round 1 corrected reviewed tip
`71f16b885d79a9df32b4dec011895032339d75c6`. The corrected interface returns an
opaque mutation guard bound to one canonical managed root, its deterministic
coordination file, and safe filesystem identity where stable Rust exposes it.
Append and rename are guard-owned operations; an arbitrary lock can no longer
be paired with an unrelated target.

Review/fix round 2 corrected reviewed tip
`3e05edf3480a0139cc252e9a0b785ddf1a4c8e52`. The final contract binds a guard to
the target's exact canonical parent, reserves the coordination entry from every
mutation, restricts publication to same-directory rename, and exposes typed
identity-drift and cross-device refusals. Windows namespace policy is checked
before an ordinary parent open or any mutation; existing-file append remains
available and retains its file barrier.

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
  missing strict-reader lock acquisition creates neither the file nor parent;
- refuses dangling symlink, FIFO, socket, directory, and other non-regular
  existing entries without opening or following them;
- refuses the deterministic coordination entry as an append target, rename
  source, or rename destination, preserving the lock inode and pathname;
- returns `IdentityChanged` when stable source, parent, or root identity drifts,
  and `CrossDeviceRenameRefused` for a safe preflight volume mismatch or
  `EXDEV`/`CrossesDevices`;
- retains both `ErrorKind` and `raw_os_error: Option<i32>` in content-free I/O
  failures; and
- keeps filesystem qualification opaque, non-forgeable, and bound to the exact
  managed root plus safe volume/object identity. Production has no constructor
  until the later platform probe supplies real qualification evidence.

The module is declared from `persistence::canonical_durability`. Existing
`canonical_log` and strict-reader implementations are unchanged, so this work
does not activate a writer or make strict reads mutate.

## Public interface and deep-module seam

`CanonicalDurability` is a guard factory. Its small interface consists of:

- `try_lock_exclusive`: acquire an opaque mutation guard for one existing
  exact target parent and its deterministic coordination file; and
- `try_lock_shared`: open-only strict-reader acquisition;

`CanonicalExclusiveGuard` owns the two mutation operations:

- `append`: existing append or atomically classified first-create inside the
  bound root; and
- `rename`: unique-destination publication between two immediate children of
  the same bound parent.

Callers receive typed `Accepted`, `Rejected`, or
`DurabilityIndeterminate` outcomes. Paths, payload bytes, OS error messages,
and opaque recovery bytes are absent from `Debug`; only numeric OS error codes
remain. `CanonicalFilesystemQualification` has private fields and no production
constructor. The internal test seam can mint evidence bound to a Unix
`dev`/`ino` root identity, allowing the substrate to be exercised without
claiming that the local test filesystem is production-qualified.

## Invariants

1. `Accepted(ExistingAppend)` follows write, flush, and file `sync_all`.
2. `Accepted(FirstCreate)` follows `create_new`, write, flush, file `sync_all`,
   and qualified parent-directory `sync_all`.
3. `Accepted(Rename)` follows source file `sync_all`, same-directory rename,
   and the changed parent-directory `sync_all`.
4. A first-created name is never inferred from a preflight existence check.
5. A canonical parent directory is never provisioned by this module.
6. The deterministic lock path is derived internally from the exact target
   parent. Guard-owned operations reject every other parent, including a
   descendant selected through an overlapping ancestor root.
7. Qualification evidence is opaque and must exactly match the guard's root,
   volume, and directory object identity. Production remains unqualified until
   the later platform probe adds a reviewed construction path.
8. Windows first-create and rename remain refused before mutation. Rust 1.95's
   Windows volume/file-index metadata methods are nightly-only, so this module
   does not use them or overclaim Windows object identity.
9. macOS namespace acceptance remains conditional on later APFS qualification;
   no production proof can currently be constructed.
10. Any failed visible or possibly visible write/flush/sync/rename barrier is
   indeterminate and carries the caller-supplied recovery identity.
11. Every content-free I/O outcome carries both `ErrorKind` and the optional
    raw OS error number. The recovery identity has no diagnostic byte accessor
    and formats only as `[REDACTED]`.
12. Destination and existing-entry probes use non-following metadata and refuse
   all non-regular entries before open. Stable source, parent, and root identity
   is compared where safe `std` metadata exposes it.
13. The coordination file is reserved from append and both sides of rename; it
   is never renamed or removed by the module. Shared acquisition opens existing
   state only.
14. Locks remain cooperative/advisory. An uncooperative pathname swap after a
   validated open remains outside this contract; no absolute namespace lease is
   claimed.

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

### Review round 1 RED: non-following destination collision

Command:

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud rename_refuses_a_dangling_symlink_destination_without_mutation -- --nocapture --test-threads=1
```

Real result: exit 101. The old following probe replaced the dangling symlink:

```text
assertion `left == right` failed
  left: Accepted(CanonicalDurabilityReceipt { mutation: Rename, barrier: FileAndParentNamespace })
 right: Rejected(DestinationAlreadyExists)
test result: FAILED. 0 passed; 1 failed
```

GREEN changed the collision probe to `symlink_metadata`. The focused test then
passed without replacing the symlink or source.

### Review round 1 RED: namespace-bound guard interface

Command:

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud guard_owned_append_rejects_a_target_outside_its_managed_namespace -- --nocapture --test-threads=1
```

Real result: exit 101.

```text
error[E0599]: no method named `append` found for struct `CanonicalExclusiveGuard`
error[E0599]: no variant or associated item named `TargetOutsideManagedNamespace`
```

GREEN moved append/rename onto the opaque bound guard, derived the coordination
path internally, bound safe root/volume/object identity, and rejected the
outside target without creating it.

### Review round 1 focused GREEN

Command:

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud canonical_durability -- --nocapture --test-threads=1
```

Real result:

```text
running 15 tests
test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 1588 filtered out; finished in 0.05s
```

The 15 tests cover public existing/first-create classification, a deterministic
real concurrent create race, public rename, parent and unqualified refusal,
shared/exclusive contention and release, missing-root and missing-lock strict
non-mutation, writer-root non-provisioning, target/root mismatch, qualification
reuse on another root, Linux/macOS/Windows policy, every append/namespace
barrier failure, numeric raw OS error retention, regular/dangling-symlink/FIFO
destination collision, non-regular existing-entry refusal, and diagnostic
redaction.

### Review round 2 RED/GREEN

Each RED used the same locked, cloud-only, serialized public-module command,
with the named test substituted for `TEST`:

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud TEST -- --nocapture --test-threads=1
```

The five exact RED cases were:

1. `only_the_exact_target_parent_can_coordinate_a_nested_target`: exit 101;
   the ancestor guard reached qualification validation and returned
   `QualificationBindingMismatch` instead of refusing the overlapping root as
   `TargetOutsideManagedNamespace`.
2. `coordination_entry_is_reserved_from_append_and_rename`: exit 101 at compile
   time because `CanonicalDurabilityRejection::ReservedCoordinationEntry` did
   not exist. The fixture covered append, rename source, rename destination,
   lock length, source retention, and absent destination.
3. `windows_policy_path_allows_existing_append_and_refuses_namespace_mutation`:
   exit 101 at compile time because the actual guard factory had no injected
   platform constructor. The fixture exercised guard-owned existing append,
   absent append, and rename rather than only testing a policy predicate.
4. `source_identity_replacement_is_refused_before_rename`: exit 101 at compile
   time because the identity-race seam and typed `IdentityChanged` refusal did
   not exist.
5. `cross_device_rename_is_refused_with_zero_mutation`: exit 101 at compile time
   because typed `CrossDeviceRenameRefused` did not exist. The fixture injects
   Linux raw OS error 18 at rename and checks source retention and destination
   absence.

GREEN implemented exact-parent binding, reserved-name rejection, cfg-specific
parent opening, pre-mutation Windows policy, safe identity comparison, and
cross-device classification. Each named test first passed alone as 1/1. The
final consolidated focused command was:

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud canonical_durability -- --nocapture --test-threads=1
```

Final real result after the last production edit:

```text
running 20 tests
test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 1588 filtered out; finished in 0.07s
```

The five new tests add overlapping-root coordination, reserved lock entry,
actual Windows guard policy, source identity replacement, and typed
cross-device zero-mutation coverage to the round-1 set.

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
Finished `dev` profile [unoptimized + debuginfo] target(s) in 20.57s
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
Finished `dev` profile [unoptimized + debuginfo] target(s) in 29.85s
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
running 1608 tests
test result: ok. 1600 passed; 0 failed; 8 ignored; 0 measured; 0 filtered out; finished in 39.90s
```

### Windows actual-module compile proof

The default toolchain target listing was not authoritative for the pinned
toolchain. `rustup component list --toolchain
1.95.0-x86_64-unknown-linux-gnu --installed` confirmed both
`rust-std-x86_64-pc-windows-msvc` and the Linux standard library. The actual
dormant module, including its real `cfg(target_os = "windows")` branch, was then
cross-compiled directly:

```text
mkdir -p src-tauri/target/c2e3-windows-module-proof
rustc +1.95.0 --edition=2024 --crate-type lib --target x86_64-pc-windows-msvc src-tauri/src/persistence/canonical_durability.rs --out-dir src-tauri/target/c2e3-windows-module-proof
```

Real result: exit 0; `libcanonical_durability.rlib` was emitted. A source check
also confirmed the Windows directory-opening branch contains no ordinary
`File::open`. This is a compile proof of the actual module, not native NTFS
durability execution; the later platform qualification workstream still owns
native Windows evidence.

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
Checked 174 files in 427ms. No fixes applied.
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
scanned ~323414 bytes (323.41 KB)
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
is reversal of the isolated c2e3 commits or branch disposal. Round 2 is a
separate correction commit on reviewed tip
`3e05edf3480a0139cc252e9a0b785ddf1a4c8e52`; it is not an amendment. No on-disk
migration or runtime reconciliation is required.

## Findings and open questions

- No unrelated problem was changed.
- macOS APFS and native Windows NTFS execution remain owned by the later
  platform qualification workstream. This implementation preserves the
  conditional and refusal contracts; the module itself cross-compiles for
  Windows, while the injected-platform test executes its policy sequencing on
  Linux.
- Rust 1.95 local standard-library documentation marks Windows
  `volume_serial_number` and `file_index` metadata methods as nightly-only.
  The correction therefore uses no unstable/unsafe platform expansion:
  Windows guards are deterministic path/type-bound under cooperation, and no
  Windows namespace qualification can be constructed. Safe Unix `dev`/`ino`
  identity binds the internal Linux/macOS test evidence.
- Stable locks are cooperative OS file locks. An uncooperative process can
  still replace a root or mutate names on Unix, and Windows rename/delete
  sharing is a distinct policy; the module does not overstate the lock as a
  namespace lease or an uncooperative-process security boundary.
- No Docker, Blacksmith, workflow, push, merge, deployment, or release action
  was run.
