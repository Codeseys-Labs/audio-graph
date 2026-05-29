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

## Residual Risk

This was a static color audit, not a full screen-reader or keyboard navigation
review. The next accessibility pass should cover focus order, ARIA labels,
live-region behavior, the two untrapped `role="dialog"` overlays
(`App.tsx:291-317`), the 3 text inputs that still set `outline: none` on plain
`:focus` (they show a border change but no ring), and Playwright/axe coverage
once the desktop app can run in an environment with the required Tauri system
libraries.
