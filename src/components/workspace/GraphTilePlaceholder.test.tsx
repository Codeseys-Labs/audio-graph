import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { useAudioGraphStore } from "../../store";
import type { GraphSnapshot } from "../../types";
import { GraphTilePlaceholder } from "./GraphTilePlaceholder";

const EMPTY_GRAPH: GraphSnapshot = {
  nodes: [],
  links: [],
  stats: { total_nodes: 0, total_edges: 0, total_episodes: 0 },
};

const POPULATED_GRAPH: GraphSnapshot = {
  nodes: [
    {
      id: "n1",
      name: "Node 1",
      entity_type: "person",
      val: 1,
      color: "#000000",
      first_seen: 0,
      last_seen: 0,
      mention_count: 1,
    },
    {
      id: "n2",
      name: "Node 2",
      entity_type: "organization",
      val: 1,
      color: "#000000",
      first_seen: 0,
      last_seen: 0,
      mention_count: 1,
    },
  ],
  links: [
    {
      source: "n1",
      target: "n2",
      relation_type: "works_at",
      weight: 1,
      color: "#000000",
    },
  ],
  stats: { total_nodes: 2, total_edges: 1, total_episodes: 0 },
};

// Real store, unwrapped by `SessionViewProvider` — per
// `SessionViewProvider.test.tsx`'s own documented behavior, `useSessionView`
// falls back to the live store's values with no provider present, so this
// exercises the exact fallback path (`materializedGraphToSnapshot(...) ??
// graphSnapshot`) the component's doc comment claims without needing any
// extra scaffolding.
describe("GraphTilePlaceholder (ticket W4)", () => {
  beforeEach(() => {
    useAudioGraphStore.setState({
      materializedProjectionGraph: null,
      graphSnapshot: EMPTY_GRAPH,
    });
  });

  it("renders the workspace.tile.graphEmpty copy when the graph has zero nodes, never graph.empty's capture-only copy", () => {
    render(<GraphTilePlaceholder />);
    expect(screen.getByText("No graph activity yet")).toBeInTheDocument();
    expect(
      screen.queryByText(/start capturing audio/i),
    ).not.toBeInTheDocument();
  });

  it("renders node/edge counts from the shared-selector fallback (materializedGraphToSnapshot(...) ?? graphSnapshot) when the graph is populated", () => {
    useAudioGraphStore.setState({ graphSnapshot: POPULATED_GRAPH });
    render(<GraphTilePlaceholder />);
    expect(screen.getByText("Nodes: 2")).toBeInTheDocument();
    expect(screen.getByText("Edges: 1")).toBeInTheDocument();
  });
});
