import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { render, renderHook, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { useAudioGraphStore } from "../store";
import type { GraphSnapshot, MaterializedGraph } from "../types";
import { useActiveGraphSnapshot } from "./useActiveGraphSnapshot";

const EMPTY_SNAPSHOT: GraphSnapshot = {
  nodes: [],
  links: [],
  stats: { total_nodes: 0, total_edges: 0, total_episodes: 0 },
};

function snapshot(overrides: Partial<GraphSnapshot> = {}): GraphSnapshot {
  return { ...EMPTY_SNAPSHOT, ...overrides };
}

function materializedGraph(
  overrides: Partial<MaterializedGraph> = {},
): MaterializedGraph {
  return {
    schema_version: 1,
    session_id: "s1",
    last_sequence: 1,
    nodes: [],
    edges: [],
    ...overrides,
  };
}

/** A minimal harness rendering the hook's output as text, so this file can
 * assert behavior via `@testing-library/react` without needing a full
 * consumer component. */
function Harness() {
  const { snapshot: view } = useActiveGraphSnapshot();
  return (
    <div>
      <span data-testid="nodes">{view.nodes.length}</span>
      <span data-testid="edges">{view.links.length}</span>
      <span data-testid="node-ids">
        {view.nodes.map((node) => node.id).join(",")}
      </span>
    </div>
  );
}

describe("useActiveGraphSnapshot — fallback order (ticket W7)", () => {
  beforeEach(() => {
    useAudioGraphStore.setState({
      materializedProjectionGraph: null,
      graphSnapshot: EMPTY_SNAPSHOT,
    });
  });

  it("falls back to legacy graphSnapshot when materializedProjectionGraph is null", () => {
    useAudioGraphStore.setState({
      graphSnapshot: snapshot({
        nodes: [
          {
            id: "n1",
            name: "Legacy Node",
            entity_type: "person",
            val: 1,
            color: "#000",
            first_seen: 0,
            last_seen: 0,
            mention_count: 1,
          },
        ],
      }),
    });
    render(<Harness />);
    expect(screen.getByTestId("nodes")).toHaveTextContent("1");
  });

  it("prefers the materialized projection graph over the legacy snapshot when both are present", () => {
    useAudioGraphStore.setState({
      graphSnapshot: snapshot({
        nodes: [
          {
            id: "legacy",
            name: "Legacy",
            entity_type: "person",
            val: 1,
            color: "#000",
            first_seen: 0,
            last_seen: 0,
            mention_count: 1,
          },
        ],
      }),
      materializedProjectionGraph: materializedGraph({
        nodes: [
          {
            id: "m1",
            name: "Materialized",
            entity_type: "person",
            confidence: 1,
            valid_from_ms: 0,
            updated_by_sequence: 1,
            updated_at_ms: 0,
            basis: null,
            provenance: null,
          },
        ],
      }),
    });
    render(<Harness />);
    // Exactly one node — the materialized one, not the union of both. A
    // mutation swapping the fallback's operand order (always-legacy) would
    // ALSO leave the count at 1 here (both fixtures have exactly one node),
    // so pin the WINNING node's id too — this is what actually proves
    // materialized won, not legacy.
    expect(screen.getByTestId("nodes")).toHaveTextContent("1");
    expect(screen.getByTestId("node-ids")).toHaveTextContent("m1");
  });

  it("an accepted delete that empties the materialized graph renders empty, never falling through to a stale legacy snapshot", () => {
    useAudioGraphStore.setState({
      graphSnapshot: snapshot({
        nodes: [
          {
            id: "stale-legacy",
            name: "Stale",
            entity_type: "person",
            val: 1,
            color: "#000",
            first_seen: 0,
            last_seen: 0,
            mention_count: 1,
          },
        ],
      }),
      materializedProjectionGraph: materializedGraph({ nodes: [] }),
    });
    render(<Harness />);
    expect(screen.getByTestId("nodes")).toHaveTextContent("0");
  });
});

/**
 * `react-force-graph` mutates each node object IN PLACE with its live
 * simulation state (x/y/vx/vy/fx/fy) — `store/index.ts`'s `setGraphSnapshot`
 * doc comment explains the hazard in full: a fresh object per update reheats
 * the whole D3 layout. `materializedGraphToSnapshot` has no memory of its
 * own, so this hook must be the one preserving node object identity across
 * re-derivations for any id that survives from one call to the next.
 */
describe("useActiveGraphSnapshot — node object identity across re-derivations (reheat guard)", () => {
  beforeEach(() => {
    useAudioGraphStore.setState({
      materializedProjectionGraph: null,
      graphSnapshot: EMPTY_SNAPSHOT,
    });
  });

  it("reuses the same node object across two materialized re-derivations for an unchanged id", () => {
    useAudioGraphStore.setState({
      materializedProjectionGraph: materializedGraph({
        last_sequence: 1,
        nodes: [
          {
            id: "m1",
            name: "Materialized",
            entity_type: "person",
            confidence: 1,
            valid_from_ms: 0,
            updated_by_sequence: 1,
            updated_at_ms: 0,
            basis: null,
            provenance: null,
          },
        ],
      }),
    });
    const { result, rerender } = renderHook(() => useActiveGraphSnapshot());
    const firstNode = result.current.snapshot.nodes[0];
    expect(firstNode).toBeDefined();
    // A live simulation would have stamped these onto the object; a fresh
    // object per render silently drops them.
    (firstNode as unknown as { x: number; y: number }).x = 42;
    (firstNode as unknown as { x: number; y: number }).y = 7;

    // A second accepted patch that folds onto the SAME materialized graph
    // (a new object, per `applyProjectionGraphPatch`'s spread-and-clone),
    // updating this node's data but not removing it.
    useAudioGraphStore.setState({
      materializedProjectionGraph: materializedGraph({
        last_sequence: 2,
        nodes: [
          {
            id: "m1",
            name: "Materialized (renamed)",
            entity_type: "person",
            confidence: 1,
            valid_from_ms: 0,
            updated_by_sequence: 2,
            updated_at_ms: 5,
            basis: null,
            provenance: null,
          },
        ],
      }),
    });
    rerender();

    const secondNode = result.current.snapshot.nodes[0];
    expect(secondNode).toBe(firstNode); // same object identity, not just equal
    expect((secondNode as unknown as { x: number }).x).toBe(42); // sim state survived
    expect(secondNode.name).toBe("Materialized (renamed)"); // data still refreshed
  });

  it("drops a removed node from the identity cache instead of resurrecting stale simulation state", () => {
    useAudioGraphStore.setState({
      materializedProjectionGraph: materializedGraph({
        last_sequence: 1,
        nodes: [
          {
            id: "gone",
            name: "Gone",
            entity_type: "person",
            confidence: 1,
            valid_from_ms: 0,
            updated_by_sequence: 1,
            updated_at_ms: 0,
            basis: null,
            provenance: null,
          },
        ],
      }),
    });
    const { result, rerender } = renderHook(() => useActiveGraphSnapshot());
    expect(result.current.snapshot.nodes).toHaveLength(1);

    useAudioGraphStore.setState({
      materializedProjectionGraph: materializedGraph({
        last_sequence: 2,
        nodes: [],
      }),
    });
    rerender();
    expect(result.current.snapshot.nodes).toHaveLength(0);

    // A later patch re-introduces a DIFFERENT node under a coincidentally
    // reused id ("gone" could be reissued by a merge/split); it must not
    // come back carrying the previous node's stale simulation-state object.
    useAudioGraphStore.setState({
      materializedProjectionGraph: materializedGraph({
        last_sequence: 3,
        nodes: [
          {
            id: "gone",
            name: "Reincarnated",
            entity_type: "person",
            confidence: 1,
            valid_from_ms: 0,
            updated_by_sequence: 3,
            updated_at_ms: 10,
            basis: null,
            provenance: null,
          },
        ],
      }),
    });
    rerender();
    expect(result.current.snapshot.nodes[0]?.name).toBe("Reincarnated");
  });
});

/**
 * Durable source-text contract, same style `layout.bento.contract.test.ts`
 * established for CSS: grep every `.ts`/`.tsx` source file OTHER than this
 * hook's own module for the literal fallback expression. Migration is only
 * real if no second copy survives — a component that re-derives the same
 * `?? ` fallback locally would silently drift the moment one copy changes
 * and the other doesn't (exactly the risk this ticket's selector exists to
 * remove).
 *
 * `store/index.ts`'s `loadSession` is a DISCLOSED, deliberate exception,
 * not a gap this test missed: it collapses a `LoadedSession` RPC payload's
 * OWN `materialized_graph`/`graph` fields (session-hydration data, a
 * completely different pair of values from the live `state.
 * materializedProjectionGraph`/`state.graphSnapshot` this ticket's three
 * render-time consumers read) into the store's `graphSnapshot` field once,
 * at load time — and a plain store action cannot call a React hook anyway.
 * `LiveGraphStrip`/`KnowledgeGraphViewer` still read the resulting
 * `graphSnapshot` back through THIS hook afterward, so no live consumer
 * ever computes the fallback a second way.
 */
describe("useActiveGraphSnapshot — no remaining inline duplicates of the fallback rule", () => {
  it("is the ONLY place `materializedGraphToSnapshot(...) ?? graphSnapshot`-shaped fallback logic appears in src/, aside from the disclosed store/index.ts loadSession hydration exception", () => {
    const offenders: string[] = [];
    const pattern = /materializedGraphToSnapshot\([^)]*\)\s*\?\?/;
    const disclosedExceptions = [join("src", "store", "index.ts")];

    function walk(dir: string) {
      for (const entry of readdirSync(dir, { withFileTypes: true })) {
        if (entry.name === "node_modules") continue;
        const full = join(dir, entry.name);
        if (entry.isDirectory()) {
          walk(full);
          continue;
        }
        if (!/\.(ts|tsx)$/.test(entry.name)) continue;
        if (full.endsWith("useActiveGraphSnapshot.ts")) continue;
        if (disclosedExceptions.includes(full)) continue;
        if (/\.test\.tsx?$/.test(entry.name)) continue; // test fixtures may reference the pattern in prose/comments
        const text = readFileSync(full, "utf8");
        if (pattern.test(text)) offenders.push(full);
      }
    }
    walk("src");

    expect(
      offenders,
      `found a duplicate inline fallback outside useActiveGraphSnapshot.ts: ${offenders.join(", ")}`,
    ).toEqual([]);
  });
});
