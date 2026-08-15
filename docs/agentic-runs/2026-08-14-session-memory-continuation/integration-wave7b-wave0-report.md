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

## Wave0 Seed reconciliation

Custody `b890d3c2102c73ebe77ab736de30fb74f9ebdd62` was merged
history-preservingly at `52a83a308a67a1df833452f284e15323faa11b44`.
Its one linear commit after custody `dd038da` changes only
`.seeds/issues.jsonl`; the integrated 639-row queue is byte-identical to
custody and remains valid JSONL.

Seed 1189 is closed with the integrated Wave0 evidence. D1 `c2e3` and M0
`661f` are both `in_progress`, unblocked, assigned to their Wave1 owners, and
recorded as parallel `ACTIVE_MILESTONE` workstreams. Parent 8e73 records the
active parallel Wave1, WIP cap 2, the optional Linux-only Docker boundary, and
the monitored Blacksmith platform-qualification boundary.

The complete queue contains 90 ready and 101 blocked issues. Seeds output
stress, all five contracts, pinned `verify:fast`, Betterleaks, docs/Seeds
secret hygiene, and exact range diff passed. Seeds Doctor reported 10 passed,
2 custody-carried warning groups, and 0 failures. Product files did not change,
so no product suite was rerun; no workflow, push, or extra Seed edit occurred.

## Wave1a partial fan-in

Custody `566fd959619b810116f055619e95e3d4bc488225` was merged
history-preservingly at `6c3b7ea70a4bd13600c1cf5341246d7603a7a027`.
Its seven linear commits after custody `b890d3c` change only
`.seeds/issues.jsonl`; the integrated 641-row queue is byte-identical to
custody and remains valid JSONL. Reviewed 661f integration tip
`e0b8c941145445973975acd4ca2076f2a206c99b` was then merged
history-preservingly at `259dfee74b9323d73c6c4c74d5d4fb788fb349d0`.
Its direct one-commit contribution from `d259105` is exactly the manifest
transaction design and implementation report. Standards and Spec returned
**SHIP** after correction.

The dedicated prototype tip `88849b89cea3aaf476ffcf5fdd98029a4f095822`
remains separate and is not an integration ancestor; its prototype path is
absent from the assembled tree. Streaming that Git object directly through
Node in ESM mode passed syntax and reproduced 124 cases, 1,158 transitions,
411 states, 22,121 assertions, and 48 invariant families, selecting the
versioned atomic snapshot with generation CAS. The first syntax pipe used
Node's default CommonJS stdin mode, and the next shell assertion expected
comma-formatted summaries rather than the model's labeled integer lines. Both
wrapper-only attempts occurred before merge; the corrected exact-label ESM
pipe passed.

Blocked c2e3 tip `477df40` was not merged. Seed c2e3 is open and blocked by
review-cap successors `ce19` and `83e2`; ce19 is `READY_NEXT`, and 83e2 remains
blocked by ce19. Custody also records the authorized supplemental Dockurr guest
runbook, audio E2E, Windows durability, and macOS/Windows virtualization
boundaries while keeping native Blacksmith evidence authoritative and the
Windows virtual-driver scope open.

The complete queue contains 91 ready and 103 blocked issues. The 661f links,
all five contracts, pinned `verify:fast`, Seeds output stress, Betterleaks,
docs/Seeds secret hygiene, and exact three-file pre-report range passed. Seeds
Doctor reported 10 passed, 2 custody-carried warning groups, and 0 failures.
No Rust or frontend full suite ran because no product file changed; no product,
runtime, prototype, workflow, push, or extra Seed mutation occurred.

## Wave1b durability-stack fan-in

Custody `8dd2dc1036ab625a4a73d8b913b865bd059ecbd1` was merged
history-preservingly at `509813c37596dabf62c98202831197846880a799`.
Its two linear commits after custody `566fd95` change only
`.seeds/issues.jsonl`; the integrated 641-row queue is byte-identical to
custody and remains valid JSONL.

The reviewed Rust stack is exact and linear: base `d259105` to c2e3 tip
`477df40` in three commits, to ce19 tip `28961a7` in two commits, and to 83e2
tip `234ebe9` in two commits. Only the final tip was merged, once,
history-preservingly at `45c51592ea03f38087d21baa16390edf684f9b76`.
Its true contribution is exactly `canonical_durability.rs`, the persistence
module declaration, and the c2e3, ce19, and 83e2 reports. The c2e3 review cap
backflow is resolved by the stacked successors; ce19 was reviewed Standards
**SHIP-WITH-NITS** and Spec **SHIP**, and 83e2 was reviewed Standards and Spec
**SHIP**.

The assembled module reserves every ASCII-case coordination basename before
filesystem access, binds mutation to the exact managed parent, refuses Windows
absent append and rename before mutation, returns
`CrossDeviceRenameRefused` only for a proven preflight device mismatch, and
returns `DurabilityIndeterminate` with raw OS code and recovery key for runtime
`EXDEV`. There is no new unsafe code and no non-test runtime caller; the only
outside reference is the dormant persistence module declaration. Prototype
tip `88849b8` remains a non-ancestor and its file is absent.

| Gate | Result |
| --- | --- |
| focused serialized durability | 23 passed, 0 failed |
| locked cloud lib/tests check | passed |
| full direct locked cloud library, serialized | 1,603 passed, 0 failed, 8 ignored in 43.84 seconds |
| strict cloud Clippy | passed with `-D warnings` in 30.49 seconds |
| rustfmt | passed |
| pinned Windows actual-module compile | passed; 571,590-byte rlib emitted |
| exact frontend `test:local` | 70 files and 968 tests passed in 105.33 seconds |
| all generated contracts and pinned `verify:fast` | all five current; Biome 174/typecheck/output/secret/diff green |
| Seeds ready-all / blocked | 90 ready; 102 blocked |
| Seeds output stress | ready 50, blocked 102, and list 50 parsed |
| Seeds Doctor | 10 passed, 2 custody-carried warning groups, 0 failures |
| Betterleaks and docs/Seeds secret hygiene | approximately 2.70 MB, no leaks; 0 secret findings |
| exact range, placeholder, and diff hygiene | passed |

Two integration assertions were initially overbroad. The unsafe scan covered
all of `persistence/mod.rs` and found four unchanged test-only unsafe blocks;
the corrected merge-base delta proved zero added unsafe. The placeholder scan
covered the complete Seeds authority and matched an old closed issue's task-marker
string; the corrected custody-added-lines plus five stack-owned-file scan
passed. Both were harness-scope false positives, not accepted-input or product
failures.

Custody has closed 661f. Seeds c2e3, ce19, and 83e2 remain open or in-progress
for root reconciliation even though the reviewed complete stack is now
integrated. No Docker, Blacksmith, workflow, push, extra Seed edit, or
prototype merge occurred.

## Wave1b Seed reconciliation

Custody `42045658e96a65e72d2c8fdda62c04c6184e48c7` was merged
history-preservingly at `315037ee121ae7a6234eac17c2e2fe882647b307`.
Its one linear commit after custody `8dd2dc1` changes only
`.seeds/issues.jsonl`; the integrated 641-row queue is byte-identical to
custody and remains valid JSONL.

Seeds c2e3, ce19, and 83e2 are closed and absent from both ready and blocked
queues. Seed a596 is `in_progress`, unblocked, and assigned to
`codex-a596-wave7b`. Parent 8e73 records the active order as a596, then 3b8b,
b77b, and 2df3.

The complete queue contains 90 ready and 100 blocked issues. Seeds output
stress parsed ready 50, blocked 100, and list 50. All five contracts, pinned
`verify:fast`, Betterleaks, docs/Seeds secret hygiene, and exact range diff
passed. Seeds Doctor reported 10 passed, 2 custody-carried warning groups, and
0 failures. Product files did not change, so product suites were not rerun;
no workflow, push, or extra Seed edit occurred.

## Wave1c snapshot-replace prerequisite reconciliation

Custody `aca962113bc0d6cf90a44e022471924d25b98358` was merged
history-preservingly at `3f23d5be7851da800a28f1d719577f5e6c597101`.
Its one linear commit after custody `4204565` changes only
`.seeds/issues.jsonl`; the integrated 642-row queue is byte-identical to
custody and remains valid JSONL.

Seed c928 is `in_progress`, unblocked, and assigned to
`codex-c928-wave7b`. Seed a596 remains open and is blocked directly and only
by c928. Parent 8e73 records the active order as c928, a596, 3b8b, b77b, and
2df3; c2e3 remains closed.

The complete queue contains 90 ready and 101 blocked issues. Seeds output
stress parsed ready 50, blocked 101, and list 50. All five contracts, pinned
`verify:fast`, Betterleaks, docs/Seeds secret hygiene, and exact range diff
passed. Seeds Doctor reported 10 passed, 2 custody-carried warning groups, and
0 failures. An initial output assertion incorrectly expected an already
`in_progress` Seed in `ready-all`; direct record and blocked-queue assertions
passed after correcting that harness scope. Product files did not change, so
product suites were not rerun; no workflow, push, or extra Seed edit occurred.

## Wave1c reviewed atomic-snapshot fan-in

Custody `d4b2c94fa60e3897a1c52c4375e4471704c29fe6` was merged
history-preservingly at `1cd5a844c6874869cf4115fdf993338d85bab05b`.
Its two linear commits after custody `aca9621` are exactly `75c946b` and
`d4b2c94`, change only `.seeds/issues.jsonl`, and preserve the byte-identical
valid 642-row custody queue.

Corrected c928 tip `efc4f77ea2c3e0fb7d43618deb91de3223c2344a`
was merged history-preservingly without conflict at
`dfd48fbbe42c3a7be208a170012d2e62f5805c21`. Its five linear commits from
exact merge-base `f912a07` change only `canonical_durability.rs` and the c928
report. Standards and Spec correction re-reviews returned **SHIP** with no
findings.

The assembled seam exposes sibling qualification only as `#[cfg(test)]
pub(crate)`, keeps production qualification opaque, and makes the c928 Windows
refusal test qualification-independent. Guard-owned initial installation and
replacement cover absent late races, old-or-new restart visibility, fault
cuts, and runtime `EXDEV` as `DurabilityIndeterminate`. There is no non-test
runtime caller, dependency, unsafe addition, workflow, manifest schema,
recovery transaction, UI, or prototype ancestry.

| Gate | Result |
| --- | --- |
| focused serialized canonical durability | 38 passed, 0 failed |
| locked cloud lib/tests check | passed |
| full direct locked cloud library, serialized | 1,618 passed, 0 failed, 8 ignored in 41.71 seconds |
| strict cloud Clippy and rustfmt | passed with `-D warnings`; formatting current |
| pinned Windows production module and test object | passed; 679,068-byte rlib and 592,892-byte object |
| exact frontend `test:local` | 70 files and 968 tests passed in 116.30 seconds |
| all contracts and pinned `verify:fast` | all five current; Biome 174/typecheck/output/secret/diff green |
| Seeds ready-all / blocked | 90 ready; 101 blocked |
| Seeds output stress and Doctor | ready 50, blocked 101, list 50; 10 passed, 2 carried warning groups, 0 failures |
| Betterleaks, secret, footprint, placeholder, prototype, and diff hygiene | approximately 2.50 MB, no leaks; 0 secret findings; passed |

Custody records Testbox `tbx_01m02htqszj16b6e7v640ae114` as queued for 10
minutes and explicitly stopped before hydration. No command ran, the active
list was empty after cleanup, and the temporary remote workflow branch was
deleted. This is a capacity/no-action outcome, not native Windows evidence;
native qualification remains assigned to 2df3.

Two integration harness checks were initially overbroad: an external-caller
scan included the implementation report, and a streaming `jq -e` query did
not retain a final JSONL result. Restricting the caller scan to other Rust
production files and slurping the ledger made both intended assertions pass.
Neither was a product failure.

Seed c928 is eligible for root closure after the reviewed landing. Seed a596
remains open and blocked only by c928 until custody reconciliation. No Seed
was closed or additionally edited; no Blacksmith, Docker, workflow, push, or
runtime activation occurred.

## Wave1c closure and manifest-kernel activation

Custody `c3a0d880a41d9b1017eec87d273c15c60dc93848` was merged
history-preservingly at `bd9fe587cd179d060860187b4a5e1993d823dea8`.
Its one linear commit after custody `d4b2c94` changes only
`.seeds/issues.jsonl`; the integrated 642-row queue is byte-identical to
custody and remains valid JSONL.

Seed c928 is closed and absent from ready and blocked queues. Seed a596 is
`in_progress`, unblocked, and assigned to `codex-a596-wave7b`. Parent 8e73
records the remaining order as a596, 3b8b, b77b, and 2df3.

Seed 2df3 retains the exact c928 Testbox capacity/no-action record: the box
queued for 10 minutes, stopped before hydration, ran no command, had an empty
active list after cleanup, and used a temporary remote branch that was
deleted. It also retains the inherited native Windows fixture follow-up. This
remains external evidence work, not a native result from the c928 fan-in.

The queue contains 90 ready and 100 blocked issues. Seeds output stress parsed
ready 50, blocked 100, and list 50. All five contracts, pinned `verify:fast`,
Betterleaks, docs/Seeds secret hygiene, and exact range diff passed. Seeds
Doctor reported 10 passed, 2 custody-carried warning groups, and 0 failures.
Product files did not change, so product suites were not rerun; no product,
workflow, push, extra Seed edit, Blacksmith, or Docker action occurred.

## Wave2 reviewed manifest-kernel fan-in

Custody `6c7037f5f9f75d59627cfeee6e8024780e7c0109` was merged
history-preservingly at `c82a003268fcd341e2ffdf7f6a8573a6817fafba`.
Its four linear commits after custody `c3a0d88` are exactly `2360fd3`,
`f7c8a5f`, `2465262`, and `6c7037f`, change only `.seeds/issues.jsonl`, and
preserve the byte-identical valid 642-row custody queue.

Final-cap a596 tip `e1dd22b281f9ebee51b87bf8d2ec6595de167496`
was merged history-preservingly without conflict at
`b8988fb4f550aadfb1306ea5fca1d5ad20369a48`. Its six linear commits from
exact merge-base `5444b66` add only `session_artifact_manifest.rs`, the narrow
module declaration, and the a596 report. Standards and Spec final-cap
re-reviews returned **SHIP** with no findings.

The dormant explicit-root kernel enforces strict V1 schema and portable
identity admission, including the 1,023-byte total identity ceiling and
persisted generation floor. Its transaction owns the canonical guard and
performs initial or exact-open-head replacement CAS. Durable Prepared state
drives exact immutable completion and cannot be dropped; exact Completed
retry remains `AlreadyCompleted`. Candidate serialization is capped at 16
MiB before durability mutation, and strict loads use bounded opened-handle
reads with metadata/length revalidation. Quarantine and Original Session
Audio unavailable evidence, deletion parity, and internal identities remain
typed and explicit.

There is no non-test runtime consumer, dependency or unsafe addition,
workflow, default root, provisioning, broad repository/adapter/UI adoption,
recovery transaction, or prototype ancestry.

| Gate | Result |
| --- | --- |
| focused serialized manifest | 18 passed, 0 failed in 8.75 seconds |
| focused serialized canonical durability | 38 passed, 0 failed in 3.66 seconds |
| locked cloud lib/tests check | passed in 25.82 seconds |
| full direct locked cloud library, serialized | 1,636 passed, 0 failed, 8 ignored in 49.95 seconds |
| strict cloud Clippy and rustfmt | passed with `-D warnings`; formatting current |
| pinned Windows production and test-object module probes | passed; 10,089,418-byte rlib and 2,824,992-byte object |
| exact frontend `test:local` | 70 files and 968 tests passed in 107.48 seconds |
| all contracts and pinned `verify:fast` | all five current; Biome 174/typecheck/output/secret/diff green |
| Seeds ready-all / blocked | 90 ready; 100 blocked |
| Seeds output stress and Doctor | ready 50, blocked 100, list 50; 10 passed, 2 carried warning groups, 0 failures |
| Betterleaks, secret, footprint, placeholder, prototype, and diff hygiene | approximately 2.70 MB, no leaks; 0 secret findings; passed |

The first two Windows test-object wrapper attempts used `cargo rustc --test`,
which selected metadata-form dependency externs and stopped before module
compilation. The production Cargo probe was already green; direct pinned
`rustc` with the built Windows-target rlibs then compiled the actual two-module
test object successfully. This was probe construction, not a product failure,
and the result remains cross-compilation rather than native filesystem proof.

The first preflight forbidden-path scan also classified `src-tauri/src` as
frontend `src/`; the exact three-path candidate allowlist and corrected
contamination scan passed. Seed a596 is eligible for root closure after the
reviewed landing; 3b8b is next only after custody reconciliation. No Seed was
closed or additionally edited, and no push, workflow, Blacksmith, Docker, or
runtime activation occurred.
