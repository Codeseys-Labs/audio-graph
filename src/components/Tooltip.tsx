/**
 * Tooltip (ADR-0016 enhancement — see docs/reviews/tailwind-component-enhancement.md).
 *
 * A token-styled wrapper over Radix UI's headless Tooltip primitive — the pilot
 * for the "headless behavior libraries, à la carte, styled with our tokens"
 * direction. Radix ships NO styles and assumes NO reset/Preflight, so this slots
 * cleanly behind `styles.css` (the single source of truth) and the audited token
 * system: surfaces/borders/text use semantic-token utilities, elevation uses
 * `shadow-2`, and it sits at the `--z-popover` tier (z-[40]).
 *
 * Why this over a native `title=`:
 *   - Keyboard-focus + hover + touch parity (WCAG 1.4.13 / 4.1.2), not hover-only.
 *   - Stylable, theme-aware content (survives the planned light-theme token swap).
 *   - Controllable delay; rides the global `prefers-reduced-motion` handling.
 *
 * Behavior only — no theme, no palette, no second reset. Bundles its own
 * `Tooltip.Provider` so a call site can adopt it without an app-root change;
 * if tooltips proliferate, hoist a single `<Tooltip.Provider>` to the root and
 * drop the local one for shared delay/skip behavior.
 */
import * as RadixTooltip from "@radix-ui/react-tooltip";
import type { ReactNode } from "react";

interface TooltipProps {
  /** The tooltip text/content shown on hover or keyboard focus. */
  content: ReactNode;
  /** The trigger element (rendered as-is via Radix `asChild`). */
  children: ReactNode;
  /** Preferred side of the trigger to render on. Defaults to "top". */
  side?: "top" | "right" | "bottom" | "left";
  /** Hover/focus open delay in ms. Defaults to 300. */
  delayDuration?: number;
}

export default function Tooltip({
  content,
  children,
  side = "top",
  delayDuration = 300,
}: TooltipProps) {
  return (
    <RadixTooltip.Provider delayDuration={delayDuration}>
      <RadixTooltip.Root>
        <RadixTooltip.Trigger asChild>{children}</RadixTooltip.Trigger>
        <RadixTooltip.Portal>
          <RadixTooltip.Content
            side={side}
            sideOffset={6}
            className="z-[40] max-w-[260px] py-(--space-2) px-(--space-4) rounded-md bg-bg-elevated border border-border-color text-text-secondary text-xs leading-[1.4] shadow-2 select-none"
          >
            {content}
            <RadixTooltip.Arrow className="fill-bg-elevated" />
          </RadixTooltip.Content>
        </RadixTooltip.Portal>
      </RadixTooltip.Root>
    </RadixTooltip.Provider>
  );
}
