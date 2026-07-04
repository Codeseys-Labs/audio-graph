/**
 * Language Model rail section (blueprint §5). Thin wrapper over the
 * already-extracted `<LlmProviderSettings>`; reads its props from the settings
 * controller via `useSettings()` (Phase 2) instead of prop-drilling.
 */

import LlmProviderSettings from "../LlmProviderSettings";
import ProviderCapabilityStageSection from "./ProviderCapabilityStageSection";
import { useSettings } from "./SettingsContext";
import { LLM_PROVIDER_OPTIONS } from "./useSettingsController";

export default function LlmPanel() {
  const {
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
    handleTestSambanova,
    handleRefreshSambanovaModels,
    handleRefreshModels,
    llmEndpointSavedKeyPresent,
    llmApiKey,
    llmType,
    awsSavedKeysPresent,
    awsBedrockAccessKey,
    awsBedrockSecretKey,
    awsBedrockSessionToken,
    awsSessionTokenSavedPresent,
    awsBedrockAccessKeysAvailable,
    openrouterCredentialAvailable,
    openrouterSavedKeyPresent,
    openrouterApiKey,
    cerebrasCredentialAvailable,
    cerebrasSavedKeyPresent,
    sambanovaCredentialAvailable,
    sambanovaSavedKeyPresent,
    openrouterModelsError,
    cerebrasModelsLoading,
    cerebrasModelsError,
    sambanovaModelsLoading,
    sambanovaModelsError,
    llmApiCredentialAvailable,
    llmApiModelsLoading,
    llmApiModelsError,
    cerebrasTesting,
    cerebrasTestResult,
    sambanovaTesting,
    sambanovaTestResult,
    llmApiModelCatalog,
    cerebrasModelCatalog,
    sambanovaModelCatalog,
    mistralrsModelCatalog,
    activeLlmProviderDescriptor,
    activeLlmProviderReadiness,
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
  } = useSettings();
  return (
    <>
      <LlmProviderSettings
        state={state}
        dispatch={dispatch}
        t={t}
        modelStatus={modelStatus}
        refreshAwsProfiles={refreshAwsProfiles}
        handleTestAwsBedrock={handleTestAwsBedrock}
        handleTestOpenRouter={handleTestOpenRouter}
        handleRefreshOpenRouterModels={handleRefreshOpenRouterModels}
        handleTestCerebras={handleTestCerebras}
        handleRefreshCerebrasModels={handleRefreshCerebrasModels}
        handleTestSambanova={handleTestSambanova}
        handleRefreshSambanovaModels={handleRefreshSambanovaModels}
        handleRefreshModels={handleRefreshModels}
        llmEndpointSavedKeyPresent={
          llmEndpointSavedKeyPresent && !llmApiKey.trim()
        }
        awsSavedKeysPresent={
          awsSavedKeysPresent &&
          !awsBedrockAccessKey.trim() &&
          !awsBedrockSecretKey.trim() &&
          !awsBedrockSessionToken.trim()
        }
        awsSessionTokenSavedPresent={awsSessionTokenSavedPresent}
        awsAccessKeysAvailable={awsBedrockAccessKeysAvailable}
        openrouterCredentialAvailable={openrouterCredentialAvailable}
        openrouterSavedKeyPresent={
          openrouterSavedKeyPresent && !openrouterApiKey.trim()
        }
        cerebrasCredentialAvailable={cerebrasCredentialAvailable}
        cerebrasSavedKeyPresent={
          cerebrasSavedKeyPresent &&
          !(llmType === "cerebras" && llmApiKey.trim().length > 0)
        }
        sambanovaCredentialAvailable={sambanovaCredentialAvailable}
        sambanovaSavedKeyPresent={
          sambanovaSavedKeyPresent &&
          !(llmType === "sambanova" && llmApiKey.trim().length > 0)
        }
        openrouterModelsError={openrouterModelsError}
        cerebrasModelsLoading={cerebrasModelsLoading}
        cerebrasModelsError={cerebrasModelsError}
        sambanovaModelsLoading={sambanovaModelsLoading}
        sambanovaModelsError={sambanovaModelsError}
        llmApiCredentialAvailable={llmApiCredentialAvailable}
        llmApiModelsLoading={llmApiModelsLoading}
        llmApiModelsError={llmApiModelsError}
        cerebrasTesting={cerebrasTesting}
        cerebrasTestResult={cerebrasTestResult}
        sambanovaTesting={sambanovaTesting}
        sambanovaTestResult={sambanovaTestResult}
        providerOptions={LLM_PROVIDER_OPTIONS}
        llmApiModelCatalog={llmApiModelCatalog}
        cerebrasModelCatalog={cerebrasModelCatalog}
        sambanovaModelCatalog={sambanovaModelCatalog}
        mistralrsModelCatalog={mistralrsModelCatalog}
        activeProviderDescriptor={activeLlmProviderDescriptor}
        activeProviderReadiness={activeLlmProviderReadiness}
        credentialPresence={credentialPresence}
        providerReadinessLoading={providerReadinessLoading}
        handleClearCredential={handleClearCredential}
        renderTestResult={renderTestResult}
        openrouterAcceleratorEndpoints={openrouterAcceleratorEndpoints}
        openrouterAcceleratorProviders={openrouterAcceleratorProviders}
        openrouterAcceleratorLoading={openrouterAcceleratorLoading}
        openrouterAcceleratorError={openrouterAcceleratorError}
        openrouterAcceleratorPreset={openrouterAcceleratorPreset}
        openrouterAppliedAcceleratorPreset={openrouterAppliedAcceleratorPreset}
        setOpenrouterAcceleratorPreset={setOpenrouterAcceleratorPreset}
        handleDiscoverOpenRouterAccelerators={
          handleDiscoverOpenRouterAccelerators
        }
        handleApplyAcceleratorPreset={handleApplyAcceleratorPreset}
      />
      <ProviderCapabilityStageSection stage="llm" />
    </>
  );
}
