/**
 * Root React component for the AudioGraph Tauri window.
 *
 * Layout (desktop-first):
 *   - Top: `StorageBanner` (ENOSPC retry) + `DemoModeBanner` (first-launch
 *     local-only hint) + `ControlBar` (Start/Stop, settings, sessions).
 *   - Middle 3-column flex:
 *       - Left  aside: `AudioSourceSelector` + `SpeakerPanel`
 *       - Main:         `KnowledgeGraphViewer`
 *       - Right aside: `LiveTranscript` / `ChatSidebar` (tabbed) +
 *                      `TokenUsagePanel`
 *   - Bottom: `PipelineStatusBar` (per-stage status dots).
 *   - Overlays: error toast, `SettingsPage` modal, `SessionsBrowser` modal,
 *     `ShortcutsHelpModal`, first-launch `ExpressSetup` quickstart,
 *     `Notifications` (unified transient feedback + error queue, ADR-0011).
 *
 * Side-effects mounted at the root:
 *   - `useTauriEvents()` subscribes to all backend events exactly once.
 *   - `useKeyboardShortcuts()` registers global hotkeys (Cmd/Ctrl+R, Cmd/Ctrl+,
 *     Cmd/Ctrl+Shift+S, Escape).
 *   - A local `keydown` listener toggles the shortcuts help modal on
 *     Cmd/Ctrl+/ or "?" (outside of typing contexts).
 *
 * First-launch Express Setup is triggered from this component: on mount we
 * probe non-secret saved-credential presence for cloud provider keys from the
 * backend store metadata (desktop keychain first, with YAML import/fallback
 * sources reported when applicable). If the saved credentials do not yet
 * indicate a runnable durable notes/graph cloud pipeline, `ExpressSetup`
 * renders once; dismissal is transient (per-session), not persisted.
 *
 * No props — this component is the app shell.
 */

import { invoke } from "@tauri-apps/api/core";
import { lazy, Suspense, useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import AgentProposalsPanel from "./components/AgentProposalsPanel";
import AudioSourceSelector from "./components/AudioSourceSelector";
import ChatSidebar from "./components/ChatSidebar";
import ControlBar from "./components/ControlBar";
import Icon from "./components/Icon";
import LiveTranscript from "./components/LiveTranscript";
import NotesPanel from "./components/NotesPanel";
import PipelineStatusBar from "./components/PipelineStatusBar";
import ProjectionRuntimeStatusPanel from "./components/ProjectionRuntimeStatusPanel";
import ResizeDivider from "./components/ResizeDivider";
import SessionDataRoutePanel from "./components/SessionDataRoutePanel";
import ShortcutsHelpModal from "./components/ShortcutsHelpModal";
import SpeakerPanel from "./components/SpeakerPanel";
import TokenUsagePanel from "./components/TokenUsagePanel";

// Code-split (ADR-0016 / modernization-audit 2.3): the graph viewer pulls the
// heavy react-force-graph-2d dependency, and these modals/first-run flows are
// rendered conditionally — lazy-loading them keeps the initial bundle lean.
const KnowledgeGraphViewer = lazy(
  () => import("./components/KnowledgeGraphViewer"),
);
const SettingsPage = lazy(() => import("./components/SettingsPage"));
const SessionsBrowser = lazy(() => import("./components/SessionsBrowser"));
const ExpressSetup = lazy(() => import("./components/ExpressSetup"));

import DemoModeBanner from "./components/DemoModeBanner";
import Notifications from "./components/Notifications";
import PopoverOverlay from "./components/PopoverOverlay";
import StorageBanner from "./components/StorageBanner";
import { ONBOARDING_HANDOFF_SEEN_KEY } from "./constants/storageKeys";
import { useConverseFrontLeg } from "./hooks/useConverseFrontLeg";
import { useKeyboardShortcuts } from "./hooks/useKeyboardShortcuts";
import { useTauriEvents } from "./hooks/useTauriEvents";
import { useAudioGraphStore } from "./store";
import type { CredentialPresence } from "./types";
import "./styles/index.css";

// Credential keys that can satisfy each cloud stage in the durable notes/graph
// path. App suppresses Express Setup when saved presence can cover both cloud
// ASR and cloud LLM. The only approved single-key shortcut is OpenAI-compatible
// ASR + LLM via `openai_api_key`; other shared-mode keys are ambiguous because
// they may represent realtime-agent-only setup.
const DURABLE_CLOUD_ASR_CREDENTIAL_KEYS = new Set<string>([
  "openai_api_key",
  "gemini_api_key",
  "deepgram_api_key",
  "assemblyai_api_key",
  "soniox_api_key",
  "gladia_api_key",
  "speechmatics_api_key",
  "revai_api_key",
  "aws_access_key",
]);

const DURABLE_CLOUD_LLM_CREDENTIAL_KEYS = new Set<string>([
  "openai_api_key",
  "cerebras_api_key",
  "openrouter_api_key",
  "groq_api_key",
  "together_api_key",
  "fireworks_api_key",
  "gemini_api_key",
  "aws_access_key",
]);

function hasRunnableDurableCloudCredentialPair(
  presence: readonly CredentialPresence[],
): boolean {
  const presentKeys = new Set(
    presence.filter(({ present }) => present).map(({ key }) => key),
  );
  if (presentKeys.has("openai_api_key")) return true;

  return Array.from(presentKeys).some(
    (asrKey) =>
      DURABLE_CLOUD_ASR_CREDENTIAL_KEYS.has(asrKey) &&
      Array.from(presentKeys).some(
        (llmKey) =>
          llmKey !== asrKey && DURABLE_CLOUD_LLM_CREDENTIAL_KEYS.has(llmKey),
      ),
  );
}

// Persisted panel sizes (px). Kept in localStorage so the user's layout
// survives restarts. Clamped on every drag so panels can't vanish.
const clamp = (v: number, lo: number, hi: number) =>
  Math.max(lo, Math.min(hi, v));
function loadNum(key: string, fallback: number): number {
  try {
    const n = Number(localStorage.getItem(key));
    return Number.isFinite(n) && n > 0 ? n : fallback;
  } catch {
    return fallback;
  }
}
function saveNum(key: string, v: number) {
  try {
    localStorage.setItem(key, String(Math.round(v)));
  } catch {
    /* ignore quota/availability errors */
  }
}

// Post-Express hand-off nudge: shown once after the first-run quickstart is
// dismissed (save/skip) to guide the user toward "select source → Start".
// A simple localStorage flag keeps it a show-once affordance (NN/g: make
// onboarding hints dismissible + non-recurring). Reuses the same persistence
// pattern as the panel sizes above. The key is the shared
// ONBOARDING_HANDOFF_SEEN_KEY (src/constants/storageKeys.ts) so App + the
// "show getting-started again" control in ShortcutsHelpModal can never drift.
const HANDOFF_SEEN_KEY = ONBOARDING_HANDOFF_SEEN_KEY;
function loadHandoffSeen(): boolean {
  try {
    return localStorage.getItem(HANDOFF_SEEN_KEY) === "1";
  } catch {
    return false;
  }
}
// The hand-off is "eligible" to surface whenever its show-once flag is absent.
// ShortcutsHelpModal re-arms by removing the key, so an absent key after the
// help modal closes (or a cross-tab `storage` clear) means the user explicitly
// asked to see the getting-started guide again. Note: a never-seen flag is also
// absent, but App only re-shows on the modal-close / storage transitions below,
// never blindly on mount, so configured users aren't spammed on first launch.
function isHandoffEligible(): boolean {
  return !loadHandoffSeen();
}
function saveHandoffSeen() {
  try {
    localStorage.setItem(HANDOFF_SEEN_KEY, "1");
  } catch {
    /* ignore quota/availability errors */
  }
}

function App() {
  // Subscribe to Tauri backend events
  useTauriEvents();
  // ADR-0013 step 2: feed finalized speech turns into graph-grounded streaming
  // chat when in converse/pipelined mode (no-op otherwise).
  useConverseFrontLeg();
  // Register global keyboard shortcuts (Cmd/Ctrl+R, Cmd/Ctrl+,, Esc, Cmd/Ctrl+Shift+S)
  useKeyboardShortcuts();

  const { t, i18n } = useTranslation();

  const rightPanelTab = useAudioGraphStore((s) => s.rightPanelTab);
  const setRightPanelTab = useAudioGraphStore((s) => s.setRightPanelTab);
  const settingsOpen = useAudioGraphStore((s) => s.settingsOpen);
  const sessionsBrowserOpen = useAudioGraphStore((s) => s.sessionsBrowserOpen);
  const loadedSessionId = useAudioGraphStore((s) => s.loadedSessionId);
  const openSettings = useAudioGraphStore((s) => s.openSettings);
  const loadSampleSessionPreview = useAudioGraphStore(
    (s) => s.loadSampleSessionPreview,
  );
  const samplePreviewActive = useAudioGraphStore((s) => s.samplePreviewActive);
  const agentOverlayOpen = useAudioGraphStore((s) => s.agentOverlayOpen);
  const setAgentOverlayOpen = useAudioGraphStore((s) => s.setAgentOverlayOpen);
  const tokenOverlayOpen = useAudioGraphStore((s) => s.tokenOverlayOpen);
  const setTokenOverlayOpen = useAudioGraphStore((s) => s.setTokenOverlayOpen);

  // First-time setup: on mount, probe non-secret credential presence for a
  // complete durable notes/graph cloud path. Partial configs keep Express Setup
  // visible so it can guide the missing stage without plaintext loadback.
  // Dismissal (save or skip) sets `expressSetupVisible = false` and we never
  // re-probe during this session — the user can reach the same UI via
  // Settings when they're ready.
  const [expressSetupVisible, setExpressSetupVisible] = useState(false);
  // Post-Express hand-off nudge (B20). Shown once, after the quickstart is
  // dismissed, to point the user at "select a source → Start". Dismissible
  // and non-recurring (localStorage show-once).
  const [handoffVisible, setHandoffVisible] = useState(false);
  const dismissExpressSetup = () => {
    setExpressSetupVisible(false);
    if (isHandoffEligible()) setHandoffVisible(true);
  };
  const previewSampleSession = useCallback(() => {
    loadSampleSessionPreview(i18n.resolvedLanguage ?? i18n.language);
    setExpressSetupVisible(false);
    setHandoffVisible(false);
    saveHandoffSeen();
  }, [i18n.language, i18n.resolvedLanguage, loadSampleSessionPreview]);
  // Re-surface the hand-off whenever it's been re-armed (its show-once flag was
  // cleared), regardless of whether ExpressSetup ever popped. This is the fix
  // for configured users: they never see ExpressSetup, so "show getting-started
  // again" used to be a no-op for them. Idempotent + show-once-after-re-arm: it
  // only flips `handoffVisible` on when the flag is currently absent.
  const reEvaluateHandoff = useCallback(() => {
    if (isHandoffEligible()) setHandoffVisible(true);
  }, []);
  // Stable identity so the Escape effect below can depend on it without
  // re-subscribing every render. Closes over only stable setters + the
  // module-level `saveHandoffSeen`.
  const dismissHandoff = useCallback(() => {
    setHandoffVisible(false);
    saveHandoffSeen();
  }, []);
  // SC 1.4.13: the hand-off hint is dismissible via Escape (without moving
  // focus). It never traps focus (SC 2.1.2) and sits above the layout so it
  // doesn't obscure a focused element (SC 2.4.11).
  useEffect(() => {
    if (!handoffVisible) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") dismissHandoff();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [handoffVisible, dismissHandoff]);
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const presence = await invoke<CredentialPresence[]>(
          "load_credential_presence_cmd",
        );
        if (cancelled) return;
        if (!hasRunnableDurableCloudCredentialPair(presence)) {
          setExpressSetupVisible(true);
        }
      } catch {
        // Silently tolerate probe failures — the user can still reach
        // Settings manually.
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // Shortcuts help modal is kept as local UI state rather than in the store —
  // it has no backend tie-in and nothing else observes it.
  const [shortcutsOpen, setShortcutsOpen] = useState(false);
  // The help modal hosts the "show getting-started guide again" control, which
  // re-arms the hand-off by clearing its show-once flag. When the modal closes
  // we re-evaluate eligibility so the banner reappears immediately — even for
  // configured users who never trigger ExpressSetup (App.tsx:159 fix).
  const closeShortcuts = useCallback(() => {
    setShortcutsOpen(false);
    reEvaluateHandoff();
  }, [reEvaluateHandoff]);

  // Cross-tab re-arm: a `storage` event fires in *other* documents when the key
  // is cleared (it never fires same-document — that path is the modal-close
  // handler above). Re-evaluate so a re-arm in one window surfaces the hint in
  // the others too. Keep it dismissible/show-once via the existing flag write.
  useEffect(() => {
    const onStorage = (e: StorageEvent) => {
      if (e.key === HANDOFF_SEEN_KEY && e.newValue === null) {
        reEvaluateHandoff();
      }
    };
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, [reEvaluateHandoff]);

  // Resizable layout sizes (px), persisted across sessions.
  const [leftWidth, setLeftWidth] = useState(() =>
    loadNum("ag.leftWidth", 260),
  );
  const [rightWidth, setRightWidth] = useState(() =>
    loadNum("ag.rightWidth", 340),
  );
  const [notesHeight, setNotesHeight] = useState(() =>
    loadNum("ag.notesHeight", 220),
  );
  const resizeLeft = (dx: number) =>
    setLeftWidth((w) => {
      const n = clamp(w + dx, 200, 520);
      saveNum("ag.leftWidth", n);
      return n;
    });
  const resizeRight = (dx: number) =>
    setRightWidth((w) => {
      // Divider is on the right panel's left edge: dragging right shrinks it.
      const n = clamp(w - dx, 260, 640);
      saveNum("ag.rightWidth", n);
      return n;
    });
  const resizeNotes = (dy: number) =>
    setNotesHeight((h) => {
      // Divider sits above the notes pane: dragging up grows notes.
      const n = clamp(h - dy, 0, 560);
      saveNum("ag.notesHeight", n);
      return n;
    });

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      // Cmd/Ctrl+/ (or Shift+/ → "?") opens the help modal. Skip when typing
      // into inputs so "?" remains typeable.
      const target = e.target as HTMLElement | null;
      const typing =
        !!target &&
        (target.tagName === "INPUT" ||
          target.tagName === "TEXTAREA" ||
          target.isContentEditable);
      if (typing) return;
      const mod = e.metaKey || e.ctrlKey;
      if (mod && e.key === "/") {
        e.preventDefault();
        setShortcutsOpen((open) => !open);
      } else if (!mod && e.key === "?") {
        e.preventDefault();
        setShortcutsOpen((open) => !open);
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);

  // Roving-tabindex keyboard nav for the right-panel tablist (WCAG 4.1.2 /
  // ARIA Authoring Practices): Arrow/Home/End move between tabs and move
  // focus to the newly-selected tab so keyboard users don't get stranded.
  const handleTabKeyDown = (e: React.KeyboardEvent<HTMLButtonElement>) => {
    const NAV = [
      "ArrowRight",
      "ArrowLeft",
      "ArrowUp",
      "ArrowDown",
      "Home",
      "End",
    ];
    if (!NAV.includes(e.key)) return;
    e.preventDefault();
    const next: "transcript" | "chat" =
      e.key === "Home"
        ? "transcript"
        : e.key === "End"
          ? "chat"
          : rightPanelTab === "transcript"
            ? "chat"
            : "transcript";
    setRightPanelTab(next);
    const tablist = e.currentTarget.parentElement;
    const tabs = tablist?.querySelectorAll<HTMLButtonElement>('[role="tab"]');
    tabs?.[next === "transcript" ? 0 : 1]?.focus();
  };

  return (
    <div className="app-container">
      <StorageBanner />
      <DemoModeBanner />
      <ControlBar />
      {handoffVisible && (
        <aside
          className="flex items-center gap-(--space-5) px-(--space-6) py-(--space-3) bg-(--tint-accent-info) border-b border-(--tint-border-info) text-text-primary"
          aria-label={t("onboarding.handoffTitle")}
          // Announce the nudge when it appears: ExpressSetup just closed (its
          // focused element is gone) so SR/keyboard users would otherwise miss
          // the onboarding steps. A polite live region notifies without
          // stealing focus (mirrors ADR-0011 Notifications' status semantics).
          role="status"
          aria-live="polite"
        >
          <span className="font-semibold text-sm shrink-0">
            {t("onboarding.handoffTitle")}
          </span>
          <ol className="flex items-center gap-(--space-5) m-0 p-0 list-none text-sm text-text-secondary">
            <li>
              <span className="mr-(--space-2) font-semibold text-accent-blue">
                1.
              </span>
              {t("onboarding.handoffStep1")}
            </li>
            <li>
              <span className="mr-(--space-2) font-semibold text-accent-blue">
                2.
              </span>
              {t("onboarding.handoffStep2")}
            </li>
          </ol>
          <button
            type="button"
            className="ml-auto shrink-0 py-(--space-2) px-(--space-5) rounded-md text-sm font-semibold cursor-pointer bg-accent-blue text-white border-none hover:opacity-90"
            onClick={dismissHandoff}
            aria-label={t("onboarding.handoffDismissLabel")}
          >
            {t("onboarding.handoffDismiss")}
          </button>
        </aside>
      )}
      <div className="main-layout">
        <aside className="left-panel" style={{ width: leftWidth }}>
          <AudioSourceSelector />
          <SpeakerPanel />
        </aside>
        <ResizeDivider
          orientation="vertical"
          onResize={resizeLeft}
          ariaLabel={t("app.resizeSources")}
        />
        <main className="center-panel">
          <div className="center-panel__graph">
            <Suspense fallback={null}>
              <KnowledgeGraphViewer />
            </Suspense>
          </div>
          <ResizeDivider
            orientation="horizontal"
            onResize={resizeNotes}
            ariaLabel={t("app.resizeNotes")}
          />
          <div className="center-panel__notes" style={{ height: notesHeight }}>
            <NotesPanel />
          </div>
        </main>
        <ResizeDivider
          orientation="vertical"
          onResize={resizeRight}
          ariaLabel={t("app.resizeTranscriptChat")}
        />
        <aside className="right-panel" style={{ width: rightWidth }}>
          <div
            className="flex border-b border-b-border-color bg-bg-secondary shrink-0"
            role="tablist"
            aria-label={t("app.rightPanelViews")}
          >
            <button
              type="button"
              role="tab"
              id="right-tab-transcript"
              aria-selected={rightPanelTab === "transcript"}
              aria-controls="right-tabpanel"
              tabIndex={rightPanelTab === "transcript" ? 0 : -1}
              className={`flex-1 flex items-center justify-center gap-(--space-3) py-(--space-4) px-(--space-5) border-none bg-transparent text-[0.85rem] cursor-pointer transition-all duration-200 border-b-2 hover:text-text-primary hover:bg-(--hover-overlay) ${rightPanelTab === "transcript" ? "text-accent-blue border-b-accent-blue bg-(--tint-accent-info-hover)" : "text-text-secondary border-b-transparent"}`}
              onClick={() => setRightPanelTab("transcript")}
              onKeyDown={handleTabKeyDown}
            >
              <Icon name="transcript" size={16} /> {t("app.tabTranscript")}
            </button>
            <button
              type="button"
              role="tab"
              id="right-tab-chat"
              aria-selected={rightPanelTab === "chat"}
              aria-controls="right-tabpanel"
              tabIndex={rightPanelTab === "chat" ? 0 : -1}
              className={`flex-1 flex items-center justify-center gap-(--space-3) py-(--space-4) px-(--space-5) border-none bg-transparent text-[0.85rem] cursor-pointer transition-all duration-200 border-b-2 hover:text-text-primary hover:bg-(--hover-overlay) ${rightPanelTab === "chat" ? "text-accent-blue border-b-accent-blue bg-(--tint-accent-info-hover)" : "text-text-secondary border-b-transparent"}`}
              onClick={() => setRightPanelTab("chat")}
              onKeyDown={handleTabKeyDown}
            >
              <Icon name="chat" size={16} /> {t("app.tabChat")}
            </button>
          </div>
          <div
            id="right-tabpanel"
            role="tabpanel"
            className="flex-1 min-h-0 flex flex-col overflow-hidden"
            aria-labelledby={
              rightPanelTab === "transcript"
                ? "right-tab-transcript"
                : "right-tab-chat"
            }
          >
            {rightPanelTab === "transcript" ? (
              <LiveTranscript />
            ) : (
              <ChatSidebar />
            )}
          </div>
          {!samplePreviewActive && <ProjectionRuntimeStatusPanel />}
          {!samplePreviewActive && loadedSessionId && (
            <SessionDataRoutePanel sessionId={loadedSessionId} />
          )}
        </aside>
      </div>
      <PipelineStatusBar />

      {/* Settings modal */}
      {settingsOpen && (
        <Suspense fallback={null}>
          <SettingsPage />
        </Suspense>
      )}

      {/* Sessions browser modal */}
      {sessionsBrowserOpen && (
        <Suspense fallback={null}>
          <SessionsBrowser />
        </Suspense>
      )}

      {/* Keyboard shortcuts help modal (Cmd/Ctrl+/ or ?) */}
      {shortcutsOpen && <ShortcutsHelpModal onClose={closeShortcuts} />}

      {/* First-time quickstart — suppressed once Settings is open so the
          two modals don't stack. */}
      {expressSetupVisible && !settingsOpen && (
        <Suspense fallback={null}>
          <ExpressSetup
            onDismiss={dismissExpressSetup}
            onOpenAdvanced={() => openSettings()}
            onPreviewSampleSession={previewSampleSession}
          />
        </Suspense>
      )}

      {/* Agent proposals pop-down overlay (toggled from the top bar). */}
      {agentOverlayOpen && (
        <PopoverOverlay
          label={t("app.agentProposals")}
          onClose={() => setAgentOverlayOpen(false)}
        >
          <AgentProposalsPanel />
        </PopoverOverlay>
      )}

      {/* Gemini token usage pop-down overlay (toggled from the top bar) —
          kept out of the chat column so chat gets the full height. */}
      {tokenOverlayOpen && (
        <PopoverOverlay
          label={t("app.tokenUsage")}
          onClose={() => setTokenOverlayOpen(false)}
        >
          <TokenUsagePanel />
        </PopoverOverlay>
      )}

      {/* Unified notification host (ADR-0011): transient queue + legacy
          error string, stacked above modals with severity aria-live. */}
      <Notifications />
    </div>
  );
}

export default App;
