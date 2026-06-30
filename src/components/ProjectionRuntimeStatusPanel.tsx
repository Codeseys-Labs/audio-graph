import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useAudioGraphStore } from "../store";
import type {
  ProjectionOperation,
  ProjectionPatch,
  ProjectionReplayArtifactReport,
  ProjectionReplayReport,
  ProjectionRuntimeStatus,
  ProjectionSchedulerTelemetry,
} from "../types";
import { errorToMessage } from "../utils/errorToMessage";
import Icon from "./Icon";

type LoadState = "idle" | "loading" | "ready" | "error";

function formatCount(value: number): string {
  if (!Number.isFinite(value)) return "0";
  return Math.max(0, Math.round(value)).toLocaleString();
}

function formatMs(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return "0ms";
  if (value >= 1000) return `${(value / 1000).toFixed(1)}s`;
  return `${Math.round(value)}ms`;
}

function averageMs(total: number, count: number): number {
  if (!Number.isFinite(total) || !Number.isFinite(count) || count <= 0) {
    return 0;
  }
  return total / count;
}

function ttftSourceLabel(
  source: ProjectionSchedulerTelemetry["ttft_estimate_source"],
  t: (key: string) => string,
): string {
  return t(`projectionDiagnostics.ttftSource.${source}`);
}

function schedulerState(
  scheduler: ProjectionSchedulerTelemetry,
): "inFlight" | "pending" | "attention" | "ready" | "idle" {
  if (scheduler.in_flight_job_id) return "inFlight";
  if (scheduler.pending_span_count > 0) return "pending";
  if (
    scheduler.metrics.failed_jobs > 0 ||
    scheduler.metrics.stale_discards > 0
  ) {
    return "attention";
  }
  if (scheduler.metrics.jobs_started > 0) return "ready";
  return "idle";
}

function isEmpty(status: ProjectionRuntimeStatus): boolean {
  return (
    status.accepted_transcript_event_count === 0 &&
    status.transcript_span_count === 0 &&
    status.materialized.note_count === 0 &&
    status.materialized.graph_node_count === 0 &&
    status.materialized.graph_edge_count === 0 &&
    status.schedulers.notes.metrics.jobs_started === 0 &&
    status.schedulers.graph.metrics.jobs_started === 0
  );
}

interface MetricProps {
  label: string;
  value: string | number;
}

function Metric({ label, value }: MetricProps) {
  return (
    <div className="min-w-0">
      <dt className="m-0 text-[9px] font-semibold uppercase tracking-[0.4px] text-text-muted leading-[1.2]">
        {label}
      </dt>
      <dd className="m-0 mt-[2px] font-mono text-xs font-semibold text-text-primary leading-tight overflow-hidden text-ellipsis whitespace-nowrap">
        {value}
      </dd>
    </div>
  );
}

interface ReplayArtifactRowProps {
  label: string;
  artifact: ProjectionReplayArtifactReport;
}

interface GraphOperationSummary {
  key: string;
  sequence: number;
  title: string;
  detail: string;
}

type GraphProjectionOperation = Extract<
  ProjectionOperation,
  | { type: "upsert_graph_node" }
  | { type: "remove_graph_node" }
  | { type: "invalidate_graph_node" }
  | { type: "upsert_graph_edge" }
  | { type: "remove_graph_edge" }
  | { type: "invalidate_graph_edge" }
  | { type: "strengthen_graph_edge" }
  | { type: "weaken_graph_edge" }
  | { type: "merge_graph_nodes" }
  | { type: "split_graph_node" }
>;

function isGraphProjectionOperation(
  operation: ProjectionOperation,
): operation is GraphProjectionOperation {
  switch (operation.type) {
    case "upsert_note":
    case "delete_note":
    case "reorder_note":
      return false;
    default:
      return true;
  }
}

function graphOperationTitle(
  operation: GraphProjectionOperation,
  t: (key: string, options?: Record<string, unknown>) => string,
): string {
  return t(`projectionDiagnostics.graphOperation.${operation.type}`);
}

function graphOperationDetail(
  operation: GraphProjectionOperation,
  t: (key: string, options?: Record<string, unknown>) => string,
): string {
  switch (operation.type) {
    case "upsert_graph_node":
      return t("projectionDiagnostics.graphOperationDetail.node", {
        id: operation.id,
        name: operation.name,
      });
    case "remove_graph_node":
    case "invalidate_graph_node":
      return t("projectionDiagnostics.graphOperationDetail.id", {
        id: operation.id,
      });
    case "upsert_graph_edge":
      return t("projectionDiagnostics.graphOperationDetail.edge", {
        id: operation.id,
        source: operation.source,
        target: operation.target,
      });
    case "remove_graph_edge":
    case "invalidate_graph_edge":
      return t("projectionDiagnostics.graphOperationDetail.id", {
        id: operation.id,
      });
    case "strengthen_graph_edge":
    case "weaken_graph_edge":
      return t("projectionDiagnostics.graphOperationDetail.edgeDelta", {
        id: operation.id,
        delta: operation.weight_delta.toFixed(2),
      });
    case "merge_graph_nodes":
      return t("projectionDiagnostics.graphOperationDetail.merge", {
        source: operation.source_id,
        target: operation.target_id,
      });
    case "split_graph_node":
      return t("projectionDiagnostics.graphOperationDetail.split", {
        id: operation.id,
        replacements: operation.replacement_nodes
          .map((replacement) => replacement.id)
          .join(", "),
      });
  }
  const exhaustive: never = operation;
  return exhaustive;
}

function graphOperationSummaries(
  patches: ProjectionPatch[],
  t: (key: string, options?: Record<string, unknown>) => string,
): GraphOperationSummary[] {
  const summaries: GraphOperationSummary[] = [];
  for (const patch of patches) {
    if (patch.kind !== "graph") continue;
    patch.operations.forEach((operation, operationIndex) => {
      if (!isGraphProjectionOperation(operation)) return;
      summaries.push({
        key: `${patch.sequence}:${operationIndex}:${operation.type}`,
        sequence: patch.sequence,
        title: graphOperationTitle(operation, t),
        detail: graphOperationDetail(operation, t),
      });
    });
  }
  return summaries.slice(-8).reverse();
}

function ReplayArtifactRow({ label, artifact }: ReplayArtifactRowProps) {
  const { t } = useTranslation();
  const tone =
    artifact.status === "current"
      ? "text-accent-green bg-(--tint-success)"
      : artifact.status === "missing"
        ? "text-text-muted bg-(--hover-overlay)"
        : "text-accent-yellow bg-(--tint-warning)";

  return (
    <div className="rounded-md border border-border-color bg-bg-tertiary px-(--space-3) py-(--space-2)">
      <div className="mb-(--space-1) flex items-center justify-between gap-(--space-2)">
        <span className="min-w-0 text-xs font-semibold text-text-primary">
          {label}
        </span>
        <span
          className={`shrink-0 rounded-xl px-(--space-3) py-px text-[9px] font-semibold uppercase tracking-[0.3px] ${tone}`}
        >
          {t(`projectionDiagnostics.replayStatus.${artifact.status}`)}
        </span>
      </div>
      <p className="m-0 text-2xs text-text-secondary leading-[1.35]">
        {t("projectionDiagnostics.replayArtifactDetail", {
          storedSeq: formatCount(artifact.stored_last_sequence),
          replayedSeq: formatCount(artifact.replayed_last_sequence),
          storedItems: formatCount(artifact.stored_item_count),
          replayedItems: formatCount(artifact.replayed_item_count),
        })}
      </p>
    </div>
  );
}

interface ReplayReportCardProps {
  report: ProjectionReplayReport | null;
  loadState: LoadState;
  error: string | null;
  onRun: () => void;
}

function ReplayReportCard({
  report,
  loadState,
  error,
  onRun,
}: ReplayReportCardProps) {
  const { t } = useTranslation();

  return (
    <article
      className="border border-border-color rounded-md bg-bg-secondary p-(--space-4)"
      aria-label={t("projectionDiagnostics.replayTitle")}
    >
      <div className="mb-(--space-3) flex items-center justify-between gap-(--space-3)">
        <h4 className="m-0 flex items-center gap-(--space-2) text-xs font-semibold text-text-primary">
          <Icon name="refresh" size={14} />
          {t("projectionDiagnostics.replayTitle")}
        </h4>
        <button
          type="button"
          className="inline-flex items-center gap-(--space-2) rounded-md border border-border-color bg-(--hover-overlay) px-(--space-3) py-[2px] text-2xs font-semibold uppercase tracking-[0.3px] text-text-secondary leading-[1.3] cursor-pointer transition-colors hover:not-disabled:border-(--tint-border-accent-info) hover:not-disabled:bg-(--tint-accent-info-hover) hover:not-disabled:text-(--text-on-tint-info) disabled:cursor-not-allowed disabled:opacity-45"
          disabled={loadState === "loading"}
          onClick={onRun}
          aria-label={t("projectionDiagnostics.replayCheckLabel")}
        >
          <Icon name="refresh" size={12} />
          {loadState === "loading"
            ? t("projectionDiagnostics.replayChecking")
            : t("projectionDiagnostics.replayCheck")}
        </button>
      </div>

      {loadState === "idle" && !report && (
        <p className="m-0 text-xs italic text-text-muted leading-[1.4]">
          {t("projectionDiagnostics.replayNotChecked")}
        </p>
      )}

      {loadState === "error" && (
        <p className="m-0 text-xs text-accent-red leading-[1.4]" role="alert">
          {t("projectionDiagnostics.replayUnavailable", { message: error })}
        </p>
      )}

      {report && (
        <div className="flex flex-col gap-(--space-3)">
          <dl className="grid grid-cols-2 md:grid-cols-4 gap-x-(--space-4) gap-y-(--space-3) m-0">
            <Metric
              label={t("projectionDiagnostics.transcriptEvents")}
              value={formatCount(report.transcript_event_count)}
            />
            <Metric
              label={t("projectionDiagnostics.projectionPatches")}
              value={formatCount(report.projection_event_count)}
            />
            <Metric
              label={t("projectionDiagnostics.replayedSpans")}
              value={formatCount(report.transcript_span_count)}
            />
            <Metric
              label={t("projectionDiagnostics.invalidBasis")}
              value={formatCount(report.projection_invalid_basis_count)}
            />
          </dl>

          {(report.transcript_replay_error ||
            report.projection_replay_error) && (
            <div className="rounded-sm border border-(--tint-border-warning) bg-(--tint-warning) px-(--space-3) py-(--space-2) text-xs text-accent-yellow leading-[1.35]">
              {report.transcript_replay_error && (
                <p className="m-0 [overflow-wrap:anywhere]">
                  {t("projectionDiagnostics.transcriptReplayError", {
                    message: report.transcript_replay_error,
                  })}
                </p>
              )}
              {report.projection_replay_error && (
                <p className="m-0 [overflow-wrap:anywhere]">
                  {t("projectionDiagnostics.projectionReplayError", {
                    message: report.projection_replay_error,
                  })}
                </p>
              )}
            </div>
          )}

          <div className="grid grid-cols-1 gap-(--space-2)">
            <ReplayArtifactRow
              label={t("projectionDiagnostics.notesArtifact")}
              artifact={report.notes_artifact}
            />
            <ReplayArtifactRow
              label={t("projectionDiagnostics.graphArtifact")}
              artifact={report.graph_artifact}
            />
          </div>

          <dl
            className="grid grid-cols-2 md:grid-cols-4 gap-x-(--space-4) gap-y-(--space-3) m-0 rounded-md border border-border-color bg-bg-tertiary px-(--space-3) py-(--space-2)"
            aria-label={t("projectionDiagnostics.evalMetricsTitle")}
          >
            <Metric
              label={t("projectionDiagnostics.graphOps")}
              value={formatCount(report.evaluation.graph_operation_count)}
            />
            <Metric
              label={t("projectionDiagnostics.graphRetcons")}
              value={formatCount(
                report.evaluation.graph_retcon_operation_count,
              )}
            />
            <Metric
              label={t("projectionDiagnostics.corrections")}
              value={formatCount(report.evaluation.correction_patch_count)}
            />
            <Metric
              label={t("projectionDiagnostics.staleSkips")}
              value={formatCount(report.evaluation.stale_discard_count)}
            />
            <Metric
              label={t("projectionDiagnostics.invalidatedGraph")}
              value={t("projectionDiagnostics.graphCounts", {
                nodes: formatCount(
                  report.evaluation.invalidated_graph_node_count,
                ),
                edges: formatCount(
                  report.evaluation.invalidated_graph_edge_count,
                ),
              })}
            />
            <Metric
              label={t("projectionDiagnostics.activeGraph")}
              value={t("projectionDiagnostics.graphCounts", {
                nodes: formatCount(report.evaluation.active_graph_node_count),
                edges: formatCount(report.evaluation.active_graph_edge_count),
              })}
            />
            <Metric
              label={t("projectionDiagnostics.duplicateNodes")}
              value={formatCount(
                report.evaluation.duplicate_active_node_key_count,
              )}
            />
            <Metric
              label={t("projectionDiagnostics.duplicateEdges")}
              value={formatCount(
                report.evaluation.duplicate_active_edge_key_count,
              )}
            />
          </dl>

          <dl
            className="grid grid-cols-2 md:grid-cols-4 gap-x-(--space-4) gap-y-(--space-3) m-0 rounded-md border border-border-color bg-bg-tertiary px-(--space-3) py-(--space-2)"
            aria-label={t("projectionDiagnostics.latencyBreakdownTitle")}
          >
            <Metric
              label={t("projectionDiagnostics.asrToPatchAvg")}
              value={formatMs(
                averageMs(
                  report.latency.total_basis_to_patch_lag_ms,
                  report.latency.measured_patch_count,
                ),
              )}
            />
            <Metric
              label={t("projectionDiagnostics.asrToPatchMax")}
              value={formatMs(report.latency.max_basis_to_patch_lag_ms)}
            />
            <Metric
              label={t("projectionDiagnostics.captureAsrMax")}
              value={formatMs(report.latency.capture_asr.max_ms)}
            />
            <Metric
              label={t("projectionDiagnostics.asrToQueueMax")}
              value={formatMs(report.latency.asr_to_queue.max_ms)}
            />
            <Metric
              label={t("projectionDiagnostics.projectionQueueMax")}
              value={formatMs(report.latency.projection_queue.max_ms)}
            />
            <Metric
              label={t("projectionDiagnostics.replayGenerationMax")}
              value={formatMs(report.latency.generation.max_ms)}
            />
            <Metric
              label={t("projectionDiagnostics.replayApplyMax")}
              value={formatMs(report.latency.apply.max_ms)}
            />
            <Metric
              label={t("projectionDiagnostics.measuredPatches")}
              value={formatCount(report.latency.measured_patch_count)}
            />
            <Metric
              label={t("projectionDiagnostics.missingBasisTimestamps")}
              value={formatCount(report.latency.missing_basis_timestamp_count)}
            />
          </dl>
        </div>
      )}
    </article>
  );
}

interface GraphOperationFeedProps {
  patches: ProjectionPatch[];
}

function GraphOperationFeed({ patches }: GraphOperationFeedProps) {
  const { t } = useTranslation();
  const operations = useMemo(
    () => graphOperationSummaries(patches, t),
    [patches, t],
  );

  if (operations.length === 0) return null;

  return (
    <article
      className="border border-border-color rounded-md bg-bg-secondary p-(--space-4)"
      aria-label={t("projectionDiagnostics.graphOperationsTitle")}
    >
      <div className="mb-(--space-3) flex items-center justify-between gap-(--space-3)">
        <h4 className="m-0 flex items-center gap-(--space-2) text-xs font-semibold text-text-primary">
          <Icon name="graph" size={14} />
          {t("projectionDiagnostics.graphOperationsTitle")}
        </h4>
        <span className="shrink-0 rounded-xl bg-(--hover-overlay) px-(--space-3) py-px text-[9px] font-semibold uppercase tracking-[0.3px] text-text-secondary">
          {t("projectionDiagnostics.graphOperationCount", {
            count: operations.length,
          })}
        </span>
      </div>
      <ol className="m-0 flex list-none flex-col gap-(--space-2) p-0">
        {operations.map((operation) => (
          <li
            key={operation.key}
            className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-(--space-3) rounded-sm border border-border-color bg-bg-tertiary px-(--space-3) py-(--space-2)"
          >
            <span className="font-mono text-[10px] font-semibold text-text-muted">
              {t("projectionDiagnostics.graphOperationSequence", {
                sequence: formatCount(operation.sequence),
              })}
            </span>
            <span className="min-w-0">
              <span className="block text-xs font-semibold text-text-primary leading-[1.25]">
                {operation.title}
              </span>
              <span className="block text-2xs text-text-secondary leading-[1.35] [overflow-wrap:anywhere]">
                {operation.detail}
              </span>
            </span>
          </li>
        ))}
      </ol>
    </article>
  );
}

interface SchedulerCardProps {
  title: string;
  scheduler: ProjectionSchedulerTelemetry;
}

function SchedulerCard({ title, scheduler }: SchedulerCardProps) {
  const { t } = useTranslation();
  const state = schedulerState(scheduler);
  const badgeClass =
    state === "attention"
      ? "text-accent-yellow bg-(--tint-warning)"
      : state === "inFlight" || state === "pending"
        ? "text-accent-blue bg-(--tint-accent-info-hover)"
        : "text-text-secondary bg-(--hover-overlay)";

  return (
    <article
      className="border border-border-color rounded-md bg-bg-secondary p-(--space-4)"
      aria-label={title}
    >
      <div className="flex items-center justify-between gap-(--space-3) mb-(--space-3)">
        <h4 className="m-0 flex items-center gap-(--space-2) text-xs font-semibold text-text-primary">
          <Icon
            name={scheduler.kind === "notes" ? "notes" : "graph"}
            size={14}
          />
          {title}
        </h4>
        <span
          className={`shrink-0 rounded-xl px-(--space-3) py-px text-[9px] font-semibold uppercase tracking-[0.3px] ${badgeClass}`}
        >
          {t(`projectionDiagnostics.schedulerState.${state}`)}
        </span>
      </div>
      {scheduler.in_flight_job_id && (
        <p className="m-0 mb-(--space-3) text-2xs text-text-secondary leading-[1.3] [overflow-wrap:anywhere]">
          {t("projectionDiagnostics.inFlightJob", {
            id: scheduler.in_flight_job_id,
            count: scheduler.in_flight_span_count,
          })}
        </p>
      )}
      <dl className="grid grid-cols-3 gap-x-(--space-4) gap-y-(--space-3) m-0">
        <Metric
          label={t("projectionDiagnostics.pending")}
          value={formatCount(scheduler.pending_span_count)}
        />
        <Metric
          label={t("projectionDiagnostics.ttft")}
          value={formatMs(scheduler.ttft_estimate_ms)}
        />
        <Metric
          label={t("projectionDiagnostics.ttftSourceLabel")}
          value={ttftSourceLabel(scheduler.ttft_estimate_source, t)}
        />
        <Metric
          label={t("projectionDiagnostics.queueAge")}
          value={formatMs(scheduler.in_flight_age_ms)}
        />
        <Metric
          label={t("projectionDiagnostics.queueLag")}
          value={formatMs(scheduler.metrics.last_job_lag_ms)}
        />
        <Metric
          label={t("projectionDiagnostics.llmGeneration")}
          value={formatMs(scheduler.metrics.last_generation_latency_ms)}
        />
        <Metric
          label={t("projectionDiagnostics.materializerApply")}
          value={formatMs(scheduler.metrics.last_apply_latency_ms)}
        />
        <Metric
          label={t("projectionDiagnostics.tokens")}
          value={formatCount(scheduler.metrics.tokens_used)}
        />
        <Metric
          label={t("projectionDiagnostics.patches")}
          value={formatCount(scheduler.metrics.accepted_patches)}
        />
        <Metric
          label={t("projectionDiagnostics.completed")}
          value={formatCount(scheduler.metrics.completed_jobs)}
        />
        <Metric
          label={t("projectionDiagnostics.failed")}
          value={formatCount(scheduler.metrics.failed_jobs)}
        />
        <Metric
          label={t("projectionDiagnostics.stale")}
          value={formatCount(scheduler.metrics.stale_discards)}
        />
        <Metric
          label={t("projectionDiagnostics.generationFailures")}
          value={formatCount(scheduler.metrics.generation_failures)}
        />
        <Metric
          label={t("projectionDiagnostics.applyFailures")}
          value={formatCount(scheduler.metrics.apply_failures)}
        />
        <Metric
          label={t("projectionDiagnostics.repairs")}
          value={formatCount(scheduler.metrics.repair_jobs_started)}
        />
        <Metric
          label={t("projectionDiagnostics.followUps")}
          value={formatCount(scheduler.metrics.follow_up_jobs_started)}
        />
        <Metric
          label={t("projectionDiagnostics.coalesced")}
          value={formatCount(scheduler.metrics.coalesced_updates)}
        />
        <Metric
          label={t("projectionDiagnostics.coalescedSpans")}
          value={formatCount(scheduler.metrics.coalesced_span_count)}
        />
      </dl>
    </article>
  );
}

export default function ProjectionRuntimeStatusPanel() {
  const { t } = useTranslation();
  const projectionEvents = useAudioGraphStore((s) => s.sessionProjectionEvents);
  const pipelineLatencies = useAudioGraphStore((s) => s.pipelineLatencies);
  const [status, setStatus] = useState<ProjectionRuntimeStatus | null>(null);
  const [loadState, setLoadState] = useState<LoadState>("idle");
  const [error, setError] = useState<string | null>(null);
  const [replayReport, setReplayReport] =
    useState<ProjectionReplayReport | null>(null);
  const [replayLoadState, setReplayLoadState] = useState<LoadState>("idle");
  const [replayError, setReplayError] = useState<string | null>(null);

  const load = useCallback(async (cancelled?: () => boolean) => {
    setLoadState("loading");
    setError(null);
    try {
      const next = await invoke<ProjectionRuntimeStatus>(
        "get_projection_runtime_status_cmd",
      );
      if (cancelled?.()) return;
      setStatus(next);
      setLoadState("ready");
    } catch (err) {
      if (cancelled?.()) return;
      setError(errorToMessage(err));
      setLoadState("error");
    }
  }, []);

  const runReplayReport = useCallback(async () => {
    if (!status) return;
    setReplayLoadState("loading");
    setReplayError(null);
    try {
      const report = await invoke<ProjectionReplayReport>(
        "get_projection_replay_report_cmd",
        { sessionId: status.session_id },
      );
      setReplayReport(report);
      setReplayLoadState("ready");
    } catch (err) {
      setReplayError(errorToMessage(err));
      setReplayLoadState("error");
    }
  }, [status]);

  useEffect(() => {
    let cancelled = false;
    void load(() => cancelled);
    return () => {
      cancelled = true;
    };
  }, [load]);

  const empty = useMemo(() => (status ? isEmpty(status) : false), [status]);
  const loadingInitial = loadState === "loading" && !status;

  return (
    <section
      className="flex-shrink-0 border-t border-border-color bg-bg-tertiary py-(--space-4) px-(--space-5)"
      aria-label={t("projectionDiagnostics.label")}
      aria-busy={loadState === "loading"}
    >
      <div className="mb-(--space-3) flex items-center justify-between gap-(--space-4)">
        <h3 className="panel-title flex items-center gap-(--space-2)">
          <Icon name="graph" size={15} />
          {t("projectionDiagnostics.title")}
        </h3>
        <button
          type="button"
          className="inline-flex items-center gap-(--space-2) rounded-md border border-border-color bg-(--hover-overlay) px-(--space-4) py-[3px] text-2xs font-semibold uppercase tracking-[0.4px] text-text-secondary leading-[1.3] cursor-pointer transition-colors hover:not-disabled:border-(--tint-border-accent-info) hover:not-disabled:bg-(--tint-accent-info-hover) hover:not-disabled:text-(--text-on-tint-info) disabled:cursor-not-allowed disabled:opacity-45"
          disabled={loadState === "loading"}
          onClick={() => void load()}
          aria-label={t("projectionDiagnostics.refreshLabel")}
        >
          <Icon name="refresh" size={12} />
          {loadState === "loading"
            ? t("projectionDiagnostics.refreshing")
            : t("projectionDiagnostics.refresh")}
        </button>
      </div>

      {loadingInitial && (
        <p className="m-0 text-xs italic text-text-muted leading-[1.4]">
          {t("projectionDiagnostics.loading")}
        </p>
      )}

      {loadState === "error" && (
        <p className="m-0 text-xs text-accent-red leading-[1.4]" role="alert">
          {t("projectionDiagnostics.error", { message: error })}
        </p>
      )}

      {status && (
        <div className="flex flex-col gap-(--space-4)">
          {!status.projection_event_writer_available && (
            <p
              className="m-0 rounded-sm border border-(--tint-border-warning) bg-(--tint-warning) px-(--space-4) py-(--space-3) text-xs text-accent-yellow leading-[1.35]"
              role="status"
            >
              <Icon name="warning" size={14} />{" "}
              {t("projectionDiagnostics.writerUnavailable")}
            </p>
          )}

          {empty ? (
            <p className="m-0 text-xs italic text-text-muted leading-[1.4]">
              {t("projectionDiagnostics.empty")}
            </p>
          ) : null}

          <dl className="grid grid-cols-3 gap-x-(--space-4) gap-y-(--space-3) m-0">
            <Metric
              label={t("projectionDiagnostics.events")}
              value={formatCount(status.accepted_transcript_event_count)}
            />
            <Metric
              label={t("projectionDiagnostics.spans")}
              value={formatCount(status.transcript_span_count)}
            />
            <Metric
              label={t("projectionDiagnostics.captureLatency")}
              value={formatMs(pipelineLatencies.capture?.latency_ms ?? 0)}
            />
            <Metric
              label={t("projectionDiagnostics.asrLatency")}
              value={formatMs(pipelineLatencies.asr?.latency_ms ?? 0)}
            />
            <Metric
              label={t("projectionDiagnostics.asrEventAge")}
              value={formatMs(status.latest_asr_event_age_ms ?? 0)}
            />
            <Metric
              label={t("projectionDiagnostics.notes")}
              value={formatCount(status.materialized.note_count)}
            />
            <Metric
              label={t("projectionDiagnostics.notesSeq")}
              value={formatCount(status.materialized.notes_last_sequence)}
            />
            <Metric
              label={t("projectionDiagnostics.graph")}
              value={t("projectionDiagnostics.graphCounts", {
                nodes: formatCount(status.materialized.graph_node_count),
                edges: formatCount(status.materialized.graph_edge_count),
              })}
            />
            <Metric
              label={t("projectionDiagnostics.graphSeq")}
              value={formatCount(status.materialized.graph_last_sequence)}
            />
          </dl>

          <div className="grid grid-cols-1 gap-(--space-3)">
            <GraphOperationFeed patches={projectionEvents} />
            <ReplayReportCard
              report={replayReport}
              loadState={replayLoadState}
              error={replayError}
              onRun={() => void runReplayReport()}
            />
            <SchedulerCard
              title={t("projectionDiagnostics.notesScheduler")}
              scheduler={status.schedulers.notes}
            />
            <SchedulerCard
              title={t("projectionDiagnostics.graphScheduler")}
              scheduler={status.schedulers.graph}
            />
          </div>
        </div>
      )}
    </section>
  );
}
