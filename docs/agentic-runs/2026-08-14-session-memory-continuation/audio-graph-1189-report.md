# audio-graph-1189 implementation report

Date: 2026-08-14

## Assignment

- Seed: `audio-graph-1189` — Restore accepted canonical stream registry
  ADR-0037 lineage.
- Worktree:
  `/home/codeseys/DevBox/audio-graph/.worktrees/1189-adr0037-wave7b`
- Branch: `work/1189-adr0037-wave7b`
- Exact base: `c7d3fec2db8c60629e0b7c8b93e752c3aee85368`
- Acceptance: restore exact ADR blob
  `6f94d5a9fb183afbef70826add08fc3c1f163f59` from non-ancestor commit
  `5487c3b`; add the current-format accepted index row dated 2026-07-10;
  record the historical/current ADR-0035/0036 lineage without modifying the
  archival blob or the current decisions; provide the exact 8e73 child plan;
  and change no product, Seeds, workflow, package, generated, or unrelated ADR
  path.

## Outcome

The accepted canonical event-stream registry is restored byte-for-byte and is
linked from the current ADR index. The detailed Wave 7B plan records the exact
durability dependency stack, ownership, gates, rollback and stop lines,
platform result boundary, and Blacksmith cleanup policy.

No product code, research artifact, Seed, workflow, dependency, generated
artifact, current ADR-0035/0036 byte, or integration checkpoint was changed.
This workstream did not close a Seed, merge, push, dispatch a workflow, or
start a Blacksmith job.

## RED evidence before edits

The pre-edit command proved:

```text
RED PASS: ADR-0037 file absent
RED PASS: ADR-0037 index entry absent
src-tauri/src/persistence/canonical_reader.rs:32:// ADR-0037 pins the IPC payload version supported by outer stream schema v1.
ADR-0035 current blob: 91ffb0304c06323be6254889d716e639ebc4d79e
ADR-0036 current blob: 3af2bcfeafe14d01544b4f122c10b8df78335fe2
ADR-0037 source blob: 6f94d5a9fb183afbef70826add08fc3c1f163f59
RED PASS: 5487c3b is not an ancestor of HEAD
```

This is a documentation-lineage defect: the current strict reader already
depends on the accepted registry values while the governing ADR and index row
were absent from the current ancestry.

## What changed

- Restored
  [ADR-0037](../../adr/0037-freeze-canonical-event-stream-registry.md)
  exactly from non-ancestor commit `5487c3b` using an explicit patch, without
  merging or cherry-picking unrelated history.
- Added only ADR-0037's current-format row and reference link to the existing
  ADR index. Every prior row and reference remains unchanged.
- Added the
  [Wave 7B plan](audio-graph-8e73-wave7b-plan.md), which records the exact
  `1189 -> c2e3 + 661f -> a596 -> 3b8b -> b77b -> 2df3` dependency order,
  scoped ownership and gates, rollback/stop boundaries, Linux/macOS/Windows
  result contract, and Testbox/Actions policy.
- Added this evidence report.

## Lineage clarification

The restored ADR-0037 is immutable archival content. Its references to
ADR-0035 and ADR-0036 name non-ancestor canonical-log framing and recovery
decisions that existed beside it at source commit `5487c3b`. The current
accepted ADR-0035 and ADR-0036 are distinct speech-semantics decisions created
on the current lineage. They are not the decisions referenced by historical
ADR-0037 prose.

The clarification is therefore recorded here and in the Wave 7B plan, not in
ADR-0037. Current decision blobs remain:

- ADR-0035: `91ffb0304c06323be6254889d716e639ebc4d79e`;
- ADR-0036: `3af2bcfeafe14d01544b4f122c10b8df78335fe2`.

## Files changed

- `docs/adr/0037-freeze-canonical-event-stream-registry.md`
- `docs/adr/README.md`
- `docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-8e73-wave7b-plan.md`
- `docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-1189-report.md`

## Gates and real results

### Archival and ADR invariants

Command group: `git hash-object` for restored/current ADRs, exact index-row and
reference counts, and `git diff --exit-code` from the base for current
ADR-0035/0036.

```text
ADR-0037 blob: 6f94d5a9fb183afbef70826add08fc3c1f163f59
ADR-0035 blob: 91ffb0304c06323be6254889d716e639ebc4d79e
ADR-0036 blob: 3af2bcfeafe14d01544b4f122c10b8df78335fe2
ADR-0037 index row/reference: PASS
```

Result: PASS. The restored blob exactly matches the source commit, the current
index contains one accepted 2026-07-10 row and one resolving reference, and
the two current speech-semantics ADRs are unchanged.

### Relative-link validation

A read-only Bun validator resolved every inline and reference-style relative
Markdown target in all four owned files.

```text
Owned Markdown relative links resolve: 4 files
```

Result: PASS.

### Repository-authoritative fast gate

The required literal command was attempted first:

```text
SEEDS_CLI_ROOT=$PWD/node_modules/@os-eco/seeds-cli bun run verify:fast
```

Biome, TypeScript, and all five contract checks passed, then the Seeds-output
step stopped with:

```text
error: SEEDS_CLI_ROOT is not a Seeds CLI package: /home/codeseys/DevBox/audio-graph/.worktrees/1189-adr0037-wave7b/node_modules/@os-eco/seeds-cli
```

The assigned clean worktree has no `node_modules`; no product check failed.
Per integration-owner direction, no symlink, install, dependency edit, or
package patch was permitted. The gate was rerun from the same worktree against
the existing absolute repository-pinned root:

```text
/home/codeseys/DevBox/audio-graph/node_modules/@os-eco/seeds-cli
@os-eco/seeds-cli@0.4.5
$ bun run check && bun run typecheck && bun run verify:contracts && bun run check:seeds-json-output && bun scripts/check-docs-secret-hygiene.mjs && git diff --check
Checked 174 files in 300ms. No fixes applied.
audio source contract is current
provider registry is current
session data movement contract is current
endpoint credential routing contract is current
speech span revision contract is current
Seeds CLI outputJson patch present: /home/codeseys/DevBox/audio-graph/node_modules/@os-eco/seeds-cli/src/output.ts
sd ready --format json: parsed (50)
sd blocked --format json: parsed (103)
sd list --format json: parsed (50)
docs/Seeds secret hygiene scan passed: 0 findings
```

Result: PASS, exit 0. `verify:fast` included all five generated-contract drift
checks, so no contract check was omitted.

### Secret hygiene and Betterleaks

Commands:

```text
bun scripts/check-docs-secret-hygiene.mjs
betterleaks dir --no-banner --redact docs/adr/0037-freeze-canonical-event-stream-registry.md docs/adr/README.md docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-8e73-wave7b-plan.md docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-1189-report.md
```

Real output:

```text
docs/Seeds secret hygiene scan passed: 0 findings
scanned ~35565 bytes (35.56 KB)
no leaks found
```

Result: PASS.

### Footprint and diff hygiene

The pre-commit scope and lineage check produced:

```text
Working-tree path count: 4
README numstat: 2  0  docs/adr/README.md
ADR-0037 current blob: 6f94d5a9fb183afbef70826add08fc3c1f163f59
ADR-0035 current blob: 91ffb0304c06323be6254889d716e639ebc4d79e
ADR-0036 current blob: 3af2bcfeafe14d01544b4f122c10b8df78335fe2
Current ADR-0035/0036 base diff: PASS
Forbidden product/Seeds/workflow/package/generated/ADR footprint: PASS
Lineage/non-conflation check: PASS
```

Result: PASS. After staging, `git diff --cached --check` and the exact
base-range name-status check are rerun against the report-bearing snapshot.

## Findings

- No unrelated defect was found inside the assigned documentation surfaces.
- The original ADR's reuse of numbers 0035 and 0036 is historical lineage, not
  a current decision relationship. Editing the accepted blob to clarify that
  would violate its exact-hash acceptance criterion, so the clarification is
  intentionally external.
- Discovery worker reports for this wave were message-only/read-only and had
  no committed artifact. The plan uses current code, exact Seed records, the
  current integration checkpoint/report, and the reviewed primary-source
  research artifact as authority.

## Open questions

- None for D0. APFS namespace qualification and Windows typed refusal remain
  executable evidence owned by downstream Seed `audio-graph-2df3`; they are
  not claims made by this documentation restore.

## Git handoff

- Base: `c7d3fec2db8c60629e0b7c8b93e752c3aee85368`.
- Review snapshot: the single report-bearing commit created from the four-file
  footprint above; its exact commit id and clean status are reported in the
  worker handoff because a commit cannot contain its own object id.
