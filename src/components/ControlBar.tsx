/**
 * Top control bar — the primary capture-control surface.
 *
 * Renders:
 *   - Capture Start/Stop toggle (mirrors the Cmd/Ctrl+R hotkey).
 *   - Elapsed-time counter (MM:SS) while capturing.
 *   - Transcribe toggle (per-session local/cloud ASR pipeline).
 *   - Gemini Live toggle (independent WebSocket streaming path).
 *   - Backpressure pill when any selected source is currently dropping.
 *   - Settings and Sessions browser launchers.
 *
 * Reads from the Zustand store (`isCapturing`, `isTranscribing`,
 * `isGeminiActive`, `captureStartTime`, `backpressuredSources`,
 * `selectedSourceIds`, `audioSources`, `settings`) and dispatches via store
 * actions (`startCapture`, `stopCapture`, `startTranscribe`, `stopTranscribe`,
 * `startGemini`, `stopGemini`, `openSettings`, `openSessionsBrowser`).
 *
 * Parent: `App.tsx`. No props.
 */
import { useState, useEffect, useCallback } from "react";
import { useTranslation } from "react-i18next";
import { useAudioGraphStore } from "../store";
import { parseCaptureTargetId } from "../utils/captureTarget";
import Icon from "./Icon";
import IconButton from "./IconButton";
import ConversationModeControl from "./ConversationModeControl";

function ControlBar() {
  const { t } = useTranslation();
  const isCapturing = useAudioGraphStore((s) => s.isCapturing);
  const isTranscribing = useAudioGraphStore((s) => s.isTranscribing);
  const isGeminiActive = useAudioGraphStore((s) => s.isGeminiActive);
  const selectedSourceIds = useAudioGraphStore((s) => s.selectedSourceIds);
  const audioSources = useAudioGraphStore((s) => s.audioSources);
  const processes = useAudioGraphStore((s) => s.processes);
  const captureStartTime = useAudioGraphStore((s) => s.captureStartTime);
  const backpressuredSources = useAudioGraphStore((s) => s.backpressuredSources);
  const settings = useAudioGraphStore((s) => s.settings);
  const startCapture = useAudioGraphStore((s) => s.startCapture);
  const stopCapture = useAudioGraphStore((s) => s.stopCapture);
  const startTranscribe = useAudioGraphStore((s) => s.startTranscribe);
  const stopTranscribe = useAudioGraphStore((s) => s.stopTranscribe);
  const startGemini = useAudioGraphStore((s) => s.startGemini);
  const stopGemini = useAudioGraphStore((s) => s.stopGemini);
  const openSettings = useAudioGraphStore((s) => s.openSettings);
  const openSessionsBrowser = useAudioGraphStore((s) => s.openSessionsBrowser);
  const agentProposals = useAudioGraphStore((s) => s.agentProposals);
  const toggleAgentOverlay = useAudioGraphStore((s) => s.toggleAgentOverlay);
  const toggleTokenOverlay = useAudioGraphStore((s) => s.toggleTokenOverlay);
  const conversationMode = useAudioGraphStore((s) => s.conversationMode);
  const converseEngine = useAudioGraphStore((s) => s.converseEngine);

  const [elapsed, setElapsed] = useState("00:00");

  // Update elapsed timer every second while capturing
  useEffect(() => {
    if (!isCapturing || captureStartTime === null) {
      setElapsed("00:00");
      return;
    }

    const tick = () => {
      const diff = Math.floor((Date.now() - captureStartTime) / 1000);
      const mins = Math.floor(diff / 60)
        .toString()
        .padStart(2, "0");
      const secs = (diff % 60).toString().padStart(2, "0");
      setElapsed(`${mins}:${secs}`);
    };

    tick(); // Immediate first tick
    const interval = setInterval(tick, 1000);
    return () => clearInterval(interval);
  }, [isCapturing, captureStartTime]);

  const handleToggleCapture = useCallback(async () => {
    if (isCapturing) {
      await stopCapture();
    } else {
      await startCapture();
    }
  }, [isCapturing, startCapture, stopCapture]);

  const handleToggleTranscribe = useCallback(async () => {
    if (isTranscribing) {
      await stopTranscribe();
    } else {
      await startTranscribe();
    }
  }, [isTranscribing, startTranscribe, stopTranscribe]);

  const handleToggleGemini = useCallback(async () => {
    if (isGeminiActive) {
      await stopGemini();
    } else {
      await startGemini();
    }
  }, [isGeminiActive, startGemini, stopGemini]);

  const selectedLabels = selectedSourceIds.map((id) => {
    const source = audioSources.find((s) => s.id === id);
    if (source) {
      if (source.source_type.type === "SystemDefault") return `${source.name} system`;
      if (source.source_type.type === "Device") return `${source.name} device`;
      if (source.source_type.type === "Application") return `${source.name} application`;
      return source.name;
    }

    const target = parseCaptureTargetId(id);
    if (target.kind === "process_tree" && target.pid !== undefined) {
      const proc = processes.find((p) => p.pid === target.pid);
      return proc ? `${proc.name} process tree` : `PID ${target.pid} process tree`;
    }
    if (target.kind === "process" && target.pid !== undefined) {
      const proc = processes.find((p) => p.pid === target.pid);
      return proc ? `${proc.name} process` : `PID ${target.pid} process`;
    }
    if (target.kind === "application_name" && target.name) {
      return `${target.name} application`;
    }

    return id;
  });
  const canStart = selectedSourceIds.length > 0 && !isCapturing;
  // Transcribe requires capture to be running
  const canTranscribe = isCapturing && !isTranscribing;
  // Settings returned over IPC are redacted; API-key presence is validated in
  // the backend against the credential store when the user starts Gemini.
  const hasGeminiKey =
    settings?.gemini?.auth?.type === "api_key" ||
    settings?.gemini?.auth?.type === "vertex_ai";
  const canGemini = isCapturing && !isGeminiActive && hasGeminiKey;
  const selectedLabel = selectedLabels.join(", ");

  // Both pipelines running simultaneously = comparison mode
  const isComparing = isTranscribing && isGeminiActive;

  return (
    <header
      className="flex items-center justify-between px-(--space-6) bg-bg-tertiary border-b border-border-color h-[52px] flex-shrink-0 gap-(--space-6)"
      role="toolbar"
      aria-label="Capture controls"
    >
      <div className="flex items-center min-w-[140px]">
        <h1 className="text-lg font-bold text-accent-blue m-0 tracking-[-0.3px]">AudioGraph</h1>
      </div>

      <div className="flex items-center gap-(--space-5) flex-1 justify-center">
        <ConversationModeControl />

        {/* ── Capture controls ────────────────────────────────── */}
        <button
          className={`py-(--space-3) px-(--space-8) rounded-md text-base font-semibold cursor-pointer transition-all duration-[150ms] ease-[ease] border-2 border-transparent leading-[1.4] ${isCapturing ? "bg-accent-red text-(--on-accent-red) border-accent-red hover:bg-(--accent-red-hover) hover:border-(--accent-red-hover)" : "bg-accent-green text-[#0a2010] border-accent-green enabled:hover:bg-[#5cec92] enabled:hover:border-[#5cec92] disabled:opacity-40 disabled:cursor-not-allowed"}`}
          onClick={handleToggleCapture}
          disabled={!canStart && !isCapturing}
          aria-label={isCapturing ? t("controlBar.stop") : t("controlBar.start")}
          aria-pressed={isCapturing}
        >
          {isCapturing ? (
            <>
              <Icon name="stop" size={16} /> {t("controlBar.stop")}
            </>
          ) : (
            <>
              <Icon name="start" size={16} /> {t("controlBar.start")}
            </>
          )}
        </button>

        {isCapturing && (
          <div className="flex items-center gap-(--space-4)">
            <span className="w-[10px] h-[10px] rounded-full bg-accent-red animate-[pulse-recording_1.2s_ease-in-out_infinite]" aria-hidden="true" />
            <span
              className='font-[family-name:"SF_Mono","Fira_Code","Consolas",monospace] text-[15px] font-semibold text-text-primary tracking-[0.5px] min-w-[50px]'
              aria-live="polite"
              aria-atomic="true"
            >
              {elapsed}
            </span>
          </div>
        )}

        {/* ── Pipeline controls (visible when capturing) ──────── */}
        {isCapturing && (
          <>
            <span className="control-bar__separator" aria-hidden="true">|</span>
            <span className="control-bar__group-label">Pipelines</span>

            <button
              className={`py-(--space-3) px-(--space-7) rounded-md text-base font-semibold cursor-pointer transition-all duration-[150ms] ease-[ease] border-2 bg-transparent leading-[1.4] flex items-center gap-(--space-3) disabled:opacity-30 disabled:cursor-not-allowed disabled:border-text-muted disabled:text-text-muted ${isTranscribing ? "bg-accent-purple text-(--on-accent-purple) border-accent-purple enabled:hover:bg-(--accent-purple-hover) enabled:hover:border-(--accent-purple-hover)" : "border-accent-purple text-accent-purple enabled:hover:bg-[rgba(185,140,255,0.16)]"}`}
              onClick={handleToggleTranscribe}
              disabled={!canTranscribe && !isTranscribing}
              aria-label={isTranscribing ? "Stop transcription" : "Start transcription"}
              aria-pressed={isTranscribing}
              title="Stream audio to local Whisper ASR"
            >
              {isTranscribing && (
                <span className="w-[8px] h-[8px] rounded-full bg-(--on-accent-purple) animate-[pulse-recording_1.2s_ease-in-out_infinite] shrink-0" aria-hidden="true" />
              )}
              {isTranscribing ? "Stop Transcribe" : "Transcribe"}
            </button>

            <button
              className={`py-(--space-3) px-(--space-7) rounded-md text-base font-semibold cursor-pointer transition-all duration-[150ms] ease-[ease] border-2 bg-transparent leading-[1.4] flex items-center gap-(--space-3) disabled:opacity-30 disabled:cursor-not-allowed disabled:border-text-muted disabled:text-text-muted ${isGeminiActive ? "bg-(--accent-gemini) text-[#0a2015] border-(--accent-gemini) enabled:hover:bg-[#4aeaaa] enabled:hover:border-[#4aeaaa]" : "border-(--accent-gemini) text-(--accent-gemini) enabled:hover:bg-[rgba(52,211,153,0.12)]"}`}
              onClick={handleToggleGemini}
              disabled={!canGemini && !isGeminiActive}
              aria-label={isGeminiActive ? "Stop Gemini" : "Start Gemini"}
              aria-pressed={isGeminiActive}
              title={
                !hasGeminiKey
                  ? "Configure Gemini in Settings"
                  : "Stream audio to Gemini Live (native speech-to-speech)"
              }
              hidden={
                !(conversationMode === "converse" && converseEngine === "native")
              }
            >
              {isGeminiActive && (
                <span className="w-[8px] h-[8px] rounded-full bg-[#0a2015] animate-[pulse-recording_1.2s_ease-in-out_infinite] shrink-0" aria-hidden="true" />
              )}
              {isGeminiActive ? "Stop Gemini" : "Gemini"}
            </button>

            {isComparing && (
              <span className="control-bar__comparing" title="Both local and Gemini pipelines are running">
                Comparing...
              </span>
            )}

            {backpressuredSources.length > 0 && (
              <span
                className="inline-flex items-center py-(--space-2) px-[10px] ml-(--space-4) text-sm font-medium text-[#7a4a00] bg-[#fff4d6] border border-[#f0c36d] rounded-full animate-[pulse-backpressure_2s_ease-in-out_infinite]"
                role="status"
                aria-live="polite"
                title={
                  `Audio ring buffer is dropping chunks from ${backpressuredSources.length} source(s). ` +
                  "The pipeline consumer is too slow — consider disabling Gemini or switching to a smaller Whisper model."
                }
              >
                <Icon name="warning" size={14} /> Backpressure
              </span>
            )}
          </>
        )}

        {/* ── Idle hints ─────────────────────────────────────── */}
        {!isCapturing && selectedLabels.length > 0 && (
          <span className="text-md text-text-secondary max-w-[200px] overflow-hidden text-ellipsis whitespace-nowrap" title={selectedLabel}>
            {selectedLabels.length === 1
              ? selectedLabel
              : `${selectedLabels.length} sources selected`}
          </span>
        )}

        {selectedSourceIds.length === 0 && !isCapturing && (
          <span className="text-md text-text-muted italic">
            Select audio sources to begin
          </span>
        )}
      </div>

      <div className="flex items-center justify-end min-w-[140px]">
        {isCapturing && selectedLabels.length > 0 && (
          <span className="text-sm text-text-secondary max-w-[160px] overflow-hidden text-ellipsis whitespace-nowrap">
            <Icon name="headphones" size={14} />{" "}
            {selectedLabels.length === 1
              ? selectedLabel
              : `${selectedLabels.length} sources`}
          </span>
        )}
        <button
          className="control-bar__settings-btn relative"
          onClick={toggleAgentOverlay}
          title="Agent proposals"
          aria-label="Toggle agent proposals"
        >
          <Icon name="agent" size={16} /> Agent
          {agentProposals.length > 0 && (
            <span className="inline-flex items-center justify-center min-w-[16px] h-[16px] px-(--space-2) ml-[5px] rounded-lg bg-accent-red text-white text-2xs font-bold">
              {agentProposals.length}
            </span>
          )}
        </button>
        <button
          className="control-bar__settings-btn"
          onClick={toggleTokenOverlay}
          title="Gemini token usage"
          aria-label="Toggle token usage"
        >
          <Icon name="tokens" size={16} /> Tokens
        </button>
        <button
          className="control-bar__settings-btn"
          onClick={openSessionsBrowser}
          title="Browse recent sessions"
          aria-label={t("controlBar.sessions")}
        >
          {t("controlBar.sessions")}
        </button>
        <IconButton
          className="control-bar__settings-btn"
          icon="settings"
          label={t("controlBar.settings")}
          variant="ghost"
          onClick={openSettings}
        />
      </div>
    </header>
  );
}

export default ControlBar;
