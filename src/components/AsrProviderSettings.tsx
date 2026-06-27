/**
 * ASR provider sub-form — the choose-your-ASR surface inside
 * `SettingsPage`.
 *
 * Renders one of several backend-specific sub-panels based on the
 * reducer's current `asrType`:
 *   - `local_whisper`  — Whisper model file + language picker.
 *   - `api`            — OpenAI-compatible streaming endpoint + key.
 *   - `openai_realtime` — OpenAI Realtime transcription (`gpt-realtime-whisper`).
 *   - `aws_transcribe` — region + credential-mode selector
 *                        (`default_chain` / `profile` / `access_keys`).
 *   - `deepgram`       — API key + Deepgram model pick (nova-3, etc.).
 *   - `assemblyai`     — API key.
 *   - `sherpa_onnx`    — streaming Zipformer model selector (behind the
 *                        `sherpa-streaming` cargo feature).
 *
 * Parent: `SettingsPage.tsx`. Props: a narrowed reducer slice + dispatch
 * + translation handle + `testingKey` so concurrent "Test connection"
 * buttons stay disabled while any test is in flight.
 */

import type { TFunction } from "i18next";
import type { Dispatch, ReactNode } from "react";
import type {
  ModelStatus,
  ProviderDescriptor,
  ProviderModelCatalogItem,
  ProviderReadiness,
} from "../types";
import AdvancedSettingsDisclosure from "./AdvancedSettingsDisclosure";
import ModelCatalogPicker from "./ModelCatalogPicker";
import ProviderReadinessPanel, {
  type CredentialPresenceLookup,
} from "./ProviderReadinessPanel";
import type { ProviderSettingsOption } from "./providerRegistryHelpers";
import SecretCredentialControl, {
  AwsCredentialControl,
} from "./SecretCredentialControl";
import {
  type AwsCredentialMode,
  endpointCredentialKey,
  readinessBadge,
  type SettingsAction,
  type SettingsState,
  setField,
  type TestKey,
} from "./settingsTypes";

interface AsrProviderSettingsProps {
  state: Pick<
    SettingsState,
    | "asrType"
    | "whisperModel"
    | "asrEndpoint"
    | "asrApiKey"
    | "asrModel"
    | "openaiRealtimeApiKey"
    | "openaiRealtimeModel"
    | "openaiRealtimeLanguage"
    | "awsAsrRegion"
    | "awsAsrLanguageCode"
    | "awsAsrCredentialMode"
    | "awsAsrProfileName"
    | "awsAsrAccessKey"
    | "awsAsrSecretKey"
    | "awsAsrSessionToken"
    | "awsAsrDiarization"
    | "deepgramApiKey"
    | "deepgramModel"
    | "deepgramDiarization"
    | "deepgramEndpointingMs"
    | "deepgramUtteranceEndMs"
    | "deepgramVadEvents"
    | "deepgramEotThreshold"
    | "deepgramEagerEotThreshold"
    | "deepgramEotTimeoutMs"
    | "deepgramMaxSpeakers"
    | "assemblyaiApiKey"
    | "assemblyaiDiarization"
    | "sherpaModelDir"
    | "sherpaEndpointDetection"
    | "awsProfiles"
    | "testingKey"
    | "endpointCredentials"
  >;
  dispatch: Dispatch<SettingsAction>;
  t: TFunction;
  modelStatus: ModelStatus | null;
  refreshAwsProfiles: () => Promise<void>;
  handleTestAsrApi: () => Promise<void>;
  handleTestDeepgram: () => Promise<void>;
  handleTestAssemblyAI: () => Promise<void>;
  handleTestAwsAsr: () => Promise<void>;
  asrEndpointSavedKeyPresent: boolean;
  openaiSavedKeyPresent: boolean;
  awsSavedKeysPresent: boolean;
  awsSessionTokenSavedPresent: boolean;
  awsAccessKeysAvailable: boolean;
  deepgramCredentialAvailable: boolean;
  deepgramSavedKeyPresent: boolean;
  assemblyaiCredentialAvailable: boolean;
  assemblyaiSavedKeyPresent: boolean;
  providerOptions: ProviderSettingsOption<SettingsState["asrType"]>[];
  asrApiModelCatalog: ProviderModelCatalogItem[];
  deepgramModelCatalog: ProviderModelCatalogItem[];
  openaiRealtimeModelCatalog: ProviderModelCatalogItem[];
  sherpaModelCatalog: ProviderModelCatalogItem[];
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
}

export default function AsrProviderSettings({
  state,
  dispatch,
  t,
  modelStatus,
  refreshAwsProfiles,
  handleTestAsrApi,
  handleTestDeepgram,
  handleTestAssemblyAI,
  handleTestAwsAsr,
  asrEndpointSavedKeyPresent,
  openaiSavedKeyPresent,
  awsSavedKeysPresent,
  awsSessionTokenSavedPresent,
  awsAccessKeysAvailable,
  deepgramCredentialAvailable,
  deepgramSavedKeyPresent,
  assemblyaiCredentialAvailable,
  assemblyaiSavedKeyPresent,
  providerOptions,
  asrApiModelCatalog,
  deepgramModelCatalog,
  openaiRealtimeModelCatalog,
  sherpaModelCatalog,
  activeProviderDescriptor,
  activeProviderReadiness,
  credentialPresence,
  providerReadinessLoading,
  handleClearCredential,
  renderTestResult,
}: AsrProviderSettingsProps) {
  const {
    asrType,
    whisperModel,
    asrEndpoint,
    asrApiKey,
    asrModel,
    openaiRealtimeApiKey,
    openaiRealtimeModel,
    openaiRealtimeLanguage,
    awsAsrRegion,
    awsAsrLanguageCode,
    awsAsrCredentialMode,
    awsAsrProfileName,
    awsAsrAccessKey,
    awsAsrSecretKey,
    awsAsrSessionToken,
    awsAsrDiarization,
    deepgramApiKey,
    deepgramModel,
    deepgramDiarization,
    deepgramEndpointingMs,
    deepgramUtteranceEndMs,
    deepgramVadEvents,
    deepgramEotThreshold,
    deepgramEagerEotThreshold,
    deepgramEotTimeoutMs,
    deepgramMaxSpeakers,
    assemblyaiApiKey,
    assemblyaiDiarization,
    sherpaModelDir,
    sherpaEndpointDetection,
    awsProfiles,
    testingKey,
  } = state;
  const activeProviderDefaultModel =
    activeProviderDescriptor?.default_model ?? "";

  // When the user retargets the cloud-API endpoint to a different provider,
  // stash the key currently typed for the old endpoint into the per-endpoint
  // cache, then re-fill the visible key from whatever is cached for the new
  // endpoint. This makes provider round-trips (e.g. OpenAI → Groq → OpenAI)
  // lossless without re-typing, and avoids one provider's key bleeding into
  // another's field. (W3.5)
  const handleAsrEndpointChange = (endpoint: string) => {
    if (asrApiKey.trim()) {
      dispatch({
        type: "SET_ENDPOINT_CREDENTIALS",
        credentials: { [endpointCredentialKey(asrEndpoint)]: asrApiKey },
      });
    }
    dispatch(setField("asrEndpoint", endpoint));
    const cached =
      state.endpointCredentials[endpointCredentialKey(endpoint)] ?? "";
    if (cached !== asrApiKey) {
      dispatch(setField("asrApiKey", cached));
    }
  };

  return (
    <div className="settings-section">
      <h3 className="settings-section__title">{t("settings.sections.asr")}</h3>
      <ProviderReadinessPanel
        entry={activeProviderReadiness}
        descriptor={activeProviderDescriptor}
        credentialPresence={credentialPresence}
        loading={providerReadinessLoading}
        t={t}
      />
      <div className="settings-radio-group">
        {providerOptions.map((option) => (
          <label className="settings-radio" key={option.descriptor.id}>
            <input
              type="radio"
              name="asr-provider"
              checked={asrType === option.value}
              onChange={() => dispatch(setField("asrType", option.value))}
            />
            <span>{option.label}</span>
            {option.value === "local_whisper" &&
              asrType === "local_whisper" &&
              modelStatus && (
                <span
                  className={`status-badge ${readinessBadge(modelStatus.whisper).cls}`}
                >
                  {t(readinessBadge(modelStatus.whisper).labelKey)}
                </span>
              )}
          </label>
        ))}
      </div>

      {asrType === "local_whisper" && (
        <div className="settings-section__api-fields">
          <div className="settings-field">
            <label
              className="settings-field__label"
              htmlFor="asr-whisper-model"
            >
              {t("settings.fields.whisperModelSize")}
            </label>
            <select
              id="asr-whisper-model"
              className="settings-input"
              value={whisperModel}
              onChange={(e) =>
                dispatch(setField("whisperModel", e.target.value))
              }
            >
              <option value="ggml-tiny.en.bin">
                {t("settings.whisperModels.tiny")}
              </option>
              <option value="ggml-base.en.bin">
                {t("settings.whisperModels.base")}
              </option>
              <option value="ggml-small.en.bin">
                {t("settings.whisperModels.small")}
              </option>
              <option value="ggml-medium.en.bin">
                {t("settings.whisperModels.medium")}
              </option>
              <option value="ggml-large-v3.bin">
                {t("settings.whisperModels.large")}
              </option>
            </select>
          </div>
        </div>
      )}

      {asrType === "api" && (
        <div className="settings-section__api-fields">
          <div className="settings-field">
            <label className="settings-field__label" htmlFor="asr-endpoint">
              {t("settings.fields.endpoint")}
            </label>
            <input
              id="asr-endpoint"
              className="settings-input"
              type="text"
              value={asrEndpoint}
              onChange={(e) => handleAsrEndpointChange(e.target.value)}
              placeholder="https://api.openai.com/v1"
            />
          </div>
          <SecretCredentialControl
            id="asr-api-key"
            label={t("settings.fields.apiKey")}
            value={asrApiKey}
            onChange={(value) => dispatch(setField("asrApiKey", value))}
            placeholder="sk-..."
            saved={asrEndpointSavedKeyPresent}
            t={t}
            savedHint={t("settings.hints.endpointSavedKey")}
            onClear={
              asrEndpointSavedKeyPresent
                ? () =>
                    handleClearCredential(
                      endpointCredentialKey(asrEndpoint),
                      t("settings.fields.apiKey"),
                      () => dispatch(setField("asrApiKey", "")),
                    )
                : undefined
            }
          />
          <div className="settings-field">
            <label className="settings-field__label" htmlFor="asr-model">
              {t("settings.fields.model")}
            </label>
            <ModelCatalogPicker
              id="asr-model"
              value={asrModel}
              onChange={(value) => dispatch(setField("asrModel", value))}
              catalog={asrApiModelCatalog}
              t={t}
              placeholder="whisper-1"
            />
          </div>
          <div className="settings-field">
            <button
              type="button"
              className="settings-btn settings-btn--secondary"
              disabled={testingKey !== null || !asrEndpoint}
              onClick={handleTestAsrApi}
            >
              {testingKey === "asr_api"
                ? t("settings.buttons.testing")
                : t("settings.buttons.testConnection")}
            </button>
            {renderTestResult("asr_api")}
          </div>
        </div>
      )}

      {asrType === "openai_realtime" && (
        <div className="settings-section__api-fields">
          <SecretCredentialControl
            id="openai-realtime-api-key"
            label={t("settings.fields.apiKey")}
            value={openaiRealtimeApiKey}
            onChange={(value) =>
              dispatch(setField("openaiRealtimeApiKey", value))
            }
            placeholder="sk-..."
            saved={openaiSavedKeyPresent}
            t={t}
            savedHint={t("settings.hints.openaiSavedKey")}
            onClear={
              openaiSavedKeyPresent
                ? () =>
                    handleClearCredential(
                      "openai_api_key",
                      t("settings.fields.apiKey"),
                      () => dispatch(setField("openaiRealtimeApiKey", "")),
                    )
                : undefined
            }
          />
          <div className="settings-field">
            <label
              className="settings-field__label"
              htmlFor="openai-realtime-model"
            >
              {t("settings.fields.model")}
            </label>
            <ModelCatalogPicker
              id="openai-realtime-model"
              value={openaiRealtimeModel}
              onChange={(value) =>
                dispatch(setField("openaiRealtimeModel", value))
              }
              catalog={openaiRealtimeModelCatalog}
              t={t}
              placeholder={activeProviderDefaultModel}
            />
          </div>
          <div className="settings-field">
            <label
              className="settings-field__label"
              htmlFor="openai-realtime-language"
            >
              {t("settings.fields.languageCode")}
            </label>
            <input
              id="openai-realtime-language"
              className="settings-input"
              type="text"
              value={openaiRealtimeLanguage}
              onChange={(e) =>
                dispatch(setField("openaiRealtimeLanguage", e.target.value))
              }
              placeholder="en"
            />
          </div>
        </div>
      )}

      {asrType === "aws_transcribe" && (
        <div className="settings-section__api-fields">
          <div className="settings-field">
            <label className="settings-field__label" htmlFor="aws-asr-region">
              {t("settings.fields.region")}
            </label>
            <input
              id="aws-asr-region"
              className="settings-input"
              type="text"
              value={awsAsrRegion}
              onChange={(e) =>
                dispatch(setField("awsAsrRegion", e.target.value))
              }
              placeholder="us-east-1"
            />
          </div>
          <div className="settings-field">
            <label
              className="settings-field__label"
              htmlFor="aws-asr-language-code"
            >
              {t("settings.fields.languageCode")}
            </label>
            <input
              id="aws-asr-language-code"
              className="settings-input"
              type="text"
              value={awsAsrLanguageCode}
              onChange={(e) =>
                dispatch(setField("awsAsrLanguageCode", e.target.value))
              }
              placeholder="en-US"
            />
          </div>
          <AdvancedSettingsDisclosure
            summary={t("settings.sections.advancedProviderControls")}
          >
            <div className="settings-field">
              <label
                className="settings-field__label"
                htmlFor="aws-asr-credential-mode"
              >
                {t("settings.fields.credentialMode")}
              </label>
              <select
                id="aws-asr-credential-mode"
                className="settings-input"
                value={awsAsrCredentialMode}
                onChange={(e) =>
                  dispatch(
                    setField(
                      "awsAsrCredentialMode",
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
            {awsAsrCredentialMode === "profile" && (
              <div className="settings-field">
                <label
                  className="settings-field__label"
                  htmlFor="aws-asr-profile"
                >
                  {t("settings.fields.awsProfile")}
                </label>
                <div className="settings-inline-row">
                  <select
                    id="aws-asr-profile"
                    className="settings-input"
                    value={awsAsrProfileName}
                    onChange={(e) =>
                      dispatch(setField("awsAsrProfileName", e.target.value))
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
            {awsAsrCredentialMode === "access_keys" && (
              <AwsCredentialControl
                accessKeyId="aws-asr-access-key"
                secretKeyId="aws-asr-secret-key"
                sessionTokenId="aws-asr-session-token"
                accessKey={awsAsrAccessKey}
                secretKey={awsAsrSecretKey}
                sessionToken={awsAsrSessionToken}
                onAccessKeyChange={(value) =>
                  dispatch(setField("awsAsrAccessKey", value))
                }
                onSecretKeyChange={(value) =>
                  dispatch(setField("awsAsrSecretKey", value))
                }
                onSessionTokenChange={(value) =>
                  dispatch(setField("awsAsrSessionToken", value))
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
            <label className="settings-radio">
              <input
                type="checkbox"
                checked={awsAsrDiarization}
                onChange={(e) =>
                  dispatch(setField("awsAsrDiarization", e.target.checked))
                }
              />
              <span>{t("settings.fields.enableDiarization")}</span>
            </label>
          </div>
          <div className="settings-field">
            <button
              type="button"
              className="settings-btn settings-btn--secondary"
              disabled={
                testingKey !== null ||
                !awsAsrRegion ||
                (awsAsrCredentialMode === "access_keys" &&
                  !awsAccessKeysAvailable)
              }
              onClick={handleTestAwsAsr}
            >
              {testingKey === "aws_asr"
                ? t("settings.buttons.testing")
                : t("settings.buttons.testConnection")}
            </button>
            {renderTestResult("aws_asr")}
          </div>
        </div>
      )}

      {asrType === "deepgram" && (
        <div className="settings-section__api-fields">
          <SecretCredentialControl
            id="deepgram-api-key"
            label={t("settings.fields.apiKey")}
            value={deepgramApiKey}
            onChange={(value) => dispatch(setField("deepgramApiKey", value))}
            placeholder="dg-..."
            saved={deepgramSavedKeyPresent}
            t={t}
            savedHint={t("settings.hints.deepgramSavedKey")}
            onClear={
              deepgramSavedKeyPresent
                ? () =>
                    handleClearCredential(
                      "deepgram_api_key",
                      t("settings.tts.deepgramApiKey"),
                      () => dispatch(setField("deepgramApiKey", "")),
                    )
                : undefined
            }
          />
          <div className="settings-field">
            <label className="settings-field__label" htmlFor="deepgram-model">
              {t("settings.fields.model")}
            </label>
            <ModelCatalogPicker
              id="deepgram-model"
              value={deepgramModel}
              onChange={(value) => dispatch(setField("deepgramModel", value))}
              catalog={deepgramModelCatalog}
              t={t}
              placeholder={activeProviderDefaultModel}
            />
          </div>
          <div className="settings-field">
            <label className="settings-radio">
              <input
                type="checkbox"
                checked={deepgramDiarization}
                onChange={(e) =>
                  dispatch(setField("deepgramDiarization", e.target.checked))
                }
              />
              <span>{t("settings.fields.enableDiarization")}</span>
            </label>
          </div>
          <AdvancedSettingsDisclosure
            summary={t("settings.sections.advancedProviderControls")}
          >
            <div className="settings-field">
              <label
                className="settings-field__label"
                htmlFor="deepgram-endpointing-ms"
              >
                {t("settings.fields.deepgramEndpointingMs")}
              </label>
              <input
                id="deepgram-endpointing-ms"
                className="settings-input"
                type="number"
                min={0}
                step={50}
                value={deepgramEndpointingMs}
                onChange={(e) =>
                  dispatch(
                    setField("deepgramEndpointingMs", Number(e.target.value)),
                  )
                }
              />
            </div>
            <div className="settings-field">
              <label
                className="settings-field__label"
                htmlFor="deepgram-utterance-end-ms"
              >
                {t("settings.fields.deepgramUtteranceEndMs")}
              </label>
              <input
                id="deepgram-utterance-end-ms"
                className="settings-input"
                type="number"
                min={0}
                step={100}
                value={deepgramUtteranceEndMs}
                onChange={(e) =>
                  dispatch(
                    setField("deepgramUtteranceEndMs", Number(e.target.value)),
                  )
                }
              />
            </div>
            <div className="settings-field">
              <label className="settings-radio">
                <input
                  type="checkbox"
                  checked={deepgramVadEvents}
                  onChange={(e) =>
                    dispatch(setField("deepgramVadEvents", e.target.checked))
                  }
                />
                <span>{t("settings.fields.deepgramVadEvents")}</span>
              </label>
            </div>
            <div className="settings-field">
              <label
                className="settings-field__label"
                htmlFor="deepgram-eot-threshold"
              >
                {t("settings.fields.deepgramEotThreshold")}
              </label>
              <input
                id="deepgram-eot-threshold"
                className="settings-input"
                type="number"
                min={0}
                max={1}
                step={0.05}
                value={deepgramEotThreshold}
                onChange={(e) =>
                  dispatch(
                    setField("deepgramEotThreshold", Number(e.target.value)),
                  )
                }
              />
            </div>
            <div className="settings-field">
              <label
                className="settings-field__label"
                htmlFor="deepgram-eager-eot-threshold"
              >
                {t("settings.fields.deepgramEagerEotThreshold")}
              </label>
              <input
                id="deepgram-eager-eot-threshold"
                className="settings-input"
                type="number"
                min={0}
                max={1}
                step={0.05}
                value={deepgramEagerEotThreshold}
                onChange={(e) =>
                  dispatch(
                    setField(
                      "deepgramEagerEotThreshold",
                      Number(e.target.value),
                    ),
                  )
                }
              />
            </div>
            <div className="settings-field">
              <label
                className="settings-field__label"
                htmlFor="deepgram-eot-timeout-ms"
              >
                {t("settings.fields.deepgramEotTimeoutMs")}
              </label>
              <input
                id="deepgram-eot-timeout-ms"
                className="settings-input"
                type="number"
                min={0}
                step={100}
                value={deepgramEotTimeoutMs}
                onChange={(e) =>
                  dispatch(
                    setField("deepgramEotTimeoutMs", Number(e.target.value)),
                  )
                }
              />
            </div>
            <div className="settings-field">
              <label
                className="settings-field__label"
                htmlFor="deepgram-max-speakers"
              >
                {t("settings.fields.deepgramMaxSpeakers")}
              </label>
              <input
                id="deepgram-max-speakers"
                className="settings-input"
                type="number"
                min={0}
                step={1}
                value={deepgramMaxSpeakers}
                onChange={(e) =>
                  dispatch(
                    setField("deepgramMaxSpeakers", Number(e.target.value)),
                  )
                }
              />
              <p className="settings-hint">
                {t("settings.hints.deepgramMaxSpeakers")}
              </p>
            </div>
          </AdvancedSettingsDisclosure>
          <div className="settings-field">
            <button
              type="button"
              className="settings-btn settings-btn--secondary"
              disabled={testingKey !== null || !deepgramCredentialAvailable}
              onClick={handleTestDeepgram}
            >
              {testingKey === "deepgram"
                ? t("settings.buttons.testing")
                : t("settings.buttons.testConnection")}
            </button>
            {renderTestResult("deepgram")}
          </div>
        </div>
      )}

      {asrType === "assemblyai" && (
        <div className="settings-section__api-fields">
          <SecretCredentialControl
            id="assemblyai-api-key"
            label={t("settings.fields.apiKey")}
            value={assemblyaiApiKey}
            onChange={(value) => dispatch(setField("assemblyaiApiKey", value))}
            placeholder={t("settings.placeholders.assemblyaiApiKey")}
            saved={assemblyaiSavedKeyPresent}
            t={t}
            savedHint={t("settings.hints.assemblyaiSavedKey")}
            onClear={
              assemblyaiSavedKeyPresent
                ? () =>
                    handleClearCredential(
                      "assemblyai_api_key",
                      t("settings.fields.apiKey"),
                      () => dispatch(setField("assemblyaiApiKey", "")),
                    )
                : undefined
            }
          />
          <div className="settings-field">
            <label className="settings-radio">
              <input
                type="checkbox"
                checked={assemblyaiDiarization}
                onChange={(e) =>
                  dispatch(setField("assemblyaiDiarization", e.target.checked))
                }
              />
              <span>{t("settings.fields.enableDiarization")}</span>
            </label>
          </div>
          <div className="settings-field">
            <button
              type="button"
              className="settings-btn settings-btn--secondary"
              disabled={testingKey !== null || !assemblyaiCredentialAvailable}
              onClick={handleTestAssemblyAI}
            >
              {testingKey === "assemblyai"
                ? t("settings.buttons.testing")
                : t("settings.buttons.testConnection")}
            </button>
            {renderTestResult("assemblyai")}
          </div>
        </div>
      )}

      {asrType === "sherpa_onnx" && (
        <div className="settings-section__api-fields">
          <div className="settings-field">
            <label className="settings-field__label" htmlFor="sherpa-model-dir">
              {t("settings.fields.modelDirectory")}
            </label>
            <ModelCatalogPicker
              id="sherpa-model-dir"
              value={sherpaModelDir}
              onChange={(value) => dispatch(setField("sherpaModelDir", value))}
              catalog={sherpaModelCatalog}
              t={t}
              placeholder={activeProviderDefaultModel}
            />
          </div>
          <div className="settings-field">
            <label className="settings-radio">
              <input
                type="checkbox"
                checked={sherpaEndpointDetection}
                onChange={(e) =>
                  dispatch(
                    setField("sherpaEndpointDetection", e.target.checked),
                  )
                }
              />
              <span>{t("settings.fields.enableEndpointDetection")}</span>
            </label>
          </div>
        </div>
      )}
    </div>
  );
}
