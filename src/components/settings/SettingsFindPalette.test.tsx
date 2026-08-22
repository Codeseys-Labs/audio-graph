import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "../../i18n";
import { useAudioGraphStore } from "../../store";
import SettingsFindPalette from "./SettingsFindPalette";

type StoreState = ReturnType<typeof useAudioGraphStore.getState>;

function resetStore(overrides: Partial<StoreState> = {}) {
  useAudioGraphStore.setState({
    openSettings: vi.fn(),
    ...overrides,
  });
}

describe("SettingsFindPalette (T4b, audio-graph-4850)", () => {
  beforeEach(() => {
    resetStore();
  });

  it("renders NOTHING while closed — zero role nodes, so the 210+ role queries elsewhere in the app never see a stray combobox/listbox", () => {
    const { container } = render(<SettingsFindPalette />);
    expect(container).toBeEmptyDOMElement();
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
  });

  it("opens on Ctrl+F when focus is not in an input", () => {
    render(<SettingsFindPalette />);
    fireEvent.keyDown(window, { key: "f", ctrlKey: true });
    expect(screen.getByRole("combobox")).toBeInTheDocument();
    expect(screen.getByRole("listbox")).toBeInTheDocument();
  });

  it("opens on plain '/' when focus is not in an input", () => {
    render(<SettingsFindPalette />);
    fireEvent.keyDown(window, { key: "/" });
    expect(screen.getByRole("combobox")).toBeInTheDocument();
  });

  it("does NOT open on '/' while focus is inside a text input (typing-context guard)", () => {
    render(
      <div>
        <input aria-label="some other field" />
        <SettingsFindPalette />
      </div>,
    );
    const otherInput = screen.getByLabelText("some other field");
    otherInput.focus();
    fireEvent.keyDown(otherInput, { key: "/" });
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
  });

  it("does NOT open on Ctrl+F while focus is inside a text input", () => {
    render(
      <div>
        <input aria-label="some other field" />
        <SettingsFindPalette />
      </div>,
    );
    const otherInput = screen.getByLabelText("some other field");
    otherInput.focus();
    fireEvent.keyDown(otherInput, { key: "f", ctrlKey: true });
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
  });

  it("wires the REAL rendered combobox+listbox ARIA pattern: aria-controls points at the listbox id, aria-activedescendant points at the active option id", () => {
    render(<SettingsFindPalette />);
    fireEvent.keyDown(window, { key: "/" });

    const combobox = screen.getByRole("combobox");
    const listbox = screen.getByRole("listbox");
    expect(combobox).toHaveAttribute("aria-controls", listbox.id);
    expect(combobox).toHaveAttribute("aria-autocomplete", "list");

    const options = screen.getAllByRole("option");
    expect(options.length).toBeGreaterThan(0);
    const activeDescendant = combobox.getAttribute("aria-activedescendant");
    expect(activeDescendant).toBe(options[0].id);
    expect(options[0]).toHaveAttribute("aria-selected", "true");
  });

  it("filters results as the query changes, always naming the provider a shared field belongs to", () => {
    render(<SettingsFindPalette />);
    fireEvent.keyDown(window, { key: "/" });
    const combobox = screen.getByRole("combobox");

    fireEvent.change(combobox, { target: { value: "cerebras" } });
    const options = screen.getAllByRole("option");
    expect(options.length).toBeGreaterThan(0);
    for (const option of options) {
      expect(option.textContent).toMatch(/cerebras/i);
    }

    // A shared field label ("API Key") must still name which provider —
    // mandatory qualifier (synthesis §T4b).
    fireEvent.change(combobox, { target: { value: "api key" } });
    const apiKeyOptions = screen.getAllByRole("option");
    expect(apiKeyOptions.length).toBeGreaterThan(1);
    for (const option of apiKeyOptions) {
      expect(option.textContent).toMatch(/API Key — .+/);
    }
  });

  it("shows the empty-state copy for a query with no matches", () => {
    render(<SettingsFindPalette />);
    fireEvent.keyDown(window, { key: "/" });
    fireEvent.change(screen.getByRole("combobox"), {
      target: { value: "zzz-no-such-setting-zzz" },
    });
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    expect(screen.getByText(/no matching settings/i)).toBeInTheDocument();
  });

  it("ArrowDown moves aria-activedescendant to the next option", () => {
    render(<SettingsFindPalette />);
    fireEvent.keyDown(window, { key: "/" });
    const combobox = screen.getByRole("combobox");
    const firstOptionId = combobox.getAttribute("aria-activedescendant");

    fireEvent.keyDown(combobox, { key: "ArrowDown" });
    const secondOptionId = combobox.getAttribute("aria-activedescendant");
    expect(secondOptionId).not.toBe(firstOptionId);
    expect(document.getElementById(secondOptionId as string)).toHaveAttribute(
      "aria-selected",
      "true",
    );
  });

  it("Enter jumps via openSettings(route) with the highlighted entry's {tab, fieldId} and closes the palette", () => {
    const openSettings = vi.fn();
    resetStore({ openSettings });
    render(<SettingsFindPalette />);
    fireEvent.keyDown(window, { key: "/" });

    fireEvent.change(screen.getByRole("combobox"), {
      target: { value: "cerebras api key" },
    });
    fireEvent.keyDown(screen.getByRole("combobox"), { key: "Enter" });

    expect(openSettings).toHaveBeenCalledTimes(1);
    expect(openSettings).toHaveBeenCalledWith({
      tab: "llm",
      fieldId: "llm-cerebras-api-key",
      activate: true,
    });
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
  });

  it("Escape closes without navigating", () => {
    const openSettings = vi.fn();
    resetStore({ openSettings });
    render(<SettingsFindPalette />);
    fireEvent.keyDown(window, { key: "/" });
    expect(screen.getByRole("combobox")).toBeInTheDocument();

    fireEvent.keyDown(screen.getByRole("combobox"), { key: "Escape" });

    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
    expect(openSettings).not.toHaveBeenCalled();
  });

  it("clicking a result jumps to it (mouse path, not only keyboard)", () => {
    const openSettings = vi.fn();
    resetStore({ openSettings });
    render(<SettingsFindPalette />);
    fireEvent.keyDown(window, { key: "/" });
    fireEvent.change(screen.getByRole("combobox"), {
      target: { value: "sambanova api key" },
    });

    fireEvent.click(screen.getByRole("option"));

    expect(openSettings).toHaveBeenCalledWith({
      tab: "llm",
      fieldId: "llm-sambanova-api-key",
      activate: true,
    });
  });
});
