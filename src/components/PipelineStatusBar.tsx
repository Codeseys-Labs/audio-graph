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
import type {
  PersistenceQueueBackpressurePayload,
  PipelineLatencyEvent,
  ProcessedAudioConsumerHealthPayload,
  StageStatus,
} from "../types";
import Icon, { type IconName } from "./Icon";
import Tooltip from "./Tooltip";

// Tailwind utility groups (ADR-0016), faithfully translated from the former
// pipeline-status.css module. Colors/borders resolve through design tokens via
// the @theme bridge; spacing uses the token shorthand where it maps to scale.
const STAGE_BASE =
  "flex items-center gap-(--space-2) py-(--space-1) px-(--space-3) rounded-sm cursor-default transition-colors duration-[120ms] hover:bg-(--hover-overlay)";
const STAGE_NAME = "text-text-secondary text-[11px] font-medium";
const STAGE_LATENCY = "text-text-muted text-[10px] tabular-nums ml-[1px]";
const DOT_BASE =
  "w-[8px] h-[8px] rounded-full shrink-0 transition-[background-color,box-shadow] duration-200";

/** Dot modifier classes, keyed by StageStatus modifier. */
const DOT_MODIFIER: Record<string, string> = {
  running: "bg-accent-green shadow-[0_0_6px_var(--accent-green)]",
  idle: "bg-text-muted",
  warning: "bg-accent-yellow shadow-[0_0_6px_var(--accent-yellow)]",
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

function summarizeConsumerHealth(
  payload: ProcessedAudioConsumerHealthPayload | null,
): {
  active: number;
  total: number;
  queued: number;
  capacity: number | null;
  dropped: number;
} | null {
  if (!payload || payload.consumers.length === 0) return null;
  let capacity = 0;
  let hasCapacity = false;
  for (const consumer of payload.consumers) {
    if (typeof consumer.queue_capacity === "number") {
      capacity += consumer.queue_capacity;
      hasCapacity = true;
    }
  }
  return {
    active: payload.consumers.filter((consumer) => consumer.active).length,
    total: payload.consumers.length,
    queued: payload.consumers.reduce(
      (sum, consumer) => sum + consumer.queue_len,
      0,
    ),
    capacity: hasCapacity ? capacity : null,
    dropped: payload.consumers.reduce(
      (sum, consumer) => sum + consumer.dropped_chunks,
      0,
    ),
  };
}

function persistenceWriterLabel(writer: string, t: TFunction): string {
  switch (writer) {
    case "transcript_event":
      return t("pipeline.persistenceWriterTranscript");
    case "projection_event":
      return t("pipeline.persistenceWriterProjection");
    default:
      return writer;
  }
}

function summarizePersistenceQueues(
  payloads: PersistenceQueueBackpressurePayload[],
  t: TFunction,
): {
  writers: string;
  dropped: number;
  capacity: number;
} | null {
  if (payloads.length === 0) return null;
  return {
    writers: payloads
      .map((payload) => persistenceWriterLabel(payload.writer, t))
      .join(", "),
    dropped: payloads.reduce((sum, payload) => sum + payload.dropped_count, 0),
    capacity: payloads.reduce(
      (sum, payload) => sum + payload.queue_capacity,
      0,
    ),
  };
}

function PipelineStatusBar() {
  const { t } = useTranslation();
  const pipelineStatus = useAudioGraphStore((s) => s.pipelineStatus);
  const pipelineLatencies = useAudioGraphStore((s) => s.pipelineLatencies);
  const consumerHealth = useAudioGraphStore((s) => s.latestAudioConsumerHealth);
  const persistenceQueueBackpressure = useAudioGraphStore(
    (s) => s.persistenceQueueBackpressure,
  );
  const lastTurnEvent = useAudioGraphStore((s) =>
    s.turnEvents.length > 0 ? s.turnEvents[s.turnEvents.length - 1] : null,
  );
  const turnLabel = lastTurnEvent
    ? `${lastTurnEvent.provider}: ${lastTurnEvent.kind.split("_").join(" ")}`
    : null;
  const consumerSummary = summarizeConsumerHealth(consumerHealth);
  const consumerQueueLabel = consumerSummary
    ? consumerSummary.capacity === null
      ? String(consumerSummary.queued)
      : `${consumerSummary.queued}/${consumerSummary.capacity}`
    : null;
  const persistenceQueueSummary = summarizePersistenceQueues(
    Object.values(persistenceQueueBackpressure),
    t,
  );

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
                  // The visible "120ms" stays readable; a visually-hidden
                  // sibling carries the full "<stage> latency 120ms" so screen
                  // readers get context without putting role="img" on a text
                  // node (A11Y-1). The status dot below KEEPS role="img"
                  // because it is empty/color-only and genuinely needs a name.
                  <span className={STAGE_LATENCY}>
                    <span aria-hidden="true">{latency}</span>
                    <span className="sr-only">
                      {t("pipeline.stageLatency", {
                        stage: stageName,
                        latency,
                      })}
                    </span>
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
      {consumerSummary && (
        <>
          <span
            className="text-border-color text-[14px] mx-(--space-2) opacity-60"
            aria-hidden="true"
          >
            |
          </span>
          <Tooltip
            content={t("pipeline.audioConsumersTooltip", {
              active: consumerSummary.active,
              total: consumerSummary.total,
              queue: consumerQueueLabel,
              dropped: consumerSummary.dropped,
            })}
          >
            <div className={STAGE_BASE}>
              <span className="text-[12px] leading-none" aria-hidden="true">
                <Icon name="headphones" size={16} />
              </span>
              <span className={STAGE_NAME}>{t("pipeline.audioConsumers")}</span>
              <span className={STAGE_LATENCY}>
                {t("pipeline.audioConsumersCompact", {
                  active: consumerSummary.active,
                  total: consumerSummary.total,
                  queue: consumerQueueLabel,
                  dropped: consumerSummary.dropped,
                })}
              </span>
              <span
                className={`${DOT_BASE} ${
                  consumerSummary.dropped > 0
                    ? DOT_MODIFIER.warning
                    : consumerSummary.active > 0
                      ? DOT_MODIFIER.running
                      : DOT_MODIFIER.idle
                }`}
                role="img"
                aria-label={t("pipeline.audioConsumersTooltip", {
                  active: consumerSummary.active,
                  total: consumerSummary.total,
                  queue: consumerQueueLabel,
                  dropped: consumerSummary.dropped,
                })}
              />
            </div>
          </Tooltip>
        </>
      )}
      {persistenceQueueSummary && (
        <>
          <span
            className="text-border-color text-[14px] mx-(--space-2) opacity-60"
            aria-hidden="true"
          >
            |
          </span>
          <Tooltip
            content={t("pipeline.persistenceQueueTooltip", {
              writers: persistenceQueueSummary.writers,
              dropped: persistenceQueueSummary.dropped,
              capacity: persistenceQueueSummary.capacity,
            })}
          >
            <div className={STAGE_BASE}>
              <span
                className="text-[12px] leading-none text-accent-yellow"
                aria-hidden="true"
              >
                <Icon name="warning" size={16} />
              </span>
              <span className={STAGE_NAME}>
                {t("pipeline.persistenceQueue")}
              </span>
              <span className={STAGE_LATENCY}>
                {t("pipeline.persistenceQueueCompact", {
                  dropped: persistenceQueueSummary.dropped,
                })}
              </span>
              <span
                className={`${DOT_BASE} ${DOT_MODIFIER.warning}`}
                role="img"
                aria-label={t("pipeline.persistenceQueueTooltip", {
                  writers: persistenceQueueSummary.writers,
                  dropped: persistenceQueueSummary.dropped,
                  capacity: persistenceQueueSummary.capacity,
                })}
              />
            </div>
          </Tooltip>
        </>
      )}
    </nav>
  );
}

export default PipelineStatusBar;
