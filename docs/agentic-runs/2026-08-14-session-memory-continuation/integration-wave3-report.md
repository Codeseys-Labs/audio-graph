# Session Memory Continuation Wave 3 Integration Report

Date: 2026-08-14

Integration branch: `integration/session-memory-wave-20260814`

Exact starting tip: `7ae7f484ec7c8f2f420f6082f0c4382cc7a7bdf8`

Assembled pre-report tip: `7ac461d393914b0de145eebeb49bcb119b0a93b7`

No branch was pushed, no workflow was dispatched, no release was created, and
no Seed was closed during Wave 3 integration.

## Inputs and footprint verdicts

The integration worktree and both source worktrees were clean and at their
exact declared tips before fan-in.

| Input | Merge base | History scope | True contribution | Review / disposition |
| --- | --- | --- | --- | --- |
| Custody checkpoint `e88aca23c82f448f58a90ff916a0e15a4115cd8a` | existing custody ancestor `e7baebbcfb32a26bd7e6bc2dc70cfccce68b3163` | 6 linear commits, 0 merges | only `.seeds/issues.jsonl`, 11 insertions and 8 deletions | **landed** as merge parent in `c10d5bf13f8afbe73f982b77cc86407a8edaf3ad` |
| `research/cerebras-openrouter-projection-transports` at `29e42bf1b3e6b4524bef875069b08dfedc44f20c` | `475a1dd3d84abcc56ef2483ac7b3f66e02c6e315` | 2 linear commits, 0 merges | one 414-line research note, 23,811 bytes | Standards **SHIP**, Spec **SHIP** after correction; **landed** as merge parent in `21880cd19585b1c5abf2b9860d146c47e78af5c4` |
| `work/edc8-speaker-replay-wave3` at `9cdb84e2d6caa3516250a6d2785603c3547bb2aa` | `7ae7f484ec7c8f2f420f6082f0c4382cc7a7bdf8` | 2 linear commits, 0 merges | report plus `commands.rs`, `persistence/mod.rs`, and `projections.rs`; 1,074 insertions and 13 deletions | Spec **SHIP**, Standards **SHIP-WITH-NITS**; nit tracked as `audio-graph-f451`; **landed** as merge parent in `7ac461d393914b0de145eebeb49bcb119b0a93b7` |

No input was reverted or skipped. All histories were preserved through
non-fast-forward three-way merges; nothing was squashed, rebased, or rewritten.
No merge produced a conflict.

Post-merge deltas proved that the old-base research history added only its one
note and did not materialize the branch's stale absence of later code, docs, or
Seeds. Neither reviewed branch changes `.seeds`, credentials, credential-v2
source, vendored dependencies, `node_modules`, build output, environment files,
placeholders, dead/loud stubs, `todo!`, or `unimplemented!`.

The implementation artifacts are substantive rather than placeholders: the
report is 12,972 bytes and the three resulting source files are 674,368,
240,481, and 198,607 bytes. The complete assembled Wave 3 delta before this
report is 6 files, 1,499 insertions, and 21 deletions.

## Semantic assembly invariants

Static invariants and focused tests prove all required boundaries:

- the current runtime projection scheduler still derives transcript-only
  bases, and live apply still calls the transcript-only validation seam;
- historical projection replay accepts presence-bearing speaker history;
- speaker revisions with equal `received_at_ms` retain canonical input order
  through stable received-time-only sorting;
- patch-time speaker visibility is inclusive and historical patches remain in
  canonical projection-stream order;
- `canonical_log` and `canonical_reader` each have exactly one module
  declaration;
- the research artifact is one evidence note whose status says no product
  contract was chosen and whose body explicitly refuses to select AudioGraph's
  production transport contract.

The transcript-only runtime guard is currently source-text based. That
non-blocking test-quality nit is recorded as the post-milestone Seed
`audio-graph-f451`; it does not change the runtime behavior or Wave 3 acceptance.

## Focused implementation gates

Rust commands used Rust 1.95, `--locked`, cloud-only features, one test thread,
and the idle 8.4 GB implementation-worktree target cache.

| Filter | Result |
| --- | --- |
| `materialized_projection_history_` | 11 passed, 0 failed, including equal-time correction |
| `runtime_projection_scheduler_and_apply_remain_transcript_only` | 1 passed, 0 failed |
| `speaker_timeline` | 7 passed, 0 failed |
| `projection_replay` | 6 passed, 0 failed |
| repository speaker-bearing replay | 1 passed, 0 failed |
| projection-report speaker-bearing replay | 1 passed, 0 failed |
| `load_session` speaker-bearing replay | 1 passed, 0 failed |
| mixed transcript/speaker/projection strict reload | 1 passed, 0 failed |
| `strict_reader_` | 22 passed, 0 failed |

The 11-test historical filter includes inclusive equal-time visibility, stable
same-time correction ordering, missing versus present-empty speaker authority,
speaker append/revision/repair, content-free replay failures, and regressing
patch timestamps.

## Full assembled gates

### Backend

```text
cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud --locked
cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud --locked -- --test-threads=1
cargo +1.95.0 clippy --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud --locked -- -D warnings
cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --all -- --check
```

Results:

- locked cloud library/test check: passed in 23.80 seconds;
- full direct locked cloud library suite: 1,555 passed, 0 failed, 8 ignored in
  39.14 seconds;
- strict cloud Clippy with `-D warnings`: passed in 38.67 seconds;
- rustfmt: passed with no diagnostics.

### Frontend and generated contracts

```text
bun run test:local
bun run verify:fast
bun run verify:contracts
```

Results:

- exact local Vitest: 70/70 files and 962/962 tests in 109.89 seconds;
- Biome: 171 files checked with no fixes;
- TypeScript typecheck: passed;
- all four generated contracts were current in both `verify:fast` and the
  explicit contract run;
- Seeds output stress, docs/Seeds secret hygiene, and diff hygiene passed.

The frontend run emitted only the existing non-fatal JSDOM navigation notices.

## Research validation

The note contains 40 first-party citation occurrences and exactly 33 unique
targets across official Cerebras and OpenRouter sites/APIs. A live GET check on
2026-08-14 returned HTTP 200 for all 33; no live-link drift was found.

The current OpenRouter OpenAPI also passed these assertions:

- `POST /messages` exists as operation `createMessages`;
- its JSON request references `#/components/schemas/MessagesRequest`;
- `MessagesRequest` requires `model` and `messages`;
- `IncompleteDetails.reason` includes `max_output_tokens`.

No authenticated or content-bearing provider request was made. The report
remains research input rather than a selected product contract.

## Security, Seeds, and hygiene

Betterleaks scanned approximately 1.15 MB across the three changed source files
and two accepted reports in 444 ms and found no leaks. The repository
docs/Seeds secret-hygiene scan reported 0 findings.

Every Seeds JSONL row parsed. `ready` and `blocked` JSON envelopes parsed with
counts 50 and 88, and the pinned output stress check passed. Seeds Doctor
reported 10 checks passed, 2 warning groups, and 0 failures. The warnings remain
the eight bidirectional-link mismatches and three older closed records without
`closedAt`; this integrator did not repair or otherwise edit the queue beyond
the exact custody checkpoint.

`git diff --check 7ae7f484ec7c8f2f420f6082f0c4382cc7a7bdf8..HEAD`
passed with no diagnostics.

## Seeds handoff

The custody checkpoint records both accepted Wave 3 Seeds as `in_progress`;
closure remains conductor-owned.

- `audio-graph-edc8`: eligible for root closure after reconciliation because
  the corrected reviewed history and all assembled semantic/focused/full gates
  are green.
- `audio-graph-3d0c`: eligible for root closure because the corrected one-note
  research history is integrated, 33/33 first-party links resolve, current
  OpenAPI assertions pass, and the artifact remains non-normative research.
- `audio-graph-f451`: remains open, priority 3, and `POST_MILESTONE` as the
  non-blocking behavioral-test follow-up.
- `audio-graph-fd9f`: remains open for Windows/macOS capture evidence and an
  approval-gated actual release dry run.
- `audio-graph-617e`: remains blocked by `audio-graph-464c` and
  `audio-graph-9751`.
- `audio-graph-464c`: remains blocked by directory-durability work
  `audio-graph-8e73`.
- `audio-graph-9751`: remains design-blocked on the resume boundary and blocked
  by `audio-graph-464c`.

No failure Seed was filed because no accepted landing or assembled product gate
failed.
