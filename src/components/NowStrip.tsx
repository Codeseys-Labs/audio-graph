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
 *   - Idle collapses to one line: Start, the `.workspace-switcher__state`
 *     destination-state region ("Ready" — SHELL-R4, plan §R4, ADR-0046
 *     relocated this region here from the destination bar, class name and
 *     `workspace.stateLive` text kept byte-identical so `shell.e2e.ts` test 4
 *     needed zero edits), the selected-source count, the planned-route chip,
 *     and a neutral-tone "System status" chip (opens `SystemDrawer` — SAME
 *     action as the live health chip below, just without a health-tone
 *     claim, since nothing has run yet to classify; keeps the drawer's
 *     per-stage/token detail one click away even before capture starts, the
 *     50e3 fold's "no regression to diagnostics" half). Start is the ONLY
 *     saturated (accent-colored) element among those five — everything else
 *     is neutral text or a `.ag-chip[data-tone="neutral"]` (screenshot
 *     evidence lives in the landing PR body).
 *   - Live adds the same `.workspace-switcher__state` region (now reading
 *     "Live session"), the elapsed timer (tabular-nums), a durability
 *     readout ("notes saved · Ns ago", derived from the most recent "notes"
 *     projection patch already in `sessionProjectionEvents` — no new
 *     backend read), and swaps the neutral idle chip for the composite
 *     health chip (same `setSystemDrawerOpen` action, now tone/label-coded
 *     off `utils/pipelineHealth.ts`'s live classification).
 *
 * RELOCATED (SHELL-R5, plan §R5, ADR-0046): `ConversationModeControl` and
 * the Gemini toggle — demoted ghost controls here since SHELL-R3 named them
 * "NAMED TRANSITIONAL STATE, do not fix — R5 owns the follow-up" — have now
 * moved into `PreflightCard` as a preflight choice ("Mode: Notes /
 * Converse"), out of primary chrome per the plan. Neither renders in this
 * strip anymore, live or idle. FULL STATEMENT (review correction — the
 * original note here undersold this): this strip was the ONLY chrome that
 * rendered either control while `isCapturing` was true (it never unmounts on
 * capture start/stop; `PreflightCard` does the opposite — it only mounts
 * while idle). So the relocation doesn't just lose "a chrome-level way to
 * start Gemini mid-capture" — it loses every live-reachable entry point for
 * BOTH starting a native-converse realtime session and stopping one that's
 * already running, full stop. See `PreflightCard.tsx`'s KNOWN GAP doc
 * comment for the complete disclosure and why this is escalated rather than
 * fixed in this pass. The standalone Transcribe toggle was REMOVED outright
 * at R3 (composed into Start instead — "purple leaves the strip for free"
 * per the recomposition plan); a manual "stop transcription without
 * stopping capture" affordance is not carried forward — that one WAS an
 * intentional, disclosed simplification, unlike the Gemini gap above.
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
import Icon from "./Icon";
import IconButton from "./IconButton";

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
  // SHELL-R4 (plan §R4, ADR-0046): read alongside `isCapturing` so the
  // relocated `.workspace-switcher__state` region below can reproduce the
  // exact 4-way ternary the destination bar used to render.
  const samplePreviewActive = useAudioGraphStore((s) => s.samplePreviewActive);
  const loadedSessionId = useAudioGraphStore((s) => s.loadedSessionId);
  const selectedSourceIds = useAudioGraphStore((s) => s.selectedSourceIds);
  const captureStartTime = useAudioGraphStore((s) => s.captureStartTime);
  const settings = useAudioGraphStore((s) => s.settings);
  const credentialPresence = useAudioGraphStore((s) => s.credentialPresence);
  const modelStatus = useAudioGraphStore((s) => s.modelStatus);
  const startCaptureAndTranscribe = useAudioGraphStore(
    (s) => s.startCaptureAndTranscribe,
  );
  const stopCapture = useAudioGraphStore((s) => s.stopCapture);
  const openSettings = useAudioGraphStore((s) => s.openSettings);
  const openSessionsBrowser = useAudioGraphStore((s) => s.openSessionsBrowser);
  const setSystemDrawerOpen = useAudioGraphStore((s) => s.setSystemDrawerOpen);
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

        {/* SHELL-R4 (plan §R4, ADR-0046): relocated verbatim from the
            destination bar's `ShellDestinationBar` — class name and the
            `workspace.stateLive` text stay byte-identical (`shell.e2e.ts`
            test 4 and its `.workspace-switcher__state` selector needed zero
            edits), only the position in the tree changed. */}
        <div className="workspace-switcher__state" aria-live="polite">
          {isCapturing ? (
            <span>{t("workspace.stateLive")}</span>
          ) : samplePreviewActive ? (
            <span>{t("workspace.stateSample")}</span>
          ) : loadedSessionId ? (
            <span>{t("workspace.stateLoaded")}</span>
          ) : (
            <span>{t("workspace.stateIdle")}</span>
          )}
        </div>

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
          // Settings T1 (seed audio-graph-2b9a) widened `openSettings` to
          // take an optional route; `onClick` would otherwise forward the
          // click's `MouseEvent` as that argument. Bare navigation only
          // here — unchanged behavior, new signature.
          onClick={() => openSettings()}
        />
      </div>
    </header>
  );
}

export default NowStrip;
