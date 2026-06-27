import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useAudioGraphStore } from "../store";
import type { AppSettings, AudioSourceInfo } from "../types";
import ControlBar from "./ControlBar";

// ControlBar renders <ConversationModeControl/>, so the conversation-mode
// store fields must be populated too. Only `gemini.auth` and `llm_provider`
// gate the branches ControlBar/ConversationModeControl read.
function makeSettings(overrides: Partial<AppSettings> = {}): AppSettings {
  return {
    asr_provider: { type: "local_whisper" },
    tts_provider: { type: "none" },
    speak_aloud: false,
    whisper_model: "ggml-small.en.bin",
    llm_provider: { type: "local_llama" },
    llm_api_config: null,
    audio_settings: { sample_rate: 48000, channels: 1 },
    gemini: {
      auth: { type: "api_key", api_key: "key" },
      model: "gemini-3.1-flash-live-preview",
    },
    log_level: "info",
    ...overrides,
  };
}

function source(overrides: Partial<AudioSourceInfo> = {}): AudioSourceInfo {
  return {
    id: "system-default",
    name: "System Audio",
    source_type: { type: "SystemDefault" },
    is_active: false,
    ...overrides,
  };
}

type StoreState = ReturnType<typeof useAudioGraphStore.getState>;

const actions = {
  startCapture: vi.fn(async () => {}),
  stopCapture: vi.fn(async () => {}),
  startTranscribe: vi.fn(async () => {}),
  stopTranscribe: vi.fn(async () => {}),
  startGemini: vi.fn(async () => {}),
  stopGemini: vi.fn(async () => {}),
  openSettings: vi.fn(),
  openSessionsBrowser: vi.fn(),
  toggleAgentOverlay: vi.fn(),
  toggleTokenOverlay: vi.fn(),
  setConversationMode: vi.fn(),
  setConverseEngine: vi.fn(),
};

function resetStore(overrides: Partial<StoreState> = {}) {
  for (const fn of Object.values(actions)) fn.mockClear();
  useAudioGraphStore.setState({
    isCapturing: false,
    isTranscribing: false,
    isGeminiActive: false,
    selectedSourceIds: [],
    audioSources: [],
    processes: [],
    captureStartTime: null,
    backpressuredSources: [],
    settings: makeSettings(),
    agentProposals: [],
    conversationMode: "notes",
    converseEngine: "pipelined",
    ...actions,
    ...overrides,
  });
}

describe("ControlBar", () => {
  beforeEach(() => {
    vi.useRealTimers();
    resetStore();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("renders the capture-controls toolbar with the app title", () => {
    render(<ControlBar />);
    expect(
      screen.getByRole("toolbar", { name: /capture controls/i }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("heading", { name: "AudioGraph" }),
    ).toBeInTheDocument();
  });

  it("prompts to select sources and disables Start when none are selected", () => {
    resetStore({ selectedSourceIds: [] });
    render(<ControlBar />);
    expect(
      screen.getByText(/select audio sources to begin/i),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /^start$/i })).toBeDisabled();
  });

  it("enables Start once a source is selected and calls startCapture on click", async () => {
    resetStore({ selectedSourceIds: ["system-default"] });
    render(<ControlBar />);
    const start = screen.getByRole("button", { name: /^start$/i });
    expect(start).toBeEnabled();
    fireEvent.click(start);
    await waitFor(() => expect(actions.startCapture).toHaveBeenCalledTimes(1));
  });

  it("shows a pressed Stop button while capturing and calls stopCapture", async () => {
    resetStore({
      isCapturing: true,
      selectedSourceIds: ["system-default"],
      captureStartTime: Date.now(),
    });
    render(<ControlBar />);
    const stop = screen.getByRole("button", { name: /stop$/i });
    expect(stop).toHaveAttribute("aria-pressed", "true");
    fireEvent.click(stop);
    await waitFor(() => expect(actions.stopCapture).toHaveBeenCalledTimes(1));
  });

  it("surfaces the Transcribe control pre-capture as aria-disabled (B20 discoverability)", () => {
    resetStore({ selectedSourceIds: ["system-default"] });
    render(<ControlBar />);
    // Pipeline controls are now rendered always, so converse/Gemini and the
    // transcribe path are discoverable before the user commits to Start.
    const transcribe = screen.getByRole("button", {
      name: /start transcription/i,
    });
    expect(transcribe).toBeInTheDocument();
    // Disabled via aria-disabled (focusable + SR-located), NOT native disabled.
    expect(transcribe).toHaveAttribute("aria-disabled", "true");
    expect(transcribe).not.toBeDisabled();
    // The reason is wired for screen readers via aria-describedby.
    expect(transcribe).toHaveAttribute(
      "aria-describedby",
      "control-bar-transcribe-reason",
    );
    expect(
      document.getElementById("control-bar-transcribe-reason"),
    ).toHaveTextContent(/start capture to enable transcription/i);
  });

  it("no-ops the Transcribe click while aria-disabled (pre-capture)", () => {
    resetStore({ selectedSourceIds: ["system-default"] });
    render(<ControlBar />);
    fireEvent.click(
      screen.getByRole("button", { name: /start transcription/i }),
    );
    expect(actions.startTranscribe).not.toHaveBeenCalled();
  });

  it("reveals the Transcribe control while capturing and calls startTranscribe", async () => {
    resetStore({
      isCapturing: true,
      selectedSourceIds: ["system-default"],
      captureStartTime: Date.now(),
    });
    render(<ControlBar />);
    const transcribe = screen.getByRole("button", {
      name: /start transcription/i,
    });
    expect(transcribe).toBeEnabled();
    fireEvent.click(transcribe);
    await waitFor(() =>
      expect(actions.startTranscribe).toHaveBeenCalledTimes(1),
    );
  });

  it("calls stopTranscribe when transcription is already running", async () => {
    resetStore({
      isCapturing: true,
      isTranscribing: true,
      selectedSourceIds: ["system-default"],
      captureStartTime: Date.now(),
    });
    render(<ControlBar />);
    fireEvent.click(
      screen.getByRole("button", { name: /stop transcription/i }),
    );
    await waitFor(() =>
      expect(actions.stopTranscribe).toHaveBeenCalledTimes(1),
    );
  });

  it("does not render the Gemini control outside native converse mode", () => {
    resetStore({
      isCapturing: true,
      selectedSourceIds: ["system-default"],
      captureStartTime: Date.now(),
      conversationMode: "notes",
    });
    render(<ControlBar />);
    // In notes mode the Gemini control is not relevant, so it's absent from
    // the DOM entirely (no more `hidden` attribute on an always-present node).
    expect(
      screen.queryByRole("button", { name: /start gemini/i }),
    ).not.toBeInTheDocument();
  });

  it("shows the Gemini control in native converse mode and calls startGemini", async () => {
    resetStore({
      isCapturing: true,
      selectedSourceIds: ["system-default"],
      captureStartTime: Date.now(),
      conversationMode: "converse",
      converseEngine: "native",
    });
    render(<ControlBar />);
    const gemini = screen.getByRole("button", { name: /start gemini/i });
    expect(gemini).not.toHaveAttribute("hidden");
    expect(gemini).toHaveAttribute("aria-disabled", "false");
    fireEvent.click(gemini);
    await waitFor(() => expect(actions.startGemini).toHaveBeenCalledTimes(1));
  });

  it("surfaces the Gemini control pre-capture as aria-disabled with a key (B20)", () => {
    resetStore({
      isCapturing: false,
      selectedSourceIds: ["system-default"],
      conversationMode: "converse",
      converseEngine: "native",
    });
    render(<ControlBar />);
    const gemini = screen.getByRole("button", { name: /start gemini/i });
    // Discoverable before capture but aria-disabled (focusable, not native).
    expect(gemini).toHaveAttribute("aria-disabled", "true");
    expect(gemini).not.toBeDisabled();
    expect(
      document.getElementById("control-bar-gemini-reason"),
    ).toHaveTextContent(/start capture to enable gemini/i);
    // Clicking while aria-disabled is a no-op.
    fireEvent.click(gemini);
    expect(actions.startGemini).not.toHaveBeenCalled();
  });

  it("aria-disables Gemini and explains why when no Gemini key is configured", () => {
    resetStore({
      isCapturing: true,
      selectedSourceIds: ["system-default"],
      captureStartTime: Date.now(),
      conversationMode: "converse",
      converseEngine: "native",
      settings: makeSettings({
        gemini: {
          auth: { type: "none" } as unknown as AppSettings["gemini"]["auth"],
          model: "gemini-3.1-flash-live-preview",
        },
      }),
    });
    render(<ControlBar />);
    const gemini = screen.getByRole("button", { name: /start gemini/i });
    expect(gemini).toHaveAttribute("aria-disabled", "true");
    expect(gemini).not.toBeDisabled();
    expect(
      document.getElementById("control-bar-gemini-reason"),
    ).toHaveTextContent(/configure gemini in settings/i);
  });

  it("renders a backpressure status pill when a source is dropping chunks", () => {
    resetStore({
      isCapturing: true,
      selectedSourceIds: ["system-default"],
      captureStartTime: Date.now(),
      backpressuredSources: ["system-default"],
    });
    render(<ControlBar />);
    expect(screen.getByText(/backpressure/i)).toBeInTheDocument();
  });

  it("shows a single resolved source label when one source is selected", () => {
    resetStore({
      selectedSourceIds: ["system-default"],
      audioSources: [source({ id: "system-default", name: "System Audio" })],
    });
    render(<ControlBar />);
    expect(screen.getByText(/System Audio system/i)).toBeInTheDocument();
  });

  it("summarizes the count when multiple sources are selected", () => {
    resetStore({
      selectedSourceIds: ["a", "b"],
      audioSources: [
        source({ id: "a", name: "A" }),
        source({ id: "b", name: "B" }),
      ],
    });
    render(<ControlBar />);
    expect(screen.getByText(/2 sources selected/i)).toBeInTheDocument();
  });

  it("renders an agent-proposals badge with the pending count", () => {
    resetStore({
      agentProposals: [
        {
          id: "p1",
          source_segment_id: "s1",
          source_id: "system-default",
          kind: "note",
          title: "t",
          body: "b",
          confidence: 0.5,
          created_at_ms: 1,
        },
      ],
    });
    render(<ControlBar />);
    const agentBtn = screen.getByRole("button", {
      name: /toggle agent proposals/i,
    });
    expect(agentBtn).toHaveTextContent("1");
    fireEvent.click(agentBtn);
    expect(actions.toggleAgentOverlay).toHaveBeenCalledTimes(1);
  });

  it("wires the token, sessions, and settings launchers to their store actions", () => {
    render(<ControlBar />);
    fireEvent.click(
      screen.getByRole("button", { name: /toggle token usage/i }),
    );
    fireEvent.click(screen.getByRole("button", { name: /sessions/i }));
    fireEvent.click(screen.getByRole("button", { name: /settings/i }));
    expect(actions.toggleTokenOverlay).toHaveBeenCalledTimes(1);
    expect(actions.openSessionsBrowser).toHaveBeenCalledTimes(1);
    expect(actions.openSettings).toHaveBeenCalledTimes(1);
  });

  it("renders an elapsed timer that advances each second while capturing", () => {
    vi.useFakeTimers();
    const start = Date.now() - 65_000; // 1:05 ago
    resetStore({
      isCapturing: true,
      selectedSourceIds: ["system-default"],
      captureStartTime: start,
    });
    render(<ControlBar />);
    // The timer ticks immediately on mount.
    expect(screen.getByText("01:05")).toBeInTheDocument();
    // Advance the fake clock past the setInterval boundary and assert the tick
    // actually fires. Without this, a broken/removed setInterval would still
    // pass on the mount-time render alone. act() flushes the state update.
    act(() => {
      vi.advanceTimersByTime(1000);
    });
    expect(screen.getByText("01:06")).toBeInTheDocument();
    expect(screen.queryByText("01:05")).not.toBeInTheDocument();
  });
});
