# Session Memory Continuation Wave 7A Integration Report

Date: 2026-08-14

Integration branch: `integration/session-memory-wave-20260814`

Exact starting tip: `72e23b506d6f4d2e465aeebfb452d20fbbc0bfe5`

Assembled pre-report tip: `7a8c80c36105f8509c9904a798ece2f5eb32c9aa`

The established continuation directory was used. No Seed was edited or closed,
no accepted ADR/design was altered, no workflow was dispatched, and no branch
was pushed.

## Inputs and footprint verdicts

The integration worktree and both reviewed source worktrees were clean and at
their exact declared tips before fan-in.

| Input | Exact merge base | History scope | True contribution | Review / disposition |
| --- | --- | --- | --- | --- |
| `work/ab64-hash-v2-kernel-wave7a` at `74924bc527ab1b22591aa30c945b3c2363e329e4` | `72e23b506d6f4d2e465aeebfb452d20fbbc0bfe5` | 3 linear commits, 0 merges | exactly 7 authorized report/verifier/IPC/fixture/app-Rust paths; 2,865 insertions and 43 deletions | Standards **SHIP**, Spec **SHIP**; landed at `de623153389f53d07b774938b9433b3a2c409276` |
| `work/5e41-admission-fencing-prototype-wave7a` at `faefecee2e7c492457c4d5a14e5da999dd0e796c` | `72e23b506d6f4d2e465aeebfb452d20fbbc0bfe5` | 4 linear commits, 0 merges | exactly 3 prototype script/design/report paths; 3,445 insertions | Standards **SHIP**, Spec **SHIP**; landed at `7a8c80c36105f8509c9904a798ece2f5eb32c9aa` |

Both branches were merged history-preservingly with non-fast-forward three-way
Git. Neither merge had a conflict. No input contains a Seed, workflow,
credential, dependency-install, vendor, generated frontend, or build-output
path. Artifact sizes are plausible for the exhaustive fixture/model evidence,
and no executable path contains a placeholder, `todo!`, `unimplemented!`, or
`FIXME` implementation.

An initial broad keyword scan matched the 5e41 model's intentional malicious
negative-fixture strings such as transcript, bearer, credential, and secret.
Those values exercise content-free diagnostic rejection and are not
credentials. The narrower placeholder/build scan and final Betterleaks gate
passed.

## Semantic boundaries

The hash-v2 contribution is a dormant conformance kernel:

- validated Speech Span Revision access and normalization preserve only
  projection-semantic fields;
- positioned inputs use first canonical Accepted sequence for ordering but do
  not hash the sequence value;
- the encoder fails closed on impossible cross-payload evidence, invalid
  positions/order, non-finite evidence, and invalid supersession;
- all four active design goldens are shared with an independent Bun
  implementation, while frozen hash-v1 evidence remains unchanged; and
- repository search finds no production call to
  `projection_basis_hash_v2`; outside its implementation/verifier, the module
  is exposed from `lib.rs` and invoked only by tests in
  `speech_span_revision.rs`.

No ledger, Projection Basis, prompt, scheduler, persistence, writer, Session
floor, provider adapter, command, state, frontend, or generated contract was
activated.

The 5e41 artifact remains a throwaway non-production model. Its only import is
`node:assert/strict`; it does not import, read, write, or invoke production
code, files, network, or persistence. It explores policy parameters but does
not select them, introduce runtime Session semantics, or claim operating-system
durability.

## Integrated gates

All Rust commands used Cargo 1.95, `--locked`, cloud-only features, and the
integration worktree target. The full direct library suite was run exactly once
and serialized with `-- --test-threads=1`.

| Gate | Result |
| --- | --- |
| focused `projection_basis_hash_v2` | 6 passed, 0 failed |
| IPC contract | 19 passed, 0 failed; compile-fail doctest passed |
| application `speech_span_revision` normalization | 12 passed, 0 failed |
| frozen Projection Basis hash v1 | 1 passed, 0 failed; `fnv1a64:4eb27818db1f8b3d` preserved |
| frozen canonical-reader compatibility | 8 passed, 0 failed |
| independent Bun verifier | 4 exact SHA-256 design goldens reproduced |
| 5e41 syntax | `node --check` passed |
| 5e41 executable finite model | 814 cases, 7,671 transitions, 1,262 states, 111,905 assertions, and 31 invariant families passed |
| locked cloud lib/test check | passed; first integrated dependency check completed in 1 minute 59 seconds |
| full direct locked cloud library suite, serialized | 1,580 passed, 0 failed, 8 ignored in 38.03 seconds |
| strict cloud Clippy with `-D warnings` | passed in 41.08 seconds |
| rustfmt | passed with no diagnostics |
| all generated contracts | all five current |
| TypeScript and Biome | typecheck passed; 174 files checked with no fixes |
| pinned `SEEDS_CLI_ROOT` `verify:fast` | passed without global CLI mutation |

## Seeds, security, and hygiene

The 632-row Seeds file is byte-identical to the Wave 7A base. The complete
queue contains 90 ready and 96 blocked issues. Seeds Doctor reported 10 passed,
2 custody-carried warning groups, and 0 failures; ready/blocked/list JSON output
stress also passed.

Betterleaks scanned approximately 344 KB across the complete ten-file accepted
footprint and found no leaks. Docs/Seeds secret hygiene reported 0 findings.
The exact range passes `git diff --check`, contains no forbidden build,
credential, vendor, workflow, ADR/design, generated frontend, or extra Seed
surface, and preserves all accepted base files outside the ten authorized
paths.

## Handoff and unresolved policy decisions

`audio-graph-ab64` and `audio-graph-5e41` remain conductor-owned and are
eligible for closure after this integrated evidence is reconciled. Their
closure makes the next dependency work visible without closing the parent:
`audio-graph-8e73` follows 5e41, while later hash/ledger work still follows the
recorded dependency graph. `audio-graph-4249` remains `in_progress` and blocked
on its children; no production hash-v2 or Session-floor activation is present.

The prototype proves all eight profiles safety-valid and deliberately leaves
three human policy decisions unresolved:

1. whether the user-facing durable label is `Saved` or `Durably saved`;
2. whether an externally uncertain remote request may ever be reissued without
   route-specific provider idempotency evidence and explicit duplicate
   cost/content-egress authorization; and
3. whether deletion defaults to immediate fenced discard or offers a fenced
   wait mode that observes but never applies remote results.

Cross-platform directory-entry durability, typed quarantine registration, and
the exact predecessor canary remain later executable evidence; this wave makes
no claim for them.

## Final Seed reconciliation

Custody `faa8fe69a739220da05c8e859dc219a8db98d240` was merged
history-preservingly at `30fcc90b8057b848c7e4c619d67ccd774a7fb2f3`.
Its one linear commit after custody `bcc70d4` changes only
`.seeds/issues.jsonl`; the integrated 632-row queue is byte-identical to
custody and remains valid JSONL.

`audio-graph-ab64` is closed with the exact reviewed implementation tip,
integration tip, SHIP verdicts, focused/full Rust, four-golden, Clippy, fmt,
and contract evidence. It is no longer a blocker for 0baf or 4249.
`audio-graph-0baf` is now blocked only by `audio-graph-7e81`.

`audio-graph-5e41` remains `in_progress` and
`AWAITING_HUMAN_DECISION`. Its durable evidence retains the exact 814 cases,
7,671 transitions, 1,262 states, 111,905 assertions, and 31 families, plus the
same three recommendations. None was selected by integration. Seed 4249
remains `in_progress`; its next executable dependency is 8e73 only after the
5e41 policy decision and closure. Seed 8e73 remains blocked by 5e41.

The complete queue remains 90 ready and 96 blocked. Seeds Doctor again
reported 10 passed, 2 custody-carried warning groups, and 0 failures. Seeds
output stress, all five contracts, pinned `verify:fast`, Betterleaks,
docs/Seeds secret hygiene, and range diff passed. Product files did not change,
so no product suite was rerun.

## Wave 7B policy-accepted base

Custody `333c7f2e090aacf493dbdad50d80601185c0bd09` was merged
history-preservingly at `cc7aa79c8ef0a161c3ee7cadd2672d52114efde2`.
Its one linear commit after custody `faa8fe6` changes only
`.seeds/issues.jsonl`; the integrated 632-row queue is byte-identical to
custody and remains valid JSONL.

`audio-graph-5e41` is closed after human acceptance of all three prototype
recommendations: display `Saved` only after durable `Accepted` or
`AlreadyAccepted`; never automatically reissue externally uncertain remote
work without provider idempotency proof plus explicit cost and content-egress
authorization; and immediately fence a deleted Session and discard late
remote results. Its dependency was removed from 8873, 3b48, 44c1, 8e73, and
7e81. Seed 8e73 is now the `ACTIVE_MILESTONE`, assigned to
`codex-8e73-wave7b`; 7e81 is blocked only by 8e73, and 4249 records the Wave 7B
implementation sequence.

The complete queue contains 90 ready and 95 blocked issues. Seeds Doctor
reported 10 passed, 2 custody-carried warning groups, and 0 failures. Seeds
output stress, all five contracts, pinned `verify:fast`, Betterleaks,
docs/Seeds secret hygiene, and range diff passed. Product files did not change,
so no product suite was rerun.

## Wave 7B split-planning base

Custody `bb7c98244083f984518eedaac35143d523eece19` was merged
history-preservingly at `f44c0b335ea6db23ed41ab3e3a547ab851036a80`.
Its one linear commit after custody `333c7f2` changes only
`.seeds/issues.jsonl`; the integrated 639-row queue is byte-identical to
custody and remains valid JSONL.

The durability stack now starts with active D0 `1189`. D1 `c2e3` and manifest
model M0 `661f` follow D0 in parallel; manifest kernel M1 `a596` follows both;
recovery R1 `3b8b` follows D1 and M1; test harness T1 `b77b` follows R1; and
platform qualification CI1 `2df3` follows T1. Parent `8e73` is blocked on CI1.
Seed `464c` now depends only on D1 and M1, while `be7c` records M0 and M1.
Blacksmith work must be monitored, explicitly stopped and verified inactive,
and may not mutate workflows without separate approval. No detailed 8e73
implementation plan or product, ADR, research, or workflow change was added.

The complete queue contains 90 ready and 103 blocked issues. Seeds Doctor
reported 10 passed, 2 custody-carried warning groups, and 0 failures. Seeds
output stress, all five contracts, pinned `verify:fast`, Betterleaks,
docs/Seeds secret hygiene, and range diff passed. Product files did not change,
so no product suite was rerun.

## Wave 7B Wave0 integration

Custody `dd038da` and reviewed D0 tip `bc2da20` were merged without conflict at
`98d4013` and `367d344`. The 639-row queue is byte-identical to custody. The
corrected research and immutable ADR-0043 blobs, unchanged current
ADR-0041/0042, lineage warning, five-versus-two ownership split, complete crash
cuts, and Blacksmith stop/list-clean/no-workflow controls were verified on the
assembled snapshot. The reviewed research branch was not merged separately.

All 52 local relative-link targets across eight assembled documents, all five
contracts, Seeds output and doctor, pinned `verify:fast`, Betterleaks, secret
hygiene, and range diff passed. Seed 1189 remains active pending root
reconciliation; c2e3 and 661f remain blocked only by 1189. Exact evidence is in
[`integration-wave7b-wave0-report.md`](integration-wave7b-wave0-report.md).
