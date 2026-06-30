/**
 * Gemini rail section (blueprint §5). Thin shell over the already-extracted
 * `<GeminiSettings>` plus the native speech-to-speech conversation toggle.
 * Mounted by the parent shell only when its rail item is active, so the panel
 * is the single mount point for predictable scroll-containment.
 */

import GeminiSettings from "../GeminiSettings";
import ProviderCapabilityStageSection from "./ProviderCapabilityStageSection";
import { useSettings } from "./SettingsContext";

export default function GeminiPanel() {
  const {
    state,
    dispatch,
    t,
    nativeRealtimeSelected,
    handleNativeRealtimeToggle,
    handleTestGemini,
    geminiCredentialAvailable,
    geminiSavedKeyPresent,
    geminiApiKey,
    geminiServiceAccountPathSavedPresent,
    geminiServiceAccountPath,
    geminiModelCatalog,
    geminiProviderDescriptor,
    geminiProviderReadiness,
    credentialPresence,
    providerReadinessLoading,
    handleClearCredential,
    renderTestResult,
  } = useSettings();
  return (
    <>
      <section className="settings-section">
        <h3 className="settings-section-title">
          {t("settings.conversation.title")}
        </h3>
        <p className="settings-section-help">
          {t("settings.conversation.help")}
        </p>
        <div className="settings-field settings-field--inline">
          <label>
            <input
              type="checkbox"
              checked={nativeRealtimeSelected}
              onChange={(e) => handleNativeRealtimeToggle(e.target.checked)}
            />{" "}
            {t("settings.conversation.enableNative")}
          </label>
        </div>
      </section>
      <GeminiSettings
        state={state}
        dispatch={dispatch}
        t={t}
        handleTestGemini={handleTestGemini}
        geminiCredentialAvailable={geminiCredentialAvailable}
        geminiSavedKeyPresent={geminiSavedKeyPresent && !geminiApiKey.trim()}
        geminiServiceAccountPathSavedPresent={
          geminiServiceAccountPathSavedPresent &&
          !geminiServiceAccountPath.trim()
        }
        geminiModelCatalog={geminiModelCatalog}
        providerDescriptor={geminiProviderDescriptor}
        providerReadiness={geminiProviderReadiness}
        credentialPresence={credentialPresence}
        providerReadinessLoading={providerReadinessLoading}
        handleClearCredential={handleClearCredential}
        renderTestResult={renderTestResult}
      />
      <ProviderCapabilityStageSection stage="realtime_agent" />
    </>
  );
}
