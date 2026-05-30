/**
 * Bottom status bar — one dot per pipeline stage showing Idle / Running /
 * Error, fed by the `PIPELINE_STATUS_EVENT` backend event.
 *
 * Stages (in processing order): Capture → Resample → ASR → Diarization →
 * Extraction → Graph. Each stage shows an icon, label, and a coloured dot
 * whose modifier class is derived from `StageStatus.type`. The tooltip
 * surfaces the processed-count (Running) or error message (Error).
 *
 * Store bindings: `pipelineStatus` (the full `PipelineStatus` payload from
 * Rust).
 *
 * Parent: `App.tsx` (bottom of layout). No props — purely reflective.
 */
import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";
import { useAudioGraphStore } from "../store";
import type { PipelineLatencyEvent, StageStatus } from "../types";
import Icon, { type IconName } from "./Icon";
import Tooltip from "./Tooltip";

// Tailwind utility groups (ADR-0016), faithfully translated from the former
// pipeline-status.css module. Colors/borders resolve through design tokens via
// the @theme bridge; spacing uses the token shorthand where it maps to scale.
const STAGE_BASE =
  "flex items-center gap-(--space-2) py-(--space-1) px-(--space-3) rounded-[4px] cursor-default transition-colors duration-[120ms] hover:bg-(--hover-overlay)";
const STAGE_NAME = "text-text-secondary text-[11px] font-medium";
const STAGE_LATENCY = "text-text-muted text-[10px] tabular-nums ml-[1px]";
const DOT_BASE =
  "w-[8px] h-[8px] rounded-full shrink-0 transition-[background-color,box-shadow] duration-200";

/** Dot modifier classes, keyed by StageStatus modifier. */
const DOT_MODIFIER: Record<string, string> = {
  running: "bg-accent-green shadow-[0_0_6px_var(--accent-green)]",
  idle: "bg-text-muted",
  error: "bg-accent-red shadow-[0_0_6px_var(--accent-red)]",
};

/** Pipeline stages in processing order, with icons and i18n label keys. */
const PIPELINE_STAGES = [
  {
    key: "capture" as const,
    labelKey: "pipeline.stageCapture",
    icon: "mic" as IconName,
  },
  {
    key: "pipeline" as const,
    labelKey: "pipeline.stageResample",
    icon: "resample" as IconName,
  },
  {
    key: "asr" as const,
    labelKey: "pipeline.stageAsr",
    icon: "transcript" as IconName,
  },
  {
    key: "diarization" as const,
    labelKey: "pipeline.stageDiarization",
    icon: "diarization" as IconName,
  },
  {
    key: "entity_extraction" as const,
    labelKey: "pipeline.stageExtraction",
    icon: "extraction" as IconName,
  },
  {
    key: "graph" as const,
    labelKey: "pipeline.stageGraph",
    icon: "graph" as IconName,
  },
] as const;

/** Map StageStatus to a CSS modifier and tooltip. */
function stageStatusInfo(
  status: StageStatus,
  t: TFunction,
): {
  modifier: string;
  tooltip: string;
} {
  switch (status.type) {
    case "Idle":
      return { modifier: "idle", tooltip: t("pipeline.statusIdle") };
    case "Running":
      return {
        modifier: "running",
        tooltip: t("pipeline.statusRunning", {
          count: status.processed_count,
        }),
      };
    case "Error":
      return {
        modifier: "error",
        tooltip: t("pipeline.statusError", { message: status.message }),
      };
  }
}

/** Format a latency sample for compact display in the 32px status bar. */
function formatLatency(
  sample: PipelineLatencyEvent | undefined,
): string | null {
  if (!sample || !Number.isFinite(sample.latency_ms)) return null;
  if (sample.latency_ms >= 1000) {
    return `${(sample.latency_ms / 1000).toFixed(1)}s`;
  }
  return `${Math.round(sample.latency_ms)}ms`;
}

function PipelineStatusBar() {
  const { t } = useTranslation();
  const pipelineStatus = useAudioGraphStore((s) => s.pipelineStatus);
  const pipelineLatencies = useAudioGraphStore((s) => s.pipelineLatencies);
  const lastTurnEvent = useAudioGraphStore((s) =>
    s.turnEvents.length > 0 ? s.turnEvents[s.turnEvents.length - 1] : null,
  );
  const turnLabel = lastTurnEvent
    ? `${lastTurnEvent.provider}: ${lastTurnEvent.kind.split("_").join(" ")}`
    : null;

  return (
    <nav
      className="flex items-center justify-center py-0 px-(--space-6) bg-bg-tertiary border-t border-border-color gap-(--space-1) text-[11px] h-(--space-9) shrink-0 overflow-x-auto whitespace-nowrap [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
      aria-label={t("pipeline.label")}
      role="status"
    >
      {PIPELINE_STAGES.map((stage, idx) => {
        const status = pipelineStatus[stage.key];
        const latency = formatLatency(pipelineLatencies[stage.key]);
        const info = stageStatusInfo(status, t);
        const stageName = t(stage.labelKey);
        const tooltip = latency
          ? t("pipeline.tooltipWithLatency", {
              tooltip: info.tooltip,
              latency,
            })
          : info.tooltip;

        return (
          <div key={stage.key} className="flex items-center gap-(--space-1)">
            {idx > 0 && (
              <span
                className="text-text-muted text-[10px] opacity-50 mx-[1px]"
                aria-hidden="true"
              >
                <Icon name="arrowRight" size={14} />
              </span>
            )}
            <Tooltip content={tooltip}>
              <div className={STAGE_BASE}>
                <span className="text-[12px] leading-none" aria-hidden="true">
                  <Icon name={stage.icon} size={16} />
                </span>
                <span className={STAGE_NAME}>{stageName}</span>
                {latency && (
                  <span
                    className={STAGE_LATENCY}
                    role="img"
                    aria-label={t("pipeline.stageLatency", {
                      stage: stageName,
                      latency,
                    })}
                  >
                    {latency}
                  </span>
                )}
                <span
                  className={`${DOT_BASE} ${DOT_MODIFIER[info.modifier]}`}
                  role="img"
                  aria-label={t("pipeline.stageStatus", {
                    stage: stageName,
                    tooltip,
                  })}
                />
              </div>
            </Tooltip>
          </div>
        );
      })}
      {turnLabel && (
        <>
          <span
            className="text-border-color text-[14px] mx-(--space-2) opacity-60"
            aria-hidden="true"
          >
            |
          </span>
          <div
            className={`${STAGE_BASE} pipeline-stage--turn`}
            title={t("pipeline.lastTurnEvent", { label: turnLabel })}
          >
            <span className={STAGE_NAME}>{t("pipeline.turn")}</span>
            <span className={STAGE_LATENCY}>{turnLabel}</span>
            <span
              className={`${DOT_BASE} ${DOT_MODIFIER.running}`}
              role="img"
              aria-label={t("pipeline.lastTurnEvent", { label: turnLabel })}
            />
          </div>
        </>
      )}
    </nav>
  );
}

export default PipelineStatusBar;
