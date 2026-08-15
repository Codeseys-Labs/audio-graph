# audio-graph-1189 implementation report

Date: 2026-08-14

## Assignment and correction round

- Seed: `audio-graph-1189` — Restore accepted canonical stream registry
  ADR-0037 lineage.
- Worktree:
  `/home/codeseys/DevBox/audio-graph/.worktrees/1189-adr0037-wave7b`
- Branch: `work/1189-adr0037-wave7b`
- Exact base: `c7d3fec2db8c60629e0b7c8b93e752c3aee85368`
- Initial reviewed tip: `6f51f2626ab19740746dfb84e0678fefb7ec7876`.
- Review disposition: both axes blocked the initial tip; this is bounded
  correction round 1 of at most 2.
- Corrected acceptance: preserve exact ADR-0037 blob
  `6f94d5a9fb183afbef70826add08fc3c1f163f59`; preserve the current ADR-0035
  and ADR-0036 blobs; add corrected reviewed research blob
  `10e0049246f78ba2b2aa31abc67cd95f0866225b`; make the non-ancestor lineage
  warning discoverable from the index; correct manifest/durability ownership,
  crash cuts, and clean-worktree Seeds gates; and change no product, Seeds,
  workflow, package, generated, or unrelated ADR path.

## Outcome

The stable snapshot now contains the exact accepted canonical event-stream
registry, its current-format index entry and discoverable lineage warning, the
corrected reviewed primary-source durability research, the corrected Wave 7B
execution plan, and this report.

The plan distinguishes the five `audio-graph-8e73` durability children
(`1189`, `c2e3`, `3b8b`, `b77b`, and `2df3`) from the two
`audio-graph-be7c`-owned manifest prerequisites (`661f` and `a596`). It names
every required recovery crash cut and provides a fail-closed way to resolve an
already-installed repository-pinned Seeds CLI without installing, patching, or
symlinking packages in a clean worktree.

No product code, Seed, workflow, package, generated artifact, current
ADR-0035/0036 byte, ADR-0037 byte, or integration checkpoint changed. This
workstream did not close a Seed, merge, push, dispatch a workflow, start a
Blacksmith job, or install/patch a dependency.

## RED and source evidence

Before the initial implementation:

```text
RED PASS: ADR-0037 file absent
RED PASS: ADR-0037 index entry absent
src-tauri/src/persistence/canonical_reader.rs:32:// ADR-0037 pins the IPC payload version supported by outer stream schema v1.
ADR-0035 current blob: 91ffb0304c06323be6254889d716e639ebc4d79e
ADR-0036 current blob: 3af2bcfeafe14d01544b4f122c10b8df78335fe2
ADR-0037 source blob: 6f94d5a9fb183afbef70826add08fc3c1f163f59
RED PASS: 5487c3b is not an ancestor of HEAD
```

Before correction round 1:

```text
Correction RED: research artifact absent
Corrected research blob: 10e0049246f78ba2b2aa31abc67cd95f0866225b
Corrected research lines/bytes: 401 29754
```

The research file was restored through `apply_patch` from corrected reviewed
tip `30490249ef08fb59a4b0bebfd855009c24f90117`; neither the research branch nor
non-ancestor ADR source commit `5487c3b` was cherry-picked or merged.

## What changed

- Restored
  [ADR-0037](../../adr/0037-freeze-canonical-event-stream-registry.md)
  byte-for-byte from non-ancestor commit `5487c3b`.
- Added ADR-0037's current-format accepted index row/reference and a nearby
  warning that its internal ADR-0035/0036 references belong to the historical
  non-ancestor canonical-log lineage. The warning links the Wave 7B plan.
- Added the
  [corrected reviewed research](../../research/canonical-directory-durability-2026-08-14.md)
  byte-for-byte from corrected reviewed tip `30490249`.
- Corrected the
  [Wave 7B plan](audio-graph-8e73-wave7b-plan.md) to keep 8e73 durability
  ownership separate from be7c manifest ownership; explicitly cover
  quarantine directory-sync, manifest prepare, source truncate, source sync,
  manifest completion, pre-acknowledgement, and post-acknowledgement cuts; and
  resolve the pinned Seeds CLI fail closed in clean worktrees.
- Updated this report with exact executable commands and final five-file gate
  evidence.

## Lineage and queue clarification

The restored ADR-0037 is immutable archival content. Its references to
ADR-0035 and ADR-0036 name non-ancestor canonical-log framing and recovery
decisions that existed beside it at source commit `5487c3b`. The current
accepted ADR-0035 and ADR-0036 are distinct speech-semantics decisions. They
remain at these exact blobs:

- ADR-0035: `91ffb0304c06323be6254889d716e639ebc4d79e`;
- ADR-0036: `3af2bcfeafe14d01544b4f122c10b8df78335fe2`.

This branch's tracked Seed snapshot predates the custody review updates. The
worker did not edit it. Final fan-in must include the already-recorded custody
Seed correction before conductor reconciliation or closure.

## Final five-file footprint

- `docs/adr/0037-freeze-canonical-event-stream-registry.md`
- `docs/adr/README.md`
- `docs/research/canonical-directory-durability-2026-08-14.md`
- `docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-8e73-wave7b-plan.md`
- `docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-1189-report.md`

## Gates and real results

### Exact blob and lineage checks

Commands:

```bash
test "$(git hash-object docs/adr/0037-freeze-canonical-event-stream-registry.md)" = \
  6f94d5a9fb183afbef70826add08fc3c1f163f59
test "$(git hash-object docs/research/canonical-directory-durability-2026-08-14.md)" = \
  10e0049246f78ba2b2aa31abc67cd95f0866225b
git diff --exit-code 6f51f2626ab19740746dfb84e0678fefb7ec7876 -- \
  docs/adr/0037-freeze-canonical-event-stream-registry.md \
  docs/adr/0035-keep-versioned-speech-revisions-in-one-canonical-stream.md \
  docs/adr/0036-version-projection-basis-hashes-by-speech-semantics.md
```

Real result:

```text
ADR-0037 blob: 6f94d5a9fb183afbef70826add08fc3c1f163f59
Research blob: 10e0049246f78ba2b2aa31abc67cd95f0866225b
ADR-0035 blob: 91ffb0304c06323be6254889d716e639ebc4d79e
ADR-0036 blob: 3af2bcfeafe14d01544b4f122c10b8df78335fe2
Immutable ADR diff from 6f51f26: PASS
```

Result: PASS.

### Index, lineage-warning, and relative-link checks

Commands:

```bash
test "$(rg -c '^\| \[0037\] \| Freeze the Canonical Event Stream Registry \| accepted \| Refines ADR-0027; freezes canonical event-domain IDs and outer schema v1 \| 2026-07-10 \|$' docs/adr/README.md)" = 1
test "$(rg -c '^\[0037\]: 0037-freeze-canonical-event-stream-registry\.md$' docs/adr/README.md)" = 1
rg -n 'ADR-0037 lineage warning|non-ancestor canonical-log lineage|do not name the current speech-semantics' docs/adr/README.md
command bun - <<'BUN'
import { existsSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
const files = [
  "docs/adr/0037-freeze-canonical-event-stream-registry.md",
  "docs/adr/README.md",
  "docs/research/canonical-directory-durability-2026-08-14.md",
  "docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-8e73-wave7b-plan.md",
  "docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-1189-report.md",
];
const missing = [];
for (const file of files) {
  const body = readFileSync(file, "utf8");
  const targets = [...body.matchAll(/\]\(([^)]+)\)/g)].map((m) => m[1])
    .concat([...body.matchAll(/^\[[^\]]+\]:\s+(\S+)/gm)].map((m) => m[1]));
  for (const raw of targets) {
    const target = raw.split("#", 1)[0];
    if (!target || /^[a-z][a-z0-9+.-]*:/i.test(target)) continue;
    if (!existsSync(resolve(dirname(file), target))) {
      missing.push(file + " -> " + raw);
    }
  }
}
if (missing.length) {
  console.error(missing.join("\n"));
  process.exit(1);
}
console.log("Owned Markdown relative links resolve: " + files.length + " files");
BUN
```

Real result:

```text
ADR-0037 index row/reference: PASS
ADR-0037 lineage warning: PASS
Owned Markdown relative links resolve: 5 files
```

Result: PASS.

### Clean-worktree-safe pinned Seeds and `verify:fast`

The assigned worktree has no `node_modules`, so the literal local candidate is
absent. No install, patch, or symlink was used. Exact command:

```bash
seeds_cli_root="$PWD/node_modules/@os-eco/seeds-cli"
if ! test -f "$seeds_cli_root/package.json" ||
   ! test -f "$seeds_cli_root/src/output.ts"; then
  seeds_cli_root="/home/codeseys/DevBox/audio-graph/node_modules/@os-eco/seeds-cli"
fi
test -f "$seeds_cli_root/package.json"
test -f "$seeds_cli_root/src/output.ts"
test "$(jq -r '.name + "@" + .version' "$seeds_cli_root/package.json")" = \
  '@os-eco/seeds-cli@0.4.5'
realpath "$seeds_cli_root"
SEEDS_CLI_ROOT="$seeds_cli_root" bun run verify:fast
```

Real result:

```text
/home/codeseys/DevBox/audio-graph/node_modules/@os-eco/seeds-cli
@os-eco/seeds-cli@0.4.5
Checked 174 files. No fixes applied.
audio source contract is current
provider registry is current
session data movement contract is current
endpoint credential routing contract is current
speech span revision contract is current
sd ready --format json: parsed (50)
sd blocked --format json: parsed (103)
sd list --format json: parsed (50)
docs/Seeds secret hygiene scan passed: 0 findings
```

Result: PASS, exit 0. The gate executed all five contract-drift checks.

### Secret hygiene and Betterleaks

Commands:

```bash
bun scripts/check-docs-secret-hygiene.mjs
betterleaks dir --no-banner --redact \
  docs/adr/0037-freeze-canonical-event-stream-registry.md \
  docs/adr/README.md \
  docs/research/canonical-directory-durability-2026-08-14.md \
  docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-8e73-wave7b-plan.md \
  docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-1189-report.md
```

Real result:

```text
docs/Seeds secret hygiene scan passed: 0 findings
Betterleaks final owned-file bytes: 75355
no leaks found
```

Result: PASS.

### Status, scope, and diff hygiene

Pre-commit commands:

```bash
git status --short
git add docs/adr/0037-freeze-canonical-event-stream-registry.md \
  docs/adr/README.md \
  docs/research/canonical-directory-durability-2026-08-14.md \
  docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-8e73-wave7b-plan.md \
  docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-1189-report.md
git diff --cached --check
git diff --cached --name-status c7d3fec2db8c60629e0b7c8b93e752c3aee85368
git diff --cached --name-status 6f51f2626ab19740746dfb84e0678fefb7ec7876
```

Real result for the staged snapshot:

```text
Base-range path count: 5
Correction-range path count: 4
Product/Seeds/workflow/package/generated/unrelated-ADR paths: 0
git diff --cached --check: PASS

Correction range from 6f51f26:
M  docs/adr/README.md
M  docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-1189-report.md
M  docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-8e73-wave7b-plan.md
A  docs/research/canonical-directory-durability-2026-08-14.md

Full range from c7d3fec:
A  docs/adr/0037-freeze-canonical-event-stream-registry.md
M  docs/adr/README.md
A  docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-1189-report.md
A  docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-8e73-wave7b-plan.md
A  docs/research/canonical-directory-durability-2026-08-14.md
```

Post-commit `git status --short`, exact tip, commit count, merge count, and
base-range name-status are captured in the worker handoff because a commit
cannot contain its own object id.

## Findings and open questions

- No unrelated defect was found inside the expanded documentation/research
  scope.
- APFS namespace qualification and Windows typed refusal remain executable
  evidence owned by downstream `audio-graph-2df3`; this documentation wave
  makes no platform-execution claim.
- There is no D0 open question. The custody Seed correction is an integration
  prerequisite already recorded outside this branch, not a worker-owned edit.

## Git handoff

- Base: `c7d3fec2db8c60629e0b7c8b93e752c3aee85368`.
- Initial commit: `6f51f2626ab19740746dfb84e0678fefb7ec7876`.
- Correction: a separate compliant round-1 commit follows the initial commit.
- Exact corrected tip and clean status are reported after commit in the worker
  handoff.
