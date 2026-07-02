/**
 * ASR provider sub-form — the choose-your-ASR surface inside
 * `SettingsPage`.
 *
 * Renders one of several backend-specific sub-panels based on the
 * reducer's current `asrType`:
 *   - `local_whisper`  — Whisper model file + language picker.
 *   - `api`            — OpenAI-compatible streaming endpoint + key.
 *   - `openai_realtime` — OpenAI Realtime **transcription** (STT,
 *                        `gpt-realtime-whisper`). NOTE: this is the
 *                        speech-to-text provider and is distinct from the
 *                        OpenAI Realtime **voice agent** (S2S, `gpt-realtime-2`)
 *                        whose settings live under `openai_realtime_agent` and
 *                        are selected via the converse-mode realtime-agent
 *                        picker — do not conflate the two `openai-realtime-*`
 *                        field sets.
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
import Button from "./Button";
import FieldRow from "./FieldRow";
import ModelCatalogField from "./ModelCatalogField";
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
  // Generic model-catalog refresh keyed by provider id — powers the uniform
  // Load-models button for asr.api and asr.deepgram.
  handleRefreshModels: (providerId: string) => void;
  asrEndpointSavedKeyPresent: boolean;
  openaiSavedKeyPresent: boolean;
  awsSavedKeysPresent: boolean;
  awsSessionTokenSavedPresent: boolean;
  awsAccessKeysAvailable: boolean;
  deepgramCredentialAvailable: boolean;
  deepgramSavedKeyPresent: boolean;
  deepgramModelsLoading: boolean;
  deepgramModelsError: string | null;
  asrApiCredentialAvailable: boolean;
  asrApiModelsLoading: boolean;
  asrApiModelsError: string | null;
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
  handleRefreshModels,
  asrEndpointSavedKeyPresent,
  openaiSavedKeyPresent,
  awsSavedKeysPresent,
  awsSessionTokenSavedPresent,
  awsAccessKeysAvailable,
  deepgramCredentialAvailable,
  deepgramSavedKeyPresent,
  deepgramModelsLoading,
  deepgramModelsError,
  asrApiCredentialAvailable,
  asrApiModelsLoading,
  asrApiModelsError,
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
      <div
        className="settings-radio-group"
        role="radiogroup"
        aria-label={t("settings.a11y.chooseAsrProvider")}
      >
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
          <FieldRow
            htmlFor="asr-whisper-model"
            label={t("settings.fields.whisperModelSize")}
          >
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
          </FieldRow>
        </div>
      )}

      {asrType === "api" && (
        <div className="settings-section__api-fields">
          <FieldRow
            htmlFor="asr-endpoint"
            label={t("settings.fields.endpoint")}
          >
            <input
              id="asr-endpoint"
              className="settings-input"
              type="text"
              value={asrEndpoint}
              onChange={(e) => handleAsrEndpointChange(e.target.value)}
              placeholder="https://api.openai.com/v1"
            />
          </FieldRow>
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
          <FieldRow htmlFor="asr-model" label={t("settings.fields.model")}>
            <ModelCatalogField
              id="asr-model"
              value={asrModel}
              onChange={(value) => dispatch(setField("asrModel", value))}
              catalog={asrApiModelCatalog}
              t={t}
              providerName={t("settings.asrProviders.cloudApi")}
              placeholder="whisper-1"
              loading={asrApiModelsLoading}
              error={asrApiModelsError}
              credentialAvailable={asrApiCredentialAvailable}
              onRefresh={() => handleRefreshModels("asr.api")}
              hasRemoteCommand
            />
          </FieldRow>
          <div className="settings-field">
            <Button
              variant="info"
              disabled={testingKey !== null || !asrEndpoint}
              onClick={handleTestAsrApi}
            >
              {testingKey === "asr_api"
                ? t("settings.buttons.testing")
                : t("settings.buttons.testConnection")}
            </Button>
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
          <FieldRow
            htmlFor="openai-realtime-model"
            label={t("settings.fields.model")}
          >
            <ModelCatalogField
              id="openai-realtime-model"
              value={openaiRealtimeModel}
              onChange={(value) =>
                dispatch(setField("openaiRealtimeModel", value))
              }
              catalog={openaiRealtimeModelCatalog}
              t={t}
              providerName={t("settings.asrProviders.openaiRealtime")}
              placeholder={activeProviderDefaultModel}
              hasRemoteCommand={false}
            />
          </FieldRow>
          <FieldRow
            htmlFor="openai-realtime-language"
            label={t("settings.fields.languageCode")}
          >
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
          </FieldRow>
        </div>
      )}

      {asrType === "aws_transcribe" && (
        <div className="settings-section__api-fields">
          <FieldRow
            htmlFor="aws-asr-region"
            label={t("settings.fields.region")}
          >
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
          </FieldRow>
          <FieldRow
            htmlFor="aws-asr-language-code"
            label={t("settings.fields.languageCode")}
          >
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
          </FieldRow>
          <AdvancedSettingsDisclosure
            summary={t("settings.sections.advancedProviderControls")}
          >
            <FieldRow
              htmlFor="aws-asr-credential-mode"
              label={t("settings.fields.credentialMode")}
            >
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
            </FieldRow>
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
                  <Button variant="info" onClick={refreshAwsProfiles}>
                    {t("settings.buttons.refresh")}
                  </Button>
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
            <Button
              variant="info"
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
            </Button>
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
          <FieldRow htmlFor="deepgram-model" label={t("settings.fields.model")}>
            <ModelCatalogField
              id="deepgram-model"
              value={deepgramModel}
              onChange={(value) => dispatch(setField("deepgramModel", value))}
              catalog={deepgramModelCatalog}
              t={t}
              providerName={t("settings.asrProviders.deepgram")}
              placeholder={activeProviderDefaultModel}
              loading={deepgramModelsLoading}
              error={deepgramModelsError}
              credentialAvailable={deepgramCredentialAvailable}
              onRefresh={() => handleRefreshModels("asr.deepgram")}
              hasRemoteCommand
            />
          </FieldRow>
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
            <FieldRow
              htmlFor="deepgram-endpointing-ms"
              label={t("settings.fields.deepgramEndpointingMs")}
            >
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
            </FieldRow>
            <FieldRow
              htmlFor="deepgram-utterance-end-ms"
              label={t("settings.fields.deepgramUtteranceEndMs")}
            >
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
            </FieldRow>
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
            <FieldRow
              htmlFor="deepgram-eot-threshold"
              label={t("settings.fields.deepgramEotThreshold")}
            >
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
            </FieldRow>
            <FieldRow
              htmlFor="deepgram-eager-eot-threshold"
              label={t("settings.fields.deepgramEagerEotThreshold")}
            >
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
            </FieldRow>
            <FieldRow
              htmlFor="deepgram-eot-timeout-ms"
              label={t("settings.fields.deepgramEotTimeoutMs")}
            >
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
            </FieldRow>
            <FieldRow
              htmlFor="deepgram-max-speakers"
              label={t("settings.fields.deepgramMaxSpeakers")}
              hint={t("settings.hints.deepgramMaxSpeakers")}
            >
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
            </FieldRow>
          </AdvancedSettingsDisclosure>
          <div className="settings-field">
            <Button
              variant="info"
              disabled={testingKey !== null || !deepgramCredentialAvailable}
              onClick={handleTestDeepgram}
            >
              {testingKey === "deepgram"
                ? t("settings.buttons.testing")
                : t("settings.buttons.testConnection")}
            </Button>
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
            <Button
              variant="info"
              disabled={testingKey !== null || !assemblyaiCredentialAvailable}
              onClick={handleTestAssemblyAI}
            >
              {testingKey === "assemblyai"
                ? t("settings.buttons.testing")
                : t("settings.buttons.testConnection")}
            </Button>
            {renderTestResult("assemblyai")}
          </div>
        </div>
      )}

      {asrType === "sherpa_onnx" && (
        <div className="settings-section__api-fields">
          <FieldRow
            htmlFor="sherpa-model-dir"
            label={t("settings.fields.modelDirectory")}
          >
            <ModelCatalogField
              id="sherpa-model-dir"
              value={sherpaModelDir}
              onChange={(value) => dispatch(setField("sherpaModelDir", value))}
              catalog={sherpaModelCatalog}
              t={t}
              providerName={t("settings.asrProviders.sherpaOnnx")}
              placeholder={activeProviderDefaultModel}
              hasRemoteCommand={false}
            />
          </FieldRow>
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
