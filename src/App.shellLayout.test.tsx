/**
 * SHELL-R7 (plan §R7, ADR-0046) — App-level pins for `useShellLayout()`'s
 * tier-driven rail/aside pin-vs-drawer behavior. Unlike `App.test.tsx`
 * (onboarding hand-off flow) and `App.contract.test.tsx` (e2e-pinned facts),
 * this file is scoped entirely to the wide/standard/compact tier contract:
 * which regions render pinned vs. behind a drawer trigger, and that both
 * drawers are keyboard-reachable, focus-trapped, Escape-closable, and
 * restore focus to their trigger on close (the same contract
 * `SystemDrawer.test.tsx` pins for the drawer it established).
 */
import { invoke } from "@tauri-apps/api/core";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import i18n from "./i18n";
import { useAudioGraphStore } from "./store";
import type { AppSettings, CredentialPresence } from "./types";

// Same stub set as `App.test.tsx` — this file cares about the shell's own
// pin/drawer wiring, not any panel's internals.
vi.mock("./components/SettingsPage", () => ({
  default: () => <div data-testid="settings-stub" />,
}));
vi.mock("./components/SessionsBrowser", () => ({
  default: () => <div data-testid="sessions-stub" />,
}));
vi.mock("./components/AudioSourceSelector", () => ({
  default: () => <div data-testid="sources-stub" />,
}));
vi.mock("./components/SpeakerPanel", () => ({
  default: () => <div data-testid="speakers-stub" />,
}));
vi.mock("./components/LiveTranscript", () => ({
  default: () => <div data-testid="transcript-stub" />,
}));
vi.mock("./components/TokenUsagePanel", () => ({
  default: () => <div data-testid="tokens-stub" />,
}));
// Ticket W5 (synthesis audio-graph-a6b5): the document tile stopped hosting
// `NotesPanel` (untouched — mocked wholesale above as `sessions-stub` via
// `SessionsBrowser`) and now hosts `LiveDocument`/`LiveDocumentHeaderActions`,
// matching `App.test.tsx`'s stub — this file's own concern (pin/drawer tier
// contract) has nothing to do with the document tile's internals, so mount
// no real `useLiveDocumentModel()`/outline/Popover tree here either.
vi.mock("./components/workspace/LiveDocument", () => ({
  LiveDocument: () => <div data-testid="live-document-stub" />,
  LiveDocumentHeaderActions: () => null,
  useLiveDocumentModel: () => ({
    sections: [],
    lastSequence: 0,
    changedNodeIds: [],
    appendedAtTail: false,
  }),
}));
vi.mock("./components/PipelineStatusBar", () => ({
  default: () => <div data-testid="pipeline-stub" />,
}));
vi.mock("./components/AgentProposalsPanel", () => ({
  default: () => <div data-testid="agent-stub" />,
}));
vi.mock("./components/NowStrip", () => ({
  default: () => <div data-testid="controlbar-stub" />,
}));
vi.mock("./components/SystemDrawer", () => ({
  default: ({ onClose }: { onClose: () => void }) => (
    <div role="dialog" aria-label="System status">
      <button type="button" onClick={onClose}>
        Close
      </button>
    </div>
  ),
}));

const mockedInvoke = vi.mocked(invoke);

type ChangeListener = () => void;

interface MatchMediaControl {
  setWidth: (next: number) => void;
}

/** Same `matchMedia` mock convention as `ChatSidebar.test.tsx`/
 * `useShellLayout.test.ts` — jsdom implements neither, so every consumer
 * supplies its own. Parses the `(min-width: NNNpx)` queries
 * `useShellLayout` issues and drives `.matches` off a single controllable
 * `width`. */
function installMatchMedia(initialWidth: number): MatchMediaControl {
  let width = initialWidth;
  const listeners = new Map<string, Set<ChangeListener>>();

  function minWidthOf(query: string): number {
    const match = query.match(/min-width:\s*(\d+)px/);
    return match ? Number(match[1]) : 0;
  }

  window.matchMedia = vi.fn().mockImplementation((query: string) => {
    const threshold = minWidthOf(query);
    return {
      get matches() {
        return width >= threshold;
      },
      media: query,
      onchange: null,
      addEventListener: (_type: string, cb: ChangeListener) => {
        if (!listeners.has(query)) listeners.set(query, new Set());
        listeners.get(query)?.add(cb);
      },
      removeEventListener: (_type: string, cb: ChangeListener) => {
        listeners.get(query)?.delete(cb);
      },
      addListener: vi.fn(),
      removeListener: vi.fn(),
      dispatchEvent: vi.fn(),
    } as unknown as MediaQueryList;
  }) as unknown as typeof window.matchMedia;

  return {
    setWidth(next: number) {
      width = next;
      for (const set of listeners.values()) {
        for (const cb of set) cb();
      }
    },
  };
}

function credentialPresence(...keys: string[]): CredentialPresence[] {
  return keys.map((key) => ({
    key,
    present: true,
    source: "credentials_yaml",
  }));
}

const CONFIGURED_SETTINGS: AppSettings = {
  asr_provider: { type: "deepgram", model: "nova-3", enable_diarization: true },
  whisper_model: "ggml-small.en.bin",
  llm_provider: {
    type: "openrouter",
    model: "openai/gpt-4.1-mini",
    base_url: "https://openrouter.ai/api/v1",
    include_usage_in_stream: true,
  },
  llm_api_config: null,
  audio_settings: { sample_rate: 48_000, channels: 1 },
  gemini: { auth: { type: "api_key" }, model: "gemini-2.0-flash-live-001" },
  tts_provider: { type: "none" },
  speak_aloud: false,
  log_level: "info",
};

/** A configured Deepgram+OpenRouter credential pair keeps ExpressSetup
 * suppressed (same convention as `App.contract.test.tsx`) so the always-
 * mounted rail/aside chrome under test settles immediately. */
function installInvokeMocks() {
  mockedInvoke.mockImplementation(async (cmd: string) => {
    switch (cmd) {
      case "load_credential_presence_cmd":
        return credentialPresence("deepgram_api_key", "openrouter_api_key");
      case "load_settings_cmd":
        return CONFIGURED_SETTINGS;
      default:
        return undefined;
    }
  });
}

function seedStoreDefaults() {
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

async function waitForStartupProbeToSettle() {
  await waitFor(() =>
    expect(
      document.querySelector('[data-onboarding-probe="settled"]'),
    ).toBeInTheDocument(),
  );
}

describe("App — SHELL-R7 shell layout tiers (useShellLayout rail/aside drawers)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    localStorage.clear();
    mockedInvoke.mockReset();
    installInvokeMocks();
    seedStoreDefaults();
  });

  afterEach(async () => {
    await i18n.changeLanguage("en");
    localStorage.clear();
    // @ts-expect-error -- test-only teardown of the mock installed per test.
    delete window.matchMedia;
  });

  it("pins the rail and aside inline at the wide tier (>=1280px) with no drawer triggers", async () => {
    installMatchMedia(1400);
    render(<App />);
    await waitForStartupProbeToSettle();

    expect(screen.getByTestId("sources-stub")).toBeInTheDocument();
    expect(screen.getByTestId("speakers-stub")).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Sources" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Speakers" }),
    ).not.toBeInTheDocument();
  });

  it("collapses only the aside to a drawer at the standard tier (1024-1279px) — rail stays pinned", async () => {
    installMatchMedia(1100);
    render(<App />);
    await waitForStartupProbeToSettle();

    // Rail (AudioSourceSelector) is still pinned inline.
    expect(screen.getByTestId("sources-stub")).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Sources" }),
    ).not.toBeInTheDocument();

    // Aside (SpeakerPanel) is no longer pinned inline — only its trigger is.
    expect(screen.queryByTestId("speakers-stub")).not.toBeInTheDocument();
    const trigger = screen.getByRole("button", { name: "Speakers" });

    trigger.focus();
    expect(trigger).toHaveFocus();
    fireEvent.click(trigger);

    const dialog = await screen.findByRole("dialog", { name: "Speakers" });
    expect(dialog).toHaveAttribute("aria-modal", "true");
    expect(dialog).toHaveFocus();
    expect(within(dialog).getByTestId("speakers-stub")).toBeInTheDocument();

    fireEvent.keyDown(document, { key: "Escape" });
    await waitFor(() =>
      expect(
        screen.queryByRole("dialog", { name: "Speakers" }),
      ).not.toBeInTheDocument(),
    );
    expect(trigger).toHaveFocus();
  });

  it("collapses both the rail and aside to drawers at the compact tier (<1024px)", async () => {
    installMatchMedia(800);
    render(<App />);
    await waitForStartupProbeToSettle();

    expect(screen.queryByTestId("sources-stub")).not.toBeInTheDocument();
    expect(screen.queryByTestId("speakers-stub")).not.toBeInTheDocument();

    const sourcesTrigger = screen.getByRole("button", { name: "Sources" });
    sourcesTrigger.focus();
    expect(sourcesTrigger).toHaveFocus();
    fireEvent.click(sourcesTrigger);

    const sourcesDialog = await screen.findByRole("dialog", {
      name: "Sources",
    });
    expect(sourcesDialog).toHaveFocus();
    expect(
      within(sourcesDialog).getByTestId("sources-stub"),
    ).toBeInTheDocument();

    fireEvent.keyDown(document, { key: "Escape" });
    await waitFor(() =>
      expect(
        screen.queryByRole("dialog", { name: "Sources" }),
      ).not.toBeInTheDocument(),
    );
    expect(sourcesTrigger).toHaveFocus();

    const speakersTrigger = screen.getByRole("button", { name: "Speakers" });
    speakersTrigger.focus();
    expect(speakersTrigger).toHaveFocus();
    fireEvent.click(speakersTrigger);

    const speakersDialog = await screen.findByRole("dialog", {
      name: "Speakers",
    });
    expect(speakersDialog).toHaveFocus();

    fireEvent.keyDown(document, { key: "Escape" });
    await waitFor(() =>
      expect(
        screen.queryByRole("dialog", { name: "Speakers" }),
      ).not.toBeInTheDocument(),
    );
    expect(speakersTrigger).toHaveFocus();
  });

  it("opening one drawer closes the other — at most one drawer open at a time", async () => {
    installMatchMedia(800);
    render(<App />);
    await waitForStartupProbeToSettle();

    fireEvent.click(screen.getByRole("button", { name: "Sources" }));
    await screen.findByRole("dialog", { name: "Sources" });

    fireEvent.click(screen.getByRole("button", { name: "Speakers" }));
    await screen.findByRole("dialog", { name: "Speakers" });
    expect(
      screen.queryByRole("dialog", { name: "Sources" }),
    ).not.toBeInTheDocument();
  });

  it("closes an open drawer if a resize pins its region again, and moves focus to the now-visible panel rather than dropping it to <body>", async () => {
    const control = installMatchMedia(800);
    render(<App />);
    await waitForStartupProbeToSettle();

    fireEvent.click(screen.getByRole("button", { name: "Speakers" }));
    await screen.findByRole("dialog", { name: "Speakers" });

    // Resize up to `wide`: the aside becomes pinned again, so its drawer
    // must not linger open behind the now-visible inline panel. The trigger
    // that was `useFocusTrap`'s restore-focus target unmounts in this same
    // commit (it's `!asidePinned`-gated), so without an explicit redirect
    // focus would silently fall to `<body>`.
    act(() => {
      control.setWidth(1400);
    });
    await waitFor(() =>
      expect(
        screen.queryByRole("dialog", { name: "Speakers" }),
      ).not.toBeInTheDocument(),
    );
    expect(screen.getByTestId("speakers-stub")).toBeInTheDocument();
    expect(document.querySelector(".left-panel")).toHaveFocus();
  });

  it("closes an open rail drawer on a resize that re-pins it, and moves focus to the now-visible panel rather than dropping it to <body>", async () => {
    const control = installMatchMedia(800);
    render(<App />);
    await waitForStartupProbeToSettle();

    fireEvent.click(screen.getByRole("button", { name: "Sources" }));
    await screen.findByRole("dialog", { name: "Sources" });

    // Resize up to `standard`: the rail becomes pinned again (though the
    // aside stays a drawer there), so the rail's drawer must not linger
    // open, and focus must not drop to `<body>` once its trigger unmounts.
    act(() => {
      control.setWidth(1100);
    });
    await waitFor(() =>
      expect(
        screen.queryByRole("dialog", { name: "Sources" }),
      ).not.toBeInTheDocument(),
    );
    expect(screen.getByTestId("sources-stub")).toBeInTheDocument();
    expect(document.querySelector(".left-panel")).toHaveFocus();
  });
});
