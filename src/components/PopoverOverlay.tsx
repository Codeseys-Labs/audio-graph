/**
 * Accessible pop-down overlay used by the top-bar agent-proposals and
 * token-usage pop-downs (see `App.tsx`).
 *
 * Wraps its children in a dimmed scrim + a focus-trapped `role="dialog"`
 * surface. Improvements over the bare markup it replaces:
 *   - Closes on **Escape** (WCAG 2.1.2 — no keyboard trap; expected dismissal).
 *   - **Focus trap** + focus restoration to the trigger on unmount, via the
 *     shared `useFocusTrap` hook (WCAG 2.4.3 focus order).
 *   - `aria-modal="true"` + `aria-label` so assistive tech announces it as a
 *     modal dialog (WCAG 4.1.2).
 *   - Scrim click dismisses, same as before.
 *
 * Styling is unchanged: it reuses the existing `agent-overlay__scrim` /
 * `agent-overlay` classes so the visual layer is identical.
 */
import { type ReactNode, useEffect } from "react";
import { useFocusTrap } from "../hooks/useFocusTrap";

interface PopoverOverlayProps {
  /** Accessible name announced for the dialog. */
  label: string;
  /** Invoked when the user dismisses (Escape or scrim click). */
  onClose: () => void;
  /** Dialog surface class. Defaults to the shared `agent-overlay` look. */
  className?: string;
  children: ReactNode;
}

export default function PopoverOverlay({
  label,
  onClose,
  className = "agent-overlay",
  children,
}: PopoverOverlayProps) {
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
        className="agent-overlay__scrim"
        onClick={onClose}
        aria-hidden="true"
      />
      <div
        ref={ref}
        className={className}
        role="dialog"
        aria-modal="true"
        aria-label={label}
        tabIndex={-1}
      >
        {children}
      </div>
    </>
  );
}
