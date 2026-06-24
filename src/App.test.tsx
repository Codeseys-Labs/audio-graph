import { invoke } from "@tauri-apps/api/core";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import "./i18n";
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
  default: ({ onDismiss }: { onDismiss: () => void }) => (
    <div role="dialog" aria-label="Quick Setup">
      <button type="button" onClick={onDismiss}>
        Skip
      </button>
    </div>
  ),
}));

const mockedInvoke = vi.mocked(invoke);

import { ONBOARDING_HANDOFF_SEEN_KEY } from "./constants/storageKeys";

const HANDOFF_KEY = ONBOARDING_HANDOFF_SEEN_KEY;

function seedStore() {
  // Provide the minimal store fields the always-mounted chrome reads.
  useAudioGraphStore.setState({
    rightPanelTab: "transcript",
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
  beforeEach(() => {
    localStorage.clear();
    mockedInvoke.mockReset();
    // No cloud credential present → App pops Express Setup on mount.
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "load_credential_cmd") return null;
      return undefined;
    });
    seedStore();
  });

  afterEach(() => {
    localStorage.clear();
  });

  it("shows the hand-off nudge once Express Setup is dismissed", async () => {
    render(<App />);
    // Express Setup appears because no credentials were found.
    const skip = await screen.findByRole("button", { name: /skip/i });
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
});
