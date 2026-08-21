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
 *
 * R2 UPDATE (SHELL-R2, seed audio-graph-e0c4): `sessionsBrowserOpen`'s
 * dissolution landed partially, not fully, and that's deliberate rather
 * than an oversight. Its last remaining reader — the Escape handler in
 * `useKeyboardShortcuts.ts` — is retired (Sessions is a real destination
 * now, with nothing left to "close"), which is the behavioral fix that
 * mattered: before this, the flag latching `true` on open silently
 * swallowed the next Escape keystroke for the rest of the session. The
 * flag ITSELF (state + `openSessionsBrowser`/`closeSessionsBrowser`
 * writes) stays wired, unread by anything now, because `App.contract.
 * test.tsx` and `App.test.tsx` both set it directly via `setState` and
 * R2's own acceptance criteria requires both to stay byte-identical —
 * removing the field would force edits there. Deleting the now-inert
 * field/actions outright is left to R4 (which already owns the
 * during/after/analysis tab-id deletion those two test files are pinned
 * against, so it's the natural point to revisit their fixtures too).
 *
 * R4 UPDATE (SHELL-R4, plan §R4, ADR-0046): the collapse this file's module
 * doc has been narrating as a future event has now happened — `App.tsx`
 * deletes the `analysis` tab (and its right-rail composition) outright and
 * renders only the two ADR-0046 destinations. The "reconciling two
 * destinations with three legacy tabs during the interim" section above (and
 * the `lens === "graph"` disambiguation it describes) was, in its own words,
 * "forward-compatible scaffolding, not product intent" — it is retired
 * outright here, not merely extended. `LegacyWorkspaceView` is deleted;
 * `deriveWorkspaceView`/`navForWorkspaceView`/`setWorkspaceView` keep their
 * names (App.tsx and `store/index.ts`'s `openSessionsBrowser` still call
 * them under these names) but their bodies collapse to a trivial
 * `ShellDest`-only mapping — `dest` now literally IS the workspace view,
 * with no lens-based disambiguation left to perform. One direct consequence:
 * the R1-documented "`setNavDest` alone does not renormalize `lens`" sharp
 * edge is gone too — there is no more `analysis` value for a stale `lens`
 * to accidentally resolve to.
 */

/** The two ADR-0046 destinations. SHELL-R4: this is now literally what
 * `App.tsx` renders — no more during/after/analysis reconciliation. */
export type ShellDest = "capture" | "sessions";

/** The R2 lens set (Notes/Transcript/Timeline/Graph/Route), selectable
 * within the Sessions destination. `"graph"` was load-bearing pre-R4 as the
 * legacy-`analysis` disambiguator; SHELL-R4 retires that role — it is now
 * just an ordinary lens value like the other four. */
export type SessionLens =
  | "notes"
  | "transcript"
  | "timeline"
  | "graph"
  | "route";

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
 * Derive the workspace view from `nav`. Pure function — exported so
 * `App.tsx` (and this file's own action below) share one mapping. SHELL-R4:
 * `dest` IS the workspace view now; this is a trivial passthrough, kept
 * (rather than inlined at App.tsx's one call site) only so `setWorkspaceView`
 * below can round-trip through the same name App.tsx already imports.
 */
export function deriveWorkspaceView(nav: ShellNav): ShellDest {
  return nav.dest;
}

/**
 * Inverse of `deriveWorkspaceView`: compute the `nav` patch a destination-tab
 * click/keyboard-nav/programmatic `setWorkspaceView(view)` call should
 * produce, given the current `nav` (so an unrelated field, e.g. `sessionId`,
 * is preserved rather than clobbered).
 *
 * Same-value bailout: if `current` already derives to `view`, return
 * `current` by reference unchanged. The old App-local `useState` setter got
 * this for free from React's `Object.is` bailout; a plain store `set()`
 * doesn't, so without this check every redundant `setWorkspaceView(view)`
 * call (e.g. the `isCapturing` effect re-firing `setWorkspaceView("capture")`
 * on an unrelated re-render) would write a fresh `nav` object and force an
 * extra `App()` re-render for a no-op transition.
 */
export function navForWorkspaceView(
  view: ShellDest,
  current: ShellNav,
): ShellNav {
  if (current.dest === view) return current;
  return { ...current, dest: view };
}

/** The slice of `AudioGraphStore` this file owns. */
export interface ShellNavSlice {
  nav: ShellNav;
  /** Store-owned replacement for `App.tsx`'s old
   * `setWorkspaceView` local setter — same two-way input, same derived
   * rendering, now reachable from store actions (R2's `stopCapture`). Prefer
   * this over the blunter `setNavDest` below: it carries the same-value
   * bailout `setNavDest` deliberately does not (see that action's doc). */
  setWorkspaceView: (view: ShellDest) => void;
  /** Raw dest write — no same-value bailout, unlike `setWorkspaceView`. At
   * HEAD this has NO production call site (App.tsx/SessionsBrowser use
   * `setWorkspaceView`, `setNavSessionId`, `setNavLens` instead; the only
   * callers are `shellNav.test.ts`'s direct slice tests). Retained for a
   * caller that needs to set `dest` without touching `lens`/`sessionId` —
   * plausibly the deferred SessionsBrowser/`nav.lens` unification (see
   * `SessionsBrowser.tsx`'s module doc) — rather than deleted speculatively. */
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
