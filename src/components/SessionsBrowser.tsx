/**
 * Sessions destination — list→detail (SHELL-R2, seed audio-graph-e0c4,
 * plan §R2, ADR-0046). Rendered unconditionally inside `#workspace-panel-after`
 * (the legacy "Review" tab id/role/label — byte-identical; R4 is the unit
 * that renames it). No longer a modal: the Sessions browser IS this
 * destination's content now, not an overlay toggled by `sessionsBrowserOpen`
 * (that flag/action still exist per SHELL-R1 — `openSessionsBrowser` now
 * navigates here instead of opening anything).
 *
 * Composition: a rail (search + sort + trash filter + one row per
 * `SessionMetadata`) on the left, a detail pane on the right. Selecting a
 * row sets `nav.sessionId` (`useAudioGraphStore`'s `setNavSessionId`) and, for
 * any row that ISN'T the session that just finished recording, calls
 * `loadSession` — the existing `sessions.reviewLockedWhileLive` guard inside
 * it still applies. The just-stopped session's row is a special case: see
 * `isResidentLiveSession` below.
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
 * deliberately NOT stored on `nav.lens` — `nav.lens === "graph"` is already
 * load-bearing this run as the legacy `after`/`analysis` tab disambiguator
 * (`store/shellNav.ts`'s module doc), so routing the Graph LENS through the
 * same field would flip the whole shell to the legacy `analysis` tab instead
 * of showing the Graph lens inside this destination — R1's own documented
 * `setNavDest`/lens footgun, generalized. Lens selection here is local
 * component state instead, reset per session via `key={selectedId}` on
 * `<SessionDetail>`. R4 (which deletes the `analysis` tab and retires the
 * disambiguation need) is the natural unit to unify the two.
 */
import { lazy, Suspense, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useAudioGraphStore } from "../store";
import type { PendingFinalizingSession, SessionMetadata } from "../types";
import { downloadAsFile, filenameTimestamp } from "../utils/download";
import { formatDurationHoursAware, formatRelativeTime } from "../utils/format";
import ChatSidebar from "./ChatSidebar";
import Icon, { type IconName } from "./Icon";
import IconButton from "./IconButton";
import LiveTranscript from "./LiveTranscript";
import NotesPanel, { useNotesSynthesis } from "./NotesPanel";
import Popover, { PopoverItem } from "./Popover";
import SeekTimeline from "./SeekTimeline";
import SessionDataRoutePanel from "./SessionDataRoutePanel";

// Same code-split rationale as the old App.tsx-owned import (ADR-0016 /
// modernization-audit 2.3): react-force-graph-2d stays a deferred chunk.
// Rollup dedupes by module specifier, so this second `lazy()` call (App.tsx's
// "analysis" tab keeps its own, per the plan's deliberate interim
// duplication) does not double-bundle the vendor chunk — verified via
// `bun run build:analyze` (see the PR body / final report for the numbers).
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
  const loadSession = useAudioGraphStore((s) => s.loadSession);
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

  const [search, setSearch] = useState("");
  const [sortMode, setSortMode] = useState<SessionSortMode>(() =>
    loadSortPreference(),
  );
  const [showTrash, setShowTrash] = useState(false);
  const [recoverySummary, setRecoverySummary] = useState<string | null>(null);
  const [exportingIds, setExportingIds] = useState<Set<string>>(
    () => new Set(),
  );
  const reviewLocked = isCapturing || isTranscribing;

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

  // The just-stopped session's data is already resident in the global store
  // (it WAS the live session) — re-fetching via `loadSession` would race the
  // same finalize-on-disk gap `pendingFinalizingSession` exists to route
  // around, and could momentarily clobber good in-memory data with a
  // not-yet-fully-flushed read. Any OTHER row still goes through
  // `loadSession` as before.
  //
  // Keyed to THIS row's own `optimistic` flag, not `pendingFinalizingSession`
  // directly. `pendingFinalizingSession` is a write-only store slot in
  // production — nothing cleared it here once set, so matching on
  // `pendingFinalizingSession?.id === row.id` kept skipping `loadSession`
  // for that id forever, even long after `mergeSessionRows` stopped
  // injecting the optimistic row for it (i.e. after `sessions.json` caught
  // up and the store had since loaded a completely different session's
  // data over it). `row.optimistic` self-clears in lockstep with the merge
  // (see `mergeSessionRows`), so the skip only applies during the exact
  // unreconciled window the resident-data premise is actually true for.
  const handleSelect = (row: SessionRow) => {
    setNavSessionId(row.id);
    const isResidentLiveSession = row.optimistic === true;
    if (isResidentLiveSession) return;
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
                  reviewLocked={reviewLocked}
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
        reviewLocked={reviewLocked}
      />
    </div>
  );
}

interface SessionRowItemProps {
  row: SessionRow;
  selected: boolean;
  exporting: boolean;
  reviewLocked: boolean;
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
        disabled={reviewLocked}
        title={reviewLocked ? t("sessions.reviewLockedWhileLive") : undefined}
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

interface SessionDetailProps {
  row: SessionRow | null;
  samplePreviewActive: boolean;
  reviewLocked: boolean;
}

function SessionDetail({
  row,
  samplePreviewActive,
  reviewLocked,
}: SessionDetailProps) {
  const { t } = useTranslation();
  const loadedSessionId = useAudioGraphStore((s) => s.loadedSessionId);
  const [lens, setLens] = useState<DetailLens>("notes");
  const [askOpen, setAskOpen] = useState(false);
  const synthesis = useNotesSynthesis();

  // While live, concurrent Live+Review is not delivered (plan §R2, ADR-0046) —
  // reuses `sessions.reviewLockedWhileLive` verbatim (en.json:778), the same
  // copy `loadSession`'s own guard has always shown.
  if (reviewLocked) {
    return (
      <section
        aria-label={t("sessions.detail")}
        className="flex flex-1 min-w-0 flex-col items-center justify-center gap-(--space-3) p-(--space-6) text-center"
      >
        <Icon name="warning" size={22} className="text-text-muted opacity-40" />
        <p
          className="m-0 max-w-[360px] text-sm text-text-secondary"
          role="status"
        >
          {t("sessions.reviewLockedWhileLive")}
        </p>
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

  const canAsk = askAvailable(lens);
  const sessionIdForRoute = samplePreviewActive ? null : (row?.id ?? null);

  return (
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
          {!loadedSessionId && (
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
          )}
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
          {lens === "notes" && (
            <NotesPanel synthesis={synthesis} headerActions={false} />
          )}
          {lens === "transcript" && <LiveTranscript />}
          {lens === "timeline" && <SeekTimeline />}
          {lens === "graph" && (
            <Suspense fallback={null}>
              <KnowledgeGraphViewer />
            </Suspense>
          )}
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
}

export default SessionsBrowser;
