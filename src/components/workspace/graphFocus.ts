/**
 * `graphFocus` — the KG strip's focus-set selector (ticket W7, synthesis
 * audio-graph-a6b5). Pure, framework-free, unit-testable without a DOM —
 * `LiveGraphStrip.tsx` is the only consumer, mirroring `liveDocumentModel.ts`
 * / `LiveDocument.tsx`'s split.
 *
 * Two independent decisions, in order:
 *
 * 1. `selectTouchedNodeIds` — WHICH nodes counts as "recently updated" this
 *    tick, honestly (design-b §4.2): when the session has a materialized
 *    projection graph, that is the `RECENCY_SEQUENCE_WINDOW` (3) highest
 *    DISTINCT `updated_by_sequence` values among active nodes and every
 *    node carrying one of them — recency in PATCH terms, never speech
 *    terms, and never `mention_count` (hardcoded to `1` for every
 *    projection-derived node — `materializedGraph.ts:163` — a verified
 *    dead ranking signal, design-a §2.1). A legacy-only session (no
 *    materialized graph at all) has no sequence number to window on, so it
 *    degrades to ranking by `last_seen` (design-b §4.2's own escape hatch
 *    for exactly this case).
 * 2. `advanceGraphFocusTicks` — the 3-tick hysteresis anti-strobe rule
 *    (design-a §2.2) applied to whatever `selectTouchedNodeIds` returns:
 *    a node that drops out of the touched set stays eligible for
 *    `FOCUS_STICKY_TICKS` more ticks before it actually leaves the
 *    rendered focus set, and the combined (touched + hysteresis-held) set
 *    is capped at `FOCUS_NODE_LIMIT`, touched nodes always winning ties
 *    over hysteresis-held ones.
 *
 * Both steps are separate, ordered functions (not one combined call) so
 * `graphFocus.test.ts` can pin the hysteresis boundary with hand-built
 * touched-id lists, independent of how those lists are computed.
 */

export const FOCUS_NODE_LIMIT = 12;
/** A node dropped from the touched set stays in the rendered focus set for
 * this many MORE ticks before it actually leaves — see
 * `advanceGraphFocusTicks`'s doc comment for the exact tick-by-tick
 * boundary this constant pins. */
export const FOCUS_STICKY_TICKS = 3;
/** "Recently updated" = touched by one of the this-many highest DISTINCT
 * `updated_by_sequence` values present among active materialized nodes
 * (design-b §4.2: "the last 3 graph patches", in patch-sequence terms). */
export const RECENCY_SEQUENCE_WINDOW = 3;

/** Minimal shape this module needs from `MaterializedGraphNode`
 * (`../../types`) — declared locally so this file has zero import surface,
 * mirroring `liveDocumentModel.ts`'s `LiveDocumentSourceNote` convention.
 * The real type is structurally compatible; callers pass it directly. */
export interface GraphFocusMaterializedNode {
  id: string;
  updated_by_sequence: number;
  valid_until_ms?: number | null;
}

/** Minimal shape this module needs from the legacy `GraphNode` snapshot
 * type for the degraded ranking path (no sequence number exists on that
 * type at all — design-b §4.2's verified finding). */
export interface GraphFocusLegacyNode {
  id: string;
  last_seen: number;
}

/** Minimal shape this module needs from `GraphLink`/`MaterializedGraphEdge`
 * — endpoints may already be resolved to a node object (react-force-graph's
 * own runtime mutation of `GraphLink.source`/`target`, per
 * `KnowledgeGraphViewer.tsx`'s identical handling), so both string and
 * object-with-id forms are accepted. */
export interface GraphFocusEdgeLike {
  source: string | { id: string };
  target: string | { id: string };
}

/**
 * Per-node "ticks since last touched" for every id CURRENTLY held in the
 * focus set (touched this tick, or hysteresis-held from an earlier one).
 * An id absent from the map is not in focus. A `Map`, not a plain object —
 * a plain object's key iteration order is insertion order UNLESS a key
 * looks like an array index (e.g. an entity id that happens to be `"42"`),
 * in which case JS silently reorders it numerically-first; a `Map` never
 * does that, which matters here because insertion order IS the touched-
 * priority tiebreak `advanceGraphFocusTicks` sorts on.
 */
export interface GraphFocusTickState {
  ticksUntouched: ReadonlyMap<string, number>;
}

export const EMPTY_GRAPH_FOCUS_STATE: GraphFocusTickState = {
  ticksUntouched: new Map(),
};

/**
 * Priority-ordered (most-recently-touched-first) candidate node ids for
 * THIS tick, before hysteresis — see the module doc for the two paths.
 * `materializedNodes === null` selects the legacy `last_seen` path; an
 * empty array is a real materialized graph with zero active nodes, not
 * "no materialized graph."
 */
export function selectTouchedNodeIds(
  materializedNodes: readonly GraphFocusMaterializedNode[] | null,
  legacyNodes: readonly GraphFocusLegacyNode[],
): string[] {
  if (materializedNodes !== null) {
    const active = materializedNodes.filter(
      (node) => node.valid_until_ms == null,
    );
    const distinctSequencesDesc = [
      ...new Set(active.map((node) => node.updated_by_sequence)),
    ]
      .sort((a, b) => b - a)
      .slice(0, RECENCY_SEQUENCE_WINDOW);
    const inWindow = new Set(distinctSequencesDesc);
    return active
      .filter((node) => inWindow.has(node.updated_by_sequence))
      .sort((a, b) => b.updated_by_sequence - a.updated_by_sequence)
      .map((node) => node.id);
  }
  return [...legacyNodes]
    .sort((a, b) => b.last_seen - a.last_seen)
    .slice(0, FOCUS_NODE_LIMIT)
    .map((node) => node.id);
}

/**
 * Apply the 3-tick hysteresis anti-strobe rule and the 12-node cap.
 *
 * Boundary, pinned exactly (graphFocus.test.ts): a node touched THIS tick
 * always has `ticksUntouched === 0`. A node that stops being touched
 * accumulates `ticksUntouched` by 1 each subsequent call it is NOT in
 * `touchedIdsInPriorityOrder`: it is KEPT in the returned focus set while
 * `ticksUntouched < FOCUS_STICKY_TICKS` (i.e. ticks 1 and 2 — "survives
 * exactly 2 ticks") and DROPPED the tick its count would reach
 * `FOCUS_STICKY_TICKS` (tick 3 — "leaves on the 3rd").
 *
 * The cap applies to the COMBINED touched + hysteresis-held set, sorted by
 * `ticksUntouched` ascending (touched-now nodes, at `0`, always outrank
 * hysteresis-held ones) with ties broken by insertion order — i.e. by
 * `touchedIdsInPriorityOrder`'s own order for touched nodes, and by the
 * previous state's order for hysteresis-held ones.
 */
export function advanceGraphFocusTicks(
  touchedIdsInPriorityOrder: readonly string[],
  prevState: GraphFocusTickState,
): { ids: string[]; state: GraphFocusTickState } {
  const touchedSet = new Set(touchedIdsInPriorityOrder);
  const nextTicks = new Map<string, number>();

  for (const id of touchedIdsInPriorityOrder) {
    if (!nextTicks.has(id)) nextTicks.set(id, 0);
  }
  for (const [id, prevTicks] of prevState.ticksUntouched) {
    if (touchedSet.has(id)) continue; // already set to 0 above
    const ticks = prevTicks + 1;
    if (ticks < FOCUS_STICKY_TICKS) nextTicks.set(id, ticks);
  }

  const ids = [...nextTicks.entries()]
    .sort((a, b) => a[1] - b[1])
    .slice(0, FOCUS_NODE_LIMIT)
    .map(([id]) => id);

  // `nextTicks` can still hold MORE ids than `ids` at this point — e.g. a
  // single tick can touch more than `FOCUS_NODE_LIMIT` nodes at once, and
  // every one of them got `ticksUntouched = 0` above regardless of the cap.
  // Carrying an uncapped id into the returned STATE would violate this
  // type's own contract ("every id CURRENTLY held in the focus set") and
  // let it resurface on a later tick via hysteresis despite never having
  // actually been rendered. Trim the persisted state to exactly the ids
  // that made the cut.
  const idsSet = new Set(ids);
  const heldTicks = new Map([...nextTicks].filter(([id]) => idsSet.has(id)));

  return { ids, state: { ticksUntouched: heldTicks } };
}

function edgeEndpointId(endpoint: string | { id: string }): string {
  return typeof endpoint === "string" ? endpoint : endpoint.id;
}

/**
 * Both-endpoints-in-set edge rule (design-a §2.2: "only edges whose both
 * endpoints are in `nodes`. No ghost stubs pointing off-canvas.") — the
 * mechanic the synthesis kept over design-b's "plus one hop" for edges.
 */
export function selectFocusEdges<E extends GraphFocusEdgeLike>(
  focusNodeIds: readonly string[],
  edges: readonly E[],
): E[] {
  const idSet = new Set(focusNodeIds);
  return edges.filter(
    (edge) =>
      idSet.has(edgeEndpointId(edge.source)) &&
      idSet.has(edgeEndpointId(edge.target)),
  );
}
