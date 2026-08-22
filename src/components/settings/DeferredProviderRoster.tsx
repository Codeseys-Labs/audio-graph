/**
 * Deferred-provider roster (settings T3, audio-graph-9d2b).
 *
 * A collapsed `<details>` disclosure listing every implemented-but-withheld
 * settings-variant sub-form (`providerIsDeferred` — MVP scoping,
 * audio-graph-ad56/e153) for a stage whose picker itself has shrunk to a
 * single selectable option (STT/ASR today). Ratified framing (R1, maintainer,
 * 2026-08-21): "built, deliberately not offered; saved configs keep working"
 * — NEVER "coming soon". Per ADR-0030/0033, credential presence alone never
 * means Ready and a deferred provider is not selectable, so every row here is
 * plain text: no buttons, no links, nothing actionable.
 */

import type { TFunction } from "i18next";
import type { ProviderSettingsOption } from "../providerRegistryHelpers";

export default function DeferredProviderRoster<T extends string>({
  options,
  t,
}: {
  options: ProviderSettingsOption<T>[];
  t: TFunction;
}) {
  if (options.length === 0) return null;

  return (
    <details className="settings-provider-deferred-roster">
      <summary className="settings-provider-deferred-roster__summary">
        {t("settings.providerDeferred.roster.toggle")}
      </summary>
      <p className="settings-hint">
        {t("settings.providerDeferred.roster.summary", {
          count: options.length,
        })}
      </p>
      <ul className="settings-provider-deferred-roster__list">
        {options.map((option) => (
          <li key={option.descriptor.id}>{option.label}</li>
        ))}
      </ul>
      <p className="settings-hint settings-provider-deferred-roster__note">
        {t("settings.providerDeferred.roster.note")}
      </p>
    </details>
  );
}
