import { describe, expect, it } from "vitest";

import pt from "./locales/pt.json";

// pt-first chip length budget (settings T3, audio-graph-9d2b, synthesis §T3):
// `.ag-chip` is `nowrap` and pt-BR strings routinely run longer than their en
// source ("Sem verificação automática" is 28 chars — design-b §W4). Every new
// chip key this ticket adds MUST fit inside an 18-character budget in pt so it
// never wraps/clips inside the fixed-width chip. Iterates the actual pt.json
// values (not a hand-copied literal) so a future edit to any of these keys is
// re-checked automatically.
//
// `providerReadiness.dataBoundary.*` and `providerReadiness.status.*` are
// REUSED keys (they pre-date this ticket, rendered elsewhere in a plain
// `<dd>`) that `ProviderChooserRow.tsx` newly renders inside a `.ag-chip` —
// the first chip context either family has ever had. Covered here too (fix
// pass, reviewer finding) so a future pt edit to a reused key can't silently
// regress a chip that's been nowrap-safe only by accident.
const CHIP_KEYS = [
  "settings.providerChooser.credentialSaved",
  "settings.providerChooser.credentialNeeded",
  "settings.providerChooser.credentialNone",
  "settings.providerReadiness.dataBoundary.local_only",
  "settings.providerReadiness.dataBoundary.user_configured_endpoint",
  "settings.providerReadiness.dataBoundary.user_configured_region",
  "settings.providerReadiness.dataBoundary.provider_account_boundary",
  "settings.providerReadiness.dataBoundary.vendor_cloud",
  "settings.providerReadiness.status.ready",
  "settings.providerReadiness.status.missing_credentials",
  "settings.providerReadiness.status.unchecked",
  "settings.providerReadiness.status.error",
] as const;

const CHIP_LENGTH_BUDGET = 18;

function readKeyPath(root: unknown, path: string): unknown {
  return path
    .split(".")
    .reduce<unknown>(
      (node, segment) =>
        typeof node === "object" && node !== null
          ? (node as Record<string, unknown>)[segment]
          : undefined,
      root,
    );
}

describe("settings chooser chip pt length budget", () => {
  for (const key of CHIP_KEYS) {
    it(`pt.json "${key}" fits the ${CHIP_LENGTH_BUDGET}-char chip budget`, () => {
      const value = readKeyPath(pt, key);
      expect(typeof value).toBe("string");
      expect((value as string).length).toBeLessThanOrEqual(CHIP_LENGTH_BUDGET);
    });
  }
});
