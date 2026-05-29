# ADR-0011: Unified feedback / notification system

## Status

Accepted 2026-05-29.

## Context

The frontend has **two competing notification mechanisms** (detailed in
`docs/reviews/2026-05-29-uiux-deep-dive.md`):

1. A **persistent** error toast driven by the Zustand store's `error` field
   (`App.tsx:254-268`) — manual-dismiss only, and rendered at the app root so it
   can appear **behind** open modals (Settings/Express save errors get hidden).
2. A **transient** `Toast` component (`Toast.tsx`) — single-slot, auto-dismiss
   3.5s, which **silently overwrites** itself when multiple events fire (e.g.
   several agent proposals), so notifications are lost.

Which mechanism a given event uses depends on whether the error was
"classified" (`useTauriEvents.ts`), so Gemini errors split across both with no
clear rule. There is no success/info channel, no stacking, and no consistent
`aria-live` politeness.

## Decision Drivers

- One predictable model for all transient feedback (info/success/warning/error).
- Important notifications must not be silently overwritten or hidden behind
  modals.
- Correct accessibility: `role`/`aria-live` politeness keyed to severity.
- Per-notification behavior: auto-dismiss vs sticky (errors/actions), optional
  action button (e.g. Undo, Retry, "Open Settings").

## Considered Options

- **Option A — One store-owned notification queue + a single `<Notifications>`
  host.** `notifications: Notification[]` in the store with
  `notify({severity, message, sticky?, action?})`; the host renders a stacked,
  z-index-above-modal region with severity-mapped `aria-live` (assertive for
  error, polite otherwise) and per-item dismiss/auto-dismiss. The store `error`
  field and the old `Toast` are migrated onto it.
- **Option B — Keep both mechanisms**, just fix z-index and make the transient
  toast a small array. Less churn, but preserves two code paths and the
  "which one fires?" ambiguity.
- **Option C — Adopt a third-party toast library** (e.g. Sonner /
  react-hot-toast).

## Decision Outcome

Chosen: **Option A**. A single queue removes the dual-system ambiguity, fixes
the overwrite-and-hide bugs, and gives one place to enforce accessibility and
above-modal stacking. It composes with ADR-0009 (severity colors from semantic
tokens) and ADR-0010 (icons + accessible dismiss button). Option B leaves two
code paths and the classification fork in place. Option C adds a dependency for
behavior that is small to own in-store and would still need a Tauri-event
adapter; revisit only if requirements grow (swipe, promise-toasts, positioning).

### Consequences

- **Positive:** Deterministic feedback; no lost or hidden notifications; one
  accessible host; success/info channel becomes available (e.g. "Settings
  saved", "Connection OK").
- **Positive:** Natural home for actionable notifications (Undo destructive
  actions, Retry failed capture, "Open Settings" for the Gemini-not-configured
  case).
- **Negative:** Migration of all current `showToast`/`error` call sites and the
  `useTauriEvents` classification fork onto the new API.
- **Neutral:** `StorageBanner`/`DemoModeBanner` remain distinct (they are
  in-flow banners, not transient notifications) but should share severity tokens.

## Implementation (intended)

- Store: `notifications` slice + `notify()` / `dismiss()`; deprecate the bare
  `error` string in favor of `notify({severity:"error", sticky:true})`.
- `src/components/Notifications.tsx`: stacked host above modal z-tier
  (ADR-0009 `--z-*`), severity-mapped `aria-live`, icon + dismiss via
  `<IconButton>` (ADR-0010).
- Wave 2 of the plan; flow fixes in Wave 3 consume the `action` affordance.

## References

- `docs/reviews/2026-05-29-uiux-deep-dive.md`
- MDN ARIA live regions; WCAG 2.2 status-message guidance (4.1.3).
- ADR-0009 (tokens), ADR-0010 (icons).
