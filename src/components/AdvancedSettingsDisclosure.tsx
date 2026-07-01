import { type ReactNode, useId } from "react";

interface AdvancedSettingsDisclosureProps {
  children: ReactNode;
  summary: ReactNode;
}

/**
 * Collapsible "Advanced settings" disclosure built on native `<details>` /
 * `<summary>` so keyboard toggling (Enter / Space on the focused summary) and
 * the open/closed state come for free from the platform.
 *
 * a11y: the disclosed body is a `role="group"` labelled by the summary via
 * `aria-labelledby`, so a screen reader announces the revealed controls as a
 * named group rather than a bare run of fields. The summary id is stable per
 * instance (`useId`) so multiple disclosures on one panel stay distinct.
 */
export default function AdvancedSettingsDisclosure({
  children,
  summary,
}: AdvancedSettingsDisclosureProps) {
  const summaryId = useId();
  return (
    <details className="settings-advanced">
      <summary id={summaryId} className="settings-advanced__summary">
        {summary}
      </summary>
      {/* biome-ignore lint/a11y/useSemanticElements: a <fieldset> here inherits
          the UA border/padding/min-inline-size box that the .settings-advanced__body
          flow layout depends on; role="group" + aria-labelledby gives the same
          named-grouping semantics without disturbing the disclosure's layout. */}
      <div
        className="settings-advanced__body"
        role="group"
        aria-labelledby={summaryId}
      >
        {children}
      </div>
    </details>
  );
}
