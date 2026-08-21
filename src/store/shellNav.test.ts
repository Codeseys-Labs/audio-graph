import { beforeEach, describe, expect, it } from "vitest";
import { useAudioGraphStore } from "./index";
import {
  DEFAULT_SHELL_NAV,
  deriveWorkspaceView,
  type LegacyWorkspaceView,
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
const ALL_VIEWS: LegacyWorkspaceView[] = ["during", "after", "analysis"];

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
    it("maps dest:capture to during regardless of lens/sessionId", () => {
      for (const lens of ALL_LENSES) {
        for (const sessionId of ALL_SESSION_IDS) {
          expect(
            deriveWorkspaceView({ dest: "capture", lens, sessionId }),
          ).toBe("during");
        }
      }
    });

    it("maps dest:sessions + lens:graph to analysis", () => {
      for (const sessionId of ALL_SESSION_IDS) {
        expect(
          deriveWorkspaceView({ dest: "sessions", lens: "graph", sessionId }),
        ).toBe("analysis");
      }
    });

    it("maps dest:sessions + any non-graph lens to after", () => {
      for (const lens of ALL_LENSES.filter((l) => l !== "graph")) {
        for (const sessionId of ALL_SESSION_IDS) {
          expect(
            deriveWorkspaceView({ dest: "sessions", lens, sessionId }),
          ).toBe("after");
        }
      }
    });
  });

  describe("navForWorkspaceView", () => {
    it("during: switches dest to capture, preserves lens and sessionId", () => {
      const current: ShellNav = {
        dest: "sessions",
        lens: "transcript",
        sessionId: "s1",
      };
      expect(navForWorkspaceView("during", current)).toEqual({
        dest: "capture",
        lens: "transcript",
        sessionId: "s1",
      });
    });

    it("analysis: switches dest to sessions and lens to graph, preserves sessionId", () => {
      const current: ShellNav = {
        dest: "capture",
        lens: "notes",
        sessionId: "s2",
      };
      expect(navForWorkspaceView("analysis", current)).toEqual({
        dest: "sessions",
        lens: "graph",
        sessionId: "s2",
      });
    });

    it("after: switches dest to sessions and resets lens:graph to notes", () => {
      const current: ShellNav = {
        dest: "sessions",
        lens: "graph",
        sessionId: "s3",
      };
      expect(navForWorkspaceView("after", current)).toEqual({
        dest: "sessions",
        lens: "notes",
        sessionId: "s3",
      });
    });

    it("after: preserves a non-graph lens untouched", () => {
      const current: ShellNav = {
        dest: "capture",
        lens: "timeline",
        sessionId: null,
      };
      expect(navForWorkspaceView("after", current)).toEqual({
        dest: "sessions",
        lens: "timeline",
        sessionId: null,
      });
    });

    it("same-value bailout: returns the exact current reference when already at the requested view", () => {
      const duringNav: ShellNav = {
        dest: "capture",
        lens: "graph",
        sessionId: "s4",
      };
      expect(navForWorkspaceView("during", duringNav)).toBe(duringNav);

      const analysisNav: ShellNav = {
        dest: "sessions",
        lens: "graph",
        sessionId: null,
      };
      expect(navForWorkspaceView("analysis", analysisNav)).toBe(analysisNav);

      const afterNav: ShellNav = {
        dest: "sessions",
        lens: "route",
        sessionId: "s5",
      };
      expect(navForWorkspaceView("after", afterNav)).toBe(afterNav);
    });

    it("round-trip: deriveWorkspaceView(navForWorkspaceView(view, current)) === view for every state x view", () => {
      for (const current of allNavStates()) {
        for (const view of ALL_VIEWS) {
          const next = navForWorkspaceView(view, current);
          expect(deriveWorkspaceView(next)).toBe(view);
        }
      }
    });

    it("never touches sessionId across any state x view combination", () => {
      for (const current of allNavStates()) {
        for (const view of ALL_VIEWS) {
          expect(navForWorkspaceView(view, current).sessionId).toBe(
            current.sessionId,
          );
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

  it("setWorkspaceView drives nav through during -> analysis -> after", () => {
    const { setWorkspaceView } = useAudioGraphStore.getState();

    setWorkspaceView("during");
    expect(useAudioGraphStore.getState().nav.dest).toBe("capture");

    setWorkspaceView("analysis");
    expect(useAudioGraphStore.getState().nav).toEqual({
      dest: "sessions",
      lens: "graph",
      sessionId: null,
    });

    setWorkspaceView("after");
    expect(useAudioGraphStore.getState().nav).toEqual({
      dest: "sessions",
      lens: "notes",
      sessionId: null,
    });
  });

  it("setWorkspaceView is a same-value no-op: redundant calls don't replace the nav reference", () => {
    useAudioGraphStore.getState().setWorkspaceView("during");
    const navBefore = useAudioGraphStore.getState().nav;

    useAudioGraphStore.getState().setWorkspaceView("during");
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

  it("footgun (documented, not a bug): setNavDest('sessions') while lens is still 'graph' silently derives to analysis, not after", () => {
    // Reachable today: the graph-edge-focus effect sets
    // {dest:'sessions', lens:'graph'}; a later setNavDest('capture') then
    // setNavDest('sessions') (bypassing setWorkspaceView's lens-aware
    // normalization) would land back on Analysis, not Review/"after". This
    // test pins the current, intentional (if sharp-edged) behavior so a
    // future caller of setNavDest doesn't discover it via a UI regression.
    useAudioGraphStore.setState({
      nav: { dest: "sessions", lens: "graph", sessionId: "s10" },
    });
    useAudioGraphStore.getState().setNavDest("capture");
    useAudioGraphStore.getState().setNavDest("sessions");
    expect(deriveWorkspaceView(useAudioGraphStore.getState().nav)).toBe(
      "analysis",
    );
  });

  it("action identity is stable across nav writes (load-bearing for App.tsx's dependency arrays)", () => {
    const before = useAudioGraphStore.getState().setWorkspaceView;
    useAudioGraphStore.getState().setWorkspaceView("analysis");
    useAudioGraphStore.getState().setNavLens("notes");
    useAudioGraphStore.getState().setNavSessionId("s11");
    const after = useAudioGraphStore.getState().setWorkspaceView;
    // If this identity ever becomes unstable (e.g. a wrapper or curried
    // selector), the isCapturing effect in App.tsx
    // (`if (isCapturing) setWorkspaceView("during")`) would re-fire on every
    // render it appears in a dependency array for, because a fresh nav write
    // would produce a fresh function reference -> re-render -> effect
    // re-runs -> infinite loop.
    expect(after).toBe(before);
  });
});
