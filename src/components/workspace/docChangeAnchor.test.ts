import { describe, expect, it } from "vitest";
import {
  classifyNodePosition,
  computeScrollTopToCenter,
  nearestNodeId,
  splitByDirection,
  updateUnseenChangedIds,
} from "./docChangeAnchor";

const VIEWPORT = { scrollTop: 200, clientHeight: 100 }; // visible: [200, 300)

describe("classifyNodePosition", () => {
  it("classifies a node entirely above the viewport as 'above'", () => {
    expect(
      classifyNodePosition(VIEWPORT, { offsetTop: 50, offsetHeight: 40 }),
    ).toBe("above");
  });

  it("classifies a node entirely below the viewport as 'below'", () => {
    expect(
      classifyNodePosition(VIEWPORT, { offsetTop: 320, offsetHeight: 20 }),
    ).toBe("below");
  });

  it("classifies a node fully inside the viewport as 'visible'", () => {
    expect(
      classifyNodePosition(VIEWPORT, { offsetTop: 210, offsetHeight: 20 }),
    ).toBe("visible");
  });

  it("treats a node clipped at the top edge (partially visible) as 'visible', not 'above'", () => {
    // bottom edge at 210 > scrollTop 200 -> some of the box is on-screen.
    expect(
      classifyNodePosition(VIEWPORT, { offsetTop: 190, offsetHeight: 20 }),
    ).toBe("visible");
  });

  it("treats a node clipped at the bottom edge (partially visible) as 'visible', not 'below'", () => {
    // top edge at 290 < viewportBottom 300 -> some of the box is on-screen.
    expect(
      classifyNodePosition(VIEWPORT, { offsetTop: 290, offsetHeight: 40 }),
    ).toBe("visible");
  });

  it("treats a node exactly flush with the top edge (bottom === scrollTop) as 'above'", () => {
    expect(
      classifyNodePosition(VIEWPORT, { offsetTop: 160, offsetHeight: 40 }),
    ).toBe("above");
  });
});

describe("updateUnseenChangedIds", () => {
  it("adds newly-changed ids that are currently out of view to the tracked set", () => {
    const geometryById = new Map([["a", { offsetTop: 50, offsetHeight: 20 }]]);
    const next = updateUnseenChangedIds(
      new Set(),
      ["a"],
      VIEWPORT,
      geometryById,
    );
    expect(next.has("a")).toBe(true);
  });

  it("never adds a newly-changed id that is ALREADY visible — the anchor must not claim a change the reader can already see", () => {
    const geometryById = new Map([["a", { offsetTop: 210, offsetHeight: 20 }]]);
    const next = updateUnseenChangedIds(
      new Set(),
      ["a"],
      VIEWPORT,
      geometryById,
    );
    expect(next.has("a")).toBe(false);
  });

  it("drops a previously-tracked id once it scrolls into view", () => {
    const geometryById = new Map([
      ["a", { offsetTop: 210, offsetHeight: 20 }], // now visible
    ]);
    const next = updateUnseenChangedIds(
      new Set(["a"]),
      [],
      VIEWPORT,
      geometryById,
    );
    expect(next.has("a")).toBe(false);
  });

  it("drops a tracked id whose geometry can no longer be resolved (deleted, or a stale id from a different session)", () => {
    const next = updateUnseenChangedIds(
      new Set(["stale"]),
      [],
      VIEWPORT,
      new Map(),
    );
    expect(next.has("stale")).toBe(false);
  });

  it("keeps a tracked id that is still out of view and unrelated to this fold's new changes", () => {
    const geometryById = new Map([["a", { offsetTop: 50, offsetHeight: 20 }]]);
    const next = updateUnseenChangedIds(
      new Set(["a"]),
      [],
      VIEWPORT,
      geometryById,
    );
    expect(next.has("a")).toBe(true);
  });
});

describe("splitByDirection", () => {
  it("buckets out-of-view ids into above/below and drops unresolvable ones", () => {
    const geometryById = new Map([
      ["above1", { offsetTop: 10, offsetHeight: 10 }],
      ["below1", { offsetTop: 400, offsetHeight: 10 }],
      ["gone", null],
    ]);
    const split = splitByDirection(
      new Set(["above1", "below1", "gone"]),
      VIEWPORT,
      geometryById,
    );
    expect(split.above).toEqual(["above1"]);
    expect(split.below).toEqual(["below1"]);
  });

  it("returns empty buckets when nothing is tracked", () => {
    expect(splitByDirection(new Set(), VIEWPORT, new Map())).toEqual({
      above: [],
      below: [],
    });
  });
});

describe("nearestNodeId", () => {
  const geometryById = new Map([
    ["far-above", { offsetTop: 10, offsetHeight: 10 }],
    ["near-above", { offsetTop: 150, offsetHeight: 10 }],
    ["near-below", { offsetTop: 310, offsetHeight: 10 }],
    ["far-below", { offsetTop: 900, offsetHeight: 10 }],
  ]);

  it("picks the ABOVE id closest to the viewport (largest offsetTop)", () => {
    expect(
      nearestNodeId(["far-above", "near-above"], "above", geometryById),
    ).toBe("near-above");
  });

  it("picks the BELOW id closest to the viewport (smallest offsetTop)", () => {
    expect(
      nearestNodeId(["far-below", "near-below"], "below", geometryById),
    ).toBe("near-below");
  });

  it("returns null when no id resolves to real geometry", () => {
    expect(nearestNodeId(["ghost"], "above", new Map())).toBeNull();
  });
});

describe("computeScrollTopToCenter", () => {
  it("centers the node in the viewport", () => {
    // node spans [500, 520), viewport height 100 -> center target = 510 - 50 = 460.
    const target = computeScrollTopToCenter(
      { scrollTop: 0, clientHeight: 100 },
      { offsetTop: 500, offsetHeight: 20 },
      2000,
    );
    expect(target).toBe(460);
  });

  it("clamps to 0 for a node near the very top of the document", () => {
    const target = computeScrollTopToCenter(
      { scrollTop: 0, clientHeight: 100 },
      { offsetTop: 10, offsetHeight: 20 },
      2000,
    );
    expect(target).toBe(0);
  });

  it("clamps to scrollHeight - clientHeight for a node near the very bottom", () => {
    const target = computeScrollTopToCenter(
      { scrollTop: 0, clientHeight: 100 },
      { offsetTop: 1990, offsetHeight: 10 },
      2000,
    );
    expect(target).toBe(1900);
  });
});
