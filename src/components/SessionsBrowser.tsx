/**
 * Sessions browser modal — lets the user inspect, restore, and delete
 * past capture sessions.
 *
 * Source of truth is the backend `sessions.json` index —
 * `list_sessions` returns all known sessions, `load_session` replaces the
 * active transcript and graph views, `restore_session` untrashes a soft-deleted
 * session, `delete_session` soft-deletes (marks for expiry),
 * `delete_session_permanently` hard-deletes, and `purge_expired_sessions`
 * cleans up old soft-deletes.
 *
 * Sort mode (`newest | oldest | nameAsc | nameDesc | largest`) is
 * persisted to `localStorage` under `audiograph:sessionsBrowser:sort`
 * so it survives reloads independent of the Rust-side settings file.
 *
 * Focus-trapped via `useFocusTrap`. Escape handled at the app level by
 * `useKeyboardShortcuts`.
 *
 * Store bindings: `sessionsBrowserOpen`, `closeSessionsBrowser`.
 *
 * Parent: `App.tsx` (rendered conditionally). No props.
 */
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useFocusTrap } from "../hooks/useFocusTrap";
import { useAudioGraphStore } from "../store";
import type { SessionMetadata } from "../types";
import { downloadAsFile, filenameTimestamp } from "../utils/download";
import IconButton from "./IconButton";

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

/** Format a unix-millis timestamp into a short, human-readable local string. */
function formatTimestamp(ms: number): string {
  if (!ms) return "—";
  return new Date(ms).toLocaleString();
}

/** Format a duration in seconds as "Hh Mm" or "Mm Ss". */
function formatDuration(seconds: number | null): string {
  if (seconds === null || seconds === undefined) return "—";
  if (seconds < 60) return `${seconds}s`;
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = seconds % 60;
  if (h > 0) return `${h}h ${m}m`;
  return `${m}m ${s}s`;
}

/** CSS-class-friendly modifier for a session's status. */
function statusModifier(status: SessionMetadata["status"]): string {
  return `sessions-browser__status--${status}`;
}

/** Display name for a session — falls back to the short id. */
function displayName(s: SessionMetadata): string {
  return s.title ?? s.id.slice(0, 8);
}

/** Filter+sort pipeline. Exported for unit tests. */
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

function SessionsBrowser() {
  const { t } = useTranslation();
  const modalRef = useFocusTrap<HTMLDivElement>();
  const sessions = useAudioGraphStore((s) => s.sessions);
  const sessionsLoading = useAudioGraphStore((s) => s.sessionsLoading);
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
  const closeSessionsBrowser = useAudioGraphStore(
    (s) => s.closeSessionsBrowser,
  );
  const setRightPanelTab = useAudioGraphStore((s) => s.setRightPanelTab);

  const [search, setSearch] = useState("");
  const [sortMode, setSortMode] = useState<SessionSortMode>(() =>
    loadSortPreference(),
  );
  const [showTrash, setShowTrash] = useState(false);
  const [recoverySummary, setRecoverySummary] = useState<string | null>(null);
  const [exportingIds, setExportingIds] = useState<Set<string>>(
    () => new Set(),
  );

  // Refresh on mount — match the v2 store's own larger fetch (200) so the
  // browser's search can actually find old entries, not just the 10 most
  // recent the v1 overlay loaded.
  useEffect(() => {
    void listSessions(200);
  }, [listSessions]);

  const trashCount = useMemo(
    () => sessions.filter((s) => s.deleted === true).length,
    [sessions],
  );

  const visible = useMemo(
    () => applyFilterAndSort(sessions, search, sortMode, showTrash),
    [sessions, search, sortMode, showTrash],
  );

  const handleSortChange = (mode: SessionSortMode) => {
    setSortMode(mode);
    saveSortPreference(mode);
  };

  const handleLoad = async (sessionId: string) => {
    await loadSession(sessionId);
    setRightPanelTab("transcript");
    closeSessionsBrowser();
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
    <div
      className="settings-overlay"
      role="none"
      onClick={closeSessionsBrowser}
      onKeyDown={(e) => {
        if (e.key === "Escape") closeSessionsBrowser();
      }}
    >
      <div
        ref={modalRef}
        className="settings-modal sessions-browser"
        onClick={(e) => e.stopPropagation()}
        onKeyDown={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
        aria-labelledby="sessions-browser-title"
        tabIndex={-1}
      >
        <div className="settings-header">
          <h2 id="sessions-browser-title" className="settings-header__title">
            {t("sessions.title")}
          </h2>
          <IconButton
            icon="close"
            label={t("sessions.close")}
            variant="ghost"
            className="settings-header__close"
            onClick={closeSessionsBrowser}
          />
        </div>

        <div className="settings-content">
          <div className="sessions-browser__toolbar flex flex-wrap items-center gap-(--space-4) mb-(--space-5)">
            <input
              type="search"
              className="sessions-browser__search flex-[1_1_200px] min-w-0 py-(--space-3) px-(--space-4) rounded-md border border-border-color bg-transparent text-[inherit]"
              aria-label={t("sessions.searchLabel")}
              placeholder={t("sessions.searchPlaceholder")}
              value={search}
              onChange={(e) => setSearch(e.target.value)}
            />
            <label className="flex items-center gap-(--space-3) text-[0.85em]">
              <span>{t("sessions.sortLabel")}</span>
              <select
                aria-label={t("sessions.sortLabel")}
                value={sortMode}
                onChange={(e) =>
                  handleSortChange(e.target.value as SessionSortMode)
                }
                className="py-(--space-2) px-(--space-4) rounded-md border border-border-color bg-transparent text-[inherit]"
              >
                {SORT_MODES.map((m) => (
                  <option key={m} value={m}>
                    {t(`sessions.sort.${m}`)}
                  </option>
                ))}
              </select>
            </label>
            <button
              type="button"
              className="settings-btn"
              onClick={handleRecover}
              title={t("sessions.recoverTitle")}
            >
              {t("sessions.recover")}
            </button>
            <button
              type="button"
              className="settings-btn"
              aria-pressed={showTrash}
              onClick={() => setShowTrash((v) => !v)}
              title={
                showTrash ? t("sessions.hideTrash") : t("sessions.showTrash")
              }
            >
              {showTrash
                ? t("sessions.hideTrash")
                : t("sessions.trashCount", { count: trashCount })}
            </button>
          </div>
          {recoverySummary ? (
            <p className="settings-section__empty" role="status">
              {recoverySummary}
            </p>
          ) : null}

          {sessionsLoading ? (
            <p className="settings-section__empty" role="status">
              {t("common.loading")}
            </p>
          ) : sessions.length === 0 ? (
            <p className="settings-section__empty" role="status">
              {t("sessions.noSessions")}
            </p>
          ) : visible.length === 0 ? (
            <p className="settings-section__empty" role="status">
              {t("sessions.noMatches")}
            </p>
          ) : (
            <ul className="sessions-browser__list list-none p-0 m-0 flex flex-col gap-(--space-4)">
              {visible.map((s) => (
                <li
                  key={s.id}
                  className={`sessions-browser__item flex flex-col gap-(--space-3) rounded-md border border-border-color py-[10px] px-(--space-5) ${
                    s.deleted ? "opacity-70" : "opacity-100"
                  }`}
                  data-testid={`session-${s.id}`}
                  data-trashed={s.deleted ? "true" : "false"}
                >
                  <div className="flex justify-between items-baseline gap-(--space-4)">
                    <div className="flex flex-col gap-(--space-1) min-w-0">
                      <strong
                        className="text-[0.95em] overflow-hidden text-ellipsis whitespace-nowrap"
                        title={s.id}
                      >
                        {displayName(s)}
                      </strong>
                      <span className="text-[0.8em] opacity-70">
                        {s.deleted && s.deleted_at
                          ? t("sessions.trashedOn", {
                              date: formatTimestamp(s.deleted_at),
                            })
                          : formatTimestamp(s.created_at)}
                      </span>
                    </div>
                    <span
                      className={`sessions-browser__status ${statusModifier(s.status)} text-[0.75em] py-[2px] px-(--space-4) rounded-full border border-current opacity-80 capitalize whitespace-nowrap`}
                    >
                      {s.status}
                    </span>
                  </div>

                  <div className="text-[0.8em] opacity-75 flex gap-(--space-5) flex-wrap">
                    <span>
                      {t("sessions.stats.duration")}:{" "}
                      {formatDuration(s.duration_seconds)}
                    </span>
                    <span>
                      {t("sessions.stats.segments")}: {s.segment_count}
                    </span>
                    <span>
                      {t("sessions.stats.speakers")}: {s.speaker_count}
                    </span>
                    <span>
                      {t("sessions.stats.entities")}: {s.entity_count}
                    </span>
                  </div>

                  <div className="flex gap-(--space-4) justify-end flex-wrap">
                    {s.deleted ? (
                      <>
                        <button
                          type="button"
                          className="settings-btn"
                          onClick={() => handleRestore(s.id)}
                        >
                          {t("sessions.restore")}
                        </button>
                        <button
                          type="button"
                          className="settings-btn settings-btn--danger"
                          onClick={() => handleDeletePermanently(s.id)}
                        >
                          {t("sessions.deletePermanently")}
                        </button>
                      </>
                    ) : (
                      <>
                        <button
                          type="button"
                          className="settings-btn settings-btn--primary"
                          onClick={() => handleLoad(s.id)}
                        >
                          {t("sessions.load")}
                        </button>
                        <button
                          type="button"
                          className="settings-btn"
                          onClick={() => handleExport(s.id)}
                          disabled={exportingIds.has(s.id)}
                          aria-busy={exportingIds.has(s.id)}
                        >
                          {exportingIds.has(s.id)
                            ? t("sessions.exporting")
                            : t("sessions.export")}
                        </button>
                        <button
                          type="button"
                          className="settings-btn settings-btn--danger"
                          onClick={() => handleDelete(s.id)}
                        >
                          {t("sessions.delete")}
                        </button>
                      </>
                    )}
                  </div>
                </li>
              ))}
            </ul>
          )}
        </div>
      </div>
    </div>
  );
}

export default SessionsBrowser;
