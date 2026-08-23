/**
 * Composite pipeline health (SHELL-R3, plan §R3, ADR-0046 — folds
 * `audio-graph-50e3`'s "ambient composite health dot").
 *
 * ONE pure classification shared by two consumers so the "50e3 dot" is
 * genuinely one signal, not two that could drift:
 *   - `NowStrip`'s health chip (opens `SystemDrawer`).
 *   - `PipelineStatusBar`'s footer fold (collapses to the composite state
 *     during healthy capture; per-stage dots return on error).
 *
 * Deliberately narrow: a `Running` stage or a latency/turn-event sample is
 * NOT a problem — only an actual `Error` stage, a `Degraded` stage (honest
 * fallback reporting, e.g. audio-graph-586b's diarization degradation), or
 * queue pressure that is already dropping data (persistence writer
 * backpressure, a processed-audio consumer dropping chunks, or a capture
 * source's ring buffer dropping chunks — the same three signals the
 * pre-fold footer already surfaced separately) counts as `"degraded"`.
 */
import type {
  PersistenceQueueBackpressurePayload,
  PipelineStatus,
} from "../types";

export type CompositeHealth = "healthy" | "degraded" | "error";

export interface CompositeHealthInput {
  pipelineStatus: PipelineStatus;
  /** Total dropped chunks across every processed-audio consumer. */
  consumerDroppedChunks: number;
  /** Persistence writer (transcript/projection JSONL) backpressure queues. */
  persistenceQueueBackpressure: Record<
    string,
    PersistenceQueueBackpressurePayload
  >;
  /** Capture sources whose ring buffer is currently dropping chunks. */
  backpressuredSourceCount: number;
}

export function computeCompositeHealth({
  pipelineStatus,
  consumerDroppedChunks,
  persistenceQueueBackpressure,
  backpressuredSourceCount,
}: CompositeHealthInput): CompositeHealth {
  const stages = Object.values(pipelineStatus);
  if (stages.some((stage) => stage.type === "Error")) return "error";

  const persistenceDropped = Object.values(persistenceQueueBackpressure).reduce(
    (sum, payload) => sum + payload.dropped_count,
    0,
  );
  if (
    stages.some((stage) => stage.type === "Degraded") ||
    consumerDroppedChunks > 0 ||
    persistenceDropped > 0 ||
    backpressuredSourceCount > 0
  ) {
    return "degraded";
  }
  return "healthy";
}
