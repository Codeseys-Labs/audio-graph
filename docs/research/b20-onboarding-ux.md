# B20 — First-Run Onboarding Hand-off & Pre-Capture Affordance (UX Research)

**Date:** 2026-05-30 · **Scope:** Post-Express-Setup guidance for the AudioGraph
desktop (Tauri v2) app. Research only — no source changes.

## Problem (grounded in current code)

After Express Setup is dismissed (`App.tsx` L127–152, `expressSetupVisible`
transient/per-session), the user lands in the full 3-column shell with **no
hand-off**. The only nudges that exist:

- `ControlBar.tsx` L337–341: idle text *"Select audio sources to begin"* (only
  when zero sources selected; vanishes once one is picked) and L326–335 the
  selected-source label.
- Start button (L183–213) uses the native **`disabled`** attribute
  (`disabled={(!canStart && !isCapturing) || capturePending}`).
- Pipeline controls (Transcribe L239, Gemini L267) **do not render at all** until
  `isCapturing` (L232 `{isCapturing && …}`). Gemini button additionally uses the
  `hidden` attribute (L280) unless `conversationMode==="converse" && converseEngine==="native"`,
  with a native `title=` hint (L275–279). So pre-capture, the user cannot see that
  pipeline modes even exist — the converse/Gemini path is **undiscoverable** until
  after they guess Start.

Three gaps to close: (1) post-setup *next-step* hand-off, (2) surface pipeline
controls pre-capture as disabled+hinted (discoverability), (3) make converse/Gemini
discoverable when a key is present — all without regressing the WCAG-AA audit.

## Existing reusable assets (decisive for the recommendation)

- `src/components/Tooltip.tsx` — **token-styled Radix headless Tooltip** (already a
  dependency). Doc comment explicitly states *"Why this over native `title=`:
  keyboard-focus + hover + touch parity (WCAG 1.4.13 / 4.1.2)… stylable, rides
  global `prefers-reduced-motion`."* This is the sanctioned primitive for hints.
- `src/components/AudioSourceSelector.tsx` L385–391 / L475–481 — the codebase
  **already implements the a11y-correct disabled pattern**: `role="checkbox"` +
  `aria-disabled={isCapturing}` + `tabIndex={0}` + `opacity-60 cursor-not-allowed`
  (focusable + announced, not removed from tab order). This is the exact pattern to
  reuse for "disabled control + hint."
- `src/hooks/useFocusTrap.ts`, `Notifications` host (ADR-0011, severity `aria-live`),
  global `:focus-visible` ring, `.sr-only`, `prefers-reduced-motion` — all present.
- **No tour library installed** (verified: no react-joyride/shepherd/driver/intro in
  package.json).

## 1. Lightweight first-run patterns for pro/creator desktop tools

Convergent guidance across NN/g, UserOnboard, Appcues, Pencil & Paper, UXCam:

- **Empty states as the primary onboarding surface, not modals.** Fill the blank
  critical surface with "two parts instruction, one part delight" + **one** primary
  CTA (Hick's Law / Baymard: one primary + optional secondary link; multiple CTAs
  cause decision paralysis). For AudioGraph the empty `KnowledgeGraphViewer` and the
  idle ControlBar are the natural surfaces. *"Avoid empty states wherever you can"*
  → seed them with a next-step nudge, never a dead end.
- **Pull, not push (NN/g "Onboarding Tutorials vs Contextual Help").** Tutorials
  shown on launch are *push revelations* — intrusive, unmemorable, *"users frequently
  skip them,"* and they don't improve task performance. Contextual *pull revelations*
  (coachmark/tooltip/inline hint triggered when the user is at the relevant element)
  win. AudioGraph already does the *front-loaded setup* half (Express Setup credential
  wizard); the correct complement is **progressive, contextual** guidance afterward —
  not a second modal/tour.
- **Pro/knowledge-worker tools specifically reject heavy tours.** Lucidchart's
  10-step tour *"offended"* the knowledge workers they researched; they replaced it
  with contextual tips in a dismissible side panel. Chorus shows only essentials on
  first login. **Don't trap experienced users in a mandatory walkthrough.**
- **Web/desktop can be slightly richer than mobile** (user at a desk, no tap-target
  pressure) — but the ceiling is still "≤3 features before first core action; defer
  the rest to contextual tips."
- **"Next-step" nudges + a tiny checklist (3–5 items max).** Linear's inline keyboard
  hints and Slack's empty-state microcopy are the canonical lightweight examples.
  A 3-step "source → Start → pick a mode" nudge fits; keep it dismissible and
  re-accessible (the existing `?`/Shortcuts modal is a good home for "show again").

**Recommended pattern set for B20:** (a) an **empty-state hand-off** in the graph
viewer / idle bar with one primary CTA chain (Select a source → Start), (b)
**contextual inline hints / Radix tooltips** on the now-visible-but-disabled pipeline
controls, (c) a small **dismissible "next step" line** — *no full-screen tour, no
spotlight overlay.*

## 2. Accessibility of onboarding hints (focus, aria-live, dismissibility)

WCAG 2.2 obligations for any hint/coachmark (sources: W3C WCAG 2.2, A11Y Collective,
mgifford ACCESSIBILITY.md tooltip best-practices, Arizona Focus Management):

- **SC 1.4.13 Content on Hover or Focus (AA)** — hint content must be **Dismissible**
  (a mechanism to dismiss *without moving hover/focus*, typically **Esc**),
  **Hoverable** (pointer can move onto it without it vanishing), and **Persistent**
  (stays until trigger blur / explicit dismiss / no longer valid). Radix Tooltip
  satisfies all three; native `title=` does **not** (hover-only, not keyboard-, not
  persistent) — another reason to migrate the current `title=` hints to `Tooltip`.
  Exception: the rule's "dismissible without moving focus" clause is relaxed only when
  the content communicates an *input error* or doesn't obscure other content.
- **SC 2.1.2 No Keyboard Trap (A)** — onboarding overlays/coachmarks must let focus
  move away with keyboard alone. If you build *any* spotlight/coachmark with a focus
  trap, it must be escapable (Esc) and the dialog pattern must restore focus on close.
  `useFocusTrap` is the existing mechanism if a coachmark needs trapping — but prefer
  **non-trapping** inline hints to sidestep this risk entirely.
- **SC 2.4.11 Focus Not Obscured (AA, new in 2.2)** — a focused element must not be
  fully hidden by other content (sticky headers, the ControlBar, a coachmark popover).
  Position hints so they never cover the element they describe nor the next focus
  target.
- **`aria-live` for status, not for hints.** Reuse the existing ADR-0011 Notifications
  host (`aria-live` by severity) for *transient* state changes ("Capture started").
  Static guidance ("Select a source to enable Start") belongs in visible text +
  `aria-describedby`, **not** an aria-live region (avoid chatty announcements). The
  ControlBar elapsed timer (L221–227) and backpressure pill (L309–321) already model
  `aria-live="polite"` / `role="status"` correctly.
- **Dismissibility + recall (NN/g).** *"Make it easy to dismiss (and recall)."*
  Persist a `localStorage` flag (the app already uses localStorage for panel sizes,
  `App.tsx` L85–99) so the hand-off shows once; expose a "show getting-started again"
  entry (e.g. in the Shortcuts/help modal) so it's recoverable.
- **`prefers-reduced-motion`** — any reveal/pulse animation must respect it; the app
  has global handling and `Tooltip` already rides it.

## 3. The "disabled control + explanatory hint" pattern, done accessibly

**The core a11y trap (CSS-Tricks, Kitty Giraudel, Deque, MDN):** the native
`disabled` attribute makes a button **unfocusable** and **hard for screen readers to
locate** — so a disabled Start/Transcribe button is *invisible to keyboard and SR
users*, and a `title=`/tooltip on it **cannot fire** (no focus, no hover semantics).
This is exactly the wrong pattern for a discoverability hint.

**Correct alternative — `aria-disabled="true"` (a "disabled-focusable" control):**

| Aspect | `disabled` (current Start btn) | `aria-disabled="true"` (recommended) |
|---|---|---|
| Keyboard focus | Skipped (can't Tab to it) | **Focusable** (stays in tab order) |
| Screen reader | Hard to locate | **Located + announced** "dimmed/unavailable" |
| Click | Browser prevents | **Must prevent in JS** (no-op the handler) |
| Styling | UA `:disabled` styles | **Author-styled** via `[aria-disabled="true"]` (e.g. `opacity`, `cursor-not-allowed`) |
| Tooltip/`aria-describedby` | Can't trigger (unfocusable) | **Works** on focus + hover |

Implementation contract for B20:
1. Replace `disabled` with `aria-disabled={true}` on controls that should be
   *visible-but-not-yet-usable* (pipeline buttons surfaced pre-capture; Start when no
   source). Keep the button **focusable** (`tabIndex` default / `0`).
2. **No-op the click/keydown handler** while `aria-disabled` (ARIA changes only
   semantics, never behavior — MDN). E.g. early-return in `handleToggleCapture`.
3. Style via `[aria-disabled="true"]` (mirror the existing
   `opacity-60 cursor-not-allowed` from `AudioSourceSelector`).
4. Attach the explanation two ways: a **Radix `Tooltip`** (hover+focus parity,
   1.4.13-compliant) **and** `aria-describedby` pointing to a (possibly `.sr-only`)
   text node, so the *reason* ("Select an audio source first" / "Configure Gemini in
   Settings") is announced after the button name — not just shown on hover. The
   Carbon issue (#17828) warns: don't let a tooltip *replace* the accessible name;
   use `aria-describedby` (description) alongside the existing `aria-label` (name).
5. **Precedent already in-repo:** `AudioSourceSelector` L388/L478 uses
   `aria-disabled={isCapturing}` + `tabIndex={0}` + opacity — extend that same idiom
   to ControlBar. (Note: React Aria / Fluent call this `disabledFocusable`/
   `isDisabled` with `disabledBehavior` — same concept, but no need to add that dep.)

Caveat: keep using native `disabled` for the *Save* button inside Express Setup form
(L541) — there a focusable-but-dead submit adds nothing; the rule is about controls
whose presence/affordance you want users to *discover*, not form-submit gating.

## 4. Tiny dependency-free guided approach vs a tour library

**Tour-library landscape (2026):** react-joyride (~30–45KB, MIT) is **broken on
React 19** (uses removed `unmountComponentAtNode`/`unstable_renderSubtreeIntoContainer`,
unmaintained 9+ mo) and injects inline styles that fight Tailwind/tokens; Intro.js is
**AGPL/paid**; Shepherd.js is **AGPL/paid commercial**; Driver.js (~5KB, MIT) is
vanilla-JS, no React bindings; newer MIT React-native micro-libs exist
(react-tourlight ~5KB, guidex-react ~4KB, tour-kit) but are very young/low-adoption.
All of them deliver **spotlight tours** — a pattern §1 shows pro tools should *avoid*.

**ADR-0016 is dispositive.** It records (and ADR-0017 reaffirms) the project's stance:
- shadcn-style **wholesale component/tour libraries are rejected** — they *"replace
  working, owned, tested a11y components and add runtime dependencies… for no
  capability gain."*
- **Radix *headless behavior* primitives à la carte are the sanctioned, narrow
  exception** — *"e.g. `@radix-ui/react-tooltip`… a deliberate, narrow exception, not
  a shadcn-style wholesale component library."* That tooltip is **already installed
  and wrapped** (`Tooltip.tsx`).

**Recommendation: build it dependency-free using existing primitives. Do NOT add a
tour library.** Rationale:

1. The need is *not* a multi-step spotlight tour; it's **3 contextual hints + one
   empty-state CTA chain** — exactly what NN/g/Lucidchart say pro tools should use
   instead of a tour.
2. Every primitive already exists: `Tooltip` (1.4.13-safe hints), `aria-disabled`+
   `tabIndex` idiom (disabled-focusable controls), `Notifications`/`aria-live`
   (transient status), `localStorage` (show-once), `useFocusTrap` (only if a
   coachmark must trap). Net new runtime deps: **zero**.
3. Adding a tour lib regresses ADR-0016 (runtime dep, style-injection vs token
   system), risks React-19 breakage, and ships a UX pattern the research advises
   against for this audience.

**Concrete B20 shape (all dependency-free):**
- **Hand-off empty state:** in the idle ControlBar / empty graph, a single primary
  nudge: *"1. Select an audio source → 2. Start capture."* Dismissible; `localStorage`
  show-once; recallable from the help modal. One primary CTA only.
- **Surface pipeline controls pre-capture:** render Transcribe (and Gemini when
  `hasGeminiKey`) **always**, but `aria-disabled` until `isCapturing`, with a
  `Tooltip` + `aria-describedby` reason ("Start capture to enable transcription").
  This makes converse/Gemini **discoverable** before the user commits to Start, fixing
  the L232/L280 invisibility. When no Gemini key, show it `aria-disabled` with
  "Configure Gemini in Settings" (replace the current hover-only `title=`).
- **Mode discoverability:** when a key is present (`hasGeminiKey`, L158–160), give the
  `ConversationModeControl` / Gemini button a one-time, dismissible inline hint via
  `Tooltip` — not an auto-opening popover.
- **Status feedback:** route "Capture started / mode switched" through the existing
  `Notifications` `aria-live` host; keep static guidance as visible text +
  `aria-describedby` (not aria-live).
- **Optional later:** if a guided 2–3 step coachmark is ever wanted, prefer a tiny
  hand-rolled non-trapping popover over a library; only reach for `useFocusTrap` if it
  must trap, and guarantee Esc-dismiss + focus restore (SC 2.1.2) and non-obscured
  focus (SC 2.4.11).

## Key sources

- NN/g — *Onboarding Tutorials vs. Contextual Help* (push vs pull, dismiss+recall): https://www.nngroup.com/articles/onboarding-tutorials/
- UserOnboard — *Empty States* (instruction+delight, one CTA): https://www.useronboard.com/onboarding-ux-patterns/empty-states/
- Appcues — *26 onboarding examples* (Lucidchart tour "offended" knowledge workers; ≤3 before first action): https://www.appcues.com/blog/best-user-onboarding-examples
- W3C — WCAG 2.2 (1.4.13, 2.1.2, 2.4.11): https://www.w3.org/TR/WCAG22
- CSS-Tricks — *Making Disabled Buttons More Inclusive* (disabled vs aria-disabled table): https://css-tricks.com/making-disabled-buttons-more-inclusive
- Kitty Giraudel — *On Disabled & ARIA-Disabled* (must JS-prevent click; no auto-style): https://kittygiraudel.com/2024/03/29/on-disabled-and-aria-disabled-attributes
- MDN — `aria-disabled` (perceivable-but-disabled; style via `[aria-disabled="true"]`): https://developer.mozilla.org/en-US/docs/Web/Accessibility/ARIA/Reference/Attributes/aria-disabled
- Deque — ARIA vs native attributes (aria-disabled = semantics only): https://www.deque.com/blog/distinguishing-between-aria-and-native-html-attributes
- mgifford ACCESSIBILITY.md — tooltip best-practices (role=tooltip, aria-describedby, 1.4.13 mapping): https://github.com/mgifford/ACCESSIBILITY.md/blob/main/examples/TOOLTIP_ACCESSIBILITY_BEST_PRACTICES.md
- btahir/react-tourlight & userpilot OSS review — tour-lib landscape (joyride broken on React 19; Intro/Shepherd AGPL/paid): https://github.com/btahir/react-tourlight , https://userpilot.com/blog/open-source-user-onboarding/
- **Internal:** ADR-0016 (headless-Radix-à-la-carte; reject wholesale libs), `src/components/Tooltip.tsx`, `src/components/AudioSourceSelector.tsx` L385–391/L475–481, `src/components/ControlBar.tsx` L183–341, `src/App.tsx` L127–152.
