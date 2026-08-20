# audio-graph-661f implementation report

Date: 2026-08-15

Review round: 1 of at most 2

## Assignment and boundaries

- Seed: `audio-graph-661f` — select and crash-model the persisted artifact
  manifest transaction.
- Acceptance: compare a versioned atomic snapshot with generation CAS,
  append-only manifest log, and log plus materialized view; executable finite
  coverage of prepare, quarantine publish and namespace durability, manifest
  acceptance, source truncate and sync, completion persistence,
  pre/post-acknowledgement, restart, generation conflict, exact retry,
  concurrent writers, exact residual state, every crash cut, typed unsupported
  namespace refusal, visible failure uncertainty, no duplicate advancement,
  and content-free diagnostics; select one physical form without silently
  adding a fifth canonical stream.
- Worktree:
  `/home/codeseys/DevBox/audio-graph/.worktrees/661f-manifest-model-wave7b`.
- Integration branch: `work/661f-manifest-model-wave7b-integration`.
- Durable prototype branch:
  `prototype/audio-graph-661f-manifest-model-wave7b`.
- Exact base: `d2591059f70247c41f054880682560fc18528622`.
- Integration-owned paths: the retained
  [design verdict](../../designs/persisted-artifact-manifest-transaction.md)
  and this report.
- Prototype-only path:
  `scripts/prototypes/manifest-transaction-crash-model.mjs` on the dedicated
  prototype branch, never in integration ancestry.
- Exclusions honored: no product/runtime, Rust, frontend, Cargo, package,
  Seeds, generated, workflow, provider, Docker, Blacksmith, or external-CI
  changes or runs.

## Review round 1 corrections

The blocked review snapshot is preserved unchanged at
`940299f107e57af2f037b5c6c460ffa957049a0f` by both the original
`work/661f-manifest-model-wave7b` ref and the explicit
`review/audio-graph-661f-manifest-model-wave7b-round1` ref.

The four review findings were corrected as follows:

1. **Prototype durability and integration ancestry.** The runnable model now
   has the dedicated durable branch ref
   `prototype/audio-graph-661f-manifest-model-wave7b` at corrected tip
   `88849b89cea3aaf476ffcf5fdd98029a4f095822`. The integration branch was
   created directly from the exact base; it contains neither prototype commit
   in ancestry and contains no runnable script.
2. **Completion generation CAS.** `PersistCompleted` now requires manifest
   head generation `tx.expectedGeneration + 1` before validating the exact
   `Prepared` transaction identity. An exact same transaction/fingerprint at
   generation `expected + 2` returns `GenerationConflict { expected, actual }`
   and changes neither physical state nor advancement counters.
3. **Proven no-mutation failure.** `Prepare/NoMutation` now returns
   `IoFailedBeforeAcceptance`, retains no indeterminate recovery stage, and
   changes no durable state. Partial or exact temp observations and every later
   visible or possibly visible failure remain `DurabilityIndeterminate`.
4. **Commit evidence.** The corrected prototype commit and the single
   integration commit both use multi-paragraph bodies containing their actual
   scoped gate evidence. The original blocked commits remain immutable review
   evidence and are not integration ancestors.

## Branch topology

```text
prototype/audio-graph-661f-manifest-model-wave7b
d2591059 (exact base)
  -> 37eb29b7 (captured runnable model)
  -> 88849b89 (review RED/GREEN corrections; durable prototype tip)

work/661f-manifest-model-wave7b-integration
d2591059 (exact base)
  -> one documentation-only integration commit

review/audio-graph-661f-manifest-model-wave7b-round1
d2591059 -> 37eb29b7 -> 940299f1 (blocked review snapshot only)
```

The integration branch has merge base, parent, and root contribution all at
the exact assigned base. Squashing or merging its single commit cannot include
the throwaway script. The prototype remains reachable through its dedicated
local branch ref rather than incidental ancestry.

## Outcome

Select one versioned atomic Session Artifact manifest snapshot with generation
CAS under the stable coordination lock. Persist `Prepared` only after the
quarantine final name and its namespace barrier are durable. Truncate and sync
the same verified source handle only after `Prepared`. Persist `Completed`
only from the exact same transaction at generation `expected + 1`, then
acknowledge. Each installed manifest transition advances the generation once;
exact completed retry returns `AlreadyCompleted` without replacement or
generation advancement.

The selected manifest is typed state rather than a fifth append-only canonical
event stream, so ADR-0043 backflow is not triggered. Both log candidates are
crash-correct under the model, but selecting either would have stopped this
integration path for a new ADR and ADR-0043 registry update.

The selected snapshot still requires qualified namespace durability for
quarantine publication and manifest replacement. Unsupported capability is a
no-mutation typed refusal only when preflighted; discovery after visible or
possibly visible mutation remains `DurabilityIndeterminate`. This design does
not upgrade the current Windows refusal or the macOS APFS evidence condition.

## Durable prototype and reproduction

Corrected runnable source:

```text
branch: prototype/audio-graph-661f-manifest-model-wave7b
tip: 88849b89cea3aaf476ffcf5fdd98029a4f095822
path: scripts/prototypes/manifest-transaction-crash-model.mjs
```

Exact reproduction from the repository:

```text
git switch prototype/audio-graph-661f-manifest-model-wave7b
test "$(git rev-parse HEAD)" = 88849b89cea3aaf476ffcf5fdd98029a4f095822
node --check scripts/prototypes/manifest-transaction-crash-model.mjs
bun scripts/prototypes/manifest-transaction-crash-model.mjs
betterleaks dir --no-banner --redact \
  scripts/prototypes/manifest-transaction-crash-model.mjs
```

The source can be inspected without switching branches:

```text
git show 88849b89cea3aaf476ffcf5fdd98029a4f095822:scripts/prototypes/manifest-transaction-crash-model.mjs
```

## Exact corrected model evidence

The complete one-command run compared all three forms and reported:

```text
forms: 3
crash cuts: 10
completion-generation regression cases: 3
successful form cases: 3
crash-cut cases: 46
visible-failure cases: 54
namespace-refusal cases: 6
concurrent-writer cases: 12
cases explored: 124
transitions evaluated: 1158
unique full states observed: 411
invariant assertions: 22121
invariant families passed: 48
```

The ten explicit cuts were `BeforePrepare`, `AfterPrepare`,
`AfterQuarantinePublish`, `AfterQuarantineNamespaceDurability`,
`AfterManifestAcceptance`, `AfterSourceTruncate`, `AfterSourceSync`,
`AfterCompletionPersistence`, `PreAcknowledgement`, and
`PostAcknowledgement`.

The 48 invariant families were:

```text
accepted-only-after-completion
accepted-requires-quarantine-namespace
accepted-requires-source-sync
all-forms-compared-fairly
completed-exact-retry-is-idempotent
completion-before-acknowledgement
completion-generation-conflict-does-not-mutate
completion-requires-exact-prepared-generation
concurrent-writer-is-contended
content-free-diagnostics
crash-cut-exact-residual
crash-restart-converges
deterministic-exact-retry
exact-residual-state
exact-source-residual
exact-visible-source-state
fault-retry-converges
fault-retry-no-duplicate-advancement
generation-conflict-does-not-advance
generation-conflict-is-explicit
generation-is-monotonic-integer
known-manifest-phase
known-physical-form
late-unsupported-is-indeterminate
late-unsupported-never-accepts-or-truncates
log-generation-contiguity
manifest-prepared-before-truncate
materialized-view-is-never-authority
model-selects-simplest-crash-correct-form
no-duplicate-advancement-on-retry
no-duplicate-manifest-advancement
pre-mutation-failure-does-not-create-uncertainty
pre-mutation-failure-has-no-visible-state
pre-mutation-failure-is-io-before-acceptance
pre-post-acknowledgement-cut
quarantine-durable-before-manifest
quarantine-durable-before-truncate
same-id-different-fingerprint-refused
selection-does-not-add-fifth-stream
single-authority-form
source-sync-before-completion
successful-exact-retry-does-not-advance
successful-form-accepts-once
uncertainty-survives-restart
unsupported-namespace-has-no-mutation
unsupported-namespace-never-accepted
unsupported-namespace-refuses-before-mutation
visible-failure-remains-uncertain
```

The exact physical comparison was unchanged by the corrections:

```text
AtomicSnapshotCas:         safety cases 34, complexity score 6, ADR backflow false
AppendOnlyLog:             safety cases 36, complexity score 7, ADR backflow true
LogWithMaterializedView:   safety cases 42, complexity score 10, ADR backflow true
SELECTED: AtomicSnapshotCas
```

The form-specific safety counts differ because the log adds absent/torn/exact
tail outcomes and the hybrid adds lagging/exact view outcomes. Every form ran
the same ten logical crash cuts and common invariant families.

## Test-first review evidence

### Proven no-mutation failure

The assertion was changed before the reducer. RED command and real output:

```text
set -o pipefail; bun scripts/prototypes/manifest-transaction-crash-model.mjs \
  2>&1 | rg -m 1 'pre-mutation-failure|wrong failure boundary'

exit 1
error: [pre-mutation-failure-is-io-before-acceptance] AtomicSnapshotCas/Prepare/NoMutation used the wrong failure boundary
```

GREEN classifies only the proven `NoMutation` variant as
`IoFailedBeforeAcceptance`, asserts byte-for-byte unchanged durable state, and
asserts that restart has no indeterminate recovery stage. `PartialTemp` and
`ExactTemp` remain in the visible-failure uncertainty matrix.

### Exact completion generation

The all-form same-transaction unexpected-generation assertion was added before
the reducer guard. RED command and real output:

```text
set -o pipefail; bun scripts/prototypes/manifest-transaction-crash-model.mjs \
  2>&1 | rg -m 1 'completion-requires-exact-prepared-generation|unexpected same-transaction generation'

exit 1
error: [completion-requires-exact-prepared-generation] AtomicSnapshotCas: completion accepted an unexpected same-transaction generation
```

GREEN checks snapshot, log, and hybrid heads at `expected + 2`; each returns
`GenerationConflict` with expected `1`, actual `2`, unchanged durable state,
and unchanged advancement counters. Existing successful cases prove completion
still succeeds at exactly `expected + 1`.

## Verification

### Corrected prototype tip

Commit: `88849b89cea3aaf476ffcf5fdd98029a4f095822`.

Commands:

```text
node --check scripts/prototypes/manifest-transaction-crash-model.mjs
bun scripts/prototypes/manifest-transaction-crash-model.mjs
betterleaks dir --no-banner --redact \
  scripts/prototypes/manifest-transaction-crash-model.mjs
git diff --check
```

Real results:

```text
node --check: exit 0, no output
bun model: exit 0; 124 cases, 1158 transitions, 411 states,
  22121 assertions, 48 invariant families; SELECTED AtomicSnapshotCas; PASS
Betterleaks: scanned approximately 54.87 KB; no leaks found
git diff --check: exit 0, no output
correction footprint: one prototype file, 96 insertions, 6 deletions
```

The corrected commit body records both RED failures and these exact GREEN
gates.

### Integration-ready documentation tip

Commands run against the staged single-commit integration contribution:

```text
git diff --cached --check

bounded Bun relative-link resolver over:
  docs/designs/persisted-artifact-manifest-transaction.md
  docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-661f-report.md

bun scripts/check-docs-secret-hygiene.mjs

betterleaks dir --no-banner --redact \
  docs/designs/persisted-artifact-manifest-transaction.md \
  docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-661f-report.md

git rev-parse \
  prototype/audio-graph-661f-manifest-model-wave7b \
  review/audio-graph-661f-manifest-model-wave7b-round1 \
  work/661f-manifest-model-wave7b

git merge-base HEAD prototype/audio-graph-661f-manifest-model-wave7b
git merge-base --is-ancestor \
  88849b89cea3aaf476ffcf5fdd98029a4f095822 HEAD

git show \
  88849b89cea3aaf476ffcf5fdd98029a4f095822:scripts/prototypes/manifest-transaction-crash-model.mjs \
  | wc -l

test ! -e scripts/prototypes/manifest-transaction-crash-model.mjs
git diff --cached --name-status
```

Real output:

```text
git diff --cached --check: exit 0, no output
Owned Markdown relative links resolve: 2 files, 5 targets
docs/Seeds secret hygiene scan passed: 0 findings
Betterleaks: no leaks found
prototype tip: 88849b89cea3aaf476ffcf5fdd98029a4f095822
review snapshot ref: 940299f107e57af2f037b5c6c460ffa957049a0f
original blocked branch: 940299f107e57af2f037b5c6c460ffa957049a0f
integration/prototype merge base: d2591059f70247c41f054880682560fc18528622
prototype-is-integration-ancestor check: exit 1, expected false
corrected prototype source length: 1530 lines
integration-tree script absence check: exit 0, no output
A  docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-661f-report.md
A  docs/designs/persisted-artifact-manifest-transaction.md
```

Result: pass. The integration contribution relative to exact base
`d2591059f70247c41f054880682560fc18528622` is only the two retained documents;
the script is absent from both its tree and ancestry. No product suite was run
because the integration tree changes documentation only. No Rust/frontend full
suite, Docker, Blacksmith, workflow, push, merge, or external CI run was
performed.

## Exact residual-state coverage

The model classified every restart into one of these exact states before
retry: `CleanSourceFull`, `TempOnlySourceFull`,
`PartialTempSourceFull`, `PublishedNamespaceUncertainSourceFull`,
`QuarantineOnlySourceFull`, `PreparedSourceFull`,
`PreparedSourceTruncated`, `CompletedSourceTruncated`, plus form-specific
`SnapshotReplacementTemp:<phase>` and `TornManifestTail:<phase>` states before
restart repair.

- Temp or published-only residuals never permit truncation.
- `PreparedSourceFull` performs truncate and source sync once.
- `PreparedSourceTruncated` persists completion without truncating again.
- `CompletedSourceTruncated` exact retry returns `AlreadyCompleted` without a
  new manifest generation.
- A same transaction id with a different fingerprint is refused as
  `IdempotencyConflict`.
- A same transaction id and fingerprint at an unexpected prepared generation
  is `GenerationConflict` and cannot complete or mutate.
- A cooperating second writer is `Contended` while the first lock is held;
  after release, its stale generation is an explicit `GenerationConflict` with
  no mutation.
- A failure proven to occur before any visible mutation is
  `IoFailedBeforeAcceptance` and creates no uncertain recovery state.
- A failure after any visible or possibly visible mutation remains
  `DurabilityIndeterminate`; restart repair may converge it but never rewrites
  the original result as `Accepted`.

## Findings and open questions

- The model selects the physical form but does not freeze the complete
  production manifest schema. `audio-graph-a596` still owns strict schema,
  typed entries, privacy/availability classes, stable relative managed
  identity, hashes/lengths, source identity, and persisted residual details.
- Generation CAS is meaningful only inside the stable cooperative lock plus
  exact head validation. It is not a claim against uncooperative mutation.
- The abstract model proves transition ordering and recovery closure, not real
  filesystem atomicity, OS barriers, process crash, or power loss.
- The selected snapshot performs two namespace-changing manifest replacements
  per completed recovery transaction. That is intentionally accepted in
  exchange for avoiding a fifth canonical stream, torn-tail framing/replay,
  and hybrid view skew. Performance remains an implementation measurement, not
  a modeled claim.
- No unrelated defect was changed. Seed status and later work remain
  conductor-owned; this worktree did not edit or close Seeds.
