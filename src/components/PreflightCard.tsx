/**
 * PREFLIGHT CARD — the Capture destination's idle-state surface (SHELL-R5,
 * plan §R5, ADR-0046). Replaces the pre-R5 "empty live cockpit" (NotesPanel +
 * LiveTranscript rendered with nothing in them yet) with a checklist the
 * user can act on before starting a session, using the `.ag-card`/`.ag-field`
 * tier-3 recipes (ADR-0047 names this card as their first intended adopter).
 *
 * Three pass/fail rows (`.ag-field[data-layout="row"]`), each with a fix
 * action:
 *   - Sources — n selected, plus (fold of seed audio-graph-4a22) the
 *     resolved source NAME. SHELL-R3 dropped the pre-R3 `ControlBar`'s
 *     `selectedLabels` resolution (parseCaptureTargetId/audioSources/
 *     processes) when it became `NowStrip` — the strip only ever needed a
 *     bare count. That logic is recovered VERBATIM from git history into
 *     `utils/captureTarget.ts`'s `describeSelectedSourceLabels`; this row is
 *     its first R5+ consumer. A single selection shows just the resolved
 *     name; multiple selections show the count AND every resolved name
 *     (joined, with the untruncated join repeated in `detailTitle`) — master
 *     ControlBar's `selectedLabels.join(", ")` behavior, restored for the
 *     multi-select case too, not just computed and discarded. Fix action
 *     focuses the source search input.
 *   - Route — "planned: {ASR} → {LLM}", the SAME settings-derived read the
 *     NowStrip chip uses (`hasConfiguredDurableNotesRoute`/
 *     `describePlannedRoute`, `utils/durableRoute.ts` — passive, no
 *     provider egress, ADR-0028). Always labeled "planned", never "observed"
 *     (ADR-0030/0034) — credential presence alone never renders as "Ready".
 *     Fix action opens Settings (the same `openSettings()` the gear icon
 *     already uses — not a new invoke).
 *   - Storage — reuses `StorageBanner`'s own module-level state
 *     (`useCaptureStorageFullState`, `./StorageBanner.tsx`), a passive read
 *     with no invoke of its own. Fix action focuses the banner's own Resume
 *     button (`#storage-banner-resume`) rather than re-invoking
 *     `retry_storage_write` from a second call site, so the set of backend
 *     commands reachable from the idle Capture surface doesn't grow.
 *
 * One "Start session" button dispatches the SAME `startCaptureAndTranscribe`
 * action the NowStrip's own Start button does (see that action's doc
 * comment in `store/index.ts` for the ADR-0033 gating proof) — a second call
 * site for the identical action, not a parallel one.
 *
 * `ConversationModeControl` and the Gemini realtime toggle move IN HERE from
 * `NowStrip` as a preflight choice ("Mode: Notes / Converse") — SHELL-R3's
 * "NAMED TRANSITIONAL STATE" doc comment explicitly deferred this move to
 * R5, and `NowStrip` no longer renders either.
 *
 * KNOWN GAP, FULL STATEMENT (review finding, SHELL-R5 fix pass — disclosed
 * plainly here, not softened): `startGemini` hard-requires `isCapturing`
 * (`store/index.ts`), but this card only ever mounts while `isCapturing` is
 * false (`App.tsx`'s `showPreflightCard` predicate) and unmounts the instant
 * it flips true. Those two conditions are mutually exclusive, so `canGemini`
 * below is FALSE on every render this card can ever produce — the
 * Start-Gemini button is not "reachable except mid-capture", it is
 * PERMANENTLY inert in its new home, full stop. Before R5, `NowStrip`
 * rendered this same toggle unconditionally (idle AND live, since the strip
 * itself never unmounts), so a user could click "Start Gemini" once capture
 * was running, and — separately — click "Stop realtime session" to tear an
 * active one down without stopping the whole capture. R5's relocation loses
 * BOTH: there is no chrome-level way left anywhere in the app to start a
 * native-converse realtime session, or to stop one that's already running
 * short of `stopCapture()` ending the entire session (which clears
 * `isGeminiActive` in the store but does not itself invoke the matching
 * `stop_gemini`/`stop_converse`/`stop_openai_realtime` teardown — see that
 * action's own comment). The wiring below stays end-to-end correct (so a
 * future unit can give it a live-reachable home) but has no path to ever run
 * in production today. This needs its own scoped design decision — does
 * "Start session" auto-start the realtime session when native converse mode
 * is selected, does live chrome come back, or something else — not a
 * unilateral call from this pass; flagged as a blocking follow-up-seed
 * candidate, escalated rather than guessed at.
 *
 * Passive reads only (ADR-0028): every field this card reads is already in
 * the store from App.tsx's own mount-time credential-presence probe or the
 * source/process fetches AudioSourceSelector already performs — this card
 * issues zero invoke calls of its own.
 *
 * Parent: `App.tsx` (Capture destination, idle state only). No props.
 */
import { useCallback, useState } from "react";
import { useTranslation } from "react-i18next";
import { useAudioGraphStore } from "../store";
import { describeSelectedSourceLabels } from "../utils/captureTarget";
import {
  describePlannedRoute,
  hasConfiguredDurableNotesRoute,
} from "../utils/durableRoute";
import ConversationModeControl from "./ConversationModeControl";
import Icon from "./Icon";
import { PROVIDER_DESCRIPTORS } from "./providerRegistryHelpers";
import { useCaptureStorageFullState } from "./StorageBanner";
import Tooltip from "./Tooltip";

function focusById(id: string) {
  document.getElementById(id)?.focus();
}

interface PreflightRowProps {
  testId: string;
  label: string;
  detail: string;
  detailTitle?: string;
  pass: boolean;
  actionLabel: string;
  onAction: () => void;
  actionDisabled?: boolean;
}

/** One checklist row: label + status detail on the left, a pass/fail chip
 * and its fix action on the right — `.ag-field[data-layout="row"]`. */
function PreflightRow({
  testId,
  label,
  detail,
  detailTitle,
  pass,
  actionLabel,
  onAction,
  actionDisabled = false,
}: PreflightRowProps) {
  const { t } = useTranslation();
  return (
    <div className="ag-field" data-layout="row" data-testid={testId}>
      <div className="flex flex-col gap-(--space-1) min-w-0">
        <span className="ag-label">{label}</span>
        <span
          className="text-sm text-text-secondary truncate max-w-[320px]"
          title={detailTitle ?? detail}
        >
          {detail}
        </span>
      </div>
      <div className="flex items-center gap-(--space-3) shrink-0">
        <span
          className="ag-chip"
          data-tone={pass ? "success" : "warning"}
          data-testid={`${testId}-status`}
        >
          <Icon name={pass ? "success" : "warning"} size={12} />
          {pass ? t("preflight.statusPass") : t("preflight.statusFail")}
        </span>
        <button
          type="button"
          className="ag-btn-micro"
          onClick={onAction}
          disabled={actionDisabled}
        >
          {actionLabel}
        </button>
      </div>
    </div>
  );
}

function PreflightCard() {
  const { t } = useTranslation();
  const selectedSourceIds = useAudioGraphStore((s) => s.selectedSourceIds);
  const audioSources = useAudioGraphStore((s) => s.audioSources);
  const processes = useAudioGraphStore((s) => s.processes);
  const settings = useAudioGraphStore((s) => s.settings);
  const credentialPresence = useAudioGraphStore((s) => s.credentialPresence);
  const modelStatus = useAudioGraphStore((s) => s.modelStatus);
  const isCapturing = useAudioGraphStore((s) => s.isCapturing);
  const startCaptureAndTranscribe = useAudioGraphStore(
    (s) => s.startCaptureAndTranscribe,
  );
  const openSettings = useAudioGraphStore((s) => s.openSettings);

  // Gemini/converse wiring — relocated verbatim from NowStrip (SHELL-R3's
  // NAMED TRANSITIONAL STATE deferred this move to R5). See the KNOWN GAP
  // doc comment above this component.
  const isGeminiActive = useAudioGraphStore((s) => s.isGeminiActive);
  const startGemini = useAudioGraphStore((s) => s.startGemini);
  const stopGemini = useAudioGraphStore((s) => s.stopGemini);
  const conversationMode = useAudioGraphStore((s) => s.conversationMode);
  const converseEngine = useAudioGraphStore((s) => s.converseEngine);
  const converseRealtimeAgentProvider = useAudioGraphStore(
    (s) => s.converseRealtimeAgentProvider,
  );

  const [capturePending, setCapturePending] = useState(false);
  const [geminiPending, setGeminiPending] = useState(false);

  // Storage row's data source — a passive read of the exact state
  // StorageBanner itself renders from (see that file's doc comment).
  const storagePayload = useCaptureStorageFullState();

  const handleStart = useCallback(async () => {
    setCapturePending(true);
    try {
      await startCaptureAndTranscribe();
    } finally {
      setCapturePending(false);
    }
  }, [startCaptureAndTranscribe]);

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

  // ── Sources row ─────────────────────────────────────────────────────────
  const sourceLabels = describeSelectedSourceLabels(
    selectedSourceIds,
    audioSources,
    processes,
  );
  const sourcesPass = selectedSourceIds.length > 0;
  // The ticket asks for "n selected, AND the resolved source NAME" — for a
  // single selection the resolved name IS the detail; for multiple, join
  // every resolved name behind the count so the fold's output is actually
  // rendered (not computed-and-discarded) in the multi-select case too. The
  // row's own `truncate max-w-[320px]` handles overflow; `detailTitle` below
  // carries the untruncated join for a hover/AT-accessible full list, same
  // pattern as the Storage row's `detailTitle`.
  const sourcesDetail = sourcesPass
    ? sourceLabels.length === 1
      ? sourceLabels[0]
      : `${t("controlBar.sourcesSummary", { count: sourceLabels.length })} (${sourceLabels.join(", ")})`
    : t("preflight.sourcesEmpty");
  const sourcesDetailTitle =
    sourcesPass && sourceLabels.length > 1
      ? sourceLabels.join(", ")
      : undefined;

  // ── Route row ───────────────────────────────────────────────────────────
  const routeConfigured = hasConfiguredDurableNotesRoute(
    settings,
    credentialPresence,
    modelStatus,
    // aws_bedrock + a profile credential source is the one case this row
    // cannot fully verify (AWS profile enumeration isn't persisted to the
    // store — App.tsx's probe keeps it local) — documented gap, not silent,
    // same disclosure as the NowStrip chip this row mirrors.
    [],
  );
  const plannedRoute = describePlannedRoute(settings);
  const routeDetail =
    routeConfigured && plannedRoute
      ? t("nowStrip.routePlanned", { route: plannedRoute })
      : t("nowStrip.routeUnconfigured");

  // ── Storage row ─────────────────────────────────────────────────────────
  const storagePass = storagePayload === null;
  const storageDetail = storagePass
    ? t("preflight.storageOk")
    : t("storage.title");

  const canStart = selectedSourceIds.length > 0 && !isCapturing;

  // Gemini gating — unchanged from NowStrip/the old ControlBar (ADR-0013
  // sibling mode).
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
    <section
      className="ag-card flex flex-col gap-(--space-6) max-w-[560px] w-full mx-auto my-(--space-8)"
      data-elevation="raised"
      aria-label={t("preflight.title")}
      data-testid="preflight-card"
    >
      <h2 className="m-0 text-text-primary text-lg font-semibold">
        {t("preflight.title")}
      </h2>

      <div className="flex flex-col gap-(--space-4)">
        <PreflightRow
          testId="preflight-row-sources"
          label={t("preflight.sourcesLabel")}
          detail={sourcesDetail}
          detailTitle={sourcesDetailTitle}
          pass={sourcesPass}
          actionLabel={t("preflight.sourcesFixAction")}
          onAction={() => focusById("audio-source-search")}
        />
        <PreflightRow
          testId="preflight-row-route"
          label={t("preflight.routeLabel")}
          detail={routeDetail}
          pass={routeConfigured}
          actionLabel={t("controlBar.configure")}
          onAction={openSettings}
        />
        <PreflightRow
          testId="preflight-row-storage"
          label={t("preflight.storageLabel")}
          detail={storageDetail}
          detailTitle={storagePass ? undefined : t("storage.message")}
          pass={storagePass}
          actionLabel={t("preflight.storageFixAction")}
          onAction={() => focusById("storage-banner-resume")}
          actionDisabled={storagePass}
        />
      </div>

      <div className="flex flex-col gap-(--space-3) pt-(--space-4) border-t border-(--edge-subtle)">
        <span className="ag-label">{t("preflight.modeLabel")}</span>
        <div className="flex items-center gap-(--space-3) flex-wrap">
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
                  aria-describedby="preflight-gemini-reason"
                  aria-pressed={isGeminiActive}
                  aria-busy={geminiPending}
                >
                  {isGeminiActive
                    ? t("controlBar.stopRealtime")
                    : t("controlBar.gemini")}
                </button>
              </Tooltip>
              <span id="preflight-gemini-reason" className="sr-only">
                {geminiReason}
              </span>
            </>
          )}
        </div>
      </div>

      <button
        type="button"
        className="self-start py-(--space-3) px-(--space-8) rounded-md text-base font-semibold cursor-pointer transition-[background-color,border-color,opacity] duration-(--motion-base) ease-(--ease-standard) border-2 border-transparent leading-[1.4] bg-accent-green text-(--on-accent-green) border-accent-green enabled:hover:bg-(--accent-green-hover) enabled:hover:border-(--accent-green-hover) disabled:opacity-40 disabled:cursor-not-allowed"
        onClick={() => void handleStart()}
        disabled={!canStart || capturePending}
        aria-busy={capturePending}
      >
        <Icon name="start" size={16} /> {t("preflight.startSession")}
        {capturePending && (
          <span className="ml-(--space-2) opacity-70" aria-hidden="true">
            …
          </span>
        )}
      </button>
    </section>
  );
}

export default PreflightCard;
