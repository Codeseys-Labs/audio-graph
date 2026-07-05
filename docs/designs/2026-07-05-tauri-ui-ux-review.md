# AudioGraph — Tauri UI/UX Review & Ranked Improvement Plan

**Date:** 2026-07-05
**Author:** UX Architect (review synthesis)
**Scope:** The During/After/Analysis shell (seed d633, shipped a0e8666), first-run, ControlBar,
empty states, native-Tauri capability delta, and the local-first / privacy product thesis.
**Inputs:** live visual audit (`/tmp/ux-review/shots/`), read-only frontend code audit,
Tauri v2 capability research, competitor + WCAG pattern research.
**Anchors:** ADR-0009 (tokens/theming), ADR-0010 (icons), ADR-0011 (unified feedback),
ADR-0016 (Tailwind v4 token bridge), and `docs/designs/2026-07-04-during-after-shell.md`.

---

## 1. Current-State Assessment

### 1.1 What the d633 shell already gets right

The stage-based shell has landed and holds up to its own design intent:

- **During reads notes-first.** Desktop During is a 2-column grid — NOTES primary (~640px left),
  LIVE TRANSCRIPT secondary (right), no graph, no projection diagnostics
  (`during-1440.png`, confirmed via `aria-selected`). This is the core d633 win.
- **The graph is confined to Analysis.** During/After never render `KnowledgeGraphViewer`;
  Analysis shows the graph hero + notes split + Transcript/Chat rail (`analysis-1440.png`).
- **Narrow reflow of the *workspace* is sound.** At 768px both the 1120px and 900px breakpoints
  engage; panels stack; `scrollWidth === clientWidth` (zero horizontal overflow) at every
  stage×width combination measured (`layout.css:201/223`).
- **Modals are clean** at both widths — Settings (readiness badges, sticky Save), Sessions,
  Shortcuts (`settings-*.png`, `sessions-*.png`, `shortcuts-1440.png`).
- **The token/a11y baseline is mature.** Complete token system (spacing/radius/type/shadow/z/
  motion + dark & light + `@theme inline` bridge), global `:focus-visible`, `.sr-only`,
  `prefers-color-scheme`, roving-tabindex tablists, focus traps, and a rich role inventory
  (29 status / 12 alert / 10 tab). `IconButton` enforces `aria-label` per ADR-0010.
- **Contrast passes AA everywhere measured** — idle-hint `#5c6677` on `#e9edf3` = 4.93:1;
  text-secondary on white = 8.09:1; toast body = 16.5:1. No contrast failures found.
- **Reduced-motion is already handled globally.** `styles.css:479` has a `prefers-reduced-motion:
  reduce` block that zeroes all `animation-duration`/`iteration-count`. *Correction to the
  pattern research:* the "no reduced-motion fallback for pulse-recording" finding is a
  **false positive** — the global rule already neutralizes the infinite pulse. No work needed.

### 1.2 Where it falls short (evidence-anchored)

The shell demoted diagnostics and confined the graph, but three classes of gap remain:

1. **The ControlBar never got the responsive treatment the shell design flagged.** The header
   `role=toolbar` (`ControlBar.tsx:205`) is a single-row `flex items-center justify-between
   h-[52px]` with **no `flex-wrap` and no media query**, while every sibling region has 1120/900
   breakpoints. Below ~1120px the center group is crushed (measured 397px available vs ~950px
   needed): the green **Start button (x237→273) is painted over by the Notes/Converse fieldset**,
   and the "Select audio sources" hint collides with the Agent button. The primary capture
   action is partially obscured at narrow width — violating the shell's own rule "narrow windows
   should stack panels without hiding the primary workspace action"
   (`during-768.png`; design doc lines 116–124).

2. **Raw developer errors leak to the user on two surfaces.** `Notifications.tsx:113–131` renders
   the legacy `error` string with **no auto-dismiss** (only queued items get the 4s timer), so
   `"Cannot read properties of undefined (reading 'invoke')"` is a sticky `role=alert`
   `aria-live=assertive` banner in every shot — a JS `TypeError` with no plain-language cause or
   recovery, that a screen reader interrupts to read aloud. `analysis-768.png` echoes the *same*
   verbatim TypeError inline in the PROJECTION DIAGNOSTICS panel. (In-browser this is env noise,
   but the *presentation pattern* — unmapped backend/IPC failure → raw stack string → permanent
   assertive banner — is a real product defect that will fire on any real IPC failure.)

3. **First-run is fragile and the During surface is still a cockpit relative to competitors.**
   ExpressSetup + the hand-off nudge gate on a `load_credential_presence_cmd` IPC probe whose
   throw is silently swallowed (`App.tsx:285`), so a probe failure lands the user on empty panels
   + a red error toast with *no* onboarding path (seed 75a1 already tracks time-to-first-note).
   And per competitor research (Granola/Fathom/Cluely), the During surface still shows Notes +
   Transcript + PipelineStatusBar simultaneously where competitors keep During to one primary
   surface and hide pipeline state until an error.

### 1.3 Confirmed code defects (from the read-only audit)

- **Invisible error background — `NotesPanel.tsx:157` uses `bg-(--tint-accent-danger)`, an
  undefined token.** Verified: `styles.css` defines `--tint-danger` / `--tint-danger-strong`
  but **not** `--tint-accent-danger`; grep confirms this is the sole call site. The synthesize-
  error alert renders with a transparent fill. One-token fix.
- **`SessionsBrowser.tsx` ghost-token fallbacks** — `border:"1px solid var(--border,#333)"` at
  :304/327/398. `--border` is not a token (`--border-color` is), so it **always** falls back to
  `#333` and ignores the light theme. Plus 13 inline `style={{}}` blocks (ADR-0016 violation).
- **ExpressSetup "Setup modes" section is hardcoded English** (`ExpressSetup.tsx:769–859`:
  aria-label "Setup modes", "Checking provider readiness…", "Data boundary", "Product path",
  "No blockers reported.", "Review sources") — breaks the pt locale on the **first-run** flow.
- **No skip link** anywhere in the shell (`App.tsx:494–666`); keyboard users tab through two
  banners + ControlBar + nav before reaching `<main>`.
- **`AudioSourceSelector` search input has no `aria-label`** and a hardcoded placeholder
  (`:659–663`) — placeholder is not an accessible name (WCAG 4.1.2).
- **`LoggingSettings.tsx:296/319` async results in a bare `<p>`** with no `role=status`/aria-live,
  inconsistent with every peer settings panel — SR users miss the apply/purge outcome.
- **Hardcoded z-index vs the `--z-*` ladder** — `DemoModeBanner:71` `z-[1099]`,
  `StorageBanner:83` `z-[1100]`, `PopoverOverlay:34/53`, `Tooltip:49`, `KnowledgeGraphViewer:718/743`.
- **Pasted mono font stack** at `LiveTranscript.tsx:288/333` instead of the `font-mono` utility.

---

## 2. Ranked Improvement List

Ranked by the thesis priority: **a first-run user's first 5 minutes and During-stage calm matter
most.** Seed mapping is called out; where an item belongs to an existing seed I **extend** it
rather than file a duplicate.

### Tier A — First-run + During calm (highest thesis leverage)

**A1. Fix the ControlBar narrow-width collision [extends 103a/d633].**
Add a header breakpoint to `ControlBar.tsx:205` mirroring the existing 1120/900 pattern
(`flex-wrap` or a two-row stack at ≤1120px), so the green Start button and audio-source hint are
never occluded. *Evidence:* `during-768.png`, `ControlBar.tsx:204–207`, design doc 116–124.
This is the shell's own unfixed rule; CSS-only, no logic. **S, executable.**

**A2. Map backend/IPC failures to friendly copy + auto-expire transient errors [extends d633].**
`Notifications.tsx:113–131` — stop rendering raw `error` strings verbatim; map known IPC/backend
failures to a plain-language cause ("The desktop backend isn't reachable") + a Retry/Details
affordance, and give transient probe errors the same auto-dismiss timer queued items get. Also
stop echoing the verbatim TypeError in the Analysis PROJECTION DIAGNOSTICS panel (`analysis-768.png`).
*Evidence:* `Notifications.tsx:113–131`, `analysis-768.png`. **M, executable.**

**A3. Probe-failure onboarding fallback [extends 75a1].**
When `load_credential_presence_cmd` throws (`App.tsx:285`), still surface a minimal "Get started /
Preview a sample session" path instead of empty panels + red toast. Belt-and-suspenders so a
single failed probe never leaves a first-run user with nothing. *Evidence:* `App.tsx:285`, visual
finding #7. Maps to 75a1's open "first-run recovery for provider health failures". **M, executable.**

**A4. Give the NOTES empty state the During hero treatment [extends 75a1].**
The During hero (notes-first column) has the weakest empty state — one grey run-on sentence, no
icon, no CTA (`NotesPanel`, visual finding #6), while Audio Sources and Live Transcript have
icon + cause + action. Per IBM Carbon first-use guidance, add an icon, positive-framing title,
and a single CTA (e.g. "Preview a sample session" → After). *Evidence:* `during-1440.png`,
carbondesignsystem.com empty-states pattern. **S, executable.**

**A5. Collapse PipelineStatusBar to a single ambient dot During capture [extends d633/392b].**
Per calm-tech (Amber Case P3) + NNG progressive disclosure: During should show a *composite*
health dot (green/degraded/error), expanding to per-stage dots only on error or in Analysis.
This is *one* calm-tech gap vs Granola/Fathom/Cluely — not the largest; research-ux-patterns §7
assigns the single largest calm-tech opportunity to demoting the live Transcript during capture
(see A7). *Evidence:* research-ux-patterns §1/§4/§7, `progressive-disclosure-nng`. Extends d633's
"move diagnostics into a Health drawer".
**M, executable** (component logic; the AdvancedSettingsDisclosure `<details>` pattern already exists).

**A7. Decide the live Transcript during capture — hold, don't demote [d633].**
research-ux-patterns §7 ranks demoting/hiding the During Transcript (the Granola/Fathom "stay
present" pattern) as the single largest calm-tech opportunity. We **reject demotion for d633**:
d633 deliberately ships During as a 2-column NOTES-primary / TRANSCRIPT-secondary grid
(`during-1440.png`), and the transcript already sits secondary (right, no graph, no diagnostics) —
it is demoted structurally without being hidden, which preserves the local-first "you can see
exactly what we captured" trust affordance that a hidden transcript would erode. Revisit only if
usage data shows the secondary transcript pulls attention off notes. *Evidence:* research-ux-patterns
§7, `during-1440.png`, design doc lines 116–124. **Decision, no code.**

**A6. During-capture data-route badge [depends on 72d5/c282].**
`SessionDataRoutePanel` (the full ledger) is Analysis-only; During capture gives users *zero*
visibility into where audio is going. Add a minimal "Data route: Cloud (Deepgram + OpenAI)" /
"Local only" badge to the ControlBar session-status area (full ledger stays in Analysis). This is
a privacy-thesis differentiator competitors only address at the marketing level. **This is a UI
consumer of state owned elsewhere:** the active route / per-data-class boundary is produced by
72d5 (DataMovementLedger + cloud-transfer policy) and c282 (local/cloud boundary matrix) — the
badge reads that state, it is not net-new route logic and depends on that policy work landing.
*Evidence:* research-ux-patterns §3, `SessionDataRoutePanel.tsx`, seeds 72d5/c282. **M, executable
(pending 72d5/c282).**

### Tier B — Accessibility + i18n correctness (WCAG / locale parity)

**B1. Localize the ExpressSetup "Setup modes" section [extends 75a1].**
`ExpressSetup.tsx:769–859` hardcoded English on the **first-run** flow breaks pt. Route all
strings through `t()`. *Evidence:* `ExpressSetup.tsx:769–859`. **S, executable.**

**B2. Add a skip link + `id` on `<main>`.**
`App.tsx:494–666` — keyboard users tab through 2 banners + ControlBar + nav every time. Add a
visually-hidden skip-to-main link and an `id` target. WCAG 2.4.1. **S, executable.**

**B3. "Recording started/stopped" assertive announcement.**
ControlBar's `aria-live=polite` timer announces only the counter ("0:01, 0:02…"), never the
transition. Add a separate `aria-live=assertive` status on the `isCapturing` false→true/true→false
transition. WCAG 4.1.3. *Evidence:* research-ux-patterns §3. **S, executable.**

**B4. AudioSourceSelector search accessible name.**
Add `aria-label` and localize the placeholder (`AudioSourceSelector.tsx:659–663`). WCAG 4.1.2. **S, executable.**

**B5. Per-stage dot text alternative (color-blind).**
PipelineStatusBar dots convey status by color alone; add an `aria-label` with stage name + status
to each. WCAG 1.4.11 / NNG. (Folds naturally into A5's rework.) *Evidence:* research-ux-patterns §5. **S, executable.**

**B6. LoggingSettings status announcement.**
Wrap async apply/purge/analytics results (`LoggingSettings.tsx:296/319`) in `role=status
aria-live=polite` to match peer panels. **S, executable.**

**B7. Announce phase auto-transitions (capture→During, load→After).**
research-ux-patterns §5 Gap 1: when the shell auto-switches phase, SR users get no announcement of
the new stage. Add an `aria-live=polite` region that announces the destination phase on the
capture→During and load→After transitions. WCAG 4.1.3. *Evidence:* research-ux-patterns §5 Gap 1.
**S, executable.**

### Tier C — Design-system + token hygiene (ADR-0009/0016)

**C1. Fix `--tint-accent-danger` → `--tint-danger` (NotesPanel.tsx:157).**
Invisible error background today. One-token fix. **S, executable.**

**C2. SessionsBrowser: kill ghost `var(--border,#333)` fallbacks + migrate inline styles.**
Replace `--border` (undefined) with `--border-color`; migrate the 13 inline `style={{}}` blocks
to tokens per ADR-0016. Light-theme correctness. *Evidence:* `SessionsBrowser.tsx:304/327/398`. **M, executable.**

**C3. Replace hardcoded z-index with the `--z-*` ladder + give the banners `role=alert`.**
`DemoModeBanner:71`, `StorageBanner:83`, `PopoverOverlay:34/53`, `Tooltip:49`,
`KnowledgeGraphViewer:718/743`. While touching `DemoModeBanner`/`StorageBanner`, also add
`role=alert` when they appear so SR users hear the demo/storage state change (research-ux-patterns
§5 recommends this alongside the z-index fix). **S, executable.**

**C4. Replace pasted mono font stack with `font-mono` (LiveTranscript.tsx:288/333).** **S, executable.**

**C5. SettingsPage top-level inline error state [ADR-0011].**
`SettingsPage` has loading but no top-level inline error (save failures only via toast — the exact
case ADR-0011 flagged). Add an inline error region. **Scope is deliberately narrow:** do *not*
migrate the panel-local `useState` error channels to `notify()`. The ground-code audit rated
NotesPanel and LiveTranscript inline errors "complete"/"best-in-class" and flagged several as
"legitimately in-flow (banners)"; routing those through the bottom-right toast queue would regress
their in-context visibility. If any specific export-error path is confirmed *not* in-flow, migrate
that path individually — but no blanket migration. *Evidence:* ADR-0011, ground-code audit. **S, executable.**

### Tier D — Information scent (lower impact, design-touch)

**D1. Raise tab scent + idle-state guidance.**
Workspace tabs are bare text with `title=null`; the idle pill just says "Ready". Add icons or a
one-word descriptor/tooltip per tab (capture/review/graph) and make idle state suggest a next step.
*Evidence:* visual finding #5. **S, design-needed** (needs iconography decision per ADR-0010).

**D2. Analysis Refresh occluded by the fixed bottom-right toast.**
The toast host and the diagnostics-rail Refresh button compete for the same corner with no offset
(`analysis-1440.png`). Give the toast host a bottom offset or move the Refresh control. **S, executable.**

---

## 3. Native-Tauri Opportunity Shortlist

The app is a plain undecorated single-window shell — only `tauri-plugin-single-instance` +
`core:default` are wired (`src-tauri/Cargo.toml`, `capabilities/default.json`). For a local-first
meeting-memory product where capture runs in the background, the **tray + global-shortcut pair is
the single highest-leverage native integration** — it enables "hit record, forget the window"
with zero new ML/backend work.

| # | Capability | Thesis impact | Wiring cost | Seed |
|---|---|---|---|---|
| 1 | **System tray + recording indicator** — red-dot icon swap when capturing, Stop/Open/Quit menu, hide-to-tray so background capture survives window close | Critical (core capture UX) | Low — `features=["tray-icon"]` (built-in, no plugin) + ~30 LOC in `lib.rs` setup | new (propose `[extends d633]` follow-up) |
| 2 | **Global shortcut (start/stop capture)** — `Cmd/Ctrl+Shift+R` fires even unfocused; the current `useKeyboardShortcuts` is window-focus-only | Critical (hit-record workflow) | Low — `tauri-plugin-global-shortcut` + register in `lib.rs` + `global-shortcut:default` cap + 10 LOC JS | new |
| 3 | **Native OS notifications** — **content-free only:** titles/bodies carry no transcript, note, or action-item content (a bare "Session ended — review in app" at most, not a derived count like "3 action items"). OS notification centers persist/sync bodies (lock screen, Handoff, Action Center history), so this surface is a **data-movement egress that must be reconciled with 72d5 (cloud-transfer policy) + c282 (retention matrix) before wiring** | High | Low — `tauri-plugin-notification` + cap + 5 LOC JS (complements the in-app queue, doesn't replace it) | new |
| 4 | **Window-state persistence** — restore position/size/maximized across launches (multi-monitor) | High (polish) | Trivial — `tauri-plugin-window-state` + 2 LOC | new |
| 5 | **Autostart** — "Start with system" toggle; combine with tray for always-ready capture | Medium (power users) | Low — `tauri-plugin-autostart` + toggle in SettingsPage | new |
| 6 | **In-app updater** | Medium (distribution) | Medium — signing infra | **[extends fdaa]** (already tracked, blocked on OS code-signing) |
| 7 | **File drag-drop import** — drop an audio/transcript file onto the window; `onDragDropEvent` is already in `@tauri-apps/api/window` | Low-Medium | Trivial — no plugin, one capability + handler | new |
| 8 | **Floating overlay live-assist window** — `WindowBuilder` `always_on_top` + `decorations(false)` + window effects (HudWindow/Acrylic). **Gated: ships only after 392b's in-workspace live-assist cards land.** No `skip_taskbar`/"undetectable" framing — 392b's refined scope requires cards *in the active meeting workspace, not only a top-bar popover* and *no hidden/stealth capture path*; a taskbar-hidden HUD contradicts that visible-capture requirement | Low (premature — no in-shell live-assist has shipped) | Medium — new WebviewWindow + 2nd capabilities entry; **macOS transparency needs `macos-private-api` (App-Store-blocking — evaluate distribution)** | **[extends 392b]** live-assist (gated) |

Deep links (`audiograph://`) and custom titlebar (recording pill) are viable but lower-priority
(items 9–10 in the research); defer until tray + notifications land, since notification action
buttons are the main deep-link consumer.

---

## 4. Suggested First Wave

### Wave 1 — Quick wins (S/M, executable, no design decisions)

Ship-now items that are pure correctness or CSS/logic with clear acceptance:

- **A1** ControlBar responsive fix (S) — the visible collision, thesis-critical.
- **C1** NotesPanel `--tint-accent-danger` → `--tint-danger` (S) — invisible error, one token.
- **A2** Friendly IPC-error mapping + auto-expire (M) — kills the raw-TypeError banner.
- **B1** Localize ExpressSetup Setup-modes (S) — first-run locale correctness.
- **B2** Skip link + `<main>` id (S). **B4** AudioSourceSelector aria-label (S).
  **B6** LoggingSettings `role=status` (S). **B3** Recording started/stopped announcement (S).
- **C2/C3/C4** SessionsBrowser ghost-token + z-index ladder + font-mono (S–M) — token hygiene.
- **A4** NOTES empty-state icon+CTA (S). **D2** Refresh/toast corner offset (S).
- **Native #4** window-state persistence (trivial) — high polish-to-cost ratio.

### Wave 2 — Higher-leverage, some design/backend (M)

- **A5** PipelineStatusBar → ambient composite dot During (calm-tech) — needs a small design pass
  on the dot states; extends d633.
- **A6** During-capture data-route badge — privacy differentiator; depends on 72d5/c282 (consumes
  the ledger's active-route/boundary state).
- **A3** Probe-failure onboarding fallback — extends 75a1.
- **C5** SettingsPage inline error + panel-local error migration (ADR-0011).
- **Native #1 + #2** tray + global shortcut — the core "hit record, forget the window" pair.

### Design-needed (not first-wave)

- **D1** tab iconography/scent — needs an ADR-0010 icon decision.
- **Native #8** floating live-assist overlay — extends 392b, **gated behind 392b's in-workspace
  cards shipping first**; needs a distribution-model decision (`macos-private-api` blocks App Store)
  and a full window/IPC design. Not before the in-shell live-assist work.
- **Native #3/#5** notifications + autostart — small once tray lands, but sequence after it.
  Notifications must ship content-free and reconciled with 72d5/c282 (see shortlist row 3).

---

## Appendix — Corrections to input research

- **Reduced-motion is NOT missing.** `styles.css:479` already has a global
  `@media (prefers-reduced-motion: reduce)` block zeroing all animation-duration/iteration-count.
  The pattern-research recommendation to add a `pulse-recording` fallback is already satisfied.
- **Seed IDs 103a / ca78 / 8235 are not sd seeds** — "103a" is the d633 screenshot-baseline label
  (`docs/designs/screenshots/d633/`), not an issue. Live-assist = **392b**, memory = **ceda**,
  first-run/sample = **75a1**, shell = **d633**, updater = **fdaa**. No system-tray seed exists yet;
  recommend filing one as a d633 follow-up.
