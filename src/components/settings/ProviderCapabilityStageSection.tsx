/**
 * Provider-capability cards for a SINGLE stage, co-located with the provider
 * panel that owns that stage (blueprint §1.2, Phase 4 STEP 2).
 *
 * The registry capability cards used to live in a dense "Provider capability
 * overview" block on the Overview tab. Blueprint §1.2 relocates them next to the
 * provider they describe, behind each provider panel's "Show advanced"
 * disclosure. This component renders the disclosure + the stage's cards; the
 * STT / LLM / Gemini / TTS panels each render one for their own stage.
 *
 * The region keeps its accessible name ("Provider capability overview") and the
 * per-card markup (`.settings-provider-capability-card`) so the capability-card
 * assertions are unchanged — only the navigation to reach them moves (the tests
 * now open the panel's advanced disclosure first).
 */

import type { ProviderStage } from "../../types";
import AdvancedSettingsDisclosure from "../AdvancedSettingsDisclosure";
import ProviderCapabilityCard from "./ProviderCapabilityCard";
import { useSettings } from "./SettingsContext";
import {
  PROVIDER_CAPABILITY_STAGES,
  providerCapabilityDescriptorsForStage,
} from "./useSettingsController";

export default function ProviderCapabilityStageSection({
  stage,
}: {
  stage: ProviderStage;
}) {
  const { t } = useSettings();
  const stageMeta = PROVIDER_CAPABILITY_STAGES.find(
    (entry) => entry.stage === stage,
  );
  if (!stageMeta) return null;

  const descriptors = providerCapabilityDescriptorsForStage(stage);
  if (descriptors.length === 0) return null;

  const regionTitleId = `settings-provider-capabilities-${stage}`;
  const stageTitleId = `settings-provider-capabilities-stage-${stage}`;

  return (
    <AdvancedSettingsDisclosure
      summary={t("settings.sections.providerCapabilities")}
    >
      <section
        className="settings-provider-capabilities"
        aria-labelledby={regionTitleId}
      >
        <div className="settings-provider-capabilities__header">
          <div>
            <h3
              id={regionTitleId}
              className="settings-provider-capabilities__title"
            >
              Provider capability overview
            </h3>
            <p className="settings-provider-capabilities__help">
              Registry-backed capability cards for this stage, including planned
              providers that are not selectable yet.
            </p>
          </div>
        </div>
        <div className="settings-provider-capabilities__stages">
          <section
            className="settings-provider-capability-stage"
            aria-labelledby={stageTitleId}
          >
            <div className="settings-provider-capability-stage__header">
              <div>
                <h4
                  id={stageTitleId}
                  className="settings-provider-capability-stage__title"
                >
                  {stageMeta.label} capabilities
                </h4>
                <p className="settings-provider-capability-stage__help">
                  {stageMeta.description}
                </p>
              </div>
            </div>
            <div className="settings-provider-capability-stage__grid">
              {descriptors.map((descriptor) => (
                <ProviderCapabilityCard
                  key={descriptor.id}
                  descriptor={descriptor}
                  stageLabel={stageMeta.label}
                />
              ))}
            </div>
          </section>
        </div>
      </section>
    </AdvancedSettingsDisclosure>
  );
}
