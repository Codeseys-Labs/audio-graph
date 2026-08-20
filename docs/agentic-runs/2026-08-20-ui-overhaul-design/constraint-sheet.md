# Constraint sheet: UI overhaul / rebuild decision

Maintainer framing (verbatim): "the ui might need an overhaul cause it does
not look nice... a nice ui can help with ux and adoption... the ui and Ux
drive how this is to be used not only on desktop but on mobile and
wearables."

This is a read-only inventory for a conductor agent deciding overhaul scope.
No code was changed. Repo: `/home/codeseys/DevBox/audio-graph` at commit
`a4265de` (branch `master`).

---

## 1. Stack

**Frontend:** React `^19.2.6` / `react-dom ^19.2.6`, TypeScript `^5.9.3`,
Vite `^6.4.2` (`vite.config.ts`), package manager `bun@1.3.14`
(`package.json`). No router — the app is a single-window Tauri shell with
in-app view state, not URL routes.

**Styling:** Hand-authored CSS custom-property design-token system, NOT a
component library. `src/styles.css` (618 lines) is the single source of
truth for tokens (ADR-0009): primitive scales (space/radius/type/shadow/
z-index/motion) plus semantic tokens (surfaces, text, accents, status tints,
graph-canvas colors, interaction overlays, scrim, focus ring), duplicated for
a light theme under `@media (prefers-color-scheme: light)` and
`[data-theme="light"]` (ADR-0009 Wave 4). Tailwind v4 (`tailwindcss ^4.3.0`,
`@tailwindcss/vite ^4.3.0`) is imported **without Preflight** — only the
`theme` and `utilities` layers — specifically so `@theme inline` can map the
CSS custom properties into Tailwind namespaces (ADR-0016); the app's own
reset/`:focus-visible`/reduced-motion rules stay authoritative. Component
JSX overwhelmingly uses Tailwind utility classes reading through
`var(--token)` (e.g. `className="... bg-accent-blue text-(--on-accent-blue)"`)
rather than separate `.module.css` files. Additional plain CSS files:
`src/styles/layout.css` (main shell grid + the app's only two responsive
breakpoints), `src/styles/settings.css` (37 KB — the Settings modal, largest
CSS file in the repo, has its own `@media (max-width: 720px)`), `primitives.css`,
`keyframes.css`, `express-setup.css`, `shortcuts-modal.css`.

**Theme:** Dark-first (`color-scheme: dark` on `:root`), with a fully audited
light theme (WCAG-contrast-audited, see `docs/reviews/wcag-contrast-audit.md`
referenced in comments) and a `system`/`light`/`dark` user choice
(`src/theme.ts`, persisted to `localStorage` under `ag.theme`). No further
theme customization (no brand-color picker, no per-user accent).

**Build tooling:** Vite + `@vitejs/plugin-react`; `tsc && vite build` for
production; Biome `^2.5.1` for lint/format (`biome.json`), not ESLint/
Prettier. Bundle is manually chunked (`vite.config.ts:43-71`): `react-vendor`,
`i18n-vendor`, one general `vendor` chunk (force-graph/d3, Radix, Tauri API,
lucide, zustand). Heavy/conditional UI is `React.lazy`-split in `App.tsx`:
`KnowledgeGraphViewer`, `SettingsPage`, `SessionsBrowser`, `ExpressSetup`.

**State:** Single global Zustand store (`zustand ^5.0.14`), one file
`src/store/index.ts` (104,740 bytes / ~2,860+ lines — no slice files, despite
a documented "slice layout" in the header comment). Slices per that header
comment: audio sources, capture lifecycle, transcribe pipeline, Gemini Live,
knowledge graph, speakers, pipeline status, chat, settings/UI (modal flags +
`rightPanelTab`), error/toast wiring. The store both holds UI state
(`settingsOpen`, `sessionsBrowserOpen`, `rightPanelTab`, `agentOverlayOpen`,
`tokenOverlayOpen`, `graphEdgeFocus`) and is the invoke-bridge to Rust
(`safeInvoke`-wrapped `invoke<T>(command, args)` for every backend call, with
events flowing the other way via `useTauriEvents`). Historical Review and
Live capture **share one frontend view store** — a documented constraint
(`src/store/index.ts:2100`, cited by the 1d92 prototype review) that a
second, isolated per-session view store does not yet exist.

**i18n:** `i18next ^26.3.0` / `react-i18next ^17.0.8` /
`i18next-browser-languagedetector ^8.2.1`. Two locales: `src/i18n/locales/
en.json`, `pt.json`. `src/i18n/locale-parity.test.ts` is an automated gate —
it flattens both JSON trees to dotted key paths and fails the suite if `pt`
is missing any `en` key or has an extra one (shape parity, not translation
quality). Any new UI copy must ship both locale entries or CI fails.

**Component inventory** (`src/components/*.tsx`, one line each; `settings/`
subfolder listed separately):

| File | Role |
|---|---|
| AdvancedSettingsDisclosure | Collapsible `<details>`-based "Advanced settings" wrapper |
| AgentProposalsPanel | Live-assist agent proposal cards (graph suggestion / question / note) |
| AsrProviderSettings | ASR provider choice sub-form (Settings) |
| AudioSettings | Sample-rate + channel-count capture settings |
| AudioSourceSelector | Left-column grouped audio-source picker (largest component file, 43 KB) |
| Button | Design-system button (ADR-0009/0011) |
| ChatSidebar | Free-form chat grounded in the current knowledge graph |
| ControlBar | Top toolbar — the primary capture-control surface (single-row `flex`, no `flex-wrap`/media query) |
| ConversationModeControl | ADR-0013 conversation-mode switch |
| CredentialsManager | Local-models readiness sub-panel |
| DemoModeBanner | First-launch local-only-mode hint banner |
| ExpressSetup | First-launch quickstart wizard |
| FieldRow | Settings field/label/control layout primitive |
| GeminiSettings | Gemini Live auth + model sub-form |
| GetStartedFallback | Empty-state fallback shown in During when the credential probe throws |
| HumanizedError | Plain-language error renderer (ADR-0011) |
| Icon | SVG icon registry (ADR-0010) |
| IconButton | Icon-only button enforcing `aria-label` (ADR-0010) |
| KnowledgeGraphViewer | Center-panel force-directed 2D graph (`react-force-graph-2d`), lazy-loaded, 28 KB |
| LiveTranscript | Scrolling live/historical transcript list, tagged by speaker |
| LlmProviderSettings | LLM provider choice sub-form (largest settings sub-form, 42 KB) |
| LoggingSettings | File-logging controls |
| ModelCatalogField / ModelCatalogPicker | Shared "choose a model" catalog control |
| NotesPanel | Structured running-summary notes panel (primary During/Review surface) |
| Notifications | Unified toast/error host (ADR-0011) |
| OpenRouterAcceleratorDiscovery | OpenRouter accelerator discovery surface |
| PipelineStatusBar | Bottom bar, one status dot per pipeline stage |
| PopoverOverlay | Accessible pop-down overlay (agent proposals, token usage) |
| ProjectionRuntimeStatusPanel | **Just landed** (Aug 20) — projection scheduler/replay runtime status in the right rail |
| ProviderReadinessPanel | Full selected-configuration fidelity view |
| ResizeDivider | Pointer-event draggable panel-resize divider |
| SecretCredentialControl | Masked secret-credential input control |
| SeekTimeline | After-mode session seek-timeline (ADR-0026 §4.1) |
| SessionDataRoutePanel | Session data-route / privacy ledger report |
| SessionsBrowser | Modal: browse/restore/delete historical sessions |
| SettingsPage | Full configuration modal (thin — actual content lives in `settings/`) |
| ShortcutsHelpModal | Keyboard-shortcut reference modal |
| SpeakerPanel | Detected-speaker list with per-speaker controls |
| StorageBanner | Storage-full (ENOSPC) recovery banner |
| TokenUsagePanel | Gemini Live token-usage totals |
| Tooltip | Hover+focus+touch tooltip (ADR-0016) |

`src/components/settings/` (Settings-modal internals, "blueprint" refactor —
see §4 prior art): `Badge`, `CredentialsPanel`, `GeminiPanel`, `GeneralPanel`
(theme/language prefs), `LlmPanel`, `LoggingPanel`, `ModelActionButtons`,
`OverviewPanel` (orientation homepage), `ProductModeSummaryCards`,
`ProviderCapabilityCard`/`ProviderCapabilityStageSection`,
`ReadinessModelActions`, `SettingsContext`, `SttPanel`, `TtsPanel`,
`settingsRail`/`settingsRailConfig` (vertical left-rail tablist),
`useSettingsController`.

---

## 2. Information architecture, as implemented today

`App.tsx` (1,012 lines) is a single root component, no router. Top-level
layout, top to bottom: `StorageBanner` → `DemoModeBanner` → `ControlBar` →
optional onboarding hand-off banner → a **3-tab workspace switcher**
(`role="tablist"`) → the main resizable 3-pane layout → `PipelineStatusBar`
→ overlay modals/toasts.

**The 3 workspace tabs are internally still `during` / `after` / `analysis`**
(`WORKSPACE_VIEWS` const, `App.tsx:216`), but their **i18n labels have
already been relabeled** to the ADR-0030 vocabulary: `workspace.during` =
"Ready" / `workspace.liveNow` = "Live now" (shown instead of "Ready" while
`isCapturing`) / `workspace.after` = "Review" / `workspace.analysis` =
"Inspect" (`src/i18n/locales/en.json:65-80`). This is a **label-only**
migration — see §4, ADR-0030 is accepted but its structural rewrite (a true
4th "Live" destination, Inspect nested under Review rather than a sibling
tab) has not landed; open seed `audio-graph-19c7` owns that work.

- **during ("Ready"/"Live now")**: two-pane During workspace —
  `NotesPanel` (primary) + `LiveTranscript` (secondary), plus
  `AgentProposalsPanel` inline when there's live agent activity. Falls back
  to `GetStartedFallback` if the first-run credential-presence probe threw
  and nothing else (capture/sample/loaded session) fills the panels.
- **after ("Review")**: `NotesPanel` + `LiveTranscript` again, plus
  `SeekTimeline` when a sample preview or loaded session is active. This
  **is** the Review surface for a historical session; there is no separate
  "Review" route — it's the same `after` tab, populated by
  `SessionsBrowser`'s Load action (`loadedSessionId`) or a sample preview.
- **analysis ("Inspect")**: `KnowledgeGraphViewer` (hero) + `NotesPanel`
  (bottom strip) in a 2-row grid, plus a right-hand context panel
  (`LiveTranscript`/`ChatSidebar` tabbed, `ProjectionRuntimeStatusPanel`
  always, `SessionDataRoutePanel` when a session is loaded). Only this tab
  shows the graph and the projection/data-route diagnostics.

**Navigation model:** `role="tablist"` / `role="tab"` with APG roving
tabindex (arrow keys + wraparound; the driver used in E2E does not deliver
literal Home/End, only arrows) — `handleWorkspaceViewKeyDown` /
`handleTabKeyDown`. Left/right asides (`AudioSourceSelector` + `SpeakerPanel`
on the left; the transcript/chat tab pair on the right, only in `analysis`)
are resizable via `ResizeDivider`, sizes persisted to `localStorage`
(`ag.leftWidth`/`ag.rightWidth`/`ag.notesHeight`, clamped).

**SessionsBrowser** is a separate lazy-loaded modal (not a tab): search,
sort, load (sets `loadedSessionId` → flips to `after`), export, delete.
Per the 1d92 prototype review, `store.loadSession` independently refuses
while `isCapturing || isTranscribing`, and Load is the *only* path that ever
sets `loadedSessionId` — stopping a capture does not.

**ProjectionRuntimeStatusPanel** (landed same day as this task) sits in the
`analysis` tab's right rail, always mounted (unless in sample-preview mode):
projection scheduler telemetry, replay report, patch counts — diagnostic-tier
surface, not primary-journey.

**Core user journey today:** select source (`AudioSourceSelector`, left
aside, always mounted) → Start (`ControlBar`) → live in `during`, notes
primary / transcript secondary → Stop → auto-transitions to `after` if a
sample/loaded session is active, otherwise stays on `during` (now idle) →
user can browse graph/diagnostics only by manually switching to `analysis` →
`SessionsBrowser` modal is the only path back into a past session.

---

## 3. UI contracts that must survive any overhaul

**WDIO E2E (`e2e/specs/shell.e2e.ts`, 447 lines, runs against a real compiled
Tauri binary via `@wdio/tauri-service`, CI-only, `wdio-e2e` Cargo feature):**

1. Window **title exactly `"AudioGraph"`** (`toHaveTitle`).
2. `#workspace-tab-during` exists and is displayed on first paint.
3. `load_settings_cmd` round-trips through the real IPC bridge and the
   response must have `asr_provider`/`llm_provider` keys and contain **no
   secret-shaped strings** (see pattern list below) — proves
   `redacted_settings()` on the backend actually redacted before the
   renderer ever saw it.
4. **Tab IDs are pinned**: `#workspace-tab-during`, `#workspace-tab-after`,
   `#workspace-tab-analysis`, `#workspace-panel-during/after/analysis`.
   Clicking each must flip `aria-selected="true"` and display the matching
   panel. Arrow-key roving tabindex must move `aria-selected` and focus
   (tested via wraparound since Home/End don't reach this driver).
5. Mocked capture flow: `list_audio_sources` → a `[role="checkbox"]` row
   containing the mock source name must appear after clicking
   `button[aria-label="Refresh sources"]`; clicking that row then
   `button[aria-label="Start"]` must flip `.workspace-switcher__state` to
   contain "Live session"; `button[aria-label="Stop"]` must clear it; a
   second, rejected `start_capture` must surface
   `.notifications .notification--error` (not hang). **These exact
   `aria-label` strings and the `.workspace-switcher__state` text contract
   are load-bearing selectors.**
6. **Zero ERROR-level frontend log lines** captured during the run
   (`hasErrorLevelToken` scans the first 60 chars of each captured line for
   `\bERROR\b`), and **no secret-shaped string** in any captured log line —
   patterns for OpenAI/OpenRouter/Anthropic (`sk-...`), Google (`AIza...`),
   AWS (`AKIA`/`ASIA...`), Deepgram (`dg[_-]...`), Tavily (`tvly-...`),
   Slack (`xox[baprs]-...`). This mirrors `scripts/check-docs-secret-hygiene.mjs`'s
   `keyRules` by hand.
7. Final health check: the embedded session/app process must still be alive
   (`load_settings_cmd` succeeds again; the OS process is still present).

**i18n locale-parity test** (`src/i18n/locale-parity.test.ts`, Vitest): every
dotted leaf key in `en.json` must exist in `pt.json` and vice versa. Any
overhaul that adds/removes/renames copy must touch both files or the suite
fails — a hard gate, not a lint warning.

**Accessibility patterns already in place** (per the 2026-07-05 UX review
and `App.tsx` itself): complete design-token system with dark+light contrast
audited to WCAG AA (`docs/reviews/wcag-contrast-audit.md`); global
`:focus-visible` ring; `.sr-only` utility; `prefers-reduced-motion: reduce`
neutralizes all animation globally (`styles.css:507`); roving-tabindex
tablists (workspace switcher, right-panel tabs); a skip-to-main link keyed
to the active workspace panel id; two distinct live regions — an assertive
one for recording start/stop and a polite one for phase-transition
announcements; focus traps used by modal overlays (`SessionsBrowser`,
`PopoverOverlay`); `IconButton` enforces `aria-label` (ADR-0010, `Icon.test.tsx`
presumably checks this). `styles.a11y.test.ts` exists at the repo root as a
dedicated a11y regression test for styles.

**Redaction rules (no transcript content in logs):** `src/analytics/
safeInvoke.ts` wraps every Tauri `invoke` call and, on failure, forwards only
a structured diagnostic (category `frontend`, surface `invoke`, component =
command name **only**) to `report_frontend_diagnostic` via
`captureFrontendError` — "the caught error is never forwarded, so its
message/stack never leave the renderer" (doc comment, verbatim intent).
Backend data-movement events (`src/generated/sessionDataMovement.ts`, a
generated JSON-schema literal) enforce the same discipline structurally:
`error_message_redacted` is documented "never a raw provider payload or
stack trace with data"; `source_label` is "documented as redaction-safe";
artifact paths are stored only as a 64-bit non-cryptographic path hash,
never the raw path. This is an ADR-0034-governed constraint (exhaustive
evidence required for any negative data-egress claim rendered to a user) —
any new UI surface that renders a "nothing left this device" style claim
inherits that evidentiary bar, not just a copy-writing one.

---

## 4. Prior art in-repo

**`.worktrees/1d92-prototype`** (branch `prototype/audio-graph-1d92`, tip
`95af7e5`) — a Review-workspace prototype of **Finalizing / Finalization
Blocked** states (ADR-0035/0036), not a general UI-overhaul prototype. Five
design questions were run through two low-fidelity variants each and
adversarially reviewed
(`docs/agentic-runs/2026-08-20-audio-graph-1d92/prototype-findings.md`, in
that worktree). Settled: banner-style non-dismissable blocked presentation
(not a modal/dialog); list **and** detail surfacing together (detail alone
misses the just-stopped session because `loadedSessionId` is set only by
`SessionsBrowser`'s Load, never by Stop); explicit class-typed retry buttons
over silent self-healing; informational (visible, non-gating) graph-lane
coverage. **Open**, and directly relevant to any rebuild: background
finalization access while another session is Live doesn't actually work
today — `perSessionLoadGate` only edits a button's `disabled` predicate while
`store.loadSession` independently refuses whenever
`isCapturing || isTranscribing`, because **Historical Review and Live share
one frontend view store** (`src/store/index.ts:2100`) — loading a session
overwrites `transcriptSegments`, `graphSnapshot`, `sessionTimeline`,
`materializedNotes`, etc. Any rebuild that wants concurrent Live+Review
needs per-session view isolation in the store, not a predicate change.

**`docs/designs/`** — UX-relevant docs:
- `2026-07-04-during-after-shell.md` — the original During/After/Analysis
  shell design (seed d633) that shipped and is still what's running today.
- `2026-07-05-tauri-ui-ux-review.md` (301 lines) — a 7-agent adversarially-
  critiqued review (live screenshots at 2 widths × 3 stages + code audit +
  Tauri-v2 capability research + competitor/pattern research). Confirms
  d633's During is notes-first and calm; graph is confined to Analysis;
  narrow reflow is sound (zero horizontal overflow measured at every
  stage×width); modals are clean; token/a11y baseline is mature; contrast
  passes AA everywhere measured. Flags, still apparently unresolved as of
  this read (`ControlBar.tsx` has no `flex-wrap`/media query today): the
  **ControlBar never got its responsive treatment** — below ~1120px the
  Start button gets painted over by other controls; raw JS `TypeError`
  strings leak to users on two surfaces via `Notifications.tsx`'s
  no-auto-dismiss legacy `error` path; first-run probe failures silently
  swallow and land the user on empty panels. Also flags an undefined-token
  bug (`bg-(--tint-accent-danger)` doesn't exist, only `--tint-danger`), a
  `--border` ghost-token fallback (`SessionsBrowser.tsx`, always resolves to
  `#333`, ignores light theme), hardcoded z-index literals bypassing the
  `--z-*` ladder, and hardcoded English strings in `ExpressSetup`'s
  first-run flow (breaks `pt` parity).
- `2026-07-09-mvp-product-and-experience.md` — the product/experience design
  underlying ADR-0030 (Ready/LiveNow/Review/Inspect), "studio tape meets
  temporal memory" visual direction.
- `_settings-refactor-2026-06-29/` — the Settings-modal "blueprint" refactor
  research (`settings-refactor-blueprint.md` + desktop-apps/modal-a11y/
  web-saas research notes) that produced the current `src/components/settings/`
  rail architecture (`OverviewPanel`, per-provider panels, `settingsRail`).

**ADR-0026** ("Session timeline: who said what when") establishes the
speaker/timing provenance model that `SeekTimeline` renders — the "span-
anchored provenance is the ADR-0026 UX hook" referenced by open seed
`audio-graph-6829`.

**ADR-0030** ("Organize the MVP Shell Around Ready, LiveNow, Review, and
Inspect", accepted 2026-07-09) is the single most important prior decision
for this task. It explicitly diagnoses the current shell's problem in
product terms ("exposes implementation stages as peer destinations, opens
into an empty live cockpit, gives graph diagnostics visual weight comparable
to notes") and prescribes: **Ready** (source/permission/provider/storage
preflight, one Start action, no egress) → **LiveNow** (automatic notes
primary, transcript/provenance secondary, persistent Stop+health+route) →
**Review** (browse/search/export/delete/inspect a historical session without
touching the live lifecycle) → **Inspect** (a *contextual* Review workspace
or Live drawer for graph/projection/diagnostics — "not a peer primary
product mode", i.e. it should NOT be a 4th top-level tab). It names a visual
direction — "studio tape meets temporal memory: a calm recording deck,
high-legibility type, restrained semantic color, and a temporal spine" — and
mandates validation at 1440/1024/768px, light+dark, idle/loading/empty/
degraded/recovery/error, keyboard-only, 200% zoom, forced-colors,
reduced-motion, NVDA, VoiceOver, three-OS packaged smoke. **As implemented,
only the tab labels moved (§2); Inspect is still a 4th sibling tab, not
nested under Review; LiveNow is still the same `during` view, not a
separate destination.** The open, unstarted feature seed that owns finishing
this is `audio-graph-19c7` ("Rewrite the app shell as Ready, Live, Review,
and contextual Inspect"), child of epic `audio-graph-99eb` and "the next
composition wave for `audio-graph-5c24`" — its acceptance criteria already
specify "narrow layouts use contextual drawers rather than one long
diagnostic stack" (the only concrete responsive/narrow-layout ambition found
anywhere in the backlog) and "manual whole-session synthesis moves to
overflow as Generate prose summary."

**Open UI-relevant seeds** (`.seeds/issues.jsonl`):
- `audio-graph-19c7` (open, priority 1, feature) — the ADR-0030 shell rewrite,
  described above. **This is the seed a UI overhaul decision should treat as
  the closest thing to an already-scoped mandate.**
- `audio-graph-5c24` (open, priority 2, epic) — "UI/UX polish wave: calm
  During, honest errors, first-run, a11y, native-Tauri capture UX" — the
  parent epic for the 2026-07-05 UX review's findings; 19c7 is its next
  composition wave.
- `audio-graph-99eb` (in_progress, priority 1, epic) — "MVP hardening:
  end-to-end capture, durable memory, and coherent desktop UX" — the overall
  umbrella; explicitly names the target IA as "Ready/LiveNow/Review/Inspect
  UX" and lists 5c24 as the owning UI/UX seed.
- `audio-graph-75a1` (open) — "Time-to-first-note onboarding and sample
  session UX" — owns the probe-failure first-run recovery gap the UX review
  flagged (extended by A3/A4 in that review).
- `audio-graph-6829` (open, priority 2) — "Bring the frontend projection fold
  and TS types up to the evidence contract" — frontend-only; surfacing
  admitted evidence in Review is explicitly named "the ADR-0026 UX hook."
- `audio-graph-e7e5` (open, priority 4) — three diverging copies of an
  hours-aware duration formatter (`ProjectionRuntimeStatusPanel.tsx`,
  `SessionsBrowser.tsx`, `SpeakerPanel`/`format.ts`) producing inconsistent
  output ("60m 0s" vs "1h 0m" for the same 3600s) — a small but real
  visual-consistency bug an overhaul should fold in rather than re-introduce.
- `audio-graph-8055` (open, priority 3, task) — "Architecture session: mobile
  and in-person capture companion" — explicitly the seed that would own the
  maintainer's "mobile" ambition. Still at the architecture-session stage:
  scope is "recommend build/buy/defer," explicitly stages mobile *after*
  core desktop usability, and requires explicit privacy/consent design before
  any implementation. **No wearable-specific seed exists anywhere in the
  backlog.**
- `audio-graph-b153` (open, priority 2, epic) — "Competitive product roadmap:
  overtake Granola and Cluely" — later-roadmap umbrella, explicitly gated to
  stay P2/later until "the P1 usable cross-platform cutline is green";
  Cluely's competitive positioning is noted as including "desktop/mobile
  availability."

---

## 5. Constraints

**Tauri webview / bundle:** Tauri v2 (`@tauri-apps/api ^2.11.0`,
`@tauri-apps/cli ^2.11.2`). Fixed window: 1400×900, resizable, not
fullscreen-locked (`src-tauri/tauri.conf.json:14-20`). CSP is locked down:
`default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline';
img-src 'self' data:; connect-src ipc: http://ipc.localhost` — no remote
script/style/connect origins, so any overhaul that wants to pull a CDN font,
icon set, or remote asset must vendor it instead. Vite manual-chunking splits
`react-vendor`/`i18n-vendor`/`vendor` to keep the app-code entry chunk under
Rollup's 500 KB warning threshold (`vite.config.ts:29-71`); heavy/conditional
surfaces (graph viewer, Settings, SessionsBrowser, ExpressSetup) are already
`React.lazy`-split. `index.html` carries a standard responsive viewport meta
tag (`width=device-width, initial-scale=1.0`) but that is boilerplate, not
evidence of an actual mobile layout — there is no mobile/touch/wearable-
specific code anywhere in `src/` (confirmed by grep: zero matches for
`mobile`/`wearable`/viewport-media/`@media (hover`, and `capture: touch`-style
handling exists only generically in `ResizeDivider`'s pointer events and
`Tooltip`'s focus/hover/touch parity, not as a distinct mobile mode).

**wdio-e2e feature bridge:** the real Tauri binary must be built with the
`wdio-e2e` Cargo feature plus `src-tauri/tauri.e2e.conf.json`'s capability
overlay for the E2E suite to run at all; it drives the actual compiled
WebKitGTK/WebView2/WKWebView through `@wdio/tauri-service`'s embedded
WebDriver provider, with native audio mocked via a hand-rolled `fetch`
patch (`installIpcMockBridge`, `shell.e2e.ts:150-215`) because
`@wdio/tauri-plugin@1.3.0`'s own invoke-interception is inert against
Tauri v2's frozen `window.__TAURI_INTERNALS__.invoke` (documented in detail
in the spec file's own comments — a real, confirmed library limitation, not
a workaround of convenience). This means E2E selector/behavior contracts
(§3) are exercised against the genuine production IPC path, not a jsdom
mock — any overhaul must keep those exact ids/labels/text or update the
spec file in lockstep.

**Theme/dark-mode status:** shipping and audited, not a gap (see §1). Not a
blocker for an overhaul; a new visual system would need to either keep the
token-swap mechanism or replace it wholesale (ADR-0009 superseded).

**Current visual style, read directly from the CSS:**
- Palette: dark theme is a "deep, neutral slate with a cohesive indigo
  accent" — `--bg-primary #0e1117` → `--bg-elevated #232c3a` (4-step
  ascending-elevation slate), `--accent #6c8cff` (indigo-blue), plus six
  named accent hues (red/green/gemini-teal/blue/yellow/purple), each with a
  paired `--on-accent-*` foreground for contrast. Light theme swaps to
  white/off-white surfaces (`#ffffff`→`#e9edf3`) with darkened/desaturated
  accents re-audited for AA on white.
- Typography: system font stack (`-apple-system, BlinkMacSystemFont, "Segoe
  UI", Roboto, sans-serif`) — no custom/branded typeface anywhere. Type scale
  runs 10px (`--font-size-2xs`) to 24px (`--font-size-3xl`), i.e. deliberately
  compact/dense — this reads as a dense "operator console" type scale, not a
  marketing-site scale.
- Spacing: 4px-base scale, `--space-1` (2px) through `--space-12` (48px) —
  tight, utilitarian spacing typical of a dense desktop tool, not generous
  "product" whitespace.
- Shape/elevation: small radii (`--radius-xs` 2px → `--radius-xl` 12px),
  soft dark shadows (`0 2–8px ... rgba(0,0,0,0.35–0.5)`), status expressed via
  translucent color tints (`--tint-success/warning/danger/info`, ~12-22%
  alpha) rather than filled badges in the dark theme (opaque pale fills in
  light theme, per an explicit comment that translucent tints fail contrast
  on light backgrounds).
- Net visual read: a **utilitarian, developer/operator-console aesthetic** —
  accurate, accessible, internally consistent — but explicitly not a
  "product" or "consumer" visual language (no illustration, no marketing
  typography, no brand personality beyond the indigo accent). This is
  consistent with the maintainer's framing ("does not look nice") even
  though the accessibility/correctness engineering underneath it is mature.

**Mobile/responsive affordances:** two CSS breakpoints in the whole app —
`max-width: 1120px` and `max-width: 900px` in `src/styles/layout.css` (main
shell reflow, confirmed by the 2026-07-05 review to produce zero horizontal
overflow) and one more, `max-width: 720px`, scoped only to the Settings
modal (`src/styles/settings.css:1538`). Nothing below 720px is targeted
anywhere in the codebase — there is no phone-width layout, no touch-target
sizing pass, no PWA manifest, no Capacitor/Tauri-mobile target configured in
`tauri.conf.json`, and (per §4) mobile support is explicitly staged as a
future architecture-session (`audio-graph-8055`) rather than in-flight work.
Wearables have zero representation in code, design docs, or the backlog —
an overhaul that takes the maintainer's "not only desktop but mobile and
wearables" framing literally would be greenfield product/architecture work,
not a restyle.
