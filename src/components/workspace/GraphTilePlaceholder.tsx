import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { useSessionView } from "../../session/SessionViewProvider";
import { useAudioGraphStore } from "../../store";
import { materializedGraphToSnapshot } from "../../utils/materializedGraph";

/**
 * Placeholder content for the bento graph tile (ticket W4). The tile
 * REGION and shell ship in this ticket; W7 replaces this body with the KG
 * strip's focus canvas/feed modes (synthesis §W7). Until then this renders
 * the same client-derived node/edge summary `KnowledgeGraphViewer` already
 * computes — the same shared-selector fallback rule constraints.md's recon
 * verified live (`materializedGraphToSnapshot(...) ?? graphSnapshot`) — or
 * `workspace.tile.graphEmpty` when the graph has nothing yet, so the tile is
 * never a bare unlabeled box. NOT `graph.empty`: that string ("Start
 * capturing audio to build the knowledge graph") was written for
 * `KnowledgeGraphViewer`'s Sessions-replay-only empty state; this tile
 * mounts exclusively in the live/reviewing capture branch (`App.tsx`), i.e.
 * precisely while the user IS already capturing or is reviewing a finished
 * session — "start capturing" is wrong copy in both of this tile's actual
 * contexts.
 */
export function GraphTilePlaceholder() {
  const { t } = useTranslation();
  const { graphSnapshot } = useSessionView();
  const materializedProjectionGraph = useAudioGraphStore(
    (s) => s.materializedProjectionGraph,
  );
  const graph = useMemo(
    () =>
      materializedGraphToSnapshot(materializedProjectionGraph) ?? graphSnapshot,
    [materializedProjectionGraph, graphSnapshot],
  );

  if (graph.nodes.length === 0) {
    return <p className="panel-empty">{t("workspace.tile.graphEmpty")}</p>;
  }

  return (
    <div className="flex flex-col gap-(--space-2) p-(--space-5) text-sm text-text-secondary">
      <span>{t("graph.stats.nodes", { count: graph.nodes.length })}</span>
      <span>{t("graph.stats.edges", { count: graph.links.length })}</span>
    </div>
  );
}

export default GraphTilePlaceholder;
