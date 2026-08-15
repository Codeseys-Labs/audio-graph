# audio-graph-b77b canonical subprocess durability report

Date: 2026-08-15

Seed: `audio-graph-b77b`

Branch: `work/b77b-canonical-subprocess-wave7b`

Worktree:
`/home/codeseys/DevBox/audio-graph/.worktrees/b77b-canonical-subprocess-wave7b`

Exact base: `4a4288627b4d0ad4f9d02259548fa15921946139`

Implementation commit: `3a24248fec11d7638fdf552cf193bd71ebe8848a`

## Outcome and claim boundary

Implemented a Linux-only, non-ignored subprocess crash and cross-process
exclusion harness for the dormant canonical recovery transaction. The harness
self-spawns the current library test binary at the exact child entrypoint:

```text
current_exe
  --exact persistence::canonical_crash_harness::tests::subprocess_child_entrypoint
  --nocapture
  --test-threads=1
```

The parent and child communicate only with content-free, versioned
`AUDIO_GRAPH_B77B_HANDSHAKE_V1:<stage>` tokens. Every parent wait is bounded to
10 seconds, every checkpoint self-expires after 30 seconds, and `ManagedChild`
kills and waits in explicit and RAII cleanup paths so a failure cannot leave a
test child running.

All hooks and the harness are `cfg(test)` only. There is no production caller,
runtime writer activation, Session semantics change, UI change, dependency or
manifest change, generated-file change, workflow change, Docker or Blacksmith
action, platform guest, Seeds mutation, push, merge, or `sd sync`.

This is process-crash recovery and completed OS-barrier evidence on the named
Linux filesystem. It is not power-loss proof and does not claim to simulate
loss of the kernel page cache, device cache, firmware state, or hardware power.

## Host and filesystem evidence

```text
Linux DESKTOP-CP4EDJH 6.18.33.2-microsoft-standard-WSL2 #1 SMP PREEMPT_DYNAMIC Thu Jun 18 21:54:43 UTC 2026 x86_64 x86_64 x86_64 GNU/Linux
/ /dev/sdd ext4 rw,relatime,discard,errors=remount-ro,data=ordered
filesystem_type=ext2/ext3 block_size=4096 blocks=263940717 free_blocks=153146649 name_max=255
```

`findmnt` identifies the qualified worktree filesystem as local ext4 on
`/dev/sdd`. GNU `stat -f` reports the ext-family magic as `ext2/ext3`; the
`findmnt` result is the more specific mounted filesystem identity.

## TDD evidence

The agreed public seam is the self-spawn helper driving the exact lib-test
child entrypoint. The test and module declaration were authored before that
helper existed. The first crate compile produced the captured RED:

```text
error[E0425]: cannot find function `spawn_child` in module `super`
 --> src/persistence/canonical_crash_harness.rs:7:32
  |
7 |         let mut child = super::spawn_child("seam");
  |                                ^^^^^^^^^^^ not found in `super`

error: could not compile `audio-graph` (lib test) due to 1 previous error
```

The exact public seam then became GREEN:

```text
running 1 test
test persistence::canonical_crash_harness::tests::public_subprocess_harness_seam_self_spawns_exact_child ... ok
test result: ok. 1 passed; 0 failed; 0 ignored
```

The final focused harness command was:

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud 'persistence::canonical_crash_harness::tests' -- --nocapture --test-threads=1
```

Result: `7 passed; 0 failed; 0 ignored; 1668 filtered out` in 4.82 seconds.

## Crash-cut evidence

Each recovery case created a new managed root, seeded generation 1, spawned one
child, waited for the named handshake, killed and waited for that child with
signal 9, inspected the exact residual, spawned a fresh retry child, and then
spawned a separate fresh strict-reopen oracle. The oracle held a shared
coordination guard while reading canonical state.

Synthetic fixture identities are stable and content-free outside this test:

- full damaged source: length 35, SHA-256
  `6503f619b147e275fefa3f986547ac238a9cc155674e9488daedb3827e058e6a`;
- retained strict source: length 12, SHA-256
  `3a37782e8974c48eebf2a0517c866ad15641c53b3d31993188796b56aeb79624`;
- exact quarantine: length 23, SHA-256
  `fe27b7aee64f84a7f777321ff87134ca7f5027a1509ace85af41a1f3b343e2f1`;
  and
- empty created temp: length 0, SHA-256
  `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`.

| Kill checkpoint | Signal | Exact crash residual | Fresh retry | Fresh reopen and final evidence |
| --- | ---: | --- | --- | --- |
| `quarantine_create_before` | 9 | gen 1; source 35/full hash; no recovery entry | Accepted | pass; gen 3; source 12/retained hash; quarantine 23/exact hash; no temp |
| `quarantine_create_after` | 9 | gen 1; source 35/full hash; temp 0/empty hash | Accepted | pass; gen 3; source 12/retained hash; quarantine 23/exact hash; no temp |
| `quarantine_write_before` | 9 | gen 1; source 35/full hash; temp 0/empty hash | Accepted | pass; gen 3; source 12/retained hash; quarantine 23/exact hash; no temp |
| `quarantine_write_after` | 9 | gen 1; source 35/full hash; temp 23/exact hash | Accepted | pass; gen 3; source 12/retained hash; quarantine 23/exact hash; no temp |
| `quarantine_flush_before` | 9 | gen 1; source 35/full hash; temp 23/exact hash | Accepted | pass; gen 3; source 12/retained hash; quarantine 23/exact hash; no temp |
| `quarantine_flush_after` | 9 | gen 1; source 35/full hash; temp 23/exact hash | Accepted | pass; gen 3; source 12/retained hash; quarantine 23/exact hash; no temp |
| `quarantine_file_sync_before` | 9 | gen 1; source 35/full hash; temp 23/exact hash | Accepted | pass; gen 3; source 12/retained hash; quarantine 23/exact hash; no temp |
| `quarantine_file_sync_after` | 9 | gen 1; source 35/full hash; temp 23/exact hash | Accepted | pass; gen 3; source 12/retained hash; quarantine 23/exact hash; no temp |
| `quarantine_rename_before` | 9 | gen 1; source 35/full hash; temp 23/exact hash | Accepted | pass; gen 3; source 12/retained hash; quarantine 23/exact hash; no temp |
| `quarantine_rename_after` | 9 | gen 1; source 35/full hash; final 23/exact hash | Accepted | pass; gen 3; source 12/retained hash; quarantine 23/exact hash; no temp |
| `quarantine_parent_sync_before` | 9 | gen 1; source 35/full hash; final 23/exact hash | Accepted | pass; gen 3; source 12/retained hash; quarantine 23/exact hash; no temp |
| `quarantine_parent_sync_after` | 9 | gen 1; source 35/full hash; final 23/exact hash | Accepted | pass; gen 3; source 12/retained hash; quarantine 23/exact hash; no temp |
| `manifest_prepared_before` | 9 | gen 1; source 35/full hash; final 23/exact hash | Accepted | pass; gen 3; source 12/retained hash; quarantine 23/exact hash; no temp |
| `manifest_prepared_after` | 9 | gen 2 Prepared; source 35/full hash; final 23/exact hash | Accepted | pass; gen 3; source 12/retained hash; quarantine 23/exact hash; no temp |
| `source_truncate_before` | 9 | gen 2 Prepared; source 35/full hash; final 23/exact hash | Accepted | pass; gen 3; source 12/retained hash; quarantine 23/exact hash; no temp |
| `source_truncate_after` | 9 | gen 2 Prepared; source 12/retained hash; final 23/exact hash | Accepted | pass; gen 3; source 12/retained hash; quarantine 23/exact hash; no temp |
| `source_sync_before` | 9 | gen 2 Prepared; source 12/retained hash; final 23/exact hash | Accepted | pass; gen 3; source 12/retained hash; quarantine 23/exact hash; no temp |
| `source_sync_after` | 9 | gen 2 Prepared; source 12/retained hash; final 23/exact hash | Accepted | pass; gen 3; source 12/retained hash; quarantine 23/exact hash; no temp |
| `manifest_completed_before` | 9 | gen 2 Prepared; source 12/retained hash; final 23/exact hash | Accepted | pass; gen 3; source 12/retained hash; quarantine 23/exact hash; no temp |
| `manifest_completed_after` | 9 | gen 3 Completed; source 12/retained hash; final 23/exact hash | AlreadyCompleted | pass; same gen 3 identities; no temp |
| `acknowledgement_before` | 9 | gen 3 Completed; source 12/retained hash; final 23/exact hash | AlreadyCompleted | pass; same gen 3 identities; no temp |
| `acknowledgement_after` | 9 | gen 3 Completed after `execute` returned Accepted; source 12/retained hash; final 23/exact hash | AlreadyCompleted | pass; same gen 3 identities; no temp |

No real subprocess cut exposed a production recovery defect. Every cut retained
the full source or the exact published quarantine, and every fresh retry
converged idempotently without a duplicate quarantine, extra generation, or
leftover temp. No successor defect Seed is proposed.

### First-create file versus namespace barrier

The separate first-create matrix killed before and after file `sync_all` and
before and after parent-directory `sync_all`. Every child exited by signal 9;
fresh-process reopen found exactly one synthetic entry of length 18 and SHA-256
`bda22f24c7a63e7576093c8a478f7d5951b8f301cad73a599b3ac17acab43722`.

| Kill checkpoint | Signal | Fresh reopen | Exact entry evidence |
| --- | ---: | --- | --- |
| `first_create_file_sync_before` | 9 | pass | length 18; stable hash |
| `first_create_file_sync_after` | 9 | pass | length 18; stable hash |
| `first_create_parent_sync_before` | 9 | pass | length 18; stable hash |
| `first_create_parent_sync_after` | 9 | pass | length 18; stable hash |

The visible entry after process death does not upgrade the pre-parent-sync cuts
to namespace-durable or power-loss proof. The test establishes ordered and
separately reachable OS barrier boundaries.

## Cross-process exclusion and rename evidence

The focused suite proves:

- exclusive holder versus exclusive contender: `Contended`;
- exclusive holder versus shared contender: `Contended`;
- shared holder versus exclusive contender: `Contended`;
- shared holder versus shared contender: acquired;
- signal-9 death of both exclusive and shared holders releases the OS lock;
- a strict canonical reader takes the shared coordination guard and excludes
  an exclusive writer;
- a public data rename leaves the stable coordination-file identity locked,
  so another process remains contended until the original guard drops;
- an existing destination is `DestinationAlreadyExists` and a different
  directory is `TargetOutsideManagedNamespace`, both observed in a child
  process; and
- the advisory limitation is explicit: a raw uncooperative `fs::rename`
  succeeds while a cooperating reader holds a shared coordination guard. The
  reader's already-open file continues to read the old inode while the path
  names the replacement inode.

The last case is the accepted cooperative-process boundary, not a claim that
advisory locks fence uncooperative filesystem calls.

## Regression and broad gates

All Rust commands used Rust/Cargo 1.95.0, `--locked`, and the stable
worktree-local `src-tauri/target`.

Focused and persistence baselines:

```text
canonical crash harness: 7 passed; 0 failed
canonical_log baseline: 46 passed; 0 failed
session_artifact_manifest baseline: 18 passed; 0 failed
canonical_durability baseline: 40 passed; 0 failed
```

Locked cloud check:

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 check --locked --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud
Finished `dev` profile [unoptimized + debuginfo] target(s) in 2m 08s
```

Final serialized full locked cloud library:

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --quiet --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud -- --test-threads=1
running 1675 tests
test result: ok. 1667 passed; 0 failed; 8 ignored; 0 measured; 0 filtered out; finished in 57.00s
```

The Linux host emitted the existing PipeWire/ALSA no-device diagnostics; they
did not fail the suite.

Strict Clippy and rustfmt:

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 clippy --locked --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud -- -D warnings
cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --all -- --check
```

Result: both exit 0; final Clippy completed in 18.46 seconds.

The authoritative Seeds CLI root resolved without installation or symlink
creation:

```text
/home/codeseys/DevBox/audio-graph/node_modules/@os-eco/seeds-cli
@os-eco/seeds-cli@0.4.5
```

Pinned `SEEDS_CLI_ROOT=... bun run verify:fast` exited 0: Biome checked 174
files, TypeScript passed, all five generated contract checks passed, the Seeds
stdout stress check parsed ready 50, blocked 96, and list 50, docs/Seeds secret
hygiene reported 0 findings, and diff check passed. Explicit
`bun run verify:contracts` also exited 0 and reconfirmed audio-source,
provider-registry, session-data-movement, endpoint-credential-routing, and
speech-span generated contracts are current.

Initial implementation security and static gates:

```text
Betterleaks scanned ~626.72 KB: no leaks found
docs/Seeds secret hygiene scan passed: 0 findings
git diff --check: exit 0
```

Final code-plus-report Betterleaks scanned approximately 641 KB and found no
leaks. The final docs/Seeds secret hygiene scan again reported 0 findings, and
`git diff --check` exited 0.

Static searches showed one `cfg(test)` module declaration, every checkpoint
inside a `cfg(test)` statement or block, no recovery descriptor/transaction
caller outside `canonical_log.rs` and the test harness, and no changed path
outside the four owned Rust files before this report.

## Footprint, rollback, and remaining ownership

The implementation commit changes exactly:

```text
src-tauri/src/persistence/canonical_crash_harness.rs
src-tauri/src/persistence/canonical_durability.rs
src-tauri/src/persistence/canonical_log.rs
src-tauri/src/persistence/mod.rs
```

This report is the only additional branch path. The branch has no `.seeds`,
workflow, Cargo manifest or dependency, runtime command/caller, Session, UI,
generated, Docker, Blacksmith, or platform-qualification change.

Rollback before runtime adoption is reversal of the implementation commit and
the report commit, or disposal of this branch. The harness is non-production
and creates only temporary synthetic fixtures that its test guards remove.

Native macOS/APFS and Windows/NTFS execution remains `audio-graph-2df3`.
Runtime adoption remains owned by its existing downstream Seeds. The expected
uncooperative advisory-lock limitation remains explicit; there are no newly
discovered production defects or open questions requiring this worker to
expand scope.
