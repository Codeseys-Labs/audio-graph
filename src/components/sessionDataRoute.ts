/**
 * Pure transforms for the session data-route / privacy report
 * (seed audio-graph-51e0), which consumes the backend data-movement ledger
 * (seed audio-graph-70a3).
 *
 * The ledger is a stream of already-redacted {@link DataMovementEvent}s. These
 * helpers fold that stream into a report the UI can render directly: which data
 * stayed local vs. left the device, the provider/model transfers, artifact and
 * export/delete lifecycle, redacted provider errors, and the saved-credential
 * source/readiness summary.
 *
 * By construction the ledger never carries a secret or a raw payload — only
 * data *classes*, boundary hops, provider/model ids, hashed artifact paths, and
 * pre-redacted error strings — so every field surfaced here is safe to show.
 * These functions add no field that could reintroduce one; they only group,
 * count, and de-duplicate what the backend already redacted.
 */
import type {
  DataClass,
  DataMovementEvent,
  DestinationBoundary,
  LedgerPrivacyMode as PrivacyMode,
} from "../types";

/** A destination boundary is an egress boundary when it leaves the device. */
export function isEgressBoundary(boundary: DestinationBoundary): boolean {
  return boundary !== "local";
}

/**
 * Whether an event represents content actually leaving the device.
 *
 * A `provider`/`org`/`export` boundary is only egress if the movement carried
 * at least one data class — a pure lifecycle event (e.g. a readiness check that
 * sends no content, or a policy-blocked call) is not a content transfer even
 * though its nominal destination is a provider.
 */
export function isContentEgress(event: DataMovementEvent): boolean {
  if (!isEgressBoundary(event.destination.boundary)) return false;
  if (event.result.status === "blocked") return false;
  return (event.data_classes?.length ?? 0) > 0;
}

/** One provider/model + data-class transfer, de-duplicated for the summary. */
export interface ProviderTransfer {
  key: string;
  boundary: DestinationBoundary;
  providerId: string | null;
  modelId: string | null;
  endpointClass: string | null;
  dataClasses: DataClass[];
}

/**
 * A redacted provider error surfaced to the user, grouped by
 * provider + error code so a burst of transient failures collapses to one row
 * with an occurrence `count` instead of one row per failed event.
 */
export interface RedactedError {
  key: string;
  providerId: string | null;
  errorCode: string | null;
  /** Redacted message from the most recent occurrence in the group. */
  message: string | null;
  /** Timestamp of the most recent occurrence in the group. */
  createdAtMs: number;
  /** How many failed events collapsed into this group (always ≥ 1). */
  count: number;
}

/** Saved-credential source/readiness summary derived from credential events. */
export interface CredentialSummary {
  key: string;
  providerId: string | null;
  sourceLabel: string | null;
  lastEventType: DataMovementEvent["event_type"];
  ready: boolean | null;
  createdAtMs: number;
}

/** Artifact lifecycle roll-up: what was written, exported, or deleted. */
export interface ArtifactLifecycle {
  written: number;
  loaded: number;
  exported: number;
  softDeleted: number;
  hardDeleted: number;
  deleteFailed: number;
}

/** The full report the UI renders. */
export interface SessionDataRouteReport {
  eventCount: number;
  /** Privacy modes observed across the ledger (usually one). */
  privacyModes: PrivacyMode[];
  /** Capture source labels (kind + optional label), de-duplicated. */
  captureSources: string[];
  /** Events that stayed on the device. */
  localEvents: DataMovementEvent[];
  /** Events that carried content off the device. */
  egressEvents: DataMovementEvent[];
  /** Whether any content left the device at all. */
  contentLeftDevice: boolean;
  /** De-duplicated provider/model transfers. */
  providerTransfers: ProviderTransfer[];
  /** Distinct data classes that left the device. */
  egressDataClasses: DataClass[];
  artifacts: ArtifactLifecycle;
  redactedErrors: RedactedError[];
  credentials: CredentialSummary[];
}

const CREDENTIAL_EVENT_TYPES: ReadonlySet<DataMovementEvent["event_type"]> =
  new Set([
    "credential_saved",
    "credential_deleted",
    "credential_source_changed",
    "provider_readiness_checked",
  ]);

function pushUnique<T>(list: T[], value: T): void {
  if (!list.includes(value)) list.push(value);
}

function captureSourceLabel(event: DataMovementEvent): string | null {
  const source = event.source;
  if (!source) return null;
  if (source.source_label) return `${source.kind}: ${source.source_label}`;
  return source.kind;
}

/**
 * Fold a session's data-movement ledger into a render-ready report.
 *
 * Input order is preserved for the local/egress event lists (the ledger is
 * append-ordered), while the provider/credential/error summaries are
 * de-duplicated by their salient identity so the UI shows one row per distinct
 * transfer.
 */
export function buildSessionDataRouteReport(
  events: DataMovementEvent[],
): SessionDataRouteReport {
  const privacyModes: PrivacyMode[] = [];
  const captureSources: string[] = [];
  const localEvents: DataMovementEvent[] = [];
  const egressEvents: DataMovementEvent[] = [];
  const egressDataClasses: DataClass[] = [];
  const artifacts: ArtifactLifecycle = {
    written: 0,
    loaded: 0,
    exported: 0,
    softDeleted: 0,
    hardDeleted: 0,
    deleteFailed: 0,
  };
  const transferByKey = new Map<string, ProviderTransfer>();
  const credentialByKey = new Map<string, CredentialSummary>();
  const redactedErrorByKey = new Map<string, RedactedError>();

  for (const event of events) {
    pushUnique(privacyModes, event.policy.privacy_mode);

    const sourceLabel = captureSourceLabel(event);
    if (sourceLabel) pushUnique(captureSources, sourceLabel);

    // Artifact lifecycle roll-up.
    switch (event.event_type) {
      case "artifact_written":
        artifacts.written += 1;
        break;
      case "artifact_loaded":
        artifacts.loaded += 1;
        break;
      case "artifact_exported":
        artifacts.exported += 1;
        break;
      case "artifact_soft_deleted":
        artifacts.softDeleted += 1;
        break;
      case "artifact_hard_deleted":
        artifacts.hardDeleted += 1;
        break;
      case "artifact_delete_failed":
        artifacts.deleteFailed += 1;
        break;
    }

    // Redacted provider errors (never a raw payload — the backend redacted it).
    // Grouped by provider + error code with an occurrence count so a burst of
    // transient failures (e.g. many provider_call_failed events) collapses to
    // one row instead of one DOM row per event (seed audio-graph-0bcf). Mirrors
    // the provider-transfer de-dup above; the group keeps the most recent
    // message/timestamp.
    if (
      event.result.status === "failed" &&
      (event.result.error_code || event.result.error_message_redacted)
    ) {
      const providerId =
        event.destination.provider_id ?? event.model?.provider_id ?? null;
      const errorCode = event.result.error_code ?? null;
      const errorKey = `${providerId ?? ""}|${errorCode ?? ""}`;
      const existing = redactedErrorByKey.get(errorKey);
      if (existing) {
        existing.count += 1;
        // Keep the most recent occurrence's message/timestamp.
        if (event.created_at_ms >= existing.createdAtMs) {
          existing.message = event.result.error_message_redacted ?? null;
          existing.createdAtMs = event.created_at_ms;
        }
      } else {
        redactedErrorByKey.set(errorKey, {
          key: errorKey,
          providerId,
          errorCode,
          message: event.result.error_message_redacted ?? null,
          createdAtMs: event.created_at_ms,
          count: 1,
        });
      }
    }

    // Saved-credential source/readiness summary.
    if (CREDENTIAL_EVENT_TYPES.has(event.event_type)) {
      const providerId =
        event.destination.provider_id ?? event.source?.source_id ?? null;
      const credKey = providerId ?? event.source?.kind ?? event.event_id;
      const ready =
        event.event_type === "provider_readiness_checked"
          ? event.result.status === "succeeded"
          : event.event_type === "credential_deleted"
            ? false
            : null;
      credentialByKey.set(credKey, {
        key: credKey,
        providerId,
        sourceLabel: event.source?.source_label ?? event.source?.kind ?? null,
        lastEventType: event.event_type,
        ready,
        createdAtMs: event.created_at_ms,
      });
    }

    // Local vs. egress split.
    if (isContentEgress(event)) {
      egressEvents.push(event);
      for (const cls of event.data_classes ?? []) {
        pushUnique(egressDataClasses, cls);
      }
      const providerId =
        event.destination.provider_id ?? event.model?.provider_id ?? null;
      const modelId = event.model?.model_id ?? null;
      const endpointClass = event.destination.endpoint_class ?? null;
      const transferKey = [
        event.destination.boundary,
        providerId ?? "",
        modelId ?? "",
        endpointClass ?? "",
      ].join("|");
      const existing = transferByKey.get(transferKey);
      if (existing) {
        for (const cls of event.data_classes ?? []) {
          pushUnique(existing.dataClasses, cls);
        }
      } else {
        transferByKey.set(transferKey, {
          key: transferKey,
          boundary: event.destination.boundary,
          providerId,
          modelId,
          endpointClass,
          dataClasses: [...(event.data_classes ?? [])],
        });
      }
    } else {
      localEvents.push(event);
    }
  }

  return {
    eventCount: events.length,
    privacyModes,
    captureSources,
    localEvents,
    egressEvents,
    contentLeftDevice: egressEvents.length > 0,
    providerTransfers: [...transferByKey.values()],
    egressDataClasses,
    artifacts,
    redactedErrors: [...redactedErrorByKey.values()],
    credentials: [...credentialByKey.values()],
  };
}
