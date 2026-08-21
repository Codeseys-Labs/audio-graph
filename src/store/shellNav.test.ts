import { beforeEach, describe, expect, it } from "vitest";
import { useAudioGraphStore } from "./index";
import {
  DEFAULT_SHELL_NAV,
  deriveWorkspaceView,
  navForWorkspaceView,
  type SessionLens,
  type ShellDest,
  type ShellNav,
} from "./shellNav";

const ALL_LENSES: SessionLens[] = [
  "notes",
  "transcript",
  "timeline",
  "graph",
  "route",
];
const ALL_DESTS: ShellDest[] = ["capture", "sessions"];
const ALL_SESSION_IDS: Array<string | null> = [null, "session-1"];

/** Every combination of dest x lens x sessionId — the grid the "20-nav-state"
 * exhaustive check from the review referred to (2 dests x 5 lenses x 2
 * sessionIds = 20 states). */
function allNavStates(): ShellNav[] {
  const states: ShellNav[] = [];
  for (const dest of ALL_DESTS) {
    for (const lens of ALL_LENSES) {
      for (const sessionId of ALL_SESSION_IDS) {
        states.push({ dest, lens, sessionId });
      }
    }
  }
  return states;
}

describe("store: shellNav pure helpers", () => {
  describe("deriveWorkspaceView", () => {
    // SHELL-R4 (plan §R4, ADR-0046): the three-view during/after/analysis
    // mapping (and its `lens === "graph"` disambiguation) is retired
    // outright — `dest` IS the workspace view now, regardless of `lens` or
    // `sessionId`.
    it("maps dest:capture to capture regardless of lens/sessionId", () => {
      for (const lens of ALL_LENSES) {
        for (const sessionId of ALL_SESSION_IDS) {
          expect(
            deriveWorkspaceView({ dest: "capture", lens, sessionId }),
          ).toBe("capture");
        }
      }
    });

    it("maps dest:sessions to sessions regardless of lens/sessionId", () => {
      for (const lens of ALL_LENSES) {
        for (const sessionId of ALL_SESSION_IDS) {
          expect(
            deriveWorkspaceView({ dest: "sessions", lens, sessionId }),
          ).toBe("sessions");
        }
      }
    });
  });

  describe("navForWorkspaceView", () => {
    it("switches dest to the requested view, preserving lens and sessionId", () => {
      const current: ShellNav = {
        dest: "sessions",
        lens: "transcript",
        sessionId: "s1",
      };
      expect(navForWorkspaceView("capture", current)).toEqual({
        dest: "capture",
        lens: "transcript",
        sessionId: "s1",
      });
    });

    it("preserves a graph lens untouched when switching dest (no more legacy disambiguation)", () => {
      const current: ShellNav = {
        dest: "capture",
        lens: "graph",
        sessionId: "s2",
      };
      expect(navForWorkspaceView("sessions", current)).toEqual({
        dest: "sessions",
        lens: "graph",
        sessionId: "s2",
      });
    });

    it("same-value bailout: returns the exact current reference when already at the requested view", () => {
      const captureNav: ShellNav = {
        dest: "capture",
        lens: "graph",
        sessionId: "s4",
      };
      expect(navForWorkspaceView("capture", captureNav)).toBe(captureNav);

      const sessionsNav: ShellNav = {
        dest: "sessions",
        lens: "route",
        sessionId: null,
      };
      expect(navForWorkspaceView("sessions", sessionsNav)).toBe(sessionsNav);
    });

    it("round-trip: deriveWorkspaceView(navForWorkspaceView(view, current)) === view for every state x view", () => {
      for (const current of allNavStates()) {
        for (const view of ALL_DESTS) {
          const next = navForWorkspaceView(view, current);
          expect(deriveWorkspaceView(next)).toBe(view);
        }
      }
    });

    it("never touches lens or sessionId across any state x view combination", () => {
      for (const current of allNavStates()) {
        for (const view of ALL_DESTS) {
          const next = navForWorkspaceView(view, current);
          expect(next.lens).toBe(current.lens);
          expect(next.sessionId).toBe(current.sessionId);
        }
      }
    });
  });
});

describe("store: shellNav slice (wired into useAudioGraphStore)", () => {
  beforeEach(() => {
    useAudioGraphStore.setState({ nav: DEFAULT_SHELL_NAV });
  });

  it("defaults to DEFAULT_SHELL_NAV (capture / notes / no session)", () => {
    expect(useAudioGraphStore.getState().nav).toEqual(DEFAULT_SHELL_NAV);
  });

  it("setWorkspaceView drives nav through capture -> sessions -> capture", () => {
    const { setWorkspaceView } = useAudioGraphStore.getState();

    setWorkspaceView("capture");
    expect(useAudioGraphStore.getState().nav.dest).toBe("capture");

    setWorkspaceView("sessions");
    expect(useAudioGraphStore.getState().nav).toEqual({
      dest: "sessions",
      lens: "notes",
      sessionId: null,
    });

    setWorkspaceView("capture");
    expect(useAudioGraphStore.getState().nav).toEqual({
      dest: "capture",
      lens: "notes",
      sessionId: null,
    });
  });

  it("setWorkspaceView is a same-value no-op: redundant calls don't replace the nav reference", () => {
    useAudioGraphStore.getState().setWorkspaceView("capture");
    const navBefore = useAudioGraphStore.getState().nav;

    useAudioGraphStore.getState().setWorkspaceView("capture");
    expect(useAudioGraphStore.getState().nav).toBe(navBefore);
  });

  it("setNavDest updates only dest, preserving lens and sessionId", () => {
    useAudioGraphStore.setState({
      nav: { dest: "capture", lens: "timeline", sessionId: "keep-me" },
    });
    useAudioGraphStore.getState().setNavDest("sessions");
    expect(useAudioGraphStore.getState().nav).toEqual({
      dest: "sessions",
      lens: "timeline",
      sessionId: "keep-me",
    });
  });

  it("setNavSessionId updates only sessionId, preserving dest and lens", () => {
    useAudioGraphStore.setState({
      nav: { dest: "sessions", lens: "route", sessionId: null },
    });
    useAudioGraphStore.getState().setNavSessionId("abc-123");
    expect(useAudioGraphStore.getState().nav).toEqual({
      dest: "sessions",
      lens: "route",
      sessionId: "abc-123",
    });
    useAudioGraphStore.getState().setNavSessionId(null);
    expect(useAudioGraphStore.getState().nav.sessionId).toBeNull();
  });

  it("setNavLens updates only lens, preserving dest and sessionId", () => {
    useAudioGraphStore.setState({
      nav: { dest: "sessions", lens: "notes", sessionId: "s9" },
    });
    useAudioGraphStore.getState().setNavLens("graph");
    expect(useAudioGraphStore.getState().nav).toEqual({
      dest: "sessions",
      lens: "graph",
      sessionId: "s9",
    });
  });

  // SHELL-R4 retires the R1-documented "setNavDest alone does not
  // renormalize lens" footgun outright: there is no more `analysis` value
  // for a stale `lens` to accidentally resolve to, so setNavDest round-trips
  // (capture -> sessions) no longer change what `deriveWorkspaceView` reports
  // regardless of the lens left over from an earlier graph-edge focus.
  it("setNavDest round-trips (capture -> sessions) do not disturb deriveWorkspaceView, even with a leftover graph lens", () => {
    useAudioGraphStore.setState({
      nav: { dest: "sessions", lens: "graph", sessionId: "s10" },
    });
    useAudioGraphStore.getState().setNavDest("capture");
    useAudioGraphStore.getState().setNavDest("sessions");
    expect(deriveWorkspaceView(useAudioGraphStore.getState().nav)).toBe(
      "sessions",
    );
  });

  it("action identity is stable across nav writes (load-bearing for App.tsx's dependency arrays)", () => {
    const before = useAudioGraphStore.getState().setWorkspaceView;
    useAudioGraphStore.getState().setWorkspaceView("sessions");
    useAudioGraphStore.getState().setNavLens("notes");
    useAudioGraphStore.getState().setNavSessionId("s11");
    const after = useAudioGraphStore.getState().setWorkspaceView;
    // If this identity ever becomes unstable (e.g. a wrapper or curried
    // selector), the isCapturing effect in App.tsx
    // (`if (isCapturing) setWorkspaceView("capture")`) would re-fire on every
    // render it appears in a dependency array for, because a fresh nav write
    // would produce a fresh function reference -> re-render -> effect
    // re-runs -> infinite loop.
    expect(after).toBe(before);
  });
});
