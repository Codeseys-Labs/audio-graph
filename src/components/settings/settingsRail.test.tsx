import { fireEvent, render, screen, within } from "@testing-library/react";
import type { TFunction } from "i18next";
import { useRef, useState } from "react";
import { describe, expect, it, vi } from "vitest";
import type { SettingsControllerValue } from "./useSettingsController";

// The rail is a thin presentational view over `useSettings()`. We mock the
// context with a minimal controllable value so this test isolates the rail's
// a11y contract (roving tabindex, group labels, aria-controls/labelledby
// wiring, orientation, keyboard delegation) from the heavyweight controller
// (which pulls in Tauri invoke and is exercised end-to-end in SettingsPage).
const mockUseSettings = vi.fn();
vi.mock("./SettingsContext", () => ({
  useSettings: () => mockUseSettings(),
}));

import SettingsRail from "./settingsRail";

const t = ((key: string) => key) as TFunction;

function makeValue(
  overrides: Partial<SettingsControllerValue> = {},
): SettingsControllerValue {
  return {
    t,
    activeTab: "overview",
    setActiveTab: vi.fn(),
    handleSettingsTabKeyDown: vi.fn(),
    railHorizontal: false,
    tabRefs: { current: {} },
    tabButtonId: (tab: string) => `settings-tab-${tab}`,
    tabPanelId: (tab: string) => `settings-panel-${tab}`,
    ...overrides,
  } as unknown as SettingsControllerValue;
}

/** Stateful harness so clicking/keying a tab reflects the new selection. */
function Harness({
  onKeyDown,
}: {
  onKeyDown?: (key: string, tab: string) => void;
}) {
  const [activeTab, setActiveTab] = useState("overview");
  const tabRefs = useRef<Record<string, HTMLButtonElement | null>>({});
  mockUseSettings.mockReturnValue(
    makeValue({
      activeTab: activeTab as SettingsControllerValue["activeTab"],
      setActiveTab: setActiveTab as SettingsControllerValue["setActiveTab"],
      tabRefs: tabRefs as SettingsControllerValue["tabRefs"],
      handleSettingsTabKeyDown: ((e, tab) =>
        onKeyDown?.(
          e.key,
          tab,
        )) as SettingsControllerValue["handleSettingsTabKeyDown"],
    }),
  );
  return <SettingsRail />;
}

describe("SettingsRail a11y", () => {
  it("renders an APG vertical tablist with grouped, presentational headers", () => {
    mockUseSettings.mockReturnValue(makeValue());
    render(<SettingsRail />);

    const tablist = screen.getByRole("tablist", { name: /settings.title/i });
    expect(tablist).toHaveAttribute("aria-orientation", "vertical");

    // Group headers must not be exposed as headings/list items that pollute the
    // tab sequence — they are presentational dividers only.
    const groupHeaders = screen.getAllByText(/settings.railGroups\./);
    for (const header of groupHeaders) {
      expect(header).toHaveAttribute("role", "presentation");
    }

    // Every rail item is a tab wired to its panel both ways.
    const tabs = screen.getAllByRole("tab");
    expect(tabs.length).toBeGreaterThanOrEqual(8);
    for (const tab of tabs) {
      const panelId = tab.getAttribute("aria-controls");
      expect(panelId).toMatch(/^settings-panel-/);
      expect(tab.id).toMatch(/^settings-tab-/);
    }
  });

  it("flips orientation to horizontal below the narrow breakpoint", () => {
    mockUseSettings.mockReturnValue(makeValue({ railHorizontal: true }));
    render(<SettingsRail />);
    expect(
      screen.getByRole("tablist", { name: /settings.title/i }),
    ).toHaveAttribute("aria-orientation", "horizontal");
  });

  it("keeps exactly one tab in the focus order (roving tabindex)", () => {
    mockUseSettings.mockReturnValue(makeValue({ activeTab: "llm" }));
    render(<SettingsRail />);

    const tabs = screen.getAllByRole("tab");
    const focusable = tabs.filter(
      (tab) => tab.getAttribute("tabindex") === "0",
    );
    expect(focusable).toHaveLength(1);
    expect(focusable[0]).toHaveAttribute("aria-selected", "true");
    expect(focusable[0].id).toBe("settings-tab-llm");
    for (const tab of tabs) {
      if (tab.id !== "settings-tab-llm") {
        expect(tab).toHaveAttribute("tabindex", "-1");
        expect(tab).toHaveAttribute("aria-selected", "false");
      }
    }
  });

  it("delegates arrow/Home/End keys to the controller handler with the tab id", () => {
    const onKeyDown = vi.fn();
    render(<Harness onKeyDown={onKeyDown} />);

    const tablist = screen.getByRole("tablist");
    const firstTab = within(tablist).getAllByRole("tab")[0];
    firstTab.focus();

    fireEvent.keyDown(firstTab, { key: "ArrowDown" });
    fireEvent.keyDown(firstTab, { key: "End" });
    fireEvent.keyDown(firstTab, { key: "Home" });

    expect(onKeyDown).toHaveBeenCalledWith("ArrowDown", "overview");
    expect(onKeyDown).toHaveBeenCalledWith("End", "overview");
    expect(onKeyDown).toHaveBeenCalledWith("Home", "overview");
  });

  it("selects a tab on click so pointer and keyboard stay in sync", () => {
    render(<Harness />);

    const tabs = screen.getAllByRole("tab");
    const llmTab = tabs.find((tab) => tab.id === "settings-tab-llm");
    if (!llmTab) throw new Error("expected an LLM rail tab");
    expect(llmTab).toHaveAttribute("aria-selected", "false");

    fireEvent.click(llmTab);

    expect(llmTab).toHaveAttribute("aria-selected", "true");
    expect(llmTab).toHaveAttribute("tabindex", "0");
  });
});
