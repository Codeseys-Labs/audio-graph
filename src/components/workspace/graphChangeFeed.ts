/**
 * `graphChangeFeed` — the KG strip's `feed` mode change-list derivation
 * (ticket W7, synthesis audio-graph-a6b5). Pure, framework-free.
 *
 * Unlike `graphFocus.ts` (which reads the MATERIALIZED graph's own
 * `updated_by_sequence` bookkeeping — cheap, no replay), the feed needs
 * per-operation SEMANTICS ("added" vs. "renamed" vs. a new edge) that only
 * the projection patch stream itself carries; the materialized graph is
 * already-folded state with no memory of what kind of change produced it.
 * So this module reads `sessionProjectionEvents` directly (design-b §4.2:
 * "read `sessionProjectionEvents` filtered to `kind === 'graph'`, one line
 * per operation") — a single forward scan, oldest-to-newest, tracking node
 * names seen so far to distinguish a genuinely new id (→ "added") from an
 * id re-upserted under a different name (→ "renamed"); a re-upsert with an
 * UNCHANGED name is deliberately not a line at all, mirroring W5's own
 * "byte-identical churn produces no visual event" honesty rule instead of
 * inventing motion for a no-op.
 *
 * Deliberately out of scope (disclosed, not silently dropped): edge
 * strengthen/weaken, node/edge remove/invalidate, merges, and splits do not
 * produce a feed line yet. The ticket's three required shapes are node-add,
 * node-rename, and edge-add; widening the vocabulary is a follow-up, not a
 * silent gap — `sessionProjectionEvents` is never truncated by this module
 * (design-b §4.2: "do not cap the store array"), only the RENDERED feed is,
 * by the caller.
 */

export type GraphChangeLine =
  | { key: string; kind: "added"; name: string; entityType: string }
  | { key: string; kind: "renamed"; name: string }
  | { key: string; kind: "edge"; sourceName: string; targetName: string };

/** Minimal shape this module needs from `ProjectionOperation`'s graph-kind
 * variants (`../../types`) — declared locally, zero import surface,
 * mirroring `graphFocus.ts`/`liveDocumentModel.ts`'s convention. */
export type GraphChangeOperation =
  | { type: "upsert_graph_node"; id: string; name: string; entity_type: string }
  | { type: "upsert_graph_edge"; id: string; source: string; target: string }
  | { type: string; id?: string };

export interface GraphChangePatch {
  sequence: number;
  kind: string;
  operations: readonly GraphChangeOperation[];
}

/**
 * Forward scan over every `kind === "graph"` patch, oldest to newest (the
 * order `sessionProjectionEvents` is already appended in — `store/index.ts`
 * `addProjectionPatch` never reorders). Returns lines in the SAME
 * chronological order; the caller (`LiveGraphStrip`) reverses for
 * newest-first display and applies the render cap.
 */
export function selectGraphChangeLines(
  patches: readonly GraphChangePatch[],
): GraphChangeLine[] {
  const knownNodeNames = new Map<string, string>();
  const knownEdgeIds = new Set<string>();
  const lines: GraphChangeLine[] = [];

  for (const patch of patches) {
    if (patch.kind !== "graph") continue;
    patch.operations.forEach((op, opIndex) => {
      const key = `${patch.sequence}-${opIndex}`;
      if (op.type === "upsert_graph_node" && "name" in op) {
        const priorName = knownNodeNames.get(op.id);
        if (priorName === undefined) {
          lines.push({
            key,
            kind: "added",
            name: op.name,
            entityType: op.entity_type,
          });
        } else if (priorName !== op.name) {
          lines.push({ key, kind: "renamed", name: op.name });
        }
        knownNodeNames.set(op.id, op.name);
      } else if (op.type === "upsert_graph_edge" && "source" in op) {
        if (knownEdgeIds.has(op.id)) return;
        knownEdgeIds.add(op.id);
        lines.push({
          key,
          kind: "edge",
          sourceName: knownNodeNames.get(op.source) ?? op.source,
          targetName: knownNodeNames.get(op.target) ?? op.target,
        });
      }
    });
  }

  return lines;
}

/**
 * Closed-ontology display key for the `graphStrip.entityType.*` i18n
 * namespace, given a raw `entity_type` wire value. The ontology
 * (`src-tauri/src/ontology.rs`'s `ENTITY_TYPES`) is a closed ten-name set —
 * Person, Organization, Location, Event, Topic, Product, Task, Question,
 * Decision, Date — but nothing guarantees the exact byte-for-byte casing
 * that reaches the frontend (the backend itself compares `entity_type`
 * case-insensitively, `projections.rs`), so this normalizes the same way
 * `materializedGraph.ts`'s `entityTypeColor` already does before matching.
 * Anything unrecognized falls back to `"other"` rather than ever handing an
 * untranslated raw enum value to a caller for direct interpolation — the
 * leak this function exists to close (a raw `entity_type` like
 * `"organization"` rendering verbatim, in English, inside a pt-locale
 * feed line).
 */
export function entityTypeI18nKey(entityType: string): string {
  const normalized = entityType.trim().toLowerCase();
  switch (normalized) {
    case "person":
    case "organization":
    case "location":
    case "event":
    case "topic":
    case "product":
    case "task":
    case "question":
    case "decision":
    case "date":
      return normalized;
    default:
      return "other";
  }
}
