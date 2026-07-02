/**
 * LLM provider sub-form — the choose-your-LLM surface inside
 * `SettingsPage` (used for both entity extraction and chat).
 *
 * Renders backend-specific panels based on `llmType`:
 *   - `local_llama` — GGUF model path + llama.cpp-2 inference params.
 *   - `mistralrs`   — GGUF path + mistral.rs / Candle-based engine.
 *   - `api`         — OpenAI-compatible endpoint (works for OpenAI,
 *                     Ollama, LM Studio, vLLM, Together, Groq) + API key
 *                     + model string. OpenRouter has its own first-class
 *                     panel (`openrouter`, ADR-0005).
 *   - `aws_bedrock` — region + credential-mode selector + Bedrock model
 *                     ID.
 *
 * Parent: `SettingsPage.tsx`. Props mirror `AsrProviderSettings`: a
 * narrowed reducer slice + dispatch + translation handle + `testingKey`.
 */

import type { TFunction } from "i18next";
import type { Dispatch, ReactNode } from "react";
import type {
  ModelStatus,
  OpenRouterModelEndpoints,
  OpenRouterProvider,
  ProviderDescriptor,
  ProviderModelCatalogItem,
  ProviderReadiness,
} from "../types";
import type { AcceleratorPreset } from "../utils/openrouterCatalog";
import AdvancedSettingsDisclosure from "./AdvancedSettingsDisclosure";
import ModelCatalogField from "./ModelCatalogField";
import OpenRouterAcceleratorDiscovery from "./OpenRouterAcceleratorDiscovery";
import ProviderReadinessPanel, {
  type CredentialPresenceLookup,
} from "./ProviderReadinessPanel";
import {
  defaultModelForProvider,
  type ProviderSettingsOption,
} from "./providerRegistryHelpers";
import SecretCredentialControl, {
  AwsCredentialControl,
} from "./SecretCredentialControl";
import {
  type AwsCredentialMode,
  CEREBRAS_BASE_URL,
  endpointCredentialKey,
  readinessBadge,
  type SettingsAction,
  type SettingsState,
  setField,
  type TestKey,
} from "./settingsTypes";

interface LlmProviderSettingsProps {
  state: Pick<
    SettingsState,
    | "llmType"
    | "llmEndpoint"
    | "llmApiKey"
    | "llmModel"
    | "llmMaxTokens"
    | "llmTemperature"
    | "streamingPrefill"
    | "mistralrsModelId"
    | "openrouterApiKey"
    | "openrouterModel"
    | "openrouterBaseUrl"
    | "openrouterIncludeUsageInStream"
    | "openrouterRoutingPreset"
    | "openrouterProviderOrderText"
    | "openrouterModels"
    | "openrouterModelsLoadedAt"
    | "openrouterModelsLoading"
    | "awsBedrockRegion"
    | "awsBedrockModelId"
    | "awsBedrockCredentialMode"
    | "awsBedrockProfileName"
    | "awsBedrockAccessKey"
    | "awsBedrockSecretKey"
    | "awsBedrockSessionToken"
    | "awsProfiles"
    | "testingKey"
    | "endpointCredentials"
  >;
  dispatch: Dispatch<SettingsAction>;
  t: TFunction;
  modelStatus: ModelStatus | null;
  refreshAwsProfiles: () => Promise<void>;
  handleTestAwsBedrock: () => Promise<void>;
  handleTestOpenRouter: () => Promise<void>;
  handleRefreshOpenRouterModels: () => Promise<void>;
  handleTestCerebras: () => Promise<void>;
  handleRefreshCerebrasModels: () => void;
  // Generic model-catalog refresh keyed by provider id — powers the uniform
  // Load-models button for llm.api (and any future remote-command LLM provider).
  handleRefreshModels: (providerId: string) => void;
  llmEndpointSavedKeyPresent: boolean;
  awsSavedKeysPresent: boolean;
  awsSessionTokenSavedPresent: boolean;
  awsAccessKeysAvailable: boolean;
  openrouterCredentialAvailable: boolean;
  openrouterSavedKeyPresent: boolean;
  cerebrasCredentialAvailable: boolean;
  cerebrasSavedKeyPresent: boolean;
  openrouterModelsError: string | null;
  cerebrasModelsLoading: boolean;
  cerebrasModelsError: string | null;
  llmApiCredentialAvailable: boolean;
  llmApiModelsLoading: boolean;
  llmApiModelsError: string | null;
  cerebrasTesting: boolean;
  cerebrasTestResult: { ok: boolean; msg: string } | null;
  providerOptions: ProviderSettingsOption<SettingsState["llmType"]>[];
  llmApiModelCatalog: ProviderModelCatalogItem[];
  cerebrasModelCatalog: ProviderModelCatalogItem[];
  mistralrsModelCatalog: ProviderModelCatalogItem[];
  activeProviderDescriptor: ProviderDescriptor | null;
  activeProviderReadiness: ProviderReadiness | null;
  credentialPresence: CredentialPresenceLookup;
  providerReadinessLoading: boolean;
  handleClearCredential: (
    key: string | string[],
    label: string,
    clearLocal: () => void,
  ) => Promise<void>;
  renderTestResult: (key: TestKey) => ReactNode;
  // OpenRouter accelerator discovery (seed 7809): saved-key catalog payloads +
  // discovery state, plus the discover/select/apply callbacks the controller
  // owns. The discovered candidates replace the hardcoded provider order.
  openrouterAcceleratorEndpoints: OpenRouterModelEndpoints | null;
  openrouterAcceleratorProviders: OpenRouterProvider[] | null;
  openrouterAcceleratorLoading: boolean;
  openrouterAcceleratorError: string | null;
  openrouterAcceleratorPreset: AcceleratorPreset;
  openrouterAppliedAcceleratorPreset: AcceleratorPreset | null;
  setOpenrouterAcceleratorPreset: (preset: AcceleratorPreset) => void;
  handleDiscoverOpenRouterAccelerators: () => void;
  handleApplyAcceleratorPreset: (
    preset: AcceleratorPreset,
    order: string[],
  ) => void;
}

export default function LlmProviderSettings({
  state,
  dispatch,
  t,
  modelStatus,
  refreshAwsProfiles,
  handleTestAwsBedrock,
  handleTestOpenRouter,
  handleRefreshOpenRouterModels,
  handleTestCerebras,
  handleRefreshCerebrasModels,
  handleRefreshModels,
  llmEndpointSavedKeyPresent,
  awsSavedKeysPresent,
  awsSessionTokenSavedPresent,
  awsAccessKeysAvailable,
  openrouterCredentialAvailable,
  openrouterSavedKeyPresent,
  cerebrasCredentialAvailable,
  cerebrasSavedKeyPresent,
  openrouterModelsError,
  cerebrasModelsLoading,
  cerebrasModelsError,
  llmApiCredentialAvailable,
  llmApiModelsLoading,
  llmApiModelsError,
  cerebrasTesting,
  cerebrasTestResult,
  providerOptions,
  llmApiModelCatalog,
  cerebrasModelCatalog,
  mistralrsModelCatalog,
  activeProviderDescriptor,
  activeProviderReadiness,
  credentialPresence,
  providerReadinessLoading,
  handleClearCredential,
  renderTestResult,
  openrouterAcceleratorEndpoints,
  openrouterAcceleratorProviders,
  openrouterAcceleratorLoading,
  openrouterAcceleratorError,
  openrouterAcceleratorPreset,
  openrouterAppliedAcceleratorPreset,
  setOpenrouterAcceleratorPreset,
  handleDiscoverOpenRouterAccelerators,
  handleApplyAcceleratorPreset,
}: LlmProviderSettingsProps) {
  const {
    llmType,
    llmEndpoint,
    llmApiKey,
    llmModel,
    llmMaxTokens,
    llmTemperature,
    streamingPrefill,
    mistralrsModelId,
    openrouterApiKey,
    openrouterModel,
    openrouterBaseUrl,
    openrouterIncludeUsageInStream,
    openrouterRoutingPreset,
    openrouterProviderOrderText,
    openrouterModels,
    openrouterModelsLoading,
    awsBedrockRegion,
    awsBedrockModelId,
    awsBedrockCredentialMode,
    awsBedrockProfileName,
    awsBedrockAccessKey,
    awsBedrockSecretKey,
    awsBedrockSessionToken,
    awsProfiles,
    testingKey,
  } = state;
  const activeProviderDefaultModel =
    activeProviderDescriptor?.default_model ?? "";

  const openrouterModelCatalog: ProviderModelCatalogItem[] =
    openrouterModels.map((model) => ({
      id: model.id,
      display_name: model.name || model.id,
      is_default: false,
    }));
  const openrouterRoutingPresetOptions: Array<{
    value: SettingsState["openrouterRoutingPreset"];
    label: string;
  }> = [
    {
      value: "balanced",
      label: t("settings.openrouterRoutingPresets.balanced.label"),
    },
    {
      value: "low_latency",
      label: t("settings.openrouterRoutingPresets.lowLatency.label"),
    },
    {
      value: "high_throughput",
      label: t("settings.openrouterRoutingPresets.highThroughput.label"),
    },
    {
      value: "privacy_zdr",
      label: t("settings.openrouterRoutingPresets.privacyZdr.label"),
    },
    {
      value: "strict_accelerator",
      label: t("settings.openrouterRoutingPresets.strictAccelerator.label"),
    },
  ];
  const openrouterRoutingPresetHintKey: Record<
    SettingsState["openrouterRoutingPreset"],
    string
  > = {
    legacy: "settings.openrouterRoutingPresets.legacy.hint",
    balanced: "settings.openrouterRoutingPresets.balanced.hint",
    low_latency: "settings.openrouterRoutingPresets.lowLatency.hint",
    high_throughput: "settings.openrouterRoutingPresets.highThroughput.hint",
    privacy_zdr: "settings.openrouterRoutingPresets.privacyZdr.hint",
    strict_accelerator:
      "settings.openrouterRoutingPresets.strictAccelerator.hint",
    custom: "settings.openrouterRoutingPresets.custom.hint",
  };
  if (openrouterRoutingPreset === "legacy") {
    openrouterRoutingPresetOptions.unshift({
      value: "legacy",
      label: t("settings.openrouterRoutingPresets.legacy.label"),
    });
  }
  if (openrouterRoutingPreset === "custom") {
    openrouterRoutingPresetOptions.unshift({
      value: "custom",
      label: t("settings.openrouterRoutingPresets.custom.label"),
    });
  }
  const showOpenrouterProviderOrderText =
    openrouterRoutingPreset === "strict_accelerator" ||
    openrouterRoutingPreset === "legacy";
  const handleOpenrouterRoutingPresetChange = (
    value: SettingsState["openrouterRoutingPreset"],
  ) => {
    dispatch(setField("openrouterRoutingPreset", value));
    // No hardcoded `"cerebras, groq"` seed here (seed 7809): the accelerator
    // provider order is now sourced from the live OpenRouter catalog via the
    // discovery panel below. Switching to strict-accelerator leaves the field
    // empty so the user discovers + applies a ranked preset (or types their
    // own) — the catalog is the source of truth, not a baked-in constant.
  };

  const applyVllmPreset = () => {
    dispatch(setField("llmEndpoint", "http://localhost:8000/v1"));
    dispatch(setField("llmModel", "Qwen/Qwen2.5-1.5B-Instruct"));
    dispatch(setField("llmApiKey", ""));
  };

  // Stash the key typed for the old endpoint into the per-endpoint cache, then
  // re-fill the visible key from whatever is cached for the new endpoint. Makes
  // provider round-trips (OpenAI → Groq → OpenAI) lossless without re-typing
  // and prevents one provider's key from leaking into another's field. (W3.5)
  const handleLlmEndpointChange = (endpoint: string) => {
    if (llmApiKey.trim()) {
      dispatch({
        type: "SET_ENDPOINT_CREDENTIALS",
        credentials: { [endpointCredentialKey(llmEndpoint)]: llmApiKey },
      });
    }
    dispatch(setField("llmEndpoint", endpoint));
    const cached =
      state.endpointCredentials[endpointCredentialKey(endpoint)] ?? "";
    if (cached !== llmApiKey) {
      dispatch(setField("llmApiKey", cached));
    }
  };

  const handleLlmProviderChange = (value: SettingsState["llmType"]) => {
    if (llmApiKey.trim() && (llmType === "api" || llmType === "cerebras")) {
      dispatch({
        type: "SET_ENDPOINT_CREDENTIALS",
        credentials: {
          [llmType === "cerebras"
            ? "cerebras_api_key"
            : endpointCredentialKey(llmEndpoint)]: llmApiKey,
        },
      });
    }

    if (value === "cerebras") {
      dispatch(setField("llmEndpoint", CEREBRAS_BASE_URL));
      dispatch(setField("llmModel", defaultModelForProvider("llm.cerebras")));
      const cached = state.endpointCredentials.cerebras_api_key ?? "";
      if (cached !== llmApiKey) dispatch(setField("llmApiKey", cached));
    } else if (llmType === "cerebras" && value === "api") {
      const nextEndpoint = "http://localhost:8000/v1";
      dispatch(setField("llmEndpoint", nextEndpoint));
      const cached =
        state.endpointCredentials[endpointCredentialKey(nextEndpoint)] ?? "";
      if (cached !== llmApiKey) dispatch(setField("llmApiKey", cached));
    }

    dispatch(setField("llmType", value));
  };

  return (
    <div className="settings-section">
      <h3 className="settings-section__title">{t("settings.sections.llm")}</h3>
      <ProviderReadinessPanel
        entry={activeProviderReadiness}
        descriptor={activeProviderDescriptor}
        credentialPresence={credentialPresence}
        loading={providerReadinessLoading}
        t={t}
      />
      <div
        className="settings-radio-group"
        role="radiogroup"
        aria-label={t("settings.a11y.chooseLlmProvider")}
      >
        {providerOptions.map((option) => (
          <label className="settings-radio" key={option.descriptor.id}>
            <input
              type="radio"
              name="llm-provider"
              checked={llmType === option.value}
              onChange={() => handleLlmProviderChange(option.value)}
            />
            <span>{option.label}</span>
            {option.value === "local_llama" &&
              llmType === "local_llama" &&
              modelStatus && (
                <span
                  className={`status-badge ${readinessBadge(modelStatus.llm).cls}`}
                >
                  {t(readinessBadge(modelStatus.llm).labelKey)}
                </span>
              )}
          </label>
        ))}
      </div>

      {llmType === "local_llama" && (
        <div className="settings-section__api-fields">
          <div className="settings-field settings-field--inline">
            <label htmlFor="streaming-prefill-toggle">
              <input
                id="streaming-prefill-toggle"
                type="checkbox"
                checked={streamingPrefill}
                onChange={(e) =>
                  dispatch(setField("streamingPrefill", e.target.checked))
                }
              />{" "}
              {t("settings.llmExtras.streamingPrefill")}
            </label>
            <p className="settings-hint">
              {t("settings.llmExtras.streamingPrefillHint")}
            </p>
          </div>
        </div>
      )}

      {llmType === "api" && (
        <div className="settings-section__api-fields">
          <div className="settings-inline-row">
            <button
              type="button"
              className="settings-btn"
              onClick={applyVllmPreset}
            >
              {t("settings.llmExtras.vllmPreset")}
            </button>
          </div>
          <div className="settings-field">
            <label
              className="settings-field__label"
              htmlFor="llm-custom-endpoint"
            >
              {t("settings.fields.endpoint")}
            </label>
            <input
              id="llm-custom-endpoint"
              className="settings-input"
              type="text"
              value={llmEndpoint}
              onChange={(e) => handleLlmEndpointChange(e.target.value)}
              placeholder="http://localhost:8000/v1"
            />
          </div>
          <SecretCredentialControl
            id="llm-custom-api-key"
            label={t("settings.fields.apiKey")}
            value={llmApiKey}
            onChange={(value) => dispatch(setField("llmApiKey", value))}
            placeholder="sk-..."
            saved={llmEndpointSavedKeyPresent}
            t={t}
            savedHint={t("settings.hints.endpointSavedKey")}
            onClear={
              llmEndpointSavedKeyPresent
                ? () =>
                    handleClearCredential(
                      endpointCredentialKey(llmEndpoint),
                      t("settings.fields.apiKey"),
                      () => dispatch(setField("llmApiKey", "")),
                    )
                : undefined
            }
          />
          <div className="settings-field">
            <label className="settings-field__label" htmlFor="llm-custom-model">
              {t("settings.fields.model")}
            </label>
            <ModelCatalogField
              id="llm-custom-model"
              value={llmModel}
              onChange={(value) => dispatch(setField("llmModel", value))}
              catalog={llmApiModelCatalog}
              t={t}
              providerName={t("settings.llmProviders.openaiCompatible")}
              placeholder="gpt-4o-mini"
              loading={llmApiModelsLoading}
              error={llmApiModelsError}
              credentialAvailable={llmApiCredentialAvailable}
              onRefresh={() => handleRefreshModels("llm.api")}
              hasRemoteCommand
            />
          </div>
          <AdvancedSettingsDisclosure
            summary={t("settings.sections.advancedProviderControls")}
          >
            <div className="settings-field">
              <label
                className="settings-field__label"
                htmlFor="llm-custom-max-tokens"
              >
                {t("settings.fields.maxTokens", { count: llmMaxTokens })}
              </label>
              <input
                id="llm-custom-max-tokens"
                className="settings-input"
                type="number"
                value={llmMaxTokens}
                onChange={(e) =>
                  dispatch(setField("llmMaxTokens", Number(e.target.value)))
                }
                min={1}
                max={32768}
              />
            </div>
            <div className="settings-field">
              <label
                className="settings-field__label"
                htmlFor="llm-custom-temperature"
              >
                {t("settings.fields.temperature", { value: llmTemperature })}
              </label>
              <input
                id="llm-custom-temperature"
                className="settings-input"
                type="number"
                step="0.1"
                value={llmTemperature}
                onChange={(e) =>
                  dispatch(setField("llmTemperature", Number(e.target.value)))
                }
                min={0}
                max={2}
              />
            </div>
          </AdvancedSettingsDisclosure>
        </div>
      )}

      {llmType === "cerebras" && (
        <div className="settings-section__api-fields">
          <div className="settings-field">
            <label
              className="settings-field__label"
              htmlFor="llm-cerebras-endpoint"
            >
              {t("settings.fields.endpoint")}
            </label>
            <input
              id="llm-cerebras-endpoint"
              className="settings-input"
              type="text"
              value={CEREBRAS_BASE_URL}
              readOnly
            />
            <p className="settings-hint">
              {t("settings.hints.cerebrasEndpoint")}
            </p>
          </div>
          <SecretCredentialControl
            id="llm-cerebras-api-key"
            label={t("settings.fields.cerebrasApiKey")}
            value={llmApiKey}
            onChange={(value) => dispatch(setField("llmApiKey", value))}
            placeholder="csk-..."
            saved={cerebrasSavedKeyPresent}
            t={t}
            savedHint={t("settings.hints.cerebrasSavedKey")}
            onClear={
              cerebrasSavedKeyPresent
                ? () =>
                    handleClearCredential(
                      "cerebras_api_key",
                      t("settings.credentialConfirm.cerebrasApiKeyLabel"),
                      () => dispatch(setField("llmApiKey", "")),
                    )
                : undefined
            }
          />
          <div className="settings-field">
            <label
              className="settings-field__label"
              htmlFor="llm-cerebras-model"
            >
              {t("settings.fields.model")}
            </label>
            <ModelCatalogField
              id="llm-cerebras-model"
              value={llmModel}
              onChange={(value) => dispatch(setField("llmModel", value))}
              catalog={cerebrasModelCatalog}
              t={t}
              providerName={t("settings.llmProviders.cerebras")}
              placeholder={
                activeProviderDefaultModel ||
                defaultModelForProvider("llm.cerebras")
              }
              loading={cerebrasModelsLoading}
              error={cerebrasModelsError}
              credentialAvailable={cerebrasCredentialAvailable}
              onRefresh={handleRefreshCerebrasModels}
              hasRemoteCommand
            />
          </div>
          <AdvancedSettingsDisclosure
            summary={t("settings.sections.advancedProviderControls")}
          >
            <div className="settings-field">
              <label
                className="settings-field__label"
                htmlFor="llm-cerebras-max-tokens"
              >
                {t("settings.fields.maxTokens", { count: llmMaxTokens })}
              </label>
              <input
                id="llm-cerebras-max-tokens"
                className="settings-input"
                type="number"
                value={llmMaxTokens}
                onChange={(e) =>
                  dispatch(setField("llmMaxTokens", Number(e.target.value)))
                }
                min={1}
                max={32768}
              />
            </div>
            <div className="settings-field">
              <label
                className="settings-field__label"
                htmlFor="llm-cerebras-temperature"
              >
                {t("settings.fields.temperature", { value: llmTemperature })}
              </label>
              <input
                id="llm-cerebras-temperature"
                className="settings-input"
                type="number"
                step="0.1"
                value={llmTemperature}
                onChange={(e) =>
                  dispatch(setField("llmTemperature", Number(e.target.value)))
                }
                min={0}
                max={2}
              />
            </div>
          </AdvancedSettingsDisclosure>
          <div className="settings-field">
            <button
              type="button"
              className="settings-btn settings-btn--secondary"
              disabled={
                testingKey !== null ||
                cerebrasTesting ||
                !cerebrasCredentialAvailable
              }
              onClick={handleTestCerebras}
            >
              {cerebrasTesting
                ? t("settings.buttons.testing")
                : t("settings.buttons.testConnection")}
            </button>
            {cerebrasTestResult && (
              <div
                className={
                  cerebrasTestResult.ok
                    ? "settings-test-ok"
                    : "settings-test-err"
                }
                role="status"
                aria-live="polite"
                aria-atomic="true"
              >
                {cerebrasTestResult.msg}
              </div>
            )}
          </div>
        </div>
      )}

      {llmType === "openrouter" && (
        <div className="settings-section__api-fields">
          <SecretCredentialControl
            id="llm-openrouter-api-key"
            label={t("settings.fields.openrouterApiKey")}
            value={openrouterApiKey}
            onChange={(value) => dispatch(setField("openrouterApiKey", value))}
            placeholder="sk-or-..."
            saved={openrouterSavedKeyPresent}
            t={t}
            savedHint={t("settings.hints.openrouterSavedKey")}
            onClear={
              openrouterSavedKeyPresent
                ? () =>
                    handleClearCredential(
                      "openrouter_api_key",
                      t("settings.credentialConfirm.openrouterApiKeyLabel"),
                      () => dispatch(setField("openrouterApiKey", "")),
                    )
                : undefined
            }
          />
          <div className="settings-field">
            <label
              className="settings-field__label"
              htmlFor="llm-openrouter-model"
            >
              {t("settings.fields.model")}
            </label>
            <ModelCatalogField
              id="llm-openrouter-model"
              value={openrouterModel}
              onChange={(value) => dispatch(setField("openrouterModel", value))}
              catalog={openrouterModelCatalog}
              t={t}
              providerName={t("settings.llmProviders.openrouter")}
              placeholder={t("settings.placeholders.selectOpenrouterModel")}
              ariaLabel={t("settings.fields.openrouterModel")}
              loading={openrouterModelsLoading}
              error={openrouterModelsError}
              credentialAvailable={openrouterCredentialAvailable}
              onRefresh={handleRefreshOpenRouterModels}
              hasRemoteCommand
            />
          </div>
          <AdvancedSettingsDisclosure
            summary={t("settings.sections.advancedProviderControls")}
          >
            <div className="settings-field">
              <label
                className="settings-field__label"
                htmlFor="llm-openrouter-endpoint"
              >
                {t("settings.fields.endpoint")}
              </label>
              <input
                id="llm-openrouter-endpoint"
                className="settings-input"
                type="text"
                value={openrouterBaseUrl}
                onChange={(e) =>
                  dispatch(setField("openrouterBaseUrl", e.target.value))
                }
                placeholder="https://openrouter.ai/api/v1"
              />
            </div>
            <div className="settings-field">
              <label className="settings-radio">
                <input
                  type="checkbox"
                  checked={openrouterIncludeUsageInStream}
                  onChange={(e) =>
                    dispatch(
                      setField(
                        "openrouterIncludeUsageInStream",
                        e.target.checked,
                      ),
                    )
                  }
                />
                <span>{t("settings.fields.openrouterIncludeUsage")}</span>
              </label>
            </div>
            <div className="settings-field">
              <label
                className="settings-field__label"
                htmlFor="llm-openrouter-routing-preset"
              >
                {t("settings.fields.openrouterRoutingPreset")}
              </label>
              <select
                id="llm-openrouter-routing-preset"
                className="settings-input"
                value={openrouterRoutingPreset}
                onChange={(e) =>
                  handleOpenrouterRoutingPresetChange(
                    e.target.value as SettingsState["openrouterRoutingPreset"],
                  )
                }
              >
                {openrouterRoutingPresetOptions.map((option) => (
                  <option key={option.value} value={option.value}>
                    {option.label}
                  </option>
                ))}
              </select>
              <p className="settings-hint">
                {t(openrouterRoutingPresetHintKey[openrouterRoutingPreset])}
              </p>
            </div>
            {showOpenrouterProviderOrderText ? (
              <div className="settings-field">
                <label
                  className="settings-field__label"
                  htmlFor="llm-openrouter-provider-order"
                >
                  {t("settings.fields.openrouterProviderOrder")}
                </label>
                <textarea
                  id="llm-openrouter-provider-order"
                  className="settings-input"
                  rows={3}
                  value={openrouterProviderOrderText}
                  onChange={(e) =>
                    dispatch(
                      setField("openrouterProviderOrderText", e.target.value),
                    )
                  }
                  placeholder="cerebras, groq"
                />
                <p className="settings-hint">
                  {t("settings.hints.openrouterProviderOrder")}
                </p>
              </div>
            ) : null}
            <OpenRouterAcceleratorDiscovery
              t={t}
              endpoints={openrouterAcceleratorEndpoints}
              providers={openrouterAcceleratorProviders}
              modelId={openrouterModel}
              loading={openrouterAcceleratorLoading}
              error={openrouterAcceleratorError}
              credentialAvailable={openrouterCredentialAvailable}
              selectedPreset={openrouterAcceleratorPreset}
              appliedPreset={openrouterAppliedAcceleratorPreset}
              onSelectPreset={setOpenrouterAcceleratorPreset}
              onDiscover={handleDiscoverOpenRouterAccelerators}
              onApplyPreset={handleApplyAcceleratorPreset}
            />
          </AdvancedSettingsDisclosure>
          <div className="settings-field">
            <button
              type="button"
              className="settings-btn settings-btn--secondary"
              disabled={testingKey !== null || !openrouterCredentialAvailable}
              onClick={handleTestOpenRouter}
            >
              {testingKey === "openrouter"
                ? t("settings.buttons.testing")
                : t("settings.buttons.testConnection")}
            </button>
            {renderTestResult("openrouter")}
          </div>
        </div>
      )}

      {llmType === "aws_bedrock" && (
        <div className="settings-section__api-fields">
          <div className="settings-field">
            <label
              className="settings-field__label"
              htmlFor="llm-bedrock-region"
            >
              {t("settings.fields.region")}
            </label>
            <input
              id="llm-bedrock-region"
              className="settings-input"
              type="text"
              value={awsBedrockRegion}
              onChange={(e) =>
                dispatch(setField("awsBedrockRegion", e.target.value))
              }
              placeholder="us-east-1"
            />
          </div>
          <div className="settings-field">
            <label
              className="settings-field__label"
              htmlFor="llm-bedrock-model-id"
            >
              {t("settings.fields.modelId")}
            </label>
            <input
              id="llm-bedrock-model-id"
              className="settings-input"
              type="text"
              value={awsBedrockModelId}
              onChange={(e) =>
                dispatch(setField("awsBedrockModelId", e.target.value))
              }
              placeholder="anthropic.claude-3-haiku-20240307-v1:0"
            />
          </div>
          <AdvancedSettingsDisclosure
            summary={t("settings.sections.advancedProviderControls")}
          >
            <div className="settings-field">
              <label
                className="settings-field__label"
                htmlFor="llm-bedrock-credential-mode"
              >
                {t("settings.fields.credentialMode")}
              </label>
              <select
                id="llm-bedrock-credential-mode"
                className="settings-input"
                value={awsBedrockCredentialMode}
                onChange={(e) =>
                  dispatch(
                    setField(
                      "awsBedrockCredentialMode",
                      e.target.value as AwsCredentialMode,
                    ),
                  )
                }
              >
                <option value="default_chain">
                  {t("settings.credentialModes.defaultChain")}
                </option>
                <option value="profile">
                  {t("settings.credentialModes.profile")}
                </option>
                <option value="access_keys">
                  {t("settings.credentialModes.accessKeys")}
                </option>
              </select>
            </div>
            {awsBedrockCredentialMode === "profile" && (
              <div className="settings-field">
                <label
                  className="settings-field__label"
                  htmlFor="llm-bedrock-profile"
                >
                  {t("settings.fields.awsProfile")}
                </label>
                <div className="settings-inline-row">
                  <select
                    id="llm-bedrock-profile"
                    className="settings-input"
                    value={awsBedrockProfileName}
                    onChange={(e) =>
                      dispatch(
                        setField("awsBedrockProfileName", e.target.value),
                      )
                    }
                  >
                    <option value="">
                      {t("settings.placeholders.selectProfile")}
                    </option>
                    {awsProfiles.map((name) => (
                      <option key={name} value={name}>
                        {name}
                      </option>
                    ))}
                  </select>
                  <button
                    type="button"
                    className="settings-btn settings-btn--secondary"
                    onClick={refreshAwsProfiles}
                  >
                    {t("settings.buttons.refresh")}
                  </button>
                </div>
                {awsProfiles.length === 0 && (
                  <p className="settings-hint">
                    {t("settings.hints.noAwsProfiles")}{" "}
                    <code>aws configure</code>{" "}
                    {t("settings.hints.noAwsProfilesSuffix")}
                  </p>
                )}
              </div>
            )}
            {awsBedrockCredentialMode === "access_keys" && (
              <AwsCredentialControl
                accessKeyId="llm-bedrock-access-key"
                secretKeyId="llm-bedrock-secret-key"
                sessionTokenId="llm-bedrock-session-token"
                accessKey={awsBedrockAccessKey}
                secretKey={awsBedrockSecretKey}
                sessionToken={awsBedrockSessionToken}
                onAccessKeyChange={(value) =>
                  dispatch(setField("awsBedrockAccessKey", value))
                }
                onSecretKeyChange={(value) =>
                  dispatch(setField("awsBedrockSecretKey", value))
                }
                onSessionTokenChange={(value) =>
                  dispatch(setField("awsBedrockSessionToken", value))
                }
                saved={awsSavedKeysPresent}
                sessionTokenSaved={awsSessionTokenSavedPresent}
                t={t}
                onClear={
                  awsSavedKeysPresent
                    ? () =>
                        handleClearCredential(
                          [
                            "aws_access_key",
                            "aws_secret_key",
                            "aws_session_token",
                          ],
                          t("settings.credentialConfirm.awsKeysLabel"),
                          () => dispatch({ type: "CLEAR_AWS_SHARED_KEYS" }),
                        )
                    : undefined
                }
              />
            )}
          </AdvancedSettingsDisclosure>
          <div className="settings-field">
            <button
              type="button"
              className="settings-btn settings-btn--secondary"
              disabled={
                testingKey !== null ||
                !awsBedrockRegion ||
                (awsBedrockCredentialMode === "access_keys" &&
                  !awsAccessKeysAvailable)
              }
              onClick={handleTestAwsBedrock}
            >
              {testingKey === "aws_bedrock"
                ? t("settings.buttons.testing")
                : t("settings.buttons.testConnection")}
            </button>
            {renderTestResult("aws_bedrock")}
          </div>
        </div>
      )}

      {llmType === "mistralrs" && (
        <div className="settings-section__api-fields">
          <div className="settings-field">
            <label
              className="settings-field__label"
              htmlFor="llm-mistralrs-model-id"
            >
              {t("settings.fields.modelId")}
            </label>
            <ModelCatalogField
              id="llm-mistralrs-model-id"
              value={mistralrsModelId}
              onChange={(value) =>
                dispatch(setField("mistralrsModelId", value))
              }
              catalog={mistralrsModelCatalog}
              t={t}
              providerName={t("settings.llmProviders.mistralrs")}
              placeholder={activeProviderDefaultModel}
              hasRemoteCommand={false}
            />
          </div>
        </div>
      )}
    </div>
  );
}
