# ADR-0045 scheduler epic — implementation design

Synthesized 2026-08-20 from three competing designs (minimal-diff, clean-module,
defect-first) against the constraint sheet in this directory. Every line number
below was re-verified against `master` at `bcaaa6e` — which is two commits past
the constraint sheet's `3354f9d` and includes the landed 2cf9 claim-class
validator (`99530ad`).

## Fact base (verified at `bcaaa6e`, 2026-08-20)

- **2cf9 has LANDED.** The constraint sheet's §5 "coordinate or sequence"
  question is moot. `apply_validated_patch_with_speaker_timeline_opt` is now
  `src-tauri/src/projections.rs:2506-2546`; the two-way gate call is
  `:2512-2514`; 2cf9's evidence-resolution block is `:2516-2527`; the
  `apply_patch(patch, Some(&evidence_basis))` calls are `:2531`/`:2538`.
  There is no remaining external in-flight dependency for this epic.
- **A second two-way gate exists in the replay path**:
  `replay_accepted_patches_with_history` calls
  `validate_basis_with_speaker_timeline` at `projections.rs:2449-2461` and, on
  rejection, increments `invalid_patch_count` and skips the patch. Critically,
  `load_session_impl` (`commands.rs:7081`) refuses to load ANY derived caches
  when `invalid_patch_count > 0`. So fixing only the live gate would mean the
  first persisted append-only patch breaks reopen of that session visibly.
  Both gates must change in the same commit.
- **The mandatory coalesced follow-up already exists.** `run_projection_job`
  routes a `StaleBasis` apply error to `ProjectionJobCompletion::Completed`
  (`speech/mod.rs:2352-2373`), and `complete_in_flight`'s
  `BasisCurrency::AppendOnlyStale` arm (`projection_scheduler.rs:400-415`)
  sets `last_completed_basis` and unconditionally starts exactly one
  `Background` follow-up, covered today by
  `scheduler_coalesces_append_only_completion_into_one_background_follow_up`.
  Decision 2's follow-up needs **zero new scheduler code**; the only live
  defect is that the append-only patch *body* is discarded at the gate, and
  the telemetry conflates applied-append-only with discarded-revised.
- `validate_basis` / `validate_basis_with_speaker_timeline`
  (`projections.rs:786`/`:796`) have real two-way callers that must not
  change: `projections.rs:985` (`is_basis_current`), `projection_llm.rs:668`,
  `promotion.rs:552`. The three-way `classify_basis_currency` (`:813`) is
  already correct.
- The Idle-forever stall is `projection_scheduler.rs:354-356`. The scheduler
  struct's four `Option` fields are `:158-161`. `restore_from_snapshot`
  (`:845`) has zero production call sites; its only callers are its own three
  unit tests (`:1500`, `:1580`, `:1665`). `save_scheduler_queue_state` is
  written once at `state.rs:961` inside `rotate_session`.
- `spawn_projection_job` (`speech/mod.rs:1991-2014`) discards the
  `JoinHandle`. The reusable join pattern is `retired_session_workers`
  (`state.rs:286`) + `join_worker_with_timeout` (`commands.rs:892`), already
  used inside `stop_capture_impl`'s `spawn_blocking` block
  (`commands.rs:1770-1788`, 3s timeouts). `ensure_session_workers_quiesced`
  (`commands.rs:919`) fences Start/rotation on the retired vec.
- The revert-pin idiom already exists: `include_str!("projection_scheduler.rs")`
  / `include_str!("state.rs")` source assertions at `projections.rs:5161-5177`
  (one already pins that `state.rs` calls `.apply_validated_patch(ledger, &patch)`
  — any gate-entry rename must update this pin in the same commit).
- The env-gated heavy-test precedent is `rotation_under_concurrent_load`
  (`state.rs:2232`, gated on `RSAC_TORTURE=1`).
- The live runtime apply caller is `state.rs:566-568`; the result type is
  `ProjectionRuntimeApplyResult` (`state.rs:353-361`). The test that pins the
  bug is `runtime_projection_patch_rejects_stale_basis_without_persistence`
  (`state.rs:1771`). The reusable dispatch harness is
  `runtime_projection_dispatch_follows_up_append_only_apply_with_current_basis`
  (`speech/mod.rs:10124`, `FnProjectionPatchGenerator`).
- Cold start mints a fresh session id unconditionally (`state.rs:684`);
  `load_session_impl` never touches `AppState.projection_schedulers`. There is
  no live "reopen into a running lane" product surface today.

## Base selection: Design 1 (minimal-diff)

Judged on the four axes:

**Satisfies all six decisions.** Design 1 covers all six with the least
machinery because it is built on the (verified) discovery that decision 2's
follow-up mechanism already exists. Design 2 spends its largest artifact — a
`#[must_use] CoalescedFollowUpObligation` threaded through the runtime apply
result — re-implementing what `complete_in_flight` already does, and its own
weakness statement concedes the `#[must_use]` is a lint, not a proof. Design
2 also under-delivers decision 3: its "no timer thread, the only clock is
`due_at_ms` read by existing event sources" means a lane that fails and then
receives no further finals never fires its ~60s retry at all — the one
deferred retry needs exactly one clock source, which Design 1's registered
one-shot thread provides without becoming a backoff ladder.

**Survives partial delivery.** Design 1's commits are file-disjoint (the
scheduler, dispatch, stop, and benchmark commits never touch the gate commit's
files). Design 2's C5 — rewriting the scheduler's internals to `LaneActivity`
in the same wave that changes what "stale" means — is the worst
partial-delivery outcome in any of the three designs: it invalidates the only
regression net (the 15 scheduler unit tests) at the exact moment the gate
semantics change, and its own weakness statement admits bisect gives no clean
answer afterward. The type-algebra payoff (12 unreachable state combinations
eliminated) is real but the one combination that actually escapes today is
produced only by `restore_from_snapshot`, which this epic deletes anyway — so
the migration buys little and risks much. Rejected.

**Honest about 2cf9.** Design 1 is the only design that states the true fact:
2cf9 landed at `99530ad`, master is `bcaaa6e`, and the epic rebases onto the
landed signatures. Design 3 still entertains "if T1 must precede 2cf9" and
cites pre-2cf9 line numbers (`projections.rs:2327-2329`); Design 2 verified
the landed content but still phrases the plan as "land 2cf9 first."

**Smallest reader load.** No new modules (`persistence/scheduler_queue.rs` is
never created, matching 464c's rescope), ~6 production lines at the gate,
reuse of the existing join-with-timeout pattern, the existing `include_str!`
pin idiom, and the existing dispatch-test harness.

### Grafts taken from the other designs

1. **From Design 2 (correctness-critical): both gates go three-way in the
   same commit.** Design 1 misses the replay gate at `projections.rs:2450`
   entirely. Verified consequence of missing it: after the live-gate fix
   persists one append-only patch, `load_session_impl` sees
   `invalid_patch_count > 0` on that session and refuses to load its derived
   caches — reopen breaks visibly. (The replayed point-in-time ledger at
   `patch.created_at_ms` includes the tail appended during LLM generation, so
   the replayed classification of an append-only-accepted patch is also
   `AppendOnlyStale`.) Also grafted: a `gate_arms_agree_with_classify_basis_currency`
   test so the two sites cannot silently diverge, and the explicit statement
   that `validate_basis`/`validate_basis_with_speaker_timeline` survive for
   their three real two-way callers.
2. **From Design 3: defect-first ordering and prefix shippability.** Every
   ticket prefix leaves the system strictly better than HEAD: the gate fix
   alone kills a live data-loss bug; + budget bounds stalls; + registry makes
   stop honest; + heads/benchmark makes reopen honest and measured; deferred
   retry — the only new concurrent actor — lands last, when the registry
   exists to make its bug class (double-dispatch) diagnosable.
3. **From Design 3: `basis_currency_at_apply` on `ProjectionRuntimeApplyResult`.**
   The maintainer-mandated telemetry split (applied-append-only vs
   discarded-revised) belongs in `record_projection_apply_result` /
   `run_projection_job`'s logging in `speech/mod.rs`, which needs the currency
   as data, not as a log line buried at the gate. This is a two-field-smaller
   version of Design 2's obligation type: it carries information outward but
   creates no discharge obligation the scheduler doesn't already meet.
4. **From Design 3: the attempt budget is keyed to basis identity and resets
   on any basis change.** This keeps today's self-healing property (a new
   basis always un-wedges the lane), makes the budget ticket shippable before
   the deferred-retry ticket, and honestly names the residual hole — a ledger
   that appends between failures gets unbounded per-lane attempts — as
   ADR-0036's territory (per-lane totals, constraints MUST #20), stated in
   the commit body, not silently absorbed.
5. **From Designs 2/3: the registry carries per-job identity**
   (`(ProjectionKind, job_id, JoinHandle)`), and timed-out handles spill into
   `retired_session_workers` so `ensure_session_workers_quiesced` keeps
   fencing rotation with zero new fence logic. Design 1's bare
   `Vec<(String, JoinHandle)>` loses the kind and invents a second stopping
   fence semantics; the spill idea is strictly better.
6. **From Design 3: the benchmark is an env-gated `#[ignore]` test with a
   code-generated fixture**, following the `rotation_under_concurrent_load` /
   `RSAC_TORTURE=1` precedent (`state.rs:2232`) — not a Criterion bench
   (Criterion has no assertion story for a p95 gate and would add a dev-dep)
   and not a checked-in fixture blob (2cf9-style evidence-field churn would
   rot it; a `synth_two_hour_session(seed)` generator cannot rot).

### Design 1's worst weakness, addressed head-on

Design 1's own weakness statement is correct: there is no live
reopen-into-a-running-lane product surface, so the coverage-head reseed at
`start_capture` is a no-op in most real runs, and the mechanism risks
bit-rotting like `restore_from_snapshot` did. The honest scope, adopted here:

- Decision 6's crash story is **recovery-by-re-derivation**, and the
  re-derivation mechanism (`replay_accepted_patches_with_history`) is already
  a real, shipping path — `load_session_impl` runs it on every historical
  open. The p95 benchmark therefore measures a path users hit today, not a
  hypothetical; it is not vulnerable to the weakness.
- The reseed itself is one call at the one place schedulers attach to a
  session (`start_capture_impl`, before the speech thread spawns), plus a
  test that constructs the "session with existing accepted patches enters a
  live lane" scenario by hand. That is the minimal honest consumer. The
  anti-bit-rot control is the deletion of `restore_from_snapshot` in the same
  ticket: after it, `reseed_coverage_heads` is the *only* way scheduler
  coverage state comes from disk, so any future resume/reopen surface must go
  through it — it cannot be routed around the way the snapshot was.
- Building an actual reopen product surface is out of scope for this epic
  (it is `9751`-adjacent product work, not ADR-0045 hardening) and is named
  as such rather than smuggled in.

## Data shapes (all additive; no new modules)

In `projections.rs`, beside the existing wrappers (`:786-822`):

```rust
pub enum AppliedBasisCurrency {
    Current,
    AppendedTail(ProjectionBasisStaleness),
}
```

- The live gate (`:2506`) and the replay gate (`:2450`) both match
  `classify_basis_currency` three ways: `Current` and `AppendOnlyStale` fall
  through to 2cf9's evidence-resolution block (sound: reaching
  `AppendOnlyStale` proved every pinned span still resolves at its pinned
  revision with matching hash — only `Revised` breaks the precondition the
  `:2516-2521` comment states; both comments get reworded, no span is
  laundered); `Revised` still errors `StaleBasis`.
- `apply_validated_patch` grows a currency-reporting sibling used by the one
  live caller (`state.rs:567`); `projection_eval.rs:434/1516/1538` keep their
  arity. The `projections.rs:5171-5177` source pin is updated to require the
  new entry point.

In `state.rs`:

```rust
pub struct ProjectionRuntimeApplyResult {
    // ...existing fields...
    pub basis_currency_at_apply: AppliedBasisCurrency,   // telemetry substrate
}
```

In `projection_scheduler.rs` (extend, never replace, per constraint sheet §3):

```rust
// on ProjectionScheduler
failed_attempts: u8,                    // keyed to last_failed_basis identity
deferred_retry_at_ms: Option<u64>,
pending_since_ms: Option<u64>,

// new ProjectionSchedulerDecision variant — THE typed no-progress signal.
// 3b48 emits only; no FinalizationBlocked type is created
// (llm/executor.rs:846-850 stays true; ADR-0036 D6/C11 / MUST #20 boundary).
AttemptBudgetExhausted { kind, basis, attempts, last_staleness, oldest_pending_since_ms }

// constants, TRANSCRIPT_WRITER_SHUTDOWN_TIMEOUT doc style (state.rs:1112-1128)
pub const PROJECTION_LANE_ATTEMPT_BUDGET: u8 = 3;
pub const PROJECTION_DEFERRED_RETRY_DELAY_MS: u64 = 60_000;

// telemetry (ProjectionSchedulerTelemetry :64-73)
oldest_pending_since_ms: Option<u64>,
failed_attempts: u8,
```

Coverage heads need no new type: a free function
`derive_coverage_heads(&[ProjectionPatch]) -> (Option<ProjectionBasis>, Option<ProjectionBasis>)`
(basis of the max-`sequence` accepted patch per kind) plus
`ProjectionSchedulers::reseed_coverage_heads(..)` writing
`last_completed_basis`. No `Serialize`/`Deserialize`, not a
`SchedulerQueueState` field — decision 6 enforced structurally (Design 2's I7,
kept without the module).

In `state.rs` / `speech/mod.rs`:

```rust
pub projection_job_workers: Arc<Mutex<Vec<(ProjectionKind, String /*job_id*/, JoinHandle<()>)>>>,
pub projection_lane_stopping: Arc<AtomicBool>,
const PROJECTION_JOB_FLUSH_TIMEOUT: Duration = Duration::from_secs(20); // first-cut, documented retune procedure
```

Separate from `retired_session_workers` because that vec fences Start/New
Session until empty — live handles must not fence; **timed-out** handles spill
into it, so the existing fence covers wedged jobs with no new mechanism.
`ProjectionDispatchContext` (`speech/mod.rs:1643`) gains both fields.

## Seam-by-seam plan

1. **Gate (`projections.rs:2512-2514` and `:2449-2461`)** — both go
   three-way in one commit; evidence-resolution comments reworded; currency
   threaded out via `ProjectionRuntimeApplyResult`; `stale_apply=true` warn
   (`speech/mod.rs:2358`) becomes Revised-only by construction (append-only no
   longer errors), new INFO for applied-append-only in the Ok arm using the
   carried currency. Zero scheduler change — the existing
   `Completed` → `complete_in_flight` → `AppendOnlyStale` arm issues the one
   mandatory Background follow-up exactly as it does today.
2. **Stall (`projection_scheduler.rs:354-356`)** — becomes the named budget
   branch: same failed basis and under budget → start retry job (attempt++);
   over budget → `AttemptBudgetExhausted` (emit-only); any basis change
   resets the counter. Purely in-crate, no I/O, no new threads.
3. **Threads (`speech/mod.rs:1991`, `commands.rs:1770-1788`)** —
   `spawn_projection_job` registers `(kind, job_id, handle)`;
   `run_projection_job` self-deregisters on exit; `stop_capture_impl`'s
   existing `spawn_blocking` block sets `projection_lane_stopping`, drains the
   registry via `join_worker_with_timeout(.., PROJECTION_JOB_FLUSH_TIMEOUT, ..)`,
   spilling timeouts to `retired_session_workers`. Graph lane provably drains
   here (decision 4, drain half).
4. **Open (`commands.rs:1494+` `start_capture_impl`)** — one
   `reseed_coverage_heads(derive_coverage_heads(load_projection_patches(sid)?))`
   call before the speech thread spawns. `load_session_impl` stays read-only.
   `restore_from_snapshot` deleted; `save_scheduler_queue_state`
   (`state.rs:961`) keeps writing, documented as diagnostics-only, never read
   as authority; the three `scheduler_queue_*` tests reframed as
   disposable-diagnostics assertions, not deleted.
5. **Retry (decision 3)** — `fail_in_flight`'s `Current` arm, while under
   budget, sets `deferred_retry_at_ms = now + 60s`; `observe_ledger` fires a
   due retry; one one-shot thread `projection-retry-<kind>` (registered in the
   step-3 registry, polling `projection_lane_stopping` every 250ms) provides
   the single clock so the retry fires even with no further finals. Exactly
   one; cancelled by an intervening final revision or stop; no ladder, no
   backoff schedule (the memo's Option B 2s/8s/30s is explicitly NOT built).
6. **Visibility (decision 4, render half)** — `oldest_pending_since_ms` +
   `failed_attempts` on `ProjectionSchedulerTelemetry`; `commands.rs:6323`
   already calls `telemetry_at`, so the wire surface is free; Review renders
   oldest-pending-since (1d92 prototype at `.worktrees/1d92-prototype` is the
   prior art to consult, not to merge).
7. **Proof (decision 5)** — env-gated `#[ignore]` test with
   `synth_two_hour_session(seed)` (~1440 finals @5s cadence, ~240 accepted
   patches across both kinds, evidence fields populated through the real
   constructors so 2cf9-era shapes are exercised); 20 iterations of
   `replay_accepted_patches_with_history` + `derive_coverage_heads`; asserts
   p95 < 2000 ms, prints p50/p95/p99. This is the artifact that would justify
   or foreclose reopening ADR-0029.

## Decision → ticket discharge map

| ADR-0045 decision | Discharged by |
|---|---|
| 1. No considered-bases audit trail | All tickets (nothing builds one; coalescing overwrites remain traceless by design) |
| 2. Duplication over loss; mandatory follow-up | T1 (gate applies + existing scheduler follow-up; retcon per ADR-0024 §4) |
| 3. Exactly one ~60s deferred retry, then event-driven | T5 (bounded by T2's budget) |
| 4. Graph drains at stop; oldest-pending-since visible | T3 (drain) + T6 (render) |
| 5. Replay-on-open p95 < ~2s / 2h, benchmark ships | T4 |
| 6. No durable in-flight state; recovery by re-derivation | T4 (heads + `restore_from_snapshot` deletion + diagnostics demotion) |
| Shared floor: caad fix, telemetry split | T1 |
| Shared floor: JoinHandle registry + stop flush | T3 |
| Shared floor: named 3-attempt budget + typed no-progress signal | T2 |

## Ticket ordering and independence

```
T1 (gate, both sites)  ──→ T4 (heads + demotion + benchmark)
T2 (attempt budget)    ──→ T5 (deferred retry) ──┐
T3 (registry + flush)  ──→ T5                    │
T2 ──→ T6 (telemetry render)                     │
```

T1, T2, T3 are mutually file-disjoint and can land in any order (T1:
`projections.rs`/`state.rs`; T2: `projection_scheduler.rs` only; T3:
`speech/mod.rs`/`state.rs` fields/`commands.rs` stop block). T4 needs T1 so
the benchmark's realistic fixture (containing append-only-accepted patches)
replays cleanly. T5 needs T2's fields and T3's registry + stopping flag. Every
prefix is strictly better than HEAD. Each ticket carries its own gates
(`cargo test` for the touched crate, plus the ticket's named new tests) and is
rebase-squash-merge landable.

The external 2cf9 dependency the constraint sheet §5 flagged is **already
satisfied** — 2cf9 landed at `99530ad`; T1 is written against its landed
signatures and re-litigates nothing.
