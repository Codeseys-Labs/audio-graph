import { invoke } from "@tauri-apps/api/core";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
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
    agentProposals: [],
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
    expect(screen.getByTestId("projection-runtime-stub")).toBeInTheDocument();
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
