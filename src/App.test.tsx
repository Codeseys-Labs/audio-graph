import { invoke } from "@tauri-apps/api/core";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import i18n from "./i18n";
import { useAudioGraphStore } from "./store";

// App mounts several heavy/async children (the force-graph viewer, settings &
// sessions modals) that are irrelevant to the B20 hand-off orchestration under
// test. Stub them so the render stays synchronous and dependency-light.
vi.mock("./components/KnowledgeGraphViewer", () => ({
  default: () => <div data-testid="graph-stub" />,
}));
vi.mock("./components/SettingsPage", () => ({
  default: () => <div data-testid="settings-stub" />,
}));
vi.mock("./components/SessionsBrowser", () => ({
  default: () => <div data-testid="sessions-stub" />,
}));
// Panel children that read store slices unrelated to the hand-off flow are
// stubbed so the test stays focused on App's onboarding orchestration and
// doesn't have to seed every panel's full store shape.
vi.mock("./components/AudioSourceSelector", () => ({
  default: () => <div data-testid="sources-stub" />,
}));
vi.mock("./components/SpeakerPanel", () => ({
  default: () => <div data-testid="speakers-stub" />,
}));
vi.mock("./components/LiveTranscript", () => ({
  default: () => <div data-testid="transcript-stub" />,
}));
vi.mock("./components/ChatSidebar", () => ({
  default: () => <div data-testid="chat-stub" />,
}));
vi.mock("./components/TokenUsagePanel", () => ({
  default: () => <div data-testid="tokens-stub" />,
}));
vi.mock("./components/NotesPanel", () => ({
  default: () => <div data-testid="notes-stub" />,
}));
vi.mock("./components/PipelineStatusBar", () => ({
  default: () => <div data-testid="pipeline-stub" />,
}));
vi.mock("./components/ProjectionRuntimeStatusPanel", () => ({
  default: () => <div data-testid="projection-runtime-stub" />,
}));
vi.mock("./components/AgentProposalsPanel", () => ({
  default: () => <div data-testid="agent-stub" />,
}));
vi.mock("./components/ControlBar", () => ({
  default: () => <div data-testid="controlbar-stub" />,
}));
// A minimal ExpressSetup stub: a single "Skip" button that fires onDismiss,
// letting us drive the dismissal → hand-off flow deterministically without
// the real wizard's async credential plumbing.
vi.mock("./components/ExpressSetup", () => ({
  default: ({
    onDismiss,
    onPreviewSampleSession,
  }: {
    onDismiss: () => void;
    onPreviewSampleSession: () => void;
  }) => (
    <div role="dialog" aria-label="Quick Setup">
      <button type="button" onClick={onPreviewSampleSession}>
        Preview sample session
      </button>
      <button type="button" onClick={onDismiss}>
        Skip
      </button>
    </div>
  ),
}));

const mockedInvoke = vi.mocked(invoke);

import { ONBOARDING_HANDOFF_SEEN_KEY } from "./constants/storageKeys";
import type { AppSettings, CredentialPresence, ModelStatus } from "./types";

const HANDOFF_KEY = ONBOARDING_HANDOFF_SEEN_KEY;

function credentialPresence(...keys: string[]): CredentialPresence[] {
  return keys.map((key) => ({
    key,
    present: true,
    source: "credentials_yaml",
  }));
}

function startupSettings(overrides: Partial<AppSettings> = {}): AppSettings {
  return {
    asr_provider: {
      type: "deepgram",
      model: "nova-3",
      enable_diarization: true,
    },
    whisper_model: "ggml-small.en.bin",
    llm_provider: {
      type: "openrouter",
      model: "openai/gpt-4.1-mini",
      base_url: "https://openrouter.ai/api/v1",
      include_usage_in_stream: true,
    },
    llm_api_config: null,
    audio_settings: { sample_rate: 48_000, channels: 1 },
    gemini: {
      auth: { type: "api_key" },
      model: "gemini-2.0-flash-live-001",
    },
    tts_provider: { type: "none" },
    speak_aloud: false,
    log_level: "info",
    ...overrides,
  };
}

const readyModels: ModelStatus = {
  whisper: "Ready",
  llm: "Ready",
  sortformer: "Ready",
};

function mockStartupProbe(
  keys: readonly string[],
  settings: AppSettings = startupSettings(),
  modelStatus: ModelStatus = readyModels,
  awsProfiles: readonly string[] = [],
) {
  mockedInvoke.mockImplementation(async (cmd: string) => {
    if (cmd === "load_credential_cmd") {
      throw new Error(
        "load_credential_cmd should not be invoked by frontend tests; use load_credential_presence_cmd and provider readiness instead.",
      );
    }
    if (cmd === "load_credential_presence_cmd") {
      return credentialPresence(...keys);
    }
    if (cmd === "load_settings_cmd") return settings;
    if (cmd === "get_model_status") return modelStatus;
    if (cmd === "list_aws_profiles") return [...awsProfiles];
    return undefined;
  });
}

function mockCredentialPresence(...keys: string[]) {
  mockStartupProbe(keys);
}

async function waitForStartupProbeToSettle() {
  await waitFor(() =>
    expect(
      document.querySelector('[data-onboarding-probe="settled"]'),
    ).toBeInTheDocument(),
  );
}

function expectNoPlaintextCredentialLoadback() {
  expect(mockedInvoke.mock.calls.map(([cmd]) => cmd)).not.toContain(
    "load_credential_cmd",
  );
}

function seedStore() {
  // Provide the minimal store fields the always-mounted chrome reads.
  useAudioGraphStore.setState({
    rightPanelTab: "transcript",
    samplePreviewActive: false,
    loadedSessionId: null,
    transcriptSegments: [],
    materializedNotes: null,
    materializedProjectionGraph: null,
    settingsOpen: false,
    sessionsBrowserOpen: false,
    agentOverlayOpen: false,
    tokenOverlayOpen: false,
    selectedSourceIds: [],
    audioSources: [],
    processes: [],
    isCapturing: false,
    isTranscribing: false,
    isGeminiActive: false,
    settings: null,
    modelStatus: null,
    backpressuredSources: [],
    agentStatus: null,
    agentProposals: [],
    liveAssistCards: [],
    conversationMode: "notes",
    converseEngine: "pipelined",
  });
}

describe("App — post-Express hand-off nudge (B20)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    localStorage.clear();
    mockedInvoke.mockReset();
    // No cloud credential present → App pops Express Setup on mount.
    mockCredentialPresence();
    seedStore();
  });

  afterEach(async () => {
    await i18n.changeLanguage("en");
    localStorage.clear();
  });

  it("shows the hand-off nudge once Express Setup is dismissed", async () => {
    render(<App />);
    // Express Setup appears because no credentials were found.
    const skip = await screen.findByRole("button", { name: /skip/i });
    expectNoPlaintextCredentialLoadback();
    // The hand-off nudge is not shown while the wizard is open.
    expect(screen.queryByText(/here's how to start/i)).not.toBeInTheDocument();

    fireEvent.click(skip);

    await waitFor(() =>
      expect(screen.getByText(/here's how to start/i)).toBeInTheDocument(),
    );
    // It guides toward source → Start.
    expect(screen.getByText(/select an audio source/i)).toBeInTheDocument();
    expect(
      screen.getByText(/click start to begin capture/i),
    ).toBeInTheDocument();
  });

  it("loads the sample session preview from Express Setup without showing the hand-off nudge", async () => {
    render(<App />);
    expect(
      screen.queryByTestId("projection-runtime-stub"),
    ).not.toBeInTheDocument();
    const preview = await screen.findByRole("button", {
      name: /preview sample session/i,
    });
    expectNoPlaintextCredentialLoadback();

    preview.focus();
    fireEvent.click(preview);

    await waitFor(() =>
      expect(
        screen.queryByRole("dialog", { name: /quick setup/i }),
      ).not.toBeInTheDocument(),
    );
    const state = useAudioGraphStore.getState();
    expect(state.samplePreviewActive).toBe(true);
    expect(state.agentOverlayOpen).toBe(false);
    expect(state.transcriptSegments).toHaveLength(4);
    expect(state.materializedNotes?.session_id).toBe("sample-session-preview");
    expect(state.materializedProjectionGraph?.session_id).toBe(
      "sample-session-preview",
    );
    expect(state.liveAssistCards).toHaveLength(2);
    expect(screen.queryByText(/here's how to start/i)).not.toBeInTheDocument();
    expect(localStorage.getItem(HANDOFF_KEY)).toBe("1");
    expect(
      mockedInvoke.mock.calls.some(([cmd]) =>
        [
          "save_credential_cmd",
          "save_settings_cmd",
          "load_session",
          "add_question_to_graph",
          "start_capture",
          "start_transcribe",
        ].includes(cmd),
      ),
    ).toBe(false);
    expect(
      screen.queryByTestId("projection-runtime-stub"),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("tab", { name: /review/i })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    await waitFor(() =>
      expect(screen.getByRole("tab", { name: /review/i })).toHaveFocus(),
    );
    // SHELL-R2: the Review/"after" panel's content is SessionsBrowser now
    // (rail→detail), not NotesPanel/LiveTranscript directly — it's mocked
    // wholesale like every other heavy/lazy child in this file (see the
    // file-level `vi.mock` block), so `sessions-stub` is the fact to pin.
    expect(screen.getByTestId("sessions-stub")).toBeInTheDocument();
    expect(screen.queryByTestId("transcript-stub")).not.toBeInTheDocument();
    expect(screen.queryByTestId("graph-stub")).not.toBeInTheDocument();
    expect(screen.queryByTestId("agent-stub")).not.toBeInTheDocument();
  });

  it("passes the active i18n language into the sample session preview", async () => {
    await i18n.changeLanguage("pt");
    render(<App />);
    const preview = await screen.findByRole("button", {
      name: /preview sample session/i,
    });

    fireEvent.click(preview);

    await waitFor(() =>
      expect(useAudioGraphStore.getState().samplePreviewActive).toBe(true),
    );
    expect(useAudioGraphStore.getState().transcriptSegments[0]?.text).toContain(
      "credenciais salvas",
    );
    expectNoPlaintextCredentialLoadback();
  });

  it("persists a show-once flag and hides the nudge on dismiss", async () => {
    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: /skip/i }));
    const dismiss = await screen.findByRole("button", {
      name: /dismiss getting-started hint/i,
    });
    dismiss.focus();
    fireEvent.click(dismiss);
    expect(screen.queryByText(/here's how to start/i)).not.toBeInTheDocument();
    expect(localStorage.getItem(HANDOFF_KEY)).toBe("1");
    await waitFor(() =>
      expect(screen.getByRole("tab", { name: /ready/i })).toHaveFocus(),
    );
  });

  it("dismisses the hand-off nudge with Escape (WCAG 1.4.13)", async () => {
    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: /skip/i }));
    expect(await screen.findByText(/here's how to start/i)).toBeInTheDocument();
    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() =>
      expect(
        screen.queryByText(/here's how to start/i),
      ).not.toBeInTheDocument(),
    );
    expect(localStorage.getItem(HANDOFF_KEY)).toBe("1");
  });

  it("does not re-show the hand-off nudge when the flag is already set", async () => {
    localStorage.setItem(HANDOFF_KEY, "1");
    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: /skip/i }));
    // Give any state update a tick; the nudge must stay hidden.
    await waitFor(() =>
      expect(
        screen.queryByRole("dialog", { name: /quick setup/i }),
      ).not.toBeInTheDocument(),
    );
    expect(screen.queryByText(/here's how to start/i)).not.toBeInTheDocument();
  });

  it("shows Express Setup when only an OpenRouter key exists", async () => {
    mockCredentialPresence("openrouter_api_key");
    render(<App />);

    expect(
      await screen.findByRole("dialog", { name: /quick setup/i }),
    ).toBeInTheDocument();
    expectNoPlaintextCredentialLoadback();
  });

  it("shows Express Setup when only a Cerebras key exists", async () => {
    mockCredentialPresence("cerebras_api_key");
    render(<App />);

    expect(
      await screen.findByRole("dialog", { name: /quick setup/i }),
    ).toBeInTheDocument();
    expectNoPlaintextCredentialLoadback();
  });

  it("shows Express Setup when only a Deepgram key exists", async () => {
    mockCredentialPresence("deepgram_api_key");
    render(<App />);

    expect(
      await screen.findByRole("dialog", { name: /quick setup/i }),
    ).toBeInTheDocument();
    expectNoPlaintextCredentialLoadback();
  });

  it("shows Express Setup when only a Gemini key exists", async () => {
    mockCredentialPresence("gemini_api_key");
    render(<App />);

    expect(
      await screen.findByRole("dialog", { name: /quick setup/i }),
    ).toBeInTheDocument();
    expectNoPlaintextCredentialLoadback();
  });

  it("shows Express Setup when only an OpenAI-compatible saved key exists", async () => {
    mockCredentialPresence("openai_api_key");
    render(<App />);

    await waitFor(() =>
      expect(mockedInvoke).toHaveBeenCalledWith("load_credential_presence_cmd"),
    );
    expectNoPlaintextCredentialLoadback();
    expect(
      await screen.findByRole("dialog", { name: /quick setup/i }),
    ).toBeInTheDocument();
  });

  it("does not show Express Setup when a Deepgram and OpenRouter credential pair exists", async () => {
    mockCredentialPresence("deepgram_api_key", "openrouter_api_key");
    render(<App />);

    await waitForStartupProbeToSettle();
    expectNoPlaintextCredentialLoadback();
    expect(
      screen.queryByRole("dialog", { name: /quick setup/i }),
    ).not.toBeInTheDocument();
  });

  it("does not show Express Setup when a Deepgram and Cerebras credential pair exists", async () => {
    mockStartupProbe(
      ["deepgram_api_key", "cerebras_api_key"],
      startupSettings({
        llm_provider: {
          type: "api",
          endpoint: "https://api.cerebras.ai/v1",
          model: "gpt-oss-120b",
        },
      }),
    );
    render(<App />);

    await waitForStartupProbeToSettle();
    expectNoPlaintextCredentialLoadback();
    expect(
      screen.queryByRole("dialog", { name: /quick setup/i }),
    ).not.toBeInTheDocument();
  });

  it("accepts a selected AWS profile only when it exists locally", async () => {
    mockStartupProbe(
      ["deepgram_api_key"],
      startupSettings({
        llm_provider: {
          type: "aws_bedrock",
          region: "us-west-2",
          model_id: "anthropic.claude-sonnet-4-5-20250929-v1:0",
          credential_source: { type: "profile", name: "dictation-prod" },
        },
      }),
      readyModels,
      ["default", "dictation-prod"],
    );
    render(<App />);

    await waitForStartupProbeToSettle();
    expect(mockedInvoke).toHaveBeenCalledWith("list_aws_profiles");
    expect(
      screen.queryByRole("dialog", { name: /quick setup/i }),
    ).not.toBeInTheDocument();
  });

  it("keeps setup visible for a missing saved AWS profile", async () => {
    mockStartupProbe(
      ["deepgram_api_key"],
      startupSettings({
        llm_provider: {
          type: "aws_bedrock",
          region: "us-west-2",
          model_id: "anthropic.claude-sonnet-4-5-20250929-v1:0",
          credential_source: { type: "profile", name: "missing-profile" },
        },
      }),
      readyModels,
      ["default"],
    );
    render(<App />);

    expect(
      await screen.findByRole("dialog", { name: /quick setup/i }),
    ).toBeInTheDocument();
    expect(mockedInvoke).toHaveBeenCalledWith("list_aws_profiles");
  });

  it("does not show Express Setup for selected Deepgram plus a ready local LLM", async () => {
    mockStartupProbe(
      ["deepgram_api_key"],
      startupSettings({ llm_provider: { type: "local_llama" } }),
    );
    render(<App />);

    await waitForStartupProbeToSettle();
    expect(mockedInvoke).toHaveBeenCalledWith("get_model_status");
    expect(
      screen.queryByRole("dialog", { name: /quick setup/i }),
    ).not.toBeInTheDocument();
  });

  it("shows Express Setup when the selected local LLM model is not downloaded", async () => {
    mockStartupProbe(
      ["deepgram_api_key"],
      startupSettings({ llm_provider: { type: "local_llama" } }),
      { ...readyModels, llm: "NotDownloaded" },
    );
    render(<App />);

    expect(
      await screen.findByRole("dialog", { name: /quick setup/i }),
    ).toBeInTheDocument();
  });

  it("keeps MistralRs setup visible until its selected model has specific readiness", async () => {
    mockStartupProbe(
      ["deepgram_api_key"],
      startupSettings({
        llm_provider: { type: "mistralrs", model_id: "selected-model.gguf" },
      }),
      readyModels,
    );
    render(<App />);

    expect(
      await screen.findByRole("dialog", { name: /quick setup/i }),
    ).toBeInTheDocument();
  });

  it("hydrates shared provider settings from the passive startup read", async () => {
    const settings = startupSettings();
    mockStartupProbe(["deepgram_api_key", "openrouter_api_key"], settings);
    render(<App />);

    await waitForStartupProbeToSettle();
    expect(useAudioGraphStore.getState().settings).toEqual(settings);
  });

  it("shows Express Setup when a saved key does not belong to the selected endpoint", async () => {
    mockStartupProbe(
      ["deepgram_api_key", "cerebras_api_key"],
      startupSettings({
        llm_provider: {
          type: "api",
          endpoint: "https://openrouter.ai/api/v1",
          model: "openai/gpt-4.1-mini",
        },
      }),
    );
    render(<App />);

    expect(
      await screen.findByRole("dialog", { name: /quick setup/i }),
    ).toBeInTheDocument();
  });

  it("shows Express Setup when the selected provider configuration is invalid", async () => {
    mockStartupProbe(
      ["deepgram_api_key", "openrouter_api_key"],
      startupSettings({
        llm_provider: {
          type: "openrouter",
          model: "",
          base_url: "https://openrouter.ai/api/v1",
          include_usage_in_stream: true,
        },
      }),
    );
    render(<App />);

    expect(
      await screen.findByRole("dialog", { name: /quick setup/i }),
    ).toBeInTheDocument();
  });

  it("keeps startup passive and never performs provider readiness egress", async () => {
    mockCredentialPresence("deepgram_api_key", "openrouter_api_key");
    render(<App />);

    await waitForStartupProbeToSettle();
    expect(mockedInvoke).toHaveBeenCalledWith("load_settings_cmd");
    expect(mockedInvoke).not.toHaveBeenCalledWith(
      "get_provider_readiness_cmd",
      expect.anything(),
    );
  });

  it("starts in the Ready workspace with notes and transcript ahead of graph diagnostics", async () => {
    mockCredentialPresence("openai_api_key");
    render(<App />);

    await waitFor(() =>
      expect(mockedInvoke).toHaveBeenCalledWith("load_credential_presence_cmd"),
    );

    expect(screen.getByRole("tab", { name: /ready/i })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    expect(screen.getByTestId("notes-stub")).toBeInTheDocument();
    expect(screen.getByTestId("transcript-stub")).toBeInTheDocument();
    expect(screen.queryByTestId("graph-stub")).not.toBeInTheDocument();
    expect(
      screen.queryByTestId("projection-runtime-stub"),
    ).not.toBeInTheDocument();
  });

  it("reveals graph and runtime diagnostics only after switching to Inspect", async () => {
    mockCredentialPresence("openai_api_key");
    render(<App />);

    fireEvent.click(screen.getByRole("tab", { name: /inspect/i }));

    expect(screen.getByRole("tab", { name: /inspect/i })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    expect(await screen.findByTestId("graph-stub")).toBeInTheDocument();
    expect(screen.getByTestId("projection-runtime-stub")).toBeInTheDocument();
  });

  it("routes loaded historical sessions to Review without showing graph diagnostics", async () => {
    mockCredentialPresence("openai_api_key");
    useAudioGraphStore.setState({ loadedSessionId: "session-1" });

    render(<App />);

    await waitFor(() =>
      expect(screen.getByRole("tab", { name: /review/i })).toHaveAttribute(
        "aria-selected",
        "true",
      ),
    );
    // SHELL-R2: the Review/"after" panel's content is SessionsBrowser now
    // (rail→detail), not NotesPanel/LiveTranscript directly — it's mocked
    // wholesale like every other heavy/lazy child in this file (see the
    // file-level `vi.mock` block), so `sessions-stub` is the fact to pin.
    expect(screen.getByTestId("sessions-stub")).toBeInTheDocument();
    expect(screen.queryByTestId("notes-stub")).not.toBeInTheDocument();
    expect(screen.queryByTestId("transcript-stub")).not.toBeInTheDocument();
    expect(screen.queryByTestId("graph-stub")).not.toBeInTheDocument();
    expect(
      screen.queryByTestId("projection-runtime-stub"),
    ).not.toBeInTheDocument();
  });

  it("returns to Live now and restores transcript focus when capture starts from Inspect", async () => {
    mockCredentialPresence("openai_api_key");
    useAudioGraphStore.setState({ rightPanelTab: "chat" });
    render(<App />);

    fireEvent.click(screen.getByRole("tab", { name: /inspect/i }));
    expect(screen.getByRole("tab", { name: /inspect/i })).toHaveAttribute(
      "aria-selected",
      "true",
    );

    act(() => {
      useAudioGraphStore.setState({ isCapturing: true });
    });

    await waitFor(() =>
      expect(screen.getByRole("tab", { name: /live now/i })).toHaveAttribute(
        "aria-selected",
        "true",
      ),
    );
    expect(screen.getByText("Live session")).toBeInTheDocument();
    expect(useAudioGraphStore.getState().rightPanelTab).toBe("transcript");
  });

  it("supports roving keyboard navigation across workspace tabs", async () => {
    mockCredentialPresence("openai_api_key");
    render(<App />);

    const during = screen.getByRole("tab", { name: /ready/i });
    during.focus();
    fireEvent.keyDown(during, { key: "ArrowRight" });

    const after = screen.getByRole("tab", { name: /review/i });
    expect(after).toHaveAttribute("aria-selected", "true");
    expect(after).toHaveFocus();

    fireEvent.keyDown(after, { key: "End" });

    const analysis = screen.getByRole("tab", { name: /inspect/i });
    expect(analysis).toHaveAttribute("aria-selected", "true");
    expect(analysis).toHaveFocus();
  });

  it("renders live assist inline in the During workspace when agent activity exists", async () => {
    mockCredentialPresence("openai_api_key");
    useAudioGraphStore.setState({
      agentStatus: {
        state: "running",
        message: "Checking context",
        timestamp_ms: Date.now(),
      },
    });

    render(<App />);

    expect(screen.getByTestId("agent-stub")).toBeInTheDocument();
  });

  it("re-shows the hand-off for a configured user after re-arming via the help modal (App.tsx:159)", async () => {
    // Configured user: a complete durable cloud credential pair exists, so
    // ExpressSetup never pops and the hand-off was previously seen (flag set).
    // The re-arm path is the ONLY way the banner can come back for them — the
    // bug this finding fixes.
    mockCredentialPresence("deepgram_api_key", "openrouter_api_key");
    localStorage.setItem(HANDOFF_KEY, "1");
    render(<App />);

    // No ExpressSetup, no banner to start with.
    await waitFor(() =>
      expect(mockedInvoke).toHaveBeenCalledWith("load_credential_presence_cmd"),
    );
    expect(
      screen.queryByRole("dialog", { name: /quick setup/i }),
    ).not.toBeInTheDocument();
    expect(screen.queryByText(/here's how to start/i)).not.toBeInTheDocument();

    // Open the keyboard-shortcuts help modal (Cmd/Ctrl+/).
    fireEvent.keyDown(window, { key: "/", ctrlKey: true });
    const reArm = await screen.findByRole("button", {
      name: /show getting-started guide again/i,
    });
    // Re-arm clears the show-once flag…
    fireEvent.click(reArm);
    expect(localStorage.getItem(HANDOFF_KEY)).toBeNull();
    // …and closing the modal (Escape) re-surfaces the banner for this user.
    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() =>
      expect(screen.getByText(/here's how to start/i)).toBeInTheDocument(),
    );

    // Still dismissible/show-once after re-arm.
    fireEvent.click(
      screen.getByRole("button", { name: /dismiss getting-started hint/i }),
    );
    expect(screen.queryByText(/here's how to start/i)).not.toBeInTheDocument();
    expect(localStorage.getItem(HANDOFF_KEY)).toBe("1");
  });
});

describe("App — probe-failure Get-started fallback (fbf0 / A3)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    localStorage.clear();
    mockedInvoke.mockReset();
    seedStore();
  });

  afterEach(async () => {
    await i18n.changeLanguage("en");
    localStorage.clear();
  });

  /** Make the credential-presence probe throw, as if the backend/keychain is
   * not ready. Other commands stay inert. */
  function mockProbeRejection() {
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "load_credential_cmd") {
        throw new Error(
          "load_credential_cmd should not be invoked by frontend tests; use load_credential_presence_cmd and provider readiness instead.",
        );
      }
      if (cmd === "load_credential_presence_cmd") {
        throw new Error("backend not ready");
      }
      return undefined;
    });
  }

  function probeCallCount() {
    return mockedInvoke.mock.calls.filter(
      ([cmd]) => cmd === "load_credential_presence_cmd",
    ).length;
  }

  it("renders the Get-started fallback instead of empty panels when the probe throws", async () => {
    mockProbeRejection();
    render(<App />);

    // The fallback replaces the During notes/transcript panels — not empty.
    expect(
      await screen.findByTestId("get-started-fallback"),
    ).toBeInTheDocument();
    expect(screen.queryByTestId("notes-stub")).not.toBeInTheDocument();
    expect(screen.queryByTestId("transcript-stub")).not.toBeInTheDocument();
    // The During phase tab stays selected — the shell is intact, just recovered.
    expect(screen.getByRole("tab", { name: /ready/i })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    // No ExpressSetup wizard, no plaintext key loadback.
    expect(
      screen.queryByRole("dialog", { name: /quick setup/i }),
    ).not.toBeInTheDocument();
    expectNoPlaintextCredentialLoadback();
    // Friendly copy, not a raw error string.
    expect(screen.getByText(/let's get you started/i)).toBeInTheDocument();
    expect(screen.queryByText(/backend not ready/i)).not.toBeInTheDocument();
  });

  it("re-runs the probe when Retry is clicked", async () => {
    mockProbeRejection();
    render(<App />);

    await screen.findByTestId("get-started-fallback");
    const callsAfterMount = probeCallCount();
    expect(callsAfterMount).toBeGreaterThanOrEqual(1);

    fireEvent.click(screen.getByRole("button", { name: /retry/i }));

    await waitFor(() =>
      expect(probeCallCount()).toBeGreaterThan(callsAfterMount),
    );
  });

  it("clears the fallback and restores the workspace on a successful retry", async () => {
    // Fail on mount, then succeed with a configured Deepgram + OpenRouter route so
    // ExpressSetup stays suppressed and the During panels come back.
    let probeCalls = 0;
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "load_credential_cmd") {
        throw new Error("plaintext loadback is forbidden");
      }
      if (cmd === "load_credential_presence_cmd") {
        probeCalls += 1;
        if (probeCalls === 1) throw new Error("backend not ready");
        return credentialPresence("deepgram_api_key", "openrouter_api_key");
      }
      if (cmd === "load_settings_cmd") return startupSettings();
      return undefined;
    });
    render(<App />);

    await screen.findByTestId("get-started-fallback");

    fireEvent.click(screen.getByRole("button", { name: /retry/i }));

    // Fallback clears; the real During panels render again.
    await waitFor(() =>
      expect(
        screen.queryByTestId("get-started-fallback"),
      ).not.toBeInTheDocument(),
    );
    expect(screen.getByTestId("notes-stub")).toBeInTheDocument();
    expect(screen.getByTestId("transcript-stub")).toBeInTheDocument();
    // A runnable saved pair keeps ExpressSetup suppressed.
    expect(
      screen.queryByRole("dialog", { name: /quick setup/i }),
    ).not.toBeInTheDocument();
    expectNoPlaintextCredentialLoadback();
  });

  it("launches the sample session preview from the fallback CTA", async () => {
    mockProbeRejection();
    render(<App />);

    await screen.findByTestId("get-started-fallback");

    fireEvent.click(
      screen.getByRole("button", { name: /preview sample session/i }),
    );

    // Sample flow fires: preview state hydrates and the shell routes to After.
    await waitFor(() =>
      expect(useAudioGraphStore.getState().samplePreviewActive).toBe(true),
    );
    expect(
      screen.queryByTestId("get-started-fallback"),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("tab", { name: /review/i })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    expect(useAudioGraphStore.getState().transcriptSegments).toHaveLength(4);
    // No backend session/capture/persistence commands from the sample path.
    expect(
      mockedInvoke.mock.calls.some(([cmd]) =>
        [
          "save_credential_cmd",
          "save_settings_cmd",
          "load_session",
          "start_capture",
          "start_transcribe",
        ].includes(cmd),
      ),
    ).toBe(false);
  });

  it("opens Settings from the fallback escape hatch", async () => {
    mockProbeRejection();
    render(<App />);

    await screen.findByTestId("get-started-fallback");

    fireEvent.click(screen.getByRole("button", { name: /open settings/i }));

    await waitFor(() =>
      expect(useAudioGraphStore.getState().settingsOpen).toBe(true),
    );
  });

  it("does not show the fallback while a sample preview is already active", async () => {
    mockProbeRejection();
    useAudioGraphStore.setState({ samplePreviewActive: true });
    render(<App />);

    // Probe still throws, but real content (the sample) owns the surface.
    await waitFor(() =>
      expect(mockedInvoke).toHaveBeenCalledWith("load_credential_presence_cmd"),
    );
    expect(
      screen.queryByTestId("get-started-fallback"),
    ).not.toBeInTheDocument();
  });

  it("localizes the fallback in Portuguese", async () => {
    await i18n.changeLanguage("pt");
    mockProbeRejection();
    render(<App />);

    expect(
      await screen.findByTestId("get-started-fallback"),
    ).toBeInTheDocument();
    expect(screen.getByText(/vamos começar/i)).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /visualizar sessão de exemplo/i }),
    ).toBeInTheDocument();
  });

  it("shows a repair hint (not the first-run card) when saved credentials are unreadable", async () => {
    // cred-review m6 + PR #84 review: the repair-hint copy is reserved for a
    // RECOGNIZED parse failure. The backend's redacted_yaml_parse_error keeps
    // the stable "Failed to parse {path}:" prefix (content omitted), which is
    // what the frontend classifies on. The fallback must warn about unreadable
    // credentials rather than tell the user to "get started" (which would
    // invite ExpressSetup to re-prompt and overwrite recoverable keys).
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "load_credential_cmd") {
        throw new Error("plaintext loadback is forbidden");
      }
      if (cmd === "load_credential_presence_cmd") {
        // Structured AppError payload as serde emits it, with the real
        // redacted parse-reason shape from credentials/mod.rs.
        throw {
          code: "credential_file_error",
          message: {
            reason:
              "Failed to parse /home/user/.config/audio-graph/credentials.yaml: invalid YAML at line 3 column 18 (content omitted)",
          },
        };
      }
      return undefined;
    });
    render(<App />);

    await screen.findByTestId("get-started-fallback");
    expect(
      screen.getByText(/couldn't read your credentials/i),
    ).toBeInTheDocument();
    // The fresh-install copy must NOT show.
    expect(
      screen.queryByText(/let's get you started/i),
    ).not.toBeInTheDocument();
    // ExpressSetup must not pop (it would overwrite recoverable keys).
    expect(
      screen.queryByRole("dialog", { name: /quick setup/i }),
    ).not.toBeInTheDocument();
    // Never leak the raw parse error / any path fragment into the UI.
    expect(screen.queryByText(/failed to parse/i)).not.toBeInTheDocument();
    expectNoPlaintextCredentialLoadback();
  });

  it("shows the generic retryable fallback (not the corrupt-file hint) when the keychain is locked", async () => {
    // PR #84 review (Codex P2): the backend wraps EVERY presence-load failure
    // as credential_file_error — including OS-keychain-locked/unavailable
    // failures, whose reason strings come from the keyring layer ("Failed to
    // read OS credential …"), not the parse formatter. Telling that user the
    // credential FILE is corrupt is wrong and scary; unlock+retry is the right
    // recovery, so the generic fallback copy (with its Retry button) must show.
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "load_credential_cmd") {
        throw new Error("plaintext loadback is forbidden");
      }
      if (cmd === "load_credential_presence_cmd") {
        throw {
          code: "credential_file_error",
          message: {
            reason:
              "Failed to read OS credential openai_api_key: keyring is locked",
          },
        };
      }
      return undefined;
    });
    render(<App />);

    await screen.findByTestId("get-started-fallback");
    // Generic retryable copy — NOT the unreadable-credentials hint.
    expect(screen.getByText(/let's get you started/i)).toBeInTheDocument();
    expect(
      screen.queryByText(/couldn't read your credentials/i),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: /retry/i })).toBeInTheDocument();
    // Never leak the raw keychain error into the UI.
    expect(screen.queryByText(/keyring is locked/i)).not.toBeInTheDocument();
    expectNoPlaintextCredentialLoadback();
  });
});

describe("App — a11y batch (seed 4f2e)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    localStorage.clear();
    mockedInvoke.mockReset();
    // Saved cloud pair present → Express Setup stays closed so the shell chrome
    // (skip link, live regions, workspace panels) is what we assert against.
    mockCredentialPresence("openai_api_key");
    seedStore();
  });

  afterEach(async () => {
    await i18n.changeLanguage("en");
    localStorage.clear();
  });

  it("renders a skip-to-main link that targets the active workspace panel (WCAG 2.4.1)", () => {
    render(<App />);
    const skip = screen.getByRole("link", { name: /skip to main content/i });
    expect(skip).toHaveClass("skip-to-main");
    // Default phase is During; the link points at that panel's id, and the
    // panel is a <main> landmark with the matching id.
    expect(skip).toHaveAttribute("href", "#workspace-panel-during");
    const main = document.getElementById("workspace-panel-during");
    expect(main?.tagName).toBe("MAIN");
  });

  it("announces recording start/stop assertively, distinct from the polite state region (WCAG 4.1.3)", async () => {
    render(<App />);
    const assertive = document.querySelector(
      '[role="status"][aria-live="assertive"]',
    );
    expect(assertive).toBeInstanceOf(HTMLElement);
    // Empty on mount — no spurious announcement.
    expect(assertive?.textContent).toBe("");

    await act(async () => {
      useAudioGraphStore.setState({ isCapturing: true });
    });
    await waitFor(() =>
      expect(assertive?.textContent).toMatch(/recording started/i),
    );

    await act(async () => {
      useAudioGraphStore.setState({ isCapturing: false });
    });
    await waitFor(() =>
      expect(assertive?.textContent).toMatch(/recording stopped/i),
    );
  });

  it("announces workspace phase transitions politely (critique B7)", async () => {
    render(<App />);
    const politeRegions = Array.from(
      document.querySelectorAll('[role="status"][aria-live="polite"]'),
    );
    const phaseRegion = politeRegions.find((el) =>
      el.classList.contains("sr-only"),
    );
    expect(phaseRegion).toBeInstanceOf(HTMLElement);
    // No announcement on initial mount.
    expect(phaseRegion?.textContent).toBe("");

    fireEvent.click(screen.getByRole("tab", { name: /^review$/i }));
    await waitFor(() =>
      expect(phaseRegion?.textContent).toMatch(/review view/i),
    );
  });
});
