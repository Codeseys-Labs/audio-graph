/**
 * Logging & Analytics rail section (blueprint §5). Thin wrapper over the
 * already-extracted `<LoggingSettings>`. Reads nothing from context today (the
 * sub-component is self-contained), but lives as a panel file so the shell
 * mounts every rail section uniformly (Phase 2).
 */

import LoggingSettings from "../LoggingSettings";

export default function LoggingPanel() {
  return <LoggingSettings />;
}
