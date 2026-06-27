import type { ReactNode } from "react";

interface AdvancedSettingsDisclosureProps {
  children: ReactNode;
  summary: ReactNode;
}

export default function AdvancedSettingsDisclosure({
  children,
  summary,
}: AdvancedSettingsDisclosureProps) {
  return (
    <details className="settings-advanced">
      <summary className="settings-advanced__summary">{summary}</summary>
      <div className="settings-advanced__body">{children}</div>
    </details>
  );
}
