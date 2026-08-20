# Prototype findings: Finalizing / Finalization Blocked in Review (audio-graph-1d92)

Read-only review of branch `prototype/audio-graph-1d92`, tip
`95af7e5f9ede6940ff1d0e7af6639d296b78addb`, worktree
`/home/codeseys/DevBox/audio-graph/.worktrees/1d92-prototype`. Diff reviewed:
`git diff master...HEAD` (11 files, +2345/-10). Line numbers are as of that
tip. This is a decision artifact for the maintainer; the seed stays open.

Governing records: `CONTEXT.md:29-30` (definition of Finalization Blocked),
[ADR-0035](../../adr/0035-record-post-stop-finalization-failure-as-per-session-finalization-blocked.md)
(per-Session Blocked, split from app-modal `RecoveryRequired`),
[ADR-0036](../../adr/0036-derive-session-finalization-state-from-durable-barriers.md)
(derived barrier reconciler; no persisted stage enum).

## 0. Evidence base

### 0.1 The prototype's own tests (run by the reviewer, real output)

```
$ bun run test -- src/components/ReviewFinalizationPanel.test.tsx src/components/SessionsBrowser.test.tsx
$ vitest run src/components/ReviewFinalizationPanel.test.tsx src/components/SessionsBrowser.test.tsx

 RUN  v4.1.7 /home/codeseys/DevBox/audio-graph/.worktrees/1d92-prototype

(node:440252) ExperimentalWarning: localStorage is not available because --localstorage-file was not provided.
(node:440253) ExperimentalWarning: localStorage is not available because --localstorage-file was not provided.

 Test Files  2 passed (2)
      Tests  36 passed (36)
   Start at  23:32:58
   Duration  2.45s (transform 569ms, setup 336ms, import 991ms, tests 1.46s, environment 1.39s)
```

The load-bearing cases, verbatim from `--reporter=verbose`:

```
 ✓ ReviewFinalizationPanel > shows Finalizing with computed, non-persisted progress and per-lane coverage 40ms
 ✓ ReviewFinalizationPanel > shows STT interim vs. confirmed text as a per-lane confirmation summary and per-line badges 20ms
 ✓ ReviewFinalizationPanel > Finalization Blocked (external_uncertain) is non-dismissable and needs explicit cost/egress authorization 278ms
 ✓ ReviewFinalizationPanel > Blocked{UserCancelled} reads calmly and stays retryable without nagging 28ms
 ✓ ReviewFinalizationPanel > auto-retry-eligible classes clear with zero cost/egress once the ledger shows a later success (re-derived on render) 11ms
 ✓ ReviewFinalizationPanel > Finalized is reached on notes-lane coverage alone; a lagging graph lane never gates it 11ms
 ✓ ReviewFinalizationPanel > graph lane visibility variant: 'hidden' omits the graph lane entirely 8ms
 ✓ ReviewFinalizationPanel > blocked presentation variant: 'badge' renders a compact record without the long-form summary/detail text 13ms
 ✓ ReviewFinalizationPanel > retry affordance variant: 'autoHealOnly' hides the free-retry button for auto-eligible classes but keeps an explicit control for external_uncertain 27ms
 ✓ ReviewFinalizationPanel > retry affordance variant: 'autoHealOnly' passively re-polls an auto-eligible unresolved blocker without any click 61ms
 ✓ ReviewFinalizationPanel > evidence inspection is collapsed by default and expands to show redacted ledger entries 36ms
 ✓ ReviewFinalizationPanel > two different sessions never bleed into each other's derived stage 25ms
 ✓ SessionsBrowser — 1d92 > shows a per-row finalization pill derived fresh from the fetched status (Q2 default: list + detail) 25ms
 ✓ SessionsBrowser — 1d92 > one session's Finalization Blocked record never degrades another row (ADR-0035's core point) 27ms
 ✓ SessionsBrowser — 1d92 > the list-level Retry is never gated by reviewLocked, so a background session stays retryable while another Session is Live (Q5 default) 60ms
 ✓ SessionsBrowser — 1d92 > list surface variant 'detailOnly' hides every row pill without fetching finalization data 21ms
 ✓ SessionsBrowser — 1d92 > background access variant 'perSessionLoadGate' loads a non-active background session even while another Session is Live 36ms
```

`bun run typecheck` exits 0; `biome check` on the five touched source files
reports no findings. The suite emits React `act(...)` warnings on the four new
`SessionsBrowser` cases (the per-row `Promise.allSettled` fan-out resolves
outside `act`) — cosmetic, but it is the visible symptom of the fan-out
described in §2.3.

### 0.2 Reviewer probes (temporary test file, run then deleted, not committed)

Four claims that the prototype's own tests do not cover were verified with a
throwaway vitest file in the same worktree. Real output:

```
PROBE1 store.error = "Stop the live capture before opening a past session. Historical Review is locked so session data cannot mix."
 ✓ reviewer probe > perSessionLoadGate: Load is enabled but store.loadSession still refuses while live 240ms

PROBE2 status fetches: before=12 after=12 rows_visible=1
 ✓ reviewer probe > fetches finalization status for every store session, not just filtered-visible rows 132ms

PROBE3 notes.covered=true, graph-only blocker -> stage=finalization_blocked
 ✓ reviewer probe — lane blindness > a graph-lane-only blocker overrides notes-lane Finalized 2ms

PROBE4 notes blocker + graph-only success -> resolved=true, stage=finalizing
 ✓ reviewer probe — lane blindness > a graph-lane success clears a notes-lane never_dispatched blocker 1ms
```

What each probe establishes:

- **PROBE1** — with `isCapturing = isTranscribing = true` and
  `backgroundAccessMode = "perSessionLoadGate"`, the background row's **Load
  button is enabled and clicking it does nothing**: `load_session` is never
  invoked, `loadedSessionId` stays `null`, and the store sets the
  "Historical Review is locked" error. The gate the variant edits
  (`isLoadLocked`, `SessionsBrowser.tsx:117-125`) is not the gate that
  actually blocks loading — `store.loadSession` has its own guard at
  `src/store/index.ts:2938-2943` (and `loadSessionTranscript` a second one at
  `:2895-2899`). The prototype's own Q5 test asserts only
  `expect(loadBtn).not.toBeDisabled()`, so it passes without ever proving a
  load.
- **PROBE2** — the list fetch is scoped to `sessions` (the whole store list,
  refreshed at 200 on mount), not to the filtered/sorted `visible` array:
  `const visibleIdsKey = sessions.map((s) => s.id).join(",")`
  (`SessionsBrowser.tsx:248`), while the rendered rows come from `visible`
  (`:300`). 12 sessions produce 12 `get_session_finalization_status_cmd`
  invokes, and narrowing the search box to a single row does not reduce that
  count.
- **PROBE3 / PROBE4** — `FinalizationBlockedReason`
  (`src/types/reviewFinalization.ts:58-73`) carries **no lane**, and
  `isBlockedRecordResolved` (`:166-197`) filters the ledger by timestamp only.
  Consequences: (a) a blocker attributable to the graph lane forces the whole
  Session to read `Finalization Blocked` even with the notes lane fully
  covered — the "graph never gates Finalized" rule is honoured by the coverage
  path and violated by the blocked path; (b) a graph-lane-only ledger success
  auto-clears a notes-lane `never_dispatched` blocker.

## 1. Q1 — Blocking vs. dismissible presentation: **SETTLED — `banner`**

**What the prototype shows.** Both variants are genuinely non-dismissable and
genuinely not app-modal. There is no close/X control anywhere in the blocked
subtree, no `role="dialog"`, and no focus trap — the record is a plain
`role="status"` region (`ReviewFinalizationPanel.tsx:398-405`), and the panel
never imports `useFocusTrap` (only `SessionsBrowser` does, for its own
overlay). That is precisely the ADR-0035 split expressed in markup: same
undismissability, none of `RecoveryRequired`'s modality.

**Why `banner` wins, on contract rather than taste.** `CONTEXT.md:30` defines
Finalization Blocked as a state that "retains an exact reason and a retry
path." The `banner` variant renders the reason's `summary` **and** long-form
`detail` (`:420-429`), the class label, the `since` timestamp, the
"can't be dismissed" caption, and the class-appropriate retry control. The
`badge` variant deliberately drops `summary` and `detail` — its own test
asserts their absence — leaving a 4-way class label ("External — uncertain")
as the entire reason. A class label is a reason *class*, not "an exact
reason", so `badge` cannot satisfy the definition standing alone; it can only
be a pointer to something that does. And in the `badge` × `autoHealOnly`
combination the retry path also collapses to a caption, so the compact variant
can lose both halves of the CONTEXT.md guarantee at once.

`Blocked{UserCancelled}` is correctly de-escalated in both variants — neutral
border, no warning tint, "Finalization paused" / "Resume finalization"
(`:377`, `:385-396`, `:416-419`) — which is the operational reading of
ADR-0036's "Review does not nag about it."

**Residual, not reopening the choice.**

1. *Placement.* `App.tsx:924` mounts the panel as a fourth child of
   `.workspace-panel--after`, which is a two-column grid with
   `grid-template-rows: minmax(0, 1fr) auto` and `overflow: hidden`
   (`src/styles/layout.css:113-181`). Unlike `.workspace-panel__seek` — which
   explicitly spans `grid-column: 1 / -1` and caps at `min(240px, 34vh)` — the
   new section carries no grid placement and no height cap, so it auto-places
   into a half-width implicit third row and its content (blocked card + two
   lane rows + an uncapped transcript-line list + gaps + evidence) is unbounded
   inside a clipping container. "Always present" is therefore a layout claim
   the prototype has not paid for; it needs a full-width, height-capped slot.
2. *Announcement.* `role="status"` is a polite live region announced once. A
   Blocked record that appears while the user is in another workspace tab is
   never re-announced; the list pill is the only later discovery path.

## 2. Q2 — List-level vs. detail-level surfacing: **SETTLED — both (`listAndDetail`), and yes, the list must re-derive per row**

**Why `detailOnly` is disqualified by a fact, not a preference.** The detail
panel is keyed off `loadedSessionId` (`App.tsx:924`), and `loadedSessionId` is
set **only** by `store.loadSession` (`src/store/index.ts:2980`) — i.e. only by
SessionsBrowser's Load — and is explicitly cleared by `startCapture`
(`:2099`). Stopping a capture never sets it. So for the single most common
Finalizing case — *the session you just stopped recording* — `detailOnly`
surfaces **nothing at all**: there is no session-level status affordance in
the After workspace for the just-captured session, and reaching one requires
opening the browser and loading the session, which is itself refused while
transcription is still draining (§4). `detailOnly` would make the default path
into Finalizing invisible, which is exactly ADR-0035's named "quiet per-Session
debt" regression.

**Derivation across the two surfaces.** Both surfaces call the same
`deriveFinalizationStage` (`types/reviewFinalization.ts:201-212`) over a
payload that deliberately has no `stage` field, so the two cannot disagree
*given the same input*. `SessionsBrowser`'s `FinalizationPill` (`:666-704`)
recomputes on every render from the fetched row status; nothing is cached or
persisted. That is the right shape and it answers the seed's sub-question
affirmatively: with no persisted stage, the list must evaluate the predicates
for every row it wants to badge.

**Two measured caveats the acceptance contract has to absorb.**

1. *Freshness, not purity, is the real requirement.* ADR-0036 says Blocked is
   "re-derived before it is shown or retried." The prototype re-derives on
   every render but from **one snapshot fetched once**: the panel fetches on
   `sessionId` change only (`ReviewFinalizationPanel.tsx:120-130`), the list
   fetches on `visibleIdsKey` / variant change only (`SessionsBrowser.tsx:249-273`),
   neither polls in the default variant, and the `reviewFinalization.refresh`
   i18n key added in this branch is dead (no component uses it). A pure
   function over a stale snapshot satisfies "re-derived on render" while
   displaying a stale claim. The only place in the branch where inputs are
   actually re-read without a click is the `autoHealOnly` 50 ms
   `setTimeout` (`:136-149`), which the builder correctly labels a mechanic
   proof rather than a policy.
2. *Fan-out is unscoped.* PROBE2: one IPC call per **store** session (up to
   200 on mount), not per visible row, and independent of the search filter.
   The `act(...)` warnings in §0.1 are this fan-out resolving outside React's
   batch. A bulk/list-scoped endpoint is not an optimisation here; it is what
   makes the list surface admissible at all.

## 3. Q3 — Retry affordance vs. silent self-healing: **SETTLED — explicit, class-typed button (`explicitButton`), with auto-clear as a predicate property rather than a UI promise**

**What the prototype proves.** The panel encodes ADR-0036 sub-question default
2 in the *predicate*, not in button visibility:
`isBlockedRecordResolved` (`types/reviewFinalization.ts:166-197`) clears
`never_dispatched` / `provably_absent` on any later ledger success (free, no
egress, no click) but requires, for `external_uncertain` and `user_cancelled`,
a success whose `attempted_at_ms >= retry_requested_at_ms` — so those two can
never be cleared by inferred forward progress. `FX_BLOCKED_AUTOHEALED` never
renders as Blocked at all; the panel shows "Cleared automatically — a later
attempt already succeeded." That is the seed's "reads from the same derived
predicates rather than assuming forward progress," demonstrated.
`external_uncertain` keeps an "Authorize retry (may contact a provider and
incur cost)" control in **both** variants, and the test proves
`authorizeCostAndEgress: true` is sent only after the inline Confirm step —
no network call fires from the first click.

**Why `explicitButton` beats `autoHealOnly` as the default.** The auto-heal
re-poll lives in a `useEffect` on the mounted panel (`:136-149`). It therefore
only runs *while the user is already looking at the session* — the one
situation in which a click costs nothing. When the user is not in Review (the
case where the debt rots), `autoHealOnly`'s caption "AudioGraph rechecks this
automatically — no click needed" is a promise the frontend cannot keep and the
backend has not been specified to keep (real polling/push policy is on the
builder's unanswerable list). An explicit free-retry button makes no claim
about background behaviour, and it costs nothing for the two provably free
classes.

**One must-fix before this answer can ship: the list-level retry contradicts
it.** `SessionsBrowser.retryFinalization` (`:275-291`) hardcodes
`authorizeCostAndEgress: false`, and `FinalizationPill` renders one generic
`sessions.finalization.retry` = "Retry" button for **every** blocked class,
including `external_uncertain` and `user_cancelled` (`:691-701`), with the
failure path a bare `catch {}` (`:283-286`) that shows the user nothing. The
branch's own Q5 test drives exactly this path — it clicks the generic Retry on
`fx-blocked-external` (class `external_uncertain`) with
`authorizeCostAndEgress: false`, and the *fixture* clears the blocker. A
backend that honours ADR-0036 must refuse that call, so against real code the
dominant ADR-0035 failure class gets a button that silently does nothing. The
list pill's retry must be class-typed (offer it only for the two cost-free
classes, plus "Resume" for `user_cancelled`), and for `external_uncertain` it
must route to the authorization UI instead of firing an unauthorized retry.

## 4. Q4 — Per-lane coverage granularity: **SETTLED — `informational`** (show the graph lane), with lane attribution as a required model fix

**Which failure mode the prototype chooses to risk, and why it is the right
one.** `informational` shows the graph row and mitigates its named risk with
copy in two places rather than leaving it to inference: the row itself carries
"Not required to finish" (`:337-343`) and the block carries "Graph coverage
never blocks Finalized — shown for visibility only" (`:280-282`). Meanwhile
`FX_FINALIZED` renders the chip as **Finalized** with the graph lane at 9
pending, and a test asserts exactly that pairing — so the prototype
demonstrates the strongest possible refutation of "showing it implies it is
required." `hidden`, by contrast, makes ADR-0036's own named negative
consequence undiagnosable: a graph lane stalled on an unchanged basis
(`projection_scheduler.rs:355-357`, whose owner ADR-0036 explicitly leaves
unassigned, with no watchdog) would be invisible forever, and ADR-0036 already
notes that `audio-graph-a668`'s absence-claim class is inert for graph facts.
Hiding the only remaining signal is the worse risk.

**Two required additions.**

1. *A count is not a stall signal.* The payload carries
   `oldest_pending_since_ms` per lane and the branch adds a
   `reviewFinalization.lane.pendingSince` string — but nothing renders either
   (the key is dead). "41 pending" looks identical after one minute and after
   one month, so as prototyped the informational row cannot actually reveal a
   stalled lane. Render lane age (or an explicit stall marker) or the variant's
   whole justification evaporates.
2. *The blocked record must be lane-attributed.* PROBE3: a graph-lane blocker
   sets the whole Session to `Finalization Blocked` even with notes covered —
   the non-required lane gates the boundary through the blocked path. PROBE4: a
   graph-lane success auto-clears a notes-lane `never_dispatched` blocker.
   Both follow from `FinalizationBlockedReason` having no `lane` field. The
   derivation must (a) attribute each blocker to a lane, (b) let only
   *required*-lane blockers set the Session stage, and (c) match ledger
   successes lane-for-lane when testing resolution.

## 5. Q5 — Background finalization vs. live-session usability: **OPEN**

**What works.** The default `coarseLockPlusInlineRetry` is real and proven:
with `isCapturing = isTranscribing = true`, the blocked row's inline Retry is
enabled, fires, and clears the pill while Load stays disabled — the branch's
own test asserts all four facts in one case. Today's coarse
`reviewLocked = isCapturing || isTranscribing` behaviour is untouched for Load
(`isLoadLocked(..., "coarseLockPlusInlineRetry", ...)` returns `reviewLocked`
verbatim), so the variant is additive.

**Why the question is still open.** The two halves fail for different reasons.

- `perSessionLoadGate` **does not work** (PROBE1). It only edits the button's
  `disabled` predicate; `store.loadSession` independently refuses while
  capturing/transcribing (`src/store/index.ts:2938-2943`), so the enabled
  button produces an error toast and no load. The root cause is named in the
  store itself at `:2100`: "Historical Review and Live currently share one
  frontend view store" — `loadSession` overwrites `transcriptSegments`,
  `graphSnapshot`, `sessionTimeline`, `materializedNotes` and more. Making the
  per-session gate real is per-session view isolation in the frontend store,
  not a predicate swap, and the prototype does not model that.
- `coarseLockPlusInlineRetry` covers only the cost-free classes once §3's
  must-fix is applied. The authorization + confirm UI for
  `external_uncertain` — the dominant ADR-0035 failure — lives only in the
  detail panel, which is reachable only via `loadedSessionId`, which is
  reachable only via `loadSession`, which is refused while another Session is
  Live. So the *most likely* background Blocked state remains unreachable and
  un-retryable exactly while a second Session is Live, which is the scenario
  the question asks about.

**What specifically is missing.** A read-only per-Session finalization detail
path that does **not** go through `loadSession` (a row-expansion or a
side-panel that renders the same `ReviewFinalizationPanel` content from the
already-fetched status, touching none of the shared view store) — or, more
expensively, the store-level view isolation that would make
`perSessionLoadGate` honest. Until one of those exists, the prototype cannot
answer the question, and a third variant is the cheapest next experiment.

## 6. Also worth recording

- **The transcript interim/confirmed presentation is the cleanest part of the
  branch** and is not one of the five questions: aggregate counts plus per-line
  "Interim — not yet confirmed" / "Confirmed" badges (`:549-597`), distinct
  from live capture's `asrPartial` badge. Its counts, however, come from a
  `transcript_confirmation` block that is a separate payload field from
  `lane_coverage`; if those two are not read from the same drain watermark in
  the real backend, Review will show a confirmed count from one authority and a
  stage from another. That is a contract requirement, not a UI choice.
- **Evidence disclosure is correctly redacted**: collapsed by default, and
  expanded it shows only `lane · outcome · timestamp` plus a cost/egress flag,
  with a test asserting no "chunk" text ever appears. This satisfies the seed's
  "keep transport-level LLM chunks out of canonical notes."
- **Knowledge Gaps never gate the stage** — rendered informationally
  (`:599-628`), consistent with Q0.2 as recorded in ADR-0036.
- **Degradation is honest.** With no Rust command, `invoke` rejects and the
  panel renders a calm "No finalization data for this session," keeping the raw
  error on `data-error` — and the list simply shows no pills. Nothing in this
  branch changes behaviour against today's real backend.
- **Dead i18n keys** added by this branch: `reviewFinalization.refresh`,
  `.error`, `.retry.button`, `.lane.pendingSince`, `.knowledgeGaps.empty`,
  `.evidence.title`, `sessions.finalization.viewDetails`, `.hideDetails`.
  Two of them (`refresh`, `pendingSince`) mark exactly the two missing
  affordances named in §2 and §4.
- **`src/test/setup.ts` localStorage polyfill** is unrelated to the design
  questions but is a real, correctly-scoped fix (Node ships an unwired Web
  Storage and jsdom then declines to shim it); it is guarded to no-op wherever
  `localStorage` already works.

## 7. Recommendation for how `audio-graph-44c1` should reference these states

`audio-graph-44c1` is the claim-bounded acceptance and failure-injection
contract. It should reference Finalizing / Finalization Blocked / Finalized
**behaviourally — by predicate and display obligation — never by payload
shape**, because the payload in `src/types/reviewFinalization.ts` is a
documented guess blocked on `audio-graph-90f3` / `audio-graph-8e73`. Concretely,
44c1 should require:

1. **One derivation, zero stage fields.** Every acceptance assertion about a
   Review-visible stage must be produced by the single shared derivation
   function over durable inputs. The contract should *forbid* a fixture that
   seeds a stage string, and should state — per ADR-0032's "a command may claim
   only what it asserts" — that "Finalized" in Review may be asserted only from
   required-lane coverage over the durable watermark.
2. **A stated freshness bound on the inputs, not on the derivation.** Purity
   over a stale snapshot passes any "re-derived on render" test (§2). 44c1 must
   fix a maximum input staleness for any displayed stage and require a
   failure injection in which the blocker clears while the surface is mounted
   and the surface reflects it without a click.
3. **Lane-attributed blockers.** Two mandatory injections, both currently
   failing (PROBE3/PROBE4): a graph-lane-only blocker with notes covered must
   still read Finalized; a notes-lane `never_dispatched` blocker must **not**
   auto-clear on a graph-lane success.
4. **Class-typed retry, with the egress claim treated as a claim.** Assert that
   `authorizeCostAndEgress: false` is reachable only for
   `{never_dispatched, provably_absent}`; assert **zero** provider attempts
   before the explicit Confirm for `external_uncertain`; assert that a refused
   unauthorized retry is surfaced to the user rather than swallowed (§3's
   `catch {}`). The UI string "Retry (free — no data leaves this device)" is a
   negative data-egress claim rendered to a user, so under
   [ADR-0034](../../adr/0034-require-exhaustive-evidence-for-negative-data-egress-claims.md)
   it needs exhaustive producer-inventory evidence, not a fixture.
5. **Two-surface consistency as an explicit injection.** Require that the list
   pill and the detail surface be produced by the same function, and add a
   divergence case (a retry heals the session while the list is open). Today
   divergence is unreachable only by accident, because SessionsBrowser closes
   on Load.
6. **A live-concurrency case, and an explicit product decision on its ceiling.**
   With a second Session Live, the just-stopped Session's Blocked state must be
   reachable and — for the cost-free classes — retryable. If 44c1 wants the
   `external_uncertain` authorization reachable in that state, it must require
   a read-only detail path that does not call `loadSession`; otherwise it must
   record "authorization for externally-uncertain blockers is unavailable while
   another Session is Live" as a *documented product constraint* inherited from
   the shared frontend view store (`src/store/index.ts:2100`), not leave it
   implied.
7. **List scale.** Include a 200-session list case and forbid per-row IPC
   fan-out as the shipping shape (PROBE2: 12 rows → 12 invokes, filter-independent).
8. **Accessibility evidence for a non-modal undismissable state.** State what a
   screen-reader user is guaranteed — announced once on appearance
   (`role="status"`), rediscoverable later via the list pill — and *forbid*
   `role="dialog"` / focus trapping for Finalization Blocked, so the ADR-0035
   split from `RecoveryRequired` is enforced in the accessibility layer and not
   only in the visual one.
9. **Restart recovery, phrased as re-derivation.** Since ADR-0036 forbids a
   resume-at-stage branch, the restart injection must assert that the same
   surfaces produce the same stage from the same durable inputs after a
   simulated restart. The prototype's fixtures are static and cannot model
   this; 44c1 should not treat this branch as evidence for it.

Nothing in §7 requires the missing durable `Accepted` barrier to be specified
now — each item is a claim bound, which is what 44c1 owns.

## 8. Verdict summary

| # | Question | Verdict | Winner / gap |
|---|---|---|---|
| Q1 | Blocked presentation | **SETTLED** | `banner` — only variant that retains "an exact reason and a retry path" (`CONTEXT.md:30`); both variants stay non-modal (no dialog role, no focus trap) |
| Q2 | List vs. detail surfacing | **SETTLED** | Both (`listAndDetail`); `detailOnly` cannot show the just-stopped session at all because `loadedSessionId` is set only by `loadSession`. List must re-derive per row; freshness + fan-out become contract items |
| Q3 | Retry affordance | **SETTLED** | `explicitButton`, class-typed; auto-clear belongs in the predicate, not in a caption promising background work. Must-fix: the list-level retry is class-blind, hardcodes `authorizeCostAndEgress: false`, and swallows failures |
| Q4 | Graph-lane visibility | **SETTLED** | `informational`; risk mitigated by two explicit "not required" affordances and by Finalized-with-graph-pending being demonstrated. Requires rendering lane age and lane-attributing the blocker |
| Q5 | Background access while Live | **OPEN** | `perSessionLoadGate` is a dead button (`store.loadSession` refuses independently); inline retry covers only cost-free classes. Missing: a read-only detail path that does not call `loadSession` |
