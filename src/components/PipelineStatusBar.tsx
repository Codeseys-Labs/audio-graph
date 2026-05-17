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
import { useAudioGraphStore } from "../store";
import type { PipelineLatencyEvent, StageStatus } from "../types";

/** Pipeline stages in processing order, with icons. */
const PIPELINE_STAGES = [
  { key: "capture" as const, name: "Capture", icon: "🎙️" },
  { key: "pipeline" as const, name: "Resample", icon: "🔄" },
  { key: "asr" as const, name: "ASR", icon: "📝" },
  { key: "diarization" as const, name: "Diarization", icon: "👥" },
  { key: "entity_extraction" as const, name: "Extraction", icon: "🔍" },
  { key: "graph" as const, name: "Graph", icon: "🕸️" },
] as const;

/** Map StageStatus to a CSS modifier and tooltip. */
function stageStatusInfo(status: StageStatus): {
  modifier: string;
  tooltip: string;
} {
  switch (status.type) {
    case "Idle":
      return { modifier: "idle", tooltip: "Idle" };
    case "Running":
      return {
        modifier: "running",
        tooltip: `Running — ${status.processed_count} processed`,
      };
    case "Error":
      return { modifier: "error", tooltip: `Error: ${status.message}` };
  }
}

/** Format a latency sample for compact display in the 32px status bar. */
function formatLatency(sample: PipelineLatencyEvent | undefined): string | null {
  if (!sample || !Number.isFinite(sample.latency_ms)) return null;
  if (sample.latency_ms >= 1000) {
    return `${(sample.latency_ms / 1000).toFixed(1)}s`;
  }
  return `${Math.round(sample.latency_ms)}ms`;
}

function PipelineStatusBar() {
  const pipelineStatus = useAudioGraphStore((s) => s.pipelineStatus);
  const pipelineLatencies = useAudioGraphStore((s) => s.pipelineLatencies);

  return (
    <nav
      className="pipeline-status"
      aria-label="Pipeline status"
      role="status"
    >
      {PIPELINE_STAGES.map((stage, idx) => {
        const status = pipelineStatus[stage.key];
        const latency = formatLatency(pipelineLatencies[stage.key]);
        const info = stageStatusInfo(status);
        const tooltip = latency
          ? `${info.tooltip} • last latency ${latency}`
          : info.tooltip;

        return (
          <div key={stage.key} className="pipeline-stage__wrapper">
            {idx > 0 && (
              <span className="pipeline-stage__arrow" aria-hidden="true">
                →
              </span>
            )}
            <div className="pipeline-stage" title={tooltip}>
              <span className="pipeline-stage__icon" aria-hidden="true">
                {stage.icon}
              </span>
              <span className="pipeline-stage__name">{stage.name}</span>
              {latency && (
                <span className="pipeline-stage__latency" aria-label={`${stage.name} last latency ${latency}`}>
                  {latency}
                </span>
              )}
              <span
                className={`pipeline-stage__dot pipeline-stage__dot--${info.modifier}`}
                aria-label={`${stage.name}: ${tooltip}`}
              />
            </div>
          </div>
        );
      })}
    </nav>
  );
}

export default PipelineStatusBar;
