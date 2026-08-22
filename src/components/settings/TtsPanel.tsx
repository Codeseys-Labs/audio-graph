/**
 * Text-to-Speech rail section (blueprint §5, Phase 3). Extracts the inline TTS
 * block — provider select, Aura voice/speed, speak-aloud, Deepgram key, and the
 * test-connection control. The TTS draft state (ttsType/auraVoice/auraSpeed/
 * speakAloud) deliberately stays in the controller so the dirty
 * `settingsFingerprint` + Save keep reading it unchanged (plan Phase 3 note);
 * TtsPanel OWNS the TTS UI and reads/writes that state via `useSettings()`.
 * Test ids tts-provider-select / aura-voice-select / tts-deepgram-api-key are
 * preserved for the TTS tests.
 */

import ModelCatalogPicker from "../ModelCatalogPicker";
import ProviderReadinessPanel from "../ProviderReadinessPanel";
import SecretCredentialControl from "../SecretCredentialControl";
import { setField } from "../settingsTypes";
import ProviderCapabilityStageSection from "./ProviderCapabilityStageSection";
import { useSettings } from "./SettingsContext";
import {
  DEFAULT_AURA_VOICE,
  TTS_PROVIDER_OPTIONS,
  type TtsType,
} from "./useSettingsController";

export default function TtsPanel() {
  const {
    t,
    dispatch,
    settingsLoading,
    activeTtsProviderReadiness,
    activeTtsProviderDescriptor,
    credentialPresence,
    providerReadinessLoading,
    ttsType,
    setTtsType,
    auraVoice,
    setAuraVoice,
    auraVoiceCatalog,
    auraSpeed,
    setAuraSpeed,
    speakAloud,
    setSpeakAloud,
    deepgramApiKey,
    deepgramSavedKeyPresent,
    deepgramCredentialAvailable,
    handleClearCredential,
    handleTestTts,
    testingTts,
    ttsTestResult,
  } = useSettings();
  // Silent-discard warning (settings T3, audio-graph-9d2b): `tts_provider`
  // only persists voice/speed when `ttsType === "deepgram_aura"`
  // (useSettingsController's save payload collapses to `{type: "none"}`
  // otherwise) — the aura draft state itself survives in memory regardless
  // of `ttsType`, so a switch-away-then-save silently drops it with no
  // preview today. Only warn once there is something non-default to lose —
  // a fresh install with untouched aura defaults and `ttsType: "none"`
  // should not see a warning about discarding nothing.
  //
  // `speak_aloud` is deliberately EXCLUDED from both this condition and the
  // warning copy below: unlike voice/speed, `speak_aloud` is written to the
  // save payload unconditionally (useSettingsController's save payload sets
  // `speak_aloud: speakAloud` outside the `tts_provider` ternary), so it is
  // never actually discarded on save — claiming otherwise would be a false
  // statement inside the discard-warning trust mechanism (reviewer finding,
  // audio-graph-9d2b fix pass).
  const auraSettingsWouldBeDiscarded =
    ttsType !== "deepgram_aura" &&
    (auraVoice !== DEFAULT_AURA_VOICE || auraSpeed !== 1.0);
  return (
    <>
      {/* ── Text-to-Speech (Wave C / ADR-0004 + ADR-0006) ─────────── */}
      <section className="settings-section">
        <h3 className="settings-section-title">{t("settings.tts.title")}</h3>
        <p className="settings-section-help">{t("settings.tts.help")}</p>
        <ProviderReadinessPanel
          entry={activeTtsProviderReadiness}
          descriptor={activeTtsProviderDescriptor}
          credentialPresence={credentialPresence}
          loading={providerReadinessLoading}
          t={t}
        />

        <div className="settings-field">
          <label htmlFor="tts-provider-select">
            {t("settings.tts.provider")}
          </label>
          <select
            id="tts-provider-select"
            value={ttsType}
            onChange={(e) => setTtsType(e.target.value as TtsType)}
            disabled={settingsLoading}
          >
            {TTS_PROVIDER_OPTIONS.map((option) => (
              <option key={option.descriptor.id} value={option.value}>
                {option.label}
              </option>
            ))}
          </select>
          {auraSettingsWouldBeDiscarded && (
            <p
              className="settings-hint settings-tts-discard-warning"
              role="status"
            >
              {t("settings.tts.discardWarning", {
                voice: auraVoice,
                speed: auraSpeed,
              })}
            </p>
          )}
        </div>

        {ttsType === "deepgram_aura" && (
          <>
            <div className="settings-field">
              <label
                className="settings-field__label"
                htmlFor="aura-voice-select"
              >
                {t("settings.tts.voice")}
              </label>
              <ModelCatalogPicker
                id="aura-voice-select"
                value={auraVoice}
                onChange={setAuraVoice}
                catalog={auraVoiceCatalog}
                t={t}
                placeholder={DEFAULT_AURA_VOICE}
                ariaLabel={t("settings.tts.voice")}
                disabled={settingsLoading}
              />
            </div>

            <div className="settings-field">
              <label htmlFor="aura-speed-input">
                {t("settings.tts.speed")}
              </label>
              <input
                id="aura-speed-input"
                type="number"
                step="0.1"
                min="0.7"
                max="1.5"
                value={auraSpeed}
                onChange={(e) =>
                  setAuraSpeed(
                    Math.max(0.7, Math.min(1.5, Number(e.target.value))),
                  )
                }
                disabled={settingsLoading}
              />
            </div>

            <div className="settings-field">
              <label htmlFor="speak-aloud-toggle">
                <input
                  id="speak-aloud-toggle"
                  type="checkbox"
                  checked={speakAloud}
                  onChange={(e) => setSpeakAloud(e.target.checked)}
                  disabled={settingsLoading}
                />
                &nbsp;{t("settings.tts.speakAloud")}
              </label>
            </div>

            <SecretCredentialControl
              id="tts-deepgram-api-key"
              label={t("settings.tts.deepgramApiKey")}
              value={deepgramApiKey}
              onChange={(value) => dispatch(setField("deepgramApiKey", value))}
              placeholder="dg-..."
              saved={deepgramSavedKeyPresent}
              t={t}
              disabled={settingsLoading}
              savedHint={t("settings.hints.deepgramSavedKey")}
              missingHint={t("settings.tts.needKeyHint")}
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
              <button
                type="button"
                className="settings-btn"
                onClick={handleTestTts}
                disabled={
                  settingsLoading || testingTts || !deepgramCredentialAvailable
                }
              >
                {testingTts
                  ? t("settings.buttons.testing")
                  : t("settings.buttons.testConnection")}
              </button>
              {ttsTestResult && (
                <div
                  className={
                    ttsTestResult.ok
                      ? "settings-test-result settings-test-result--ok"
                      : "settings-test-result settings-test-result--err"
                  }
                >
                  {ttsTestResult.msg}
                </div>
              )}
            </div>
          </>
        )}
      </section>
      <ProviderCapabilityStageSection stage="tts" />
    </>
  );
}
