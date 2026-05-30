import { fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useAudioGraphStore } from "../store";
import type { AudioSourceInfo, ProcessInfo } from "../types";
import AudioSourceSelector from "./AudioSourceSelector";

function deviceSource(
  id: string,
  name: string,
  deviceId: string,
): AudioSourceInfo {
  return {
    id,
    name,
    source_type: { type: "Device", device_id: deviceId },
    is_active: false,
  };
}

function systemSource(): AudioSourceInfo {
  return {
    id: "system-default",
    name: "System Audio",
    source_type: { type: "SystemDefault" },
    is_active: false,
  };
}

function proc(pid: number, name: string): ProcessInfo {
  return { pid, name, exe_path: null };
}

function resetStore(
  overrides: Partial<ReturnType<typeof useAudioGraphStore.getState>> = {},
) {
  useAudioGraphStore.setState({
    audioSources: [],
    selectedSourceIds: [],
    toggleSourceId: vi.fn(),
    fetchSources: vi.fn(async () => {}),
    fetchProcesses: vi.fn(async () => {}),
    isCapturing: false,
    processes: [],
    searchFilter: "",
    setSearchFilter: vi.fn(),
    ...overrides,
  });
}

describe("AudioSourceSelector", () => {
  beforeEach(() => {
    localStorage.clear();
    resetStore();
  });

  it("fetches sources and processes on mount", () => {
    const fetchSources = vi.fn(async () => {});
    const fetchProcesses = vi.fn(async () => {});
    resetStore({ fetchSources, fetchProcesses });
    render(<AudioSourceSelector />);
    expect(fetchSources).toHaveBeenCalledTimes(1);
    expect(fetchProcesses).toHaveBeenCalledTimes(1);
  });

  it("renders the title and an accessible Refresh button", () => {
    render(<AudioSourceSelector />);
    expect(screen.getByText("Audio Sources")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /refresh sources/i }),
    ).toBeInTheDocument();
  });

  it("shows the no-targets empty state when there are no sources or processes", () => {
    render(<AudioSourceSelector />);
    expect(
      screen.getByText(/no capture targets detected/i),
    ).toBeInTheDocument();
    // Empty state offers a Retry action.
    expect(screen.getByRole("button", { name: /retry/i })).toBeInTheDocument();
  });

  it("Retry in the empty state re-fetches sources and processes", () => {
    const fetchSources = vi.fn(async () => {});
    const fetchProcesses = vi.fn(async () => {});
    resetStore({ fetchSources, fetchProcesses });
    render(<AudioSourceSelector />);
    fetchSources.mockClear();
    fetchProcesses.mockClear();
    fireEvent.click(screen.getByRole("button", { name: /retry/i }));
    expect(fetchSources).toHaveBeenCalledTimes(1);
    expect(fetchProcesses).toHaveBeenCalledTimes(1);
  });

  it("renders selectable source rows as checkboxes with accessible state", () => {
    resetStore({
      audioSources: [
        systemSource(),
        deviceSource("device:mic", "Microphone", "{0.0.1.001}"),
      ],
      selectedSourceIds: ["system-default"],
    });
    render(<AudioSourceSelector />);
    const checkboxes = screen.getAllByRole("checkbox");
    expect(checkboxes.length).toBeGreaterThanOrEqual(2);
    // The selected system source row reports aria-checked=true.
    const systemRow = screen
      .getByText("System Audio")
      .closest('[role="checkbox"]') as HTMLElement;
    expect(systemRow).toHaveAttribute("aria-checked", "true");
  });

  it("clicking a source row toggles its selection", () => {
    const toggleSourceId = vi.fn();
    resetStore({
      audioSources: [deviceSource("device:mic", "Microphone", "{0.0.1.001}")],
      toggleSourceId,
    });
    render(<AudioSourceSelector />);
    fireEvent.click(screen.getByText("Microphone"));
    expect(toggleSourceId).toHaveBeenCalledWith("device:mic");
  });

  it("toggles a source row via the keyboard (Enter / Space)", () => {
    const toggleSourceId = vi.fn();
    resetStore({
      audioSources: [deviceSource("device:mic", "Microphone", "{0.0.1.001}")],
      toggleSourceId,
    });
    render(<AudioSourceSelector />);
    const row = screen
      .getByText("Microphone")
      .closest('[role="checkbox"]') as HTMLElement;
    fireEvent.keyDown(row, { key: "Enter" });
    fireEvent.keyDown(row, { key: " " });
    expect(toggleSourceId).toHaveBeenCalledTimes(2);
  });

  it("disables Refresh and locks toggling while capturing", () => {
    const toggleSourceId = vi.fn();
    resetStore({
      audioSources: [deviceSource("device:mic", "Microphone", "{0.0.1.001}")],
      toggleSourceId,
      isCapturing: true,
    });
    render(<AudioSourceSelector />);
    // Refresh is disabled and relabeled to the capture-locked message.
    expect(
      screen.getByRole("button", { name: /stop capture to change sources/i }),
    ).toBeDisabled();
    // Clicking a row while capturing is a no-op.
    fireEvent.click(screen.getByText("Microphone"));
    expect(toggleSourceId).not.toHaveBeenCalled();
    // The capture-locked notice is shown.
    expect(
      screen.getAllByText(/stop capture to change sources/i).length,
    ).toBeGreaterThanOrEqual(1);
  });

  it("typing in the search box drives setSearchFilter", () => {
    const setSearchFilter = vi.fn();
    resetStore({
      audioSources: [deviceSource("device:mic", "Microphone", "{0.0.1.001}")],
      setSearchFilter,
    });
    render(<AudioSourceSelector />);
    fireEvent.change(
      screen.getByPlaceholderText(/search sources & processes/i),
      { target: { value: "mic" } },
    );
    expect(setSearchFilter).toHaveBeenCalledWith("mic");
  });

  it("shows a Clear-search button only when the filter is non-empty, and clears on click", () => {
    const setSearchFilter = vi.fn();
    // No filter → no clear button.
    resetStore({
      audioSources: [systemSource()],
      setSearchFilter,
    });
    const { rerender } = render(<AudioSourceSelector />);
    expect(
      screen.queryByRole("button", { name: /clear search/i }),
    ).not.toBeInTheDocument();

    // With a filter set, the clear button appears and resets the filter.
    resetStore({
      audioSources: [systemSource()],
      searchFilter: "mic",
      setSearchFilter,
    });
    rerender(<AudioSourceSelector />);
    fireEvent.click(screen.getByRole("button", { name: /clear search/i }));
    expect(setSearchFilter).toHaveBeenCalledWith("");
  });

  it("shows a no-matches message when a filter excludes every source", () => {
    resetStore({
      audioSources: [deviceSource("device:mic", "Microphone", "{0.0.1.001}")],
      processes: [],
      searchFilter: "zzz-nothing",
    });
    render(<AudioSourceSelector />);
    expect(
      screen.getByText(/no matches for "zzz-nothing"/i),
    ).toBeInTheDocument();
  });

  it("filters sources by name (case-insensitive) when searching", () => {
    resetStore({
      audioSources: [
        deviceSource("device:mic", "Microphone", "{0.0.1.001}"),
        deviceSource("device:spk", "Speakers", "{0.0.0.001}"),
      ],
      searchFilter: "micro",
    });
    render(<AudioSourceSelector />);
    expect(screen.getByText("Microphone")).toBeInTheDocument();
    expect(screen.queryByText("Speakers")).not.toBeInTheDocument();
  });

  it("classifies devices into Input vs Output groups by WASAPI endpoint id", () => {
    resetStore({
      audioSources: [
        deviceSource("device:mic", "Mic A", "{0.0.1.111}"),
        deviceSource("device:spk", "Speaker A", "{0.0.0.222}"),
      ],
    });
    render(<AudioSourceSelector />);
    expect(screen.getByText("Input Devices")).toBeInTheDocument();
    expect(screen.getByText("Output Devices")).toBeInTheDocument();
  });

  it("collapses a group when its header is toggled", () => {
    resetStore({
      audioSources: [deviceSource("device:mic", "Microphone", "{0.0.1.001}")],
    });
    render(<AudioSourceSelector />);
    const groupBtn = screen.getByRole("button", {
      name: /input devices/i,
    });
    expect(groupBtn).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText("Microphone")).toBeInTheDocument();
    fireEvent.click(groupBtn);
    expect(groupBtn).toHaveAttribute("aria-expanded", "false");
    // Collapsed → the row is no longer rendered.
    expect(screen.queryByText("Microphone")).not.toBeInTheDocument();
  });

  it("offers Audio-apps vs All-processes scope tabs", () => {
    resetStore({ audioSources: [systemSource()] });
    render(<AudioSourceSelector />);
    const audioTab = screen.getByRole("tab", { name: /audio apps/i });
    const allTab = screen.getByRole("tab", { name: /all processes/i });
    // Default scope is "audio".
    expect(audioTab).toHaveAttribute("aria-selected", "true");
    expect(allTab).toHaveAttribute("aria-selected", "false");
    fireEvent.click(allTab);
    expect(allTab).toHaveAttribute("aria-selected", "true");
  });

  it("lists Running Processes with per-process and process-tree toggles in All scope", () => {
    const toggleSourceId = vi.fn();
    resetStore({
      audioSources: [systemSource()],
      processes: [proc(1234, "code.exe")],
      toggleSourceId,
    });
    render(<AudioSourceSelector />);
    fireEvent.click(screen.getByRole("tab", { name: /all processes/i }));
    expect(screen.getByText("code.exe")).toBeInTheDocument();
    expect(screen.getByText(/PID 1234/)).toBeInTheDocument();

    // "Process" button captures app:<pid>; "Tree" captures process-tree:<pid>.
    fireEvent.click(screen.getByRole("button", { name: "Process" }));
    expect(toggleSourceId).toHaveBeenCalledWith("app:1234");
    fireEvent.click(screen.getByRole("button", { name: "Tree" }));
    expect(toggleSourceId).toHaveBeenCalledWith("process-tree:1234");
  });

  it("reveals matching processes even in Audio scope when a search filter is active", () => {
    resetStore({
      audioSources: [systemSource()],
      processes: [proc(99, "spotify.exe")],
      searchFilter: "spotify",
    });
    render(<AudioSourceSelector />);
    // Audio scope normally hides Running Processes, but a search reaches them.
    expect(screen.getByText("spotify.exe")).toBeInTheDocument();
  });

  it("shows the no-process-targets hint when processes are empty and not searching", () => {
    resetStore({ audioSources: [systemSource()], processes: [] });
    render(<AudioSourceSelector />);
    expect(
      screen.getByText(/no process targets detected/i),
    ).toBeInTheDocument();
  });

  it("tags the system-default source with a Default badge", () => {
    resetStore({ audioSources: [systemSource()] });
    render(<AudioSourceSelector />);
    const row = screen
      .getByText("System Audio")
      .closest('[role="checkbox"]') as HTMLElement;
    expect(within(row).getByText(/default/i)).toBeInTheDocument();
  });
});
