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

## Residual Risk

This was a static color audit, not a full screen-reader or keyboard navigation
review. The next accessibility pass should cover focus order, ARIA labels,
live-region behavior, the two untrapped `role="dialog"` overlays
(`App.tsx:291-317`), the 3 text inputs that still set `outline: none` on plain
`:focus` (they show a border change but no ring), and Playwright/axe coverage
once the desktop app can run in an environment with the required Tauri system
libraries.
