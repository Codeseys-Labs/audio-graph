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

## Residual Risk

This was a static color audit, not a full screen-reader or keyboard navigation
review. The next accessibility pass should cover focus order, ARIA labels,
live-region behavior, and Playwright/axe coverage once the desktop app can run
in an environment with the required Tauri system libraries.
