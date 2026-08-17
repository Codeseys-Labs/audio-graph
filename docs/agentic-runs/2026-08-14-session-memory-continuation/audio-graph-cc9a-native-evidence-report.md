# audio-graph-cc9a macOS diagnostic harness correction report

## Assignment and custody

- Seed: `audio-graph-cc9a`.
- Acceptance for this bounded correction: keep the macOS diagnostic collection
  from preventing the cc9a, canonical-durability, and crash-harness tests; map
  each diagnostic directory to its serving mount before `diskutil`; preserve
  the prior diagnostic evidence; make the summary enforce exact counts; retain
  all 42 checker mutations and add fail-closed coverage; make no product change.
- Exact base and merge-base:
  `86614dc44823ca61c09b6e137de3732331c98297`.
- Worktree:
  `/home/codeseys/DevBox/audio-graph/.worktrees/cc9a-macos-diagnostics2-wave7c`.
- Branch: `work/audio-graph-cc9a-macos-diagnostics2-wave7c`.
- Workflow/checker correction commit:
  `f02ab7a54546a54ec0da0ce240c93ff52893f2c9`.
- Stat-success P1 correction: this artifact's containing commit. Its SHA
  cannot be embedded in its own Git object and is recorded in the final
  handoff.
- Report commit: this artifact's containing commit. Its SHA cannot be embedded
  in its own Git object and is recorded in the final handoff.

The correction changes only the assigned native workflow and its source-aware
checker. This report is the only third path. No Seeds, Rust/product source,
dependency, frontend, generated file, other workflow, GitHub state, or remote
ref changed. The conductor retains remote rerun, integration, push, merge,
Seed reconciliation, and cleanup authority.

## Diagnostic run 31985989709

Run `31985989709` executed exact SHA
`6d1e5ac76b4425a5de0b3975894ac0f107952c6e`. The supplied downloaded bundle
is retained at:

```text
/tmp/audio-graph-cc9a-native-31985989709.wxoQUQ
```

The bundle's `SHA256SUMS` file verifies as:

```text
e64f2412258040d07c6c506d2869c98bd3624734e6d157b196ffd55873ee0b24  SHA256SUMS
```

Platform results are deliberately separated:

- Linux Blacksmith Ubuntu 24.04 on ext4: accepted. `cc9a_native_` 3/3,
  `canonical_durability` 47/47, and crash harness 11/11 all passed.
- Windows GitHub `windows-2025` on NTFS: accepted. `cc9a_native_` 1/1,
  `canonical_durability` 14/14, and crash harness 9/9 all passed.
- macOS Blacksmith macOS 15: not accepted and not diagnostic-complete. The
  cc9a, canonical-durability, and crash-harness steps did not run; the summary
  records `not_run` and counts 0/3, 0/17, and 0/11 respectively. No
  `CC9A_MACOS_DIAGNOSTIC` Rust marker exists in the artifact.

The macOS shell step passed the probe directory itself to `diskutil info`:

```text
Could not find disk: /Users/runner/_work/_temp/audio-graph-2df3-filesystem-probe
Process completed with exit code 1.
```

Because the step used `set -euo pipefail`, that first `diskutil` failure
aborted collection and caused the ordinary Rust test steps to be skipped.
The always-run summary and artifact upload still ran, which preserved this
partial evidence:

- the probe directory, `/`, and `/System/Volumes/Data` all had stat device
  `16777226`;
- `/` was `/dev/disk2s1s1`, APFS, sealed and read-only;
- `/System/Volumes/Data` was `/dev/disk2s5`, APFS, writable Data; and
- the earlier fixture preflight had already resolved the probe through
  `df -P` to `/System/Volumes/Data` and recorded `/dev/disk2s5`.

This is a diagnostic-harness defect, not evidence for changing production
mount selection. The live Rust ambiguity remains unobserved because the test
never ran.

## Correction

The macOS-only diagnostic step now uses one quoted Bash target array for the
probe directory, `/`, and `/System/Volumes/Data`. For each target it:

1. preserves BSD `stat -f` device/inode/flags output and relevant `mount`
   records;
2. captures `df -P` exit and output, parses the POSIX record's mounted-on field,
   and records both the original target and resolved serving mount;
3. calls `diskutil info` only with the resolved mount; and
4. captures the per-target `diskutil` exit, complete command output, and the
   existing bounded identity/policy fields without aborting on a failed target.

The collection always emits `target_count`, `resolved_count`, `success_count`,
`failure_count`, and one `diagnostics_complete=true` marker before exiting 0.
This keeps the Rust tests runnable. It does not hide diagnostic failures: the
Unix summary marks the macOS preflight complete only with exactly 3 targets,
3 resolutions, 3 zero-exit `diskutil` observations, 3 successes, 0 failures,
one completion marker, exactly 3 stat targets, exactly 3 `stat_exit=0`
observations, and mount evidence. Any collection defect therefore leaves the
job FAIL after all test and upload evidence has had a chance to run.

The checker retains every previous mutation and adds five cases for:

- directory-to-mount resolution;
- nonfatal per-target collection;
- exact count and completion output;
- a collection exit that would skip the Rust tests; and
- exact Unix-summary success enforcement.

The Spec P1 follow-up adds a sixth case for exact stat-success enforcement.
The exact current checker total is 48 mutations.

## TDD evidence

The unchanged baseline passed all 42 existing mutations:

```text
PASS: direct LABSN and cc9a native evidence contract with 42 mutations
```

After only the checker contract and new mutations were added, before the
workflow correction, the checker exited 1:

```text
error: macOS directory-to-mount diskutil identity/policy evidence drift
```

After the workflow correction:

```text
PASS: direct LABSN and cc9a native evidence contract with 47 mutations
```

The Spec P1 review found that the first correction counted three
`stat_target` rows but did not require three successful stat exits. After only
the P1 checker guard and mutation were added, the unchanged summary exited 1:

```text
error: macOS summary diagnostic completeness/fail-closed gate drift
```

The permanent deterministic simulation constructs an otherwise complete
artifact with stat exits `0,1,0`. It proves the prior predicate accepted that
artifact, the corrected predicate rejects it, and a `0,0,0` artifact passes:

```text
PASS: macOS summary simulation prior_false_pass=true corrected_failure_rejected=true full_good_pass=true
PASS: direct LABSN and cc9a native evidence contract with 48 mutations
```

## Local gates

All successful Rust tests used locked dependencies, cloud features, Rust/Cargo
1.95.0, and the worktree-local `src-tauri/target`.

- `cc9a_native_` regression:

  ```text
  test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 1702 filtered out
  ```

- `canonical_durability` regression:

  ```text
  test result: ok. 47 passed; 0 failed; 0 ignored; 0 measured; 1658 filtered out
  ```

- One final serialized full cloud library suite:

  ```text
  test result: ok. 1697 passed; 0 failed; 8 ignored; 0 measured; 0 filtered out; finished in 55.52s
  ```

  An initial root-directory invocation selected system Rust 1.88 and was
  refused before compilation because current locked dependencies require Rust
  1.95. The recorded final suite ran from `src-tauri`, where the repository's
  pinned Rust 1.95 toolchain is selected.

- `bun run typecheck`: exit 0 with no diagnostics.
- `bun run verify:contracts`: exit 0; all five generated contracts were
  current: audio source, provider registry, session data movement, endpoint
  credential routing, and speech span revision.
- `bun run verify:fast`: exit 0. Biome checked 174 files, typecheck and all five
  contracts passed, the checker rejected all 47 mutations, Seeds JSON stress
  parsed ready 50 / blocked 94 / list 50, docs and Seeds secret hygiene found
  0 issues, and diff hygiene passed.
- Repo-configured actionlint, `yq eval '.'`, Ruby `YAML.safe_load_file`, Node
  syntax, and all 7 independently extracted `shell: bash` bodies passed.
- Neither `pwsh`, `powershell`, nor `powershell.exe` is installed on this Linux
  host. PowerShell parser validation is recorded as unavailable; no tool was
  installed.

The P1 follow-up reran the 48-mutation checker and deterministic simulation,
repo-configured actionlint, yq and Ruby YAML parsing, all 7 independently
extracted Bash bodies, Node syntax, Biome over 174 files, and diff hygiene.
All passed. Focused Rust tests were not repeated because this follow-up changes
only one shell-summary predicate and its JavaScript checker; no test command,
Rust source, dependency, or product path changed.

## Security, footprint, and runtime-dark checks

The report-inclusive Betterleaks scan over the exact three authorized paths
reported `no leaks found`. The report-inclusive docs/Seeds secret-hygiene scan
reported `0 findings`, and `git diff --check` passed.

The exact cumulative footprint from the assigned base is:

```text
.github/workflows/2df3-native-durability.yml
docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-cc9a-native-evidence-report.md
scripts/check-2df3-labsn-action.mjs
```

An exact semantic assertion confirms that every Rust/product/dependency,
frontend, generated, Seeds, and other-workflow path is byte-identical to the
assigned base. With no product path changed and no runtime call site added,
the correction is runtime-dark outside the diagnostic workflow.

## Findings and open questions

The Spec review's P1 finding was valid: a partial `stat` failure could leave
three target rows and still satisfy the first correction's summary. The Unix
summary now requires exactly three `stat_exit=0` rows before PASS. Collection
remains nonfatal, so this fail-closed check still occurs only after the Rust
test steps and artifact collection can run. No unrelated finding was changed
in this workstream.

The next evidence action is a conductor-authorized remote rerun at the reviewed
correction tip, followed by download, hashing, and artifact review. Until that
rerun produces the Rust marker and exact 3/3, 17/17, and 11/11 macOS results,
there is no corrected macOS acceptance claim and no diagnostic-complete claim.
Production mount-selection work remains stopped pending that live evidence.
