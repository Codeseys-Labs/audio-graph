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
import { useCallback, useEffect, useMemo, useState } from "react";
import { useAudioGraphStore } from "../store";
import type { AudioSourceInfo } from "../types";
import {
  captureTargetModeLabel,
  processCaptureId,
  processTreeCaptureId,
} from "../utils/captureTarget";
import Icon, { type IconName } from "./Icon";
import IconButton from "./IconButton";

// Classify a Device source as input (capture) or output (render).
//
// On Windows, WASAPI endpoint IDs encode direction: `{0.0.0.*}` is a render
// (output) endpoint, `{0.0.1.*}` is a capture (input) endpoint. We use that
// when available and fall back to a name heuristic on other platforms.
// (A fully backend-driven DeviceKind is tracked as a follow-up.)
function classifyDevice(
  source: AudioSourceInfo,
): "Input Devices" | "Output Devices" {
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
  icon: IconName;
} {
  switch (source.source_type.type) {
    case "SystemDefault":
      return { label: "System", icon: "system" };
    case "Device": {
      const label = classifyDevice(source);
      return { label, icon: label === "Input Devices" ? "mic" : "speaker" };
    }
    case "Application":
      return { label: "Applications", icon: "apps" };
    default:
      return { label: "Other", icon: "package" };
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

  // Process-list scope: "audio" shows only audio-emitting apps (the
  // Applications group); "all" also reveals the full Running Processes list.
  // Default to "audio" so users aren't drowned in 500+ system processes.
  const [processScope, setProcessScope] = useState<"audio" | "all">(() => {
    try {
      return localStorage.getItem("ag.processScope") === "all"
        ? "all"
        : "audio";
    } catch {
      return "audio";
    }
  });
  const setScope = useCallback((scope: "audio" | "all") => {
    setProcessScope(scope);
    try {
      localStorage.setItem("ag.processScope", scope);
    } catch {
      /* ignore */
    }
  }, []);
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
      { icon: IconName; sources: AudioSourceInfo[] }
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

  // Tailwind utility groups (ADR-0016). Colors/radii/fonts resolve through the
  // design tokens via the @theme bridge; spacing uses the token shorthand.
  const groupLabel =
    "text-2xs font-semibold uppercase tracking-[0.6px] text-text-muted mx-0 mb-(--space-1) px-(--space-2)";
  const groupToggle =
    "flex items-center gap-(--space-2) w-full bg-none border-none cursor-pointer text-left mt-(--space-3) hover:text-text-primary";
  const sourceItem =
    "flex items-center gap-(--space-4) py-(--space-3) px-(--space-4) rounded-sm transition-[background-color] duration-[120ms] ease-[ease] text-md text-text-primary";
  const sectionEmpty =
    "border border-border-color rounded-sm py-(--space-3) px-(--space-4) text-text-muted text-sm leading-[1.35]";

  return (
    <div className="border-b border-border-color">
      <div className="flex items-center justify-between mb-[10px]">
        <span className="audio-source-selector__title">Audio Sources</span>
        <IconButton
          icon="refresh"
          className="bg-none border-none cursor-pointer text-base py-(--space-1) px-(--space-2) rounded-sm transition-[background-color] duration-[150ms] ease-[ease] leading-none enabled:hover:bg-[rgba(255,255,255,0.08)] disabled:opacity-40 disabled:cursor-not-allowed"
          onClick={handleRefresh}
          disabled={isCapturing}
          variant="ghost"
          label={isCapturing ? captureLockedMessage : "Refresh sources"}
        />
      </div>

      {isCapturing && (
        <div className="border rounded-sm py-(--space-3) px-(--space-4) text-sm leading-[1.35] m-0 mb-(--space-4) bg-[rgba(250,204,21,0.08)] border-[rgba(250,204,21,0.28)] text-text-secondary">
          {captureLockedMessage}
        </div>
      )}

      {/* Search input */}
      <div className="relative pt-0 pb-(--space-4) px-(--space-4)">
        <input
          type="text"
          className="w-full py-(--space-3) pr-[28px] pl-[10px] bg-bg-tertiary border border-border-color rounded-sm text-text-primary text-sm outline-none box-border placeholder:text-text-muted focus:border-accent-blue"
          placeholder="Search sources & processes..."
          value={searchFilter}
          onChange={(e) => setSearchFilter(e.target.value)}
        />
        {searchFilter && (
          <IconButton
            icon="close"
            className="absolute right-[14px] top-1/2 -translate-y-1/2 bg-none border-none text-text-muted cursor-pointer text-sm py-(--space-1) px-(--space-2) leading-none hover:text-text-primary"
            onClick={() => setSearchFilter("")}
            variant="ghost"
            label="Clear search"
          />
        )}
      </div>

      {/* Process scope toggle: audio-emitting apps vs every process. */}
      <div
        className="flex gap-(--space-2) pt-0 pb-(--space-3) px-(--space-4)"
        role="tablist"
      >
        <button
          type="button"
          role="tab"
          aria-selected={processScope === "audio"}
          className={`flex-1 text-xs font-semibold py-(--space-2) px-(--space-3) rounded-md border cursor-pointer whitespace-nowrap ${processScope === "audio" ? "bg-bg-elevated text-accent border-accent" : "border-border-color bg-transparent text-text-muted hover:text-text-primary"}`}
          onClick={() => setScope("audio")}
          title="Show only applications currently emitting audio"
        >
          <Icon name="speaker" size={14} /> Audio apps
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={processScope === "all"}
          className={`flex-1 text-xs font-semibold py-(--space-2) px-(--space-3) rounded-md border cursor-pointer whitespace-nowrap ${processScope === "all" ? "bg-bg-elevated text-accent border-accent" : "border-border-color bg-transparent text-text-muted hover:text-text-primary"}`}
          onClick={() => setScope("all")}
          title="Show every running process / process tree"
        >
          <Icon name="processes" size={14} /> All processes
        </button>
      </div>

      {audioSources.length === 0 && processes.length === 0 ? (
        <div className="flex flex-col items-center gap-(--space-4) py-(--space-6) px-0 text-text-secondary text-center">
          <p className="m-0 text-text-primary text-md font-semibold">
            No capture targets detected
          </p>
          <ul className="m-0 py-0 px-(--space-5) list-none text-sm leading-[1.4] [&>li+li]:mt-(--space-2)">
            {emptyStateHints.map((hint) => (
              <li key={hint}>{hint}</li>
            ))}
          </ul>
          <button
            type="button"
            className="bg-none border border-accent-blue text-accent-blue rounded-sm py-(--space-2) px-(--space-5) text-sm cursor-pointer transition-[background-color] duration-[150ms] ease-[ease] hover:bg-[rgba(96,165,250,0.12)]"
            onClick={handleRefresh}
          >
            Retry
          </button>
        </div>
      ) : noResults ? (
        <div className="flex flex-col items-center gap-(--space-4) py-(--space-6) px-0 text-text-secondary text-center">
          <p className="m-0 text-text-primary text-md font-semibold">
            No matches for "{searchFilter}"
          </p>
        </div>
      ) : (
        <div className="flex flex-col gap-(--space-4)">
          {/* Audio Source Groups (System, Input/Output Devices, Applications) */}
          {[...groupedSources.entries()].map(([label, { icon, sources }]) => {
            const isCollapsed = collapsed.has(label);
            return (
              <div key={label}>
                <button
                  type="button"
                  className={`${groupLabel} ${groupToggle}`}
                  onClick={() => toggleGroup(label)}
                  aria-expanded={!isCollapsed}
                  title={isCollapsed ? `Expand ${label}` : `Collapse ${label}`}
                >
                  <span className="inline-block w-[10px] text-[9px] text-text-muted">
                    <Icon
                      name={isCollapsed ? "chevronRight" : "chevronDown"}
                      size={14}
                    />
                  </span>
                  <Icon name={icon} size={14} /> {label}
                  <span className="ml-(--space-3) text-2xs text-text-muted font-normal">
                    {sources.length}
                  </span>
                </button>
                {!isCollapsed && (
                  <div className="list-none m-0 p-0">
                    {sources.map((source) => {
                      const selected = isSelected(source.id);
                      const modeLabel = captureTargetModeLabel(source.id);
                      return (
                        // A native checkbox input cannot render the custom row
                        // layout (icon, name, badges); role keeps it accessible.
                        // biome-ignore lint/a11y/useSemanticElements: see comment above
                        <div
                          key={source.id}
                          role="checkbox"
                          aria-checked={selected}
                          aria-disabled={isCapturing}
                          tabIndex={0}
                          className={`${sourceItem} ${selected ? "bg-[rgba(74,222,128,0.12)]" : ""} ${isCapturing ? "opacity-60 cursor-not-allowed" : selected ? "cursor-pointer hover:bg-[rgba(74,222,128,0.18)]" : "cursor-pointer hover:bg-[rgba(255,255,255,0.05)]"}`}
                          onClick={() => handleToggle(source.id)}
                          onKeyDown={(e) => handleKeyDown(e, source.id)}
                          title={isCapturing ? captureLockedMessage : undefined}
                        >
                          <span
                            className={`w-[14px] h-[14px] rounded-[3px] border-2 shrink-0 relative transition-[border-color,background-color] duration-[120ms] ease-[ease] ${selected ? "border-accent-green bg-accent-green after:content-[''] after:absolute after:top-px after:left-(--space-2) after:w-(--space-2) after:h-[7px] after:border-solid after:border-[#0a2010] after:border-[0_2px_2px_0] after:rotate-45" : "border-text-muted"}`}
                          />
                          <span className="flex-1 overflow-hidden text-ellipsis whitespace-nowrap">
                            {source.name}
                          </span>
                          {source.source_type.type === "SystemDefault" && (
                            <span className="text-2xs font-semibold uppercase bg-[rgba(96,165,250,0.15)] text-accent-blue py-px px-(--space-3) rounded-[3px] tracking-[0.3px] shrink-0">
                              Default
                            </span>
                          )}
                          {source.source_type.type !== "SystemDefault" &&
                            modeLabel && (
                              <span className="text-2xs font-semibold uppercase bg-[rgba(96,165,250,0.15)] text-accent-blue py-px px-(--space-3) rounded-[3px] tracking-[0.3px] shrink-0">
                                {modeLabel}
                              </span>
                            )}
                          {selected && (
                            <span className="text-accent-green text-base font-bold shrink-0">
                              <Icon name="check" size={14} />
                            </span>
                          )}
                        </div>
                      );
                    })}
                  </div>
                )}
              </div>
            );
          })}

          {/* Running Processes Section — only in "All processes" scope (or
              while searching, so search can reach any process). */}
          {(processScope === "all" || filterText) &&
            filteredProcesses.length > 0 && (
              <div>
                <button
                  type="button"
                  className={`${groupLabel} ${groupToggle}`}
                  onClick={() => toggleGroup("Running Processes")}
                  aria-expanded={!collapsed.has("Running Processes")}
                  title={
                    collapsed.has("Running Processes")
                      ? "Expand Running Processes"
                      : "Collapse Running Processes"
                  }
                >
                  <span className="inline-block w-[10px] text-[9px] text-text-muted">
                    <Icon
                      name={
                        collapsed.has("Running Processes")
                          ? "chevronRight"
                          : "chevronDown"
                      }
                      size={14}
                    />
                  </span>
                  <Icon name="processes" size={14} /> Running Processes
                  <span className="ml-(--space-3) text-2xs text-text-muted font-normal">
                    {filteredProcesses.length}
                  </span>
                </button>
                {!collapsed.has("Running Processes") && (
                  <div className="list-none m-0 p-0">
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
                        // A native checkbox input cannot render the custom row
                        // layout (icon, name, PID, mode buttons); role keeps it
                        // accessible.
                        // biome-ignore lint/a11y/useSemanticElements: see comment above
                        <div
                          key={proc.pid}
                          role="checkbox"
                          aria-checked={selected || treeSelected}
                          aria-disabled={isCapturing}
                          tabIndex={0}
                          className={`${sourceItem} ${selected || treeSelected ? "bg-[rgba(74,222,128,0.12)]" : ""} ${isCapturing ? "opacity-60 cursor-not-allowed" : selected || treeSelected ? "cursor-pointer hover:bg-[rgba(74,222,128,0.18)]" : "cursor-pointer hover:bg-[rgba(255,255,255,0.05)]"}`}
                          onClick={() => handleToggle(processId)}
                          onKeyDown={(e) => handleKeyDown(e, processId)}
                          title={
                            isCapturing
                              ? captureLockedMessage
                              : `${activeMode}: ${proc.name}`
                          }
                        >
                          <span
                            className={`w-[14px] h-[14px] rounded-[3px] border-2 shrink-0 relative transition-[border-color,background-color] duration-[120ms] ease-[ease] ${selected || treeSelected ? "border-accent-green bg-accent-green after:content-[''] after:absolute after:top-px after:left-(--space-2) after:w-(--space-2) after:h-[7px] after:border-solid after:border-[#0a2010] after:border-[0_2px_2px_0] after:rotate-45" : "border-text-muted"}`}
                          />
                          <span className="flex-1 overflow-hidden text-ellipsis whitespace-nowrap">
                            {proc.name}
                          </span>
                          <span className="text-2xs text-text-muted font-mono whitespace-nowrap">
                            PID {proc.pid}
                          </span>
                          <button
                            type="button"
                            className={`border rounded-[3px] py-px px-(--space-3) text-2xs leading-[16px] min-w-[42px] text-center whitespace-nowrap cursor-pointer shrink-0 disabled:cursor-not-allowed disabled:opacity-60 ${selected ? "border-accent-green bg-[rgba(74,222,128,0.12)] text-accent-green" : "border-border-color bg-[rgba(255,255,255,0.04)] text-text-secondary enabled:hover:border-accent-blue enabled:hover:text-text-primary"}`}
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
                            className={`border rounded-[3px] py-px px-(--space-3) text-2xs leading-[16px] min-w-[42px] text-center whitespace-nowrap cursor-pointer shrink-0 disabled:cursor-not-allowed disabled:opacity-60 ${treeSelected ? "border-accent-green bg-[rgba(74,222,128,0.12)] text-accent-green" : "border-border-color bg-[rgba(255,255,255,0.04)] text-text-secondary enabled:hover:border-accent-blue enabled:hover:text-text-primary"}`}
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
                            <span className="text-accent-green text-base font-bold shrink-0">
                              <Icon name="check" size={14} />
                            </span>
                          )}
                        </div>
                      );
                    })}
                  </div>
                )}
              </div>
            )}
          {!filterText && filteredProcesses.length === 0 && (
            <div className={`${sectionEmpty} mt-(--space-2)`}>
              No process targets detected. Start an app and refresh to capture a
              process or process tree.
            </div>
          )}
        </div>
      )}
    </div>
  );
}
