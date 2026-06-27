/**
 * Gemini Live sub-form — auth + model configuration for the dedicated
 * Gemini WebSocket path (distinct from the generic "LLM" selector).
 *
 * Auth modes:
 *   - `api_key`   — AI Studio API key (plain header auth).
 *   - `vertex_ai` — Google Cloud service account + project + location for
 *                   Vertex AI bearer-token auth (via `gcp_auth`).
 *
 * Model picker covers the Gemini Live-capable variants (e.g.
 * `gemini-2.0-flash-live-001`); language picker is documented separately in
 * `docs/GEMINI_LANGUAGES.md`.
 *
 * Parent: `SettingsPage.tsx`. Props: narrowed reducer slice + dispatch +
 * translation handle + `testingKey` for the Test-API-key button.
 */

import type { TFunction } from "i18next";
import type { Dispatch, ReactNode } from "react";
import type {
  ProviderDescriptor,
  ProviderModelCatalogItem,
  ProviderReadiness,
} from "../types";
import ModelCatalogPicker from "./ModelCatalogPicker";
import ProviderReadinessPanel, {
  type CredentialPresenceLookup,
} from "./ProviderReadinessPanel";
import SecretCredentialControl from "./SecretCredentialControl";
import {
  type SettingsAction,
  type SettingsState,
  setField,
  type TestKey,
} from "./settingsTypes";

interface GeminiSettingsProps {
  state: Pick<
    SettingsState,
    | "geminiAuthMode"
    | "geminiApiKey"
    | "geminiModel"
    | "geminiProjectId"
    | "geminiLocation"
    | "geminiServiceAccountPath"
    | "testingKey"
  >;
  dispatch: Dispatch<SettingsAction>;
  t: TFunction;
  handleTestGemini: () => Promise<void>;
  geminiCredentialAvailable: boolean;
  geminiSavedKeyPresent: boolean;
  geminiServiceAccountPathSavedPresent: boolean;
  geminiModelCatalog: ProviderModelCatalogItem[];
  providerDescriptor: ProviderDescriptor | null;
  providerReadiness: ProviderReadiness | null;
  credentialPresence: CredentialPresenceLookup;
  providerReadinessLoading: boolean;
  handleClearCredential: (
    key: string | string[],
    label: string,
    clearLocal: () => void,
  ) => Promise<void>;
  renderTestResult: (key: TestKey) => ReactNode;
}

export default function GeminiSettings({
  state,
  dispatch,
  t,
  handleTestGemini,
  geminiCredentialAvailable,
  geminiSavedKeyPresent,
  geminiServiceAccountPathSavedPresent,
  geminiModelCatalog,
  providerDescriptor,
  providerReadiness,
  credentialPresence,
  providerReadinessLoading,
  handleClearCredential,
  renderTestResult,
}: GeminiSettingsProps) {
  const {
    geminiAuthMode,
    geminiApiKey,
    geminiModel,
    geminiProjectId,
    geminiLocation,
    geminiServiceAccountPath,
    testingKey,
  } = state;
  const defaultModel = providerDescriptor?.default_model ?? "";

  return (
    <div className="settings-section">
      <h3 className="settings-section__title">
        {t("settings.sections.gemini")}
      </h3>
      <ProviderReadinessPanel
        entry={providerReadiness}
        descriptor={providerDescriptor}
        credentialPresence={credentialPresence}
        loading={providerReadinessLoading}
        t={t}
      />
      <div className="settings-radio-group">
        <label className="settings-radio">
          <input
            type="radio"
            name="gemini-auth"
            checked={geminiAuthMode === "api_key"}
            onChange={() => dispatch(setField("geminiAuthMode", "api_key"))}
          />
          <span>{t("settings.geminiAuth.apiKey")}</span>
        </label>
        <label className="settings-radio">
          <input
            type="radio"
            name="gemini-auth"
            checked={geminiAuthMode === "vertex_ai"}
            onChange={() => dispatch(setField("geminiAuthMode", "vertex_ai"))}
          />
          <span>{t("settings.geminiAuth.vertexAi")}</span>
        </label>
      </div>

      <div className="settings-section__api-fields">
        {geminiAuthMode === "api_key" && (
          <>
            <SecretCredentialControl
              id="gemini-api-key"
              label={t("settings.fields.geminiApiKey")}
              value={geminiApiKey}
              onChange={(value) => dispatch(setField("geminiApiKey", value))}
              placeholder="AIza..."
              saved={geminiSavedKeyPresent}
              t={t}
              savedHint={t("settings.hints.geminiSavedKey")}
              onClear={
                geminiSavedKeyPresent
                  ? () =>
                      handleClearCredential(
                        "gemini_api_key",
                        t("settings.fields.geminiApiKey"),
                        () => dispatch(setField("geminiApiKey", "")),
                      )
                  : undefined
              }
            />
            <div className="settings-field">
              <button
                type="button"
                className="settings-btn settings-btn--secondary"
                disabled={testingKey !== null || !geminiCredentialAvailable}
                onClick={handleTestGemini}
              >
                {testingKey === "gemini"
                  ? t("settings.buttons.testing")
                  : t("settings.buttons.testConnection")}
              </button>
              {renderTestResult("gemini")}
            </div>
          </>
        )}

        {geminiAuthMode === "vertex_ai" && (
          <>
            <div className="settings-field">
              <label
                className="settings-field__label"
                htmlFor="gemini-project-id"
              >
                {t("settings.fields.projectId")}
              </label>
              <input
                id="gemini-project-id"
                className="settings-input"
                type="text"
                value={geminiProjectId}
                onChange={(e) =>
                  dispatch(setField("geminiProjectId", e.target.value))
                }
                placeholder="my-gcp-project"
              />
            </div>
            <div className="settings-field">
              <label
                className="settings-field__label"
                htmlFor="gemini-location"
              >
                {t("settings.fields.location")}
              </label>
              <input
                id="gemini-location"
                className="settings-input"
                type="text"
                value={geminiLocation}
                onChange={(e) =>
                  dispatch(setField("geminiLocation", e.target.value))
                }
                placeholder="us-central1"
              />
            </div>
            <SecretCredentialControl
              id="gemini-service-account-path"
              label={t("settings.fields.serviceAccountPathOptional")}
              value={geminiServiceAccountPath}
              onChange={(value) =>
                dispatch(setField("geminiServiceAccountPath", value))
              }
              placeholder="/path/to/service-account.json"
              saved={geminiServiceAccountPathSavedPresent}
              t={t}
              savedHint={t("settings.hints.geminiSavedServiceAccountPath")}
              onClear={
                geminiServiceAccountPathSavedPresent
                  ? () =>
                      handleClearCredential(
                        "google_service_account_path",
                        t("settings.fields.serviceAccountPathOptional"),
                        () =>
                          dispatch(setField("geminiServiceAccountPath", "")),
                      )
                  : undefined
              }
            />
          </>
        )}

        <div className="settings-field">
          <label className="settings-field__label" htmlFor="gemini-model">
            {t("settings.fields.model")}
          </label>
          <ModelCatalogPicker
            id="gemini-model"
            value={geminiModel}
            onChange={(value) => dispatch(setField("geminiModel", value))}
            catalog={geminiModelCatalog}
            t={t}
            placeholder={defaultModel}
          />
        </div>
      </div>
    </div>
  );
}
