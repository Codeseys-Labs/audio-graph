# audio-graph-cc9a native evidence closure plan

## Acceptance and custody

Seed `audio-graph-cc9a` authorizes a bounded native-evidence closure patch on
exact base `2212354c0dddd25f837eec408474b95fc9be9e29`. Execution is isolated to
branch `work/audio-graph-cc9a-native-evidence-wave7c` in the clean worktree
`/home/codeseys/DevBox/audio-graph/.worktrees/cc9a-native-evidence-wave7c`.

The patch must expose exactly three `cc9a_native_` production-qualification
tests on Linux and macOS and exactly one production Windows refusal test. The
Windows test must call `SessionArtifactManifestStore::qualified_existing_root`
and prove the typed `NamespaceDurabilityUnsupported { platform: Windows }`
refusal leaves the existing root entry-identical with no coordination, temp,
or manifest-head mutation. No synthetic qualification path may stand in for
that production proof.

The workflow must execute the named filter on every native matrix member,
retain full logs plus native/tee exits, counts, and name-marker evidence, and
fail closed unless the exact platform-specific names and counts are present.
The broad canonical durability matrix is Linux 46, macOS 16, and Windows 15;
the crash-harness matrix remains 11, 11, and 9. The pinned direct LABSN action,
license gate, cleanup, and its existing mutation protections remain unchanged;
LABSN is unrelated to the cc9a namespace proof.

## Owned footprint

The only writable paths are:

- `src-tauri/src/persistence/canonical_durability.rs`
- `src-tauri/src/persistence/session_artifact_manifest.rs`
- `.github/workflows/2df3-native-durability.yml`
- `scripts/check-2df3-labsn-action.mjs`
- this plan
- `audio-graph-cc9a-native-evidence-report.md` beside this plan

Seeds, dependencies, frontend, generated files, other workflows, dispatch,
GitHub state, and every other path are outside this worker's authority.

## TDD seams and RED/GREEN slices

1. Rename and broaden the live recreated-root guard test through the production
   `CanonicalFilesystemQualification::for_existing_managed_root` and qualified
   guard seam. Capture the checker RED while its required cc9a name is absent,
   then make the Linux/macOS source contract GREEN.
2. Rename and broaden the live initial/replacement manifest tests through
   `SessionArtifactManifestStore::qualified_existing_root`. Preserve the exact
   parent-barrier and foreign-open-head/no-temp assertions. Capture checker RED
   before making the source contract GREEN.
3. Add the Windows-only production constructor refusal test. Assert the exact
   typed platform error and entry-identical root inventory with absent lock,
   temp, and manifest head. The checker must reject a synthetic constructor,
   weakened error assertion, weakened equality proof, or narrowed cfg.
4. Add `expected_cc9a_native_tests` to the native matrix and execute
   `cc9a_native_` on Unix and Windows with complete evidence files. Enforce the
   exact names and counts in platform summaries and artifact completeness. The
   checker must reject command, evidence, name/count gate, summary, and platform
   count drift.
5. Extend the checker from its 18 LABSN mutations to approximately 32 total
   mutations by loading both Rust sources and planting fail-closed cc9a
   regressions. Capture a checker RED before the source/workflow GREEN, then
   require the checker and every mutation to pass.

## Gates and evidence boundary

During the loop, run the checker and focused `cc9a_native_`,
`canonical_durability`, and `session_artifact_manifest` cloud-library filters.
Final local evidence must include:

- Linux `cc9a_native_` 3/3, canonical durability 46/46, and the current full
  manifest-filter count;
- locked cloud check, strict cloud Clippy with `-D warnings`, rustfmt, and one
  final serialized full cloud library run;
- frontend typecheck, repo-pinned `verify:fast`, and all five contracts;
- repo-configured actionlint, yq and Ruby YAML parsing, Bash syntax, and a
  PowerShell parser when installed (otherwise an explicit absence record);
- the checker and every mutation, Betterleaks, docs/Seeds secret hygiene,
  `git diff --check`, exact merge-base/footprint, and runtime-dark checks.

Only local Linux execution can become native evidence in this worktree. The
report must not claim native macOS or Windows execution before an authorized
remote dispatch.

## Commit and handoff shape

Commit this plan before RED. Commit tests/checker/workflow/source implementation
separately from the final report. The final tree must be clean and limited to
the six owned paths. The conductor owns review, integration, Seed mutation,
push, dispatch, merge, and worktree cleanup.
