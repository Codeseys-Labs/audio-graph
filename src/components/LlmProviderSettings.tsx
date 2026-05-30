/**
 * LLM provider sub-form — the choose-your-LLM surface inside
 * `SettingsPage` (used for both entity extraction and chat).
 *
 * Renders backend-specific panels based on `llmType`:
 *   - `local_llama` — GGUF model path + llama.cpp-2 inference params.
 *   - `mistralrs`   — GGUF path + mistral.rs / Candle-based engine.
 *   - `api`         — OpenAI-compatible endpoint (works for OpenAI,
 *                     OpenRouter, Ollama, LM Studio, vLLM, Together,
 *                     Groq) + API key + model string.
 *   - `aws_bedrock` — region + credential-mode selector + Bedrock model
 *                     ID.
 *
 * Parent: `SettingsPage.tsx`. Props mirror `AsrProviderSettings`: a
 * narrowed reducer slice + dispatch + translation handle + `testingKey`.
 */

import { invoke } from "@tauri-apps/api/core";
import type { TFunction } from "i18next";
import type { Dispatch, ReactNode } from "react";
import { LFM2_EXTRACT_MODEL_FILENAME } from "../modelConstants";
import type { ModelStatus } from "../types";
import {
  type AwsCredentialMode,
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
  handleClearCredential: (
    key: string,
    label: string,
    clearLocal: () => void,
  ) => Promise<void>;
  renderTestResult: (key: TestKey) => ReactNode;
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
  handleClearCredential,
  renderTestResult,
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

  return (
    <div className="settings-section">
      <h3 className="settings-section__title">{t("settings.sections.llm")}</h3>
      <div className="settings-radio-group">
        <label className="settings-radio">
          <input
            type="radio"
            name="llm-provider"
            checked={llmType === "local_llama"}
            onChange={() => dispatch(setField("llmType", "local_llama"))}
          />
          <span>{t("settings.llmProviders.localLlama")}</span>
          {llmType === "local_llama" && modelStatus && (
            <span
              className={`status-badge ${readinessBadge(modelStatus.llm).cls}`}
            >
              {t(readinessBadge(modelStatus.llm).labelKey)}
            </span>
          )}
        </label>
        <label className="settings-radio">
          <input
            type="radio"
            name="llm-provider"
            checked={llmType === "api"}
            onChange={() => dispatch(setField("llmType", "api"))}
          />
          <span>{t("settings.llmProviders.openaiCompatible")}</span>
        </label>
        <label className="settings-radio">
          <input
            type="radio"
            name="llm-provider"
            checked={llmType === "openrouter"}
            onChange={() => dispatch(setField("llmType", "openrouter"))}
          />
          <span>{t("settings.llmProviders.openrouter")}</span>
        </label>
        <label className="settings-radio">
          <input
            type="radio"
            name="llm-provider"
            checked={llmType === "aws_bedrock"}
            onChange={() => dispatch(setField("llmType", "aws_bedrock"))}
          />
          <span>{t("settings.llmProviders.awsBedrock")}</span>
        </label>
        <label className="settings-radio">
          <input
            type="radio"
            name="llm-provider"
            checked={llmType === "mistralrs"}
            onChange={() => dispatch(setField("llmType", "mistralrs"))}
          />
          <span>{t("settings.llmProviders.mistralrs")}</span>
        </label>
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
              Streaming prefill (experimental)
            </label>
            <p className="settings-hint">
              Warm the model with transcript while you speak and defer decoding
              until the turn ends, lowering post-turn extraction latency. Only
              the local llama.cpp backend supports this; other backends ignore
              it. (ADR-0012)
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
              vLLM local preset
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
              placeholder="https://openrouter.ai/api/v1"
            />
          </div>
          <div className="settings-field">
            <label
              className="settings-field__label"
              htmlFor="llm-custom-api-key"
            >
              {t("settings.fields.apiKey")}
            </label>
            <input
              id="llm-custom-api-key"
              className="settings-input"
              type="password"
              value={llmApiKey}
              onChange={(e) => dispatch(setField("llmApiKey", e.target.value))}
              placeholder="sk-..."
            />
          </div>
          <div className="settings-field">
            <label className="settings-field__label" htmlFor="llm-custom-model">
              {t("settings.fields.model")}
            </label>
            <input
              id="llm-custom-model"
              className="settings-input"
              type="text"
              value={llmModel}
              onChange={(e) => dispatch(setField("llmModel", e.target.value))}
              placeholder="gpt-4o-mini"
            />
          </div>
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
        </div>
      )}

      {llmType === "openrouter" && (
        <div className="settings-section__api-fields">
          <div className="settings-field">
            <label
              className="settings-field__label"
              htmlFor="llm-openrouter-api-key"
            >
              {t("settings.fields.apiKey")}
            </label>
            <input
              id="llm-openrouter-api-key"
              className="settings-input"
              type="password"
              value={openrouterApiKey}
              onChange={(e) =>
                dispatch(setField("openrouterApiKey", e.target.value))
              }
              placeholder="sk-or-..."
              aria-label={t("settings.fields.openrouterApiKey")}
            />
          </div>
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
            <label
              className="settings-field__label"
              htmlFor="llm-openrouter-model"
            >
              {t("settings.fields.model")}
            </label>
            <div className="settings-inline-row">
              <select
                id="llm-openrouter-model"
                className="settings-input"
                value={openrouterModel}
                onChange={(e) =>
                  dispatch(setField("openrouterModel", e.target.value))
                }
                aria-label={t("settings.fields.openrouterModel")}
              >
                <option value="">
                  {t("settings.placeholders.selectOpenrouterModel")}
                </option>
                {openrouterModels.map((m) => (
                  <option key={m.id} value={m.id}>
                    {m.name ? `${m.name} (${m.id})` : m.id}
                  </option>
                ))}
              </select>
              <button
                type="button"
                className="settings-btn settings-btn--secondary"
                disabled={openrouterModelsLoading || !openrouterApiKey.trim()}
                onClick={handleRefreshOpenRouterModels}
              >
                {openrouterModelsLoading
                  ? t("settings.buttons.refreshing")
                  : t("settings.buttons.refreshModels")}
              </button>
            </div>
            {openrouterModels.length === 0 && (
              <p className="settings-hint">
                {t("settings.hints.openrouterNoModels")}
              </p>
            )}
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
            <button
              type="button"
              className="settings-btn settings-btn--secondary"
              disabled={testingKey !== null || !openrouterApiKey.trim()}
              onClick={handleTestOpenRouter}
            >
              {testingKey === "openrouter"
                ? t("settings.buttons.testing")
                : t("settings.buttons.testConnection")}
            </button>
            {renderTestResult("openrouter")}
          </div>
          <div className="settings-field">
            <button
              type="button"
              className="settings-btn settings-btn--danger"
              onClick={() =>
                handleClearCredential(
                  "openrouter_api_key",
                  t("settings.credentialConfirm.openrouterApiKeyLabel"),
                  () => dispatch(setField("openrouterApiKey", "")),
                )
              }
            >
              {t("settings.buttons.clearSavedKey")}
            </button>
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
                    dispatch(setField("awsBedrockProfileName", e.target.value))
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
                  {t("settings.hints.noAwsProfiles")} <code>aws configure</code>{" "}
                  {t("settings.hints.noAwsProfilesSuffix")}
                </p>
              )}
            </div>
          )}
          {awsBedrockCredentialMode === "access_keys" && (
            <>
              <div className="settings-field">
                <label
                  className="settings-field__label"
                  htmlFor="llm-bedrock-access-key"
                >
                  {t("settings.fields.accessKeyId")}
                </label>
                <input
                  id="llm-bedrock-access-key"
                  className="settings-input"
                  type="password"
                  value={awsBedrockAccessKey}
                  onChange={(e) =>
                    dispatch(setField("awsBedrockAccessKey", e.target.value))
                  }
                  placeholder="AKIA..."
                />
              </div>
              <div className="settings-field">
                <label
                  className="settings-field__label"
                  htmlFor="llm-bedrock-secret-key"
                >
                  {t("settings.fields.secretAccessKey")}
                </label>
                <input
                  id="llm-bedrock-secret-key"
                  className="settings-input"
                  type="password"
                  value={awsBedrockSecretKey}
                  onChange={(e) =>
                    dispatch(setField("awsBedrockSecretKey", e.target.value))
                  }
                  placeholder="wJalr..."
                />
              </div>
              <div className="settings-field">
                <label
                  className="settings-field__label"
                  htmlFor="llm-bedrock-session-token"
                >
                  {t("settings.fields.sessionTokenOptional")}
                </label>
                <input
                  id="llm-bedrock-session-token"
                  className="settings-input"
                  type="password"
                  value={awsBedrockSessionToken}
                  onChange={(e) =>
                    dispatch(setField("awsBedrockSessionToken", e.target.value))
                  }
                  placeholder={t("settings.placeholders.sessionTokenHint")}
                />
              </div>
              <div className="settings-field">
                <button
                  type="button"
                  className="settings-btn settings-btn--danger"
                  onClick={() =>
                    handleClearCredential(
                      "aws_secret_key",
                      t("settings.credentialConfirm.awsKeysLabel"),
                      () => {
                        dispatch({ type: "CLEAR_AWS_SHARED_KEYS" });
                        invoke("delete_credential_cmd", {
                          key: "aws_session_token",
                        }).catch((e) =>
                          console.error(
                            "Failed to clear aws_session_token:",
                            e,
                          ),
                        );
                      },
                    )
                  }
                >
                  {t("settings.buttons.clearSavedAwsKeys")}
                </button>
              </div>
            </>
          )}
          <div className="settings-field">
            <button
              type="button"
              className="settings-btn settings-btn--secondary"
              disabled={testingKey !== null || !awsBedrockRegion}
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
            <input
              id="llm-mistralrs-model-id"
              className="settings-input"
              type="text"
              value={mistralrsModelId}
              onChange={(e) =>
                dispatch(setField("mistralrsModelId", e.target.value))
              }
              placeholder={LFM2_EXTRACT_MODEL_FILENAME}
            />
          </div>
        </div>
      )}
    </div>
  );
}
