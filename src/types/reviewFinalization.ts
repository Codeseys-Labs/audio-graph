/**
 * Review Finalizing / Finalization Blocked — LOW-FIDELITY PROTOTYPE TYPES
 * (seed audio-graph-1d92, ADR-0036 "Downstream ownership").
 *
 * ADR-0036 assigns 1d92 the job of prototyping how Review presents the
 * Finalizing / Finalization Blocked lifecycle states ahead of the real
 * backend derivation, which ADR-0036 itself says is "currently unimplementable"
 * pending audio-graph-90f3 / audio-graph-8e73 delivering a durable `Accepted`
 * write-path acknowledgement. This ticket is greenfield UX modeling — there is
 * no lifecycle contract for Finalizing/Finalization Blocked anywhere in the
 * stack today.
 *
 * These types describe the SHAPE a future `get_session_finalization_status_cmd`
 * / `retry_session_finalization_cmd` IPC surface might take — but NO Rust
 * command by these names exists yet. Components that call them
 * (`ReviewFinalizationPanel`, `SessionsBrowser`'s finalization pill) degrade
 * gracefully when the real `invoke` rejects (command not found), and tests
 * fake the data boundary the same way `ProjectionRuntimeStatusPanel.test.tsx`
 * and `SessionsBrowser.test.tsx` already do: by mocking `@tauri-apps/api/core`'s
 * `invoke`.
 *
 * Unlike `SessionMetadata` in `./index.ts` (whose header requires a matching
 * Rust change for every edit), nothing here mirrors a real `Serialize`/
 * `Deserialize` struct — do not add a matching Rust type until a real seed
 * builds the backend derivation (ADR-0036 names audio-graph-70c8 for the
 * retry / closed-reason taxonomy).
 *
 * Faithful to ADR-0036's "no persisted stage enum anywhere" constraint: the
 * status payload below carries only durable INPUTS (per-lane coverage
 * watermarks, a remote-attempt ledger, and the per-session Finalization
 * Blocked record). The stage itself (`SessionFinalizationStage`) is
 * deliberately NOT part of the payload — it is always re-derived by
 * `deriveFinalizationStage` at render time, so "progress is computed, not
 * glanced at" (ADR-0036's own named negative consequence).
 */

export type FinalizationLane = "notes" | "graph";

/**
 * ADR-0035's full closed-reason taxonomy is explicitly deferred to
 * audio-graph-70c8. This prototype only needs the four classes ADR-0036
 * already names, because each drives distinct retry UI behavior:
 *   - `never_dispatched` / `provably_absent`: the two AudioGraph-owned classes
 *     ADR-0036 permits auto-retry for "without asking" — provably free of
 *     cost or egress.
 *   - `external_uncertain`: the dominant cause named by ADR-0035 (remote LLM
 *     route rate-limit / timeout / outage) — retry may cost money or contact
 *     a provider, so it always needs explicit authorization, in every variant.
 *   - `user_cancelled`: `Blocked{UserCancelled}` — stays retryable
 *     indefinitely; "Review does not nag about it" (ADR-0036, verbatim).
 */
export type FinalizationBlockedReasonClass =
  | "never_dispatched"
  | "provably_absent"
  | "external_uncertain"
  | "user_cancelled";

export interface FinalizationBlockedReason {
  class: FinalizationBlockedReasonClass;
  /** Short human summary, e.g. "Remote LLM route timed out". */
  summary: string;
  /** Longer explanation shown in the expanded detail. */
  detail: string;
  /** Unix millis the record was raised. */
  since_ms: number;
  /**
   * Unix millis of the most recent user-initiated retry request, if any.
   * `external_uncertain` and `user_cancelled` records only clear once a
   * ledger entry AFTER this timestamp reports success — auto-heal never
   * resolves these two classes on its own.
   */
  retry_requested_at_ms: number | null;
}

export type RemoteAttemptOutcome =
  | "success"
  | "rate_limited"
  | "timeout"
  | "outage"
  | "never_dispatched";

export interface RemoteAttemptLedgerEntry {
  id: string;
  lane: FinalizationLane;
  attempted_at_ms: number;
  outcome: RemoteAttemptOutcome;
  /** Whether this attempt actually reached the network / provider. */
  dispatched: boolean;
  /** Whether this attempt incurred (or would incur) provider cost/egress. */
  cost_incurred: boolean;
}

export interface LaneCoverage {
  lane: FinalizationLane;
  /**
   * ADR-0036's accepted sub-question default: notes lane required, graph
   * lane recorded-but-not-required (graph coverage never gates Finalized).
   */
  required: boolean;
  covered: boolean;
  pending_span_count: number;
  /** Unix millis of the oldest still-pending span, or null when none are pending. */
  oldest_pending_since_ms: number | null;
}

export type KnowledgeGapKind =
  | "unmet_evidence_obligation"
  | "unconfirmed_high_impact_inference";

export interface KnowledgeGap {
  id: string;
  kind: KnowledgeGapKind;
  summary: string;
  related_note_id: string | null;
}

export interface TranscriptConfirmationLine {
  id: string;
  text: string;
  /** Durably accepted (past the drain watermark) vs. still in-flight. */
  confirmed: boolean;
}

export interface TranscriptConfirmationSummary {
  confirmed_count: number;
  interim_count: number;
  lines: TranscriptConfirmationLine[];
}

/**
 * The durable-inputs payload a future `get_session_finalization_status_cmd`
 * would return. Deliberately has NO `stage` field (see module doc).
 */
export interface SessionFinalizationStatus {
  session_id: string;
  lane_coverage: LaneCoverage[];
  remote_attempt_ledger: RemoteAttemptLedgerEntry[];
  blocked_record: FinalizationBlockedReason | null;
  /**
   * Q0.2 (inherited, not re-derived here per ADR-0036): Knowledge Gaps are
   * informational. Their presence never blocks Finalized on its own.
   */
  knowledge_gaps: KnowledgeGap[];
  transcript_confirmation: TranscriptConfirmationSummary;
}

export type SessionFinalizationStage =
  | "finalizing"
  | "finalization_blocked"
  | "finalized";

function laneCoverage(
  status: SessionFinalizationStatus,
  lane: FinalizationLane,
): LaneCoverage | undefined {
  return status.lane_coverage.find((l) => l.lane === lane);
}

/**
 * Re-derives whether `status.blocked_record` has *already* cleared, given the
 * ledger — with zero cost and zero egress for the two AudioGraph-owned
 * classes (ADR-0036). `external_uncertain` and `user_cancelled` only clear via
 * a ledger success AFTER an explicit `retry_requested_at_ms`; auto-heal never
 * resolves either on its own.
 */
export function isBlockedRecordResolved(
  status: SessionFinalizationStatus,
): boolean {
  const reason = status.blocked_record;
  if (!reason) return true;
  const sinceReason = status.remote_attempt_ledger.filter(
    (entry) => entry.attempted_at_ms >= reason.since_ms,
  );
  const latestSuccess = sinceReason
    .filter((entry) => entry.outcome === "success")
    .sort((a, b) => b.attempted_at_ms - a.attempted_at_ms)[0];

  switch (reason.class) {
    case "never_dispatched":
    case "provably_absent":
      // Auto-retry eligible: a later ledger success — even one AudioGraph
      // re-derived on its own, with no user click — clears it for free.
      return Boolean(latestSuccess);
    case "external_uncertain":
    case "user_cancelled":
      // Needs an explicit ask first; only a success AFTER that ask clears it.
      if (reason.retry_requested_at_ms === null) return false;
      return Boolean(
        latestSuccess &&
          latestSuccess.attempted_at_ms >= reason.retry_requested_at_ms,
      );
  }
}

/**
 * The single source of truth for "what stage is this session in" — computed
 * fresh from durable inputs every call, per ADR-0036 ("no persisted stage
 * enum anywhere"). Never cache the result of this call across a
 * re-derivation boundary; call it again next render/poll instead.
 */
export function deriveFinalizationStage(
  status: SessionFinalizationStatus,
): SessionFinalizationStage {
  if (status.blocked_record && !isBlockedRecordResolved(status)) {
    return "finalization_blocked";
  }
  const notes = laneCoverage(status, "notes");
  // Graph coverage never gates Finalized (ADR-0036 accepted sub-question
  // default) — only notes lane coverage does.
  if (notes?.covered) return "finalized";
  return "finalizing";
}

/** Whether this reason class is one of the two AudioGraph-owned, cost/egress-free auto-retry classes. */
export function isAutoRetryEligible(
  reasonClass: FinalizationBlockedReasonClass,
): boolean {
  return (
    reasonClass === "never_dispatched" || reasonClass === "provably_absent"
  );
}
