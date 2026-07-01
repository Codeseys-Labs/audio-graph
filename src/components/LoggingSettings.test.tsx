/**
 * LoggingSettings — analytics-toggle independence + invoke wiring.
 *
 * The "Privacy & Diagnostics" (anonymous analytics) toggle and the file-logging
 * toggle are documented as fully independent: enable either, both, or neither,
 * with separate local state and separate busy/status flows
 * (`LoggingSettings.tsx` header). These tests pin that contract:
 *
 *   1. The analytics toggle wires to the `set_analytics_enabled` Tauri command
 *      with the new checked value (invoke wiring).
 *   2. Toggling analytics does NOT touch the file-logging command
 *      (`set_logging_config`) — analytics is independent of file logging.
 *   3. Toggling file logging does NOT touch `set_analytics_enabled` — file
 *      logging is independent of analytics.
 *
 * The Tauri `invoke` is globally mocked in `src/test/setup.ts`; here we route
 * it per-command so the component's initial reads resolve and we can assert on
 * the side-effecting writes.
 */

import { invoke } from "@tauri-apps/api/core";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, type vi } from "vitest";
import type { AnalyticsInfo } from "../types";
import LoggingSettings from "./LoggingSettings";
import "../i18n";

interface LogInfo {
  enabled: boolean;
  mode: string;
  level: string;
  dir: string;
  active_path: string | null;
  files: {
    name: string;
    size_bytes: number;
    modified_ms: number | null;
    is_active: boolean;
  }[];
}

const mockedInvoke = invoke as unknown as ReturnType<typeof vi.fn>;

function logInfo(overrides: Partial<LogInfo> = {}): LogInfo {
  return {
    enabled: true,
    mode: "archive",
    level: "info",
    dir: "/tmp/logs",
    active_path: null,
    files: [],
    ...overrides,
  };
}

function analyticsInfo(overrides: Partial<AnalyticsInfo> = {}): AnalyticsInfo {
  return {
    enabled: false,
    dsn_configured: true,
    pii_disabled: true,
    ...overrides,
  };
}

/**
 * Route the global `invoke` mock per command. Returns the call log so tests can
 * assert which commands fired (and which did NOT).
 */
function installInvoke(
  opts: { logEnabled?: boolean; analyticsEnabled?: boolean } = {},
) {
  const initialLog = logInfo({ enabled: opts.logEnabled ?? true });
  const initialAnalytics = analyticsInfo({
    enabled: opts.analyticsEnabled ?? false,
  });

  mockedInvoke.mockReset();
  mockedInvoke.mockImplementation(
    async (cmd: string, args?: Record<string, unknown>) => {
      switch (cmd) {
        case "get_log_info":
          return initialLog;
        case "get_analytics_info":
          return initialAnalytics;
        case "set_logging_config":
          return logInfo({ enabled: Boolean(args?.enabled) });
        case "set_analytics_enabled":
          return analyticsInfo({ enabled: Boolean(args?.enabled) });
        default:
          return undefined;
      }
    },
  );
}

function commandCalls(cmd: string): Record<string, unknown>[] {
  return mockedInvoke.mock.calls
    .filter(([name]) => name === cmd)
    .map(([, args]) => (args ?? {}) as Record<string, unknown>);
}

describe("LoggingSettings — analytics toggle", () => {
  beforeEach(() => {
    installInvoke();
  });

  it("wires the analytics toggle to set_analytics_enabled with the new value", async () => {
    const user = userEvent.setup();
    render(<LoggingSettings />);

    // Wait for the initial analytics read to resolve and enable the toggle.
    const analyticsToggle = await screen.findByLabelText(
      "Send anonymous analytics",
    );
    await waitFor(() => expect(analyticsToggle).toBeEnabled());
    expect(analyticsToggle).not.toBeChecked();

    await user.click(analyticsToggle);

    const calls = commandCalls("set_analytics_enabled");
    expect(calls).toHaveLength(1);
    expect(calls[0]).toEqual({ enabled: true });
  });

  it("is independent of file logging: toggling analytics does not touch set_logging_config", async () => {
    const user = userEvent.setup();
    render(<LoggingSettings />);

    const analyticsToggle = await screen.findByLabelText(
      "Send anonymous analytics",
    );
    await waitFor(() => expect(analyticsToggle).toBeEnabled());

    await user.click(analyticsToggle);

    // The analytics write fired...
    expect(commandCalls("set_analytics_enabled")).toHaveLength(1);
    // ...but the file-logging command did NOT — the two toggles are decoupled.
    expect(commandCalls("set_logging_config")).toHaveLength(0);
  });

  it("is independent of analytics: toggling file logging does not touch set_analytics_enabled", async () => {
    const user = userEvent.setup();
    render(<LoggingSettings />);

    const loggingToggle = await screen.findByLabelText("Write logs to a file");
    await waitFor(() => expect(loggingToggle).toBeEnabled());

    await user.click(loggingToggle);

    // The file-logging write fired...
    expect(commandCalls("set_logging_config")).toHaveLength(1);
    // ...but the analytics command did NOT — flipping logging leaves analytics
    // untouched.
    expect(commandCalls("set_analytics_enabled")).toHaveLength(0);
  });
});

describe("LoggingSettings — level select", () => {
  beforeEach(() => {
    installInvoke();
  });

  it("offers exactly off/error/warn/info/debug/trace (the off capability moved here from the deduped Diagnostics control)", async () => {
    render(<LoggingSettings />);

    // Wait for the initial get_log_info read to hydrate the select.
    const select = (await screen.findByLabelText(
      "Log level",
    )) as HTMLSelectElement;
    const options = Array.from(select.options).map((o) => o.value);
    expect(options).toEqual(["off", "error", "warn", "info", "debug", "trace"]);
  });
});
