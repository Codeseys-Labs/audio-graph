# Constraint sheet: ADR-0045 projection scheduler implementation

Scope: `audio-graph-3b48`'s accepted architecture (ADR-0045). Read-only
input for whoever plans/implements the epic. Code line numbers verified
against `HEAD` on `master` at commit `3354f9d` (2026-08-20), which is
**newer** than the 3b48 code-reality audit's cited commit `27e584e` —
`audio-graph-3624`/`862c`/`7da4` (route-table + ledger-every-attempt work)
landed in between. All cited line numbers were re-checked against current
`HEAD`, not copied from the audit.

## 1. The six binding maintainer decisions (ADR-0045, accepted 2026-08-20)

These are not open — implementation must match them, not re-derive them.

1. **Provenance depth**: obligation is "no accepted patch is silently
   discarded." Coalescing may overwrite intermediate bases with no durable
   trace of what was *considered* — only the ledger's proof of what was
   *applied* is required. Do not build a considered-bases audit trail.
2. **Duplication over loss**: the caad fix ships accepting transient visible
   duplication in live notes. The reconciling follow-up projection is
   mandatory and must not be best-effort — a duplicate must not outlive the
   next projection tick (retcon is the correction mechanism, ADR-0024 §4).
3. **Retry shape**: exactly one deferred retry (~60s) after a lane failure,
   then purely event-driven (next final revision or stop). No backoff
   loops, no timer-driven retry ladder. This directly contradicts Option
   B's timer-driven backoff schedule (2s/8s/30s) proposed in the memo — that
   option was **not** chosen; do not implement it.
4. **Graph freshness**: graph-lane lag during live capture is unbounded by
   design. The lane must provably drain at stop; Review must render
   oldest-pending-since so the lag is visible to the user (per the 1d92
   prototype findings — worth reading if UI surfacing is in scope).
5. **Reopen latency**: replay-on-open is accepted under an explicit budget —
   **p95 under ~2s for a 2-hour session** — and this budget MUST be proven
   by a replay benchmark on a long-session fixture that **ships with the
   implementation** (not deferred, not "measure later"). ADR-0029 reopens
   for a coverage cache only on a *measured breach* of that budget, so the
   benchmark is also the artifact that would justify (or foreclose)
   reopening ADR-0029.
6. **Durable in-flight state**: `464c`/`9751` are rescoped (confirmed live
   via `sd show`, see §4). Crash resume is recovery-by-re-derivation; the
   cost of a crash is one repeated LLM call. Durable in-flight state returns
   only when a real batch/long-refinement mode exists — do not build it
   speculatively.

Everything else in the ADR ("shared hardening floor") is **mandatory**, not
optional, regardless of which of the memo's two options reads as
"recommended": the caad apply-gate fix, the JoinHandle registry + stop-time
flush, and the named 3-attempt counter handing off to Finalization Blocked.
The maintainer's Option A lane-equals-kind shape (memo's recommendation) is
the one ADR-0045 encodes — no lane vocabulary, no `ProjectionPriority`
promotion to load-bearing, no timer-driven backoff.

## 2. The exact seams (symbol names, current `HEAD`)

### 2a. The caad root cause — apply gate collapses three-way to two-way

- `TranscriptLedger::classify_basis_currency` — `src-tauri/src/projections.rs:813-822+`
  (three-way: `Current` / `AppendOnlyStale(staleness)` / `Revised(staleness)`,
  `BasisCurrency` enum at `projections.rs:1117-1122`). This is already
  correct and is the scheduler's own function (used by
  `ProjectionScheduler::complete_in_flight` at `projection_scheduler.rs:378`
  and `fail_in_flight` at `projection_scheduler.rs:459`).
- `TranscriptLedger::validate_basis_with_speaker_timeline` —
  `projections.rs:796-807` — the two-way collapse:
  ```rust
  match self.classify_basis_currency(basis, speaker_timeline) {
      BasisCurrency::Current => Ok(()),
      BasisCurrency::AppendOnlyStale(staleness) | BasisCurrency::Revised(staleness) => {
          Err(staleness)
      }
  }
  ```
- **The bug, confirmed still present at current HEAD**:
  `MaterializedProjectionState::apply_validated_patch_with_speaker_timeline_opt`
  — `projections.rs:2321-2352` (a few lines later than the audit's cited
  `2295-2322` due to intervening commits, same function, same bug) — calls
  the two-way gate at `projections.rs:2327-2329`:
  ```rust
  ledger
      .validate_basis_with_speaker_timeline(&patch.basis, speaker_timeline)
      .map_err(|staleness| ProjectionApplyError::StaleBasis { staleness })?;
  ```
  This is reached from the live path via
  `ProjectionRuntimeHandle::apply_runtime_projection_patch_with_ledger_and_savers`
  in `state.rs` → `apply_validated_patch` (`projections.rs:2302-2308`) →
  the `_opt` function above. Fixing caad means this call site must branch
  three ways (`Current` applies; `AppendOnlyStale` applies AND the caller
  schedules exactly one coalesced Background follow-up for the appended
  tail; `Revised` still errors to `StaleBasis`/Replay repair) — not just
  swap which function it calls.
- **A test currently encodes the bug as correct behavior and will need to
  change**: `state.rs::runtime_projection_patch_rejects_stale_basis_without_persistence`
  (`state.rs:1769-1809+`) constructs an **append-only** basis (`span-1` then
  seeds `span-2` as a pure append, never touching `span-1`) and asserts the
  patch built against the old `span-1`-only basis is rejected with
  `StaleBasis` and never mutates materialized notes. Under the ADR-0045 fix
  this exact scenario must instead **apply** (with a coalesced follow-up
  scheduled). This test's assertion will fail once the gate is fixed and
  must be rewritten as part of the same change, not left as a regression.

### 2b. The scheduler state machine — capacity-one slot per kind, no backlog

- `ProjectionScheduler` struct — `projection_scheduler.rs:151-163`. Fields:
  `in_flight: Option<ProjectionJob>`, `pending_basis: Option<ProjectionBasis>`,
  `last_completed_basis: Option<ProjectionBasis>`,
  `last_failed_basis: Option<ProjectionBasis>`.
  **`last_completed_basis` is NOT part of `SchedulerQueueState`** (see
  below) — it is in-memory-only and resets to `None` on process restart
  *and* on `ProjectionSchedulers::reset` (session rotation,
  `projection_scheduler.rs:642-654`). There is currently no persisted or
  re-derivable "coverage head" at all; ADR-0045's coverage head must be
  built net-new, re-derived on open from the accepted `projection_patches`
  stream via `load_projection_patches(session_id)` (already exists,
  `persistence/mod.rs:527` trait method, `canonical_reader::load_projection_patches`
  concrete impl referenced at `persistence/mod.rs:851`, `2829`).
- `ProjectionScheduler::observe_ledger` — `projection_scheduler.rs:317-360`.
  The **unowned Idle-forever stall** ADR-0036 flags is exactly
  `projection_scheduler.rs:354-356`:
  ```rust
  if self.last_failed_basis.as_ref() == Some(&basis) {
      return ProjectionSchedulerDecision::Idle;
  }
  ```
  This must become the named 3-attempt-budget branch that hands off to
  Finalization Blocked on exhaustion (see §2d — that handoff type does not
  exist yet).
- `ProjectionSchedulerDecision` — `projection_scheduler.rs:97-149` — full
  enum of every scheduler outcome (`Idle`, `StartJob`, `Coalesced`,
  `CompletedCurrent`, `CompletedAndStartedFollowUp`,
  `DiscardedStaleAndStartedRepair`, `DiscardedStaleNoCurrentBasis`,
  `FailedCurrent`, `FailedAndStartedFollowUp`,
  `FailedStaleAndStartedRepair`, `FailedStaleNoCurrentBasis`,
  `IgnoredSupersededCompletion`). This is the taxonomy `44c1` needs;
  extending it (e.g. an explicit "AttemptsExhausted"/"HandedToFinalizationBlocked"
  variant) is likely required for the 3-attempt handoff.
- `SchedulerQueueState` — `projection_scheduler.rs:619-625`. Fields:
  `notes_pending_basis`, `notes_in_flight`, `graph_pending_basis`,
  `graph_in_flight`. Doc comment (`605-618`) already states in-flight jobs
  are "persisted for diagnostics only" and are never resurrected as running.
  Written once, at `state.rs:961` (`save_scheduler_queue_state`, inside
  `rotate_session`, right before `schedulers.reset(new_session_id)` at
  `state.rs:962`). Read back **only** by
  `ProjectionSchedulers::restore_from_snapshot` (`projection_scheduler.rs:845-856+`),
  which has **zero call sites outside its own unit tests** — confirmed by
  grep against current `HEAD`. ADR-0045 demotes this store's *authority*
  entirely (per rescoped `464c`, §4); whatever survives is at most a
  disposable diagnostics hint.

### 2c. The dispatch/thread wiring — untracked, unjoined projection threads

- Trigger chain (unchanged from the audit, current line numbers):
  `record_asr_span_revision_event_and_observe_projection`
  (`speech/mod.rs:1891-1908`) → `observe_projection_schedulers_for_asr_revision`
  (`speech/mod.rs:1910-1953`, finality-gate at `1916-1921`) →
  `ProjectionSchedulers::observe_ledger` (`projection_scheduler.rs:656-665`)
  → `dispatch_projection_observation` / `dispatch_projection_decision`
  (`speech/mod.rs:1955-1983`) → `spawn_projection_job` (`speech/mod.rs:1991-2014`)
  → `run_projection_job` (`speech/mod.rs:2215-...`) →
  `finish_projection_scheduler_job` (`speech/mod.rs:2490-...`).
- `spawn_projection_job` (`speech/mod.rs:1991-2014`) spawns a bare
  `std::thread::Builder::new().name(...).spawn(...)` and **discards the
  `JoinHandle`** — confirmed unchanged at current HEAD. Nothing retains it.
- **The reusable pattern already exists for other worker categories and is
  the shape a projection JoinHandle registry should follow**:
  `AppState.retired_session_workers: Arc<Mutex<Vec<std::thread::JoinHandle<()>>>>`
  (`state.rs:286`, initialized `state.rs:780`) plus the helper
  `join_worker_with_timeout` (`commands.rs:892-...`), used for the speech
  processor and ASR worker threads inside `stop_capture_impl`
  (`commands.rs:1706`, joins at `commands.rs:1770-1788` with a **3-second**
  timeout) and for Gemini/converse/OpenAI worker threads elsewhere in
  `commands.rs`. Projection job threads are conspicuously **absent** from
  every `retired_session_workers`/`join_worker_with_timeout` call site
  (confirmed by grep) — `stop_capture_impl` has no equivalent step for a
  still-running projection LLM call. ADR-0045's "JoinHandle registry with
  stop-time flush" is net-new work that should reuse this exact
  registry+timeout-join pattern (or extend `retired_session_workers`
  itself) rather than invent a different shape. The named budget constant
  should follow the style of `TRANSCRIPT_WRITER_SHUTDOWN_TIMEOUT`
  (`state.rs:1112-1128`, currently 5s, explicitly documented as a
  first-cut-empirical value with a p99-retuning procedure) — this is the
  exact "p99-tuned constant" the memo points to.
- `finish_projection_scheduler_job` (`speech/mod.rs:2490-...`) re-locks the
  schedulers and re-fetches a **fresh** ledger clone before calling
  `complete_notes_in_flight`/`fail_notes_in_flight` etc. — this is how
  follow-up/repair jobs self-perpetuate; unchanged from the audit.

### 2d. The Finalization Blocked handoff target — does not exist in code yet

- Grepped across all of `src-tauri/src` for `FinalizationBlocked` / an
  enum/struct implementing ADR-0035/0036's "Finalization Blocked": **zero
  hits that are a real type**. The only reference in code is a comment,
  `llm/executor.rs:846-850`:
  > "ADR-0035's `Finalization Blocked` is per-Session post-stop and does not
  > exist in this runtime, and the stalled-lane behaviour is
  > `audio-graph-3b48`'s. This function ships the classification and the
  > no-larger-budget guarantee; it does not invent a resting state."
- So the epic implementing ADR-0045 is **not** just wiring into an existing
  `Finalization Blocked` state — that state itself has no runtime
  representation yet. The named 3-attempt counter's "hand off to
  Finalization Blocked" therefore either (a) needs ADR-0035/0036's own
  implementation to land first or alongside, or (b) needs this epic to
  define the minimal typed "no-progress" signal now and let ADR-0035/0036's
  eventual implementation consume it — the ADR text says "emits a typed
  no-progress signal and hands off," which reads as (b): 3b48/0045 owns
  emitting the signal, not building the Blocked state itself. Whoever plans
  the epic must decide this boundary explicitly (it is exactly the
  ADR-0036 "contested reconciliation D6/C11" ownership line, constraints.md
  MUST #20).
- `audio-graph-21e9` (referenced, not re-read in this pass) owns the
  per-attempt deadline; ADR-0036 itself owns per-lane totals and the
  `Finalized` verdict. 3b48/0045 owns the retry-progression / attempt-budget
  counter in between. Do not let this epic redefine either neighbor's
  territory (MUST #20 boundary statement requirement).

### 2e. Coverage/basis state — where it lives today vs. where ADR-0045 needs it

- **Per-accepted-patch basis** (the thing coverage heads must be re-derived
  from): `ProjectionPatch.basis: ProjectionBasis` on every accepted patch,
  readable via `load_projection_patches(session_id)`
  (`persistence/mod.rs:527` trait signature; concrete readers at
  `persistence/mod.rs:1319`, `canonical_reader::load_projection_patches`
  wired at `2829`). This already exists and is durable (ADR-0027 Accepted
  boundary) — it is the correct re-derivation source.
- **Per-materialized-item basis**: `MaterializedNote.basis`,
  `MaterializedGraphNode.basis`, `MaterializedGraphEdge.basis` all carry
  `ProjectionBasis` (`projections.rs` structs, unchanged shape on master).
- **In-memory scheduler coverage approximation**: `last_completed_basis`
  (`projection_scheduler.rs:161`) — NOT persisted, NOT re-derived on open,
  wiped on rotation/restart. This is the field that must be replaced by (or
  seeded from) a real re-derivation call on open, per decision 6 /
  ADR-0045's "coverage heads re-seeded on open from the last accepted
  `projection_patches` record for that kind."
- **Cold start has zero resume-detection logic**: `AppState::new()` mints a
  fresh `uuid::Uuid::new_v4()` session id unconditionally (`state.rs:684`)
  and builds empty ledger/materialized-state/schedulers
  (`state.rs:690-693` region). `load_session` (`commands.rs:7021`) is a
  fully separate read-only historical-replay path that never touches
  `AppState.projection_schedulers`. Building the "reconciliation on open"
  code path (shared-floor item 3) is genuinely new wiring, not a flag flip.

## 3. What exists vs. what the epic must create

**Exists and correct, reuse as-is:**
- `classify_basis_currency` three-way classifier (`projections.rs:813`).
- `ProjectionScheduler`/`ProjectionSchedulers` capacity-one-per-kind
  structure and all coalescing logic (`projection_scheduler.rs:317-360`).
- `ProjectionSchedulerDecision` taxonomy (extend, don't replace).
- `load_projection_patches` accepted-log reader (`persistence/mod.rs`).
- `retired_session_workers` + `join_worker_with_timeout` generic
  join-with-timeout pattern (`state.rs:286`, `commands.rs:892`) — reusable
  shape for the JoinHandle registry, not a drop-in (projection jobs need
  per-kind/per-job identity checks the generic Vec doesn't provide).
- `replay_accepted_patches_with_history` / historical replay path
  (`projections.rs:2218+`) as the reconciliation-on-open replay mechanism.
- `TRANSCRIPT_WRITER_SHUTDOWN_TIMEOUT` naming convention (`state.rs:1112`)
  for the stop-time flush budget constant.

**Exists but is the bug to fix (not a seam to preserve):**
- `apply_validated_patch_with_speaker_timeline_opt`'s two-way gate call
  (`projections.rs:2327-2329`) — must become three-way-aware.
- `runtime_projection_patch_rejects_stale_basis_without_persistence`
  (`state.rs:1769`) — currently pins the bug; must be rewritten to assert
  the new AppendOnlyStale-applies behavior.

**Does not exist yet, net-new for this epic:**
- Coverage-head re-derivation on open (per-kind, from accepted
  `projection_patches`), and the `observe_ledger` call that follows it
  (shared-floor item 3).
- The JoinHandle registry for projection job threads + the stop-time flush
  step in `stop_capture_impl` (no equivalent of the speech/ASR worker join
  exists today for projections).
- The named 3-attempt budget replacing the Idle-forever branch at
  `projection_scheduler.rs:354-356`, plus whatever typed "no-progress"
  signal it emits toward Finalization Blocked.
- The coalesced-follow-up scheduling call at the fixed apply gate (the
  caad fix's second half — classifying `AppendOnlyStale` as applicable is
  necessary but the mandatory "exactly one coalesced Background follow-up"
  dispatch must be wired at the same call site).
- Telemetry split of the currently-indistinguishable `MissingCurrentSpan`
  log line into applied-append-only vs. discarded-revised (shared-floor
  item 1, explicit maintainer requirement).
- Any Review-surfacing of "oldest-pending-since" for graph-lane lag
  visibility (decision 4) — no such UI/data surfacing exists today; the
  1d92 prototype worktree (`.worktrees/1d92-prototype`) may have relevant
  prior art, unexamined in this pass.
- The p95 ~2s/2h replay benchmark itself (decision 5) — no such benchmark
  exists in the repo today (not found in this pass's search); it must ship
  as part of the implementation, not as a follow-up.
- `464c`'s narrowed typed artifact descriptor (+ optional disposable
  diagnostics snapshot) — the seed is rescoped but not yet built to that
  shape; `src-tauri/src/persistence/scheduler_queue.rs` does not exist yet
  (confirmed: no such file on disk).

## 4. Rescoped seeds (`sd show`, live 2026-08-20)

- **`audio-graph-464c`** — "Typed projection artifact descriptor (rescoped
  from durable scheduler queue by ADR-0045)." Description confirms: the
  durable scheduler-queue store this seed originally specified "has no
  authoritative consumer" under ADR-0045; rescoped to the typed artifact
  descriptor for projection outputs plus at most a **disposable** (never
  read as authority, safe to delete) diagnostics snapshot. Durable
  in-flight state returns only if a real batch/long-refinement mode ships.
  Still blocks `audio-graph-9751` and `audio-graph-617e`. Its extensions
  still carry stale pre-ADR-0045 wave-planning notes (module path
  `persistence/scheduler_queue.rs`, "capacity-one latest-state slot"
  framing) that predate the rescope description at the top — read the
  top-level `description` field as authoritative, not the older
  `extensions` blobs.
- **`audio-graph-9751`** — "Recover the projection scheduler on open by
  re-derivation (reframed by ADR-0045)." Description confirms ADR-0045
  answers the resume-vs-recovery question as startup recovery by
  re-derivation: on open, per-kind coverage heads re-derive from the last
  accepted `projection_patches` record and materialized state rebuilds from
  the accepted patch log; crash cost is one repeated LLM call. Carries the
  replay-on-open p95 ~2s/2h benchmark requirement explicitly in its own
  description now (duplicated from the ADR). Still `blockedBy: [464c]`.
  Historical `load_session`/Review must stay read-only — invariant survives
  from the original seed.
- **`audio-graph-caad`** — description now states the "DECIDED SHAPE
  (ADR-0045, grilling 2026-08-20)" verbatim: match `classify_basis_currency`
  three ways so `AppendOnlyStale` applies with **one** coalesced follow-up
  as the lane's mandatory next item; transient visible duplication accepted
  as product behavior; the follow-up must not be able to outlive the next
  projection tick. Ships **inside** the ADR-0045 scheduler implementation
  (not a separate seed to close independently). Its `extensions` retain the
  July independent-review findings (the stale-basis test that will now
  need rewriting — see §2a — and the hash-before-append-check ordering,
  which `classify_basis_currency` already implements correctly per the
  code-reality audit's §4).

## 5. The 2cf9 merge note (claim-class validator, in flight)

Worktree `.worktrees/2cf9-claim-class` exists and has a real diff against
`master` (branch not yet merged): 12 files, +2508/-162, dominated by a new
`src-tauri/src/claim_evidence.rs` (904 lines) and large diffs to
`projections.rs` (+738 lines, mostly test-fixture churn from a new
constructor arg) and `projection_llm.rs` (+767 lines, not read in this
pass).

**Symbols 2cf9 adds:**
- New module `crate::claim_evidence` (added to `lib.rs`) with public API:
  `ClaimClass` (enum), `EvidenceAnchor` (struct, `Default` impl lands on
  the always-refused class), `ClaimEvidenceRequirement`,
  `evidence_requirement(ClaimClass) -> ClaimEvidenceRequirement`,
  `ResolvedSpanEvidence`, `ClaimEvidenceDeficiency` (+ `Display`),
  `AdmittedClaimEvidence` (+ methods `claim_class()`, `span()`),
  `ClaimAdmission` enum (`Admitted`/`Refused`), and
  `judge_claim_evidence(&EvidenceAnchor, &BasisMap) -> ClaimAdmission`.
- New `ProjectionOperation` variants/fields in `projections.rs`:
  `evidence: EvidenceAnchor` added to `UpsertNote`, `UpsertGraphNode`,
  `UpsertGraphEdge` (all `#[serde(default)]`); new variant
  `InvalidateNote { id }` (parity with existing `InvalidateGraphNode`/
  `InvalidateGraphEdge`; currently materializes identically to
  `DeleteNote`, and is refused for LLM-submitted drafts via
  `projection_llm::validate_operation` → `ProjectionPatchDraftError::DerivedOnlyOperation`).
- New field `evidence: Option<AdmittedClaimEvidence>` on
  `MaterializedNote`, `MaterializedGraphNode`, `MaterializedGraphEdge`.
- **New parameter on the apply surface**: `MaterializedNotes::apply_patch`
  and `MaterializedGraph::apply_patch` both gain a second parameter
  `evidence_basis: Option<&BTreeMap<&str, &TranscriptEvent>>`.
  `MaterializedProjectionState::apply_replayed_patch` gains the same
  parameter and threads it through. **`apply_validated_patch_with_speaker_timeline_opt`
  is modified** (in the worktree) to compute `evidence_basis` via a new
  `pub(crate) fn resolve_claim_evidence_basis_events(&ProjectionBasis, &TranscriptLedger, Option<&SpeakerTimeline>) -> Vec<TranscriptEvent>`
  and pass `Some(&evidence_basis)` through — **but this happens strictly
  AFTER the existing `validate_basis_with_speaker_timeline(...)?` gate
  call**, which 2cf9 does not touch or change (grep-confirmed: no diff
  hunk touches `validate_basis_with_speaker_timeline` or
  `classify_basis_currency` themselves). Also new:
  `pub(crate) fn transcript_event_content_hash(&TranscriptEvent) -> String`
  and a private `resolve_admitted_claim_evidence(&EvidenceAnchor, Option<&BTreeMap<...>>) -> Option<AdmittedClaimEvidence>`.
- `replay_accepted_patches_with_history` in 2cf9 now reconstructs a
  per-patch ledger/speaker-timeline snapshot and resolves evidence against
  it (previously hardcoded `None`), fixing what 2cf9 calls a real bug in
  replay/live divergence for evidence fields specifically.
- Also touches `commands.rs` (+145), `llm/executor.rs` (+80),
  `persistence/mod.rs` (+18), `persistence/canonical_reader.rs` (+1),
  `projection_eval.rs` (+3), `state.rs` (+2), `speech/mod.rs` (+2, minimal),
  `timeline.rs` (+9) — not read line-by-line in this pass; flagged for the
  epic's own review, not re-derived here.

**Why this is the merge point, concretely:** ADR-0045's caad fix and 2cf9
both need to touch the **same function**,
`apply_validated_patch_with_speaker_timeline_opt` (`projections.rs:2321`
on master today). 2cf9's version adds evidence-basis resolution *after*
the (still two-way, still buggy) gate call; ADR-0045's fix needs to change
the gate call itself to branch three ways and, on `AppendOnlyStale`,
schedule a coalesced follow-up. These are two different edits to the same
few lines, plus a public-signature change 2cf9 has already made
(`MaterializedNotes::apply_patch`/`MaterializedGraph::apply_patch` gaining
`evidence_basis`) that any ADR-0045 refactor of the apply path must
preserve or consciously rebase. **Whoever plans the ADR-0045 epic should
either (a) sequence after 2cf9 lands and rebase the caad fix on top of its
new `apply_patch`/`apply_replayed_patch` signatures, or (b) coordinate
explicitly with the 2cf9 owner on the exact diff shape of
`apply_validated_patch_with_speaker_timeline_opt`** — landing both
independently against master risks a real (not cosmetic) merge conflict
in the one function both changes must edit, plus every call site of
`apply_patch`/`apply_replayed_patch` 2cf9 already touched (its diff shows
~40+ test call sites alone updated for the new arity). 2cf9 does not touch
`projection_scheduler.rs` at all (confirmed: not in its changed-file list),
so the scheduler-side work (coverage heads, JoinHandle registry, 3-attempt
counter) has no overlap and can proceed independently of 2cf9's timeline.

## 6. Test surface available today

**`projection_scheduler.rs`** (15 unit tests, all scheduler-state-machine
level, no I/O): coalescing → one background follow-up
(`scheduler_coalesces_append_only_completion_into_one_background_follow_up`),
revised-completion → replay repair, appended-partial does not follow-up
(both completion and failure paths), superseded job/session ignoring,
reset-never-reuses-job-identity, TTFT estimate updates, coalescing-pressure
classification, current-completion idles, failure-clears-and-idles
(**this is the test that currently asserts the exact "unchanged failed
basis must not retry forever" behavior the 3-attempt counter will need to
generalize** — `scheduler_failure_clears_in_flight_and_idles_until_basis_changes`,
`projection_scheduler.rs:1337`), append-only-failure → background not
repair, revised-failure → repair, three `scheduler_queue_*` tests for
snapshot/restore/disk-roundtrip (these test the store ADR-0045 is
demoting to non-authoritative — expect them to need reframing as
diagnostics-only assertions, not deletion, since the store itself
survives narrowed).

**`projections.rs`** (relevant subset): `classify_basis_currency_distinguishes_current_append_only_and_revised`,
`same_span_revisions_with_wrong_hash_are_revised_and_match_validation`,
`append_only_uses_audio_chronology_not_span_id_sort_order`,
`transcript_ledger_rejects_stale_and_conflicting_revisions`,
`materialized_notes_reject_stale_or_wrong_kind_patches`,
`materialized_graph_rejects_stale_wrong_kind_note_ops_and_dangling_edges`,
`materialized_projection_state_replays_accepted_patches_without_final_ledger_staleness`,
`materialized_projection_history_rejects_stale_note_and_replays_retcon_repair`,
`materialized_projection_state_rejects_stale_basis_before_mutation`. None
of these currently exercise the "AppendOnlyStale applies" path end-to-end
— that is new required coverage under caad's `required_tests` list
(runtime append-only patch applies+enqueues+saves; runtime revised basis
still rejects without persistence; accepted append-only patch reload/
replay reproduces materialized state).

**`speech/mod.rs`** (8 relevant tests):
`runtime_projection_dispatch_applies_fake_notes_and_graph_patches`,
`runtime_projection_dispatch_discards_same_session_replaced_worker`,
`runtime_projection_dispatch_ledgers_remote_llm_flow_and_gates_local_only`,
`runtime_projection_dispatch_ledgers_served_route_not_configured_intent`,
`runtime_projection_dispatch_clears_scheduler_on_generation_failure`,
`runtime_projection_dispatch_follows_up_append_only_apply_with_current_basis`
(`speech/mod.rs:10122` — **this test already exercises an append-only
mid-generation mutation scenario and its generator/dispatch scaffolding
(`FnProjectionPatchGenerator`, `projection_dispatch_for_app`) is the most
direct reusable harness for the caad fix's new tests** — read this test's
full body before writing new ones, it is >100 lines of working fixture
setup), `runtime_projection_dispatch_ignores_partials_even_with_generator`,
`runtime_projection_scheduler_observes_finals_without_partial_job_churn`.

**`state.rs`** (7 relevant tests):
`runtime_projection_patch_persists_notes_and_projection_event`,
`runtime_projection_patch_can_enqueue_event_through_repository_writer`,
`canonical_projection_sequence_advances_when_snapshot_cache_save_fails`,
`runtime_projection_patch_queue_full_does_not_save_materialized_state`,
`runtime_projection_patch_persists_materialized_graph`,
`runtime_projection_patch_rejects_stale_basis_without_persistence`
(`state.rs:1769` — **the test that must change**, see §2a/§3),
`rotate_session_resets_projection_schedulers`.

**Not present anywhere in the repo (confirmed by search this pass):** a
replay-on-open latency benchmark of any kind; a long-session (2-hour-scale)
fixture; any test naming "Finalization Blocked," "coverage head," or a
3-attempt budget constant. All net-new for this epic.
