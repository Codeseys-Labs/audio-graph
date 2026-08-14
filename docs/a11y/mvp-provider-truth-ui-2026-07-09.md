# Accessibility review: MVP provider-truth UI

Date: 2026-07-09

Target standard: WCAG 2.2 AA

Scope: onboarding handoff/sample preview, `ControlBar`,
`ConversationModeControl`, provider-mode/settings cards, `NotesPanel` errors,
and the ADR-0033 deferred-provider experience.

Tools and evidence: Biome accessibility/static checks, Testing Library/Vitest,
token-pair contrast calculation, keyboard/focus DOM assertions, and manual
source/semantic review. No real Tauri WebView, NVDA, VoiceOver, axe, Lighthouse,
forced-colors, or 200 percent zoom run was available in this slice.

## Summary

- Blockers: 0
- Serious findings found: 4
- Serious findings fixed in this slice: 4
- Warnings still open: 1

## Resolved findings

### Serious 01: primary accent actions failed dark-theme text contrast

- WCAG: 1.4.3 Contrast Minimum
- Affected: onboarding handoff dismissal, Get Started sample preview,
  Notes sample preview, and the skip-to-main link.
- Problem: white on dark-theme `--accent-blue` was 2.72:1.
- Fix: every solid blue action now uses `--on-accent-blue`. A regression test
  parses the dark/light semantic tokens and requires at least 4.5:1; the pairs
  measure 6.68:1 dark and 4.76:1 light.
- Verification: `src/styles.a11y.test.ts` passes both theme cases.

### Serious 02: provider-boundary alerts mixed English into Portuguese UI

- WCAG: 3.1.2 Language of Parts
- Affected: Notes settings-hydration and structured `provider_deferred` alerts.
- Problem: both immediate `role="alert"` paths contained hard-coded English.
- Fix: the canonical en/pt `errors` namespace now owns both messages;
  `errorToMessage` resolves deferred-provider copy through the active i18n
  language and retains only the provider display name.
- Verification: English and Portuguese alert tests assert localized copy and
  prove the provider id, endpoint, and object-coercion text are absent.

### Serious 03: removed controls had no deterministic focus successor

- WCAG: 2.4.3 Focus Order
- Affected: handoff dismissal, sample-preview workspace transition, and Notes
  error dismissal.
- Problem: removing the focused control could return focus to the document
  body and restart keyboard navigation.
- Fix: handoff dismissal focuses the active workspace tab; a newly active
  sample focuses the visible Review tab; Notes error dismissal returns focus to the
  Synthesize/Refresh action.
- Verification: DOM focus assertions cover all three transitions. A real
  Tauri/NVDA pass remains required.

### Serious 04: modal focus could escape and descendant Escape was swallowed

- WCAG: 2.1.1 Keyboard; 2.4.3 Focus Order; 4.1.2 Name, Role, Value
- Affected: Express Setup and Sessions Browser dialogs.
- Problem: the initially focused `tabindex=-1` dialog container was not treated
  as the position before the first focusable descendant, so immediate
  Shift+Tab could escape. Descendant key handlers stopped propagation before
  the dialog's Escape handler ran.
- Fix: the focus trap now wraps Tab/Shift+Tab from the dialog container, and
  each modal handles Escape before stopping descendant propagation.
- Verification: focused DOM tests cover immediate forward/backward wrapping and
  Escape dispatched from focused controls in both dialogs.

## Open warning

### Warning 01: mutually exclusive modes use toggle-button semantics

- WCAG: 1.3.1 Info and Relationships; 4.1.2 Name, Role, Value
- Affected: `ConversationModeControl` and product mode summary cards.
- Current state: native buttons, names, pressed state, disabled explanations,
  keyboard activation, and mode-specific card action names work. However,
  mutually exclusive choices remain a collection of `aria-pressed` buttons
  rather than native radio groups.
- Recommended fix: convert selectable choices to labelled fieldsets with native
  radios and render deferred saved routes as noninteractive status cards.
- Owner: `audio-graph-8e51`.

## Passed checks

- Deferred modes cannot start but retain visible status/recovery copy.
- Stop remains available for active realtime work after mode changes.
- Notes errors use an assertive alert and content-safe structured formatting.
- Readiness summaries use a separate polite, atomic status region.
- Mode status is conveyed with text as well as color.
- Historical Review disables live-context Notes and Chat actions with localized
  explanatory status copy instead of sending the hidden active session.
- Missing or incomplete movement evidence renders Unknown rather than a green
  no-egress claim.
- Global focus-visible and reduced-motion foundations remain present.
- Final focused shell/privacy/canonical-view run: 4 files / 91 tests passed.
- Final serial frontend run: 70 files / 962 tests passed.

## Runtime evidence still required

- axe-backed component or browser coverage for idle, pending, deferred, active,
  error, and Settings states;
- NVDA on Windows and VoiceOver on macOS;
- 200 percent zoom/reflow at 1440, 1024, and 768 pixels;
- Windows forced colors, dark/light compositing, reduced motion, and target
  geometry; and
- packaged Tauri focus and announcement order after asynchronous transitions.

These remain release acceptance in `audio-graph-8e51`; static/JSDOM evidence is
not represented as a screen-reader or packaged-app pass.
