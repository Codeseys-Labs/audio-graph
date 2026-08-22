/**
 * Rail configuration — the single source of truth for the Settings left rail
 * (blueprint §1.1, Phase 4 STEP 4).
 *
 * The rail items, their grouping (Conversation pipeline / App), the group ordering,
 * and the `SettingsTab` union all live here so the controller, the route `tab`
 * union, and the presentational `settingsRail` component reference one
 * definition instead of duplicating it. Order follows the user's mental model
 * of the pipeline (Modes → STT → LLM → TTS → realtime agent → credentials) with
 * low-risk prefs, then diagnostics last.
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
export type RailGroup = "providers" | "app";

export interface RailSection {
  id: SettingsTab;
  labelKey: string;
  /**
   * T4a dual-label rail (audio-graph-4850, synthesis §T4a): the goal-vocabulary
   * line rendered ABOVE `labelKey`'s existing engine-vocabulary line inside
   * the tab button. `labelKey` itself is UNCHANGED text — it is what every
   * `goToTab`/reserved-token assertion still matches — so `goalLabelKey` is
   * purely additive chrome, hidden below the 720px breakpoint (known
   * compromise, ratified R2; see the media query in `settings.css`).
   */
  goalLabelKey: string;
  group: RailGroup;
}

/**
 * The rail items in display order. Grouped under Conversation pipeline / App; the
 * provider cluster leads with Modes (the interactive mode selector) and sits
 * together so the pipeline configures as one unit, and diagnostics (logging)
 * stays last. The former "setup" group is folded into "providers" so Modes is
 * the first item of the provider cluster (ADR-0006 B1).
 */
export const RAIL_SECTIONS: RailSection[] = [
  {
    id: "overview",
    labelKey: "settings.tabs.overview",
    goalLabelKey: "settings.tabs.goal.overview",
    group: "providers",
  },
  {
    id: "stt",
    labelKey: "settings.tabs.stt",
    goalLabelKey: "settings.tabs.goal.stt",
    group: "providers",
  },
  {
    id: "llm",
    labelKey: "settings.tabs.llm",
    goalLabelKey: "settings.tabs.goal.llm",
    group: "providers",
  },
  {
    id: "tts",
    labelKey: "settings.tabs.tts",
    goalLabelKey: "settings.tabs.goal.tts",
    group: "providers",
  },
  {
    id: "gemini",
    labelKey: "settings.tabs.gemini",
    goalLabelKey: "settings.tabs.goal.gemini",
    group: "providers",
  },
  {
    id: "credentials",
    labelKey: "settings.tabs.credentials",
    goalLabelKey: "settings.tabs.goal.credentials",
    group: "providers",
  },
  {
    id: "general",
    labelKey: "settings.tabs.general",
    goalLabelKey: "settings.tabs.goal.general",
    group: "app",
  },
  {
    id: "logging",
    labelKey: "settings.tabs.logging",
    goalLabelKey: "settings.tabs.goal.logging",
    group: "app",
  },
];

/** i18n label key per group header. */
export const RAIL_GROUP_LABEL_KEYS: Record<RailGroup, string> = {
  providers: "settings.railGroups.providers",
  app: "settings.railGroups.app",
};

/** Group render order, top to bottom. */
export const RAIL_GROUP_ORDER: RailGroup[] = ["providers", "app"];
