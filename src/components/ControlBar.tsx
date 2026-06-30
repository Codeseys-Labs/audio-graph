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
 * Pre-capture discoverability (B20 / ADR-0016): the pipeline controls
 * (Transcribe, Gemini) are rendered ALWAYS — not gated behind `isCapturing`.
 * Before capture (or when prerequisites are unmet) they are `aria-disabled`
 * rather than natively `disabled`, so they stay focusable + screen-reader
 * located, and the reason ("Start capture to enable transcription", "Configure
 * Gemini in Settings") is surfaced via a Radix `Tooltip` (hover + keyboard
 * focus parity, WCAG 1.4.13) plus `aria-describedby`. The click handlers no-op
 * while `aria-disabled` (ARIA changes semantics only — never behaviour). This
 * mirrors the disabled-focusable idiom already used in `AudioSourceSelector`.
 *
 * Reads from the Zustand store (`isCapturing`, `isTranscribing`,
 * `isGeminiActive`, `captureStartTime`, `backpressuredSources`,
 * `selectedSourceIds`, `audioSources`, `settings`) and dispatches via store
 * actions (`startCapture`, `stopCapture`, `startTranscribe`, `stopTranscribe`,
 * `startGemini`, `stopGemini`, `openSettings`, `openSessionsBrowser`).
 *
 * Parent: `App.tsx`. No props.
 */
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useAudioGraphStore } from "../store";
import { parseCaptureTargetId } from "../utils/captureTarget";
import ConversationModeControl from "./ConversationModeControl";
import Icon from "./Icon";
import IconButton from "./IconButton";
import Tooltip from "./Tooltip";

function ControlBar() {
  const { t } = useTranslation();
  const isCapturing = useAudioGraphStore((s) => s.isCapturing);
  const isTranscribing = useAudioGraphStore((s) => s.isTranscribing);
  const isGeminiActive = useAudioGraphStore((s) => s.isGeminiActive);
  const selectedSourceIds = useAudioGraphStore((s) => s.selectedSourceIds);
  const audioSources = useAudioGraphStore((s) => s.audioSources);
  const processes = useAudioGraphStore((s) => s.processes);
  const captureStartTime = useAudioGraphStore((s) => s.captureStartTime);
  const backpressuredSources = useAudioGraphStore(
    (s) => s.backpressuredSources,
  );
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

  // Per-control in-flight flags. The store actions are fire-and-forget toggles
  // that flip the relevant `is*` flag only on success, so we track pending
  // locally to disable the button and surface aria-busy while the underlying
  // invoke is resolving (prevents double-clicks / "is it working?" ambiguity).
  const [capturePending, setCapturePending] = useState(false);
  const [transcribePending, setTranscribePending] = useState(false);
  const [geminiPending, setGeminiPending] = useState(false);

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
    setCapturePending(true);
    try {
      if (isCapturing) {
        await stopCapture();
      } else {
        await startCapture();
      }
    } finally {
      setCapturePending(false);
    }
  }, [isCapturing, startCapture, stopCapture]);

  const handleToggleTranscribe = useCallback(async () => {
    setTranscribePending(true);
    try {
      if (isTranscribing) {
        await stopTranscribe();
      } else {
        await startTranscribe();
      }
    } finally {
      setTranscribePending(false);
    }
  }, [isTranscribing, startTranscribe, stopTranscribe]);

  const handleToggleGemini = useCallback(async () => {
    setGeminiPending(true);
    try {
      if (isGeminiActive) {
        await stopGemini();
      } else {
        await startGemini();
      }
    } finally {
      setGeminiPending(false);
    }
  }, [isGeminiActive, startGemini, stopGemini]);

  const selectedLabels = selectedSourceIds.map((id) => {
    const source = audioSources.find((s) => s.id === id);
    if (source) {
      if (source.source_type.type === "SystemDefault")
        return `${source.name} system`;
      if (source.source_type.type === "Device") return `${source.name} device`;
      if (source.source_type.type === "Application")
        return `${source.name} application`;
      if (source.source_type.type === "ApplicationName")
        return `${source.name} application`;
      if (source.source_type.type === "ProcessTree")
        return `${source.name} process tree`;
      return source.name;
    }

    const target = parseCaptureTargetId(id);
    if (target.kind === "process_tree" && target.pid !== undefined) {
      const proc = processes.find((p) => p.pid === target.pid);
      return proc
        ? `${proc.name} process tree`
        : `PID ${target.pid} process tree`;
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

  // ── Pre-capture affordance (B20) ─────────────────────────────────────
  // The pipeline controls render at all times for discoverability. They are
  // `aria-disabled` (focusable + announced, not removed from the tab order)
  // until usable, with the reason surfaced via Tooltip + aria-describedby.
  const transcribeDisabled =
    (!canTranscribe && !isTranscribing) || transcribePending;
  const transcribeReason = !isCapturing
    ? t("controlBar.transcribeNeedsCapture")
    : t("controlBar.transcribeHint");
  // The Gemini button is only relevant in native converse mode. We keep it in
  // the DOM (no `hidden`) but only show it then, so converse/Gemini becomes
  // discoverable instead of being absent before capture.
  const geminiVisible =
    conversationMode === "converse" && converseEngine === "native";
  const geminiDisabled = (!canGemini && !isGeminiActive) || geminiPending;
  const geminiReason = !hasGeminiKey
    ? t("controlBar.geminiNeedsKey")
    : !isCapturing
      ? t("controlBar.geminiNeedsCapture")
      : t("controlBar.geminiHint");

  return (
    <header
      className="flex items-center justify-between px-(--space-6) bg-bg-tertiary border-b border-border-color h-[52px] flex-shrink-0 gap-(--space-6)"
      role="toolbar"
      aria-label={t("controlBar.toolbarLabel")}
    >
      <div className="flex items-center min-w-[140px]">
        <h1 className="text-lg font-bold text-accent-blue m-0 tracking-[-0.3px]">
          AudioGraph
        </h1>
      </div>

      <div className="flex items-center gap-(--space-5) flex-1 justify-center">
        <ConversationModeControl />

        {/* ── Capture controls ────────────────────────────────── */}
        <button
          type="button"
          className={`py-(--space-3) px-(--space-8) rounded-md text-base font-semibold cursor-pointer transition-all duration-[150ms] ease-[ease] border-2 border-transparent leading-[1.4] ${isCapturing ? "bg-accent-red text-(--on-accent-red) border-accent-red hover:bg-(--accent-red-hover) hover:border-(--accent-red-hover)" : "bg-accent-green text-(--on-accent-green) border-accent-green enabled:hover:bg-(--accent-green-hover) enabled:hover:border-(--accent-green-hover) disabled:opacity-40 disabled:cursor-not-allowed"}`}
          onClick={handleToggleCapture}
          disabled={(!canStart && !isCapturing) || capturePending}
          aria-label={
            isCapturing ? t("controlBar.stop") : t("controlBar.start")
          }
          aria-pressed={isCapturing}
          aria-busy={capturePending}
        >
          {isCapturing ? (
            <>
              <Icon name="stop" size={16} /> {t("controlBar.stop")}
              {capturePending && (
                <span className="ml-(--space-2) opacity-70" aria-hidden="true">
                  …
                </span>
              )}
            </>
          ) : (
            <>
              <Icon name="start" size={16} /> {t("controlBar.start")}
              {capturePending && (
                <span className="ml-(--space-2) opacity-70" aria-hidden="true">
                  …
                </span>
              )}
            </>
          )}
        </button>

        {isCapturing && (
          <div className="flex items-center gap-(--space-4)">
            <span
              className="w-[10px] h-[10px] rounded-full bg-accent-red animate-[pulse-recording_1.2s_ease-in-out_infinite]"
              aria-hidden="true"
            />
            <span
              className="font-mono text-[15px] font-semibold text-text-primary tracking-[0.5px] min-w-[50px]"
              aria-live="polite"
              aria-atomic="true"
            >
              {elapsed}
            </span>
          </div>
        )}

        {/* ── Pipeline controls (always rendered for discoverability) ──
            Disabled state uses aria-disabled (focusable + SR-located) with the
            reason surfaced via Tooltip + aria-describedby, not native disabled. */}
        <span className="control-bar__separator" aria-hidden="true">
          |
        </span>
        <span className="control-bar__group-label">
          {t("controlBar.pipelines")}
        </span>

        <Tooltip content={transcribeReason}>
          <button
            type="button"
            className={`py-(--space-3) px-(--space-7) rounded-md text-base font-semibold cursor-pointer transition-all duration-[150ms] ease-[ease] border-2 bg-transparent leading-[1.4] flex items-center gap-(--space-3) aria-disabled:opacity-30 aria-disabled:cursor-not-allowed aria-disabled:border-text-muted aria-disabled:text-text-muted ${isTranscribing ? "bg-accent-purple text-(--on-accent-purple) border-accent-purple hover:bg-(--accent-purple-hover) hover:border-(--accent-purple-hover)" : "border-accent-purple text-accent-purple hover:bg-(--tint-purple)"}`}
            onClick={() => {
              // ARIA-disabled controls must no-op in JS (semantics only).
              if (transcribeDisabled) return;
              void handleToggleTranscribe();
            }}
            aria-disabled={transcribeDisabled}
            aria-label={
              isTranscribing
                ? t("controlBar.stopTranscription")
                : t("controlBar.startTranscription")
            }
            aria-describedby="control-bar-transcribe-reason"
            aria-pressed={isTranscribing}
            aria-busy={transcribePending}
          >
            {isTranscribing && (
              <span
                className="w-[8px] h-[8px] rounded-full bg-(--on-accent-purple) animate-[pulse-recording_1.2s_ease-in-out_infinite] shrink-0"
                aria-hidden="true"
              />
            )}
            {isTranscribing
              ? t("controlBar.stopTranscribe")
              : t("controlBar.transcribe")}
            {transcribePending && (
              <span className="opacity-70" aria-hidden="true">
                …
              </span>
            )}
          </button>
        </Tooltip>
        <span id="control-bar-transcribe-reason" className="sr-only">
          {transcribeReason}
        </span>

        {geminiVisible && (
          <>
            <Tooltip content={geminiReason}>
              <button
                type="button"
                className={`py-(--space-3) px-(--space-7) rounded-md text-base font-semibold cursor-pointer transition-all duration-[150ms] ease-[ease] border-2 bg-transparent leading-[1.4] flex items-center gap-(--space-3) aria-disabled:opacity-30 aria-disabled:cursor-not-allowed aria-disabled:border-text-muted aria-disabled:text-text-muted ${isGeminiActive ? "bg-(--accent-gemini) text-(--on-accent-gemini) border-(--accent-gemini) hover:bg-(--accent-gemini-hover) hover:border-(--accent-gemini-hover)" : "border-(--accent-gemini) text-(--accent-gemini) hover:bg-(--tint-gemini)"}`}
                onClick={() => {
                  if (geminiDisabled) return;
                  void handleToggleGemini();
                }}
                aria-disabled={geminiDisabled}
                aria-label={
                  isGeminiActive
                    ? t("controlBar.stopGeminiLabel")
                    : t("controlBar.startGeminiLabel")
                }
                aria-describedby="control-bar-gemini-reason"
                aria-pressed={isGeminiActive}
                aria-busy={geminiPending}
              >
                {isGeminiActive && (
                  <span
                    className="w-[8px] h-[8px] rounded-full bg-(--on-accent-gemini) animate-[pulse-recording_1.2s_ease-in-out_infinite] shrink-0"
                    aria-hidden="true"
                  />
                )}
                {isGeminiActive
                  ? t("controlBar.stopGemini")
                  : t("controlBar.gemini")}
                {geminiPending && (
                  <span className="opacity-70" aria-hidden="true">
                    …
                  </span>
                )}
              </button>
            </Tooltip>
            <span id="control-bar-gemini-reason" className="sr-only">
              {geminiReason}
            </span>
          </>
        )}

        {isComparing && (
          <span
            className="control-bar__comparing"
            title={t("controlBar.comparingHint")}
          >
            {t("controlBar.comparing")}
          </span>
        )}

        {backpressuredSources.length > 0 && (
          <span
            className="inline-flex items-center py-(--space-2) px-[10px] ml-(--space-4) text-sm font-medium text-(--text-on-tint-warning) bg-(--tint-warning) border border-(--tint-border-warning) rounded-full animate-[pulse-backpressure_2s_ease-in-out_infinite]"
            role="status"
            aria-live="polite"
            title={t("controlBar.backpressureHint", {
              count: backpressuredSources.length,
            })}
          >
            <Icon name="warning" size={14} /> {t("controlBar.backpressure")}
          </span>
        )}

        {/* ── Idle hints ─────────────────────────────────────── */}
        {!isCapturing && selectedLabels.length > 0 && (
          <span
            className="text-md text-text-secondary max-w-[200px] overflow-hidden text-ellipsis whitespace-nowrap"
            title={selectedLabel}
          >
            {selectedLabels.length === 1
              ? selectedLabel
              : t("controlBar.sourcesSelected", {
                  count: selectedLabels.length,
                })}
          </span>
        )}

        {selectedSourceIds.length === 0 && !isCapturing && (
          <span className="text-md text-text-muted italic">
            {t("controlBar.selectSourcesToBegin")}
          </span>
        )}
      </div>

      <div className="flex items-center justify-end min-w-[140px]">
        {isCapturing && selectedLabels.length > 0 && (
          <span className="text-sm text-text-secondary max-w-[160px] overflow-hidden text-ellipsis whitespace-nowrap">
            <Icon name="headphones" size={14} />{" "}
            {selectedLabels.length === 1
              ? selectedLabel
              : t("controlBar.sourcesSummary", {
                  count: selectedLabels.length,
                })}
          </span>
        )}
        <button
          type="button"
          className="control-bar__settings-btn relative"
          onClick={toggleAgentOverlay}
          title={t("controlBar.agentProposals")}
          aria-label={t("controlBar.toggleAgentProposals")}
        >
          <Icon name="agent" size={16} /> {t("controlBar.agent")}
          {agentProposals.length > 0 && (
            <span className="inline-flex items-center justify-center min-w-[16px] h-[16px] px-(--space-2) ml-[5px] rounded-lg bg-accent-red text-white text-2xs font-bold">
              {agentProposals.length}
            </span>
          )}
        </button>
        <button
          type="button"
          className="control-bar__settings-btn"
          onClick={toggleTokenOverlay}
          title={t("controlBar.tokenUsage")}
          aria-label={t("controlBar.toggleTokenUsage")}
        >
          <Icon name="tokens" size={16} /> {t("controlBar.tokens")}
        </button>
        <button
          type="button"
          className="control-bar__settings-btn"
          onClick={openSessionsBrowser}
          title={t("controlBar.browseSessions")}
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
