# UI/UX Deep Dive + Improvement Plan

Date: 2026-05-29
Scope: the React/Tauri frontend in `src/` — design system, interaction flows,
accessibility, internationalization, and component architecture.
Method: full canvas of `src/` (App shell, ~30 components, Zustand store, hooks,
i18n, CSS) cross-referenced against established UI/UX practice (W3C DTCG design
tokens, WCAG 2.2 AA, streaming-response / loading-state patterns).

This document is the durable output of a deep-dive pass. It records **what we
have**, **what's wrong**, and a **prioritized, waved plan** to fix it. Three
durable decisions are split out into proposed ADRs:

- ADR-0009 — Layered design-token system + theming.
- ADR-0010 — Icon system (lucide-react) replacing emoji iconography.
- ADR-0011 — Unified feedback/notification system.

---

## 1. What we have (canvas)

**Stack:** React 18 + Zustand 5 + i18next, `react-force-graph-2d` for the graph,
Tauri v2 events as the backend bus. No UI component library, no CSS framework,
no icon library.

**Layout (`src/App.tsx`):** desktop-first 3-column shell — left sources/speakers,
center graph + notes, right tabbed transcript/chat — with drag-resizable panels
persisted to `localStorage`, plus overlays (Settings, Sessions, Shortcuts,
Express Setup, Agent proposals, Token usage) and two notification mechanisms.

**Styling:** one 78-line `styles.css` (`:root` color tokens + reset) and one
**2,695-line monolithic `App.css`**. Single dark theme. Iconography is **100%
emoji / Unicode dingbats** inline in JSX.

**Strengths worth preserving:**

- Disciplined BEM-ish class naming; logically sectioned CSS.
- A real focus-trap hook (`useFocusTrap.ts`) wired into the 4 primary modals.
- Good `aria-live` coverage on the transcript log, toasts, and storage banner.
- Genuinely good empty states in `ChatSidebar` (example prompts) and `NotesPanel`.
- Strong error-recovery in `StorageBanner` (retry keeps banner + reason).
- i18next set up correctly with en/pt at full key parity (224 keys each).
- A prior contrast audit (`wcag-contrast-audit.md`, 2026-05-17) already added
  paired `--on-accent-*` foreground tokens.

---

## 2. Findings (what's wrong)

### 2.1 Design system — fragmented and self-contradictory

- **Ghost palette via divergent fallbacks (highest-leverage cleanup).**
  `App.css` overwhelmingly writes `var(--token, FALLBACK)` where the FALLBACK is
  from an **older, abandoned palette** and no longer matches `styles.css`:
  `--bg-primary` real `#0e1117` vs fallback `#1a1a2e`; `--text-primary` real
  `#e7ebf2` vs `#e0e0e0` (~23×); `--border-color` real `#2a3342` vs `#2a2a4a`
  (~31×); `--accent-red` real `#ff6b85` vs `#e94560`; etc. The real token wins
  at runtime, so these are latent — but the file documents a UI we don't ship,
  and any token rename would snap the app back to a 2018-era navy/purple theme.
- **Color is the only tokenized axis.** No spacing, radius, typography, shadow,
  z-index, or motion scales. ~252 hex literals + 81 `rgba()` literals; font
  sizes span 18 distinct values mixing `px` and `rem`; border-radius uses 10
  distinct values (`1/2/3/4/6/8/10/12px`, `50%`, `999px`); z-index is magic
  numbers (`40/41`, `1099/1100`, three overlays all at `1000`).
- **No shared button base** — ~10 bespoke button rules re-declare
  padding/radius/font/transition with inconsistent values.
- **Single dark theme only** — no `prefers-color-scheme`, no `color-scheme`, no
  toggle. Adding light mode today means touching ~333 hardcoded color literals.
- **No responsiveness** — zero `@media` queries; fixed pixel column widths.
  Acceptable for desktop, but no small-window handling at all.

### 2.2 Accessibility — real WCAG 2.2 AA gaps

- **No visible focus indicator on buttons (WCAG 2.4.7, AA fail).** Only 3
  `:focus` rules exist, all on text inputs; inputs even `outline: none` with
  only a border-color change. Keyboard users get no ring on the dozens of
  `<button>`s.
- **Contrast residue.** `--text-muted #6f7a8c` on `--bg-primary #0e1117` ≈ 4.0:1
  — below 4.5:1 for normal text, and used pervasively. The 2026-05-17 audit
  fixed it against `bg-secondary/tertiary` but not `bg-primary`. Backpressure
  pill (`#7a4a00` on `#fff4d6`) is borderline.
- **Two overlays announce as dialogs but trap nothing.** The Agent-proposals
  and Token-usage overlays (`App.tsx:291-317`) are `role="dialog"` with **no
  focus trap, no `aria-modal`, no focus restore, and no Escape handler** — only
  a scrim click closes them (mouse-only).
- **No `prefers-reduced-motion`.** Three infinite animations (`pulse-recording`,
  `pulse-backpressure`, `chat-dot-bounce`) always run.
- **Emoji-only buttons missing `aria-label`:** source Refresh `🔄`, chat Send
  `➤`, chat Clear `🗑️` (settings `⚙️` has one). Emoji can't be recolored,
  render inconsistently across OS, and `✕`-close is reimplemented 8+ times.
- **Documented `Cmd/Ctrl+/` help shortcut isn't registered** in
  `useKeyboardShortcuts.ts` (only a local listener in `App.tsx` covers it; the
  hook handles R, `,`, Shift+S, Escape).

### 2.3 Internationalization — partial (~33% of components)

- Only 8 of 24 components use `useTranslation`. The **always-on main chrome**
  (transcript, pipeline status bar, chat, toasts, app tabs, most provider
  settings panels) is hardcoded English, so a `pt` user sees a mixed UI. Some
  files (e.g. `ControlBar`) mix `t()` and hardcoded strings in the same render.
- No language-switcher UI (auto-detect only). No RTL support.

### 2.4 Interaction flows — friction and dead-ends

- **Onboarding dead-ends.** ExpressSetup's "Save & Start" starts nothing; it
  closes onto an idle app, and the three pipeline controls (Transcribe / Gemini)
  are invisible until capture is already running — the value proposition is
  hidden exactly when a new user is deciding what to do.
- **Hidden core capability.** The top-bar Gemini button only appears when
  `nativeS2sEnabled` is on — a checkbox buried in Settings → Gemini. Users who
  add a Gemini key in Express never see the button and get no hint why.
- **No loading/in-flight feedback** on the primary controls
  (Start/Stop/Transcribe/Gemini) or on async source enumeration; double-click
  risk, and "is it working?" ambiguity.
- **Misleading empty state.** `LiveTranscript` shows "Waiting for speech…" even
  when Transcribe was never started — implies listening when it isn't.
- **Two competing notification systems.** A persistent store-`error` toast
  (manual dismiss) vs a transient auto-dismiss `Toast` (single-slot, silently
  overwrites). Which one fires depends on whether an error was "classified," and
  the persistent toast renders *behind* open modals.
- **No unsaved-changes guard / no save confirmation** in Settings — Escape or
  scrim click silently drops all edits; Save shows no success state.
- **Graph has no click-to-inspect** (hover-only tooltips, keyboard-inaccessible)
  and no search/filter as it grows.
- **Shipped placeholders/stale artifacts:** non-existent model id
  `gemini-3.1-flash-live-preview`, OpenRouter code/comment mismatch, contradictory
  TTS comment in Settings.

---

## 3. Proposed decisions (see ADRs)

- **ADR-0009 — Layered design tokens + theming.** Adopt a two-layer token model
  (primitive → semantic) covering color, space, radius, type, shadow, z-index,
  and motion as CSS custom properties on `:root` + `[data-theme]`; delete the
  divergent `App.css` fallbacks; wire `color-scheme` and `prefers-color-scheme`;
  ship dark + light. Keep it hand-authored CSS (no Style Dictionary) until a
  second platform or designer pipeline justifies it.
- **ADR-0010 — Icon system.** Add `lucide-react` and a single `<Icon>` /
  `<IconButton>` component; replace all emoji/dingbat glyphs; one accessible,
  recolorable close button.
- **ADR-0011 — Unified feedback system.** One store-owned notification queue
  with typed severities (info/success/warning/error), per-item persistence vs
  auto-dismiss, stacking, `aria-live`, and correct z-index above modals; retire
  the dual toast/error-toast split.

---

## 4. Prioritized plan (waves)

Waves are ordered so foundations land before the work that depends on them.
Within a wave, items are parallelizable. Sizes: S < 0.5d, M ~1d, L ~2-3d.

### Wave 1 — Design-system foundation (unblocks everything visual)

| Item | Size | Notes |
|---|---|---|
| W1.1 Expand token layer: add `--space-*`, `--radius-*`, `--font-size-*`, `--shadow-*`, `--z-*`, `--motion-*` primitives + semantic aliases (ADR-0009) | M | Source of truth in `styles.css`. |
| W1.2 Delete/realign all divergent `var(--x, FALLBACK)` fallbacks in `App.css` | M | Mechanical but high-value; removes ghost palette. Verify visually unchanged. |
| W1.3 Global `:focus-visible` ring + reduced-motion media query | S | Fixes WCAG 2.4.7 + 2.3.3 in one pass. |
| W1.4 Fix `--text-muted` contrast on `bg-primary` (raise to ≥4.5:1) | S | Update `wcag-contrast-audit.md`. |

### Wave 2 — Component primitives (depends on W1)

| Item | Size | Notes |
|---|---|---|
| W2.1 Add `lucide-react`; build `<Icon>` + `<IconButton>` (ADR-0010) | M | Vite tree-shakes named imports. |
| W2.2 Replace emoji across all components with `<Icon>`; single accessible close button | L | Removes `line-height:1` patches; adds `aria-label`s. |
| W2.3 Shared `<Button>` base (variant/size props → token-driven CSS) | M | Collapse ~10 bespoke button rules. |
| W2.4 Unified notification store + `<Notifications>` host (ADR-0011) | M | Retire dual toast/error systems; queue + stack + above-modal z-index. |

### Wave 3 — Flow fixes (depends on W2 primitives)

| Item | Size | Notes |
|---|---|---|
| W3.1 Loading/pending states on Start/Stop/Transcribe/Gemini + source fetch | M | Disable-while-pending; spinner in `<Button>`/`<IconButton>`. |
| W3.2 Make Gemini control discoverable — auto-enable native-S2S when a Gemini key is present, or always show with a "configure" affordance | S | Removes the worst dead-end. |
| W3.3 Onboarding hand-off — after Express "Save", guide to select source → Start; surface pipeline controls pre-capture (disabled w/ hint) | M | Fix the value-prop-hidden problem. |
| W3.4 `LiveTranscript` empty state distinguishes "not started" vs "no speech yet" with a CTA | S | |
| W3.5 Settings: unsaved-changes guard + explicit save confirmation | M | Prevent silent data loss. |
| W3.6 Graph: click-to-inspect node detail panel (keyboard reachable) + node search/filter | L | Biggest "wow" for the core surface. |
| W3.7 Remove stale artifacts (bad model id, OpenRouter mismatch, contradictory comments) | S | |

### Wave 4 — Theming + i18n completeness (depends on W1/W2)

| Item | Size | Notes |
|---|---|---|
| W4.1 Ship light theme via semantic-token swap + system-pref detection + toggle in Settings | L | Desaturate accents for light; define surface elevation levels. |
| W4.2 i18n sweep — externalize remaining ~16 components; add language switcher in Settings | L | Reach ~100% chrome coverage. |
| W4.3 (Optional) RTL groundwork — logical properties + `dir` wiring | M | Defer until a RTL locale is actually planned. |

### Wave 5 — CSS architecture hygiene (lowest urgency)

| Item | Size | Notes |
|---|---|---|
| W5.1 Split `App.css` per section (or CSS Modules) co-located with components | L | Pure maintainability; do last so churn doesn't fight earlier waves. |
| W5.2 Dead-rule audit after splits | S | |

---

## 5. Verification per wave

- **Build/test:** `bun run typecheck`, `bun run test`, `bun run tauri dev` smoke.
- **A11y:** keyboard-only pass (focus ring visible everywhere, all overlays
  trap + Escape-close + restore focus); axe/Lighthouse once Tauri can run with
  system libs; re-run the contrast pairs in `wcag-contrast-audit.md`.
- **i18n:** switch to `pt`, confirm no English leaks in main chrome.
- **Visual regression:** before/after screenshots of each panel after W1.2 to
  prove the fallback cleanup changed nothing.

---

## 6. Sequencing rationale

Wave 1 is the keystone: every later visual change should consume tokens, so the
token layer and fallback cleanup must land first. Icons and the button/feedback
primitives (Wave 2) are the substrate the flow fixes (Wave 3) build on. Theming
and i18n (Wave 4) are only cheap *after* tokens exist and strings are
externalized. CSS file-splitting (Wave 5) is deferred so it doesn't create merge
churn against the earlier, higher-value edits.
