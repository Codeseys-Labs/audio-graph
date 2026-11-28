/**
 * Sessions destination — list→detail (SHELL-R2, seed audio-graph-e0c4,
 * plan §R2, ADR-0046). Rendered unconditionally inside
 * `#workspace-panel-sessions` (renamed from the legacy "Review" tab's
 * `#workspace-panel-after` by SHELL-R4). No longer a modal: the Sessions
 * browser IS this destination's content now, not an overlay toggled by
 * `sessionsBrowserOpen` (that flag/action still exist per SHELL-R1 —
 * `openSessionsBrowser` now navigates here instead of opening anything).
 *
 * Composition: a rail (search + sort + trash filter + one row per
 * `SessionMetadata`) on the left, a detail pane on the right. Selecting a
 * row sets `nav.sessionId` (`useAudioGraphStore`'s `setNavSessionId`) and, for
 * any row that ISN'T the session that just finished recording, calls
 * `loadSession` when no capture is running. Every lens of a reviewed session
 * (Transcript, Notes, Timeline, Graph) is a query-backed Review pane keyed by
 * the row's id (`session/reviewStore.ts`), so it never reads or writes the
 * global store, with capture running or not; only Ask and "Generate prose
 * summary", which need the backend's current buffers, stay behind
 * `globalHoldsRow` in `SessionDetail`. The just-stopped session's row, whose
 * data is resident in the global store, is a special case: see
 * `handleSelect` and `residentRow`.
 *
 * Source of truth is still the backend `sessions.json` index —
 * `list_sessions` returns all known sessions, `load_session` hydrates the
 * detail lenses, `restore_session` untrashes a soft-deleted session,
 * `delete_session` soft-deletes (marks for expiry),
 * `delete_session_permanently` hard-deletes, and `purge_expired_sessions`
 * cleans up old soft-deletes — all unchanged from the v2 modal.
 *
 * Sort mode (`newest | oldest | nameAsc | nameDesc | largest`) is still
 * persisted to `localStorage` under `audiograph:sessionsBrowser:sort`.
 *
 * Detail lenses (Notes default / Transcript / Timeline / Graph / Route) are
 * still deliberately NOT stored on `nav.lens`, even after SHELL-R4 deleted
 * the `analysis` tab and retired the `nav.lens === "graph"` legacy-tab
 * disambiguation this doc used to warn about (`store/shellNav.ts`'s "R4
 * UPDATE" note): unifying the two remains a real behavioral change (wiring
 * this component's lens tabs to read/write the global `nav.lens`, auditing
 * what happens when a stale lens value meets no active session) rather than
 * a rename, so it's left as a follow-up rather than folded into R4's
 * near-pure deletion+rename. The `graphEdgeFocus` bridge (`App.tsx`) does set
 * `nav.lens = "graph"` on a "→N" focus request, but this component does not
 * yet read that field — lens selection here stays local component state,
 * reset per session via `key={selectedId}` on `<SessionDetail>`.
 */
import {
  createContext,
  lazy,
  type ReactNode,
  Suspense,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import { useReviewStore } from "../session/reviewStore";
import { SessionViewProvider } from "../session/SessionViewProvider";
import { useAudioGraphStore } from "../store";
import type { PendingFinalizingSession, SessionMetadata } from "../types";
import { downloadAsFile, filenameTimestamp } from "../utils/download";
import { formatDurationHoursAware, formatRelativeTime } from "../utils/format";
import ChatSidebar from "./ChatSidebar";
import Icon, { type IconName } from "./Icon";
import IconButton from "./IconButton";
import LiveTranscript from "./LiveTranscript";
import NotesPanel from "./NotesPanel";
import Popover, { PopoverItem } from "./Popover";
import ReviewLensGate from "./ReviewLensGate";
import SeekTimeline from "./SeekTimeline";
import SessionDataRoutePanel from "./SessionDataRoutePanel";
import {
  type NotesSynthesisController,
  useNotesSynthesis,
} from "./useNotesSynthesis";

// Same code-split rationale as the old App.tsx-owned import (ADR-0016 /
// modernization-audit 2.3): react-force-graph-2d stays a deferred chunk.
// SHELL-R4 deleted App.tsx's own duplicate `lazy()` call along with the
// "analysis" tab it belonged to, making this the ONLY `lazy()` call for this
// module for a time. Ticket W7 (synthesis audio-graph-a6b5) reintroduced a
// SECOND call site — `LiveGraphStrip.tsx`'s `canvas` mode mounts this exact
// component lazily too — but this is NOT a regression of SHELL-R4's fix:
// Rollup/Vite resolve a dynamic `import()` by module specifier, so both
// `lazy()` wrappers share the identical underlying chunk file regardless of
// how many call sites reference it. Re-verified via `bun run build`: the
// force-graph vendor chunk still stays its own deferred chunk (not inlined
// into either entry's eager bundle) with the two call sites present.
const KnowledgeGraphViewer = lazy(() => import("./KnowledgeGraphViewer"));

/** Sort modes. Values double as i18n keys under `sessions.sort.*`. */
export type SessionSortMode =
  | "newest"
  | "oldest"
  | "nameAsc"
  | "nameDesc"
  | "largest";

const SORT_MODES: SessionSortMode[] = [
  "newest",
  "oldest",
  "nameAsc",
  "nameDesc",
  "largest",
];

/** localStorage key for the sort preference. */
const SORT_STORAGE_KEY = "audiograph:sessionsBrowser:sort";

/** Id of the visible "only completed sessions" rule in the rail, referenced by
 * disabled rows via `aria-describedby` (a `title` on a native disabled button
 * is unreachable by keyboard). */
const LIVE_REVIEW_RULE_ID = "sessions-live-review-rule";

function loadSortPreference(): SessionSortMode {
  try {
    const raw = localStorage.getItem(SORT_STORAGE_KEY);
    if (raw && (SORT_MODES as string[]).includes(raw)) {
      return raw as SessionSortMode;
    }
  } catch {
    // localStorage unavailable (SSR, permission-denied, etc.) — fall back.
  }
  return "newest";
}

function saveSortPreference(mode: SessionSortMode): void {
  try {
    localStorage.setItem(SORT_STORAGE_KEY, mode);
  } catch {
    // Non-fatal — preference just won't persist across restarts.
  }
}

/** Display name for a session — falls back to the short id. */
function displayName(s: SessionMetadata): string {
  return s.title ?? s.id.slice(0, 8);
}

/** Filter+sort pipeline. Unchanged signature/behavior from the v2 modal —
 * exported for unit tests, and reused verbatim over the merged row set. */
export function applyFilterAndSort(
  sessions: SessionMetadata[],
  search: string,
  sortMode: SessionSortMode,
  showTrash: boolean,
): SessionMetadata[] {
  const needle = search.trim().toLowerCase();
  const filtered = sessions.filter((s) => {
    const isTrash = s.deleted === true;
    if (showTrash !== isTrash) return false;
    if (!needle) return true;
    const name = displayName(s).toLowerCase();
    return name.includes(needle) || s.id.toLowerCase().includes(needle);
  });

  const sorted = [...filtered];
  switch (sortMode) {
    case "newest":
      sorted.sort((a, b) => b.created_at - a.created_at);
      break;
    case "oldest":
      sorted.sort((a, b) => a.created_at - b.created_at);
      break;
    case "nameAsc":
      sorted.sort((a, b) =>
        displayName(a).localeCompare(displayName(b), undefined, {
          sensitivity: "base",
        }),
      );
      break;
    case "nameDesc":
      sorted.sort((a, b) =>
        displayName(b).localeCompare(displayName(a), undefined, {
          sensitivity: "base",
        }),
      );
      break;
    case "largest":
      sorted.sort((a, b) => b.segment_count - a.segment_count);
      break;
  }
  return sorted;
}

/** A rail row: a real `SessionMetadata` or the optimistic finalizing one. */
export type SessionRow = SessionMetadata & { optimistic?: true };

/**
 * Merge the optimistic "finalizing" row (the 1d92 gap — `stopCapture` writes
 * it before `sessions.json` is confirmed to contain the just-ended session)
 * into the real list. Self-clearing: once a real `listSessions()` result
 * contains a row with the pending id, this stops injecting it — no explicit
 * "clear" action needed.
 */
export function mergeSessionRows(
  sessions: SessionMetadata[],
  pending: PendingFinalizingSession | null,
): SessionRow[] {
  if (!pending) return sessions;
  if (sessions.some((s) => s.id === pending.id)) return sessions;
  return [pending, ...sessions];
}

/** Closed tone map for the state chip (`.ag-chip[data-tone]`, T2/T3) —
 * extensible to `finalizing`/`blocked` per ADR-0035/0036 as those land. */
function sessionChipTone(
  row: SessionRow,
): "info" | "success" | "danger" | "warning" {
  if (row.optimistic) return "warning";
  switch (row.status) {
    case "complete":
      return "success";
    case "crashed":
      return "danger";
    case "active":
      return "info";
    default:
      return "info";
  }
}

function sessionStatusLabel(
  row: SessionRow,
  t: (key: string) => string,
): string {
  return row.optimistic
    ? t("sessions.status.finalizing")
    : t(`sessions.status.${row.status}`);
}

const DETAIL_LENSES = [
  "notes",
  "transcript",
  "timeline",
  "graph",
  "route",
] as const;
type DetailLens = (typeof DETAIL_LENSES)[number];

const LENS_ICON: Record<DetailLens, IconName> = {
  notes: "notes",
  transcript: "transcript",
  timeline: "timeline",
  graph: "graph",
  route: "route",
};

/** Plan §R2 (ADR-0046) aside: ChatSidebar is available as an "Ask" aside only on
 * the Notes/Graph lenses. */
function askAvailable(lens: DetailLens): boolean {
  return lens === "notes" || lens === "graph";
}

/**
 * While capture or transcription runs, only a completed, indexed row can be
 * opened: it is reviewed through the query-backed Review store
 * (`session/reviewStore.ts`), never hydrated into the Live global fields. The
 * live session's own row (`active`) and the optimistic "finalizing" row (its
 * data is still resident in the Live store) stay locked.
 */
function reviewableWhileLive(row: SessionRow): boolean {
  return row.status === "complete" && row.optimistic !== true;
}

function SessionsBrowser() {
  const { t, i18n } = useTranslation();
  const sessions = useAudioGraphStore((s) => s.sessions);
  const sessionsLoading = useAudioGraphStore((s) => s.sessionsLoading);
  const isCapturing = useAudioGraphStore((s) => s.isCapturing);
  const isTranscribing = useAudioGraphStore((s) => s.isTranscribing);
  const samplePreviewActive = useAudioGraphStore((s) => s.samplePreviewActive);
  const pendingFinalizingSession = useAudioGraphStore(
    (s) => s.pendingFinalizingSession,
  );
  const listSessions = useAudioGraphStore((s) => s.listSessions);
  const deleteSession = useAudioGraphStore((s) => s.deleteSession);
  const restoreSession = useAudioGraphStore((s) => s.restoreSession);
  const deleteSessionPermanently = useAudioGraphStore(
    (s) => s.deleteSessionPermanently,
  );
  const recoverOrphanedSessions = useAudioGraphStore(
    (s) => s.recoverOrphanedSessions,
  );
  const exportSessionBundle = useAudioGraphStore((s) => s.exportSessionBundle);
  const navSessionId = useAudioGraphStore((s) => s.nav.sessionId);
  const setNavSessionId = useAudioGraphStore((s) => s.setNavSessionId);
  const loadSession = useAudioGraphStore((s) => s.loadSession);

  const [search, setSearch] = useState("");
  const [sortMode, setSortMode] = useState<SessionSortMode>(() =>
    loadSortPreference(),
  );
  const [showTrash, setShowTrash] = useState(false);
  const [recoverySummary, setRecoverySummary] = useState<string | null>(null);
  const [exportingIds, setExportingIds] = useState<Set<string>>(
    () => new Set(),
  );
  const liveRunning = isCapturing || isTranscribing;

  // Refresh on mount — match the v2 store's own larger fetch (200) so the
  // rail's search can actually find old entries, not just the 10 most recent
  // the v1 overlay loaded. Also the fallback net for the 1d92 race: if
  // `stopCapture`'s own refresh raced the index write, this mount-time fetch
  // (re-run every time the "after" tab is (re)entered, since the panel only
  // mounts SessionsBrowser while active) tries again.
  useEffect(() => {
    void listSessions(200);
  }, [listSessions]);

  const rows = useMemo(
    () => mergeSessionRows(sessions, pendingFinalizingSession),
    [sessions, pendingFinalizingSession],
  );

  const trashCount = useMemo(
    () => rows.filter((s) => s.deleted === true).length,
    [rows],
  );

  const visible = useMemo(
    () => applyFilterAndSort(rows, search, sortMode, showTrash),
    [rows, search, sortMode, showTrash],
  );

  const selectedId = samplePreviewActive ? null : navSessionId;
  const selectedRow = useMemo(
    () => rows.find((r) => r.id === selectedId) ?? null,
    [rows, selectedId],
  );

  const handleSortChange = (mode: SessionSortMode) => {
    setSortMode(mode);
    saveSortPreference(mode);
  };

  // Choosing a row is an explicit user action, so (as on master) it is what
  // loads the global store, and only when no capture or transcription is
  // running: while capture runs NOTHING is written to the global store, the
  // Transcript lens is a Review pane that queries `load_session` itself keyed
  // by the row's id, whatever the capture flags say. The optimistic
  // just-stopped row is skipped: its data is already resident (it WAS the live
  // session) and re-reading it could race the finalize-on-disk gap
  // `pendingFinalizingSession` exists to route around. `row.optimistic` (not
  // the write-only `pendingFinalizingSession` slot) is the key because it
  // self-clears with `mergeSessionRows`.
  const handleSelect = (row: SessionRow) => {
    setNavSessionId(row.id);
    if (row.optimistic === true) return;
    if (liveRunning) return;
    void loadSession(row.id);
  };

  const handleDelete = async (sessionId: string) => {
    const ok = window.confirm(t("sessions.deleteConfirm"));
    if (!ok) return;
    await deleteSession(sessionId);
  };

  const handleRestore = async (sessionId: string) => {
    await restoreSession(sessionId);
  };

  const handleDeletePermanently = async (sessionId: string) => {
    const ok = window.confirm(t("sessions.deletePermanentlyConfirm"));
    if (!ok) return;
    await deleteSessionPermanently(sessionId);
  };

  const handleRecover = async () => {
    const report = await recoverOrphanedSessions();
    if (!report) return;
    setRecoverySummary(
      t("sessions.recoverySummary", {
        recovered: report.recovered,
        skipped: report.skipped,
        errors: report.errors.length,
      }),
    );
  };

  const handleExport = async (sessionId: string) => {
    setExportingIds((prev) => new Set(prev).add(sessionId));
    try {
      const bundle = await exportSessionBundle(sessionId);
      if (!bundle) return;
      const filename = `session-${sessionId}-${filenameTimestamp()}.json`;
      downloadAsFile(
        JSON.stringify(bundle, null, 2),
        filename,
        "application/json",
      );
    } finally {
      setExportingIds((prev) => {
        const next = new Set(prev);
        next.delete(sessionId);
        return next;
      });
    }
  };

  return (
    <div className="flex flex-1 min-w-0 min-h-0 overflow-hidden">
      <nav
        aria-label={t("sessions.rail")}
        className="flex w-[300px] min-w-[240px] shrink-0 flex-col overflow-hidden border-r border-(--edge)"
      >
        <div className="ag-panel-head">
          <h2 className="ag-panel-head__title">{t("sessions.title")}</h2>
        </div>
        <div className="flex flex-col gap-(--space-3) p-(--space-4) shrink-0">
          <input
            type="search"
            className="ag-field__control"
            aria-label={t("sessions.searchLabel")}
            placeholder={t("sessions.searchPlaceholder")}
            value={search}
            onChange={(e) => setSearch(e.target.value)}
          />
          <div className="flex items-center gap-(--space-3)">
            <label className="flex flex-1 items-center gap-(--space-2) text-xs">
              <span className="ag-label whitespace-nowrap">
                {t("sessions.sortLabel")}
              </span>
              <select
                aria-label={t("sessions.sortLabel")}
                value={sortMode}
                onChange={(e) =>
                  handleSortChange(e.target.value as SessionSortMode)
                }
                className="ag-field__control flex-1"
              >
                {SORT_MODES.map((m) => (
                  <option key={m} value={m}>
                    {t(`sessions.sort.${m}`)}
                  </option>
                ))}
              </select>
            </label>
          </div>
          <div className="flex items-center gap-(--space-3)">
            <button
              type="button"
              className="ag-btn-micro"
              onClick={handleRecover}
              title={t("sessions.recoverTitle")}
            >
              <Icon name="refresh" size={14} /> {t("sessions.recover")}
            </button>
            <button
              type="button"
              className="ag-btn-micro"
              aria-pressed={showTrash}
              onClick={() => setShowTrash((v) => !v)}
              title={
                showTrash ? t("sessions.hideTrash") : t("sessions.showTrash")
              }
            >
              <Icon name="trash" size={14} />{" "}
              {showTrash
                ? t("sessions.hideTrash")
                : t("sessions.trashCount", { count: trashCount })}
            </button>
          </div>
          {recoverySummary && (
            <p className="m-0 text-xs text-text-muted" role="status">
              {recoverySummary}
            </p>
          )}
          {liveRunning && (
            <p
              id={LIVE_REVIEW_RULE_ID}
              className="m-0 text-xs text-text-muted"
              data-testid="sessions-live-review-rule"
            >
              {t("sessions.reviewNeedsCompletedWhileLive")}
            </p>
          )}
        </div>

        <div className="flex-1 min-h-0 overflow-y-auto px-(--space-4) pb-(--space-4)">
          {sessionsLoading && rows.length === 0 ? (
            <p
              className="m-0 py-(--space-5) text-sm text-text-muted"
              role="status"
            >
              {t("common.loading")}
            </p>
          ) : rows.length === 0 ? (
            <p
              className="m-0 py-(--space-5) text-sm text-text-muted"
              role="status"
            >
              {t("sessions.noSessions")}
            </p>
          ) : visible.length === 0 ? (
            <p
              className="m-0 py-(--space-5) text-sm text-text-muted"
              role="status"
            >
              {t("sessions.noMatches")}
            </p>
          ) : (
            <ul className="list-none p-0 m-0 flex flex-col gap-(--space-3)">
              {visible.map((row) => (
                <SessionRowItem
                  key={row.id}
                  row={row}
                  selected={row.id === selectedId}
                  exporting={exportingIds.has(row.id)}
                  reviewLocked={liveRunning}
                  selectLocked={liveRunning && !reviewableWhileLive(row)}
                  locale={i18n.resolvedLanguage ?? i18n.language}
                  onSelect={() => handleSelect(row)}
                  onExport={() => handleExport(row.id)}
                  onDelete={() => handleDelete(row.id)}
                  onRestore={() => handleRestore(row.id)}
                  onDeletePermanently={() => handleDeletePermanently(row.id)}
                />
              ))}
            </ul>
          )}
        </div>
      </nav>

      <SessionDetail
        key={samplePreviewActive ? "sample" : (selectedId ?? "none")}
        row={selectedRow}
        samplePreviewActive={samplePreviewActive}
        liveRunning={liveRunning}
      />
    </div>
  );
}

interface SessionRowItemProps {
  row: SessionRow;
  selected: boolean;
  exporting: boolean;
  reviewLocked: boolean;
  /** Row cannot be opened right now (live run + not a completed row). */
  selectLocked: boolean;
  locale: string;
  onSelect: () => void;
  onExport: () => void;
  onDelete: () => void;
  onRestore: () => void;
  onDeletePermanently: () => void;
}

function SessionRowItem({
  row,
  selected,
  exporting,
  reviewLocked,
  selectLocked,
  locale,
  onSelect,
  onExport,
  onDelete,
  onRestore,
  onDeletePermanently,
}: SessionRowItemProps) {
  const { t } = useTranslation();
  const title = displayName(row);
  return (
    <li
      className={`ag-card border-(--edge) flex items-start gap-(--space-3) p-(--space-4)${row.deleted ? " opacity-70" : ""}${selected ? " border-accent" : ""}`}
      data-testid={`session-${row.id}`}
      data-trashed={row.deleted ? "true" : "false"}
      data-selected={selected ? "true" : "false"}
    >
      <button
        type="button"
        className="flex-1 min-w-0 flex flex-col gap-(--space-2) text-left bg-transparent border-none p-0 cursor-pointer text-text-primary"
        data-testid={`session-select-${row.id}`}
        // Explicit accessible name (rather than the default concatenation of
        // every visible descendant's text — chip/timestamp/duration/counts)
        // so activating a row announces "Session Name", not a long run-on of
        // stats; the metadata stays visible for sighted users regardless.
        aria-label={title}
        onClick={onSelect}
        aria-current={selected ? "true" : undefined}
        disabled={selectLocked}
        aria-describedby={selectLocked ? LIVE_REVIEW_RULE_ID : undefined}
      >
        <div className="flex items-baseline justify-between gap-(--space-3)">
          <strong
            className="text-sm overflow-hidden text-ellipsis whitespace-nowrap"
            title={row.id}
          >
            {title}
          </strong>
          <span className="ag-chip" data-tone={sessionChipTone(row)}>
            {sessionStatusLabel(row, t)}
          </span>
        </div>
        <div className="flex flex-wrap gap-(--space-4) text-xs text-text-muted tabular-nums">
          <span>
            {row.deleted && row.deleted_at
              ? t("sessions.trashedOn", {
                  date: new Date(row.deleted_at).toLocaleString(),
                })
              : formatRelativeTime(row.created_at, Date.now(), locale)}
          </span>
          <span>{formatDurationHoursAware(row.duration_seconds)}</span>
          <span>
            {t("sessions.stats.segments")}: {row.segment_count}
          </span>
          <span>
            {t("sessions.stats.speakers")}: {row.speaker_count}
          </span>
          <span>
            {t("sessions.stats.entities")}: {row.entity_count}
          </span>
        </div>
      </button>

      <Popover
        trigger={
          <IconButton
            icon="more"
            label={t("sessions.rowActionsLabel", { title })}
            variant="ghost"
          />
        }
      >
        {row.deleted ? (
          <>
            {/* Restore/permanent-delete are destructive/state-mutating on a
             * PAST session's trash entry — disabled while `reviewLocked`
             * (matching the detail pane's own live-lock posture) even
             * though export stays available below. Trash-vs-restore state
             * flipping underneath the rail while nobody can even open the
             * detail pane to see the result felt like the wrong default. */}
            <PopoverItem
              onClick={onRestore}
              disabled={reviewLocked}
              title={
                reviewLocked ? t("sessions.reviewLockedWhileLive") : undefined
              }
            >
              {t("sessions.restore")}
            </PopoverItem>
            <PopoverItem
              danger
              onClick={onDeletePermanently}
              disabled={reviewLocked}
              title={
                reviewLocked ? t("sessions.reviewLockedWhileLive") : undefined
              }
            >
              {t("sessions.deletePermanently")}
            </PopoverItem>
          </>
        ) : (
          <>
            {/* Export is read-only against a past session's already-persisted
             * files — it doesn't touch anything the live capture is writing
             * to, so it stays available while `reviewLocked` (unlike the
             * destructive actions below). */}
            <PopoverItem
              onClick={onExport}
              disabled={exporting || row.optimistic}
            >
              {exporting ? t("sessions.exporting") : t("sessions.export")}
            </PopoverItem>
            <PopoverItem
              danger
              onClick={onDelete}
              disabled={
                row.status === "active" || row.optimistic || reviewLocked
              }
              title={
                row.status === "active"
                  ? t("sessions.activeDeleteLocked")
                  : reviewLocked
                    ? t("sessions.reviewLockedWhileLive")
                    : undefined
              }
            >
              {t("sessions.delete")}
            </PopoverItem>
          </>
        )}
      </Popover>
    </li>
  );
}

/** Localized notice for a pane that cannot be used right now. */
function ReviewLockedNotice({ message }: { message: string }) {
  return (
    <div className="flex flex-1 min-w-0 flex-col items-center justify-center gap-(--space-3) p-(--space-6) text-center">
      <Icon name="warning" size={22} className="text-text-muted opacity-40" />
      <p
        className="m-0 max-w-[360px] text-sm text-text-secondary"
        role="status"
      >
        {message}
      </p>
    </div>
  );
}

/**
 * "Generate prose summary" reads the BACKEND's current buffers, so it is
 * offered only for the session whose data is resident in the global store. Its
 * controller (`useNotesSynthesis`, which follows the Live transcript and graph)
 * therefore lives in this scope, mounted ONLY while the action is offered: a
 * reviewed session never pays for, or re-renders on, Live updates through it.
 */
const SynthesisContext = createContext<NotesSynthesisController | null>(null);

function ResidentSynthesisScope({ children }: { children: ReactNode }) {
  const synthesis = useNotesSynthesis();
  return (
    <SynthesisContext.Provider value={synthesis}>
      {children}
    </SynthesisContext.Provider>
  );
}

function SummaryMenu() {
  const { t } = useTranslation();
  const synthesis = useContext(SynthesisContext);
  if (synthesis === null) return null;
  return (
    <Popover
      trigger={
        <IconButton
          icon="more"
          label={t("sessions.detailActionsLabel")}
          variant="ghost"
        />
      }
    >
      <PopoverItem
        onClick={() => void synthesis.handleSynthesize()}
        disabled={synthesis.loading}
      >
        {synthesis.loading
          ? t("notes.synthesizing")
          : t("sessions.generateProseSummary")}
      </PopoverItem>
    </Popover>
  );
}

/** The resident session's Notes lens: shares the summary's controller. */
function ResidentNotesLens() {
  const synthesis = useContext(SynthesisContext);
  return (
    <NotesPanel synthesis={synthesis ?? undefined} headerActions={false} />
  );
}

/** The lens body, shared by the Review and resident (global store) paths. */
function LensBody({
  lens,
  reviewed,
}: {
  lens: "notes" | "transcript" | "timeline" | "graph";
  /** True under a Review provider: Notes, Timeline and Graph sit in a gate. */
  reviewed: boolean;
}) {
  const graph = (
    <Suspense fallback={null}>
      <KnowledgeGraphViewer />
    </Suspense>
  );
  switch (lens) {
    case "transcript":
      return <LiveTranscript />;
    case "notes":
      return reviewed ? (
        <ReviewLensGate kind="notes">
          <NotesPanel headerActions={false} />
        </ReviewLensGate>
      ) : (
        <ResidentNotesLens />
      );
    case "timeline":
      return reviewed ? (
        <ReviewLensGate kind="timeline">
          <SeekTimeline />
        </ReviewLensGate>
      ) : (
        <SeekTimeline />
      );
    case "graph":
      return reviewed ? (
        <ReviewLensGate kind="graph">{graph}</ReviewLensGate>
      ) : (
        graph
      );
  }
}

interface SessionDetailProps {
  row: SessionRow | null;
  samplePreviewActive: boolean;
  liveRunning: boolean;
}

function SessionDetail({
  row,
  samplePreviewActive,
  liveRunning,
}: SessionDetailProps) {
  const { t } = useTranslation();
  const loadedSessionId = useAudioGraphStore((s) => s.loadedSessionId);
  // While capture runs the Transcript lens is the cheapest to land on (Notes,
  // Timeline and Graph each fetch a heavy artifact when they activate).
  const [lens, setLens] = useState<DetailLens>(
    liveRunning ? "transcript" : "notes",
  );
  const [askOpen, setAskOpen] = useState(false);

  // What reads the BACKEND's current buffers or the global store, and so needs
  // the global store to hold THIS row's data: Ask and "Generate prose summary"
  // (the lenses no longer do, see `reviewLensId` below). ONE predicate, store
  // truth: nothing is capturing or transcribing, and the global store holds
  // this session's data, two ways:
  //   - `loadedSessionId === row.id` (a session loaded by explicit action), or
  //   - `loadedSessionId === null && pendingFinalizingSession?.id === row.id`:
  //     the just-stopped session's resident live data. `stopCapture` writes
  //     the slot, and `startCapture`, `loadSession` (of any other session),
  //     `resetSessionView`, `clearTranscript`, `loadSessionTranscript` and
  //     sample-preview enter/exit clear it, so it names the session whose data
  //     is resident (see the invariant at the slot's declaration). It covers
  //     the row both while optimistic and after it is indexed, and is false
  //     after an unresolved Stop (null slot).
  // Sample preview holds the store by construction. When false, those two
  // actions are simply unavailable; nothing here loads the global store on the
  // user's behalf.
  const pendingFinalizingSession = useAudioGraphStore(
    (s) => s.pendingFinalizingSession,
  );
  const residentRow =
    !liveRunning &&
    row !== null &&
    (row.optimistic === true ||
      (loadedSessionId === null && pendingFinalizingSession?.id === row.id));
  const globalHoldsRow =
    samplePreviewActive ||
    (!liveRunning &&
      row !== null &&
      (loadedSessionId === row.id ||
        (loadedSessionId === null && pendingFinalizingSession?.id === row.id)));
  const synthesisOffered = globalHoldsRow && !loadedSessionId;

  // Concurrent Live + Review (ADR-0028): every lens (Transcript, Notes,
  // Timeline, Graph) is a Review pane keyed by the row's own id, whatever the
  // capture flags say, so Start/Stop never swaps what a reviewed session shows
  // or exports and nothing reads the global store for it. Two cases hold their
  // data in the global store by construction and keep reading it: sample
  // preview, and the just-stopped session's resident data (`residentRow`).
  // While capture runs a row that is not completed (the live session itself)
  // stays locked.
  const reviewLensId =
    !samplePreviewActive && row && !residentRow ? row.id : null;

  // A "→N" badge click on a reviewed session's Timeline (an explicit user
  // action) records Review's own edge-focus target; bring this pane to its
  // Graph lens, where the edges are emphasised. The Live view has no equivalent
  // (the `App.tsx` bridge sets `nav.lens`, which nothing reads), and this
  // reads only Review's store, never a Live field.
  const edgeFocusNonce = useReviewStore((s) => s.graphEdgeFocus?.nonce ?? null);
  const seenEdgeFocus = useRef(edgeFocusNonce);
  const focusGraphTab = useRef(false);
  useEffect(() => {
    if (edgeFocusNonce === seenEdgeFocus.current) return;
    seenEdgeFocus.current = edgeFocusNonce;
    if (edgeFocusNonce !== null && reviewLensId !== null) {
      focusGraphTab.current = true;
      setLens("graph");
    }
  }, [edgeFocusNonce, reviewLensId]);
  // The badge that was activated is gone with the Timeline lens: when focus
  // fell to the body, move it to the Graph tab (never if the user moved it).
  useEffect(() => {
    if (!focusGraphTab.current || lens !== "graph") return;
    focusGraphTab.current = false;
    const active = document.activeElement;
    if (active === null || active === document.body) {
      document.getElementById("sessions-lens-tab-graph")?.focus();
    }
  }, [lens]);
  if (liveRunning && row && !samplePreviewActive && !reviewableWhileLive(row)) {
    return (
      <section
        aria-label={t("sessions.detail")}
        className="flex flex-1 min-w-0 flex-col"
      >
        <ReviewLockedNotice
          message={t("sessions.reviewNeedsCompletedWhileLive")}
        />
      </section>
    );
  }

  if (!samplePreviewActive && !row) {
    return (
      <section
        aria-label={t("sessions.detail")}
        className="flex flex-1 min-w-0 flex-col items-center justify-center gap-(--space-3) p-(--space-6) text-center"
      >
        <Icon name="notes" size={22} className="text-text-muted opacity-40" />
        <p className="m-0 max-w-[360px] text-sm text-text-muted">
          {t("sessions.detailEmpty")}
        </p>
      </section>
    );
  }

  const handleLensKeyDown = (e: React.KeyboardEvent<HTMLButtonElement>) => {
    const NAV = ["ArrowRight", "ArrowLeft", "Home", "End"];
    if (!NAV.includes(e.key)) return;
    e.preventDefault();
    const currentIndex = DETAIL_LENSES.indexOf(lens);
    const nextIndex =
      e.key === "Home"
        ? 0
        : e.key === "End"
          ? DETAIL_LENSES.length - 1
          : e.key === "ArrowLeft"
            ? (currentIndex - 1 + DETAIL_LENSES.length) % DETAIL_LENSES.length
            : (currentIndex + 1) % DETAIL_LENSES.length;
    const next = DETAIL_LENSES[nextIndex];
    setLens(next);
    const tablist = e.currentTarget.parentElement;
    const tabs = tablist?.querySelectorAll<HTMLButtonElement>('[role="tab"]');
    tabs?.[nextIndex]?.focus();
  };

  // Ask/Chat reads the global store.
  const canAsk = askAvailable(lens) && globalHoldsRow;
  const sessionIdForRoute = samplePreviewActive ? null : (row?.id ?? null);

  const detail = (
    <section
      aria-label={t("sessions.detail")}
      className="flex flex-1 min-w-0 flex-col overflow-hidden"
    >
      <div className="ag-panel-head shrink-0">
        <div className="flex items-center gap-(--space-3) min-w-0">
          <h2 className="ag-panel-head__title overflow-hidden text-ellipsis whitespace-nowrap">
            {samplePreviewActive
              ? t("workspace.stateSample")
              : displayName(row as SessionRow)}
          </h2>
          {samplePreviewActive && (
            <span className="ag-chip" data-tone="accent">
              {t("sessions.samplePreviewLabel")}
            </span>
          )}
        </div>
        <div className="flex items-center gap-(--space-3) shrink-0">
          <button
            type="button"
            className="ag-btn-micro"
            aria-pressed={askOpen}
            disabled={!canAsk}
            onClick={() => setAskOpen((v) => !v)}
            title={t("sessions.ask")}
          >
            <Icon name="chat" size={14} /> {t("sessions.ask")}
          </button>
          {synthesisOffered && <SummaryMenu />}
        </div>
      </div>

      <div
        role="tablist"
        aria-label={t("sessions.lensTabs")}
        className="flex border-b border-(--edge) shrink-0"
      >
        {DETAIL_LENSES.map((l) => (
          <button
            key={l}
            type="button"
            role="tab"
            id={`sessions-lens-tab-${l}`}
            aria-selected={lens === l}
            // Only ONE `tabpanel` is ever mounted (the active lens's — see
            // below); an inactive tab pointing `aria-controls` at another
            // lens's panel id would reference an element that doesn't exist
            // in the DOM. Omit the attribute entirely for inactive tabs
            // rather than mount all five panels hidden — Graph's viewer
            // must stay lazy-loaded, only rendered while its lens is
            // selected (module doc above / ADR-0016).
            aria-controls={lens === l ? `sessions-lens-panel-${l}` : undefined}
            tabIndex={lens === l ? 0 : -1}
            className={`flex items-center gap-(--space-2) py-(--space-3) px-(--space-4) text-sm border-b-2 bg-transparent cursor-pointer transition-colors ${lens === l ? "border-b-accent text-text-primary" : "border-b-transparent text-text-secondary hover:text-text-primary"}`}
            onClick={() => setLens(l)}
            onKeyDown={handleLensKeyDown}
          >
            <Icon name={LENS_ICON[l]} size={14} /> {t(`sessions.lens.${l}`)}
          </button>
        ))}
      </div>

      <div className="flex flex-1 min-h-0 overflow-hidden">
        <div
          id={`sessions-lens-panel-${lens}`}
          role="tabpanel"
          aria-labelledby={`sessions-lens-tab-${lens}`}
          className="flex-1 min-w-0 min-h-0 overflow-hidden flex flex-col"
        >
          {lens !== "route" &&
            (reviewLensId !== null ? (
              <SessionViewProvider
                reviewSessionId={reviewLensId}
                reviewLens={lens === "transcript" ? null : lens}
              >
                <LensBody lens={lens} reviewed />
              </SessionViewProvider>
            ) : (
              <LensBody lens={lens} reviewed={false} />
            ))}
          {lens === "route" && (
            <SessionDataRoutePanel sessionId={sessionIdForRoute} />
          )}
        </div>
        {canAsk && askOpen && (
          <div className="w-[320px] min-w-[260px] shrink-0 border-l border-(--edge) overflow-hidden flex flex-col">
            <ChatSidebar />
          </div>
        )}
      </div>
    </section>
  );
  return synthesisOffered ? (
    <ResidentSynthesisScope>{detail}</ResidentSynthesisScope>
  ) : (
    detail
  );
}

export default SessionsBrowser;
