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

  // Keyboard nudging: arrow keys move the divider by a fixed step, reusing the
  // same `onResize` delta path as the pointer drag. For a vertical divider the
  // left/right arrows resize; for a horizontal one the up/down arrows do.
  const onKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLDivElement>) => {
      const STEP = e.shiftKey ? 32 : 8;
      let delta = 0;
      if (orientation === "vertical") {
        if (e.key === "ArrowLeft") delta = -STEP;
        else if (e.key === "ArrowRight") delta = STEP;
      } else {
        if (e.key === "ArrowUp") delta = -STEP;
        else if (e.key === "ArrowDown") delta = STEP;
      }
      if (delta !== 0) {
        e.preventDefault();
        onResize(delta);
      }
    },
    [orientation, onResize],
  );

  return (
    // An <hr> cannot carry the pointer/keyboard drag handlers a resizable
    // separator needs, so we keep role="separator" on a focusable div.
    // biome-ignore lint/a11y/useSemanticElements: see comment above
    <div
      className={`resize-divider resize-divider--${orientation}`}
      role="separator"
      tabIndex={0}
      aria-orientation={orientation}
      aria-valuenow={0}
      aria-label={ariaLabel ?? "Resize panel"}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={end}
      onPointerCancel={end}
      onKeyDown={onKeyDown}
    />
  );
}
