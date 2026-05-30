# Research: CSS modernization — Tailwind v4 / shadcn/ui vs. modularizing vanilla CSS

Status: Research only (no code changed, nothing installed). Date: 2026-05-29.
Author: styling-modernization investigation.

## Problem

The frontend's styling is split across two files imported globally in `src/main.tsx`:

- `src/styles.css` (~230 lines, ~7 KB): a mature, WCAG-audited **design-token
  layer** — primitive scales (`--space-*`, `--radius-*`, `--font-size-*`,
  `--shadow-*`, `--z-*`, `--motion-*`) + semantic color tokens, `color-scheme: dark`,
  a global `:focus-visible` ring (WCAG 2.4.7), `prefers-reduced-motion` handling
  (WCAG 2.3.3), `.sr-only`, and themed scrollbars. Governed by **ADR-0009**.
- `src/App.css` (~3,100 lines, ~63 KB): a single monolith of component styles.
  Measured in this repo: **413 class selectors**, **372 `var(--token)` references**,
  **9 `@keyframes`**, BEM-style naming (`.control-bar__capture-btn--start`).

Total ~70 KB of CSS. The stated goals: (1) "modularize the CSS so it's not a huge
monster file" and (2) "modernize it, maybe shadcn or tailwind to reduce total CSS,
or something."

Two distinct problems are bundled in that ask, and they have different answers:

1. **Organization** (the monolith) — real, worth fixing, cheap to fix.
2. **Technology** (vanilla CSS vs. Tailwind/shadcn) — a much bigger, ADR-governed
   decision where the case for switching is weak for *this* app.

**Prior art (important):** ADR-0009 ("Layered design-token system + theming",
Accepted 2026-05-29) already evaluated this. Its **Option C** was "Adopt a CSS
framework / component lib (Tailwind, Radix Themes, Park UI)" and it was rejected:
*"Option C would impose a large migration on a codebase with an already-disciplined
BEM convention and would fight the existing hand-tuned, contrast-audited palette."*
Any move to Tailwind/shadcn is therefore a **reversal of a just-accepted ADR** and
must clear a high bar. This report tests whether new facts (Tailwind v4's CSS-first
engine) change that conclusion.

---

## Findings per question

### Q1 — Tailwind v4 install/config on Vite 6 + React 18 + Tauri, and can it consume our existing tokens?

**Install/config is genuinely small in v4.** The first-party `@tailwindcss/vite`
plugin replaces the v3 PostCSS/`tailwind.config.js` dance:

```ts
// vite.config.ts
import tailwindcss from "@tailwindcss/vite";
export default defineConfig({ plugins: [react(), tailwindcss()] });
```

```css
/* entry CSS */
@import "tailwindcss";
```

That's the whole setup — no `postcss.config`, no `content` globs (v4 auto-detects
source files). Source: Tailwind "Installing with Vite"
(https://tailwindcss.com/docs/installation/using-vite). No PostCSS is added to this
repo's toolchain; the plugin is self-contained. It is **zero-runtime** (compiles to
a static stylesheet), which is correct for a Tauri offline desktop app — no CDN, no
network, no runtime JS cost.

**Yes — it can consume an existing CSS-custom-property token system, and v4 is
specifically built for this.** Tailwind v4 is "CSS-first": the theme is declared in
CSS via the `@theme` directive instead of a JS config. The key feature for us is
`@theme inline`, which lets a Tailwind theme variable *reference another CSS
variable* and emit the utility against that reference:

```css
@import "tailwindcss";
/* styles.css still owns the real tokens (single source of truth) */
@theme inline {
  --color-bg-primary: var(--bg-primary);     /* -> bg-bg-primary, text-bg-primary */
  --color-accent:     var(--accent);
  --spacing-6:        var(--space-6);         /* -> p-6, gap-6, etc. (careful, see below) */
  --radius-md:        var(--radius-md);       /* -> rounded-md */
  --shadow-2:         var(--shadow-2);        /* -> shadow-2 */
}
```

Tailwind docs, "Theme variables" (https://tailwindcss.com/docs/theme):

- *"Theme variables are special CSS variables defined using the `@theme` directive
  that influence which utility classes exist in your project."* Defining
  `--color-mint-500` makes `bg-mint-500`/`text-mint-500` exist.
- **"Referencing other variables":** *"When defining theme variables that reference
  other variables, use the `inline` option."* With `@theme inline { --font-sans:
  var(--font-inter) }`, the generated utility is `font-family: var(--font-inter)`.
  This is exactly the mapping we'd want: our `styles.css` stays the source of truth,
  Tailwind utilities resolve *through* our tokens.

So the tokens are **not duplicated** — `styles.css` keeps the values, `@theme inline`
just registers names so Tailwind generates matching utilities. Theming (the
dark/light swap ADR-0009 plans) still happens by overriding the semantic layer in
`styles.css`; the utilities follow automatically because they're `var()` references.

**Two real caveats to the "single source of truth" claim:**

- **Namespace renaming.** Tailwind's utility namespaces are fixed: colors must live
  under `--color-*`, spacing under `--spacing-*`, radius under `--radius-*`, etc.
  Our tokens are named `--bg-primary`, `--space-6`, `--accent-red`. To get utilities
  you must *introduce a second name* (`--color-bg-primary: var(--bg-primary)`). The
  values aren't duplicated, but the **name list is** — a parallel registry to keep
  in sync whenever a token is added.
- **Spacing model mismatch.** Tailwind's spacing utilities are multiples of a single
  `--spacing` base (default `0.25rem`), so `p-4` = `calc(var(--spacing) * 4)`. Our
  scale is non-linear in px (`--space-1: 2px … --space-6: 16px`). Mapping per-step
  (`--spacing-6: var(--space-6)`) works but you lose the arithmetic niceties and have
  to enumerate each step; the two systems don't align 1:1.
- **Preflight reset.** `@import "tailwindcss"` pulls in Preflight, Tailwind's own
  base reset, into the `base` layer. We already ship a reset in `styles.css`
  (`*{box-sizing;margin;padding}`, scrollbars, focus ring). Two resets coexisting is
  a source of subtle diffs (margins, form-control defaults) — see Q4.

### Q2 — Realistically, how much hand-written CSS does Tailwind eliminate here?

Tailwind eliminates CSS most effectively for **high-volume, repetitive utility
styling**: spacing, fl/grid layout, typography, simple color fills — the stuff that
otherwise becomes hundreds of near-identical declarations. It eliminates CSS *least*
for **bespoke, stateful, or computed** styling.

For *this* app the bespoke share is high. Sampling `App.css`:

- **Layout shell** (`.app-container`, `.main-layout`, `.left/center/right-panel`,
  resizable dividers driven by JS) — bespoke fl/grid with runtime-resized panels.
  Translatable to utilities but low payoff; it's written once.
- **Graph surface** (`react-force-graph-2d`) — canvas-rendered; styling is JS/props,
  not CSS classes. Tailwind does nothing here.
- **Stateful component variants** — `.control-bar__capture-btn--start`,
  `--stop`, `:hover:not(:disabled)`, `--active`, recording-dot pulse animations,
  backpressure indicators. These are multi-state, animation-bearing, and read
  cleanly as named BEM blocks. As Tailwind they become long `className` strings with
  `data-*`/variant prefixes, plus the 9 `@keyframes` still hand-written in CSS.
- **Genuinely repetitive bits** — paddings, gaps, font sizes, simple borders. This
  *is* where utilities shine, and it's a real but modest slice of 63 KB.

**Honest estimate:** Tailwind could absorb perhaps **30–45%** of `App.css` (the
repetitive layout/spacing/type/color declarations). The remaining **55–70%**
(component-specific states, animations, canvas/graph integration, panel resize,
overlay/z-index orchestration, focus management) stays bespoke — either as `@layer
components` CSS *inside* the Tailwind project or untouched vanilla CSS. The total
"lines of styling" rarely drops as much as advertised; it **moves from `.css` files
into JSX `className` attributes**. For a finite component set (~35 components) that
is built once and rarely re-themed at the markup level, that relocation is lateral,
not a clear win.

### Q3 — shadcn/ui: fit for this app

**What it is.** shadcn/ui is *not* a dependency you install and import. It's a
**copy-paste distribution**: a CLI (`shadcn@latest add button`) drops component
source into your repo (`src/components/ui/*`). Those components are **Tailwind CSS
for styling + Radix UI primitives for behavior/accessibility** (Source: shadcn/ui
GitHub — "beautifully-designed, accessible components"; Vercel Academy — "Radix UI
primitives… provide all the complex behavior, accessibility features… while
remaining unstyled"). It therefore **hard-requires Tailwind** (the Vite guide's
first step is "Add Tailwind CSS") plus `class-variance-authority`, `clsx`,
`tailwind-merge`, and a `@/*` path alias.

**Dependency/bundle/maintenance cost.** Even though the component code is "yours,"
each shadcn component pulls **runtime Radix packages** (`radix-ui`, formerly
`@radix-ui/react-*`) into `node_modules` and the bundle. That's net-new runtime
dependencies for an app that currently has **zero UI-framework deps** beyond React.
Maintenance flips from "we own ~35 small components" to "we own copy-pasted
component source *and* track upstream Radix + shadcn changes manually" (copy-paste
means no `npm update` — you re-pull to get fixes).

**The decisive issue for THIS app: it duplicates work we already did.** The team
*just hand-built* accessible primitives — an ARIA `tablist`, a focus-trapped dialog
via `useFocusTrap` (with tests: `useFocusTrap.test.ts`), `role=log` live regions,
`Button`/`IconButton`. shadcn's value proposition is *"you don't have to build
accessible Tabs/Dialog/Buttons."* We already have them, tested, contrast-audited,
and matched to our palette. Adopting shadcn would **replace working, owned,
test-covered a11y components with copy-pasted Radix-based equivalents** — pure churn
with regression risk and no capability gain. shadcn earns its keep when you're
*starting* a component layer, not when you've finished one.

**Tauri offline fit.** Technically fine — it's zero-runtime CSS + client-side React,
no network. So "does it fit Tauri" is yes; "is it worth it here" is no.

**Where shadcn *would* help:** if we needed many *new* complex widgets we don't have
(Combobox, Command palette, Date picker, Data table, Menubar). If the roadmap grows
toward that, revisit — but adopt *individual* primitives à la carte, not the whole
system.

### Q4 — Migration cost & risk: big-bang vs. incremental; can Tailwind coexist?

**Coexistence: yes, Tailwind can run alongside vanilla CSS.** Utilities live in
Tailwind's `utilities` layer and you keep importing `styles.css`/`App.css`. The
practical friction points:

- **Two resets.** Preflight (from `@import "tailwindcss"`) plus our existing reset.
  Preflight unsets margins, list styles, and form-control styling; differences can
  shift spacing on already-tuned components. Mitigation exists (import Tailwind into
  an explicit `@layer`, or selectively disable Preflight), but it's careful work.
- **Specificity & cascade.** Utilities are low-specificity single classes; our BEM
  rules (`.control-bar__capture-btn--start:hover:not(:disabled)`) are higher
  specificity and will **win** over utilities on the same element. During a hybrid
  period an element styled by both BEM and utilities has non-obvious precedence —
  the classic "why won't `p-4` apply" confusion. Tailwind v4 uses CSS `@layer` to
  order things, which helps, but mixing paradigms on one element is where bugs hide.
- **Token alignment.** The `@theme inline` mapping (Q1) must be built before
  utilities are useful, and the spacing-scale mismatch means some utilities won't
  match our px scale unless every step is mapped.

**Big-bang vs incremental:**

- *Big-bang* (rewrite all 413 selectors to utilities): weeks of work, touches every
  component and every test that asserts on class names/structure, high regression
  surface across the a11y work, and reverses ADR-0009 wholesale. **Not justified.**
- *Incremental* (Tailwind installed, used for new components only, old CSS untouched):
  feasible, but you now maintain **two styling systems** indefinitely, two resets,
  and a token-name registry — *added* complexity, with the "reduce total CSS" goal
  unmet until/unless a long migration finishes. The realistic outcome is a permanent
  hybrid, which is worse for comprehension than either pure approach.

### Q5 — Accessibility: does utility-class / shadcn migration help or hurt the WCAG work?

- **Tailwind utilities are a11y-neutral-to-slightly-negative *here*.** Tailwind
  doesn't manage focus, ARIA, or live regions — that's all still your JS. The risks
  are concrete: the global `:focus-visible` ring and `prefers-reduced-motion` block
  live in `styles.css` and must be preserved exactly (Preflight + utility churn can
  step on them); and porting hand-tuned, **contrast-audited** colors
  (`docs/reviews/wcag-contrast-audit.md`) onto Tailwind's default OKLCH palette would
  *break* the audit. Mapping via `@theme inline` to our existing hexes avoids that,
  but it must be deliberate. Net: at best neutral, with re-audit cost.
- **shadcn/Radix a11y is good but we already have it.** Radix primitives are
  well-tested for ARIA/keyboarding — genuinely valuable *if you have none*. We have
  ours, tested. Swapping introduces a window where our verified behavior is replaced
  by un-reverified behavior, plus our `useFocusTrap` tests would be discarded. That's
  **a11y risk taken on for no a11y gain.**

Conclusion: neither option *improves* accessibility for this app; both create a
re-verification burden against work that already passed.

### Q6 — Recommendation matrix

#### Option A — Stay vanilla CSS, MODULARIZE `App.css` into per-component files

Split the 63 KB monolith into co-located files (e.g. `ControlBar.css`,
`NotesPanel.css`, …) imported by their components, keeping `styles.css` as the
shared token/reset layer. BEM names already map 1:1 to components.

- **Pros:** Directly solves the *actual* stated pain (the monster file). Zero new
  deps, zero new build steps, zero bundle change. Preserves the WCAG audit, focus
  ring, reduced-motion, and the just-accepted ADR-0009. Co-location improves
  discoverability and makes dead-CSS removal tractable. Tests/markup untouched.
  Reversible and low-risk; can be done incrementally, one component at a time.
- **Cons:** Doesn't "reduce total CSS" much (organization, not volume). Doesn't add
  utilities for fast one-off spacing tweaks. No design-system "wow." Discipline still
  rests on BEM convention rather than tooling.

#### Option B — Add Tailwind v4 mapped onto existing tokens, incrementally; keep bespoke CSS

Install `@tailwindcss/vite`, map tokens via `@theme inline`, use utilities for new
components, leave existing BEM CSS in place.

- **Pros:** Utilities available for rapid layout/spacing on new work. v4 install is
  small and zero-runtime (Tauri-safe). `@theme inline` keeps `styles.css` as source
  of truth (no value duplication). Modern, familiar to many contributors.
- **Cons:** Two styling systems + two resets to maintain *indefinitely* (the hybrid
  rarely ends). Token **name** registry duplicated; spacing scale mismatch. Reverses
  ADR-0009's Option-C rejection → needs a new/superseding ADR. Re-audit risk on focus
  ring, reduced-motion, contrast. "Reduce total CSS" goal largely unmet; styling just
  relocates into `className`. Net new complexity for a finite, built-once UI.

#### Option C — Adopt shadcn/ui

Bring in Tailwind + Radix, replace hand-rolled primitives with shadcn copies.

- **Pros:** Best-in-class a11y primitives and a large catalog (Combobox, Command,
  Data table…) if we needed them. Good for *greenfield* component layers.
- **Cons:** Requires Tailwind (inherits all of B's costs) **plus** runtime Radix
  deps + `cva`/`clsx`/`tailwind-merge` into an app with no UI-framework deps today.
  **Duplicates/replaces the accessible Tabs/Dialog/Buttons we just built and tested**
  — churn and regression risk for no capability gain. Copy-paste means manual upstream
  tracking. Largest migration, hardest ADR reversal, highest a11y re-verification
  cost. Misaligned with a mature, finished, ADR-governed component set.

---

## Recommendation

**Choose Option A: modularize `App.css` into per-component CSS files; keep the
vanilla-CSS + design-token architecture. Do not adopt Tailwind or shadcn now.**

Rationale, objectively:

1. **The real, stated pain is organization, not technology.** A 63 KB monolith of
   413 well-named BEM selectors is a *file-splitting* problem. Option A fixes it in
   hours, reversibly, with no new dependencies, no bundle change, and no risk to the
   WCAG work — and it leaves the door open to B later.
2. **Tailwind/shadcn don't meaningfully "reduce total CSS" here.** ~55–70% of this
   app's styling is bespoke (panels, resize, canvas graph, multi-state controls,
   animations, overlays). Utilities relocate the repetitive remainder into `className`
   rather than deleting it. For a finite, built-once UI the payoff is low and the
   carrying cost (two systems, two resets, token-name registry, re-audit) is real and
   permanent.
3. **shadcn would duplicate work just completed.** We hand-built tested, contrast-
   audited, ARIA-correct Tabs/Dialog/Buttons + `useFocusTrap`. shadcn's core value is
   handing those to teams that *lack* them. Replacing working a11y components is pure
   risk.
4. **ADR-0009 already rejected this (Option C) for sound reasons, days ago.** Tailwind
   v4's CSS-first engine is a genuinely better integration story than v3, but it does
   not change the underlying cost/benefit for *this* mature, single-target, BEM-
   disciplined, accessibility-led codebase. Reversing a just-accepted ADR needs a
   stronger driver than "modernize, maybe."

**If/when to revisit Option B:** if the UI grows substantially (many new screens),
if multiple contributors find BEM discipline slipping, or if a `prefers-color-scheme`
light theme lands and utilities would simplify it. At that point, adopt Tailwind v4
*incrementally* with `@theme inline` mapped onto the existing tokens — and supersede
ADR-0009 with a new ADR recording the reversal. **Revisit Option C (à la carte Radix
primitives, not whole shadcn) only** for specific complex widgets we don't have and
don't want to hand-build (Combobox, Command palette, Data table).

**Concrete next step (Option A) — no decision reversal required:**
1. Create `src/components/styles/` (or co-locate `Foo.css` next to `Foo.tsx`).
2. Move each BEM block out of `App.css` into its component's file; import it from the
   component. Keep `styles.css` as the shared token + reset + a11y layer.
3. Delete `App.css` once empty; verify focus ring, reduced-motion, contrast, and
   snapshot/RTL tests still pass.
4. Optionally add a short ADR ("CSS modularization") noting App.css was split with no
   change to the token architecture — keeping ADR-0009 intact.

---

## Risks

| # | Risk | Likelihood | Impact | Mitigation |
|---|------|-----------|--------|-----------|
| 1 | **Adopting Tailwind/shadcn yields a permanent hybrid** (two styling systems, two resets) instead of the promised simplification | High if B/C chosen | High (worse comprehension, ongoing maintenance) | Prefer A; if B, commit to a finish-line migration plan, not "new code only" |
| 2 | **Regressing the WCAG work** — focus-visible ring, prefers-reduced-motion, contrast-audited palette — during any reset/utility/Radix swap | Medium (B), High (C) | High (accessibility is a core value) | A avoids it entirely; for B/C, re-run `wcag-contrast-audit.md` and a11y tests before merge |
| 3 | **Reversing ADR-0009 without sufficient justification**, eroding the ADR discipline the team relies on | Medium (B/C) | Medium | Only reverse via a superseding ADR with a concrete, measured driver; "modernize, maybe" is not one |
| 4 | Even Option A's split could break specificity/cascade if files import in a different order than the monolith | Low | Low–Medium | Keep a single import order; `styles.css` first; verify visually + tests after the split |
| 5 | shadcn copy-paste means **no automated upstream security/bug updates** for Radix-based component source | Medium (C only) | Medium | Avoid C now; if adopted later, track Radix advisories manually |

## Sources

- Tailwind CSS v4 — Installing with Vite (`@tailwindcss/vite`, `@import "tailwindcss"`, zero-config): https://tailwindcss.com/docs/installation/using-vite
- Tailwind CSS v4 — Theme variables (`@theme`, `@theme inline`, "Referencing other variables", namespaces, Preflight import): https://tailwindcss.com/docs/theme
- shadcn/ui — Vite installation (requires Tailwind, `@/*` alias, CLI `add`): https://ui.shadcn.com/docs/installation/vite
- shadcn/ui — Manual install (deps: `class-variance-authority`, `clsx`, `tailwind-merge`): https://ui.shadcn.com/docs/installation/manual
- shadcn/ui — Feb 2026 unified `radix-ui` package (runtime Radix dependency): https://ui.shadcn.com/docs/changelog/2026-02-radix-ui
- shadcn/ui GitHub (copy-paste, Radix-based, "Open Code"): https://github.com/shadcn-ui/ui
- Internal: `docs/adr/0009-design-token-system-and-theming.md` (Option C considered & rejected), `docs/reviews/wcag-contrast-audit.md`, `src/styles.css`, `src/App.css`.
