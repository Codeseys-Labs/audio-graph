import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useAudioGraphStore } from "../store";
import type { AppSettings, AudioSourceInfo, ProjectionPatch } from "../types";
import NowStrip from "./NowStrip";

// NowStrip renders <ConversationModeControl/> unconditionally (B20 /
// ADR-0016 — idle AND capturing), so the conversation-mode store fields must
// be populated too. Only `gemini.auth` and `llm_provider` gate the branches
// NowStrip/ConversationModeControl read.
function makeSettings(overrides: Partial<AppSettings> = {}): AppSettings {
  return {
    asr_provider: {
      type: "deepgram",
      model: "nova-3",
      enable_diarization: true,
    },
    tts_provider: { type: "none" },
    speak_aloud: false,
    whisper_model: "ggml-small.en.bin",
    llm_provider: {
      type: "openrouter",
      model: "openai/gpt-4.1-mini",
      base_url: "https://openrouter.ai/api/v1",
      include_usage_in_stream: true,
    },
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

function notesPatch(overrides: Partial<ProjectionPatch> = {}): ProjectionPatch {
  return {
    sequence: 1,
    kind: "notes",
    llm_request_id: "req-1",
    basis: null,
    operations: [],
    confidence: 1,
    provenance: null,
    created_at_ms: Date.now(),
    ...overrides,
  };
}

type StoreState = ReturnType<typeof useAudioGraphStore.getState>;

const actions = {
  startCaptureAndTranscribe: vi.fn(async () => {}),
  stopCapture: vi.fn(async () => {}),
  startGemini: vi.fn(async () => {}),
  stopGemini: vi.fn(async () => {}),
  openSettings: vi.fn(),
  openSessionsBrowser: vi.fn(),
  setSystemDrawerOpen: vi.fn(),
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
    credentialPresence: [],
    modelStatus: null,
    sessionProjectionEvents: [],
    pipelineStatus: {
      capture: { type: "Idle" },
      pipeline: { type: "Idle" },
      asr: { type: "Idle" },
      diarization: { type: "Idle" },
      entity_extraction: { type: "Idle" },
      graph: { type: "Idle" },
    },
    latestAudioConsumerHealth: null,
    persistenceQueueBackpressure: {},
    agentProposals: [],
    conversationMode: "notes",
    converseEngine: "pipelined",
    converseRealtimeAgentProvider: "gemini",
    ...actions,
    ...overrides,
  });
}

describe("NowStrip", () => {
  beforeEach(() => {
    vi.useRealTimers();
    resetStore();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("renders the capture-controls toolbar with the app title", () => {
    render(<NowStrip />);
    expect(
      screen.getByRole("toolbar", { name: /capture controls/i }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("heading", { name: "AudioGraph" }),
    ).toBeInTheDocument();
  });

  // ── Idle strip: "Ready · N sources · route · Start" (plan §R3) ─────────
  it("collapses to Ready + source count + a neutral route chip + Start while idle", () => {
    resetStore({ selectedSourceIds: [] });
    const { container } = render(<NowStrip />);
    expect(screen.getByText(/ready/i)).toBeInTheDocument();
    expect(screen.getByText(/0 sources/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /^start$/i })).toBeDisabled();
    // Both the route chip and the idle System status chip are neutral tone
    // — not one of the saturated tones.
    for (const chip of container.querySelectorAll(".ag-chip")) {
      expect(chip).toHaveAttribute("data-tone", "neutral");
    }
    // B20 / ADR-0016: pipeline controls stay discoverable pre-capture.
    // `ConversationModeControl` renders (demoted, not gated behind
    // `isCapturing`) even while idle — see this file's NAMED TRANSITIONAL
    // STATE doc comment.
    expect(
      container.querySelector('[aria-label="Conversation mode"]'),
    ).not.toBeNull();
  });

  // 50e3's "no regression to diagnostics" half: `SystemDrawer` (per-stage
  // pipeline detail + token usage) must stay reachable even before capture
  // starts — not just from the live composite health chip.
  it("opens SystemDrawer from a neutral-tone chip while idle, without asserting an unobserved health claim", () => {
    resetStore({ selectedSourceIds: [] });
    render(<NowStrip />);
    const idleStatusChip = screen.getByRole("button", {
      name: /open system status/i,
    });
    expect(idleStatusChip).toHaveAttribute("data-tone", "neutral");
    // Deliberately NOT "All systems normal"/healthy copy — nothing has run
    // yet, so the idle entry point must not claim an observed health state
    // (ADR-0030/0034).
    expect(idleStatusChip).not.toHaveTextContent(/all systems normal/i);
    fireEvent.click(idleStatusChip);
    expect(actions.setSystemDrawerOpen).toHaveBeenCalledWith(true);
  });

  it("enables Start once a source is selected and calls the merged startCaptureAndTranscribe on click", async () => {
    resetStore({ selectedSourceIds: ["system-default"] });
    render(<NowStrip />);
    const start = screen.getByRole("button", { name: /^start$/i });
    expect(start).toBeEnabled();
    fireEvent.click(start);
    await waitFor(() =>
      expect(actions.startCaptureAndTranscribe).toHaveBeenCalledTimes(1),
    );
  });

  it("shows a pressed Stop button while capturing and calls stopCapture", async () => {
    resetStore({
      isCapturing: true,
      selectedSourceIds: ["system-default"],
      captureStartTime: Date.now(),
    });
    render(<NowStrip />);
    const stop = screen.getByRole("button", { name: /stop$/i });
    expect(stop).toHaveAttribute("aria-pressed", "true");
    fireEvent.click(stop);
    await waitFor(() => expect(actions.stopCapture).toHaveBeenCalledTimes(1));
  });

  it("renders an elapsed timer that advances each second while capturing", () => {
    vi.useFakeTimers();
    const start = Date.now() - 65_000; // 1:05 ago
    resetStore({
      isCapturing: true,
      selectedSourceIds: ["system-default"],
      captureStartTime: start,
    });
    render(<NowStrip />);
    expect(screen.getByText("01:05")).toBeInTheDocument();
    act(() => {
      vi.advanceTimersByTime(1000);
    });
    expect(screen.getByText("01:06")).toBeInTheDocument();
    expect(screen.queryByText("01:05")).not.toBeInTheDocument();
  });

  // ── Durability readout ──────────────────────────────────────────────────
  it("omits the durability readout until a notes patch has landed this session", () => {
    resetStore({
      isCapturing: true,
      selectedSourceIds: ["system-default"],
      captureStartTime: Date.now(),
      sessionProjectionEvents: [],
    });
    render(<NowStrip />);
    expect(screen.queryByText(/notes saved/i)).toBeNull();
  });

  it("shows 'notes saved · Ns ago' from the most recent notes projection patch", () => {
    vi.useFakeTimers();
    const now = Date.now();
    vi.setSystemTime(now);
    resetStore({
      isCapturing: true,
      selectedSourceIds: ["system-default"],
      captureStartTime: now - 10_000,
      sessionProjectionEvents: [
        notesPatch({ sequence: 1, created_at_ms: now - 8_000 }),
        // A later "graph" patch must not be mistaken for a notes save.
        {
          ...notesPatch({ sequence: 2, created_at_ms: now - 1_000 }),
          kind: "graph",
        },
      ],
    });
    render(<NowStrip />);
    expect(screen.getByText(/notes saved.*8s ago/i)).toBeInTheDocument();
  });

  // ── Planned-route chip (never "observed" — ADR-0030/0034) ───────────────
  it("labels the route chip 'planned:' with the configured ASR/LLM provider names", () => {
    resetStore({
      selectedSourceIds: ["system-default"],
      credentialPresence: [
        { key: "deepgram_api_key", present: true, source: "credentials_yaml" },
        {
          key: "openrouter_api_key",
          present: true,
          source: "credentials_yaml",
        },
      ],
    });
    const { container } = render(<NowStrip />);
    const chip = container.querySelector(".ag-chip") as HTMLElement;
    expect(chip.textContent).toMatch(/^planned:/i);
    expect(chip.textContent).not.toMatch(/observed/i);
    expect(chip.textContent).toContain("Deepgram");
    expect(chip.textContent).toContain("OpenRouter");
  });

  it("labels the route chip as unconfigured (still 'planned', never 'observed') with no credentials", () => {
    resetStore({
      selectedSourceIds: ["system-default"],
      credentialPresence: [],
    });
    const { container } = render(<NowStrip />);
    const chip = container.querySelector(".ag-chip") as HTMLElement;
    // The exact `nowStrip.routeUnconfigured` copy, not just a loose
    // "contains 'planned'" check — that alone can't distinguish the
    // unconfigured label from the configured "planned: {route}" one above,
    // so the real teeth for "configured vs. unconfigured" live in
    // App.test's shared-function ExpressSetup tests, not here.
    expect(chip.textContent).toBe("planned: not configured");
    expect(chip.textContent).not.toMatch(/observed/i);
  });

  // ── Composite health chip (50e3 fold) ────────────────────────────────────
  it("opens the System drawer from the health chip while capturing", () => {
    resetStore({
      isCapturing: true,
      selectedSourceIds: ["system-default"],
      captureStartTime: Date.now(),
    });
    render(<NowStrip />);
    const healthChip = screen.getByRole("button", {
      name: /open system status/i,
    });
    fireEvent.click(healthChip);
    expect(actions.setSystemDrawerOpen).toHaveBeenCalledWith(true);
  });

  it("shows the degraded health tone when a persistence writer queue is dropping events", () => {
    resetStore({
      isCapturing: true,
      selectedSourceIds: ["system-default"],
      captureStartTime: Date.now(),
      persistenceQueueBackpressure: {
        transcript_event: {
          writer: "transcript_event",
          is_backpressured: true,
          queue_capacity: 2048,
          dropped_count: 3,
        },
      },
    });
    const { container } = render(<NowStrip />);
    const healthChip = screen.getByRole("button", {
      name: /open system status/i,
    });
    expect(healthChip).toHaveAttribute("data-tone", "warning");
    expect(container.querySelectorAll('[data-tone="danger"]')).toHaveLength(0);
  });

  it("shows the error health tone when a pipeline stage reports Error", () => {
    resetStore({
      isCapturing: true,
      selectedSourceIds: ["system-default"],
      captureStartTime: Date.now(),
      pipelineStatus: {
        capture: { type: "Idle" },
        pipeline: { type: "Idle" },
        asr: { type: "Idle" },
        diarization: { type: "Idle" },
        entity_extraction: { type: "Idle" },
        graph: { type: "Error", message: "boom" },
      },
    });
    render(<NowStrip />);
    expect(
      screen.getByRole("button", { name: /open system status/i }),
    ).toHaveAttribute("data-tone", "danger");
  });

  // ── Gemini (demoted, NAMED TRANSITIONAL STATE) ───────────────────────────
  it("does not render the Gemini control outside native converse mode", () => {
    resetStore({
      isCapturing: true,
      selectedSourceIds: ["system-default"],
      captureStartTime: Date.now(),
      conversationMode: "notes",
    });
    render(<NowStrip />);
    expect(
      screen.queryByRole("button", { name: /start gemini/i }),
    ).not.toBeInTheDocument();
  });

  it("renders the Gemini control pre-capture too (B20 / ADR-0016), aria-disabled with a start-capture reason", () => {
    resetStore({
      isCapturing: false,
      selectedSourceIds: ["system-default"],
      conversationMode: "converse",
      converseEngine: "native",
    });
    render(<NowStrip />);
    const gemini = screen.getByRole("button", { name: /start gemini/i });
    expect(gemini).toBeInTheDocument();
    expect(gemini).toHaveAttribute("aria-disabled", "true");
    fireEvent.click(gemini);
    expect(actions.startGemini).not.toHaveBeenCalled();
  });

  it("keeps a persisted deferred native control visible but blocks a new start", () => {
    resetStore({
      isCapturing: true,
      selectedSourceIds: ["system-default"],
      captureStartTime: Date.now(),
      conversationMode: "converse",
      converseEngine: "native",
    });
    render(<NowStrip />);
    const gemini = screen.getByRole("button", { name: /start gemini/i });
    expect(gemini).toHaveAttribute("aria-disabled", "true");
    fireEvent.click(gemini);
    expect(actions.startGemini).not.toHaveBeenCalled();
  });

  it("allows stop for an already-running deferred native session", async () => {
    resetStore({
      isCapturing: true,
      isGeminiActive: true,
      selectedSourceIds: ["system-default"],
      captureStartTime: Date.now(),
      conversationMode: "converse",
      converseEngine: "native",
    });
    render(<NowStrip />);
    const stop = screen.getByRole("button", { name: /stop realtime session/i });
    expect(stop).toHaveAttribute("aria-disabled", "false");
    fireEvent.click(stop);
    await waitFor(() => expect(actions.stopGemini).toHaveBeenCalledTimes(1));
  });

  it("wires the sessions and settings launchers to their store actions", () => {
    render(<NowStrip />);
    fireEvent.click(screen.getByRole("button", { name: /sessions/i }));
    fireEvent.click(screen.getByRole("button", { name: /settings/i }));
    expect(actions.openSessionsBrowser).toHaveBeenCalledTimes(1);
    expect(actions.openSettings).toHaveBeenCalledTimes(1);
  });

  it("does not render the retired Agent/Tokens toggle buttons or the Transcribe control", () => {
    resetStore({
      isCapturing: true,
      selectedSourceIds: ["system-default"],
      captureStartTime: Date.now(),
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
    render(<NowStrip />);
    expect(
      screen.queryByRole("button", { name: /toggle agent proposals/i }),
    ).toBeNull();
    expect(
      screen.queryByRole("button", { name: /toggle token usage/i }),
    ).toBeNull();
    expect(
      screen.queryByRole("button", { name: /start transcription/i }),
    ).toBeNull();
  });

  it("uses singular grammar for exactly one selected source (i18next _one/_other plural forms)", () => {
    resetStore({
      selectedSourceIds: ["system-default"],
      audioSources: [source({ id: "system-default", name: "System Audio" })],
    });
    render(<NowStrip />);
    expect(screen.getByText(/1 source\b/i)).toBeInTheDocument();
    expect(screen.queryByText(/1 sources/i)).not.toBeInTheDocument();
  });
});
