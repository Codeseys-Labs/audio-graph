import type { ReactNode } from "react";

/**
 * The frozen bento tile-id union (ticket W4, synthesis audio-graph-a6b5;
 * design-a §4.3 requirement 7). These four strings are a public contract:
 * phase-2's `WorkspaceLayoutPrefs` (order/hidden/sizes, not shipped until
 * phase 2 — see that interface's doc comment in this same file's ticket)
 * persists them verbatim in `localStorage`, so adding, removing, or
 * renaming a member here is a breaking change to a schema this repo has
 * already committed to reading forward-compatibly. `WorkspaceTile.test.tsx`
 * pins the exact set with a frozen-contract test — a change here without a
 * matching, deliberate test update is treated as a regression.
 */
export type WorkspaceTileId = "transcript" | "graph" | "document" | "agent";

/**
 * `WorkspaceLayoutPrefs` — the phase-2 persistence schema, designed now
 * (design-a §4.3) but NOT read or written in phase 1 (constraints.md:
 * "Phase 1 of bento is a FIXED default layout. No drag/resize machinery in
 * phase 1 tickets. Persistence schema may be designed now but ships in
 * phase 2."). No reader/writer exists yet; this type is documentation for
 * the phase-2 implementer, not live code.
 *
 * localStorage key (reserved, unused in phase 1): `ag.workspaceLayout`.
 * READ RULES the phase-2 reader must implement:
 *   - unknown tile id in `order`/`hidden`/`sizes` → ignored
 *   - known tile id missing from `order`          → appended in default order
 *   - `schema_version !== 1`                      → discard, use defaults
 *   - all four tiles in `hidden`                  → discard (never an empty room)
 */
export interface WorkspaceLayoutPrefs {
  schema_version: 1;
  order: WorkspaceTileId[];
  hidden: WorkspaceTileId[];
  sizes: Partial<Record<WorkspaceTileId, number>>;
}

export interface WorkspaceTileProps {
  id: WorkspaceTileId;
  title: string;
  /**
   * Right-hand header action slot. Always rendered (even when omitted, as
   * an empty element) rather than conditionally omitted — design-a §4.3
   * requirement 5: phase-2's collapse/hide control lands here without the
   * header's flex layout shifting when it arrives.
   */
  headerSlot?: ReactNode;
  children: ReactNode;
}

/**
 * The shared bento tile shell (ticket W4, synthesis audio-graph-a6b5). Every
 * tile — transcript/graph/document/agent — renders through this component so
 * the phase-2 markup contract (design-a §4.3) is uniform from phase 1:
 *
 *  - `data-tile={id}` is the ONLY thing that ever assigns `grid-area`
 *    (`[data-tile="…"]` rules in `layout.css`, never an inline `style`) —
 *    phase 2's "arrange" rewrites `grid-template-areas` on the grid
 *    container from saved prefs with zero changes to this component or its
 *    callers (requirement 2/3).
 *  - The implicit `region` role (a `<section>` with an accessible name —
 *    see the `aria-labelledby` below, pointing at a real rendered title
 *    node) gives every tile a stable accessible name (requirement 1).
 *  - `container: tile / inline-size` lands on the root via the
 *    `.workspace-tile` CSS class in the unlayered `layout.css` barrel
 *    (responsive-memo-72d4 §2/§4 item 8) — phase 1 ships no resize, but
 *    adding containment now means nothing already inside a tile ever
 *    depended on its absence when phase 2's resize lands.
 *  - The frame (`.workspace-tile`) is `min-width:0; min-height:0;
 *    overflow:hidden` so a phase-2 resize can never blow out the grid
 *    (requirement 6); the tile's own content owns its scroll behavior
 *    inside `.workspace-tile__body` (mirrors the pre-W4
 *    `workspace-panel__primary`/`__transcript` convention, since every
 *    phase-1 child — `NotesPanel`/`LiveTranscript`/`AgentProposalsPanel` —
 *    already manages its own internal scroll region).
 */
export function WorkspaceTile({
  id,
  title,
  headerSlot,
  children,
}: WorkspaceTileProps) {
  const titleId = `tile-${id}-title`;
  // No explicit `role="region"`: a `<section>` with an accessible name
  // (the `aria-labelledby` below) already computes to the `region` role
  // per HTML-AAM — biome's `noRedundantRoles` lint (repo gate) flags the
  // literal attribute as redundant. `getByRole("region", …)` in tests
  // resolves this implicit mapping identically to an explicit role.
  return (
    <section
      className="workspace-tile"
      data-tile={id}
      aria-labelledby={titleId}
    >
      <div className="ag-panel-head workspace-tile__head">
        <span id={titleId} className="ag-panel-head__title">
          {title}
        </span>
        <span className="workspace-tile__header-slot">
          {headerSlot ?? null}
        </span>
      </div>
      <div className="workspace-tile__body">{children}</div>
    </section>
  );
}

export default WorkspaceTile;
