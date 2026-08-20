# Constraint set for Projection Backlog scheduling (audio-graph-3b48)

Extracted from `docs/adr/0024`, `0027`, `0029`, `0031`, `0035`, `0036`, `0041`,
`0042`, `0043`, `0044`, and the `sd show` output for `audio-graph-3b48`,
`audio-graph-fbca`, `audio-graph-44c1`. This is a read-only extraction; it does
not decide anything itself.

## MUST — binding constraints

### Basis, staleness, and classification (ADR-0024, ADR-0031)

1. Every projection job is bound to an exact `ProjectionBasis`; a completion is
   applied only if the basis still validates. Stale output must be rejected by
   construction, not by convention (ADR-0024 Decision Drivers, §2).
2. The scheduling design must use ADR-0031's classifier — **Current /
   AppendOnly / Revised** — as the *sole* basis-currency test. No independent
   second interpretation of basis currency is permitted; `validate_basis` must
   delegate to (or be mechanically proven equal to) this classifier
   (ADR-0031, Decision Outcome, item labelled "one shared classifier").
3. Classification order is fixed: select the current-ledger subset covered by
   the basis → hash that subset → compare hash + ordered identities/revisions
   → classify missing/reordered/deleted/revised/hash-mismatched coverage as
   **Revised** → unchanged-and-no-extra as **Current** → unchanged-with-only-
   later-appended spans as **AppendOnly** (ADR-0031, Decision Outcome §1-6).
4. **Current** output follows the normal apply path. **AppendOnly** output may
   also apply, but must schedule exactly **one coalesced Background follow-up**
   job for the appended spans — not one job per append. **Revised** output
   must never mutate notes/graph state; it schedules **Replay repair** only
   when scheduler policy permits (ADR-0031, Decision Outcome, final paragraph).
5. Automatic projection bases are restricted to final/end-of-turn/stable
   transcript revisions. An appended *provisional* span must never mark a
   final-only basis stale and must never trigger a follow-up job (ADR-0031,
   Decision Outcome, "Automatic projection bases…").
6. Every completion, failure, and telemetry update must be matched against the
   exact job id, session id, scheduler session, and ledger session before it
   is allowed to mutate scheduler state. A late worker from a rotated or
   superseded session must be ignored with an observable counter; "completion
   by projection kind alone is not permitted" (ADR-0031, Decision Outcome,
   penultimate paragraph). This directly bounds how the design handles session
   rotation / bounded concurrency.
7. Classification governs semantic applicability only, not durability: a
   Current/AppendOnly output becomes visible as durable state only after its
   canonical projection event is **Accepted** under ADR-0027. Queue enqueue,
   writer send, or a materialized snapshot save is insufficient (ADR-0031,
   Decision Outcome, last paragraph; reaffirmed by ADR-0024 §6 replay
   semantics).

### TTFT-aware scheduling and coalescing (ADR-0024)

8. While a job is in flight, newer ledger state must be **coalesced** into the
   pending basis (not spawn a duplicate LLM call), with a typed reason
   (`PendingSpanThreshold` / `InFlightAgeThreshold` / `TtftWindow`) driven by an
   observed `ttft_estimate_ms` (ADR-0024 §5).
9. On completion, a basis found stale must be **discarded and a repair job
   started**; an unchanged failed basis must **idle rather than retry forever**
   (ADR-0024 §5). The design must define what "unchanged" and "idle" mean
   precisely enough to avoid the unowned stall noted below (constraint 21).
10. Notes and graph are scheduled per-kind (`ProjectionSchedulers` = notes +
    graph lanes) but share one basis/patch-log/replay contract (ADR-0024 §1-3,
    §5-6). The two-lane structure is existing architecture to extend, not an
    open design choice to redo.
11. `apply_validated_patch*` (live path, re-validates basis) and
    `apply_replayed_patch`/`replay_accepted_patches` (trusts the accepted log,
    because a later transcript span would make an earlier valid patch look
    stale on replay) are two distinct, already-settled code paths. The
    scheduling design must preserve this split, not collapse it (ADR-0024 §6).

### Storage authority (ADR-0027, ADR-0029, ADR-0043, ADR-0044)

12. A record is `Pending` on bounded enqueue and becomes `Accepted` only after
    its framed canonical bytes and required filesystem metadata cross the
    declared durability boundary. Backlog/scheduler state must treat only
    `Accepted` canonical events as durable authority; anything else (in-memory
    queue, worker state, snapshot) is not (ADR-0027, Decision Outcome).
13. Independent canonical streams never claim a shared total order — no global
    sequence across the notes lane and the graph lane. Cross-lane
    reconciliation must be expressed without inventing one (ADR-0027, Decision
    Outcome; ADR-0042 §4 for the multi-source ordering rule specifically).
14. A snapshot/materialized-cache failure is derived-cache lag, not logical
    event failure; snapshots never override canonical logs (ADR-0027, Decision
    Outcome). Any scheduler-visible "coverage head" cache is a derivative, not
    an authority.
15. One active-session aggregate owns the schedulers, buffers, and writers;
    historical review is read-only with respect to that aggregate (ADR-0027,
    Decision Outcome). Backlog state belongs to the single owning aggregate.
16. Backlog/scheduler bookkeeping must not create a fifth canonical event
    stream. The registry is frozen at four: `transcript_revisions`,
    `speaker_revisions`, `projection_patches`, `data_movement_events` (ADR-0043,
    Decision Outcome table). Any new persisted scheduler state must be either
    non-canonical (disposable, rebuildable) or expressed inside the existing
    `projection_patches` stream's accepted-patch semantics.
17. Any derived/cached structure the scheduler needs (e.g. a coverage head
    cache) must be disposable, versioned, deterministically rebuildable from
    canonical streams, and a head-vector mismatch must trigger rebuild/bounded
    catch-up — never "authority arbitration" (ADR-0029, Decision Outcome
    items 3, and the "Any accepted index is disposable…" paragraph).
18. Backlog/index absence, lag, corruption, or rebuild must never block
    canonical capture, review, export, or deletion (ADR-0029, Decision Outcome
    item 4).
19. Integrating with the v1→v2 `session_semantics_version` floor admission must
    use the existing store-owned global lock; the design must not introduce a
    second, unproved per-Session lock identity (ADR-0044, Decision Outcome
    item 4). The one v1-to-v2 transition proof is per-Session control state,
    not a queue/backlog record and not a canonical stream (ADR-0044 items 6-7).

### Finalization-lane consumers (ADR-0035, ADR-0036)

20. `audio-graph-3b48` is explicitly assigned ownership of **backlog
    scheduling, lane reconciliation, and the retry progression** whose budgets
    ADR-0036 sketches (ADR-0036, "Downstream ownership"). The retry-progression
    design must be defined in terms of, and must not conflict with: the
    per-attempt deadline (owned by `audio-graph-21e9`'s route contract) and the
    per-lane totals / no-progress rule that lands in `Finalization Blocked`
    (claimed by ADR-0036 itself). ADR-0036 flags this exact ownership line as
    contested (reconciliation D6/C11) — 3b48 must state its boundary
    explicitly rather than silently redefining either neighbor's territory.
21. ADR-0036 flags a currently unowned stall: `projection_scheduler.rs:355-357`
    returns `Idle` forever when `last_failed_basis == basis` on an unchanged
    basis. The scheduling design must give this an explicit owner/resolution
    (ADR-0036, "Not decided here"; Negative consequence "an unowned stall
    survives this decision").
22. Required Projection Lanes for `Finalized`: **notes required, graph
    recorded-but-not-required**. A Session reaches `Finalized` when the notes
    lane is covered; graph-lane backlog/failure must not gate that boundary
    (ADR-0036, "accepted sub-question defaults," item 1).
23. Nothing in the retry/backoff design may make automatic cross-provider
    fallback load-bearing — no ADR authorizes that egress, and ADR-0035/0036
    exist partly to remove the pressure toward it (ADR-0035 Decision Drivers;
    ADR-0036 Decision Drivers, last bullet).
24. Restart recovery must be **normal progress on the same code path** — there
    is no persisted "resume at stage N" branch to build. Backlog/lane state
    must be re-derivable from durable evidence on every open, not resumed from
    a stage field (ADR-0036, Decision Outcome, "Restart recovery is normal
    progress…").
25. Deletion always wins: late results after a Session is deleted must be
    discarded via fencing (ADR-0036, Decision Outcome, "Deletion always wins").
26. Cancellation of finalization work lands as `Blocked{UserCancelled}`, never
    silently as `Finalized` (ADR-0036, Decision Outcome).
27. Coverage/backlog state must be computed (re-derived), not read from a
    glanceable persisted field, and any cache used to make that affordable
    must qualify as an ADR-0029-class disposable derivative — whether a
    per-Session finalization cache clears ADR-0029's seven preconditions is
    explicitly **unresolved** (ADR-0036, Negative consequences). The design
    must not assume that gate is already clear.

### Speech-revision/basis versioning (ADR-0041, ADR-0042)

28. Basis creation, patch creation, and patch apply must refuse v2 work while
    the durable `session_semantics_version` floor is v1; a v2 basis/patch
    observed ahead of an unaccepted floor is **typed corruption**, not ordinary
    staleness, and must not promote or repair the Session (ADR-0041 Decision
    Outcome §7; ADR-0042 Decision Outcome §8).
29. Basis creation, covered-subset hashing, currency classification, patch
    validation, and replay must all dispatch on the basis's **declared hash
    version** (frozen hash v1 vs. hash v2); an unsupported/mismatched version
    is a typed failure and must never fall back to v1 (ADR-0042 Decision
    Outcome §1, §9).
30. Each logical span keeps its **first-Accepted canonical position** across
    later revisions — a revision never moves a span after later spans.
    Cross-source ordering uses first-Accepted canonical sequence, not
    source-local ordinals or provider timestamps (ADR-0042 Decision Outcome
    §4). Any notion of "covered subset" or lane coverage the scheduler
    computes must respect this ordering.

## SHOULD — soft preferences

- Prefer giving useful progressive notes during continuous speech over
  starving output until a pause — this is the stated rationale for choosing
  AppendOnly-with-follow-up over discard-until-latest (ADR-0031, "Pros and
  Cons," chosen option). Live-vs-finalization priority should favor
  progressive availability of notes during active speech.
- Prefer retcon operations (invalidate/merge/split) over duplicate nodes when
  later transcript context corrects earlier assumptions, mirroring
  `graph/temporal.rs` (ADR-0024 §4). The two-lane reconciliation the ticket
  asks for should lean on this existing mechanism rather than inventing a
  parallel one.
- Prefer coalescing repeatedly (one Background follow-up, one repair job) over
  one job per revision/append, to bound backlog growth under continuous
  speech (ADR-0024 §5, ADR-0031 Decision Outcome §item 4).
- Prefer minimal additional persisted state. The project's consistent bias
  across ADR-0027/0029/0036 is to re-derive from canonical evidence rather
  than add a durable "progress" field; any new backlog-scheduler state should
  default to non-durable/derivable unless a specific durability need is
  argued (ADR-0027 Decision Outcome; ADR-0029 Decision Outcome; ADR-0036
  Decision Outcome/"Pros and Cons" of option A vs B).
- Prefer named, measured budget constants in the style of the existing
  p99-tuned constant at `src-tauri/src/state.rs:1112-1128`, rather than ad hoc
  numbers, for any per-lane attempt/backoff budget the design introduces
  (ADR-0036, Decision Outcome, "Local Durable Stop" bullet).
- Should explicitly flag, rather than silently resolve, any place where this
  design's scope touches a claimed-but-uncut seam — in particular the
  four-way claimed ownership of "final refinement" budgets/coverage
  (`3b48` / `fbca` / `21e9` / ADR-0036 itself) that ADR-0036 calls out as
  undeclared (ADR-0036, Negative consequences, reconciliation C11).

## Explicit non-goals (already settled by these ADRs — do not re-decide)

- **Whether to add an embedded query index/database.** Settled by ADR-0029:
  gated on measured product demand with seven named preconditions; not
  reopened by backlog scheduling.
- **The basis-currency classifier itself** (Current/AppendOnly/Revised).
  Settled by ADR-0031; 3b48 consumes it, does not redesign it.
- **The basis hash algorithm and versioning scheme.** Settled by ADR-0042
  (hash v1 frozen; hash v2 semantic encoding, domain-separated SHA-256); 3b48
  dispatches on the declared version, does not redefine it.
- **Canonical stream identities and count.** Frozen at four by ADR-0043; not
  open for 3b48 to add a fifth stream for backlog/queue bookkeeping.
- **The durable commit protocol (Pending → Accepted) and the no-dual-authority
  storage principle.** Settled by ADR-0027; 3b48 conforms, does not redesign
  storage durability.
- **Whether finalization state is a persisted stage machine or a derived
  barrier reconciler.** Settled as derived-barrier-reconciler by ADR-0036;
  3b48's scheduling design must integrate with a derived model and must not
  introduce its own persisted "stage" field for lane progress.
- **Whether post-stop finalization failure is app-modal or per-Session.**
  Settled by ADR-0035 as per-Session `Finalization Blocked`; 3b48's retry
  progression feeds this state but does not redecide its scope or
  non-dismissable property.
- **Cross-provider fallback authorization / provider selectability.** Owned by
  `audio-graph-21e9` and gated by ADR-0033 (referenced, not read directly in
  this pass); 3b48 must avoid creating pressure toward unauthorized fallback
  but does not decide fallback policy itself.
- **Session control-plane addressing and the locking scheme.** Settled by
  ADR-0044 (flat artifact root, Base32 session key, single store-owned lock);
  3b48 integrates with the existing lock, does not design a new one.
- **Output budgets and coverage accounting for long-Session final
  refinement.** ADR-0036 records this as `audio-graph-fbca`'s explicit claim;
  3b48 must not annex it even though "coverage" and "budgets" also appear in
  3b48's own scope (see MUST #20 and SHOULD, last bullet) — the two must be
  reconciled by boundary statement, not by one ticket absorbing the other.
- **Whether an unmet evidence obligation or unconfirmed High-Impact Inference
  holds the `Finalized` boundary (Q0.2).** Settled outside this ADR set
  (referenced by ADR-0036 as a premise); not 3b48's to reopen.

## What `audio-graph-fbca` and `audio-graph-44c1` need from this decision

`sd show` confirms the dependency shape: `3b48` blocks both `fbca` and `44c1`;
`fbca` also blocks `44c1`. Decision order is `3b48` → `fbca` → `44c1`.

**`audio-graph-fbca`** ("Define evidence-aware long-Session refinement") is the
provider-neutral final-refinement algorithm over Sessions that may exceed
model context. From `3b48` it needs:

- The **stop-time flush state** of both lanes — what "coalesced Background
  follow-up" and "Replay repair" backlog looks like at the moment finalization
  begins, so fbca's partitioning has a well-defined starting point rather than
  an ambiguous in-flight scheduler state.
- The **per-lane coverage definition** (notes required / graph
  recorded-but-not-required, MUST #22) that fbca's "coverage accounting" and
  "evidence never silently truncated" guarantee must build on — fbca cannot
  invent its own coverage notion that disagrees with the lane semantics 3b48
  fixes.
- The **basis/hash-version dispatch rules** (MUST #28-30) so fbca's
  partition-and-reconcile algorithm composes correctly across sessions that
  straddle the v1→v2 semantics floor, and across a mixed legacy/v2 covered
  set, without re-deriving that logic itself.
- An explicit statement of what 3b48 does **not** own — "output budgets" and
  "coverage accounting" are fbca's named claims per ADR-0036; 3b48 must leave
  that boundary clear rather than pre-empting it under "retry progression."

**`audio-graph-44c1`** ("Define the trustworthy Session Memory acceptance
contract") turns the resolved architecture into claim-bounded
acceptance/failure-injection evidence for the Deepgram + Cerebras/OpenRouter
vertical slice. From `3b48` (and `fbca`) it needs:

- The **resolved state set and taxonomy** for backlog scheduling outcomes
  (Current/AppendOnly/Revised dispositions, coalesced follow-up, Replay
  repair, idle-vs-retry, session-rotation rejection) so 44c1 can write
  deterministic, crash/restart, deletion, truncation, and provider-failure
  fixtures against named states rather than ad hoc behavior.
- The **bounded-concurrency and retry-progression rules** (MUST #20-21) as
  concrete, testable budgets/backoff definitions — 44c1's failure-injection
  contract needs named thresholds to inject failures at, not prose.
- Confirmation that **restart recovery is one code path** (MUST #24), which is
  exactly what 44c1's deterministic offline fixture tier (ADR-0032-style)
  depends on to prove crash/restart without a second code path to fixture
  separately.
- The **lane-reconciliation-without-erasing-provenance** answer (the ticket's
  own framing) as a concrete mechanism (e.g., retcon ops, Replay repair) that
  44c1 can assert against directly — e.g. "no accepted patch is silently
  discarded; a Revised basis's prior accepted output remains inspectable."
