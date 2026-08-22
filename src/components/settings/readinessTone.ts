/**
 * The tone LAW for readiness chips (audio-graph-2554 — settings T2, "the tone
 * law", ratified e388 design panel, synthesis §T2).
 *
 * B's three-axes model — PLANNED (config/selection), PREREQUISITES
 * (credential/data-boundary claims), OBSERVED (a real backend probe) — draws
 * one hard line: `data-tone="success"` and the word "Ready" are an OBSERVED
 * claim. They may render ONLY when a status of `"ready"` came from a probe
 * that is both fresh (`!stale`) and about the provider actually in
 * active/selected use right now (Axis 3). Every other input — a saved
 * credential, a status nobody has checked yet, a provider this app cannot
 * auto-probe at all, a cached result — is Axis 1/2 at best and must never
 * borrow the OBSERVED tone or copy (ADR-0030 line 76: "credential presence
 * alone never means Ready"; ADR-0034: negative-egress/observed-vs-planned
 * discipline).
 *
 * This module is the ONE place that decides *when* the OBSERVED claim is
 * reachable. It is generic over the caller's own status enum (the raw
 * `ProviderReadinessStatus` for CredentialsPanel/ProviderCapabilityCard, the
 * mode-aggregate `ProviderSetupReadinessStatus` — which additionally has
 * "blocked" — for ProductModeSummaryCards) and does NOT reinvent each
 * caller's concrete status→tone palette: `forceNeutral` tells the caller the
 * LAW overrides whatever its own map would say (an unchecked/unprobeable
 * provider is never a warning); otherwise the caller's own
 * `readinessTone`/`modeReadinessTone`/`selectabilityTone` (badgeTone.ts)
 * still owns the concrete color for `effectiveStatus`. This module owns only
 * the gating law in front of it, so later surfaces (T3's chooser, T4a's rail
 * chips) get the same law by construction instead of re-deriving it.
 */

import type { BadgeTone } from "./badgeTone";

/** The "nothing has been (successfully, freshly, actively) observed" status
 * every demotion collapses to for copy purposes — reuses each caller's
 * existing "unchecked" key/label (e.g.
 * `settings.providerReadiness.status.unchecked`, "Unchecked"). No new i18n
 * key is introduced for a demoted chip. */
export type Unchecked = "unchecked";

export interface ReadinessAxisInput<S extends string> {
  /** Backend-reported status, or omitted/null when no probe has ever run at
   * all (only credential presence, if any, is known). */
  status?: S | Unchecked | null;
  /** True when the last successful check is stale relative to the current
   * credential/config epoch — a cached "ready" is not the same fact as a
   * fresh one. */
  stale?: boolean;
  /** False when this app cannot automatically probe the provider at all
   * (e.g. `asr.gladia`, Gemini Vertex). Undefined/true means automatic
   * probing is possible (even if it simply hasn't run yet). Only demotes a
   * reported `"ready"` (the backend sets this unconditionally whenever
   * required credentials are missing, so it is NOT independent evidence
   * against a `missing_credentials`/`error`/other caller-owned status —
   * see the demotion rule below). */
  automaticProbeAvailable?: boolean;
  /** Axis 3 gate: is this the provider actually in active/selected use right
   * now (i.e. is anything actually probing THIS provider)? */
  active: boolean;
}

export interface ReadinessAxisResult<S extends string> {
  /** `false` — the caller renders NO axis-3 chip at all for this provider
   * (a non-active provider's only-ever-checked-elsewhere "ready" is not a
   * claim worth making about a provider nobody is using). */
  render: boolean;
  /** The status to use for tone AND copy lookups. Demotions collapse to
   * `"unchecked"`; every other input passes through unchanged so the caller
   * keeps using its OWN status→tone/status→label map. */
  effectiveStatus: S | Unchecked;
  /** `true` — the LAW insists on a neutral tone regardless of what the
   * caller's own status→tone map says for `effectiveStatus` (every caller's
   * map treats "unchecked" as a warning; the law never does — presence or
   * absence of a check is not evidence of a problem). */
  forceNeutral: boolean;
}

export function readinessAxisTone<S extends string>({
  status,
  stale = false,
  automaticProbeAvailable,
  active,
}: ReadinessAxisInput<S>): ReadinessAxisResult<S> {
  const reported: S | Unchecked = status ?? "unchecked";

  // Rule (Axis 3 / non-active provider): "ready" is a claim about the
  // provider actually in active use. A provider nobody is actively probing
  // never earns it, no matter what its last (or only) readiness entry says —
  // render NO axis-3 chip at all rather than a misleading claim about a
  // provider that isn't the one running.
  if (reported === "ready" && !active) {
    return { render: false, effectiveStatus: "unchecked", forceNeutral: true };
  }

  // Rule (stale demotes tone, + automatic_probe_available:false demotes a
  // "ready" the same way): a cached "ready" — or a "ready" this app can no
  // longer automatically re-verify — collapses to the same effective status
  // as "never checked", for BOTH tone and copy. Collapsing here (rather than
  // leaving `effectiveStatus` at "ready" and only forcing the tone below)
  // is what stops the word "Ready" itself from leaking through at a neutral
  // tone — the law gates the claim, not just its color.
  //
  // `automatic_probe_available` is deliberately scoped to the "ready" arm
  // only, mirroring `providerRecoveryAction` (ProviderReadinessPanel.tsx),
  // which likewise only consults it inside its `case "unchecked":` branch.
  // The backend sets `automatic_probe_available: false` unconditionally
  // whenever required credentials are missing
  // (`automatic_probe_available_from_decision`, commands.rs — it short-
  // circuits on `missing.is_empty()`), so `missing_credentials` (and any
  // other caller-owned status this generic law doesn't recognize, e.g.
  // "error"/"blocked") ALWAYS reports `automatic_probe_available: false` in
  // production. Those are real, structural facts — a missing key, a real
  // failure — and must keep their own tone/copy regardless; only a "ready"
  // claim this app cannot itself re-verify is untrustworthy enough to
  // demote to neutral "unchecked".
  const effectiveStatus: S | Unchecked =
    reported === "ready" && (stale || automaticProbeAvailable === false)
      ? "unchecked"
      : reported;

  // Rule (ADR-0030 line 76): nothing has been OBSERVED — reads as neutral,
  // never a fabricated warning. This is now the ONLY forceNeutral rule:
  // every input that should demote (no probe has ever run, a stale ready, an
  // unverifiable ready) already collapsed to "unchecked" above.
  const forceNeutral = effectiveStatus === "unchecked";

  return { render: true, effectiveStatus, forceNeutral };
}

export interface ReadinessChipResult<S extends string> {
  /** Same meaning as {@link ReadinessAxisResult.render}. */
  render: boolean;
  /** Same meaning as {@link ReadinessAxisResult.effectiveStatus}. */
  effectiveStatus: S | Unchecked;
  /** The tone to render — `forceNeutral` already folded in, so callers never
   * hand-apply the `forceNeutral ? "neutral" : statusToneMap(effectiveStatus)`
   * branch themselves. */
  tone: BadgeTone;
}

/**
 * The thin wrapper the law's own doc comment (above) promises: every call
 * site that used to compute `readinessAxisTone(...)` and then hand-apply
 * `axis.forceNeutral ? "neutral" : someStatusToneMap(axis.effectiveStatus)`
 * now makes ONE call instead (settings T3, audio-graph-9d2b — folds in seed
 * 73bf). The render/forceNeutral contract is enforced at this one seam;
 * `statusToneMap` still owns each caller's own concrete status→tone palette
 * (`readinessTone`/`modeReadinessTone` from `badgeTone.ts`) for every
 * effective status the law does NOT force-neutral.
 */
export function readinessChipTone<S extends string>(
  input: ReadinessAxisInput<S>,
  statusToneMap: (status: S | Unchecked) => BadgeTone,
): ReadinessChipResult<S> {
  const axis = readinessAxisTone(input);
  return {
    render: axis.render,
    effectiveStatus: axis.effectiveStatus,
    tone: axis.forceNeutral ? "neutral" : statusToneMap(axis.effectiveStatus),
  };
}
