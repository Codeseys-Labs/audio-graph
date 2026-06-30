// SettingsContext — Phase 1 (audio-graph-settings-refactor).
//
// Wraps `useSettingsController()` so the SettingsPage shell and every per-section
// panel can read the orchestration value (state, dispatch, routes, handlers,
// derived flags) without prop-drilling 40+ props (blueprint §5). The provider
// calls the controller hook once; consumers read the memo-free value object via
// `useSettings()`. The value identity changes every render (same as before the
// hoist, when these lived as locals), so behavior is preserved.

import { createContext, type ReactNode, useContext } from "react";
import {
  type SettingsControllerValue,
  useSettingsController,
} from "./useSettingsController";

const SettingsContext = createContext<SettingsControllerValue | null>(null);

/**
 * Provides the settings controller value to descendant panels. Must wrap the
 * SettingsPage render tree. Calls `useSettingsController()` exactly once.
 */
export function SettingsProvider({
  children,
}: {
  children: (value: SettingsControllerValue) => ReactNode;
}) {
  const value = useSettingsController();
  return (
    <SettingsContext.Provider value={value}>
      {children(value)}
    </SettingsContext.Provider>
  );
}

/**
 * Reads the settings controller value. Throws when used outside a
 * `SettingsProvider` so a misuse fails loudly rather than rendering with a
 * null context.
 */
export function useSettings(): SettingsControllerValue {
  const value = useContext(SettingsContext);
  if (value === null) {
    throw new Error("useSettings must be used within a SettingsProvider");
  }
  return value;
}
