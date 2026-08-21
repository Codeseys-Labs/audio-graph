# Shell recomposition — unit-by-unit execution plan

Planning pass, 2026-08-20, against master at `a4265de` (+ seeds T1–T3 filed as
`audio-graph-8d89`/`b9dc`/`99aa`; T1 in implementation at the time of writing). The maintainer has
ratified the **full shell recomposition** (seed `audio-graph-19c7`, extension
`ratified_full_recomposition_2026_08_20`) over the panel-recommended graft-only path, together with
the panel's decisions D2 (native font stack for now), D3 (`--accent-blue` aliased to `--accent`
at ~265°, `--accent-gemini` widened to ~178°), D4 (Settings bridge committed), D5
(`@radix-ui/react-popover` only), and D6 (reading surfaces raised, diagnostics dense).

Primary sources: Design 3 ("session-first shell", workflow `wf_993ac5be-0aa`) for the
recomposition units; Design 2 ("Deck Kit") for component-kit-shaped mechanisms; the judge synthesis
(`ui-design.md`) for verified facts and the T1–T7 ticket set; `constraint-sheet.md` for the
grounded inventory. Every load-bearing code claim below was re-verified directly against the
working tree (file:line cited inline).

**T4 (Sessions graft into the old Review tab) is dissolved** — the Sessions destination is now a
recomposition unit (R2). **T5 is superseded**: its ControlBar/switcher scope dissolves into R3/R4
and its PopoverOverlay anchor fix dissolves into R2/R3 (the component is retired outright, which
fixes the ≤1120px overlap bug by deletion). **T6 and T7 are re-cut** as R6 and R8 below.

---

## 1. Global invariants (every unit, no exceptions)

1. **ADR before ids.** The ADR-0030 amendment (R0) lands as its own decision record before any
   unit renames a tab id or touches `e2e/specs/shell.e2e.ts`. No unit bundles
   "amend ADR + rename ids + rewrite E2E" — that exact bundle is the named stall risk.
2. **The suite never lies.** The E2E rewrite travels *with* the unit that renames ids (R4), in the
   same landing. Until R4, `git diff --stat e2e/` is empty for every unit. Every landing is
   rebase-squash-merged onto master with `bun run verify:fast`, `bun run test` (vitest incl.
   `locale-parity.test.ts`, `styles.a11y.test.ts`, `App.contract.test.tsx`) green; cargo suites
   untouched (this plan contains **zero Rust changes** — `get_session_id` already exists,
   `src/store/index.ts:1979`) . Units that repaint or re-home E2E-pinned surfaces (R2, R3, R4)
   additionally require a CI E2E run before merge.
3. **Recipes first, never parallel styles.** T2 (`audio-graph-b9dc`) and T3 (`audio-graph-99aa`)
   land before any recomposition unit restyles anything. New chrome built by R2/R3/R5 (strip,
   destination bar, lens tabs, preflight card, drawers) is composed from `.ag-*` recipes +
   existing tokens from day one. R6 is the only unit that restyles *existing* panel internals.
4. **Contracts that survive every unit:** i18n en/pt parity (both locale files in the same
   commit; never rename a key in the same review commit that adds one); a11y invariants (roving
   tabindex tablists, `aria-selected`/`aria-controls`/`aria-labelledby` wiring, both live regions,
   skip-to-main, focus traps, `prefers-reduced-motion`); the ADR-0034 redaction/evidence copy in
   `SessionDataRoutePanel` **byte-identical** (re-parenting is allowed, re-wording is not — state
   the non-engagement in each PR body); session restore/trash/export/delete flows; and the
   credential-v2 worktrees under `.worktrees/` are never referenced, rebased, or touched.
5. **Mobile/wearable shape stays explicit.** Session-as-noun list→detail (R2) and session-scoped
   view providers (`SessionViewProvider`/`useSessionView`, R1) are the load-bearing shapes this
   run buys. No `<768px` work, no touch/gesture/wearable scope (deferred to seed
   `audio-graph-8055`'s architecture session, per Design 3's own weakness analysis).
6. **No new negative-egress claims.** New surfaces render key-*presence* booleans and
   settings-derived *planned* routes only; anything labeled **observed** requires session-scoped
   data-movement evidence (ADR-0034 / ADR-0030). Verified: no active-route state exists in
   `src/store/index.ts`, `src/types.ts`, or `src/generated/*` today, so the strip route chip in
   R3 ships planned-labeled (see §4, 8d18).

---

## 2. Unit sequence

Order: **R0 → R1 → R2 → R3 → R4 → R5 → {R6 ∥ R7} → R8**, R9 decide-gated. T2/T3 proceed in
flight and must merge before R2 starts restyled chrome (they are shell-independent by
construction). Every unit is independently landable and leaves the app coherent — the two
transitional states are named in R3.

### R0 — ADR-0030 amendment record (docs only)

- **WHAT:** Land the amendment drafted in §3 as its own ADR (next free number at landing time —
  0046 unless T2's 0009/0016 recipe-layer amendment claims it first), plus a note banner at the
  top of `docs/adr/0030-*.md` pointing to it (same pattern ADR-0035 used on ADR-0028). Update the
  ADR index. No code.
- **FILES:** `docs/adr/00XX-*.md` (new), `docs/adr/0030-*.md` (note banner), `docs/adr/README.md`.
- **ACCEPTANCE:** ADR process run by the conductor; `verify:fast` (docs secret hygiene) green;
  no `src/`/`e2e/` diff.
- **SIZE:** S. **DEPS:** maintainer ratification (given 2026-08-20). Blocks R4 (and morally
  everything after).

### R1 — ShellNav slice + SessionViewProvider shim + region extraction (contract-neutral)

- **WHAT:** (1) `src/store/shellNav.ts` — the first real store slice file: a typed nav object
  replacing the seven scattered flags (`workspaceView` App-local `useState` at `App.tsx:297`,
  `loadedSessionId`, `rightPanelTab`, `sessionsBrowserOpen`, `agentOverlayOpen`,
  `tokenOverlayOpen`, `settingsOpen`) —
  `{dest:"capture"|"sessions", sessionId, lens} + drawer`. During this unit the object still
  *drives the existing three-tab shell* (a `during/after/analysis` view is derived from it);
  ids, classes, labels, DOM structure byte-identical. Moving nav into the store is what later
  lets `stopCapture` route (R2) — a store action cannot set App-local state today.
  (2) `SessionViewProvider` + `useSessionView()` returning the session-scoped selector set
  (`transcriptSegments`, `graphSnapshot`, `materializedNotes`, `sessionTimeline`,
  `sessionProjectionEvents`), defaulting to the global store; one-line reads changed in
  `NotesPanel`/`LiveTranscript`/`KnowledgeGraphViewer`/`SeekTimeline`. Zero behavior change; when
  per-session view isolation lands in the store (`store/index.ts:2100`, explicitly out of scope),
  Live and Review render two providers and no panel reopens.
  (3) Extract App.tsx's four named regions (banners+strip / destination bar / rail–content–aside /
  footer) as composition seams rendering today's content.
- **FILES:** `src/store/shellNav.ts` (new), `src/store/index.ts` (flag delegation),
  `src/App.tsx`, `src/components/{NotesPanel,LiveTranscript,KnowledgeGraphViewer,SeekTimeline}.tsx`
  (one-line reads), `src/session/SessionViewProvider.tsx` (new), tests for the slice.
- **ACCEPTANCE:** This unit exists to prove the refactor is contract-neutral:
  `App.contract.test.tsx` green **untouched**, `git diff --stat e2e/` empty, full vitest green,
  zero i18n churn, zero visual diff. E2E run in CI green with zero spec edits.
- **SIZE:** M. **DEPS:** T1 (`audio-graph-8d89` — the contract net must exist first).
- **LANDED STATUS (seed `audio-graph-59fb`, branch `shell/audio-graph-59fb`) — read before starting
  R2/R3:** what actually shipped is narrower than the WHAT bullet above promises, by deliberate
  design (reset call sites in `store/index.ts` bypass the named flag setters, so a synchronized
  mirror would silently drift — worse than not shipping it). Recorded here so R2/R3 don't assume
  state that doesn't exist yet:
  - Only **1 of 7** flags actually moved: App-local `workspaceView` → `nav.dest`/`nav.lens`
    (`deriveWorkspaceView`/`navForWorkspaceView` in `src/store/shellNav.ts`). The other six —
    `loadedSessionId`, `rightPanelTab`, `sessionsBrowserOpen`, `agentOverlayOpen`,
    `tokenOverlayOpen`, `settingsOpen` — are **untouched**: same state, same actions, same call
    sites. `src/store/index.ts` gained only the 4-line slice spread (`createShellNavSlice`), not
    the "flag delegation" this section's FILES line named.
  - The `+ drawer` half of the shape is **not instantiated**. `ShellDrawerState` exists in
    `src/store/shellNav.ts` as a documented type only (no store field, no reads, no writes) —
    R3's System drawer lands *on* this name, not on live state.
  - `nav.sessionId` has **zero writers and zero readers** this unit; R2's `stopCapture` is meant
    to be its first caller (`nav = {dest:"sessions", sessionId, lens:"notes"}`).
  - `setNavDest`/`setNavSessionId`/`setNavLens` ship with no call sites yet — R2 is the intended
    first caller. Known sharp edge for R2: `setNavDest` alone does not renormalize `lens`, so
    calling it while `lens` is still `"graph"` (reachable via the graph-edge-focus effect) derives
    to `analysis` rather than `after`/Review — use `setWorkspaceView` (which does renormalize) or
    normalize `lens` explicitly at the call site.
  - Tests for the slice (named in this section's FILES line) landed in a follow-up fix pass, not
    the original commit: `src/store/shellNav.test.ts` (pure-function + store-wiring coverage,
    including the same-value-bailout and dest/lens footgun above) and
    `src/session/SessionViewProvider.test.tsx`.
  - E2E was not run locally for this unit; `git diff --stat origin/master...HEAD -- e2e/` was
    confirmed empty (no spec edits), and the ACCEPTANCE line's "E2E run in CI green" is satisfied
    by the PR's CI run, not by a local run — flagging explicitly rather than leaving it ambiguous.

### R2 — Sessions destination: list→detail with lenses; Stop lands on your session

- **WHAT:** Promote the session from a modal-mutated global to a real destination, rendered
  *inside* `#workspace-panel-after` with all three tabs/ids intact (zero E2E edits).
  (1) `SessionsBrowser` modal → rail(list) + detail composition: search + persisted
  `SessionSortMode`, one row per `SessionMetadata` (title, relative time, duration, counts, state
  chip via `.ag-chip[data-tone]`, extensible to `finalizing`/`blocked` when ADR-0035/0036 surface
  them); export/trash/restore/permanent-delete move to a row overflow menu
  (`@radix-ui/react-popover` per D5 — anchored disclosure, roving focus hand-rolled on existing
  patterns); Trash filter in the list header. The lazy split for `SessionsBrowser` ends;
  keep the force-graph vendor chunk deferred and verify with `bun run build:analyze`.
  (2) **Stop lands you on your own session:** `stopCapture` (`store/index.ts:2157-2177`, verified
  it sets only capture flags today) reads `get_session_id` before stopping, sets
  `nav = {dest:"sessions", sessionId, lens:"notes"}`, calls `listSessions()`; if the index has
  not been written yet, render an optimistic "finalizing" row (the 1d92 gap: detail alone misses
  the just-stopped session).
  (3) Detail = lens tabs (second tablist, same roving pattern as the right-panel pair):
  **Notes** (default) · **Transcript** · **Timeline** (`SeekTimeline` promoted from a
  `min(240px,34vh)` strip to a full lens) · **Graph** (`KnowledgeGraphViewer`, still lazy) ·
  **Route** (`SessionDataRoutePanel`, component and copy untouched → ADR-0034 non-engagement).
  Aside complements the lens; `ChatSidebar` becomes an "Ask" aside lens on Notes/Graph.
  (4) While live, detail shows the live-locked state reusing the existing
  `sessions.reviewLockedWhileLive` copy (`en.json:778`) — concurrent Live+Review is *not*
  delivered, only designed-for via the R1 shim.
  (5) Manual whole-session synthesis moves to the detail overflow as **Generate prose summary**
  (19c7 acceptance): the `data-notes-synthesize` control leaves the NotesPanel header in Sessions
  context; retarget the keyboard shortcut that queries it (`NotesPanel.tsx:118`).
  (6) Fold seed `audio-graph-e7e5`: one hours-aware duration formatter in `src/utils/format.ts`;
  delete `SessionsBrowser.tsx:77`'s local copy; align `ProjectionRuntimeStatusPanel`'s
  `formatAgeMs`.
  (7) Sample preview (`previewSampleSession`, `App.tsx:392-409`) renders as an ephemeral session
  detail; `workspace.stateSample` narration unchanged.
  Interim note: the `analysis` tab still exists and still shows graph + diagnostics; the Graph
  and Route lenses duplicate reach, not state. That duplication is deliberate staging — it is
  what makes R4 pure deletion.
- **FILES:** `src/components/SessionsBrowser.tsx` (rebuilt in place), `src/App.tsx` (after-panel
  composition), `src/store/index.ts` (`stopCapture`, nav wiring), `src/store/shellNav.ts`,
  `src/utils/format.ts`, `src/components/{NotesPanel,ProjectionRuntimeStatusPanel}.tsx`
  (formatter import, synthesize relocation), `package.json` (+`@radix-ui/react-popover`),
  `en.json`+`pt.json` (net-new keys, budget ≈ 20), `SessionsBrowser.test.tsx` (rewritten not
  deleted; d19f case preserved), ~3 Review-touching `App.test.tsx` cases.
- **ACCEPTANCE:** stopping a capture leaves the user viewing the just-ended session's notes;
  `git diff --stat e2e/` empty; `App.contract.test.tsx` green untouched; all tab ids/roles/labels
  byte-identical; restore/trash/export/delete and session restore regression-free;
  `SessionDataRoutePanel` copy byte-identical; new shared-formatter test; parity green; full
  vitest + CI E2E run (pinned Review surfaces repaint).
- **SIZE:** L. **DEPS:** R1; T2/T3 (recipes/accents for the new chrome). Sequential before R3
  (both touch `App.tsx` and the store).

### R3 — NOW STRIP + one Start + System drawer + composite health (folds 50e3)

- **WHAT:** (1) `ControlBar` (490 lines) → **NOW STRIP**: brand (16px/600 `--text-primary`, not
  accent-colored); **Start/Stop with verbatim aria-labels** (`controlBar.start`/`stop` keys
  unrenamed — E2E-pinned); elapsed (tabular-nums via T3); durability ("notes saved · Ns ago"
  from existing persistence state); **planned-route chip** ("planned: Deepgram → OpenRouter",
  derived the same way `hasConfiguredDurableNotesRoute` (`App.tsx:111`) already reads settings —
  the word *planned*, never *observed*, per ADR-0030/0034); **composite health chip**
  (green/degraded/error, the 50e3 dot) opening the System drawer; settings/sessions icon cluster
  retained at the right end. Idle it collapses to one line ("Ready · N sources · route · Start").
  (2) **One Start:** the strip's Start = `start_capture`, then `startTranscribe()` **only if the
  existing `transcribeDisabled` predicate is false** — the same ADR-0033 gate the Transcribe
  button renders today. In CI no route is configured, so the transcribe leg never fires and the
  mocked E2E flow issues exactly `start_capture` — **verify this before coding**: unmocked
  commands fall through to real Rust (`shell.e2e.ts:186`) and an unconditional second invoke can
  emit an ERROR-level line, breaking gate 5. The Transcribe and Gemini buttons leave the primary
  surface (purple leaves the strip for free).
  (3) **System drawer** (hand-rolled, `useFocusTrap` + Escape, per D5 — no Radix dialog):
  `ProjectionRuntimeStatusPanel` + `TokenUsagePanel` + per-stage pipeline detail. Opened from the
  health chip. `PopoverOverlay` is **retired** (its two consumers move here / to the Assist
  inline surface), which deletes the ≤1120px anchor-overlap bug rather than fixing it.
  (4) **50e3 fold:** during healthy capture `PipelineStatusBar` collapses to the composite state
  (per-stage dots return on error and live permanently in the System drawer) — progressive
  disclosure exactly as seeded; announce state *transitions* only (no live-region churn).
  Transitional states, named: `ConversationModeControl` and the Gemini toggle remain in the strip
  as demoted ghost/secondary controls until R5 relocates them into the preflight card; the
  workspace switcher (and `.workspace-switcher__state`) remains untouched below the strip until
  R4.
- **FILES:** `src/components/ControlBar.tsx` (→ `NowStrip.tsx`), `src/components/SystemDrawer.tsx`
  (new), `src/components/PipelineStatusBar.tsx`, `src/components/PopoverOverlay.tsx` (deleted),
  `src/App.tsx`, `src/store/index.ts` (merged-start action), `src/styles/layout.css`,
  `en.json`+`pt.json` (strip/drawer keys, budget ≈ 15), `ControlBar`/`PipelineStatusBar` tests.
- **ACCEPTANCE:** E2E is the real gate and must run in CI: `button[aria-label="Start"]`/`"Stop"`
  verbatim, `.workspace-switcher__state` flips "Live session" (element untouched in this unit),
  rejected second start surfaces `.notifications .notification--error`, zero ERROR-level frontend
  log lines (the merged-start gating proof), zero secret shapes; `git diff --stat e2e/` empty;
  exactly one saturated element in the idle strip (screenshot evidence 1440/1120, both themes);
  composite dot behavior per 50e3 with per-stage detail intact in the drawer; parity green.
  **Closes `audio-graph-50e3`** (acceptance folded here; "no regression to Analysis diagnostics"
  = System drawer + the R2 lenses keep full detail).
- **SIZE:** L. **DEPS:** R1, T2/T3; after R2 (shared `App.tsx`/store surface).

### R4 — Destination collapse + id rename + E2E/contract rewrite (travels together)

- **WHAT:** The mechanically smallest possible rename unit — by this point it is almost pure
  deletion. (1) `WORKSPACE_VIEWS` (`App.tsx:216`) → `["capture","sessions"]`;
  `#workspace-tab-during`→`#workspace-tab-capture`, `#workspace-tab-after`→`#workspace-tab-sessions`,
  panels likewise; the `analysis` tab and its panel/right-rail composition
  (`App.tsx:919-949`, `analysisContextPanel` at `:680-731`) are **deleted** — every occupant
  already has a home (Graph/Route/Ask lenses from R2; diagnostics in the System drawer from R3).
  (2) `.workspace-switcher__state` moves onto the strip **keeping class name and
  `workspace.stateLive` text verbatim** (E2E test 4 then needs zero edits). (3) The
  `graphEdgeFocus` bridge (`App.tsx:350-360`) retargets from `setWorkspaceView("analysis")` to
  `nav.lens = "graph"` within the same session — the ADR-0026 provenance hook, strictly smaller
  side effect. (4) Skip-to-main targets `#workspace-panel-{dest}`; the polite announcement
  becomes "destination entered"; roving handler is already length-generic
  (`App.tsx:650-664`). (5) **In the same landing:** rewrite `shell.e2e.ts` tests 1 and 3
  (first-paint selector → `#workspace-tab-capture`; the 3-tab arrow-wraparound block
  (`:250-294`) becomes the 2-tab equivalent — one ArrowLeft from `sessions` lands `capture`, a
  second wraps back), and rewrite the id/wraparound facts in `App.contract.test.tsx` to match.
  Tests 2, 4, 5, 6 need zero edits by construction. (6) i18n: `workspace.capture`/`sessions`
  added, `during`/`after`/`analysis` retired, both locales, rename-only review commit separate
  from add-only.
- **FILES:** `src/App.tsx`, `src/store/shellNav.ts`, `e2e/specs/shell.e2e.ts`,
  `src/App.contract.test.tsx`, ~9 IA-shaped `App.test.tsx` cases (32 of 41 survive untouched —
  Design 3's count, spot-verified against the suite's role/label-query dominance),
  `en.json`+`pt.json`, `src/styles/layout.css` (switcher → destination bar styling on recipes).
- **ACCEPTANCE:** Because R0 (ADR), R2 (Sessions), and R3 (strip) landed first, **a red E2E in
  this unit has exactly one possible cause**. Full CI E2E mandatory; contract test and spec
  updated in lockstep in one squash; zero copy changes other than the workspace keys; parity
  green; screenshots 1440/1024 both themes.
- **SIZE:** M (mechanical but wide). **DEPS:** R0, R2, R3. **This is the only unit allowed to
  touch `e2e/`.**
### R5 — Ready preflight card + mode-control relocation

- **WHAT:** The Capture destination's idle state stops being an empty live cockpit: a preflight
  card (`.ag-card`/`.ag-field`) with three pass/fail rows, each with a fix action — Sources
  (n selected), Route (planned: ASR → LLM — same settings read as the strip chip), Storage
  (reusing `StorageBanner`'s data) — then one **Start session** button (the strip's Start stays;
  same action). `ConversationModeControl` moves into the card as a preflight choice
  ("Mode: Notes / Converse"); the Gemini toggle moves with it (ADR-0013 sibling mode, out of
  primary chrome). `GetStartedFallback` keeps its exact role (probe threw) as the card's error
  state, preserving the fbf0/m6 probe-retry flows (`App.tsx:368-525`). Passive reads only — no
  provider egress (ADR-0028); "Credential presence alone never means Ready" copy discipline.
- **FILES:** `src/components/PreflightCard.tsx` (new), `src/App.tsx` (capture-idle panel),
  `src/components/{ConversationModeControl,GetStartedFallback}.tsx` (re-home),
  `NowStrip` (drop transitional controls), `en.json`+`pt.json` (budget ≈ 12), App tests.
- **ACCEPTANCE:** first-run and probe-failure paths regression-free (probe retry, unreadable
  hint, sample preview, handoff banner); no new invoke calls on the idle path beyond today's
  probe; zero E2E edits; parity green; screenshots idle/degraded 1440/1024 both themes.
- **SIZE:** M. **DEPS:** R3, R4.

### R6 — Reading + Inspect surfaces adopt the recipes (T6 re-cut)

- **WHAT:** T6 (judge ticket) executed against the new homes: NotesPanel (one `.ag-panel-head`,
  body 14px/`--leading-base` per D6, cards `.ag-card[data-elevation=flat]`), LiveTranscript
  (speaker names `.ag-label`, ghost buttons `.ag-btn-micro`), SeekTimeline as the Timeline lens
  (tick labels `.ag-label` tabular-nums, 2px `--accent` playhead; retire `text-[8px]` at
  `SeekTimeline.tsx:337`), SessionDataRoutePanel + ProjectionRuntimeStatusPanel + TokenUsagePanel
  in the Route lens / System drawer (dt → `.ag-label`, dd → 13px tabular-nums, pills →
  `.ag-chip[data-tone]`; diagnostics stay dense per D6). Retire every sub-11px site (21×
  `text-[9px]` + the 8px). No status-chip animation.
- **FILES:** `src/components/{NotesPanel,LiveTranscript,SessionDataRoutePanel,ProjectionRuntimeStatusPanel,TokenUsagePanel,SeekTimeline}.tsx`,
  `KnowledgeGraphViewer` legend chips only.
- **ACCEPTANCE:** grep gate zero `text-[9px]`/`text-[8px]` in `src/components`;
  `SessionDataRoutePanel` copy byte-identical (ADR-0034 non-engagement in PR body); component
  suites green; zero i18n churn; CI E2E; screenshots of each lens 1440/1024 both themes.
- **SIZE:** L. **DEPS:** T2/T3, R4 (homes settled). Parallel-safe with R7 (different files).

### R7 — Compact-tier drawers + `useShellLayout` (the 200%-zoom unit)

- **WHAT:** `useShellLayout()` (matchMedia tiers, generalizing
  `useSettingsController.tsx:1614-1621`): `wide ≥1280` rail+content+aside pinned; `standard
  1024–1279` aside → right drawer; `compact 768–1023` rail and aside both focus-trapped drawers.
  Honest justification is **WCAG 1.4.4**: 200% zoom in the default 1400px window ≈ 700px CSS
  width lands in `compact` — the drawer work *is* the zoom work — plus 19c7's accepted criterion
  "narrow layouts use contextual drawers rather than one long diagnostic stack". Drawers are
  hand-rolled (`useFocusTrap` + Escape, D5: no Radix dialog). **No `stack` tier (<768px)** — cut
  per Design 3's own weakness analysis; it belongs to the 8055 mobile epoch.
- **FILES:** `src/hooks/useShellLayout.ts` (new), `src/App.tsx` regions,
  `src/styles/layout.css`, drawer chrome, hook tests.
- **ACCEPTANCE:** keyboard-only traversal of both drawers; 200% zoom manual check ≡ compact;
  screenshots 1440/1024/768 dark+light (the 19c7 screenshot matrix); zero E2E edits (the suite
  runs at default window size); reduced-motion respected.
- **SIZE:** M. **DEPS:** R4 (destination shell), R3 (System drawer pattern to reuse).

### R8 — Settings bridge (T7, committed per D4)

- **WHAT:** Unchanged from the judge's T7 text: bring `settings.css` (1731 lines, 234 `var(--`,
  364 raw px) onto the token/recipe system; adopt `.ag-field`/`.ag-chip`/`.ag-card` where
  `settings/Badge` established the pattern; retire the competing font ramp onto the T3 scale;
  rail IA untouched; 720px Settings breakpoint preserved. Re-cut delta: sequencing only — the
  "recipes proven on cheaper surfaces first" gate is now R3+R6 instead of old T5/T6.
- **FILES:** `src/styles/settings.css`, `src/components/settings/*` (~18 files),
  `settings/Badge.tsx` (absorbed into `.ag-chip`).
- **ACCEPTANCE:** all 121 `SettingsPage.test.tsx` cases green (4 class-based selectors
  re-pointed, not deleted); undefined-token gate green; parity untouched; `var(--` count strictly
  up / raw px strictly down, reported in PR body; screenshots 1440/1120/900/720 both themes.
- **SIZE:** L. **DEPS:** T2/T3, R6 (recipes proven). Parallel-safe with R7/R9 (disjoint files).

### R9 — Temporal spine (decide-gated, severable)

- **WHAT:** ADR-0030's named visual motif on data that already exists: a thin event-mark lane in
  the Sessions detail header (from `sessionTimeline` + `sessionProjectionEvents`, the same data
  `SeekTimeline` renders) and a 1px live-progress lane in the NOW STRIP during capture. Event
  marks only — "not a decorative fake waveform" is the ADR's own constraint.
- **FILES:** `src/components/TemporalSpine.tsx` (new), Sessions detail header, `NowStrip`.
- **ACCEPTANCE:** marks correspond 1:1 to real events (no interpolation); zero new i18n beyond
  labels; zero E2E edits; reduced-motion static.
- **SIZE:** S/M. **DEPS:** R2, R3. **Maintainer decide item M4** (in-wave vs. follow-up seed).

### Dependency graph

```
T1 (in flight) ──► T2 ──► T3 (shell-independent, land first)
R0 (ADR) ────────────────────────────────┐
T1 ──► R1 ──► R2 ──► R3 ──► R4 ──► R5    │  R4 requires R0+R2+R3
        (T2/T3 gate R2/R3 chrome)  ├──► R6 ──► R8
                                   ├──► R7
                                   └──► R9 (decide)
```

Smallest honest count: 9 committed units + 1 decide-gated. R2/R3/R6/R8 are the L's; everything
id-or-spec-shaped is quarantined in R4.

---

## 3. Draft ADR-0030 amendment (for the conductor's ADR process — lands as R0)

> Draft only. Number = next free at landing time (0046 unless T2's recipe-layer amendment takes
> it). MADR 3.0. ADR-0030 itself stays `accepted` and gains a note banner pointing here, the
> ADR-0035→0028 pattern.

```markdown
---
status: proposed
date: 2026-08-20
deciders: [AudioGraph maintainers]
---

# ADR-00XX: Collapse the Shell to Capture and Sessions with a Persistent
# Active-Session Strip (amends ADR-0030)

## Context and Problem Statement

ADR-0030 accepted Ready, LiveNow, Review, and Inspect as the MVP shell's
information architecture. Only the tab labels ever migrated: the shell still
exposes three peer tabs (`during`/`after`/`analysis`), Inspect is still a
sibling tab despite ADR-0030's own text ("not a peer primary product mode"),
and Ready/LiveNow are one tab whose label swaps on `isCapturing` — they were
never a navigable choice. The 2026-08-20 design panel verified the resulting
product gap: `stopCapture` never selects the finished session and the only
path to a recording is a modal (`store/index.ts:2157`, `App.tsx:441-445`) —
the product's noun (a session) has no destination. The maintainer has ratified
a full recomposition over the panel's graft-only recommendation.

This record amends ADR-0030's workspace *structure* while preserving its
decision drivers, lifecycle contract (ADR-0028), route-evidence rules
(ADR-0034), and visual direction.

## Decision Drivers

All of ADR-0030's drivers, plus two the label-only migration exposed:

- The finished recording must land somewhere the user can see.
- The IA must express list→detail with one primary action — the only shape a
  future mobile/wearable client (seed audio-graph-8055) can inherit.

## Considered Options

1. Keep the accepted four peer workspaces, implemented literally as four tabs.
2. Two destinations — Capture and Sessions — with a persistent NOW STRIP and
   contextual lenses/drawers.
3. Graft-only: Sessions rail+detail inside the existing Review tab, no
   structural change (the panel's recommendation).

## Decision Outcome

Chosen option: **two destinations + strip + lenses**, because it delivers what
ADR-0030 already argued: Ready/LiveNow are not a user choice (`isCapturing`
picks), Inspect "is not a peer primary product mode", and the lifecycle strip
is ADR-0028's "compact active-session control" made permanent.

ADR-0030's four names map, none are deleted:

| ADR-0030 workspace | Where it lives now |
|---|---|
| Ready   | Capture destination, idle state: preflight card (sources, planned route, storage) + one Start |
| LiveNow | Capture destination, capturing state: notes primary, transcript/assist aside; Stop/health/route/durability on the strip |
| Review  | Sessions destination: list rail → detail; Stop selects the just-ended session |
| Inspect | Contextual lenses on a session (Timeline/Graph/Route/Ask) + the System drawer (projection runtime, token usage, per-stage pipeline detail) |

Structural consequences, stated explicitly because they are contract changes:

- Workspace tab ids become `#workspace-tab-capture` / `#workspace-tab-sessions`
  (panels likewise); the `analysis` tab is removed. `e2e/specs/shell.e2e.ts`
  and `App.contract.test.tsx` are rewritten in the same landing as the rename,
  and in no other landing.
- Navigation state becomes one serializable object (store slice `shellNav`),
  not seven flags.
- The NOW STRIP owns Start/Stop, elapsed, durability, composite health, and
  the route chip. Ready labels the route **planned**; **observed** appears
  only when session-scoped data-movement evidence exists (ADR-0034) — until
  the audio-graph-70a3/51e0 ledger surfaces active-route state, the live chip
  stays planned-labeled.
- One Start on the strip composes the existing gated actions
  (`start_capture`, then transcribe only where ADR-0033's enablement gate
  already permits). It claims no atomicity; ADR-0028's coordinated Start
  (seed audio-graph-10ff) replaces the composition behind the same button
  when it lands.
- Validation matrix restated for the new shape: Capture-idle, Capture-live,
  Sessions-list, Sessions-detail(+each lens) at 1440/1024/768, light+dark,
  idle/loading/empty/degraded/recovery/error, keyboard-only, 200% zoom
  (== compact tier), forced colors, reduced motion, NVDA, VoiceOver,
  packaged three-OS smoke. Nothing below 768px is claimed.

### Consequences

- Positive: recordings become objects with a destination; Stop has a landing.
- Positive: the IA is expressible on a phone tab bar or a watch (shape only —
  no mobile client is claimed; ADR-0030's storage/lifecycle contracts are
  untouched).
- Positive: diagnostics stop competing with reading surfaces without losing
  reach (drawer + lenses).
- Negative: every E2E/contract id fact changes once, in one landing; saved
  workspace state and screenshots need one migration.
- Negative: two-destination chrome must carry the ADR-0028 active-session
  strip on every width tier or the lifecycle becomes invisible.
- Neutral: realtime speech-to-speech remains outside the shell (ADR-0013).

## Pros and Cons of the Options

### Four peer tabs, literal reading
- Good: no contract churn beyond labels (already paid).
- Bad: contradicts ADR-0030's own Inspect clause; Ready/LiveNow tabs are
  fake choices; the session still has no destination.

### Two destinations + strip + lenses (chosen)
- Good: aligns structure with the accepted product argument; one rename event.
- Good: list→detail + one action is the durable mobile-ready shape.
- Bad: one expensive, carefully-sequenced rename/E2E landing (mitigated: it
  is quarantined as pure deletion+rename after Sessions and the strip exist).

### Graft-only (panel recommendation)
- Good: ~70% of the product delight at ~30% of the risk; zero id churn.
- Bad: permanently entrenches the pipeline-stage tabs ADR-0030 diagnosed;
  ratification 2026-08-20 explicitly chose against it.

## More Information

Amends ADR-0030 (structure); lifecycle ADR-0028; evidence ADR-0034; provider
gates ADR-0033. Design basis: workflow wf_993ac5be-0aa Design 3 +
docs/agentic-runs/2026-08-20-ui-overhaul-design/{ui-design,recomposition-plan}.md.
Execution seed: audio-graph-19c7.
```

---

## 4. Blocker disposition (19c7's `blockedBy`: 10ff, 8d18, 50e3)

### audio-graph-10ff — atomic one-action start with rollback (P0, Critical)

**Blocked by five backend seeds, chain depth 2, none closed:** `da33` (in_progress —
ui_selectable enforcement, residual acceptance is invoke-coverage evidence), `a339` (open —
storage-full as non-dismissible state), `b5ef` (in_progress — capture-start acknowledgement;
itself blocked by `fd9f`, in_progress, whose residual acceptance is Windows/macOS CI evidence +
release dry run on the pinned rsac v0.4.4), `90f3` (in_progress — canonical-log durability),
`4521` (in_progress — atomic session rotation). This is the ADR-0028 backend hardening track;
none of it is frontend work and none of it is accelerated by the shell.

**Disposition: proceed in parallel; keep the edge for closure.** The recomposition's "one Start"
(R3) is a frontend composition of today's actions behind today's gates — exactly what clicking
Start already does, plus a transcribe leg behind the existing ADR-0033 predicate. It makes **no
new atomicity or Live-means-durable claim** (the strip reads the same `isCapturing` flag the
ControlBar reads now), so no ADR-0028 acceptance is weakened by building the shell first. Because
the strip owns Start, swapping the composition for 10ff's coordinated backend command later is a
one-site change. Recommended seed mechanics for the conductor: shell units R1–R9 land under 19c7
while 10ff stays open; **19c7 is not closed until 10ff lands and the strip's Start routes through
the coordinated command** — 19c7's "state transitions are deterministic" acceptance genuinely
needs the acknowledged lifecycle.
**MAINTAINER DECIDE (M1):** only if the maintainer wants 19c7 *closed* before 10ff — that would
weaken the deterministic-transitions acceptance inherited from ADR-0028/0030 and is not this
plan's call. Recommendation: keep the edge, close in order 10ff → 19c7.

### audio-graph-8d18 — during-capture data-route badge (blocked by 51e0 ← 70a3)

**Chain:** `8d18` ← `51e0` (open — session data route UI; note: `SessionDataRoutePanel` already
ships the *historical* report, so 51e0 is partially superseded by landed code) ← `70a3` (open —
backend data-movement ledger; `src/generated/sessionDataMovement.ts` exists, but **no
active-route state is exposed anywhere in the store, types, or generated code — verified by
grep this run**). The observed-route badge cannot be built honestly today.

**Disposition: split planned/observed; sequence the planned half in, keep the observed half
blocked.** R3 ships the strip's route chip **planned-labeled** (settings-derived, the same read
`hasConfiguredDurableNotesRoute` performs; no egress, no evidence claim — ADR-0034 not engaged).
That is compliant with ADR-0030's rule ("label a route observed only when session-scoped
data-movement evidence proves it") and delivers the glanceable-route UX now. `8d18` stays open,
still blocked by `51e0`, retargeted by the conductor as: "upgrade the NOW STRIP route chip from
planned to observed (active providers + did-content-leave-device + consent-blocked state) when
the ledger exposes active-route state; no transcript content in the chip." Conductor should also
have `51e0` re-scoped against landed code (its historical half appears done).
**MAINTAINER DECIDE (M2):** does 19c7 close with a planned-labeled chip, or wait for the
observed upgrade? 19c7's text promises Live shows the "exact data route", and ADR-0030's LiveNow
names the *observed* route — closing on planned-only weakens that. Recommendation: 19c7 closure
waits on 8d18 (keep the edge), while shell units land in parallel — same shape as M1.

### audio-graph-50e3 — ambient composite health dot (open, no blockers)

**Disposition: sequence in — folded into R3.** The strip's health chip is the composite dot;
per-stage detail moves to the System drawer and error states re-expand the footer; announce
transitions only. 50e3's acceptance ("no regression to Analysis diagnostics") is satisfied by
the drawer + R2's lenses carrying full detail. The conductor can close 50e3 when R3 lands, citing
R3's acceptance evidence. No decide item — this strengthens, not weakens, the seeded scope.

---

## 5. Additional maintainer decide items

- **M1 / M2** — 19c7 closure gating vs 10ff and vs the observed-route chip (above).
- **M3 — old-shell restyle debt is deliberately skipped.** T5's ControlBar/switcher restyle and
  the PopoverOverlay anchor fix are *not* re-implemented on the old shell; R3/R4 replace those
  surfaces outright. Consequence: between now and R3 landing, the top bar keeps today's known
  defects (20px settings glyph, undefined `__separator`/`__group-label` classes, popover overlap
  ≤1120px). If R3 is more than ~2–3 weeks out after T3, the maintainer may want a throwaway
  S-sized cosmetic patch; recommendation is to skip it and let R3 delete the surface.
- **M4 — temporal spine (R9) in-wave or follow-up.** ADR-0030 names the motif; 19c7's acceptance
  does not require it. Recommendation: file the seed now, land after R6 if the wave has budget,
  otherwise it survives as a clean follow-up (it touches only additive surfaces).
- **M5 — focus-trap depth gap (new seed, not a decision blocker).** `useFocusTrap` does no
  background `aria-hidden`/`inert` or scroll-lock (Design 2's verified finding). D5 rejected
  `@radix-ui/react-dialog`, and R3/R7 add two more drawer consumers of the hand-rolled trap.
  Conductor should file "harden useFocusTrap with background inert + scroll-lock" as its own
  a11y seed rather than smuggling it into a recomposition unit.
- **M6 — retired scope, for the record:** `stack` tier (<768px) cut per Design 3's own weakness
  (belongs to 8055); vendored font stays out per D2 (revisit as a reversible S experiment after
  R6, judged on screenshots); cross-session knowledge exploration not designed
  (`graphSnapshot` is session-scoped by construction — a UI promise would be a lie);
  per-session view isolation (`store/index.ts:2100`) remains a store-track dependency, shimmed
  by R1, not delivered; ExpressSetup internals untouched (seed 75a1 owns onboarding).

---

## 6. Seed-ready ticket texts

Parent for all: `audio-graph-19c7` (which stays the umbrella; conductor may label `epic-f318`
for continuity with T1–T3). Texts follow the 8d89/b9dc/99aa house style.

**R0 — "SHELL-R0: ADR amendment — Capture|Sessions shell with persistent active-session strip"**
> Land the ADR-0030 amendment (docs/agentic-runs/2026-08-20-ui-overhaul-design/recomposition-plan.md §3) as its own decision record at the next free ADR number, add the amended-by note banner to docs/adr/0030 (ADR-0035→0028 pattern), update the ADR index. Docs only — zero src/ or e2e/ diff. BINDING: this record must be merged before any unit renames a workspace tab id or touches e2e/specs/shell.e2e.ts. ACCEPTANCE: ADR process complete; verify:fast green. Size S, no code deps. Parent audio-graph-19c7.

**R1 — "SHELL-R1: ShellNav store slice + SessionViewProvider shim + region extraction (contract-neutral)"**
> Introduce src/store/shellNav.ts: one typed nav object ({dest:"capture"|"sessions", sessionId, lens} + drawer) replacing App-local workspaceView (App.tsx:297) and absorbing loadedSessionId/rightPanelTab/sessionsBrowserOpen/agentOverlayOpen/tokenOverlayOpen/settingsOpen routing; during this unit it still derives and drives the existing during/after/analysis shell — ids, classes, labels, DOM byte-identical. Add SessionViewProvider + useSessionView() (transcriptSegments, graphSnapshot, materializedNotes, sessionTimeline, sessionProjectionEvents; defaults to global store) with one-line reads in NotesPanel/LiveTranscript/KnowledgeGraphViewer/SeekTimeline — zero behavior change, so later per-session store isolation (store/index.ts:2100) never reopens panels. Extract App.tsx's four regions (banners+chrome / destination bar / rail-content-aside / footer) rendering today's content. HONEST CONSTRAINTS: nav must live in the store because stopCapture (a store action) must route in R2 — App-local state cannot deliver that. ACCEPTANCE: App.contract.test.tsx green UNTOUCHED; git diff --stat e2e/ empty; full vitest + parity green; zero visual diff; CI E2E green with zero spec edits (this unit exists to prove the refactor is contract-neutral). Size M. Depends on audio-graph-8d89 (T1). Parent audio-graph-19c7.
>
> **LANDED (audio-graph-59fb):** only workspaceView actually moved (→ nav.dest/nav.lens); the other
> six flags and the `+ drawer` shape are explicitly deferred (see §R1's "LANDED STATUS" note above
> for the full disposition and the setNavDest/lens sharp edge R2 should know about) — absorbing
> them remains R2/R3's job, not done here. Tests for the slice landed in a follow-up fix pass:
> src/store/shellNav.test.ts, src/session/SessionViewProvider.test.tsx.

**R2 — "SHELL-R2: Sessions destination — list→detail with lenses; Stop lands on your own session"**
> Inside #workspace-panel-after with all three tab ids intact: SessionsBrowser modal → rail(list)+detail. List: search + persisted SessionSortMode, SessionMetadata rows (title/relative time/duration/counts/state chip via .ag-chip[data-tone], extensible to finalizing/blocked per ADR-0035/0036); export/trash/restore/permanent-delete → row overflow on @radix-ui/react-popover (ratified D5); Trash filter. stopCapture (store/index.ts:2157-2177) reads get_session_id (already exists, :1979) before stopping, sets nav={dest:sessions, sessionId, lens:notes}, calls listSessions(); optimistic "finalizing" row if the index lags (1d92 gap). Detail lens tabs (same roving-tablist pattern as the right-panel pair): Notes(default)/Transcript/Timeline(SeekTimeline promoted to full lens)/Graph(lazy)/Route(SessionDataRoutePanel unchanged); ChatSidebar becomes an Ask aside lens on Notes/Graph. Live-locked detail reuses sessions.reviewLockedWhileLive verbatim — concurrent Live+Review NOT delivered. Manual synthesis moves to detail overflow as "Generate prose summary" (retarget the data-notes-synthesize shortcut, NotesPanel.tsx:118). Fold e7e5: one hours-aware formatter in utils/format.ts; delete SessionsBrowser.tsx:77 local copy; align ProjectionRuntimeStatusPanel formatAgeMs. Sample preview renders as ephemeral detail. SessionsBrowser stops being lazy — verify force-graph chunk still defers via bun run build:analyze. HONEST CONSTRAINTS: analysis tab keeps its graph/diagnostics until R4 (deliberate interim duplication of reach, not state); SessionDataRoutePanel copy byte-identical (ADR-0034 non-engagement in PR body); recipes from T2/T3 only, no parallel styles. ACCEPTANCE: stop leaves the user viewing the just-ended session's notes; git diff --stat e2e/ empty; App.contract.test.tsx green untouched; restore/trash/export/delete + session restore regression-free; SessionsBrowser.test.tsx rewritten not deleted (d19f case preserved); shared-formatter test; en+pt same commit, parity green; full vitest + CI E2E. Size L. Depends on SHELL-R1, audio-graph-b9dc, audio-graph-99aa. Parent audio-graph-19c7.

**R3 — "SHELL-R3: NOW STRIP + one Start + System drawer + composite health (closes 50e3)"**
> ControlBar → NOW STRIP: brand 16px/600 --text-primary; Start/Stop with controlBar.start/stop aria-labels VERBATIM (E2E-pinned); elapsed tabular-nums; durability readout; planned-route chip (settings-derived per the hasConfiguredDurableNotesRoute read, App.tsx:111 — labeled "planned", never "observed", ADR-0030/0034; verified this run: no active-route state exists in store/types/generated, so observed is impossible today and remains seed 8d18); composite health chip opening the System drawer; settings/sessions cluster right. One Start = start_capture then startTranscribe() ONLY if the existing transcribeDisabled predicate is false (ADR-0033 gate) — VERIFY BEFORE CODING that CI issues exactly start_capture: unmocked commands reach real Rust (shell.e2e.ts:186) and a stray invoke's ERROR line breaks gate 5. Transcribe/Gemini leave primary chrome (ConversationModeControl + Gemini remain as demoted ghost controls until R5 — named transitional state). System drawer (hand-rolled useFocusTrap+Escape per D5): ProjectionRuntimeStatusPanel + TokenUsagePanel + per-stage pipeline detail; PopoverOverlay RETIRED (deletes the ≤1120px anchor-overlap bug). 50e3 fold: PipelineStatusBar collapses to composite during healthy capture, per-stage on error + always in drawer; announce transitions only. Workspace switcher + .workspace-switcher__state untouched below the strip until R4. ACCEPTANCE: CI E2E mandatory — Start/Stop labels verbatim, "Live session" state flip, .notification--error on rejected start, zero ERROR-level frontend lines (merged-start proof), zero secret shapes, git diff --stat e2e/ empty; exactly one saturated element in the idle strip (screenshots 1440/1120 both themes); 50e3 acceptance folded (no regression to diagnostics = drawer + R2 lenses); parity green. Closes audio-graph-50e3 on land. Size L. Depends on SHELL-R1, SHELL-R2, audio-graph-b9dc/99aa. Parent audio-graph-19c7.

**R4 — "SHELL-R4: destination collapse + tab-id rename + E2E/contract rewrite (one landing)"**
> The only unit allowed to touch e2e/. Precondition: SHELL-R0 (ADR) merged. WORKSPACE_VIEWS (App.tsx:216) → ["capture","sessions"]; ids #workspace-tab/panel-{capture,sessions}; DELETE the analysis tab, panel (App.tsx:919-949) and right-rail composition (:680-731) — every occupant already lives in R2 lenses / R3 drawer, so this unit is near-pure deletion+rename. Move .workspace-switcher__state onto the strip keeping class name AND workspace.stateLive text verbatim (E2E test 4 then needs zero edits). graphEdgeFocus bridge (App.tsx:350-360): setWorkspaceView("analysis") → nav.lens="graph" same session (ADR-0026 hook, smaller side effect). Skip-link → #workspace-panel-{dest}; "destination entered" announcement; roving handler is already length-generic. SAME LANDING: rewrite shell.e2e.ts tests 1+3 (first-paint → #workspace-tab-capture; 3-tab wraparound :250-294 → 2-tab equivalent) and the id/wraparound facts in App.contract.test.tsx; tests 2/4/5/6 zero edits by construction. i18n: add workspace.capture/sessions, retire during/after/analysis, both locales; rename-only commit separate from add-only. ACCEPTANCE: red E2E has exactly one possible cause (single-cause by construction); full CI E2E; ~9 IA-shaped App.test.tsx cases rewritten, 32/41 untouched; parity green; screenshots 1440/1024 both themes. Size M. Depends on SHELL-R0, SHELL-R2, SHELL-R3. Parent audio-graph-19c7.

**R5 — "SHELL-R5: Ready preflight card + mode-control relocation"**
> Capture-idle stops being an empty cockpit: preflight card (.ag-card/.ag-field) — Sources(n)/Route(planned: ASR→LLM)/Storage(StorageBanner data) rows each pass/fail with a fix action, then one Start session button (same strip action). ConversationModeControl moves in as "Mode: Notes/Converse"; Gemini toggle moves with it (ADR-0013 sibling mode out of primary chrome); GetStartedFallback keeps its exact probe-failure role as the card's error state (preserve fbf0/m6 retry + unreadable flows, App.tsx:368-525). Passive reads only, no provider egress (ADR-0028); credential presence alone never reads as Ready. ACCEPTANCE: first-run/probe-failure/sample-preview/handoff flows regression-free; no new invoke on the idle path; zero e2e/ diff; parity green; idle+degraded screenshots 1440/1024 both themes. Size M. Depends on SHELL-R3, SHELL-R4. Parent audio-graph-19c7.

**R6 — "SHELL-R6: reading + Inspect surfaces adopt the recipes (T6 re-cut to new homes)"**
> Former UI-T6 executed against the recomposed shell: NotesPanel (one .ag-panel-head, body 14px --leading-base per D6, cards .ag-card[data-elevation=flat]); LiveTranscript (.ag-label speakers, .ag-btn-micro ghosts); SeekTimeline as the Timeline lens (.ag-label tabular-nums ticks, 2px --accent playhead, retire text-[8px] :337); SessionDataRoutePanel/ProjectionRuntimeStatusPanel/TokenUsagePanel in Route lens + System drawer (dt→.ag-label, dd→13px tabular-nums, pills→.ag-chip[data-tone]; diagnostics stay dense per D6); KnowledgeGraphViewer legend chips only. Retire all 21 text-[9px] sites. No status-chip animation. HONEST CONSTRAINTS: SessionDataRoutePanel copy byte-identical (ADR-0034; state non-engagement in PR body); zero i18n churn. ACCEPTANCE: grep gate zero text-[9px]/text-[8px] in src/components; component suites green; CI E2E; lens screenshots 1440/1024 both themes. Size L. Depends on audio-graph-b9dc/99aa, SHELL-R4. Parallel-safe with SHELL-R7. Parent audio-graph-19c7.

**R7 — "SHELL-R7: compact-tier drawers + useShellLayout (WCAG 1.4.4 / 200% zoom)"**
> useShellLayout() matchMedia tiers generalizing useSettingsController.tsx:1614-1621: wide ≥1280 (rail+content+aside pinned), standard 1024–1279 (aside → right drawer), compact 768–1023 (rail+aside focus-trapped drawers). Honest justification: 200% zoom in the 1400px default window ≈ 700px CSS ⇒ compact — the drawer work IS the zoom work — plus 19c7's accepted "narrow layouts use contextual drawers rather than one long diagnostic stack". Drawers hand-rolled on useFocusTrap+Escape (D5: no Radix dialog). NO stack tier <768px (8055 territory, cut per Design 3's own weakness). ACCEPTANCE: keyboard-only drawer traversal; manual 200%-zoom ≡ compact check; screenshots 1440/1024/768 dark+light (the 19c7 matrix); zero e2e/ diff; reduced-motion respected. Size M. Depends on SHELL-R4. Parent audio-graph-19c7.

**R8 — "SHELL-R8: Settings bridge (T7, committed per D4)"**
> Unchanged scope from the judge's T7: settings.css (1731 lines, 234 var(--, 364 raw px) onto tokens; .ag-field/.ag-chip/.ag-card where settings/Badge established the pattern (Badge absorbed); competing font ramp onto the T3 scale; rail IA untouched (2026-06-29 refactor); 720px breakpoint preserved; no copy changes (ExpressSetup English is a separate translation seed). Re-cut delta: "recipes proven first" gate is now SHELL-R3+R6. ACCEPTANCE: all 121 SettingsPage.test.tsx cases green (4 class selectors re-pointed, not deleted); undefined-token gate green; parity untouched; var(-- strictly up / raw px strictly down reported in PR body; screenshots 1440/1120/900/720 both themes. Size L. Depends on audio-graph-99aa, SHELL-R6. Parallel-safe with SHELL-R7/R9. Parent audio-graph-19c7.

**R9 — "SHELL-R9: temporal spine (decide-gated)"**
> ADR-0030's named motif on existing data: thin event-mark lane in Sessions detail header (sessionTimeline + sessionProjectionEvents — the data SeekTimeline already renders) and a 1px live-progress lane in the NOW STRIP during capture. Event marks only, never a decorative waveform (ADR-0030's own constraint). ACCEPTANCE: marks 1:1 with real events; reduced-motion static; zero e2e/ diff; parity green. Size S/M. Depends on SHELL-R2, SHELL-R3. GATED on maintainer decide item M4 (in-wave vs follow-up). Parent audio-graph-19c7.

**Retarget (conductor action, not a new seed):** `audio-graph-8d18` → "upgrade the NOW STRIP
route chip planned→observed when 51e0/70a3 expose active-route state; exact providers +
did-content-leave-device + consent-blocked; no transcript content" (stays blocked by 51e0; ask
for 51e0 re-scope against landed `SessionDataRoutePanel`). **New seed (M5):** harden
`useFocusTrap` with background `inert`/`aria-hidden` + scroll-lock (a11y, S/M, no blockers).
