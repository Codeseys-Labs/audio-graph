# audio-graph-c928 atomic snapshot durability report

Date: 2026-08-15

Seed: `audio-graph-c928`

Branch: `work/c928-atomic-snapshot-wave7b`

Exact base: `f912a073dd4acedde6df8d352cbe33dbb605fca7`

Worktree:
`/home/codeseys/DevBox/audio-graph/.worktrees/c928-atomic-snapshot-wave7b`

Implementation commit: `c5daccc68a4865e90586e8d7c62359922b49a8b7`

## Outcome

Added one guard-owned `CanonicalExclusiveGuard::install_snapshot` seam for
initial or replacement snapshot installation. The interface keeps the
filesystem protocol inside the existing opaque exact-root exclusive guard and
does not add a manifest schema, generation state machine, runtime caller, or
forgeable production qualification constructor.

The operation accepts an exact caller-chosen temp path, destination, complete
candidate bytes, opaque recovery key, qualified namespace evidence, and one
of two destination expectations:

- `Absent` for the first manifest head; or
- `Existing(&File)` for replacement, where the handle is the exact file from
  which the caller validated its manifest generation.

The replacement expectation lets `audio-graph-a596` bind its generation CAS
to one open destination identity without teaching the durability module the
manifest schema. The guard verifies that the handle and destination pathname
still identify the same regular file before temp creation and immediately
before rename. Cooperating writers remain serialized by the stable
coordination lock. As documented by the existing module contract,
uncooperative pathname activity is outside the advisory-lock guarantee; a
detected late replacement is nevertheless preserved and returned as
indeterminate rather than accepted.

The accepted ordering is:

1. reject reserved coordination aliases and temp/destination overlap;
2. acquire the guard's in-process operation mutex;
3. refuse Windows or an unqualified namespace before temp creation;
4. bind both paths to the same exact canonical parent and current root
   identity;
5. validate absent or exact-open existing destination identity and safe volume;
6. create the exact temp with `create_new` and no append fallback;
7. write all bytes, flush userspace buffering, enforce owner-only protection,
   and call temp `sync_all`;
8. revalidate the destination expectation;
9. invoke one atomic same-directory `std::fs::rename`, replacing an existing
   regular destination where expected; and
10. complete the qualified parent-directory `sync_all` before returning
    `Accepted(FileAndParentNamespace)`.

Unix temp creation requests mode `0600`, then reapplies exact owner-only
permissions before the file barrier. The second step prevents a permissive
pre-publication mode even if process defaults change; the later `sync_all`
covers both content and protection metadata. Windows cannot reach either step
because atomic snapshot namespace durability remains unsupported.

Typed outcomes distinguish `InitialSnapshotInstall` from
`SnapshotReplacement`, `AtomicSnapshotInstall` namespace refusal,
`SnapshotTempAlreadyExists`, and `SnapshotPathOverlap`. Existing regular,
symlink, directory, FIFO, and Unix-socket temp or destination collisions are
refused without opening or replacing them. ASCII-case-equivalent temp/head
basenames are conservatively treated as overlap, matching the module's
volume-independent reserved-name floor.

Failures before temp creation are typed rejections. Once a temp may be
visible, write, flush, protection, file-sync, destination-revalidation,
runtime rename including `EXDEV`, and parent-sync failures return
`DurabilityIndeterminate` with stage, numeric OS error when available, and the
redacted recovery key. Tests reopen only complete old or complete new head
bytes and preserve any candidate temp needed for reconciliation.

## TDD evidence

The agreed public seam was the guard-owned snapshot operation. Tests observe
filesystem outcomes through that interface; internal barriers exist only
under `cfg(test)` for deterministic races. The production filesystem
qualification remains opaque and has no constructor.

### Public-seam RED

Command:

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud public_snapshot_install_creates_an_owner_only_durable_head -- --nocapture --test-threads=1
```

Real result: exit 101.

```text
error[E0599]: no method named `install_snapshot` found for struct `CanonicalExclusiveGuard`
error[E0433]: cannot find type `CanonicalSnapshotExpectation` in this scope
error[E0599]: no variant or associated item named `InitialSnapshotInstall` found for enum `CanonicalMutation`
error: could not compile `audio-graph` (lib test) due to 3 previous errors
```

### Initial-install GREEN

The same command passed after the first vertical slice:

```text
running 1 test
test persistence::canonical_durability::tests::public_snapshot_install_creates_an_owner_only_durable_head ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1611 filtered out
```

### Replace-existing GREEN

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud public_snapshot_install_replaces_only_the_open_expected_head -- --nocapture --test-threads=1
```

Real result:

```text
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1612 filtered out
```

The test reads generation-one bytes through the exact expected handle, then
asserts only the generation-two candidate is visible after accepted
replacement.

### Deterministic coverage

Thirteen new tests cover:

- initial owner-only installation and replacement bound to an open validated
  destination;
- exact competing `create_new` temp races with no fallback;
- regular, dangling-symlink, directory, FIFO, and Unix-socket collisions for
  temp and destination;
- exact and ASCII-case-equivalent temp/destination overlap;
- reserved coordination aliases before filesystem access;
- overlapping parents and replaced managed-root identity;
- destination replacement before temp creation, which safely rejects with no
  temp;
- destination replacement after temp sync, which preserves candidate, old,
  and replacement bytes and returns indeterminate;
- safe preflight cross-device refusal versus runtime `EXDEV` after rename
  invocation;
- Windows and unqualified no-mutation namespace refusal;
- create, write, flush, owner-protection, file-sync, rename, and parent-sync
  fault cuts with restart-visible old-or-new complete bytes; and
- content-free diagnostics with numeric raw OS error retention.

The late destination replacement test is deliberately stronger than a simple
preflight mismatch: after the candidate temp is synchronized, an injected
race replaces the destination. Revalidation returns
`DurabilityIndeterminate { stage: InspectEntry }`, leaves the candidate temp,
and does not rename over the competing head.

One test-only correction shortened the fixture root prefix after the Unix
socket case exceeded Linux `sun_path`; the failing fixture never reached the
durability operation. The corrected full focused suite is recorded below.

## Gates and real results

All Rust gates used Rust/Cargo 1.95.0 and the stable worktree-local target
`src-tauri/target`.

### Focused serialized canonical durability suite

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud canonical_durability -- --nocapture --test-threads=1
```

Final result:

```text
test result: ok. 36 passed; 0 failed; 0 ignored; 0 measured; 1588 filtered out; finished in 3.70s
```

This includes every inherited c2e3, ce19, and 83e2 regression, including all
8,388,608 ASCII-case permutations of the reserved coordination basename.

### One full serialized locked cloud library suite

The final-source run completed after all production behavior and interface
documentation changes:

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud -- --test-threads=1
```

Result:

```text
test result: ok. 1616 passed; 0 failed; 8 ignored; 0 measured; 0 filtered out; finished in 43.45s
```

### Locked check, strict Clippy, rustfmt, and diff

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 check --locked --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 clippy --locked --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud -- -D warnings
cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --check
git diff --check
```

Results: locked check exit 0 in 8.25s; strict Clippy exit 0 in 17.37s;
rustfmt and diff checks exit 0 with no output.

### Pinned Windows actual-module compile

Installed pinned targets included `x86_64-pc-windows-msvc` and
`x86_64-unknown-linux-gnu`.

```text
rustc +1.95.0 --edition=2024 --crate-type lib --target x86_64-pc-windows-msvc src-tauri/src/persistence/canonical_durability.rs --out-dir src-tauri/target/c928-windows-module-proof
```

Result: exit 0; final `libcanonical_durability.rlib` size 674192 bytes. This
is an actual-module cross-compile proving the Windows unsupported path remains
buildable, not native NTFS execution or namespace-durability evidence.

### Pinned repository and contract verification

The worktree-local package root was absent. `SEEDS_CLI_ROOT` was pinned to the
existing custody package
`/home/codeseys/DevBox/audio-graph/node_modules/@os-eco/seeds-cli`, verified as
`@os-eco/seeds-cli@0.4.5`; no dependency or symlink was installed.

```text
SEEDS_CLI_ROOT=/home/codeseys/DevBox/audio-graph/node_modules/@os-eco/seeds-cli bun run verify:fast
bun run verify:contracts
```

Results: exit 0. Biome checked 174 files without fixes, TypeScript passed, all
five generated contracts were current, Seeds JSON stress parsing reported
`ready 50`, `blocked 101`, and `list 50`, docs/Seeds secret hygiene reported 0
findings, and `git diff --check` passed. The explicit contract rerun again
reported all five contracts current.

### Security and footprint

Before the implementation commit:

```text
betterleaks dir --no-banner --redact src-tauri/src/persistence/canonical_durability.rs
bun scripts/check-docs-secret-hygiene.mjs
git diff --check
git diff --name-only f912a073dd4acedde6df8d352cbe33dbb605fca7
```

Results: Betterleaks scanned approximately 118.44 KB and found no leaks;
docs/Seeds secret hygiene found 0 findings; diff checks passed; the exact
implementation footprint contained only
`src-tauri/src/persistence/canonical_durability.rs`.

The report-inclusive security scan, exact base-to-tip footprint, ancestry, and
clean tip are recorded in the final handoff after the report commit.

## Scope, rollback, and findings

Owned paths:

- `src-tauri/src/persistence/canonical_durability.rs`
- this report

There is no unsafe code, dependency or feature expansion, manifest model,
generation state machine, recovery transaction, runtime caller, UI, workflow,
Docker, Blacksmith, guest, or Seeds mutation. No push, merge, deployment,
release, `sd update`, `sd close`, or `sd sync` action was run.

Rollback before runtime adoption is reversal of the isolated implementation
and report commits or disposal of this worktree branch. No on-disk migration
or reconciliation is required because no caller is active.

Remaining ownership is explicit:

- `audio-graph-a596` must read and validate the current manifest generation
  through the same open `File` passed as `Existing`, choose a stable exact temp
  identity, and consume the typed durability result without relabeling an
  indeterminate outcome.
- `audio-graph-2df3` still owns native Linux filesystem allowlisting, macOS
  APFS directory qualification, and Windows native policy evidence. Windows
  atomic snapshot installation remains a pre-mutation
  `NamespaceDurabilityUnsupported` result.
- Locked recovery, subprocess kill/reopen, and broad artifact consumer
  adoption remain outside c928 under their existing Seeds.

No safe-std blocker remains for the qualified Linux/macOS operation. The
accepted interface deliberately stops at the platform evidence boundary and
does not treat cross-compilation or process restart as power-loss proof.
