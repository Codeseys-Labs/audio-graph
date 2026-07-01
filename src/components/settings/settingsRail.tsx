/**
 * Settings rail — the vertical left-rail tablist (blueprint §1.1/§2, Phase 4
 * STEP 4).
 *
 * Thin presentational component extracted from the shell: it renders the
 * grouped `role="tablist"` from `settingsRailConfig` (the single source of
 * truth) and wires roving tabindex + the doubled-arrow keyboard handler from
 * the controller. Tablist semantics are unchanged — vertical orientation that
 * flips to horizontal below the narrow breakpoint, `aria-selected`, the active
 * filled state, and `aria-controls`/`aria-labelledby` linking each tab to its
 * panel. Reads everything from `useSettings()`; no props.
 */

import { useSettings } from "./SettingsContext";
import {
  RAIL_GROUP_LABEL_KEYS,
  RAIL_GROUP_ORDER,
  RAIL_SECTIONS,
} from "./settingsRailConfig";

export default function SettingsRail() {
  const {
    t,
    activeTab,
    setActiveTab,
    handleSettingsTabKeyDown,
    railHorizontal,
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
            {groupTabs.map((tab) => (
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
                {t(tab.labelKey)}
              </button>
            ))}
          </div>
        );
      })}
    </div>
  );
}
