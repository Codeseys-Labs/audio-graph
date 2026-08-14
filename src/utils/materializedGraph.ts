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
