/**
 * Find-a-setting palette manifest (audio-graph-4850, settings T4b, synthesis
 * §T4b: "Manifest-backed (`ROUTE_INDEX` + 37 `settings.fields.*` labels + 36
 * registry descriptors)").
 *
 * Pure, derived data — NOT a hand-authored second route table. Every entry's
 * destination comes from a function `settingsRoutes.ts` already exports
 * (`providerRouteForProviderId`, `credentialRouteForProviderCredential`,
 * `credentialRouteForProviderSetupSelection`, `modelRouteForProviderId`) —
 * this module never invents a `{tab, fieldId}` pair on its own, so it cannot
 * drift from `ROUTE_INDEX` (the completeness test in
 * `settingsPaletteManifest.test.ts` asserts every entry with a `fieldId`
 * appears in `ROUTE_INDEX`, which T1's own drift tripwire already resolves
 * against the real DOM).
 *
 * Sweep, per one of the 36 `PROVIDER_DESCRIPTORS`:
 *   1. `providerRouteForProviderId(id)` -> a "configure" entry (kind label
 *      reuses `controlBar.configure`, "Configure" — no new i18n key).
 *   2. EVERY one of the descriptor's OWN `credential_keys` (not just the
 *      first), each tried against `credentialRouteForProviderCredential` ??
 *      `credentialRouteForProviderSetupSelection` -> one "credential" entry
 *      per UNIQUE `{tab, fieldId}` destination those keys resolve to (kind
 *      label = that key's own `settings.fields.*` label, reused, never a new
 *      key). Several keys resolving to the IDENTICAL field collapse to one
 *      entry (e.g. `llm.api` accepts 7 OpenAI-compatible key names into the
 *      same field); keys resolving to GENUINELY DIFFERENT fields (e.g.
 *      `realtime_agent.gemini_live`'s api-key field vs. its separate Vertex
 *      service-account-path field) each get their own entry. Deliberately
 *      does NOT stop after the first resolvable key — an early "one result
 *      per provider" cutoff (the T4b implementer's original shape) made
 *      coverage depend on `credential_keys`' array ORDER: whichever key
 *      happened to resolve first silently ate the rest, so a future
 *      registry reordering could delete an existing entry (e.g. Gemini
 *      Live's API-key entry) without failing any test. Per-destination
 *      dedupe instead makes the final entry set a fixed point independent
 *      of iteration order.
 *   3. `modelRouteForProviderId(id)` -> a "model" entry (kind label reuses
 *      `settings.fields.model`), skipped when it resolves to the SAME
 *      `{tab, fieldId}` an entry above already covered (`asr.local_whisper`
 *      delegates its model route to its own provider route, for example).
 *
 * A descriptor that resolves NONE of the above (a deferred/registry-only
 * provider with no dedicated settings sub-form, or a `diarization`-stage
 * descriptor configured only via the ASR provider's own diarization toggle)
 * is not silently dropped — it is recorded in `PALETTE_EXCLUDED_PROVIDER_IDS`
 * with a reason, and the completeness test asserts every registry id is
 * EITHER covered OR excluded-with-a-reason.
 *
 * Tab-jump entries (kind "tab") are the 8 `RAIL_SECTIONS` rows, unchanged —
 * reuses each tab's own existing `labelKey`, no qualifier, no `fieldId` (the
 * external `SettingsRoute.fieldId` is optional — landing on a bare tab is a
 * valid jump).
 *
 * KNOWN LIMITATION, disclosed rather than fixed here (T4b review,
 * audio-graph-4850): `openSettings(route)`'s external `SettingsRoute` shape
 * deliberately carries no `apply`/mutation member (see its doc comment in
 * `types/index.ts`) — every caller outside the settings controller may only
 * NAVIGATE, never silently flip a provider selection. Several
 * credential/model entries this sweep produces land on a field that is
 * itself gated behind a variant toggle the ROUTE'S OWN `applyAction` would
 * normally flip (e.g. the "Cerebras API Key" entry's target field only
 * renders when `llmType === "cerebras"`). Under the app's DEFAULT provider
 * selection, jumping to such an entry lands on the right TAB with no
 * visible field to focus. `ROUTE_INDEX` containment (asserted below) proves
 * the destination `{tab, fieldId}` pair is a real DOM node under SOME
 * app state, not that it is visible under the CURRENT one — that stronger
 * guarantee does not hold today and would require either widening
 * `SettingsRoute` to carry `applyAction` for these entries (a product
 * decision, not made here) or excluding them from the manifest.
 *
 * SEPARATE KNOWN GAP, also disclosed rather than fixed here: this manifest
 * only indexes {tab jumps} + {registry-descriptor-derived provider/
 * credential/model entries}. It does NOT index the remaining
 * `settings.fields.*` labels for fields that aren't tied to a specific
 * registry descriptor (e.g. `temperature`, `maxTokens`, `endpoint`,
 * `enableDiarization`, `whisperModelSize`, `captureSampleRate`, AWS
 * profile/region fields, the Deepgram/OpenRouter tuning fields, ...) —
 * searching for "temperature" or "diarization" today returns "No matching
 * settings" even though those ARE real, reachable Settings fields. Adding
 * them would mean hand-picking a `{tab, fieldId}` per field (rather than
 * deriving every entry from a registry sweep, this module's core
 * invariant) and DOM-verifying each one — a real scope addition, not a bug
 * fix, and out of scope for this review pass.
 */

import { PROVIDER_DESCRIPTORS } from "../providerRegistryHelpers";
import { RAIL_SECTIONS, type SettingsTab } from "./settingsRailConfig";
import {
  credentialRouteForProviderCredential,
  credentialRouteForProviderSetupSelection,
  modelRouteForProviderId,
  providerRouteForProviderId,
  type RouteEntry,
} from "./settingsRoutes";

export type PaletteEntryKind = "tab" | "provider" | "credential" | "model";

export interface PaletteEntry {
  /** Stable, human-inspectable id — never rendered. */
  id: string;
  kind: PaletteEntryKind;
  /** i18n key for the entry's own kind/field label — reused, never a
   * one-off literal (`controlBar.configure`, `settings.fields.model`,
   * `settings.fields.<credentialField>`, or a tab's own `labelKey`). */
  kindLabelKey: string;
  /** The live provider's display name (unlocalized everywhere else this
   * app renders one too — see `railEngineInfo.ts`). `null` for tab entries,
   * which need no qualifier — they are not ambiguous. */
  qualifier: string | null;
  tab: SettingsTab;
  fieldId?: string;
  activate?: boolean;
}

/** `settings.fields.*` label key for a credential key that has no
 * provider-specific field name of its own — the generic OpenAI-compatible
 * endpoints (`asr.api`/`llm.api`) plus Deepgram/AssemblyAI, none of which
 * has a dedicated `settings.fields.*API Key` key. */
const GENERIC_API_KEY_LABEL = "settings.fields.apiKey";

/** Credential key -> its own `settings.fields.*` label, where one exists;
 * falls back to {@link GENERIC_API_KEY_LABEL} otherwise. */
const CREDENTIAL_KEY_LABEL: Partial<Record<string, string>> = {
  cerebras_api_key: "settings.fields.cerebrasApiKey",
  sambanova_api_key: "settings.fields.sambanovaApiKey",
  openrouter_api_key: "settings.fields.openrouterApiKey",
  gemini_api_key: "settings.fields.geminiApiKey",
  aws_access_key: "settings.fields.accessKeyId",
  aws_secret_key: "settings.fields.secretAccessKey",
  aws_session_token: "settings.fields.sessionTokenOptional",
  google_service_account_path: "settings.fields.serviceAccountPathOptional",
};

/**
 * Registry ids the sweep below resolves NO entry for, with a stated reason
 * (synthesis §T4b: "every route reachable from the palette or explicitly
 * excluded with a reason"). Populated lazily by `buildPaletteManifest`
 * itself (not hand-maintained) so it can never drift from what the sweep
 * actually finds — but the REASON text is authored, since "why" is not
 * derivable from the registry alone.
 */
export type PaletteExclusionReason =
  | "deferred_not_selectable"
  | "diarization_internal"
  | "no_settings_subform";

function exclusionReasonFor(descriptor: {
  stage: string;
  ui_selectable: boolean;
}): PaletteExclusionReason {
  if (descriptor.stage === "diarization") return "diarization_internal";
  if (!descriptor.ui_selectable) return "deferred_not_selectable";
  return "no_settings_subform";
}

function routeKey(route: RouteEntry): string {
  return `${route.tab}:${route.fieldId}`;
}

interface SweepResult {
  entries: PaletteEntry[];
  excluded: ReadonlyMap<string, PaletteExclusionReason>;
}

function sweepProviderDescriptors(): SweepResult {
  const entries: PaletteEntry[] = [];
  const excluded = new Map<string, PaletteExclusionReason>();

  for (const descriptor of PROVIDER_DESCRIPTORS.values()) {
    const seenFieldKeys = new Set<string>();
    let coveredThisDescriptor = false;

    const providerRoute = providerRouteForProviderId(descriptor.id);
    if (providerRoute) {
      entries.push({
        id: `provider:${descriptor.id}`,
        kind: "provider",
        kindLabelKey: "controlBar.configure",
        qualifier: descriptor.display_name,
        tab: providerRoute.tab,
        fieldId: providerRoute.fieldId,
        activate: providerRoute.activate,
      });
      seenFieldKeys.add(routeKey(providerRoute));
      coveredThisDescriptor = true;
    }

    // Visits EVERY credential key (no early exit) so coverage is a fixed
    // point over the SET of distinct destination fields, independent of
    // `credential_keys`' array order (T4b review fix — see the module doc
    // comment). `tts.deepgram_aura` (no case in
    // `credentialRouteForProviderCredential` at all, only the
    // `credentialRouteForProviderSetupSelection` fallback resolves it) and
    // `realtime_agent.gemini_live` (a SECOND, distinct Vertex
    // service-account-path field alongside its api-key field) both fall out
    // of this generic loop now — no separate hand-authored special case
    // needed for either.
    for (const key of descriptor.credential_keys) {
      const route =
        credentialRouteForProviderCredential(descriptor.id, key) ??
        credentialRouteForProviderSetupSelection(descriptor.id, key);
      if (!route) continue;
      const dedupeKey = routeKey(route);
      if (seenFieldKeys.has(dedupeKey)) continue; // identical destination to an earlier key (or the provider route) — not a second entry
      seenFieldKeys.add(dedupeKey);
      entries.push({
        id: `credential:${descriptor.id}:${key}`,
        kind: "credential",
        kindLabelKey: CREDENTIAL_KEY_LABEL[key] ?? GENERIC_API_KEY_LABEL,
        qualifier: descriptor.display_name,
        tab: route.tab,
        fieldId: route.fieldId,
        activate: route.activate,
      });
      coveredThisDescriptor = true;
    }

    const modelRoute = modelRouteForProviderId(descriptor.id);
    if (modelRoute && !seenFieldKeys.has(routeKey(modelRoute))) {
      seenFieldKeys.add(routeKey(modelRoute));
      entries.push({
        id: `model:${descriptor.id}`,
        kind: "model",
        kindLabelKey: "settings.fields.model",
        qualifier: descriptor.display_name,
        tab: modelRoute.tab,
        fieldId: modelRoute.fieldId,
        activate: modelRoute.activate,
      });
      coveredThisDescriptor = true;
    }

    if (!coveredThisDescriptor) {
      excluded.set(descriptor.id, exclusionReasonFor(descriptor));
    }
  }

  return { entries, excluded };
}

function tabEntries(): PaletteEntry[] {
  return RAIL_SECTIONS.map((section) => ({
    id: `tab:${section.id}`,
    kind: "tab",
    kindLabelKey: section.labelKey,
    qualifier: null,
    tab: section.id,
  }));
}

const sweep = sweepProviderDescriptors();

/** Every searchable destination the palette can jump to. */
export const PALETTE_ENTRIES: readonly PaletteEntry[] = [
  ...tabEntries(),
  ...sweep.entries,
];

/** Registry ids the sweep resolved no entry for, with why — see the module
 * doc comment. Consumed only by the completeness test. */
export const PALETTE_EXCLUDED_PROVIDER_IDS: ReadonlyMap<
  string,
  PaletteExclusionReason
> = sweep.excluded;
