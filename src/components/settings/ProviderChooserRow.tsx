/**
 * Annotated provider-chooser row (settings T3, audio-graph-9d2b).
 *
 * Shared between `AsrProviderSettings` and `LlmProviderSettings` — the two
 * remaining provider radiogroups (synthesis §T3 item 2). Each row is a
 * `.settings-radio-row` wrapping the EXISTING `<label className=
 * "settings-radio">` radio control (passed in verbatim as `children` —
 * this component never touches its contents, so the 7 anchored
 * `getByRole("radio", {name: /.../i})` accessible-name assertions the
 * tripwire warns about stay untouched) plus a SIBLING annotation node
 * linked by `aria-describedby` (never inside the `<label>`).
 *
 * Cap: ≤2 chips + 1 requirement line. Axis 1 (PLANNED — data boundary) always
 * renders. Axis 2 (PREREQUISITES — credential presence) renders for
 * non-active rows; the active row swaps it for Axis 3 (OBSERVED — the T2
 * tone-law-gated readiness chip) instead, via `readinessChipTone` so this
 * row can never disagree with the `ProviderReadinessPanel` above the
 * radiogroup for the same provider.
 */

import type { TFunction } from "i18next";
import type { ReactNode } from "react";
import type { ProviderDescriptor, ProviderReadiness } from "../../types";
import type { ProviderCredentialPresenceLookup } from "../providerRegistryHelpers";
import type { BadgeTone } from "./badgeTone";
import { readinessTone as statusTone } from "./badgeTone";
import { readinessChipTone } from "./readinessTone";

/** Deterministic, descriptor-derived id — no `useId()` needed (and none of
 * this component's callers may call hooks inside their `.map()`). */
export function providerChooserAnnotationId(descriptorId: string): string {
  return `provider-annotation-${descriptorId.replace(/\./g, "-")}`;
}

/** Axis-2 credential-presence chip tone: PREREQUISITES, never OBSERVED —
 * "saved" is `neutral` (a saved key is not proof it works), "needed" is
 * `warning`, and "no credential required" is `neutral`. Never `success`. */
function credentialChipTone(
  descriptor: ProviderDescriptor,
  credentialPresence: ProviderCredentialPresenceLookup,
): BadgeTone {
  if (descriptor.credential_keys.length === 0) return "neutral";
  const anyPresent = descriptor.credential_keys.some(
    (key) => credentialPresence[key]?.present === true,
  );
  return anyPresent ? "neutral" : "warning";
}

function credentialChipLabel(
  descriptor: ProviderDescriptor,
  credentialPresence: ProviderCredentialPresenceLookup,
  t: TFunction,
): string {
  if (descriptor.credential_keys.length === 0) {
    return t("settings.providerChooser.credentialNone");
  }
  const anyPresent = descriptor.credential_keys.some(
    (key) => credentialPresence[key]?.present === true,
  );
  return t(
    anyPresent
      ? "settings.providerChooser.credentialSaved"
      : "settings.providerChooser.credentialNeeded",
  );
}

function requirementLine(
  descriptor: ProviderDescriptor,
  t: TFunction,
): string | null {
  if (descriptor.model_catalog === "local_files") {
    return t("settings.providerChooser.requirementLocalModel");
  }
  if (descriptor.default_model) {
    return t("settings.providerChooser.requirementDefaultModel", {
      model: descriptor.default_model,
    });
  }
  return null;
}

interface ProviderChooserRowProps {
  descriptor: ProviderDescriptor;
  /** Is this the currently-selected radio in the group? */
  active: boolean;
  /** The ACTIVE provider's readiness entry (only consulted when `active` is
   * true — callers already have this on hand as `activeProviderReadiness`,
   * so no per-option readiness lookup is needed). */
  activeReadiness: ProviderReadiness | null;
  credentialPresence: ProviderCredentialPresenceLookup;
  t: TFunction;
  children: ReactNode;
}

export default function ProviderChooserRow({
  descriptor,
  active,
  activeReadiness,
  credentialPresence,
  t,
  children,
}: ProviderChooserRowProps) {
  const annotationId = providerChooserAnnotationId(descriptor.id);
  const boundaryLabel = t(
    `settings.providerReadiness.dataBoundary.${descriptor.privacy.data_boundary}`,
  );
  const readinessChip = active
    ? readinessChipTone(
        {
          status: activeReadiness?.status,
          stale: activeReadiness?.stale,
          automaticProbeAvailable: activeReadiness?.automatic_probe_available,
          active: true,
        },
        statusTone,
      )
    : null;
  const line = requirementLine(descriptor, t);

  return (
    <div className="settings-radio-row">
      {children}
      <div id={annotationId} className="settings-radio-annotation">
        {/* Axis 1 (PLANNED) — never `success`; a config/registry fact, not an
            observed claim. */}
        <span className="ag-chip" data-tone="accent">
          {boundaryLabel}
        </span>
        {active && readinessChip?.render ? (
          // Axis 3 (OBSERVED) on the active row — the same T2 law gate the
          // ProviderReadinessPanel above this radiogroup uses.
          <span className="ag-chip" data-tone={readinessChip.tone}>
            {t(
              `settings.providerReadiness.status.${readinessChip.effectiveStatus}`,
            )}
          </span>
        ) : !active ? (
          // Axis 2 (PREREQUISITES) on non-active rows — credential presence
          // is a prerequisite fact, never the OBSERVED "Ready" claim.
          <span
            className="ag-chip"
            data-tone={credentialChipTone(descriptor, credentialPresence)}
          >
            {credentialChipLabel(descriptor, credentialPresence, t)}
          </span>
        ) : null}
        {line && <p className="settings-radio-annotation__line">{line}</p>}
      </div>
    </div>
  );
}
