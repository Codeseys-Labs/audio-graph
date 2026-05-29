# ADR-0009: Layered design-token system + theming

## Status

Accepted 2026-05-29.

## Context

The frontend's visual layer is fragmented (see
`docs/reviews/2026-05-29-uiux-deep-dive.md`):

- `styles.css` defines a single layer of **color-only** CSS custom properties on
  `:root`. `App.css` (2,695 lines) references them ~232 times but also embeds
  ~252 hardcoded hex literals + 81 `rgba()` literals.
- Most `App.css` rules use `var(--token, FALLBACK)` where the FALLBACK encodes a
  **stale, abandoned palette** (e.g. `--bg-primary` fallback `#1a1a2e` vs the
  real `#0e1117`; `--text-primary` `#e0e0e0` vs `#e7ebf2`; `--border-color`
  `#2a2a4a` vs `#2a3342`). The real token wins at runtime, so this is latent —
  but the file documents a UI we don't ship, and renaming any token would snap
  the app back to the old theme.
- There is **no scale** for spacing, radius, typography, shadow, z-index, or
  motion — all magic numbers (z-index alone uses `40/41`, `1099/1100`, and three
  overlays stacked at `1000`).
- There is **one dark theme**; no `prefers-color-scheme`, no `color-scheme`, no
  toggle. Adding light mode today requires editing ~333 hardcoded literals.

## Decision Drivers

- One source of truth for every visual axis, not just color.
- Theming (dark + light, future high-contrast) must be a token swap, not a
  component rewrite.
- Components should reference **semantic** tokens (`--color-bg-surface`), never
  raw primitives or hex, so themes swap cleanly.
- Keep tooling proportional — this is a single web target inside Tauri with no
  designer-in-Figma pipeline yet.
- Native UI affordances (scrollbars, form controls) must follow the theme.

## Considered Options

- **Option A — Two-layer hand-authored CSS custom properties.** Primitive layer
  (`--blue-500`, `--space-4`, `--radius-md`, `--shadow-2`, `--z-modal`,
  `--motion-fast`) + semantic layer (`--color-bg-surface: var(--gray-900)`,
  `--color-action-primary`, …) in `styles.css`. Theme = override the semantic
  layer under `[data-theme="light"]` / `@media (prefers-color-scheme)`. Set
  `color-scheme` per theme. Components consume only semantic tokens.
- **Option B — Style Dictionary / DTCG JSON pipeline.** Author tokens in JSON,
  generate CSS (and future iOS/Android). Industry standard for multi-platform.
- **Option C — Adopt a CSS framework / component lib** (Tailwind, Radix Themes,
  Park UI) and inherit its token system wholesale.

## Decision Outcome

Chosen: **Option A**. It directly fixes the ghost-palette and missing-scale
problems, enables dark+light via a semantic swap, and adds zero build complexity
or dependencies — appropriate for a single web target. Option B is the right
*eventual* shape if a second platform or a Figma→code workflow appears, and
Option A is forward-compatible with it (the JSON would generate exactly this CSS
with `outputReferences: true`). Option C would impose a large migration on a
codebase with an already-disciplined BEM convention and would fight the existing
hand-tuned, contrast-audited palette.

### Consequences

- **Positive:** Theming becomes a token swap; the ghost palette is deleted; all
  visual axes get a documented scale; z-index conflicts get named tiers.
- **Positive:** `color-scheme` opts native controls into the active theme.
- **Negative:** Up-front churn to migrate ~333 literals and re-point components
  at semantic tokens; must verify visual parity (screenshots) after the
  fallback cleanup.
- **Neutral:** No new dependency or build step; a later ADR can introduce Style
  Dictionary without changing component code.

## Implementation (intended)

- `src/styles.css`: add primitive + semantic token blocks; `:root { color-scheme: dark }`,
  `[data-theme="light"] { color-scheme: light; … }`, plus a
  `@media (prefers-color-scheme: light)` default before any stored override.
- Wave 1 of the plan: expand tokens, delete divergent fallbacks, add global
  `:focus-visible` + reduced-motion, fix `--text-muted` on `--bg-primary`.
- Wave 4: ship the light theme + Settings toggle.

## References

- `docs/reviews/2026-05-29-uiux-deep-dive.md`
- `docs/reviews/wcag-contrast-audit.md`
- W3C Design Tokens Format Module; CSS Color Module L5 (`light-dark()`,
  `color-scheme`); Martin Fowler, "Design Token-Based UI Architecture".
- ADR-0010 (icon system), ADR-0011 (feedback system) — sibling UI decisions.
