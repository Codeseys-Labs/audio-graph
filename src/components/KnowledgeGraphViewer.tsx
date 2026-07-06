/**
 * Knowledge graph viewer — the centre-panel force-directed 2D graph.
 *
 * Renders the current `GraphSnapshot` (entities as nodes, relations as
 * edges) using `react-force-graph-2d`. A `ResizeObserver` keeps the canvas
 * sized to its parent container; node radius scales with `val` (mention
 * count). Click-to-focus, hover tooltip, and a JSON export button that
 * dumps the current graph via `exportGraph` + `downloadAsFile` are wired in
 * this component.
 *
 * Store bindings: `materializedProjectionGraph` when available, otherwise
 * legacy `graphSnapshot` from `GRAPH_UPDATE` events, plus `exportGraph` and
 * `getSessionId`.
 *
 * Parent: `App.tsx` main panel. No props.
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import ForceGraph2D, {
  type ForceGraphMethods,
  type LinkObject,
  type NodeObject,
} from "react-force-graph-2d";
import { useTranslation } from "react-i18next";
import { useAudioGraphStore } from "../store";
import type {
  GraphLink,
  GraphNode,
  GraphSnapshot,
  MaterializedGraph,
  MaterializedGraphEdge,
  MaterializedGraphNode,
} from "../types";
import { downloadAsFile, filenameTimestamp } from "../utils/download";
import { errorToMessage } from "../utils/errorToMessage";
import { formatTime } from "../utils/format";
import Icon from "./Icon";
import IconButton from "./IconButton";

/** Compute node radius from val. */
function nodeRadius(val: number): number {
  const r = Math.sqrt(val) * 3 + 4;
  return Math.max(4, Math.min(24, r));
}

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

function materializedGraphToSnapshot(
  graph: MaterializedGraph | null,
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

  if (nodes.length === 0 && links.length === 0) return null;

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

/**
 * Custom d3 force that gently pulls every node toward the origin (0,0),
 * proportional to its distance. This contains "alienated" nodes — entities
 * with no edges receive no link-force pull, and with only a repulsive charge
 * they otherwise drift toward infinity, which makes zoomToFit zoom out so far
 * the connected core becomes an unreadable speck (W3.6). A weak radial pull
 * keeps isolates near the cluster without collapsing the layout.
 */
function forceContain(strength: number) {
  let nodes: Array<{ x?: number; y?: number; vx?: number; vy?: number }> = [];
  const force = (alpha: number) => {
    for (const n of nodes) {
      n.vx = (n.vx ?? 0) - (n.x ?? 0) * strength * alpha;
      n.vy = (n.vy ?? 0) - (n.y ?? 0) * strength * alpha;
    }
  };
  force.initialize = (n: typeof nodes) => {
    nodes = n;
  };
  return force;
}

/// Escape HTML special chars. react-force-graph renders node/link labels as
/// raw HTML tooltips, and entity text is model-derived from arbitrary speech,
/// so all interpolated values must be escaped (XSS guard — critique H7).
function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

function KnowledgeGraphViewer() {
  const { t } = useTranslation();
  const graphSnapshot = useAudioGraphStore((s) => s.graphSnapshot);
  const materializedProjectionGraph = useAudioGraphStore(
    (s) => s.materializedProjectionGraph,
  );
  const exportGraph = useAudioGraphStore((s) => s.exportGraph);
  const getSessionId = useAudioGraphStore((s) => s.getSessionId);
  // Cross-component edge focus from the After seek-timeline's related-edges
  // badge (audio-graph-a2a7): the ids of the graph edges an utterance produced.
  const graphEdgeFocus = useAudioGraphStore((s) => s.graphEdgeFocus);
  // Re-read theme-derived canvas colors whenever the explicit choice changes.
  // (System mode is handled by the prefers-color-scheme listener below.)
  const theme = useAudioGraphStore((s) => s.theme);

  // ResizeObserver for auto-sizing to parent container
  const containerRef = useRef<HTMLDivElement>(null);
  const graphRef = useRef<ForceGraphMethods | undefined>(undefined);
  const [dimensions, setDimensions] = useState({ width: 600, height: 400 });

  const [isExporting, setIsExporting] = useState(false);
  const [exportError, setExportError] = useState<string | null>(null);
  const activeGraphSnapshot = useMemo(
    () =>
      materializedGraphToSnapshot(materializedProjectionGraph) ?? graphSnapshot,
    [materializedProjectionGraph, graphSnapshot],
  );

  // Theme-aware canvas colors. The react-force-graph canvas is painted in JS,
  // so it cannot consume CSS tokens directly; we resolve the relevant
  // semantic tokens via getComputedStyle and repaint when the theme changes
  // (explicit choice via `theme`, or the system scheme via the media query).
  const [graphColors, setGraphColors] = useState({
    nodeLabel: "#e8e8e8",
    highlightRing: "#e7ebf2",
  });
  useEffect(() => {
    const readColors = () => {
      const styles = getComputedStyle(document.documentElement);
      const nodeLabel =
        styles.getPropertyValue("--graph-node-label").trim() || "#e8e8e8";
      const highlightRing =
        styles.getPropertyValue("--text-primary").trim() || "#e7ebf2";
      setGraphColors({ nodeLabel, highlightRing });
    };
    readColors();
    // Only the "system" choice follows the OS scheme; an explicit light/dark
    // choice pins the palette, so we don't need the media listener then.
    // (matchMedia is also absent in non-browser/test environments.)
    if (theme !== "system" || typeof window.matchMedia !== "function") return;
    const mq = window.matchMedia("(prefers-color-scheme: light)");
    mq.addEventListener("change", readColors);
    return () => mq.removeEventListener("change", readColors);
  }, [theme]);

  const handleExportJson = useCallback(async () => {
    setIsExporting(true);
    setExportError(null);
    try {
      const json = await exportGraph();
      let sessionId = "session";
      try {
        sessionId = await getSessionId();
      } catch {
        // Non-fatal — keep the fallback.
      }
      const filename = `graph-${sessionId}-${filenameTimestamp()}.json`;
      downloadAsFile(json, filename, "application/json");
    } catch (e) {
      setExportError(errorToMessage(e));
    } finally {
      setIsExporting(false);
    }
  }, [exportGraph, getSessionId]);

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;

    const observer = new ResizeObserver((entries) => {
      for (const entry of entries) {
        const { width, height } = entry.contentRect;
        if (width > 0 && height > 0) {
          setDimensions({
            width: Math.floor(width),
            height: Math.floor(height),
          });
        }
      }
    });

    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  // Spread nodes out a bit more than the library default and reheat the
  // simulation only when the node COUNT grows (new nodes were seeded near
  // their neighbours by the store; a gentle reheat lets them settle without
  // disturbing the already-placed graph). Avoids the "all edges fan to origin"
  // jank without reheating on every data refresh.
  const prevNodeCount = useRef(0);
  const forcesTuned = useRef(false);
  useEffect(() => {
    const fg = graphRef.current;
    if (!fg) return;
    if (!forcesTuned.current) {
      const charge = fg.d3Force("charge");
      if (charge && "strength" in charge) {
        // Moderate repulsion: enough to separate nodes, not so much that a
        // small graph flings nodes off-screen.
        (charge as unknown as { strength: (n: number) => void }).strength(-90);
      }
      const link = fg.d3Force("link");
      if (link && "distance" in link) {
        (link as unknown as { distance: (n: number) => void }).distance(45);
      }
      // Radial containment so edgeless/peripheral nodes stay near the cluster
      // instead of drifting off-screen and wrecking zoomToFit (W3.6).
      (
        fg as unknown as { d3Force: (name: string, f: unknown) => void }
      ).d3Force("contain", forceContain(0.045));
      forcesTuned.current = true;
    }
    const count = activeGraphSnapshot.nodes.length;
    if (count > prevNodeCount.current) {
      fg.d3ReheatSimulation();
    }
    prevNodeCount.current = count;
  }, [activeGraphSnapshot.nodes.length]);

  // Frame all nodes into the viewport. Called from the Fit button and
  // automatically when the layout settles, so the graph never drifts off-screen.
  const fitView = useCallback(() => {
    graphRef.current?.zoomToFit(400, 60);
  }, []);

  // Highlight state — track clicked node
  const [highlightNodeId, setHighlightNodeId] = useState<string | null>(null);
  const [highlightNeighbors, setHighlightNeighbors] = useState<Set<string>>(
    new Set(),
  );
  // Click-to-inspect: the node whose details are shown in the side panel.
  const [selectedNode, setSelectedNode] = useState<GraphNode | null>(null);

  // Focused-edge state — the set of edge ids the After seek-timeline badge asked
  // us to emphasize (audio-graph-a2a7). Held locally (seeded from the store's
  // `graphEdgeFocus` on each new nonce) so a background click can clear the
  // emphasis without racing the store, exactly like `highlightNodeId`.
  const [focusedEdgeIds, setFocusedEdgeIds] = useState<Set<string>>(new Set());
  // Keyed on the nonce so re-activating the SAME badge re-fires; the id list is
  // read fresh off the store (via `getState`, deliberately non-reactive) inside
  // the effect, so the nonce alone is a sufficient — and stable — trigger.
  const edgeFocusNonce = graphEdgeFocus?.nonce ?? null;
  useEffect(() => {
    if (edgeFocusNonce === null) {
      setFocusedEdgeIds(new Set());
      return;
    }
    const ids = useAudioGraphStore.getState().graphEdgeFocus?.edgeIds ?? [];
    setFocusedEdgeIds(new Set(ids));
    // A fresh edge focus supersedes any prior node highlight so the two
    // dimming modes never fight; the inspect panel is closed for the same reason.
    setHighlightNodeId(null);
    setHighlightNeighbors(new Set());
    setSelectedNode(null);
  }, [edgeFocusNonce]);

  // id → node lookup for the detail panel + neighbor resolution.
  const nodeById = useMemo(() => {
    const map = new Map<string, GraphNode>();
    for (const n of activeGraphSnapshot.nodes) map.set(n.id, n);
    return map;
  }, [activeGraphSnapshot.nodes]);

  // Build neighbor lookup once per snapshot
  const neighborMap = useMemo(() => {
    const map = new Map<string, Set<string>>();
    for (const link of activeGraphSnapshot.links) {
      const src =
        typeof link.source === "object"
          ? (link.source as GraphNode).id
          : link.source;
      const tgt =
        typeof link.target === "object"
          ? (link.target as GraphNode).id
          : link.target;
      if (!map.has(src)) map.set(src, new Set());
      if (!map.has(tgt)) map.set(tgt, new Set());
      map.get(src)?.add(tgt);
      map.get(tgt)?.add(src);
    }
    return map;
  }, [activeGraphSnapshot.links]);

  // Graph data — stable reference for react-force-graph
  const graphData = useMemo(
    () => ({
      nodes: activeGraphSnapshot.nodes as NodeObject[],
      links: activeGraphSnapshot.links as unknown as LinkObject[],
    }),
    [activeGraphSnapshot.nodes, activeGraphSnapshot.links],
  );

  // Click on a node → highlight it + neighbors, and open the inspect panel. A
  // node highlight supersedes any active edge focus (the two dimming modes are
  // mutually exclusive), so clear the focused-edge set here too.
  const handleNodeClick = useCallback(
    (node: NodeObject) => {
      const id = node.id as string;
      setFocusedEdgeIds(new Set());
      if (highlightNodeId === id) {
        setHighlightNodeId(null);
        setHighlightNeighbors(new Set());
        setSelectedNode(null);
      } else {
        setHighlightNodeId(id);
        setHighlightNeighbors(neighborMap.get(id) ?? new Set());
        setSelectedNode(nodeById.get(id) ?? null);
      }
    },
    [highlightNodeId, neighborMap, nodeById],
  );

  // Click on background → reset highlight + edge focus + close the inspect panel.
  const handleBackgroundClick = useCallback(() => {
    setHighlightNodeId(null);
    setHighlightNeighbors(new Set());
    setSelectedNode(null);
    setFocusedEdgeIds(new Set());
  }, []);

  useEffect(() => {
    if (selectedNode && !nodeById.has(selectedNode.id)) {
      setHighlightNodeId(null);
      setHighlightNeighbors(new Set());
      setSelectedNode(null);
    }
  }, [selectedNode, nodeById]);

  // Neighbors of the selected node, resolved to full nodes for the panel.
  const selectedNeighbors = useMemo(() => {
    if (!selectedNode) return [];
    const ids = neighborMap.get(selectedNode.id) ?? new Set<string>();
    return [...ids]
      .map((id) => nodeById.get(id))
      .filter((n): n is GraphNode => Boolean(n))
      .sort((a, b) => b.mention_count - a.mention_count);
  }, [selectedNode, neighborMap, nodeById]);

  // Custom node canvas rendering
  const nodeCanvasObject = useCallback(
    (node: NodeObject, ctx: CanvasRenderingContext2D, globalScale: number) => {
      const gNode = node as NodeObject & GraphNode;
      const x = node.x ?? 0;
      const y = node.y ?? 0;
      const r = nodeRadius(gNode.val ?? 1);

      // Determine dim state when a node is highlighted
      const isDimmed =
        highlightNodeId !== null &&
        highlightNodeId !== gNode.id &&
        !highlightNeighbors.has(gNode.id);

      const alpha = isDimmed ? 0.15 : 1;

      // Draw circle
      ctx.beginPath();
      ctx.arc(x, y, r, 0, 2 * Math.PI, false);
      ctx.globalAlpha = alpha;
      ctx.fillStyle = gNode.color || "#6b7280";
      ctx.fill();

      // Highlight ring on selected node
      if (highlightNodeId === gNode.id) {
        ctx.strokeStyle = graphColors.highlightRing;
        ctx.lineWidth = 2;
        ctx.stroke();
      }

      // Label — show when zoomed in or node is selected
      const fontSize = Math.max(10 / globalScale, 3);
      if (globalScale >= 0.6 || highlightNodeId === gNode.id) {
        ctx.font = `${fontSize}px sans-serif`;
        ctx.textAlign = "center";
        ctx.textBaseline = "top";
        ctx.globalAlpha = alpha;
        ctx.fillStyle = graphColors.nodeLabel;
        ctx.fillText(gNode.name, x, y + r + 2);
      }

      ctx.globalAlpha = 1;
    },
    [highlightNodeId, highlightNeighbors, graphColors],
  );

  // Node pointer area for hit detection
  const nodePointerAreaPaint = useCallback(
    (node: NodeObject, color: string, ctx: CanvasRenderingContext2D) => {
      const gNode = node as NodeObject & GraphNode;
      const x = node.x ?? 0;
      const y = node.y ?? 0;
      const r = nodeRadius(gNode.val ?? 1) + 2;
      ctx.beginPath();
      ctx.arc(x, y, r, 0, 2 * Math.PI, false);
      ctx.fillStyle = color;
      ctx.fill();
    },
    [],
  );

  // Edge focus is only meaningful when at least one focused id is actually
  // present in the RENDERED graph. The seek-timeline badge emits LIVE
  // TemporalKnowledgeGraph edge ids (`edge-{seq}`, from timeline.rs), but the
  // viewer prefers the materialized projection graph when present (loaded
  // sessions / sessions with projection patches), whose link ids are a DIFFERENT
  // namespace (UUIDs) with no per-utterance segment provenance to map through —
  // a materialized edge carries only a whole-window `ProjectionBasis`, never a
  // `source_segment_id` (ADR-0026 §4.1; commands.rs `session_timeline`). So a
  // live-id focus set matches nothing in materialized mode. If dimming keyed on
  // `focusedEdgeIds.size > 0` alone, that would fade EVERY edge (the all-dimmed
  // state) instead of focusing. Invariant (audio-graph-a2a7 fix): a badge click
  // NEVER produces the all-dimmed state — when zero focused ids are present in
  // the rendered graph we treat it as no-focus (no dimming, no-op).
  const hasEdgeFocus = useMemo(() => {
    if (focusedEdgeIds.size === 0) return false;
    return activeGraphSnapshot.links.some(
      (link) => link.id != null && focusedEdgeIds.has(link.id),
    );
  }, [focusedEdgeIds, activeGraphSnapshot.links]);

  // Link width based on weight, thickened when the edge is one of the
  // seek-timeline-focused edges (audio-graph-a2a7) so the focus reads at a glance.
  const linkWidth = useCallback(
    (link: LinkObject) => {
      const gLink = link as LinkObject & GraphLink;
      const base = Math.sqrt(gLink.weight ?? 1) + 0.5;
      if (hasEdgeFocus && gLink.id != null && focusedEdgeIds.has(gLink.id)) {
        return base + 2;
      }
      return base;
    },
    [hasEdgeFocus, focusedEdgeIds],
  );

  // Link color with transparency and dimming. Two dimming modes, checked in
  // priority order: an active edge focus (from the seek-timeline badge) dims
  // every edge that ISN'T in the focused set; otherwise a node highlight dims
  // every edge not incident to the highlighted node. The two are mutually
  // exclusive (selecting one clears the other), so they never compound.
  const linkColor = useCallback(
    (link: LinkObject) => {
      const gLink = link as LinkObject & GraphLink;
      const base = gLink.color || "#6b7280";

      if (hasEdgeFocus) {
        return gLink.id != null && focusedEdgeIds.has(gLink.id)
          ? base
          : `${base}15`;
      }

      if (highlightNodeId !== null) {
        const src =
          typeof gLink.source === "object"
            ? (gLink.source as GraphNode).id
            : gLink.source;
        const tgt =
          typeof gLink.target === "object"
            ? (gLink.target as GraphNode).id
            : gLink.target;
        if (src !== highlightNodeId && tgt !== highlightNodeId) {
          return `${base}15`;
        }
      }

      return `${base}99`;
    },
    [highlightNodeId, hasEdgeFocus, focusedEdgeIds],
  );

  // Link label (shown on hover). react-force-graph renders labels as HTML, so
  // escape model-derived text to prevent injection.
  const linkLabel = useCallback((link: LinkObject) => {
    const gLink = link as LinkObject & GraphLink;
    return escapeHtml(gLink.relation_type ?? "");
  }, []);

  // Node tooltip (HTML). Entity name/type/description are model-derived from
  // arbitrary speech, so every interpolated value is HTML-escaped (XSS guard).
  const nodeLabel = useCallback(
    (node: NodeObject) => {
      const gNode = node as NodeObject & GraphNode;
      const parts = [
        `<strong>${escapeHtml(gNode.name)}</strong>`,
        `${t("graph.inspect.type")}: ${escapeHtml(gNode.entity_type)}`,
        `${t("graph.inspect.mentions")}: ${gNode.mention_count}`,
      ];
      if (gNode.description) parts.push(escapeHtml(gNode.description));
      parts.push(
        `${t("graph.inspect.firstSeen")}: ${formatTime(gNode.first_seen)}`,
      );
      parts.push(
        `${t("graph.inspect.lastSeen")}: ${formatTime(gNode.last_seen)}`,
      );
      return parts.join("<br/>");
    },
    [t],
  );

  const hasNodes = activeGraphSnapshot.nodes.length > 0;
  const { total_nodes, total_edges, total_episodes } =
    activeGraphSnapshot.stats;

  return (
    <div
      className="relative w-full h-full flex-1 flex min-h-0 bg-(--graph-bg)"
      ref={containerRef}
    >
      {!hasNodes ? (
        <div
          className="flex flex-col items-center justify-center w-full h-full select-none"
          role="status"
        >
          <div
            className="text-[56px] text-text-muted opacity-40 mb-(--space-5) leading-none"
            aria-hidden="true"
          >
            <Icon name="graph" size={48} />
          </div>
          <p className="text-text-muted text-base m-0">{t("graph.empty")}</p>
        </div>
      ) : (
        <div className="w-full h-full [&_canvas]:block">
          <ForceGraph2D
            ref={
              graphRef as React.MutableRefObject<ForceGraphMethods | undefined>
            }
            graphData={graphData}
            width={dimensions.width}
            height={dimensions.height}
            backgroundColor="transparent"
            nodeCanvasObject={nodeCanvasObject}
            nodePointerAreaPaint={nodePointerAreaPaint}
            nodeLabel={nodeLabel}
            onNodeClick={handleNodeClick}
            onBackgroundClick={handleBackgroundClick}
            linkWidth={linkWidth}
            linkColor={linkColor}
            linkLabel={linkLabel}
            linkDirectionalArrowLength={4}
            linkDirectionalArrowRelPos={1}
            cooldownTicks={100}
            d3AlphaDecay={0.02}
            d3VelocityDecay={0.3}
            onEngineStop={fitView}
            enableZoomInteraction={true}
            enablePanInteraction={true}
          />
        </div>
      )}

      {hasNodes && (
        <div
          className="absolute bottom-(--space-4) left-(--space-4) flex items-center gap-(--space-3) bg-(--graph-overlay-bg) [backdrop-filter:blur(4px)] py-(--space-2) px-[10px] rounded-md text-xs text-text-secondary pointer-events-none"
          role="status"
          aria-live="polite"
          aria-label={
            total_episodes > 0
              ? t("graph.stats.ariaWithEpisodes", {
                  nodes: total_nodes,
                  edges: total_edges,
                  episodes: total_episodes,
                })
              : t("graph.stats.aria", {
                  nodes: total_nodes,
                  edges: total_edges,
                })
          }
        >
          <span aria-hidden="true">
            {t("graph.stats.nodes", { count: total_nodes })}
          </span>
          <span className="opacity-40" aria-hidden="true">
            |
          </span>
          <span aria-hidden="true">
            {t("graph.stats.edges", { count: total_edges })}
          </span>
          {total_episodes > 0 && (
            <>
              <span className="opacity-40" aria-hidden="true">
                |
              </span>
              <span aria-hidden="true">
                {t("graph.stats.episodes", { count: total_episodes })}
              </span>
            </>
          )}
        </div>
      )}

      {/* Click-to-inspect detail panel (W3.6). Keyboard-reachable, unlike the
          hover-only tooltip. */}
      {selectedNode && (
        <section
          className="absolute top-(--space-5) right-(--space-5) w-[260px] max-h-[calc(100%-var(--space-10))] overflow-y-auto bg-bg-elevated border border-border-color rounded-lg shadow-2 p-(--space-5) z-[var(--z-popover)]"
          aria-label={t("graph.inspect.detailsFor", {
            name: selectedNode.name,
          })}
        >
          <header className="flex items-center gap-(--space-3) mb-(--space-5)">
            <span
              className="w-[10px] h-[10px] rounded-full shrink-0"
              style={{ backgroundColor: selectedNode.color || "#6b7280" }}
              aria-hidden="true"
            />
            <h3 className="flex-1 min-w-0 text-base font-semibold text-text-primary break-words">
              {selectedNode.name}
            </h3>
            <IconButton
              icon="close"
              label={t("graph.inspect.closeDetails")}
              size={14}
              variant="ghost"
              onClick={handleBackgroundClick}
            />
          </header>
          <dl className="grid grid-cols-2 gap-(--space-4) mb-(--space-5)">
            <div>
              <dt className="text-2xs uppercase tracking-[0.04em] text-text-muted">
                {t("graph.inspect.type")}
              </dt>
              <dd className="text-md text-text-primary">
                {selectedNode.entity_type}
              </dd>
            </div>
            <div>
              <dt className="text-2xs uppercase tracking-[0.04em] text-text-muted">
                {t("graph.inspect.mentions")}
              </dt>
              <dd className="text-md text-text-primary">
                {selectedNode.mention_count}
              </dd>
            </div>
            <div>
              <dt className="text-2xs uppercase tracking-[0.04em] text-text-muted">
                {t("graph.inspect.firstSeen")}
              </dt>
              <dd className="text-md text-text-primary">
                {formatTime(selectedNode.first_seen)}
              </dd>
            </div>
            <div>
              <dt className="text-2xs uppercase tracking-[0.04em] text-text-muted">
                {t("graph.inspect.lastSeen")}
              </dt>
              <dd className="text-md text-text-primary">
                {formatTime(selectedNode.last_seen)}
              </dd>
            </div>
          </dl>
          {selectedNode.description && (
            <p className="text-md text-text-secondary leading-[1.5] mb-(--space-5)">
              {selectedNode.description}
            </p>
          )}
          <div className="graph-inspect__neighbors">
            <h4 className="text-xs uppercase tracking-[0.04em] text-text-muted mb-(--space-3)">
              {t("graph.inspect.connections", {
                count: selectedNeighbors.length,
              })}
            </h4>
            {selectedNeighbors.length === 0 ? (
              <p className="text-sm text-text-muted">
                {t("graph.inspect.noConnections")}
              </p>
            ) : (
              <ul className="list-none flex flex-col gap-(--space-1)">
                {selectedNeighbors.slice(0, 12).map((n) => (
                  <li key={n.id}>
                    <button
                      type="button"
                      className="flex items-center gap-(--space-3) w-full py-(--space-2) px-(--space-3) border-none rounded-sm bg-transparent text-text-primary text-md text-left cursor-pointer hover:bg-(--hover-overlay)"
                      onClick={() => {
                        setHighlightNodeId(n.id);
                        setHighlightNeighbors(
                          neighborMap.get(n.id) ?? new Set(),
                        );
                        setSelectedNode(n);
                      }}
                    >
                      <span
                        className="w-[8px] h-[8px] rounded-full shrink-0"
                        style={{ backgroundColor: n.color || "#6b7280" }}
                        aria-hidden="true"
                      />
                      <span className="flex-1 min-w-0 overflow-hidden text-ellipsis whitespace-nowrap">
                        {n.name}
                      </span>
                      <span className="text-2xs text-text-muted shrink-0">
                        {n.entity_type}
                      </span>
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </div>
        </section>
      )}

      <div className="absolute top-(--space-4) right-(--space-4) flex items-center gap-(--space-3) z-[2]">
        <button
          type="button"
          className="inline-flex items-center gap-(--space-2) py-[3px] px-(--space-4) text-2xs font-semibold tracking-[0.4px] uppercase text-text-secondary bg-bg-secondary border border-border-color rounded-md cursor-pointer transition-colors leading-[1.3] hover:not-disabled:text-(--text-on-tint-info) hover:not-disabled:bg-(--tint-accent-info-hover) hover:not-disabled:border-(--tint-border-accent-info) disabled:opacity-40 disabled:cursor-not-allowed"
          onClick={fitView}
          disabled={!hasNodes}
          title={t("graph.fit")}
          aria-label={t("graph.fit")}
        >
          <Icon name="fit" size={14} /> {t("graph.fitShort")}
        </button>
        <button
          type="button"
          className="inline-flex items-center gap-(--space-2) py-[3px] px-(--space-4) text-2xs font-semibold tracking-[0.4px] uppercase text-text-secondary bg-bg-secondary border border-border-color rounded-md cursor-pointer transition-colors leading-[1.3] hover:not-disabled:text-(--text-on-tint-info) hover:not-disabled:bg-(--tint-accent-info-hover) hover:not-disabled:border-(--tint-border-accent-info) disabled:opacity-40 disabled:cursor-not-allowed"
          onClick={handleExportJson}
          disabled={isExporting || !hasNodes}
          title={t("graph.exportJson")}
          aria-label={t("graph.exportJson")}
        >
          <Icon name="download" size={14} /> {t("graph.exportShort")}
        </button>
      </div>

      {exportError && (
        <div
          className="absolute top-[44px] right-(--space-4) max-w-[240px] py-(--space-3) px-(--space-4) text-xs text-(--text-on-tint-danger) bg-(--tint-danger) border border-(--tint-border-danger) rounded-sm [backdrop-filter:blur(4px)] z-[2]"
          role="alert"
        >
          {t("transcript.exportFailed", { error: exportError })}
        </div>
      )}
    </div>
  );
}

export default KnowledgeGraphViewer;
