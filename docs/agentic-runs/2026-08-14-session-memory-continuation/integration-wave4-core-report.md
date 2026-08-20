# Session Memory Continuation Wave 4 Core Integration Report

Date: 2026-08-14

Integration branch: `integration/session-memory-wave-20260814`

Exact starting tip: `f82052d30b8d34e451b078aec2fe371e99d5bebd`

Assembled pre-report tip: `9065146c4344bdc052a029c4afe43d68e369c03f`

No branch was pushed, no workflow was dispatched, and no Seed was closed or
edited outside the accepted custody history.

## Inputs and footprint verdicts

The integration and implementation worktrees were clean at their exact
declared tips before fan-in.

| Input | Merge base | History scope | True contribution | Disposition |
| --- | --- | --- | --- | --- |
| Custody `10cc6b60b3b0d1c4c519ec75b8394be0cba52459` | `4d806eea29c8453d99b420cdd2355fbe5172b3c3` | 3 linear commits, 0 merges | only `.seeds/issues.jsonl`, 2 insertions and 2 deletions | landed at `063984c8b60a52d9ce5e4f78e996696297dba21a` |
| Reviewed `work/4dbb-speech-span-contract-wave4` at `a68c0befd4471f718bdc0fdfa95efde6f0dad29b` | exact integration base `f82052d30b8d34e451b078aec2fe371e99d5bebd` | 4 linear commits, 0 merges | 15 files, 2,685 insertions and 8 deletions | landed at `9065146c4344bdc052a029c4afe43d68e369c03f` |

Both inputs were merged history-preservingly with three-way Git. Neither merge
had a conflict. The reviewed footprint contains its implementation report,
Rust contract and compatibility seams, generator, generated TypeScript, and
package wiring. It contains no `.seeds`, workflow, credential, environment,
vendor, dependency-install, or build-output path. No added placeholder,
`todo!`, or `unimplemented!` marker was found.

The artifacts are substantive: the accepted report is 13,276 bytes, the IPC
contract module is 45,228 bytes, the application compatibility module is
26,887 bytes, and the generated TypeScript contract is 15,286 bytes.

## Semantic assembly invariants

Static inspection and focused tests prove the accepted boundaries:

- callers create v2 rows only through
  `SpeechSpanRevisionNormalizer::admit(SpanObservation)`; raw parts and
  `try_from_parts` are private;
- each nested fidelity union rejects unknown fields, and public decode applies
  the same strict v2 validation;
- admitted rows serialize explicit `contract_version: 2` and nested timing,
  confidence, turn, speaker, and channel authority without co-authoritative
  legacy scalars;
- a missing contract version selects legacy-v1 compatibility without
  fabricating unavailable values;
- `ProjectionBasis` defaults a missing hash version to v1 and serializes
  explicit v1, while the frozen v1 transcript hashes remain
  `fnv1a64:4eb27818db1f8b3d` and `fnv1a64:1708ff3ca940aa59` for their fixtures;
- framed v1 transcript bytes decode through compatibility, replay through the
  ledger, produce and validate a basis, and remain unchanged;
- Rust plus schema generation owns the TypeScript contract;
  `SpeechSpeakerValue` accepts id-only, label-only, or both, and TypeScript
  rejects empty and all-null values;
- no adapter, readiness, UI, provider-selectability, or production writer path
  was activated by this child.

## Assembled gates

Rust used Cargo 1.95, `--locked`, cloud-only features, and the reviewed idle
worker target cache. The full library suite was explicitly serialized with
`-- --test-threads=1`.

| Gate | Result |
| --- | --- |
| `audio-graph-ipc-contract` | 17 passed, 0 failed; exporter bins passed; compile-fail doctest 1/1 passed |
| focused `speech_span_revision` | 8 passed, 0 failed |
| focused ProjectionBasis v1 hash/version | 1 passed, 0 failed |
| focused `canonical_reader` | 8 passed, 0 failed |
| locked cloud library/test check | passed in 2 minutes 31 seconds |
| full direct locked cloud library suite, serialized | 1,566 passed, 0 failed, 8 ignored in 38.58 seconds |
| strict cloud Clippy with `-D warnings` | passed in 37.61 seconds |
| rustfmt | passed with no diagnostics |
| speech contract generate then check | generated deterministically; check reported current |
| complete generated-contract suite | all five contracts current |
| TypeScript and Biome | typecheck passed; 173 files checked with no fixes |
| exact `bun run test:local` | 70/70 files and 962/962 tests in 111.25 seconds |

The frontend run emitted only the existing non-fatal JSDOM navigation notices.

## Seeds CLI boundary, queue, and hygiene

`SEEDS_CLI_ROOT` was set to the repository-pinned
`node_modules/@os-eco/seeds-cli` package for the authoritative
`bun run verify:fast` run. Biome, typecheck, all five contracts, ready/blocked/
list JSON stress, docs/Seeds secret hygiene, and diff hygiene passed.

The unrelated machine-global package remains drifted. A read-only check with
that package selected exited 1 because its `output.ts` lacks the pipe-safe
stdout retry patch. The worker's unoverridden global-fallback result and the
do-not-mutate boundary are already recorded on `audio-graph-9e23`; this
integration did not modify the global install.

The integrated Seeds file is byte-identical to custody and all 624 JSONL rows
parse. The complete queue contains 91 ready and 91 blocked issues. Seeds
Doctor reported 10 checks passed, 2 pre-existing warning groups, and 0
failures.

Betterleaks scanned approximately 805 KB across the reviewed footprint and
found no leaks. Docs/Seeds secret hygiene reported 0 findings, and
`git diff --check f82052d30b8d34e451b078aec2fe371e99d5bebd..HEAD`
passed.

## Handoff

`audio-graph-4dbb` remains conductor-owned for closure and is eligible after
root reconciliation of this integrated evidence. `audio-graph-ada2` remains
`in_progress`. After 4dbb closure and a custody refresh,
`audio-graph-48de` and `audio-graph-98ef` can proceed in parallel;
`audio-graph-fcca` remains blocked by 98ef. `audio-graph-9e23` remains open for
the global-CLI/tooling follow-up and its broader validation-facade acceptance.

No integration failure Seed was filed because both landings and every required
assembled product gate passed.
