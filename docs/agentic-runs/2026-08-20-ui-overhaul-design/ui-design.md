# UI overhaul — design judgment and synthesis

Judge run, 2026-08-20, against `a4265de`. Three candidate directions were produced by independent
design agents: **D1 "Legible Deck"** (polish-in-place), **D2 "Deck Kit"** (design-system rebuild on
Radix), **D3 "session-first shell"** (product-led IA redesign). This document records the
verification of their load-bearing claims, the scoring against the five judging criteria, and the
synthesized recommendation. The maintainer ratifies the direction; nothing here is built yet.

---

## 1. Fact verification (repo state at `a4265de`)

Every claim below was checked directly against the working tree, not taken from the design text.

### Claims all three designs agree on — all confirmed

| Fact | Verdict |
|---|---|
| ControlBar responsive wrap fix landed (seed 88e2) | **Confirmed** — `src/styles/layout.css:213-265`, `@media (max-width:1120px)` block with `flex-wrap`, `order:`, `flex-basis:100%` |
| CSP is `default-src 'self'`, no `font-src`, `data:` only under `img-src` | **Confirmed** — `src-tauri/tauri.conf.json:23`. Self-hosted woff2 needs no CSP edit; inlined `data:` fonts would be blocked |
| No `minWidth` in the Tauri window config (1400×900 default) | **Confirmed** — `tauri.conf.json:15-17` |
| E2E pins `.workspace-switcher__state` containing "Live session" | **Confirmed** — `e2e/specs/shell.e2e.ts:342-346` |
| Seeds `audio-graph-19c7` (shell rewrite), `audio-graph-e7e5` (duration formatters), `audio-graph-8055` (mobile arch session) exist and are open | **Confirmed** — `.seeds/issues.jsonl` |

### Design 1 — every spot-checked claim exact

| Claim | Verdict |
|---|---|
| 1.4.11 contrast table (border/surface ratios) | **Confirmed digit-for-digit** by independent computation: dark border-on-elevated **1.11**, dark tertiary-on-secondary **1.12**, light border-on-tertiary **1.20**, dark border-on-tertiary **1.23**, dark border-on-secondary **1.37**, light border-on-primary **1.41**. Light `--bg-elevated` = `--bg-primary` = `#ffffff` → **1.00:1** (`src/styles.css:242,245,327,330`) |
| `@layer theme, base, components, utilities` declared; `components` layer empty | **Confirmed** — declaration at `src/styles.css:25`; zero rules anywhere target `@layer components`. The insertion point exists and is unused |
| Five undefined-token fallbacks in `settings.css` | **Confirmed at the exact lines** — `:413 var(--bg-hover,…)`, `:417 var(--accent-color,#4c8bf5)`, `:854 var(--surface-muted,…)`, `:881 var(--surface-raised, var(--surface,#fff))`. None of the five custom properties is defined anywhere in `src/`. The `:881` rule styles `.settings-credential-health__overflow-menu` (`CredentialsPanel.tsx:430`) → **live white-popup-in-dark-theme defect** |
| `.control-bar__settings-btn` at 20px + opacity trick | **Confirmed** — `settings.css:1718-1731` (`font-size:20px`, hover `opacity:1`) — larger than the 18px h1 |
| `.control-bar__separator` / `__group-label` / `__comparing` used but defined in no stylesheet | **Confirmed** — used in `ControlBar.tsx`, zero hits in `src/styles/` |
| 21× `text-[9px]`, 1× `text-[8px]` | **Confirmed** — 21 exactly; the 8px is `SeekTimeline.tsx:337` |
| `settings.css` 1731 lines, ~26% tokenized, 364 raw px literals | **Confirmed** — 1731 lines, 234 `var(--` references, 364 `px` literals |
| `styles.a11y.test.ts` asserts **only** the `--accent-blue`/`--on-accent-blue` pair | **Confirmed** — `src/styles.a11y.test.ts:33-36`. (This matters: Design 2 mis-states this gate — see below) |
| Corrections to the constraint sheet (88e2 landed, Notifications auto-dismiss landed, `tint-accent-danger` gone) | **All confirmed** — `Notifications.tsx:30 AUTO_DISMISS_MS = 4000` + `humanizeError`; zero hits for `tint-accent-danger` |

### Design 2 — mostly accurate; one inflated gate claim

| Claim | Verdict |
|---|---|
| `PopoverOverlay` hard-pins `fixed top-[52px] right-[12px]` while `layout.css` lets the bar grow (`height:auto; min-height:52px` ≤1120px) → overlap bug | **Confirmed** — `PopoverOverlay.tsx:34` default className; the bug is real |
| Radix substrate already in tree via the Tooltip pilot | **Confirmed** — `@radix-ui/react-tooltip@^1.2.8` in `package.json`; `node_modules/@radix-ui/` carries popper, portal, presence, slot, dismissable-layer, etc. Marginal cost of dialog/popover genuinely low |
| Test suite is role/label-query dominated; only ~7 class-based `querySelector` calls | **Confirmed** — 7 hits across `Button.test.tsx`, `ResizeDivider.test.tsx`, `SettingsPage.test.tsx` |
| "`styles.a11y.test.ts` … asserts **every** declared `--accent-*`/`--on-accent-*` pair ≥4.5:1" | **FALSE** — the test covers only the `accent-blue` pair. D2 uses this inflated gate as a reason hexes are frozen; the actual gate is far narrower. D1's characterization is the correct one, and D1's plan to extend the test to all 7 pairs is the right response |
| Contract-test idea (`App.contract.test.tsx` pinning E2E facts in jsdom) | Sound and cheap — the single best graftable mechanism in D2 |

### Design 3 — sharpest product diagnosis; weakest fact hygiene

| Claim | Verdict |
|---|---|
| `stopCapture` never sets `loadedSessionId`, never navigates → "you record a session and it appears nowhere" | **Confirmed** — `store/index.ts:2157-2177` sets only capture flags; `App.tsx:441-445` routes to `after` only on `samplePreviewActive \|\| loadedSessionId`, which only `SessionsBrowser` Load sets |
| E2E mock bridge falls through to real backend for unmocked commands | **Confirmed** — `shell.e2e.ts` `if (!mockFn) return originalFetch(input, init)` — correctly identified as the constraint on any merged-Start behavior |
| Arrow-key E2E block hard-assumes exactly 3 tabs with `analysis` at index 2 | **Confirmed** — `shell.e2e.ts:278-293` |
| `matchMedia` tier precedent at `useSettingsController.tsx:1614-1621` | **Confirmed** — `:1615-1616` |
| **Unit 0 scopes three already-fixed defects as work**: `bg-(--tint-accent-danger)` (zero hits in repo), SessionsBrowser `var(--border,#333)` ghost (fixed, guarded by the d19f regression test at `SessionsBrowser.test.tsx:166`), Notifications auto-dismiss + raw-TypeError (landed: `AUTO_DISMISS_MS`, `humanizeError`) | **Stale-fact error.** D3's §0 claims its corrections were "verified in code" yet it verified only the ControlBar item and re-scoped the rest. D1 and D2 both independently caught these as fixed. This is the exact defect class D3's own §0 warns about |
| e7e5 "three diverging duration formatters" | **Confirmed** — seed title says three; `utils/format.ts:19`, local copy `SessionsBrowser.tsx:77`, and `ProjectionRuntimeStatusPanel.tsx` `formatAgeMs` mirroring the convention by comment rather than by import |

Theme default is `system` (`src/theme.ts` — `prefers-color-scheme` unless pinned), relevant to the
decisions section.

---

## 2. Scoring against the five criteria

### (1) Delight / adoption per unit of effort

- The **two highest-leverage units in the entire field** are from different designs:
  **D1-U1** (edge/elevation contrast: a measured 1.0–1.4:1 structural invisibility fixed by token
  edits plus a mechanical 57-site sweep — the largest visual delta per diff anywhere) and
  **D3-Unit-2** (Sessions destination + "Stop lands you on your own session" — the largest
  *product* delta; the recording-vanishes-into-a-modal dead end is verified real, and D1's own
  ceiling admits polish cannot fix it).
- D2 spends 2L+4M rebuilding substrate; its own weakness concedes the output is "a nicer-looking
  version of the same app". Its genuinely novel verified wins (PopoverOverlay anchor bug, focus-trap
  `aria-hidden` gap, contract test, font) are all graftable at a small fraction of the plan's cost.
- **Ranking: D1 ≥ D3 (different axes) ≫ D2.**

### (2) Incremental landability

- **D1 is the strongest plan in the field on this criterion**: the recipe layer lands in an
  *already-declared, empty* `@layer components` (verified), so adoption is strictly additive and
  cannot regress a surface until it opts in; units are 4S+4M+1L; every unit is copy-neutral.
- D3 Units 0–3 are landable with ids intact; Unit 4 (tab collapse + id rename + E2E rewrite + ADR
  amendment in one squash) is the stall point its own weakness names — and its Unit 0 contains
  phantom (already-done) work.
- D2's `[data-ui="kit"]` scope is a clever coexistence mechanism, but its own weakness admits the
  partial-migration state "will look worse than either endpoint", and both of its L units sit on
  the critical path.
- **Ranking: D1 ≫ D3 > D2.**

### (3) Survival of pinned contracts

- **D1**: copy-neutral by construction (zero i18n churn), recipes are classes, ids/roles/labels
  untouched, redaction untouched. Adds two *new* gates (1.4.11 edge test, undefined-token test)
  that make its fixes permanent. Best in field.
- **D2**: strong mechanisms (jsdom contract test, `git diff --stat e2e/` empty gate, kit
  prop-forwarding contract) — but one inflated claim about the a11y gate's scope.
- **D3**: structurally the riskiest (id renames, spec rewrite, ADR-0030 amendment) and its own
  weakness correctly identifies that the ADR amendment is mis-sequenced (inside Unit 4 instead of
  before Unit 1).
- **Ranking: D1 > D2 > D3.**

### (4) Honesty of the self-stated weakness

- **D1**: weakness (Settings split-brain; U9 the unit most likely never to land) verified exactly —
  1731 lines / 234 token refs / 364 px literals; first-run path lands on Settings. The weakness is
  structural, correctly attributed to the minimal-diff premise, and comes with the correct remedy
  (promote U9 to committed scope). All three of its constraint-sheet corrections check out. **Best
  fact hygiene of the three.**
- **D2**: weakness (rebuilds substrate when the complaint is composition; no ordering avoids paying
  for pixels twice vs 19c7) is honest and decision-relevant. One factual inflation found
  (a11y-gate scope).
- **D3**: weakness (mobile framing is the weakest load-bearing claim; cut Unit 7; Unit 6 survives
  only as WCAG 1.4.4 zoom work) is genuinely honest — but the design failed the verification test
  elsewhere: three already-fixed defects scoped as Unit 0 work under a "verified in code" banner.
- **Ranking: D1 > D3 ≈ D2** (D3's weakness prose is the most self-critical, but its plan contains
  the field's only outright stale-fact work items).

### (5) Mobile/wearable readiness as an IA constraint

- Only D3 moves this needle: a serializable nav object, list→detail as the session shape, one
  primary action, and a glanceable NOW-STRIP data model (which D3 correctly identifies as the
  entire honest wearable claim). Its own weakness concedes the mobile client will be a separate
  thin app against a backend that doesn't exist — so what is worth buying *now* is the **shape**
  (session as the noun, one primary action, glanceable live state), not the <1024px layout tiers.
- D1 and D2 are explicit non-answers here (acceptably so — the criterion is a constraint, not a
  build target).
- **Ranking: D3 ≫ D1 ≈ D2.**

---

## 3. Synthesis: why the base is D1 with a D3 product graft, not D3 with a D1 skin

Two candidate hybrids were on the table:

**(A) Base D1, graft D3's Sessions-destination core** — vs — **(B) Base D3, graft D1's token work
as its "Unit 5" visual pass.**

(A) wins for four reasons:

1. **The ordering dilemma D2 named dissolves under (A) but not under (B).** D1's recipes are CSS
   classes bound to *semantic roles* (label, chip, card, panel-head), not React components. When
   seed `19c7` eventually recomposes the shell, re-parented DOM keeps its classes — the visual
   investment survives an IA rewrite nearly intact. D3-as-base front-loads the IA rewrite whose
   most expensive step (tab collapse + E2E rewrite + ADR amendment) is precisely the step the
   maintainer may not ratify, and whose stall leaves a second half-migrated shell — the failure
   mode the label-only ADR-0030 migration already produced once (D3's own admission).
2. **D3's one unambiguous product win is separable.** Its Unit 2 explicitly renders inside
   `#workspace-panel-after` with all three tabs and ids intact — it needs neither the tab
   collapse, nor the NOW STRIP, nor the ADR amendment. Grafting it costs one L ticket and zero
   E2E spec edits.
3. **Verified-diagnosis quality tracks plan quality.** D1's diagnosis survived every spot-check
   including independent recomputation of its contrast table; D3 shipped phantom work items. For
   a run where the conductor dispatches implementation agents against ticket text, fact hygiene
   in the base plan is worth a lot.
4. **Contract risk ordering.** (A) defers all id/spec/ADR churn to a later, separately-ratified
   19c7 run; (B) puts it mid-sequence.

**What is taken from D2** (as grafts, rejecting its base): the `App.contract.test.tsx` jsdom
contract-pinning idea (small, protects every later unit, moves the E2E facts into the fast suite);
the PopoverOverlay anchored-popover fix (confirmed bug; `@radix-ui/react-popover` is ~2.5 KB
marginal because the substrate is already in the tree — decision-gated); and its warning that
D1's U9 deferral is the plan's real hole, which converts into the recommendation to promote the
Settings bridge to committed scope.

**What is explicitly not adopted now:** D2's kit (`src/ui/`), `react-dialog`/`react-dropdown-menu`,
D3's tab collapse / id renames / ADR-0030 amendment (stays with seed 19c7, sequenced after the
visual pass, with the ADR amendment as its own pre-decision), D3's NOW STRIP and merged-Start
(19c7 territory), D3's `compact`/`stack` layout tiers (Unit 6 only ever justified as 200%-zoom
work; Unit 7 cut per D3's own advice), and the vendored font (a maintainer taste decision, below —
both sides' CSP analysis verified compatible).

**Residual risks of the chosen hybrid, stated plainly:**
- It inherits D1's ceiling: Inspect stays a peer tab (arguably *entrenched* by making it look
  better), concurrent Live+Review stays impossible (mitigated: the `SessionViewProvider` shim in
  T4 makes the later store fix panel-local), and density remains a product decision (Decision 6).
- The Settings bridge (T7) is still the worst effort-to-payoff unit in the plan; promoting it to
  committed scope (Decision 4) is a budget call only the maintainer can make.
- Two visual grammars coexist for the T2→T6 window (edges/accents land globally, recipes adopt
  per-surface). D1's sequencing keeps each unit visually complete per surface, which bounds but
  does not eliminate the mixed state.

---

## 4. Ticket set (7, seed-ready)

Order: **T1 → T2 → T3 → {T4 ∥ T5} → T6 → T7**. T4 is parallel-safe with T2/T3/T5 (different
files; only merge-order awareness with T6). Full WHAT/ACCEPTANCE/FILES text is in the structured
output; summary:

| # | Ticket | From | Size | Deps |
|---|---|---|---|---|
| T1 | Ghost tokens, z-ladder, and the jsdom contract net | D1-U0 + D2 graft | S | — |
| T2 | Recipe layer + non-text contrast (1.4.11) | D1-U2+U1 (V1–V3) | M | T1 |
| T3 | Accent normalization (OKLCH) + type/radius/motion collapse | D1-U3+U4 (V4–V8) | M | T2 |
| T4 | Sessions destination: Stop lands on your session | **D3-Unit-2 graft** + shim + e7e5 | L | T1 |
| T5 | ControlBar hierarchy + switcher + popover anchor | D1-V9/U5 + D2 popover graft | M | T2, T3 |
| T6 | Reading + Inspect surfaces adopt the recipes | D1-U6+U7 | L | T2, T3 |
| T7 | Settings bridge (decision-gated but recommended committed) | D1-U9, promoted | L | T2, T3, Decision 4 |

Not ticketed (deliberately): 19c7 shell recomposition (own ratification, after T6), vendored font
(Decision 2; one S ticket if approved), anything <1024px (Decision on 200%-zoom scope can ride a
future 19c7 run), mobile/wearable (seed 8055's architecture session owns it).

## 5. Decisions reserved for the maintainer

Six either/or calls, each with a recommendation — see structured output. In one line each:
IA appetite (graft-only now — rec), typeface (system stack + tabular-nums now, font as reversible
follow-up experiment — rec), blue merge (`--accent-blue` → alias of `--accent` — rec), Settings
budget (commit T7 — rec), Radix line (popover only — rec), density (raise reading surfaces only,
keep diagnostics dense — rec).
