import { useEffect, useState } from "react";
import AudioSourceSelector from "./components/AudioSourceSelector";
import LiveTranscript from "./components/LiveTranscript";
import ChatSidebar from "./components/ChatSidebar";
import KnowledgeGraphViewer from "./components/KnowledgeGraphViewer";
import ControlBar from "./components/ControlBar";
import SpeakerPanel from "./components/SpeakerPanel";
import PipelineStatusBar from "./components/PipelineStatusBar";
import SettingsPage from "./components/SettingsPage";
import SessionsBrowser from "./components/SessionsBrowser";
import ShortcutsHelpModal from "./components/ShortcutsHelpModal";
import TokenUsagePanel from "./components/TokenUsagePanel";
import Toast from "./components/Toast";
import { useTauriEvents } from "./hooks/useTauriEvents";
import { useKeyboardShortcuts } from "./hooks/useKeyboardShortcuts";
import { useAudioGraphStore } from "./store";
import "./App.css";

function App() {
  // Subscribe to Tauri backend events
  useTauriEvents();
  // Register global keyboard shortcuts (Cmd/Ctrl+R, Cmd/Ctrl+,, Esc, Cmd/Ctrl+Shift+S)
  useKeyboardShortcuts();

  const error = useAudioGraphStore((s) => s.error);
  const clearError = useAudioGraphStore((s) => s.clearError);
  const rightPanelTab = useAudioGraphStore((s) => s.rightPanelTab);
  const setRightPanelTab = useAudioGraphStore((s) => s.setRightPanelTab);
  const settingsOpen = useAudioGraphStore((s) => s.settingsOpen);
  const sessionsBrowserOpen = useAudioGraphStore((s) => s.sessionsBrowserOpen);

  // Shortcuts help modal is kept as local UI state rather than in the store —
  // it has no backend tie-in and nothing else observes it.
  const [shortcutsOpen, setShortcutsOpen] = useState(false);
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

  return (
    <div className="app-container">
      <ControlBar />
      <div className="main-layout">
        <aside className="left-panel">
          <AudioSourceSelector />
          <SpeakerPanel />
        </aside>
        <main className="center-panel">
          <KnowledgeGraphViewer />
        </main>
        <aside className="right-panel">
          <div className="right-panel__tabs">
            <button
              className={`right-panel__tab ${rightPanelTab === "transcript" ? "right-panel__tab--active" : ""}`}
              onClick={() => setRightPanelTab("transcript")}
            >
              📝 Transcript
            </button>
            <button
              className={`right-panel__tab ${rightPanelTab === "chat" ? "right-panel__tab--active" : ""}`}
              onClick={() => setRightPanelTab("chat")}
            >
              💬 Chat
            </button>
          </div>
          {rightPanelTab === "transcript" ? (
            <LiveTranscript />
          ) : (
            <ChatSidebar />
          )}
          <TokenUsagePanel />
        </aside>
      </div>
      <PipelineStatusBar />

      {/* Error toast notification */}
      {error && (
        <div className="error-toast" role="alert">
          <span className="error-toast__icon" aria-hidden="true">
            ⚠️
          </span>
          <span className="error-toast__message">{error}</span>
          <button
            className="error-toast__dismiss"
            onClick={clearError}
            aria-label="Dismiss error"
          >
            ✕
          </button>
        </div>
      )}

      {/* Settings modal */}
      {settingsOpen && <SettingsPage />}

      {/* Sessions browser modal */}
      {sessionsBrowserOpen && <SessionsBrowser />}

      {/* Keyboard shortcuts help modal (Cmd/Ctrl+/ or ?) */}
      {shortcutsOpen && (
        <ShortcutsHelpModal onClose={() => setShortcutsOpen(false)} />
      )}

      {/* Ephemeral status toast (Gemini reconnect, etc.) */}
      <Toast />
    </div>
  );
}

export default App;
