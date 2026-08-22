/**
 * Rail engine-line derivation (audio-graph-4850, settings T4a, synthesis
 * §T4a: "Two-line rail tabs: goal line + engine line (live provider name +
 * one T2-derived chip)").
 *
 * Pure function: given a provider's descriptor + its readiness entry + a
 * caller-computed `active` flag, decides what the rail's second line shows —
 * the provider's own display name (never localized anywhere else in this
 * app either, `PROVIDER_READINESS_LABELS`/`CredentialsPanel.tsx`; kept
 * consistent rather than introducing the first localized provider name) plus
 * whether a T2 readiness chip renders and its tone/effective status. This
 * module never decides the `active` flag itself — the controller passes the
 * SAME `activeReadinessProviderIdSet` membership check every other T2/T3
 * consumer uses (ADR-0030): a reported `"ready"` about a provider nobody is
 * actively using never earns the chip (renders none at all — that specific
 * claim would be about the wrong provider). That is the ONLY axis-3
 * `render: false` case `readinessTone.ts`'s law defines; a non-active
 * provider with any OTHER status (never probed, missing credentials, error)
 * still renders its normal (forced-neutral-when-unchecked) chip, same as
 * every other T2/T3 consumer of the law — this module does not special-case
 * "non-active" into "no chip at all" beyond what the shared law already
 * does.
 */

import type { ProviderDescriptor, ProviderReadiness } from "../../types";
import type { BadgeTone } from "./badgeTone";
import { readinessTone as statusTone } from "./badgeTone";
import { readinessChipTone } from "./readinessTone";

export interface RailEngineInfo {
  /** The live provider's display name, or `null` when no descriptor is
   * resolvable (defensive — every rail-eligible tab always has one in
   * practice). */
  providerLabel: string | null;
  /** Same contract as `ReadinessChipResult.render` — `false` means render no
   * chip at all for this row. */
  chipRender: boolean;
  chipTone: BadgeTone;
  /** `settings.providerReadiness.status.<effectiveStatus>` is the copy key —
   * reused, never a new i18n key. */
  chipEffectiveStatus: string;
}

export function railEngineInfoForProvider(
  descriptor: ProviderDescriptor | null,
  readiness: ProviderReadiness | null,
  active: boolean,
): RailEngineInfo {
  const chip = readinessChipTone(
    {
      status: readiness?.status,
      stale: readiness?.stale,
      automaticProbeAvailable: readiness?.automatic_probe_available,
      active,
    },
    statusTone,
  );
  return {
    providerLabel: descriptor?.display_name ?? null,
    chipRender: chip.render,
    chipTone: chip.tone,
    chipEffectiveStatus: chip.effectiveStatus,
  };
}
