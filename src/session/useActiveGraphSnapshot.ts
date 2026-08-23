/**
 * `useActiveGraphSnapshot` — the ONE shared graph-view selector (ticket W7,
 * synthesis audio-graph-a6b5). Before this ticket, the fallback rule
 * `materializedGraphToSnapshot(materializedProjectionGraph) ?? graphSnapshot`
 * was hand-written inline in TWO places (`KnowledgeGraphViewer.tsx` and the
 * now-deleted `GraphTilePlaceholder.tsx`) — both `useMemo`-wrapped, both
 * reading the identical two store fields. A design that needs a third
 * consumer (`LiveGraphStrip`, this ticket) is exactly the point where "two
 * copies" becomes "the two copies silently drift" — constraints.md's recon
 * already had to verify live that the two existing copies agreed. This hook
 * is the single seam every present and future consumer reads through;
 * `KnowledgeGraphViewer` migrates to it in this same ticket (see that file's
 * diff) so there is no second copy of the `??` fallback left anywhere in the
 * tree (`useActiveGraphSnapshot.test.tsx` pins that with a source-text
 * grep, the same durable-contract style `layout.bento.contract.test.ts`
 * established for CSS).
 *
 * Returns BOTH the merged display snapshot (what every consumer needs for
 * node/edge rendering) and the raw `materialized` graph (what `graphFocus.ts`
 * needs for patch-recency: `MaterializedGraphNode.updated_by_sequence` has no
 * equivalent on the legacy `GraphNode` snapshot type — design-b's synthesis
 * §1.4 finding — so a recency-aware caller needs the untranslated
 * materialized shape, not just the snapshot).
 */
import { useMemo, useRef } from "react";
import { useAudioGraphStore } from "../store";
import type { GraphNode, GraphSnapshot, MaterializedGraph } from "../types";
import { materializedGraphToSnapshot } from "../utils/materializedGraph";
import { useSessionView } from "./SessionViewProvider";

export interface ActiveGraphView {
  /** `materializedGraphToSnapshot(materialized) ?? snapshot` — the ONE
   * fallback rule, computed here and nowhere else. */
  snapshot: GraphSnapshot;
  /** The raw materialized graph, when this session has one. `null` for a
   * legacy-only session (pre-projection-graph recordings) — callers that
   * need recency (`updated_by_sequence`) must treat `null` as "rank by
   * `GraphNode.last_seen` instead," never as "empty." */
  materialized: MaterializedGraph | null;
}

export function useActiveGraphSnapshot(): ActiveGraphView {
  const { graphSnapshot } = useSessionView();
  const materializedProjectionGraph = useAudioGraphStore(
    (s) => s.materializedProjectionGraph,
  );
  // `materializedGraphToSnapshot` is a pure function with no memory of its
  // own: it allocates a brand-new `GraphNode` object for every node on
  // EVERY call. The legacy lane's own store actions (`setGraphSnapshot`,
  // `applyGraphDelta` in `store/index.ts`) defend against this exact hazard
  // by reusing each node's prior object identity across updates — its doc
  // comment there: "react-force-graph stores each node's live simulation
  // state (x/y/vx/vy/fx/fy) ON the node object; if we hand it brand-new
  // objects every GRAPH_UPDATE the D3 force sim reheats and all nodes
  // jump." The materialized lane had no equivalent defense before this
  // hook could be mounted live (ticket W7's `LiveGraphStrip` canvas mode is
  // the first live mount of `KnowledgeGraphViewer` — previously it only
  // ever loaded a session's materialized graph ONCE), so every accepted
  // graph patch would otherwise reheat the canvas. `nodeIdentityRef` is
  // scoped to THIS hook call (i.e. private per mounted consumer), mirroring
  // the store's own id-keyed `Object.assign(existing, incoming)` reuse.
  const nodeIdentityRef = useRef<Map<string, GraphNode>>(new Map());
  const snapshot = useMemo(() => {
    const derived =
      materializedGraphToSnapshot(materializedProjectionGraph) ?? graphSnapshot;
    if (derived === graphSnapshot) {
      // Legacy path: the store already preserves node object identity, so
      // there is nothing to merge here.
      return derived;
    }
    const prevById = nodeIdentityRef.current;
    const nodes = derived.nodes.map((incoming) => {
      const existing = prevById.get(incoming.id);
      return existing ? Object.assign(existing, incoming) : incoming;
    });
    nodeIdentityRef.current = new Map(nodes.map((node) => [node.id, node]));
    return { ...derived, nodes };
  }, [materializedProjectionGraph, graphSnapshot]);
  return { snapshot, materialized: materializedProjectionGraph };
}

export default useActiveGraphSnapshot;
