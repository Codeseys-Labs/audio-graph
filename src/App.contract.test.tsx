/**
 * Contract test (seed audio-graph-8d89, ticket UI-T1) — pins facts
 * `e2e/specs/shell.e2e.ts` depends on into the fast jsdom suite, so a
 * regression is caught in `bun run test` instead of only in the slow,
 * CI-only, real-binary WebdriverIO run.
 *
 * Unlike `App.test.tsx` (which stubs `ControlBar` and `AudioSourceSelector`
 * to focus on the onboarding hand-off flow), this file renders BOTH real —
 * they own the exact DOM contracts (`button[aria-label="Start"]`,
 * `role="checkbox"` source rows) the e2e suite queries by.
 *
 * Two kinds of assertion live in this file, and each `it()` says which:
 *
 * - **E2E-pinned facts**: traces to a specific `shell.e2e.ts` line/comment;
 *   verified by grepping the spec for the quoted literal or attribute. These
 *   are the ones a regression in the real binary would actually catch, so
 *   this file exists to catch them earlier, in jsdom.
 * - **jsdom-only a11y/behavior extensions**: real assertions this file
 *   chooses to make (roving tabindex internals, `aria-labelledby`/
 *   `role="tabpanel"` on the workspace panel, `aria-checked` on source rows,
 *   the true wraparound boundary-cross, the exact `workspace.stateIdle`
 *   copy) that go beyond what `shell.e2e.ts` itself asserts. `shell.e2e.ts`'s
 *   own panel assertion, for example, is only `toBeDisplayed()` (line 257) —
 *   it never checks `aria-labelledby` or `role`, so that fact is NOT
 *   e2e-pinned even though it lives in the same `it()` as ones that are.
 *   These extensions are worth keeping (they catch real regressions this
 *   file's author judged worth catching) but must not be described as
 *   e2e-traced.
 *
 * Facts NOT pinned here at all, because `shell.e2e.ts` does not depend on
 * them (verified by grepping the spec for every quoted literal): source
 * ordering/count beyond the mocked row, "Refresh sources"'s own copy (it is
 * a literal string in `AudioSourceSelector.tsx`, not an i18n key — pinning
 * its i18n *value* would be fabricated), and window title (comes from
 * `tauri.conf.json`, not `en.json`).
 */
import { invoke } from "@tauri-apps/api/core";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import i18n from "./i18n";
import { useAudioGraphStore } from "./store";
import type { AppSettings, AudioSourceInfo, CredentialPresence } from "./types";

// Heavy/lazy or irrelevant-to-this-contract children are stubbed exactly like
// `App.test.tsx` — ControlBar, AudioSourceSelector, and Notifications are
// deliberately left REAL (see file doc comment).
vi.mock("./components/KnowledgeGraphViewer", () => ({
  default: () => <div data-testid="graph-stub" />,
}));
vi.mock("./components/SettingsPage", () => ({
  default: () => <div data-testid="settings-stub" />,
}));
vi.mock("./components/SessionsBrowser", () => ({
  default: () => <div data-testid="sessions-stub" />,
}));
vi.mock("./components/ExpressSetup", () => ({
  default: () => <div data-testid="express-setup-stub" />,
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

const mockedInvoke = vi.mocked(invoke);

// A configured Deepgram+OpenRouter credential pair (mirrors App.test.tsx's own
// convention) keeps ExpressSetup suppressed so the always-mounted chrome
// (ControlBar, workspace tabs, AudioSourceSelector) is what's under test.
const SETTINGS: AppSettings = {
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

function credentialPresence(...keys: string[]): CredentialPresence[] {
  return keys.map((key) => ({
    key,
    present: true,
    source: "credentials_yaml",
  }));
}

// Verbatim shape of `e2e/specs/shell.e2e.ts`'s `MOCK_SOURCE` (lines 109-124):
// the minimal `AudioSourceInfo` that renders as a normal, selectable
// `role="checkbox"` row instead of an "unsupported" one.
const MOCK_SOURCE: AudioSourceInfo = {
  id: "e2e-mock-mic",
  name: "E2E Mock Microphone",
  source_type: { type: "SystemDefault" },
  is_active: false,
  device_kind: "Input",
  capabilities: {
    backend_name: "e2e-mock",
    capture_supported: true,
    supports_system_capture: true,
    supports_application_capture: false,
    supports_process_tree_capture: false,
    supports_device_selection: false,
    supports_device_change_notifications: false,
  },
};

/** Tracks `start_capture` call count so the 2nd call rejects, mirroring the
 * e2e mock sequence at lines 302-306 (`mockResolvedValueOnce` then
 * `mockRejectedValueOnce`). */
let startCaptureCalls = 0;

function installInvokeMocks() {
  startCaptureCalls = 0;
  mockedInvoke.mockImplementation(async (cmd: string) => {
    switch (cmd) {
      case "load_credential_presence_cmd":
        return credentialPresence("deepgram_api_key", "openrouter_api_key");
      case "load_settings_cmd":
        return SETTINGS;
      case "list_audio_sources":
        return [MOCK_SOURCE];
      case "list_running_processes":
        return [];
      case "start_capture":
        startCaptureCalls += 1;
        if (startCaptureCalls === 1) return null;
        throw new Error("e2e-mocked-capture-failure");
      case "stop_capture":
        return null;
      default:
        return undefined;
    }
  });
}

function seedChromeStoreDefaults() {
  // Data-only reset (no action overrides): ControlBar/AudioSourceSelector's
  // REAL store actions (`startCapture`, `toggleSourceId`, `fetchSources`, …)
  // must run unmocked so a click genuinely round-trips through the same
  // `store/index.ts` code paths the real app (and the e2e binary) uses.
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
    searchFilter: "",
    sourceRecoveryIntent: null,
    isCapturing: false,
    isTranscribing: false,
    isGeminiActive: false,
    captureStartTime: null,
    backpressuredSources: [],
    settings: null,
    modelStatus: null,
    agentStatus: null,
    agentProposals: [],
    liveAssistCards: [],
    conversationMode: "notes",
    converseEngine: "pipelined",
    converseRealtimeAgentProvider: "gemini",
    notifications: [],
    error: null,
  });
}

async function waitForStartupProbeToSettle() {
  await waitFor(() =>
    expect(
      document.querySelector('[data-onboarding-probe="settled"]'),
    ).toBeInTheDocument(),
  );
}

/** Finds the mocked source's `role="checkbox"` row the way the e2e suite
 * does (lines 319-329): scan every checkbox row's text, not an aria-label —
 * the row has no accessible name of its own, only visible text content. */
async function findMockSourceRow(): Promise<HTMLElement> {
  return await waitFor(() => {
    const rows = screen.getAllByRole("checkbox");
    const match = rows.find((row) =>
      (row.textContent ?? "").includes(MOCK_SOURCE.name),
    );
    if (!match) throw new Error("mock source row not yet rendered");
    return match;
  });
}

describe("App shell contract — pins e2e/specs/shell.e2e.ts facts (audio-graph-8d89)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    localStorage.clear();
    mockedInvoke.mockReset();
    installInvokeMocks();
    seedChromeStoreDefaults();
  });

  afterEach(async () => {
    await i18n.changeLanguage("en");
    localStorage.clear();
  });

  // ── shell.e2e.ts test 3, lines 253-258: clicking each tab by id flips
  // aria-selected and displays its panel. E2E-pinned: the tab ids, the
  // aria-selected flip, and the panel's `toBeDisplayed()` (the ONE fact the
  // spec actually asserts about the panel — it never checks the panel's
  // `aria-labelledby` or `role`).
  //
  // jsdom-only extensions in the same test: `aria-labelledby` and
  // `role="tabpanel"` on the panel, and that the other two tabs deselect.
  // None of these three appear in shell.e2e.ts; they are real accessibility
  // invariants this file additionally chooses to guard, not e2e-traced ones.
  it("wires #workspace-tab-{during,after,analysis} to #workspace-panel-* via aria-labelledby, flipping aria-selected on click", async () => {
    render(<App />);
    await waitForStartupProbeToSettle();

    for (const view of ["during", "after", "analysis"] as const) {
      const tab = document.getElementById(`workspace-tab-${view}`);
      expect(tab).not.toBeNull();
      fireEvent.click(tab as HTMLElement);

      // E2E-pinned (line 256).
      expect(tab).toHaveAttribute("aria-selected", "true");
      const panel = document.getElementById(`workspace-panel-${view}`);
      // E2E-pinned (line 257: `toBeDisplayed()`) — existence + visibility.
      expect(panel).not.toBeNull();
      // jsdom-only extensions below: shell.e2e.ts never asserts these.
      expect(panel).toHaveAttribute("aria-labelledby", `workspace-tab-${view}`);
      expect(panel).toHaveAttribute("role", "tabpanel");

      // jsdom-only extension: the other two tabs must be deselected — a real
      // flip, not a sticky one. Not asserted by the spec.
      for (const other of ["during", "after", "analysis"] as const) {
        if (other === view) continue;
        expect(
          document.getElementById(`workspace-tab-${other}`),
        ).toHaveAttribute("aria-selected", "false");
      }
    }
  });

  // ── jsdom-only extension: roving tabindex (only the selected tab is 0).
  // shell.e2e.ts never asserts a `tabindex` attribute directly; it only
  // relies on roving tabindex being correctly implemented as the reason its
  // `browser.execute()` focus-emulation (lines 260-269) is a faithful stand-in
  // for real Tab-key navigation. This test pins the underlying mechanism the
  // spec's own comment depends on, but not a literal spec assertion.
  it("keeps roving tabindex on the workspace tablist — exactly one tab has tabIndex 0", async () => {
    render(<App />);
    await waitForStartupProbeToSettle();

    const during = document.getElementById(
      "workspace-tab-during",
    ) as HTMLElement;
    const after = document.getElementById("workspace-tab-after") as HTMLElement;
    const analysis = document.getElementById(
      "workspace-tab-analysis",
    ) as HTMLElement;

    expect(during).toHaveAttribute("tabindex", "0");
    expect(after).toHaveAttribute("tabindex", "-1");
    expect(analysis).toHaveAttribute("tabindex", "-1");

    fireEvent.click(after);
    expect(during).toHaveAttribute("tabindex", "-1");
    expect(after).toHaveAttribute("tabindex", "0");
    expect(analysis).toHaveAttribute("tabindex", "-1");
  });

  // ── shell.e2e.ts test 3, lines 250-294: the exact wraparound path the
  // embedded WebKitGTK driver exercises (Home/End are dropped there, so the
  // real suite substitutes two ArrowLefts / two ArrowRights from "analysis").
  it("wraps ArrowLeft/ArrowRight around the 3-tab workspace list the same way the embedded e2e driver does", async () => {
    render(<App />);
    await waitForStartupProbeToSettle();

    // Mirror the click-through-all-three-tabs step (lines 253-258) before the
    // keyboard portion.
    for (const view of ["during", "after", "analysis"] as const) {
      fireEvent.click(
        document.getElementById(`workspace-tab-${view}`) as HTMLElement,
      );
    }

    const analysis = document.getElementById(
      "workspace-tab-analysis",
    ) as HTMLElement;
    analysis.focus();
    expect(analysis).toHaveFocus();

    // Two ArrowLefts from "analysis" (index 2) wrap through "after" (1) to
    // "during" (0) — shell.e2e.ts lines 279-288.
    fireEvent.keyDown(analysis, { key: "ArrowLeft" });
    const after = document.getElementById("workspace-tab-after") as HTMLElement;
    fireEvent.keyDown(after, { key: "ArrowLeft" });

    const during = document.getElementById(
      "workspace-tab-during",
    ) as HTMLElement;
    expect(during).toHaveAttribute("aria-selected", "true");
    expect(during).toHaveFocus();

    // Two ArrowRights back from "during" return to "analysis" — lines
    // 289-293.
    fireEvent.keyDown(during, { key: "ArrowRight" });
    fireEvent.keyDown(after, { key: "ArrowRight" });

    expect(analysis).toHaveAttribute("aria-selected", "true");
    expect(analysis).toHaveFocus();
  });

  // ── jsdom-only extension (NOT pinned to shell.e2e.ts): a true boundary
  // cross. The e2e path above only ever walks index 2 -> 1 -> 0 and back —
  // it never asks the handler to wrap AROUND an end of the list, so a
  // regression that replaces modulo-wrap with index clamping (in which
  // ArrowLeft from index 0 would stick at "during" instead of wrapping to
  // "analysis") would leave every e2e-pinned assertion (and the wrap test
  // above) green. This assertion exercises exactly that missing case: one
  // ArrowLeft from the FIRST tab ("during", index 0) must land on the LAST
  // tab ("analysis", index 2).
  it("wraps ArrowLeft from the first tab to the last tab (boundary cross the e2e path never exercises)", async () => {
    render(<App />);
    await waitForStartupProbeToSettle();

    const during = document.getElementById(
      "workspace-tab-during",
    ) as HTMLElement;
    during.focus();
    expect(during).toHaveFocus();
    expect(during).toHaveAttribute("aria-selected", "true");

    fireEvent.keyDown(during, { key: "ArrowLeft" });

    const analysis = document.getElementById(
      "workspace-tab-analysis",
    ) as HTMLElement;
    expect(analysis).toHaveAttribute("aria-selected", "true");
    expect(analysis).toHaveFocus();
  });

  // ── shell.e2e.ts test 4, lines 298-339 + 348-364: real Start/Stop button +
  // "Refresh sources" aria-labels, and role=checkbox source rows.
  // jsdom-only extension in this test: the spec finds the source row via
  // `[role="checkbox"]` + text match and clicks it, but never asserts
  // `aria-checked` before or after — the `aria-checked` assertions below are
  // this file's own addition, not e2e-traced.
  it("labels the capture toggle Start/Stop and exposes 'Refresh sources', with real role=checkbox source rows", async () => {
    const { container } = render(<App />);
    await waitForStartupProbeToSettle();

    // shell.e2e.ts queries these by ATTRIBUTE, not accessible name
    // (`$('button[aria-label="Refresh sources"]')` / `$('button[aria-label="Start"]')`
    // at lines 317/338). ControlBar/AudioSourceSelector render both an
    // aria-label AND matching visible text, so a `getByRole(..., { name })`
    // query alone stays green even if the aria-label the e2e binary actually
    // selects on is deleted — pin the attribute itself too.
    expect(
      container.querySelector('button[aria-label="Refresh sources"]'),
    ).not.toBeNull();
    expect(
      container.querySelector('button[aria-label="Start"]'),
    ).not.toBeNull();

    const refreshButton = screen.getByRole("button", {
      name: "Refresh sources",
    });
    fireEvent.click(refreshButton);

    const startButton = screen.getByRole("button", { name: "Start" });
    expect(startButton).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Stop" })).toBeNull();

    const sourceRow = await findMockSourceRow();
    // jsdom-only extension (see file-level comment above): not e2e-traced.
    expect(sourceRow).toHaveAttribute("aria-checked", "false");
    fireEvent.click(sourceRow);
    expect(sourceRow).toHaveAttribute("aria-checked", "true");
    expect(useAudioGraphStore.getState().selectedSourceIds.length).toBe(1);
  });

  // ── shell.e2e.ts test 4, lines 338-364: Start flips `.workspace-switcher__
  // state` to "Live session"; Stop flips it back; a mocked rejection on the
  // 2nd Start surfaces `.notifications .notification--error`, not a hang.
  it("flips .workspace-switcher__state to 'Live session' on Start, back on Stop, and shows .notification--error on a rejected re-Start", async () => {
    const { container } = render(<App />);
    await waitForStartupProbeToSettle();

    fireEvent.click(screen.getByRole("button", { name: "Refresh sources" }));
    const sourceRow = await findMockSourceRow();
    fireEvent.click(sourceRow);

    const startButton = screen.getByRole("button", { name: "Start" });
    fireEvent.click(startButton);

    const stateRegion = await waitFor(() => {
      const el = document.querySelector(".workspace-switcher__state");
      if (!el?.textContent?.includes("Live session")) {
        throw new Error("not live yet");
      }
      return el;
    });
    expect(stateRegion.textContent).toContain("Live session");

    const stopButton = await screen.findByRole("button", { name: "Stop" });
    // shell.e2e.ts selects Stop by attribute too
    // (`$('button[aria-label="Stop"]')`, line 348) — pin it the same way as
    // Start/Refresh above.
    expect(container.querySelector('button[aria-label="Stop"]')).not.toBeNull();
    fireEvent.click(stopButton);
    await waitFor(() =>
      expect(
        document.querySelector(".workspace-switcher__state")?.textContent,
      ).not.toContain("Live session"),
    );

    // Second Start (mocked rejection) must surface via Notifications, not
    // hang the renderer — shell.e2e.ts lines 354-364.
    const restartButton = await screen.findByRole("button", { name: "Start" });
    fireEvent.click(restartButton);

    await waitFor(() =>
      expect(
        document.querySelector(".notifications .notification--error"),
      ).toBeInTheDocument(),
    );
  });

  // ── The English values shell.e2e.ts's assertions are actually load-bearing
  // on. Verified by grepping every quoted literal in the spec file: only
  // "Start" (controlBar.start), "Stop" (controlBar.stop), and "Live session"
  // (workspace.stateLive, asserted verbatim via `toHaveText` at lines
  // 344-345) are real i18n dependencies.
  //
  // `workspace.stateIdle` is a narrower case: the spec's own negative
  // assertion after Stop (lines 350-351: state must NOT contain "Live
  // session") is only meaningful because idle text differs from live text —
  // that's the actual, implicit e2e-pinned fact, and it only requires
  // inequality, not any specific idle string. `expect(...).toBe("Ready")`
  // below is stronger than that: it pins the literal idle copy, which
  // shell.e2e.ts does not itself need. Kept anyway as a jsdom-only
  // extension — a real, useful copy-regression guard — but listed
  // separately so it isn't mistaken for something the spec requires.
  it("pins the English copy of the en.json keys shell.e2e.ts's assertions depend on", () => {
    const t = i18n.getFixedT("en");
    // E2E-pinned facts.
    expect(t("controlBar.start")).toBe("Start");
    expect(t("controlBar.stop")).toBe("Stop");
    expect(t("workspace.stateLive")).toBe("Live session");
    expect(t("workspace.stateIdle")).not.toBe(t("workspace.stateLive"));

    // jsdom-only extension: pins the exact idle copy, stronger than the
    // spec's negative form above.
    expect(t("workspace.stateIdle")).toBe("Ready");
  });
});
