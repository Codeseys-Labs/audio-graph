# ADR-0015: Modularize App.css into per-component stylesheets; keep vanilla CSS (defer Tailwind/shadcn)

> **Superseded by [ADR-0016](0016-adopt-tailwind-v4-incremental.md)** on the
> technology question: Tailwind v4 was subsequently adopted under an explicit
> owner decision. The *modularization* this ADR implemented remains in effect and
> is the unit of the incremental migration in ADR-0016.

## Status

Superseded by ADR-0016 (2026-05-29). Implemented **Option A** (the modular CSS
split, which stands). Its "defer Tailwind/shadcn" conclusion was reversed by
ADR-0016. Backed by research:
[`docs/research/css-modernization-tailwind-shadcn.md`](../research/css-modernization-tailwind-shadcn.md).

## Context

`src/App.css` had grown to a single **3114-line / ~63 KB** monolith holding every
component's styles, while `src/styles.css` holds the WCAG-audited design-token
layer (ADR-0009). The monolith is hard to navigate, review, and own — editing one
component means scrolling a 3k-line file, and ownership boundaries are invisible.

The prompt that triggered this was "modularize the CSS so it's not a huge monster
file" and "modernize it — maybe shadcn or Tailwind to reduce the amount of CSS."
Two distinct questions fall out: (1) *organization* of the existing CSS, and
(2) whether to *change the styling technology*.

Research findings (full report in `docs/research/`):
- The app is a single-target Tauri desktop UI with a finite component set and
  ~70 KB total CSS, already governed by a token system and recent accessibility
  work (hand-built ARIA tablist, focus-trapped dialog, `useFocusTrap`, `role=log`
  live regions, `.sr-only`).
- Tailwind v4 *can* consume the existing CSS-variable tokens via `@theme inline`
  and installs cleanly through `@tailwindcss/vite` (zero runtime, Tauri-safe), but
  realistically absorbs only ~30–45 % of this app's CSS — bespoke layout, the
  resize system, the force-graph canvas, multi-state controls, 9 keyframes, and
  overlays remain hand-written. It mostly *relocates* styling into `className`.
- shadcn/ui is Tailwind + runtime Radix primitives; adopting it would **duplicate
  and replace the accessible Tabs/Dialog/Buttons just built and tested**, for no
  functional gain, plus new dependencies.
- The file is 100 % class-scoped (no globals/tokens live in it), every `@keyframes`
  is co-located with its sole consumer, there are no `@media`/`@import`, and the
  only cross-section cascade dependency is that the settings-modal base must
  precede the two modals that extend it.

## Decision Drivers

- Fix the actual pain (navigation/ownership of a 3k-line file) with the least
  risk and zero new runtime dependencies.
- Preserve the audited token system and the accessibility work intact.
- Don't reverse a just-accepted ADR (0009) on a "maybe" basis without a concrete,
  measured driver.
- Keep the door open to incremental Tailwind later if the UI grows or theming
  lands.

## Considered Options

- **Option A — Modularize into per-component CSS, stay vanilla.** Split `App.css`
  into ~19 files under `src/styles/`, imported via a single barrel `index.css`
  that preserves cascade order. No technology change.
- **Option B — Adopt Tailwind v4 mapped onto existing tokens, incrementally.**
  Add `@tailwindcss/vite`, expose tokens via `@theme inline`, write new components
  with utilities, migrate existing CSS opportunistically.
- **Option C — Adopt shadcn/ui (Tailwind + Radix).** Replace hand-rolled
  components with shadcn equivalents.

## Decision Outcome

Proposed: **Option A.** The monster-file problem is an *organization* problem, not
a *technology* one; it is fixed in hours, reversibly, with no new dependencies, no
bundle change, and no risk to the WCAG work. Tailwind/shadcn solve a problem this
app doesn't strongly have (CSS *volume*) while adding permanent carrying cost, and
shadcn would actively duplicate the accessible components from this same work
stream. ADR-0009 already weighed and rejected a utility/framework approach;
Tailwind v4 improves the *integration* story but not the cost/benefit for a mature,
single-target, accessibility-led app.

Option B is the right thing to revisit **later** — via a superseding ADR — if a
concrete driver appears (substantial UI growth, a light theme, or repeated
copy-paste of utility-like CSS). Option C is rejected: it imports a runtime
dependency to re-solve already-solved accessibility.

### Consequences

- **Positive**: Each component's styles live in a small, named file
  (`chat-sidebar.css`, `settings.css`, …) — easy to find, review, and own.
- **Positive**: Zero new dependencies; bundle output unchanged (Vite inlines the
  `@import` barrel back into one ~49.7 KB stylesheet); token system + a11y intact.
- **Positive**: Smaller per-file diffs going forward; clearer git blame.
- **Negative**: 19 files + a barrel instead of one file — slightly more files to
  scan, and a new contributor must know the barrel encodes the one ordering
  constraint (settings before its modal extensions).
- **Negative**: Does not reduce the *amount* of CSS — that was never the real
  problem here, but the original prompt framed it that way.
- **Neutral**: A future Tailwind adoption (Option B) is unaffected — modular files
  make per-component conversion easier, not harder.

## Implementation notes

- Split performed by a validated script: ranges expressed per file, asserting no
  overlaps and that every uncovered line is blank (no rule could be dropped).
  3094 content lines distributed across 19 files (3114 − 20 blank separators).
- `src/App.tsx` now imports `./styles/index.css` instead of `./App.css`; the
  monolith is deleted.
- Verified: `tsc --noEmit` clean, `vite build` green (single 49.7 KB CSS chunk),
  148/148 frontend tests pass.

## References

- Research: `docs/research/css-modernization-tailwind-shadcn.md`
- [ADR-0009](0009-design-token-system-and-theming.md) (design tokens; rejected
  utility-framework Option C — this ADR reaffirms that for the same reasons)
- [ADR-0010](0010-icon-system.md)
- Tailwind v4 theme variables: https://tailwindcss.com/docs/theme
- shadcn/ui: https://ui.shadcn.com/docs
