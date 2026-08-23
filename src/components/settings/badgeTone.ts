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
 * Map an agent-proposal chip's AXIS status to a tone (ticket W8, synthesis
 * audio-graph-a6b5 — the agent tile's status chip, third surface to adopt
 * the T2 tone law after the notes/graph recency chips, `workspace/
 * liveWorkspaceTone.ts`). The input here is NOT the raw
 * `LiveAssistCardStatus` ("pending"/"approved"/"dismissed") — it is
 * `agentOutcomeChipTone`'s post-law axis status: `"approved"` has already
 * been mapped to the law's `"ready"` sentinel and demoted back to
 * `"unchecked"` when unevidenced, before this map ever runs (readinessTone.ts:
 * "the law gates the claim, not just its color"). `"ready"` is therefore the
 * ONLY value this map sends to `success`, and it is reachable only for an
 * `"approved"` card that also carries a recorded outcome.
 *
 * `"pending"` maps to `accent`, mirroring `selectabilityTone`'s own
 * reasoning immediately below: a pending proposal is a PLANNED-axis fact (an
 * action available to take), not yet an OBSERVED success or failure, so it
 * is not a warning-shaped color — "waiting for you" is neutral-to-positive
 * information, not a problem.
 *
 * DISCLOSED DEVIATION (review finding, not in the ticket's spec_deviations
 * list): design-a §8's S3 row names `pending -> info` as the phase-1
 * rendered tone, not `accent`. `BadgeTone` (this file, line 19) has no
 * `"info"` member — only `.ag-chip[data-tone="accent"]` exists as a
 * pending-shaped rendered style today — so returning `info` here would not
 * typecheck without widening the closed tone union. `accent` was chosen
 * instead of adding a new tone member because it is the exact color
 * `selectabilityTone` already uses for the same PLANNED-axis reasoning
 * (see above); revisit if a future ticket adds a real `"info"` tone.
 *
 * Typed as `string` (not the closed `"ready" | "pending" | "dismissed" |
 * "unchecked"` union) to match this file's other maps' D3 open-set
 * convention: `readinessChipTone`'s generic `statusToneMap` parameter
 * accepts any function whose parameter type is a superset of its own `S |
 * Unchecked`, so this stays a valid `statusToneMap` for
 * `agentOutcomeChipTone`'s call while also making the `default` arm below
 * reachable and testable (an unrecognized value falls back to `neutral`,
 * exactly like every sibling helper in this file).
 */
export function agentProposalStatusTone(status: string): BadgeTone {
  switch (status) {
    case "ready":
      return "success";
    case "pending":
      return "accent";
    case "dismissed":
    case "unchecked":
      return "neutral";
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
