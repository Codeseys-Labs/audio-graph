/**
 * Settings route table — pure data/lookup extraction (seed audio-graph-2b9a,
 * T1 "give every setting an address", synthesis
 * `docs/agentic-runs/2026-08-21-settings-ux-design/synthesis.md` §T1).
 *
 * Every one of these functions was previously a closure inside
 * `useSettingsController.tsx`, closed over `dispatch`/`setTtsType` for its
 * `apply` side effect. This module holds the SAME decisions (which tab, which
 * fieldId, whether to activate, and — when navigating should also mutate a
 * provider selection — which mutation) as pure, side-effect-free functions.
 * The mutation itself is expressed as a serializable `ApplyAction` rather
 * than a bound closure, so this module never needs `dispatch` and stays a
 * plain data/lookup table the controller (or a future non-controller
 * consumer, e.g. T4b's jump palette) can import without pulling in the
 * entire settings reducer.
 *
 * `useSettingsController.tsx` re-wraps every function here that used to have
 * an `apply` closure: it resolves the returned `applyAction` into the real
 * `dispatch(setField(...))` call and attaches it as `apply` before handing
 * the route to the rest of the controller. The wrapped functions keep their
 * pre-extraction names and signatures (`credentialRouteForProviderCredential`,
 * `credentialRouteForKey`, `providerRouteForProviderId`,
 * `modelRouteForProviderId`, `credentialRouteForProviderSetupSelection`,
 * `credentialRouteForReadiness`) so every existing call site and test that
 * reads them off the controller's return value is unaffected.
 *
 * `providerRouteForStage(stage, variant)` is new: it is
 * `providerRouteForProviderId(`${stage}.${variant}`)` under a name that
 * doesn't require the caller to know the `"stage.variant"` provider-id
 * string convention — used by `ExpressSetup`'s "Advanced" handoff, which
 * only knows a stage + a settings-variant string, not a registry provider id.
 *
 * `ROUTE_INDEX` flattens every `{tab, fieldId}` pair this module can produce
 * (deduplicated) — the drift tripwire test in `SettingsPage.test.tsx`
 * ("ROUTE_INDEX drift tripwire") mounts each tab and asserts
 * `document.getElementById(fieldId)` resolves for every entry reachable
 * under the default fixture, so a route entry that stops matching a real DOM
 * node fails a test instead of silently misrouting (synthesis §T1 RISK).
 */
import type { ProviderReadiness } from "../../types";
import { PROVIDER_DESCRIPTORS } from "../providerRegistryHelpers";
import type { AsrType, LlmType } from "../settingsTypes";
import type { SettingsTab } from "./settingsRailConfig";

/** Settings-variant string for the two TTS provider choices this table routes. */
export type TtsRouteVariant = "none" | "deepgram_aura";

/**
 * A serializable description of the provider-selection mutation a route's
 * `apply` used to perform inline. `useSettingsController.tsx` is the only
 * consumer that turns these back into real `dispatch` calls (it owns
 * `dispatch`/`setTtsType`); this module never executes them.
 */
export type ApplyAction =
  | { kind: "asr_variant"; variant: AsrType }
  | { kind: "llm_variant"; variant: LlmType }
  // asr.aws_transcribe's CREDENTIAL route (not its plain provider route) also
  // flips the AWS auth-mode toggle to access_keys so the key fields render.
  | { kind: "asr_aws_transcribe_credential" }
  // llm.aws_bedrock's CREDENTIAL route mirrors the above for Bedrock.
  | { kind: "llm_aws_bedrock_credential" }
  | { kind: "gemini_api_key" }
  | { kind: "gemini_vertex_ai" }
  | { kind: "tts_variant"; variant: TtsRouteVariant };

/** Pure route decision: where to land, and (optionally) what selection to apply. */
export interface RouteEntry {
  tab: SettingsTab;
  fieldId: string;
  activate?: boolean;
  applyAction?: ApplyAction;
}

/** Live settings-session context `credentialRouteForKey` needs to resolve
 * "which of the several providers that could use this credential is the one
 * actually selected right now" — passed explicitly instead of closed over,
 * so this module stays pure. */
export interface RouteContext {
  activeReadinessProviderIds: readonly string[];
  providerReadinessEntries: readonly ProviderReadiness[];
  asrType: AsrType;
  llmType: LlmType;
  /** `endpointCredentialKey(asrEndpoint)` result for the current `asr.api` endpoint. */
  asrEndpointCredentialKey: string | null;
  /** `endpointCredentialKey(llmEndpoint)` result for the current `llm.api` endpoint. */
  llmEndpointCredentialKey: string | null;
}

function firstCredentialKey(entry: ProviderReadiness): string | null {
  return entry.credentials[0]?.key ?? null;
}

/**
 * "Add or replace this provider's credential" route — the annotated-chooser
 * / credential-health-row destination. Distinct from
 * `providerRouteForProviderId` (below) because a couple of providers need an
 * extra selection mutation here (AWS auth-mode toggles) that the plain
 * provider route does not: navigating here should also make the credential
 * fields render, not just select the provider.
 */
export function credentialRouteForProviderCredential(
  providerId: string,
  credentialKey: string | null,
): RouteEntry | null {
  switch (providerId) {
    case "asr.api":
      return {
        tab: "stt",
        fieldId: "asr-api-key",
        activate: true,
        applyAction: { kind: "asr_variant", variant: "api" },
      };
    case "asr.openai_realtime":
      return {
        tab: "stt",
        fieldId: "openai-realtime-api-key",
        activate: true,
        applyAction: { kind: "asr_variant", variant: "openai_realtime" },
      };
    case "realtime_agent.openai_realtime":
      // Native voice-agent OpenAI credential. This is the
      // realtime_agent.openai_realtime provider (native voice agent), NOT
      // asr.openai_realtime (pipeline STT). Route to the Realtime-agent tab's
      // capability card (where the native agent + its OpenAI credential
      // live), NOT the STT tab's `openai-realtime-api-key` field — that field
      // only renders when `asrType === "openai_realtime"`, so pointing here
      // used to FORCE re-selecting the pipeline STT provider, silently
      // rewriting the user's saved `asr_provider` on the next Save (the
      // pipeline-STT vs native-agent split-brain). No `applyAction`: mirrors
      // the sibling `realtime_agent.gemini_live` route below, which
      // navigates without mutating state.
      if (credentialKey !== "openai_api_key") return null;
      return {
        tab: "gemini",
        fieldId: "settings-provider-capability-realtime_agent.openai_realtime",
        activate: true,
      };
    case "asr.deepgram":
      return {
        tab: "stt",
        fieldId: "deepgram-api-key",
        activate: true,
        applyAction: { kind: "asr_variant", variant: "deepgram" },
      };
    case "asr.assemblyai":
      return {
        tab: "stt",
        fieldId: "assemblyai-api-key",
        activate: true,
        applyAction: { kind: "asr_variant", variant: "assemblyai" },
      };
    case "asr.aws_transcribe":
      return {
        tab: "stt",
        fieldId: "aws-asr-access-key",
        activate: true,
        applyAction: { kind: "asr_aws_transcribe_credential" },
      };
    case "llm.api":
      return {
        tab: "llm",
        fieldId: "llm-custom-api-key",
        activate: true,
        applyAction: { kind: "llm_variant", variant: "api" },
      };
    case "llm.cerebras":
      return {
        tab: "llm",
        fieldId: "llm-cerebras-api-key",
        activate: true,
        applyAction: { kind: "llm_variant", variant: "cerebras" },
      };
    case "llm.sambanova":
      return {
        tab: "llm",
        fieldId: "llm-sambanova-api-key",
        activate: true,
        applyAction: { kind: "llm_variant", variant: "sambanova" },
      };
    case "llm.openrouter":
      return {
        tab: "llm",
        fieldId: "llm-openrouter-api-key",
        activate: true,
        applyAction: { kind: "llm_variant", variant: "openrouter" },
      };
    case "llm.aws_bedrock":
      return {
        tab: "llm",
        fieldId: "llm-bedrock-access-key",
        activate: true,
        applyAction: { kind: "llm_aws_bedrock_credential" },
      };
    case "realtime_agent.gemini_live":
      if (credentialKey !== "gemini_api_key") return null;
      return {
        tab: "gemini",
        fieldId: "gemini-api-key",
        activate: true,
        applyAction: { kind: "gemini_api_key" },
      };
    default:
      return null;
  }
}

/** `credentialRouteForProviderCredential`, keyed off a `ProviderReadiness`
 * entry's own provider id + (by default) its first known credential key. */
export function credentialRouteForReadiness(
  entry: ProviderReadiness,
  credentialKey: string | null = firstCredentialKey(entry),
): RouteEntry | null {
  return credentialRouteForProviderCredential(entry.provider_id, credentialKey);
}

function activeOpenAiCredentialRoute(ctx: RouteContext): RouteEntry | null {
  if (
    ctx.llmType === "api" &&
    ctx.llmEndpointCredentialKey === "openai_api_key"
  ) {
    return credentialRouteForProviderCredential("llm.api", "openai_api_key");
  }
  if (
    ctx.asrType === "api" &&
    ctx.asrEndpointCredentialKey === "openai_api_key"
  ) {
    return credentialRouteForProviderCredential("asr.api", "openai_api_key");
  }
  if (ctx.asrType === "openai_realtime") {
    return credentialRouteForProviderCredential(
      "asr.openai_realtime",
      "openai_api_key",
    );
  }
  return null;
}

function readinessOpenAiCredentialRoute(
  readinessEntries: readonly ProviderReadiness[],
): RouteEntry | null {
  const readinessPriority = ["llm.api", "asr.api", "asr.openai_realtime"];
  for (const providerId of readinessPriority) {
    const entry = readinessEntries.find(
      (candidate) => candidate.provider_id === providerId,
    );
    if (!entry) continue;
    const route = credentialRouteForReadiness(entry, "openai_api_key");
    if (route) return route;
  }

  return (
    readinessEntries
      .map((entry) => credentialRouteForReadiness(entry, "openai_api_key"))
      .find((route): route is RouteEntry => route != null) ?? null
  );
}

function activeProviderCredentialRouteForKey(
  key: string,
  ctx: RouteContext,
): RouteEntry | null {
  if (key === "openai_api_key") return activeOpenAiCredentialRoute(ctx);

  for (const providerId of ctx.activeReadinessProviderIds) {
    if (providerId === "asr.api" && ctx.asrEndpointCredentialKey !== key) {
      continue;
    }
    if (providerId === "llm.api" && ctx.llmEndpointCredentialKey !== key) {
      continue;
    }
    const descriptor = PROVIDER_DESCRIPTORS.get(providerId);
    if (!descriptor?.credential_keys.includes(key)) continue;
    const route = credentialRouteForProviderCredential(providerId, key);
    if (route) return route;
  }

  return null;
}

/** The static (registry-independent) fallback when no active provider claims
 * a credential key — e.g. a saved-but-unselected provider's key row.
 *
 * Extraction note: the pre-extraction closure of this name opened with an
 * `activeProviderCredentialRouteForKey(key)` pre-check before this switch;
 * that pre-check is NOT reproduced here — it now lives only in
 * `credentialRouteForKey`'s call ordering (it resolves
 * `activeProviderCredentialRouteForKey` at an earlier step and only falls
 * through to this function after that returns null), so `credentialRouteForKey`
 * is behavior-identical. But this function, taken on its own, is not: a
 * future caller invoking this export directly (bypassing
 * `credentialRouteForKey`) will get the static fallback even when an active
 * provider claims the key, which the pre-extraction function never did. */
export function fallbackCredentialRouteForKey(key: string): RouteEntry | null {
  switch (key) {
    case "openai_api_key":
      return null;
    case "openrouter_api_key":
      return {
        tab: "llm",
        fieldId: "llm-openrouter-api-key",
        activate: true,
        applyAction: { kind: "llm_variant", variant: "openrouter" },
      };
    case "cerebras_api_key":
      return {
        tab: "llm",
        fieldId: "llm-cerebras-api-key",
        activate: true,
        applyAction: { kind: "llm_variant", variant: "cerebras" },
      };
    case "sambanova_api_key":
      return {
        tab: "llm",
        fieldId: "llm-sambanova-api-key",
        activate: true,
        applyAction: { kind: "llm_variant", variant: "sambanova" },
      };
    case "deepgram_api_key":
      return {
        tab: "stt",
        fieldId: "deepgram-api-key",
        activate: true,
        applyAction: { kind: "asr_variant", variant: "deepgram" },
      };
    case "assemblyai_api_key":
      return {
        tab: "stt",
        fieldId: "assemblyai-api-key",
        activate: true,
        applyAction: { kind: "asr_variant", variant: "assemblyai" },
      };
    case "gemini_api_key":
      return {
        tab: "gemini",
        fieldId: "gemini-api-key",
        activate: true,
        applyAction: { kind: "gemini_api_key" },
      };
    case "aws_access_key":
    case "aws_secret_key":
    case "aws_session_token":
      return {
        tab: "stt",
        fieldId: "aws-asr-access-key",
        activate: true,
        applyAction: { kind: "asr_aws_transcribe_credential" },
      };
    default:
      return null;
  }
}

/** Every readiness entry that lists `key` among its credentials — exposed
 * (not just an internal helper) because `CredentialsPanel` reads it off the
 * controller directly to group a credential's affected providers. */
export function relatedReadinessForCredential(
  key: string,
  providerReadinessEntries: readonly ProviderReadiness[],
): ProviderReadiness[] {
  return providerReadinessEntries.filter((entry) =>
    entry.credentials.some((credential) => credential.key === key),
  );
}

/**
 * Top-level "where does this credential key's fix action go" resolver.
 * Priority: the active/selected provider that owns this key, then any
 * provider-readiness entry that mentions it, then the static fallback.
 */
export function credentialRouteForKey(
  key: string,
  ctx: RouteContext,
): RouteEntry | null {
  const relatedReadiness = relatedReadinessForCredential(
    key,
    ctx.providerReadinessEntries,
  );
  if (key === "openai_api_key") {
    return (
      activeOpenAiCredentialRoute(ctx) ??
      readinessOpenAiCredentialRoute(relatedReadiness)
    );
  }

  const activeReadinessRoute = ctx.activeReadinessProviderIds
    .flatMap((providerId) =>
      relatedReadiness.filter((entry) => entry.provider_id === providerId),
    )
    .map((entry) => credentialRouteForReadiness(entry, key))
    .find((route): route is RouteEntry => route != null);
  if (activeReadinessRoute) return activeReadinessRoute;

  const activeConfiguredRoute = activeProviderCredentialRouteForKey(key, ctx);
  if (activeConfiguredRoute) return activeConfiguredRoute;

  const readinessRoute = relatedReadiness
    .map((entry) => credentialRouteForReadiness(entry, key))
    .find((route): route is RouteEntry => route != null);
  return readinessRoute ?? fallbackCredentialRouteForKey(key);
}

/**
 * "Inspect / configure this provider" route — selects the provider and lands
 * on its own settings field (as opposed to `credentialRouteForProviderCredential`,
 * which lands on the credential field specifically).
 */
export function providerRouteForProviderId(
  providerId: string,
): RouteEntry | null {
  switch (providerId) {
    case "asr.local_whisper":
      return {
        tab: "stt",
        fieldId: "asr-whisper-model",
        applyAction: { kind: "asr_variant", variant: "local_whisper" },
      };
    case "asr.api":
      return {
        tab: "stt",
        fieldId: "asr-endpoint",
        applyAction: { kind: "asr_variant", variant: "api" },
      };
    case "asr.openai_realtime":
      return {
        tab: "stt",
        fieldId: "openai-realtime-model",
        applyAction: { kind: "asr_variant", variant: "openai_realtime" },
      };
    case "asr.aws_transcribe":
      return {
        tab: "stt",
        fieldId: "aws-asr-region",
        applyAction: { kind: "asr_variant", variant: "aws_transcribe" },
      };
    case "asr.deepgram":
      return {
        tab: "stt",
        fieldId: "deepgram-model",
        applyAction: { kind: "asr_variant", variant: "deepgram" },
      };
    case "asr.assemblyai":
      return {
        tab: "stt",
        fieldId: "assemblyai-api-key",
        applyAction: { kind: "asr_variant", variant: "assemblyai" },
      };
    case "asr.sherpa_onnx":
      return {
        tab: "stt",
        fieldId: "sherpa-model-dir",
        applyAction: { kind: "asr_variant", variant: "sherpa_onnx" },
      };
    case "llm.local_llama":
      return {
        tab: "llm",
        fieldId: "streaming-prefill-toggle",
        applyAction: { kind: "llm_variant", variant: "local_llama" },
      };
    case "llm.api":
      return {
        tab: "llm",
        fieldId: "llm-custom-endpoint",
        applyAction: { kind: "llm_variant", variant: "api" },
      };
    case "llm.cerebras":
      return {
        tab: "llm",
        fieldId: "llm-cerebras-model",
        applyAction: { kind: "llm_variant", variant: "cerebras" },
      };
    case "llm.sambanova":
      return {
        tab: "llm",
        fieldId: "llm-sambanova-model",
        applyAction: { kind: "llm_variant", variant: "sambanova" },
      };
    case "llm.openrouter":
      return {
        tab: "llm",
        fieldId: "llm-openrouter-model",
        applyAction: { kind: "llm_variant", variant: "openrouter" },
      };
    case "llm.aws_bedrock":
      return {
        tab: "llm",
        fieldId: "llm-bedrock-region",
        applyAction: { kind: "llm_variant", variant: "aws_bedrock" },
      };
    case "llm.mistralrs":
      return {
        tab: "llm",
        fieldId: "llm-mistralrs-model-id",
        applyAction: { kind: "llm_variant", variant: "mistralrs" },
      };
    case "realtime_agent.gemini_live":
      return { tab: "gemini", fieldId: "gemini-model" };
    case "tts.none":
      return {
        tab: "tts",
        fieldId: "tts-provider-select",
        applyAction: { kind: "tts_variant", variant: "none" },
      };
    case "tts.deepgram_aura":
      return {
        tab: "tts",
        fieldId: "tts-provider-select",
        applyAction: { kind: "tts_variant", variant: "deepgram_aura" },
      };
    default:
      return null;
  }
}

/**
 * `providerRouteForProviderId` addressed by stage + settings-variant string
 * instead of the `"stage.variant"` registry provider-id convention — used by
 * callers that only know a stage and a settings-level variant (e.g.
 * `ExpressSetup`'s Advanced handoff, which tracks `AsrType`/`LlmType`
 * choices, not registry provider ids).
 */
export function providerRouteForStage(
  stage: "asr" | "llm",
  variant: string,
): RouteEntry | null {
  return providerRouteForProviderId(`${stage}.${variant}`);
}

/** "Inspect / choose this provider's model" route. */
export function modelRouteForProviderId(providerId: string): RouteEntry | null {
  switch (providerId) {
    case "asr.local_whisper":
      return providerRouteForProviderId(providerId);
    case "asr.api":
      return {
        tab: "stt",
        fieldId: "asr-model",
        applyAction: { kind: "asr_variant", variant: "api" },
      };
    case "asr.openai_realtime":
      return {
        tab: "stt",
        fieldId: "openai-realtime-model",
        applyAction: { kind: "asr_variant", variant: "openai_realtime" },
      };
    case "asr.deepgram":
      return {
        tab: "stt",
        fieldId: "deepgram-model",
        applyAction: { kind: "asr_variant", variant: "deepgram" },
      };
    case "asr.sherpa_onnx":
      return {
        tab: "stt",
        fieldId: "sherpa-model-dir",
        applyAction: { kind: "asr_variant", variant: "sherpa_onnx" },
      };
    case "llm.local_llama":
      // T4a (audio-graph-4850, synthesis §T4a): the Models section moved
      // General -> Credentials ("Setup health") — `CredentialsManager` now
      // renders `#settings-models-section` inside `CredentialsPanel.tsx`.
      return { tab: "credentials", fieldId: "settings-models-section" };
    case "llm.api":
      return {
        tab: "llm",
        fieldId: "llm-custom-model",
        applyAction: { kind: "llm_variant", variant: "api" },
      };
    case "llm.cerebras":
      return {
        tab: "llm",
        fieldId: "llm-cerebras-model",
        applyAction: { kind: "llm_variant", variant: "cerebras" },
      };
    case "llm.sambanova":
      return {
        tab: "llm",
        fieldId: "llm-sambanova-model",
        applyAction: { kind: "llm_variant", variant: "sambanova" },
      };
    case "llm.openrouter":
      return {
        tab: "llm",
        fieldId: "llm-openrouter-model",
        applyAction: { kind: "llm_variant", variant: "openrouter" },
      };
    case "llm.aws_bedrock":
      return {
        tab: "llm",
        fieldId: "llm-bedrock-model-id",
        applyAction: { kind: "llm_variant", variant: "aws_bedrock" },
      };
    case "llm.mistralrs":
      return {
        tab: "llm",
        fieldId: "llm-mistralrs-model-id",
        applyAction: { kind: "llm_variant", variant: "mistralrs" },
      };
    case "realtime_agent.gemini_live":
      return { tab: "gemini", fieldId: "gemini-model" };
    case "tts.deepgram_aura":
      return {
        tab: "tts",
        fieldId: "aura-voice-select",
        applyAction: { kind: "tts_variant", variant: "deepgram_aura" },
      };
    default:
      return null;
  }
}

/**
 * Credential route for a provider-setup-mode-card's selected provider — two
 * cases (aura TTS, Gemini Vertex service account) land on a field that
 * differs from the plain `credentialRouteForProviderCredential` lookup;
 * everything else delegates there.
 */
export function credentialRouteForProviderSetupSelection(
  providerId: string,
  credentialKey: string | null,
): RouteEntry | null {
  if (providerId === "tts.deepgram_aura") {
    return {
      tab: "tts",
      fieldId: "tts-deepgram-api-key",
      applyAction: { kind: "tts_variant", variant: "deepgram_aura" },
    };
  }
  if (
    providerId === "realtime_agent.gemini_live" &&
    credentialKey === "google_service_account_path"
  ) {
    return {
      tab: "gemini",
      fieldId: "gemini-service-account-path",
      applyAction: { kind: "gemini_vertex_ai" },
    };
  }
  return credentialRouteForProviderCredential(providerId, credentialKey);
}

// ── ROUTE_INDEX ──────────────────────────────────────────────────────────
// Every provider id / credential key this table has a case for, used only to
// generate the flattened `{tab, fieldId}` index below (the drift tripwire
// test's enumeration). Keeping these lists next to the functions that
// consume the ids means a new `case` added above without a matching entry
// here simply doesn't appear in the index, rather than silently drifting.
const CREDENTIAL_PROVIDER_IDS = [
  "asr.api",
  "asr.openai_realtime",
  "realtime_agent.openai_realtime",
  "asr.deepgram",
  "asr.assemblyai",
  "asr.aws_transcribe",
  "llm.api",
  "llm.cerebras",
  "llm.sambanova",
  "llm.openrouter",
  "llm.aws_bedrock",
  "realtime_agent.gemini_live",
] as const;
const CREDENTIAL_PROVIDER_KEYS: Record<string, string> = {
  "realtime_agent.openai_realtime": "openai_api_key",
  "realtime_agent.gemini_live": "gemini_api_key",
};
const PROVIDER_ROUTE_IDS = [
  "asr.local_whisper",
  "asr.api",
  "asr.openai_realtime",
  "asr.aws_transcribe",
  "asr.deepgram",
  "asr.assemblyai",
  "asr.sherpa_onnx",
  "llm.local_llama",
  "llm.api",
  "llm.cerebras",
  "llm.sambanova",
  "llm.openrouter",
  "llm.aws_bedrock",
  "llm.mistralrs",
  "realtime_agent.gemini_live",
  "tts.none",
  "tts.deepgram_aura",
] as const;
const MODEL_ROUTE_IDS = [
  "asr.local_whisper",
  "asr.api",
  "asr.openai_realtime",
  "asr.deepgram",
  "asr.sherpa_onnx",
  "llm.local_llama",
  "llm.api",
  "llm.cerebras",
  "llm.sambanova",
  "llm.openrouter",
  "llm.aws_bedrock",
  "llm.mistralrs",
  "realtime_agent.gemini_live",
  "tts.deepgram_aura",
] as const;
const FALLBACK_CREDENTIAL_KEYS = [
  "openrouter_api_key",
  "cerebras_api_key",
  "sambanova_api_key",
  "deepgram_api_key",
  "assemblyai_api_key",
  "gemini_api_key",
  "aws_access_key",
] as const;
const PROVIDER_SETUP_SPECIAL_IDS = [
  ["tts.deepgram_aura", null],
  ["realtime_agent.gemini_live", "google_service_account_path"],
] as const;

function collectRouteIndex(): ReadonlyArray<{
  tab: SettingsTab;
  fieldId: string;
}> {
  const seen = new Set<string>();
  const index: Array<{ tab: SettingsTab; fieldId: string }> = [];
  const record = (entry: RouteEntry | null) => {
    if (!entry) return;
    const dedupeKey = `${entry.tab}:${entry.fieldId}`;
    if (seen.has(dedupeKey)) return;
    seen.add(dedupeKey);
    index.push({ tab: entry.tab, fieldId: entry.fieldId });
  };

  for (const providerId of CREDENTIAL_PROVIDER_IDS) {
    record(
      credentialRouteForProviderCredential(
        providerId,
        CREDENTIAL_PROVIDER_KEYS[providerId] ?? null,
      ),
    );
  }
  for (const providerId of PROVIDER_ROUTE_IDS) {
    record(providerRouteForProviderId(providerId));
  }
  for (const providerId of MODEL_ROUTE_IDS) {
    record(modelRouteForProviderId(providerId));
  }
  for (const key of FALLBACK_CREDENTIAL_KEYS) {
    record(fallbackCredentialRouteForKey(key));
  }
  for (const [providerId, credentialKey] of PROVIDER_SETUP_SPECIAL_IDS) {
    record(credentialRouteForProviderSetupSelection(providerId, credentialKey));
  }

  return index;
}

/**
 * Flattened `{tab, fieldId}` pairs across every route this module can
 * produce, deduplicated. Consumed by the drift tripwire contract test
 * (mounts each tab, asserts `getElementById(fieldId)` resolves) and
 * available to T4b's future jump-palette manifest.
 */
export const ROUTE_INDEX: ReadonlyArray<{ tab: SettingsTab; fieldId: string }> =
  collectRouteIndex();
