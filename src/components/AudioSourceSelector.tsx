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
import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import { useAudioGraphStore } from "../store";
import type {
  AudioFormatInfo,
  AudioPermissionStatus,
  AudioSourceCapabilities,
  AudioSourceInfo,
  SourceRecoveryIssue,
} from "../types";
import {
  captureTargetModeLabel,
  processCaptureId,
  processTreeCaptureId,
  sourceCaptureTargetId,
} from "../utils/captureTarget";
import Icon, { type IconName } from "./Icon";
import IconButton from "./IconButton";

function classifyDevice(
  source: AudioSourceInfo,
): "Input Devices" | "Output Devices" | "Unknown Devices" {
  if (source.device_kind === "Input") return "Input Devices";
  if (source.device_kind === "Output") return "Output Devices";
  return "Unknown Devices";
}

function formatDefaultAudioFormat(
  format?: AudioFormatInfo | null,
): string | null {
  if (!format) return null;
  const rate =
    format.sample_rate % 1000 === 0
      ? `${format.sample_rate / 1000} kHz`
      : `${format.sample_rate} Hz`;
  return `${rate} / ${format.channels}ch`;
}

function sourceMetadataLabel(source: AudioSourceInfo): string | null {
  if (source.source_type.type === "Application") {
    return source.source_type.bundle_id ?? null;
  }
  return null;
}

interface SourceCapabilityRequirement {
  key: keyof Pick<
    AudioSourceCapabilities,
    | "supports_system_capture"
    | "supports_application_capture"
    | "supports_process_tree_capture"
    | "supports_device_selection"
  >;
  label: string;
}

function sourceCapabilityRequirement(
  source: AudioSourceInfo,
): SourceCapabilityRequirement | null {
  const target = source.capture_target ?? source.id;
  if (target === "system" || target === "system-default") {
    return { key: "supports_system_capture", label: "System" };
  }
  if (target.startsWith("device:")) {
    return { key: "supports_device_selection", label: "Device" };
  }
  if (target.startsWith("tree:") || target.startsWith("process-tree:")) {
    return {
      key: "supports_process_tree_capture",
      label: "Process-tree",
    };
  }
  if (
    target.startsWith("app:") ||
    target.startsWith("name:") ||
    target.startsWith("app-name:")
  ) {
    return { key: "supports_application_capture", label: "Application" };
  }

  switch (source.source_type.type) {
    case "SystemDefault":
      return { key: "supports_system_capture", label: "System" };
    case "Device":
      return { key: "supports_device_selection", label: "Device" };
    case "Application":
    case "ApplicationName":
      return { key: "supports_application_capture", label: "Application" };
    case "ProcessTree":
      return {
        key: "supports_process_tree_capture",
        label: "Process-tree",
      };
  }
}

function sourceUnsupportedReason(source: AudioSourceInfo): string | null {
  const capabilities = source.capabilities ?? null;
  if (!capabilities) return null;

  const explicitReason = capabilities.unsupported_reason?.trim();
  if (capabilities.capture_supported === false) {
    return explicitReason || "the selected source is not supported";
  }

  const requirement = sourceCapabilityRequirement(source);
  if (requirement && capabilities[requirement.key] === false) {
    return `${requirement.label} capture is not supported`;
  }

  return null;
}

function sourceCapabilityUnsupportedReason(
  capabilities: readonly AudioSourceCapabilities[],
  key: keyof Pick<
    AudioSourceCapabilities,
    "supports_application_capture" | "supports_process_tree_capture"
  >,
  label: string,
): string | null {
  if (capabilities.length === 0) return null;
  if (capabilities.some((capability) => capability[key])) return null;

  const backendName = capabilities.find((capability) =>
    capability.backend_name.trim(),
  )?.backend_name;
  return backendName
    ? `${label} capture is not supported by ${backendName}`
    : `${label} capture is not supported`;
}

function permissionNeedsRepair(
  status: AudioPermissionStatus | null | undefined,
): status is Exclude<AudioPermissionStatus, "Granted" | "NotRequired"> {
  return Boolean(status && status !== "Granted" && status !== "NotRequired");
}

function sourcePermissionLabel(status: AudioPermissionStatus): string {
  switch (status) {
    case "Denied":
      return "denied";
    case "NotDetermined":
      return "not granted";
    case "Unknown":
      return "unavailable";
    case "Granted":
      return "granted";
    case "NotRequired":
      return "not required";
  }
}

function sourcePermissionRecoveryMessage(source: AudioSourceInfo): string {
  const recovery = source.permission_recovery ?? null;
  if (recovery) return `${source.name}: ${recovery.summary} ${recovery.body}`;
  const status = source.permission_status ?? "Unknown";
  return `Audio capture permission is ${sourcePermissionLabel(status)} for ${source.name}.`;
}

function uniqueSourceIds(ids: readonly (string | undefined)[]): string[] {
  return [...new Set(ids.filter((id): id is string => Boolean(id)))];
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
      return {
        label,
        icon:
          label === "Input Devices"
            ? "mic"
            : label === "Output Devices"
              ? "speaker"
              : "package",
      };
    }
    case "Application":
    case "ApplicationName":
      return { label: "Applications", icon: "apps" };
    case "ProcessTree":
      return { label: "Running Processes", icon: "processes" };
    default:
      return { label: "Other", icon: "package" };
  }
}

// Group ordering for consistent display
const GROUP_ORDER: Record<string, number> = {
  System: 0,
  "Input Devices": 1,
  "Output Devices": 2,
  "Unknown Devices": 3,
  Applications: 4,
  "Running Processes": 5,
  Other: 6,
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
  return [
    "Check OS audio-capture permissions.",
    "Start the target application, then refresh the source list.",
  ];
}

export default function AudioSourceSelector() {
  const audioSources = useAudioGraphStore((s) => s.audioSources);
  const selectedSourceIds = useAudioGraphStore((s) => s.selectedSourceIds);
  const toggleSourceId = useAudioGraphStore((s) => s.toggleSourceId);
  const removeSelectedSourceIds = useAudioGraphStore(
    (s) => s.removeSelectedSourceIds,
  );
  const fetchSources = useAudioGraphStore((s) => s.fetchSources);
  const isCapturing = useAudioGraphStore((s) => s.isCapturing);
  const processes = useAudioGraphStore((s) => s.processes);
  const searchFilter = useAudioGraphStore((s) => s.searchFilter);
  const setSearchFilter = useAudioGraphStore((s) => s.setSearchFilter);
  const fetchProcesses = useAudioGraphStore((s) => s.fetchProcesses);
  const sourceRecoveryIntent = useAudioGraphStore(
    (s) => s.sourceRecoveryIntent,
  );
  const clearSourceRecoveryIntent = useAudioGraphStore(
    (s) => s.clearSourceRecoveryIntent,
  );
  const { t } = useTranslation();
  const captureLockedMessage = "Stop capture to change sources";
  const emptyStateHints = useMemo(getEmptyStateHints, []);
  const selectorRef = useRef<HTMLDivElement | null>(null);
  const searchInputRef = useRef<HTMLInputElement | null>(null);
  // Stable id prefix for the visually-hidden reason elements wired via
  // aria-describedby so assistive tech announces disabled/unavailable reasons
  // (house pattern: ControlBar.tsx sr-only reason spans).
  const reasonIdPrefix = useId();

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

  useEffect(() => {
    if (!sourceRecoveryIntent) return;

    setSearchFilter("");
    setCollapsed(new Set());
    void fetchSources();
    void fetchProcesses();
    window.setTimeout(() => {
      selectorRef.current?.scrollIntoView?.({ block: "nearest" });
      searchInputRef.current?.focus();
    }, 0);
  }, [fetchProcesses, fetchSources, setSearchFilter, sourceRecoveryIntent]);

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
      let group = groups.get(label);
      if (!group) {
        group = { icon, sources: [] };
        groups.set(label, group);
      }
      group.sources.push(source);
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
        p.exe_path?.toLowerCase().includes(filterText),
    );
  }, [processes, filterText]);

  const sourcesBySelectionId = useMemo(() => {
    const byId = new Map<string, AudioSourceInfo>();
    for (const source of audioSources) {
      byId.set(source.id, source);
      byId.set(sourceCaptureTargetId(source), source);
    }
    return byId;
  }, [audioSources]);

  const activeRecoveryIssues = useMemo<SourceRecoveryIssue[]>(() => {
    const issues: SourceRecoveryIssue[] = [];
    for (const sourceId of selectedSourceIds) {
      const source = sourcesBySelectionId.get(sourceId);
      if (!source) {
        issues.push({
          kind: "unavailable",
          sourceId,
          message: `Selected audio source ${sourceId} is not available.`,
        });
        continue;
      }

      const captureTargetId = sourceCaptureTargetId(source);
      const unsupportedReason = sourceUnsupportedReason(source);
      if (unsupportedReason) {
        issues.push({
          kind: "unsupported",
          sourceId: captureTargetId,
          sourceName: source.name,
          message: `${source.name} cannot be captured: ${unsupportedReason}`,
        });
      }

      if (permissionNeedsRepair(source.permission_status)) {
        issues.push({
          kind: "permission",
          sourceId: captureTargetId,
          sourceName: source.name,
          permissionStatus: source.permission_status,
          permissionRecovery: source.permission_recovery ?? undefined,
          message: sourcePermissionRecoveryMessage(source),
        });
      }
    }
    return issues;
  }, [selectedSourceIds, sourcesBySelectionId]);

  const recoveryIssues = useMemo<SourceRecoveryIssue[]>(() => {
    const issues = [...activeRecoveryIssues];
    const seen = new Set(
      issues.map(
        (issue) => `${issue.kind}:${issue.sourceId ?? ""}:${issue.message}`,
      ),
    );

    for (const issue of sourceRecoveryIntent?.issues ?? []) {
      if (issue.kind !== "unselected" && issue.kind !== "policy_conflict") {
        continue;
      }
      if (issue.kind === "unselected" && selectedSourceIds.length > 0) {
        continue;
      }
      const key = `${issue.kind}:${issue.sourceId ?? ""}:${issue.message}`;
      if (seen.has(key)) continue;
      seen.add(key);
      issues.push(issue);
    }

    if (issues.length > 0) return issues;
    return sourceRecoveryIntent?.issues ?? [];
  }, [activeRecoveryIssues, selectedSourceIds.length, sourceRecoveryIntent]);

  const recoveryIssueBySourceId = useMemo(() => {
    const byId = new Map<string, SourceRecoveryIssue>();
    for (const issue of activeRecoveryIssues) {
      if (issue.sourceId) byId.set(issue.sourceId, issue);
    }
    return byId;
  }, [activeRecoveryIssues]);

  const processControlCapability = useMemo(() => {
    const capabilities = audioSources
      .map((source) => source.capabilities)
      .filter(
        (
          capabilities,
        ): capabilities is NonNullable<AudioSourceInfo["capabilities"]> =>
          Boolean(capabilities),
      );
    return {
      processUnsupportedReason: sourceCapabilityUnsupportedReason(
        capabilities,
        "supports_application_capture",
        "Application",
      ),
      processTreeUnsupportedReason: sourceCapabilityUnsupportedReason(
        capabilities,
        "supports_process_tree_capture",
        "Process tree",
      ),
    };
  }, [audioSources]);

  const staleSelectedSourceIds = useMemo(
    () =>
      uniqueSourceIds(
        activeRecoveryIssues
          .filter((issue) => issue.kind === "unavailable")
          .map((issue) => issue.sourceId),
      ),
    [activeRecoveryIssues],
  );
  const unsupportedSelectedSourceIds = useMemo(
    () =>
      uniqueSourceIds(
        activeRecoveryIssues
          .filter(
            (issue) =>
              issue.kind === "unsupported" || issue.kind === "permission",
          )
          .map((issue) => issue.sourceId),
      ),
    [activeRecoveryIssues],
  );
  const repairSelectedSourceIds = useMemo(
    () =>
      uniqueSourceIds([
        ...staleSelectedSourceIds,
        ...unsupportedSelectedSourceIds,
      ]),
    [staleSelectedSourceIds, unsupportedSelectedSourceIds],
  );

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

  const focusSourcePicker = useCallback(() => {
    setSearchFilter("");
    window.setTimeout(() => {
      selectorRef.current?.scrollIntoView?.({ block: "nearest" });
      searchInputRef.current?.focus();
    }, 0);
  }, [setSearchFilter]);

  const clearSelectedSourceIds = useCallback(
    (ids: readonly string[]) => {
      removeSelectedSourceIds(uniqueSourceIds(ids));
    },
    [removeSelectedSourceIds],
  );

  const handleReselectSources = useCallback(() => {
    if (repairSelectedSourceIds.length > 0) {
      removeSelectedSourceIds(repairSelectedSourceIds);
    }
    focusSourcePicker();
  }, [focusSourcePicker, removeSelectedSourceIds, repairSelectedSourceIds]);

  const isSelected = useCallback(
    (id: string) => selectedSourceIds.includes(id),
    [selectedSourceIds],
  );
  const isSourceSelected = useCallback(
    (source: AudioSourceInfo, captureTargetId: string) =>
      selectedSourceIds.includes(captureTargetId) ||
      selectedSourceIds.includes(source.id),
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
    <div ref={selectorRef} className="border-b border-border-color">
      <div className="flex items-center justify-between mb-[10px]">
        <span className="audio-source-selector__title">Audio Sources</span>
        <IconButton
          icon="refresh"
          className="bg-none border-none cursor-pointer text-base py-(--space-1) px-(--space-2) rounded-sm transition-[background-color] duration-[150ms] ease-[ease] leading-none enabled:hover:bg-(--hover-overlay-strong) disabled:opacity-40 disabled:cursor-not-allowed"
          onClick={handleRefresh}
          disabled={isCapturing}
          variant="ghost"
          label={isCapturing ? captureLockedMessage : "Refresh sources"}
        />
      </div>

      {isCapturing && (
        <div className="border rounded-sm py-(--space-3) px-(--space-4) text-sm leading-[1.35] m-0 mb-(--space-4) bg-(--tint-warning) border-(--tint-border-warning) text-(--text-on-tint-warning)">
          {captureLockedMessage}
        </div>
      )}

      {recoveryIssues.length > 0 && (
        <div
          className="mx-(--space-4) mb-(--space-4) border border-(--tint-border-warning) rounded-sm bg-(--tint-warning) text-(--text-on-tint-warning) py-(--space-3) px-(--space-4) text-sm leading-[1.35]"
          role="status"
        >
          <p className="m-0 font-semibold">Source needs attention</p>
          <ul className="my-(--space-2) pl-(--space-5)">
            {recoveryIssues.slice(0, 4).map((issue) => (
              <li
                key={`${issue.kind}-${issue.sourceId ?? issue.sourceName ?? issue.message}`}
              >
                {issue.message}
              </li>
            ))}
          </ul>
          <div className="flex flex-wrap gap-(--space-2)">
            {staleSelectedSourceIds.length > 0 && (
              <button
                type="button"
                className="bg-none border border-current rounded-sm py-(--space-1) px-(--space-3) text-xs font-semibold cursor-pointer"
                onClick={() => clearSelectedSourceIds(staleSelectedSourceIds)}
              >
                Clear unavailable
              </button>
            )}
            {unsupportedSelectedSourceIds.length > 0 && (
              <button
                type="button"
                className="bg-none border border-current rounded-sm py-(--space-1) px-(--space-3) text-xs font-semibold cursor-pointer"
                onClick={() =>
                  clearSelectedSourceIds(unsupportedSelectedSourceIds)
                }
              >
                Clear unsupported
              </button>
            )}
            <button
              type="button"
              className="bg-none border border-current rounded-sm py-(--space-1) px-(--space-3) text-xs font-semibold cursor-pointer"
              onClick={handleReselectSources}
            >
              {repairSelectedSourceIds.length > 0
                ? "Reselect sources"
                : "Choose source"}
            </button>
            {sourceRecoveryIntent && (
              <button
                type="button"
                className="bg-transparent border-0 text-inherit underline cursor-pointer py-(--space-1) px-(--space-2) text-xs"
                onClick={clearSourceRecoveryIntent}
              >
                Dismiss
              </button>
            )}
          </div>
        </div>
      )}

      {/* Search input */}
      <div className="relative pt-0 pb-(--space-4) px-(--space-4)">
        <input
          ref={searchInputRef}
          type="text"
          className="w-full py-(--space-3) pr-[28px] pl-[10px] bg-bg-tertiary border border-border-color rounded-sm text-text-primary text-sm outline-none box-border placeholder:text-text-muted focus:border-accent-blue"
          aria-label={t("settings.audioSources.searchLabel")}
          placeholder={t("settings.audioSources.searchPlaceholder")}
          value={searchFilter}
          onChange={(e) => setSearchFilter(e.target.value)}
        />
        {searchFilter && (
          <IconButton
            icon="close"
            className="absolute right-[14px] top-1/2 -translate-y-1/2 bg-none border-none text-text-muted cursor-pointer text-sm py-(--space-1) px-(--space-2) leading-none hover:text-text-primary"
            onClick={() => setSearchFilter("")}
            variant="ghost"
            label={t("settings.audioSources.clearSearch")}
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
          title={t("settings.audioSources.scopeAudioHint")}
        >
          <Icon name="speaker" size={14} /> Audio apps
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={processScope === "all"}
          className={`flex-1 text-xs font-semibold py-(--space-2) px-(--space-3) rounded-md border cursor-pointer whitespace-nowrap ${processScope === "all" ? "bg-bg-elevated text-accent border-accent" : "border-border-color bg-transparent text-text-muted hover:text-text-primary"}`}
          onClick={() => setScope("all")}
          title={t("settings.audioSources.scopeAllHint")}
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
            className="bg-none border border-accent-blue text-accent-blue rounded-sm py-(--space-2) px-(--space-5) text-sm cursor-pointer transition-[background-color] duration-[150ms] ease-[ease] hover:bg-(--tint-accent-info-strong)"
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
                  title={
                    isCollapsed
                      ? t("settings.audioSources.expandGroup", { label })
                      : t("settings.audioSources.collapseGroup", { label })
                  }
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
                      const captureTargetId = sourceCaptureTargetId(source);
                      const selected = isSourceSelected(
                        source,
                        captureTargetId,
                      );
                      const modeLabel = captureTargetModeLabel(captureTargetId);
                      const metadataLabel = sourceMetadataLabel(source);
                      const formatLabel = formatDefaultAudioFormat(
                        source.default_format,
                      );
                      const unsupported =
                        source.capabilities?.capture_supported === false;
                      const disabled = isCapturing || unsupported;
                      const disabledReason = isCapturing
                        ? captureLockedMessage
                        : source.capabilities?.unsupported_reason;
                      const recoveryIssue =
                        recoveryIssueBySourceId.get(captureTargetId);
                      const rowReason =
                        disabledReason ?? recoveryIssue?.message;
                      // Mirror the title-only disabled reason (capture-locked,
                      // unsupported) into a visually-hidden span so assistive tech
                      // announces it. Recovery-issue messages are intentionally
                      // skipped here — they are already surfaced in the
                      // role="status" recovery banner above, so duplicating them
                      // would double-announce. (House pattern: ControlBar.tsx.)
                      const rowReasonId = disabledReason
                        ? `${reasonIdPrefix}-source-${source.id}`
                        : undefined;
                      return (
                        // A native checkbox input cannot render the custom row
                        // layout (icon, name, badges); role keeps it accessible.
                        // biome-ignore lint/a11y/useSemanticElements: see comment above
                        <div
                          key={source.id}
                          role="checkbox"
                          aria-checked={selected}
                          aria-disabled={disabled}
                          aria-describedby={rowReasonId}
                          tabIndex={0}
                          className={`${sourceItem} ${selected ? "bg-(--tint-success)" : ""} ${recoveryIssue ? "bg-(--tint-warning)" : ""} ${disabled ? "opacity-60 cursor-not-allowed" : selected ? "cursor-pointer hover:bg-(--tint-success-strong)" : "cursor-pointer hover:bg-(--hover-overlay)"}`}
                          style={
                            recoveryIssue
                              ? {
                                  boxShadow:
                                    "inset 0 0 0 1px var(--tint-border-warning)",
                                }
                              : undefined
                          }
                          onClick={() => {
                            if (!disabled) handleToggle(captureTargetId);
                          }}
                          onKeyDown={(e) => {
                            if (
                              disabled &&
                              (e.key === "Enter" || e.key === " ")
                            ) {
                              e.preventDefault();
                              return;
                            }
                            handleKeyDown(e, captureTargetId);
                          }}
                          title={rowReason}
                        >
                          {rowReasonId && (
                            <span id={rowReasonId} className="sr-only">
                              {rowReason}
                            </span>
                          )}
                          <span
                            className={`w-[14px] h-[14px] rounded-[3px] border-2 shrink-0 relative transition-[border-color,background-color] duration-[120ms] ease-[ease] ${selected ? "border-accent-green bg-accent-green after:content-[''] after:absolute after:top-px after:left-(--space-2) after:w-(--space-2) after:h-[7px] after:border-solid after:border-(--on-accent-green) after:border-[0_2px_2px_0] after:rotate-45" : "border-text-muted"}`}
                          />
                          <span className="flex-1 overflow-hidden text-ellipsis whitespace-nowrap">
                            {source.name}
                          </span>
                          {metadataLabel && (
                            <span className="text-2xs font-mono bg-(--hover-overlay) text-text-muted py-px px-(--space-3) rounded-[3px] tracking-[0.3px] shrink-0">
                              {metadataLabel}
                            </span>
                          )}
                          {(source.is_default === true ||
                            source.source_type.type === "SystemDefault") && (
                            <span className="text-2xs font-semibold uppercase bg-(--tint-accent-info) text-(--text-on-tint-info) py-px px-(--space-3) rounded-[3px] tracking-[0.3px] shrink-0">
                              Default
                            </span>
                          )}
                          {source.source_type.type !== "SystemDefault" &&
                            modeLabel && (
                              <span className="text-2xs font-semibold uppercase bg-(--tint-accent-info) text-(--text-on-tint-info) py-px px-(--space-3) rounded-[3px] tracking-[0.3px] shrink-0">
                                {modeLabel}
                              </span>
                            )}
                          {formatLabel && (
                            <span className="text-2xs font-semibold bg-(--hover-overlay) text-text-secondary py-px px-(--space-3) rounded-[3px] tracking-[0.3px] shrink-0">
                              {formatLabel}
                            </span>
                          )}
                          {unsupported && (
                            <span className="text-2xs font-semibold uppercase bg-(--tint-warning) text-(--text-on-tint-warning) py-px px-(--space-3) rounded-[3px] tracking-[0.3px] shrink-0">
                              Unsupported
                            </span>
                          )}
                          {recoveryIssue?.kind === "permission" && (
                            <span className="text-2xs font-semibold uppercase bg-(--tint-warning) text-(--text-on-tint-warning) py-px px-(--space-3) rounded-[3px] tracking-[0.3px] shrink-0">
                              Permission
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
                      ? t("settings.audioSources.expandGroup", {
                          label: t("settings.audioSources.runningProcesses"),
                        })
                      : t("settings.audioSources.collapseGroup", {
                          label: t("settings.audioSources.runningProcesses"),
                        })
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
                      const processUnsupportedReason =
                        processControlCapability.processUnsupportedReason;
                      const processTreeUnsupportedReason =
                        processControlCapability.processTreeUnsupportedReason;
                      const processDisabled = Boolean(
                        isCapturing || (processUnsupportedReason && !selected),
                      );
                      const treeDisabled = Boolean(
                        isCapturing ||
                          (processTreeUnsupportedReason && !treeSelected),
                      );
                      const activeMode = treeSelected
                        ? "Process tree"
                        : selected
                          ? "Process"
                          : "Not selected";
                      const rowToggleId = treeSelected
                        ? processTreeId
                        : selected
                          ? processId
                          : processDisabled && !treeDisabled
                            ? processTreeId
                            : processId;
                      const rowUnsupportedReason =
                        rowToggleId === processTreeId
                          ? processTreeUnsupportedReason
                          : processUnsupportedReason;
                      const rowDisabled = Boolean(
                        isCapturing ||
                          (rowUnsupportedReason && !selected && !treeSelected),
                      );
                      const rowTitle = isCapturing
                        ? captureLockedMessage
                        : rowUnsupportedReason && !selected && !treeSelected
                          ? rowUnsupportedReason
                          : `${activeMode}: ${proc.name}`;
                      const processTitle = isCapturing
                        ? captureLockedMessage
                        : processUnsupportedReason && !selected
                          ? processUnsupportedReason
                          : `Capture only ${proc.name}`;
                      const treeTitle = isCapturing
                        ? captureLockedMessage
                        : processTreeUnsupportedReason && !treeSelected
                          ? processTreeUnsupportedReason
                          : `Capture ${proc.name} and child processes`;
                      const rowReasonId = `${reasonIdPrefix}-proc-${proc.pid}`;
                      const processReasonId = `${reasonIdPrefix}-proc-${proc.pid}-only`;
                      const treeReasonId = `${reasonIdPrefix}-proc-${proc.pid}-tree`;
                      return (
                        // A native checkbox input cannot render the custom row
                        // layout (icon, name, PID, mode buttons); role keeps it
                        // accessible.
                        // biome-ignore lint/a11y/useSemanticElements: see comment above
                        <div
                          key={proc.pid}
                          role="checkbox"
                          aria-checked={selected || treeSelected}
                          aria-disabled={rowDisabled}
                          aria-describedby={rowReasonId}
                          tabIndex={0}
                          className={`${sourceItem} ${selected || treeSelected ? "bg-(--tint-success)" : ""} ${rowDisabled ? "opacity-60 cursor-not-allowed" : selected || treeSelected ? "cursor-pointer hover:bg-(--tint-success-strong)" : "cursor-pointer hover:bg-(--hover-overlay)"}`}
                          onClick={() => {
                            if (!rowDisabled) handleToggle(rowToggleId);
                          }}
                          onKeyDown={(e) => {
                            if (!rowDisabled) handleKeyDown(e, rowToggleId);
                          }}
                          title={rowTitle}
                        >
                          <span id={rowReasonId} className="sr-only">
                            {rowTitle}
                          </span>
                          <span
                            className={`w-[14px] h-[14px] rounded-[3px] border-2 shrink-0 relative transition-[border-color,background-color] duration-[120ms] ease-[ease] ${selected || treeSelected ? "border-accent-green bg-accent-green after:content-[''] after:absolute after:top-px after:left-(--space-2) after:w-(--space-2) after:h-[7px] after:border-solid after:border-(--on-accent-green) after:border-[0_2px_2px_0] after:rotate-45" : "border-text-muted"}`}
                          />
                          <span className="flex-1 overflow-hidden text-ellipsis whitespace-nowrap">
                            {proc.name}
                          </span>
                          <span className="text-2xs text-text-muted font-mono whitespace-nowrap">
                            PID {proc.pid}
                          </span>
                          <button
                            type="button"
                            className={`border rounded-[3px] py-px px-(--space-3) text-2xs leading-[16px] min-w-[42px] text-center whitespace-nowrap cursor-pointer shrink-0 disabled:cursor-not-allowed disabled:opacity-60 ${selected ? "border-accent-green bg-(--tint-success) text-accent-green" : "border-border-color bg-(--hover-overlay) text-text-secondary enabled:hover:border-accent-blue enabled:hover:text-text-primary"}`}
                            disabled={processDisabled}
                            title={processTitle}
                            aria-pressed={selected}
                            aria-describedby={processReasonId}
                            onClick={(e) => {
                              e.stopPropagation();
                              handleToggle(processId);
                            }}
                          >
                            Process
                          </button>
                          <span id={processReasonId} className="sr-only">
                            {processTitle}
                          </span>
                          <button
                            type="button"
                            className={`border rounded-[3px] py-px px-(--space-3) text-2xs leading-[16px] min-w-[42px] text-center whitespace-nowrap cursor-pointer shrink-0 disabled:cursor-not-allowed disabled:opacity-60 ${treeSelected ? "border-accent-green bg-(--tint-success) text-accent-green" : "border-border-color bg-(--hover-overlay) text-text-secondary enabled:hover:border-accent-blue enabled:hover:text-text-primary"}`}
                            disabled={treeDisabled}
                            title={treeTitle}
                            aria-pressed={treeSelected}
                            aria-describedby={treeReasonId}
                            onClick={(e) => {
                              e.stopPropagation();
                              handleToggle(processTreeId);
                            }}
                          >
                            Tree
                          </button>
                          <span id={treeReasonId} className="sr-only">
                            {treeTitle}
                          </span>
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
