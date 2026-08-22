/**
 * Settings rail — the vertical left-rail tablist (blueprint §1.1/§2, Phase 4
 * STEP 4; dual-label rows added by T4a, audio-graph-4850, synthesis §T4a).
 *
 * Thin presentational component extracted from the shell: it renders the
 * grouped `role="tablist"` from `settingsRailConfig` (the single source of
 * truth) and wires roving tabindex + the doubled-arrow keyboard handler from
 * the controller. Tablist semantics are unchanged — vertical orientation that
 * flips to horizontal below the narrow breakpoint, `aria-selected`, the active
 * filled state, and `aria-controls`/`aria-labelledby` linking each tab to its
 * panel. Reads everything from `useSettings()`; no props.
 *
 * Each tab button renders TWO lines: a new goal-vocabulary line
 * (`tab.goalLabelKey`, hidden below 720px via `settings.css` — the ratified
 * known compromise, R2) above the UNCHANGED engine-vocabulary line
 * (`tab.labelKey` — the exact text every `goToTab`/reserved-token test
 * already matches). Provider-bearing tabs (stt/llm/tts/gemini) append the
 * live provider's name + a T2-derived readiness chip to the engine line via
 * `railEngineInfo` (`useSettingsController`/`railEngineInfo.ts`); the other
 * four tabs have no single "live provider" and render the engine line alone.
 */

import type { RailEngineInfo } from "./railEngineInfo";
import { useSettings } from "./SettingsContext";
import {
  RAIL_GROUP_LABEL_KEYS,
  RAIL_GROUP_ORDER,
  RAIL_SECTIONS,
  type SettingsTab,
} from "./settingsRailConfig";

/** Only these four tabs have a single "live provider" to name on the engine
 * line — the other four (overview/general/credentials/logging) render the
 * engine line alone (synthesis §T4a: engine line = "live provider name + one
 * T2-derived chip", which has no referent for a non-provider tab). */
const RAIL_ENGINE_TABS = new Set<SettingsTab>(["stt", "llm", "tts", "gemini"]);

export default function SettingsRail() {
  const {
    t,
    activeTab,
    setActiveTab,
    handleSettingsTabKeyDown,
    railHorizontal,
    railEngineInfo,
    tabRefs,
    tabButtonId,
    tabPanelId,
  } = useSettings();

  return (
    <div
      className="settings-tabs"
      role="tablist"
      aria-label={t("settings.title")}
      aria-orientation={railHorizontal ? "horizontal" : "vertical"}
    >
      {RAIL_GROUP_ORDER.map((group) => {
        const groupTabs = RAIL_SECTIONS.filter((tab) => tab.group === group);
        if (groupTabs.length === 0) return null;
        return (
          <div key={group} className="settings-rail-group">
            <p className="settings-rail-group__label" role="presentation">
              {t(RAIL_GROUP_LABEL_KEYS[group])}
            </p>
            {groupTabs.map((tab) => {
              const engine: RailEngineInfo | undefined = RAIL_ENGINE_TABS.has(
                tab.id,
              )
                ? railEngineInfo?.[tab.id as keyof typeof railEngineInfo]
                : undefined;
              return (
                <button
                  key={tab.id}
                  id={tabButtonId(tab.id)}
                  ref={(node) => {
                    tabRefs.current[tab.id] = node;
                  }}
                  type="button"
                  role="tab"
                  aria-selected={activeTab === tab.id}
                  aria-controls={tabPanelId(tab.id)}
                  tabIndex={activeTab === tab.id ? 0 : -1}
                  className={`settings-tab ${activeTab === tab.id ? "settings-tab--active" : ""}`}
                  onClick={() => setActiveTab(tab.id)}
                  onKeyDown={(e) => handleSettingsTabKeyDown(e, tab.id)}
                >
                  <span className="settings-tab__goal">
                    {t(tab.goalLabelKey)}
                  </span>
                  <span className="settings-tab__engine">
                    {t(tab.labelKey)}
                    {engine?.providerLabel && (
                      <span className="settings-tab__provider">
                        {engine.providerLabel}
                      </span>
                    )}
                    {engine?.chipRender && (
                      <span className="ag-chip" data-tone={engine.chipTone}>
                        {t(
                          `settings.providerReadiness.status.${engine.chipEffectiveStatus}`,
                        )}
                      </span>
                    )}
                  </span>
                </button>
              );
            })}
          </div>
        );
      })}
    </div>
  );
}
