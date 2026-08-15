# audio-graph-ce19 coordination-equivalence report

Date: 2026-08-15

Seed: `audio-graph-ce19`

Branch: `work/ce19-coordination-equivalence-wave7b`

Exact stacked base: `477df40a1660a807b94064569ee7f4e5f89dca6c`

Worktree:
`/home/codeseys/DevBox/audio-graph/.worktrees/ce19-coordination-equivalence-wave7b`

Implementation commit: `fa3619ea96a53fdc519be16ad256c33c49942f56`

## Outcome

The dormant canonical-durability module now reserves every ASCII-case spelling
of `.audio-graph-canonical.lock` on every platform. Append and rename preflight
their complete target sets before root validation, parent opening, entry
inspection, or mutation. A rename therefore rejects a reserved source or
destination before touching the other endpoint.

The internal name-equivalence interface deliberately exposes one conservative
floor: `AsciiCaseInsensitive`. This is volume-independent policy, not a claim
that platform defaults are uniform. It does not claim arbitrary Unicode,
trailing-character, short-name, or other filesystem equivalence.

The correction preserves c2e3's exact-parent binding, same-directory rename,
Windows existing-append versus namespace-mutation policy, opaque qualification,
content-free diagnostics with raw OS codes, and cooperative held-lock behavior.
No runtime caller was added.

## TDD evidence

The public test seam is `CanonicalExclusiveGuard::{append, rename}` plus the
coordination acquisition interface. The internal name-policy seam owns the
exhaustive spelling property.

### RED

Command:

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud mixed_case_coordination_aliases_are_reserved_before_filesystem_access -- --nocapture --test-threads=1
```

Real result: exit 101. The injected name-policy interface did not exist:

```text
error[E0599]: no function or associated item named `for_test_name_equivalence`
error[E0433]: use of undeclared type `ReservedInternalNameEquivalence`
```

### GREEN tracer

The same command passed after centralizing the ASCII-case-insensitive policy and
preflighting all operation endpoints:

```text
running 1 test
test persistence::canonical_durability::tests::mixed_case_coordination_aliases_are_reserved_before_filesystem_access ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 1608 filtered out
```

The fixture moves the managed root after the guard is acquired. Mixed-case
append, rename-source, and rename-destination aliases still return
`ReservedCoordinationEntry`, proving rejection precedes filesystem access. It
then proves source/lock bytes are unchanged, a second lock contends while the
original guard is held, and shared acquisition succeeds after release.

### Exhaustive ASCII-case property

Command:

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud reserved_name_policy_covers_every_ascii_case_permutation_only -- --nocapture --test-threads=1
```

Real result:

```text
test persistence::canonical_durability::tests::reserved_name_policy_covers_every_ascii_case_permutation_only ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 1609 filtered out; finished in 3.40s
```

The Gray-code property visits all `2^23 = 8,388,608` ASCII-case permutations
without per-case allocation. Distinct suffix and Unicode near-misses are also
asserted non-reserved, keeping the equivalence claim narrow.

## Gates and real results

All Rust gates used Rust/Cargo 1.95.0 and the stable worktree target
`src-tauri/target`.

### Focused serialized durability suite

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud canonical_durability -- --nocapture --test-threads=1
```

Result: `22 passed; 0 failed; 0 ignored; 1588 filtered out; finished in 3.47s`.

### Locked cloud check

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 check --locked --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud
```

Result: exit 0; `Finished dev profile ... in 2m 03s`.

### Strict Clippy and rustfmt

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 clippy --locked --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud -- -D warnings
cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --check
git diff --check
```

Results: Clippy exit 0 in 39.55s; rustfmt and diff checks exit 0 with no output.

### Pinned Windows actual-module compile

Installed pinned components included both
`rust-std-x86_64-pc-windows-msvc` and
`rust-std-x86_64-unknown-linux-gnu`.

```text
rustc +1.95.0 --edition=2024 --crate-type lib --target x86_64-pc-windows-msvc src-tauri/src/persistence/canonical_durability.rs --out-dir src-tauri/target/ce19-windows-module-proof
```

Result: exit 0; `libcanonical_durability.rlib` emitted at 570056 bytes. This is
an actual-module cross-compile, not native NTFS execution evidence.

### One full serialized locked cloud library suite

This ran once after the final production change:

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud -- --test-threads=1
```

Result: `1602 passed; 0 failed; 8 ignored; finished in 41.50s`.

### Pinned repository and contract verification

The worktree-local package root was absent. `SEEDS_CLI_ROOT` was pinned to the
existing custody package
`/home/codeseys/DevBox/audio-graph/node_modules/@os-eco/seeds-cli`, verified as
`@os-eco/seeds-cli@0.4.5`; no package or symlink was installed.

```text
SEEDS_CLI_ROOT=/home/codeseys/DevBox/audio-graph/node_modules/@os-eco/seeds-cli bun run verify:fast
bun run verify:contracts
```

Results: exit 0. Biome checked 174 files with no fixes, TypeScript passed, all
five generated contracts were current, Seeds JSON parsed (`ready 50`,
`blocked 103`, `list 50`), docs/Seeds secret hygiene reported 0 findings, and
`git diff --check` passed. The explicit contract rerun also reported all five
contracts current.

### Security and footprint

Before the implementation commit:

```text
bun scripts/check-docs-secret-hygiene.mjs
betterleaks dir --no-banner --redact src-tauri/src/persistence/canonical_durability.rs
```

Results: secret hygiene found 0 findings; Betterleaks scanned approximately
69.98 KB and found no leaks. The final report-inclusive scan and exact
base-to-tip footprint are recorded in the handoff after the report commit.

## Scope and rollback

Owned paths:

- `src-tauri/src/persistence/canonical_durability.rs`
- this report

There is no unsafe code, dependency or feature expansion, runtime activation,
workflow edit, generated artifact edit, or Seeds mutation. Rollback before
runtime adoption is reversal of the two isolated ce19 commits or disposal of
this worktree branch; no on-disk migration or reconciliation is required.

## Findings and open questions

- `audio-graph-83e2` remains unchanged and in scope for the next stacked
  successor: runtime `EXDEV` is still classified as
  `CrossDeviceRenameRefused` in the c2e3 base.
- Native macOS APFS and Windows NTFS evidence remains owned by
  `audio-graph-2df3`; this work supplies policy and pinned compile evidence
  only.
- No unrelated issue was changed. No Docker, Blacksmith, push, workflow,
  deployment, release, `sd update`, `sd close`, or `sd sync` action was run.
