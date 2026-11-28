import { invoke } from "@tauri-apps/api/core";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useAudioGraphStore } from "../store";
import type { PendingFinalizingSession, SessionMetadata } from "../types";
import NowStrip from "./NowStrip";
import SessionsBrowser, {
  applyFilterAndSort,
  mergeSessionRows,
} from "./SessionsBrowser";
import "../i18n";

const mockedInvoke = vi.mocked(invoke);

function makeSession(overrides: Partial<SessionMetadata>): SessionMetadata {
  return {
    id: overrides.id ?? "00000000-0000-0000-0000-000000000000",
    title: null,
    created_at: 1_700_000_000_000,
    ended_at: null,
    duration_seconds: 60,
    status: "complete",
    segment_count: 10,
    speaker_count: 2,
    entity_count: 5,
    transcript_path: "",
    graph_path: "",
    deleted: false,
    deleted_at: null,
    ...overrides,
  };
}

function makePending(
  overrides: Partial<PendingFinalizingSession> = {},
): PendingFinalizingSession {
  return {
    ...makeSession({ id: "pending-1" }),
    status: "active",
    ...overrides,
    optimistic: true,
  };
}

describe("applyFilterAndSort", () => {
  const sessions: SessionMetadata[] = [
    makeSession({
      id: "alpha-1",
      title: "Alpha",
      created_at: 3000,
      segment_count: 50,
    }),
    makeSession({
      id: "beta-2",
      title: "Beta",
      created_at: 2000,
      segment_count: 10,
    }),
    makeSession({
      id: "gamma-3",
      title: "Gamma",
      created_at: 1000,
      segment_count: 500,
    }),
    makeSession({
      id: "trashed-4",
      title: "Trashed",
      created_at: 4000,
      segment_count: 7,
      deleted: true,
      deleted_at: 5000,
    }),
  ];

  it("hides trashed by default, shows trashed when requested", () => {
    const active = applyFilterAndSort(sessions, "", "newest", false);
    expect(active.map((s) => s.id)).toEqual(["alpha-1", "beta-2", "gamma-3"]);

    const trash = applyFilterAndSort(sessions, "", "newest", true);
    expect(trash.map((s) => s.id)).toEqual(["trashed-4"]);
  });

  it("filters by case-insensitive substring across title and id", () => {
    expect(
      applyFilterAndSort(sessions, "BET", "newest", false).map((s) => s.id),
    ).toEqual(["beta-2"]);
    expect(
      applyFilterAndSort(sessions, "gamma-3", "newest", false).map((s) => s.id),
    ).toEqual(["gamma-3"]);
    expect(
      applyFilterAndSort(sessions, "nothingmatches", "newest", false),
    ).toEqual([]);
  });

  it("sorts by newest / oldest / name / largest", () => {
    expect(
      applyFilterAndSort(sessions, "", "newest", false).map((s) => s.id),
    ).toEqual(["alpha-1", "beta-2", "gamma-3"]);
    expect(
      applyFilterAndSort(sessions, "", "oldest", false).map((s) => s.id),
    ).toEqual(["gamma-3", "beta-2", "alpha-1"]);
    expect(
      applyFilterAndSort(sessions, "", "nameAsc", false).map((s) => s.id),
    ).toEqual(["alpha-1", "beta-2", "gamma-3"]);
    expect(
      applyFilterAndSort(sessions, "", "nameDesc", false).map((s) => s.id),
    ).toEqual(["gamma-3", "beta-2", "alpha-1"]);
    expect(
      applyFilterAndSort(sessions, "", "largest", false).map((s) => s.id),
    ).toEqual(["gamma-3", "alpha-1", "beta-2"]);
  });
});

// SHELL-R2 (audio-graph-e0c4): the 1d92 optimistic-row merge, unit-tested
// directly against the pure function so the self-clearing behavior doesn't
// depend on component-level timing.
describe("mergeSessionRows", () => {
  it("injects the pending row when it has no match in the real list", () => {
    const pending = makePending({ id: "pending-1" });
    const rows = mergeSessionRows([makeSession({ id: "other-1" })], pending);
    expect(rows.map((r) => r.id)).toEqual(["pending-1", "other-1"]);
    expect(rows[0].optimistic).toBe(true);
  });

  it("self-clears once the real list contains the pending id", () => {
    const pending = makePending({ id: "pending-1" });
    const real = makeSession({ id: "pending-1", status: "complete" });
    const rows = mergeSessionRows([real], pending);
    expect(rows).toEqual([real]);
    expect(rows[0].optimistic).toBeUndefined();
  });

  it("passes real rows through unchanged when there is no pending row", () => {
    const real = [makeSession({ id: "a" }), makeSession({ id: "b" })];
    expect(mergeSessionRows(real, null)).toBe(real);
  });
});

describe("SessionsBrowser component", () => {
  beforeEach(() => {
    mockedInvoke.mockReset();
    // Reset persisted sort preference across tests.
    localStorage.clear();
    // Default: list_sessions returns the store's seeded sessions.
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "list_sessions")
        return useAudioGraphStore.getState().sessions;
      if (cmd === "load_session") {
        return {
          transcript: [
            {
              id: "seg-1",
              source_id: "system-default",
              speaker_id: null,
              speaker_label: null,
              text: "historical transcript",
              start_time: 0,
              end_time: 1,
              confidence: 0.9,
            },
          ],
          graph: {
            nodes: [
              {
                id: "node-1",
                name: "Alice",
                entity_type: "Person",
                val: 3,
                color: "#4ade80",
                first_seen: 0,
                last_seen: 1,
                mention_count: 1,
              },
            ],
            links: [],
            stats: {
              total_nodes: 1,
              total_edges: 0,
              total_episodes: 1,
            },
          },
        };
      }
      // Notes/materialized-graph moved off `load_session` into their own
      // lens-fetch commands (seed audio-graph-4fa5 deliverable a); the
      // Notes lens is default-active, so every "select a row" test fires
      // this too. Empty-but-well-shaped defaults keep unrelated tests from
      // tripping the notes/graph lens into an "error" status.
      if (cmd === "load_session_notes_artifacts_cmd") {
        return { notes: null, projection_events: [] };
      }
      if (cmd === "load_session_graph_artifact_cmd") return null;
      if (cmd === "build_session_timeline_cmd")
        return { entries: [], total_count: 0 };
      if (cmd === "purge_expired_sessions") return [];
      if (cmd === "delete_session") return null;
      if (cmd === "restore_session") return null;
      if (cmd === "delete_session_permanently") return null;
      return null;
    });
  });

  afterEach(() => {
    // Leave the store in a known state for the next test.
    useAudioGraphStore.setState({
      sessions: [],
      sessionsLoading: false,
      isCapturing: false,
      isTranscribing: false,
    });
  });

  function seed(sessions: SessionMetadata[]): void {
    useAudioGraphStore.setState({ sessions, sessionsLoading: false });
  }

  /** Opens a row's overflow (export/trash/restore/permanent-delete) menu —
   * SHELL-R2 moved those actions off the row body into a per-row Popover
   * (D5: `@radix-ui/react-popover`), so exercising them now requires opening
   * the trigger first. */
  async function openRowMenu(title: string): Promise<void> {
    const trigger = await screen.findByRole("button", {
      name: new RegExp(`actions for ${title}`, "i"),
    });
    fireEvent.click(trigger);
  }

  it("uses design tokens for borders — no ghost var(--border,#333) fallbacks (d19f)", async () => {
    seed([makeSession({ id: "tokens-1", title: "Token Session" })]);
    const { container } = render(<SessionsBrowser />);

    const item = await screen.findByTestId("session-tokens-1");
    // SHELL-R2 migrates the row itself onto the ADR-0047 recipe layer
    // (`.ag-card`), but a selectable session row is still an object
    // boundary (ADR-0047: rows take --edge, not the flat card's
    // decorative --edge-subtle) — so the row keeps the explicit
    // `border-(--edge)` override on top of `.ag-card`. Same "tokens,
    // never a raw #333 fallback" invariant as the original d19f fix,
    // plus the non-text-contrast floor it guarded.
    expect(item).toHaveClass("ag-card");
    expect(item).toHaveClass("border-(--edge)");
    expect(item.getAttribute("style")).toBeFalsy();

    const searchBox = screen.getByRole("searchbox");
    expect(searchBox).toHaveClass("ag-field__control");
    expect(searchBox.getAttribute("style")).toBeFalsy();

    const sortSelect = screen.getByLabelText(/sort by/i);
    expect(sortSelect).toHaveClass("ag-field__control");

    // No element in the rail/detail composition may carry a ghost #333
    // border fallback.
    for (const el of container.querySelectorAll<HTMLElement>("*")) {
      expect(el.getAttribute("style") ?? "").not.toContain("#333");
    }
  });

  it("is embedded Sessions-destination content, not a modal dialog (plan §R2, ADR-0046, retires the SessionsBrowser overlay)", async () => {
    seed([makeSession({ id: "no-modal-1", title: "No Modal" })]);
    render(<SessionsBrowser />);

    await screen.findByTestId("session-no-modal-1");
    // No `role="dialog"`/`aria-modal` anywhere — SessionsBrowser is the
    // "after" tabpanel's content now, not an overlay `openSessionsBrowser`
    // toggles. Escape-to-close (the retired test this replaces) doesn't
    // apply to embedded destination content the same way a modal's did.
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("filters by search text live (no submit)", async () => {
    seed([
      makeSession({ id: "alpha-1", title: "Alpha Meeting" }),
      makeSession({ id: "beta-2", title: "Beta Sync" }),
    ]);
    render(<SessionsBrowser />);

    // Both visible initially.
    expect(await screen.findByTestId("session-alpha-1")).toBeInTheDocument();
    expect(screen.getByTestId("session-beta-2")).toBeInTheDocument();

    const searchBox = screen.getByRole("searchbox");
    fireEvent.change(searchBox, { target: { value: "Alpha" } });

    expect(screen.getByTestId("session-alpha-1")).toBeInTheDocument();
    expect(screen.queryByTestId("session-beta-2")).not.toBeInTheDocument();
  });

  it("persists sort selection to localStorage", async () => {
    seed([makeSession({ id: "a-1", title: "A" })]);
    render(<SessionsBrowser />);

    const sortSelect = await screen.findByLabelText(/sort by/i);
    fireEvent.change(sortSelect, { target: { value: "nameDesc" } });

    expect(localStorage.getItem("audiograph:sessionsBrowser:sort")).toBe(
      "nameDesc",
    );
  });

  it("hides trashed sessions from the default view", async () => {
    seed([
      makeSession({ id: "live-1", title: "Live" }),
      makeSession({
        id: "dead-2",
        title: "Dead",
        deleted: true,
        deleted_at: 1_700_000_000_000,
      }),
    ]);
    render(<SessionsBrowser />);

    expect(await screen.findByTestId("session-live-1")).toBeInTheDocument();
    expect(screen.queryByTestId("session-dead-2")).not.toBeInTheDocument();
  });

  it("soft-delete calls delete_session and toggles deleted flag", async () => {
    seed([makeSession({ id: "to-trash", title: "Trash Me" })]);
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(true);

    render(<SessionsBrowser />);
    await openRowMenu("Trash Me");

    const deleteBtn = await screen.findByRole("button", {
      name: /^delete$/i,
    });
    fireEvent.click(deleteBtn);

    await waitFor(() => {
      expect(mockedInvoke).toHaveBeenCalledWith("delete_session", {
        sessionId: "to-trash",
      });
    });

    // Entry is now flagged deleted → hidden from default view.
    expect(screen.queryByTestId("session-to-trash")).not.toBeInTheDocument();

    confirmSpy.mockRestore();
  });

  it("disables deletion for the active session", async () => {
    seed([
      makeSession({
        id: "active-session",
        title: "Recording now",
        status: "active",
      }),
    ]);
    render(<SessionsBrowser />);
    await openRowMenu("Recording now");

    const deleteButton = await screen.findByRole("button", {
      name: /^delete$/i,
    });
    expect(deleteButton).toBeDisabled();
    expect(deleteButton).toHaveAttribute(
      "title",
      expect.stringMatching(/active session cannot be deleted/i),
    );
    fireEvent.click(deleteButton);
    expect(mockedInvoke).not.toHaveBeenCalledWith(
      "delete_session",
      expect.anything(),
    );
  });

  it("disables export and delete for the optimistic finalizing row (no backend record exists yet)", async () => {
    // No seeded session — only the client-only optimistic row `stopCapture`
    // writes for the 1d92 gap.
    useAudioGraphStore.setState({
      pendingFinalizingSession: {
        id: "finalizing-1",
        title: null,
        created_at: 1_700_000_000_000,
        ended_at: 1_700_000_060_000,
        duration_seconds: 60,
        status: "active",
        segment_count: 4,
        speaker_count: 1,
        entity_count: 0,
        transcript_path: "",
        graph_path: "",
        deleted: false,
        deleted_at: null,
        optimistic: true,
      },
    });
    render(<SessionsBrowser />);

    const row = await screen.findByTestId("session-finalizing-1");
    expect(row).toHaveTextContent(/finalizing/i);
    await openRowMenu("finalizing-1".slice(0, 8));

    expect(
      await screen.findByRole("button", { name: /^export$/i }),
    ).toBeDisabled();
    expect(screen.getByRole("button", { name: /^delete$/i })).toBeDisabled();
  });

  it("trash view shows restore + delete-permanently actions", async () => {
    seed([
      makeSession({
        id: "trashed-1",
        title: "Trashed One",
        deleted: true,
        deleted_at: 1_700_000_000_000,
      }),
    ]);
    render(<SessionsBrowser />);

    // Toggle trash view on.
    const trashToggle = await screen.findByRole("button", {
      name: /trash \(1\)/i,
    });
    fireEvent.click(trashToggle);

    expect(screen.getByTestId("session-trashed-1")).toBeInTheDocument();
    await openRowMenu("Trashed One");
    expect(
      screen.getByRole("button", { name: /restore/i }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /delete permanently/i }),
    ).toBeInTheDocument();
  });

  it("restore calls restore_session command", async () => {
    seed([
      makeSession({
        id: "to-restore",
        title: "Please restore",
        deleted: true,
        deleted_at: 1_700_000_000_000,
      }),
    ]);
    render(<SessionsBrowser />);

    const trashToggle = await screen.findByRole("button", {
      name: /trash \(1\)/i,
    });
    fireEvent.click(trashToggle);
    await openRowMenu("Please restore");

    // Exact match — the session's own title ("Please restore") also
    // contains "restore", so a loose regex would match its overflow
    // trigger's "Actions for Please restore" label too.
    const restoreBtn = screen.getByRole("button", { name: /^restore$/i });
    fireEvent.click(restoreBtn);

    await waitFor(() => {
      expect(mockedInvoke).toHaveBeenCalledWith("restore_session", {
        sessionId: "to-restore",
      });
    });
  });

  it("selecting a row loads both transcript and graph, and sets the Sessions nav selection", async () => {
    seed([makeSession({ id: "load-me", title: "Load Me" })]);
    useAudioGraphStore.setState({
      transcriptSegments: [],
      graphSnapshot: {
        nodes: [],
        links: [],
        stats: { total_nodes: 0, total_edges: 0, total_episodes: 0 },
      },
    });
    render(<SessionsBrowser />);

    const selectBtn = await screen.findByTestId("session-select-load-me");
    fireEvent.click(selectBtn);

    await waitFor(() => {
      expect(mockedInvoke).toHaveBeenCalledWith("load_session", {
        sessionId: "load-me",
      });
    });
    expect(useAudioGraphStore.getState().transcriptSegments).toHaveLength(1);
    expect(useAudioGraphStore.getState().graphSnapshot.stats.total_nodes).toBe(
      1,
    );
    // SHELL-R2: selection routes through `nav.sessionId`, not the retired
    // `sessionsBrowserOpen`/`rightPanelTab` modal-close side effects.
    expect(useAudioGraphStore.getState().nav.sessionId).toBe("load-me");
  });

  // seed audio-graph-4fa5 deliverable a: opening a session must fetch the
  // Notes lens's own artifacts (the default-active lens) once `load_session`
  // resolves, and the Graph lens's artifact only once the user actually
  // switches to that tab — proving the fetch is gated on lens activation,
  // not bundled into the initial session open.
  it("fetches the Notes lens's artifacts on session open and the Graph lens's only once that tab activates", async () => {
    seed([makeSession({ id: "lens-gate", title: "Lens Gate" })]);
    render(<SessionsBrowser />);

    fireEvent.click(await screen.findByTestId("session-select-lens-gate"));

    await waitFor(() => {
      expect(mockedInvoke).toHaveBeenCalledWith(
        "load_session_notes_artifacts_cmd",
        { sessionId: "lens-gate" },
      );
    });
    expect(mockedInvoke).not.toHaveBeenCalledWith(
      "load_session_graph_artifact_cmd",
      expect.anything(),
    );

    fireEvent.click(screen.getByRole("tab", { name: /^graph$/i }));

    await waitFor(() => {
      expect(mockedInvoke).toHaveBeenCalledWith(
        "load_session_graph_artifact_cmd",
        { sessionId: "lens-gate" },
      );
    });
  });

  // seed audio-graph-4fa5 deliverables a/b: a real, store-driven render
  // (the `App.test.tsx` degradation-notice pattern) proving the typed
  // `artifact_too_large` refusal renders as a plain notice — never a blank
  // panel — and that the transcript lens next to it is unaffected.
  it("renders the artifact refusal notice when the Notes lens's fetch is refused by the byte ceiling", async () => {
    seed([makeSession({ id: "too-big", title: "Too Big" })]);
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "list_sessions")
        return useAudioGraphStore.getState().sessions;
      if (cmd === "load_session") {
        return {
          transcript: [],
          graph: {
            nodes: [],
            links: [],
            stats: { total_nodes: 0, total_edges: 0, total_episodes: 0 },
          },
        };
      }
      if (cmd === "load_session_notes_artifacts_cmd") {
        return Promise.reject({
          code: "artifact_too_large",
          message: {
            artifact_class: "materialized_notes",
            size_bytes: 19_063_321,
            ceiling_bytes: 8 * 1024 * 1024,
          },
        });
      }
      if (cmd === "load_session_graph_artifact_cmd") return null;
      if (cmd === "build_session_timeline_cmd")
        return { entries: [], total_count: 0 };
      return null;
    });
    render(<SessionsBrowser />);

    fireEvent.click(await screen.findByTestId("session-select-too-big"));

    const notice = await screen.findByTestId("artifact-refusal-notice");
    expect(notice).toBeInTheDocument();
    // 19,063,321 bytes / 1024^2 ≈ 18.2 MiB; 8 * 1024 * 1024 bytes = 8.0 MiB
    // exactly — the notice's `formatMb` divides by 1024^2, matching the
    // Rust ceiling constants (defined in MiB, e.g. `8 * 1024 * 1024`).
    expect(notice).toHaveTextContent("18.2 MB");
    expect(notice).toHaveTextContent("8.0 MB");

    // The transcript lens (a different, unrefused artifact) still works.
    fireEvent.click(screen.getByRole("tab", { name: /^transcript$/i }));
    expect(
      screen.queryByTestId("artifact-refusal-notice"),
    ).not.toBeInTheDocument();
  });

  // Fix-round finding: a non-ceiling lens-fetch failure (e.g. the canonical
  // projection replay rejecting one or more patches, surfaced as a plain
  // `SessionInvalid` string rather than the typed `artifact_too_large`
  // shape) used to render as an ordinary empty Notes panel — indistinguishable
  // from "this session has no notes". It must now render its own visible
  // notice instead.
  it("renders the lens fetch error notice when the Notes lens's fetch fails for a non-ceiling reason", async () => {
    seed([makeSession({ id: "corrupt-replay", title: "Corrupt Replay" })]);
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "list_sessions")
        return useAudioGraphStore.getState().sessions;
      if (cmd === "load_session") {
        return {
          transcript: [],
          graph: {
            nodes: [],
            links: [],
            stats: { total_nodes: 0, total_edges: 0, total_episodes: 0 },
          },
        };
      }
      if (cmd === "load_session_notes_artifacts_cmd") {
        return Promise.reject(
          "Canonical projection replay rejected 2 patch(es); derived caches were not loaded",
        );
      }
      if (cmd === "load_session_graph_artifact_cmd") return null;
      if (cmd === "build_session_timeline_cmd")
        return { entries: [], total_count: 0 };
      return null;
    });
    render(<SessionsBrowser />);

    fireEvent.click(await screen.findByTestId("session-select-corrupt-replay"));

    const notice = await screen.findByTestId("lens-fetch-error-notice");
    expect(notice).toBeInTheDocument();
    expect(notice).toHaveTextContent(/rejected 2 patch/i);
    expect(
      screen.queryByTestId("artifact-refusal-notice"),
    ).not.toBeInTheDocument();

    // The transcript lens (unaffected by the Notes lens's own fetch failure)
    // still works.
    fireEvent.click(screen.getByRole("tab", { name: /^transcript$/i }));
    expect(
      screen.queryByTestId("lens-fetch-error-notice"),
    ).not.toBeInTheDocument();
  });

  it("does not call loadSession for the just-stopped session's own resident-live row (the 1d92 optimistic row)", async () => {
    seed([]);
    useAudioGraphStore.setState({
      pendingFinalizingSession: {
        id: "resident-live-1",
        title: null,
        created_at: 1_700_000_000_000,
        ended_at: 1_700_000_060_000,
        duration_seconds: 60,
        status: "active",
        segment_count: 2,
        speaker_count: 1,
        entity_count: 0,
        transcript_path: "",
        graph_path: "",
        deleted: false,
        deleted_at: null,
        optimistic: true,
      },
    });
    render(<SessionsBrowser />);

    const selectBtn = await screen.findByTestId(
      "session-select-resident-live-1",
    );
    fireEvent.click(selectBtn);

    expect(useAudioGraphStore.getState().nav.sessionId).toBe("resident-live-1");
    expect(mockedInvoke).not.toHaveBeenCalledWith(
      "load_session",
      expect.anything(),
    );
  });

  // R2 adversary review finding #1 (MAJOR): `pendingFinalizingSession` is a
  // write-only store slot in production — nothing ever cleared it, so
  // keying the "skip loadSession" decision to `pendingFinalizingSession?.id
  // === row.id` kept skipping forever, long after the row had reconciled
  // into real data. Reproduces the reviewer's exact sequence: stop A
  // (simulated here as a reconciled row A whose `pendingFinalizingSession`
  // slot was never cleared), select B, re-select A — `loadSession` must be
  // invoked for A, not silently skipped.
  it("re-selecting an already-reconciled row still calls loadSession, even though the store's stale pendingFinalizingSession slot from an earlier stop still names that same id", async () => {
    seed([
      makeSession({ id: "session-a", title: "Session A", status: "complete" }),
      makeSession({ id: "session-b", title: "Session B", status: "complete" }),
    ]);
    useAudioGraphStore.setState({
      pendingFinalizingSession: {
        id: "session-a",
        title: null,
        created_at: 1_700_000_000_000,
        ended_at: 1_700_000_060_000,
        duration_seconds: 60,
        status: "active",
        segment_count: 2,
        speaker_count: 1,
        entity_count: 0,
        transcript_path: "",
        graph_path: "",
        deleted: false,
        deleted_at: null,
        optimistic: true,
      },
    });
    // `load_session` never resolves in this test. This isolates the
    // component's row-selection decision (finding #1a, under test here)
    // from the store's OWN defense-in-depth clear of
    // `pendingFinalizingSession` inside a successful `loadSession` (finding
    // #1b, covered separately in store/index.test.ts) — that store-side
    // clear would otherwise fire when B loads and mask a reverted fix (a)
    // by the time A is re-selected.
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "list_sessions")
        return useAudioGraphStore.getState().sessions;
      if (cmd === "load_session") return new Promise(() => {});
      if (cmd === "purge_expired_sessions") return [];
      return null;
    });

    render(<SessionsBrowser />);

    // Row A already reconciled — `sessions` has a real entry for it, so
    // `mergeSessionRows` no longer injects the optimistic placeholder, even
    // though the raw `pendingFinalizingSession` store field is still set.
    const rowA = await screen.findByTestId("session-session-a");
    expect(rowA).not.toHaveTextContent(/finalizing/i);

    fireEvent.click(await screen.findByTestId("session-select-session-b"));
    expect(mockedInvoke).toHaveBeenCalledWith("load_session", {
      sessionId: "session-b",
    });
    expect(useAudioGraphStore.getState().nav.sessionId).toBe("session-b");

    fireEvent.click(await screen.findByTestId("session-select-session-a"));
    expect(mockedInvoke).toHaveBeenCalledWith("load_session", {
      sessionId: "session-a",
    });
    expect(useAudioGraphStore.getState().nav.sessionId).toBe("session-a");
  });

  describe("while capture is running (issue #140: Review next to Live)", () => {
    const LIVE_SEGMENT = {
      id: "live-seg-1",
      source_id: "system-default",
      speaker_id: null,
      speaker_label: null,
      text: "live capture line",
      start_time: 0,
      end_time: 1,
      confidence: 1,
    };

    function startLiveCapture(): void {
      useAudioGraphStore.setState({
        isCapturing: true,
        isTranscribing: true,
        selectedSourceIds: ["system-default"],
        captureStartTime: Date.now(),
        transcriptSegments: [LIVE_SEGMENT],
        loadedSessionId: null,
      });
    }

    /** Every command the mocked `invoke` saw, in order. */
    function invokedCommands(): string[] {
      return mockedInvoke.mock.calls.map(([cmd]) => cmd);
    }

    afterEach(() => {
      useAudioGraphStore.setState({
        selectedSourceIds: [],
        captureStartTime: null,
        transcriptSegments: [],
        loadedSessionId: null,
      });
    });

    it("opens a completed row's transcript in Review while Stop stays reachable, issuing only load_session", async () => {
      seed([makeSession({ id: "past-session", title: "Past Session" })]);
      startLiveCapture();
      const liveSegments = useAudioGraphStore.getState().transcriptSegments;

      render(
        <>
          <NowStrip />
          <SessionsBrowser />
        </>,
      );

      const selectButton = await screen.findByTestId(
        "session-select-past-session",
      );
      expect(selectButton).toBeEnabled();
      expect(selectButton).toHaveAccessibleName("Past Session");
      selectButton.focus();
      expect(document.activeElement).toBe(selectButton);
      fireEvent.click(selectButton);

      // Review lands on the Transcript lens and shows the historical session.
      expect(await screen.findByText("historical transcript")).toBeVisible();
      expect(
        screen.getByRole("tab", { name: /^transcript$/i }),
      ).toHaveAttribute("aria-selected", "true");
      expect(screen.queryByText("live capture line")).not.toBeInTheDocument();
      expect(useAudioGraphStore.getState().nav.sessionId).toBe("past-session");

      // The live session's Stop control is reachable and not disabled.
      const stop = screen.getByRole("button", { name: "Stop" });
      expect(stop).toBeEnabled();
      expect(stop).not.toHaveAttribute("tabindex", "-1");
      stop.focus();
      expect(document.activeElement).toBe(stop);

      // Review issued no capture/writer command: only the rail's list query
      // and the one read-only `load_session`.
      expect(new Set(invokedCommands())).toEqual(
        new Set(["list_sessions", "load_session"]),
      );
      expect(mockedInvoke).toHaveBeenCalledWith("load_session", {
        sessionId: "past-session",
      });

      // The Live fields are exactly what capture left there.
      const live = useAudioGraphStore.getState();
      expect(live.transcriptSegments).toBe(liveSegments);
      expect(live.loadedSessionId).toBeNull();
      expect(live.isCapturing).toBe(true);
      expect(live.isTranscribing).toBe(true);
    });

    it("Stop still stops the live capture while a past session is under Review", async () => {
      seed([makeSession({ id: "past-session", title: "Past Session" })]);
      startLiveCapture();
      render(
        <>
          <NowStrip />
          <SessionsBrowser />
        </>,
      );
      fireEvent.click(await screen.findByTestId("session-select-past-session"));
      await screen.findByText("historical transcript");

      await act(async () => {
        fireEvent.click(screen.getByRole("button", { name: "Stop" }));
      });

      await waitFor(() =>
        expect(mockedInvoke).toHaveBeenCalledWith("stop_capture", {
          sourceId: "system-default",
        }),
      );
    });

    it("keeps Notes, Timeline and Graph gated with the existing message, for those lenses only", async () => {
      seed([makeSession({ id: "past-session", title: "Past Session" })]);
      startLiveCapture();
      render(<SessionsBrowser />);
      fireEvent.click(await screen.findByTestId("session-select-past-session"));
      await screen.findByText("historical transcript");

      for (const lens of [/^notes$/i, /^timeline$/i, /^graph$/i]) {
        fireEvent.click(screen.getByRole("tab", { name: lens }));
        const panel = screen.getByRole("tabpanel");
        expect(
          within(panel).getByText(/stop the live capture to use this view/i),
        ).toBeInTheDocument();
      }
      // The gated lenses did not fetch (their actions hydrate Live fields).
      expect(invokedCommands()).not.toContain(
        "load_session_notes_artifacts_cmd",
      );
      expect(invokedCommands()).not.toContain(
        "load_session_graph_artifact_cmd",
      );
      expect(invokedCommands()).not.toContain("build_session_timeline_cmd");

      // Transcript is not gated.
      fireEvent.click(screen.getByRole("tab", { name: /^transcript$/i }));
      const panel = screen.getByRole("tabpanel");
      expect(
        within(panel).queryByText(/stop the live capture/i),
      ).not.toBeInTheDocument();
      expect(
        await within(panel).findByText("historical transcript"),
      ).toBeVisible();
    });

    it("keeps the live session's own row (active) disabled, with the completed-only message", async () => {
      seed([
        makeSession({
          id: "the-live-one",
          title: "Live Now",
          status: "active",
        }),
      ]);
      startLiveCapture();
      render(<SessionsBrowser />);

      const selectButton = await screen.findByTestId(
        "session-select-the-live-one",
      );
      expect(selectButton).toBeDisabled();
      expect(selectButton).toHaveAttribute(
        "aria-describedby",
        "sessions-live-review-rule",
      );
      // The rule is persistent visible text in the rail (not a hover-only
      // `title` on a disabled button, which Tab skips).
      expect(
        document.getElementById("sessions-live-review-rule"),
      ).toHaveTextContent(
        "Only completed sessions can be opened while capture is running.",
      );
      expect(selectButton).not.toHaveAttribute("title");
      fireEvent.click(selectButton);
      expect(mockedInvoke).not.toHaveBeenCalledWith(
        "load_session",
        expect.anything(),
      );
    });

    it("shows the localized closed error for a corrupt session and never a raw backend string", async () => {
      seed([makeSession({ id: "broken", title: "Broken" })]);
      startLiveCapture();
      mockedInvoke.mockImplementation(async (cmd: string) => {
        if (cmd === "list_sessions")
          return useAudioGraphStore.getState().sessions;
        if (cmd === "load_session") {
          throw { code: "session_invalid", message: { reason: "C:\\boom\\x" } };
        }
        return null;
      });
      render(<SessionsBrowser />);
      fireEvent.click(await screen.findByTestId("session-select-broken"));

      const alert = await screen.findByRole("alert");
      expect(alert).toHaveTextContent(
        "This session's transcript is missing or could not be read.",
      );
      expect(document.body.textContent).not.toContain("boom");
      // Live is untouched by the failed Review.
      expect(useAudioGraphStore.getState().error).toBeNull();
    });

    it("a Review opened while live loads normally if the run ends with nav still on that session", async () => {
      seed([makeSession({ id: "past-session", title: "Past Session" })]);
      startLiveCapture();
      render(<SessionsBrowser />);
      fireEvent.click(await screen.findByTestId("session-select-past-session"));
      await screen.findByText("historical transcript");
      expect(useAudioGraphStore.getState().loadedSessionId).toBeNull();

      // Stop that could not re-route nav (e.g. unresolved session id).
      act(() => {
        useAudioGraphStore.setState({
          isCapturing: false,
          isTranscribing: false,
        });
      });

      await waitFor(() =>
        expect(useAudioGraphStore.getState().loadedSessionId).toBe(
          "past-session",
        ),
      );
      expect(
        useAudioGraphStore.getState().transcriptSegments.map((s) => s.text),
      ).toEqual(["historical transcript"]);
    });
  });

  it("export button invokes export_session_bundle with the session id", async () => {
    // jsdom lacks URL.createObjectURL; stub the download primitives so the
    // happy path drives downloadAsFile instead of throwing (mirrors
    // LiveTranscript / KnowledgeGraphViewer export tests).
    const createObjectURL = vi
      .spyOn(URL, "createObjectURL")
      .mockReturnValue("blob:fake");
    const revokeObjectURL = vi
      .spyOn(URL, "revokeObjectURL")
      .mockImplementation(() => {});
    const anchorClick = vi
      .spyOn(HTMLAnchorElement.prototype, "click")
      .mockImplementation(() => {});
    const sessionId = "export-me";
    seed([makeSession({ id: sessionId, title: "Export Me" })]);
    const mockBundle = {
      schema_version: 1,
      session_id: sessionId,
      transcript: [],
      transcript_events: [],
      diarization_events: [],
      projection_events: [],
    };
    mockedInvoke.mockImplementation(async (cmd: string, _args?: unknown) => {
      if (cmd === "list_sessions")
        return useAudioGraphStore.getState().sessions;
      if (cmd === "export_session_bundle") return mockBundle;
      if (cmd === "purge_expired_sessions") return [];
      return null;
    });

    render(<SessionsBrowser />);
    await openRowMenu("Export Me");

    const exportBtn = await screen.findByRole("button", { name: /^export$/i });
    fireEvent.click(exportBtn);

    await waitFor(() => {
      expect(mockedInvoke).toHaveBeenCalledWith("export_session_bundle", {
        sessionId,
      });
    });
    // The download helper was driven with the bundle blob.
    await waitFor(() => expect(createObjectURL).toHaveBeenCalledTimes(1));
    const blob = createObjectURL.mock.calls[0][0] as Blob;
    expect(blob).toBeInstanceOf(Blob);
    expect(blob.type).toBe("application/json");
    expect(anchorClick).toHaveBeenCalledTimes(1);
    expect(revokeObjectURL).toHaveBeenCalledWith("blob:fake");

    createObjectURL.mockRestore();
    revokeObjectURL.mockRestore();
    anchorClick.mockRestore();
  });

  // R2 adversary review finding #3 (MINOR): the row overflow menu's
  // destructive actions (trash/restore/permanent-delete) were reachable
  // while a live capture is running, even though the detail pane locks
  // entirely in that state. Export stays available — it's read-only
  // against a past session's already-persisted files.
  it("disables the row overflow's destructive actions while a live capture is running, but leaves export enabled", async () => {
    seed([
      makeSession({ id: "live-lock-active", title: "Active During Live" }),
      makeSession({
        id: "live-lock-trashed",
        title: "Trashed During Live",
        deleted: true,
        deleted_at: 1_700_000_000_000,
      }),
    ]);
    useAudioGraphStore.setState({ isCapturing: true });
    render(<SessionsBrowser />);

    await openRowMenu("Active During Live");
    expect(
      await screen.findByRole("button", { name: /^export$/i }),
    ).not.toBeDisabled();
    expect(screen.getByRole("button", { name: /^delete$/i })).toBeDisabled();

    const trashToggle = await screen.findByRole("button", {
      name: /trash \(1\)/i,
    });
    fireEvent.click(trashToggle);
    await openRowMenu("Trashed During Live");
    expect(
      await screen.findByRole("button", { name: /^restore$/i }),
    ).toBeDisabled();
    expect(
      screen.getByRole("button", { name: /delete permanently/i }),
    ).toBeDisabled();
  });
});

// R2 adversary review finding #2 (MAJOR): the lens tab roving keyboard
// interaction had zero test coverage — deleting the ArrowRight handling
// entirely left the full suite green. These exercise ArrowRight/ArrowLeft
// (with wraparound at both ends), Home/End, and the aria-selected +
// tabindex + focus-move contract a roving tablist must uphold.
describe("SessionDetail lens tab roving (SHELL-R2)", () => {
  beforeEach(() => {
    mockedInvoke.mockReset();
    localStorage.clear();
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "list_sessions")
        return useAudioGraphStore.getState().sessions;
      if (cmd === "load_session") {
        return {
          transcript: [],
          graph: {
            nodes: [],
            links: [],
            stats: { total_nodes: 0, total_edges: 0, total_episodes: 0 },
          },
        };
      }
      if (cmd === "purge_expired_sessions") return [];
      return null;
    });
  });

  afterEach(() => {
    useAudioGraphStore.setState({
      sessions: [],
      sessionsLoading: false,
      isCapturing: false,
      isTranscribing: false,
    });
  });

  async function renderWithSelectedSession(): Promise<void> {
    useAudioGraphStore.setState({
      sessions: [makeSession({ id: "lens-1", title: "Lens Session" })],
      sessionsLoading: false,
    });
    render(<SessionsBrowser />);
    const selectBtn = await screen.findByTestId("session-select-lens-1");
    fireEvent.click(selectBtn);
    await screen.findByRole("tablist");
  }

  function tab(name: RegExp): HTMLElement {
    return screen.getByRole("tab", { name });
  }

  it("ArrowRight advances through the lenses in order and wraps from the last back to the first", async () => {
    await renderWithSelectedSession();
    const notesTab = tab(/^notes$/i);
    notesTab.focus();

    fireEvent.keyDown(notesTab, { key: "ArrowRight" });
    expect(tab(/^transcript$/i)).toHaveAttribute("aria-selected", "true");
    expect(tab(/^transcript$/i)).toHaveAttribute("tabindex", "0");
    expect(notesTab).toHaveAttribute("aria-selected", "false");
    expect(notesTab).toHaveAttribute("tabindex", "-1");
    expect(document.activeElement).toBe(tab(/^transcript$/i));

    fireEvent.keyDown(tab(/^transcript$/i), { key: "ArrowRight" });
    expect(tab(/^timeline$/i)).toHaveAttribute("aria-selected", "true");

    fireEvent.keyDown(tab(/^timeline$/i), { key: "ArrowRight" });
    expect(tab(/^graph$/i)).toHaveAttribute("aria-selected", "true");

    fireEvent.keyDown(tab(/^graph$/i), { key: "ArrowRight" });
    expect(tab(/^route$/i)).toHaveAttribute("aria-selected", "true");

    // Wraparound: ArrowRight from the last tab (Route) goes to the first (Notes).
    fireEvent.keyDown(tab(/^route$/i), { key: "ArrowRight" });
    expect(tab(/^notes$/i)).toHaveAttribute("aria-selected", "true");
    expect(tab(/^notes$/i)).toHaveAttribute("tabindex", "0");
    expect(tab(/^route$/i)).toHaveAttribute("aria-selected", "false");
    expect(document.activeElement).toBe(tab(/^notes$/i));
  });

  it("ArrowLeft retreats through the lenses and wraps from the first back to the last", async () => {
    await renderWithSelectedSession();
    const notesTab = tab(/^notes$/i);
    notesTab.focus();

    // Wraparound: ArrowLeft from the first tab (Notes) goes to the last (Route).
    fireEvent.keyDown(notesTab, { key: "ArrowLeft" });
    expect(tab(/^route$/i)).toHaveAttribute("aria-selected", "true");
    expect(tab(/^route$/i)).toHaveAttribute("tabindex", "0");
    expect(notesTab).toHaveAttribute("aria-selected", "false");
    expect(notesTab).toHaveAttribute("tabindex", "-1");
    expect(document.activeElement).toBe(tab(/^route$/i));

    fireEvent.keyDown(tab(/^route$/i), { key: "ArrowLeft" });
    expect(tab(/^graph$/i)).toHaveAttribute("aria-selected", "true");
  });

  it("Home jumps to the first lens and End jumps to the last, from anywhere in the list", async () => {
    await renderWithSelectedSession();
    const notesTab = tab(/^notes$/i);
    notesTab.focus();

    fireEvent.keyDown(notesTab, { key: "End" });
    expect(tab(/^route$/i)).toHaveAttribute("aria-selected", "true");
    expect(tab(/^route$/i)).toHaveAttribute("tabindex", "0");
    expect(document.activeElement).toBe(tab(/^route$/i));

    fireEvent.keyDown(tab(/^route$/i), { key: "Home" });
    expect(tab(/^notes$/i)).toHaveAttribute("aria-selected", "true");
    expect(tab(/^notes$/i)).toHaveAttribute("tabindex", "0");
    expect(document.activeElement).toBe(tab(/^notes$/i));
  });

  // R2 adversary review finding #5 (MINOR): every inactive tab's
  // `aria-controls` pointed at a `sessions-lens-panel-*` id that doesn't
  // exist in the DOM — only the active lens's tabpanel is ever mounted.
  it("only the selected tab's aria-controls points at a real, rendered tabpanel — inactive tabs carry no aria-controls", async () => {
    await renderWithSelectedSession();
    const notesTab = tab(/^notes$/i);
    expect(notesTab).toHaveAttribute(
      "aria-controls",
      "sessions-lens-panel-notes",
    );
    expect(document.getElementById("sessions-lens-panel-notes")).not.toBeNull();

    for (const inactive of [
      tab(/^transcript$/i),
      tab(/^timeline$/i),
      tab(/^graph$/i),
      tab(/^route$/i),
    ]) {
      expect(inactive).not.toHaveAttribute("aria-controls");
    }

    fireEvent.keyDown(notesTab, { key: "ArrowRight" });
    const transcriptTab = tab(/^transcript$/i);
    expect(transcriptTab).toHaveAttribute(
      "aria-controls",
      "sessions-lens-panel-transcript",
    );
    expect(
      document.getElementById("sessions-lens-panel-transcript"),
    ).not.toBeNull();
    expect(notesTab).not.toHaveAttribute("aria-controls");
  });
});
