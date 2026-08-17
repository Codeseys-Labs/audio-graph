# audio-graph-cc9a macOS diagnostic evidence report

## Assignment and custody

- Seed: `audio-graph-cc9a`.
- Acceptance for this bounded workstream: add test-only macOS mount-selection
  diagnostics to the existing counted native test, collect matching read-only
  workflow evidence, extract and fail-closed-check the artifact, preserve the
  original 33 checker mutations, and record the terminal native evidence
  without claiming corrected macOS acceptance.
- Exact base and merge-base:
  `01426350bc94e6d8723465f1cb46af67b3381b7a`.
- Worktree:
  `/home/codeseys/DevBox/audio-graph/.worktrees/cc9a-macos-diagnostics-wave7c`.
- Branch: `work/audio-graph-cc9a-macos-diagnostics-wave7c`.
- Diagnostic implementation commit:
  `1dc71a6cd76570971e748e312b506654ccf0ba6f`.
- Report commit: this artifact's containing commit. Its SHA cannot be embedded
  in its own Git object and is recorded in the final handoff.

The implementation commit changes only the assigned Rust source, native
workflow, and source-aware checker. This report is the only fourth path. No
Seeds, dependency, frontend, generated-file, other-workflow, GitHub, remote-ref,
or production selection change was made. The conductor retains native rerun,
review, integration, push, merge, Seed reconciliation, and cleanup authority.

## Terminal run 31983646869

Run `31983646869` executed exact SHA
`31e9994` (`31e9994...` as recorded by the supplied evidence). Its downloaded
bundle is retained at `/tmp/audio-graph-cc9a-native-31983646869.mOJT3O`.

The supplied artifact hashes are:

| Evidence | SHA-256 |
| --- | --- |
| Manifest | `88d674d6a704422f3fdd6800d05f63d6eb33cb9fefee824cad7f4b3ad28c7974` |
| Linux tree | `7323709c7c1b21ab568628f777060d59efc8acdbc7e42a7b4cf331584c1e7e0b` |
| macOS tree | `1875459bb438cfb6db3066a104ca27db730675c4d0a2e66a57c3c51c2e0e0a25` |
| Windows tree | `f961c269cc0fb5e8f50bf3f45147113491595a15b1f5a69449a688a240f484bb` |

Platform conclusions are deliberately separated:

- Linux Blacksmith Ubuntu 24.04 on ext4 is accepted for this run:
  `cc9a_native_` 3/3 PASS, `canonical_durability` 47/47 PASS, and crash
  harness 11/11 PASS.
- Windows GitHub `windows-2025` on NTFS is accepted for this run:
  `cc9a_native_` 1/1 PASS, `canonical_durability` 14/14 PASS, and crash
  harness 9/9 PASS.
- macOS Blacksmith macOS 15 on the APFS Data volume is not accepted:
  `cc9a_native_` 0/3 with `IdentityUnavailable`, broad canonical durability
  16/17, and crash harness 11/11 PASS.

The generic `IdentityUnavailable` result does not distinguish two
load-bearing live branches: multiple sysinfo observations can match the same
`st_dev`, or zero observations can match while at least one observation has
unavailable mount metadata. There is therefore a hard stop on any further
production mount-selection change until reviewed live evidence distinguishes
the branch. This patch makes no corrected macOS acceptance claim.

## Diagnostic change

The Rust source adds one function compiled only under
`cfg(all(test, target_os = "macos"))`. The existing counted test
`cc9a_native_qualified_guard_refuses_recreated_root_before_coordination_mutation`
calls it once, before its first live production qualification. It emits the
stable `CC9A_MACOS_DIAGNOSTIC` marker with:

- canonical root, numeric root device when available, and sysinfo inventory
  count;
- one observation row per sysinfo disk with mount path, filesystem class and
  string, metadata availability, numeric device when available,
  `same_root_dev`, read-only, and removable state; and
- metadata-unavailable and same-root-device cardinalities plus exactly one
  branch: `root_missing`, `zero_match_clean`,
  `zero_match_with_unavailable`, `unique`, `ambiguous`, or
  `unique_then_validate_mismatch`.

The helper reuses the private live inventory, filesystem classification,
identity, and independent mount validation seams. It does not change a
production API, production selection, production error, or test count.

The macOS workflow now always attempts an artifact-backed, read-only
diagnostic preflight. It retains the existing `df -P` probe and additionally
records BSD `stat -f` device/inode/flags for the probe root, `/`, and
`/System/Volumes/Data`; relevant `mount` records; and bounded `diskutil info`
identity, role, filesystem, read-only, internal, and removable fields for all
three paths. The cc9a Unix step extracts the stable Rust rows into
`cc9a_macos_diagnostics.txt` even when cargo exits nonzero. The always-run Unix
summary records marker presence, schema/cardinality completeness, and mount
preflight completeness. These diagnostics can be complete while the job still
correctly remains FAIL when the product test exit is nonzero.

Linux and Windows runners, counts, product commands, LABSN action/license
boundary, and certificate cleanup are unchanged. Linux gains only
`not_applicable` diagnostic summary fields; its pass/fail predicates are
semantically unchanged.

## TDD evidence

The pre-agreed seam was the source-aware checker plus the existing counted
Rust test. After adding only the checker requirement for the macOS call, before
adding instrumentation, this command exited 1:

```text
$ bun run check:2df3-labsn-action
error: cc9a macOS live diagnostic call missing from counted guard test
error: script "check:2df3-labsn-action" exited with code 1
```

After the cfg(test)-only helper, workflow evidence, extraction, summary gate,
and mutations were implemented, the checker reported:

```text
$ bun scripts/check-2df3-labsn-action.mjs
PASS: direct LABSN and cc9a native evidence contract with 42 mutations
```

The checker preserves all existing 33 mutations and adds exactly 9 for the
macOS diagnostic call, root identity fields, inventory fields, cardinality
branch, `stat`, `mount`, `diskutil`, extraction artifact, and summary
completeness.

## Local gates

All Rust commands used Rust/Cargo 1.95.0, locked dependencies, cloud features,
and the worktree-local `src-tauri/target`.

- `cc9a_native_`: `3 passed; 0 failed; 1702 filtered out`.
- `canonical_durability`: `47 passed; 0 failed; 1658 filtered out`; the live
  Linux ext4 qualification was admitted.
- `session_artifact_manifest`: `26 passed; 0 failed; 1679 filtered out`.
- Locked cloud `cargo check --lib --tests`: exit 0.
- Strict cloud Clippy with `-D warnings`: exit 0 with no warnings.
- `cargo fmt --all -- --check`: exit 0 with no diff.
- The single final serialized full cloud library suite reported:

  ```text
  test result: ok. 1697 passed; 0 failed; 8 ignored; 0 measured; 0 filtered out; finished in 55.05s
  ```

- `bun run typecheck`: exit 0 with no diagnostics.
- `bun run verify:contracts`: exit 0; audio source, provider registry, session
  data movement, endpoint credential routing, and speech span revision were
  current.
- Repo-pinned `SEEDS_CLI_ROOT=... bun run verify:fast`: exit 0. Biome checked
  174 files, typecheck and five contracts passed, all 42 mutations were
  rejected, Seeds JSON stress parsed ready 50 / blocked 94 / list 50, docs and
  Seeds secret hygiene reported 0 findings, and diff hygiene passed.
- Repo-configured actionlint, `yq eval '.'`, Ruby `YAML.safe_load_file`, and
  Node syntax checks passed. All 7 workflow steps declaring `shell: bash` were
  extracted independently and passed `bash -n`.
- A source-boundary assertion removed the new cfg(test)-only helper and the
  macOS-only call from the current source and reproduced the exact base file:

  ```text
  PASS: no non-test canonical durability source delta
  ```

- Neither `pwsh`, `powershell`, nor `powershell.exe` is installed on this
  Linux host, so a PowerShell AST parse is recorded as unavailable; no tool was
  installed.

## Security, footprint, and runtime-dark checks

The report-inclusive Betterleaks scan over the exact four authorized paths
reported `no leaks found`. The report-inclusive docs/Seeds secret-hygiene scan
reported `0 findings`, and `git diff --check` passed.

The exact cumulative footprint from the assigned base is:

```text
.github/workflows/2df3-native-durability.yml
docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-cc9a-native-evidence-report.md
scripts/check-2df3-labsn-action.mjs
src-tauri/src/persistence/canonical_durability.rs
```

A bounded call-site search outside `canonical_durability.rs` and
`session_artifact_manifest.rs` reported:

```text
runtime_dark_external_hits=0
```

The diagnostic patch remains runtime-dark and test-only. It does not activate
a Session writer or consumer.

## Findings and open questions

No unrelated code finding was changed in this workstream. One initial
runtime-dark probe used an exclusion glob that did not match nested paths; the
corrected read-only probe excluded the two intended modules and returned zero
external hits. This was a gate-command correction, not a product change.

Native macOS diagnostics remain unclaimed until a conductor-authorized remote
rerun on the reviewed diagnostic tip is downloaded, hashed, and reviewed. The
next evidence decision is whether the stable branch marker reports
`ambiguous`, `zero_match_with_unavailable`, or another branch. Production
selection work must remain stopped until that evidence is reconciled.
