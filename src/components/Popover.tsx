/**
 * Popover (ADR-0016 enhancement, D5/SHELL-R2, plan §R2, ADR-0046) — a token-styled
 * wrapper over Radix UI's headless Popover primitive, following `Tooltip.tsx`'s
 * "headless behavior library, styled with our tokens" pattern (the popper
 * substrate — floating positioning, outside-click/Escape dismissal, focus
 * return — already ships via `@radix-ui/react-tooltip`; this is the second,
 * ratified-by-name consumer: D5 chose `@radix-ui/react-popover` specifically,
 * not the heavier `react-dropdown-menu`, so this wraps a floating disclosure
 * panel of plain buttons rather than an ARIA `menu` widget with roving
 * tabindex. Content is real, individually focusable `<button>`s in normal Tab
 * order — Radix supplies focus-trap-free dismissal (Escape, outside click,
 * focus return to the trigger) for free.
 *
 * First adopter: SessionsBrowser's per-row overflow (export/trash/restore/
 * permanent-delete) and the Sessions detail's "Generate prose summary"
 * disclosure.
 */
import * as RadixPopover from "@radix-ui/react-popover";
import type { ReactNode } from "react";

export interface PopoverProps {
  /** The trigger element (rendered as-is via Radix `asChild`). */
  trigger: ReactNode;
  /** Popover panel content — typically a list of `<button>`s. */
  children: ReactNode;
  /** Preferred side of the trigger to render on. Defaults to "bottom". */
  side?: "top" | "right" | "bottom" | "left";
  /** Preferred alignment along that side. Defaults to "end" (menus hang off
   * the trigger's trailing edge, matching a typical row-overflow control). */
  align?: "start" | "center" | "end";
}

export default function Popover({
  trigger,
  children,
  side = "bottom",
  align = "end",
}: PopoverProps) {
  return (
    <RadixPopover.Root>
      <RadixPopover.Trigger asChild>{trigger}</RadixPopover.Trigger>
      <RadixPopover.Portal>
        <RadixPopover.Content
          side={side}
          align={align}
          sideOffset={6}
          className="z-[var(--z-popover)] flex min-w-[180px] flex-col gap-(--space-1) rounded-md border border-(--edge) bg-bg-elevated p-(--space-2) shadow-2 outline-none"
        >
          {children}
          <RadixPopover.Arrow className="fill-bg-elevated" />
        </RadixPopover.Content>
      </RadixPopover.Portal>
    </RadixPopover.Root>
  );
}

/**
 * A single popover action row — full-width ghost button, styled to sit
 * inside `<Popover>`'s content. Not an ARIA `menuitem` (see module doc): a
 * plain button in normal Tab order. Wrapped in `Popover.Close` so activating
 * ANY item (export/delete/restore/generate-summary, …) also dismisses the
 * popover — a disabled button never dispatches `click`, so a disabled item
 * correctly does nothing rather than closing on a no-op.
 */
export function PopoverItem({
  children,
  danger = false,
  ...rest
}: {
  children: ReactNode;
  danger?: boolean;
} & React.ButtonHTMLAttributes<HTMLButtonElement>) {
  return (
    <RadixPopover.Close asChild>
      <button
        type="button"
        className={`flex w-full items-center gap-(--space-3) rounded-sm px-(--space-4) py-(--space-3) text-left text-sm text-text-primary hover:bg-(--hover-overlay) disabled:cursor-not-allowed disabled:opacity-50 ${danger ? "text-(--text-on-tint-danger) hover:bg-(--tint-danger)" : ""}`}
        {...rest}
      >
        {children}
      </button>
    </RadixPopover.Close>
  );
}
