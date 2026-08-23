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

// App mounts several heavy/async children (settings & sessions modals) that
// are irrelevant to the B20 hand-off orchestration under test. Stub them so
// the render stays synchronous and dependency-light. (SHELL-R4: this file no
// longer mocks `KnowledgeGraphViewer`/`ProjectionRuntimeStatusPanel`/
// `ChatSidebar` — App.tsx doesn't import any of them post-R4, and
// `SessionsBrowser` (their only remaining importer) is itself mocked
// wholesale below as `sessions-stub`, so none of the three can ever mount in
// this file regardless; keeping their mocks around asserted a fact that had
// already gone permanently true, see the follow-up fix that removed the
// corresponding `queryByTestId` checks.)
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
vi.mock("./components/TokenUsagePanel", () => ({
  default: () => <div data-testid="tokens-stub" />,
}));
// Ticket W5 (synthesis audio-graph-a6b5): the document tile's body/header
// stopped hosting `NotesPanel` (that file is untouched — it still hosts the
// Sessions detail's Notes lens, mocked wholesale above as `sessions-stub`)
// and now hosts `LiveDocument`/`LiveDocumentHeaderActions`. `useLiveDocumentModel`
// is stubbed to an empty VM so the body stub below doesn't need a seeded
// `materializedNotes` store shape.
vi.mock("./components/workspace/LiveDocument", () => ({
  LiveDocument: () => <div data-testid="live-document-stub" />,
  LiveDocumentHeaderActions: () => null,
  DocRecencyChip: () => null,
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
// Ticket W9 (review finding: the previous version of this mock stubbed
// `useAgentQueueFilter`/`AgentQueueFilterToggle` to `["signal", () => {}]`/
// `() => null`, so NO test in this file could observe whether `App.tsx`
// actually threads `agentQueueFilter` into `AgentProposalsPanel`'s `filter`
// prop — severing `filter={agentQueueFilter}` entirely still passed every
// assertion. `useAgentQueueFilter`/`AgentQueueFilterToggle` are kept REAL
// via `importActual` instead — the exact `useGraphStripMode` precedent
// below (same file, same rationale) — while only the panel's rendered BODY
// (`default`) and `AgentTileHeaderActions` are stubbed. The stub renders
// the received `filter` prop into a `data-agent-queue-filter` attribute so
// a test can assert end-to-end that clicking the REAL toggle actually
// changes what's threaded into the REAL panel body prop.
vi.mock("./components/AgentProposalsPanel", async () => {
  const actual = await vi.importActual<
    typeof import("./components/AgentProposalsPanel")
  >("./components/AgentProposalsPanel");
  return {
    ...actual,
    default: ({ filter }: { filter?: string } = {}) => (
      <div data-testid="agent-stub" data-agent-queue-filter={filter} />
    ),
    // Ticket W8: the tile's Clear action moved out of the panel body into
    // `WorkspaceTile`'s `headerSlot` (`App.tsx`) — the mock must export it
    // too, or mounting the agent tile throws "no such export" under this
    // file's module-level `vi.mock`.
    AgentTileHeaderActions: () => null,
  };
});
// Ticket W7 (synthesis audio-graph-a6b5): the graph tile's KG strip — its
// rendered content reads store slices unrelated to the hand-off flow, same
// rationale as the other panel stubs above. `useGraphStripMode` is kept
// REAL (via `importActual`), not stubbed: it's a plain `localStorage`-backed
// hook (the `AudioSourceSelector` `processScope` precedent) with no async
// I/O, and the canvas row-swap wiring test below needs it to actually read
// `localStorage`.
vi.mock("./components/workspace/LiveGraphStrip", async () => {
  const actual = await vi.importActual<
    typeof import("./components/workspace/LiveGraphStrip")
  >("./components/workspace/LiveGraphStrip");
  return {
    ...actual,
    LiveGraphStrip: () => <div data-testid="graph-stub" />,
    GraphStripModeSwitcher: () => null,
  };
});
// SHELL-R3 (plan §R3, ADR-0046): ControlBar -> NowStrip.
vi.mock("./components/NowStrip", () => ({
  default: () => <div data-testid="controlbar-stub" />,
}));
// A minimal dialog-shaped stub (same convention as the ExpressSetup stub
// above) so App-level tests can prove the ACTUAL `systemDrawerOpen` mount
// wiring in App.tsx fires, without needing SystemDrawer's own nested
// TokenUsagePanel/ProjectionRuntimeStatusPanel/PipelineStageDetail fixtures
// (those are covered in SystemDrawer.test.tsx).
vi.mock("./components/SystemDrawer", () => ({
  default: ({ onClose }: { onClose: () => void }) => (
    <div role="dialog" aria-label="System status">
      <button type="button" onClick={onClose}>
        Close
      </button>
    </div>
  ),
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

  it("keeps setup visible when the saved keys are reported present:false (revoked/absent, not just missing)", async () => {
    // `hasConfiguredDurableNotesRoute` filters `presence` down to
    // `present: true` entries before checking a provider's
    // `credential_keys` against them (`utils/durableRoute.ts`). No existing
    // fixture anywhere in the repo seeds a `present: false` entry — this
    // pins that the filter is load-bearing, not a no-op: an otherwise
    // fully-configured Deepgram → OpenRouter route must NOT read as
    // configured when the backend reports both keys as absent.
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "load_credential_presence_cmd") {
        return [
          { key: "deepgram_api_key", present: false, source: "missing" },
          { key: "openrouter_api_key", present: false, source: "missing" },
        ] satisfies CredentialPresence[];
      }
      if (cmd === "load_settings_cmd") return startupSettings();
      if (cmd === "get_model_status") return readyModels;
      if (cmd === "list_aws_profiles") return [];
      return undefined;
    });
    render(<App />);

    expect(
      await screen.findByRole("dialog", { name: /quick setup/i }),
    ).toBeInTheDocument();
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

  // SHELL-R5 (plan §R5, ADR-0046): rewritten. The genuinely-idle Capture
  // surface no longer renders the live Notes/Transcript panels with nothing
  // in them yet (the "empty live cockpit" this unit fixes) — it renders
  // `PreflightCard` instead. `PreflightCard.test.tsx` owns the card's own
  // pass/fail row logic; this test only pins that App.tsx actually mounts
  // it here, not the stubbed live panels.
  it("starts in the Ready workspace showing the preflight card, not empty notes/transcript panels", async () => {
    mockCredentialPresence("openai_api_key");
    render(<App />);

    await waitFor(() =>
      expect(mockedInvoke).toHaveBeenCalledWith("load_credential_presence_cmd"),
    );

    expect(screen.getByRole("tab", { name: /ready/i })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    expect(screen.getByTestId("preflight-card")).toBeInTheDocument();
    expect(screen.queryByTestId("live-document-stub")).not.toBeInTheDocument();
    expect(screen.queryByTestId("transcript-stub")).not.toBeInTheDocument();
  });

  // Review fix (mutation-probe finding, SHELL-R5): `showPreflightCard`'s
  // `!isCapturing` conjunct had no test pinning it — a mutant that dropped
  // that leg left the ENTIRE suite green, because every other test either
  // stays idle (never flips `isCapturing`) or is the "renders live assist
  // inline" test below, which only pins `!hasAgentActivity`. Without this
  // leg, a LIVE capture session would render the preflight checklist instead
  // of the NotesPanel/LiveTranscript cockpit — the exact inverse of the
  // empty-cockpit bug SHELL-R5 exists to fix.
  it("swaps the preflight card for the live Notes/Transcript workspace once capture starts (mutation-probe: showPreflightCard's !isCapturing leg)", async () => {
    mockCredentialPresence("openai_api_key");
    render(<App />);

    await waitForStartupProbeToSettle();
    expect(screen.getByTestId("preflight-card")).toBeInTheDocument();

    act(() => {
      useAudioGraphStore.setState({ isCapturing: true });
    });

    expect(screen.queryByTestId("preflight-card")).not.toBeInTheDocument();
    expect(screen.getByTestId("live-document-stub")).toBeInTheDocument();
    expect(screen.getByTestId("transcript-stub")).toBeInTheDocument();
  });

  // SHELL-R4 (plan §R4, ADR-0046) deleted the "Inspect" tab and its
  // App.tsx-owned graph/diagnostics panel outright — every occupant already
  // has a home (Graph/Route lenses on SessionsBrowser's own Sessions
  // destination, R2; diagnostics in the NowStrip System drawer, R3), both
  // covered by their own component test files. There is no more App-level
  // "switch to Inspect" case to test, so this test is removed rather than
  // rewritten (a design signal explicitly anticipated by the plan, not an
  // oversight).

  it("routes loaded historical sessions to Review", async () => {
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
    expect(screen.queryByTestId("live-document-stub")).not.toBeInTheDocument();
    expect(screen.queryByTestId("transcript-stub")).not.toBeInTheDocument();
  });

  // SHELL-R4: rewritten to navigate away via the Sessions tab (formerly
  // "Review", the same visible label) instead of the deleted Inspect tab.
  // The "Live session" text this test used to also assert on lived in the
  // (unmocked, pre-R4) destination bar; SHELL-R4 relocated that region onto
  // `NowStrip`, which this file mocks wholesale (line ~57) — that fact is
  // now covered by `App.contract.test.tsx` (where NowStrip is real) and
  // `shell.e2e.ts`, not here, so it is intentionally dropped rather than
  // rewritten against the stub.
  it("returns to the capture destination and restores transcript focus when capture starts from Sessions", async () => {
    mockCredentialPresence("openai_api_key");
    useAudioGraphStore.setState({ rightPanelTab: "chat" });
    render(<App />);

    fireEvent.click(screen.getByRole("tab", { name: /review/i }));
    expect(screen.getByRole("tab", { name: /review/i })).toHaveAttribute(
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
    // `rightPanelTab`'s reset-to-"transcript" write survives SHELL-R4 as a
    // now-UI-orphaned store field (see App.tsx's `isCapturing` effect
    // comment) — still real, testable store behavior even with no renderer
    // left to observe it.
    expect(useAudioGraphStore.getState().rightPanelTab).toBe("transcript");
  });

  // SHELL-R4: rewritten for the 2-tab destination bar. `handleWorkspaceViewKeyDown`
  // is length-generic (App.tsx), so ArrowRight (forward), Home (jump to
  // first), and End (jump to last) each still exercise a distinct branch of
  // its switch even with only two tabs — the sequence below lands each key
  // where it's the only one that could produce that transition (Home from
  // "sessions" and End from "capture" both move, so neither is a same-tab
  // no-op): ArrowRight covers the forward branch, Home covers the
  // jump-to-first branch, and the trailing End (from "capture", the
  // already-first tab) covers the jump-to-last branch that no other suite
  // exercises — `shell.e2e.ts` cannot cover it, since its own comment
  // documents that the embedded WebKitGTK provider silently drops Home/End
  // key events.
  it("supports roving keyboard navigation across the capture/sessions destination tabs", async () => {
    mockCredentialPresence("openai_api_key");
    render(<App />);

    const capture = screen.getByRole("tab", { name: /ready/i });
    capture.focus();
    fireEvent.keyDown(capture, { key: "ArrowRight" });

    const sessions = screen.getByRole("tab", { name: /review/i });
    expect(sessions).toHaveAttribute("aria-selected", "true");
    expect(sessions).toHaveFocus();

    fireEvent.keyDown(sessions, { key: "Home" });

    expect(capture).toHaveAttribute("aria-selected", "true");
    expect(capture).toHaveFocus();

    fireEvent.keyDown(capture, { key: "End" });

    expect(sessions).toHaveAttribute("aria-selected", "true");
    expect(sessions).toHaveFocus();
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

  // Ticket W4 (synthesis audio-graph-a6b5), ratified R3: the agent tile
  // region is now ALWAYS mounted, even with zero agent activity — the old
  // `hasAgentActivity &&` gate around it is gone (that flag survives only
  // for `showPreflightCard`'s get-started exclusion, covered above by
  // "starts in the Ready workspace showing the preflight card"). This is
  // the mutation-probe complement to the test above: that one proves
  // agent-stub renders WITH activity; this one proves it also renders
  // WITHOUT activity, so a regression that reintroduces the gate is caught
  // regardless of which direction it breaks.
  it("mounts all four bento tile regions during a live capture, including the agent tile with zero agent activity (R3)", async () => {
    mockCredentialPresence("openai_api_key");
    render(<App />);

    await waitForStartupProbeToSettle();

    act(() => {
      useAudioGraphStore.setState({ isCapturing: true });
    });

    expect(screen.getByTestId("live-document-stub")).toBeInTheDocument();
    expect(screen.getByTestId("graph-stub")).toBeInTheDocument();
    expect(screen.getByTestId("agent-stub")).toBeInTheDocument();
    expect(screen.getByTestId("transcript-stub")).toBeInTheDocument();
  });

  // DOM-order pin (ticket W4, responsive-memo-72d4 §4 item 9, RATIFIED R2
  // override): the compact single-column stack order is document, graph,
  // agent, transcript — NOT the memo's own draft order (graph-first) — and
  // `App.tsx`'s source order must match it exactly so wide/standard tiers
  // can reorder purely via `grid-template-areas` (never `order:`) while
  // compact's reading order stays correct by construction.
  it("orders the bento tiles in the DOM as document, graph, agent, transcript (compact stack order, R2)", async () => {
    mockCredentialPresence("openai_api_key");
    render(<App />);

    await waitForStartupProbeToSettle();

    act(() => {
      useAudioGraphStore.setState({ isCapturing: true });
    });

    const capturePanel = document.getElementById("workspace-panel-capture");
    const tiles = Array.from(
      capturePanel?.querySelectorAll("[data-tile]") ?? [],
    );
    expect(tiles.map((tile) => tile.getAttribute("data-tile"))).toEqual([
      "document",
      "graph",
      "agent",
      "transcript",
    ]);
  });

  // audio-graph-586b review follow-up: the notice render condition and the
  // `data-diarization-degraded` grid-row attribute flip had zero behavioral
  // coverage — both were only pinned by CSS-source-regex contract tests
  // (`layout.bento.contract.test.ts`) plus a manual `dist/` grep, so a
  // mutation that dropped either the notice's render guard or the attribute
  // entirely (App.tsx) survived the full suite. This pair proves the real
  // banner mounts (and the attribute flips) ONLY while
  // `pipelineStatus.diarization.type === "Degraded"`, and that both are
  // absent otherwise — catching a regression in either direction.
  it("mounts the diarization degradation notice and flips data-diarization-degraded when the backend reports Degraded", async () => {
    mockCredentialPresence("openai_api_key");
    render(<App />);

    await waitForStartupProbeToSettle();

    act(() => {
      useAudioGraphStore.setState({ isCapturing: true });
    });

    const capturePanel = document.getElementById("workspace-panel-capture");
    expect(capturePanel).not.toHaveAttribute("data-diarization-degraded");
    expect(
      screen.queryByTestId("diarization-degradation-notice"),
    ).not.toBeInTheDocument();

    act(() => {
      const current = useAudioGraphStore.getState().pipelineStatus;
      useAudioGraphStore.setState({
        pipelineStatus: {
          ...current,
          // `reason` is the backend's stable snake_case code (review
          // follow-up, audio-graph-586b — see
          // `DiarizationDegradationNotice.tsx`'s doc comment), not English
          // prose; the component translates it via
          // `pipeline.diarizationDegradedReason.<code>`.
          diarization: { type: "Degraded", reason: "engine_not_compiled" },
        },
      });
    });

    expect(capturePanel).toHaveAttribute("data-diarization-degraded", "true");
    const notice = screen.getByTestId("diarization-degradation-notice");
    expect(notice).toBeInTheDocument();
    expect(notice).toHaveTextContent(
      "Speaker detection is running in basic mode",
    );
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
    expect(screen.queryByTestId("live-document-stub")).not.toBeInTheDocument();
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

    // Fallback clears; the real idle workspace renders again — SHELL-R5:
    // that's the preflight card now, not the live Notes/Transcript panels
    // (there's no capture running yet, just a now-runnable route).
    await waitFor(() =>
      expect(
        screen.queryByTestId("get-started-fallback"),
      ).not.toBeInTheDocument(),
    );
    expect(screen.getByTestId("preflight-card")).toBeInTheDocument();
    expect(screen.queryByTestId("live-document-stub")).not.toBeInTheDocument();
    expect(screen.queryByTestId("transcript-stub")).not.toBeInTheDocument();
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
    // Default destination is Capture (SHELL-R4, plan §R4, ADR-0046 renamed
    // the id from `#workspace-panel-during`); the link points at that
    // panel's id, and the panel is a <main> landmark with the matching id.
    expect(skip).toHaveAttribute("href", "#workspace-panel-capture");
    const main = document.getElementById("workspace-panel-capture");
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

// SHELL-R3 review fix: `NowStrip`'s composite health chip only calls
// `setSystemDrawerOpen(true)` — it never asserts App actually mounts the
// drawer in response. Pin the App.tsx `{systemDrawerOpen && <SystemDrawer
// .../>}` wiring directly via `setState`, the same pattern the retired
// overlays' fixtures used, so deleting that conditional mount fails a test.
describe("App — System drawer mount wiring (SHELL-R3, ADR-0046)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    localStorage.clear();
    mockedInvoke.mockReset();
    mockCredentialPresence("openai_api_key");
    seedStore();
  });

  afterEach(async () => {
    await i18n.changeLanguage("en");
    localStorage.clear();
  });

  it("mounts SystemDrawer only once systemDrawerOpen flips true, and its onClose clears the flag", async () => {
    render(<App />);
    expect(
      screen.queryByRole("dialog", { name: /system status/i }),
    ).not.toBeInTheDocument();

    await act(async () => {
      useAudioGraphStore.setState({ systemDrawerOpen: true });
    });
    const dialog = await screen.findByRole("dialog", {
      name: /system status/i,
    });
    expect(dialog).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /close/i }));
    expect(useAudioGraphStore.getState().systemDrawerOpen).toBe(false);
    expect(
      screen.queryByRole("dialog", { name: /system status/i }),
    ).not.toBeInTheDocument();
  });
});

// Ticket W7 (synthesis audio-graph-a6b5): the W4-deferred canvas row-swap.
// `useGraphStripMode` is kept real in this file's top-level mock
// specifically so this reads `localStorage` for real, exercising the exact
// wiring `App.tsx` ships: the persisted mode choice both renders
// `LiveGraphStrip` AND flags the grid container's `data-graph-mode`
// attribute — the fix agent's readout this ticket honors requires the
// attribute present ONLY for `"canvas"`, never any other mode's string
// value and never an empty-string placeholder.
describe("App — KG strip canvas row-swap attribute wiring (ticket W7, synthesis audio-graph-a6b5)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    localStorage.clear();
    mockedInvoke.mockReset();
    mockCredentialPresence("openai_api_key");
    seedStore();
  });

  afterEach(async () => {
    await i18n.changeLanguage("en");
    localStorage.clear();
  });

  it('sets data-graph-mode="canvas" on the capture container when the persisted graph strip mode is canvas', async () => {
    localStorage.setItem("ag.graphStripMode", "canvas");
    render(<App />);
    await waitForStartupProbeToSettle();
    act(() => {
      useAudioGraphStore.setState({ isCapturing: true });
    });

    const capturePanel = document.getElementById("workspace-panel-capture");
    expect(capturePanel).toHaveAttribute("data-graph-mode", "canvas");
  });

  it("omits data-graph-mode entirely for the default (focus) mode — never renders the attribute with a non-canvas value", async () => {
    render(<App />);
    await waitForStartupProbeToSettle();
    act(() => {
      useAudioGraphStore.setState({ isCapturing: true });
    });

    const capturePanel = document.getElementById("workspace-panel-capture");
    expect(capturePanel).not.toHaveAttribute("data-graph-mode");
  });

  it('omits data-graph-mode entirely for the feed mode too (only "canvas" ever sets it)', async () => {
    localStorage.setItem("ag.graphStripMode", "feed");
    render(<App />);
    await waitForStartupProbeToSettle();
    act(() => {
      useAudioGraphStore.setState({ isCapturing: true });
    });

    const capturePanel = document.getElementById("workspace-panel-capture");
    expect(capturePanel).not.toHaveAttribute("data-graph-mode");
  });
});

// GREP-PIN (review finding: this file's `AgentProposalsPanel` mock stubs
// `AgentTileHeaderActions` to `() => null` — same as `LiveDocumentHeaderActions`
// (W5 precedent) — so a regression that deletes the real headerSlot wiring in
// App.tsx would render identically here and pass every render-based
// assertion in this suite. A source-text pin is the cheap way to keep that
// wiring load-bearing without rearchitecting the header-actions mocking
// convention this file shares with `App.contract.test.tsx` /
// `App.shellLayout.test.tsx`.)
describe("agent tile headerSlot wiring — grep-pin (ticket W8 review finding, updated ticket W9)", () => {
  it("App.tsx wires AgentTileHeaderActions AND the ticket W9 Signal/All toggle into the agent WorkspaceTile's headerSlot, with the real filter value threaded to both", async () => {
    const { readFileSync } = await import("node:fs");
    const source = readFileSync("src/App.tsx", "utf8");
    const agentTileSource = source.slice(source.indexOf('id="agent"'));
    const headerSlotSource = agentTileSource.slice(
      0,
      agentTileSource.indexOf("</WorkspaceTile>"),
    );
    expect(headerSlotSource).toMatch(/<AgentTileHeaderActions\s*\/>/);
    expect(headerSlotSource).toMatch(/<AgentQueueFilterToggle\b/);
    // Review finding: presence of the elements alone doesn't prove the
    // lifted filter state is actually threaded to either consumer — a
    // regression severing `filter={agentQueueFilter}` (or hardcoding
    // `mode="signal"`) left every other assertion in this file green. This
    // is intentionally belt-and-suspenders with the real end-to-end test
    // below (which exercises the real hook via `importActual`), not a
    // substitute for it.
    expect(headerSlotSource).toMatch(/mode=\{agentQueueFilter\}/);
    expect(headerSlotSource).toMatch(/onModeChange=\{setAgentQueueFilter\}/);
    // `filter={agentQueueFilter}` lives on `<AgentProposalsPanel>`, inside
    // the tile's children — after the `headerSlot` prop closes — so this
    // checks the WHOLE agent-tile slice, not just the headerSlot substring.
    expect(agentTileSource).toMatch(
      /<AgentProposalsPanel\s+filter=\{agentQueueFilter\}/,
    );
  });

  /**
   * REAL END-TO-END WIRING PROOF (review finding, majors: the previous
   * mock stubbed `useAgentQueueFilter`/`AgentQueueFilterToggle`, so nothing
   * in this file could ever observe `App.tsx`'s actual composition —
   * mirrors this SAME file's `useGraphStripMode` precedent, kept real for
   * the identical reason). `useAgentQueueFilter`/`AgentQueueFilterToggle`
   * are the REAL implementations here (only the panel BODY is stubbed);
   * clicking the real toggle must change the `filter` value the real
   * `AgentProposalsPanel` call site receives. A mutation that severs
   * `filter={agentQueueFilter}` (stub always sees `undefined`) or that
   * hardcodes `mode="signal"` (the toggle never flips) fails this test.
   */
  it("clicking the real Signal/All toggle changes the filter value threaded into AgentProposalsPanel's real filter prop", async () => {
    render(<App />);
    await waitForStartupProbeToSettle();

    const stub = screen.getByTestId("agent-stub");
    expect(stub).toHaveAttribute("data-agent-queue-filter", "signal");

    const allTab = screen.getByRole("tab", { name: "All" });
    expect(allTab).toHaveAttribute("aria-selected", "false");
    fireEvent.click(allTab);

    expect(allTab).toHaveAttribute("aria-selected", "true");
    expect(screen.getByTestId("agent-stub")).toHaveAttribute(
      "data-agent-queue-filter",
      "all",
    );

    const signalTab = screen.getByRole("tab", { name: "Signal" });
    fireEvent.click(signalTab);
    expect(screen.getByTestId("agent-stub")).toHaveAttribute(
      "data-agent-queue-filter",
      "signal",
    );
  });
});
