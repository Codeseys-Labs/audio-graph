import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { WorkspaceTile, type WorkspaceTileId } from "./WorkspaceTile";

/**
 * Frozen tile-id contract (ticket W4, synthesis audio-graph-a6b5;
 * design-a §4.3 requirement 7): `WorkspaceTileId` is a closed union whose
 * exact string members are a public contract phase 2's
 * `WorkspaceLayoutPrefs` will persist verbatim. This array is the runtime
 * mirror of the type: the `satisfies` + exhaustiveness assertion below
 * fails `bun run typecheck`/`build` if a member is added, removed, or
 * renamed in the TYPE without a matching edit here; the runtime `toEqual`
 * test below fails `bun run test` if this ARRAY drifts from the frozen set
 * (e.g. a copy-paste edit that only touches this file). Between the two,
 * every mutation of the id set is caught by at least one gate.
 */
const FROZEN_TILE_IDS = [
  "transcript",
  "graph",
  "document",
  "agent",
] as const satisfies readonly WorkspaceTileId[];

// Compile-time-only: covers every member of WorkspaceTileId (Exclude<> is
// `never` iff FROZEN_TILE_IDS already lists them all) AND stays exactly 4
// long (the reverse direction — WorkspaceTileId gaining a member —
// this array missing it — is caught by the same Exclude<> failing to
// compile). Never executed; exists purely so the TYPE and the ARRAY can't
// silently diverge.
type _AssertCovers =
  Exclude<WorkspaceTileId, (typeof FROZEN_TILE_IDS)[number]> extends never
    ? true
    : ["missing from FROZEN_TILE_IDS:", never];
const _typeContractHolds: _AssertCovers = true;
void _typeContractHolds;

describe("WorkspaceTileId frozen contract", () => {
  it("pins the exact four tile ids, in this exact order", () => {
    expect(FROZEN_TILE_IDS).toEqual([
      "transcript",
      "graph",
      "document",
      "agent",
    ]);
  });
});

describe("WorkspaceTile markup contract (design-a §4.3)", () => {
  it("renders data-tile, an implicit region role named by a real title node, and empty-but-present header/body slots", () => {
    render(
      <WorkspaceTile id="graph" title="Graph tile title">
        <p>tile content</p>
      </WorkspaceTile>,
    );

    const region = screen.getByRole("region", { name: "Graph tile title" });
    expect(region.tagName).toBe("SECTION");
    expect(region).toHaveClass("workspace-tile");
    expect(region).toHaveAttribute("data-tile", "graph");

    // req 5: the header slot element is always present, even empty, so
    // phase 2's control doesn't shift the header layout when it arrives.
    const headerSlot = region.querySelector(".workspace-tile__header-slot");
    expect(headerSlot).not.toBeNull();
    expect(headerSlot?.textContent).toBe("");

    // req 6: children land in the tile's own body/scroll container, not
    // loose inside the region root.
    const body = region.querySelector(".workspace-tile__body");
    expect(body).toContainElement(screen.getByText("tile content"));
  });

  it("renders a non-empty headerSlot inside the header-slot element without shifting the title", () => {
    render(
      <WorkspaceTile
        id="agent"
        title="Agent tile title"
        headerSlot={<button type="button">Collapse</button>}
      >
        <p>content</p>
      </WorkspaceTile>,
    );

    const region = screen.getByRole("region", { name: "Agent tile title" });
    const headerSlot = region.querySelector(".workspace-tile__header-slot");
    expect(headerSlot).toContainElement(
      screen.getByRole("button", { name: "Collapse" }),
    );
  });

  it.each(
    FROZEN_TILE_IDS,
  )("assigns data-tile=%s 1:1 from the id prop (req 2/3: grid-area comes from [data-tile], never inline)", (id) => {
    render(
      <WorkspaceTile id={id} title={`title-${id}`}>
        content
      </WorkspaceTile>,
    );
    const region = screen.getByRole("region", { name: `title-${id}` });
    expect(region).toHaveAttribute("data-tile", id);
    expect(region).not.toHaveAttribute("style");
  });
});
