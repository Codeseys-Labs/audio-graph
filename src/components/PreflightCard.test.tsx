import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../i18n";
import { useAudioGraphStore } from "../store";
import type { AppSettings, AudioSourceInfo, ProcessInfo } from "../types";
import PreflightCard from "./PreflightCard";
import StorageBanner, { publishStorageFull } from "./StorageBanner";

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

function processInfo(overrides: Partial<ProcessInfo> = {}): ProcessInfo {
  return {
    pid: 4242,
    name: "zoom",
    exe_path: null,
    ...overrides,
  };
}

type StoreState = ReturnType<typeof useAudioGraphStore.getState>;

const actions = {
  startCaptureAndTranscribe: vi.fn(async () => {}),
  openSettings: vi.fn(),
  startGemini: vi.fn(async () => {}),
  stopGemini: vi.fn(async () => {}),
  setConversationMode: vi.fn(),
  setConverseEngine: vi.fn(),
};

function resetStore(overrides: Partial<StoreState> = {}) {
  for (const fn of Object.values(actions)) fn.mockClear();
  useAudioGraphStore.setState({
    isCapturing: false,
    isGeminiActive: false,
    selectedSourceIds: [],
    audioSources: [],
    processes: [],
    settings: makeSettings(),
    credentialPresence: [],
    modelStatus: null,
    conversationMode: "notes",
    converseEngine: "pipelined",
    converseRealtimeAgentProvider: "gemini",
    ...actions,
    ...overrides,
  });
}

/**
 * `StorageBanner.tsx`'s "is storage currently full" state is module-level
 * (by design — see that file's doc comment: a fresh subscriber must learn
 * the CURRENT state on mount, not just future publishes). Tests in this
 * file that publish a storage-full event must clear it back to null
 * afterwards through the SAME real dismiss path production code uses (no
 * test-only reset API), so a later test in this file always starts from a
 * clean slate regardless of declaration order.
 */
function clearSharedStorageFullState() {
  const { queryByTestId, getByRole, unmount } = render(<StorageBanner />);
  if (queryByTestId("storage-banner")) {
    fireEvent.click(getByRole("button", { name: /dismiss/i }));
  }
  unmount();
}

describe("PreflightCard", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    resetStore();
  });

  afterEach(() => {
    clearSharedStorageFullState();
  });

  // ── Sources row ───────────────────────────────────────────────────────
  it("fails the Sources row with no selection, and offers a fix action", () => {
    render(<PreflightCard />);
    expect(screen.getByText(/no source selected yet/i)).toBeInTheDocument();
    expect(screen.getByTestId("preflight-row-sources-status")).toHaveAttribute(
      "data-tone",
      "warning",
    );
  });

  it("resolves the single selected source's name (fold of seed audio-graph-4a22)", () => {
    resetStore({
      selectedSourceIds: ["zoom-app"],
      audioSources: [
        source({
          id: "zoom-app",
          name: "Zoom",
          source_type: { type: "Application", pid: 100, app_name: "zoom" },
        }),
      ],
    });
    render(<PreflightCard />);
    expect(screen.getByText("Zoom application")).toBeInTheDocument();
    expect(screen.getByTestId("preflight-row-sources-status")).toHaveAttribute(
      "data-tone",
      "success",
    );
  });

  it("falls back to a process-tree pid/name lookup when the source list hasn't been fetched yet", () => {
    resetStore({
      selectedSourceIds: ["tree:4242"],
      audioSources: [],
      processes: [processInfo({ pid: 4242, name: "zoom" })],
    });
    render(<PreflightCard />);
    expect(screen.getByText("zoom process tree")).toBeInTheDocument();
  });

  it("summarizes multiple selected sources with a count AND every resolved name (review fix: the fold was half-restored — names were computed then discarded for multi-select)", () => {
    resetStore({
      selectedSourceIds: ["system-default", "device:mic-1"],
      audioSources: [
        source({ id: "system-default" }),
        source({
          id: "device:mic-1",
          name: "Built-in Mic",
          source_type: { type: "Device", device_id: "mic-1" },
        }),
      ],
    });
    render(<PreflightCard />);
    const detail = screen.getByText(/2 sources/i);
    expect(detail.textContent).toContain("System Audio system");
    expect(detail.textContent).toContain("Built-in Mic device");
    // Untruncated join available via title for the (visually truncated) row.
    expect(detail).toHaveAttribute(
      "title",
      "System Audio system, Built-in Mic device",
    );
  });

  it("focuses the audio source search input from the Sources row's fix action", () => {
    render(
      <>
        <input id="audio-source-search" aria-label="search" />
        <PreflightCard />
      </>,
    );
    fireEvent.click(screen.getByRole("button", { name: /choose sources/i }));
    expect(document.getElementById("audio-source-search")).toHaveFocus();
  });

  // ── Route row ─────────────────────────────────────────────────────────
  it("labels the Route row 'planned:' with provider names when a durable route is configured", () => {
    resetStore({
      credentialPresence: [
        {
          key: "deepgram_api_key",
          present: true,
          source: "credentials_yaml",
        },
        {
          key: "openrouter_api_key",
          present: true,
          source: "credentials_yaml",
        },
      ],
    });
    render(<PreflightCard />);
    const detail = screen.getByText(/^planned:/i);
    expect(detail.textContent).toContain("Deepgram");
    expect(detail.textContent).toContain("OpenRouter");
    expect(detail.textContent).not.toMatch(/observed/i);
    expect(screen.getByTestId("preflight-row-route-status")).toHaveAttribute(
      "data-tone",
      "success",
    );
  });

  it("labels the Route row unconfigured (still 'planned', never 'observed') with no credentials", () => {
    resetStore({ credentialPresence: [] });
    render(<PreflightCard />);
    expect(screen.getByText("planned: not configured")).toBeInTheDocument();
    expect(screen.getByTestId("preflight-row-route-status")).toHaveAttribute(
      "data-tone",
      "warning",
    );
  });

  it("opens Settings from the Route row's fix action (the same action the gear icon already uses)", () => {
    render(<PreflightCard />);
    fireEvent.click(
      within(screen.getByTestId("preflight-row-route")).getByRole("button", {
        name: /configure/i,
      }),
    );
    expect(actions.openSettings).toHaveBeenCalledTimes(1);
  });

  // ── Storage row (reuses StorageBanner's data source) ───────────────────
  it("passes the Storage row when no storage-full event has published, with its fix action disabled (nothing to fix)", () => {
    render(<PreflightCard />);
    expect(screen.getByText(/storage is ready/i)).toBeInTheDocument();
    expect(screen.getByTestId("preflight-row-storage-status")).toHaveAttribute(
      "data-tone",
      "success",
    );
    expect(
      within(screen.getByTestId("preflight-row-storage")).getByRole("button", {
        name: /resolve/i,
      }),
    ).toBeDisabled();
  });

  it("fails the Storage row and reflects an ALREADY-published storage-full event on mount (no replay gap)", () => {
    publishStorageFull({
      path: "/tmp/session/transcript.jsonl",
      bytes_written: 0,
      bytes_lost: 4096,
    });
    render(<PreflightCard />);
    expect(screen.getByText(/storage full/i)).toBeInTheDocument();
    expect(screen.getByTestId("preflight-row-storage-status")).toHaveAttribute(
      "data-tone",
      "warning",
    );
  });

  it("focuses the real StorageBanner's Resume button from the Storage row's fix action, without invoking anything itself", () => {
    publishStorageFull({ path: "/tmp/x", bytes_written: 0, bytes_lost: 1 });
    render(
      <>
        <button id="storage-banner-resume" type="button">
          Resume
        </button>
        <PreflightCard />
      </>,
    );
    const fixAction = within(
      screen.getByTestId("preflight-row-storage"),
    ).getByRole("button", { name: /resolve/i });
    expect(fixAction).toBeEnabled();
    fireEvent.click(fixAction);
    expect(document.getElementById("storage-banner-resume")).toHaveFocus();
  });

  // ── Start session button ────────────────────────────────────────────────
  it("disables Start session with no source selected, and calls the SAME startCaptureAndTranscribe action once one is", async () => {
    resetStore({ selectedSourceIds: [] });
    const { rerender } = render(<PreflightCard />);
    expect(
      screen.getByRole("button", { name: /start session/i }),
    ).toBeDisabled();

    resetStore({ selectedSourceIds: ["system-default"] });
    rerender(<PreflightCard />);
    const start = screen.getByRole("button", { name: /start session/i });
    expect(start).toBeEnabled();
    fireEvent.click(start);
    await waitFor(() =>
      expect(actions.startCaptureAndTranscribe).toHaveBeenCalledTimes(1),
    );
  });

  // ── Mode relocation (ConversationModeControl + Gemini toggle) ──────────
  it("renders ConversationModeControl as a preflight choice", () => {
    render(<PreflightCard />);
    expect(
      document.querySelector('[aria-label="Conversation mode"]'),
    ).not.toBeNull();
    expect(screen.getByText(/^notes$/i)).toBeInTheDocument();
    expect(screen.getByText(/^converse$/i)).toBeInTheDocument();
  });

  it("shows the Gemini toggle only in native converse mode, permanently aria-disabled (KNOWN GAP, full statement in this file's doc comment: canGemini requires isCapturing, which is never true while this card is mounted — the button cannot be enabled from this card at all)", () => {
    resetStore({ conversationMode: "notes" });
    const { rerender } = render(<PreflightCard />);
    expect(
      screen.queryByRole("button", { name: /start gemini/i }),
    ).not.toBeInTheDocument();

    resetStore({
      conversationMode: "converse",
      converseEngine: "native",
      selectedSourceIds: ["system-default"],
    });
    rerender(<PreflightCard />);
    const gemini = screen.getByRole("button", { name: /start gemini/i });
    expect(gemini).toHaveAttribute("aria-disabled", "true");
    fireEvent.click(gemini);
    expect(actions.startGemini).not.toHaveBeenCalled();
  });

  // Review fix (major finding: the relocated stop-branch had zero
  // assertions in its new home — `stopGemini` was registered in the action
  // mocks and never asserted). This pins the dormant wiring the doc comment
  // above says "stays end-to-end correct" — it cannot run in production
  // today (the card never mounts while `isGeminiActive` could be true), but
  // if it silently broke, nothing would catch it.
  it("renders an enabled Stop control and calls stopGemini when isGeminiActive is seeded true (dormant wiring — see KNOWN GAP doc comment; unreachable in production today)", () => {
    resetStore({
      isGeminiActive: true,
      conversationMode: "converse",
      converseEngine: "native",
    });
    render(<PreflightCard />);
    const stop = screen.getByRole("button", { name: /stop realtime/i });
    expect(stop).toHaveAttribute("aria-disabled", "false");
    fireEvent.click(stop);
    expect(actions.stopGemini).toHaveBeenCalledTimes(1);
  });
});
