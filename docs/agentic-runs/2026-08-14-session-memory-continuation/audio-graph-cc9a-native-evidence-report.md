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
