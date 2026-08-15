# audio-graph-836b native-Windows recovery fixture report

Date: 2026-08-15

Seed: `audio-graph-836b`

Branch: `work/836b-windows-recovery-fixtures-wave7b`

Worktree:
`/home/codeseys/DevBox/audio-graph/.worktrees/836b-windows-recovery-fixtures-wave7b`

Exact base: `cffb0c5f340d73ec9377319dc3cdbcfbae9074fc`

Implementation commit: `eda0a8a2f47470bb990bb51a0d0708e312497478`

## Outcome

The 15 platform-independent canonical recovery fixtures now opt into one
explicit cfg(test)-only algorithm qualification seam instead of asking the
native filesystem qualification helper to mint unavailable volume/object
identity on Windows. `SessionArtifactManifestStore::qualified_for_algorithm_test`
pairs a synthetic qualification with a durability instance carrying the same
synthetic namespace identity through guard acquisition, exact-parent binding,
descendant-parent volume validation, manifest installation, and restart.

The synthetic identity is labeled algorithm evidence throughout the fixture.
It is not native NTFS durability evidence. Retained-file and snapshot-head
replacement checks remain exact in this seam through a cfg(test)-only safe
open-handle lock oracle. Cross-root qualification reuse is rejected before a
target is created.

Real Windows policy fixtures are deliberately separate. The manifest store's
test-platform constructor always carries `None` qualification. Recovery,
snapshot, first-create, and rename policy tests therefore continue to return
`NamespaceDurabilityUnsupported` before temp, head, source, or destination
mutation. `qualified_for_test` still requires real filesystem identity and
does not silently synthesize on Windows.

All synthetic variants, fields, constructors, and lock-oracle helpers are
cfg(test). The pinned Windows production rlib contains none of
`SyntheticAlgorithm`, `for_algorithm_test`, or
`qualified_for_algorithm_test`. There is no production qualification
constructor, unsafe code, dependency, manifest, workflow, dispatch, runtime
caller, generated file, frontend, or Seeds change.

## Native RED and TDD

Preliminary Blacksmith run `31901995995` supplied the native RED. Windows cloud
job `95054075141` finished with 1,618 passed, 15 failed, and 8 ignored; Windows
default job `95054075199` finished with 1,634 passed, 15 failed, and 8 ignored.
Both suites failed the same 15 `canonical_log` recovery tests while
`seed_recovery_manifest` called `qualified_for_test` and received
`Coordination(IdentityUnavailable)`. Linux and macOS native Rust/cloud jobs
passed, including the live macOS rsac job. This is a fixture portability gap,
not a production failure.

The agreed seams were the explicit algorithm-qualified manifest store and the
typed unqualified Windows refusal path. Before implementation, the focused
test `algorithm_test_qualification_is_explicit_and_cannot_cross_roots` failed
to compile:

```text
error[E0599]: no function or associated item named `for_algorithm_test_root`
error[E0599]: no function or associated item named `for_algorithm_test`
error: could not compile `audio-graph` (lib test) due to 2 previous errors
```

After the minimum seam existed, it passed 1/1 with 1,679 filtered tests. A
second tracer exposed that the synthetic open-existing path initially compared
an injected observation with a real Unix identity. The retained replacement
test failed before recovery with `Rejected(Durability(IdentityChanged))`.
Pairing both observations with the injected identity made the retained
replacement seam pass 1/1; the test then displaced the source and still
received `IdentityChanged` before destructive mutation through the open-handle
oracle.

The final injected-versus-real-refusal tracer passed 1/1:

```text
test persistence::session_artifact_manifest::tests::algorithm_qualification_and_windows_policy_refusal_are_explicitly_separate ... ok
test result: ok. 1 passed; 0 failed; 1680 filtered out
```

It proves two synthetic CAS generations are accepted, then independently
proves an actual `CanonicalPlatform::Windows` store has `None` qualification,
returns `NamespaceDurabilityUnsupported { Windows, AtomicSnapshotInstall }`,
and creates neither manifest head nor temp.

One safe Windows metadata probe confirmed why this work did not widen native
identity policy: stable Rust 1.95 rejects
`MetadataExt::volume_serial_number` and `file_index` with E0658
`windows_by_handle`. No unsafe API, nightly feature, or dependency was added.

## Focused GREEN gates

All Rust commands used Rust/Cargo 1.95.0, `--locked`, cloud-only features, and
the worktree-local `src-tauri/target`.

```text
canonical_log: 46 passed; 0 failed; 1635 filtered out; 2.22s
session_artifact_manifest: 19 passed; 0 failed; 1662 filtered out; 13.06s
canonical_durability: 41 passed; 0 failed; 1640 filtered out; 6.12s
```

The 41-test durability suite includes the unchanged c928 snapshot refusal and
67d3 Windows policy tests:

```text
snapshot_windows_and_unqualified_paths_refuse_before_temp_or_head_mutation ... ok
windows_policy_path_allows_existing_append_and_refuses_namespace_mutation ... ok
```

The canonical-log suite includes all 15 formerly failing recovery fixtures,
including cross-directory recovery, the ten restart cuts, partial writes,
manifest inner faults, retained-source substitution, collisions, contention,
content drift, prepared-manifest conflict, and inventory reservation. The
real Windows recovery policy test also passed with `None` qualification and
unchanged source/temp/final assertions.

The inherited Linux crash harness remained exact:

```text
running 11 tests
AUDIO_GRAPH_67D3_FIXTURE_FS_V1 platform=linux expected=ext4 observed=ext4 outcome=qualified ...
test result: ok. 11 passed; 0 failed; 0 ignored; 1670 filtered out; 7.66s
```

All b77b crash residuals, hashes, generations, fresh-process retries, and four
first-create cuts remained green. This preserves the Linux 11-cut proof. The
native macOS/APFS path remains `qualified_for_test` and was not changed or
relabelled as synthetic.

## Pinned Windows compile and policy evidence

An ignored dependency-minimal wrapper imported the actual current
`canonical_durability.rs`, `session_artifact_manifest.rs`, `canonical_log.rs`,
and `canonical_crash_harness.rs`. It reused already-built pinned Windows
dependency artifacts without creating or editing a Cargo manifest. Direct
Rust 1.95 compilation for `x86_64-pc-windows-msvc` produced:

```text
libnative_wrapper.rlib 6563504 bytes
canonical_native_tests.obj 6488104 bytes
```

The production rlib string gate passed: no synthetic identity or algorithm
constructor exists in the non-test artifact. The full cfg(test) object
contained all exact 15 formerly failing fixture symbols:

```text
fixture_symbol_count=15
```

It also contained the new cross-root and injected/refusal isolation tests, the
c928 snapshot refusal, the inherited Windows policy test, and
`windows_ntfs_namespace_paths_refuse_before_temp_head_or_source_mutation`.
That 67d3 native-Windows test retains exactly five typed
`NamespaceDurabilityUnsupported` assertions: first-create, ordinary rename,
recovery preflight, recovery rename, and atomic snapshot install. Object
presence is cross-compile evidence only; the native NTFS execution remains for
the integration Blacksmith rerun.

The Windows test object emitted only the inherited target-specific unused
`AtomicBool` and `thread` warnings. A first direct standalone test-object
attempt correctly failed because the module expects the crate's
`persistence::canonical_crash_harness`; the complete actual-module wrapper
above is the authoritative passing gate.

## Broad and repository gates

Locked check, strict Clippy, and rustfmt passed:

```text
cargo +1.95.0 check --locked ... --lib --tests --features cloud
Finished `dev` profile ... in 4m 18s

cargo +1.95.0 clippy --locked ... --lib --tests --features cloud -- -D warnings
Finished `dev` profile ... in 1m 17s

cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --all -- --check
exit 0
```

The final serialized locked cloud library snapshot passed after all source
corrections and static review:

```text
running 1681 tests
test result: ok. 1673 passed; 0 failed; 8 ignored; 0 measured; 0 filtered out; finished in 69.37s
```

The Linux host emitted the existing PipeWire/ALSA no-device diagnostics; they
did not fail the suite.

Pinned repository verification passed:

```text
SEEDS_CLI_ROOT=/home/codeseys/DevBox/audio-graph/node_modules/@os-eco/seeds-cli bun run verify:fast
bun run verify:contracts
```

Biome checked 174 files without fixes, TypeScript passed, all five generated
contracts were current, Seeds stdout stress parsed ready 50, blocked 97, and
list 50, docs/Seeds secret hygiene reported 0 findings, and `git diff --check`
passed.

## Security, cfg-only, runtime-dark, and footprint

Before the implementation commit:

```text
betterleaks scanned ~447843 bytes (447.84 KB): no leaks found
docs/Seeds secret hygiene scan passed: 0 findings
git diff --check: exit 0
unsafe_addition=none
production_cfg_only=pass
runtime_dark=pass
forbidden_scope=pass
footprint=pass
content_free_additions=pass
```

The first aggregate runtime-dark shell assertion produced a false failure
because its negative `rg --glob` expression did not exclude the three owned
paths. The corrected bounded assertion searched all `src-tauri/src` matches,
removed exactly the owned modules, found no residual caller, and passed. This
was a gate-script correction, not a code correction.

The implementation commit changes exactly:

```text
src-tauri/src/persistence/canonical_durability.rs
src-tauri/src/persistence/canonical_log.rs
src-tauri/src/persistence/session_artifact_manifest.rs
```

This report is the only additional branch path. There is no `.seeds`, Cargo
manifest/lockfile, workflow, generated contract, runtime command/caller,
frontend, Docker, Blacksmith dispatch, guest, dependency, or unsafe change.
The report-inclusive Betterleaks rerun scanned approximately 458.77 KB with no
leaks; docs/Seeds secret hygiene again reported 0 findings, and
`git diff --check` exited 0.

## Review, findings, and open questions

One bounded implementation critique corrected cfg shadowing so the direct
non-test Windows compile did not carry avoidable `unused_mut` warnings. The
post-correction production rlib, full Windows cfg(test) object, focused suites,
locked check, strict Clippy/fmt, contracts, static gates, and final serialized
library are all green. No second correction round was needed.

No production defect or scope blocker was found. The native Windows failures
were fixture-only, and the synthetic seam remains impossible to construct in
non-test builds. The only remaining evidence is the root-owned integration
rerun on native Windows/NTFS; this worker did not push, dispatch Blacksmith,
use Docker, or start a guest. Native macOS/APFS evidence remains unchanged.

Rollback before runtime adoption is reversal of the isolated implementation
and report commits, or disposal of this worktree branch. No on-disk migration
or recovery action is required because every added behavior is test-only.
