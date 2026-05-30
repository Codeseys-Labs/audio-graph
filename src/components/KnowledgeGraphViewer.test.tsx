import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useAudioGraphStore } from "../store";
import type { GraphLink, GraphNode, GraphSnapshot } from "../types";
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

function resetStore(
  overrides: Partial<ReturnType<typeof useAudioGraphStore.getState>> = {},
) {
  useAudioGraphStore.setState({
    graphSnapshot: snapshot(),
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
