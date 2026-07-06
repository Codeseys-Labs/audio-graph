import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useAudioGraphStore } from "../store";
import type {
  GraphLink,
  GraphNode,
  GraphSnapshot,
  MaterializedGraph,
  MaterializedGraphEdge,
  MaterializedGraphNode,
} from "../types";
import KnowledgeGraphViewer from "./KnowledgeGraphViewer";

// react-force-graph-2d renders to canvas/WebGL which jsdom cannot drive. Mock
// the module with a lightweight stub that (a) records the last props it was
// rendered with so tests can invoke the wiring callbacks (onNodeClick,
// onBackgroundClick, nodeLabel, linkColor, …), and (b) exposes a ref whose
// imperative methods (zoomToFit, d3Force, d3ReheatSimulation) are spies.
type FgProps = Record<string, unknown>;
const lastProps: { current: FgProps | null } = { current: null };
const zoomToFit = vi.fn();
const d3ReheatSimulation = vi.fn();
const d3Force = vi.fn(() => ({ strength: vi.fn(), distance: vi.fn() }));

vi.mock("react-force-graph-2d", async () => {
  const React = await vi.importActual<typeof import("react")>("react");
  return {
    default: React.forwardRef((props: FgProps, ref: React.Ref<unknown>) => {
      lastProps.current = props;
      React.useImperativeHandle(ref, () => ({
        zoomToFit,
        d3ReheatSimulation,
        d3Force,
      }));
      return React.createElement("div", { "data-testid": "force-graph" });
    }),
  };
});

function node(overrides: Partial<GraphNode> = {}): GraphNode {
  return {
    id: crypto.randomUUID(),
    name: "Node",
    entity_type: "Person",
    val: 1,
    color: "#60a5fa",
    first_seen: 0,
    last_seen: 0,
    mention_count: 1,
    ...overrides,
  };
}

function snapshot(overrides: Partial<GraphSnapshot> = {}): GraphSnapshot {
  const nodes = overrides.nodes ?? [];
  const links = overrides.links ?? [];
  return {
    nodes,
    links,
    stats: overrides.stats ?? {
      total_nodes: nodes.length,
      total_edges: links.length,
      total_episodes: 0,
    },
  };
}

function materializedNode(
  overrides: Partial<MaterializedGraphNode> = {},
): MaterializedGraphNode {
  return {
    id: crypto.randomUUID(),
    name: "Projected node",
    entity_type: "Topic",
    description: null,
    confidence: 0.9,
    valid_from_ms: 1_700_000_000_000,
    valid_until_ms: null,
    updated_by_sequence: 1,
    updated_at_ms: 1_700_000_000_100,
    basis: { transcript_hash: "fnv1a64:test" },
    provenance: { provider: "test", model: "projection-test" },
    ...overrides,
  };
}

function materializedEdge(
  overrides: Partial<MaterializedGraphEdge> = {},
): MaterializedGraphEdge {
  return {
    id: crypto.randomUUID(),
    source: "a",
    target: "b",
    relation_type: "tracks",
    label: null,
    weight: 1,
    confidence: 0.85,
    valid_from_ms: 1_700_000_000_000,
    valid_until_ms: null,
    updated_by_sequence: 1,
    updated_at_ms: 1_700_000_000_100,
    basis: { transcript_hash: "fnv1a64:test" },
    provenance: { provider: "test", model: "projection-test" },
    ...overrides,
  };
}

function materializedGraph(
  overrides: Partial<MaterializedGraph> = {},
): MaterializedGraph {
  return {
    schema_version: 1,
    session_id: "session-projection",
    last_sequence: 1,
    nodes: [],
    edges: [],
    ...overrides,
  };
}

function renderedGraphData(): { nodes: GraphNode[]; links: GraphLink[] } {
  return lastProps.current?.graphData as {
    nodes: GraphNode[];
    links: GraphLink[];
  };
}

function resetStore(
  overrides: Partial<ReturnType<typeof useAudioGraphStore.getState>> = {},
) {
  useAudioGraphStore.setState({
    samplePreviewActive: false,
    graphSnapshot: snapshot(),
    materializedProjectionGraph: null,
    graphEdgeFocus: null,
    exportGraph: vi.fn(async () => "{}"),
    getSessionId: vi.fn(async () => "sess-1"),
    ...overrides,
  });
}

describe("KnowledgeGraphViewer", () => {
  beforeEach(() => {
    lastProps.current = null;
    vi.clearAllMocks();
    resetStore();
  });

  it("shows the empty state with a status role when there are no nodes", () => {
    render(<KnowledgeGraphViewer />);
    expect(
      screen.getByText(/start capturing audio to build the knowledge graph/i),
    ).toBeInTheDocument();
    expect(screen.queryByTestId("force-graph")).not.toBeInTheDocument();
  });

  it("disables the Fit and Export buttons in the empty state", () => {
    render(<KnowledgeGraphViewer />);
    expect(
      screen.getByRole("button", { name: /fit graph to view/i }),
    ).toBeDisabled();
    expect(
      screen.getByRole("button", { name: /export knowledge graph as json/i }),
    ).toBeDisabled();
  });

  it("renders the graph and stats overlay when there are nodes", () => {
    resetStore({
      graphSnapshot: snapshot({
        nodes: [node({ id: "a" }), node({ id: "b" })],
        links: [],
        stats: { total_nodes: 2, total_edges: 1, total_episodes: 0 },
      }),
    });
    render(<KnowledgeGraphViewer />);
    expect(screen.getByTestId("force-graph")).toBeInTheDocument();
    expect(screen.getByText("Nodes: 2")).toBeInTheDocument();
    expect(screen.getByText("Edges: 1")).toBeInTheDocument();
    // The aria-label summarizes the graph for assistive tech.
    expect(
      screen.getByRole("status", { name: /2 nodes, 1 edges/i }),
    ).toBeInTheDocument();
  });

  it("prefers the active materialized projection graph when present", () => {
    resetStore({
      graphSnapshot: snapshot({
        nodes: [node({ id: "legacy", name: "Legacy" })],
        stats: { total_nodes: 1, total_edges: 0, total_episodes: 2 },
      }),
      materializedProjectionGraph: materializedGraph({
        nodes: [
          materializedNode({
            id: "projected-a",
            name: "Projected A",
            entity_type: "Person",
          }),
          materializedNode({
            id: "projected-b",
            name: "Projected B",
            entity_type: "Project",
          }),
        ],
        edges: [
          materializedEdge({
            id: "projected-edge",
            source: "projected-a",
            target: "projected-b",
            relation_type: "owns",
            label: "owns",
          }),
        ],
      }),
    });

    render(<KnowledgeGraphViewer />);

    expect(screen.getByTestId("force-graph")).toBeInTheDocument();
    expect(screen.getByText("Nodes: 2")).toBeInTheDocument();
    expect(screen.getByText("Edges: 1")).toBeInTheDocument();
    expect(screen.queryByText(/episodes:/i)).not.toBeInTheDocument();
    const graphData = renderedGraphData();
    expect(graphData.nodes.map((n) => n.id)).toEqual([
      "projected-a",
      "projected-b",
    ]);
    expect(graphData.links).toEqual([
      expect.objectContaining({
        id: "projected-edge",
        source: "projected-a",
        target: "projected-b",
        relation_type: "owns",
        label: "owns",
      }),
    ]);
  });

  it("filters invalidated materialized graph records and dangling edges", () => {
    resetStore({
      materializedProjectionGraph: materializedGraph({
        nodes: [
          materializedNode({ id: "active-a", name: "Active A" }),
          materializedNode({ id: "active-b", name: "Active B" }),
          materializedNode({
            id: "invalidated",
            name: "Invalidated",
            valid_until_ms: 1_700_000_000_500,
          }),
        ],
        edges: [
          materializedEdge({
            id: "active-edge",
            source: "active-a",
            target: "active-b",
          }),
          materializedEdge({
            id: "dangling-edge",
            source: "active-a",
            target: "invalidated",
          }),
          materializedEdge({
            id: "invalidated-edge",
            source: "active-a",
            target: "active-b",
            valid_until_ms: 1_700_000_000_600,
          }),
        ],
      }),
    });

    render(<KnowledgeGraphViewer />);

    const graphData = renderedGraphData();
    expect(graphData.nodes.map((n) => n.id)).toEqual(["active-a", "active-b"]);
    expect(graphData.links.map((link) => link.id)).toEqual(["active-edge"]);
    expect(screen.getByText("Nodes: 2")).toBeInTheDocument();
    expect(screen.getByText("Edges: 1")).toBeInTheDocument();
  });

  it("closes the inspect panel when a materialized graph retcon invalidates the selected node", async () => {
    resetStore({
      materializedProjectionGraph: materializedGraph({
        nodes: [
          materializedNode({
            id: "projected-a",
            name: "Projected A",
          }),
        ],
      }),
    });
    render(<KnowledgeGraphViewer />);

    act(() =>
      (lastProps.current?.onNodeClick as (n: GraphNode) => void)(
        renderedGraphData().nodes[0],
      ),
    );
    expect(
      screen.getByRole("region", { name: /details for projected a/i }),
    ).toBeInTheDocument();

    act(() => {
      useAudioGraphStore.getState().setMaterializedProjectionGraph(
        materializedGraph({
          nodes: [
            materializedNode({
              id: "projected-a",
              name: "Projected A",
              valid_until_ms: 1_700_000_000_500,
            }),
          ],
        }),
      );
    });

    await waitFor(() =>
      expect(
        screen.queryByRole("region", { name: /details for projected a/i }),
      ).not.toBeInTheDocument(),
    );
  });

  it("falls back to the legacy graph when materialized projection graph has no active records", () => {
    resetStore({
      graphSnapshot: snapshot({
        nodes: [node({ id: "legacy", name: "Legacy" })],
        stats: { total_nodes: 1, total_edges: 0, total_episodes: 1 },
      }),
      materializedProjectionGraph: materializedGraph({
        nodes: [
          materializedNode({
            id: "old-projection",
            valid_until_ms: 1_700_000_000_500,
          }),
        ],
        edges: [],
      }),
    });

    render(<KnowledgeGraphViewer />);

    const graphData = renderedGraphData();
    expect(graphData.nodes.map((n) => n.id)).toEqual(["legacy"]);
    expect(screen.getByText("Nodes: 1")).toBeInTheDocument();
    expect(screen.getByText("Episodes: 1")).toBeInTheDocument();
  });

  it("omits the episodes segment when total_episodes is 0", () => {
    resetStore({
      graphSnapshot: snapshot({
        nodes: [node({ id: "a" })],
        stats: { total_nodes: 1, total_edges: 0, total_episodes: 0 },
      }),
    });
    render(<KnowledgeGraphViewer />);
    expect(screen.queryByText(/episodes:/i)).not.toBeInTheDocument();
  });

  it("includes the episodes segment when total_episodes > 0", () => {
    resetStore({
      graphSnapshot: snapshot({
        nodes: [node({ id: "a" })],
        stats: { total_nodes: 1, total_edges: 0, total_episodes: 4 },
      }),
    });
    render(<KnowledgeGraphViewer />);
    expect(screen.getByText("Episodes: 4")).toBeInTheDocument();
  });

  // --- Click-to-inspect panel --------------------------------------------

  function renderWithGraph() {
    const alice = node({
      id: "alice",
      name: "Alice",
      entity_type: "Person",
      mention_count: 5,
      description: "A person of interest",
    });
    const bob = node({
      id: "bob",
      name: "Bob",
      entity_type: "Person",
      mention_count: 3,
    });
    const carol = node({
      id: "carol",
      name: "Carol",
      entity_type: "Org",
      mention_count: 8,
    });
    const links: GraphLink[] = [
      {
        id: "e1",
        source: "alice",
        target: "bob",
        relation_type: "knows",
        weight: 1,
        color: "#999",
      },
      {
        id: "e2",
        source: "alice",
        target: "carol",
        relation_type: "works_at",
        weight: 2,
        color: "#999",
      },
    ];
    resetStore({
      graphSnapshot: snapshot({
        nodes: [alice, bob, carol],
        links,
        stats: { total_nodes: 3, total_edges: 2, total_episodes: 0 },
      }),
    });
    render(<KnowledgeGraphViewer />);
    return { alice, bob, carol };
  }

  it("opens the inspect panel on node click with details and neighbors", () => {
    const { alice } = renderWithGraph();
    const onNodeClick = lastProps.current?.onNodeClick as (
      n: GraphNode,
    ) => void;
    act(() => onNodeClick(alice));

    const panel = screen.getByRole("region", { name: /details for alice/i });
    expect(panel).toBeInTheDocument();
    // Heading + facts.
    expect(screen.getByRole("heading", { name: "Alice" })).toBeInTheDocument();
    expect(screen.getByText("A person of interest")).toBeInTheDocument();
    // Two neighbors (Bob, Carol), sorted by mention_count desc (Carol first).
    expect(screen.getByText(/connections \(2\)/i)).toBeInTheDocument();
    const neighborButtons = screen.getAllByRole("button", {
      name: /carol|bob/i,
    });
    expect(neighborButtons[0]).toHaveTextContent("Carol");
  });

  it("clicking the same node again closes the inspect panel (toggle)", () => {
    const { alice } = renderWithGraph();
    // The handler closes over highlight state, so re-read the latest prop
    // after each click (the stub records the most recent render's props).
    act(() =>
      (lastProps.current?.onNodeClick as (n: GraphNode) => void)(alice),
    );
    expect(
      screen.getByRole("region", { name: /details for alice/i }),
    ).toBeInTheDocument();
    act(() =>
      (lastProps.current?.onNodeClick as (n: GraphNode) => void)(alice),
    );
    expect(
      screen.queryByRole("region", { name: /details for alice/i }),
    ).not.toBeInTheDocument();
  });

  it("the close button dismisses the inspect panel", () => {
    const { alice } = renderWithGraph();
    const onNodeClick = lastProps.current?.onNodeClick as (
      n: GraphNode,
    ) => void;
    act(() => onNodeClick(alice));
    fireEvent.click(screen.getByRole("button", { name: /close details/i }));
    expect(
      screen.queryByRole("region", { name: /details for alice/i }),
    ).not.toBeInTheDocument();
  });

  it("background click closes the inspect panel", () => {
    const { alice } = renderWithGraph();
    const onNodeClick = lastProps.current?.onNodeClick as (
      n: GraphNode,
    ) => void;
    const onBackgroundClick = lastProps.current
      ?.onBackgroundClick as () => void;
    act(() => onNodeClick(alice));
    act(() => onBackgroundClick());
    expect(
      screen.queryByRole("region", { name: /details for alice/i }),
    ).not.toBeInTheDocument();
  });

  it("clicking a neighbor button re-targets the inspect panel", () => {
    const { alice } = renderWithGraph();
    const onNodeClick = lastProps.current?.onNodeClick as (
      n: GraphNode,
    ) => void;
    act(() => onNodeClick(alice));
    // Carol is the top neighbor; clicking it focuses Carol.
    fireEvent.click(screen.getByRole("button", { name: /carol/i }));
    expect(
      screen.getByRole("region", { name: /details for carol/i }),
    ).toBeInTheDocument();
  });

  it("shows the no-connections copy for an isolated node", () => {
    const lonely = node({ id: "lonely", name: "Lonely" });
    resetStore({
      graphSnapshot: snapshot({
        nodes: [lonely],
        links: [],
        stats: { total_nodes: 1, total_edges: 0, total_episodes: 0 },
      }),
    });
    render(<KnowledgeGraphViewer />);
    const onNodeClick = lastProps.current?.onNodeClick as (
      n: GraphNode,
    ) => void;
    act(() => onNodeClick(lonely));
    expect(screen.getByText(/no connections yet/i)).toBeInTheDocument();
    expect(screen.getByText(/connections \(0\)/i)).toBeInTheDocument();
  });

  // --- Label / color callbacks (HTML escaping XSS guard) ------------------

  it("nodeLabel HTML-escapes model-derived entity text", () => {
    renderWithGraph();
    const nodeLabel = lastProps.current?.nodeLabel as (n: GraphNode) => string;
    const html = nodeLabel(
      node({
        name: "<img src=x onerror=alert(1)>",
        entity_type: "Person & Co",
      }),
    );
    expect(html).toContain("&lt;img");
    expect(html).not.toContain("<img");
    expect(html).toContain("Person &amp; Co");
  });

  it("linkLabel escapes the relation type", () => {
    renderWithGraph();
    const linkLabel = lastProps.current?.linkLabel as (l: GraphLink) => string;
    expect(
      linkLabel({
        source: "a",
        target: "b",
        relation_type: "<script>",
        weight: 1,
        color: "#000",
      }),
    ).toBe("&lt;script&gt;");
  });

  it("linkColor dims links not touching the highlighted node", () => {
    const { alice, bob, carol } = renderWithGraph();
    const onNodeClick = lastProps.current?.onNodeClick as (
      n: GraphNode,
    ) => void;
    // Highlight bob; the alice<->carol link (e2) should be dimmed.
    act(() => onNodeClick(bob));
    const linkColor = lastProps.current?.linkColor as (l: GraphLink) => string;
    const e2: GraphLink = {
      source: alice.id,
      target: carol.id,
      relation_type: "works_at",
      weight: 1,
      color: "#999999",
    };
    // Not adjacent to bob → faint (15 alpha suffix).
    expect(linkColor(e2)).toBe("#99999915");
    const e1: GraphLink = {
      source: alice.id,
      target: bob.id,
      relation_type: "knows",
      weight: 1,
      color: "#999999",
    };
    // Adjacent to bob → opaque-ish (99 alpha suffix).
    expect(linkColor(e1)).toBe("#99999999");
  });

  // --- Seek-timeline edge focus (audio-graph-a2a7) ------------------------

  it("emphasizes focused edges and dims the rest when graphEdgeFocus is set", () => {
    renderWithGraph();
    // Focus e1 (alice<->bob) via the store, exactly as the seek-timeline badge
    // does. The Analysis view-switch is App's job; the viewer just paints.
    act(() => useAudioGraphStore.getState().focusGraphEdges(["e1"]));

    const linkColor = lastProps.current?.linkColor as (l: GraphLink) => string;
    const linkWidth = lastProps.current?.linkWidth as (l: GraphLink) => number;
    const e1: GraphLink = {
      id: "e1",
      source: "alice",
      target: "bob",
      relation_type: "knows",
      weight: 1,
      color: "#999999",
    };
    const e2: GraphLink = {
      id: "e2",
      source: "alice",
      target: "carol",
      relation_type: "works_at",
      weight: 1,
      color: "#999999",
    };
    // Focused edge → full-strength base color; unfocused → faint (15 alpha).
    expect(linkColor(e1)).toBe("#999999");
    expect(linkColor(e2)).toBe("#99999915");
    // Focused edge is also thickened relative to the same-weight unfocused one.
    expect(linkWidth(e1)).toBeGreaterThan(linkWidth(e2));
  });

  it("clears edge focus on background click", () => {
    renderWithGraph();
    act(() => useAudioGraphStore.getState().focusGraphEdges(["e1"]));
    const e2: GraphLink = {
      id: "e2",
      source: "alice",
      target: "carol",
      relation_type: "works_at",
      weight: 1,
      color: "#999999",
    };
    // While focused, the unfocused edge is dimmed…
    expect((lastProps.current?.linkColor as (l: GraphLink) => string)(e2)).toBe(
      "#99999915",
    );
    // …a background click clears the focus, restoring the default treatment.
    act(() => (lastProps.current?.onBackgroundClick as () => void)());
    expect((lastProps.current?.linkColor as (l: GraphLink) => string)(e2)).toBe(
      "#99999999",
    );
  });

  it("does NOT dim any edge when the badge's live ids miss the materialized graph", () => {
    // Regression (audio-graph-a2a7 Codex P2): the seek-timeline badge emits LIVE
    // graph edge ids (`edge-{seq}`), but the viewer renders the MATERIALIZED
    // projection graph when present, whose edge ids are a different namespace
    // (UUIDs). A focus set that matches nothing in the rendered graph must be
    // treated as no-focus — never the all-dimmed state that would result from
    // keying dimming on `focusedEdgeIds.size > 0` alone.
    resetStore({
      materializedProjectionGraph: materializedGraph({
        nodes: [
          materializedNode({ id: "mat-a", name: "Mat A" }),
          materializedNode({ id: "mat-b", name: "Mat B" }),
          materializedNode({ id: "mat-c", name: "Mat C" }),
        ],
        edges: [
          materializedEdge({
            id: "mat-edge-1",
            source: "mat-a",
            target: "mat-b",
            relation_type: "owns",
          }),
          materializedEdge({
            id: "mat-edge-2",
            source: "mat-a",
            target: "mat-c",
            relation_type: "tracks",
          }),
        ],
      }),
    });
    render(<KnowledgeGraphViewer />);

    // Focus LIVE ids that exist in no materialized edge (the exact bug shape).
    act(() =>
      useAudioGraphStore.getState().focusGraphEdges(["edge-0", "edge-1"]),
    );

    const linkColor = lastProps.current?.linkColor as (l: GraphLink) => string;
    const linkWidth = lastProps.current?.linkWidth as (l: GraphLink) => number;
    const matEdge1: GraphLink = {
      id: "mat-edge-1",
      source: "mat-a",
      target: "mat-b",
      relation_type: "owns",
      weight: 1,
      color: "#999999",
    };
    const matEdge2: GraphLink = {
      id: "mat-edge-2",
      source: "mat-a",
      target: "mat-c",
      relation_type: "tracks",
      weight: 1,
      color: "#999999",
    };
    // Neither edge is dimmed: both render at the default opaque-ish treatment
    // (99 alpha), NOT the faint (15 alpha) dim. This is the invariant — a badge
    // click NEVER produces the all-dimmed state.
    expect(linkColor(matEdge1)).toBe("#99999999");
    expect(linkColor(matEdge2)).toBe("#99999999");
    // …and no edge is thickened either (no phantom focus emphasis).
    expect(linkWidth(matEdge1)).toBe(linkWidth(matEdge2));
  });

  it("a new node highlight supersedes an active edge focus", () => {
    const { bob } = renderWithGraph();
    act(() => useAudioGraphStore.getState().focusGraphEdges(["e1"]));
    // Clicking a node clears edge focus and switches to node-highlight dimming:
    // e2 (alice<->carol) is not incident to bob, so it's dimmed by the node
    // rule — proving the edge-focus set was cleared (else e2 would already be
    // dimmed by focus, but e1 would be full-strength; here we assert e1 is now
    // dimmed because it's also not incident to bob).
    act(() => (lastProps.current?.onNodeClick as (n: GraphNode) => void)(bob));
    const linkColor = lastProps.current?.linkColor as (l: GraphLink) => string;
    const e2: GraphLink = {
      id: "e2",
      source: "alice",
      target: "carol",
      relation_type: "works_at",
      weight: 1,
      color: "#999999",
    };
    // e2 is not incident to bob → dimmed by the node-highlight rule.
    expect(linkColor(e2)).toBe("#99999915");
    // e1 IS incident to bob → opaque under the node rule (not the focus rule).
    const e1: GraphLink = {
      id: "e1",
      source: "alice",
      target: "bob",
      relation_type: "knows",
      weight: 1,
      color: "#999999",
    };
    expect(linkColor(e1)).toBe("#99999999");
  });

  // --- Export -------------------------------------------------------------

  it("exporting JSON invokes exportGraph + getSessionId and triggers a download", async () => {
    const createObjectURL = vi
      .spyOn(URL, "createObjectURL")
      .mockReturnValue("blob:fake");
    const revokeObjectURL = vi
      .spyOn(URL, "revokeObjectURL")
      .mockImplementation(() => {});
    const exportGraph = vi.fn(async () => '{"nodes":[]}');
    const getSessionId = vi.fn(async () => "abc123");
    resetStore({
      graphSnapshot: snapshot({
        nodes: [node({ id: "a" })],
        stats: { total_nodes: 1, total_edges: 0, total_episodes: 0 },
      }),
      exportGraph,
      getSessionId,
    });
    render(<KnowledgeGraphViewer />);
    await act(async () => {
      fireEvent.click(
        screen.getByRole("button", { name: /export knowledge graph as json/i }),
      );
    });
    await waitFor(() => expect(exportGraph).toHaveBeenCalledTimes(1));
    expect(getSessionId).toHaveBeenCalled();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    createObjectURL.mockRestore();
    revokeObjectURL.mockRestore();
  });

  it("surfaces an export error alert when exportGraph rejects", async () => {
    const exportGraph = vi.fn(async () => {
      throw new Error("disk gone");
    });
    resetStore({
      graphSnapshot: snapshot({
        nodes: [node({ id: "a" })],
        stats: { total_nodes: 1, total_edges: 0, total_episodes: 0 },
      }),
      exportGraph,
    });
    render(<KnowledgeGraphViewer />);
    await act(async () => {
      fireEvent.click(
        screen.getByRole("button", { name: /export knowledge graph as json/i }),
      );
    });
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(/export failed/i);
    expect(alert).toHaveTextContent(/disk gone/i);
  });

  it("falls back to a 'session' filename when getSessionId rejects", async () => {
    const createObjectURL = vi
      .spyOn(URL, "createObjectURL")
      .mockReturnValue("blob:fake");
    const revokeObjectURL = vi
      .spyOn(URL, "revokeObjectURL")
      .mockImplementation(() => {});
    const exportGraph = vi.fn(async () => "{}");
    const getSessionId = vi.fn(async () => {
      throw new Error("no session");
    });
    resetStore({
      graphSnapshot: snapshot({
        nodes: [node({ id: "a" })],
        stats: { total_nodes: 1, total_edges: 0, total_episodes: 0 },
      }),
      exportGraph,
      getSessionId,
    });
    render(<KnowledgeGraphViewer />);
    await act(async () => {
      fireEvent.click(
        screen.getByRole("button", { name: /export knowledge graph as json/i }),
      );
    });
    // getSessionId rejecting is non-fatal: export still succeeds, no alert.
    await waitFor(() => expect(exportGraph).toHaveBeenCalled());
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    createObjectURL.mockRestore();
    revokeObjectURL.mockRestore();
  });

  it("clicking Fit invokes the graph's zoomToFit", () => {
    resetStore({
      graphSnapshot: snapshot({
        nodes: [node({ id: "a" })],
        stats: { total_nodes: 1, total_edges: 0, total_episodes: 0 },
      }),
    });
    render(<KnowledgeGraphViewer />);
    fireEvent.click(screen.getByRole("button", { name: /fit graph to view/i }));
    expect(zoomToFit).toHaveBeenCalled();
  });
});
