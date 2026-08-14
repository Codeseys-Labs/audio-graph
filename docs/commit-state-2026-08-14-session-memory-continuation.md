# Session Memory implementation continuation

Date: 2026-08-14

## Fixed base

- Integration branch: `integration/session-memory-wave-20260814`
- Verified starting commit: `a4d0c20d677065f7ebe9112e37259e95b67aca87`
- The integration worktree was clean at the start of this continuation.
- Custody `master` is `e7baebbcfb32a26bd7e6bc2dc70cfccce68b3163` and retains the pre-existing untracked agent, preview-harness, backlog, and plan artifacts. It remains custody-only.
- No product commit from this continuation will be written directly in the custody checkout.

## Accepted evidence at the base

- Exact frontend gate: 70 files and 962 tests passed.
- Locked cloud Rust suite: 1,543 passed, 0 failed, 8 ignored.
- Strict Clippy, rustfmt, `verify:fast`, generated contracts, Seeds output checks, secret hygiene, and full-range diff hygiene passed.
- Independent Standards and Spec review accepted both completed waves after one bounded correction round per affected workstream.

## Queue snapshot

- Complete ready queue: 93 issues; blocked queue: 85 issues.
- `audio-graph-edc8` is the next dependency-complete Session Memory workstream: speaker-aware persisted Projection Basis replay.
- `audio-graph-617e` is a later executable session-export/projection-scheduler durability workstream and must be re-scoped before Act because it spans frontend export UX and backend queue persistence.
- Wayfinder decision tickets `audio-graph-70c8`, `audio-graph-5e41`, and `audio-graph-a668` remain design/prototype questions, not implementation work.
- `audio-graph-fd9f` remains open for Windows/macOS capture evidence and an approval-gated release dry run.
- `audio-graph-99eb` remains open; realtime speech-to-speech and unrelated credential work stay outside this continuation.

## Continuation contract

- Use TDD at public replay, command, persistence, and frontend-to-Tauri seams already named by the owning Seed.
- Use one clean worktree per implementation workstream, at most two implementation workers, one integration owner, and one review-fix round per workstream.
- Review stable committed tips on separate Standards and Spec axes before fan-in.
- Only the integration owner merges accepted branches and re-runs assembled gates.
- Close Seeds only after integrated acceptance evidence. Record every remaining blocker or follow-up in Seeds.
- Do not push, dispatch workflows, or run `sd sync` from the custody checkout without explicit authorization and a clean staged scope.

## Immediate wave

1. Discover and implement `audio-graph-edc8` from this documented base.
2. Review, integrate, and re-gate the accepted snapshot.
3. Refresh ready/blocked Seeds and select the next decision-complete Session Memory workstream by milestone impact and bounded effort.

## Wave 3 assembled status

Custody checkpoint `e88aca2`, corrected research tip `29e42bf`, and corrected
speaker-replay tip `9cdb84e` were integrated without conflict or history
rewrite. The assembled pre-report tip is
`7ac461d393914b0de145eebeb49bcb119b0a93b7`.

Historical replay now accepts strict speaker history while current runtime
scheduling and live apply remain transcript-only. Equal-received-time speaker
corrections preserve canonical order. The Cerebras/OpenRouter artifact remains
Wayfinder research input and selects no product transport contract.

All focused replay/reader, full locked cloud Rust, strict Clippy, rustfmt, exact
frontend, generated-contract, 33-link/OpenAPI, Betterleaks, Seeds, secret, and
diff gates passed. Exact evidence is recorded in
[`integration-wave3-report.md`](agentic-runs/2026-08-14-session-memory-continuation/integration-wave3-report.md).

`audio-graph-edc8` and `audio-graph-3d0c` are eligible for conductor closure.
`audio-graph-f451` remains `POST_MILESTONE`; `audio-graph-fd9f` remains open for
external evidence; and `audio-graph-617e`, `audio-graph-464c`, and
`audio-graph-9751` remain blocked as recorded. No workflow was dispatched and
no release was published.

## Final Wave 3 closure checkpoint

Custody closure commit `7969d02` was merged history-preservingly at
`21841cbd8302365ffe6e64265dd70f477c86cdac`. Its delta from `e88aca2` changes
only `.seeds/issues.jsonl`, and the integrated queue is byte-identical to
custody.

The current complete queue is 92 ready and 87 blocked. `audio-graph-edc8` and
`audio-graph-3d0c` are closed and absent from the open queues;
`audio-graph-f451` remains open and `POST_MILESTONE`; and
`audio-graph-617e`, `audio-graph-464c`, and `audio-graph-9751` retain their
recorded blockers. Seeds Doctor has 0 failures. Final `verify:fast`, generated
contracts, Betterleaks, secret hygiene, and diff hygiene passed without product
file changes or full-suite reruns.

## Wave 4 Seeds base

Custody tip `4d806ee` was merged history-preservingly at
`b46004ea2d1df9e9f279b4a9a72c7e240f331c78`. Its two-commit delta from
`7969d02` changes only `.seeds/issues.jsonl`, and the integrated queue is
byte-identical to custody.

`audio-graph-ada2` is claimed and `in_progress`. Wave 4 uses the Rust-owned
deep-module seam `SpeechSpanRevisionNormalizer::admit(SpanObservation)`, with
nested authoritative v2 evidence, legacy-v1 read compatibility, unchanged
outer framing v1, and an explicitly frozen transcript-hash v1 path. Child
`audio-graph-4dbb` owns the core contract; `audio-graph-48de` and
`audio-graph-98ef` follow it in parallel; `audio-graph-fcca` follows the
readiness contract. Design tickets `audio-graph-0d72` and `audio-graph-21e9`
remain `BLOCKED_DESIGN`.

The complete queue is 92 ready and 91 blocked. Seeds Doctor has 0 failures.
Final `verify:fast`, generated contracts, Betterleaks, secret hygiene, and diff
hygiene passed. Product files did not change, so the full suites were not
rerun.

## Wave 4 core assembled status

Custody `10cc6b6` and reviewed 4dbb tip `a68c0be` were integrated without
conflict or history rewrite. The assembled pre-report tip is
`9065146c4344bdc052a029c4afe43d68e369c03f`, and the Seeds file is
byte-identical to custody.

The Rust-owned deep module now seals v2 Speech Span Revision creation behind
`SpeechSpanRevisionNormalizer::admit(SpanObservation)`. Strict nested fidelity,
legacy-v1 decoding, explicit/frozen ProjectionBasis hash v1 behavior, framed
v1 ledger replay, and schema-owned TypeScript speaker constraints are green.
No adapter, readiness, UI, selectability, or production writer was activated.

The assembled gates passed: IPC 17 plus the compile-fail doctest; focused
speech/hash/canonical tests; locked check; serialized full cloud library suite
with 1,566 passed, 0 failed, and 8 ignored; strict Clippy; rustfmt; all five
contracts; typecheck; Biome; and exact frontend 70 files/962 tests. The
repo-authoritative `SEEDS_CLI_ROOT` `verify:fast` run passed. The unrelated
global CLI drift remains recorded on `audio-graph-9e23` and was not mutated.

`audio-graph-4dbb` is eligible for conductor closure after reconciliation.
`audio-graph-ada2` remains `in_progress`; 48de and 98ef follow the core in
parallel, and fcca remains blocked by 98ef. Exact evidence is in
[`integration-wave4-core-report.md`](agentic-runs/2026-08-14-session-memory-continuation/integration-wave4-core-report.md).

## Wave 5 assembled status

Custody `01f1aa5`, reviewed readiness tip `2f5bd68`, and proposed-design tip
`88358e1` were integrated without conflict or history rewrite. The assembled
pre-report tip is `b6304b0784ee14f1a0341fb5ebe80de0e6bf86bb`, and the
Seeds file is byte-identical to custody. Root authorized one documentation-only
hygiene correction: removal of the surplus EOF blank line in the accepted
48de report.

Static and effective STT fidelity remain separate. Global diarization policy
drives speaker fidelity and cache fingerprints; Deepgram channel fidelity
remains unavailable; healthy final-only STT remains ready but typed degraded;
and content-free diagnostics are enforced. The exact selectable-provider block
and sorted set are unchanged from the base. No adapter, UI, writer, or provider
promotion was activated.

Provider registry 23, four focused readiness tests, generated registry 19,
all five contracts, typecheck, Biome, locked check, the serialized full cloud
suite with 1,570 passed/0 failed/8 ignored, strict Clippy, rustfmt, and exact
frontend 70 files/963 tests all passed. Repo-authoritative `verify:fast`, Seeds
JSON/Doctor/output, ADR/static-hash, Betterleaks, secret, and diff gates passed.

`audio-graph-98ef` is eligible for conductor closure after reconciliation.
ADR-0035 and ADR-0036 are **proposed** and only ready for human review;
`audio-graph-4249` remains `BLOCKED_DESIGN`, and `audio-graph-48de` remains
blocked by 4249. Exact evidence is in
[`integration-wave5-report.md`](agentic-runs/2026-08-14-session-memory-continuation/integration-wave5-report.md).

## Wave 6 assembled status

Custody `08d4b91` and reviewed frontend tip `3850c26` were integrated without
conflict or history rewrite. The assembled pre-report tip is
`0098ca28579635469e7293c8ada75b9afda068ec`, and the Seeds file is
byte-identical to custody.

Operational readiness remains separate from fidelity evidence. Missing
degradations fail conservatively; turn fields stay typed; endpointing false
means provider default; detailed evidence is selected-card gated; and the
localized readiness/fidelity landmarks are unique. Provider IDs do not infer
capabilities. The selector, generated registry, store, controller, and backend
are unchanged, and proposed ADRs remain proposed.

The focused frontend suite passed 300/300 tests, and exact `test:local` passed
70/70 files and 968/968 tests. Typecheck, Biome, build, locale parity, all five
contracts, repository-authoritative `verify:fast`, Seeds JSON/Doctor/output,
Betterleaks, secret, and diff gates passed. Rust product files did not change,
so Wave 5's serialized 1,570-passed cloud suite remains applicable and was not
rerun.

Custody has closed `audio-graph-98ef`; `audio-graph-fcca` is eligible for
conductor closure after reconciliation. `audio-graph-4249` remains in its
proposed-decision state, and `audio-graph-48de` remains blocked by 4249. Exact
evidence is in
[`integration-wave6-report.md`](agentic-runs/2026-08-14-session-memory-continuation/integration-wave6-report.md).
