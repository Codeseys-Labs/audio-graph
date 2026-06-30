/**
 * Badge — token-driven status chip with a CLOSED, typed tone set (Phase 2, D3).
 *
 * Replaces the ~4 open-set BEM badge roots in the settings module
 * (`settings-readiness__badge--${status}`, `settings-mode-card__badge--${...}`,
 * `settings-provider-capability-card__badge--${...}`, the credential chips).
 * Those interpolated `--${status}` modifiers meant a status value with no
 * matching CSS rule rendered an UNSTYLED badge — a latent production bug when a
 * new backend status appears.
 *
 * Here the variants are a closed `BadgeTone` union mapped to the existing
 * status-tint tokens, and the status→tone helpers fall back to `neutral` for
 * any unknown value, so an unrecognized status always renders a styled (if
 * neutral) badge instead of an unstyled one. No new runtime dependency — this
 * is a local extraction (ADR-0016 conventions clause).
 */

import type { ReactNode } from "react";
import type { ProviderReadinessStatus } from "../../types";

/** The closed set of badge color roles. Unknown statuses map to `neutral`. */
export type BadgeTone = "success" | "warning" | "danger" | "neutral" | "accent";

const TONE_CLASS: Record<BadgeTone, string> = {
  success: "bg-(--tint-success) text-(--text-on-tint-success)",
  warning: "bg-(--tint-warning) text-(--text-on-tint-warning)",
  danger: "bg-(--tint-danger) text-(--text-on-tint-danger)",
  // Neutral falls back to the muted surface used by the "unused" credential
  // chip so an unknown status is visibly styled, never blank.
  neutral: "bg-(--hover-overlay) text-text-muted",
  // Solid accent fill, e.g. the "Selected" marker.
  accent: "bg-accent text-(--on-accent)",
};

export interface BadgeProps {
  tone: BadgeTone;
  children: ReactNode;
  /** Extra utility classes appended after the tone classes. */
  className?: string;
  title?: string;
}

/**
 * The shared visual frame: matches the prior `.settings-*__badge` rules
 * (4px radius, 11px/600, 2px×7px padding, nowrap) so the rendered chips are
 * visually identical to the BEM badges they replace.
 */
const BADGE_BASE =
  "inline-block rounded-sm px-[7px] py-(--space-1) text-xs font-semibold leading-[1.4] whitespace-nowrap";

export default function Badge({
  tone,
  children,
  className,
  title,
}: BadgeProps) {
  return (
    <span
      className={`${BADGE_BASE} ${TONE_CLASS[tone]}${className ? ` ${className}` : ""}`}
      title={title}
    >
      {children}
    </span>
  );
}

/**
 * Map a provider-readiness status to a badge tone. The `default` arm guarantees
 * any value outside the typed union — including a future backend status — gets
 * a styled neutral badge rather than an unstyled one (D3 fix).
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
 * Map a product-mode / capability-card readiness status to a tone. Mirrors the
 * prior `.settings-mode-card__badge--*` grouping (ready=success;
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
 * Map a capability-card selectability status to a tone. Mirrors the prior
 * `.settings-provider-capability-card__badge--*` grouping
 * (selected/selectable/ready=success; planned/setup/unchecked/
 * missing_credentials=warning; error=danger) and defaults to neutral.
 */
export function selectabilityTone(status: string): BadgeTone {
  switch (status) {
    case "selected":
    case "selectable":
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
