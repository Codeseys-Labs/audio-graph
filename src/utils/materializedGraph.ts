import type {
  GraphLink,
  GraphNode,
  GraphSnapshot,
  MaterializedGraph,
  MaterializedGraphEdge,
  MaterializedGraphNode,
} from "../types";

function entityTypeColor(entityType: string): string {
  switch (entityType.trim().toLowerCase()) {
    case "person":
      return "#60a5fa";
    case "organization":
    case "org":
      return "#a78bfa";
    case "location":
      return "#34d399";
    case "project":
    case "product":
      return "#f59e0b";
    case "topic":
      return "#f472b6";
    default:
      return "#94a3b8";
  }
}

function relationTypeColor(relationType: string): string {
  switch (relationType.trim().toLowerCase()) {
    case "owns":
    case "works_at":
      return "#60a5fa";
    case "tracks":
    case "mentions":
      return "#a78bfa";
    case "evaluates":
    case "shortlists":
      return "#f59e0b";
    default:
      return "#94a3b8";
  }
}

function projectionNodeValue(confidence: number): number {
  if (!Number.isFinite(confidence)) return 1;
  return Math.max(1, Math.round(Math.max(0, Math.min(1, confidence)) * 3));
}

function isActiveMaterializedNode(node: MaterializedGraphNode): boolean {
  return node.valid_until_ms == null;
}

/**
 * Minimum shared-prefix length ratio (shorter/longer, over the
 * alphanumeric-only "fuzzy core" — see {@link fuzzyEntityNameCore}) for two
 * entity names to be treated as the same real-world entity purely from
 * their spelling (seed audio-graph-e700 sub-fix 3). Mirrors the Rust
 * constant of the same purpose in `src-tauri/src/projections.rs`
 * (`FUZZY_ENTITY_NAME_MIN_PREFIX_RATIO`) — this frontend copy exists because
 * the live incremental view (`applyProjectionGraphPatch` in `store/index.ts`)
 * applies the SAME `ProjectionPatch.operations` the backend's
 * `MaterializedGraph::apply_patch` does, and must resolve node identity
 * identically or the live view would visibly diverge from what a session
 * reload replays. See the Rust constant's doc comment for the full
 * calibration rationale (measured Jaro-Winkler false positives on
 * generic model-invented labels like "task 1"/"task 2", and why a plain
 * similarity threshold cannot separate that class from a genuine
 * abbreviation like "postgres"/"postgresql").
 */
const FUZZY_ENTITY_NAME_MIN_PREFIX_RATIO = 0.6;

/**
 * Case/whitespace-normalized name for EXACT-match comparisons: trim,
 * collapse internal whitespace runs to a single space, lowercase.
 * Punctuation is preserved at this tier — see {@link fuzzyEntityNameCore}
 * for the looser tier that also strips it.
 */
function normalizedEntityName(name: string): string {
  return name.trim().split(/\s+/).join(" ").toLowerCase();
}

/**
 * Alphanumeric-only "core" of a name, used ONLY by the prefix/ratio fuzzy
 * tier in {@link fuzzyEntityNameMatch}. Unicode-aware to mirror the Rust
 * side's `char::is_alphanumeric()` (audio-graph-e700 fix): iterates by
 * Unicode CODE POINT via `Array.from` (not `.split("")`, which splits by
 * UTF-16 code UNIT and would corrupt astral-plane characters) and keeps
 * every letter or number in ANY script via the `\p{L}`/`\p{N}` Unicode
 * property escapes, not just ASCII `[a-z0-9]`. The previous ASCII-only
 * filter silently stripped accents and non-Latin scripts entirely — e.g.
 * "José" became "jos", a false PREFIX of "jose" — and produced an EMPTY
 * core for CJK/other non-Latin names, which always fails the match, so the
 * live incremental view (this file) and the backend replay
 * (`fuzzy_entity_name_core` in `src-tauri/src/projections.rs`) resolved node
 * identity differently for any non-ASCII name. This is not a byte-for-byte
 * equivalent of Rust's exact Unicode category boundaries for every
 * character class in existence (`\p{L}`/`\p{N}` vs `is_alphabetic() ||
 * is_numeric()` can disagree on a handful of rare combining-mark/letter-
 * number edge cases) — it is a close, disclosed match that is correct for
 * ordinary person/place/organization names in any script, which is the
 * entire practical surface this function exists to compare.
 */
function fuzzyEntityNameCore(name: string): string {
  return Array.from(name.toLowerCase())
    .filter((char) => /[\p{L}\p{N}]/u.test(char))
    .join("");
}

/**
 * True when `a` and `b` name the SAME real-world entity closely enough to
 * merge automatically at projection-graph ingest (seed audio-graph-e700
 * sub-fixes 2 and 3). Mirrors
 * `projections::fuzzy_entity_name_match` in the Rust backend exactly — see
 * that function's doc comment for the full rationale.
 */
export function fuzzyEntityNameMatch(a: string, b: string): boolean {
  if (normalizedEntityName(a) === normalizedEntityName(b)) return true;
  const coreA = fuzzyEntityNameCore(a);
  const coreB = fuzzyEntityNameCore(b);
  if (coreA === "" || coreB === "") return false;
  if (coreA === coreB) return true;
  // Length in Unicode CODE POINTS (`Array.from(...).length`), not UTF-16
  // code units (`.length`): the Rust mirror counts `.chars()` (Unicode
  // scalar values), and a plain `.length` disagrees with that for any
  // multi-byte-per-character script, which would silently desync the ratio
  // — and therefore the merge decision — between the live view and a
  // replayed session for the identical pair of names (audio-graph-e700).
  const lenA = Array.from(coreA).length;
  const lenB = Array.from(coreB).length;
  const [shorter, longer, lenShorter, lenLonger] =
    lenA <= lenB ? [coreA, coreB, lenA, lenB] : [coreB, coreA, lenB, lenA];
  if (!longer.startsWith(shorter)) return false;
  return lenShorter / lenLonger >= FUZZY_ENTITY_NAME_MIN_PREFIX_RATIO;
}

function isActiveMaterializedEdge(edge: MaterializedGraphEdge): boolean {
  return edge.valid_until_ms == null;
}

/**
 * Convert the canonical projection graph into the common display snapshot.
 *
 * A present-but-empty graph returns an empty snapshot, not `null`: an accepted
 * delete/retcon that removes every node is still authoritative and must never
 * fall through to an older legacy graph cache.
 */
export function materializedGraphToSnapshot(
  graph: MaterializedGraph | null | undefined,
): GraphSnapshot | null {
  if (!graph) return null;

  const activeNodes = graph.nodes.filter(isActiveMaterializedNode);
  const activeNodeIds = new Set(activeNodes.map((node) => node.id));
  const nodes: GraphNode[] = activeNodes.map((node) => ({
    id: node.id,
    name: node.name,
    entity_type: node.entity_type,
    val: projectionNodeValue(node.confidence),
    color: entityTypeColor(node.entity_type),
    first_seen: node.valid_from_ms,
    last_seen: node.updated_at_ms,
    mention_count: 1,
    description: node.description ?? undefined,
  }));

  const links: GraphLink[] = graph.edges
    .filter(
      (edge) =>
        isActiveMaterializedEdge(edge) &&
        activeNodeIds.has(edge.source) &&
        activeNodeIds.has(edge.target),
    )
    .map((edge) => ({
      id: edge.id,
      source: edge.source,
      target: edge.target,
      relation_type: edge.relation_type,
      weight: edge.weight,
      color: relationTypeColor(edge.relation_type),
      label: edge.label ?? undefined,
    }));

  return {
    nodes,
    links,
    stats: {
      total_nodes: nodes.length,
      total_edges: links.length,
      total_episodes: 0,
    },
  };
}
