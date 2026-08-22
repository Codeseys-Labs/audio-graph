import { describe, expect, it } from "vitest";
import { PROVIDER_DESCRIPTORS } from "../providerRegistryHelpers";
import {
  PALETTE_ENTRIES,
  PALETTE_EXCLUDED_PROVIDER_IDS,
  type PaletteExclusionReason,
} from "./settingsPaletteManifest";
import { RAIL_SECTIONS } from "./settingsRailConfig";
import { ROUTE_INDEX } from "./settingsRoutes";

const ROUTE_INDEX_KEYS = new Set(
  ROUTE_INDEX.map((entry) => `${entry.tab}:${entry.fieldId}`),
);

const VALID_EXCLUSION_REASONS: ReadonlySet<PaletteExclusionReason> = new Set([
  "deferred_not_selectable",
  "diarization_internal",
  "no_settings_subform",
]);

/** The registry id an entry's own id embeds (`kind:id[:credentialKey]`) —
 * `credential:`/`model:`/`provider:` ids may themselves contain `.`/`:`? no,
 * registry ids never contain a colon, so splitting on the FIRST colon is
 * exact for every non-tab entry. */
function coveredProviderIdFor(entryId: string): string | null {
  const [kind, ...rest] = entryId.split(":");
  if (kind === "tab") return null;
  // `credential:<providerId>:<credentialKey>` vs `provider:<providerId>` /
  // `model:<providerId>` — the providerId itself may contain a `.` (e.g.
  // "asr.deepgram") but never a `:`, so the first remaining segment is it.
  return rest[0] ?? null;
}

describe("settings palette manifest (T4b, audio-graph-4850)", () => {
  it("manifest completeness: every registry provider id is covered by >=1 entry OR explicitly excluded with a reason", () => {
    const coveredIds = new Set(
      PALETTE_ENTRIES.map((entry) => coveredProviderIdFor(entry.id)).filter(
        (id): id is string => id !== null,
      ),
    );

    const unaccountedFor: string[] = [];
    for (const id of PROVIDER_DESCRIPTORS.keys()) {
      const covered = coveredIds.has(id);
      const excludedReason = PALETTE_EXCLUDED_PROVIDER_IDS.get(id);
      if (!covered && excludedReason === undefined) {
        unaccountedFor.push(id);
      }
      // Mutually exclusive: an id is never BOTH covered and excluded.
      if (covered && excludedReason !== undefined) {
        unaccountedFor.push(`${id} (covered AND excluded — contradictory)`);
      }
    }
    expect(unaccountedFor).toEqual([]);

    // Every excluded id must carry one of the closed, real reasons — never
    // an empty string or a typo that would silently read as "excluded".
    for (const [id, reason] of PALETTE_EXCLUDED_PROVIDER_IDS) {
      expect(VALID_EXCLUSION_REASONS.has(reason), `${id} -> "${reason}"`).toBe(
        true,
      );
    }
  });

  it("every entry with a fieldId points at a {tab, fieldId} pair that ROUTE_INDEX (T1's own DOM-verified drift tripwire) already covers", () => {
    const drifted = PALETTE_ENTRIES.filter(
      (entry) =>
        entry.fieldId !== undefined &&
        !ROUTE_INDEX_KEYS.has(`${entry.tab}:${entry.fieldId}`),
    );
    expect(drifted).toEqual([]);
  });

  // T4b review fix (audio-graph-4850): the reverse direction of the assertion
  // above. Both the "manifest completeness" test (registry ids covered OR
  // excluded) AND `PALETTE_EXCLUDED_PROVIDER_IDS` itself are DERIVED from the
  // same `sweepProviderDescriptors()` sweep — so a sweep regression that
  // silently drops entries (mutation probe M7: forcing
  // `providerRouteForProviderId` to return `null` inside the sweep) converts
  // coverage loss into structurally valid exclusions and passes every
  // existing test. `ROUTE_INDEX` is independent of the sweep (built directly
  // from `settingsRoutes.ts`'s own hardcoded id lists), so checking against
  // it catches exactly this class of regression: real routes the DOM exposes
  // that the palette can no longer reach at all.
  it("every ROUTE_INDEX {tab, fieldId} pair (T1's independent route enumeration) is reachable from >=1 palette entry", () => {
    const reachable = new Set(
      PALETTE_ENTRIES.filter((entry) => entry.fieldId !== undefined).map(
        (entry) => `${entry.tab}:${entry.fieldId}`,
      ),
    );
    const unreachable = ROUTE_INDEX.filter(
      (route) => !reachable.has(`${route.tab}:${route.fieldId}`),
    );
    expect(unreachable).toEqual([]);
  });

  // Pins the exact entry-count breakdown so a sweep regression that drops (or
  // duplicates) a whole KIND of entry fails immediately and loudly, instead
  // of only showing up as a smaller, easier-to-miss diff in the two tests
  // above. Update these numbers deliberately (with a comment explaining why)
  // if the registry or the sweep's own coverage rules genuinely change.
  it("pins the manifest's entry-count breakdown by kind (8 tab / 17 provider / 13 credential / 5 model = 43 total)", () => {
    const byKind: Record<string, number> = {};
    for (const entry of PALETTE_ENTRIES) {
      byKind[entry.kind] = (byKind[entry.kind] ?? 0) + 1;
    }
    expect(byKind).toEqual({
      tab: 8,
      provider: 17,
      credential: 13,
      model: 5,
    });
    expect(PALETTE_ENTRIES.length).toBe(43);
    expect(PALETTE_EXCLUDED_PROVIDER_IDS.size).toBe(18);
  });

  it("tab entries are exactly the 8 rail sections, unchanged", () => {
    const tabEntryIds = PALETTE_ENTRIES.filter((e) => e.kind === "tab").map(
      (e) => e.tab,
    );
    expect(tabEntryIds.sort()).toEqual(RAIL_SECTIONS.map((s) => s.id).sort());
  });

  it("mandatory provider qualifier: every non-tab entry names which provider it belongs to (synthesis §T4b — a shared field label like 'Model' or 'API Key' must say whose)", () => {
    const unqualified = PALETTE_ENTRIES.filter(
      (entry) => entry.kind !== "tab" && !entry.qualifier,
    );
    expect(unqualified).toEqual([]);
  });

  it("has no duplicate entry ids", () => {
    const ids = PALETTE_ENTRIES.map((e) => e.id);
    expect(new Set(ids).size).toBe(ids.length);
  });
});
