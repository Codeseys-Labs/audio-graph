# audio-graph-cc9a exact macOS mount resolver report

Date: 2026-08-16 (America/Los_Angeles)

## Custody and scope

- Seed: `audio-graph-cc9a`.
- Worktree: `/home/codeseys/DevBox/audio-graph/.worktrees/cc9a-exact-mount-wave7c`.
- Branch: `work/audio-graph-cc9a-exact-mount-wave7c`.
- Exact base and merge-base: `dfd7ce0c5428c8eade75c2864efd1107d1b6ff05`.
- Rust/Cargo: `1.95.0`; Bun: `1.3.14`.
- No push, dispatch, merge, GitHub mutation, Seed mutation, frontend change,
  generated-file change, other-workflow change, or ref deletion was performed.

The bounded acceptance was to replace macOS `st_dev` mount selection with a
safe fd-bound exact live-mount join, retain Linux and unsupported-platform
behavior, preserve all native test names/counts, and strengthen the native
evidence gate without claiming macOS acceptance before a new native run.

## Governing evidence and decision

Native run `31987754437` at `ca356409a6ac756fb17fa1e98f2f63eb609e2fbd`
proved that `/` and `/System/Volumes/Data` both had
`st_dev/root_dev=16777227`; `same_root_dev_count=2`, so the old selector
correctly refused as ambiguous.

The governing research artifact was
`/tmp/audio-graph-artifacts/2026-08-16/cc9a-macos-exact-mount-api-research.md`,
SHA-256
`cd75aee3ba5f7126350d3904785a5366ac5a064299ecac321a946d5933edb84d`.
Its selected boundary was implemented: macOS-target-only direct `nix 0.31.3`
with feature `fs`, using safe `nix::sys::statfs::fstatfs` on retained directory
handles and typed `filesystem_id()` equality. No Rustix mounted-on matching,
mount text parsing, `st_dev` fallback, write probe, hardcoded firmlink, or
application-level safety escape hatch was added.

## Implementation

`CanonicalFilesystemQualification::for_existing_managed_root` and
`CanonicalDurability::validate_guard_binding` now call one shared private live
resolver.

On macOS the resolver:

1. opens and retains the canonical managed-root directory;
2. requires its handle metadata to match the loaded namespace directory
   identity;
3. captures the root's fd-bound live filesystem ID;
4. refreshes `sysinfo::Disks`, opens every mountpoint, checks handle metadata,
   and calls safe `fstatfs` for every candidate, refusing the whole inventory
   on any probe error;
5. requires exactly one candidate with the same live filesystem ID;
6. applies the existing APFS, writable, and non-removable policy to that exact
   record;
7. rechecks the retained root ID, retained handle identity, freshly loaded
   canonical pathname identity, freshly opened pathname handle identity, and
   live filesystem ID; and
8. stores the exact private live-mount identity in the qualification binding,
   so every guard acquisition must rederive and equal it before coordination
   open/create.

Linux retains longest lexical mount selection plus ext4 policy and volume
validation. Windows and Other retain typed refusal before inventory or
mutation.

The lockfile changed only the root `audio-graph` dependency list. The existing
single `nix 0.31.3` package node, version, and checksum are unchanged.

## Commit series

Commits were intentionally separate and were not amended:

```text
fc87c4d1a8ef5440256f7b2c7ac9b0f1958c65c7 audio-graph-cc9a add macOS fs identity dependency
2bba11cf1efd2ab26ddf961eb923dd4980ec03fc audio-graph-cc9a bind macOS qualification to exact mount
f5e6eb77a036247f6901591023da4a865cf70ba4 audio-graph-cc9a gate macOS exact mount evidence
cc25d85eb03947e5b689000c265e2e812bce9c2e audio-graph-cc9a harden exact mount evidence checker
700556846617368a3494547f34184cb9d8d73f27 checker evidence follow-up
```

This report is the separate final commit after the report-inclusive gates.

## TDD RED and GREEN

The existing pure test
`macos_volume_group_selection_binds_logical_root_to_unique_data_volume` was
modified without adding or renaming a counted test. Its System and Data
observations share synthetic `st_dev=42`, have distinct live IDs `7` and `42`,
and the root live ID is `42`.

RED against the old `st_dev` selector:

```text
left: Err(IdentityUnavailable)
right: Ok(QualifiedFilesystemMount { mount_point: "/System/Volumes/Data",
class: Apfs, live_mount: Some(Synthetic(42)) })
test result: FAILED. 0 passed; 1 failed
```

GREEN after the exact selector:

```text
running 1 test
test ...macos_volume_group_selection_binds_logical_root_to_unique_data_volume ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 1704 filtered out
```

That same pure test now refuses zero and duplicate ID matches, any unavailable
candidate probe, read-only, removable, non-APFS, and before/after root-ID
change. It also proves that the root's lexical `/` relationship cannot override
the exact Data filesystem ID.

## Native diagnostic and workflow contract

The existing counted macOS diagnostic marker retains all prior fields and adds
content-safe fields only:

- per-root and per-observation probe availability;
- root/candidate filesystem-ID equality booleans;
- root equals Data;
- root differs from System;
- exact-match cardinality;
- candidate probe-unavailable cardinality;
- before/after root stability; and
- `selection_authority=fsid mounted_on_text_authority=false`.

The Unix summary permits macOS PASS only when the exact marker says:

```text
root_equals_data=true
root_differs_system=true
same_root_fsid_count=1
probe_unavailable_count=0
root_before_after_stable=true
selection_authority=fsid
mounted_on_text_authority=false
```

It still requires the prior diagnostic artifact, exact test names/counts, and
all existing mount/diskutil evidence. Runner, LABSN pin, license gate,
certificate restoration, and cleanup logic are unchanged.

The source-aware checker retains its deterministic prior-false-PASS simulation
and now guards the dependency boundary, shared fd resolver, candidate-probe
failure, before/after stability, pure refusal/masking fixture, exact diagnostic
schema, and exact Unix-summary PASS conditions:

```text
PASS: macOS summary simulation prior_false_pass=true corrected_failure_rejected=true full_good_pass=true
PASS: direct LABSN and cc9a native evidence contract with 56 mutations
```

## Local Rust gates

All commands used locked dependencies and cloud features where applicable.

Focused exact selector:

```text
test result: ok. 1 passed; 0 failed; 0 ignored; 1704 filtered out
```

Linux cc9a, canonical durability, and manifest filters:

```text
test result: ok. 3 passed; 0 failed; 0 ignored; 1702 filtered out
test result: ok. 47 passed; 0 failed; 0 ignored; 1658 filtered out
test result: ok. 26 passed; 0 failed; 0 ignored; 1679 filtered out
```

Locked cloud `cargo check --lib --tests`, strict Clippy with `-D warnings`, and
`cargo fmt --all -- --check` all exited 0.

The final serialized full cloud library suite was the last Rust test run:

```text
running 1705 tests
test result: ok. 1697 passed; 0 failed; 8 ignored; 0 measured; 0 filtered out; finished in 54.52s
```

## Dependency and cross-target evidence

macOS target tree:

```text
nix v0.31.3
├── nix feature "default"
│   └── audio-graph v0.1.0-rc.1
└── nix feature "fs"
    └── audio-graph v0.1.0-rc.1
```

The locked Linux and Windows inverse trees both reported `warning: nothing to
print`; neither target has a direct or transitive `nix 0.31.3` edge.

Installed pinned targets are:

```text
x86_64-pc-windows-msvc
x86_64-unknown-linux-gnu
```

A worktree-local ignored minimal Cargo probe imported the actual
`canonical_durability.rs`. Both pinned Windows commands passed:

```text
cargo +1.95.0 check --locked --offline ... --target x86_64-pc-windows-msvc --lib
Finished `dev` profile ...
cargo +1.95.0 check --locked --offline ... --target x86_64-pc-windows-msvc --tests
Finished `dev` profile ...
```

Expected dead-code/unused warnings in the isolated module probe were nonfatal;
the repository strict Clippy gate is clean. A raw `rustc` attempt stopped on
the module's external `sysinfo` dependency and was not counted as evidence.

No Apple standard-library target is installed. Per instruction, none was
installed. `rustc --print cfg --target aarch64-apple-darwin` confirmed
`target_os="macos"`, but no Apple actual-module compile is claimed.

## Repository, workflow, and security gates

- `bun run typecheck`: exit 0.
- `bun run verify:contracts`: exit 0; all five generated contracts are current:
  audio source, provider registry, session data movement, endpoint credential
  routing, and speech span revision.
- `bun run verify:fast`: exit 0; Biome checked 174 files, the 56-mutation
  checker passed, Seeds JSON stress parsed ready 50 / blocked 94 / list 50,
  docs/Seeds secret hygiene found 0 findings, and diff hygiene passed.
- Repo-configured actionlint, `yq eval '.'`, Ruby safe YAML load, Node syntax,
  and all 7 independently extracted `shell: bash` bodies passed.
- Report-inclusive Betterleaks scanned 649.61 KB across the exact six paths and
  reported `no leaks found`; docs/Seeds secret hygiene reported 0 findings.

The exact final tracked footprint is six authorized paths:

```text
.github/workflows/2df3-native-durability.yml
docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-cc9a-native-evidence-report.md
scripts/check-2df3-labsn-action.mjs
src-tauri/Cargo.lock
src-tauri/Cargo.toml
src-tauri/src/persistence/canonical_durability.rs
```

There are no added Rust safety-escape tokens anywhere in the repository delta.
No call site outside the existing dormant canonical qualification module
changed, so the resolver is runtime-dark outside that already-defined
production seam.

## Findings and open questions

No unrelated repository issue was changed. The raw standalone Windows `rustc`
probe cannot resolve external crates; the accepted dependency-minimal Cargo
probe is the real production/test module compile evidence.

The only remaining acceptance question is native and intentionally unresolved:
does the implementation SHA produce a unique fd-derived Data match on the
Blacksmith macOS 15 runner while retaining exact macOS counts 3/3 cc9a, 17/17
canonical durability, and 11/11 crash harness? A conductor-authorized native
rerun and artifact/hash review is required.

**There is no native macOS acceptance claim in this report.** Until that rerun
passes the new exact relationship/cardinality gate, cc9a remains open for
native evidence. No remote run was dispatched from this workstream.

## 2026-08-16 macOS test-initializer correction

### Custody and bounded scope

- Seed: `audio-graph-cc9a`.
- Worktree: `/home/codeseys/DevBox/audio-graph/.worktrees/cc9a-macos-test-initializer-wave7c`.
- Branch: `work/audio-graph-cc9a-macos-test-initializer-wave7c`.
- Exact base and merge-base: `dca04b322912a95e815b381a45641aaa9d319196`.
- Source/checker commit: `f63f43e82be82fa1b8e77e97b677b7d3a8469900`
  (`fix(audio-graph-cc9a): initialize macOS test volume`).

The native macOS job in GitHub run `31991181344`, job `95274932231`, stopped
at the compiler before any cc9a, canonical-durability, or crash tests could
run. Its retained RED is:

```text
error[E0063]: missing field `volume` in initializer of `FilesystemObservation`
  --> src-tauri/src/persistence/canonical_durability.rs:2551
```

The macOS-only exact-mount resolver was constructing a test-build
`FilesystemObservation` without its test-only `volume` field. The correction
derives `volume` from the already-opened mount metadata with
`filesystem_identity(&metadata).volume` and supplies it under `#[cfg(test)]`.
It changes no production field layout or resolver behavior.

### Test-first checker proof

The checker guard was added first. Against the untouched Rust source it failed
with the following deterministic RED:

```text
error: cc9a macOS filesystem observations must retain test-only volume identity
```

The guard locates `resolve_exact_macos_mount`, requires every contained
`FilesystemObservation` initializer to retain the cfg(test) `volume` field,
and requires the value to derive from `filesystem_identity(&metadata).volume`.
Its mutation removes that field from the macOS observation and is rejected.

After the source correction:

```text
PASS: macOS summary simulation prior_false_pass=true corrected_failure_rejected=true full_good_pass=true
PASS: direct LABSN and cc9a native evidence contract with 57 mutations
```

### Local gates

All Rust commands below used locked dependencies and the cloud feature set:

```text
cargo test ... cc9a_native_:              3 passed, 0 failed
cargo test ... canonical_durability:      47 passed, 0 failed
cargo test ... session_artifact_manifest: 26 passed, 0 failed
cargo check --locked --lib --tests:        exit 0
cargo clippy --locked --lib --tests -- -D warnings: exit 0
cargo fmt --all -- --check:                exit 0
bun run verify:contracts:                  exit 0
```

`bun run verify:fast` reached and passed Biome (174 files), TypeScript,
all five generated-contract checks, and the 57-mutation checker. It then
failed at the Seeds JSON stress check because the global package outside this
worktree lacks the required stdout patch:

```text
Seeds CLI outputJson does not have the pipe-safe stdout retry patch:
/home/codeseys/.bun/install/global/node_modules/@os-eco/seeds-cli/src/output.ts
```

No global package, workflow, dependency, Seed, or other-worktree file was
modified to work around that environmental gate. This correction has no new
native macOS acceptance claim; the conductor must rerun and inspect the
authorized native job before cc9a is considered accepted.

## 2026-08-16 archived macOS evidence parser correction

### Custody and bounded scope

- Seed: `audio-graph-cc9a`.
- Worktree:
  `/home/codeseys/DevBox/audio-graph/.worktrees/cc9a-macos-evidence-parser-wave7c`.
- Branch: `work/audio-graph-cc9a-macos-evidence-parser-wave7c`.
- Exact base and merge-base: `b906ab9a2b13e82142188223ee0cf95b6c985fe7`.
- Workflow/checker commit: `d412f225b30a5f4afdcaffe493333d94c12c9cb2`
  (`fix(audio-graph-cc9a): parse inline macOS evidence`).

The authorized scope was only the Unix cc9a evidence parser, its source-aware
checker, and this report. No Rust product source, dependency, Seed, GitHub
state, other workflow, or other worktree was changed. No push, dispatch,
merge, PR mutation, or cleanup was performed.

### Archived native evidence and cause

GitHub run `31992425071`, macOS job `95278220020`, is terminal and archived at
`/tmp/audio-graph-cc9a-macos-31992425071/files`. The native commands all
exited 0 and retained exact counts:

```text
cc9a native:              3/3
canonical durability:   17/17
canonical crash harness: 11/11
```

The exact fsid relationship marker is present and says
`root_equals_data=true`, `root_differs_system=true`,
`same_root_fsid_count=1`, `probe_unavailable_count=0`,
`root_before_after_stable=true`, `selection_authority=fsid`, and
`mounted_on_text_authority=false`.

The job summary nevertheless failed closed. Rust printed the first diagnostic
inline after the counted canonical test name, then printed the test's result
as a standalone line after the diagnostic sequence:

```text
test persistence::canonical_durability::tests::cc9a_native_qualified_guard_refuses_recreated_root_before_coordination_mutation ... CC9A_MACOS_DIAGNOSTIC root ...
CC9A_MACOS_DIAGNOSTIC observation ...
CC9A_MACOS_DIAGNOSTIC observation ...
CC9A_MACOS_DIAGNOSTIC summary ...
CC9A_MACOS_DIAGNOSTIC exact ...
ok
```

The old anchored grep retained only the four column-1 diagnostic lines and
omitted the root line. The old name awk required `ok` on the test-name line,
so it retained only two of the three canonical names. The resulting archived
summary correctly remained `status=FAIL`, with
`cc9a_native_name_markers=FAIL`,
`cc9a_macos_diagnostics_complete=false`, and
`cc9a_macos_exact_mount_identity=false`.

### Test-first correction

The checker received the exact live inline/split log shape before the workflow
changed. Against the old parser it produced the required RED:

```text
Error: Unix cc9a exit or split-diagnostic exact-name capture drift
```

The Unix parser now uses a bounded awk state machine. Ordinary tests are
accepted only from an exact `test ... ... ok` line. A diagnostic-bearing test
name remains pending only across `CC9A_MACOS_DIAGNOSTIC` lines and is emitted
only when the sequence terminates in a standalone `ok`; `FAILED` and unrelated
lines clear the pending name. Diagnostic extraction admits a marker only at
column 1 or immediately after a valid cc9a test prefix, and writes only the
marker substring. The existing exact name list, diagnostic schemas/counts,
exit checks, and fail-closed summary are unchanged.

The checker also mutates away inline-marker acceptance and mutates standalone
`ok` into a non-`ok` predicate. Both mutations are rejected. Its live-log
simulation proves the old 2-name/4-diagnostic failure, the corrected exact
3-name/5-diagnostic extraction, unrelated-marker rejection, and failure-result
rejection:

```text
PASS: macOS summary simulation prior_false_pass=true corrected_failure_rejected=true full_good_pass=true
PASS: cc9a live inline simulation anchored_root_omitted=true split_name_omitted=true corrected_exact=true failed_rejected=true
PASS: direct LABSN and cc9a native evidence contract with 59 mutations
```

### Gates and results

The corrected workflow parser was also run directly against the archived
`cc9a_native.log`:

```text
PASS: live run 31992425071 parser names=3 diagnostics=5 root=1 observations=2 summary=1 exact=1 failed_split_rejected=true
```

The remaining workflow/checker gates all exited 0:

```text
PASS: checker and mutations
PASS: actionlint
PASS: yq YAML parse
PASS: Ruby YAML safe_load
PASS: Node syntax
PASS: extracted 7 bash bodies
PASS: git diff --check
PASS: Betterleaks exact three-path scan (129.07 KB, no leaks found)
docs/Seeds secret hygiene scan passed: 0 findings
PASS: exact base-relative footprint (3 paths)
```

The final base-relative footprint is exactly the three authorized paths:

```text
.github/workflows/2df3-native-durability.yml
docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-cc9a-native-evidence-report.md
scripts/check-2df3-labsn-action.mjs
```

### Findings and open questions

No unrelated issue was changed. The absent worktree-local Biome binary was not
installed or worked around; repository Biome configuration includes TypeScript
sources only and excludes this checker, so Node syntax and the 59-mutation
checker are its applicable local gates.

This workstream validates the corrected parser against the terminal archived
native log but does not rewrite that immutable artifact or claim a new remote
workflow PASS. Integration and any conductor-authorized native rerun remain
outside this workstream.

## 2026-08-16 correction round 1: exact diagnostic cardinality

### Independent review BLOCKs

Both Spec and Standards review independently blocked the prior worktree tip
`e9163031017a0e25fbbf53b5bdcfa9b2edc01344`. The summary required one root,
one summary, one exact marker, and observation/schema counts equal to the
root-declared inventory, but it did not require exactly five extracted
diagnostic lines or exactly two inventory observations.

That left two fail-open shapes:

1. a sixth anchored `CC9A_MACOS_DIAGNOSTIC` line that matched no counted
   schema did not affect any comparison; and
2. `inventory_count=3` plus a third schema-valid observation satisfied both
   dynamic inventory comparisons.

Both artifacts therefore passed the prior completeness predicate even though
neither was the exact native evidence shape.

### RED and GREEN

The checker added executable forms of both reviewer findings before the YAML
changed. The first appends a sixth valid-prefix diagnostic. The second changes
the live root to `inventory_count=3` and inserts a third schema-valid APFS
observation while retaining a self-consistent summary and exact marker. Both
were accepted by the prior predicate. The checker-first RED against the
reviewed tip was:

```text
Error: macOS summary diagnostic completeness/fail-closed gate drift
```

The Unix summary now requires this complete exact tuple:

```text
diagnostic_total_count=5
diagnostic_inventory_count=2
diagnostic_observation_count=2
diagnostic_observation_schema_count=2
diagnostic_root_count=1
diagnostic_summary_count=1
diagnostic_exact_count=1
```

The source/checker correction is commit
`0324dd11554571091332798e7e500deffc373d0a`
(`fix(audio-graph-cc9a): enforce exact evidence counts`). Six new mutations
weaken the total, inventory, observation, observation-schema, root, and
summary comparisons; the existing exact-count mutation continues to protect
the seventh comparison. All are rejected.

GREEN checker output:

```text
PASS: macOS summary simulation prior_false_pass=true corrected_failure_rejected=true full_good_pass=true
PASS: cc9a live inline simulation anchored_root_omitted=true split_name_omitted=true corrected_exact=true failed_rejected=true
PASS: cc9a diagnostic cardinality prior_sixth_pass=true prior_inventory3_pass=true exact_5_2_2_2_1_1_1=true extras_rejected=true
PASS: direct LABSN and cc9a native evidence contract with 65 mutations
```

The archived run log was also exercised through the shell parsers and both old
and corrected summary predicates without rewriting its artifacts:

```text
PASS: archived live parser names=3 diagnostics=5 failed_split_rejected=true exact_5_2_2_2_1_1_1=true prior_sixth_pass=true prior_inventory3_pass=true extras_rejected=true
```

This preserves the exact three native test names, the five live diagnostics,
and standalone `FAILED` rejection while rejecting both review fixtures.

### Focused gates

The correction snapshot passed:

```text
PASS: checker and 65 mutations
PASS: actionlint
PASS: yq YAML parse
PASS: Ruby YAML safe_load
PASS: Node syntax
PASS: extracted 7 bash bodies
PASS: git diff --check
PASS: Betterleaks exact three-path scan (no leaks found)
docs/Seeds secret hygiene scan passed: 0 findings
PASS: exact base-relative footprint (3 paths)
```

### Scope, findings, and open questions

The base-relative footprint remains exactly the same three authorized paths.
No Rust product source, dependency, Seed, GitHub state, other workflow, or
other worktree changed. No push, dispatch, merge, PR mutation, or cleanup was
performed.

No additional issue was found. This correction hardens local evidence
acceptance only; integration and any conductor-authorized native rerun remain
outside this workstream.
