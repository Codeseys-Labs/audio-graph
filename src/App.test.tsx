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
import type { CredentialPresence } from "./types";

const HANDOFF_KEY = ONBOARDING_HANDOFF_SEEN_KEY;

function credentialPresence(...keys: string[]): CredentialPresence[] {
  return keys.map((key) => ({
    key,
    present: true,
    source: "credentials_yaml",
  }));
}

function mockCredentialPresence(...keys: string[]) {
  mockedInvoke.mockImplementation(async (cmd: string) => {
    if (cmd === "load_credential_cmd") {
      throw new Error(
        "load_credential_cmd should not be invoked by frontend tests; use load_credential_presence_cmd and provider readiness instead.",
      );
    }
    if (cmd === "load_credential_presence_cmd") {
      return credentialPresence(...keys);
    }
    return undefined;
  });
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
    expect(screen.getByRole("tab", { name: /after/i })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    expect(screen.getByTestId("transcript-stub")).toBeInTheDocument();
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
    fireEvent.click(dismiss);
    expect(screen.queryByText(/here's how to start/i)).not.toBeInTheDocument();
    expect(localStorage.getItem(HANDOFF_KEY)).toBe("1");
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

  it("does not show Express Setup when only an OpenAI-compatible saved key exists", async () => {
    mockCredentialPresence("openai_api_key");
    render(<App />);

    await waitFor(() =>
      expect(mockedInvoke).toHaveBeenCalledWith("load_credential_presence_cmd"),
    );
    expectNoPlaintextCredentialLoadback();
    expect(
      screen.queryByRole("dialog", { name: /quick setup/i }),
    ).not.toBeInTheDocument();
  });

  it("does not show Express Setup when a Deepgram and OpenRouter credential pair exists", async () => {
    mockCredentialPresence("deepgram_api_key", "openrouter_api_key");
    render(<App />);

    await waitFor(() =>
      expect(mockedInvoke).toHaveBeenCalledWith("load_credential_presence_cmd"),
    );
    expectNoPlaintextCredentialLoadback();
    expect(
      screen.queryByRole("dialog", { name: /quick setup/i }),
    ).not.toBeInTheDocument();
  });

  it("does not show Express Setup when a Deepgram and Cerebras credential pair exists", async () => {
    mockCredentialPresence("deepgram_api_key", "cerebras_api_key");
    render(<App />);

    await waitFor(() =>
      expect(mockedInvoke).toHaveBeenCalledWith("load_credential_presence_cmd"),
    );
    expectNoPlaintextCredentialLoadback();
    expect(
      screen.queryByRole("dialog", { name: /quick setup/i }),
    ).not.toBeInTheDocument();
  });

  it("starts in the During workspace with notes and transcript ahead of graph diagnostics", async () => {
    mockCredentialPresence("openai_api_key");
    render(<App />);

    await waitFor(() =>
      expect(mockedInvoke).toHaveBeenCalledWith("load_credential_presence_cmd"),
    );

    expect(screen.getByRole("tab", { name: /during/i })).toHaveAttribute(
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

  it("reveals graph and runtime diagnostics only after switching to Analysis", async () => {
    mockCredentialPresence("openai_api_key");
    render(<App />);

    fireEvent.click(screen.getByRole("tab", { name: /analysis/i }));

    expect(screen.getByRole("tab", { name: /analysis/i })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    expect(await screen.findByTestId("graph-stub")).toBeInTheDocument();
    expect(screen.getByTestId("projection-runtime-stub")).toBeInTheDocument();
  });

  it("routes loaded historical sessions to the After workspace without showing graph diagnostics", async () => {
    mockCredentialPresence("openai_api_key");
    useAudioGraphStore.setState({ loadedSessionId: "session-1" });

    render(<App />);

    await waitFor(() =>
      expect(screen.getByRole("tab", { name: /after/i })).toHaveAttribute(
        "aria-selected",
        "true",
      ),
    );
    expect(screen.getByTestId("notes-stub")).toBeInTheDocument();
    expect(screen.getByTestId("transcript-stub")).toBeInTheDocument();
    expect(screen.queryByTestId("graph-stub")).not.toBeInTheDocument();
    expect(
      screen.queryByTestId("projection-runtime-stub"),
    ).not.toBeInTheDocument();
  });

  it("returns to During and restores transcript focus when capture starts from Analysis", async () => {
    mockCredentialPresence("openai_api_key");
    useAudioGraphStore.setState({ rightPanelTab: "chat" });
    render(<App />);

    fireEvent.click(screen.getByRole("tab", { name: /analysis/i }));
    expect(screen.getByRole("tab", { name: /analysis/i })).toHaveAttribute(
      "aria-selected",
      "true",
    );

    act(() => {
      useAudioGraphStore.setState({ isCapturing: true });
    });

    await waitFor(() =>
      expect(screen.getByRole("tab", { name: /during/i })).toHaveAttribute(
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

    const during = screen.getByRole("tab", { name: /during/i });
    during.focus();
    fireEvent.keyDown(during, { key: "ArrowRight" });

    const after = screen.getByRole("tab", { name: /after/i });
    expect(after).toHaveAttribute("aria-selected", "true");
    expect(after).toHaveFocus();

    fireEvent.keyDown(after, { key: "End" });

    const analysis = screen.getByRole("tab", { name: /analysis/i });
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
    expect(screen.getByRole("tab", { name: /during/i })).toHaveAttribute(
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
    // Fail on mount, then succeed with a runnable OpenAI key on retry so
    // ExpressSetup stays suppressed and the During panels come back.
    let probeCalls = 0;
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "load_credential_cmd") {
        throw new Error("plaintext loadback is forbidden");
      }
      if (cmd === "load_credential_presence_cmd") {
        probeCalls += 1;
        if (probeCalls === 1) throw new Error("backend not ready");
        return credentialPresence("openai_api_key");
      }
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
    expect(screen.getByRole("tab", { name: /after/i })).toHaveAttribute(
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
    // cred-review m6: a `credential_file_error` AppError means saved
    // credentials exist but can't be parsed — distinct from a fresh-install
    // throw. The fallback must warn about unreadable credentials rather than
    // tell the user to "get started" (which would invite ExpressSetup to
    // re-prompt and overwrite recoverable keys).
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "load_credential_cmd") {
        throw new Error("plaintext loadback is forbidden");
      }
      if (cmd === "load_credential_presence_cmd") {
        // Structured AppError payload as serde emits it.
        throw {
          code: "credential_file_error",
          message: { reason: "invalid yaml at line 3" },
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
    // Never leak the raw parse error / any key fragment into the UI.
    expect(
      screen.queryByText(/invalid yaml at line 3/i),
    ).not.toBeInTheDocument();
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

    fireEvent.click(screen.getByRole("tab", { name: /^after$/i }));
    await waitFor(() =>
      expect(phaseRegion?.textContent).toMatch(/after view/i),
    );
  });
});
