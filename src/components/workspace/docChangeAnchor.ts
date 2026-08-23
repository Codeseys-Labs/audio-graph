/**
 * `docChangeAnchor` — pure geometry math for the living-document's
 * "announce, don't chase" change anchor (ticket W10, synthesis
 * audio-graph-a6b5, design-a §1.6's `DocChangeAnchor`; L1/L2 laws).
 *
 * Framework-free and DOM-free by construction (mirrors `liveDocumentModel.ts`
 * / `graphFocus.ts`'s own "pure module, thin DOM-reading caller" split) — the
 * ONLY thing `LiveDocument.tsx` does that this file can't cover in a unit
 * test is read `getBoundingClientRect()`/`scrollTop` off real elements. Every
 * decision ("is this node above/below the visible area", "which node do we
 * jump to", "where do we scroll") is a plain-number function here, so the
 * STOP CONDITION in this ticket ("if the anchor's scroll-geometry needs
 * cannot be met in jsdom AND the component structure resists a pure-function
 * extraction ... STOP") does not apply: the extraction is exactly this file.
 *
 * L1 discipline, restated for this module specifically: NOTHING here ever
 * reads `window`/`document` or calls `scrollIntoView`/`.focus()` — the
 * anchor's job is to REPORT geometry and a target scroll offset, never to
 * move the viewport or steal focus on its own. `LiveDocument.tsx` is the
 * only place a scroll actually happens, and only in direct response to a
 * user click on the rendered anchor button.
 */

export interface ViewportGeometry {
  /** The scroll container's `scrollTop`. */
  scrollTop: number;
  /** The scroll container's `clientHeight` (visible height). */
  clientHeight: number;
}

export interface NodeGeometry {
  /** Offset of the node's top edge from the scroll container's own
   * content-box top, in the SAME coordinate space as `scrollTop` — i.e.
   * `elRect.top - containerRect.top + container.scrollTop`, not a raw
   * `offsetTop` (the note rows have no positioned ancestor between them and
   * the scroll container, so plain `offsetTop` would resolve against some
   * unrelated further-up ancestor instead). */
  offsetTop: number;
  offsetHeight: number;
}

export type ViewportPosition = "above" | "below" | "visible";

/** A node counts as "above" only when its ENTIRE box has scrolled past the
 * top edge (its bottom is at or above `scrollTop`) — a node that's merely
 * partially clipped at the top edge is still "visible" (the reader can see
 * part of it, so it isn't an undiscovered change). Symmetric for "below". */
export function classifyNodePosition(
  viewport: ViewportGeometry,
  node: NodeGeometry,
): ViewportPosition {
  const nodeBottom = node.offsetTop + node.offsetHeight;
  const viewportBottom = viewport.scrollTop + viewport.clientHeight;
  if (nodeBottom <= viewport.scrollTop) return "above";
  if (node.offsetTop >= viewportBottom) return "below";
  return "visible";
}

/**
 * Folds this fold's newly-changed ids into the tracked "unseen" set, then
 * drops anything that's now visible OR no longer resolvable to a real node
 * (deleted, or — the session-switch case — belongs to a document that isn't
 * the one currently mounted; `LiveDocument.tsx`'s caller passes `null`
 * geometry for any id it can't find via `[data-note-id]`, which is exactly
 * how a stale id from a previous session gets garbage-collected here without
 * this module needing an explicit "session changed" signal).
 *
 * Pure: the caller (`LiveDocument.tsx`) owns the `Set` this persists across
 * renders/events in a ref: `next = updateUnseenChangedIds(ref.current, ...);
 * ref.current = next;`.
 */
export function updateUnseenChangedIds(
  previouslyUnseen: ReadonlySet<string>,
  newlyChangedIds: readonly string[],
  viewport: ViewportGeometry,
  geometryById: ReadonlyMap<string, NodeGeometry | null>,
): Set<string> {
  const next = new Set(previouslyUnseen);
  for (const id of newlyChangedIds) next.add(id);
  for (const id of next) {
    const geometry = geometryById.get(id);
    if (!geometry) {
      next.delete(id);
      continue;
    }
    if (classifyNodePosition(viewport, geometry) === "visible") {
      next.delete(id);
    }
  }
  return next;
}

export interface AnchorSplit {
  above: string[];
  below: string[];
}

/** Splits the tracked unseen ids by direction, dropping anything that has no
 * resolvable geometry (same reasoning as `updateUnseenChangedIds`) rather
 * than surfacing a stale entry with no coordinates to jump to. */
export function splitByDirection(
  unseenIds: ReadonlySet<string>,
  viewport: ViewportGeometry,
  geometryById: ReadonlyMap<string, NodeGeometry | null>,
): AnchorSplit {
  const above: string[] = [];
  const below: string[] = [];
  for (const id of unseenIds) {
    const geometry = geometryById.get(id);
    if (!geometry) continue;
    const position = classifyNodePosition(viewport, geometry);
    if (position === "above") above.push(id);
    else if (position === "below") below.push(id);
  }
  return { above, below };
}

/**
 * The id whose top edge is CLOSEST to the current viewport in the given
 * direction — jumping there covers the shortest distance rather than
 * whichever id happens to iterate first. `null` when none of `ids` resolve
 * to real geometry.
 */
export function nearestNodeId(
  ids: readonly string[],
  direction: "above" | "below",
  geometryById: ReadonlyMap<string, NodeGeometry | null>,
): string | null {
  let bestId: string | null = null;
  let bestTop = direction === "above" ? -Infinity : Infinity;
  for (const id of ids) {
    const geometry = geometryById.get(id);
    if (!geometry) continue;
    const isBetter =
      direction === "above"
        ? geometry.offsetTop > bestTop
        : geometry.offsetTop < bestTop;
    if (bestId === null || isBetter) {
      bestId = id;
      bestTop = geometry.offsetTop;
    }
  }
  return bestId;
}

/** The `scrollTop` that centers `node` in the viewport, clamped to
 * `[0, scrollHeight - clientHeight]` so a near-the-edge target never asks
 * for an out-of-range scroll position (design-a §1.6: `block: "center"`). */
export function computeScrollTopToCenter(
  viewport: ViewportGeometry,
  node: NodeGeometry,
  scrollHeight: number,
): number {
  const target =
    node.offsetTop + node.offsetHeight / 2 - viewport.clientHeight / 2;
  const max = Math.max(0, scrollHeight - viewport.clientHeight);
  return Math.max(0, Math.min(target, max));
}
