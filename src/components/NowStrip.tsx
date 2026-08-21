/**
 * NOW STRIP — the persistent active-session strip (SHELL-R3, plan §R3,
 * ADR-0046). Replaces `ControlBar` in place, on the same store surface
 * (`isCapturing`, `selectedSourceIds`, `settings`, ...).
 *
 * Start/Stop keep their `controlBar.start`/`controlBar.stop` aria-labels
 * VERBATIM — `e2e/specs/shell.e2e.ts` selects on the literal attribute
 * (`button[aria-label="Start"]`), never the visual treatment, so this unit
 * restyles the button and never retitles it.
 *
 * ONE START (ADR-0046's "one Start on the strip"): the button dispatches
 * `startCaptureAndTranscribe` (`store/index.ts`) instead of `startCapture`
 * directly — see that action's doc comment for the full ADR-0033 gating
 * proof (jsdom-verified in `store/index.test.ts`'s "merged Start" describe
 * block) and the explicit no-atomicity disclaimer (ADR-0028's coordinated
 * start, seed audio-graph-10ff, replaces this composition later). The
 * Cmd/Ctrl+R hotkey (`useKeyboardShortcuts.ts`) dispatches the SAME merged
 * action, so the button and the hotkey never silently diverge.
 *
 * IDLE vs LIVE:
 *   - Idle collapses to one line: Start, "Ready", the selected-source count,
 *     the planned-route chip, and a neutral-tone "System status" chip
 *     (opens `SystemDrawer` — SAME action as the live health chip below,
 *     just without a health-tone claim, since nothing has run yet to
 *     classify; keeps the drawer's per-stage/token detail one click away
 *     even before capture starts, the 50e3 fold's "no regression to
 *     diagnostics" half). Start is the ONLY saturated (accent-colored)
 *     element among those five — everything else is neutral text or a
 *     `.ag-chip[data-tone="neutral"]` (screenshot evidence lives in the
 *     landing PR body).
 *   - Live adds the elapsed timer (tabular-nums), a durability readout
 *     ("notes saved · Ns ago", derived from the most recent "notes"
 *     projection patch already in `sessionProjectionEvents` — no new
 *     backend read), and swaps the neutral idle chip for the composite
 *     health chip (same `setSystemDrawerOpen` action, now tone/label-coded
 *     off `utils/pipelineHealth.ts`'s live classification).
 *
 * NAMED TRANSITIONAL STATE (do not "fix" — R5 owns the follow-up):
 * `ConversationModeControl` and the Gemini toggle remain in the strip,
 * demoted (ghost/muted via a wrapping `opacity`+`grayscale` filter, since
 * neither component's own internals are in this unit's scope) until R5
 * relocates them into the preflight card. Per B20 / ADR-0016, they render
 * UNCONDITIONALLY — idle AND live, same as master's `ControlBar` — not
 * gated behind `isCapturing`; `geminiVisible` (unchanged from master) still
 * governs whether the Gemini toggle itself is in the DOM at all, and its own
 * `aria-disabled`/Tooltip reason (`geminiReason`) covers the pre-capture
 * case exactly as before. The standalone Transcribe toggle is REMOVED
 * outright (composed into Start instead — "purple leaves the strip for
 * free" per the recomposition plan); a manual "stop transcription without
 * stopping capture" affordance is not carried forward.
 *
 * The route chip is passive-read-derived, not settings-alone (`settings` +
 * `credentialPresence` + `modelStatus`, all local/already-in-store per
 * ADR-0028 — no new egress; `utils/durableRoute.ts`'s
 * `hasConfiguredDurableNotesRoute` is the SAME predicate `App.tsx`'s startup
 * probe performs), and is ALWAYS labeled "planned:" — never "observed"
 * (ADR-0030/0034; no active-route state exists yet, seed audio-graph-8d18
 * owns that upgrade). Note `hasConfiguredDurableNotesRoute` hard-requires
 * `asr.type === "deepgram"`, so "not configured" really means "no
 * MVP-selectable Deepgram → LLM route with present credentials" — a user on
 * AWS Transcribe or local Whisper reads as unconfigured even though
 * `describePlannedRoute` could name their actual selection; that mismatch is
 * a known, not-yet-fixed gap, not a doc inaccuracy.
 *
 * Parent: `App.tsx`. No props.
 */
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useAudioGraphStore } from "../store";
import {
  describePlannedRoute,
  hasConfiguredDurableNotesRoute,
} from "../utils/durableRoute";
import { computeCompositeHealth } from "../utils/pipelineHealth";
import ConversationModeControl from "./ConversationModeControl";
import Icon from "./Icon";
import IconButton from "./IconButton";
import { PROVIDER_DESCRIPTORS } from "./providerRegistryHelpers";
import Tooltip from "./Tooltip";

function formatElapsed(totalSeconds: number): string {
  const clamped = Math.max(0, totalSeconds);
  const mins = Math.floor(clamped / 60)
    .toString()
    .padStart(2, "0");
  const secs = (clamped % 60).toString().padStart(2, "0");
  return `${mins}:${secs}`;
}

const HEALTH_TONE: Record<"healthy" | "degraded" | "error", string> = {
  healthy: "success",
  degraded: "warning",
  error: "danger",
};

function NowStrip() {
  const { t } = useTranslation();
  const isCapturing = useAudioGraphStore((s) => s.isCapturing);
  const isGeminiActive = useAudioGraphStore((s) => s.isGeminiActive);
  const selectedSourceIds = useAudioGraphStore((s) => s.selectedSourceIds);
  const captureStartTime = useAudioGraphStore((s) => s.captureStartTime);
  const settings = useAudioGraphStore((s) => s.settings);
  const credentialPresence = useAudioGraphStore((s) => s.credentialPresence);
  const modelStatus = useAudioGraphStore((s) => s.modelStatus);
  const startCaptureAndTranscribe = useAudioGraphStore(
    (s) => s.startCaptureAndTranscribe,
  );
  const stopCapture = useAudioGraphStore((s) => s.stopCapture);
  const startGemini = useAudioGraphStore((s) => s.startGemini);
  const stopGemini = useAudioGraphStore((s) => s.stopGemini);
  const openSettings = useAudioGraphStore((s) => s.openSettings);
  const openSessionsBrowser = useAudioGraphStore((s) => s.openSessionsBrowser);
  const setSystemDrawerOpen = useAudioGraphStore((s) => s.setSystemDrawerOpen);
  const conversationMode = useAudioGraphStore((s) => s.conversationMode);
  const converseEngine = useAudioGraphStore((s) => s.converseEngine);
  const converseRealtimeAgentProvider = useAudioGraphStore(
    (s) => s.converseRealtimeAgentProvider,
  );
  const sessionProjectionEvents = useAudioGraphStore(
    (s) => s.sessionProjectionEvents,
  );
  const pipelineStatus = useAudioGraphStore((s) => s.pipelineStatus);
  const consumerHealth = useAudioGraphStore((s) => s.latestAudioConsumerHealth);
  const persistenceQueueBackpressure = useAudioGraphStore(
    (s) => s.persistenceQueueBackpressure,
  );
  const backpressuredSources = useAudioGraphStore(
    (s) => s.backpressuredSources,
  );

  // Ticks once a second while capturing — drives BOTH the elapsed timer and
  // the durability readout's "Ns ago" off the same clock, so they never
  // visibly disagree.
  const [nowMs, setNowMs] = useState(() => Date.now());
  useEffect(() => {
    if (!isCapturing) return;
    setNowMs(Date.now());
    const interval = setInterval(() => setNowMs(Date.now()), 1000);
    return () => clearInterval(interval);
  }, [isCapturing]);

  const [capturePending, setCapturePending] = useState(false);
  const [geminiPending, setGeminiPending] = useState(false);

  const elapsed =
    isCapturing && captureStartTime !== null
      ? formatElapsed(Math.floor((nowMs - captureStartTime) / 1000))
      : "00:00";

  // Durability readout: the most recent "notes" projection patch already in
  // the store IS the durably-persisted signal (the backend's notes writer
  // durably applies every patch it emits) — no new backend read.
  const lastNotesSavedAtMs = useMemo(() => {
    let latest: number | null = null;
    for (const patch of sessionProjectionEvents) {
      if (patch.kind !== "notes") continue;
      if (latest === null || patch.created_at_ms > latest) {
        latest = patch.created_at_ms;
      }
    }
    return latest;
  }, [sessionProjectionEvents]);
  const durabilitySeconds =
    lastNotesSavedAtMs === null
      ? null
      : Math.max(0, Math.floor((nowMs - lastNotesSavedAtMs) / 1000));

  const handleToggleCapture = useCallback(async () => {
    setCapturePending(true);
    try {
      if (isCapturing) {
        await stopCapture();
      } else {
        await startCaptureAndTranscribe();
      }
    } finally {
      setCapturePending(false);
    }
  }, [isCapturing, startCaptureAndTranscribe, stopCapture]);

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

  const canStart = selectedSourceIds.length > 0 && !isCapturing;

  // Route chip: settings-derived only, "planned:" never "observed" — see
  // this file's doc comment and utils/durableRoute.ts.
  const routeConfigured = hasConfiguredDurableNotesRoute(
    settings,
    credentialPresence,
    modelStatus,
    // aws_bedrock + a profile credential source is the one case this chip
    // cannot fully verify (AWS profile enumeration isn't persisted to the
    // store — App.tsx's probe keeps it local) — documented gap, not silent.
    [],
  );
  const plannedRoute = describePlannedRoute(settings);
  const routeLabel =
    routeConfigured && plannedRoute
      ? t("nowStrip.routePlanned", { route: plannedRoute })
      : t("nowStrip.routeUnconfigured");

  // Composite health (50e3 fold) — one classification shared with
  // PipelineStatusBar's footer collapse, see utils/pipelineHealth.ts.
  const consumerDroppedChunks = useMemo(
    () =>
      consumerHealth
        ? consumerHealth.consumers.reduce(
            (sum, consumer) => sum + consumer.dropped_chunks,
            0,
          )
        : 0,
    [consumerHealth],
  );
  const health = computeCompositeHealth({
    pipelineStatus,
    consumerDroppedChunks,
    persistenceQueueBackpressure,
    backpressuredSourceCount: backpressuredSources.length,
  });

  // Gemini gating — unchanged from the old ControlBar (ADR-0013 sibling
  // mode; demoted here, not removed, per this file's NAMED TRANSITIONAL
  // STATE doc comment above).
  const hasGeminiKey =
    settings?.gemini?.auth?.type === "api_key" ||
    settings?.gemini?.auth?.type === "vertex_ai";
  const realtimeAgentProviderId =
    converseRealtimeAgentProvider === "openai"
      ? "realtime_agent.openai_realtime"
      : "realtime_agent.gemini_live";
  const realtimeAgentSelectable =
    PROVIDER_DESCRIPTORS.get(realtimeAgentProviderId)?.ui_selectable === true;
  const hasRealtimeAgentAuth =
    converseRealtimeAgentProvider === "openai" ? true : hasGeminiKey;
  const canGemini =
    isCapturing &&
    !isGeminiActive &&
    hasRealtimeAgentAuth &&
    realtimeAgentSelectable;
  const geminiVisible =
    isGeminiActive ||
    (conversationMode === "converse" && converseEngine === "native");
  const geminiDisabled = (!canGemini && !isGeminiActive) || geminiPending;
  const geminiReason = isGeminiActive
    ? t("controlBar.stopRealtimeHint")
    : !realtimeAgentSelectable
      ? t("controlBar.engineNotInMvp")
      : !hasRealtimeAgentAuth
        ? t("controlBar.geminiNeedsKey")
        : !isCapturing
          ? t("controlBar.geminiNeedsCapture")
          : t("controlBar.geminiHint");

  return (
    <header
      className="now-strip flex items-center justify-between px-(--space-6) bg-bg-tertiary border-b border-(--edge) h-[52px] flex-shrink-0 gap-(--space-6)"
      role="toolbar"
      aria-label={t("controlBar.toolbarLabel")}
    >
      <div className="now-strip__brand flex items-center min-w-[140px]">
        {/* Brand: 16px/600, --text-primary — NOT accent-colored (plan §R3).
            The Start/Stop button below is the strip's one intentionally
            saturated element while idle. */}
        <h1 className="text-[16px] font-semibold text-text-primary m-0 tracking-normal">
          AudioGraph
        </h1>
      </div>

      <div className="now-strip__center flex items-center gap-(--space-4) flex-1 justify-center">
        <button
          type="button"
          className={`py-(--space-3) px-(--space-8) rounded-md text-base font-semibold cursor-pointer transition-[background-color,border-color,opacity] duration-(--motion-base) ease-(--ease-standard) border-2 border-transparent leading-[1.4] ${isCapturing ? "bg-accent-red text-(--on-accent-red) border-accent-red hover:bg-(--accent-red-hover) hover:border-(--accent-red-hover)" : "bg-accent-green text-(--on-accent-green) border-accent-green enabled:hover:bg-(--accent-green-hover) enabled:hover:border-(--accent-green-hover) disabled:opacity-40 disabled:cursor-not-allowed"}`}
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
            </>
          ) : (
            <>
              <Icon name="start" size={16} /> {t("controlBar.start")}
            </>
          )}
          {capturePending && (
            <span className="ml-(--space-2) opacity-70" aria-hidden="true">
              …
            </span>
          )}
        </button>

        {isCapturing ? (
          <>
            <span
              className="font-mono text-[15px] font-semibold text-text-primary tracking-wide tabular-nums min-w-[50px]"
              aria-live="polite"
              aria-atomic="true"
            >
              {elapsed}
            </span>
            {durabilitySeconds !== null && (
              <span className="text-sm text-text-secondary whitespace-nowrap">
                {t("nowStrip.durabilitySaved", { seconds: durabilitySeconds })}
              </span>
            )}
            <span className="ag-chip" data-tone="neutral" title={routeLabel}>
              {routeLabel}
            </span>
            <button
              type="button"
              className="ag-chip cursor-pointer border-none"
              data-tone={HEALTH_TONE[health]}
              onClick={() => setSystemDrawerOpen(true)}
              aria-label={t("pipeline.openSystemStatus")}
              title={t(`pipeline.${health}`)}
            >
              <Icon name="system" size={12} /> {t(`pipeline.${health}`)}
            </button>
          </>
        ) : (
          <>
            <span className="text-sm text-text-secondary whitespace-nowrap">
              {t("workspace.stateIdle")} ·{" "}
              {t("controlBar.sourcesSummary", {
                count: selectedSourceIds.length,
              })}
            </span>
            <span className="ag-chip" data-tone="neutral" title={routeLabel}>
              {routeLabel}
            </span>
            {/* Idle reach for SystemDrawer (50e3's "no regression to
                diagnostics" half): the composite health chip is the ONLY
                caller of `setSystemDrawerOpen`, and it used to live only in
                the isCapturing branch above — leaving PipelineStageDetail
                (and TokenUsagePanel) with zero reach while idle, the exact
                state master's per-stage footer always showed. Neutral tone
                and copy deliberately here (`systemDrawer.title`, not
                `pipeline.healthy`/"All systems normal") — nothing has run
                yet, so this entry point does not assert an observed health
                claim (ADR-0030/0034), it only opens the drawer. Stays the
                strip's non-saturated idle chrome, same as the route chip
                beside it. */}
            <button
              type="button"
              className="ag-chip cursor-pointer border-none"
              data-tone="neutral"
              onClick={() => setSystemDrawerOpen(true)}
              aria-label={t("pipeline.openSystemStatus")}
              title={t("systemDrawer.title")}
            >
              <Icon name="system" size={12} /> {t("systemDrawer.title")}
            </button>
          </>
        )}

        {/* NAMED TRANSITIONAL STATE — see file doc comment. Rendered
            unconditionally (B20 / ADR-0016: pipeline controls stay
            discoverable pre-capture, `aria-disabled` rather than gated out
            of the DOM), demoted via a wrapping filter rather than by
            editing ConversationModeControl/the Gemini button's own
            internals (out of this unit's file scope). */}
        <div className="flex items-center gap-(--space-3) opacity-75 grayscale-[35%]">
          <ConversationModeControl />
          {geminiVisible && (
            <>
              <Tooltip content={geminiReason}>
                <button
                  type="button"
                  className={`py-(--space-2) px-(--space-5) rounded-md text-sm font-semibold cursor-pointer transition-[background-color,border-color,color,opacity] duration-(--motion-base) ease-(--ease-standard) border-2 bg-transparent leading-[1.4] flex items-center gap-(--space-2) aria-disabled:opacity-30 aria-disabled:cursor-not-allowed aria-disabled:border-text-muted aria-disabled:text-text-muted ${isGeminiActive ? "bg-(--accent-gemini) text-(--on-accent-gemini) border-(--accent-gemini) hover:bg-(--accent-gemini-hover) hover:border-(--accent-gemini-hover)" : "border-(--accent-gemini) text-(--accent-gemini) hover:bg-(--tint-gemini)"}`}
                  onClick={() => {
                    if (geminiDisabled) return;
                    void handleToggleGemini();
                  }}
                  aria-disabled={geminiDisabled}
                  aria-label={
                    isGeminiActive
                      ? t("controlBar.stopRealtimeLabel")
                      : t("controlBar.startGeminiLabel")
                  }
                  aria-describedby="now-strip-gemini-reason"
                  aria-pressed={isGeminiActive}
                  aria-busy={geminiPending}
                >
                  {isGeminiActive
                    ? t("controlBar.stopRealtime")
                    : t("controlBar.gemini")}
                </button>
              </Tooltip>
              <span id="now-strip-gemini-reason" className="sr-only">
                {geminiReason}
              </span>
            </>
          )}
        </div>
      </div>

      <div className="now-strip__actions flex items-center justify-end min-w-[140px]">
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

export default NowStrip;
