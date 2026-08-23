import { describe, expect, it } from "vitest";
import {
  entityTypeI18nKey,
  type GraphChangePatch,
  selectGraphChangeLines,
} from "./graphChangeFeed";

function graphPatch(
  sequence: number,
  operations: GraphChangePatch["operations"],
): GraphChangePatch {
  return { sequence, kind: "graph", operations };
}

describe("graphChangeFeed — selectGraphChangeLines (patch fixture)", () => {
  it('renders a new node id as "added" with its name and entity type', () => {
    const patches = [
      graphPatch(1, [
        {
          type: "upsert_graph_node",
          id: "n1",
          name: "Acme Corp",
          entity_type: "organization",
        },
      ]),
    ];
    const lines = selectGraphChangeLines(patches);
    expect(lines).toEqual([
      {
        key: "1-0",
        kind: "added",
        name: "Acme Corp",
        entityType: "organization",
      },
    ]);
  });

  it('renders a re-upsert of an EXISTING id under a DIFFERENT name as "renamed", not a second "added"', () => {
    const patches = [
      graphPatch(1, [
        {
          type: "upsert_graph_node",
          id: "n1",
          name: "Acme Corp",
          entity_type: "organization",
        },
      ]),
      graphPatch(2, [
        {
          type: "upsert_graph_node",
          id: "n1",
          name: "Acme Corporation",
          entity_type: "organization",
        },
      ]),
    ];
    const lines = selectGraphChangeLines(patches);
    expect(lines).toEqual([
      {
        key: "1-0",
        kind: "added",
        name: "Acme Corp",
        entityType: "organization",
      },
      { key: "2-0", kind: "renamed", name: "Acme Corporation" },
    ]);
  });

  it("renders NO line at all for a byte-identical re-upsert (same id, same name) — honesty rule: no motion for a no-op", () => {
    const patches = [
      graphPatch(1, [
        {
          type: "upsert_graph_node",
          id: "n1",
          name: "Acme Corp",
          entity_type: "organization",
        },
      ]),
      graphPatch(2, [
        {
          type: "upsert_graph_node",
          id: "n1",
          name: "Acme Corp",
          entity_type: "organization",
        },
      ]),
    ];
    const lines = selectGraphChangeLines(patches);
    expect(lines).toHaveLength(1);
    expect(lines[0].kind).toBe("added");
  });

  it('renders a new edge as "A -> B", resolving endpoint ids to names already seen in the scan', () => {
    const patches = [
      graphPatch(1, [
        {
          type: "upsert_graph_node",
          id: "n1",
          name: "Acme Corp",
          entity_type: "organization",
        },
        {
          type: "upsert_graph_node",
          id: "n2",
          name: "Postgres",
          entity_type: "product",
        },
      ]),
      graphPatch(2, [
        { type: "upsert_graph_edge", id: "e1", source: "n1", target: "n2" },
      ]),
    ];
    const lines = selectGraphChangeLines(patches);
    expect(lines[2]).toEqual({
      key: "2-0",
      kind: "edge",
      sourceName: "Acme Corp",
      targetName: "Postgres",
    });
  });

  it("does not re-emit an edge line for a re-upsert of an already-seen edge id", () => {
    const patches = [
      graphPatch(1, [
        { type: "upsert_graph_edge", id: "e1", source: "n1", target: "n2" },
      ]),
      graphPatch(2, [
        { type: "upsert_graph_edge", id: "e1", source: "n1", target: "n2" },
      ]),
    ];
    const lines = selectGraphChangeLines(patches);
    expect(lines).toHaveLength(1);
  });

  it('ignores non-"graph" kind patches entirely (a notes patch must never contribute a graph feed line)', () => {
    const patches: GraphChangePatch[] = [
      {
        sequence: 1,
        kind: "notes",
        operations: [
          {
            type: "upsert_graph_node",
            id: "n1",
            name: "Should not appear",
            entity_type: "organization",
          },
        ],
      },
    ];
    const lines = selectGraphChangeLines(patches);
    expect(lines).toEqual([]);
  });

  it("preserves chronological (oldest-first) order across multiple patches and operations within a patch", () => {
    const patches = [
      graphPatch(1, [
        {
          type: "upsert_graph_node",
          id: "n1",
          name: "A",
          entity_type: "person",
        },
        {
          type: "upsert_graph_node",
          id: "n2",
          name: "B",
          entity_type: "person",
        },
      ]),
      graphPatch(2, [
        {
          type: "upsert_graph_node",
          id: "n3",
          name: "C",
          entity_type: "person",
        },
      ]),
    ];
    const lines = selectGraphChangeLines(patches);
    expect(lines.map((l) => l.key)).toEqual(["1-0", "1-1", "2-0"]);
  });
});

describe("graphChangeFeed — entityTypeI18nKey (closed-ontology i18n mapping)", () => {
  it("maps every one of the closed ten ontology names to its own key, case-insensitively", () => {
    const names = [
      "Person",
      "Organization",
      "Location",
      "Event",
      "Topic",
      "Product",
      "Task",
      "Question",
      "Decision",
      "Date",
    ];
    for (const name of names) {
      const expected = name.toLowerCase();
      expect(entityTypeI18nKey(name)).toBe(expected); // PascalCase (wire)
      expect(entityTypeI18nKey(name.toUpperCase())).toBe(expected); // UPPERCASE
      expect(entityTypeI18nKey(expected)).toBe(expected); // lowercase
    }
  });

  it('falls back to "other" for anything outside the closed set, never returning the raw value', () => {
    expect(entityTypeI18nKey("not_a_real_type")).toBe("other");
    expect(entityTypeI18nKey("")).toBe("other");
  });
});
