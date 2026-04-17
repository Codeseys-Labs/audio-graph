import { useEffect, useRef } from "react";

/**
 * Minimal focus management for modal dialogs.
 *
 * On mount: remembers the currently-focused element (the thing that opened
 * the modal) and moves focus into the modal container — preferring the
 * container itself if it's focusable (e.g. `tabIndex={-1}`), otherwise
 * falling back to the first focusable descendant.
 *
 * On unmount: restores focus to whatever was focused before the modal opened,
 * so keyboard users land back where they started.
 *
 * This is intentionally NOT a full focus trap (no Tab cycling inside the
 * modal) — just the pragmatic "open → focus in, close → focus out" dance.
 *
 * Usage:
 *   const ref = useFocusTrap<HTMLDivElement>();
 *   return <div ref={ref} role="dialog" tabIndex={-1}>…</div>;
 */
export function useFocusTrap<T extends HTMLElement = HTMLElement>() {
  const containerRef = useRef<T | null>(null);
  const previouslyFocusedRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    previouslyFocusedRef.current =
      (document.activeElement as HTMLElement | null) ?? null;

    const el = containerRef.current;
    if (el) {
      // If the container itself is focusable (has a tabIndex), prefer it so
      // screen readers announce the dialog's aria-labelledby on entry.
      // Otherwise reach for the first focusable child.
      const hasTabIndex = el.hasAttribute("tabindex");
      if (hasTabIndex) {
        el.focus();
      } else {
        const focusable = el.querySelector<HTMLElement>(
          'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])',
        );
        focusable?.focus();
      }
    }

    return () => {
      const prev = previouslyFocusedRef.current;
      // Only restore if the previous element still exists in the DOM and is
      // focusable. Guard against the opener having been unmounted while the
      // modal was open.
      if (prev && typeof prev.focus === "function" && document.contains(prev)) {
        prev.focus();
      }
    };
  }, []);

  return containerRef;
}
