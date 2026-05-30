/**
 * A draggable divider for resizing adjacent panels. Pointer-event based (works
 * with mouse/touch/pen), captures the pointer for the drag duration, and
 * reports the delta in CSS pixels via `onResize`. The parent owns the sizes.
 *
 * Vertical orientation = a vertical bar between left/right columns (drag X).
 * Horizontal orientation = a horizontal bar between stacked panes (drag Y).
 */
import { useCallback, useRef } from "react";

interface ResizeDividerProps {
  orientation: "vertical" | "horizontal";
  /** Called with the pixel delta since the last move event. */
  onResize: (delta: number) => void;
  ariaLabel?: string;
}

export default function ResizeDivider({
  orientation,
  onResize,
  ariaLabel,
}: ResizeDividerProps) {
  const last = useRef<number | null>(null);

  const onPointerDown = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      e.preventDefault();
      (e.target as HTMLElement).setPointerCapture(e.pointerId);
      last.current = orientation === "vertical" ? e.clientX : e.clientY;
    },
    [orientation],
  );

  const onPointerMove = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      if (last.current === null) return;
      const pos = orientation === "vertical" ? e.clientX : e.clientY;
      const delta = pos - last.current;
      last.current = pos;
      if (delta !== 0) onResize(delta);
    },
    [orientation, onResize],
  );

  const end = useCallback((e: React.PointerEvent<HTMLDivElement>) => {
    last.current = null;
    try {
      (e.target as HTMLElement).releasePointerCapture(e.pointerId);
    } catch {
      /* pointer already released */
    }
  }, []);

  return (
    <div
      className={`resize-divider resize-divider--${orientation}`}
      role="separator"
      aria-orientation={orientation}
      aria-label={ariaLabel ?? "Resize panel"}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={end}
      onPointerCancel={end}
    />
  );
}
