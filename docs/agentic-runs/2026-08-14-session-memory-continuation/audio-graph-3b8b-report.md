# audio-graph-3b8b locked canonical recovery report

Date: 2026-08-15

Seed: `audio-graph-3b8b`

Branch: `work/3b8b-locked-recovery-wave7b`

Worktree:
`/home/codeseys/DevBox/audio-graph/.worktrees/3b8b-locked-recovery-wave7b`

Exact base: `183f78a80546f2f7fa80c8394ede69d9d56e526b`

Implementation commit: `577c83c93372795692afe5f5c4caedb9574b834b`

## Outcome

Implemented one dormant `CanonicalRecoveryTransaction` deep module. It has no
runtime caller and does not change commands, state, repositories, Sessions,
scheduler, speech, providers, UI, generated contracts, dependencies,
workflows, Docker, Blacksmith, or Seeds state.

The public seam is deliberately small:

- one content-redacted `CanonicalRecoveryDescriptor` carries the Session,
  canonical stream contract, attempted-event recovery identity/fingerprint,
  and three stable managed identities;
- `CanonicalRecoveryTransaction::begin<T>` acquires the manifest write
  transaction, validates the exact tail basis through one retained source
  `File`, and refuses missing, contended, substituted, unqualified, Windows,
  or conflicting state before destructive mutation; and
- `execute` returns typed `Accepted`, `AlreadyCompleted`, `Rejected`, or
  `DurabilityIndeterminate` outcomes. Indeterminate outcomes retain the exact
  stage, OS error class/code, opaque recovery key, and recoverable residual.

The manifest write transaction remains the owner of the one stable
`CanonicalExclusiveGuard`. The recovery transaction retains that manifest
transaction and the exact open source `File` for its entire lifetime. The
durability substrate exposes only narrow crate-private helpers for recovery
namespace preflight, verified source open/revalidation, and retrying an
already-published parent barrier. The manifest kernel exposes only a narrow
crate-private borrow of its owned guard and qualification.

## Ordered protocol and restart contract

For a new repair, the implementation performs exactly:

1. acquire the stable canonical exclusive guard through
   `ManifestWriteTransaction`;
2. preflight supported qualified namespace mutation before a temp can exist;
3. open one regular source handle, validate the pathname identity, parse the
   structural canonical prefix, decode that prefix as `T`, and hash the full,
   retained, and exact tail bytes;
4. collision-refusing create of the stable quarantine temp, exact tail write,
   flush, and file `sync_all`;
5. guard-owned same-directory rename and qualified parent namespace barrier;
6. exact generation CAS of the typed manifest to `Prepared`;
7. only after `Prepared` is `Accepted`, revalidate the source pathname against
   the retained handle, truncate through that same handle, and source
   `sync_all`;
8. exact immutable generation CAS to `Completed`; and
9. only after `Completed` is `Accepted`, return acknowledgement.

The manifest is durable authority. The old free reader now ignores the legacy
repair selector and never mutates. Public appender open is also strict. The
legacy in-memory `take_quarantine_receipts` method is private to colocated
regression tests and cannot authorize production recovery.

Restart uses manifest transaction id, fingerprint, generation, exact source
before/after content identities, and exact quarantine identity. An exact
`Prepared` head resumes from source-full or source-truncated state without a
second quarantine or generation. An exact `Completed` retry returns
`AlreadyCompleted` at the existing generation. A different id or fingerprint
fails closed. The cooperative-process threat boundary is unchanged: an
uncooperative pathname replacement is detected before truncation, but this
module does not claim to fence writers that ignore the coordination lock.

## TDD evidence

The agreed test seam was the public descriptor plus the guard-owning recovery
transaction. Colocated tests observe only transaction outcomes, strict loads,
durable manifest reopen, source/quarantine bytes, and tree state.

### Public-seam RED note

The public-seam test was authored before the production types. The initial
cold Cargo command was:

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud public_recovery_transaction_seam_binds_one_descriptor_before_mutation -- --nocapture --test-threads=1
```

The cold dependency build outlived the tool response, and implementation began
before Cargo reached the crate, so the intended missing-symbol compiler output
was not captured. This is a process-evidence gap and is not represented as an
exact RED result. The first captured crate compile RED instead found one
missing return lifetime and four immutable/mutable borrow conflicts in the new
transaction; those were corrected before the first GREEN.

First captured GREEN for the public seam plus successful transaction:

```text
running 2 tests
test persistence::canonical_log::tests::public_recovery_transaction_seam_binds_one_descriptor_before_mutation ... ok
test persistence::canonical_log::tests::recovery_transaction_publishes_exact_tail_and_completes_manifest_before_ack ... ok
test result: ok. 2 passed; 0 failed; 0 ignored
```

### Final focused GREEN

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud 'persistence::canonical_log::tests' -- --nocapture --test-threads=1
```

Result: `37 passed; 0 failed; 0 ignored; 1620 filtered out`.

The suite covers strict free-reader zero-tree mutation; strict public appender
open; successful exact-tail transaction; coordination contention and missing
source; typed-prefix and source-content mismatch; same-content pathname
substitution against the held handle; exact quarantine bytes; temp/final
regular collisions; symlink and directory entries; reserved internal names;
unqualified and Windows-policy zero-mutation refusal; content-free
diagnostics; Prepared id/fingerprint conflicts; exact Completed retry; and the
ten deterministic fault cuts below.

The restart matrix injects one lost/failing boundary at each of:

- quarantine write;
- quarantine flush;
- quarantine file sync;
- quarantine rename;
- quarantine namespace sync;
- manifest Prepared CAS;
- source truncate;
- source sync;
- manifest Completed CAS; and
- acknowledgement.

Every case first returns `DurabilityIndeterminate`, preserves a source or exact
quarantine plus manifest state, then a fresh transaction converges to
`Accepted` or `AlreadyCompleted`. Each final manifest is generation 3 (seed,
Prepared, Completed), the source is the exact retained prefix, one exact final
quarantine exists, and no temp remains.

Manifest regression command result: `18 passed; 0 failed; 0 ignored`.

Canonical durability regression command result:
`38 passed; 0 failed; 0 ignored`.

## Broad gates and real results

All Rust gates used Rust/Cargo 1.95.0 and the stable worktree-local target
`src-tauri/target`.

### Locked check, Clippy, rustfmt, and full library

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 check --locked --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 clippy --locked --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud -- -D warnings
cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --all -- --check
```

Results: locked check passed; strict Clippy passed after two
`unnecessary_lazy_evaluations` nits were corrected; rustfmt passed.

One serialized full locked cloud library suite:

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --quiet --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud -- --test-threads=1
```

Result:

```text
running 1657 tests
test result: ok. 1649 passed; 0 failed; 8 ignored; 0 measured; 0 filtered out; finished in 50.68s
```

The Linux host emitted expected PipeWire/ALSA no-device diagnostics during
existing audio tests; they did not fail the suite.

### Windows compile evidence

A dependency-minimal ignored probe imported the actual production
`canonical_durability.rs`, `session_artifact_manifest.rs`, and
`canonical_log.rs` modules and compiled them for the installed
`x86_64-pc-windows-msvc` target:

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target/3b8b-windows-probe/target" cargo +1.95.0 check --locked --offline --manifest-path src-tauri/target/3b8b-windows-probe/Cargo.toml --target x86_64-pc-windows-msvc
```

Result: exit 0; `Finished dev profile`.

Direct `rustc +1.95.0 --test --emit=obj` compiled the same actual modules,
including `cfg(test)`, to
`src-tauri/target/3b8b-windows-probe/proof/audio_graph_3b8b_tests.obj`.
Result: exit 0 with only expected unused/dead-code warnings from inherited
Windows-gated test helpers. A Cargo test-link attempt stopped at missing MSVC
`link.exe`; it is not counted as a product failure or native Windows runtime
evidence. Native Windows and APFS execution remain `audio-graph-2df3`.

### Contracts and security

```text
SEEDS_CLI_ROOT=/home/codeseys/DevBox/audio-graph/node_modules/@os-eco/seeds-cli bun run verify:fast
```

Result: exit 0. Biome checked 174 files; TypeScript and all five generated
contract checks passed; Seeds JSON stress parsed ready 50, blocked 97, and list
50; docs/Seeds secret hygiene and diff checks passed.

```text
betterleaks dir --no-banner --redact src-tauri/src/persistence/canonical_log.rs src-tauri/src/persistence/canonical_durability.rs src-tauri/src/persistence/session_artifact_manifest.rs
bun scripts/check-docs-secret-hygiene.mjs
git diff --check
```

Results: Betterleaks found no leaks in approximately 402.70 KB; docs/Seeds secret
hygiene found 0 findings; diff check passed.

## Footprint, rollback, and remaining ownership

Implementation footprint from the exact base is only:

```text
src-tauri/src/persistence/canonical_durability.rs
src-tauri/src/persistence/canonical_log.rs
src-tauri/src/persistence/session_artifact_manifest.rs
```

Search found no non-test recovery transaction or descriptor caller. No
`.seeds` state was edited in this worktree. No runtime writer, command,
repository, Session, scheduler, or UI is activated. No push, merge, workflow,
Docker, Blacksmith, or guest action was run.

Rollback before runtime adoption is reversal of the isolated implementation
commit or disposal of this branch. No product data migration or reconciliation
is needed because the transaction remains dormant.

Remaining ownership is explicit:

- `audio-graph-90f3` and `audio-graph-3b48` persist and bind attempted-event,
  lease, and scheduler recovery identity before any runtime adoption;
- `audio-graph-b77b` owns subprocess kill/reopen proof and does not need a
  second recovery interface;
- `audio-graph-2df3` owns native Windows, macOS/APFS, and Linux filesystem
  qualification evidence; and
- broad manifest consumer/delete/export/purge adoption remains outside this
  Seed.

No durability or manifest interface blocker remains for those successor
workstreams. The only evidence caveat is the uncaptured intended missing-symbol
RED described above; all behavior and broad gates are captured and passing.
