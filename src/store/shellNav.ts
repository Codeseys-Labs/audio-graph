/**
 * ShellNav — the first real store slice file (SHELL-R1, seed
 * audio-graph-59fb, parent audio-graph-19c7, plan §R1, ADR-0046).
 *
 * ADR-0046 collapses the shell to two destinations (Capture / Sessions) with
 * contextual lenses, but that collapse itself is R4's job (tab-id rename +
 * E2E rewrite, gated on R0+R2+R3 landing first). THIS unit is contract-
 * neutral: it introduces the typed nav object the later units need, while
 * the app keeps rendering today's `during`/`after`/`analysis` three-tab
 * shell, byte-identical, ids/classes/labels/DOM untouched.
 *
 * `ShellNav` is deliberately shaped like the FUTURE two-destination world
 * (`dest: "capture" | "sessions"`) rather than the current three-tab one,
 * because that's the whole point: `stopCapture` (a store action, landing in
 * R2) needs to set `nav = { dest: "sessions", sessionId, lens: "notes" }`
 * directly, and a store action cannot reach into App-local `useState`
 * (`App.tsx`'s old `workspaceView`). Moving nav into the store is what makes
 * that later routing possible at all.
 *
 * Reconciling two destinations with three legacy tabs during the interim:
 * `lens` doubles as the disambiguator between the legacy `after` and
 * `analysis` tabs, both of which map to `dest: "sessions"` today (neither
 * requires a *loaded* session yet — that gate is unchanged, still owned by
 * `App.tsx`'s existing `samplePreviewActive`/`loadedSessionId` reads, not by
 * nav). `lens: "graph"` ⇒ legacy `analysis`; any other lens ⇒ legacy `after`.
 * This mapping is forward-compatible with R2 (Graph becomes a lens on the
 * Sessions destination) and R4 (the `analysis` tab is deleted outright, its
 * occupants already homed under lenses/drawers) — it is not a new invention
 * this unit introduces just to thread the needle.
 *
 * Seven flags named in the ticket, one fully migrated here, six delegated:
 *   - FULLY MIGRATED: App-local `workspaceView` (`App.tsx`'s old
 *     `useState<WorkspaceView>`) → `nav.dest` + `nav.lens`, derived via
 *     `deriveWorkspaceView` / `navForWorkspaceView` below. This is the one
 *     migration R2 structurally requires (see above), so it happens now.
 *   - DELEGATED (state, actions, and every existing call site unchanged):
 *     `loadedSessionId`, `rightPanelTab`, `sessionsBrowserOpen`,
 *     `agentOverlayOpen`, `tokenOverlayOpen`, `settingsOpen`. Each has
 *     several deep call sites in `store/index.ts` (sample-preview resets,
 *     session-load success, credential-probe flows) that this contract-
 *     neutral unit must not touch. Their actual behavioral absorption is
 *     later units' job by design: `sessionsBrowserOpen` dissolves once
 *     Sessions is a real destination (R2); `rightPanelTab` becomes lens tabs
 *     (R2); `agentOverlayOpen`/`tokenOverlayOpen`/`settingsOpen` move under
 *     the System drawer (R3). `ShellDrawerState` below documents that target
 *     shape so R3 has a name to land on, without this unit faking a
 *     synchronized mirror that could silently drift from the real flags.
 */

/** The two ADR-0046 destinations. Still not what `App.tsx` renders this
 * unit — see the module doc for the during/after/analysis reconciliation. */
export type ShellDest = "capture" | "sessions";

/** The R2 lens set (Notes/Transcript/Timeline/Graph/Route). Unused for
 * rendering until R2; `"graph"` is load-bearing THIS unit only as the
 * legacy-`analysis` disambiguator (module doc above). */
export type SessionLens =
  | "notes"
  | "transcript"
  | "timeline"
  | "graph"
  | "route";

/** The legacy three-tab shell's view, kept structurally identical to
 * `App.tsx`'s local `WorkspaceView` type so the two are interchangeable
 * without an import — R4 deletes this union's `"analysis"` member. */
export type LegacyWorkspaceView = "during" | "after" | "analysis";

/** One typed nav object replacing App-local `workspaceView`. */
export interface ShellNav {
  dest: ShellDest;
  /** Id of the session being routed to/viewed. `null` outside Sessions.
   * NOT wired to `loadedSessionId` this unit (see module doc) — R2's
   * `stopCapture` sets it directly. */
  sessionId: string | null;
  lens: SessionLens;
}

/**
 * Target shape for R3's System drawer. Documented here (not instantiated as
 * live store state) so R3 has a name to land on; instantiating a mirror of
 * `settingsOpen`/`sessionsBrowserOpen`/`agentOverlayOpen`/`tokenOverlayOpen`
 * now, without rewiring every reset call site that writes those flags
 * directly, would create a second source of truth that silently drifts from
 * the real one — worse than not shipping it yet.
 */
export interface ShellDrawerState {
  settingsOpen: boolean;
  sessionsBrowserOpen: boolean;
  agentOverlayOpen: boolean;
  tokenOverlayOpen: boolean;
}

export const DEFAULT_SHELL_NAV: ShellNav = {
  dest: "capture",
  sessionId: null,
  lens: "notes",
};

/**
 * Derive the legacy three-tab `workspaceView` from `nav`. Pure function —
 * exported so `App.tsx` (and this file's own action below) share one
 * mapping. `dest === "capture"` ⇒ `during`; `dest === "sessions"` splits on
 * `lens` (see module doc for why `"graph"` means `analysis`).
 */
export function deriveWorkspaceView(nav: ShellNav): LegacyWorkspaceView {
  if (nav.dest === "capture") return "during";
  return nav.lens === "graph" ? "analysis" : "after";
}

/**
 * Inverse of `deriveWorkspaceView`: compute the `nav` patch a legacy tab
 * click/keyboard-nav/programmatic `setWorkspaceView(view)` call should
 * produce, given the current `nav` (so an unrelated field, e.g. a future
 * `sessionId`, is preserved rather than clobbered).
 *
 * Same-value bailout: if `current` already derives to `view`, return
 * `current` by reference unchanged. The old App-local `useState` setter got
 * this for free from React's `Object.is` bailout; a plain store `set()`
 * doesn't, so without this check every redundant `setWorkspaceView(view)`
 * call (e.g. the `isCapturing` effect re-firing `setWorkspaceView("during")`
 * on an unrelated re-render) would write a fresh `nav` object and force an
 * extra `App()` re-render for a no-op transition.
 */
export function navForWorkspaceView(
  view: LegacyWorkspaceView,
  current: ShellNav,
): ShellNav {
  if (deriveWorkspaceView(current) === view) return current;
  switch (view) {
    case "during":
      return { ...current, dest: "capture" };
    case "analysis":
      return { ...current, dest: "sessions", lens: "graph" };
    case "after":
      return {
        ...current,
        dest: "sessions",
        // Leaving a lens-disambiguated analysis view for "after" must not
        // silently keep pointing at the graph lens.
        lens: current.lens === "graph" ? "notes" : current.lens,
      };
  }
}

/** The slice of `AudioGraphStore` this file owns. */
export interface ShellNavSlice {
  nav: ShellNav;
  /** Store-owned replacement for `App.tsx`'s old
   * `setWorkspaceView` local setter — same three-way input, same derived
   * rendering, now reachable from store actions (R2's `stopCapture`). */
  setWorkspaceView: (view: LegacyWorkspaceView) => void;
  setNavDest: (dest: ShellDest) => void;
  setNavSessionId: (sessionId: string | null) => void;
  setNavLens: (lens: SessionLens) => void;
}

type ShellNavSet = (
  partial:
    | Partial<ShellNavSlice>
    | ((state: ShellNavSlice) => Partial<ShellNavSlice>),
) => void;
type ShellNavGet = () => ShellNavSlice;

/**
 * Slice creator, spread into the single `create<AudioGraphStore>(...)` call
 * in `store/index.ts` — this repo has no multi-store slice machinery yet, so
 * this is a plain factory function rather than a `StateCreator` generic; it
 * is typed against the minimal `ShellNavSlice` shape it needs, which the
 * full `AudioGraphStore` structurally satisfies (it is a superset).
 */
export function createShellNavSlice(
  set: ShellNavSet,
  _get: ShellNavGet,
): ShellNavSlice {
  return {
    nav: DEFAULT_SHELL_NAV,
    setWorkspaceView: (view) =>
      set((state) => ({ nav: navForWorkspaceView(view, state.nav) })),
    setNavDest: (dest) => set((state) => ({ nav: { ...state.nav, dest } })),
    setNavSessionId: (sessionId) =>
      set((state) => ({ nav: { ...state.nav, sessionId } })),
    setNavLens: (lens) => set((state) => ({ nav: { ...state.nav, lens } })),
  };
}
