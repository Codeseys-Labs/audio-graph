# audio-graph-cc9a native evidence correction report

## Assignment and custody

- Seed: `audio-graph-cc9a`.
- Acceptance for this correction: bind macOS APFS selection to the managed
  root's native volume identity, retain all existing policy and mount-volume
  checks, correct the Windows broad-test expectation, strengthen its checker,
  and preserve an honest terminal-run record pending a native rerun.
- Exact base and merge-base:
  `3afe56c52e74804372cb752e418b4004b1e24694`.
- Worktree:
  `/home/codeseys/DevBox/audio-graph/.worktrees/cc9a-native-correction-wave7c`.
- Branch: `work/audio-graph-cc9a-native-correction-wave7c`.
- Implementation commit:
  `9ba603eaff57bf502a547d87941e6a98da7e6367`.
- Report commit: this artifact's containing commit. Its exact SHA cannot be
  embedded in its own Git object and is recorded in the final handoff.

The correction changes exactly the assigned source, workflow, checker, and
this report. It does not change Seeds, dependencies, frontend or generated
files, other workflows, runtime consumers, GitHub state, or remote refs. The
conductor retains Seed reconciliation, native rerun, integration, push, merge,
and cleanup authority.

## Terminal run 31981177828

The terminal native run executed exact head
`02a09282b56427bc39088bc126968aa68190f90f`. Downloaded logs and artifacts are
retained at `/tmp/audio-graph-cc9a-native-31981177828.LspXQy`.

The upload logs record these SHA-256 digests for the finalized artifact zips:

| Platform | Artifact SHA-256 |
| --- | --- |
| Linux | `3c91bd25dd7ecba60205142dbbb18940a910b217903d83f9ac4861b67458b053` |
| macOS | `30018aaefa880b29bbafad8f242d27326d029efd525d24655435953201a88b7c` |
| Windows | `abcde903b9e53d81536bb7d00f204755bbdc9af11d7a26c1647a7925833c0ff6` |

The platform evidence is intentionally separated from the correction status:

- Linux Blacksmith Ubuntu 24.04 on ext4 passed `cc9a_native_` 3/3,
  `canonical_durability` 46/46, and crash harness 11/11.
- macOS Blacksmith macOS 15 on APFS failed all three `cc9a_native_` tests and
  the canonical qualification guard test; the broad suite was 15 passed and
  1 failed, while crash harness 11/11 passed. Each failed qualification
  returned `ReadOnlyFilesystem { class: Apfs }`.
- Windows GitHub `windows-2025` on NTFS passed the exact production typed
  refusal proof 1/1 and crash harness 9/9. Its broad canonical suite actually
  passed 14 tests and exited 0, but the workflow expected 15, so the summary
  correctly failed the harness count gate.

The macOS fixture evidence identifies `/dev/disk2s5` mounted at
`/System/Volumes/Data`, reports APFS, and shows the fixture under logical
`/Users`. The former lexical longest-prefix selection attributed logical
`/Users` and `/var` roots to the read-only APFS System `/` observation even
though root metadata belongs to the writable Data device. LABSN completed on
Windows but is unrelated to the namespace qualification result. No active
Blacksmith Testboxes remained after the terminal run.

## Correction

Live filesystem observations now derive an optional, content-free
`VolumeIdentity` from safe `std::fs::metadata` and the existing
`filesystem_identity` seam. On macOS, qualification selects exactly one mount
observation whose native volume identity equals the canonical root's volume
identity before applying the existing policy gates. It refuses closed when
the root or mount identity is unavailable, no same-volume mount exists, or
more than one observation claims the root volume.

The selected mount must still be exactly APFS, writable, and non-removable.
`validate_mount_volume` independently re-reads the selected mount metadata and
compares its volume identity to the root. Initial qualification and guard
revalidation both call the same selector and independent validator. Linux
retains longest lexical-prefix selection and exact ext4 policy. No unsafe,
libc/statfs call, dependency, firmlink allowlist, test write, caller authority,
or arbitrary APFS acceptance was added.

The deterministic macOS topology test models:

- logical `/Users/...` root identity on the Data device;
- read-only APFS `/` on a different System device; and
- writable, non-removable APFS `/System/Volumes/Data` on the root's device.

The same test covers read-only and removable matched Data mounts, missing root
identity, missing mount identity, no same-volume match, and ambiguous duplicate
same-volume observations. It is compiled for Linux and macOS, so broad native
expectations are now Linux 47, macOS 17, and Windows 14.

The checker now extracts the Windows matrix row and jointly requires
`cc9a_native_` 1, broad canonical 14, and crash harness 9 in that row. A new
mutation changes only the Windows canonical count from 14 to 13 and is
rejected. The checker total is 33 mutations. Summary equality gates, the
focused Windows `cc9a_native_` proof, and LABSN license, certificate cleanup,
and evidence boundaries remain unchanged.

## TDD evidence

### Deterministic RED

After adding observation identity and the desired firmlink topology test but
before changing selection, this command exited 101:

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked \
  --manifest-path src-tauri/Cargo.toml --lib --no-default-features \
  --features cloud \
  macos_volume_group_selection_binds_logical_root_to_unique_data_volume -- \
  --nocapture --test-threads=1
```

The exact assertion mismatch was:

```text
left: Err(ReadOnlyFilesystem { class: Apfs })
right: Ok(QualifiedFilesystemMount { mount_point: "/System/Volumes/Data", class: Apfs })
```

This reproduces the native failure without requiring a macOS host.

### GREEN

The same focused command passed 1/1 after the identity-bound selector was
implemented. The companion fail-closed cases execute inside that same test.
The full focused module then passed 47/47 on Linux, including the live ext4
production qualification.

## Final local gates on implementation commit

All Rust commands used Rust/Cargo 1.95.0 and the worktree-local
`src-tauri/target`.

- Focused macOS policy topology: 1 passed, 0 failed.
- `cc9a_native_`: 3 passed, 0 failed, 1,702 filtered out.
- `canonical_durability`: 47 passed, 0 failed, 1,658 filtered out; live Linux
  ext4 qualification admitted.
- `session_artifact_manifest`: 26 passed, 0 failed, 1,679 filtered out.
- Locked cloud `cargo check --lib --tests`: exit 0.
- Strict cloud Clippy with `-D warnings`: exit 0 with no warnings.
- `cargo fmt --all -- --check`: exit 0 with no diff.
- The single final serialized full cloud library suite: 1,697 passed, 0
  failed, 8 ignored in 64.67 seconds. Expected PipeWire/ALSA diagnostics on
  the audio-less Linux host did not fail a test.
- `bun run typecheck`: exit 0 with no diagnostics.
- `bun run verify:contracts`: exit 0; all five generated contracts were
  current: audio source, provider registry, session data movement, endpoint
  credential routing, and speech span revision.
- Repo-pinned `SEEDS_CLI_ROOT=... bun run verify:fast`: exit 0. Biome checked
  174 files; TypeScript and all five contracts passed; all 33 checker
  mutations were rejected; Seeds JSON stress parsed ready 50, blocked 94, and
  list 50; docs/Seeds hygiene reported 0 findings; diff hygiene passed.
- Direct `bun run check:2df3-labsn-action`: exit 0 with
  `PASS: direct LABSN and cc9a native evidence contract with 33 mutations`.

The repository-configured actionlint, `yq eval '.'`, Ruby
`YAML.safe_load_file`, and Node syntax checks passed. All six workflow steps
declaring `shell: bash` were extracted independently with `yq` and passed
`bash -n`. Neither `pwsh` nor `powershell.exe` is installed on this Linux host,
so the PowerShell AST parser is recorded as unavailable; no tool was installed.

## Security, footprint, and runtime-dark checks

The report-inclusive Betterleaks scan covered all four authorized paths and
reported `no leaks found`. The report-inclusive docs/Seeds secret-hygiene scan
reported `0 findings`, and `git diff --check` passed. The exact cumulative
correction footprint from the assigned base is:

```text
.github/workflows/2df3-native-durability.yml
docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-cc9a-native-evidence-report.md
scripts/check-2df3-labsn-action.mjs
src-tauri/src/persistence/canonical_durability.rs
```

A bounded call-site search over `src-tauri/src` outside
`canonical_durability.rs` and `session_artifact_manifest.rs` found no production
caller of `SessionArtifactManifestStore::qualified_existing_root` or
`try_lock_exclusive_qualified`. The correction remains runtime-dark; it does
not activate a Session writer or consumer. The bounded gate reported
`runtime_dark_external_hits=0`.

## Outcome and open question

The reviewed local correction is implemented and locally verified. It does
**not** establish corrected native macOS or Windows success. Seed closure is
not allowed from this local evidence. The remaining work is a conductor-
authorized native matrix rerun on the correction tip, capture and hash of the
new artifacts, and reconciliation of the resulting Linux, macOS, and Windows
evidence before any closure claim.
