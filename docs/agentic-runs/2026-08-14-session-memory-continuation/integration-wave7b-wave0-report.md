# Wave 7B Wave0 integration report

Date: 2026-08-15

## Outcome

Wave 7B Wave0 assembled without conflict on
`integration/session-memory-wave-20260814`. Custody
`dd038da1688e8e91025cc94dc76c66c6a7923741` was merged
history-preservingly at `98d40133083b4d3919ff919888689498934c2880`.
Reviewed D0 tip `bc2da20661f838429097b7508cb0f96cc0f28ab2` was then
merged history-preservingly at `367d344d81e1768ca8a3774f58fb9375750b1046`.

The D0 snapshot already contains the corrected research blob reviewed at
`30490249ef08fb59a4b0bebfd855009c24f90117`. That research commit is not an
ancestor of the assembled branch and was not merged separately.

## Input and footprint validation

The starting integration tip was the exact clean
`c7d3fec2db8c60629e0b7c8b93e752c3aee85368`.

- Custody is one linear, non-merge commit after `bb7c982`, and its only path is
  `.seeds/issues.jsonl`. Its queue contains 639 valid JSONL rows.
- D0 has exact merge-base `c7d3fec`, two linear non-merge commits, and a clean
  owning worktree. Its true contribution is 1,195 added lines across exactly
  five authorized paths: ADR-0037, the ADR index, the corrected research note,
  the 8e73 Wave 7B plan, and the 1189 implementation report.
- The D0 footprint contains no Seed, workflow, product, generated, dependency,
  credential, vendor, or build-output path. Placeholder, whitespace, and
  Betterleaks preflight checks passed. Standards and Spec both returned
  **SHIP** after the recorded correction round.

## Assembled semantic assertions

The integrated Seeds file is byte-identical to custody and remains valid with
639 rows. Seed `audio-graph-1189` retains its active ownership, initial blocking
review, bounded correction footprint, corrected research tip and review, and
exact ADR blob requirements.

- Corrected research blob:
  `10e0049246f78ba2b2aa31abc67cd95f0866225b`.
- Restored immutable ADR-0037 blob:
  `6f94d5a9fb183afbef70826add08fc3c1f163f59`.
- Current ADR-0035 and ADR-0036 remain unchanged from the integration base at
  blobs `91ffb0304c06323be6254889d716e639ebc4d79e` and
  `3af2bcfeafe14d01544b4f122c10b8df78335fe2`.
- The ADR index has exactly the accepted ADR-0037 row and file link plus a
  discoverable warning that the archival ADR's internal ADR-0035/0036 names
  belong to the non-ancestor canonical-log lineage. The warning links the Wave
  7B plan.
- The plan distinguishes the five 8e73 durability children (`1189`, `c2e3`,
  `3b8b`, `b77b`, and `2df3`) from the two be7c-owned manifest prerequisites
  (`661f` and `a596`).
- The ordered crash-cut sequence includes before/after write, userspace flush,
  file sync, new-entry sync, quarantine rename and directory sync, manifest
  prepare, source truncate and sync, manifest completion, and distinct pre- and
  post-acknowledgement cuts.
- Blacksmith evidence must record monitoring and exits, explicitly stop every
  disposable Testbox and verify an empty active list, fall back to the existing
  macOS/Windows Actions jobs when Testboxes are unsuitable, and make no
  workflow mutation without separate authorization.

## Gates

| Gate | Result |
| --- | --- |
| assembled Markdown relative links | 8 files and 52 local targets resolved |
| Seeds ready-all / blocked | 90 ready; 103 blocked |
| Seeds JSON output stress | ready 50, blocked 103, and list 50 parsed |
| Seeds Doctor | 10 passed, 2 custody-carried warning groups, 0 failures |
| all generated contracts | all five current |
| pinned `SEEDS_CLI_ROOT` `verify:fast` | passed; Biome checked 174 files, typecheck/contracts/output/secret/diff green |
| Betterleaks | approximately 2.41 MB scanned; no leaks |
| docs/Seeds secret hygiene | 0 findings |
| exact range diff | passed `git diff --check` |

The first ADR-0035/0036 immutability command used stale filenames and stopped
before assembly. Resolving the paths from the exact base tree and comparing
their blobs directly passed. This was an assertion-harness error, not a
snapshot or product failure.

No product suite was run because no product file changed. No Blacksmith job or
Testbox was started, no workflow was created, edited, or dispatched, and no
Seed was closed or additionally edited.

## Queue handoff

`audio-graph-1189` remains `in_progress`, is the `ACTIVE_MILESTONE`, and is
assigned to `codex-1189-wave7b`; integration does not close it. D1
`audio-graph-c2e3` and M0 `audio-graph-661f` remain open and each is blocked
only by 1189. After root reconciles the integrated SHIP evidence and closes
1189, c2e3 and 661f become the next parallel queue-eligible workstreams.
