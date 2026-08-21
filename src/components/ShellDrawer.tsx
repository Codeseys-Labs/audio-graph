/**
 * Shared drawer chrome for the compact/standard-tier rail + aside drawers
 * (SHELL-R7, plan §R7, ADR-0046, the `useShellLayout()` unit).
 *
 * Generalizes `SystemDrawer`'s hand-rolled `useFocusTrap` + Escape + scrim
 * pattern (D5: `@radix-ui/react-popover` is the only Radix dep — no dialog)
 * to a two-sided shape, so `App.tsx` can mount the SAME chrome for both:
 *   - the rail drawer (`side="start"`, hosts `AudioSourceSelector`), and
 *   - the aside drawer (`side="end"`, hosts `SpeakerPanel`),
 * without forking the focus-trap/Escape/scrim wiring per drawer.
 * `SystemDrawer.tsx` itself is UNCHANGED — this is a new, separate
 * component, not a refactor of it (its own health-chip-triggered drawer is
 * unconditional across every tier, not part of this unit's scope).
 *
 * Parent: `App.tsx`, conditionally on the rail/aside drawer-open state that
 * `useShellLayout()`'s tier drives.
 */
import type { ReactNode } from "react";
import { useEffect } from "react";
import { useFocusTrap } from "../hooks/useFocusTrap";
import IconButton from "./IconButton";

interface ShellDrawerProps {
  /** Which edge the drawer is anchored to (and slides in from). */
  side: "start" | "end";
  /** Accessible name for the dialog; also rendered as its visible heading. */
  label: string;
  /** Accessible name for the close button. */
  closeLabel: string;
  onClose: () => void;
  children: ReactNode;
}

export default function ShellDrawer({
  side,
  label,
  closeLabel,
  onClose,
  children,
}: ShellDrawerProps) {
  const ref = useFocusTrap<HTMLDivElement>();

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onClose();
      }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [onClose]);

  return (
    <>
      <div
        className="fixed inset-0 z-[var(--z-modal)] bg-(--scrim-color)"
        onClick={onClose}
        aria-hidden="true"
      />
      <div
        ref={ref}
        role="dialog"
        aria-modal="true"
        aria-label={label}
        tabIndex={-1}
        className={`shell-drawer shell-drawer--${side}`}
      >
        <div className="ag-panel-head">
          <h2 className="ag-panel-head__title">{label}</h2>
          <IconButton
            icon="close"
            label={closeLabel}
            variant="ghost"
            onClick={onClose}
          />
        </div>
        <div className="shell-drawer__body">{children}</div>
      </div>
    </>
  );
}
