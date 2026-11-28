/**
 * `useRetryFocus` — keyboard focus after a Review "Try again" control goes away.
 *
 * Activating a retry button by keyboard removes the focused element (the
 * first-load retry as soon as loading starts, the stale retry once recovery
 * succeeds), and focus would fall to the document body. This parks it on the
 * lens region instead, but ONLY when that is what happened: the retry control
 * itself had focus when it was activated, and when it went away focus is on the
 * body. If the user moved focus anywhere else meanwhile, it is left alone.
 *
 * Retry buttons carry `data-review-retry`; `searchRef` is the container that
 * holds them and `focusRef` the (tabIndex -1) element that receives focus.
 */
import {
  type MouseEvent,
  type RefObject,
  useCallback,
  useEffect,
  useRef,
} from "react";

export function useRetryFocus(
  searchRef: RefObject<HTMLElement | null>,
  focusRef: RefObject<HTMLElement | null>,
): (event: MouseEvent<HTMLElement>) => void {
  const pending = useRef(false);

  // Focus moving to anything but a retry control cancels the pending restore.
  useEffect(() => {
    const onFocusIn = (event: FocusEvent) => {
      const target = event.target;
      if (
        !(target instanceof Element) ||
        !target.hasAttribute("data-review-retry")
      ) {
        pending.current = false;
      }
    };
    document.addEventListener("focusin", onFocusIn);
    return () => document.removeEventListener("focusin", onFocusIn);
  }, []);

  // After every render: has the retry control gone away?
  useEffect(() => {
    if (!pending.current) return;
    const container = searchRef.current;
    if (container === null || container.querySelector("[data-review-retry]")) {
      return;
    }
    pending.current = false;
    const active = document.activeElement;
    if (active === null || active === document.body) focusRef.current?.focus();
  });

  return useCallback((event: MouseEvent<HTMLElement>) => {
    pending.current = document.activeElement === event.currentTarget;
  }, []);
}
