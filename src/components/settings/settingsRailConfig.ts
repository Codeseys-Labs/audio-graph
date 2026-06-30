/**
 * Rail configuration — the single source of truth for the Settings left rail
 * (blueprint §1.1, Phase 4 STEP 4).
 *
 * The rail items, their grouping (Setup / Providers / App), the group ordering,
 * and the `SettingsTab` union all live here so the controller, the route `tab`
 * union, and the presentational `settingsRail` component reference one
 * definition instead of duplicating it. Order follows the user's mental model
 * of the pipeline (Overview → STT → LLM → Gemini → TTS) with low-risk prefs,
 * credentials, then diagnostics last.
 */

/** Every Settings rail item / deep-link `tab` target. */
export type SettingsTab =
  | "overview"
  | "general"
  | "stt"
  | "llm"
  | "gemini"
  | "tts"
  | "credentials"
  | "logging";

/** Two-level rail grouping (Discord/Linear pattern) — blueprint §1.1. */
export type RailGroup = "setup" | "providers" | "app";

export interface RailSection {
  id: SettingsTab;
  labelKey: string;
  group: RailGroup;
}

/**
 * The rail items in display order. Grouped under Setup / Providers / App; the
 * provider cluster sits together so the pipeline configures as one unit, and
 * diagnostics (logging) stays last.
 */
export const RAIL_SECTIONS: RailSection[] = [
  { id: "overview", labelKey: "settings.tabs.overview", group: "setup" },
  { id: "stt", labelKey: "settings.tabs.stt", group: "providers" },
  { id: "llm", labelKey: "settings.tabs.llm", group: "providers" },
  { id: "gemini", labelKey: "settings.tabs.gemini", group: "providers" },
  { id: "tts", labelKey: "settings.tabs.tts", group: "providers" },
  { id: "general", labelKey: "settings.tabs.general", group: "app" },
  { id: "credentials", labelKey: "settings.tabs.credentials", group: "app" },
  { id: "logging", labelKey: "settings.tabs.logging", group: "app" },
];

/** i18n label key per group header. */
export const RAIL_GROUP_LABEL_KEYS: Record<RailGroup, string> = {
  setup: "settings.railGroups.setup",
  providers: "settings.railGroups.providers",
  app: "settings.railGroups.app",
};

/** Group render order, top to bottom. */
export const RAIL_GROUP_ORDER: RailGroup[] = ["setup", "providers", "app"];
