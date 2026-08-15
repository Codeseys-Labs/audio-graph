# audio-graph-83e2 runtime EXDEV classification report

Date: 2026-08-15

Seed: `audio-graph-83e2`

Branch: `work/83e2-runtime-exdev-wave7b`

Exact stacked base: `28961a7a5f2f262f6dffed498ba56f5a384db0c0`

Worktree:
`/home/codeseys/DevBox/audio-graph/.worktrees/83e2-runtime-exdev-wave7b`

Implementation commit: `22e7cae067e983be8090f90a62830c619376d88f`

## Outcome

The dormant canonical-durability module now distinguishes the two
cross-device rename states required by the corrected durability research:

- a safe source/parent volume mismatch proven during preflight returns
  `CrossDeviceRenameRefused { raw_os_error: None }` before the rename-operation
  seam is invoked; and
- any error returned by the invoked rename operation, including
  `ErrorKind::CrossesDevices` / Linux `EXDEV`, returns
  `DurabilityIndeterminate` at `CanonicalDurabilityStage::Rename` with the
  caller's opaque recovery key, `ErrorKind`, and `raw_os_error` preserved.

The runtime fault hook does not claim that a real EXDEV mutates. It marks that
the rename-operation seam was invoked and then deterministically returns EXDEV
without calling the host filesystem. The unchanged injected fixture therefore
is not evidence that a real rename attempt could not have made state visible.
The indeterminate classification exists precisely because visibility after the
actual operation returns an error is not proven.

The safe-preflight fixture separately proves zero mutation: source bytes are
unchanged, the destination is absent, and the rename-operation observer remains
false.

No public interface was added or widened. Existing c2e3 and ce19 behavior is
preserved, including ASCII-case-equivalent coordination-name reservation,
exact-parent lock binding, opaque qualification, Windows namespace refusal,
content-free diagnostics, raw OS code retention, and no runtime writer.

## Deep-module seam and implementation

The external seam remains `CanonicalExclusiveGuard::rename` returning
`CanonicalDurabilityOutcome`. The implementation adds only an internal,
test-only rename fault state with two distinct points:

1. `PreflightDeviceMismatch`, consulted by the volume comparison before any
   rename invocation; and
2. `RuntimeExdev`, returned from the invoked rename-operation seam.

Production rename errors now all use the existing `indeterminate` constructor.
This concentrates recovery identity, stage, `ErrorKind`, and raw OS error
handling behind the existing interface instead of asking callers to interpret
errno values.

## TDD evidence

The agreed test seam was the public canonical durability rename interface. The
internal deterministic fault state observes whether execution reached the
rename-operation seam without exposing a new production interface.

### RED 1: safe preflight mismatch

Command:

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud preflight_device_mismatch_refuses_before_invoking_rename -- --nocapture --test-threads=1
```

Real result: exit 101. The distinct fault point and constructor did not exist:

```text
error[E0599]: no function or associated item named `with_rename_fault` found for struct `CanonicalDurability`
error[E0433]: failed to resolve: use of undeclared type `InjectedRenameFault`
```

### GREEN 1

The same command passed after adding only the preflight mismatch point and a
rename-operation observer:

```text
running 1 test
test persistence::canonical_durability::tests::preflight_device_mismatch_refuses_before_invoking_rename ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1609 filtered out
```

The test asserts the typed refusal has no runtime OS error, the rename observer
is false, source bytes are unchanged, and the destination remains absent.

### RED 2: runtime rename EXDEV

Command:

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud runtime_rename_exdev_is_indeterminate_after_invocation -- --nocapture --test-threads=1
```

Real result: exit 101 before the runtime fault point existed:

```text
error[E0599]: no variant named `RuntimeExdev` found for enum `InjectedRenameFault`
```

### GREEN 2

The same command passed after adding the runtime point and removing the unsafe
zero-mutation classification from the invoked rename-error branch:

```text
running 1 test
test persistence::canonical_durability::tests::runtime_rename_exdev_is_indeterminate_after_invocation ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1610 filtered out; finished in 0.01s
```

The test asserts the rename-operation observer is true and the exact result is
`DurabilityIndeterminate` with stage `Rename`, kind `CrossesDevices`, raw OS
error 18, and the caller-supplied recovery key.

## Gates and real results

All Rust gates used Rust/Cargo 1.95.0 and the stable worktree-local target
`src-tauri/target`.

### Focused serialized durability suite

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud canonical_durability -- --nocapture --test-threads=1
```

Result:

```text
test result: ok. 23 passed; 0 failed; 0 ignored; 0 measured; 1588 filtered out; finished in 3.31s
```

### Locked cloud check

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 check --locked --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud
```

Result: exit 0; `Finished dev profile ... in 2m 01s`.

### Strict Clippy and rustfmt

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 clippy --locked --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud -- -D warnings
cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --check
```

Results: Clippy exit 0 in 40.50s; rustfmt exit 0 with no output.

### Pinned Windows actual-module cross-compile

```text
rustc +1.95.0 --edition=2024 --crate-type lib --target x86_64-pc-windows-msvc src-tauri/src/persistence/canonical_durability.rs --out-dir src-tauri/target/83e2-windows-module-proof
```

Result: exit 0; `libcanonical_durability.rlib` emitted at 571584 bytes. This is
an actual-module cross-compile, not native NTFS execution evidence.

### One full serialized locked cloud library suite

This ran once after the final production change:

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud -- --test-threads=1
```

Result:

```text
test result: ok. 1603 passed; 0 failed; 8 ignored; 0 measured; 0 filtered out; finished in 42.01s
```

### Pinned repository and contract verification

The Seeds CLI root was pinned to the existing custody package at
`/home/codeseys/DevBox/audio-graph/node_modules/@os-eco/seeds-cli`; no package
or symlink was installed.

```text
SEEDS_CLI_ROOT=/home/codeseys/DevBox/audio-graph/node_modules/@os-eco/seeds-cli bun run verify:fast
bun run verify:contracts
```

Results: exit 0. Biome checked 174 files with no fixes, TypeScript passed, all
five generated contracts were current, Seeds JSON parsed (`ready 50`,
`blocked 102`, `list 50`), docs/Seeds secret hygiene reported 0 findings, and
`git diff --check` passed. The explicit contract rerun also reported all five
contracts current.

### Security and implementation footprint

Before the implementation commit:

```text
bun scripts/check-docs-secret-hygiene.mjs
betterleaks dir --no-banner --redact src-tauri/src/persistence/canonical_durability.rs
git diff --check
git diff --name-only 28961a7a5f2f262f6dffed498ba56f5a384db0c0
```

Results: secret hygiene found 0 findings; Betterleaks scanned approximately
73.66 KB and found no leaks; diff checks passed; the exact implementation
footprint contained only
`src-tauri/src/persistence/canonical_durability.rs`.

The report-inclusive Betterleaks rerun scanned approximately 83.01 KB and found
no leaks; docs/Seeds secret hygiene again reported 0 findings. The staged
base-to-candidate footprint contained exactly this report and
`src-tauri/src/persistence/canonical_durability.rs` (`370 insertions`,
`19 deletions`); staged diff checks passed. The clean tip and final commit list
are recorded in the handoff after the report commit.

## Scope and rollback

Owned paths:

- `src-tauri/src/persistence/canonical_durability.rs`
- this report

There is no unsafe code, dependency or feature expansion, runtime activation,
workflow edit, generated artifact edit, or Seeds mutation. No Docker,
Blacksmith, push, merge, deployment, release, `sd update`, `sd close`, or
`sd sync` action was run.

Rollback before runtime adoption is reversal of the isolated implementation
and report commits or disposal of this worktree branch. No on-disk migration or
runtime reconciliation is required.

## Findings and open questions

- Native macOS APFS and Windows NTFS execution remain owned by
  `audio-graph-2df3`; this work supplies policy, Linux-host deterministic
  classification tests, and pinned Windows compile evidence only.
- The c2e3, ce19, and 83e2 stack remains integration-pending. This worktree did
  not merge or alter its exact reviewed base.
- No unrelated issue was changed.
