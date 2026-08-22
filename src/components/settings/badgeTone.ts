/**
 * Status → chip-tone mapping helpers (audio-graph-4d67 — Badge absorption).
 *
 * `settings/Badge.tsx` (Phase 2, D3) established a closed, typed tone set so
 * an unrecognized backend status always renders a styled (if neutral) chip
 * instead of a blank one. The Tier-3 recipe layer (ADR-0047) generalized
 * Badge's rendered shape into `.ag-chip[data-tone]` — a plain-CSS
 * attribute-selector recipe every settings render site now uses directly
 * (`<span className="ag-chip" data-tone={tone}>`), so the component itself
 * is retired. These pure status→tone helpers have no rendering logic of
 * their own and are still needed to compute the `data-tone` value, so they
 * move here unchanged rather than disappearing with the component.
 */

import type { ProviderReadinessStatus } from "../../types";

/** The closed set of chip color roles `.ag-chip[data-tone]` supports that
 * these helpers ever produce. Unknown statuses map to `neutral`. */
export type BadgeTone = "success" | "warning" | "danger" | "neutral" | "accent";

/**
 * Map a provider-readiness status to a chip tone. The `default` arm
 * guarantees any value outside the typed union — including a future backend
 * status — gets a styled neutral chip rather than an unstyled one (D3 fix).
 */
export function readinessTone(
  status: ProviderReadinessStatus | string,
): BadgeTone {
  switch (status) {
    case "ready":
      return "success";
    case "error":
      return "danger";
    case "missing_credentials":
    case "unchecked":
      return "warning";
    default:
      return "neutral";
  }
}

/**
 * Map a product-mode / capability-card readiness status to a tone. Mirrors
 * the prior `.settings-mode-card__badge--*` grouping (ready=success;
 * missing_credentials/blocked/unchecked=warning; error=danger) and defaults
 * unknown values to neutral.
 */
export function modeReadinessTone(status: string): BadgeTone {
  switch (status) {
    case "ready":
      return "success";
    case "error":
      return "danger";
    case "missing_credentials":
    case "blocked":
    case "unchecked":
      return "warning";
    default:
      return "neutral";
  }
}

/**
 * Map a capability-card selectability status to a tone. `selectable` is a
 * PLANNED-axis fact (`ui_selectable && status === "implemented" &&
 * providerRoute != null` — pure registry/config state, ProviderCapabilityCard.tsx)
 * — never an OBSERVED claim, so it maps to `accent` (audio-graph-2554,
 * settings T2 tone law: "Axes 1 and 2 may never render success tone",
 * design-b §2), matching the existing "Selected" chip convention used
 * elsewhere for the same axis. `selected`/`ready` are legacy arms retained
 * for the closed-variant fallback contract (D3) but are not passed by the
 * current caller.
 */
export function selectabilityTone(status: string): BadgeTone {
  switch (status) {
    case "selectable":
      return "accent";
    case "selected":
    case "ready":
      return "success";
    case "error":
      return "danger";
    case "planned":
    case "setup":
    case "unchecked":
    case "missing_credentials":
      return "warning";
    default:
      return "neutral";
  }
}
