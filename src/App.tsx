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
 * probe `credentials.yaml` via `load_credential_cmd` for any known cloud
 * provider key. If none exist, `ExpressSetup` renders once; dismissal is
 * transient (per-session), not persisted.
 *
 * No props — this component is the app shell.
 */

import { invoke } from "@tauri-apps/api/core";
import { lazy, Suspense, useEffect, useState } from "react";
import AgentProposalsPanel from "./components/AgentProposalsPanel";
import AudioSourceSelector from "./components/AudioSourceSelector";
import ChatSidebar from "./components/ChatSidebar";
import ControlBar from "./components/ControlBar";
import Icon from "./components/Icon";
import LiveTranscript from "./components/LiveTranscript";
import NotesPanel from "./components/NotesPanel";
import PipelineStatusBar from "./components/PipelineStatusBar";
import ResizeDivider from "./components/ResizeDivider";
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
import { useKeyboardShortcuts } from "./hooks/useKeyboardShortcuts";
import { useTauriEvents } from "./hooks/useTauriEvents";
import { useAudioGraphStore } from "./store";
import "./styles/index.css";

// Credential keys that, when any is present in credentials.yaml, indicate the
// user has already configured at least one provider. Missing all of these
// triggers the Express Setup quickstart on launch. Matches the cloud-provider
// keys the Express dialog writes to — local-only users fall through to Skip.
const FIRST_TIME_CREDENTIAL_KEYS = [
  "openai_api_key",
  "groq_api_key",
  "gemini_api_key",
  "deepgram_api_key",
  "assemblyai_api_key",
  "aws_access_key",
];

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

function App() {
  // Subscribe to Tauri backend events
  useTauriEvents();
  // Register global keyboard shortcuts (Cmd/Ctrl+R, Cmd/Ctrl+,, Esc, Cmd/Ctrl+Shift+S)
  useKeyboardShortcuts();

  const rightPanelTab = useAudioGraphStore((s) => s.rightPanelTab);
  const setRightPanelTab = useAudioGraphStore((s) => s.setRightPanelTab);
  const settingsOpen = useAudioGraphStore((s) => s.settingsOpen);
  const sessionsBrowserOpen = useAudioGraphStore((s) => s.sessionsBrowserOpen);
  const openSettings = useAudioGraphStore((s) => s.openSettings);
  const agentOverlayOpen = useAudioGraphStore((s) => s.agentOverlayOpen);
  const setAgentOverlayOpen = useAudioGraphStore((s) => s.setAgentOverlayOpen);
  const tokenOverlayOpen = useAudioGraphStore((s) => s.tokenOverlayOpen);
  const setTokenOverlayOpen = useAudioGraphStore((s) => s.setTokenOverlayOpen);

  // First-time setup: on mount, probe credentials.yaml for any known cloud
  // provider key. If none are present, pop the Express Setup modal once.
  // Dismissal (save or skip) sets `expressSetupVisible = false` and we never
  // re-probe during this session — the user can reach the same UI via
  // Settings when they're ready.
  const [expressSetupVisible, setExpressSetupVisible] = useState(false);
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const results = await Promise.all(
          FIRST_TIME_CREDENTIAL_KEYS.map((key) =>
            invoke<string | null>("load_credential_cmd", { key }).catch(
              () => null,
            ),
          ),
        );
        if (cancelled) return;
        const hasAny = results.some((v) => v && v.length > 0);
        if (!hasAny) {
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
      <div className="main-layout">
        <aside className="left-panel" style={{ width: leftWidth }}>
          <AudioSourceSelector />
          <SpeakerPanel />
        </aside>
        <ResizeDivider
          orientation="vertical"
          onResize={resizeLeft}
          ariaLabel="Resize sources panel"
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
            ariaLabel="Resize notes panel"
          />
          <div className="center-panel__notes" style={{ height: notesHeight }}>
            <NotesPanel />
          </div>
        </main>
        <ResizeDivider
          orientation="vertical"
          onResize={resizeRight}
          ariaLabel="Resize transcript and chat panel"
        />
        <aside className="right-panel" style={{ width: rightWidth }}>
          <div
            className="flex border-b border-b-border-color bg-bg-secondary shrink-0"
            role="tablist"
            aria-label="Right panel views"
          >
            <button
              type="button"
              role="tab"
              id="right-tab-transcript"
              aria-selected={rightPanelTab === "transcript"}
              aria-controls="right-tabpanel"
              tabIndex={rightPanelTab === "transcript" ? 0 : -1}
              className={`flex-1 flex items-center justify-center gap-(--space-3) py-(--space-4) px-(--space-5) border-none bg-transparent text-[0.85rem] cursor-pointer transition-all duration-200 border-b-2 hover:text-text-primary hover:bg-[rgba(255,255,255,0.03)] ${rightPanelTab === "transcript" ? "text-accent-blue border-b-accent-blue bg-[rgba(96,165,250,0.05)]" : "text-text-secondary border-b-transparent"}`}
              onClick={() => setRightPanelTab("transcript")}
              onKeyDown={handleTabKeyDown}
            >
              <Icon name="transcript" size={16} /> Transcript
            </button>
            <button
              type="button"
              role="tab"
              id="right-tab-chat"
              aria-selected={rightPanelTab === "chat"}
              aria-controls="right-tabpanel"
              tabIndex={rightPanelTab === "chat" ? 0 : -1}
              className={`flex-1 flex items-center justify-center gap-(--space-3) py-(--space-4) px-(--space-5) border-none bg-transparent text-[0.85rem] cursor-pointer transition-all duration-200 border-b-2 hover:text-text-primary hover:bg-[rgba(255,255,255,0.03)] ${rightPanelTab === "chat" ? "text-accent-blue border-b-accent-blue bg-[rgba(96,165,250,0.05)]" : "text-text-secondary border-b-transparent"}`}
              onClick={() => setRightPanelTab("chat")}
              onKeyDown={handleTabKeyDown}
            >
              <Icon name="chat" size={16} /> Chat
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
      {shortcutsOpen && (
        <ShortcutsHelpModal onClose={() => setShortcutsOpen(false)} />
      )}

      {/* First-time quickstart — suppressed once Settings is open so the
          two modals don't stack. */}
      {expressSetupVisible && !settingsOpen && (
        <Suspense fallback={null}>
          <ExpressSetup
            onDismiss={() => setExpressSetupVisible(false)}
            onOpenAdvanced={() => openSettings()}
          />
        </Suspense>
      )}

      {/* Agent proposals pop-down overlay (toggled from the top bar). */}
      {agentOverlayOpen && (
        <PopoverOverlay
          label="Agent proposals"
          onClose={() => setAgentOverlayOpen(false)}
        >
          <AgentProposalsPanel />
        </PopoverOverlay>
      )}

      {/* Gemini token usage pop-down overlay (toggled from the top bar) —
          kept out of the chat column so chat gets the full height. */}
      {tokenOverlayOpen && (
        <PopoverOverlay
          label="Token usage"
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
