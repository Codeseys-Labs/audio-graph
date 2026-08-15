# audio-graph-5e41 prototype report

## Question and assumptions

Question: can a finite, receipt-bearing projection-admission and Session-fence
model prove the state boundary needed before production persistence work begins?

Assumptions: this is a metadata-only, in-memory, throwaway logic prototype. It
models one attempted event per Notes/Graph lane, treats scheduler persistence
as distinct from canonical event persistence, requires an additional
directory-entry barrier for newly created streams, binds canonical receipts to
their exact Session/lane/owner/event/prestate, tracks each remote attempt as a
separate effect identity, and refuses to infer an in-flight remote request's
external result after restart. It neither calls nor imports production code and
makes no operating-system durability claim.

## Assignment

- Seed: `audio-graph-5e41` — Define canonical projection admission and session
  job fencing.
- Acceptance: executable bounded coverage of `Pending`, `Accepted`,
  `AlreadyAccepted`, `Rejected`, and `OutcomeUncertain`; materialization and
  Projection Basis eligibility only after acceptance; exact idempotent retry
  with a stable committed sequence; Session epoch/lease replacement; late
  success/failure and detached-writer refusal; durable-queued versus
  external-effect-unknown restart; deletion fencing; independent Notes/Graph
  lanes; and crash cuts at enqueue, write, flush, file sync, directory sync,
  and acknowledgement. Diagnostics must remain content-free and the three
  product policies must remain explicit human decisions.
- Review correction acceptance: reject delayed pre-rotation receipts and
  acceptance during deletion; validate exact owner, Session epoch/lease, lane,
  event id/digest, lifecycle, and prestate; fail closed on unknown stream kinds;
  track deletion waits by exact effect identity across multiple attempts; and
  emit only structurally typed, opaque diagnostics under malicious input.
- Second review correction acceptance: actual restart from canonical `Pending`
  at every crash cut must enter a recoverable state, quarantine the old receipt
  binding, converge under current reconciliation/exact retry without duplicate
  advancement, and remain fenced across rotation and deletion.
- Third review correction acceptance: direct retry from `OutcomeUncertain` or a
  generic `Rejected` state must fail; only an authorized `Absent`
  reconciliation may retry using the retained stream kind and supplied barrier
  proof, while durable exact bytes never append again.
- Worktree:
  `/home/codeseys/DevBox/audio-graph/.worktrees/5e41-admission-fencing-prototype-wave7a`
- Branch: `work/5e41-admission-fencing-prototype-wave7a`
- Exact base: `72e23b506d6f4d2e465aeebfb452d20fbbc0bfe5`.
- Owned paths:
  `docs/prototypes/session-projection-admission-and-fencing.md`,
  `scripts/prototype-session-projection-admission.mjs`, and this report.
- Explicit exclusions: every production Rust/TypeScript/frontend file,
  generated artifact, workflow, package file, and Seeds file.

## Outcome

The finite model supports a receipt-bearing boundary with these required
semantics:

- canonical enqueue produces `Pending`, not visibility or durability;
- only `Accepted` or exact-retry `AlreadyAccepted` atomically establishes the
  committed sequence, materialized sequence, and basis-eligible sequence, and
  every acceptance-producing path validates its exact current receipt binding;
- exact retry returns the original sequence, while the same identity with a
  different digest is `Rejected` without changing the commit;
- snapshot failure after acceptance is rebuildable-cache lag and never rolls
  back logical state or frees the sequence;
- restart replaces the process lease; rotation replaces Session epoch and
  lease; success, failure, and writer calls from a retired token are refused;
- restart from canonical `Pending` rebases it to `OutcomeUncertain`, retains
  durable pending evidence and its exact crash-cut observation domain,
  quarantines the old binding by opaque reference, and issues a current-lease
  recovery binding;
- `OutcomeUncertain` cannot retry: durable exact reconciliation yields
  `AlreadyAccepted`, `Absent` yields a typed `AbsentRetryAuthorized` rejection,
  and torn tail remains uncertain pending typed quarantine;
- an authorized retry must match the retained stream kind and supply its full
  barriers; a cross-kind or weak new-file proof is refused without visibility;
- `DurableQueued` survives restart as never-dispatched work, while
  `RemoteInFlight` recovers as `ExternalEffectUnknown`;
- each remote dispatch/reissue retains an exact
  lane/Session/epoch/lease/job/attempt/result-correlation identity; deletion
  removes only a matching terminal, never a bare job id or stale cross-lane token;
- Notes and Graph receipts commute and retain independent scheduler,
  canonical, materialized, and basis heads; and
- deletion fences every writer before either immediate discard or a wait that
  observes but never applies remote terminals; and
- diagnostics use closed code/reason enums, validated lane/numeric fields, and
  opaque hashes instead of copying arbitrary job, result, or status strings.

For an existing stream, acceptance requires write, flush, and file-sync
barriers. For a newly created stream, it also requires the parent-directory
barrier. Any other stream kind fails closed even if every flag is true. The
model uses abstract proof flags only; `audio-graph-8e73` still owns
actual cross-platform primitives, quarantine-manifest registration, locked
recovery, and subprocess evidence.

## Artifacts

- `scripts/prototype-session-projection-admission.mjs` is the single command,
  pure reducer, representative full-state trace, and exhaustive invariant run.
- `docs/prototypes/session-projection-admission-and-fencing.md` records the
  model, crash/reconciliation table, human decision table, recommendation, and
  transition-by-transition production handoff.
- This report captures the bounded evidence and scope.

No package script was added because this prototype must remain visibly
throwaway and is already one-command runnable:

```text
bun scripts/prototype-session-projection-admission.mjs
```

## Exhaustive finite-model evidence

The successful run covered all eight combinations of Saved wording, remote
reissue policy, and deletion policy. Its bounded dimensions were:

```text
correction regression cases:  70
admission/crash cases:       368
receipt cases:                80
scheduler restart cases:      32
lane independence cases:     200
rotation/deletion cases:       64
total cases:                  814
reducer transitions:        7,671
unique full states:         1,262
invariant assertions:     111,905
invariant families:            31
```

All five receipts were observed. The 31 passing invariant families were:

```text
accepted-only-advancement
accepted-requires-durability
active-owner-current
all-receipts-observed
atomic-materialization-and-basis
automatic-reissue-risk-visible
committed-sequence-stability
content-free-diagnostics
crash-cut-receipt
deleted-state-empty-and-fenced
deletion-effect-identity
deletion-fence
detached-result-refusal
durable-queue-recovery
exact-retry
external-effect-unknown-recovery
idempotency-conflict-refused
immediate-delete-policy
lane-order-independence
manual-remote-reconciliation
outstanding-effect-identity
pending-receipt-binding
pending-restart-recovery
receipt-binding-refusal
receipt-domain
recovery-retry-authorization
rotation-fence
saved-label-requires-commit
structural-diagnostic-regression
wait-delete-policy
wait-effect-exactness
```

The pure transition function also fails loudly on illegal admission, scheduler,
snapshot, deletion, and detached-writer actions. Those reducer precondition
guards are not included in the 31 reported invariant-family count.

The first executable pass correctly stopped on a prototype assertion bug: the
Saved invariant used substring matching and therefore mistook `Not saved` for
the positive `Saved` label. The invariant now compares only the configured
positive label. The complete matrix then passed. No production behavior was
changed in response.

### Review correction TDD evidence

The correction used the pure `transition(state, action)` seam inside the same
one-command executable. No separate test framework or persistence was added.

1. Receipt binding and durability domain:
   - Red: exit 1 reported all three forbidden acceptances:
     `delayed-pre-rotation-receipt`, `accepted-during-deletion`, and
     `bogus-stream-kind`.
   - Green: acceptance now validates exact owner, Session key/epoch/lease,
     lane, event id/digest, Active lifecycle, allowed prestate, and a stream kind
     of exactly `Existing` or `New`.
   - Field matrix: 14 additional cases refuse mutated action/bound lanes, every
     owner and Session generation field, event id/digest, lifecycle, prestate,
     and an otherwise well-formed acceptance attempted from Idle.
2. Exact deletion-wait effects:
   - Red: exit 1 reported missing two-attempt registration, incorrect first and
     replayed-attempt draining, five mutated identity fields
     (`lane`, `sessionKey`, `sessionEpoch`, `lease`, `resultRef`), and a
     cross-lane stale token. The final green matrix also refuses an invalid
     terminal result kind without draining the wait.
   - Green: every dispatch attempt has one exact effect tuple; the fenced wait
     removes only a matching terminal and preserves every other attempt.
3. Structural diagnostics:
   - Red: exit 1 reported raw arbitrary fields and malicious values in both
     detached-result and detached-writer diagnostics.
   - Green: the diagnostic builder accepts only closed codes/reasons, validated
     lane/numeric fields, and `opaque:xxxxxxxx` identifier/effect hashes.

The final executable run includes all 30 first-round correction regressions in its passing
case count.

### Pending-restart correction TDD evidence

The second correction stayed on the same pure `transition(state, action)` seam
and made a separate red/green pass inside the one-command executable.

1. Minimal transition tracer:
   - Red: actual `Restart` from `Pending` reported
     `restart-state-invariants`, `pending-to-recoverable-uncertain`, and
     `current-reconciliation-rejected`.
   - Green: restart now produces `OutcomeUncertain` with durable event,
     digest, attempted-sequence, prior-receipt, stream-kind, crash-cut, and
     allowed-disk-outcome evidence. The old binding is retained only as an
     `opaque:xxxxxxxx` quarantine reference; the new binding names the current
     lease and recovery prestate.
2. Every-cut matrix:
   - Red: all 14 `Existing`/`New` by seven-crash-cut cases reported a missing
     recovery domain.
   - Green: every case invokes the real `Restart` action from `Pending`, rejects
     the stale pre-restart receipt, checks no materialization/basis advancement,
     and explores every allowed disk observation. Durable exact converges
     without appending, Absent authorizes one exact retry, and torn tail remains
     safely uncertain.
3. Lifecycle fences:
   - Red: deletion of the recovered admission reported
     `deletion-fence-state-invariants`.
   - Green: rotation invalidates the recovery capability; deletion quarantines
     it and leaves no current binding. Both refuse reconciliation through the
     retired binding, and the full state invariants still hold.

These 21 second-round regression cases bring the complete correction total to
51 without adding a separate test framework.

### Recovery-retry correction TDD evidence

The third correction reused the pure `transition(state, action)` seam and the
same in-command exhaustive checker.

1. Direct uncertainty bypass:
   - Red: all 14 `Existing`/`New` by seven-crash-cut direct retries reported
     `direct-retry-visible`.
   - Green: an uncommitted retry now requires an `AbsentRetryAuthorized`
     recovery state; `OutcomeUncertain`, plain `Rejected`, and torn-tail states
     cannot append or advance materialized/basis state.
2. Recovery-domain and barrier proof:
   - Red: New/AfterEnqueue -> Absent accepted both
     `cross-kind-retry-visible` and `missing-directory-sync-retry-visible`.
   - Green: Absent reconciliation retains the exact cut and `New` kind, rebinds
     authorization to the current lease, rejects `Existing`, and requires the
     supplied New proof to include directory sync. Restart rebinds but does not
     broaden the authorization; its stale binding is refused.
3. Idempotent convergence:
   - DurableExact reconciliation returns `AlreadyAccepted(1)` and a subsequent
     exact retry preserves the identical commit rather than appending.
   - The authorized retained-kind retry commits sequence `1` once; its exact
     replay returns `AlreadyAccepted(1)` with no duplicate materialization or
     Projection Basis advancement.

These 19 third-round regression cases bring the complete correction total to
70 without adding persistence or a separate test framework.

## Human decisions still open

All policy profiles are safety-valid, so these remain product decisions rather
than hidden defaults:

1. **Saved wording.** Recommendation: reserve the shorter `Saved` exclusively
   for durable `Accepted`/`AlreadyAccepted`; use `Saving`, `Recovery required`,
   and `Not saved` elsewhere. Human acceptance or a choice of the more explicit
   `Durably saved` is still required.
2. **Remote reissue.** Recommendation: do not automatically reissue an
   `ExternalEffectUnknown` request unless the exact route has independently
   proven provider idempotency. Human acceptance or explicit authorization of
   duplicate cost/content-egress risk is still required.
3. **Deletion.** Recommendation: make immediate fenced discard the default.
   A future wait mode may observe provider terminals but must never accept a
   result or reopen a writer. Human acceptance or a requirement for wait mode
   is still required.

## Later-work mapping

- `audio-graph-3b48` owns durable scheduler enqueue, exact job/attempt recovery,
  exact effect identity persistence, independent lane progression, and accepted
  remote-reissue policy.
- `audio-graph-90f3` owns the production `Pending` to receipt-bearing canonical
  commit boundary, stable sequence/idempotency, atomic post-accept materialized
  advancement, exact receipt binding, restart rebase/reconciliation, typed
  Absent retry authorization, and snapshot-cache authority.
- `audio-graph-8e73` owns new-file directory entry durability, typed quarantine
  registration, locked destructive recovery, supplied retry-barrier proof,
  subprocess cut points, and the rule that a weak platform level cannot claim
  Accepted/Saved.
- `audio-graph-7e81` may advance the monotonic Session semantics floor only
  from this model's `Accepted`/`AlreadyAccepted` receipt and must preserve
  guard-ahead idempotency.
- `audio-graph-0baf` and `audio-graph-4c82` consume the accepted canonical
  sequence for first-position ledger/basis order, currency, scheduler, and
  patch validation; no Pending or uncertain event is eligible.
- `audio-graph-9c89` and later `audio-graph-e969` own Session rotation/load and
  deletion: revoke writers first, use the typed inventory, refuse late results,
  track exact outstanding effects, and report residual artifacts exactly.
- `audio-graph-44c1` owns acceptance evidence for the content-free diagnostic
  codes/counters and the later production failure matrix.

The detailed transition-by-transition table is in the prototype document. No
later Seed was modified, unblocked, closed, dispatched, merged, or pushed here.

## Verification

### Syntax check

Command:

```text
node --check scripts/prototype-session-projection-admission.mjs
```

Output: none. Result: pass, exit 0.

### Executable invariant run

Command:

```text
bun scripts/prototype-session-projection-admission.mjs
```

Final output summary:

```text
cases explored: 814
transitions evaluated: 7671
unique full states observed: 1262
invariant assertions: 111905
invariant families passed (31)
Policy decision table (all eight combinations passed safety invariants)
PASS: finite model exhausted; every invariant held; diagnostics remained content-free.
```

Result: pass, exit 0.

### Docs/Seeds secret hygiene

Command:

```text
bun scripts/check-docs-secret-hygiene.mjs
```

Output:

```text
docs/Seeds secret hygiene scan passed: 0 findings
```

Result: pass, exit 0.

### Betterleaks

Command (complete assigned footprint):

```text
betterleaks dir --no-banner --redact \
  docs/prototypes/session-projection-admission-and-fencing.md \
  scripts/prototype-session-projection-admission.mjs \
  docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-5e41-prototype-report.md
```

Output:

```text
no leaks found
```

Result: pass, exit 0 across the complete three-file correction footprint.

### Diff hygiene and footprint

Commands:

```text
git diff --cached --check
git diff --cached --name-only
git status --short
```

Output:

```text
git diff --cached --check: no output
docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-5e41-prototype-report.md
docs/prototypes/session-projection-admission-and-fencing.md
scripts/prototype-session-projection-admission.mjs
M  docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-5e41-prototype-report.md
M  docs/prototypes/session-projection-admission-and-fencing.md
M  scripts/prototype-session-projection-admission.mjs
```

Result: pass, exit 0. The correction footprint is exactly the same three
assigned paths. `HEAD` and its merge base with the reviewed correction base
were both `3a869efa187ed302490eb30758ca18b9fb07c14a` before commit.

The repository Biome configuration intentionally ignores the new `.mjs`
script (`Checked 0 files` / `No files were processed`), so that invocation was
not counted as a gate. The executable invariant run is the prototype's code
and behavior gate, as required by the LOGIC prototype contract.

## Findings and open questions

- A newly created canonical provenance/projection file still cannot make a
  production `Accepted` claim until `audio-graph-8e73` proves directory entry
  and typed quarantine-manifest durability on Windows, macOS, and Linux.
- Provider idempotency is not assumed. Without route-specific evidence,
  automatic retry after an unknown external effect can duplicate cost and
  content egress even though local fences prevent duplicate state.
- Waiting during deletion changes latency, not write safety: the fence is
  mandatory, wait completion requires exact outstanding-effect terminals, and
  observed remote results remain unusable under both policies.
- The bounded model intentionally does not choose backlog coalescing,
  concurrency, final-refinement, migration, or exact UI presentation beyond
  the three explicit policy recommendations.

No unrelated defect was changed and no additional Seed proposal was created in
this bounded prototype workstream.
