import { describe, expect, it } from "vitest";
import {
  advanceGraphFocusTicks,
  EMPTY_GRAPH_FOCUS_STATE,
  FOCUS_NODE_LIMIT,
  type GraphFocusMaterializedNode,
  type GraphFocusTickState,
  RECENCY_SEQUENCE_WINDOW,
  selectFocusEdges,
  selectTouchedNodeIds,
} from "./graphFocus";

function materializedNode(
  id: string,
  sequence: number,
  validUntilMs: number | null = null,
): GraphFocusMaterializedNode {
  return { id, updated_by_sequence: sequence, valid_until_ms: validUntilMs };
}

describe("graphFocus — selectTouchedNodeIds", () => {
  it("takes nodes touched by the top-3 DISTINCT updated_by_sequence values, not the top-3 nodes (a batch tick can touch more than 3 nodes at one sequence)", () => {
    const nodes = [
      materializedNode("a", 10),
      materializedNode("b", 10),
      materializedNode("c", 10),
      materializedNode("d", 9),
      materializedNode("e", 8),
      materializedNode("f", 7), // 4th-highest distinct sequence — excluded
    ];
    const ids = selectTouchedNodeIds(nodes, []);
    expect(ids).toEqual(["a", "b", "c", "d", "e"]);
    expect(ids).not.toContain("f");
  });

  it("excludes inactive nodes (valid_until_ms set) from the recency window entirely", () => {
    const nodes = [
      materializedNode("active", 5),
      materializedNode("retired", 99, 12345), // highest sequence but inactive
    ];
    const ids = selectTouchedNodeIds(nodes, []);
    expect(ids).toEqual(["active"]);
  });

  it(`considers exactly the top ${RECENCY_SEQUENCE_WINDOW} distinct sequence values, never mention_count or any other field`, () => {
    // 5 nodes, each its own sequence — only the top 3 distinct values' nodes
    // should come back, confirming the window width is the constant, not a
    // hardcoded literal that happens to match it.
    const nodes = [1, 2, 3, 4, 5].map((seq) =>
      materializedNode(`n${seq}`, seq),
    );
    const ids = selectTouchedNodeIds(nodes, []);
    expect(ids).toEqual(["n5", "n4", "n3"]);
  });

  it("degrades to last_seen ranking, capped at FOCUS_NODE_LIMIT, when there is no materialized graph at all (legacy-only session)", () => {
    const legacy = Array.from({ length: 15 }, (_, i) => ({
      id: `l${i}`,
      last_seen: i,
    }));
    const ids = selectTouchedNodeIds(null, legacy);
    expect(ids).toHaveLength(FOCUS_NODE_LIMIT);
    expect(ids[0]).toBe("l14"); // most recently seen first
    expect(ids).not.toContain("l0"); // oldest, past the cap
  });

  it("treats a materialized graph with zero active nodes as a real (empty) answer, not a fallback to legacy ranking", () => {
    const ids = selectTouchedNodeIds([], [{ id: "legacy-only", last_seen: 1 }]);
    expect(ids).toEqual([]);
  });
});

describe("graphFocus — advanceGraphFocusTicks (3-tick hysteresis boundary, pinned exactly)", () => {
  it("keeps a touched node's ticksUntouched at 0 every tick it is touched", () => {
    const tick1 = advanceGraphFocusTicks(["a"], EMPTY_GRAPH_FOCUS_STATE);
    expect(tick1.ids).toEqual(["a"]);
    expect(tick1.state.ticksUntouched.get("a")).toBe(0);

    const tick2 = advanceGraphFocusTicks(["a"], tick1.state);
    expect(tick2.ids).toEqual(["a"]);
    expect(tick2.state.ticksUntouched.get("a")).toBe(0);
  });

  it("an untouched node survives exactly 2 ticks and leaves on the 3rd — the exact boundary the anti-strobe rule exists to pin", () => {
    let state: GraphFocusTickState = advanceGraphFocusTicks(
      ["a"],
      EMPTY_GRAPH_FOCUS_STATE,
    ).state; // tick 0: touched

    // tick 1 untouched: still present (grace tick 1 of FOCUS_STICKY_TICKS=3)
    const untouched1 = advanceGraphFocusTicks([], state);
    expect(untouched1.ids).toContain("a");
    expect(untouched1.state.ticksUntouched.get("a")).toBe(1);
    state = untouched1.state;

    // tick 2 untouched: still present ("survives exactly 2 ticks")
    const untouched2 = advanceGraphFocusTicks([], state);
    expect(untouched2.ids).toContain("a");
    expect(untouched2.state.ticksUntouched.get("a")).toBe(2);
    state = untouched2.state;

    // tick 3 untouched: GONE ("leaves on the 3rd")
    const untouched3 = advanceGraphFocusTicks([], state);
    expect(untouched3.ids).not.toContain("a");
    expect(untouched3.state.ticksUntouched.has("a")).toBe(false);
  });

  it("re-touching a hysteresis-held node resets its ticksUntouched back to 0 instead of continuing to count", () => {
    const t0 = advanceGraphFocusTicks(["a"], EMPTY_GRAPH_FOCUS_STATE);
    const t1 = advanceGraphFocusTicks([], t0.state); // ticksUntouched: a=1
    expect(t1.state.ticksUntouched.get("a")).toBe(1);

    const t2 = advanceGraphFocusTicks(["a"], t1.state); // touched again
    expect(t2.state.ticksUntouched.get("a")).toBe(0);
    expect(t2.ids).toContain("a");
  });

  it(`caps the combined touched + hysteresis-held set at FOCUS_NODE_LIMIT (${FOCUS_NODE_LIMIT}), touched nodes always outranking hysteresis-held ones`, () => {
    // Seed 3 hysteresis-held nodes (ticksUntouched=1) plus more touched
    // nodes than the cap allows — touched nodes must win every slot.
    const seeded = advanceGraphFocusTicks(
      ["h1", "h2", "h3"],
      EMPTY_GRAPH_FOCUS_STATE,
    );
    const held = advanceGraphFocusTicks([], seeded.state); // h1-h3 now ticksUntouched=1

    const touchedNow = Array.from(
      { length: FOCUS_NODE_LIMIT + 2 },
      (_, i) => `t${i}`,
    );
    const result = advanceGraphFocusTicks(touchedNow, held.state);
    expect(result.ids).toHaveLength(FOCUS_NODE_LIMIT);
    expect(result.ids.every((id) => id.startsWith("t"))).toBe(true);
    expect(result.ids).not.toContain("h1");
  });

  it("keeps hysteresis-held nodes within the cap when there is room, ordered after touched nodes", () => {
    const seeded = advanceGraphFocusTicks(["h1"], EMPTY_GRAPH_FOCUS_STATE);
    const held = advanceGraphFocusTicks([], seeded.state); // h1 ticksUntouched=1
    const result = advanceGraphFocusTicks(["a", "b"], held.state);
    expect(result.ids).toEqual(["a", "b", "h1"]);
  });

  it("does not resurrect an id that was touched past the cap and therefore never actually rendered — the returned state must hold ONLY ids in the returned focus set", () => {
    // Touch more nodes than FOCUS_NODE_LIMIT allows in one tick; the
    // overflow ids get ticksUntouched=0 internally but are excluded from
    // `ids` by the cap.
    const touchedNow = Array.from(
      { length: FOCUS_NODE_LIMIT + 1 },
      (_, i) => `t${i}`,
    );
    const tick1 = advanceGraphFocusTicks(touchedNow, EMPTY_GRAPH_FOCUS_STATE);
    expect(tick1.ids).toHaveLength(FOCUS_NODE_LIMIT);
    const overflowId = touchedNow.find((id) => !tick1.ids.includes(id));
    expect(overflowId).toBeDefined();
    // The overflow id must not linger in the persisted tick state — it was
    // never in the rendered focus set.
    expect(tick1.state.ticksUntouched.has(overflowId as string)).toBe(false);

    // Two ticks later, with NOTHING touched, the overflow id must still be
    // absent — if it had wrongly survived in state, hysteresis would keep
    // it "held" for up to 2 more ticks and it could resurface in `ids`
    // despite never having been rendered even once.
    const tick2 = advanceGraphFocusTicks([], tick1.state);
    expect(tick2.ids).not.toContain(overflowId);
    expect(tick2.state.ticksUntouched.has(overflowId as string)).toBe(false);
  });
});

describe("graphFocus — selectFocusEdges (both-endpoints-in-set rule)", () => {
  it("keeps only edges whose BOTH endpoints are in the focus set — no one-hop ghost stubs", () => {
    const edges = [
      { source: "a", target: "b" },
      { source: "a", target: "outside" },
      { source: "outside", target: "b" },
    ];
    const kept = selectFocusEdges(["a", "b"], edges);
    expect(kept).toEqual([{ source: "a", target: "b" }]);
  });

  it("resolves object-shaped endpoints (react-force-graph's own runtime mutation) the same as plain string ids", () => {
    const edges = [{ source: { id: "a" }, target: { id: "b" } }];
    const kept = selectFocusEdges(["a", "b"], edges);
    expect(kept).toHaveLength(1);
  });
});
