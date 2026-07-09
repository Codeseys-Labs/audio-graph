# audio-graph-caad — Notes apply-policy design

**Seed:** audio-graph-caad (P1) — Notes patches discarded as stale during continuous speech; they only land after a pause (`DiscardedStaleAndStartedRepair` churn), leaving the During view empty until the speaker stops.

**Status:** design decision (read-only against master). Implementation is a follow-up PR.

---

## 1. The mechanism, end to end

An ASR span revision (`is_final` / `end_of_turn`) drives
`observe_projection_schedulers_for_asr_revision`
(`src-tauri/src/speech/mod.rs:1728`) →
`ProjectionSchedulers::observe_ledger`. With nothing in flight the scheduler
starts a job whose **basis** is the current ledger basis — a
`ProjectionBasis` capturing `(span_id, revision_number)` for every latest span,
a `transcript_hash` over all of them, and a `summarized_through_revision`
window boundary (`projections.rs:203`). While the job runs, newer spans
**coalesce** into `pending_basis` (no new job) —
`ProjectionScheduler::observe_ledger` (`projection_scheduler.rs:267`).

The job runs on a thread in `run_projection_job` (`speech/mod.rs:1934`):
generate the patch via the LLM (~10-20s), then apply it. There are **two
staleness gates**, both calling the same `TranscriptLedger::validate_basis`
(`projections.rs:700`) against the *current* ledger:

- **Gate 1 (apply-time):** `apply_runtime_projection_patch_with_savers`
  (`state.rs:454`) snapshots the current ledger and calls
  `apply_validated_patch` → `apply_validated_patch_with_speaker_timeline_opt`
  (`projections.rs:2046`), which `validate_basis(...)?` **before** mutating
  materialized state. If the ledger moved, it returns
  `Err(StaleBasis { MissingCurrentSpan .. })`; the patch is **never applied and
  never emitted** to the During view (`speech/mod.rs:2027`, `stale_apply=true`
  → completion reclassified as `Completed`).
- **Gate 2 (scheduler-complete):** `finish_projection_scheduler_job`
  (`speech/mod.rs:2145`) → `complete_in_flight` (`projection_scheduler.rs:312`).
  Its `Err(staleness)` arm bumps `stale_discards`, starts a **Replay-priority
  repair** job for the current basis, and returns
  `DiscardedStaleAndStartedRepair`.

`validate_basis` (`projections.rs:739`) iterates the *current* spans and
returns on the **first** mismatch: a span present in the ledger but absent from
the basis → `MissingCurrentSpan` (a pure **append**); a basis span at an older
revision → `StaleSpanRevision` (a **revise**); plus `UnknownBasisSpan`,
`TranscriptHashMismatch`, `SummaryWindowMismatch`.

**Round-3 symptom, confirmed in the log** (session `a26e85c0`,
`audio-graph-20260707-230626.log`): during ~3 min of continuous speech, spans
append every ~2s but generation takes ~10-20s, so every completion's basis is
missing spans that landed mid-generation. **68 consecutive Notes
`DiscardedStaleAndStartedRepair` decisions, every one `MissingCurrentSpan`** —
zero `StaleSpanRevision`. The single `CompletedCurrent` arrived 8s *after*
capture stopped, when the ledger finally stopped moving. Generation is no
longer the bottleneck (the a324 schema fix eliminated notes-kind generation
failures); the **apply policy** is: it treats "the transcript grew" identically
to "the transcript was corrected," and throws away sound work.

---

## 2. The three fix directions, grounded in the code

### (a) Progressive apply of stale patches, repair overwrites later

Change Gate 1 (`apply_validated_patch_with_speaker_timeline_opt`) to apply a
stale patch anyway (mark it partial), emit it to the During view, and let the
Gate-2 repair overwrite it when the fuller basis completes.

- **What changes:** the `validate_basis(...)?` early-return in
  `projections.rs:2052`, plus a "partial" flag on `MaterializedNote`
  (`projections.rs:1181`) and the `MATERIALIZED_NOTES_UPDATE` payload.
- **Invariants risked:** applies patches built on **revised** (corrected)
  transcript too, since it doesn't discriminate — reintroducing exactly the bug
  ADR-0024 §2 exists to prevent ("reject stale LLM output by construction").
  Introduces a **novel** partial→repair-overwrite state machine: duplicate-note
  risk (the LLM chooses `UpsertNote` ids; "keep stable ids" is a soft prompt
  instruction, not enforced — `projection_llm.rs:509`), visible flicker, and new
  metric semantics. Does **not** reduce churn — still one repair per stale
  completion.
- **Round-3 behavior:** During view populates progressively (the win), but every
  completion is unconditionally accepted including any revise-stale ones, and the
  repair path fires 68 times on top of 68 applies.

### (b) Pinned basis window accepted as-of job start

Drop the current-ledger re-validation at both gates; accept every patch for the
basis it was pinned to at job start (the `patch.basis == expected_basis` check
in `state.rs:492` already guarantees the pin).

- **What changes:** remove `validate_basis` from Gate 1
  (`projections.rs:2052`) and delete the `Err(staleness)` discard arm of
  `complete_in_flight` (`projection_scheduler.rs:343`).
- **Invariants risked:** strictly the most dangerous. A patch pinned at T1 is
  applied even when a span it covered was **revised** by T2 (partial→final text
  correction) — this shows the user notes built on transcript that has since been
  corrected. Single-in-flight ordering means no concurrent overwrite, but the
  correctness loss is the whole ADR-0024 guarantee, unconditionally.
- **Round-3 behavior:** During view populates, but every partial→final ASR
  correction can surface a wrong note until the next pass. Only safe if combined
  with append-vs-revise discrimination — which *is* option (c).

### (c) Append-vs-revise discrimination in the discard predicate

Only discard when the patch's own spans were **revised/dropped** (or the summary
refolded existing turns); when the ledger merely **appended** spans the patch
never saw, the patch is sound for the spans it covered — accept it and start a
follow-up for the appended spans, reusing the scheduler's existing
`CompletedAndStartedFollowUp` path.

- **What changes:** a pure set-comparison classifier on `TranscriptLedger`
  (append-only iff the basis's `(span_id, revision)` pairs are a **subset** of
  the current ledger's — every covered span still present at the exact revision
  the note saw; appends only add pairs); Gate 1 applies when
  `Current | AppendOnlyStale`, discards only on `Revised`; Gate 2's completion
  match becomes three-way, routing `AppendOnlyStale` to
  `CompletedAndStartedFollowUp` instead of `DiscardedStaleAndStartedRepair`.
- **Invariants risked:** cannot key off the single variant `validate_basis`
  returns (it short-circuits on the first mismatch) — needs the dedicated subset
  check (pure, deterministic, unit-testable). Duplicate-id/flicker risk is the
  **same surface the happy-path `CompletedAndStartedFollowUp` already carries
  today** — no new state machine. Revise-stale patches are **still**
  discarded+repaired, so ADR-0024's real guarantee (never show notes built on
  superseded/corrected transcript) is preserved. Requires an ADR-0025 addendum:
  a grown summary window over **unrevised** turns no longer forces
  `SummaryWindowMismatch`, justified because content generated from the verbatim
  view of unchanged spans stays valid and the follow-up regenerates against the
  fuller window regardless.
- **Round-3 behavior:** all 68 append-only completions become "apply + follow-up"
  — the During view accumulates notes every generation instead of after the
  pause — while `stale_discards` drops to ~0, giving a clean telemetry proof.

---

## 3. Decision

**Adopt option (c): discriminate append-only staleness from revise staleness in
the completion predicate.** The round-3 log proves the entire symptom is
append-only staleness (68/68 `MissingCurrentSpan`, zero revise), so the smallest
fix that addresses the user-visible bug is to stop treating "the transcript
grew" as grounds for discard. A new subset classifier on `TranscriptLedger`
(basis span-revisions ⊆ current ledger span-revisions) marks such completions
`AppendOnlyStale`; the apply gate then applies them (populating the During view
progressively) and the scheduler routes them through the **existing**
`CompletedAndStartedFollowUp` path to cover the appended spans — no new
partial/repair-overwrite state machine and no new duplicate-note surface beyond
what the happy path already carries. Crucially, patches whose own spans were
**revised** (partial→final corrections) are still discarded and repaired, so
ADR-0024's staleness guarantee is preserved exactly where it matters. This
rejects (b) (accepts revise-stale patches unconditionally — the ADR-0024 bug)
and rejects standalone (a) (same over-broad correctness loss as (b) plus a novel
overwrite machine); (c) subsumes the useful half of (a) — progressive apply —
without its cost.

---

## 4. Implementation sketch

1. **`src-tauri/src/projections.rs` — new classifier.** Add
   ```rust
   pub enum BasisCurrency { Current, AppendOnlyStale, Revised(ProjectionBasisStaleness) }
   ```
   and `TranscriptLedger::classify_basis_currency(&self, basis, speaker_timeline) -> BasisCurrency`.
   Reuse the diarization/timeline checks from
   `validate_basis_with_speaker_timeline` (`projections.rs:711`), then: build
   `current_spans` / `basis_spans` maps as today; if every basis span exists in
   current at the same revision **and** no basis span is dropped, classify
   `AppendOnlyStale` when current has extra spans (or a differing summary window
   over unrevised turns), else `Current`; any revised/dropped span → `Revised`
   carrying the corresponding `ProjectionBasisStaleness` (so existing repair
   telemetry is unchanged). Keep `validate_basis` as a thin wrapper
   (`Current => Ok, _ => Err`) so promotion.rs and the replay path are untouched.

2. **`src-tauri/src/projections.rs` — Gate 1.** In
   `apply_validated_patch_with_speaker_timeline_opt` (`projections.rs:2046`),
   replace the `validate_basis(...)?` early-return with a
   `classify_basis_currency` match: apply on `Current | AppendOnlyStale`, return
   `Err(StaleBasis { staleness })` on `Revised`. The `apply_patch` sequence guard
   (`projections.rs:1247`) still enforces monotonic overwrite.

3. **`src-tauri/src/projection_scheduler.rs` — Gate 2.** In `complete_in_flight`
   (`projection_scheduler.rs:312`) switch the two-arm `validate_basis` match to a
   three-way `classify_basis_currency`: `Current` → today's Ok arm
   (`CompletedCurrent` / `CompletedAndStartedFollowUp`); `AppendOnlyStale` →
   count `completed_jobs`, set `last_completed_basis`, start a Background
   follow-up for `current_basis` → `CompletedAndStartedFollowUp` (do **not** bump
   `stale_discards` or start a Replay repair); `Revised` → today's discard+repair
   arm. Apply the same classifier in `fail_in_flight`
   (`projection_scheduler.rs:366`) for symmetry (a failed generation has no
   patch, so append-only there simply advances to a follow-up instead of a
   repair). The `run_projection_job` `stale_apply` branch (`speech/mod.rs:2034`)
   already maps a Gate-1 `StaleBasis` to `Completed`; leave it — Gate 2 now makes
   the accept/discard call.

4. **ADR update.** Add an addendum to
   `docs/adr/0025-stt-llm-context-efficiency-and-diff-based-updates.md` (and a
   note in ADR-0024 §2) recording that append-only staleness is applied
   progressively to the live During view and that a summary window grown over
   unrevised turns no longer triggers `SummaryWindowMismatch` for the live apply.

5. **Metrics/telemetry.** No schema change required; `stale_discards` naturally
   falls to ~0 for pure-append sessions and `follow_up_jobs_started` rises,
   giving the regression signal. Optionally add a `progressive_applies` counter
   to `ProjectionSchedulerMetrics` (`projection_scheduler.rs:39`) if a dedicated
   metric is wanted.

**Race note:** Gate 1 (ledger snapshot in `state.rs:473`) and Gate 2 (ledger
snapshot in `speech/mod.rs:2150`) are separate snapshots. If a *revision* lands
between them, Gate 1 may apply (append-only) while Gate 2 classifies `Revised`
and starts a repair — self-healing: the repair overwrites the just-applied
partial. Worth a comment, not a blocker.

---

## 5. Test plan (no live LLM)

All three layers are testable with hand-built ledgers and the existing
stub-generator harness — no provider I/O.

1. **Classifier unit tests (`projections.rs`)** —
   `classify_basis_currency_distinguishes_append_from_revise`: exact match →
   `Current`; ledger with one extra appended span → `AppendOnlyStale`; a covered
   span bumped to a higher revision → `Revised(StaleSpanRevision)`; a basis span
   absent from the ledger → `Revised(UnknownBasisSpan)`.

2. **Apply-gate unit tests (`projections.rs`)** —
   `apply_validated_patch_applies_append_only_stale_notes_patch`: seed ledger
   `{span-1@1}`, build a notes patch on that basis, append `span-2@1`, then
   `apply_validated_patch` → `Ok`, `notes.len() == 1` (During view populated).
   Companion `..._rejects_revise_stale_notes_patch`: revise `span-1` to rev 2 →
   `Err(StaleBasis)`, note count unchanged.

3. **Scheduler unit tests (`projection_scheduler.rs`)** — mirror the existing
   `scheduler_starts_coalesces_and_repairs_stale_in_flight_job`
   (`projection_scheduler.rs:702`):
   `scheduler_accepts_append_only_stale_completion_as_followup_not_discard` —
   start job on `{span-1@1}`, append `span-2@1`, `complete_in_flight` → assert
   `CompletedAndStartedFollowUp` (Background), `stale_discards == 0`,
   `completed_jobs == 1`, `follow_up_jobs_started == 1`, follow-up basis has 2
   spans. Keep a contrast case where `span-1` is *revised* → still
   `DiscardedStaleAndStartedRepair`, `stale_discards == 1`.

4. **Integration test (`speech/mod.rs`)** — clone the existing
   "stale projection apply repair completes" harness (`speech/mod.rs:~8990`,
   stub `patch_generator` with a `calls` counter and
   `RecordingProjectionRuntimeEventSink`): seed `span-1`, start the notes job,
   append `span-2` **before** the job's apply runs, `run_projection_job`, then
   `wait_until` asserts `materialized.notes.notes.len() == 1` **and**
   `event_sink.notes_count() >= 1` (a `MATERIALIZED_NOTES_UPDATE` was emitted)
   **and** a follow-up job was started — the direct proof that the During view is
   non-empty during continuous appends, without waiting for a pause. This is the
   regression test for the caad symptom.
