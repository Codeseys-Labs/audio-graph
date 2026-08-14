import { describe, expect, it } from "vitest";
import type { MaterializedGraph } from "../types";
import { materializedGraphToSnapshot } from "./materializedGraph";

function graph(overrides: Partial<MaterializedGraph> = {}): MaterializedGraph {
  return {
    schema_version: 1,
    session_id: "session-1",
    last_sequence: 1,
    nodes: [],
    edges: [],
    ...overrides,
  };
}

describe("materializedGraphToSnapshot", () => {
  it("keeps a present canonical empty graph authoritative", () => {
    expect(materializedGraphToSnapshot(graph())).toEqual({
      nodes: [],
      links: [],
      stats: { total_nodes: 0, total_edges: 0, total_episodes: 0 },
    });
  });

  it("omits retired nodes and edges from the display snapshot", () => {
    const basis = { transcript_hash: "fnv1a64:test" };
    const provenance = {
      provider: "test",
      model: "test",
      prompt_id: "graph-v1",
    };
    const snapshot = materializedGraphToSnapshot(
      graph({
        nodes: [
          {
            id: "active",
            name: "Active",
            entity_type: "Topic",
            description: null,
            confidence: 0.8,
            valid_from_ms: 1,
            valid_until_ms: null,
            updated_by_sequence: 1,
            updated_at_ms: 2,
            basis,
            provenance,
          },
          {
            id: "retired",
            name: "Retired",
            entity_type: "Topic",
            description: null,
            confidence: 0.8,
            valid_from_ms: 1,
            valid_until_ms: 3,
            updated_by_sequence: 1,
            updated_at_ms: 3,
            basis,
            provenance,
          },
        ],
        edges: [
          {
            id: "retired-edge",
            source: "active",
            target: "retired",
            relation_type: "mentions",
            label: null,
            weight: 1,
            confidence: 0.8,
            valid_from_ms: 1,
            valid_until_ms: null,
            updated_by_sequence: 1,
            updated_at_ms: 2,
            basis,
            provenance,
          },
        ],
      }),
    );

    expect(snapshot?.nodes.map((node) => node.id)).toEqual(["active"]);
    expect(snapshot?.links).toEqual([]);
  });
});
