# Code Reality: Projection Scheduling in AudioGraph

Scope: `audio-graph-3b48` ("Define Projection Backlog scheduling and lane
reconciliation"). This is a read-only code audit, not a design proposal. All
line numbers are current as of `HEAD` on `master` (`git log -1`: commit
`27e584e`, 2026-08-19 era). File paths are relative to
`/home/codeseys/DevBox/audio-graph`.

## 1. Seed context

- **`audio-graph-3b48`** (this ticket, priority 0, open): asks to define
  durable scheduling semantics for notes/graph projection while Speech Span
  Revisions accumulate — basis selection, coalescing, lane-local coverage
  heads, priority, bounded concurrency, retry, stop-time flush, and
  reconciliation "without erasing provenance." It blocks the parent epic
  `audio-graph-8873` (Deepgram-to-Finalized Session Memory) and two others
  (`fbca`, `44c1`).
- **`audio-graph-464c`** (open, priority 1): "Add durable projection scheduler
  queue store and typed artifact descriptor." Its own extensions explicitly
  say the current persistence is "log-only" and name the exact files it
  intends to touch: `src-tauri/src/projection_scheduler.rs`,
  `src-tauri/src/persistence/mod.rs`, `src-tauri/src/sessions/mod.rs`, and a
  *not-yet-existing* `src-tauri/src/persistence/scheduler_queue.rs`. It is
  marked "ready" but scheduling notes say it hasn't been activated.
- **`audio-graph-9751`** (open, priority 1, blocked by `464c`): "Resume
  projection scheduler after crash without mutating historical Review." Its
  extensions record `class: BLOCKED_DESIGN` — the resume boundary (explicit
  transactional Resume vs. approved startup recovery) has not been decided,
  and explicitly guards that historical `load_session`/Review must stay
  read-only.
- **`audio-graph-ab10`** (in_progress, priority 1, bug): "Correlate projection
  completion by job and session across rotation." Its extensions record that
  job/session-id correlation through completion/failure is now implemented
  and tested, but explicit-cancellation correlation, persisted-recovery
  correlation, and simultaneous Notes/Graph acceptance proof remain open, plus
  an unresolved lock-order finding (ASR path locks ledger-then-schedulers,
  apply path locked schedulers-then-ledger — "permitting a permanent
  deadlock").
- **`audio-graph-caad`** (open, priority 1): "Notes patches discarded as stale
  during continuous speech — only land after a pause
  (DiscardedStaleAndStartedRepair churn)." Manual test: 22 of 23 completed
  Notes jobs during ~3 minutes of continuous speech were
  `DiscardedStaleAndStartedRepair` with `staleness=MissingCurrentSpan`; only 1
  landed, 8s after capture stopped. A later note on the ticket
  (`checkpoint_review_2026_08_13`) already names the mechanism: "Valid
  AppendOnly prefix patches are classified but rejected as StaleBasis; the
  follow-up eventually emits while the useful prefix never becomes visible,"
  citing `src-tauri/src/projections.rs:2192` (pre-refactor line number; the
  logic now lives at the lines cited in §4 below). This audit independently
  re-derived and confirmed that mechanism from current code — see §4.

## 2. Trigger model: what actually starts a projection job

Projection scheduling is driven purely by ASR span-revision events, not by a
timer or a poll loop.

- `record_asr_span_revision_event_and_observe_projection`
  (`src-tauri/src/speech/mod.rs:1877-1894`) is called from the ASR ingestion
  path on every accepted span revision. It first durably appends the
  transcript event, then calls
  `observe_projection_schedulers_for_asr_revision`.
- `observe_projection_schedulers_for_asr_revision`
  (`src-tauri/src/speech/mod.rs:1896-1939`) **gates on finality**: it returns
  immediately unless `payload.is_final || payload.end_of_turn ||
  stability == Final` (line 1902-1907). Partial/provisional ASR revisions are
  durable transcript events but never reach the scheduler.
- It locks the transcript ledger then the schedulers (in that order — see the
  lock-order note in `ab10` above) and calls
  `ProjectionSchedulers::observe_ledger(&ledger, now_ms)`
  (`src-tauri/src/projection_scheduler.rs:656-665`), which calls
  `ProjectionScheduler::observe_ledger` independently for `Notes` and `Graph`
  (`src-tauri/src/projection_scheduler.rs:317-360`).
- The resulting `ProjectionSchedulersObservation` (one decision per kind) is
  dispatched via `dispatch_projection_observation` →
  `dispatch_projection_decision` (`src-tauri/src/speech/mod.rs:1941-1969`),
  which pattern-matches the decision and, for any variant carrying a `job`
  (`StartJob`, `CompletedAndStartedFollowUp`, `DiscardedStaleAndStartedRepair`,
  `FailedAndStartedFollowUp`, `FailedStaleAndStartedRepair`), calls
  `spawn_projection_job`.
- `spawn_projection_job` (`src-tauri/src/speech/mod.rs:1977-2000`) spawns a
  **bare, untracked, unjoined `std::thread`** per job (`thread::Builder::new()
  .name(...).spawn(move || run_projection_job(dispatch, job))`). Nothing
  retains the `JoinHandle` — the runtime cannot wait for, cancel, or observe
  this thread except through the scheduler-state mutations it eventually
  makes via `finish_projection_scheduler_job`.
- `run_projection_job` (`src-tauri/src/speech/mod.rs:2104-2298`) does the LLM
  call (`dispatch.patch_generator.generate_projection_patch`), then — only on
  success — calls `dispatch.projection_runtime.apply_runtime_projection_patch`
  (the apply gate, see §4), then calls `finish_projection_scheduler_job` in
  every terminal branch (success, apply failure, generation failure).
- `finish_projection_scheduler_job` (`src-tauri/src/speech/mod.rs:2377-2424`)
  re-locks the schedulers and calls `complete_notes_in_flight` /
  `complete_graph_in_flight` / `fail_notes_in_flight` / `fail_graph_in_flight`
  against a **freshly re-fetched ledger clone** (line 2382-2388) — i.e. the
  ledger state at *completion* time, which may already be newer than the
  ledger state the LLM call actually saw at generation time. The resulting
  decision is dispatched again, which is how follow-up/repair jobs
  self-perpetuate.

## 3. Queue/backlog reality: there is no backlog

Per `ProjectionKind` (`Notes` and `Graph`), `ProjectionScheduler` holds exactly
two slots (`src-tauri/src/projection_scheduler.rs:151-163`):

```rust
in_flight: Option<ProjectionJob>,
pending_basis: Option<ProjectionBasis>,
```

- **One in-flight job, ever, per kind per session.** `observe_ledger`
  (`projection_scheduler.rs:317-360`) only calls `start_job` (spawning a new
  LLM call) when `self.in_flight` is `None`. While a job is in flight, every
  new final ASR revision instead takes the `Coalesced` branch
  (lines 327-349): it **overwrites** `pending_basis` with the latest full
  ledger basis (`ledger.current_projection_basis()`), it does not append to
  any list. A `SchedulerQueueState` comment (line 605-618) calls this a
  "capacity-one latest-state slot."
- **No priority queue, no multi-job backlog, no FIFO.** `ProjectionPriority`
  (`Realtime` / `Background` / `Replay`, defined
  `src-tauri/src/projections.rs:1131-1137`) is a label attached to a
  `ProjectionJob` at creation time and is never read back anywhere in
  runtime dispatch — confirmed by grep: the only non-test, non-construction
  read of `.priority` is `projection_eval.rs:933`, which just clones it into
  an offline-replay metrics record. Nothing branches execution order, thread
  priority, timeout, or model choice on it. It is descriptive telemetry, not
  a scheduling lane.
- **No cross-session or cross-kind concurrency limiter.** The only bound on
  simultaneous LLM calls is the structural "one in-flight per kind" rule
  above. For the single active session (`AppState` holds one `session_id` —
  `src-tauri/src/state.rs:684`), that caps live concurrency at 2 threads
  (Notes + Graph). There is no `Semaphore`, no rate limiter, no queue-depth
  cap anywhere in the projection path (grep for `Semaphore`/`max_concurrent`/
  `concurrency_limit` under `src-tauri/src` returns nothing relevant to
  projections).
- **Coalescing reason is diagnostic only.** `coalescing_reason`
  (`projection_scheduler.rs:566-578`) classifies *why* a coalesce happened
  (`PendingSpanThreshold` / `InFlightAgeThreshold` / `TtftWindow`) purely for
  telemetry (`ProjectionSchedulerDecision::Coalesced { reason, .. }`); it does
  not change behavior — the pending basis is always replaced wholesale
  regardless of reason.

## 4. THE caad ROOT CAUSE: the apply gate is stricter than the scheduler's own classifier, in direct violation of ADR-0031

This is the central defect this decision must explain and account for.

**Two different functions answer "is this basis still good?", and only one of
them is a three-way classifier.**

- `TranscriptLedger::classify_basis_currency`
  (`src-tauri/src/projections.rs:813-982`) is the three-way classifier ADR-0031
  mandates: `Current`, `AppendOnlyStale(staleness)`, or `Revised(staleness)`
  (`BasisCurrency` enum, `projections.rs:1117-1122`). It proves the exact
  covered-subset hash first (lines 852-914, matching ADR-0031's "hash the
  covered subset before inspecting extra spans" — this is the actual fix for
  the earlier hash-corruption bug the ADR describes), then only if the
  covered subset is untouched and the ledger has *only later appended* spans
  does it return `AppendOnlyStale` (lines 948-981) rather than `Revised`.
  This is the function the **scheduler** uses in `complete_in_flight`
  (`projection_scheduler.rs:378-436`) and `fail_in_flight`
  (`projection_scheduler.rs:459-496`) to decide whether to start a
  `Background` follow-up (append-only case) or a `Replay` repair (revised
  case).
- `TranscriptLedger::validate_basis` /
  `validate_basis_with_speaker_timeline` (`projections.rs:785-807`) is a
  **two-way** wrapper around the same classifier:

  ```rust
  match self.classify_basis_currency(basis, speaker_timeline) {
      BasisCurrency::Current => Ok(()),
      BasisCurrency::AppendOnlyStale(staleness) | BasisCurrency::Revised(staleness) => {
          Err(staleness)
      }
  }
  ```

  (`projections.rs:801-806`). It collapses `AppendOnlyStale` and `Revised`
  into the *same* `Err(staleness)` — there is no way for a caller of
  `validate_basis` to distinguish "your content is a valid unrevised prefix,
  just apply it" from "content you relied on was actually rewritten, discard
  it."

**The live apply gate uses the two-way function, not the three-way one.**
`MaterializedProjectionState::apply_validated_patch_with_speaker_timeline_opt`
— the function every live notes/graph patch must pass through before it is
allowed to mutate materialized state — calls:

```rust
ledger
    .validate_basis_with_speaker_timeline(&patch.basis, speaker_timeline)
    .map_err(|staleness| ProjectionApplyError::StaleBasis { staleness })?;
```

(`src-tauri/src/projections.rs:2301-2303`, inside
`apply_validated_patch_with_speaker_timeline_opt`,
`projections.rs:2295-2322`). This is reached from the real runtime path via
`ProjectionRuntimeHandle::apply_runtime_projection_patch_with_ledger_and_savers`
(`src-tauri/src/state.rs:505-637`), specifically line 566-568:

```rust
let outcome = next_materialized
    .apply_validated_patch(ledger, &patch)
    .map_err(|error| ProjectionRuntimeApplyError::Apply { error })?;
```

`apply_validated_patch` (`projections.rs:2276-2282`) forwards straight into
the two-way `validate_basis_with_speaker_timeline_opt` above.

**Consequence, reproduced step by step:**

1. A Notes job starts against basis B0 (the ledger at job-start time).
2. During LLM generation (which is not instant — TTFT/decode time), new final
   ASR spans keep arriving and append to the ledger. Nothing in B0 was
   revised — they are pure appends.
3. Generation succeeds. `run_projection_job` calls
   `dispatch.projection_runtime.apply_runtime_projection_patch(&job.session_id,
   &job.basis, patch)` (`speech/mod.rs:2206-2210`), which re-fetches the
   **current** ledger snapshot inside `apply_runtime_projection_patch_with_savers`
   (`state.rs:494`, `state.rs:566`) — now strictly newer than B0.
4. The apply gate's `validate_basis_with_speaker_timeline` classifies B0
   against the newer ledger as `AppendOnlyStale(MissingCurrentSpan { .. })`
   (the exact append-only path at `projections.rs:977-981`) — but because
   `validate_basis` maps both `AppendOnlyStale` and `Revised` to the same
   `Err`, the apply call returns
   `Err(ProjectionApplyError::StaleBasis { staleness: MissingCurrentSpan
   { .. } })`. **The patch is never materialized. Nothing is written to
   notes/graph state.**
5. Back in `run_projection_job` (`speech/mod.rs:2239-2260`), this is detected
   as `stale_apply = matches!(&error, ProjectionRuntimeApplyError::Apply {
   error: ProjectionApplyError::StaleBasis { .. } })` and reported to the
   scheduler as `ProjectionJobCompletion::Completed` (not `Failed` — this
   part is deliberate and correct per ADR-0031's "same classifier governs
   successful and failed completions").
6. `finish_projection_scheduler_job` calls `complete_notes_in_flight`, which
   re-runs `ledger.classify_basis_currency(&completed.basis, None)`
   (`projection_scheduler.rs:378`) against an even-newer ledger snapshot. If
   it again lands on `AppendOnlyStale`, the *scheduler* would actually be
   fine with it — but that classification only controls whether to spawn a
   `Background` follow-up job for the newly appended tail; it does **not**
   retroactively apply the already-discarded patch. If by this point the
   classification instead reads as `Revised` (extremely likely under
   continuous speech, since even more spans have landed), the decision is
   `DiscardedStaleAndStartedRepair` — **exactly the telemetry signature caad
   reports**, and `staleness=MissingCurrentSpan` is precisely the payload
   both the append-only and the non-tail-insertion revised branches use
   (`projections.rs:971-981`), so the log line alone cannot even distinguish
   "this was actually a harmless append" from "this was a genuine reorder."

**This directly contradicts the accepted ADR-0031 text**, which states:
"`AppendOnly` output may follow the same semantic apply path and schedules
one coalesced Background follow-up for the appended spans" and "`validate_basis`
delegates to this classifier **or is mechanically proven to produce the same
result**. No independent second interpretation of basis currency is allowed."
(`docs/adr/0031-classify-projection-bases-as-current-append-only-or-revised.md`,
Decision Outcome section). The code satisfies the letter of "delegates to
this classifier" but violates the substance: it delegates and then discards
exactly the distinction the ADR exists to preserve. `git log -S
"apply_validated_patch_with_speaker_timeline_opt" -- src-tauri/src/projections.rs`
shows this function has had exactly one commit since it was introduced
(`8a603e4`) — it has never been revisited to thread the three-way result
through to the apply boundary. The caad ticket's own
`checkpoint_review_2026_08_13` note already named this exact mechanism
("Valid AppendOnly prefix patches are classified but rejected as
StaleBasis"); it is still true today.

Any scheduling redesign under `audio-graph-3b48` must either (a) change the
apply gate to accept `AppendOnlyStale` patches (materializing the append-only
prefix, matching ADR-0031's stated intent) or (b) explicitly re-decide
ADR-0031's outcome. Redesigning coalescing/backlog semantics without touching
this gate will not fix caad — the scheduler-level classifier already agrees
with ADR-0031; the apply gate does not.

## 5. Lane reality: "live vs review vs rebuild" does not exist in code

A repo-wide grep for `lane`/`lanes` as a scheduling concept
(`grep -rn "\blane\b" src-tauri/src`) returns zero hits related to
projection scheduling — only unrelated hits in `analytics/mod.rs` (telemetry
tag naming) and a capture-device preset literally named "Meeting lanes"
(`audio/capture.rs:2290-2291`). The only separations that exist today are:

1. **`ProjectionKind::Notes` vs `ProjectionKind::Graph`**
   (`projections.rs:1124-1129`) — two structurally independent
   `ProjectionScheduler` instances inside `ProjectionSchedulers`
   (`projection_scheduler.rs:627-631`), each with its own in-flight slot,
   pending basis, metrics, and TTFT estimate. This is a *kind* split, not a
   live/review/rebuild split.
2. **`ProjectionPriority::Realtime` / `Background` / `Replay`** — as shown in
   §3, this is a descriptive tag with zero runtime effect on scheduling,
   concurrency, or execution order. `Realtime` is assigned only to the very
   first job for a fresh basis (`observe_ledger`, line 358); `Background` to
   append-only follow-ups; `Replay` to revised-basis repairs. None of these
   get a different thread pool, different timeout, different model, or
   different queue treatment — they all go through the identical
   `spawn_projection_job` → `run_projection_job` path.
3. **Review** (historical, `load_session`/`commands.rs:7032-7121`) is a
   read-only replay path over already-accepted canonical projection events
   (`MaterializedProjectionState::replay_accepted_patches_with_history`,
   `projections.rs:2192-2274`). It is architecturally disjoint from the live
   scheduler — it builds a fresh `TranscriptLedger`/`MaterializedProjectionState`
   from disk and never touches `AppState.projection_schedulers`. It is not a
   "lane" the live scheduler reconciles into; it is a separate code path
   that never runs concurrently with live capture for the same session (a
   session is either the one live `AppState.session_id` or a historical id
   being read via `load_session`).
4. There is no "rebuild" concept distinct from the `Replay` priority
   tag/repair-job mechanism described above, and no code distinguishes a
   "coverage head" per lane — `ProjectionBasis` tracks one linear
   `span_revisions` + `diarization_span_revisions` + `transcript_hash`
   position per *kind*, not per lane.

Any "lane" vocabulary this decision adopts (live/review/rebuild or
otherwise) is being introduced net-new; it has no existing implementation to
preserve compatibility with, only the Notes/Graph kind split and the ADR-0027
Accepted-commit boundary (`docs/adr/0027-file-canonical-durable-session-store.md`)
that already gates durable visibility.

## 6. Crash behavior: there is no resume

- **Persistence write path exists but fires only on graceful session
  rotation**, not after every mutation. The only call site of
  `save_scheduler_queue_state` in the entire codebase is
  `src-tauri/src/state.rs:961`, inside the rotate-session function, right
  before `schedulers.reset(new_session_id)` (line 962). The comment at
  lines 959-960 says the snapshot is saved "so `load_session` can rehydrate
  it later" — but that rehydration **never happens**: `load_scheduler_queue_state`
  and `ProjectionSchedulers::restore_from_snapshot` have **zero call sites
  outside `projection_scheduler.rs`'s own unit tests** (confirmed by grep
  across all of `src-tauri/src`). `audio-graph-9751`'s own extension record
  agrees: "no durable `Accepted` barrier in the runtime yet" and the resume
  boundary is `BLOCKED_DESIGN`.
- **`load_session`** (`commands.rs:7032-7037`, doc comment above it) is
  explicitly documented as read-only Review: "the historical ledger is never
  installed into live AppState" (`commands.rs:7117-7118`). It replays
  accepted canonical projection events into a throwaway
  `MaterializedProjectionState`, validates them, and returns a `LoadedSession`
  DTO — it has no interaction with `ProjectionSchedulers` at all, durable or
  otherwise.
- **Cold start always mints a brand-new random session** —
  `AppState::new()` calls `uuid::Uuid::new_v4()` (`state.rs:684`) and builds
  fresh, empty `TranscriptLedger`, `MaterializedProjectionState`, and
  `ProjectionSchedulers::new(session_id)` (`state.rs:690-693`). There is no
  "was there an interrupted session, should I resume it" check anywhere in
  the startup path. Practically: a crash (panic, kill -9, power loss) mid
  in-flight-projection means (a) the transcript ledger itself is safe —
  `TranscriptEventWriter`/`ProjectionEventWriter` are separate durable JSONL
  appenders unrelated to the scheduler queue — but (b) *whatever the
  scheduler believed was in-flight or pending* for that old session is
  simply gone; nothing on next launch looks for it, and even if something
  did, `restore_from_snapshot`'s own doc comment
  (`projection_scheduler.rs:829-856`) confirms a persisted `in_flight` job is
  never resurrected as running — it can only ever be *demoted* into
  `pending_basis` so a subsequent `observe_ledger` call starts a fresh real
  job. Today nothing calls that demotion path outside tests.
- **Spawned projection threads are untracked.** `spawn_projection_job`
  (`speech/mod.rs:1977-2000`) never stores the `JoinHandle`. `stop_capture_impl`
  (`commands.rs:1706-1800+`) explicitly joins the speech-processor and ASR
  worker threads with a 3-second timeout (lines 1755-1780) but has no
  equivalent step for any projection job thread that might still be running
  an LLM call when capture stops — it is left to finish (or not) fully
  detached from the shutdown sequence. There is no "stop-time flush" of a
  final basis today; whatever `pending_basis` exists when the ASR worker
  stops producing final events simply never gets its own job unless another
  final ASR event happens to arrive first.
- **No automatic retry.** On job failure with an *unchanged* basis,
  `fail_in_flight`'s `BasisCurrency::Current` branch
  (`projection_scheduler.rs:459-465`) just records `FailedCurrent` and clears
  `in_flight` — the scheduler test
  `scheduler_failure_clears_in_flight_and_idles_until_basis_changes`
  (`projection_scheduler.rs:1337-1373`) asserts this explicitly: "unchanged
  failed basis must not retry forever." The only way a failed job's content
  gets attempted again is if new ASR revisions change the basis, which then
  produces `StartJob` on the next `observe_ledger` call — an organic retry
  driven by new speech, not a scheduler-owned retry/backoff policy.

## 7. Hard facts any design must accommodate

1. **The trigger is ASR-finality-gated, event-driven, per-kind, one-shot.**
   Only `is_final`/`end_of_turn`/`Final`-stability span revisions reach the
   scheduler (`speech/mod.rs:1902-1907`); each triggers an independent
   `observe_ledger` call for Notes and Graph.
2. **There is no backlog to schedule — there is a capacity-one coalescing
   slot per kind.** At most one job is in flight and at most one basis is
   pending, per `ProjectionKind`, per session. Any "Projection Backlog"
   design vocabulary must map onto (or explicitly replace) this
   `in_flight: Option<ProjectionJob>` / `pending_basis: Option<ProjectionBasis>`
   pair (`projection_scheduler.rs:158-159`) — there is no list/queue
   structure underneath it today.
3. **The caad defect is a proven, ADR-violating apply-gate bug, not a
   scheduler-policy question.** The scheduler's own three-way classifier
   (`classify_basis_currency`) already implements ADR-0031's
   Current/AppendOnly/Revised distinction correctly. The live apply gate
   (`state.rs:566-568` → `projections.rs:2301-2303`) discards it back down to
   two-way via `validate_basis`, so a generation that arrives after
   legitimate continuous-speech appends is discarded even though nothing it
   covered was revised. Any scheduling redesign must explicitly decide
   whether to fix this gate (materialize `AppendOnlyStale` patches) or
   formally re-open/amend ADR-0031 — silently redesigning coalescing without
   touching this gate leaves caad's symptom unchanged.
4. **"Lanes" (live/review/rebuild) are not an existing code concept.** The
   only real separations are per-`ProjectionKind` (Notes/Graph, structurally
   independent schedulers) and a cosmetic `ProjectionPriority` tag with no
   runtime effect. Historical Review is a fully separate, read-only replay
   code path that never touches the live scheduler. Whatever lane model this
   decision adopts is net-new and has no legacy behavior to stay compatible
   with beyond the Notes/Graph split and the ADR-0027 Accepted-commit
   durability boundary.
5. **There is no crash resume today, and the one persistence write that
   exists is a one-shot, best-effort side effect of graceful rotation, not a
   durability contract.** `save_scheduler_queue_state` fires exactly once,
   only on rotation, and nothing ever reads it back outside tests
   (`state.rs:961`, `projection_scheduler.rs` tests only). Cold start always
   creates a brand-new session id (`state.rs:684`) with no interrupted-work
   detection. Spawned projection-job threads are untracked and unjoined, so
   there is also no guaranteed stop-time flush of the last pending basis.
   Any "durable scheduler queue store" or "resume" decision under this
   ticket is designing net-new behavior, not hardening an existing one.
6. **There is no automatic retry and no explicit concurrency/quota
   control.** A failed job on an unchanged basis is dropped and only
   reattempted if new speech changes the basis (organic, not policy-driven).
   Concurrency is implicitly capped at one in-flight job per kind (2 total
   for the app's single active session) purely by the in-flight-slot
   structure — there is no semaphore, rate limiter, or cost/quota gate
   anywhere in the projection path. The parent map (`audio-graph-8873`)
   itself lists "Operational concurrency, quota, and cost controls" under
   "Not yet specified," consistent with this audit.

## 8. File index for follow-up reading

- `src-tauri/src/projection_scheduler.rs` — scheduler state machine, decision
  enum, snapshot/restore, all unit tests for coalescing/repair/follow-up
  behavior.
- `src-tauri/src/projections.rs` — `TranscriptLedger`, `ProjectionBasis`,
  `classify_basis_currency` (three-way), `validate_basis` (two-way, the apply
  gate's function), `MaterializedProjectionState::apply_validated_patch*`,
  historical replay (`replay_accepted_patches_with_history`).
- `src-tauri/src/speech/mod.rs` — the actual trigger wiring: ASR event →
  `observe_ledger` → decision dispatch → `spawn_projection_job` →
  `run_projection_job` → `finish_projection_scheduler_job`.
- `src-tauri/src/state.rs` — `ProjectionRuntimeHandle`/`AppState`'s
  `apply_runtime_projection_patch*`, session rotation (the one
  `save_scheduler_queue_state` call site), `AppState::new()` cold-start.
- `src-tauri/src/commands.rs` — `load_session` (read-only historical Review
  path), `stop_capture_impl` (worker-thread joins, no projection-thread join).
- `src-tauri/src/persistence/mod.rs:2896` (`save_scheduler_queue_state`) and
  `:2912` (`load_scheduler_queue_state`) — the "log-only" store
  `audio-graph-464c` targets for replacement.
- `src-tauri/src/projection_eval.rs` — offline deterministic replay harness
  that exercises the same apply-gate logic (`complete_kind`) for
  diagnostics/tests; mirrors the live bug's shape (`stale_apply` handling at
  line 464-474).
- `docs/adr/0031-classify-projection-bases-as-current-append-only-or-revised.md` —
  the accepted decision the apply gate currently violates.
- `docs/adr/0024-event-sourced-notes-graph-projections.md`,
  `docs/adr/0025-stt-llm-context-efficiency-and-diff-based-updates.md`,
  `docs/adr/0027-file-canonical-durable-session-store.md` — event-sourcing
  foundation, rolling-summary windowing, and the Accepted-commit durability
  boundary this scheduling decision must not weaken.
