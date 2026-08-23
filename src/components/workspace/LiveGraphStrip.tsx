/**
 * `LiveGraphStrip` — the KG strip (ticket W7, synthesis audio-graph-a6b5).
 * Replaces `GraphTilePlaceholder` (W4's placeholder body) as the bento
 * graph tile's content. Three user-choosable display modes, default
 * `focus` (ratified decision 2):
 *
 *  - `focus`: up to 12 nodes from `graphFocus.ts`'s recency+hysteresis
 *    selector, rendered as a lightweight dot+label strip (NOT
 *    `ForceGraph2D` — no simulation, no canvas, no zoom/pan; a flex layout
 *    of chips is the whole renderer).
 *  - `canvas`: the existing `KnowledgeGraphViewer`, mounted lazily and
 *    reused VERBATIM (this ticket adds zero props/behavior to it beyond
 *    migrating its internal selector — see that file's W7 diff). Drives
 *    the tier-scoped `data-graph-mode="canvas"` row-swap on the workspace
 *    grid container (wired in `App.tsx`, CSS in `layout.css`).
 *  - `feed`: a compact textual change list (`graphChangeFeed.ts`) — the
 *    a11y/low-power path AND the `canvas` mode's `Suspense` fallback
 *    (design-a §2.6: "a graph that will not draw degrades to a readable
 *    list", not a spinner).
 *
 * The zero-nodes empty state (`workspace.tile.graphEmpty`, W4's fix) is
 * shared across all three modes — checked BEFORE the mode dispatch, so no
 * mode ever renders its own "nothing yet" copy.
 *
 * Mode choice is a per-user DISPLAY preference (ratified: "Mode choice
 * persists"), persisted the same way `AudioSourceSelector`'s `processScope`
 * is — plain `localStorage`-backed `useState`, not a Zustand slice, and NOT
 * cleared by `resetSessionView` (a session boundary must never reset a
 * standing display preference). `useGraphStripMode` is called ONCE, in
 * `App.tsx`'s `ShellRailContentAside` (mirroring `useLiveDocumentModel`'s
 * own lift for the identical reason: the mode value is also needed two
 * levels up, on the grid container's `data-graph-mode` attribute, so a
 * second internal copy inside this component would desync from the
 * container the moment they read different renders).
 *
 * Honesty (L3/T2 law): phase 1 has no lane-health evidence, so this file
 * never renders the word "Live" anywhere. Ticket W6 (synthesis
 * audio-graph-a6b5 §2) added the actual tone-routed freshness chip
 * (`GraphRecencyChip`, exported below, mounted in this tile's `headerSlot`
 * by `App.tsx`) and removed this component's own pre-W6 plain, untoned
 * "Graph as of HH:MM:SS" text — one source of truth for the graph lane's
 * freshness claim, not two.
 */
import { lazy, Suspense, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useSessionView } from "../../session/SessionViewProvider";
import {
  type ActiveGraphView,
  useActiveGraphSnapshot,
} from "../../session/useActiveGraphSnapshot";
import { useAudioGraphStore } from "../../store";
import type { GraphLink, GraphNode, ProjectionPatch } from "../../types";
import IconButton from "../IconButton";
import { entityTypeI18nKey, selectGraphChangeLines } from "./graphChangeFeed";
import {
  advanceGraphFocusTicks,
  EMPTY_GRAPH_FOCUS_STATE,
  type GraphFocusTickState,
  selectFocusEdges,
  selectTouchedNodeIds,
} from "./graphFocus";
import { laneRecencyChipTone, selectLaneRecency } from "./liveWorkspaceTone";

// Ticket W7: SECOND `lazy()` call site for this module — see
// `SessionsBrowser.tsx`'s updated comment on its own call site for why this
// does not reintroduce the bundling regression SHELL-R4 fixed (both call
// sites resolve to the identical chunk).
const KnowledgeGraphViewer = lazy(() => import("../KnowledgeGraphViewer"));

export type GraphStripMode = "focus" | "canvas" | "feed";

const GRAPH_STRIP_MODE_STORAGE_KEY = "ag.graphStripMode";
const GRAPH_STRIP_MODES: readonly GraphStripMode[] = [
  "focus",
  "canvas",
  "feed",
];
/** The rendered feed never grows unbounded even though the STORE array
 * (`sessionProjectionEvents`) does — design-b §4.2: "cap the rendered feed
 * (e.g. last 100 lines), do not cap the store array". */
const GRAPH_CHANGE_FEED_RENDER_CAP = 100;

function isGraphStripMode(value: string | null): value is GraphStripMode {
  return (
    value !== null && (GRAPH_STRIP_MODES as readonly string[]).includes(value)
  );
}

function loadGraphStripMode(): GraphStripMode {
  try {
    const raw = localStorage.getItem(GRAPH_STRIP_MODE_STORAGE_KEY);
    return isGraphStripMode(raw) ? raw : "focus";
  } catch {
    return "focus";
  }
}

/**
 * The persisted mode-choice hook — see the module doc for why this is
 * `localStorage`-backed `useState` (the `processScope` precedent) rather
 * than a store slice, and why it is called once, up in `App.tsx`. The lazy
 * initializer runs `loadGraphStripMode()` exactly once, on mount, matching
 * `processScope`'s own `useState(() => ...)` idiom.
 */
export function useGraphStripMode(): [
  GraphStripMode,
  (mode: GraphStripMode) => void,
] {
  const [mode, setModeState] = useState<GraphStripMode>(loadGraphStripMode);
  const setMode = (next: GraphStripMode) => {
    setModeState(next);
    try {
      localStorage.setItem(GRAPH_STRIP_MODE_STORAGE_KEY, next);
    } catch {
      // Persistence failure is non-fatal — the in-memory choice still
      // applies for the rest of this session, same posture as
      // `AudioSourceSelector`'s `processScope` setter.
    }
  };
  return [mode, setMode];
}

/** The `id` the CURRENTLY-active mode's tab `aria-controls`s and the
 * rendered tabpanel (`LiveGraphStrip`'s content region) carries — same
 * one-panel-at-a-time shape as `App.tsx`'s `workspace-panel-${view}` (only
 * the selected view's `<main>` is ever mounted), so a non-selected tab's
 * `aria-controls` pointing at an id that isn't in the DOM right now mirrors
 * that same established, accepted pattern rather than inventing a new one. */
export function graphStripPanelId(mode: GraphStripMode): string {
  return `graph-strip-panel-${mode}`;
}

/** Header-slot content — the three-way segmented mode switcher. Plain
 * buttons, not `.ag-chip`s (synthesis W7: "prefer plain buttons" sidesteps
 * the pt chip-length budget entirely for this control). Implements the full
 * WAI-ARIA APG tabs keyboard contract — roving `tabIndex` (only the
 * selected tab is a Tab stop) plus ArrowLeft/ArrowRight/Home/End moving
 * both selection and focus together — mirroring `App.tsx`'s
 * `ShellDestinationBar`/`handleWorkspaceViewKeyDown` reference
 * implementation for the Capture/Sessions tablist exactly, rather than
 * shipping `role="tablist"`/`role="tab"` without the keyboard behavior a
 * screen-reader user is told to expect ("tab, 1 of 3" implies arrow-key
 * traversal). */
export function GraphStripModeSwitcher({
  mode,
  onModeChange,
}: {
  mode: GraphStripMode;
  onModeChange: (mode: GraphStripMode) => void;
}) {
  const { t } = useTranslation();
  const handleKeyDown = (e: React.KeyboardEvent<HTMLButtonElement>) => {
    const NAV = ["ArrowLeft", "ArrowRight", "Home", "End"];
    if (!NAV.includes(e.key)) return;
    e.preventDefault();
    const currentIndex = GRAPH_STRIP_MODES.indexOf(mode);
    const nextIndex =
      e.key === "Home"
        ? 0
        : e.key === "End"
          ? GRAPH_STRIP_MODES.length - 1
          : e.key === "ArrowLeft"
            ? (currentIndex - 1 + GRAPH_STRIP_MODES.length) %
              GRAPH_STRIP_MODES.length
            : (currentIndex + 1) % GRAPH_STRIP_MODES.length;
    const next = GRAPH_STRIP_MODES[nextIndex];
    onModeChange(next);
    const tablist = e.currentTarget.parentElement;
    const tabs = tablist?.querySelectorAll<HTMLButtonElement>('[role="tab"]');
    tabs?.[nextIndex]?.focus();
  };
  return (
    <div
      role="tablist"
      aria-label={t("graphStrip.mode.label")}
      className="flex items-center gap-(--space-1)"
    >
      {GRAPH_STRIP_MODES.map((m) => (
        <button
          key={m}
          type="button"
          role="tab"
          id={`graph-strip-tab-${m}`}
          aria-selected={mode === m}
          aria-controls={graphStripPanelId(m)}
          tabIndex={mode === m ? 0 : -1}
          className={`py-[2px] px-(--space-3) text-xs font-semibold rounded-sm border cursor-pointer whitespace-nowrap ${
            mode === m
              ? "bg-bg-elevated text-accent border-accent"
              : "border-transparent bg-transparent text-text-muted hover:text-text-primary"
          }`}
          onClick={() => onModeChange(m)}
          onKeyDown={handleKeyDown}
        >
          {t(`graphStrip.mode.${m}`)}
        </button>
      ))}
    </div>
  );
}

/**
 * Incrementally advances the focus/hysteresis tick state, keyed on a
 * PATCH-scoped signal, NOT the merged view's `snapshot` object identity.
 * `useActiveGraphSnapshot`'s `useMemo` hands back a brand-new `snapshot`
 * object whenever EITHER `materializedProjectionGraph` OR the legacy
 * `graphSnapshot` changes, so gating on that reference double-counts ticks
 * in a live session where both lanes run concurrently:
 *
 *  - the legacy extraction lane's `GRAPH_DELTA`/`GRAPH_UPDATE` events touch
 *    only `graphSnapshot`, never `materializedProjectionGraph` — but they
 *    still changed `view.snapshot`'s reference, so they used to tick the
 *    hysteresis for free, with no graph patch behind them at all;
 *  - the backend pairs every accepted patch with a `MATERIALIZED_GRAPH_UPDATE`
 *    full replace (`addProjectionPatch`'s fold, then `setMaterializedProjectionGraph`'s
 *    replace), so one accepted patch used to produce TWO reference changes
 *    and thus two ticks.
 *
 * `MaterializedGraph.last_sequence` is the correct signal instead:
 * `applyProjectionGraphPatch` sets it to `patch.sequence` on the fold, and
 * the paired full replace that follows carries the SAME `last_sequence` (it
 * is the backend's canonical copy of the identical accepted patch), so the
 * fold+replace pair collapses to exactly one tick, and a legacy-lane event
 * with no accepted patch behind it does not change `last_sequence` at all
 * and so does not tick. A legacy-only session (`view.materialized === null`)
 * has no sequence number to key on and only ever has one lane active, so it
 * safely falls back to the snapshot reference — there is no double-lane or
 * double-replace inflation to guard against in that branch.
 *
 * Session-scoped: a session boundary (new live session, or loading a
 * different recorded one) resets the hysteresis state so a freshly mounted
 * graph doesn't inherit the previous session's stickiness. Mirrors
 * `LiveDocument.tsx`'s `useLiveDocumentModel` guard shape exactly, including
 * its React-18-Strict-Mode-double-invoke safety (the guard is false on a
 * repeated call with the same input, so a double-invoke re-reads instead of
 * re-advancing).
 */
function useGraphFocusIds(
  view: ActiveGraphView,
  sessionKey: string | null,
): string[] {
  const sessionKeyRef = useRef<string | null>(null);
  const advancedForRef = useRef<unknown>(null);
  const tickStateRef = useRef<GraphFocusTickState>(EMPTY_GRAPH_FOCUS_STATE);
  const idsRef = useRef<string[]>([]);

  if (sessionKey !== sessionKeyRef.current) {
    sessionKeyRef.current = sessionKey;
    tickStateRef.current = EMPTY_GRAPH_FOCUS_STATE;
    advancedForRef.current = null;
    idsRef.current = [];
  }
  const tickSignal: unknown =
    view.materialized !== null
      ? view.materialized.last_sequence
      : view.snapshot;
  if (advancedForRef.current !== tickSignal) {
    const touchedIds = selectTouchedNodeIds(
      view.materialized?.nodes ?? null,
      view.snapshot.nodes,
    );
    const { ids, state } = advanceGraphFocusTicks(
      touchedIds,
      tickStateRef.current,
    );
    tickStateRef.current = state;
    idsRef.current = ids;
    advancedForRef.current = tickSignal;
  }
  return idsRef.current;
}

function edgeEndpointId(endpoint: string | GraphNode): string {
  return typeof endpoint === "string" ? endpoint : endpoint.id;
}

/** `focus` mode's renderer: dot+label chips, entity_type-colored (the
 * closed 10-name enum `entityTypeColor` already computes into `node.color`
 * — no re-derivation here), plus a compact "A · B" list for edges whose
 * both endpoints are focused. No canvas, no simulation, no positions —
 * synthesis W7: "a simple flex/svg layout is fine." */
function GraphFocusStripView({
  nodes,
  edges,
}: {
  nodes: GraphNode[];
  edges: GraphLink[];
}) {
  const { t } = useTranslation();
  const nameById = new Map(nodes.map((node) => [node.id, node.name]));
  return (
    <div className="flex flex-col gap-(--space-3) p-(--space-4)">
      <ul
        className="flex flex-wrap gap-(--space-2) list-none p-0 m-0"
        aria-label={t("graphStrip.mode.focus")}
      >
        {nodes.map((node) => (
          <li
            key={node.id}
            className="inline-flex items-center gap-(--space-2) py-[2px] px-(--space-3) rounded-full bg-bg-elevated border border-(--edge) text-xs text-text-primary max-w-[160px]"
          >
            <span
              className="w-[8px] h-[8px] rounded-full shrink-0"
              style={{ backgroundColor: node.color || "#6b7280" }}
              aria-hidden="true"
            />
            <span className="whitespace-nowrap overflow-hidden text-ellipsis">
              {node.name}
            </span>
          </li>
        ))}
      </ul>
      {edges.length > 0 && (
        <ul className="flex flex-col gap-(--space-1) list-none p-0 m-0 text-xs text-text-muted">
          {edges.map((edge) => {
            const sourceId = edgeEndpointId(edge.source);
            const targetId = edgeEndpointId(edge.target);
            return (
              <li
                key={edge.id ?? `${sourceId}-${targetId}`}
                className="[overflow-wrap:anywhere]"
              >
                {nameById.get(sourceId) ?? sourceId}
                {" · "}
                {nameById.get(targetId) ?? targetId}
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}

/** `feed` mode's renderer AND the `canvas` mode's `Suspense` fallback.
 * Reads `sessionProjectionEvents` directly (design-b §4.2) — see
 * `graphChangeFeed.ts`'s module doc for why this needs the raw patch
 * stream rather than `graphFocus.ts`'s materialized-graph shortcut. */
export function GraphChangeFeedView({
  patches,
}: {
  patches: readonly ProjectionPatch[];
}) {
  const { t } = useTranslation();
  // `sessionProjectionEvents` (the `patches` this reads) is unbounded in the
  // store by design (design-b §4.2) and this component re-renders on every
  // change to ANY of `useSessionView`'s subscribed fields, including
  // `transcriptSegments` (several updates/sec during live capture) — so an
  // unmemoized call here would re-scan the full patch/operation history on
  // every unrelated re-render, for the life of a long session. Memoized on
  // the `patches` array reference, which only changes when a new patch is
  // actually appended.
  const lines = useMemo(() => selectGraphChangeLines(patches), [patches]);
  // Newest first; render-capped. `patches` (the store array) is untouched.
  // Slice to the cap BEFORE reversing (not reverse-then-slice) so this is
  // O(cap), not O(total history).
  const visible = useMemo(
    () => lines.slice(-GRAPH_CHANGE_FEED_RENDER_CAP).reverse(),
    [lines],
  );
  if (lines.length === 0) {
    return (
      <p className="panel-empty p-(--space-4)">{t("graphStrip.feed.empty")}</p>
    );
  }
  return (
    <ul
      className="flex flex-col gap-(--space-1) list-none p-(--space-4) m-0 text-sm text-text-secondary"
      aria-label={t("graphStrip.mode.feed")}
    >
      {visible.map((line) => (
        <li key={line.key} className="[overflow-wrap:anywhere]">
          {line.kind === "added" &&
            t("graphStrip.feed.added", {
              name: line.name,
              // Never interpolate `line.entityType` (the raw wire enum
              // value, e.g. "organization") directly — that leaks an
              // untranslated English token into every other locale. Route
              // it through the closed `graphStrip.entityType.*` namespace
              // instead (`entityTypeI18nKey` normalizes case).
              entityType: t(
                `graphStrip.entityType.${entityTypeI18nKey(line.entityType)}`,
              ),
            })}
          {line.kind === "renamed" &&
            t("graphStrip.feed.renamed", { name: line.name })}
          {line.kind === "edge" &&
            t("graphStrip.feed.edge", {
              source: line.sourceName,
              target: line.targetName,
            })}
        </li>
      ))}
    </ul>
  );
}

/**
 * The graph tile's recency chip (ticket W6, synthesis audio-graph-a6b5 §2).
 * Renders in the tile's `headerSlot` ALONGSIDE `GraphStripModeSwitcher`
 * (composed by the caller, `App.tsx`) — replaces W7's plain, untoned
 * "Graph as of HH:MM:SS" text that used to render inside this component's
 * own stats row (see the removed `graphStrip.asOf` render site below this
 * component). One source of truth for the graph lane's freshness claim.
 *
 * Shares `selectLaneRecency`/`laneRecencyChipTone` with `DocRecencyChip`
 * (`LiveDocument.tsx`) — `kind: "graph"` is this call site's only
 * difference from that one (synthesis §2: "one function, two call sites").
 *
 * The visible label ALSO reuses `document.recency.asOf`/`document.recency.behind`
 * directly (design-a §7's i18n budget table specs this chip as reusing the
 * document chip's recency strings) — the two are byte-identical per locale
 * ("as of {{time}}" / "−{{count}} turns" in en; same in pt), so keeping a
 * separate `graphStrip.recency.asOf`/`behind` pair would just be two copies
 * of the same string that could drift apart. Only the `*Aria` variants stay
 * lane-specific (`graphStrip.recency.asOfAria`/`behindAria`) — a screen
 * reader needs to hear "Graph", not "Notes", is behind.
 */
export function GraphRecencyChip() {
  const { t, i18n } = useTranslation();
  const { sessionProjectionEvents } = useSessionView();
  const asrSpanRevisions = useAudioGraphStore((s) => s.asrSpanRevisions);
  const loadedSessionId = useAudioGraphStore((s) => s.loadedSessionId);

  // Memoized: see `DocRecencyChip`'s (LiveDocument.tsx) identical comment —
  // `useSessionView()`'s subscription (incl. `transcriptSegments`) re-renders
  // this component far more often than `sessionProjectionEvents`/
  // `asrSpanRevisions` actually change.
  const { lastAppliedAtMs, turnsBehind } = useMemo(
    () => selectLaneRecency("graph", sessionProjectionEvents, asrSpanRevisions),
    [sessionProjectionEvents, asrSpanRevisions],
  );
  const recency = laneRecencyChipTone({
    lastAppliedAtMs,
    turnsBehind,
    // Always `null` — no real call site can supply W3's evidence until
    // that ticket lands. See liveWorkspaceTone.ts's module doc.
    evidence: null,
    isLiveSession: loadedSessionId === null,
  });

  if (!recency.render || recency.lastAppliedAtMs === null) return null;

  // See `DocRecencyChip`'s identical comment: explicit locale + `hour12:
  // false` keeps the rendered time language-consistent AND fixed-width,
  // rather than resolving to the OS/runtime locale's default formatting.
  const time = new Date(recency.lastAppliedAtMs).toLocaleTimeString(
    i18n.language,
    { hour12: false },
  );
  const label = recency.behind
    ? t("document.recency.behind", { count: recency.turnsBehind })
    : t("document.recency.asOf", { time });
  const ariaLabel = recency.behind
    ? t("graphStrip.recency.behindAria", { count: recency.turnsBehind })
    : t("graphStrip.recency.asOfAria", { time });

  return (
    // See `DocRecencyChip`'s (LiveDocument.tsx) identical comment: `aria-label`
    // on a bare `<span>` is invalid ARIA (generic role excludes naming) —
    // this mirrors `PipelineStatusBar.tsx`'s aria-hidden-visible +
    // `.sr-only`-explanation idiom instead.
    <span className="ag-chip" data-tone={recency.tone}>
      <span aria-hidden="true">{label}</span>
      <span className="sr-only">{ariaLabel}</span>
    </span>
  );
}

/** The tile body. `mode`/`onModeChange` are lifted (see module doc) so the
 * grid container's `data-graph-mode` attribute and this component's own
 * rendering never read two different copies of the same choice. */
export function LiveGraphStrip({
  mode,
  onModeChange,
}: {
  mode: GraphStripMode;
  onModeChange: (mode: GraphStripMode) => void;
}) {
  const { t } = useTranslation();
  const view = useActiveGraphSnapshot();
  const { sessionProjectionEvents } = useSessionView();
  const loadedSessionId = useAudioGraphStore((s) => s.loadedSessionId);
  // Session boundary for the focus/hysteresis state (graphFocus.ts): a real
  // materialized graph carries its own `session_id`; a legacy-only session
  // (materialized graph absent entirely) has nothing on the snapshot type
  // to key on, so `loadedSessionId` is the fallback — it changes at every
  // review-session switch, and stays `null` throughout a live session
  // (`resetSessionView` sets it back to `null` on every session boundary,
  // live or reviewing — see that reducer in `store/index.ts`).
  //
  // `store/index.ts`'s `applyProjectionGraphPatch` folds the FIRST accepted
  // patch of a session onto a freshly-allocated graph carrying a hardcoded
  // `session_id: "live"` placeholder (it has no real id to put there yet);
  // the paired `MATERIALIZED_GRAPH_UPDATE` full replace that follows
  // immediately after carries the backend's real session id. Treating that
  // placeholder as "no id yet" (falling through to `loadedSessionId`, same
  // as the materialized-absent case) means `sessionKey` only ever changes
  // ONCE per session — when the real id first becomes known — instead of
  // flipping "live" -> real-id right after also flipping null -> "live",
  // which used to spuriously reset the hysteresis state twice in the first
  // couple of ticks of every live session.
  const materializedSessionId =
    view.materialized && view.materialized.session_id !== "live"
      ? view.materialized.session_id
      : null;
  const sessionKey = materializedSessionId ?? loadedSessionId ?? null;
  const focusIds = useGraphFocusIds(view, sessionKey);

  if (view.snapshot.nodes.length === 0) {
    return <p className="panel-empty">{t("workspace.tile.graphEmpty")}</p>;
  }

  const nodeById = new Map(view.snapshot.nodes.map((node) => [node.id, node]));
  const focusNodes = focusIds
    .map((id) => nodeById.get(id))
    .filter((node): node is GraphNode => node !== undefined);
  const focusEdges = selectFocusEdges(focusIds, view.snapshot.links);

  return (
    <div className="flex flex-col h-full min-h-0">
      <div className="flex items-center gap-(--space-3) py-(--space-2) px-(--space-4) text-xs text-text-muted shrink-0 border-b border-(--edge)">
        {mode === "canvas" ? (
          <span aria-hidden="true">
            {t("graph.stats.nodes", { count: view.snapshot.nodes.length })}
          </span>
        ) : (
          <button
            type="button"
            className="bg-transparent border-none p-0 text-text-muted cursor-pointer underline-offset-2 hover:underline"
            onClick={() => onModeChange("canvas")}
          >
            {t("graph.stats.nodes", { count: view.snapshot.nodes.length })}
          </button>
        )}
        {/* `aria-hidden` in canvas mode mirrors the node-count span above:
            `KnowledgeGraphViewer`'s own `role="status" aria-live="polite"`
            stats chip (KnowledgeGraphViewer.tsx) already announces
            "Knowledge graph: N nodes, M edges" — leaving this span
            announced too would double-announce the edge count on every
            screen-reader pass through the header row, the same
            duplication the node span was already guarded against. */}
        <span aria-hidden={mode === "canvas" ? "true" : undefined}>
          {t("graph.stats.edges", { count: view.snapshot.links.length })}
        </span>
        {mode !== "canvas" && (
          <IconButton
            icon="fit"
            label={t("graphStrip.expand")}
            size={14}
            variant="ghost"
            className="ml-auto"
            onClick={() => onModeChange("canvas")}
          />
        )}
      </div>
      <div
        className="flex-1 min-h-0 overflow-auto"
        role="tabpanel"
        id={graphStripPanelId(mode)}
        aria-labelledby={`graph-strip-tab-${mode}`}
      >
        {mode === "focus" && (
          <GraphFocusStripView nodes={focusNodes} edges={focusEdges} />
        )}
        {mode === "canvas" && (
          <Suspense
            fallback={<GraphChangeFeedView patches={sessionProjectionEvents} />}
          >
            <KnowledgeGraphViewer />
          </Suspense>
        )}
        {mode === "feed" && (
          <GraphChangeFeedView patches={sessionProjectionEvents} />
        )}
      </div>
    </div>
  );
}

export default LiveGraphStrip;
