# Seed audio-graph-07cb macOS Keychain candidate state

Date: 2026-08-02

## Starting point

- Worktree: `/home/codeseys/DevBox/audio-graph/.worktrees/07cb-macos-keychain`
- Branch: `work/audio-graph-07cb-macos-keychain`
- Exact base: `8be073fecd8db650548c3d28734ebdebae26e379`
- Starting status: clean.
- Exclusive candidate footprint: this document plus
  `src-tauri/src/credentials/adapters/macos_keychain.rs` and
  `src-tauri/src/credentials/adapters/macos_keychain_ffi.rs`.
- Shared module, manifest, lockfile, factory, service/runtime/domain, frontend,
  CI, and Seeds changes are excluded. Compile-only assembly remains an
  uncommitted overlay and is restored byte-for-byte before the candidate.

## Acceptance boundary

The candidate owns a capability-blind, checked Security.framework FFI core and
an unsafe-free policy adapter loaded as a lexical child of
`native_interaction::explicit_recovery`. Ordinary work suppresses interaction,
restores the exact prior state on every exit after it is captured, and
preserves the frozen pre-/post-mutation uncertainty distinction. Explicit
recovery alone may make one unlock call, followed by separate prompt-forbidden
status verification.

## Verification state

### TDD packet

- The first guard test was run RED twice against the deliberately incomplete
  adapter. Both runs exited 101 with `guarded read: Internal`, `0 passed; 1
  failed; 1655 filtered out` (0.75s and 0.59s after the initial build).
- The expanded packet was also observed RED: 17 tests ran, 4 passed, and 13
  failed at the intentionally unimplemented status, mutation, delete,
  recovery, and raw-source lanes.
- The final focused packet passed from the stable target and again from a new
  clean target: `20 passed; 0 failed; 1655 filtered out`. The clean target was
  `/tmp/audio-graph-target-07cb-clean-qC0lyx`; its cold build took 5m21s and
  the tests took 0.18s.

The packet covers both exact prior interaction states; success, native error,
disable failure, checked restoration failure, operation unwind, and
restoration panic; contextual numeric status mapping; add-vs-update selection;
mutation-start timing and ambiguity; delete plus the existing separate
absence-readback seam; recovery cancellation, one-unlock behavior, independent
unlock/read/write status bits, and prompt-forbidden verification; and raw/safe
source inventories.

### Host gates with the hashed assembly overlay

- `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --all -- --check`:
  passed.
- `cargo +1.95.0 check --jobs 2 --manifest-path src-tauri/Cargo.toml --locked
  --no-default-features --features cloud --tests`: passed in 4m07s.
- `cargo +1.95.0 clippy --jobs 2 --manifest-path src-tauri/Cargo.toml --locked
  --no-default-features --features cloud --tests -- -D warnings`: passed in
  34.60s after correcting seven test-only initializer warnings.
- Locked no-dependency Cargo metadata: passed.
- Source gates: the safe production slice contains no `unsafe`; the raw core
  contains no prompt capability or recovery-boundary types; all required
  Security.framework symbols are present; prohibited native prose/log/debug
  conveniences are absent; native bytes are zeroized before checked
  `SecKeychainItemFreeContent`; and `git diff --check` passed.

The required compile overlay had this final recorded state before restoration:

- `Cargo.toml`: `79b574d3322d3f8374eee0657f5ddd5695c0ccaccef054fbb2fec49c0871ffac`
- `Cargo.lock`: `e3d308293416103fbb97bb7404d713a8a6c5447285934a09739706a315731c2d`
- `adapters/mod.rs`: `e89f38dbc5a7eb619b6376ac4af2c8fedf5555f92ed8469dd30b8196afaf8776`
- `adapters/native_interaction.rs`:
  `6e4b51aa2202654282906344f25614d494e3df1a5c264066c900fd774577eb31`
- combined binary diff:
  `d0fdcd3a1bd59ec6f5b9db3a6377b0591c7132338e6244498f1ce5e28c422466`

The first broad adapter run under the earlier `include!` form of that overlay
was truthfully `80 passed; 3 failed`: all three failures were the frozen 0350
source digest/inventory sentinels observing the temporary lexical assembly
text (`db1bb174cde7045c78360513aef2caf5e2be35ccad224be40c26512a6f6ce291`
instead of the reviewed source digest). All adapter behavior tests passed. This
overlay-only result is not represented as a clean inherited-gate pass.

### Clean shared-source proof

All four shared files were restored byte-for-byte before staging:

- `Cargo.toml`: `20372f4cd235801d532694221ae36c547e8824bb94ac422e4805ae30fac3e4a6`
- `Cargo.lock`: `78dc36dcc3c272028e4241a532724f6776ad9c26605da183007e7a40905ba966`
- `adapters/mod.rs`: `57cf44dfec8843d4b5b1c4987b02cec148af27465a0dba50f3203d722571c6d0`
- `adapters/native_interaction.rs`:
  `46631c5488f39ad3d4c488a6e19c43ce32ee090f4c5e51b97d2f959ab0b9b9fc`

After restoration, the complete clean `native_interaction::tests::` slice
passed: `18 passed; 0 failed; 1637 filtered out`. It includes the reviewed
digest, sealed prompt-policy inventory, and complete gate/lease inventory
sentinels.

Only `x86_64-unknown-linux-gnu` is installed locally. An explicit
`aarch64-apple-darwin` check exited 101 with `can't find crate for core`; no
target was installed as part of this bounded lane. Native target compilation
and packaged macOS Keychain runtime behavior therefore remain integration/CI
and release evidence.
