/**
 * Grouped audio-source selector — the left-column picker where the user
 * chooses what to capture.
 *
 * Sources are grouped into four categories (System / Devices / Applications /
 * Running Processes) with a fixed display order. Selection is multi-select
 * (an array of source-id strings stored in the Zustand store); capture can
 * bind to any combination, which the Rust backend later multiplexes into the
 * processing pipeline.
 *
 * A search filter narrows the visible rows across all groups. While capture
 * is active the list is disabled so the user cannot mutate the selected set
 * mid-session — they must stop capture first.
 *
 * Store bindings: `audioSources`, `selectedSourceIds`, `toggleSourceId`,
 * `fetchSources`, `isCapturing`, `processes`, `searchFilter`,
 * `setSearchFilter`, `fetchProcesses`.
 *
 * Parent: `App.tsx` (left panel). No props.
 */
import { useEffect, useMemo, useCallback, useState } from "react";
import { useAudioGraphStore } from "../store";
import type { AudioSourceInfo } from "../types";
import {
  captureTargetModeLabel,
  processCaptureId,
  processTreeCaptureId,
} from "../utils/captureTarget";

// Classify a Device source as input (capture) or output (render).
//
// On Windows, WASAPI endpoint IDs encode direction: `{0.0.0.*}` is a render
// (output) endpoint, `{0.0.1.*}` is a capture (input) endpoint. We use that
// when available and fall back to a name heuristic on other platforms.
// (A fully backend-driven DeviceKind is tracked as a follow-up.)
function classifyDevice(source: AudioSourceInfo): "Input Devices" | "Output Devices" {
  const id =
    source.source_type.type === "Device" ? source.source_type.device_id : "";
  if (id.includes("{0.0.1.")) return "Input Devices";
  if (id.includes("{0.0.0.")) return "Output Devices";
  const n = source.name.toLowerCase();
  if (/(microphone|\bmic\b|\binput\b|line in|capture)/.test(n)) {
    return "Input Devices";
  }
  return "Output Devices";
}

// Group audio sources by type
function getSourceGroup(source: AudioSourceInfo): {
  label: string;
  icon: string;
} {
  switch (source.source_type.type) {
    case "SystemDefault":
      return { label: "System", icon: "🖥️" };
    case "Device": {
      const label = classifyDevice(source);
      return { label, icon: label === "Input Devices" ? "🎤" : "🔊" };
    }
    case "Application":
      return { label: "Applications", icon: "📱" };
    default:
      return { label: "Other", icon: "📦" };
  }
}

// Group ordering for consistent display
const GROUP_ORDER: Record<string, number> = {
  System: 0,
  "Input Devices": 1,
  "Output Devices": 2,
  Applications: 3,
  "Running Processes": 4,
  Other: 5,
};

const COLLAPSE_STORAGE_KEY = "audiograph.collapsedSourceGroups";

function loadCollapsedGroups(): Set<string> {
  try {
    const raw = localStorage.getItem(COLLAPSE_STORAGE_KEY);
    if (raw) return new Set(JSON.parse(raw) as string[]);
  } catch {
    /* ignore malformed persisted state */
  }
  return new Set();
}

function getEmptyStateHints(): string[] {
  const platform =
    typeof navigator === "undefined" ? "" : navigator.platform.toLowerCase();

  if (platform.includes("linux")) {
    return [
      "Check PipeWire or PulseAudio permissions for system and app capture.",
      "Start the target application, then refresh the source list.",
    ];
  }
  if (platform.includes("mac")) {
    return [
      "Check microphone and screen/audio capture permissions in System Settings.",
      "Start the target application, then refresh the source list.",
    ];
  }
  if (platform.includes("win")) {
    return [
      "Check Windows microphone privacy settings and app audio activity.",
      "Start the target application, then refresh the source list.",
    ];
  }

  return [
    "Check OS audio-capture permissions.",
    "Start the target application, then refresh the source list.",
  ];
}

export default function AudioSourceSelector() {
  const audioSources = useAudioGraphStore((s) => s.audioSources);
  const selectedSourceIds = useAudioGraphStore((s) => s.selectedSourceIds);
  const toggleSourceId = useAudioGraphStore((s) => s.toggleSourceId);
  const fetchSources = useAudioGraphStore((s) => s.fetchSources);
  const isCapturing = useAudioGraphStore((s) => s.isCapturing);
  const processes = useAudioGraphStore((s) => s.processes);
  const searchFilter = useAudioGraphStore((s) => s.searchFilter);
  const setSearchFilter = useAudioGraphStore((s) => s.setSearchFilter);
  const fetchProcesses = useAudioGraphStore((s) => s.fetchProcesses);
  const captureLockedMessage = "Stop capture to change sources";
  const emptyStateHints = useMemo(getEmptyStateHints, []);

  // Per-group collapse state (persisted across sessions).
  const [collapsed, setCollapsed] = useState<Set<string>>(loadCollapsedGroups);
  const toggleGroup = useCallback((label: string) => {
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(label)) next.delete(label);
      else next.add(label);
      try {
        localStorage.setItem(COLLAPSE_STORAGE_KEY, JSON.stringify([...next]));
      } catch {
        /* ignore persistence failures */
      }
      return next;
    });
  }, []);

  useEffect(() => {
    fetchSources();
    fetchProcesses();
  }, [fetchSources, fetchProcesses]);

  const filterText = searchFilter.toLowerCase().trim();

  // Group and filter audio sources
  const groupedSources = useMemo(() => {
    const groups = new Map<
      string,
      { icon: string; sources: AudioSourceInfo[] }
    >();

    for (const source of audioSources) {
      // Apply search filter
      if (filterText && !source.name.toLowerCase().includes(filterText)) {
        continue;
      }

      const { label, icon } = getSourceGroup(source);
      if (!groups.has(label)) {
        groups.set(label, { icon, sources: [] });
      }
      groups.get(label)!.sources.push(source);
    }

    return new Map(
      [...groups.entries()].sort(
        ([a], [b]) => (GROUP_ORDER[a] ?? 99) - (GROUP_ORDER[b] ?? 99),
      ),
    );
  }, [audioSources, filterText]);

  // Filter processes by search text
  const filteredProcesses = useMemo(() => {
    if (!filterText) return processes;
    return processes.filter(
      (p) =>
        p.name.toLowerCase().includes(filterText) ||
        (p.exe_path && p.exe_path.toLowerCase().includes(filterText)),
    );
  }, [processes, filterText]);

  const handleToggle = useCallback(
    (id: string) => {
      if (!isCapturing) toggleSourceId(id);
    },
    [isCapturing, toggleSourceId],
  );

  const handleRefresh = useCallback(() => {
    fetchSources();
    fetchProcesses();
  }, [fetchSources, fetchProcesses]);

  const isSelected = useCallback(
    (id: string) => selectedSourceIds.includes(id),
    [selectedSourceIds],
  );

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent, id: string) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        handleToggle(id);
      }
    },
    [handleToggle],
  );

  const noResults =
    filterText && groupedSources.size === 0 && filteredProcesses.length === 0;

  return (
    <div className="audio-source-selector">
      <div className="audio-source-selector__header">
        <span className="audio-source-selector__title">Audio Sources</span>
        <button
          className="audio-source-selector__refresh"
          onClick={handleRefresh}
          disabled={isCapturing}
          title={isCapturing ? captureLockedMessage : "Refresh sources"}
        >
          🔄
        </button>
      </div>

      {isCapturing && (
        <div className="audio-source-selector__locked-note">
          {captureLockedMessage}
        </div>
      )}

      {/* Search input */}
      <div className="audio-source-selector__search">
        <input
          type="text"
          className="audio-source-selector__search-input"
          placeholder="Search sources & processes..."
          value={searchFilter}
          onChange={(e) => setSearchFilter(e.target.value)}
        />
        {searchFilter && (
          <button
            className="audio-source-selector__search-clear"
            onClick={() => setSearchFilter("")}
            title="Clear search"
          >
            ✕
          </button>
        )}
      </div>

      {audioSources.length === 0 && processes.length === 0 ? (
        <div className="audio-source-selector__empty">
          <p>No capture targets detected</p>
          <ul className="audio-source-selector__empty-hints">
            {emptyStateHints.map((hint) => (
              <li key={hint}>{hint}</li>
            ))}
          </ul>
          <button className="audio-source-selector__retry" onClick={handleRefresh}>
            Retry
          </button>
        </div>
      ) : noResults ? (
        <div className="audio-source-selector__empty">
          <p>No matches for "{searchFilter}"</p>
        </div>
      ) : (
        <div className="audio-source-selector__groups">
          {/* Audio Source Groups (System, Input/Output Devices, Applications) */}
          {[...groupedSources.entries()].map(([label, { icon, sources }]) => {
            const isCollapsed = collapsed.has(label);
            return (
              <div key={label}>
                <button
                  type="button"
                  className="audio-source-selector__group-label audio-source-selector__group-toggle"
                  onClick={() => toggleGroup(label)}
                  aria-expanded={!isCollapsed}
                  title={isCollapsed ? `Expand ${label}` : `Collapse ${label}`}
                >
                  <span className="audio-source-selector__chevron">
                    {isCollapsed ? "▸" : "▾"}
                  </span>
                  {icon} {label}
                  <span className="audio-source-selector__group-count">
                    {sources.length}
                  </span>
                </button>
                {!isCollapsed && (
                  <ul className="source-list">
                    {sources.map((source) => {
                      const selected = isSelected(source.id);
                      const modeLabel = captureTargetModeLabel(source.id);
                      return (
                        <li
                          key={source.id}
                          className={`source-item ${selected ? "source-item--selected" : ""} ${isCapturing ? "source-item--disabled" : ""}`}
                          onClick={() => handleToggle(source.id)}
                          onKeyDown={(e) => handleKeyDown(e, source.id)}
                          role="checkbox"
                          aria-checked={selected}
                          aria-disabled={isCapturing}
                          tabIndex={0}
                          title={isCapturing ? captureLockedMessage : undefined}
                        >
                          <span
                            className={`source-item__checkbox ${selected ? "source-item__checkbox--checked" : ""}`}
                          />
                          <span className="source-item__name">{source.name}</span>
                          {source.source_type.type === "SystemDefault" && (
                            <span className="source-item__badge">Default</span>
                          )}
                          {source.source_type.type !== "SystemDefault" &&
                            modeLabel && (
                              <span className="source-item__badge">{modeLabel}</span>
                            )}
                          {selected && <span className="source-item__check">✓</span>}
                        </li>
                      );
                    })}
                  </ul>
                )}
              </div>
            );
          })}

          {/* Running Processes Section */}
          {filteredProcesses.length > 0 && (
            <div>
              <button
                type="button"
                className="audio-source-selector__group-label audio-source-selector__group-toggle"
                onClick={() => toggleGroup("Running Processes")}
                aria-expanded={!collapsed.has("Running Processes")}
                title={
                  collapsed.has("Running Processes")
                    ? "Expand Running Processes"
                    : "Collapse Running Processes"
                }
              >
                <span className="audio-source-selector__chevron">
                  {collapsed.has("Running Processes") ? "▸" : "▾"}
                </span>
                🖥️ Running Processes
                <span className="audio-source-selector__group-count">
                  {filteredProcesses.length}
                </span>
              </button>
              {!collapsed.has("Running Processes") && (
                <ul className="source-list">
                {filteredProcesses.map((proc) => {
                  const processId = processCaptureId(proc.pid);
                  const processTreeId = processTreeCaptureId(proc.pid);
                  const selected = isSelected(processId);
                  const treeSelected = isSelected(processTreeId);
                  const activeMode = treeSelected
                    ? "Process tree"
                    : selected
                      ? "Process"
                      : "Not selected";
                  return (
                    <li
                      key={proc.pid}
                      className={`source-item ${selected || treeSelected ? "source-item--selected" : ""} ${isCapturing ? "source-item--disabled" : ""}`}
                      onClick={() => handleToggle(processId)}
                      onKeyDown={(e) => handleKeyDown(e, processId)}
                      role="checkbox"
                      aria-checked={selected || treeSelected}
                      aria-disabled={isCapturing}
                      tabIndex={0}
                      title={isCapturing ? captureLockedMessage : `${activeMode}: ${proc.name}`}
                    >
                      <span
                        className={`source-item__checkbox ${selected || treeSelected ? "source-item__checkbox--checked" : ""}`}
                      />
                      <span className="source-item__name">{proc.name}</span>
                      <span className="source-item__pid">PID {proc.pid}</span>
                      <button
                        type="button"
                        className={`source-item__mode ${selected ? "source-item__mode--active" : ""}`}
                        disabled={isCapturing}
                        title={`Capture only ${proc.name}`}
                        aria-pressed={selected}
                        onClick={(e) => {
                          e.stopPropagation();
                          handleToggle(processId);
                        }}
                      >
                        Process
                      </button>
                      <button
                        type="button"
                        className={`source-item__mode ${treeSelected ? "source-item__mode--active" : ""}`}
                        disabled={isCapturing}
                        title={`Capture ${proc.name} and child processes`}
                        aria-pressed={treeSelected}
                        onClick={(e) => {
                          e.stopPropagation();
                          handleToggle(processTreeId);
                        }}
                      >
                        Tree
                      </button>
                      {(selected || treeSelected) && (
                        <span className="source-item__check">✓</span>
                      )}
                    </li>
                  );
                })}
              </ul>
              )}
            </div>
          )}
          {!filterText && filteredProcesses.length === 0 && (
            <div className="audio-source-selector__section-empty">
              No process targets detected. Start an app and refresh to capture
              a process or process tree.
            </div>
          )}
        </div>
      )}
    </div>
  );
}
