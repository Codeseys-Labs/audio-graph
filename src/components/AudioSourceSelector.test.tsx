import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useAudioGraphStore } from "../store";
import type { AudioDeviceKind, AudioSourceInfo, ProcessInfo } from "../types";
import AudioSourceSelector from "./AudioSourceSelector";

function deviceSource(
  id: string,
  name: string,
  deviceId: string,
  deviceKind: AudioDeviceKind | null = "Input",
): AudioSourceInfo {
  const source: AudioSourceInfo = {
    id,
    name,
    source_type: { type: "Device", device_id: deviceId },
    is_active: false,
  };
  if (deviceKind !== null) source.device_kind = deviceKind;
  return source;
}

function systemSource(): AudioSourceInfo {
  return {
    id: "system-default",
    name: "System Audio",
    source_type: { type: "SystemDefault" },
    is_active: false,
  };
}

function applicationNameSource(name: string): AudioSourceInfo {
  return {
    id: `app-name:${name}`,
    name,
    source_type: { type: "ApplicationName", app_name: name },
    is_active: false,
  };
}

function applicationSource(
  pid: number,
  name: string,
  bundleId?: string,
): AudioSourceInfo {
  return {
    id: `app:${pid}`,
    name,
    source_type: {
      type: "Application",
      pid,
      app_name: name,
      bundle_id: bundleId,
    },
    is_active: false,
  };
}

function processTreeSource(pid: number, name: string): AudioSourceInfo {
  return {
    id: `process-tree:${pid}`,
    name,
    source_type: { type: "ProcessTree", pid },
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
    removeSelectedSourceIds: vi.fn(),
    sourceRecoveryIntent: null,
    clearSourceRecoveryIntent: vi.fn(),
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

  it("refreshes and focuses the picker when source recovery is requested", async () => {
    const fetchSources = vi.fn(async () => {});
    const fetchProcesses = vi.fn(async () => {});
    const setSearchFilter = vi.fn();
    resetStore({
      fetchSources,
      fetchProcesses,
      setSearchFilter,
      sourceRecoveryIntent: {
        id: 1,
        origin: "provider_setup",
        requestedAt: Date.now(),
        issues: [
          {
            kind: "unselected",
            message: "Select an audio source before starting capture.",
          },
        ],
      },
    });

    render(<AudioSourceSelector />);

    expect(fetchSources).toHaveBeenCalledTimes(2);
    expect(fetchProcesses).toHaveBeenCalledTimes(2);
    expect(setSearchFilter).toHaveBeenCalledWith("");
    expect(screen.getByText(/select an audio source/i)).toBeInTheDocument();
    await waitFor(() =>
      expect(
        screen.getByPlaceholderText(/search sources & processes/i),
      ).toHaveFocus(),
    );
  });

  it("offers clear and reselect actions for stale or unsupported selected sources", () => {
    const removeSelectedSourceIds = vi.fn();
    const unsupported = deviceSource(
      "device:virtual",
      "Virtual Device",
      "dev-1",
    );
    unsupported.capabilities = {
      backend_name: "FixtureBackend",
      capture_supported: false,
      supports_system_capture: true,
      supports_application_capture: true,
      supports_process_tree_capture: true,
      supports_device_selection: false,
      supports_device_change_notifications: true,
      unsupported_reason: "Device selection is not supported by FixtureBackend",
    };
    resetStore({
      audioSources: [unsupported],
      selectedSourceIds: ["device:stale", "device:virtual"],
      removeSelectedSourceIds,
    });

    render(<AudioSourceSelector />);

    expect(screen.getByText(/source needs attention/i)).toBeInTheDocument();
    expect(
      screen.getByText(/selected audio source device:stale is not available/i),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/Virtual Device cannot be captured/i),
    ).toBeInTheDocument();
    expect(screen.getByText("Unsupported")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /clear unavailable/i }));
    expect(removeSelectedSourceIds).toHaveBeenCalledWith(["device:stale"]);

    fireEvent.click(screen.getByRole("button", { name: /clear unsupported/i }));
    expect(removeSelectedSourceIds).toHaveBeenCalledWith(["device:virtual"]);

    fireEvent.click(screen.getByRole("button", { name: /reselect sources/i }));
    expect(removeSelectedSourceIds).toHaveBeenCalledWith([
      "device:stale",
      "device:virtual",
    ]);
  });

  it("uses permission metadata for source recovery guidance", () => {
    const source = deviceSource("device:mic", "Studio Mic", "mic-1");
    source.permission_status = "Denied";
    source.permission_recovery = {
      platform: "Macos",
      permission_kind: "AudioCapture",
      summary: "macOS Audio Capture permission is denied.",
      body: "Grant AudioGraph permission in macOS Privacy & Security, then relaunch AudioGraph and refresh sources.",
      actions: [
        {
          kind: "GrantPermissionManually",
          label: "Grant permission manually",
        },
      ],
    };
    resetStore({
      audioSources: [source],
      selectedSourceIds: ["device:mic"],
    });

    render(<AudioSourceSelector />);

    expect(
      screen.getByText(/Studio Mic: macOS Audio Capture permission is denied/i),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        /Grant AudioGraph permission in macOS Privacy & Security/i,
      ),
    ).toBeInTheDocument();
    expect(screen.queryByText(/windows microphone/i)).not.toBeInTheDocument();
    expect(
      screen.queryByText(/audio capture permission is denied for Studio Mic/i),
    ).not.toBeInTheDocument();
  });

  it("falls back to generic source recovery guidance without backend copy", () => {
    const source = deviceSource("device:mic", "Studio Mic", "mic-1");
    source.permission_status = "Denied";
    resetStore({
      audioSources: [source],
      selectedSourceIds: ["device:mic"],
    });

    render(<AudioSourceSelector />);

    expect(
      screen.getByText(/audio capture permission is denied for Studio Mic/i),
    ).toBeInTheDocument();
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

  it("uses backend-provided capture_target instead of assuming source id is parseable", () => {
    const toggleSourceId = vi.fn();
    const source = deviceSource(
      "rsac:opaque-device-row",
      "Loopback Output",
      "opaque-rsac-id",
      "Output",
    );
    source.capture_target = "device:opaque-rsac-id";
    resetStore({
      audioSources: [source],
      selectedSourceIds: ["device:opaque-rsac-id"],
      toggleSourceId,
    });
    render(<AudioSourceSelector />);

    const row = screen
      .getByText("Loopback Output")
      .closest('[role="checkbox"]') as HTMLElement;
    expect(row).toHaveAttribute("aria-checked", "true");
    expect(within(row).getByText("Device")).toBeInTheDocument();

    fireEvent.click(row);
    expect(toggleSourceId).toHaveBeenCalledWith("device:opaque-rsac-id");
  });

  it("falls back to canonical device capture targets for raw Windows device rows", () => {
    const toggleSourceId = vi.fn();
    resetStore({
      audioSources: [
        deviceSource(
          "{0.0.1.00000000}.{fifine-guid}",
          "Fifine Microphone",
          "{0.0.1.00000000}.{fifine-guid}",
        ),
      ],
      selectedSourceIds: ["device:{0.0.1.00000000}.{fifine-guid}"],
      toggleSourceId,
    });
    render(<AudioSourceSelector />);

    const row = screen
      .getByText("Fifine Microphone")
      .closest('[role="checkbox"]') as HTMLElement;
    expect(row).toHaveAttribute("aria-checked", "true");
    expect(within(row).getByText("Device")).toBeInTheDocument();

    fireEvent.click(row);
    expect(toggleSourceId).toHaveBeenCalledWith(
      "device:{0.0.1.00000000}.{fifine-guid}",
    );
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

  it("classifies devices into Input vs Output groups by backend device_kind metadata", () => {
    resetStore({
      audioSources: [
        deviceSource(
          "device:not-a-mic",
          "Speaker-shaped Input",
          "opaque-a",
          "Input",
        ),
        deviceSource(
          "device:not-a-speaker",
          "Mic-shaped Output",
          "opaque-b",
          "Output",
        ),
      ],
    });
    render(<AudioSourceSelector />);
    expect(screen.getByText("Input Devices")).toBeInTheDocument();
    expect(screen.getByText("Output Devices")).toBeInTheDocument();
  });

  it("does not infer device direction from Windows ids or microphone-like names", () => {
    resetStore({
      audioSources: [
        deviceSource("device:mic", "Microphone Array", "{0.0.1.111}", null),
      ],
    });
    render(<AudioSourceSelector />);
    expect(screen.getByText("Unknown Devices")).toBeInTheDocument();
    expect(screen.queryByText("Input Devices")).not.toBeInTheDocument();
  });

  it("groups backend application-name and process-tree descriptors without falling back to Other", () => {
    resetStore({
      audioSources: [
        applicationNameSource("Spotify"),
        processTreeSource(4242, "DAW process tree"),
      ],
    });
    render(<AudioSourceSelector />);

    expect(screen.getByText("Applications")).toBeInTheDocument();
    expect(screen.getByText("Spotify")).toBeInTheDocument();
    expect(screen.getByText("Running Processes")).toBeInTheDocument();
    expect(screen.getByText("DAW process tree")).toBeInTheDocument();
    expect(screen.queryByText("Other")).not.toBeInTheDocument();
  });

  it("renders backend application bundle metadata when provided", () => {
    resetStore({
      audioSources: [applicationSource(2024, "Safari", "com.apple.Safari")],
    });
    render(<AudioSourceSelector />);

    expect(screen.getByText("Safari")).toBeInTheDocument();
    expect(screen.getByText("com.apple.Safari")).toBeInTheDocument();
    expect(screen.getByText("Applications")).toBeInTheDocument();
  });

  it("shows backend-provided default capture format metadata when available", () => {
    const source = deviceSource("device:mic", "Studio Mic", "mic-1", "Input");
    source.default_format = {
      sample_rate: 48000,
      channels: 2,
      sample_format: "F32",
    };
    source.supported_formats = [source.default_format];
    resetStore({ audioSources: [source] });
    render(<AudioSourceSelector />);
    expect(screen.getByText("48 kHz / 2ch")).toBeInTheDocument();
  });

  it("disables unsupported source rows from backend capability metadata", () => {
    const toggleSourceId = vi.fn();
    const source = deviceSource("device:virtual", "Virtual Device", "dev-1");
    source.capabilities = {
      backend_name: "TestBackend",
      capture_supported: false,
      supports_system_capture: true,
      supports_application_capture: true,
      supports_process_tree_capture: true,
      supports_device_selection: false,
      supports_device_change_notifications: true,
      unsupported_reason:
        "Device selection is not supported by the TestBackend backend",
    };
    resetStore({ audioSources: [source], toggleSourceId });
    render(<AudioSourceSelector />);

    const row = screen
      .getByText("Virtual Device")
      .closest('[role="checkbox"]') as HTMLElement;
    expect(row).toHaveAttribute("aria-disabled", "true");
    expect(row).toHaveAttribute(
      "title",
      "Device selection is not supported by the TestBackend backend",
    );
    expect(within(row).getByText("Unsupported")).toBeInTheDocument();

    fireEvent.click(row);
    fireEvent.keyDown(row, { key: "Enter" });
    expect(toggleSourceId).not.toHaveBeenCalled();
  });

  it("associates the disabled-source reason with the row via aria-describedby for assistive tech", () => {
    const source = deviceSource("device:virtual", "Virtual Device", "dev-1");
    source.capabilities = {
      backend_name: "TestBackend",
      capture_supported: false,
      supports_system_capture: true,
      supports_application_capture: true,
      supports_process_tree_capture: true,
      supports_device_selection: false,
      supports_device_change_notifications: true,
      unsupported_reason:
        "Device selection is not supported by the TestBackend backend",
    };
    resetStore({ audioSources: [source] });
    render(<AudioSourceSelector />);

    const row = screen
      .getByText("Virtual Device")
      .closest('[role="checkbox"]') as HTMLElement;
    expect(row).toHaveAttribute("aria-disabled", "true");
    const describedBy = row.getAttribute("aria-describedby");
    expect(describedBy).toBeTruthy();
    // The referenced element is visually hidden but carries the reason text so
    // a screen reader announces why the row is disabled.
    const reasonNode = document.getElementById(describedBy as string);
    expect(reasonNode).not.toBeNull();
    expect(reasonNode).toHaveClass("sr-only");
    expect(reasonNode).toHaveTextContent(
      /Device selection is not supported by the TestBackend backend/i,
    );
  });

  it("associates disabled process controls with their reason via aria-describedby", () => {
    const source = systemSource();
    source.capabilities = {
      backend_name: "FixtureBackend",
      capture_supported: true,
      supports_system_capture: true,
      supports_application_capture: false,
      supports_process_tree_capture: false,
      supports_device_selection: true,
      supports_device_change_notifications: true,
    };
    resetStore({
      audioSources: [source],
      processes: [proc(1234, "code.exe")],
    });
    render(<AudioSourceSelector />);
    fireEvent.click(screen.getByRole("tab", { name: /all processes/i }));

    const processButton = screen.getByRole("button", { name: "Process" });
    expect(processButton).toBeDisabled();
    const describedBy = processButton.getAttribute("aria-describedby");
    expect(describedBy).toBeTruthy();
    const reasonNode = document.getElementById(describedBy as string);
    expect(reasonNode).not.toBeNull();
    expect(reasonNode).toHaveClass("sr-only");
    expect(reasonNode).toHaveTextContent(
      /Application capture is not supported by FixtureBackend/i,
    );
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

    // "Process" button captures app:<pid>; "Tree" captures tree:<pid>.
    const processButton = screen.getByRole("button", { name: "Process" });
    const treeButton = screen.getByRole("button", { name: "Tree" });
    expect(processButton).toHaveAttribute("aria-pressed", "false");
    expect(treeButton).toHaveAttribute("aria-pressed", "false");

    fireEvent.click(processButton);
    expect(toggleSourceId).toHaveBeenCalledWith("app:1234");
    fireEvent.click(treeButton);
    expect(toggleSourceId).toHaveBeenCalledWith("tree:1234");
  });

  it("row-click toggles the active process-tree mode instead of switching back to Process", () => {
    const toggleSourceId = vi.fn();
    resetStore({
      audioSources: [systemSource()],
      processes: [proc(1234, "code.exe")],
      selectedSourceIds: ["tree:1234"],
      toggleSourceId,
    });
    render(<AudioSourceSelector />);
    fireEvent.click(screen.getByRole("tab", { name: /all processes/i }));

    const row = screen
      .getByText("code.exe")
      .closest('[role="checkbox"]') as HTMLElement;
    expect(row).toHaveAttribute("aria-checked", "true");
    expect(row).toHaveAttribute("title", "Process tree: code.exe");
    expect(screen.getByRole("button", { name: "Process" })).toHaveAttribute(
      "aria-pressed",
      "false",
    );
    expect(screen.getByRole("button", { name: "Tree" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );

    fireEvent.click(row);
    expect(toggleSourceId).toHaveBeenCalledWith("tree:1234");
  });

  it("disables process controls when backend capabilities do not support process capture", () => {
    const toggleSourceId = vi.fn();
    const source = systemSource();
    source.capabilities = {
      backend_name: "FixtureBackend",
      capture_supported: true,
      supports_system_capture: true,
      supports_application_capture: false,
      supports_process_tree_capture: false,
      supports_device_selection: true,
      supports_device_change_notifications: true,
    };
    resetStore({
      audioSources: [source],
      processes: [proc(1234, "code.exe")],
      toggleSourceId,
    });
    render(<AudioSourceSelector />);
    fireEvent.click(screen.getByRole("tab", { name: /all processes/i }));

    const row = screen
      .getByText("code.exe")
      .closest('[role="checkbox"]') as HTMLElement;
    expect(row).toHaveAttribute("aria-disabled", "true");
    expect(row).toHaveAttribute(
      "title",
      "Application capture is not supported by FixtureBackend",
    );

    const processButton = screen.getByRole("button", { name: "Process" });
    const treeButton = screen.getByRole("button", { name: "Tree" });
    expect(processButton).toBeDisabled();
    expect(processButton).toHaveAttribute(
      "title",
      "Application capture is not supported by FixtureBackend",
    );
    expect(treeButton).toBeDisabled();
    expect(treeButton).toHaveAttribute(
      "title",
      "Process tree capture is not supported by FixtureBackend",
    );

    fireEvent.click(row);
    fireEvent.click(processButton);
    fireEvent.click(treeButton);
    expect(toggleSourceId).not.toHaveBeenCalled();
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

  it("gives the search input an accessible name and a localized placeholder (seed 4f2e / WCAG 4.1.2)", () => {
    resetStore({ audioSources: [systemSource()] });
    render(<AudioSourceSelector />);
    // aria-label makes the previously label-less search input nameable by SR;
    // the placeholder is now t()-driven rather than hardcoded English.
    const search = screen.getByRole("textbox", {
      name: /search audio sources and processes/i,
    });
    expect(search).toHaveAttribute(
      "placeholder",
      "Search sources & processes...",
    );

    // The clear-search control also carries a translated accessible label.
    fireEvent.change(search, { target: { value: "chrome" } });
    expect(
      screen.getByRole("button", { name: /clear search/i }),
    ).toBeInTheDocument();
  });
});
