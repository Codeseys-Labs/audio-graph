/**
 * Root React component for the AudioGraph Tauri window.
 *
 * Layout (desktop-first; SHELL-R4, ADR-0046 collapses the shell to two
 * destinations — Capture and Sessions — deleting the old three-tab
 * during/after/analysis shell and its "Inspect" peer tab):
 *   - Top: `StorageBanner` (ENOSPC retry) + `DemoModeBanner` (first-launch
 *     local-only hint) + `NowStrip` (Start/Stop, elapsed, durability, planned
 *     route, composite health, settings/sessions, and the destination
 *     `.workspace-switcher__state` live region — SHELL-R3/R4, ADR-0046;
 *     replaces the old `ControlBar`).
 *   - Destination bar: Capture / Sessions.
 *   - Middle flex (SHELL-R7, plan §R7, ADR-0046 — `useShellLayout()` tiers):
 *       - Left rail: `AudioSourceSelector`, plus `SpeakerPanel` too at the
 *         `wide` tier (>=1280px). Below `wide`, `SpeakerPanel` becomes a
 *         right-hand focus-trapped drawer (`standard`, 1024-1279px); below
 *         `standard` (`compact`, <1024px — the floor; there is deliberately
 *         no further "stack" tier), `AudioSourceSelector` becomes a matching
 *         left-hand drawer too. Both drawers reuse `ShellDrawer`'s shared
 *         `useFocusTrap` + Escape chrome (same pattern `SystemDrawer`
 *         established, D5: no Radix dialog).
 *       - Main: destination-specific content. Capture is a 3-way choice —
 *         `GetStartedFallback` (the credential-presence probe threw),
 *         `PreflightCard` (genuinely idle — SHELL-R5, ADR-0046: a pass/fail
 *         checklist replacing the pre-R5 "empty live cockpit"), or the live/
 *         reviewing NotesPanel + LiveTranscript + AgentProposalsPanel trio.
 *         Sessions is `SessionsBrowser`'s list→detail + lenses — the old
 *         Inspect tab's graph/diagnostics reach now lives in the Sessions
 *         Graph/Route lenses (R2) and the NowStrip System drawer (R3)
 *   - Bottom: `PipelineStatusBar` (per-stage status dots; collapses to the
 *     composite health state during healthy capture, SHELL-R3 folds 50e3).
 *   - Overlays: error toast, `SettingsPage` modal, `SessionsBrowser` modal,
 *     `SystemDrawer` (projection runtime + token usage + per-stage pipeline
 *     detail, opened from NowStrip's health chip — replaces the retired
 *     `PopoverOverlay`), `ShortcutsHelpModal`, first-launch `ExpressSetup`
 *     quickstart, `Notifications` (unified transient feedback + error queue,
 *     ADR-0011).
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

import {
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
// `safeInvoke` (aliased to `invoke`) is a drop-in for the Tauri `invoke` that
// relays a command-name-only failure diagnostic to analytics then rethrows, so
// this call site's error handling is unchanged (audio-graph-3e71).
import { safeInvoke as invoke } from "./analytics/safeInvoke";
import AgentProposalsPanel from "./components/AgentProposalsPanel";
import AudioSourceSelector from "./components/AudioSourceSelector";
import IconButton from "./components/IconButton";
import LiveTranscript from "./components/LiveTranscript";
import NotesPanel from "./components/NotesPanel";
// SHELL-R3 (plan §R3, ADR-0046): ControlBar -> NowStrip (restyle + one Start
// + composite health chip; see NowStrip.tsx's doc comment).
import NowStrip from "./components/NowStrip";
import PipelineStatusBar from "./components/PipelineStatusBar";
import ResizeDivider from "./components/ResizeDivider";
// SHELL-R2 (plan §R2, ADR-0046): SessionsBrowser stops being lazy — it is no
// longer a conditionally-rendered modal but the Sessions panel's
// always-composed content whenever that tab is active. Its own Graph lens
// carries its own deferred `KnowledgeGraphViewer` import — this file no
// longer has one of its own (SHELL-R4 deleted the `analysis` tab's
// duplicate, per the plan's "deliberate interim duplication... is what
// makes R4 pure deletion") — Rollup still dedupes the vendor chunk by module
// specifier, so this doesn't re-bundle react-force-graph-2d into the main
// chunk; verified via `bun run build:analyze`.
import SessionsBrowser from "./components/SessionsBrowser";
import ShortcutsHelpModal from "./components/ShortcutsHelpModal";
import SpeakerPanel from "./components/SpeakerPanel";

// Code-split (ADR-0016 / modernization-audit 2.3): these modals/first-run
// flows are rendered conditionally — lazy-loading them keeps the initial
// bundle lean.
const SettingsPage = lazy(() => import("./components/SettingsPage"));
const ExpressSetup = lazy(() => import("./components/ExpressSetup"));

import DemoModeBanner from "./components/DemoModeBanner";
import GetStartedFallback from "./components/GetStartedFallback";
import Notifications from "./components/Notifications";
// SHELL-R5 (plan §R5, ADR-0046): the Capture destination's idle-state
// surface — see this file's doc comment on `ShellRailContentAside`.
import PreflightCard from "./components/PreflightCard";
// SHELL-R7 (plan §R7, ADR-0046): shared drawer chrome for the rail/aside
// drawers `useShellLayout()`'s tier drives — see that hook's doc comment.
import ShellDrawer from "./components/ShellDrawer";
import StorageBanner from "./components/StorageBanner";
import SystemDrawer from "./components/SystemDrawer";
import { ONBOARDING_HANDOFF_SEEN_KEY } from "./constants/storageKeys";
import { useConverseFrontLeg } from "./hooks/useConverseFrontLeg";
import { useKeyboardShortcuts } from "./hooks/useKeyboardShortcuts";
import { useNativeCapture } from "./hooks/useNativeCapture";
import { useShellLayout } from "./hooks/useShellLayout";
import { useTauriEvents } from "./hooks/useTauriEvents";
import { SessionViewProvider } from "./session/SessionViewProvider";
import { useAudioGraphStore } from "./store";
// ShellNav (SHELL-R1, ADR-0046): `deriveWorkspaceView` is the pure function
// that replaces the old App-local `workspaceView` `useState`. SHELL-R4
// retired its three-view during/after/analysis mapping outright — see
// `store/shellNav.ts` for the current two-destination shape.
import { deriveWorkspaceView } from "./store/shellNav";
import type { AppSettings, CredentialPresence, ModelStatus } from "./types";
// SHELL-R3 (plan §R3, ADR-0046): moved out of this file so NowStrip's
// planned-route chip can read the SAME predicate this probe uses — see
// `utils/durableRoute.ts`'s doc comment.
import { hasConfiguredDurableNotesRoute } from "./utils/durableRoute";
import "./styles/index.css";

// cred-review m6: a rejected `load_credential_presence_cmd` carries a
// structured `AppError` payload (`{ code, message: { reason } }`). The backend
// collapses EVERY presence-load failure into `credential_file_error` —
// including OS-keychain-locked/unavailable loads (e.g. Linux with no
// secret-service) — so the code alone cannot mean "your credential file needs
// repair". For a keychain-locked user, retry (after unlocking) is the right
// recovery, so ambiguous causes default to the generic retryable fallback
// copy. Only a RECOGNIZED file parse failure earns the review-before-
// re-entering hint (`onboarding.unreadable*`), and parse failures are
// recognizable without echoing file content: `redacted_yaml_parse_error`
// (credentials/mod.rs, PR #81) deliberately keeps the stable
// `Failed to parse {path}:` prefix in the structured reason.
function isCredentialFileParseError(e: unknown): boolean {
  if (typeof e !== "object" || e === null) return false;
  const { code, message } = e as { code?: unknown; message?: unknown };
  if (code !== "credential_file_error") return false;
  const reason =
    typeof message === "object" && message !== null
      ? (message as { reason?: unknown }).reason
      : undefined;
  return typeof reason === "string" && reason.startsWith("Failed to parse ");
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

// SHELL-R4 (plan §R4, ADR-0046): collapses the shell to the two ADR-0046
// destinations. The legacy `during`/`after`/`analysis` three-view shell (and
// the `analysis` tab's separate graph/diagnostics panel) is deleted outright
// — every occupant already has a home (Graph/Route/Ask lenses from R2;
// diagnostics in the R3 System drawer).
const WORKSPACE_VIEWS = ["capture", "sessions"] as const;
type WorkspaceView = (typeof WORKSPACE_VIEWS)[number];

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

function focusWorkspaceTab(view: WorkspaceView) {
  const focus = () => document.getElementById(`workspace-tab-${view}`)?.focus();
  if (typeof requestAnimationFrame === "function") {
    requestAnimationFrame(focus);
  } else {
    setTimeout(focus, 0);
  }
}

// ── Composition seams (SHELL-R1, ADR-0046) ─────────────────────────────────
// App.tsx's four named regions, extracted verbatim so later units (R2-R5)
// have a seam to land new chrome on without re-touching sibling regions.
// Zero behavior/DOM change this unit — every prop below is exactly the value
// the inline JSX it replaces already closed over; State/handlers stay owned
// by `App()`, which is why regions take explicit props rather than reading
// the store directly (that keeps this a pure JSX-structure move, not a data-
// flow change).

interface ShellChromeProps {
  workspaceView: WorkspaceView;
  recordingAnnouncement: string;
  phaseAnnouncement: string;
  handoffVisible: boolean;
  dismissHandoff: () => void;
}

/** Region 1: banners + chrome — skip link, live regions, top banners,
 * NowStrip, and the post-onboarding hand-off nudge. */
function ShellChrome({
  workspaceView,
  recordingAnnouncement,
  phaseAnnouncement,
  handoffVisible,
  dismissHandoff,
}: ShellChromeProps) {
  const { t } = useTranslation();
  return (
    <>
      {/* Skip-to-main link (seed audio-graph-4f2e / WCAG 2.4.1). Visually
          hidden until focused; jumps keyboard users past the banners + control
          bar straight to the active workspace panel (its id tracks the current
          phase). */}
      <a href={`#workspace-panel-${workspaceView}`} className="skip-to-main">
        {t("app.a11y.skipToMain")}
      </a>
      {/* Assertive recording-state announcement — distinct from the polite
          workspace-switcher state region below (WCAG 4.1.3). */}
      <div role="status" aria-live="assertive" className="sr-only">
        {recordingAnnouncement}
      </div>
      {/* Polite phase-transition announcement (critique B7). */}
      <div role="status" aria-live="polite" className="sr-only">
        {phaseAnnouncement}
      </div>
      <StorageBanner />
      <DemoModeBanner />
      <NowStrip />
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
            className="ml-auto shrink-0 py-(--space-2) px-(--space-5) rounded-md text-sm font-semibold cursor-pointer bg-accent-blue text-(--on-accent-blue) border-none hover:opacity-90"
            onClick={dismissHandoff}
            aria-label={t("onboarding.handoffDismissLabel")}
          >
            {t("onboarding.handoffDismiss")}
          </button>
        </aside>
      )}
    </>
  );
}

interface ShellDestinationBarProps {
  workspaceView: WorkspaceView;
  isCapturing: boolean;
  onSelectView: (view: WorkspaceView) => void;
  onTabKeyDown: (e: React.KeyboardEvent<HTMLButtonElement>) => void;
}

/** Region 2: destination bar — the Capture/Sessions tablist (SHELL-R4,
 * ADR-0046). The live `.workspace-switcher__state` region that used to live
 * here moved onto `NowStrip` this unit — class name and
 * `workspace.stateLive` text stayed byte-identical (E2E test 4 needed zero
 * edits), only its position in the tree changed, so `samplePreviewActive`/
 * `loadedSessionId` are no longer read here. */
function ShellDestinationBar({
  workspaceView,
  isCapturing,
  onSelectView,
  onTabKeyDown,
}: ShellDestinationBarProps) {
  const { t } = useTranslation();
  return (
    <nav className="workspace-switcher" aria-label={t("workspace.navigation")}>
      <div
        className="workspace-switcher__tabs"
        role="tablist"
        aria-label={t("workspace.label")}
      >
        {WORKSPACE_VIEWS.map((view) => (
          <button
            key={view}
            type="button"
            role="tab"
            id={`workspace-tab-${view}`}
            aria-selected={workspaceView === view}
            aria-controls={`workspace-panel-${view}`}
            tabIndex={workspaceView === view ? 0 : -1}
            className="workspace-switcher__tab"
            onClick={() => onSelectView(view)}
            onKeyDown={onTabKeyDown}
          >
            {view === "capture" && isCapturing
              ? t("workspace.liveNow")
              : t(`workspace.${view}`)}
          </button>
        ))}
      </div>
    </nav>
  );
}

interface ShellRailContentAsideProps {
  workspaceView: WorkspaceView;
  leftWidth: number;
  resizeLeft: (dx: number) => void;
  showGetStartedFallback: boolean;
  showPreflightCard: boolean;
  previewSampleSession: () => void;
  retryCredentialProbe: () => Promise<void>;
  openSettings: () => void;
  probeRetrying: boolean;
  probeUnreadable: boolean;
  hasAgentActivity: boolean;
  // SHELL-R7 (plan §R7, ADR-0046): `useShellLayout()`'s tier drives whether
  // the rail (`AudioSourceSelector`) and aside (`SpeakerPanel`) regions are
  // pinned inline here or collapsed behind a drawer `App()` renders itself
  // — see that component's doc comment on the wide/standard/compact split.
  railPinned: boolean;
  asidePinned: boolean;
  onOpenSourcesDrawer: () => void;
  onOpenSpeakersDrawer: () => void;
}

/** Region 3: rail + content + aside — the left source rail and the active
 * destination's main content (SHELL-R4, ADR-0046: the old Analysis-only
 * right transcript/chat aside — `analysisContextPanel` — is deleted outright
 * along with the `analysis` tab it belonged to; every occupant already has a
 * home in R2's Sessions lenses / R3's System drawer). The session-scoped
 * readers under here (`NotesPanel`, `LiveTranscript`) are wrapped in
 * `SessionViewProvider` by the caller.
 *
 * SHELL-R5 (plan §R5, ADR-0046): the capture branch is now a 3-way choice,
 * not 2-way — `showGetStartedFallback` (probe threw; unchanged, still
 * `GetStartedFallback`'s exact role), `showPreflightCard` (genuinely idle —
 * `PreflightCard` replaces the pre-R5 "empty live cockpit" of rendering
 * NotesPanel/LiveTranscript with nothing in them yet), or live/reviewing
 * (the original NotesPanel + LiveTranscript + AgentProposalsPanel trio).
 *
 * SHELL-R7 (plan §R7, ADR-0046): at the `wide` tier this renders BYTE-
 * IDENTICAL to the pre-R7 shape — `AudioSourceSelector` + `SpeakerPanel`
 * together in `.left-panel`, no drawers, no triggers — since `railPinned`
 * and `asidePinned` are both true there. Below `wide`, `SpeakerPanel` (the
 * aside) is the first to leave the pinned flow — a `shell-drawer-trigger`
 * strip takes its place at the trailing edge — and below `standard`,
 * `AudioSourceSelector` (the rail) follows it, replaced by a matching
 * leading-edge trigger. Neither trigger renders `ResizeDivider` alongside
 * it — dragging a divider next to a collapsed trigger strip has nothing to
 * resize. */
function ShellRailContentAside({
  workspaceView,
  leftWidth,
  resizeLeft,
  showGetStartedFallback,
  showPreflightCard,
  previewSampleSession,
  retryCredentialProbe,
  openSettings,
  probeRetrying,
  probeUnreadable,
  hasAgentActivity,
  railPinned,
  asidePinned,
  onOpenSourcesDrawer,
  onOpenSpeakersDrawer,
}: ShellRailContentAsideProps) {
  const { t } = useTranslation();

  return (
    <div className={`main-layout main-layout--${workspaceView}`}>
      {railPinned ? (
        // `tabIndex={-1}`: not in the tab order, but a resize-driven drawer
        // close (below) needs a programmatic focus target that survives the
        // trigger's unmount — see that effect's comment.
        <aside
          className="left-panel"
          style={{ width: leftWidth }}
          tabIndex={-1}
        >
          <AudioSourceSelector />
          {asidePinned && <SpeakerPanel />}
        </aside>
      ) : (
        <div className="shell-drawer-trigger shell-drawer-trigger--rail">
          <IconButton
            icon="mic"
            label={t("shellLayout.sources")}
            onClick={onOpenSourcesDrawer}
          />
        </div>
      )}
      {railPinned && (
        <ResizeDivider
          orientation="vertical"
          onResize={resizeLeft}
          ariaLabel={t("app.resizeSources")}
        />
      )}
      {workspaceView === "capture" &&
        (showGetStartedFallback ? (
          <main
            id="workspace-panel-capture"
            role="tabpanel"
            aria-labelledby="workspace-tab-capture"
            className="workspace-panel"
          >
            <GetStartedFallback
              onPreviewSample={previewSampleSession}
              onRetry={retryCredentialProbe}
              onOpenSettings={openSettings}
              retrying={probeRetrying}
              unreadable={probeUnreadable}
            />
          </main>
        ) : showPreflightCard ? (
          <main
            id="workspace-panel-capture"
            role="tabpanel"
            aria-labelledby="workspace-tab-capture"
            className="workspace-panel"
          >
            <PreflightCard />
          </main>
        ) : (
          <main
            id="workspace-panel-capture"
            role="tabpanel"
            aria-labelledby="workspace-tab-capture"
            className="workspace-panel workspace-panel--capture"
          >
            <section
              className="workspace-panel__primary"
              aria-label={t("workspace.duringNotes")}
            >
              <NotesPanel />
            </section>
            <section
              className="workspace-panel__transcript"
              aria-label={t("workspace.duringTranscript")}
            >
              <LiveTranscript />
            </section>
            {hasAgentActivity && (
              <section
                className="workspace-panel__assist"
                aria-label={t("workspace.liveAssist")}
              >
                <AgentProposalsPanel />
              </section>
            )}
          </main>
        ))}
      {workspaceView === "sessions" && (
        // SHELL-R2 (plan §R2, ADR-0046): the Sessions destination —
        // SessionsBrowser stopped being a modal and IS this panel's content
        // now (rail→detail with lens tabs). SHELL-R4 renames the id/aria
        // pair from `after` to `sessions`.
        <main
          id="workspace-panel-sessions"
          role="tabpanel"
          aria-labelledby="workspace-tab-sessions"
          className="workspace-panel"
        >
          <SessionsBrowser />
        </main>
      )}
      {!asidePinned && (
        <div className="shell-drawer-trigger shell-drawer-trigger--aside">
          <IconButton
            icon="speaker"
            label={t("shellLayout.speakers")}
            onClick={onOpenSpeakersDrawer}
          />
        </div>
      )}
    </div>
  );
}

/** Region 4: footer — the per-stage pipeline status strip. */
function ShellFooter() {
  return <PipelineStatusBar />;
}

function App() {
  // Subscribe to Tauri backend events
  useTauriEvents();
  // ADR-0013 step 2: feed finalized speech turns into graph-grounded streaming
  // chat when in converse/pipelined mode (no-op otherwise).
  useConverseFrontLeg();
  // Register global keyboard shortcuts (Cmd/Ctrl+R, Cmd/Ctrl+,, Esc, Cmd/Ctrl+Shift+S)
  useKeyboardShortcuts();
  // Native capture UX (epic 5c24): system tray recording indicator + OS-global
  // Cmd/Ctrl+Shift+R start/stop shortcut, both routed through the store's
  // capture actions (no parallel logic).
  useNativeCapture();

  const { t, i18n } = useTranslation();

  // SHELL-R4 (plan §R4, ADR-0046): the VALUE side of `rightPanelTab` has no
  // reader left in this file — its only renderer, `analysisContextPanel`
  // (the Analysis-only transcript/chat toggle), is deleted along with the
  // `analysis` tab. `setRightPanelTab` alone survives as a write-only
  // binding (see the `isCapturing` effect below) for the same reason
  // `setAgentOverlayOpen` does above.
  const setRightPanelTab = useAudioGraphStore((s) => s.setRightPanelTab);
  const settingsOpen = useAudioGraphStore((s) => s.settingsOpen);
  const loadedSessionId = useAudioGraphStore((s) => s.loadedSessionId);
  const isCapturing = useAudioGraphStore((s) => s.isCapturing);
  const hasAgentActivity = useAudioGraphStore(
    (s) =>
      s.agentProposals.length > 0 ||
      s.liveAssistCards.length > 0 ||
      s.agentStatus?.state === "running",
  );
  const openSettings = useAudioGraphStore((s) => s.openSettings);
  const loadSampleSessionPreview = useAudioGraphStore(
    (s) => s.loadSampleSessionPreview,
  );
  const samplePreviewActive = useAudioGraphStore((s) => s.samplePreviewActive);
  // SHELL-R3 (plan §R3, ADR-0046): `agentOverlayOpen`'s BOOLEAN and
  // `tokenOverlayOpen` (+ its setter) are intentionally NOT read here any
  // more — the pop-down overlay they drove is retired below (PopoverOverlay
  // deleted outright). `setAgentOverlayOpen` alone survives as a write-only
  // binding: `previewSampleSession` below still defensively clears it after
  // `loadSampleSessionPreview` seeds `agentOverlayOpen: true` as part of its
  // bundled sample state — `App.test.tsx` pins the post-preview value at
  // `false`, so this write stays even though nothing renders it. The read
  // side (both fields, both setters) otherwise stays wired-but-unread in the
  // store for the same reason `sessionsBrowserOpen` did after R2
  // (`store/shellNav.ts`'s module doc): `App.test.tsx`/`App.contract.
  // test.tsx` both `setState` them directly and must stay byte-identical.
  // Deleting the now-fully-inert remainder is left to R4, which already owns
  // revisiting those fixtures for the tab-id rename.
  const setAgentOverlayOpen = useAudioGraphStore((s) => s.setAgentOverlayOpen);
  const systemDrawerOpen = useAudioGraphStore((s) => s.systemDrawerOpen);
  const setSystemDrawerOpen = useAudioGraphStore((s) => s.setSystemDrawerOpen);
  const graphEdgeFocus = useAudioGraphStore((s) => s.graphEdgeFocus);
  // ShellNav (SHELL-R1, ADR-0046): nav now lives in the store (not App-local
  // useState) because R2's `stopCapture` — a store action — must be able to
  // route to it directly. `workspaceView` stays a derived local const so
  // every read site below is unchanged; see `store/shellNav.ts` — SHELL-R4
  // retired the three-view during/after/analysis mapping this derivation
  // used to preserve, in favor of the two ADR-0046 destinations directly.
  const nav = useAudioGraphStore((s) => s.nav);
  const setWorkspaceView = useAudioGraphStore((s) => s.setWorkspaceView);
  const setNavLens = useAudioGraphStore((s) => s.setNavLens);
  const workspaceView = deriveWorkspaceView(nav);

  // Assertive recording-state announcement (seed audio-graph-4f2e / WCAG
  // 4.1.3). The polite `workspace-switcher__state` region already narrates the
  // idle/live/sample/loaded label, but a start/stop transition is high-signal
  // and must be announced *assertively* — distinct from that polite region. We
  // only write on an actual `isCapturing` edge (guarded by a ref) so the
  // region stays empty on mount and doesn't re-announce on unrelated renders.
  const [recordingAnnouncement, setRecordingAnnouncement] = useState("");
  const prevCapturingRef = useRef(isCapturing);
  useEffect(() => {
    if (prevCapturingRef.current !== isCapturing) {
      prevCapturingRef.current = isCapturing;
      setRecordingAnnouncement(
        isCapturing
          ? t("app.a11y.recordingStarted")
          : t("app.a11y.recordingStopped"),
      );
    }
  }, [isCapturing, t]);

  // Destination-transition announcement (critique B7). Switching Capture /
  // Sessions is a keyboard/pointer action whose only prior signal was the
  // visual panel swap; announce the entered destination politely for SR
  // users. We skip the initial mount so it doesn't fire on first paint.
  const [phaseAnnouncement, setPhaseAnnouncement] = useState("");
  const prevWorkspaceViewRef = useRef<WorkspaceView | null>(null);
  useEffect(() => {
    if (prevWorkspaceViewRef.current !== workspaceView) {
      const isInitial = prevWorkspaceViewRef.current === null;
      prevWorkspaceViewRef.current = workspaceView;
      if (!isInitial) {
        setPhaseAnnouncement(
          t("app.a11y.phaseEntered", {
            phase:
              workspaceView === "capture" && isCapturing
                ? t("workspace.liveNow")
                : t(`workspace.${workspaceView}`),
          }),
        );
      }
    }
  }, [isCapturing, workspaceView, t]);

  // Graph-edge focus bridge (audio-graph-a2a7; retargeted SHELL-R4, plan §R4,
  // ADR-0046). Activating a Sessions seek-timeline utterance's "→N"
  // related-edges badge sets `graphEdgeFocus` (edge ids + a monotonic
  // nonce). Before R4 this forced `setWorkspaceView("analysis")` — a full
  // destination navigation, since the graph only lived on the now-deleted
  // Analysis tab. The graph now lives on the Sessions destination's own
  // Graph lens, so the bridge only needs the strictly smaller side effect of
  // pointing `nav.lens` at "graph" *within the same session* — it never
  // forces `nav.dest` away from wherever the user already is. Keyed on the
  // nonce so re-activating the same badge re-fires, but an unrelated
  // re-render (or the initial mount, where the ref starts unset) never
  // re-fires spuriously.
  //
  // KNOWN GAP (disclosed, not silently dropped): `nav.lens` currently has
  // ZERO production readers. `SessionsBrowser`'s own detail-lens selection
  // is local `useState<DetailLens>`, not wired to `nav.lens` (see its doc
  // comment) — unifying the two is real controlled-prop work (stale-lens /
  // no-active-session edge cases) that is out of this near-pure
  // deletion+rename unit's scope. Until that follow-up lands, activating
  // "→N" writes `nav.lens = "graph"` but produces NO visible effect unless
  // the user happens to already be on the Graph lens; the pre-R4 behavior
  // (jump to the Analysis tab and paint the edge emphasis) is regressed to a
  // no-op. Tracked as a follow-up seed candidate: wire SessionsBrowser's
  // lens tabs to read/write `nav.lens` so this bridge becomes observable
  // again. The `KnowledgeGraphViewer` itself still reads the same
  // `graphEdgeFocus` from the store and paints the emphasis whenever it IS
  // mounted — the gap is purely that nothing currently switches to the lens
  // that mounts it.
  const prevEdgeFocusNonceRef = useRef<number | null>(null);
  useEffect(() => {
    const nonce = graphEdgeFocus?.nonce ?? null;
    if (nonce === null) {
      prevEdgeFocusNonceRef.current = null;
      return;
    }
    if (prevEdgeFocusNonceRef.current !== nonce) {
      prevEdgeFocusNonceRef.current = nonce;
      setNavLens("graph");
    }
  }, [graphEdgeFocus?.nonce, setNavLens]);

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
  // Probe-failure fallback (seed fbf0 / review A3). When the credential-presence
  // probe *throws* (backend not ready, keychain locked, fresh-install race), we
  // must not leave a first-run user staring at empty panels + a raw error toast.
  // `probeFailed` flips the During workspace to a friendly Get-started fallback;
  // `probeRetrying` drives the Retry button's in-flight state.
  const [probeFailed, setProbeFailed] = useState(false);
  // cred-review m6: true when the probe threw a `credential_file_error` — the
  // credential store couldn't be READ (malformed credentials/state file, or a
  // keychain-unavailable load, which the backend maps to the same code), as
  // opposed to a fresh install (empty list, no throw). Drives a retry-first
  // hint instead of the first-run "get started" copy so we don't invite
  // ExpressSetup to re-prompt and overwrite existing (but unreadable) keys.
  const [probeUnreadable, setProbeUnreadable] = useState(false);
  const [probeRetrying, setProbeRetrying] = useState(false);
  const [probeCompleted, setProbeCompleted] = useState(false);
  const dismissExpressSetup = () => {
    setExpressSetupVisible(false);
    if (isHandoffEligible()) setHandoffVisible(true);
  };
  const previewSampleSession = useCallback(() => {
    loadSampleSessionPreview(i18n.resolvedLanguage ?? i18n.language);
    setWorkspaceView("sessions");
    setAgentOverlayOpen(false);
    setExpressSetupVisible(false);
    setHandoffVisible(false);
    // Clear the probe-failure fallback too — the sample flow can be launched
    // from it, and once a sample is loaded the Capture workspace should
    // never fall back to the Get-started card.
    setProbeFailed(false);
    setProbeUnreadable(false);
    saveHandoffSeen();
  }, [
    i18n.language,
    i18n.resolvedLanguage,
    loadSampleSessionPreview,
    setAgentOverlayOpen,
    setWorkspaceView,
  ]);
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
    focusWorkspaceTab(workspaceView);
  }, [workspaceView]);
  // SC 1.4.13: the hand-off hint is dismissible via Escape and returns focus
  // to the active workspace tab. It never traps focus (SC 2.1.2) and sits
  // above the layout so it doesn't obscure a focused element (SC 2.4.11).
  useEffect(() => {
    if (!handoffVisible) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") dismissHandoff();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [handoffVisible, dismissHandoff]);
  useEffect(() => {
    if (isCapturing) {
      setWorkspaceView("capture");
      // `rightPanelTab` drove the now-deleted Analysis-only transcript/chat
      // toggle (SHELL-R4 removed its last renderer, `analysisContextPanel`).
      // The write itself stays — same precedent as `sessionsBrowserOpen`/
      // `agentOverlayOpen` elsewhere in this file: it's still a real store
      // field several fixtures assert on directly, and resetting it here on
      // every capture-start remains harmless even with zero remaining UI
      // consumers. Purging the now-fully-inert field is a follow-up, not
      // this near-pure deletion+rename unit's job.
      setRightPanelTab("transcript");
      return;
    }
    if (samplePreviewActive || loadedSessionId) {
      setWorkspaceView("sessions");
    }
  }, [
    isCapturing,
    loadedSessionId,
    samplePreviewActive,
    setRightPanelTab,
    setWorkspaceView,
  ]);
  const prevSamplePreviewActiveRef = useRef(samplePreviewActive);
  useEffect(() => {
    const becameActive =
      samplePreviewActive && !prevSamplePreviewActiveRef.current;
    prevSamplePreviewActiveRef.current = samplePreviewActive;
    if (becameActive) focusWorkspaceTab("sessions");
  }, [samplePreviewActive]);
  // Credential-presence probe. Extracted so both the mount effect and the
  // fallback's Retry button share one code path. On success it clears any prior
  // failure and pops Express Setup when the saved keys can't cover a runnable
  // durable cloud pipeline. On throw it raises the Get-started fallback (seed
  // fbf0) instead of leaving the During workspace empty behind a raw toast.
  const runCredentialProbe = useCallback(async () => {
    setProbeCompleted(false);
    try {
      const presence = await invoke<CredentialPresence[]>(
        "load_credential_presence_cmd",
      );
      // SHELL-R3 (plan §R3, ADR-0046): persisted (not just a local variable)
      // so NowStrip's planned-route chip can call the SAME
      // `hasConfiguredDurableNotesRoute` read this probe performs, rather
      // than a route-chip-only re-derivation. `awsProfiles` below stays a
      // local variable, NOT mirrored to the store (aws_bedrock + a profile
      // credential source is the one narrow case the chip cannot fully
      // verify yet — documented limitation, not a silent gap).
      useAudioGraphStore.setState({ credentialPresence: presence });
      const settings = await invoke<AppSettings>("load_settings_cmd");
      // Hydrate the shared control/store view from the same passive settings
      // read so action gates never operate on a stale/null provider route.
      useAudioGraphStore.setState({ settings });
      const needsLocalModelStatus =
        settings?.llm_provider?.type === "local_llama";
      const modelStatus = needsLocalModelStatus
        ? await invoke<ModelStatus>("get_model_status")
        : null;
      if (modelStatus) useAudioGraphStore.setState({ modelStatus });
      const needsAwsProfiles =
        settings.llm_provider.type === "aws_bedrock" &&
        settings.llm_provider.credential_source.type === "profile";
      // Profile enumeration is local/passive: it validates configured state
      // without resolving the AWS chain or contacting a provider.
      const awsProfiles = needsAwsProfiles
        ? await invoke<string[]>("list_aws_profiles")
        : [];
      setProbeFailed(false);
      setProbeUnreadable(false);
      // App startup is a passive configuration check. Active provider probes
      // belong to an acknowledged session/draft audit scope (ADR-0028).
      setExpressSetupVisible(
        !hasConfiguredDurableNotesRoute(
          settings,
          presence,
          modelStatus,
          awsProfiles,
        ),
      );
    } catch (e) {
      // Probe threw. cred-review m6: a fresh install returns an empty list (no
      // throw), so a throw is either the backend not being ready yet
      // (fresh-install race, keychain locked) OR the user's saved credentials
      // being UNREADABLE (a malformed credentials-state.yaml makes
      // load_with_source return Err). The two must not look identical: telling
      // a user with saved-but-corrupt keys to "get started" invites
      // ExpressSetup to re-prompt and overwrite. But the unreadable copy is
      // reserved for a RECOGNIZED parse failure — the backend wraps keychain
      // and I/O failures under the same code, and those recover via retry, not
      // file review (see isCredentialFileParseError).
      setProbeUnreadable(isCredentialFileParseError(e));
      setProbeFailed(true);
    } finally {
      setProbeCompleted(true);
    }
  }, []);
  // Retry handler for the fallback: re-run the probe with an in-flight flag so
  // the button can show a "Retrying…" busy state; a successful retry clears the
  // fallback (setProbeFailed(false) inside runCredentialProbe).
  const retryCredentialProbe = useCallback(async () => {
    setProbeRetrying(true);
    try {
      await runCredentialProbe();
    } finally {
      setProbeRetrying(false);
    }
  }, [runCredentialProbe]);
  useEffect(() => {
    void runCredentialProbe();
  }, [runCredentialProbe]);

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

  // Resizable layout size (px), persisted across sessions. SHELL-R4 deletes
  // the Analysis-only right-rail/notes-height resize state (`rightWidth`/
  // `resizeRight`, `notesHeight`/`resizeNotes`) along with the tab that was
  // their only consumer — `ag.rightWidth`/`ag.notesHeight` are not read
  // anywhere else in the codebase (verified this run), so a previously
  // saved value simply becomes an orphaned, harmless localStorage key.
  const [leftWidth, setLeftWidth] = useState(() =>
    loadNum("ag.leftWidth", 260),
  );
  const resizeLeft = (dx: number) =>
    setLeftWidth((w) => {
      const n = clamp(w + dx, 200, 520);
      saveNum("ag.leftWidth", n);
      return n;
    });

  // SHELL-R7 (plan §R7, ADR-0046): the shell layout tier drives whether the
  // rail (`AudioSourceSelector`) and aside (`SpeakerPanel`) regions are
  // pinned inline in `ShellRailContentAside` or collapsed behind one of the
  // two drawers rendered below — see `useShellLayout.ts`'s doc comment for
  // the exact wide/standard/compact boundaries.
  const shellTier = useShellLayout();
  const railPinned = shellTier !== "compact";
  const asidePinned = shellTier === "wide";
  const [sourcesDrawerOpen, setSourcesDrawerOpen] = useState(false);
  const [speakersDrawerOpen, setSpeakersDrawerOpen] = useState(false);
  // At most one drawer open at a time — both are full-height, full-scrim
  // overlays anchored to opposite edges, and stacking two of them has no
  // sensible visual or focus-trap semantics.
  const openSourcesDrawer = useCallback(() => {
    setSpeakersDrawerOpen(false);
    setSourcesDrawerOpen(true);
  }, []);
  const openSpeakersDrawer = useCallback(() => {
    setSourcesDrawerOpen(false);
    setSpeakersDrawerOpen(true);
  }, []);
  const closeSourcesDrawer = useCallback(() => setSourcesDrawerOpen(false), []);
  const closeSpeakersDrawer = useCallback(
    () => setSpeakersDrawerOpen(false),
    [],
  );
  // A resize back up to a tier where a region is pinned again must not leave
  // its drawer dangling open behind newly-visible inline content. When the
  // drawer WAS open, its trigger unmounts in this same commit (the trigger
  // strip is `!railPinned`/`!asidePinned`-gated), so `useFocusTrap`'s own
  // "restore focus to whatever opened me" cleanup finds nothing left to
  // restore to and silently drops focus to `<body>`. Move focus to the
  // now-visible `.left-panel` ourselves first so a keyboard user doesn't
  // lose their place — this is a resize-triggered close, not the
  // Escape/click close path (which the trigger IS still mounted for).
  useEffect(() => {
    if (railPinned && sourcesDrawerOpen) {
      document.querySelector<HTMLElement>(".left-panel")?.focus();
    }
    if (railPinned) setSourcesDrawerOpen(false);
  }, [railPinned, sourcesDrawerOpen]);
  useEffect(() => {
    if (asidePinned && speakersDrawerOpen) {
      document.querySelector<HTMLElement>(".left-panel")?.focus();
    }
    if (asidePinned) setSpeakersDrawerOpen(false);
  }, [asidePinned, speakersDrawerOpen]);

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

  // APG-style roving tabindex for the top-level Capture/Sessions destination
  // tabs. Already length-generic (SHELL-R4, plan §R4): it derives every
  // index off `WORKSPACE_VIEWS.length`, so collapsing from three legacy
  // views to the two ADR-0046 destinations needed no changes to this
  // handler's own logic, only to the array it derives from.
  const handleWorkspaceViewKeyDown = (
    e: React.KeyboardEvent<HTMLButtonElement>,
  ) => {
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
    const currentIndex = WORKSPACE_VIEWS.indexOf(workspaceView);
    const nextIndex =
      e.key === "Home"
        ? 0
        : e.key === "End"
          ? WORKSPACE_VIEWS.length - 1
          : e.key === "ArrowLeft" || e.key === "ArrowUp"
            ? (currentIndex - 1 + WORKSPACE_VIEWS.length) %
              WORKSPACE_VIEWS.length
            : (currentIndex + 1) % WORKSPACE_VIEWS.length;
    const next = WORKSPACE_VIEWS[nextIndex];
    setWorkspaceView(next);
    const tablist = e.currentTarget.parentElement;
    const tabs = tablist?.querySelectorAll<HTMLButtonElement>('[role="tab"]');
    tabs?.[nextIndex]?.focus();
  };

  // Show the Get-started fallback only on a genuinely idle first-run Capture
  // surface: the probe threw AND there's no capture, sample preview, or loaded
  // session that would otherwise fill the panels with real content. This keeps
  // the fallback to the exact "empty cockpit" case it exists to prevent.
  const showGetStartedFallback =
    probeFailed && !isCapturing && !samplePreviewActive && !loadedSessionId;
  // SHELL-R5 (plan §R5, ADR-0046): the preflight card replaces the same
  // "empty cockpit" surface for the non-error idle case — same genuinely-
  // idle predicate as the fallback above (probe SUCCEEDED this time), minus
  // one more exclusion: `hasAgentActivity`. Agent proposals/live-assist cards
  // can outlive a `stopCapture()` that didn't have a session id to route to
  // (see that action's doc comment in `store/index.ts`), so there IS real
  // content to review even though `isCapturing` is already false — the
  // preflight card must not paper over that with an empty checklist.
  const showPreflightCard =
    !showGetStartedFallback &&
    !isCapturing &&
    !samplePreviewActive &&
    !loadedSessionId &&
    !hasAgentActivity;

  return (
    <div
      className="app-container"
      data-onboarding-probe={probeCompleted ? "settled" : "pending"}
    >
      <ShellChrome
        workspaceView={workspaceView}
        recordingAnnouncement={recordingAnnouncement}
        phaseAnnouncement={phaseAnnouncement}
        handoffVisible={handoffVisible}
        dismissHandoff={dismissHandoff}
      />
      <ShellDestinationBar
        workspaceView={workspaceView}
        isCapturing={isCapturing}
        onSelectView={setWorkspaceView}
        onTabKeyDown={handleWorkspaceViewKeyDown}
      />
      {/* SessionViewProvider (SHELL-R1): the seam future per-session store
          isolation (store/index.ts:2100, out of scope this unit) lands
          behind, without reopening NotesPanel/LiveTranscript/
          KnowledgeGraphViewer/SeekTimeline. Today it forwards the identical
          global-store values those panels already read. */}
      <SessionViewProvider>
        <ShellRailContentAside
          workspaceView={workspaceView}
          leftWidth={leftWidth}
          resizeLeft={resizeLeft}
          showGetStartedFallback={showGetStartedFallback}
          showPreflightCard={showPreflightCard}
          previewSampleSession={previewSampleSession}
          retryCredentialProbe={retryCredentialProbe}
          openSettings={openSettings}
          probeRetrying={probeRetrying}
          probeUnreadable={probeUnreadable}
          hasAgentActivity={hasAgentActivity}
          railPinned={railPinned}
          asidePinned={asidePinned}
          onOpenSourcesDrawer={openSourcesDrawer}
          onOpenSpeakersDrawer={openSpeakersDrawer}
        />
      </SessionViewProvider>
      <ShellFooter />

      {/* Settings modal */}
      {settingsOpen && (
        <Suspense fallback={null}>
          <SettingsPage />
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

      {/* System drawer (SHELL-R3, ADR-0046): projection runtime + token usage
          + per-stage pipeline detail, opened from NowStrip's composite health
          chip. Replaces the retired `PopoverOverlay`'s two consumers — see
          `SystemDrawer.tsx`'s doc comment for the full retirement
          disposition (agent proposals already had an inline home; token
          usage moves here). */}
      {systemDrawerOpen && (
        <SystemDrawer onClose={() => setSystemDrawerOpen(false)} />
      )}

      {/* Rail/aside drawers (SHELL-R7, plan §R7, ADR-0046): the same
          `AudioSourceSelector`/`SpeakerPanel` components `ShellRailContentAside`
          renders pinned at wider tiers, hosted in `ShellDrawer`'s shared
          focus-trap/Escape/scrim chrome once `useShellLayout()`'s tier moves
          either region out of the pinned flow. Mutually exclusive with each
          other (see `openSourcesDrawer`/`openSpeakersDrawer` above), and with
          `SystemDrawer` only by convention (nothing here forces it). */}
      {sourcesDrawerOpen && (
        <ShellDrawer
          side="start"
          label={t("shellLayout.sources")}
          closeLabel={t("shellLayout.sourcesClose")}
          onClose={closeSourcesDrawer}
        >
          <AudioSourceSelector />
        </ShellDrawer>
      )}
      {speakersDrawerOpen && (
        <ShellDrawer
          side="end"
          label={t("shellLayout.speakers")}
          closeLabel={t("shellLayout.speakersClose")}
          onClose={closeSpeakersDrawer}
        >
          <SpeakerPanel />
        </ShellDrawer>
      )}

      {/* Unified notification host (ADR-0011): transient queue + legacy
          error string, stacked above modals with severity aria-live. */}
      <Notifications />
    </div>
  );
}

export default App;
