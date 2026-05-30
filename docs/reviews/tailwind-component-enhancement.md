# Tailwind Component Enhancement — Options Review & Recommendation

**Date:** 2026-05-30
**Status:** Research + recommendation (one tiny pilot landed; see §7).
**Scope:** How to enhance the UI with component packs / libraries *on top of* the
Tailwind v4 setup, without reversing prior decisions.
**Read first:** [ADR-0016](../adr/0016-adopt-tailwind-v4-incremental.md) (Tailwind
over shadcn, Preflight OFF), [ADR-0009](../adr/0009-design-token-system-and-theming.md)
(hand-authored, contrast-audited token system + theming), `src/styles.css`,
`docs/research/css-modernization-tailwind-shadcn.md`, `docs/reviews/modernization-audit.md`.

---

## 1. The constraints that decide this (non-negotiable)

These are the fixed points every option must fit. They are not preferences — they
are accepted decisions and audited invariants.

1. **No shadcn/ui.** ADR-0016 explicitly rejected it: it would *replace* the
   hand-built, tested, contrast-audited Tabs/Dialog/Buttons this codebase just
   finished, and add runtime deps for **no capability gain**. Not revisited here.
2. **Preflight stays OFF.** `src/styles.css` ships its own audited reset, global
   `:focus-visible` ring (WCAG 2.4.7), `prefers-reduced-motion` collapse (2.3.3),
   `.sr-only`, themed scrollbars, and `color-scheme: dark`. Tailwind is imported as
   `theme` + `utilities` layers only (no `base`). **Any pack that ships its own
   reset, base styles, or assumes Preflight is a non-starter.**
3. **`styles.css` is the single source of truth for the visual layer.** Colors,
   spacing, radius, shadow, z-index, motion are hand-authored tokens bridged into
   Tailwind via `@theme inline` (utilities resolve *through* `var(--token)`). The
   palette is contrast-audited (`docs/reviews/wcag-contrast-audit.md`); a light
   theme is planned as a semantic-token swap. **Any pack that brings its own theme
   system / color palette competes with this and risks breaking the audit + the
   theming plan.**
4. **a11y + promoted lint rules are at `error`.** 9 a11y Biome rules are
   CI-enforced. The app already has correct ARIA tablists, focus-trapped dialogs
   (`useFocusTrap`), `role=log`/`aria-live` regions. Anything added must keep that
   bar — ideally *raise* it where today's affordances are thin (e.g. tooltips are
   currently native `title=`).
5. **Zero/near-zero runtime cost is the norm.** The app has *no UI-framework
   runtime deps* today (only React, zustand, i18next, the graph lib). Tauri offline
   desktop — no CDN. A heavy runtime dep needs to earn its place.
6. **No broad rewrite.** ADR-0016 deliberately stopped migration at the shared
   "component layer." This task is enhancement, not migration.

**Corollary.** The "reduce CSS" framing is settled and false for this app (ADR-0016,
the research doc): styling relocates, it doesn't shrink. So a component pack is only
worth adopting if it adds **behavior/accessibility we don't have**, not styling.

---

## 2. What's actually hand-rolled today (the real demand)

The honest question is *what behavior is currently hand-written and non-trivial to
get right*, because that's the only thing a library can usefully replace:

| Interaction | Where | Current implementation | Gap a library could close |
|---|---|---|---|
| Modal/popover dialogs | `PopoverOverlay.tsx` (+ `useFocusTrap`) used for agent-proposals & token-usage pop-downs in `App.tsx`; Settings/Sessions/Shortcuts/ExpressSetup modals | Hand-rolled scrim + focus trap + Escape + `aria-modal`. **Tested, working.** | Marginal. Trap edge cases (portal, scroll-lock, inert background, return-focus) — but ours is solid + tested. |
| Tablists | Right-panel (`App.tsx`), Settings tabs (`SettingsPage.tsx`), conversation-mode + engine (`ConversationModeControl.tsx`), audio-source scope + AsrProvider tabs | ARIA `role=tablist`/`tab` + manual arrow-key roving (`handleTabKeyDown`). **Tested, working.** | Marginal. Roving tabindex / `aria-orientation` boilerplate, but ours is correct. |
| Tooltips | ~30+ call sites use native **`title=`** (ControlBar, PipelineStatusBar, TokenUsagePanel, AudioSourceSelector…) | Native `title` attribute | **Real gap.** `title` is unstyleable, has ~1s delay, no touch/keyboard-focus parity, can't render rich content, inconsistent across platforms. This is the one place a headless behavior lib clearly *adds* capability. |
| Dropdown / menu | Audio-source group collapse, process-scope toggle, scattered selects | Hand-rolled buttons / native `<select>` | Minor. No true "menu button + roving menu + typeahead" widget exists yet; if one is needed (e.g. an overflow "⋯" menu), a headless `Menu` would help. |
| Combobox / command palette / data table | — | Don't exist | If the roadmap adds them, headless primitives are the right build-vs-buy answer (ADR-0016 §"Where shadcn would help"). |

**Reading:** dialogs and tablists are *done and tested* — replacing them is churn
with regression risk and no gain (same logic ADR-0016 used against shadcn). The
**only clear, present capability gap is tooltips** (native `title=` everywhere).
Menu/combobox are *latent* gaps for future widgets.

---

## 3. The options, scored against §1

Only options compatible with "no reset, no theme system, token-driven, a11y-first"
are realistic. Grouped by category.

### 3a. Headless behavior libraries (styles-free) — the natural fit

These ship **behavior + ARIA + keyboard interaction and NO styles**. You style with
your existing Tailwind-token utilities. They don't touch the reset, don't bring a
palette, don't assume Preflight. This is exactly the seam ADR-0016 left open ("adopt
*individual* primitives à la carte").

| Library | Runtime weight (approx, tree-shaken per-primitive) | React 19 | Styles shipped | Notes vs our constraints |
|---|---|---|---|---|
| **Radix UI** (`radix-ui` unified pkg) | Small per primitive (~3–8 KB gz each); import only what you use | ✅ 19 supported | **None** (unstyled) | Mature, the de-facto standard, excellent a11y, controlled/uncontrolled, portal + `data-state` for styling. Same primitives shadcn wraps — but used **directly + token-styled**, which sidesteps the shadcn objection (no copy-paste source, no `cva`/theme). Tooltip, Popover, DropdownMenu, Tabs, Dialog all available. **Best behavior/size/maturity balance.** |
| **Headless UI** (`@headlessui/react`) | ~20–25 KB gz (less granular; fewer primitives) | ✅ | None | Tailwind Labs' own; pairs idiomatically with Tailwind. **But:** no standalone Tooltip, no Popover-with-anchor positioning as rich as Radix; smaller catalog (Menu, Listbox, Combobox, Dialog, Tabs, Disclosure, Switch). Good if we only needed Menu/Combobox; weaker for the *tooltip* gap. |
| **Ark UI** (`@ark-ui/react`, Zag.js state machines) | Per-component; comparable to Radix | ✅ | None | Largest catalog, framework-agnostic state machines, very robust. Heavier mental model; newer; more than we need. Viable but Radix is the lower-risk incumbent. |
| **React Aria (Components)** (`react-aria-components`) | Larger baseline (Adobe collections/i18n infra; ~tens of KB) | ✅ | None | Best-in-class a11y semantics (Adobe), great for complex collections (table, combobox, date). **But** heaviest runtime + most opinionated render structure; overkill for tooltips and a finite UI. Reach for it only if we build a *data table / date picker / rich combobox*. |

**Verdict within 3a:** **Radix UI**, imported per-primitive. Smallest incremental
cost, unstyled (token-styling intact), React-19-ready, covers the present gap
(Tooltip) and the latent ones (DropdownMenu/Popover/Combobox-via-others). Using Radix
*directly* is **not** shadcn — no copied component source, no `cva`/`clsx`/
`tailwind-merge`, no second theme; we add one dependency and style with our tokens.

### 3b. CSS-class component packs

| Pack | Verdict | Why |
|---|---|---|
| **daisyUI** | ❌ Reject | It's a Tailwind plugin that **adds its own theme system** (`data-theme`, its own CSS-variable palette, semantic color names like `primary`/`base-100`). That **directly competes** with our hand-authored, contrast-audited token layer and the planned light-theme swap (constraint §1.3). It also assumes a Tailwind base/reset flow. Two theme systems = exactly the regression vector ADR-0016 avoided. No a11y/behavior gain (it's CSS-only — no focus management, no ARIA wiring). |
| **Flowbite / Preline / etc.** | ❌ Reject | Same class of problem: ship their own CSS + often a JS layer that assumes Preflight + their tokens. Competing palette, reset assumptions. |

### 3c. Copy-paste markup (no runtime dep)

| Source | Verdict | Why |
|---|---|---|
| **Tailwind Plus / tailwindui patterns** (paid) | ⚠️ Optional, à la carte only | These are **HTML/JSX snippets**, not a dependency or a theme — you paste markup and restyle to our tokens. Zero runtime cost, zero reset/theme conflict. **But** the interactive ones (menus, comboboxes, dialogs) historically lean on Headless UI for behavior, and the colors are Tailwind-palette defaults that must be re-pointed to our tokens + re-audited. Useful as *visual reference / scaffolding* for net-new screens; **not** a behavior solution and **not** worth a license purely for what we have. Pair with Radix if used. |

---

## 4. Recommendation

**Primary direction: adopt Radix UI headless primitives à la carte, styled with the
existing design tokens, starting with the genuine gap (Tooltip) and reaching for
`Popover`/`DropdownMenu`/`Combobox`-class primitives only when a concrete new widget
needs them. Do NOT replace the working, tested Dialog/Tablist implementations.**

Why Radix specifically, against the constraints:

- **Behavior, not styles** → no reset, no Preflight assumption, no competing theme.
  We keep `styles.css` as the single source of truth and style Radix parts with our
  token utilities (`bg-bg-elevated`, `text-text-secondary`, `rounded-md`,
  `shadow-2`, `z-[…]` from the `--z-popover` tier) and `data-[state=…]` variants.
- **Closes the one real gap** (tooltips) with proper focus/hover/touch parity,
  controllable delay, and rich styled content — a measurable a11y/UX upgrade over
  native `title=`, while honoring `prefers-reduced-motion` and our focus ring.
- **Not shadcn** → addresses ADR-0016's objection precisely: we add *behavior
  primitives directly*, we do **not** copy component source or replace the audited
  Button/Dialog/Tabs, and we add **no** `cva`/`clsx`/`tailwind-merge`/theme.
- **Minimal, granular runtime cost** → import only the primitives used (Tooltip ≈ a
  few KB gz). Consistent with "no heavy UI-framework dep" — each primitive earns its
  place when a real interaction needs it.
- **React 19 ready**, mature, the most-vetted a11y implementation available.

**Explicitly not chosen:** daisyUI / Flowbite (competing theme + reset, no behavior
gain), React Aria Components (heaviest; reserve for a future data-table/date-picker),
Headless UI (no standalone Tooltip; smaller catalog), wholesale shadcn (ADR-0016).
Tailwind Plus remains an optional *markup reference* for net-new screens only.

### 4a. Components that would benefit most (priority order)

1. **Tooltips — highest value.** Replace native `title=` at the dense, information-
   carrying call sites: `ControlBar` (capture/mode buttons), `PipelineStatusBar`
   (per-stage status + latency), `TokenUsagePanel` (turns/last-turn hints),
   `AudioSourceSelector` (capture-locked / mode hints), `ConversationModeControl`
   (engine availability hints). Radix `Tooltip` gives consistent styling, keyboard-
   focus + touch parity, and controllable delay. **(Pilot target — see §7.)**
2. **Overflow / context menus (latent).** If a top-bar "⋯" menu or a per-row action
   menu is introduced, use Radix `DropdownMenu` (roving focus, typeahead, `Esc`,
   pointer/keyboard) instead of hand-rolling. Don't retrofit existing toggles.
3. **Rich popovers (latent).** The agent-proposals / token-usage pop-downs work fine
   on `PopoverOverlay` today. *If* anchored positioning (flip/shift near viewport
   edges) becomes desirable, Radix `Popover` is the upgrade path — but it's optional
   and not a regression-justified change now.
4. **Combobox / searchable selects (future).** Provider/model pickers in Settings
   could become comboboxes; Radix has no Combobox, so prefer Headless UI's or Ark's
   for that *specific* future need, or compose Radix `Popover` + a listbox.
5. **Do NOT touch:** the right-panel tablist, Settings tabs, conversation-mode
   segmented control, `PopoverOverlay`/`useFocusTrap` dialogs, `Button`/`IconButton`.
   These are tested, ARIA-correct, contrast-audited, and ADR-protected. Replacing
   them is pure churn (the exact shadcn objection from ADR-0016).

### 4b. Token / theme integration approach

- **No new theme.** Radix parts are unstyled `div`/`button`/portal nodes; we apply
  our token utilities directly. Surface colors via `bg-bg-elevated` /
  `border-border-color` / `text-text-secondary`; radius via `rounded-md`; elevation
  via `shadow-2`; typography via `text-sm`. Spacing uses the token shorthand
  (`px-(--space-4)`).
- **z-index discipline.** Use the named tier: tooltips/popovers at the `--z-popover`
  (40) level via `z-[40]` (matching `PopoverOverlay`'s existing `z-[40]/[41]`), so we
  don't reintroduce magic z-index races (ADR-0009).
- **Motion.** Keep enter/exit transitions short (`--motion-fast`) and let the global
  `prefers-reduced-motion` rule neutralize them — Radix `data-state` transitions ride
  on our existing reduced-motion handling.
- **Light theme survives.** Because we style only with semantic tokens, the planned
  light-theme swap (ADR-0009 Wave 4) applies to Radix parts for free. daisyUI would
  have broken this; token-styled Radix does not.
- **`@theme inline` registry unchanged** unless a new visual axis is needed — Radix
  adds no tokens.

### 4c. Bundle impact

- **CSS:** ~0. Radix ships no styles; our utilities are already generated on demand.
- **JS:** additive and granular. Tooltip primitive ≈ a few KB gz; it lazy-imports
  cleanly and only loads where used. This is *added* runtime (the app had none), so
  each subsequent primitive (DropdownMenu, Popover) should be justified by a concrete
  interaction, not adopted speculatively. Net: small, opt-in, tree-shaken — squarely
  within the "earn its place" rule. Verify with `bun run build:analyze` after any
  primitive is added beyond the pilot.

---

## 5. What this explicitly does NOT do

- Does **not** reverse ADR-0016 (no shadcn, no Preflight, no second theme).
- Does **not** enable Tailwind `base`/Preflight.
- Does **not** add daisyUI/Flowbite or any class-pack with its own palette.
- Does **not** rewrite or replace existing tested Dialog/Tablist/Button primitives.
- Does **not** do a broad migration — primitives are added per concrete need.

## 6. When to revisit the heavier options

- **React Aria Components / Ark UI:** when a *data table*, *date picker*, or rich
  *combobox with async loading* is on the roadmap — those benefit from Adobe/Zag
  collection + i18n machinery more than from Radix.
- **Headless UI Combobox:** if searchable provider/model selects in Settings are
  prioritized (Radix lacks a Combobox).
- **Tailwind Plus:** if several *net-new* screens are designed and a markup head-start
  (token-restyled + re-audited) saves time. License is optional, not required.

---

## 7. Pilot (landed)

A minimal, reversible pilot validates the recommended direction end-to-end: add one
Radix primitive, token-styled, behind a thin wrapper, wired into **one** untested
call site that previously used a native `title=`. No tested component was touched;
no `*.test.*`, Rust, or `biome.json` changed.

**Dependency added:** `@radix-ui/react-tooltip@^1.2.8` (the *per-primitive* package,
not the unified `radix-ui` meta-package — keeps the footprint to just the tooltip's
deps, consistent with the "granular, earn-its-place" rule; React 19 is in its peer
range). Radix ships **no CSS**, so the production CSS bundle is unchanged
(`dist/assets/index-*.css` = 47.86 KB / 10.21 KB gz).

**Files:**
- `src/components/Tooltip.tsx` (new) — token-styled wrapper over Radix Tooltip.
  Surfaces via `bg-bg-elevated` / `border-border-color` / `text-text-secondary`,
  `rounded-md`, `shadow-2`, `text-xs`; sits at the `--z-popover` tier (`z-[40]`),
  matching `PopoverOverlay`. Behavior only — no reset, no palette, no Preflight
  assumption. Bundles a local `Tooltip.Provider` so adoption needs no app-root
  change; if tooltips proliferate, hoist a single root `<Tooltip.Provider>`.
- `src/components/PipelineStatusBar.tsx:118` — the per-stage indicator's native
  `title={tooltip}` (dynamic `Idle/Running — N processed/Error: … • last latency …`)
  is now a styled Radix `<Tooltip>`. The screen-reader `aria-label` on the status
  dot (`role="img"`) is preserved unchanged, so the accessible name is identical —
  this is a sighted-user UX upgrade (stylable, theme-aware, focus/touch-capable),
  not a semantics change.

**Why this site:** highest-value, lowest-risk — it carries rich dynamic content
(the worst case for `title=`), has **no test file**, and is a contained edit. The
right-panel/Settings/conversation tablists and the `PopoverOverlay` dialogs were
deliberately left untouched (tested + ADR-protected).

**Reversibility:** delete `src/components/Tooltip.tsx`, revert the one import + the
one wrapped block in `PipelineStatusBar.tsx`, and `bun remove @radix-ui/react-tooltip`.

**Verification:** `bunx biome ci src` → exit 0; `bun run typecheck` → exit 0;
`bun run test` → 148/148 pass; `bun run build` → green (CSS unchanged at 47.86 KB).
