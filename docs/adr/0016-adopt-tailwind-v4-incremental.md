# ADR-0016: Adopt Tailwind v4 (token-bridged, no Preflight) and migrate components incrementally

## Status

Proposed (2026-05-29). **Supersedes [ADR-0015](0015-modularize-css-defer-tailwind.md)**
on the technology question (it had deferred Tailwind/shadcn). The CSS
*modularization* that ADR-0015 implemented **remains in effect** and is the unit
of migration here. Reverses the "Option C" deferral in
[ADR-0009](0009-design-token-system-and-theming.md) and ADR-0015 under an
explicit owner decision to adopt a utility framework.

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
  that today has none beyond React.
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

### Phased migration plan (tracked outside this ADR)

Convert modules in ascending complexity: `banners`, `speaker-panel`,
`pipeline-status`, `notes-panel`(done), `right-panel-tabs`, `agent-proposals`,
`token-usage`, `chat-sidebar`, `transcript`, `control-bar`, `settings` (largest,
last). Re-verify focus ring / reduced-motion / contrast after each. Consider
`@theme { --*: initial; }` to drop unused default-theme variables once enough is
migrated to know which namespaces are needed.

## References

- Supersedes: [ADR-0015](0015-modularize-css-defer-tailwind.md)
- Research: `docs/research/css-modernization-tailwind-shadcn.md`
- [ADR-0009](0009-design-token-system-and-theming.md)
- Tailwind v4 — Vite install: https://tailwindcss.com/docs/installation/using-vite
- Tailwind v4 — Theme variables / `@theme inline`: https://tailwindcss.com/docs/theme
