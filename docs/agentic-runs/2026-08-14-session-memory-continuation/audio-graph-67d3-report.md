# audio-graph-67d3 portable native durability harness report

Date: 2026-08-15

Seed: `audio-graph-67d3`

Branch: `work/67d3-portable-native-harness-wave7b`

Exact base: `4d3bbb6c69b94d8a7ab089008c686721de9d13e8`

Worktree:
`/home/codeseys/DevBox/audio-graph/.worktrees/67d3-portable-native-harness-wave7b`

Implementation commit: `2ed885991ef1c1d15fadbe75aec8f540deb4b040`

## Outcome

The dormant canonical crash harness now compiles its test module on Linux,
macOS, and Windows. The change is tests-only and adds no native dispatch,
runtime caller, production filesystem-qualification constructor, unsafe code,
dependency, workflow, container, guest, or external platform execution.

Target-specific helpers now own the platform differences:

- Linux keeps exact signal-9 assertions, queries the live fixture with
  `findmnt -T` and GNU `stat -f`, and requires ext4 for this qualification
  run.
- macOS keeps exact signal-9 assertions, resolves the live fixture mount with
  `df -P`, reads its filesystem type from `diskutil info -plist`, and runs the
  qualified crash/barrier matrix only when the fixture is APFS. A non-APFS or
  unqueryable fixture emits an explicit typed refusal marker and skips the
  qualification-dependent case rather than making an Accepted claim.
- Windows treats `Child::kill` as `TerminateProcess` and requires a boundedly
  reaped unsuccessful exit with an exit code instead of importing Unix signal
  APIs. It resolves the live fixture volume and requires `fsutil fsinfo
  volumeinfo` to identify NTFS.

`ManagedChild::Drop` now routes cleanup through the same checked kill and
bounded `try_wait` reap loop as explicit cuts. Cleanup diagnostics contain only
the versioned outcome and OS error, never managed paths or fixture bytes.

The cross-process read/rename case now speaks in portable open-handle terms.
It proves that a cooperating strict reader holds the shared coordination lock,
that an uncooperative raw rename remains outside the advisory contract, and
that the already-open handle reads its original complete bytes while the path
names the complete replacement. A content-free marker retains the limitation:
`advisory_lock=cooperating_processes_only`.

On Windows/NTFS, the native-only harness case proves all of these outcomes
before mutation:

- first-create append returns
  `NamespaceDurabilityUnsupported { Windows, FirstCreate }`;
- ordinary rename returns
  `NamespaceDurabilityUnsupported { Windows, Rename }`;
- recovery preflight and recovery rename return
  `NamespaceDurabilityUnsupported { Windows, Rename }`;
- atomic snapshot installation returns
  `NamespaceDurabilityUnsupported { Windows, AtomicSnapshotInstall }`;
- the first-create and rename destinations remain absent;
- the canonical source, recovery temporary, and existing snapshot head retain
  their exact original bytes; and
- the stable coordination identity remains contended until the refusing guard
  drops, then becomes acquirable by a fresh process.

The inherited
`windows_policy_path_allows_existing_append_and_refuses_namespace_mutation`
fixture no longer calls `qualification(root)`. It passes `None` through the
accepted Windows refusal seam and also covers recovery preflight/rename without
mutating the source or recovery temporary. The exact corrected c928 snapshot
test remains qualification-independent.

## TDD RED and GREEN

The agreed seams were the cfg(test) child harness and the public typed
namespace-durability outcomes.

### RED 1: Windows object contained no crash-harness tests

The exact pinned Windows target was available. Before the portability change:

```text
rustc +1.95.0 --edition=2024 --test \
  --emit=obj=src-tauri/target/67d3-red/canonical_crash_harness_windows_red.obj \
  --target x86_64-pc-windows-msvc \
  src-tauri/src/persistence/canonical_crash_harness.rs
strings ... | rg subprocess_child_entrypoint
```

The module compiled only because the nested test module was gated to Linux.
The retained semantic gate exited 1:

```text
RED: Windows test object compiles with zero canonical subprocess tests because the module is Linux-only
```

### RED 2: inherited Windows fixture minted unavailable qualification

The extracted inherited fixture contained:

```text
8:        let proof = qualification(&root);
26:            guard.append(&absent, b"must not create", Some(&proof), key),
35:            guard.rename(&existing, &destination, Some(&proof), key),
RED: inherited native-Windows fixture still requires unavailable qualification identity
```

The gate exited 1.

### GREEN: Linux public harness seam

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked \
  --manifest-path src-tauri/Cargo.toml --lib --no-default-features \
  --features cloud 'persistence::canonical_crash_harness::tests' -- \
  --nocapture --test-threads=1
```

Real result:

```text
running 11 tests
AUDIO_GRAPH_67D3_FIXTURE_FS_V1 platform=linux expected=ext4 observed=ext4 outcome=qualified detail=filesystem_type=ext2/ext3 block_size=4096 ... context=filesystem-evidence
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 1668 filtered out; finished in 7.05s
```

All inherited checkpoint/residual/order oracles remained byte-for-byte
semantic green:

- damaged source: length 35, SHA-256
  `6503f619b147e275fefa3f986547ac238a9cc155674e9488daedb3827e058e6a`;
- retained source: length 12, SHA-256
  `3a37782e8974c48eebf2a0517c866ad15641c53b3d31993188796b56aeb79624`;
- exact quarantine: length 23, SHA-256
  `fe27b7aee64f84a7f777321ff87134ca7f5027a1509ace85af41a1f3b343e2f1`;
- empty created temp: length 0, SHA-256
  `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`;
  and
- first-create entry: length 18, SHA-256
  `bda22f24c7a63e7576093c8a478f7d5951b8f301cad73a599b3ac17acab43722`.

### GREEN: injected Windows refusal seams

```text
cargo +1.95.0 test ... windows_policy_path_allows_existing_append_and_refuses_namespace_mutation -- --nocapture --test-threads=1
cargo +1.95.0 test ... snapshot_windows_and_unqualified_paths_refuse_before_temp_or_head_mutation -- --nocapture --test-threads=1
```

Real results:

```text
test persistence::canonical_durability::tests::windows_policy_path_allows_existing_append_and_refuses_namespace_mutation ... ok
test result: ok. 1 passed; 0 failed; 1678 filtered out

test persistence::canonical_durability::tests::snapshot_windows_and_unqualified_paths_refuse_before_temp_or_head_mutation ... ok
test result: ok. 1 passed; 0 failed; 1678 filtered out
```

The post-change source gate also returned `GREEN`: the inherited fixture has
no `qualification(&root)` or `Some(&proof)` dependency.

## Platform compile evidence

Pinned toolchain inspection was explicit:

```text
rustup +1.95.0 target list --installed
x86_64-pc-windows-msvc
x86_64-unknown-linux-gnu

rustup +1.95.0 component list --installed
rust-std-x86_64-pc-windows-msvc
rust-std-x86_64-unknown-linux-gnu
```

The pinned Windows std artifacts were present:

```text
libstd-0cebe7c42cd80226.rmeta 9633944 bytes
libstd-0cebe7c42cd80226.rlib 5648528 bytes
```

A dependency-minimal wrapper imported the actual current
`canonical_durability.rs`, `session_artifact_manifest.rs`,
`canonical_log.rs`, and `canonical_crash_harness.rs`. Direct pinned `rustc`
compiled both the production module set and the full cfg(test) object for
`x86_64-pc-windows-msvc`:

```text
libcanonical_native.rlib 6588256 bytes
canonical_native_tests.obj 6440188 bytes
Windows actual-module test object contains portability, c928, and inherited refusal tests
```

Object inspection required these symbols to be present:

```text
subprocess_child_entrypoint
windows_ntfs_namespace_paths_refuse
snapshot_windows_and_unqualified_paths_refuse
windows_policy_path_allows
```

Expected unused/dead-code warnings came from Unix-only fault injectors and
crash residual helpers in the Windows object. Linux strict Clippy remained
warning-free.

No Apple std target is installed in the pinned toolchain. The strongest actual
target probe established the intended cfg and then stopped before module
compilation:

```text
rustc +1.95.0 --print cfg --target aarch64-apple-darwin
target_arch="aarch64"
target_family="unix"
target_os="macos"

rustc +1.95.0 --edition=2024 --crate-type lib \
  --target aarch64-apple-darwin \
  src-tauri/src/persistence/canonical_crash_harness.rs
error[E0463]: can't find crate for `std`
macOS actual-module compile limitation: exit=1; pinned Apple std artifact absent
```

No component was installed. The macOS helper therefore has host-executed
synthetic parser coverage and source review but no actual-target object or
native APFS execution in this worktree.

## Regression and broad gates

All Rust gates used Rust/Cargo 1.95.0, `--locked`, and the worktree-local
`src-tauri/target`.

Focused baselines:

```text
canonical crash harness: 11 passed; 0 failed
canonical_log: 46 passed; 0 failed; finished in 2.18s
session_artifact_manifest: 18 passed; 0 failed; finished in 10.78s
canonical_durability: 40 passed; 0 failed; finished in 3.35s
```

Locked cloud check:

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 check --locked \
  --manifest-path src-tauri/Cargo.toml --lib --tests \
  --no-default-features --features cloud
Finished `dev` profile [unoptimized + debuginfo] target(s) in 2m 48s
```

Final serialized full locked cloud library:

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --quiet --locked \
  --manifest-path src-tauri/Cargo.toml --lib --no-default-features \
  --features cloud -- --test-threads=1

running 1679 tests
test result: ok. 1671 passed; 0 failed; 8 ignored; 0 measured; 0 filtered out; finished in 68.70s
```

The Linux host emitted the existing PipeWire/ALSA no-device diagnostics; they
did not fail the suite.

Strict Clippy and rustfmt:

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 clippy --locked \
  --manifest-path src-tauri/Cargo.toml --lib --tests \
  --no-default-features --features cloud -- -D warnings
cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --all -- --check
```

Real result: strict Clippy exited 0 in 56.23 seconds; rustfmt exited 0 with no
output.

Pinned repository verification:

```text
SEEDS_CLI_ROOT=/home/codeseys/DevBox/audio-graph/node_modules/@os-eco/seeds-cli bun run verify:fast
bun run verify:contracts
```

Real result: both exited 0. Biome checked 174 files without fixes, TypeScript
passed, all five generated contracts were current, Seeds stdout stress parsed
ready 50, blocked 97, and list 50, docs/Seeds secret hygiene reported 0
findings, and `git diff --check` passed.

## Security, cfg-only, and footprint

Before the implementation commit:

```text
betterleaks dir --no-banner --redact \
  src-tauri/src/persistence/canonical_crash_harness.rs \
  src-tauri/src/persistence/canonical_durability.rs
scanned ~215381 bytes (215.38 KB)
no leaks found

bun scripts/check-docs-secret-hygiene.mjs
docs/Seeds secret hygiene scan passed: 0 findings

git diff --check
exit 0
```

Static scope results:

```text
footprint_before_report:
src-tauri/src/persistence/canonical_crash_harness.rs
src-tauri/src/persistence/canonical_durability.rs
forbidden_scope_diff_lines=0
cfg_test_module=pass
runtime_dark_callers=canonical_log/canonical_durability checkpoints and test harness only
unsafe_or_runtime_surface=none
inherited_none_gate=GREEN
```

The only `canonical_durability.rs` change is inside its existing `#[cfg(test)]
test module. `persistence/mod.rs` still declares the entire crash harness only
under `#[cfg(test)]`. No Cargo manifest, dependency, lockfile, workflow,
Docker, Blacksmith, Testbox, Dockurr, guest, generated contract, frontend,
runtime command, or Seeds file changed.

Report-inclusive security and ancestry results:

```text
betterleaks scanned ~228770 bytes (228.77 KB): no leaks found
docs/Seeds secret hygiene scan passed: 0 findings
git diff --check: exit 0
exact base is an ancestor of implementation tip: pass
implementation history: 2ed8859 audio-graph-67d3: port native crash harness
```

The exact report-inclusive base-to-tip footprint is the two owned cfg(test)
source files plus this report. Final staged diff and clean-tip evidence is
recorded in the commit handoff because the report commit cannot name its own
hash.

## Review and remaining evidence

One bounded self-critique round found that the first portable draft proved
Windows namespace refusal but did not explicitly carry the stable coordination
identity through a refusing rename. The correction added a fresh-process
contender while the refusing guard remains live, then a fresh-process
successful acquisition after drop. The same round made the advisory limitation
observable through a content-free marker. Focused and broad gates above are
post-correction. No second fix round was needed.

No production defect or ownership blocker was found. The following evidence is
intentionally deferred:

- native macOS 15 execution on actual APFS, including successful or typed
  refused parent-directory `sync_all` and the qualified crash/lock/read/rename
  matrix;
- native Windows 2025 execution on actual NTFS, including this harness, the
  exact c928 refusal test, and the lock/read/rename/release-on-death matrix; and
- the separately approval-gated workflow and monitored native Actions run
  owned by `audio-graph-52b9` and `audio-graph-2df3`.

Cross-compilation is compile evidence only. Process kill/reopen is process-
crash recovery evidence only. Neither is labeled native filesystem execution
or power-loss proof.

Rollback before runtime adoption is reversal of the isolated implementation
and report commits, or disposal of this worktree branch. No on-disk migration
or recovery action is needed because the harness remains dormant and
cfg(test)-only.
