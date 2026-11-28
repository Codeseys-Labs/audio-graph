import type { TFunction } from "i18next";
import { useId } from "react";
import type {
  CredentialPresence,
  EffectiveSttFidelity,
  ProviderDataClass,
  ProviderDescriptor,
  ProviderPolicyStatus,
  ProviderReadiness,
  ProviderReadinessNote,
  ProviderRuntimeReadiness,
  ProviderSensitiveErrorPolicy,
  SttFidelityDegradation,
} from "../types";
import { runtimeDiagnosticPresentation } from "../utils/runtimeDiagnostic";
import { providerRoadmapAuthLabel } from "./providerRegistryHelpers";
import type { ProviderSetupBlocker } from "./providerSetupModes";
import { readinessTone as statusTone } from "./settings/badgeTone";
import { readinessChipTone } from "./settings/readinessTone";

export type CredentialPresenceLookup = Partial<
  Record<string, CredentialPresence>
>;

interface ProviderReadinessPanelProps {
  entry: ProviderReadiness | null;
  descriptor?: ProviderDescriptor | null;
  credentialPresence?: CredentialPresenceLookup;
  loading: boolean;
  t: TFunction;
}

type ProviderCatalogKind = "models" | "voices" | "languages";

interface ProviderCatalogSummary {
  count: number;
  kind: ProviderCatalogKind;
}

interface ProviderSttFidelityPresentation {
  summary: string;
  note: string;
  rows: { label: string; value: string }[];
  limitations: string[];
}

const STT_TURN_DETECTION_FIELDS = [
  ["speech_start", "speechStart"],
  ["speech_final", "speechFinal"],
  ["endpointing_configured", "endpointing"],
  ["utterance_end", "utteranceEnd"],
  ["end_of_turn", "endOfTurn"],
  ["eager_end_of_turn", "eagerEndOfTurn"],
  ["turn_resume", "turnResume"],
] as const;

const STT_FIDELITY_DEGRADATIONS = new Set<SttFidelityDegradation>([
  "final_only_revisions",
  "app_estimated_timing",
  "timing_unavailable",
  "confidence_unavailable",
  "turn_unavailable",
  "speaker_unavailable",
  "speaker_disabled_by_configuration",
  "speaker_unavailable_for_selected_model",
  "speaker_remapped_by_configuration",
  "channel_unavailable",
  "capability_unverified",
]);
const STT_REVISION_VALUES = new Set(["final_only", "partial_and_final"]);
const STT_TIMING_VALUES = new Set([
  "unavailable",
  "app_estimated",
  "provider_coarse",
  "provider_exact",
  "unverified",
]);
const STT_EVIDENCE_VALUES = new Set([
  "unavailable",
  "app",
  "provider",
  "unverified",
]);

function isKnownSttFidelityValue(
  category: "revision" | "timing" | "evidence",
  value: unknown,
): value is string {
  const knownValues = {
    revision: STT_REVISION_VALUES,
    timing: STT_TIMING_VALUES,
    evidence: STT_EVIDENCE_VALUES,
  }[category];
  return typeof value === "string" && knownValues.has(value);
}

function sttFidelityValueLabel(
  category: "revision" | "timing" | "evidence",
  value: unknown,
  t: TFunction,
): string {
  return isKnownSttFidelityValue(category, value)
    ? t(`settings.providerReadiness.fidelity.value.${category}.${value}`)
    : t("settings.providerReadiness.fidelity.value.unknown");
}

function sttSpeakerFidelityLabel(
  fidelity: Partial<EffectiveSttFidelity>,
  degradations: ReadonlySet<string>,
  t: TFunction,
): string {
  if (degradations.has("speaker_disabled_by_configuration")) {
    return t(
      "settings.providerReadiness.fidelity.value.speaker.disabledByConfiguration",
    );
  }
  if (degradations.has("speaker_unavailable_for_selected_model")) {
    return t(
      "settings.providerReadiness.fidelity.value.speaker.unavailableForModel",
    );
  }
  if (degradations.has("speaker_remapped_by_configuration")) {
    return t(
      "settings.providerReadiness.fidelity.value.speaker.remappedByConfiguration",
    );
  }
  return sttFidelityValueLabel("evidence", fidelity.speaker, t);
}

function sttTurnDetectionRows(
  fidelity: Partial<EffectiveSttFidelity>,
  t: TFunction,
): { label: string; value: string }[] {
  const turnDetection = fidelity.turn_detection;
  return STT_TURN_DETECTION_FIELDS.map(([field, translationKey]) => ({
    label: t(
      `settings.providerReadiness.fidelity.field.turnDetection.${translationKey}`,
    ),
    value:
      turnDetection?.[field] === true
        ? t("settings.providerReadiness.fidelity.value.state.enabled")
        : field === "endpointing_configured" && turnDetection?.[field] === false
          ? t("settings.providerReadiness.fidelity.value.state.providerDefault")
          : turnDetection?.[field] === false
            ? t("settings.providerReadiness.fidelity.value.state.disabled")
            : t("settings.providerReadiness.fidelity.value.unknown"),
  }));
}

/**
 * Builds the complete user-facing selected-configuration fidelity view without
 * consulting provider ids or static registry capability flags. Unknown runtime
 * values collapse to localized, content-free copy instead of being echoed.
 */
export function providerSttFidelityPresentation(
  fidelity: EffectiveSttFidelity | null | undefined,
  t: TFunction,
): ProviderSttFidelityPresentation | null {
  if (!fidelity || typeof fidelity !== "object") return null;

  const rawDegradations = Array.isArray(fidelity.degradations)
    ? (fidelity.degradations as unknown[])
    : [];
  const degradationsComplete = Array.isArray(fidelity.degradations);
  const degradationCodes = new Set(
    rawDegradations.filter(
      (value): value is string => typeof value === "string",
    ),
  );
  const hasUnknownDegradation = rawDegradations.some(
    (value) =>
      typeof value !== "string" ||
      !STT_FIDELITY_DEGRADATIONS.has(value as SttFidelityDegradation),
  );
  const hasUnknownTypedValue =
    !isKnownSttFidelityValue("revision", fidelity.revision_semantics) ||
    !isKnownSttFidelityValue("timing", fidelity.timing) ||
    !isKnownSttFidelityValue("evidence", fidelity.confidence) ||
    !isKnownSttFidelityValue("evidence", fidelity.turn) ||
    !isKnownSttFidelityValue("evidence", fidelity.speaker) ||
    !isKnownSttFidelityValue("evidence", fidelity.channel);
  const turnDetectionComplete =
    typeof fidelity.turn_detection === "object" &&
    fidelity.turn_detection !== null &&
    STT_TURN_DETECTION_FIELDS.every(
      ([field]) => typeof fidelity.turn_detection[field] === "boolean",
    );
  const incomplete =
    !degradationsComplete ||
    hasUnknownDegradation ||
    hasUnknownTypedValue ||
    !turnDetectionComplete;
  const hasReducedTypedValue =
    fidelity.revision_semantics !== "partial_and_final" ||
    fidelity.timing === "app_estimated" ||
    fidelity.timing === "unavailable" ||
    fidelity.timing === "unverified" ||
    [
      fidelity.confidence,
      fidelity.turn,
      fidelity.speaker,
      fidelity.channel,
    ].some((value) => value === "unavailable" || value === "unverified");
  const reduced =
    degradationCodes.size > 0 ||
    hasUnknownDegradation ||
    hasUnknownTypedValue ||
    hasReducedTypedValue;

  const limitations = rawDegradations.map((value) => {
    if (
      typeof value === "string" &&
      STT_FIDELITY_DEGRADATIONS.has(value as SttFidelityDegradation)
    ) {
      return t(`settings.providerReadiness.fidelity.degradation.${value}`);
    }
    return t("settings.providerReadiness.fidelity.degradation.unknown");
  });

  return {
    summary: t(
      incomplete
        ? "settings.providerReadiness.fidelity.summary.incomplete"
        : reduced
          ? "settings.providerReadiness.fidelity.summary.reduced"
          : "settings.providerReadiness.fidelity.summary.full",
    ),
    note: t(
      incomplete
        ? "settings.providerReadiness.fidelity.noteIncomplete"
        : "settings.providerReadiness.fidelity.note",
    ),
    rows: [
      {
        label: t("settings.providerReadiness.fidelity.field.revisions"),
        value: sttFidelityValueLabel(
          "revision",
          fidelity.revision_semantics,
          t,
        ),
      },
      {
        label: t("settings.providerReadiness.fidelity.field.timing"),
        value: sttFidelityValueLabel("timing", fidelity.timing, t),
      },
      {
        label: t("settings.providerReadiness.fidelity.field.confidence"),
        value: sttFidelityValueLabel("evidence", fidelity.confidence, t),
      },
      {
        label: t("settings.providerReadiness.fidelity.field.turns"),
        value: sttFidelityValueLabel("evidence", fidelity.turn, t),
      },
      {
        label: t("settings.providerReadiness.fidelity.field.speakers"),
        value: sttSpeakerFidelityLabel(fidelity, degradationCodes, t),
      },
      {
        label: t("settings.providerReadiness.fidelity.field.channels"),
        value: sttFidelityValueLabel("evidence", fidelity.channel, t),
      },
      ...sttTurnDetectionRows(fidelity, t),
    ],
    limitations: [...new Set(limitations)],
  };
}

export function ProviderSttFidelityDetails({
  fidelity,
  regionLabel,
  t,
}: {
  fidelity: EffectiveSttFidelity | null | undefined;
  regionLabel?: string;
  t: TFunction;
}) {
  const presentation = providerSttFidelityPresentation(fidelity, t);
  if (!presentation) return null;
  const title = t("settings.providerReadiness.fidelity.title");

  return (
    <section
      className="settings-provider-readiness__fidelity"
      aria-label={regionLabel ?? title}
    >
      <p className="settings-provider-readiness__fidelity-title">
        <strong>{title}</strong>
      </p>
      <p className="settings-provider-readiness__fidelity-summary">
        <strong>{presentation.summary}</strong> {presentation.note}
      </p>
      <dl className="settings-provider-readiness__detail-grid">
        {presentation.rows.map((row) => (
          <div key={row.label}>
            <dt>{row.label}</dt>
            <dd>{row.value}</dd>
          </div>
        ))}
      </dl>
      {presentation.limitations.length > 0 && (
        <ul
          className="settings-provider-readiness__fidelity-limitations"
          aria-label={t("settings.providerReadiness.fidelity.limitationsLabel")}
        >
          {presentation.limitations.map((limitation) => (
            <li key={limitation}>{limitation}</li>
          ))}
        </ul>
      )}
    </section>
  );
}

export function providerCatalogSummary(
  entry: ProviderReadiness,
): ProviderCatalogSummary | null {
  const voiceCatalogCount = entry.voice_catalog?.length ?? 0;
  if (voiceCatalogCount > 0) {
    return { count: voiceCatalogCount, kind: "voices" };
  }

  const languageCatalogCount = entry.language_catalog?.length ?? 0;
  if (languageCatalogCount > 0) {
    return { count: languageCatalogCount, kind: "languages" };
  }

  if (typeof entry.model_count === "number") {
    return { count: entry.model_count, kind: "models" };
  }

  const modelCatalogCount = entry.model_catalog?.length ?? 0;
  if (modelCatalogCount > 0) {
    return { count: modelCatalogCount, kind: "models" };
  }

  const openrouterModelCount = entry.openrouter_models?.length ?? 0;
  return openrouterModelCount > 0
    ? { count: openrouterModelCount, kind: "models" }
    : null;
}

export function providerCatalogLabel(
  summary: ProviderCatalogSummary | null,
  t: TFunction,
): string | null {
  return summary
    ? t(`settings.providerReadiness.${summary.kind}`, {
        count: summary.count,
      })
    : null;
}

function hasCredentialSlots(entry: ProviderReadiness): boolean {
  return entry.credentials.length > 0;
}

function hasPresentCredential(entry: ProviderReadiness): boolean {
  return entry.credentials.some((credential) => credential.present);
}

function formatCheckedAt(value: number | null | undefined): string | null {
  if (typeof value !== "number" || !Number.isFinite(value) || value <= 0) {
    return null;
  }
  return new Date(value).toLocaleString();
}

function providerDataClassLabel(dataClass: ProviderDataClass, t: TFunction) {
  return t(`settings.providerReadiness.dataClass.${dataClass}`);
}

function providerDataClassListLabel(
  dataClasses: readonly ProviderDataClass[],
  t: TFunction,
): string {
  if (dataClasses.length === 0) {
    return t("settings.providerReadiness.dataClass.none");
  }

  return dataClasses
    .map((dataClass) => providerDataClassLabel(dataClass, t))
    .join(", ");
}

function providerPolicyStatusLabel(
  status: ProviderPolicyStatus,
  t: TFunction,
): string {
  return t(`settings.providerReadiness.policyStatus.${status}`);
}

function providerSensitiveErrorPolicyLabel(
  policy: ProviderSensitiveErrorPolicy,
  t: TFunction,
): string {
  return t(`settings.providerReadiness.sensitiveErrorPolicy.${policy}`);
}

// Backend `CredentialPresence.source` values that have a localized label.
// `os_keychain`/`imported_file`/`file_fallback`/`file_override`/
// `credentials_yaml`/`missing` are all emitted by `source_for()` on the live
// `load_credential_presence_cmd` IPC path (and the parity is enforced by
// `credentialSourceContract.test.ts`). `error` is the one exception: it is a
// DEFENSIVE UI fallback only — the live load path returns
// `a typed credential diagnostic` on a read failure rather than a presence row
// with `source: "error"`, so the backend never actually emits it today. The
// label is kept so a future backend that does surface a per-key read error has
// a localized string instead of a raw passthrough. (The only `"source":
// "error"` literal in `credentials/mod.rs` lives inside `#[cfg(test)]`, which
// the contract test strips, so it is not part of the live source vocabulary.)
const LOCALIZED_CREDENTIAL_SOURCES = new Set([
  "os_keychain",
  "imported_file",
  "file_fallback",
  "file_override",
  "credentials_yaml",
  "missing",
  "error",
]);

export function credentialSourceLabel(
  source: string | undefined,
  t: TFunction,
): string {
  if (source && LOCALIZED_CREDENTIAL_SOURCES.has(source)) {
    return t(`settings.providerReadiness.credentialSource.${source}`);
  }
  return (
    source?.trim() || t("settings.providerReadiness.credentialSource.unknown")
  );
}

// A readiness entry says one thing, in closed fields: a typed `diagnostic` when
// the check failed, otherwise its `note`. Both are localized here and nowhere
// else, so every surface that shows readiness (Settings panels, the
// credentials rollup, capability cards, Express setup) reads the same copy.
function providerReadinessNoteMessage(
  entry: ProviderReadiness,
  t: TFunction,
): string {
  const note: ProviderReadinessNote = entry.note;
  switch (note.kind) {
    case "ready_to_check":
    case "aws_profile_required":
    case "vertex_project_required":
    case "vertex_not_probed":
    case "no_automatic_probe":
    case "local_models_managed":
    case "not_selected":
    case "cancelled":
    case "check_in_progress":
    case "failed":
      return t(`settings.providerReadiness.note.${note.kind}`);
    case "missing_credentials":
      // An empty list would read as a sentence with a hole; the status label
      // already says the same thing without one.
      return note.keys.length > 0
        ? t("settings.providerReadiness.note.missing_credentials", {
            count: note.keys.length,
            keys: note.keys.join(", "),
          })
        : t("settings.providerReadiness.status.missing_credentials");
    case "local_models": {
      const parts = [
        note.ready === 0
          ? t("settings.providerReadiness.note.local_models_none_ready", {
              total: note.total,
            })
          : t("settings.providerReadiness.note.local_models", {
              ready: note.ready,
              total: note.total,
            }),
      ];
      if (note.missing.length > 0) {
        parts.push(
          t("settings.providerReadiness.note.local_models_missing", {
            ids: note.missing.join(", "),
          }),
        );
      }
      if (note.invalid.length > 0) {
        parts.push(
          t("settings.providerReadiness.note.local_models_invalid", {
            ids: note.invalid.join(", "),
          }),
        );
      }
      if (note.planned) {
        parts.push(t("settings.providerReadiness.note.local_models_planned"));
      }
      return parts.join(" ");
    }
    case "check_rate_limited":
      return t("settings.providerReadiness.note.check_rate_limited", {
        count: note.retry_after_secs,
      });
    case "verified": {
      // `model_count` is the catalog size for any catalog kind (a voice
      // catalog is counted too), so only claim "models" when the entry's own
      // catalog is a model catalog.
      const catalog = providerCatalogSummary(entry);
      return catalog?.kind === "models"
        ? t("settings.providerReadiness.note.verified_models", {
            count: catalog.count,
          })
        : t("settings.providerReadiness.note.verified");
    }
    default: {
      // A note kind this build does not know (newer backend): the localized
      // status label is the safe, closed statement.
      const unknownNote: never = note;
      void unknownNote;
      return t(`settings.providerReadiness.status.${entry.status}`);
    }
  }
}

export function providerReadinessMessage(
  entry: ProviderReadiness,
  t: TFunction,
): string {
  return entry.diagnostic
    ? t(runtimeDiagnosticPresentation(entry.diagnostic).translationKey)
    : providerReadinessNoteMessage(entry, t);
}

/**
 * A local runtime check, from its closed `status` and the static registry/build
 * values (`required_feature`, `model_id`, `runtime_version`) it concerns.
 */
export function providerRuntimeMessage(
  runtime: ProviderRuntimeReadiness,
  t: TFunction,
): string {
  const feature = runtime.required_feature || null;
  const model = runtime.model_id || null;
  const key = (name: string) =>
    `settings.providerReadiness.runtimeDetail.${name}`;
  let sentence: string;
  switch (runtime.status) {
    case "feature_missing":
      sentence = feature
        ? t(key("feature_missing"), { feature })
        : t(key("feature_missing_unnamed"));
      break;
    case "model_missing":
      sentence = model
        ? t(key("model_missing"), { model })
        : t(key("model_missing_unnamed"));
      break;
    case "runtime_unavailable":
      sentence = t(key("runtime_unavailable"));
      break;
    case "load_failed":
      sentence = model
        ? t(key("load_failed"), { model })
        : t(key("load_failed_unnamed"));
      break;
    case "healthy":
      sentence = model
        ? t(key("healthy"), { model })
        : t(key("healthy_unnamed"));
      break;
    default: {
      const unknownStatus: never = runtime.status;
      void unknownStatus;
      return t("settings.providerReadiness.unknown");
    }
  }
  return runtime.runtime_version
    ? `${sentence} ${t(key("version"), { version: runtime.runtime_version })}`
    : sentence;
}

/**
 * A setup blocker, presented from the readiness entry or runtime check it
 * carries when it has one; every other blocker keeps its own summary.
 */
export function providerSetupBlockerMessage(
  blocker: ProviderSetupBlocker,
  t: TFunction,
): string {
  if (blocker.runtime) return providerRuntimeMessage(blocker.runtime, t);
  if (blocker.readiness) return providerReadinessMessage(blocker.readiness, t);
  return blocker.message;
}

// The one "what to do next" rule for a failed check, decided from the typed
// cause's recovery action alone (never from message text): the same for ASR,
// TTS, LLM and AWS entries.
function providerFailureRecovery(
  entry: ProviderReadiness,
  t: TFunction,
): string | null {
  if (!entry.diagnostic) return null;
  const presentation = runtimeDiagnosticPresentation(entry.diagnostic);
  // A credential-store failure (locked, missing, corrupt) says what to do in
  // its own sentence; replacing the provider key would not help.
  if (presentation.diagnostic.kind === "credential") {
    return t(presentation.translationKey);
  }
  const recovery = (step: string) =>
    t(`settings.providerReadiness.recovery.${step}`);
  // Only a rejected or expired credential, or a refused permission, points at
  // the key; every other cause says what would actually help.
  switch (presentation.recoveryAction) {
    case "reenter_credential":
      return recovery("credentialRejected");
    // Replacing the key fixes neither an expired credential...
    case "refresh_credential":
      return t("runtimeDiagnostics.runtime.credential_expired");
    // ...nor a valid one the provider will not let do this. A depleted quota
    // or balance (HTTP 402) is not a settings problem either: its own
    // sentence says to check the plan.
    case "review_configuration": {
      const { diagnostic } = presentation;
      if (
        diagnostic.kind === "runtime" &&
        (diagnostic.detail.code === "permission_denied" ||
          diagnostic.detail.code === "capacity_exhausted")
      ) {
        return t(`runtimeDiagnostics.runtime.${diagnostic.detail.code}`);
      }
      return recovery("reviewConfiguration");
    }
    case "retry":
    case "retry_after_delay":
      return recovery("retryLater");
    case "check_network":
      return recovery("checkNetwork");
    case "choose_supported_provider":
      return recovery("chooseSupportedProvider");
    case "review_policy":
      return recovery("reviewPolicy");
    case "restart_application":
      return recovery("restartApplication");
    case "none":
      return recovery("runChecksAgain");
    default:
      return null;
  }
}

export function providerRecoveryAction(
  entry: ProviderReadiness,
  t: TFunction,
  descriptor?: ProviderDescriptor | null,
): string | null {
  switch (entry.status) {
    case "missing_credentials":
      return t("settings.providerReadiness.recovery.missingCredentials");
    case "error":
      return (
        providerFailureRecovery(entry, t) ??
        (hasPresentCredential(entry)
          ? t("settings.providerReadiness.recovery.errorWithCredentials")
          : t("settings.providerReadiness.recovery.errorWithoutCredentials"))
      );
    case "unchecked":
      if (!hasCredentialSlots(entry) || !hasPresentCredential(entry)) {
        return null;
      }
      if (entry.automatic_probe_available === false) {
        return null;
      }
      if (
        entry.automatic_probe_available !== true &&
        descriptor &&
        !descriptor.health_check_command
      ) {
        return null;
      }
      return t("settings.providerReadiness.recovery.uncheckedWithCredentials");
    case "ready":
      return null;
  }
}

interface ProviderReadinessDetailsProps {
  entry: ProviderReadiness;
  credentialPresence?: CredentialPresenceLookup;
  t: TFunction;
}

export function ProviderReadinessDetails({
  entry,
  credentialPresence,
  t,
}: ProviderReadinessDetailsProps) {
  const catalogSummary = providerCatalogSummary(entry);

  return (
    <details className="settings-provider-readiness__details">
      <summary>{t("settings.providerReadiness.details")}</summary>
      <dl className="settings-provider-readiness__detail-grid">
        <div>
          <dt>{t("settings.providerReadiness.lastChecked")}</dt>
          <dd>
            {formatCheckedAt(entry.checked_at) ??
              t("settings.providerReadiness.notChecked")}
          </dd>
        </div>
        <div>
          <dt>{t("settings.providerReadiness.catalog")}</dt>
          <dd>
            {providerCatalogLabel(catalogSummary, t) ??
              t("settings.providerReadiness.noCatalog")}
          </dd>
        </div>
        {entry.runtime && (
          <div>
            <dt>{t("settings.providerReadiness.runtime")}</dt>
            <dd>
              {t(
                `settings.providerReadiness.runtimeStatus.${entry.runtime.status}`,
              )}
              {`: ${providerRuntimeMessage(entry.runtime, t)}`}
            </dd>
          </div>
        )}
      </dl>
      {entry.credentials.length > 0 && (
        <dl className="settings-provider-readiness__credential-list">
          <dt>{t("settings.providerReadiness.credentials")}</dt>
          {entry.credentials.map((credential) => {
            const presence = credentialPresence?.[credential.key];
            const present = presence?.present ?? credential.present;
            const source = presence?.source?.trim()
              ? presence.source
              : present
                ? undefined
                : "missing";
            return (
              <dd key={credential.key}>
                <code>{credential.key}</code>
                <span>
                  {present
                    ? t("settings.providerReadiness.credentialPresent")
                    : t("settings.providerReadiness.credentialMissing")}
                </span>
                <span>{credentialSourceLabel(source, t)}</span>
              </dd>
            );
          })}
        </dl>
      )}
    </details>
  );
}

export default function ProviderReadinessPanel({
  entry,
  descriptor,
  credentialPresence,
  loading,
  t,
}: ProviderReadinessPanelProps) {
  // Stable id so the free-form backend not-selectable reason is programmatically
  // associated with the roadmap status rather than appended as a bare string.
  // Declared before the early return so the hook order stays unconditional.
  const notSelectableReasonId = useId();

  if (!entry && !descriptor && !loading) return null;

  const catalogSummary = entry ? providerCatalogSummary(entry) : null;
  const message = entry ? providerReadinessMessage(entry, t) : null;
  const recoveryAction = entry
    ? providerRecoveryAction(entry, t, descriptor)
    : null;
  // An expired credential or a refused permission is its own next step: the
  // recovery copy for it is the sentence already shown as the message.
  const nextStep = recoveryAction === message ? null : recoveryAction;
  // The tone LAW (audio-graph-2554, settings T2; folded in for T3 /
  // audio-graph-9d2b — seed 73bf): this panel always describes the provider
  // actually selected right now (every call site passes the ACTIVE
  // provider's readiness — AsrProviderSettings/LlmProviderSettings/TtsPanel/
  // GeminiSettings), so Axis 3's `active` gate is unconditionally true here.
  // What still needs gating is `stale`/`automatic_probe_available`: a cached
  // or unverifiable "ready" must not render the label OR the
  // `--tint-success` class as a bare "Ready" with a merely APPENDED stale
  // sentence — the whole claim demotes together.
  const readinessChip = entry
    ? readinessChipTone(
        {
          status: entry.status,
          stale: entry.stale,
          automaticProbeAvailable: entry.automatic_probe_available,
          active: true,
        },
        statusTone,
      )
    : null;
  const effectiveEntryStatus = readinessChip?.effectiveStatus ?? null;

  return (
    <div
      className={`settings-provider-readiness ${
        effectiveEntryStatus
          ? `settings-provider-readiness--${effectiveEntryStatus}`
          : ""
      }`}
    >
      <div
        className="settings-provider-readiness__status"
        role="status"
        aria-live="polite"
        aria-atomic="true"
        aria-busy={loading}
      >
        <div className="settings-provider-readiness__main">
          <span className="settings-provider-readiness__label">
            {effectiveEntryStatus
              ? t(`settings.providerReadiness.status.${effectiveEntryStatus}`)
              : loading
                ? t("settings.providerReadiness.checking")
                : t("settings.providerReadiness.status.unchecked")}
          </span>
          {loading && (
            <span className="settings-provider-readiness__checking">
              {t("settings.providerReadiness.checking")}
            </span>
          )}
        </div>
        {entry && (
          <p className="settings-provider-readiness__message">
            {message}
            {entry.stale ? ` ${t("settings.providerReadiness.stale")}` : ""}
            {catalogSummary
              ? ` ${providerCatalogLabel(catalogSummary, t)}`
              : ""}
          </p>
        )}
      </div>
      {nextStep && (
        <p className="settings-provider-readiness__recovery">
          <span>{t("settings.providerReadiness.recoveryLabel")}</span>{" "}
          {nextStep}
        </p>
      )}
      {entry && (
        <ProviderSttFidelityDetails
          fidelity={entry.effective_stt_fidelity}
          regionLabel={t(
            "settings.providerReadiness.fidelity.regionLabel.active",
          )}
          t={t}
        />
      )}
      {entry && (
        <ProviderReadinessDetails
          entry={entry}
          credentialPresence={credentialPresence}
          t={t}
        />
      )}
      {descriptor && (
        <dl className="settings-provider-readiness__metadata">
          <div>
            <dt>{t("settings.providerReadiness.metadata.data")}</dt>
            <dd>
              {t(
                `settings.providerReadiness.dataBoundary.${descriptor.privacy.data_boundary}`,
              )}
            </dd>
          </div>
          <div>
            <dt>{t("settings.providerReadiness.metadata.sent")}</dt>
            <dd>
              {providerDataClassListLabel(
                descriptor.privacy.data_classes_sent,
                t,
              )}
            </dd>
          </div>
          <div>
            <dt>{t("settings.providerReadiness.metadata.returned")}</dt>
            <dd>
              {providerDataClassListLabel(
                descriptor.privacy.data_classes_returned,
                t,
              )}
            </dd>
          </div>
          <div>
            <dt>{t("settings.providerReadiness.metadata.healthCheck")}</dt>
            <dd>
              {providerDataClassListLabel(
                descriptor.privacy.health_check_data_classes,
                t,
              )}
            </dd>
          </div>
          <div>
            <dt>{t("settings.providerReadiness.metadata.cloudTransfer")}</dt>
            <dd>
              {descriptor.privacy.cloud_transfer_acknowledgement_required
                ? t("settings.providerReadiness.cloudTransfer.required")
                : t("settings.providerReadiness.cloudTransfer.notRequired")}
            </dd>
          </div>
          <div>
            <dt>{t("settings.providerReadiness.metadata.retention")}</dt>
            <dd>
              {providerPolicyStatusLabel(
                descriptor.privacy.retention_policy,
                t,
              )}
            </dd>
          </div>
          <div>
            <dt>{t("settings.providerReadiness.metadata.training")}</dt>
            <dd>
              {providerPolicyStatusLabel(descriptor.privacy.training_policy, t)}
            </dd>
          </div>
          <div>
            <dt>{t("settings.providerReadiness.metadata.deletion")}</dt>
            <dd>
              {providerPolicyStatusLabel(descriptor.privacy.deletion_policy, t)}
            </dd>
          </div>
          <div>
            <dt>{t("settings.providerReadiness.metadata.residency")}</dt>
            <dd>
              {providerPolicyStatusLabel(descriptor.privacy.data_residency, t)}
            </dd>
          </div>
          <div>
            <dt>{t("settings.providerReadiness.metadata.noTraining")}</dt>
            <dd>
              {providerPolicyStatusLabel(
                descriptor.privacy.enterprise_no_training_config,
                t,
              )}
            </dd>
          </div>
          <div>
            <dt>{t("settings.providerReadiness.metadata.errors")}</dt>
            <dd>
              {providerSensitiveErrorPolicyLabel(
                descriptor.privacy.sensitive_error_policy,
                t,
              )}
            </dd>
          </div>
          <div>
            <dt>{t("settings.providerReadiness.metadata.processor")}</dt>
            <dd>
              {descriptor.privacy.processor_identity ??
                t("settings.providerReadiness.unknown")}
            </dd>
          </div>
          <div>
            <dt>{t("settings.providerReadiness.metadata.policy")}</dt>
            <dd>
              {descriptor.privacy.policy_url ??
                t("settings.providerReadiness.noPolicyUrl")}
            </dd>
          </div>
          <div>
            <dt>{t("settings.providerReadiness.metadata.session")}</dt>
            <dd>
              {t(
                `settings.providerReadiness.session.${descriptor.lifecycle.session}`,
              )}
            </dd>
          </div>
          <div>
            <dt>{t("settings.providerReadiness.metadata.auth")}</dt>
            <dd>
              {providerRoadmapAuthLabel(descriptor) ??
                t(
                  `settings.providerReadiness.auth.${descriptor.lifecycle.auth}`,
                )}
            </dd>
          </div>
          {descriptor.roadmap && (
            <div>
              <dt>{t("settings.providerReadiness.roadmap")}</dt>
              <dd
                aria-describedby={
                  descriptor.roadmap.not_selectable_reason
                    ? notSelectableReasonId
                    : undefined
                }
              >
                <span>
                  {t(
                    `settings.providerReadiness.roadmapStatus.${descriptor.status}`,
                  )}
                </span>
                {descriptor.roadmap.not_selectable_reason && (
                  <span id={notSelectableReasonId}>
                    {" "}
                    <span className="settings-provider-readiness__not-selectable-label">
                      {t("settings.providerReadiness.notSelectableReasonLabel")}
                    </span>{" "}
                    {descriptor.roadmap.not_selectable_reason}
                  </span>
                )}
              </dd>
            </div>
          )}
          {descriptor.lifecycle.keepalive !== "none" && (
            <div>
              <dt>{t("settings.providerReadiness.metadata.keepalive")}</dt>
              <dd>
                {t(
                  `settings.providerReadiness.keepalive.${descriptor.lifecycle.keepalive}`,
                )}
              </dd>
            </div>
          )}
          {descriptor.lifecycle.close !== "noop" && (
            <div>
              <dt>{t("settings.providerReadiness.metadata.close")}</dt>
              <dd>
                {t(
                  `settings.providerReadiness.close.${descriptor.lifecycle.close}`,
                )}
              </dd>
            </div>
          )}
        </dl>
      )}
    </div>
  );
}
