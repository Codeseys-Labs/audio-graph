/**
 * Session data-route / privacy report (seed audio-graph-51e0).
 *
 * Renders a non-secret route report for a loaded session from the backend
 * data-movement ledger (seed audio-graph-70a3): which data stayed local vs.
 * left the device, the provider/model + data-class transfers, artifact and
 * export/delete lifecycle, redacted provider errors, and the saved-credential
 * source/readiness summary.
 *
 * The ledger is redaction-safe by construction (data classes, boundary hops,
 * provider/model ids, hashed artifact paths, pre-redacted error strings — never
 * secrets or raw payloads), so everything here is safe to display. A local-only
 * session with no egress renders an explicit "no content left the device"
 * banner; a cloud session shows the provider/model/data-class transfer without
 * exposing any secret.
 *
 * Props:
 *   - `sessionId`: the session to report on. When absent, the panel prompts the
 *     user to load a session first.
 */
import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import type {
  DataClass,
  DataMovementEvent,
  LedgerPrivacyMode as PrivacyMode,
} from "../types";
import { errorToMessage } from "../utils/errorToMessage";
import Icon from "./Icon";
import {
  buildSessionDataRouteReport,
  type CredentialSummary,
  type ProviderTransfer,
  type RedactedError,
} from "./sessionDataRoute";

type LoadState = "idle" | "loading" | "ready" | "error";

export interface SessionDataRoutePanelProps {
  sessionId: string | null;
}

function formatTimestamp(ms: number): string {
  if (!ms) return "—";
  return new Date(ms).toLocaleString();
}

function privacyModeLabel(
  mode: PrivacyMode,
  t: (key: string) => string,
): string {
  return t(`dataRoute.privacyMode.${mode}`);
}

function dataClassLabel(cls: DataClass, t: (key: string) => string): string {
  return t(`dataRoute.dataClass.${cls}`);
}

interface DataClassChipsProps {
  classes: DataClass[];
}

function DataClassChips({ classes }: DataClassChipsProps) {
  const { t } = useTranslation();
  if (classes.length === 0) return null;
  return (
    <ul className="m-0 flex list-none flex-wrap gap-(--space-2) p-0">
      {classes.map((cls) => (
        <li
          key={cls}
          className="rounded-xl bg-(--hover-overlay) px-(--space-3) py-px text-[9px] font-semibold uppercase tracking-[0.3px] text-text-secondary"
        >
          {dataClassLabel(cls, t)}
        </li>
      ))}
    </ul>
  );
}

interface ProviderTransferRowProps {
  transfer: ProviderTransfer;
}

function ProviderTransferRow({ transfer }: ProviderTransferRowProps) {
  const { t } = useTranslation();
  return (
    <li
      className="rounded-md border border-border-color bg-bg-tertiary px-(--space-3) py-(--space-2)"
      data-testid="data-route-transfer"
    >
      <div className="mb-(--space-1) flex items-center justify-between gap-(--space-2)">
        <span className="min-w-0 text-xs font-semibold text-text-primary [overflow-wrap:anywhere]">
          {transfer.providerId ?? t(`dataRoute.boundary.${transfer.boundary}`)}
        </span>
        <span className="shrink-0 rounded-xl bg-(--tint-accent-info-hover) px-(--space-3) py-px text-[9px] font-semibold uppercase tracking-[0.3px] text-accent-blue">
          {t(`dataRoute.boundary.${transfer.boundary}`)}
        </span>
      </div>
      {transfer.modelId && (
        <p className="m-0 mb-(--space-1) font-mono text-2xs text-text-secondary [overflow-wrap:anywhere]">
          {t("dataRoute.model", { model: transfer.modelId })}
        </p>
      )}
      {transfer.endpointClass && (
        <p className="m-0 mb-(--space-1) text-2xs text-text-muted [overflow-wrap:anywhere]">
          {t("dataRoute.endpoint", { endpoint: transfer.endpointClass })}
        </p>
      )}
      <DataClassChips classes={transfer.dataClasses} />
    </li>
  );
}

interface RedactedErrorRowProps {
  error: RedactedError;
}

function RedactedErrorRow({ error }: RedactedErrorRowProps) {
  const { t } = useTranslation();
  return (
    <li className="rounded-sm border border-(--tint-border-warning) bg-(--tint-warning) px-(--space-3) py-(--space-2)">
      <div className="flex items-center justify-between gap-(--space-2)">
        <span className="text-2xs font-semibold text-accent-yellow [overflow-wrap:anywhere]">
          {error.providerId ?? t("dataRoute.errorUnknownProvider")}
        </span>
        {error.errorCode && (
          <span className="shrink-0 font-mono text-[9px] font-semibold uppercase tracking-[0.3px] text-accent-yellow">
            {error.errorCode}
          </span>
        )}
      </div>
      {error.message && (
        <p className="m-0 mt-[2px] text-2xs text-text-secondary leading-[1.35] [overflow-wrap:anywhere]">
          {error.message}
        </p>
      )}
    </li>
  );
}

interface CredentialRowProps {
  credential: CredentialSummary;
}

function CredentialRow({ credential }: CredentialRowProps) {
  const { t } = useTranslation();
  const readiness =
    credential.ready === null
      ? t("dataRoute.credentialReadinessUnknown")
      : credential.ready
        ? t("dataRoute.credentialReady")
        : t("dataRoute.credentialNotReady");
  return (
    <li className="rounded-md border border-border-color bg-bg-tertiary px-(--space-3) py-(--space-2)">
      <div className="flex items-center justify-between gap-(--space-2)">
        <span className="min-w-0 text-xs font-semibold text-text-primary [overflow-wrap:anywhere]">
          {credential.providerId ??
            credential.sourceLabel ??
            t("dataRoute.credentialUnknown")}
        </span>
        <span className="shrink-0 text-2xs font-semibold text-text-secondary">
          {readiness}
        </span>
      </div>
      <p className="m-0 mt-[2px] text-2xs text-text-muted [overflow-wrap:anywhere]">
        {t(`dataRoute.eventType.${credential.lastEventType}`)}
      </p>
    </li>
  );
}

interface SectionProps {
  title: string;
  icon: React.ComponentProps<typeof Icon>["name"];
  children: React.ReactNode;
  label?: string;
}

function Section({ title, icon, children, label }: SectionProps) {
  return (
    <article
      className="border border-border-color rounded-md bg-bg-secondary p-(--space-4)"
      aria-label={label ?? title}
    >
      <h4 className="m-0 mb-(--space-3) flex items-center gap-(--space-2) text-xs font-semibold text-text-primary">
        <Icon name={icon} size={14} />
        {title}
      </h4>
      {children}
    </article>
  );
}

export default function SessionDataRoutePanel({
  sessionId,
}: SessionDataRoutePanelProps) {
  const { t } = useTranslation();
  const [events, setEvents] = useState<DataMovementEvent[] | null>(null);
  const [loadState, setLoadState] = useState<LoadState>("idle");
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async (id: string, cancelled?: () => boolean) => {
    setLoadState("loading");
    setError(null);
    try {
      const next = await invoke<DataMovementEvent[]>(
        "load_session_data_movement_cmd",
        { sessionId: id },
      );
      if (cancelled?.()) return;
      setEvents(next);
      setLoadState("ready");
    } catch (err) {
      if (cancelled?.()) return;
      setError(errorToMessage(err));
      setLoadState("error");
    }
  }, []);

  useEffect(() => {
    if (!sessionId) {
      setEvents(null);
      setLoadState("idle");
      return;
    }
    let cancelled = false;
    void load(sessionId, () => cancelled);
    return () => {
      cancelled = true;
    };
  }, [sessionId, load]);

  const report = useMemo(
    () => (events ? buildSessionDataRouteReport(events) : null),
    [events],
  );

  return (
    <section
      className="flex-shrink-0 border-t border-border-color bg-bg-tertiary py-(--space-4) px-(--space-5)"
      aria-label={t("dataRoute.label")}
      aria-busy={loadState === "loading"}
    >
      <div className="mb-(--space-3) flex items-center justify-between gap-(--space-4)">
        <h3 className="panel-title flex items-center gap-(--space-2)">
          <Icon name="info" size={15} />
          {t("dataRoute.title")}
        </h3>
        {sessionId && (
          <button
            type="button"
            className="inline-flex items-center gap-(--space-2) rounded-md border border-border-color bg-(--hover-overlay) px-(--space-4) py-[3px] text-2xs font-semibold uppercase tracking-[0.4px] text-text-secondary leading-[1.3] cursor-pointer transition-colors hover:not-disabled:border-(--tint-border-accent-info) hover:not-disabled:bg-(--tint-accent-info-hover) hover:not-disabled:text-(--text-on-tint-info) disabled:cursor-not-allowed disabled:opacity-45"
            disabled={loadState === "loading"}
            onClick={() => void load(sessionId)}
            aria-label={t("dataRoute.refreshLabel")}
          >
            <Icon name="refresh" size={12} />
            {loadState === "loading"
              ? t("dataRoute.refreshing")
              : t("dataRoute.refresh")}
          </button>
        )}
      </div>

      {!sessionId && (
        <p className="m-0 text-xs italic text-text-muted leading-[1.4]">
          {t("dataRoute.noSession")}
        </p>
      )}

      {sessionId && loadState === "loading" && !report && (
        <p className="m-0 text-xs italic text-text-muted leading-[1.4]">
          {t("dataRoute.loading")}
        </p>
      )}

      {loadState === "error" && (
        <p className="m-0 text-xs text-accent-red leading-[1.4]" role="alert">
          {t("dataRoute.error", { message: error })}
        </p>
      )}

      {report && (
        <div className="flex flex-col gap-(--space-4)">
          {/* Headline banner: did any content leave the device? */}
          {report.contentLeftDevice ? (
            <p
              className="m-0 rounded-sm border border-(--tint-border-accent-info) bg-(--tint-accent-info-hover) px-(--space-4) py-(--space-3) text-xs text-accent-blue leading-[1.35]"
              role="status"
              data-testid="data-route-egress-banner"
            >
              <Icon name="warning" size={14} />{" "}
              {t("dataRoute.contentLeftDevice", {
                count: report.egressEvents.length,
              })}
            </p>
          ) : (
            <p
              className="m-0 rounded-sm border border-(--tint-border-success) bg-(--tint-success) px-(--space-4) py-(--space-3) text-xs text-accent-green leading-[1.35]"
              role="status"
              data-testid="data-route-local-only-banner"
            >
              <Icon name="check" size={14} />{" "}
              {t("dataRoute.noContentLeftDevice")}
            </p>
          )}

          {/* Privacy mode + capture source overview. */}
          <dl className="grid grid-cols-2 md:grid-cols-4 gap-x-(--space-4) gap-y-(--space-3) m-0">
            <div className="min-w-0">
              <dt className="m-0 text-[9px] font-semibold uppercase tracking-[0.4px] text-text-muted leading-[1.2]">
                {t("dataRoute.privacyModeLabel")}
              </dt>
              <dd className="m-0 mt-[2px] text-xs font-semibold text-text-primary leading-tight [overflow-wrap:anywhere]">
                {report.privacyModes.length > 0
                  ? report.privacyModes
                      .map((mode) => privacyModeLabel(mode, t))
                      .join(", ")
                  : t("dataRoute.privacyModeUnknown")}
              </dd>
            </div>
            <div className="min-w-0">
              <dt className="m-0 text-[9px] font-semibold uppercase tracking-[0.4px] text-text-muted leading-[1.2]">
                {t("dataRoute.eventCountLabel")}
              </dt>
              <dd className="m-0 mt-[2px] font-mono text-xs font-semibold text-text-primary leading-tight">
                {report.eventCount}
              </dd>
            </div>
            <div className="min-w-0">
              <dt className="m-0 text-[9px] font-semibold uppercase tracking-[0.4px] text-text-muted leading-[1.2]">
                {t("dataRoute.localEventsLabel")}
              </dt>
              <dd className="m-0 mt-[2px] font-mono text-xs font-semibold text-text-primary leading-tight">
                {report.localEvents.length}
              </dd>
            </div>
            <div className="min-w-0">
              <dt className="m-0 text-[9px] font-semibold uppercase tracking-[0.4px] text-text-muted leading-[1.2]">
                {t("dataRoute.egressEventsLabel")}
              </dt>
              <dd className="m-0 mt-[2px] font-mono text-xs font-semibold text-text-primary leading-tight">
                {report.egressEvents.length}
              </dd>
            </div>
          </dl>

          {report.captureSources.length > 0 && (
            <Section title={t("dataRoute.captureSourcesTitle")} icon="mic">
              <ul className="m-0 flex list-none flex-col gap-(--space-1) p-0">
                {report.captureSources.map((source) => (
                  <li
                    key={source}
                    className="text-2xs text-text-secondary [overflow-wrap:anywhere]"
                  >
                    {source}
                  </li>
                ))}
              </ul>
            </Section>
          )}

          {/* Provider/model/data-class transfers that left the device. */}
          <Section
            title={t("dataRoute.transfersTitle")}
            icon="system"
            label={t("dataRoute.transfersTitle")}
          >
            {report.providerTransfers.length === 0 ? (
              <p className="m-0 text-2xs italic text-text-muted leading-[1.4]">
                {t("dataRoute.noTransfers")}
              </p>
            ) : (
              <>
                <ul className="m-0 mb-(--space-3) flex list-none flex-col gap-(--space-2) p-0">
                  {report.providerTransfers.map((transfer) => (
                    <ProviderTransferRow
                      key={transfer.key}
                      transfer={transfer}
                    />
                  ))}
                </ul>
                <div>
                  <p className="m-0 mb-(--space-1) text-[9px] font-semibold uppercase tracking-[0.4px] text-text-muted">
                    {t("dataRoute.egressDataClassesLabel")}
                  </p>
                  <DataClassChips classes={report.egressDataClasses} />
                </div>
              </>
            )}
          </Section>

          {/* Artifact + export/delete lifecycle. */}
          <Section title={t("dataRoute.artifactsTitle")} icon="notes">
            <dl className="grid grid-cols-3 gap-x-(--space-4) gap-y-(--space-3) m-0">
              {(
                [
                  ["written", report.artifacts.written],
                  ["loaded", report.artifacts.loaded],
                  ["exported", report.artifacts.exported],
                  ["softDeleted", report.artifacts.softDeleted],
                  ["hardDeleted", report.artifacts.hardDeleted],
                  ["deleteFailed", report.artifacts.deleteFailed],
                ] as const
              ).map(([key, value]) => (
                <div key={key} className="min-w-0">
                  <dt className="m-0 text-[9px] font-semibold uppercase tracking-[0.4px] text-text-muted leading-[1.2]">
                    {t(`dataRoute.artifact.${key}`)}
                  </dt>
                  <dd className="m-0 mt-[2px] font-mono text-xs font-semibold text-text-primary leading-tight">
                    {value}
                  </dd>
                </div>
              ))}
            </dl>
          </Section>

          {/* Saved-credential source/readiness summary. */}
          {report.credentials.length > 0 && (
            <Section title={t("dataRoute.credentialsTitle")} icon="settings">
              <ul className="m-0 flex list-none flex-col gap-(--space-2) p-0">
                {report.credentials.map((credential) => (
                  <CredentialRow key={credential.key} credential={credential} />
                ))}
              </ul>
            </Section>
          )}

          {/* Redacted provider errors. */}
          {report.redactedErrors.length > 0 && (
            <Section title={t("dataRoute.errorsTitle")} icon="warning">
              <ul className="m-0 flex list-none flex-col gap-(--space-2) p-0">
                {report.redactedErrors.map((err) => (
                  <RedactedErrorRow key={err.key} error={err} />
                ))}
              </ul>
            </Section>
          )}

          <p className="m-0 text-[9px] italic text-text-muted leading-[1.3]">
            {t("dataRoute.redactionNote")}
          </p>
        </div>
      )}
    </section>
  );
}

export { formatTimestamp };
