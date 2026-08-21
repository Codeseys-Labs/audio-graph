# WCAG Contrast Audit

Date: 2026-05-17

Scope: static audit of the dark-theme palette and the high-use text/control
pairs in `src/styles.css` and `src/App.css`. The pass focused on WCAG 2.1 AA
normal-text contrast (`4.5:1`) for labels, buttons, transcript/chat text, and
toast/banner content.

## Findings

| Pair | Before | After | Status |
|---|---:|---:|---|
| `--text-muted` on `--bg-tertiary` | 2.35:1 | 4.52:1 | Fixed |
| `--text-muted` on `--bg-secondary` | 2.99:1 | 5.75:1 | Fixed |
| `--accent-purple` text on `--bg-primary` | 3.96:1 | 6.71:1 | Fixed |
| user chat / primary blue fill | 2.54:1 with white text | 7.15:1 with `--on-accent-blue` | Fixed |
| red stop/error fill | 3.83:1 with white text | 6.85:1 with `--on-accent-red` | Fixed |
| active purple fill | 4.31:1 with white text | 7.19:1 with `--on-accent-purple` | Fixed |
| success/info toast fills | 3.37:1 / 3.14:1 with white text | 9.82:1 / 12.34:1 with paired on-colors | Fixed |
| blue count/badge labels on tinted blue backgrounds | 4.37:1 | 4.80:1 with lower tint alpha | Fixed |
| danger settings hover | 4.18:1 | 4.79:1 with lower hover alpha | Fixed |

Storage and demo banners already passed with white text on their darker fills
(`4.93:1` and `6.07:1`) and were left visually unchanged.

## Changes

- Raised `--text-muted` and `--text-secondary` so small helper text remains
  readable on all three dark background layers.
- Added explicit `--on-accent-*` foreground variables for filled blue, red,
  green, yellow, and purple controls.
- Updated chat bubbles, primary buttons, stop/transcribe active states, and
  toast variants to use paired foreground colors instead of assuming white
  text works on every accent.
- Defined the Gemini accent token and reduced blue/red translucent fills where
  small badge and hover text needed a little more contrast margin.

## Follow-up — 2026-05-29 (Wave 1, ADR-0009)

A deep-dive pass (`docs/reviews/2026-05-29-uiux-deep-dive.md`) found that the
post-refresh `--text-muted` (`#6f7a8c`) still failed AA on the lightest dark
surface:

| Pair | Value | Status |
|---|---:|---|
| `#6f7a8c` on `--bg-primary` `#0e1117` | 4.35:1 | borderline (fails 4.5:1) |
| `#6f7a8c` on `--bg-secondary` `#151a23` | 4.02:1 | fails |
| `#6f7a8c` on `--bg-tertiary` `#1d2430` | 3.59:1 | fails |

Raised `--text-muted` to **`#868fa0`**, which passes AA on all three surfaces:

| Pair | Value | Status |
|---|---:|---|
| `#868fa0` on `#0e1117` | 5.80:1 | pass |
| `#868fa0` on `#151a23` | 5.36:1 | pass |
| `#868fa0` on `#1d2430` | 4.79:1 | pass |

Also in Wave 1: removed the stale divergent `var(--token, FALLBACK)` fallbacks
in `App.css` (they encoded an abandoned palette and would have resurfaced on any
token rename), added a global `:focus-visible` ring (WCAG 2.4.7) and a
`prefers-reduced-motion` guard (WCAG 2.3.3) in `src/styles.css`.

## Follow-up — 2026-08-20 (non-text contrast, WCAG 1.4.11, ADR-0047)

The previous waves audited *text* contrast (WCAG 1.4.3/1.4.6). This pass
audits **non-text contrast** (WCAG 1.4.11 — "visual information required to
identify UI components") for the border token every component boundary was
drawing from: `--border-color`.

### Baseline (measured, failing)

| Pair | Ratio | Status |
|---|---:|---|
| dark `--border-color` (`#2a3342`) on `--bg-primary` (`#0e1117`) | 1.49:1 | fails (needs 3:1) |
| dark `--border-color` on `--bg-secondary` (`#151a23`) | 1.37:1 | fails |
| dark `--border-color` on `--bg-tertiary` (`#1d2430`) | 1.23:1 | fails |
| dark `--border-color` on `--bg-elevated` (`#232c3a`) | 1.11:1 | fails |
| dark `--bg-tertiary` on `--bg-secondary` (context: two adjacent surfaces, not a border) | 1.12:1 | flat |
| light `--border-color` (`#d4dae3`) on `--bg-tertiary` (`#e9edf3`) | 1.20:1 | fails |
| light `--bg-elevated` vs. light `--bg-primary` (both `#ffffff`) | 1.00:1 | indistinguishable |

### Fix

`--border-color` is not raised in place — it still backs 42 sites in the
not-yet-migrated `settings.css`/`layout.css`/`primitives.css`/
`shortcuts-modal.css` modules, out of this pass's scope, and those callers
have not been re-verified against a changed value. Instead, two new semantic
tokens (ADR-0047) take over the "border needs to be visible" job at every
already-migrated call site:

| Token | Role | Dark value | Light value |
|---|---|---|---|
| `--edge` | Component boundaries — must clear 3:1 against every surface | `#6b7a92` | `#7285a1` |
| `--edge-subtle` | Decorative-only rules — WCAG-exempt, landed at ~1.5:1 by choice | `#37455a` | `#b6bfce` |

Computed ratios against all four surfaces (`--bg-primary/-secondary/
-tertiary/-elevated`), both themes — real relative-luminance math, also
asserted from the literal `styles.css` values by
`src/styles.a11y.test.ts`:

| Theme | Token | vs primary | vs secondary | vs tertiary | vs elevated |
|---|---|---:|---:|---:|---:|
| dark | `--edge` | 4.34:1 | 4.01:1 | 3.58:1 | 3.23:1 |
| dark | `--edge-subtle` | 1.95:1 | 1.80:1 | 1.60:1 | 1.45:1 |
| light | `--edge` | 3.76:1 | 3.48:1 | 3.20:1 | 3.63:1 |
| light | `--edge-subtle` | 1.85:1 | 1.71:1 | 1.58:1 | 1.79:1 |

`--edge` clears 3:1 against all four surfaces in both themes, with real
margin (the tightest case is dark-vs-elevated at 3.23:1 and light-vs-tertiary
at 3.20:1 — the surfaces closest in lightness to a boundary-appropriate
gray in each theme).

Light `--bg-elevated` also moves from `#ffffff` to `#fafbfd` — distinct from
`--bg-primary` (no longer 1.00:1) while remaining the brightest of the three
non-primary light surfaces, so raised surfaces keep reading as "raised" even
where no shadow is rendered.

**Where this landed:** the 57 `border-border-color` Tailwind-utility call
sites across the 16 already-migrated component files now read
`border-(--edge)`, except one — `TokenUsagePanel.tsx`'s dashed
lifetime-section divider, which separates two sections of the *same* panel
rather than marking an object boundary, so it takes `border-(--edge-subtle)`
(justified inline in the component). The utility-name grep that produced the
57-site count missed two other Tailwind forms of the same token: the
directional `border-b-border-color` variant (one site, `App.tsx`'s
right-panel tablist bottom seam — an object boundary, so `border-(--edge)`)
and the `text-border-color` utility used for three decorative separator
glyphs (`PipelineStatusBar.tsx` — rhythm cues, so `text-(--edge-subtle)`).
Both forms are swept in this pass alongside the 57, so `--border-color` is
now retired from every already-migrated `.tsx` component; the token itself
is untouched everywhere else.
**Out of scope, deliberately:** the `.banner-on-accent` focus ring and the
saturated `--banner-demo-bg`/`--banner-storage-bg` fills — those are a
separate, already-documented 1.4.11 case (the "Focus ring on saturated
banners" comment in `styles.css`) and are not touched by this pass. The 42
not-yet-migrated `.css`-module sites named above are also out of scope until
those modules migrate onto the token bridge (T7 territory).

## Follow-up — 2026-08-20 (OKLCH accent re-derivation, UI-T3/audio-graph-99aa)

Ticket UI-T3 (ratified decision D3) re-derived all 7 accent/on-accent pairs in
OKLCH, both themes, at a fixed target lightness/chroma per theme (dark
L\*72/C.16, light L\*54/C.17, guidance range ±2 L\*/±.01-.02 C) while holding
each hue fixed — except `--accent` (pushed to a decisive ~265deg) and
`--accent-gemini` (widened from ~163deg to ~178deg, true teal), which move on
purpose. `--accent-blue` (previously ~11-12deg from `--accent`, with roughly
a hundred call sites split across the two names — see the "call-site count"
note below, the exact figure is not independently verified) is no longer an
independent value: it is
now a hard alias of `--accent` (`--accent-blue: var(--accent)`), so its row
below mirrors `--accent`'s numbers exactly — that is the point of the alias.
The six literal `--on-accent-*` declarations are byte-identical to their
pre-99aa values; only the fills move (see the "`--on-accent-*` foregrounds"
note further down for the one nuance that follows from the alias, not from
an edited literal).

The 4.5:1 AA floor is the one `src/styles.a11y.test.ts` already enforced for
`--accent-blue` alone before this change; it now enforces all 7 pairs, both
themes, and never regresses below it (extension, not a weakening) — see the
"semantic accent foregrounds" describe block.

**Dark theme:**

| Token | Old hex | New hex | Old ratio | New ratio | Hue Δ |
|---|---|---|---:|---:|---:|
| `--accent` | `#6c8cff` | `#78a1ff` | 6.13:1 | 7.48:1 | -4.4deg (270→265) |
| `--accent-red` | `#ff6b85` | `#f77589` | 6.85:1 | 7.00:1 | +0.3deg (preserved) |
| `--accent-green` | `#45d483` | `#39c175` | 8.96:1 | 7.38:1 | -0.1deg (preserved) |
| `--accent-gemini` | `#34d399` | `#00bfa5` | 8.88:1 | 7.31:1 | +15.1deg (163→178) |
| `--accent-blue` | `#5b9dff` (own hue, ~258deg) | `var(--accent)` = `#78a1ff` | 6.68:1 | 7.48:1 | now == `--accent`'s hue |
| `--accent-yellow` | `#ffcc4a` | `#cc9d00` | 12.35:1 | 7.41:1 | +0.3deg (preserved) |
| `--accent-purple` | `#b98cff` | `#b58af9` | 7.19:1 | 6.94:1 | -0.1deg (preserved) |

**Light theme (as originally shipped — see the gate-fix follow-up below for
the corrected `--accent`/`--accent-green`/`--accent-gemini` values that
actually landed; this table is kept for the fill/on-fill history):**

| Token | Old hex | New hex (as first shipped) | Old ratio | New ratio | Hue Δ |
|---|---|---|---:|---:|---:|
| `--accent` | `#3a5bd9` | `#3d66d0` | 5.71:1 | 5.24:1 | -2.6deg (268→265) |
| `--accent-red` | `#c8324b` | `#bd364a` | 5.23:1 | 5.54:1 | +0.1deg (preserved) |
| `--accent-green` | `#0f7a3d` | `#008541` | 5.42:1 | 4.74:1 | +0.2deg (preserved) |
| `--accent-gemini` | `#0f7a55` | `#00816f` | 5.34:1 | 4.81:1 | +15.5deg (163→178) |
| `--accent-blue` | `#1f6fe0` (own hue, ~258deg) | `var(--accent)` = `#3d66d0` | 4.76:1 | 5.24:1 | now == `--accent`'s hue |
| `--accent-yellow` | `#d98e00` | `#c27f00` | 6.57:1 | 5.32:1 | +0.4deg (preserved) |
| `--accent-purple` | `#7c3aed` | `#7555c7` | 5.70:1 | 5.41:1 | -0.3deg (preserved) |

**Deviations from the L\*/C guidance, both documented and both resolved in
favor of the binding contrast constraint per the ticket's own tie-break:**

- **Gamut, not guidance, forces chroma down for dark `--accent`, dark
  `--accent-gemini`/light `--accent-gemini`, and light `--accent-green`.**
  No in-gamut sRGB color exists at the guidance chroma for a blue-violet
  this light, a teal this light, or a green this saturated at the target
  lightness — hue and contrast are both unaffected; only chroma clips below
  the ±.01-.02 band (e.g. light `--accent-gemini` originally landed at
  C≈0.099 against a C.17 target). **Correction (gate-fix review):** light
  `--accent` does **not** clip — at L\*54/H265.2 the max in-gamut sRGB
  chroma is ≈0.2546, and the shipped `#3d66d0` sits at C≈0.170, exactly the
  guidance target, so the original bullet's "`--accent`/`--accent-gemini`
  (both themes)" phrasing overclaimed for the light side. Conversely, dark
  `--accent-yellow` (`#cc9d00`, C≈0.147) *is* marginally below the
  .16±.01 band (max in-gamut chroma at that L/H is ≈0.1473) and was missing
  from this bullet.
- **Light `--accent-yellow` is the one true guidance-vs-contrast conflict
  in the original fill/on-fill derivation.** At the guidance L\*54, the
  maximum in-gamut chroma for yellow's hue only reaches ~0.115-0.12, and
  against the unchanged `--on-accent-yellow` (`#221700`) that computes
  3.4:1 — under the 4.5:1 floor. Contrast wins: light `--accent-yellow` is
  derived at L\*65 instead (still hue-preserved), clearing 5.32:1.

**Call-site count:** the "163 call sites split with `--accent`'s own"
figure originally recorded here could not be independently reproduced
(varying grep methodologies converge on roughly 90-176, same order of
magnitude, not an exact match) and should be read as an estimate, not a
verified count — the alias fix itself does not depend on the exact number.

**`--on-accent-*` foregrounds:** the six surviving literal declarations are
byte-identical to their pre-99aa values, but `--on-accent-blue` no longer
*resolves* to its old dark literal (`#061629`) — as `var(--on-accent)` it
now resolves to `#0a1026`. Every consumer pairs it with an accent-blue fill,
so this lands at 7.48:1 dark / 5.24:1 light (both above the floor, and the
dark case above the old pairing's 6.68:1) — a correct outcome of the alias,
just not literally "unchanged."

**Zero component edits by construction:** every existing `bg-accent-blue`,
`text-accent-blue`, `border-(--accent-blue)`, etc. call site keeps
resolving through the same token name; only the value it resolves to
changed. No `.tsx`/`.css` file outside `src/styles.css` was touched for
this fill/on-fill section (the gate-fix follow-up below does touch two
`.tsx` files, for unrelated reasons named there).

## Follow-up — 2026-08-20 (gate-fix review, UI-T3/audio-graph-99aa)

A review of the OKLCH re-derivation above found it only verified the
fill/on-fill pairing. Two defects followed from that gap:

### Blocker: light-theme accent-as-text dropped below 4.5:1

`--accent`, `--accent-green`, and `--accent-gemini` are also rendered
directly as **text** color in the light theme, on ordinary surfaces and
tints, not just as fills paired with `--on-*`. At the original L\*54 several
real call sites dropped under the 4.5:1 floor:

| Site | Pairing | Old ratio | Shipped (broken) ratio |
|---|---|---:|---:|
| `ConversationModeControl.tsx` `ENGINE_ACTIVE` | `--accent` text on `--tint-accent` | 4.64:1 | 4.25:1 |
| `ConversationModeControl.tsx` `BADGE_ACTION` | `--accent` text on `--bg-tertiary` | 4.86:1 | 4.46:1 |
| `ProjectionRuntimeStatusPanel.tsx:229` | `--accent-green` text on `--tint-success` | 4.81:1 | 4.21:1 |
| `AgentProposalsPanel.tsx:210` | `--accent-green` text on `--bg-secondary` | 5.01:1 | 4.38:1 |
| `TokenUsagePanel.tsx:478` (`ddTotal`) | `--accent-gemini` text on `--bg-tertiary` | 4.54:1 | 4.09:1 |

Fix: darkened light `--accent` (`#3d66d0`→`#3a62cc`), `--accent-green`
(`#008541`→`#00793b`), and `--accent-gemini` (`#00816f`→`#007363`) further,
holding hue fixed, until every real text-on-surface call site above clears
4.5:1 with margin (verified against the darkest surface/tint each token is
actually rendered on, not just the fill test):

| Token | New hex | vs `--bg-tertiary` | vs its tightest real tint | vs white (`--on-*`) |
|---|---|---:|---:|---:|
| `--accent` | `#3a62cc` | 4.71:1 | 4.67:1 (`--tint-accent`) | 5.53:1 |
| `--accent-green` | `#00793b` | 4.71:1 | 4.91:1 (`--tint-success`) | 5.53:1 |
| `--accent-gemini` | `#007363` | 4.92:1 | 4.67:1 (`--tint-gemini`) | 5.78:1 |

Dark theme was never at risk (fills sit at 7.0-7.5:1 with headroom); only
the light-theme hexes above changed. Hue held fixed (<0.25deg drift, same
class as the rounding noise elsewhere in this document). Side effect:
`--accent-green` and `--accent-gemini` clip chroma slightly harder at the
new, darker lightness (max in-gamut chroma shrinks as L drops for these
hues) — contrast wins per the same tie-break already used for light
`--accent-yellow`. `src/styles.a11y.test.ts` gained a
"accent-as-text on realistic surfaces" describe block asserting these
specific pairings so this defect class can't silently regress again.

### Major: stale tint tokens still pointed at retired accent hues

`--tint-accent`, `--tint-purple`, `--tint-gemini` (RGB channels), and
`--tint-danger-icon`'s dark rgba, plus light `--divider-hover`, were
literal copies of the **pre-99aa** accent hex/rgb and never moved with the
re-derivation. Most visibly: `--accent-gemini` moved 163deg→178deg
specifically so it stops reading as green, yet `ControlBar.tsx`'s
Gemini-active hover state rendered new teal text on the old green
`--tint-gemini` wash, and `TokenUsagePanel.tsx:491` hardcoded the retired
green as an inline `rgb(52 211 153/…)` literal behind teal
`text-accent-gemini`. Fixed: the dark rgba tokens now carry the current
accent RGB at their original alpha; the light opaque equivalents are
recomputed the same way the pre-existing `--tint-danger`/`--tint-success`
pairs already were (composited over white at each token's own effective
alpha, then reapplied to the new RGB); `--divider-hover` now mirrors
`--accent`; the `TokenUsagePanel.tsx` inline literal now reads
`bg-(--tint-gemini)`. `--tint-danger-icon`'s light value is intentionally
shared with `--tint-danger` (not accent-red-derived) and was left alone.

### Minor: motion rendered delta not enumerated

The original motion-scale change enumerated only the 8 `transition-all`
rewrites (explicit component edits), not the one site of *alias fallout*:
`src/styles/primitives.css:166`'s `.notification` entrance animation is the
only `var(--ease-out)` consumer in the repo (`settings.css` has none, unlike
the styles.css comment's prior claim) and the only `var(--motion-slow)`
consumer — its rendered easing moves
`cubic-bezier(0.16,1,0.3,1)`→`cubic-bezier(0.2,0,0.2,1)` and duration
`0.25s`→`0.26s`.

## Residual Risk

This was a static color audit, not a full screen-reader or keyboard navigation
review. The next accessibility pass should cover focus order, ARIA labels,
live-region behavior, the two untrapped `role="dialog"` overlays
(`App.tsx:291-317`), the 3 text inputs that still set `outline: none` on plain
`:focus` (they show a border change but no ring), and Playwright/axe coverage
once the desktop app can run in an environment with the required Tauri system
libraries.
