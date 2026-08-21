---
status: accepted
date: 2026-08-20
deciders: [AudioGraph maintainers]
---

# ADR-0047: A Tier-3 `.ag-*` Recipe Layer in `@layer components`, and `--edge`/`--edge-subtle` Non-Text Contrast Tokens (amends ADR-0016)

## Context and Problem Statement

`styles.css` has declared `@layer theme, base, components, utilities;` since
ADR-0016, but the `components` layer has stood empty ever since — every
migrated component uses the utilities-via-token-bridge channel, and
everything else stays on semantic BEM in the not-yet-migrated `.css` modules.
ADR-0016's 2026-06-29 conventions clause made this explicit and closed the
door on a third option: **"Two channels only... there is no third
channel — raw inline `style={{}}` is banned except for genuinely
data-driven values."** In context, the third channel that clause bans is
inline styles specifically, not a CSS-only recipe layer; 0016 itself already
floats `@layer components { .btn { @apply … } }` as an available-but-not-
pursued option for exactly this shape (0016's "Inlining a class used in
12-13 files..." section). This record's amendment is best read as
exercising an option 0016 left open, not reversing 0016's decision — the
full MADR treatment below is warranted by the amendment's scope (new tokens,
a sweep, a components-layer convention), not because it reverses anything.

Two independent pressures now push back on that closure:

1. **Repeated visual primitives with no shared home.** A status chip, a
   section micro-label, a card, a ghost micro-button, a labeled field, and a
   panel header recur across `NotesPanel`, `LiveTranscript`,
   `SessionDataRoutePanel`, `ProjectionRuntimeStatusPanel`, `TokenUsagePanel`,
   `SessionsBrowser`, and the settings module — each currently re-derived per
   file (or, for status chips, solved once in JS: `settings/Badge.tsx`'s
   closed `Record<BadgeTone, string>` map with a guaranteed-styled `neutral`
   fallback for unrecognized values). Badge's pattern is worth generalizing,
   but re-solving it as N more React wrapper components (one per primitive)
   adds N more modules, props contracts, and render indirections for what is,
   in every case, a fixed set of CSS rules keyed by an attribute value — a
   shape CSS attribute selectors express natively, without a component layer
   at all. The 2026-08-20 UI-overhaul design panel (`docs/agentic-runs/
   2026-08-20-ui-overhaul-design/`) plans exactly this convergence across its
   T2/T6/T7 tickets.
2. **A measured WCAG 1.4.11 (non-text contrast) failure in the token these
   primitives would have to borrow.** `--border-color` was audited against
   the four surface tokens (`--bg-primary/-secondary/-tertiary/-elevated`)
   in both themes and computes as low as **1.11:1-1.20:1** against several
   surface pairs — far under the 3:1 floor WCAG 1.4.11 requires for "visual
   information required to identify UI components." Measured baseline:

   | Pair | Ratio |
   |---|---:|
   | dark `--border-color` (`#2a3342`) on `--bg-elevated` (`#232c3a`) | 1.11:1 |
   | dark `--bg-tertiary` (`#1d2430`) on `--bg-secondary` (`#151a23`) | 1.12:1 |
   | light `--border-color` (`#d4dae3`) on `--bg-tertiary` (`#e9edf3`) | 1.20:1 |

   `--border-color` cannot simply be raised in place — it is still consumed
   by 42 not-yet-migrated sites in `settings.css`/`layout.css`/
   `primitives.css`/`shortcuts-modal.css`, out of this record's scope, and
   raising it there is exactly the kind of drive-by change ADR-0034/ADR-0039
   discipline (evidence-scoped changes, not opportunistic ones) argues
   against (see the rejected "raise in place" alternative under Considered
   Options). A new, correctly-computed token is the smaller, reviewable unit.

## Decision Drivers

- Recipes should exist *before* three tickets try to adopt the same visual
  shape independently (T2 lands the layer; T6/T7 adopt it) — proving the
  shape once, in isolation, with zero call sites at risk, is cheaper to
  review than three simultaneous adoptions inventing slightly different
  shapes under time pressure.
- Closed variant sets over open BEM modifiers, generalized from Badge's fix
  for the open-set badge bug (an interpolated `--${status}` modifier renders
  *unstyled* for any status without a matching rule): every `.ag-*` recipe
  with a variant attribute must default its base rule to a safe, styled
  fallback.
- Non-text contrast must be a computed, testable property (real luminance
  math against real token values), not an eyeballed color choice — mirrors
  how ADR-0009/the WCAG audit already treat text contrast.
- Zero new runtime dependency; stay inside the CSS-only posture ADR-0009 and
  ADR-0016 already committed to (no component library, no CSS-in-JS).
- `--border-color` itself must not be touched — its not-yet-migrated
  consumers are out of scope, and a same-named token cannot safely change
  meaning under sites that haven't been re-verified.

## Considered Options

1. **Extend Badge's JS pattern per-primitive** — a `Chip`, `Card`,
   `MicroLabel`, `FieldRow`, `PanelHead` React component per recurring
   primitive, each with its own closed prop union.
2. **Stay on two channels; keep solving this in BEM** — add the chip/card/
   label rules to `settings.css` (or a new BEM module) and let each
   consuming file import/duplicate what it needs.
3. **Populate the already-declared, empty `components` layer with a small,
   closed set of `.ag-*` recipes**, keyed by `data-*` attributes for their
   variant axes, consuming only existing semantic tokens plus two new ones
   (`--edge`, `--edge-subtle`) for non-text contrast.

For the token values specifically (the only part of this record that
repaints already-shipped pixels), the considered-and-rejected alternative
was: **raise `--border-color` in place**, re-verifying all 42 not-yet-
migrated `.css`-module sites against the new 3:1 floor. Rejected because (a)
several of those sites are decorative dividers that would legitimately want
`--edge-subtle`'s lower floor rather than a blanket raise, so "raise in
place" is not actually a single safe edit, and (b) re-verifying 42
unrelated, not-yet-migrated sites is exactly the opportunistic, evidence-
unscoped change ADR-0034/ADR-0039 discipline argues against doing inside a
record whose actual driver is the recipe layer.

## Decision Outcome

Chosen option: **3 — populate `@layer components` with closed `.ag-*`
recipes**, because it is the only option that adds a shared visual primitive
without adding a JS abstraction layer (option 1) or leaving the duplication
this record exists to end (option 2). `@layer components` was declared for
exactly this purpose in ADR-0016 and has been sitting empty; attribute
selectors are a direct CSS expression of a closed, typed variant set — no
weaker a guarantee than Badge's TypeScript union, just enforced by "no
matching selector falls through to the base rule" instead of `switch`
exhaustiveness. This is an **amendment to ADR-0016's "two channels only"
convention clause**, not a reversal of Tailwind adoption itself: utilities
remain the channel for one-off, component-specific styling; BEM remains the
channel for the not-yet-migrated remainder and deep data-layout trees;
`.ag-*` is the third channel, specifically for primitives reused across many
components' markup.

Six recipes land in this change, all unused (adoption is a later ticket —
see Consequences):

- **`.ag-label`** — 11px/600/0.04em uppercase micro section label.
- **`.ag-chip[data-tone]`** — closed tones `success | warning | danger |
  info | neutral | accent`, generalizing Badge's map into CSS: the base
  rule *is* the neutral tint, so a missing or unrecognized `data-tone`
  renders styled, never blank.
- **`.ag-card[data-elevation]`** — closed states `flat | raised | overlay`.
- **`.ag-btn-micro`** — ghost micro button for inline row actions.
- **`.ag-field`** — labeled control, generalizing `.settings-input`/
  `.settings-field`, with a `data-layout="row"` variant for checklist rows.
- **`.ag-panel-head`** — one panel header bar (title + action slot).

### The `--edge` / `--edge-subtle` token pair

`.ag-card`, `.ag-field`, and `.ag-panel-head` all draw a border that is a
real component boundary — the exact case WCAG 1.4.11 targets — so they
cannot borrow `--border-color`. Two new semantic tokens (ADR-0009's surface/
border section) fill the gap without touching `--border-color`:

- **`--edge`** — component boundaries. Computed **>=3:1** against all four
  surface tokens, in both themes.
- **`--edge-subtle`** — decorative-only rules (section rhythm, not an object
  boundary a user needs to locate). WCAG 1.4.11 exempts decorative content
  from the 3:1 floor entirely; landing this **~1.5:1** rather than at
  `--border-color`'s current ~1.1-1.3:1 is a deliberate design choice (still
  visible as a rule), not a compliance requirement.

Chosen values and their computed ratios (real WCAG relative-luminance math,
also asserted by `src/styles.a11y.test.ts` against the literal token values
— not hardcoded expectations):

| Token | Dark value | vs primary | vs secondary | vs tertiary | vs elevated |
|---|---|---:|---:|---:|---:|
| `--edge` | `#6b7a92` | 4.34:1 | 4.01:1 | 3.58:1 | 3.23:1 |
| `--edge-subtle` | `#37455a` | 1.95:1 | 1.80:1 | 1.60:1 | 1.45:1 |

| Token | Light value | vs primary | vs secondary | vs tertiary | vs elevated |
|---|---|---:|---:|---:|---:|
| `--edge` | `#7285a1` | 3.76:1 | 3.48:1 | 3.20:1 | 3.63:1 |
| `--edge-subtle` | `#b6bfce` | 1.85:1 | 1.71:1 | 1.58:1 | 1.79:1 |

The binding constraint in both themes is the surface closest in lightness to
a boundary-appropriate gray: `--bg-elevated` in dark (the lightest of the
four dark surfaces), `--bg-tertiary` in light (the darkest of the four light
surfaces). Both land with real margin above 3:1, not exactly at the floor.

**Light `--bg-elevated` also changes**, from `#ffffff` (identical to
`--bg-primary` — 1.00:1, indistinguishable) to `#fafbfd`: a value distinct
from the page canvas so a raised surface (modal/popover/tooltip) reads as
its own layer even where it happens to sit directly against the primary
background with no shadow rendered. It remains the brightest of the three
non-primary light surfaces (98.6 vs. 96.8/93.6 L*), preserving the "ascending
elevation from page → raised" comment already in `styles.css`. This token has
non-raised consumers too — `ConversationModeControl.tsx`'s active
segmented-control fill, `NotesPanel.tsx`'s inline chips,
`AudioSourceSelector.tsx`'s scope toggle buttons, and a few `.css`-module
fills — so the repaint is visible beyond modals/popovers/tooltips; none of
those consumers rely on `--bg-elevated` being *equal to* `--bg-primary`, only
on it being a light neutral fill, so the 1.035:1 nudge is a safe non-breaking
change for them. `src/styles.a11y.test.ts` regression-guards the
distinctness this fix depends on (`--bg-elevated` vs. `--bg-primary`
strictly `>1.02:1` in both themes); no before/after screenshots were taken
for this pass — a visual QA pass covering the segmented-control fill and
similar non-raised consumers in both themes is worth doing before the next
tier ticket lands more `--bg-elevated` consumers on top of this.

**Dark elevation model** (`.ag-card[data-elevation="raised"|"overlay"]`): a
surface-lightness step (`--bg-tertiary` → `--bg-elevated`) plus an
`inset 0 1px 0 rgba(255,255,255,0.06)` top highlight, with the drop shadow
demoted from the harsher `--shadow-2`/`--shadow-3` to the softer, more
"ambient" `--shadow-1` (raised) or the existing `--shadow-overlay` (overlay,
matching what floating popovers/tooltips already use). The literal white
inset is theme-independent by design — it reads as a highlight on the dark
surfaces it was tuned for, and is a harmless no-op on the near-white light
surfaces (nothing to highlight against).

### Mechanical consequence: the `--border-color` → `--edge` sweep

`border-border-color` (the Tailwind utility bridging `--border-color`) sits
at 57 sites across 16 already-migrated component files. This record repoints
those 57 to `border-(--edge)` — the same token the new recipes use — with one
exception: `TokenUsagePanel.tsx`'s dashed lifetime-section divider is a
rhythm cue between two `<fieldset>`s in the same panel, not an object
boundary, so it takes `border-(--edge-subtle)` (justified inline in the
component).

The 57-site count was a grep scoped to the exact `border-border-color`
utility name, which misses two other Tailwind forms of the same token: the
directional `border-b-border-color` variant (`App.tsx`'s right-panel
tablist bottom seam — a panel-header-style boundary, so it takes
`border-(--edge)` alongside `border-b`, matching `ControlBar.tsx`'s existing
`border-b border-(--edge)` pattern) and the `text-border-color` utility used
for three decorative `|` separator glyphs between peer stage groups in
`PipelineStatusBar.tsx` (`aria-hidden`, rhythm cues rather than object
boundaries, so they take `text-(--edge-subtle)`, justified inline in the
component). All four sites are swept in this record alongside the 57.
`--border-color` itself is untouched; its remaining 42 not-yet-migrated
`.css`-module sites are out of scope here and keep reading the old token
until those modules migrate.

### Consequences

- **Positive**: one place to fix a chip/card/label/field/panel-head visual
  bug instead of N re-derivations; the closed-variant-set discipline Badge
  proved for one primitive now covers six.
- **Positive**: `--edge` gives every future component boundary a
  contrast-correct default; the sweep (57 `border-border-color` sites plus
  the directional and text-color forms named above) retires `--border-color`
  from every already-migrated `.tsx` component in one pass.
- **Positive**: zero new runtime dependency, zero adoption risk in this
  change — the recipes ship provably inert (no `className` references them
  yet) and are exercised only by the new a11y test's raw CSS parse.
- **Negative**: a third channel is one more thing to explain to a new
  contributor; the ADR-0016 conventions clause now needs its banner note
  read alongside it.
- **Negative**: `.ag-card`/`.ag-field`/`.ag-panel-head`'s exact adoption
  shape (e.g., whether `.ag-field`'s `data-layout="row"` variant is the right
  shape for the preflight checklist) is unverified until a real component
  adopts it — this record intentionally does not force that proof into the
  same change as the token/recipe definitions.
- **Neutral**: `--border-color` is not deleted; it keeps its current meaning
  for the 42 sites this record does not touch.

## Pros and Cons of the Options

### Extend Badge's JS pattern per-primitive
- Good: same guarantee shape (typed union, default fallback) contributors
  already know from Badge.
- Bad: six new components/props contracts for what is, in every case, a
  fixed CSS ruleset keyed by one attribute — adds render indirection with no
  behavioral payoff; none of these primitives need JS logic.

### Stay on two channels, keep solving this in BEM
- Good: no ADR-0016 amendment; no new selector convention.
- Bad: exactly the duplication this record exists to end — the four
  consuming files already re-derive the same section-label/status-chip
  shape independently, and `settings.css` alone would grow another few
  hundred lines other files still couldn't share without an import.

### Populate `@layer components` with closed `.ag-*` recipes (chosen)
- Good: uses the layer ADR-0016 already declared; attribute-selector
  variants are exhaustive-by-construction (base rule = safe fallback); zero
  runtime cost.
- Bad: a third channel needs its own boundary rule (which primitives get a
  recipe vs. stay bespoke) — not yet formally written down beyond "recurs
  across many components" (left to review judgment per addition, as ADR-0016
  already does for one-off bracket values).

## More Information

Amends [ADR-0016](0016-adopt-tailwind-v4-incremental.md)'s 2026-06-29
conventions clause ("two channels only"); the new tokens extend
[ADR-0009](0009-design-token-system-and-theming.md)'s surface/border
section. WCAG 1.4.11 findings appended to
`docs/reviews/wcag-contrast-audit.md`. Design basis:
`docs/agentic-runs/2026-08-20-ui-overhaul-design/{ui-design,
recomposition-plan}.md`, ticket T2. Real contrast math for both the ratios
above and the light/dark surface set is asserted in
`src/styles.a11y.test.ts`. Execution seed: `audio-graph-b9dc`.
