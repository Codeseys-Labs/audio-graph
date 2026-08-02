# Seed audio-graph-12c4 commit state

Captured: 2026-08-02T10:36:52-07:00

## Assignment and baseline

- Seed: `audio-graph-12c4` — implement the direct, noninteractive Windows Credential Manager v2 boundary.
- Acceptance exercised here: exact `CredReadW` / `CredWriteW` / `CredDeleteW` raw surface with `CredFree`; no CredUI/retry path; Generic credentials with Local persistence and zero flags; exact target/blob bounds; immediate closed status mapping; native buffer scrubbing/freeing; one-call replace/delete/readback behavior; content-free failures; sealed prompt-policy traces; noninteractive recovery diagnosis; MSVC compile evidence.
- Accepted base and pre-commit HEAD: `8be073fecd8db650548c3d28734ebdebae26e379`.
- Branch: `work/audio-graph-12c4-windows-credential-manager`.
- Worktree: `/home/codeseys/DevBox/audio-graph/.worktrees/12c4-windows-credential-manager`.
- The worktree began clean. The main checkout's unrelated dirty state was not touched.
- No Seed, lockfile, dispatcher, factory, service, generated contract, frontend, or CI file is in this commit.

## Candidate footprint

- `src-tauri/src/credentials/adapters/windows_credential_manager_ffi.rs`
  - Capability-blind raw core over only `CredReadW`, `CredWriteW`, `CredDeleteW`, `CredFree`, and immediate `GetLastError` capture.
  - Validated, owned UTF-16 targets and `Zeroizing` binary write buffers.
  - Closed operation-specific status maps: reads fail unknown as `Internal`; mutations fail unknown as `CommitUnknown`.
  - RAII native-read guard scrubs the returned credential blob before exactly one free on success, validation error, and unwind.
- `src-tauri/src/credentials/adapters/windows_credential_manager.rs`
  - Unsafe-free ordinary/recovery policy adapter intended to load lexically under `native_interaction::explicit_recovery`.
  - Requires sealed `ForbidPrompt` for all ordinary and verification calls.
  - Prepares before the mutation cut, marks immediately before one raw mutation, and performs no implicit retry or read.
  - Recovery diagnosis makes zero native calls; verification makes exactly one authority read and treats found/missing as ready.
  - Source inventories prove the raw symbol/type surface remains confined to the two owned files and the policy file contains no unsafe/raw calls.
- This commit-state document.

Final pre-document source hashes:

```text
4984b630cdcac50752e0e2efcbea3d9db738ea6b75aeec1c11b69e2838803277  src-tauri/src/credentials/adapters/windows_credential_manager.rs
7d0e4a049afdc1398fdd321ef798797b6a1ad2d3bab172939ff793885fb0869f  src-tauri/src/credentials/adapters/windows_credential_manager_ffi.rs
```

## TDD evidence

The first command attempted from the repository root selected an older ambient Rust and stopped on MSRV before compiling the test. It is not counted as the RED. Commands were corrected to run from `src-tauri` with the repository's Rust 1.95 toolchain.

Canonical first RED:

```text
cargo test --locked --lib --no-default-features --features cloud \
  ordinary_read_maps_unknown_win32_failure_to_internal_once_without_prose \
  -- --nocapture --test-threads=1

left: Err(Unavailable)
right: Err(Internal)
test result: FAILED. 0 passed; 1 failed; 1655 filtered out
```

The test also asserted exactly one raw read and zero writes/deletes. The minimal mapper split produced the first GREEN:

```text
test result: ok. 1 passed; 0 failed; 1655 filtered out
```

Subsequent RED/GREEN slices:

- Per-operation numeric status matrix: unresolved write/delete mapping at RED; 1/1 GREEN after closed mappers were added.
- UTF-16 and binary bounds: missing request preparation at RED; 32,767 UTF-16 units and 2,560 bytes inclusive GREEN, with 32,768/2,561, empty, and embedded-NUL rejection.
- Replace/readback: missing memory-store seam at RED; one write, zero implicit reads, then one exact explicit readback GREEN.
- Mutation uncertainty: missing failing-mutation seam at RED; unknown write/delete each invoked once, returned `CommitUnknown`, and made later gate acquisition `StalledWorker` GREEN.
- Recovery: missing `RecoveryBoundary` implementation at RED; pre-cancel zero calls plus zero-call diagnosis and one authority verification read GREEN.
- Raw surface: missing direct WinCred symbols at RED; exact Generic/Local/zero-flag surface and immediate `GetLastError` branches GREEN.
- Native read ownership: missing RAII buffer seam at RED; scrub-before-one-free on success, validation error, and unwind GREEN.
- A strict-clippy RED later identified only a collapsible nested scrub guard. The minimal let-chain refactor retained the RAII tests and made strict clippy GREEN.

## Gate evidence with assembly overlay present

All host tests used `CARGO_TARGET_DIR=/tmp/audio-graph-target-12c4`, `--locked`, `--no-default-features --features cloud`, and one test thread where applicable.

- `cargo +1.95.0 fmt --all --check`: PASS, no output.
- Focused Windows modules: PASS — `12 passed; 0 failed; 0 ignored; 1655 filtered out`.
- Final sole-caller source inventory after strengthening raw-symbol confinement: PASS — `1 passed; 0 failed; 1666 filtered out`.
- Existing sealed 0350 prompt-policy inventory: PASS — `1 passed; 0 failed; 1666 filtered out`.
- Existing `native_interaction` slice: PASS — `18 passed; 0 failed; 1649 filtered out`.
- Full adapters slice: PASS — `75 passed; 0 failed; 1592 filtered out`.
- Full credentials slice, including `secret_canary_never_appears_in_safe_service_artifacts`: PASS — `243 passed; 0 failed; 1 ignored; 1423 filtered out`.
- `cargo check --locked --lib --tests --no-default-features --features cloud`: PASS — `Finished dev profile ... in 9.09s` on the final functional candidate before overlay restoration.
- `cargo clippy --locked --all-targets --no-default-features --features cloud -- -D warnings`: PASS — final rerun `Finished dev profile ... in 18.36s`.
- `cargo metadata --locked --no-deps --format-version 1 | jq -e '.packages | length > 0'`: PASS — `true`.
- `cargo audit`: PASS under repository configuration — `2 allowed warnings found` (`atomic-polyfill` and `bincode`, both pre-existing transitive unmaintained warnings).
- `bun run check:credential-contract`: PASS — `credential contract is current`.
- `git diff --check`: PASS.

The panic-hook lines emitted by RAII and existing gate tests contained only explicit non-secret canary prose. The tests caught the unwinds and passed.

## MSVC evidence and remaining native proof

A temporary ignored harness included the exact two owned sources beneath stubs matching their production lexical hierarchy and enabled `Win32_Security_Credentials`. It did not copy or rewrite implementation code.

```text
CARGO_TARGET_DIR=/tmp/audio-graph-target-12c4-cross-harness \
  cargo +1.95.0 check --locked --target x86_64-pc-windows-msvc

Checking audio-graph-12c4-wincred-cross v0.0.0 (.../target/12c4-wincred-cross)
Finished dev profile [unoptimized + debuginfo] target(s) in 0.15s
```

The full project cross command was also attempted:

```text
CARGO_TARGET_DIR=/tmp/audio-graph-target-12c4-windows \
  cargo check --locked --target x86_64-pc-windows-msvc \
  --lib --tests --no-default-features --features cloud
```

It stopped in the transitive `ring` build before AudioGraph compilation because this Linux host does not provide the MSVC librarian: `failed to find tool "lib.exe"`. This is a platform-tooling limitation, not a target-source diagnostic.

Still mandatory in Blacksmith Windows CI after 2aa8 assembly:

- Packaged Windows MSVC focused and full single-thread tests.
- Strict Windows clippy.
- Embedded-manifest target check.
- A real Windows replace/readback/delete smoke test if the CI credential isolation policy permits it.

## Temporary overlay custody and restoration

The compile overlay added only the Windows Credentials feature, the raw sibling declaration, and a lexical child declaration under `explicit_recovery`. The existing 0350 inventory correctly rejected an `include!` item macro, so the passing test overlay used `#[path = "../../windows_credential_manager.rs"]` plus a temporary empty synthetic directory chain. The directory and ignored harness were removed.

Passing overlay hashes:

```text
5853194ba8d675275d39c1191ea4fdafaa9c15ef53c70ef9bc23297a289f9e13  src-tauri/Cargo.toml
78dc36dcc3c272028e4241a532724f6776ad9c26605da183007e7a40905ba966  src-tauri/Cargo.lock
4a610873afbf1f7b9ba49ad73c0d2c7d0c35a54df92ffdcfd46bc9dc05943545  src-tauri/src/credentials/adapters/mod.rs
829bcb7a27b0732695ed1510b39b132f9ce2d9be02d27b1be0da2a0119ddcffd  src-tauri/src/credentials/adapters/native_interaction.rs
```

Harness hashes before deletion:

```text
75ef9ea84fe3f058f6b9104f1d74645f31bb7cef5d2f90a099fc89234c01244b  Cargo.toml
0a6be4b64fd63108b175de046a205d40a350c95171aff6510c290a7eed93701a  Cargo.lock
291955d5f7c195b660071e1067df13b0f2692a44f147e6adaf82a786f677fb2e  src/lib.rs
```

Restored shared-file hashes, exactly matching the pre-work values:

```text
20372f4cd235801d532694221ae36c547e8824bb94ac422e4805ae30fac3e4a6  src-tauri/Cargo.toml
78dc36dcc3c272028e4241a532724f6776ad9c26605da183007e7a40905ba966  src-tauri/Cargo.lock
57cf44dfec8843d4b5b1c4987b02cec148af27465a0dba50f3203d722571c6d0  src-tauri/src/credentials/adapters/mod.rs
46631c5488f39ad3d4c488a6e19c43ce32ee090f4c5e51b97d2f959ab0b9b9fc  src-tauri/src/credentials/adapters/native_interaction.rs
```

## Assembly handoff and findings

- Seed `audio-graph-2aa8` owns the final Cargo feature, raw module declaration, lexical-child load, parent-visible factory wrappers, Windows selection, source-seal digest update, and lockfile verification.
- A portable lexical `#[path]` load from the inline recovery module requires the synthetic `native_interaction/explicit_recovery` directory to exist in a fresh checkout. Assembly must track that directory (for example with an intentional assembly marker) or place the policy source at the module's natural nested path. It must not weaken the 0350 item-position-macro inventory.
- `Cargo.lock` did not change when `Win32_Security_Credentials` was enabled.
- No unrelated defect was found or fixed.
- Seed queue changes, integration, push, sync, and native Windows CI are intentionally left to the conductor/integrator.
