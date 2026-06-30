# ADR-0016: Adopt Tailwind v4 (token-bridged, no Preflight) and migrate components incrementally

## Status

Accepted (2026-05-30; originally proposed 2026-05-29). **Supersedes [ADR-0015](0015-modularize-css-defer-tailwind.md)**
on the technology question (it had deferred Tailwind/shadcn). The CSS
*modularization* that ADR-0015 implemented **remains in effect** and is the unit
of migration here. Reverses the "Option C" deferral in
[ADR-0009](0009-design-token-system-and-theming.md) and ADR-0015 under an
explicit owner decision to adopt a utility framework.

> **Status note (2026-05-30):** promoted proposed → accepted. The decision is
> fully implemented in code — Tailwind v4 via `@tailwindcss/vite` in
> `vite.config.ts`, the `@theme inline` token bridge in `styles.css`, and the
> incremental component migration (13 modules) per the Implementation section
> below. Status drift corrected per backlog audit 2026-05-30 (B05).

## Context

ADR-0015 (proposed earlier today) recommended staying on vanilla, modularized CSS
and *deferring* Tailwind/shadcn, because the research
([`docs/research/css-modernization-tailwind-shadcn.md`](../research/css-modernization-tailwind-shadcn.md))
showed the "monster file" pain was organization (now fixed) and that a framework
would relocate rather than reduce CSS. The research explicitly named the condition
for revisiting: "an explicit, concrete driver." The project owner has now given
that driver — a decision to adopt a utility framework and migrate.

Given the decision to adopt, the choice is **Tailwind v4 vs shadcn/ui**:
- **shadcn/ui** is Tailwind **plus runtime Radix primitives**, distributed as
  copy-pasted component source. Its core value — handing you accessible
  Tabs/Dialog/Buttons — is work this codebase **just completed by hand** (ARIA
  tablist, `useFocusTrap`-based dialog, `PopoverOverlay`, `role=log` regions,
  `Button`/`IconButton`), tested and contrast-audited. Adopting it would replace
  working, owned, tested a11y components and add runtime dependencies to an app
  that at the time had none beyond React. (Update 2026-05-30: ADR-0017's
  evaluation later adopted Radix *headless* behavior primitives à la carte —
  e.g. `@radix-ui/react-tooltip` — for genuinely new interactions; that is a
  deliberate, narrow exception, not a shadcn-style wholesale component library,
  and the hand-built tested a11y components are kept.)
- **Tailwind v4** is a zero-runtime CSS engine installed via the first-party
  `@tailwindcss/vite` plugin. Its v4 `@theme inline` directive lets utilities
  resolve **through** our existing CSS-variable tokens, so `styles.css` stays the
  single source of truth and the contrast-audited palette / theming plan survive.

## Decision Drivers

- Owner decision to modernize the styling toolchain and migrate.
- Preserve the WCAG work: audited palette, global `:focus-visible` ring,
  `prefers-reduced-motion`, `.sr-only`, and the hand-built accessible components.
- Keep `styles.css` tokens as the single source of truth (no value duplication).
- Zero runtime cost (Tauri offline desktop app); no network/CDN.
- Allow gradual, low-risk migration — coexist with the modular CSS from ADR-0015.

## Considered Options

- **Option A — Keep ADR-0015 as-is** (vanilla + modular CSS, no framework).
- **Option B — Adopt Tailwind v4, token-bridged, incremental** (no Preflight;
  `@theme inline` maps tokens; migrate one CSS module at a time).
- **Option C — Adopt shadcn/ui** (Tailwind + Radix; replace hand-rolled
  components).

## Decision Outcome

Proposed: **Option B — adopt Tailwind v4 incrementally, token-bridged, without
Preflight; migrate components module-by-module.** Tailwind is chosen over shadcn
because shadcn would duplicate and replace the accessible components built in this
same work stream and add runtime Radix dependencies for no capability gain, while
Tailwind v4 is zero-runtime and integrates with the existing token system.

Preflight (Tailwind's base reset) is **intentionally not imported** — the app
ships its own audited reset + focus ring + reduced-motion in `styles.css`; pulling
in a second reset is the main regression vector the research flagged, so we import
only the `theme` and `utilities` layers. Tokens are bridged with `@theme inline`
so every utility resolves to an existing `var(--token)`.

Option A is superseded by the owner decision. Option C is rejected for the
duplication/runtime-dependency reasons above.

### Consequences

- **Positive**: Utilities (mapped to tokens) are available for new and migrated
  components; consistent spacing/color vocabulary; familiar to many contributors.
- **Positive**: `styles.css` remains the single source of truth — utilities are
  `var(--token)` references, so the contrast audit and future theming still hold.
- **Positive**: Zero runtime cost; no Preflight means the existing reset/a11y
  layer is untouched.
- **Negative (honest)**: This does **not reduce** CSS for this app — importing
  `tailwindcss/theme.css` *added* ~7 KB (49.7→57 KB bundle) of default theme
  variables, and migrated styling moves from `.css` files into `className`
  strings rather than disappearing. The research's core finding stands: the value
  here is dev ergonomics/consistency, not bundle size.
- **Negative**: A **hybrid period** with two styling systems until migration
  completes; mixing BEM CSS and utilities on the *same* element has non-obvious
  precedence (unlayered CSS beats layered utilities), so migrate whole components.
- **Negative**: A parallel **token-name registry** in `@theme inline` to keep in
  sync when tokens are added; our non-linear px spacing scale does not match
  Tailwind's numeric base, so spacing uses the verbose arbitrary-value shorthand
  `p-(--space-5)` / `gap-(--space-3)` rather than clean `p-4`.
- **Negative**: Reverses a decision recorded twice (ADR-0009 Option C, ADR-0015);
  spends some ADR-discipline capital.
- **Neutral**: The ADR-0015 modularization is unaffected and is the migration unit.

## Implementation (done in this change)

- `bun add -d tailwindcss @tailwindcss/vite` (v4.3.0); `@tailwindcss/vite` plugin
  added to `vite.config.ts`.
- `styles.css`: `@layer theme, base, components, utilities;` +
  `@import "tailwindcss/theme.css" layer(theme)` + `…/utilities.css layer(utilities)`
  (**no Preflight**), and an `@theme inline` block mapping semantic colors → `--color-*`,
  radius → `--radius-*`, shadows → `--shadow-*`, font sizes → `--text-*`.
- **Reference migration:** `NotesPanel` fully converted to utilities;
  `src/styles/notes-panel.css` deleted and dropped from the barrel. Verified the
  generated CSS resolves to our tokens (e.g. `.bg-bg-elevated{background-color:var(--bg-elevated)}`).
- Verified: `tsc` clean, `vite build` green, 148/148 tests pass.

### Conventions (for subsequent migrations)

- Color: `bg-*` / `text-*` / `border-*` (e.g. `bg-bg-elevated`, `text-text-muted`).
- Radius: `rounded-xs..rounded-xl`; Shadow: `shadow-1..3`; Font size: `text-2xs..3xl`
  (size only — set line-height explicitly, e.g. `leading-[1.4]`).
- Spacing: token shorthand `p-(--space-5)`, `gap-(--space-3)`; one-off px in
  brackets `rounded-[10px]`.
- Migrate **one `src/styles/*.css` module per change**, deleting the module and
  its barrel entry; keep `@keyframes` and genuinely bespoke/stateful CSS as-is
  (optionally under `@layer components`) — do not force-fit complex rules into
  utilities.

### Migration outcome and deliberate boundary

The migration ran in five waves (1 reference + 4 parallelized via subagents,
each owning distinct files; barrel + verification handled centrally; every diff
reviewed). **13 component-specific modules were converted to utilities** and
**1 dead module (`toasts.css`) was retired** (Notifications uses the ADR-0011
`notification*` system): `notes-panel`, `speaker-panel`, `pipeline-status`,
`token-usage`, `banners`, `audio-source-selector`, `right-panel-tabs`,
`knowledge-graph`, `conversation-mode`, `agent-proposals`, `control-bar`,
`transcript`, `chat-sidebar`. All 9 `@keyframes` were consolidated into a
retained `keyframes.css`; animated components reference them via `animate-[…]`.

**The migration deliberately STOPS at the shared "component layer."** Usage
analysis showed the remaining modules define **reused design-system classes**,
not component styling:

- `settings.css` (`.settings-input/-field/-section/-btn/-radio/-modal`,
  `.status-badge`, `.model-card`) — used across **13 files** (every settings
  sub-panel + modals + ControlBar + SessionsBrowser).
- `primitives.css` (`.btn`, `.icon-btn`, `.notifications`/`.notification`) —
  **12 files**.
- `layout.css` (`.panel`, `.panel-title`, the app shell) — many files.
- `shortcuts-modal.css` / `express-setup.css` extend `.settings-modal` /
  `.settings-input`.

Inlining a class used in 12–13 files as a repeated utility string (or a shared
`const` of utilities, which merely re-invents the class) is a Tailwind
**anti-pattern** with high regression surface and no benefit. These stay as
**retained, token-based component-layer CSS** (alongside `keyframes.css`),
imported via the barrel. This matches Tailwind guidance (utilities for
one-off/component-specific styling; a component class for repeated patterns) and
the research's prediction that ~55–70% of this app's CSS is bespoke/shared.

If full Tailwind-nativeness is later desired, the option is to redefine these
classes via `@layer components { .btn { @apply … } }` — but that is churn over
already-clean token-based CSS with no functional gain, so it is **not** pursued
now. `@theme { --*: initial; }` to trim unused default-theme variables remains
an available bundle-size optimization.

## Conventions clause (token consolidation, 2026-06-29)

Added per the UI-styling audit
(`docs/reviews/_ui-styling-audit-2026-06-29/ui-styling-audit-report.md`),
which recommended `consolidate-tokens` — *finish this migration*, not adopt
shadcn (explicitly re-rejected). The standing conventions, now binding:

- **Two channels only.** Utilities-via-token-bridge for new and migrated
  components; semantic BEM only for (a) deep data-layout trees (the `<dl>`
  capability/detail cards) and (b) the not-yet-migrated remainder
  (`settings.css`, the modal scaffolds). There is **no third channel** — raw
  inline `style={{}}` is banned **except** for genuinely data-driven values
  (progress-bar width, per-speaker color, force-graph node coordinates).
- **No magic numbers when a token exists.** Use `rounded-xs..rounded-xl`
  (not `rounded-[Npx]`), the `--space-*` shorthand `p-(--space-5)` (not
  `p-[12px]`), `text-2xs..text-3xl`, the `--z-*` ladder via `z-(--z-banner)`
  (not `z-[1100]`), and `font-mono` / `font-sans` (not a pasted font stack).
  One-off values with no token may use a bracket arbitrary value and should
  be called out in review.
- **Closed, typed variant sets for badges/chips.** Status badges render through
  a typed `Badge` component whose `tone` set is closed; an unknown status maps
  to a neutral default rather than an unstyled BEM modifier
  (`--${status}`). This closes the open-set badge bug (audit D3).
- **Token sub-scales added in Phase 0:** `font-sans` / `font-mono`,
  `leading-tight` / `leading-base`, and `--color-accent-gemini` are now in the
  `@theme inline` bridge (`src/styles.css`). The `--radius-*` and `--space-*`
  scales already existed and are simply adopted.

This is a *conventions* extension of the accepted decision, not a new decision:
no ADR-level reversal, no new runtime dependency. The shadcn option remains
superseded.

## References

- Supersedes: [ADR-0015](0015-modularize-css-defer-tailwind.md)
- Research: `docs/research/css-modernization-tailwind-shadcn.md`
- [ADR-0009](0009-design-token-system-and-theming.md)
- Tailwind v4 — Vite install: https://tailwindcss.com/docs/installation/using-vite
- Tailwind v4 — Theme variables / `@theme inline`: https://tailwindcss.com/docs/theme
