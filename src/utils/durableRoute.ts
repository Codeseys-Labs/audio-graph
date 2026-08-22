/**
 * Durable notes-route classification (ADR-0028/0030/0034).
 *
 * `hasConfiguredDurableNotesRoute` originally lived inline in `App.tsx`
 * (its first-run/ExpressSetup-suppression gate). SHELL-R3 (plan §R3,
 * ADR-0046) pulls it out here, unchanged, so the NOW STRIP's planned-route
 * chip can read the SAME predicate App.tsx's own startup probe uses,
 * rather than re-deriving a parallel one that could silently drift.
 * `describePlannedRoute` is new in this unit: it turns a configured route
 * into the human-readable "{ASR display name} → {LLM display name}" label
 * the chip renders, using the same provider registry every other MVP-gate
 * read in this codebase uses (never a hand-rolled name).
 *
 * `blockingRouteLeg`/`preflightRouteForBlockingLeg` are settings T1 additions
 * (seed audio-graph-2b9a): `PreflightCard`'s Route row used to open Settings
 * bare (landing on "overview") for its fix action. `hasConfiguredDurableNotesRoute`
 * already knows exactly which leg — ASR or LLM — fails first; `blockingRouteLeg`
 * is that SAME check, refactored to report which leg instead of collapsing to a
 * bool (so the two can never drift against each other), and
 * `preflightRouteForBlockingLeg` turns the failing leg into a concrete
 * `{tab, fieldId}` via the settings route table — the shell already holds every
 * input (`settings`/`credentialPresence`/`modelStatus`), so this issues no new
 * provider egress (ADR-0028).
 */
import { providerDescriptorForSettingsVariant } from "../components/providerRegistryHelpers";
import {
  credentialRouteForProviderCredential,
  modelRouteForProviderId,
  providerRouteForProviderId,
} from "../components/settings/settingsRoutes";
import { endpointCredentialKey } from "../generated/endpointCredentialRouting";
import type {
  AppSettings,
  CredentialPresence,
  ModelStatus,
  SettingsRoute,
} from "../types";

export function isLoopbackEndpoint(endpoint: string): boolean {
  try {
    const hostname = new URL(endpoint).hostname.toLowerCase();
    return (
      hostname === "localhost" ||
      hostname === "127.0.0.1" ||
      hostname === "[::1]"
    );
  } catch {
    return false;
  }
}

/**
 * Passive first-run classification for the selected durable notes route.
 *
 * This deliberately does not call provider health/model-catalog endpoints:
 * startup may read local settings, key-presence metadata, and local model
 * status only (ADR-0028). A broad union of saved keys is not enough; each key
 * must belong to the provider/endpoint the user actually selected.
 */
/**
 * Which leg — ASR or LLM — is the first one `hasConfiguredDurableNotesRoute`
 * would fail on, or `null` if both are configured. `hasConfiguredDurableNotesRoute`
 * is now defined in terms of this function (not a parallel copy), so the two
 * can never drift against each other.
 */
export function blockingRouteLeg(
  settings: AppSettings | null | undefined,
  presence: readonly CredentialPresence[],
  modelStatus: ModelStatus | null,
  awsProfiles: readonly string[] = [],
): "asr" | "llm" | null {
  if (!settings) return "asr";
  const presentKeys = new Set(
    presence.filter(({ present }) => present).map(({ key }) => key),
  );
  const asr = settings.asr_provider;
  const asrDescriptor = providerDescriptorForSettingsVariant("asr", asr.type);
  if (
    !asrDescriptor?.ui_selectable ||
    asr.type !== "deepgram" ||
    !asr.model.trim() ||
    !asrDescriptor.credential_keys.some((key) => presentKeys.has(key))
  ) {
    return "asr";
  }

  const llm = settings.llm_provider;
  const llmDescriptor = providerDescriptorForSettingsVariant("llm", llm.type);
  if (!llmDescriptor?.ui_selectable) return "llm";

  const llmConfigured = ((): boolean => {
    switch (llm.type) {
      case "local_llama":
        return modelStatus?.llm === "Ready";
      case "mistralrs":
        // The aggregate `llm` status currently describes the fixed llama.cpp
        // artifact, not the selected mistral.rs model id. Stay conservative
        // until per-provider/model status is available.
        return false;
      case "api": {
        if (!llm.endpoint.trim() || !llm.model.trim()) return false;
        return (
          isLoopbackEndpoint(llm.endpoint) ||
          presentKeys.has(endpointCredentialKey(llm.endpoint))
        );
      }
      case "openrouter":
        return (
          Boolean(llm.base_url.trim() && llm.model.trim()) &&
          presentKeys.has("openrouter_api_key")
        );
      case "aws_bedrock":
        if (!llm.region.trim() || !llm.model_id.trim()) return false;
        if (llm.credential_source.type === "profile") {
          const profileName = llm.credential_source.name.trim();
          return Boolean(profileName) && awsProfiles.includes(profileName);
        }
        if (llm.credential_source.type === "access_keys") {
          return (
            presentKeys.has("aws_access_key") &&
            presentKeys.has("aws_secret_key")
          );
        }
        // The passive startup check cannot prove that an ambient AWS default
        // chain resolves; leave setup visible until a scoped audit does.
        return false;
    }
  })();

  return llmConfigured ? null : "llm";
}

export function hasConfiguredDurableNotesRoute(
  settings: AppSettings | null | undefined,
  presence: readonly CredentialPresence[],
  modelStatus: ModelStatus | null,
  awsProfiles: readonly string[] = [],
): boolean {
  return (
    blockingRouteLeg(settings, presence, modelStatus, awsProfiles) === null
  );
}

/**
 * The Settings address for the leg `blockingRouteLeg` names — the
 * PreflightCard Route row's fix action. Prefers the leg's credential route
 * (most failures are a missing key), falls back to its model route (e.g.
 * local_llama's "not Ready" case lives in the General tab's Models section),
 * then its plain provider route, then just the bare tab. Returns `undefined`
 * when the route is already fully configured (nothing to fix).
 *
 * Recorded divergence: `ExpressSetup.handleAdvanced` (triggered by
 * `asrNeedsKey`/`llmNeedsKey` — key specifically missing) routes via
 * `providerRouteForStage`, landing on the model/endpoint field rather than
 * the credential field. This function is per-LEG (any failure reason);
 * that one is per-CAUSE (key specifically). Both are spec-conformant
 * (synthesis §T1); this is a deliberate choice, not drift — flagged as a
 * candidate follow-up seed (per-cause rather than per-leg routing here too).
 */
export function preflightRouteForBlockingLeg(
  settings: AppSettings | null | undefined,
  presence: readonly CredentialPresence[],
  modelStatus: ModelStatus | null,
  awsProfiles: readonly string[] = [],
): SettingsRoute | undefined {
  if (!settings) return undefined;
  const leg = blockingRouteLeg(settings, presence, modelStatus, awsProfiles);
  if (!leg) return undefined;

  const variant =
    leg === "asr" ? settings.asr_provider.type : settings.llm_provider.type;
  const providerId = `${leg}.${variant}`;
  const route =
    credentialRouteForProviderCredential(providerId, null) ??
    modelRouteForProviderId(providerId) ??
    providerRouteForProviderId(providerId);

  return route
    ? { tab: route.tab, fieldId: route.fieldId, activate: route.activate }
    : { tab: leg === "asr" ? "stt" : "llm" };
}

/**
 * "{ASR display name} → {LLM display name}" for the NOW STRIP's planned-route
 * chip, e.g. "Deepgram streaming → OpenRouter". Registry-derived (never a
 * hand-rolled string) so it can't drift from the descriptor the settings UI
 * itself shows. Returns `null` when settings haven't hydrated yet or either
 * leg has no matching registry descriptor (should not happen for a valid
 * `AppSettings`, but a passive read must degrade rather than throw).
 */
export function describePlannedRoute(
  settings: AppSettings | null | undefined,
): string | null {
  if (!settings) return null;
  const asrDescriptor = providerDescriptorForSettingsVariant(
    "asr",
    settings.asr_provider.type,
  );
  const llmDescriptor = providerDescriptorForSettingsVariant(
    "llm",
    settings.llm_provider.type,
  );
  if (!asrDescriptor || !llmDescriptor) return null;
  return `${asrDescriptor.display_name} → ${llmDescriptor.display_name}`;
}
