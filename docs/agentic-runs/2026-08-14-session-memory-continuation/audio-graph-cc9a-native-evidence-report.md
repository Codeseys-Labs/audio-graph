# audio-graph-cc9a native evidence closure report

## Assignment and custody

- Seed: `audio-graph-cc9a`.
- Acceptance: expose the production qualification proof under one stable
  `cc9a_native_` filter, execute and preserve exact platform-specific test-name
  and count evidence in the native durability matrix, and fail closed on any
  command, artifact, summary, or proof-contract drift.
- Exact base and merge-base:
  `2212354c0dddd25f837eec408474b95fc9be9e29`.
- Worktree:
  `/home/codeseys/DevBox/audio-graph/.worktrees/cc9a-native-evidence-wave7c`.
- Branch: `work/audio-graph-cc9a-native-evidence-wave7c`.
- Initial and pre-report state: clean at implementation tip
  `28cab92d89c6f0ffca05f6ac8fb7f537b3fd8aa1`.
- Plan commit: `d54fd46095de99a812ea71b89ba9ca9801504392`.
- Implementation commit: `28cab92d89c6f0ffca05f6ac8fb7f537b3fd8aa1`.
- Report commit: this artifact's containing commit. Its exact SHA cannot be
  embedded in its own Git object and is recorded in the final handoff.

No Seed, dependency, frontend, generated file, other workflow, dispatch,
GitHub state, or runtime consumer was changed. The conductor retains
integration, Seed reconciliation, push, dispatch, merge, and cleanup authority.

## Outcome

The production qualification proof now has one stable native filter:

- Linux and macOS expose exactly three `cc9a_native_` tests. They cover a
  recreated-root refusal before coordination mutation, a qualified initial
  manifest CAS with the file-and-parent durability receipt, and a qualified
  replacement refusal when the open manifest head is replaced before temp
  creation.
- Windows exposes exactly one `cc9a_native_` test. It calls
  `SessionArtifactManifestStore::qualified_existing_root`, requires the typed
  `NamespaceDurabilityUnsupported { platform: Windows }` refusal, and proves
  the existing root remains entry-identical with no coordination, temp, or
  manifest-head mutation.
- The native workflow runs the filter on every matrix member and records the
  native process exit, tee exit, test count, exact sorted name markers, and
  complete log/artifact set. Unix and Windows summaries fail closed unless
  every platform-specific name, count, exit, and artifact is exact.
- Broad matrix expectations advance to Linux 46, macOS 16, and Windows 15;
  crash-harness expectations remain Unix 11 and Windows 9.
- The checker loads both Rust proof sources and rejects the original 18 LABSN
  regressions plus 14 cc9a proof/workflow regressions, for 32 mutations total.

The pinned direct LABSN action, license gate, cleanup, and shared Windows job
remain intact. LABSN is retained for that shared Windows job but is unrelated
to the cc9a namespace proof.

## TDD evidence

### Workflow-contract RED

After the source/checker slice existed but before the workflow step was added,
the real checker command was:

```text
bun run check:2df3-labsn-action
```

It exited nonzero with the exact load-bearing error:

```text
Error: missing workflow step: Run cc9a native qualification filter (Unix)
```

This proved the checker would not accept source-only qualification claims
without executable matrix evidence.

### Final checker GREEN

The same command on implementation tip
`28cab92d89c6f0ffca05f6ac8fb7f537b3fd8aa1` exited 0:

```text
PASS: direct LABSN and cc9a native evidence contract with 32 mutations
```

### Local Linux production-filter GREEN

Command:

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked \
  --manifest-path src-tauri/Cargo.toml --lib --no-default-features \
  --features cloud cc9a_native_ -- --nocapture --test-threads=1
```

Result: exit 0 in 2.33s; 3 passed, 0 failed, 1,701 filtered out.

```text
cc9a_native_qualified_guard_refuses_recreated_root_before_coordination_mutation
cc9a_native_qualified_initial_cas_has_parent_barrier
cc9a_native_qualified_replacement_refuses_foreign_open_head_before_temp_creation
```

This is local Linux/ext4 production evidence only. Native macOS/APFS and
Windows evidence are **not claimed** until the authorized remote matrix runs.
The Windows production-refusal test is present and statically guarded, but it
was not executed on this Linux host.

## Final gates and exact results

All Rust commands used Rust/Cargo 1.95.0 and the worktree-local
`src-tauri/target`.

### Focused and broad Rust

```text
cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib \
  --no-default-features --features cloud canonical_durability -- \
  --nocapture --test-threads=1
```

Result: exit 0 in 4.50s; 46 passed, 0 failed, 1,658 filtered out. The live
probe printed `live Linux ext4 qualification admitted`.

```text
cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib \
  --no-default-features --features cloud session_artifact_manifest -- \
  --nocapture --test-threads=1
```

Result: exit 0 in 10.39s; 26 passed, 0 failed, 1,678 filtered out.

### Locked compile, lint, formatting, and full library

```text
cargo +1.95.0 check --locked --manifest-path src-tauri/Cargo.toml --lib \
  --tests --no-default-features --features cloud
cargo +1.95.0 clippy --locked --manifest-path src-tauri/Cargo.toml --lib \
  --tests --no-default-features --features cloud -- -D warnings
cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --all -- --check
```

Final independent reruns: locked cloud check exit 0 in 0.63s; strict Clippy
exit 0 in 16.53s with no warnings; rustfmt exit 0 in 1.17s with no diff.

The one final serialized full cloud-library run used:

```text
cargo +1.95.0 test --quiet --locked --manifest-path src-tauri/Cargo.toml \
  --lib --no-default-features --features cloud -- --test-threads=1
```

Result: exit 0 in 57.11s; 1,696 passed, 0 failed, 8 ignored, finished in
56.41s. PipeWire and ALSA emitted expected diagnostics because this host has no
audio device; they did not fail a test.

### Frontend, contracts, and repo-pinned aggregate gate

`bun run typecheck` exited 0 in 6.49s with no TypeScript diagnostics.

`bun run verify:contracts` exited 0 in 19.18s. All five generated artifacts
were current: audio source, provider registry, session data movement, endpoint
credential routing, and speech span revision.

The authoritative aggregate command was:

```text
SEEDS_CLI_ROOT=/home/codeseys/DevBox/audio-graph/node_modules/@os-eco/seeds-cli \
  bun run verify:fast
```

Result: exit 0 in 9.07s. Biome checked 174 files; TypeScript passed; all five
contracts were current; the checker rejected all 32 mutations; Seeds JSON
stress parsed ready 50, blocked 94, and list 50; docs/Seeds secret hygiene
reported 0 findings; and diff hygiene passed.

### Workflow syntax and parser gates

The repository-configured actionlint command, `yq eval '.'`, Ruby
`YAML.safe_load_file`, and `node --check scripts/check-2df3-labsn-action.mjs`
all exited 0. Each of the six workflow steps declaring `shell: bash` was
extracted with `yq` and passed `bash -n` independently.

Neither `pwsh` nor `powershell.exe` is installed on this Linux host. The
PowerShell AST-parser gate was therefore recorded as unavailable and no tool
was installed. The Windows bodies still passed actionlint and both YAML
parsers; their executable parser/runtime evidence remains part of the remote
Windows matrix boundary.

### Security, custody, and runtime-dark checks

The report-inclusive Betterleaks scan covered all six authorized paths:
approximately 371,947 bytes scanned with no leaks. The report-inclusive
docs/Seeds secret-hygiene scan reported 0 findings. `git diff --check` is
clean.

A bounded production-source call-site search inspected `src-tauri/src` outside
the two owned persistence modules for
`SessionArtifactManifestStore::qualified_existing_root` and
`try_lock_exclusive_qualified`; it found no external production runtime caller.
This bounded conclusion excludes the workflow, checker, docs/report text, and
the test bodies inside the owned modules, where cc9a evidence references are
expected. The new qualification evidence remains runtime-dark: no production
Session consumer was activated.

The cumulative branch footprint from exact base through this report is
exactly these six authorized paths:

```text
.github/workflows/2df3-native-durability.yml
docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-cc9a-native-evidence-plan.md
docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-cc9a-native-evidence-report.md
scripts/check-2df3-labsn-action.mjs
src-tauri/src/persistence/canonical_durability.rs
src-tauri/src/persistence/session_artifact_manifest.rs
```

## Terminal Standards review correction

The terminal Standards review raised two documentation-accuracy nits, both
accepted without behavioral or cfg change:

- The macOS-enabled live initial-CAS test now uses platform-neutral panic text
  (`qualify existing live manifest root`) rather than describing every admitted
  platform as ext4.
- The runtime-dark statement now names its bounded production-source scope and
  exclusions. Workflow/checker/docs references and in-module test evidence are
  expected; the verified conclusion is only that no external production
  runtime consumer was activated.

Proportional correction gates on the two-file candidate passed:

- `cc9a_native_`: exit 0 in 17.26s; 3 passed, 0 failed, 1,701 filtered out;
- checker: exit 0 in 0.04s; all 32 mutations rejected;
- repository-configured actionlint: exit 0, no findings;
- rustfmt: exit 0 in 1.33s, no diff;
- checker Node syntax: exit 0;
- `git diff --check`: exit 0;
- final report-inclusive Betterleaks: no leaks; and
- final report-inclusive docs/Seeds secret hygiene: 0 findings.

### Standards re-review BLOCK(P3)

Standards re-review found that the second macOS-enabled cc9a manifest test
still described its live root as ext4. The replacement test now uses the same
platform-neutral `qualify existing live manifest root` panic text as the
initial-CAS test. No behavior or cfg changed. A bounded `rg` proof over the two
broadened cc9a native manifest test bodies confirms two matching neutral
strings and no remaining `ext4` text. The proportional correction gates
passed: bounded `rg` proof (`neutral_count=2`, `ext4_count=0`), `cc9a_native_`
3/3 in 12.50s, checker with all 32 mutations rejected in 0.03s, rustfmt in
1.18s, `git diff --check`, report-inclusive Betterleaks with no leaks, and
report-inclusive docs/Seeds secret hygiene with 0 findings.

## Findings and open questions

- No implementation blocker remains.
- Local Linux/ext4 qualification is proven. Native macOS/APFS and Windows are
  deliberately unclaimed until the remote matrix runs and preserves the new
  name/count/exit/artifact evidence.
- LABSN remains necessary to the shared Windows durability job, but it does
  not establish or weaken the cc9a namespace proof.
- No follow-up was discovered inside this bounded worker scope. Seed closure,
  remote dispatch, and integration remain conductor decisions.
