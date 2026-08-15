# audio-graph-942a RustSec remediation report

- Date: 2026-08-15
- Seed: `audio-graph-942a`
- Worktree: `/home/codeseys/DevBox/audio-graph/.worktrees/942a-cargo-audit-wave7b`
- Branch: `work/942a-cargo-audit-wave7b`
- Reviewed integration base: `9177799c5a8d40bbf06a3e4e0b8cfb37119153c1`
- Implementation commit: `3de20abdf3f5f20ba2626bb07506266db74a4649`

## Outcome

The application lockfile now resolves `ammonia` from vulnerable `4.1.3` to
fixed `4.1.4` through one targeted Cargo update. The lockfile otherwise remains
structurally unchanged: 1,186 packages before and after, with `surrealdb 3.2.0`,
`surrealdb-core 3.2.0`, `rust_decimal 1.42.1`, and resolver-retained
`rkyv 0.7.46` fixed in place.

One exact `RUSTSEC-2026-0235` exception was added for the inactive `rkyv 0.7`
edge. It is not a blanket advisory suppression: the configuration documents
the retaining crate and requirement, the current feature-resolution proof, the
risk boundary, and both activation and upstream-removal triggers. Locked
`cargo audit` now exits 0.

No source, `Cargo.toml`, generated contract, workflow, Seeds, storage-probe,
or persisted-data format changed. No migration is assumed or required.

## Primary sources and disposition

- [RustSec RUSTSEC-2026-0213](https://rustsec.org/advisories/RUSTSEC-2026-0213.html)
  identifies SVG animation URL sanitization as the `ammonia` defect and lists
  `>=4.1.4` as a patched line. AudioGraph moved exactly from `4.1.3` to `4.1.4`.
- [RustSec advisory-db RUSTSEC-2026-0235](https://github.com/RustSec/advisory-db/blob/main/crates/rkyv/RUSTSEC-2026-0235.md)
  documents insufficient shared-pointer metadata validation, safe checked
  access/deserialization exposure, and `>=0.8.17` as patched; it also states
  the affected 0.7 line is unsupported upstream.
- [cargo-audit `AdvisoryConfig`](https://docs.rs/cargo-audit/latest/cargo_audit/config/struct.AdvisoryConfig.html)
  defines `advisories.ignore` as an exact advisory-ID list. AudioGraph added
  only `RUSTSEC-2026-0235`, with its review evidence adjacent to the ID.

Local locked metadata is the authoritative dependency-provenance evidence for
this resolution. For `rust_decimal 1.42.1` it reports two `rkyv` declarations:

```text
rkyv ^0.7.46  optional=true  default-features=false  features=[size_32,std]
rkyv ^0.8.13  optional=false target=cfg(not(target_arch = "wasm32"))
```

The optional 0.7 declaration explains why Cargo retains `rkyv 0.7.46` in
`Cargo.lock`; feature-resolution probes below establish that it is not active.
Patched `rkyv 0.8.17` is semver-incompatible with `rust_decimal 1.42.1`'s
optional `^0.7.46` requirement, while manually pruning the 0.7 lock stanza is
resolver-unstable because Cargo retains or re-adds the declared optional edge.
Broadly regenerating the lockfile or upgrading SurrealDB would not preserve the
reviewed dependency contract and was intentionally not done. The independent
storage-probe lock graph remains owned by `audio-graph-c65d`.

## TDD audit seam: RED, narrower RED, GREEN

The test seam was cargo-audit's public exit status and diagnostic set against
the locked dependency graph.

1. Untouched base RED:

   ```text
   cargo audit
   Scanning Cargo.lock for vulnerabilities (1186 crate dependencies)
   RUSTSEC-2026-0213 ammonia 4.1.3
   RUSTSEC-2026-0235 rkyv 0.7.46
   error: 2 vulnerabilities found!
   warning: 4 allowed warnings found
   exit 1
   ```

2. Targeted dependency implementation:

   ```text
   cargo update -p ammonia@4.1.3 --precise 4.1.4
   Updating ammonia v4.1.3 -> v4.1.4
   note: 199 unchanged dependencies behind latest
   exit 0
   ```

3. Lock-update-only RED:

   ```text
   cargo audit
   Scanning Cargo.lock for vulnerabilities (1186 crate dependencies)
   RUSTSEC-2026-0235 rkyv 0.7.46
   error: 1 vulnerability found!
   warning: 4 allowed warnings found
   exit 1
   ```

   `RUSTSEC-2026-0213` was absent, proving the narrow lock change resolved the
   `ammonia` advisory before any policy exception was added.

4. Exact-policy GREEN:

   ```text
   cargo audit
   Scanning Cargo.lock for vulnerabilities (1186 crate dependencies)
   warning: 4 allowed warnings found
   exit 0
   ```

This is zero unignored vulnerabilities with one exact ignored inactive advisory,
`RUSTSEC-2026-0235`; it is not a claim that the retained advisory disappeared.

The four allowed warnings are custody-carried informational/yanked findings
already classified by the pre-existing audit policy. This work added no new
warning category or blanket filter.

## Lockfile and reachability invariants

The base-to-implementation lockfile diff contains only these two replacements:

```text
ammonia version:  4.1.3 -> 4.1.4
ammonia checksum: 68b9d3370580a12f4b7a10fdcc18b28942c083ba570e3d954fe59d10951b85a2
               -> dc6d763210e2eb7670d1a5183a08bebefa3f97db2a738a684f2ce00bd49f681d
```

Assertions:

| Invariant | Base | Candidate | Result |
| --- | ---: | ---: | --- |
| `Cargo.lock` package stanzas | 1,186 | 1,186 | unchanged |
| locked all-feature metadata packages | 1,186 | 1,186 | unchanged |
| `surrealdb` | 3.2.0 | 3.2.0 | unchanged |
| `surrealdb-core` | 3.2.0 | 3.2.0 | unchanged |
| `rust_decimal` | 1.42.1 | 1.42.1 | unchanged |
| `rkyv` retained node | 0.7.46 | 0.7.46 | unchanged |

Exact required all-feature reachability probe:

```text
cargo tree --locked --offline -i rkyv@0.7.46 --target all --all-features --edges all
warning: nothing to print.
exit 0
```

Default and cloud-only inverse probes both exit 101 with `package ID
specification rkyv@0.7.46 did not match any packages`, which is expected proof
that those resolved graphs do not contain the package at all. The all-feature
probe is the stronger acceptance gate: Cargo recognizes the resolver-retained
node but finds no active reverse dependency across all targets and edge kinds.

## Risk acceptance and removal trigger

The accepted residual risk is limited to a lockfile-scanner finding for an
inactive optional dependency. No current default, cloud-only, or all-feature
AudioGraph build activates `rkyv 0.7`; therefore its affected checked archive
access and deserialization APIs are not compiled into a current AudioGraph
artifact.

The exception must be removed immediately if the exact all-feature inverse-tree
command above becomes non-empty, or if any default/cloud feature begins to
resolve `rkyv 0.7`. If upstream `rust_decimal` drops or raises its optional
`^0.7.46` requirement, perform a targeted lock update, verify that the 0.7 node
is pruned, and remove `RUSTSEC-2026-0235` from `audit.toml` in the same change.
The exception is not authorization to process untrusted rkyv 0.7 archives.

## Verification evidence

All backend build/test commands used pinned Rust `1.95.0`, the checked-in
lockfile, two build jobs, and the worktree-local `src-tauri/target` directory.
The contract exporters used their repository-default targets, which also stayed
inside this worktree.

| Gate | Result |
| --- | --- |
| `cargo audit` after exact policy | exit 0; 1,186 dependencies; 0 unignored vulnerabilities; one exact ignored inactive advisory (`RUSTSEC-2026-0235`); four pre-existing allowed warnings |
| `cargo metadata --locked --offline --all-features --format-version 1` | exit 0; 1,186 packages; exact versions/optional edge above |
| required `cargo tree ... --all-features --edges all` | exit 0; `warning: nothing to print` |
| focused `persistence::surreal::tests` with `cloud,surrealdb-embedded` | 3 passed, 0 failed, 1,683 filtered; 0.73s test time |
| focused `sessions::` with `cloud,surrealdb-embedded` | 35 passed, 0 failed, 1,651 filtered; 0.58s test time |
| locked `cargo check --lib --tests` with `cloud,surrealdb-embedded` | exit 0; finished in 6m22s |
| serialized full locked lib suite with `cloud,surrealdb-embedded` | 1,678 passed, 0 failed, 8 ignored; 58.74s test time |
| strict locked Clippy with `--lib --tests ... -- -D warnings` | exit 0; finished in 50.23s |
| `cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check` | exit 0; no output |
| explicit `bun run verify:contracts` | exit 0; all five generated contracts current |
| pinned `SEEDS_CLI_ROOT=... bun run verify:fast` | exit 0; Biome checked 174 files; typecheck, five contracts, Seeds JSON stress, secret and diff hygiene passed |
| docs/Seeds secret hygiene inside `verify:fast` | 0 findings |
| final docs/Seeds secret-hygiene rerun including this report | exit 0; 0 findings |
| Betterleaks across both implementation files and this report | exit 0; approximately 334 KB scanned; no leaks |

The serialized suite emitted expected local-host PipeWire `client.conf` and ALSA
no-default-device diagnostics while its audio capability tests ran. They did
not produce failures; the final result remained 1,678/0/8.

## Footprint and custody

Implementation footprint relative to the reviewed base:

```text
src-tauri/.cargo/audit.toml | 22 insertions
src-tauri/Cargo.lock        | 2 insertions, 2 deletions
```

This report is the only additional owned artifact. No `Cargo.toml`, source,
workflow, generated file, Seeds record, or `ci/storage-probe` file changed.
`audio-graph-942a` was not closed, and nothing was pushed, merged, dispatched,
or synced.

## Separate follow-up

`audio-graph-c65d` remains open and independently owns the tracked
`ci/storage-probe/Cargo.lock`, whose audit graph and CI ownership boundary are
outside this workstream. This candidate neither edits nor claims evidence for
that workspace.

## Findings and open questions

- No new in-scope defect was found after the exact remediation and full gated
  suite.
- Remote Blacksmith cargo-audit confirmation remains an integration/CI action;
  this worktree did not push or dispatch a run.
- The `rkyv 0.7` exception requires active re-evaluation on either feature
  activation or an upstream `rust_decimal` dependency change; the removal
  commands and trigger are recorded above and beside the policy entry.

## Rollback

Revert the report commit first, then revert implementation commit
`3de20abdf3f5f20ba2626bb07506266db74a4649`. That restores `ammonia 4.1.3`
and removes the exact `RUSTSEC-2026-0235` exception, returning cargo-audit to
the known two-vulnerability RED state. No data rollback or persisted-format
migration is involved.
