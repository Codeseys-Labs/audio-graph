/**
 * ReviewFinalizationPanel — low-fidelity prototype of how Review presents
 * the Finalizing / Finalization Blocked lifecycle (seed audio-graph-1d92,
 * ADR-0036 "Downstream ownership").
 *
 * This is greenfield UX modeling ahead of the real backend derivation, which
 * ADR-0036 itself calls "currently unimplementable" pending audio-graph-90f3 /
 * audio-graph-8e73. The component calls a command
 * (`get_session_finalization_status_cmd`) that does NOT exist on the real
 * Rust backend yet — see `src/types/reviewFinalization.ts` for the full
 * rationale. Against a real (command-less) backend this degrades to the same
 * "empty" state a session with no finalization data gets; tests fake the
 * response the same way `ProjectionRuntimeStatusPanel.test.tsx` does.
 *
 * Renders nothing when `sessionId` is null (no session loaded/previewed) or
 * when the backend has no finalization data for this session (every session
 * today, since the command doesn't exist — this keeps the panel invisible
 * and harmless for the existing sample-preview / loaded-session flows).
 *
 * Faithful to ADR-0036's "no persisted stage enum" constraint: the fetched
 * payload carries only durable inputs, and `deriveFinalizationStage` recomputes
 * the stage on every render — never cached across a fetch.
 *
 * Three of this ticket's five open design questions have two plausible
 * presentations; this panel exposes each as an explicit variant (default
 * chosen + documented, both fully implemented and render-tested):
 *   - `blockedPresentation` ("banner" default | "badge"): how the
 *     non-dismissable Finalization Blocked record visually reads as
 *     "needs attention" without becoming an app-modal (ADR-0035's own named
 *     risk: per-Session Blocked debt can rot quietly unless surfaced).
 *   - `retryAffordance` ("explicitButton" default | "autoHealOnly"): whether
 *     a manual Retry control exists, or the state heals itself on
 *     next render/poll. The `external_uncertain` class always keeps an
 *     explicit control in both variants — it needs cost/egress authorization
 *     regardless.
 *   - `graphLaneVisibility` ("informational" default | "hidden"): whether the
 *     non-gating graph lane is shown at all, since hiding it risks an
 *     invisible stalled lane while showing it risks implying it's required.
 *
 * The remaining two open questions (list-vs-detail surfacing, and background
 * access while another Session is Live) are prototyped in `SessionsBrowser`.
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
// `safeInvoke` (aliased to `invoke`) is a drop-in for the Tauri `invoke` that
// relays a command-name-only failure diagnostic to analytics then rethrows, so
// this call site's error handling is unchanged (audio-graph-3e71).
import { safeInvoke as invoke } from "../analytics/safeInvoke";
import {
  deriveFinalizationStage,
  isAutoRetryEligible,
  isBlockedRecordResolved,
  type SessionFinalizationStatus,
} from "../types/reviewFinalization";
import { errorToMessage } from "../utils/errorToMessage";
import Icon from "./Icon";

export type BlockedPresentationVariant = "banner" | "badge";
export type RetryAffordanceVariant = "explicitButton" | "autoHealOnly";
export type GraphLaneVisibilityVariant = "informational" | "hidden";

export interface ReviewFinalizationPanelProps {
  sessionId: string | null;
  /** Test/story override; defaults to the panel's own "Prototype variant" select. */
  blockedPresentation?: BlockedPresentationVariant;
  retryAffordance?: RetryAffordanceVariant;
  graphLaneVisibility?: GraphLaneVisibilityVariant;
}

type LoadState = "idle" | "loading" | "ready" | "error";

function formatTimestamp(ms: number): string {
  return new Date(ms).toLocaleString();
}

export default function ReviewFinalizationPanel({
  sessionId,
  blockedPresentation: blockedPresentationProp,
  retryAffordance: retryAffordanceProp,
  graphLaneVisibility: graphLaneVisibilityProp,
}: ReviewFinalizationPanelProps) {
  const { t } = useTranslation();
  const [status, setStatus] = useState<SessionFinalizationStatus | null>(null);
  const [loadState, setLoadState] = useState<LoadState>("idle");
  const [error, setError] = useState<string | null>(null);
  const [retrying, setRetrying] = useState(false);
  const [retryError, setRetryError] = useState<string | null>(null);
  const [confirmingExternalRetry, setConfirmingExternalRetry] = useState(false);
  const [evidenceOpen, setEvidenceOpen] = useState(false);

  const [blockedPresentationLocal, setBlockedPresentationLocal] =
    useState<BlockedPresentationVariant>("banner");
  const [retryAffordanceLocal, setRetryAffordanceLocal] =
    useState<RetryAffordanceVariant>("explicitButton");
  const [graphLaneVisibilityLocal, setGraphLaneVisibilityLocal] =
    useState<GraphLaneVisibilityVariant>("informational");

  const blockedPresentation =
    blockedPresentationProp ?? blockedPresentationLocal;
  const retryAffordance = retryAffordanceProp ?? retryAffordanceLocal;
  const graphLaneVisibility =
    graphLaneVisibilityProp ?? graphLaneVisibilityLocal;

  const load = useCallback(async (id: string) => {
    setLoadState("loading");
    setError(null);
    try {
      const next = await invoke<SessionFinalizationStatus | null>(
        "get_session_finalization_status_cmd",
        { sessionId: id },
      );
      setStatus(next ?? null);
      setLoadState("ready");
    } catch (err) {
      setError(errorToMessage(err));
      setLoadState("error");
    }
  }, []);

  useEffect(() => {
    setStatus(null);
    setRetryError(null);
    setConfirmingExternalRetry(false);
    setEvidenceOpen(false);
    if (!sessionId) {
      setLoadState("idle");
      return;
    }
    void load(sessionId);
  }, [sessionId, load]);

  // "autoHealOnly": passively re-check once after a blocked record loads, so
  // an auto-retry-eligible class can clear on "next render/poll" without any
  // click (ADR-0036: re-derived fresh immediately before it is shown).
  const autoHealTimerRef = useRef<number | null>(null);
  useEffect(() => {
    if (!sessionId || !status?.blocked_record) return;
    if (retryAffordance !== "autoHealOnly") return;
    if (!isAutoRetryEligible(status.blocked_record.class)) return;
    if (isBlockedRecordResolved(status)) return;
    autoHealTimerRef.current = window.setTimeout(() => {
      void load(sessionId);
    }, 50);
    return () => {
      if (autoHealTimerRef.current !== null) {
        window.clearTimeout(autoHealTimerRef.current);
      }
    };
  }, [sessionId, status, retryAffordance, load]);

  const requestRetry = useCallback(
    async (authorizeCostAndEgress: boolean) => {
      if (!sessionId) return;
      setRetrying(true);
      setRetryError(null);
      try {
        const next = await invoke<SessionFinalizationStatus>(
          "retry_session_finalization_cmd",
          { sessionId, authorizeCostAndEgress },
        );
        setStatus(next);
        setConfirmingExternalRetry(false);
      } catch (err) {
        setRetryError(errorToMessage(err));
      } finally {
        setRetrying(false);
      }
    },
    [sessionId],
  );

  if (!sessionId) return null;
  if (loadState === "idle" || loadState === "loading") {
    return (
      <section
        className="border-t border-border-color bg-bg-tertiary px-(--space-5) py-(--space-4)"
        aria-label={t("reviewFinalization.title")}
        aria-busy="true"
      >
        <p className="m-0 text-xs italic text-text-muted">
          {t("reviewFinalization.loading")}
        </p>
      </section>
    );
  }
  if (loadState === "error") {
    // The mocked `get_session_finalization_status_cmd` doesn't exist on a
    // real backend yet, so a real app run always lands here today. Degrade
    // to the same calm "no data" message a session with no finalization
    // record gets — the raw failure (command-not-found, in production;
    // whatever a test mocks, under test) stays available via `data-error`
    // for debugging rather than alarming the user with an IPC internals
    // string. See the module doc for why this command is a documented stub.
    return (
      <section
        className="border-t border-border-color bg-bg-tertiary px-(--space-5) py-(--space-4)"
        aria-label={t("reviewFinalization.title")}
        data-error={error ?? undefined}
      >
        <p className="m-0 text-xs italic text-text-muted" role="status">
          {t("reviewFinalization.empty")}
        </p>
      </section>
    );
  }
  // No finalization data for this session (the common case today — no
  // backend command exists yet) — render nothing rather than an empty shell.
  if (!status) return null;

  const stage = deriveFinalizationStage(status);
  const notesLane = status.lane_coverage.find((l) => l.lane === "notes");
  const graphLane = status.lane_coverage.find((l) => l.lane === "graph");
  const reason = status.blocked_record;
  const reasonResolved = reason ? isBlockedRecordResolved(status) : true;
  const showBlocked = stage === "finalization_blocked";

  return (
    <section
      className="flex flex-col gap-(--space-4) border-t border-border-color bg-bg-tertiary px-(--space-5) py-(--space-4)"
      aria-label={t("reviewFinalization.title")}
      data-testid="review-finalization-panel"
      data-session-id={sessionId}
    >
      <div className="flex items-center justify-between gap-(--space-3)">
        <h3 className="panel-title flex items-center gap-(--space-2)">
          <Icon name="notes" size={15} />
          {t("reviewFinalization.title")}
        </h3>
        <span
          data-testid="finalization-stage-chip"
          className={`shrink-0 rounded-xl px-(--space-3) py-px text-[10px] font-semibold uppercase tracking-[0.3px] ${
            stage === "finalization_blocked"
              ? "text-accent-yellow bg-(--tint-warning)"
              : stage === "finalized"
                ? "text-accent-green bg-(--tint-success)"
                : "text-accent-blue bg-(--tint-accent-info-hover)"
          }`}
        >
          {t(`reviewFinalization.stage.${stage}`)}
        </span>
      </div>

      {/* Auto-healed reason kept for transparency even once resolved. */}
      {reason && reasonResolved && (
        <p
          className="m-0 rounded-sm border border-(--tint-border-success) bg-(--tint-success) px-(--space-4) py-(--space-2) text-xs text-accent-green"
          role="status"
        >
          {t("reviewFinalization.blocked.autoHealed")}
        </p>
      )}

      {showBlocked && reason && (
        <BlockedRecord
          reason={reason}
          presentation={blockedPresentation}
          retryAffordance={retryAffordance}
          retrying={retrying}
          retryError={retryError}
          confirmingExternalRetry={confirmingExternalRetry}
          onRequestConfirm={() => setConfirmingExternalRetry(true)}
          onCancelConfirm={() => setConfirmingExternalRetry(false)}
          onRetry={requestRetry}
        />
      )}

      <div
        className="flex flex-col gap-(--space-2)"
        data-testid="finalization-lanes"
      >
        {stage === "finalizing" && (
          <p className="m-0 text-2xs italic text-text-muted">
            {t("reviewFinalization.progressCaption")}
          </p>
        )}
        <LaneRow lane="notes" coverage={notesLane} />
        {graphLaneVisibility === "informational" && (
          <>
            <LaneRow lane="graph" coverage={graphLane} />
            <p className="m-0 text-2xs italic text-text-muted">
              {t("reviewFinalization.lane.graphInformationalNote")}
            </p>
          </>
        )}
      </div>

      <TranscriptConfirmation summary={status.transcript_confirmation} />

      <KnowledgeGaps gaps={status.knowledge_gaps} />

      <EvidenceInspection
        ledger={status.remote_attempt_ledger}
        open={evidenceOpen}
        onToggle={() => setEvidenceOpen((v) => !v)}
      />

      {!blockedPresentationProp &&
        !retryAffordanceProp &&
        !graphLaneVisibilityProp && (
          <VariantControls
            blockedPresentation={blockedPresentationLocal}
            onBlockedPresentation={setBlockedPresentationLocal}
            retryAffordance={retryAffordanceLocal}
            onRetryAffordance={setRetryAffordanceLocal}
            graphLaneVisibility={graphLaneVisibilityLocal}
            onGraphLaneVisibility={setGraphLaneVisibilityLocal}
          />
        )}
    </section>
  );
}

function LaneRow({
  lane,
  coverage,
}: {
  lane: "notes" | "graph";
  coverage:
    | {
        required: boolean;
        covered: boolean;
        pending_span_count: number;
        oldest_pending_since_ms: number | null;
      }
    | undefined;
}) {
  const { t } = useTranslation();
  if (!coverage) return null;
  return (
    <div
      className="flex items-center justify-between gap-(--space-3) rounded-sm border border-border-color bg-bg-secondary px-(--space-3) py-(--space-2)"
      data-testid={`finalization-lane-${lane}`}
    >
      <span className="text-xs font-semibold text-text-primary">
        {t(`reviewFinalization.lane.${lane}`)}
      </span>
      <span className="text-2xs text-text-secondary">
        {t(
          coverage.required
            ? "reviewFinalization.lane.required"
            : "reviewFinalization.lane.notRequired",
        )}
      </span>
      <span className="text-2xs text-text-secondary">
        {coverage.covered
          ? t("reviewFinalization.lane.covered")
          : t("reviewFinalization.lane.pending", {
              count: coverage.pending_span_count,
            })}
      </span>
    </div>
  );
}

function BlockedRecord({
  reason,
  presentation,
  retryAffordance,
  retrying,
  retryError,
  confirmingExternalRetry,
  onRequestConfirm,
  onCancelConfirm,
  onRetry,
}: {
  reason: NonNullable<SessionFinalizationStatus["blocked_record"]>;
  presentation: BlockedPresentationVariant;
  retryAffordance: RetryAffordanceVariant;
  retrying: boolean;
  retryError: string | null;
  confirmingExternalRetry: boolean;
  onRequestConfirm: () => void;
  onCancelConfirm: () => void;
  onRetry: (authorizeCostAndEgress: boolean) => void;
}) {
  const { t } = useTranslation();
  const isUserCancelled = reason.class === "user_cancelled";
  const eligibleForAutoRetry = isAutoRetryEligible(reason.class);
  const needsExplicitAuthorization = !eligibleForAutoRetry;

  // Non-dismissable by construction: there is intentionally no close/X
  // button anywhere in this subtree, and (unlike an app-modal) it never uses
  // role="dialog" or a focus trap — it reads as "this Session needs
  // attention" inline, not as a blocking overlay (ADR-0035 / ADR-0036).
  const container =
    presentation === "banner"
      ? `flex flex-col gap-(--space-3) rounded-md border px-(--space-4) py-(--space-3) ${
          isUserCancelled
            ? "border-border-color bg-bg-secondary"
            : "border-(--tint-border-warning) bg-(--tint-warning)"
        }`
      : `flex flex-col gap-(--space-2) rounded-md border px-(--space-3) py-(--space-2) ${
          isUserCancelled
            ? "border-border-color bg-bg-secondary"
            : "border-(--tint-border-warning) bg-(--tint-warning)"
        }`;

  return (
    <div
      className={container}
      data-testid="finalization-blocked-record"
      data-presentation={presentation}
      data-reason-class={reason.class}
      role="status"
    >
      <div className="flex items-start gap-(--space-3)">
        <Icon
          name="lock"
          size={presentation === "banner" ? 16 : 13}
          title={t("reviewFinalization.blocked.cannotDismiss")}
        />
        <div className="flex flex-col gap-(--space-1) min-w-0">
          <span
            className={`font-semibold ${presentation === "banner" ? "text-sm" : "text-xs"} text-text-primary`}
          >
            {isUserCancelled
              ? t("reviewFinalization.blocked.userCancelledTitle")
              : t("reviewFinalization.blocked.title")}
          </span>
          {presentation === "banner" && (
            <>
              <span className="text-xs text-text-secondary [overflow-wrap:anywhere]">
                {reason.summary}
              </span>
              <span className="text-2xs text-text-muted [overflow-wrap:anywhere]">
                {reason.detail}
              </span>
            </>
          )}
          <span className="text-2xs text-text-muted">
            {t(`reviewFinalization.blocked.classLabel.${reason.class}`)}
            {" · "}
            {t("reviewFinalization.blocked.since", {
              time: formatTimestamp(reason.since_ms),
            })}
          </span>
          <span className="text-2xs italic text-text-muted">
            {t("reviewFinalization.blocked.cannotDismiss")}
          </span>
        </div>
      </div>

      <div className="flex items-center gap-(--space-3) flex-wrap">
        {retryAffordance === "explicitButton" && (
          <>
            {eligibleForAutoRetry && (
              <button
                type="button"
                className="settings-btn"
                onClick={() => onRetry(false)}
                disabled={retrying}
              >
                {retrying
                  ? t("reviewFinalization.retry.retrying")
                  : t("reviewFinalization.retry.free")}
              </button>
            )}
            {isUserCancelled && (
              <button
                type="button"
                className="settings-btn"
                onClick={() => onRetry(false)}
                disabled={retrying}
              >
                {retrying
                  ? t("reviewFinalization.retry.retrying")
                  : t("reviewFinalization.retry.resume")}
              </button>
            )}
          </>
        )}

        {retryAffordance === "autoHealOnly" && eligibleForAutoRetry && (
          <span className="text-2xs italic text-text-muted">
            {t("reviewFinalization.retry.autoHealCaption")}
          </span>
        )}
        {retryAffordance === "autoHealOnly" && isUserCancelled && (
          <button
            type="button"
            className="settings-btn"
            onClick={() => onRetry(false)}
            disabled={retrying}
          >
            {retrying
              ? t("reviewFinalization.retry.retrying")
              : t("reviewFinalization.retry.resume")}
          </button>
        )}

        {/* external_uncertain keeps an explicit control in BOTH variants —
            it always needs cost/egress authorization (ADR-0036). */}
        {needsExplicitAuthorization && !isUserCancelled && (
          <div className="flex flex-col gap-(--space-2)">
            {retryAffordance === "autoHealOnly" && (
              <span className="text-2xs italic text-text-muted">
                {t("reviewFinalization.retry.autoHealNote")}
              </span>
            )}
            {!confirmingExternalRetry ? (
              <button
                type="button"
                className="settings-btn"
                onClick={onRequestConfirm}
                disabled={retrying}
              >
                {t("reviewFinalization.retry.authorize")}
              </button>
            ) : (
              <div className="flex flex-col gap-(--space-2)">
                <span className="text-2xs text-text-secondary">
                  {t("reviewFinalization.retry.confirmPrompt")}
                </span>
                <div className="flex gap-(--space-3)">
                  <button
                    type="button"
                    className="settings-btn settings-btn--primary"
                    onClick={() => onRetry(true)}
                    disabled={retrying}
                  >
                    {retrying
                      ? t("reviewFinalization.retry.retrying")
                      : t("reviewFinalization.retry.confirm")}
                  </button>
                  <button
                    type="button"
                    className="settings-btn"
                    onClick={onCancelConfirm}
                    disabled={retrying}
                  >
                    {t("reviewFinalization.retry.cancel")}
                  </button>
                </div>
              </div>
            )}
          </div>
        )}
      </div>

      {retryError && (
        <p className="m-0 text-2xs text-accent-yellow" role="alert">
          {t("reviewFinalization.retry.failed", { message: retryError })}
        </p>
      )}
    </div>
  );
}

function TranscriptConfirmation({
  summary,
}: {
  summary: SessionFinalizationStatus["transcript_confirmation"];
}) {
  const { t } = useTranslation();
  if (summary.confirmed_count === 0 && summary.interim_count === 0) {
    return null;
  }
  return (
    <div
      className="flex flex-col gap-(--space-2)"
      data-testid="finalization-transcript-confirmation"
    >
      <h4 className="m-0 text-2xs font-bold uppercase tracking-[0.4px] text-text-muted">
        {t("reviewFinalization.transcript.title")}
      </h4>
      <p className="m-0 text-xs text-text-secondary">
        {t("reviewFinalization.transcript.summary", {
          confirmed: summary.confirmed_count,
          interim: summary.interim_count,
        })}
      </p>
      {summary.lines.length > 0 && (
        <ul className="list-none p-0 m-0 flex flex-col gap-(--space-1)">
          {summary.lines.map((line) => (
            <li
              key={line.id}
              className="flex items-center gap-(--space-3) text-2xs text-text-secondary"
            >
              <span
                className={`shrink-0 rounded-[10px] px-(--space-2) py-px text-[9px] font-semibold uppercase ${
                  line.confirmed
                    ? "text-accent-green bg-(--tint-success)"
                    : "text-accent-yellow bg-(--tint-warning)"
                }`}
              >
                {line.confirmed
                  ? t("reviewFinalization.transcript.confirmedBadge")
                  : t("reviewFinalization.transcript.interimBadge")}
              </span>
              <span className="[overflow-wrap:anywhere]">{line.text}</span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

function KnowledgeGaps({
  gaps,
}: {
  gaps: SessionFinalizationStatus["knowledge_gaps"];
}) {
  const { t } = useTranslation();
  if (gaps.length === 0) return null;
  return (
    <div className="flex flex-col gap-(--space-2)" data-testid="knowledge-gaps">
      <h4 className="m-0 text-2xs font-bold uppercase tracking-[0.4px] text-text-muted">
        {t("reviewFinalization.knowledgeGaps.title")}
      </h4>
      <ul className="list-none p-0 m-0 flex flex-col gap-(--space-2)">
        {gaps.map((gap) => (
          <li
            key={gap.id}
            className="rounded-sm border border-border-color bg-bg-secondary px-(--space-3) py-(--space-2)"
          >
            <span className="block text-2xs font-semibold text-text-primary">
              {t(`reviewFinalization.knowledgeGaps.kind.${gap.kind}`)}
            </span>
            <span className="block text-2xs text-text-secondary [overflow-wrap:anywhere]">
              {gap.summary}
            </span>
          </li>
        ))}
      </ul>
    </div>
  );
}

function EvidenceInspection({
  ledger,
  open,
  onToggle,
}: {
  ledger: SessionFinalizationStatus["remote_attempt_ledger"];
  open: boolean;
  onToggle: () => void;
}) {
  const { t } = useTranslation();
  return (
    <div data-testid="finalization-evidence">
      <button
        type="button"
        className="settings-btn"
        onClick={onToggle}
        aria-expanded={open}
      >
        {open
          ? t("reviewFinalization.evidence.hide")
          : t("reviewFinalization.evidence.toggle")}
      </button>
      {open && (
        <ul className="list-none p-0 m-0 mt-(--space-2) flex flex-col gap-(--space-1)">
          {ledger.length === 0 ? (
            <li className="text-2xs italic text-text-muted">
              {t("reviewFinalization.evidence.empty")}
            </li>
          ) : (
            ledger.map((entry) => (
              <li
                key={entry.id}
                className="flex items-center justify-between gap-(--space-3) rounded-sm border border-border-color bg-bg-secondary px-(--space-3) py-(--space-1) text-2xs text-text-secondary"
              >
                <span>
                  {t("reviewFinalization.evidence.ledgerEntry", {
                    lane: entry.lane,
                    outcome: entry.outcome,
                    time: formatTimestamp(entry.attempted_at_ms),
                  })}
                </span>
                <span className="text-2xs italic text-text-muted">
                  {entry.cost_incurred
                    ? t("reviewFinalization.evidence.costBadge")
                    : t("reviewFinalization.evidence.noCostBadge")}
                </span>
              </li>
            ))
          )}
        </ul>
      )}
    </div>
  );
}

function VariantControls({
  blockedPresentation,
  onBlockedPresentation,
  retryAffordance,
  onRetryAffordance,
  graphLaneVisibility,
  onGraphLaneVisibility,
}: {
  blockedPresentation: BlockedPresentationVariant;
  onBlockedPresentation: (v: BlockedPresentationVariant) => void;
  retryAffordance: RetryAffordanceVariant;
  onRetryAffordance: (v: RetryAffordanceVariant) => void;
  graphLaneVisibility: GraphLaneVisibilityVariant;
  onGraphLaneVisibility: (v: GraphLaneVisibilityVariant) => void;
}) {
  const { t } = useTranslation();
  return (
    <fieldset className="flex flex-col gap-(--space-2) rounded-md border border-dashed border-border-color px-(--space-3) py-(--space-2)">
      <legend className="text-2xs font-semibold uppercase tracking-[0.3px] text-text-muted">
        {t("reviewFinalization.variants.title")}
      </legend>
      <label className="flex items-center justify-between gap-(--space-3) text-2xs">
        <span>{t("reviewFinalization.variants.blockedPresentation")}</span>
        <select
          data-testid="variant-blocked-presentation"
          value={blockedPresentation}
          onChange={(e) =>
            onBlockedPresentation(e.target.value as BlockedPresentationVariant)
          }
        >
          <option value="banner">
            {t("reviewFinalization.variants.blockedPresentationBanner")}
          </option>
          <option value="badge">
            {t("reviewFinalization.variants.blockedPresentationBadge")}
          </option>
        </select>
      </label>
      <label className="flex items-center justify-between gap-(--space-3) text-2xs">
        <span>{t("reviewFinalization.variants.retryAffordance")}</span>
        <select
          data-testid="variant-retry-affordance"
          value={retryAffordance}
          onChange={(e) =>
            onRetryAffordance(e.target.value as RetryAffordanceVariant)
          }
        >
          <option value="explicitButton">
            {t("reviewFinalization.variants.retryAffordanceExplicit")}
          </option>
          <option value="autoHealOnly">
            {t("reviewFinalization.variants.retryAffordanceAutoHeal")}
          </option>
        </select>
      </label>
      <label className="flex items-center justify-between gap-(--space-3) text-2xs">
        <span>{t("reviewFinalization.variants.graphLane")}</span>
        <select
          data-testid="variant-graph-lane"
          value={graphLaneVisibility}
          onChange={(e) =>
            onGraphLaneVisibility(e.target.value as GraphLaneVisibilityVariant)
          }
        >
          <option value="informational">
            {t("reviewFinalization.variants.graphLaneInformational")}
          </option>
          <option value="hidden">
            {t("reviewFinalization.variants.graphLaneHidden")}
          </option>
        </select>
      </label>
    </fieldset>
  );
}
