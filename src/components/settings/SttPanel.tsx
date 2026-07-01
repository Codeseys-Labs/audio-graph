/**
 * Speech-to-Text rail section (blueprint §5). Diarization controls +
 * `<AsrProviderSettings>` + the readiness/advanced disclosure. Consumes the
 * settings controller via `useSettings()` (Phase 2) instead of prop-drilling.
 */

import type { DiarizationMode, DiarizationSpeakerCount } from "../../types";
import AdvancedSettingsDisclosure from "../AdvancedSettingsDisclosure";
import AsrProviderSettings from "../AsrProviderSettings";
import { setField } from "../settingsTypes";
import ProviderCapabilityStageSection from "./ProviderCapabilityStageSection";
import { useSettings } from "./SettingsContext";
import { ASR_PROVIDER_OPTIONS } from "./useSettingsController";

export default function SttPanel() {
  const {
    t,
    state,
    dispatch,
    modelStatus,
    diarizationMode,
    diarizationSpeakerCount,
    diarizationMaxSpeakers,
    providerDiarizationSupported,
    localDiarizationReady,
    selectedDiarizationModeUnavailable,
    asrApiKey,
    asrApiModelCatalog,
    asrEndpointSavedKeyPresent,
    openaiRealtimeApiKey,
    openaiRealtimeModelCatalog,
    openaiSavedKeyPresent,
    deepgramApiKey,
    deepgramModelCatalog,
    deepgramCredentialAvailable,
    deepgramSavedKeyPresent,
    assemblyaiApiKey,
    assemblyaiCredentialAvailable,
    assemblyaiSavedKeyPresent,
    sherpaModelCatalog,
    awsAsrAccessKey,
    awsAsrSecretKey,
    awsAsrSessionToken,
    awsAsrAccessKeysAvailable,
    awsSavedKeysPresent,
    awsSessionTokenSavedPresent,
    activeAsrProviderDescriptor,
    activeAsrProviderReadiness,
    credentialPresence,
    providerReadinessLoading,
    refreshAwsProfiles,
    handleTestAsrApi,
    handleTestDeepgram,
    handleTestAssemblyAI,
    handleTestAwsAsr,
    handleClearCredential,
    renderTestResult,
  } = useSettings();
  return (
    <>
      <section className="settings-section">
        <h3 className="settings-section-title">
          {t("settings.diarization.title")}
        </h3>
        <p className="settings-section-help">
          {t("settings.diarization.help")}
        </p>
        <div className="settings-field">
          <label className="settings-field__label" htmlFor="diarization-mode">
            {t("settings.diarization.mode")}
          </label>
          <select
            id="diarization-mode"
            className="settings-input"
            value={diarizationMode}
            onChange={(e) =>
              dispatch(
                setField("diarizationMode", e.target.value as DiarizationMode),
              )
            }
          >
            <option value="off">{t("settings.diarization.modes.off")}</option>
            <option value="provider" disabled={!providerDiarizationSupported}>
              {t("settings.diarization.modes.provider")}
            </option>
            <option value="local" disabled={!localDiarizationReady}>
              {t("settings.diarization.modes.local")}
            </option>
            <option
              value="hybrid"
              disabled={!providerDiarizationSupported || !localDiarizationReady}
            >
              {t("settings.diarization.modes.hybrid")}
            </option>
          </select>
          {selectedDiarizationModeUnavailable && (
            <p className="settings-hint">
              {t("settings.diarization.unavailable")}
            </p>
          )}
        </div>
        <div className="settings-field">
          <label
            className="settings-field__label"
            htmlFor="diarization-speaker-count"
          >
            {t("settings.diarization.speakerCount")}
          </label>
          <select
            id="diarization-speaker-count"
            className="settings-input"
            value={diarizationSpeakerCount}
            onChange={(e) =>
              dispatch(
                setField(
                  "diarizationSpeakerCount",
                  e.target.value as DiarizationSpeakerCount,
                ),
              )
            }
          >
            <option value="auto">
              {t("settings.diarization.speakerCounts.auto")}
            </option>
            <option value="unbounded">
              {t("settings.diarization.speakerCounts.unbounded")}
            </option>
            <option value="fixed">
              {t("settings.diarization.speakerCounts.fixed")}
            </option>
          </select>
        </div>
        <AdvancedSettingsDisclosure
          summary={t("settings.sections.advancedProviderControls")}
        >
          <div className="settings-field">
            <label
              className="settings-field__label"
              htmlFor="diarization-max-speakers"
            >
              {t("settings.diarization.maxSpeakers")}
            </label>
            <input
              id="diarization-max-speakers"
              className="settings-input"
              type="number"
              min={1}
              step={1}
              value={Math.max(1, diarizationMaxSpeakers || 1)}
              disabled={diarizationSpeakerCount !== "fixed"}
              onChange={(e) =>
                dispatch(
                  setField("diarizationMaxSpeakers", Number(e.target.value)),
                )
              }
            />
            <p className="settings-hint">
              {t("settings.diarization.maxSpeakersHint")}
            </p>
          </div>
        </AdvancedSettingsDisclosure>
      </section>
      <AsrProviderSettings
        state={state}
        dispatch={dispatch}
        t={t}
        modelStatus={modelStatus}
        refreshAwsProfiles={refreshAwsProfiles}
        handleTestAsrApi={handleTestAsrApi}
        handleTestDeepgram={handleTestDeepgram}
        handleTestAssemblyAI={handleTestAssemblyAI}
        handleTestAwsAsr={handleTestAwsAsr}
        asrEndpointSavedKeyPresent={
          asrEndpointSavedKeyPresent && !asrApiKey.trim()
        }
        openaiSavedKeyPresent={
          openaiSavedKeyPresent && !openaiRealtimeApiKey.trim()
        }
        awsSavedKeysPresent={
          awsSavedKeysPresent &&
          !awsAsrAccessKey.trim() &&
          !awsAsrSecretKey.trim() &&
          !awsAsrSessionToken.trim()
        }
        awsSessionTokenSavedPresent={awsSessionTokenSavedPresent}
        awsAccessKeysAvailable={awsAsrAccessKeysAvailable}
        deepgramCredentialAvailable={deepgramCredentialAvailable}
        deepgramSavedKeyPresent={
          deepgramSavedKeyPresent && !deepgramApiKey.trim()
        }
        assemblyaiCredentialAvailable={assemblyaiCredentialAvailable}
        assemblyaiSavedKeyPresent={
          assemblyaiSavedKeyPresent && !assemblyaiApiKey.trim()
        }
        providerOptions={ASR_PROVIDER_OPTIONS}
        asrApiModelCatalog={asrApiModelCatalog}
        deepgramModelCatalog={deepgramModelCatalog}
        openaiRealtimeModelCatalog={openaiRealtimeModelCatalog}
        sherpaModelCatalog={sherpaModelCatalog}
        activeProviderDescriptor={activeAsrProviderDescriptor}
        activeProviderReadiness={activeAsrProviderReadiness}
        credentialPresence={credentialPresence}
        providerReadinessLoading={providerReadinessLoading}
        handleClearCredential={handleClearCredential}
        renderTestResult={renderTestResult}
      />
      <ProviderCapabilityStageSection stage="asr" />
    </>
  );
}
