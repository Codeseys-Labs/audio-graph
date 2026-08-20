import { invoke } from "@tauri-apps/api/core";
import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FIXTURE_FINALIZATION_STATUSES } from "../fixtures/reviewFinalizationFixtures";
import { useAudioGraphStore } from "../store";
import type { SessionMetadata } from "../types";
import SessionsBrowser, { applyFilterAndSort } from "./SessionsBrowser";
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

  it("uses design tokens for borders — no ghost var(--border,#333) fallbacks (d19f)", async () => {
    seed([makeSession({ id: "tokens-1", title: "Token Session" })]);
    render(<SessionsBrowser />);

    const item = await screen.findByTestId("session-tokens-1");
    // Migrated from inline style={{ border: "1px solid var(--border,#333)" }}
    // to the ADR-0016 token-bridged Tailwind border utility.
    expect(item).toHaveClass("border-border-color");
    expect(item.getAttribute("style")).toBeFalsy();

    const searchBox = screen.getByRole("searchbox");
    expect(searchBox).toHaveClass("border-border-color");
    expect(searchBox.getAttribute("style")).toBeFalsy();

    const sortSelect = screen.getByLabelText(/sort by/i);
    expect(sortSelect).toHaveClass("border-border-color");

    // No element in the modal may carry a ghost #333 border fallback anymore.
    const modal = screen.getByRole("dialog");
    for (const el of modal.querySelectorAll<HTMLElement>("*")) {
      expect(el.getAttribute("style") ?? "").not.toContain("#333");
    }
  });

  it("closes on Escape from a focused dialog descendant", async () => {
    seed([makeSession({ id: "escape-1", title: "Escape Session" })]);
    useAudioGraphStore.setState({ sessionsBrowserOpen: true });
    render(<SessionsBrowser />);

    const search = await screen.findByRole("searchbox");
    search.focus();
    expect(search).toHaveFocus();
    fireEvent.keyDown(search, { key: "Escape" });

    expect(useAudioGraphStore.getState().sessionsBrowserOpen).toBe(false);
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

    const restoreBtn = screen.getByRole("button", { name: /restore/i });
    fireEvent.click(restoreBtn);

    await waitFor(() => {
      expect(mockedInvoke).toHaveBeenCalledWith("restore_session", {
        sessionId: "to-restore",
      });
    });
  });

  it("loads both transcript and graph for a session", async () => {
    seed([makeSession({ id: "load-me", title: "Load Me" })]);
    useAudioGraphStore.setState({
      transcriptSegments: [],
      graphSnapshot: {
        nodes: [],
        links: [],
        stats: { total_nodes: 0, total_edges: 0, total_episodes: 0 },
      },
      sessionsBrowserOpen: true,
    });
    render(<SessionsBrowser />);

    const loadBtn = await screen.findByRole("button", { name: /^load$/i });
    fireEvent.click(loadBtn);

    await waitFor(() => {
      expect(mockedInvoke).toHaveBeenCalledWith("load_session", {
        sessionId: "load-me",
      });
    });
    expect(useAudioGraphStore.getState().transcriptSegments).toHaveLength(1);
    expect(useAudioGraphStore.getState().graphSnapshot.stats.total_nodes).toBe(
      1,
    );
    expect(useAudioGraphStore.getState().rightPanelTab).toBe("transcript");
    expect(useAudioGraphStore.getState().sessionsBrowserOpen).toBe(false);
  });

  it("keeps historical session loading disabled during live capture", async () => {
    seed([makeSession({ id: "past-session", title: "Past Session" })]);
    useAudioGraphStore.setState({
      isCapturing: true,
      isTranscribing: true,
      sessionsBrowserOpen: true,
    });

    render(<SessionsBrowser />);

    const notice = await screen.findByText(
      /stop the live capture before opening a past session/i,
    );
    expect(notice).toHaveAttribute("role", "status");
    const loadButton = screen.getByRole("button", { name: /^load$/i });
    expect(loadButton).toBeDisabled();
    fireEvent.click(loadButton);
    expect(mockedInvoke).not.toHaveBeenCalledWith(
      "load_session",
      expect.anything(),
    );
    expect(useAudioGraphStore.getState().sessionsBrowserOpen).toBe(true);
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
});

describe("SessionsBrowser — Finalizing / Finalization Blocked prototype (audio-graph-1d92)", () => {
  beforeEach(() => {
    mockedInvoke.mockReset();
    localStorage.clear();
  });

  afterEach(() => {
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

  function mockFinalizationInvoke(
    extra?: (cmd: string, args: unknown) => unknown,
  ) {
    mockedInvoke.mockImplementation(async (cmd: string, args?: unknown) => {
      if (cmd === "get_session_finalization_status_cmd") {
        const sessionId = (args as { sessionId: string })?.sessionId;
        return FIXTURE_FINALIZATION_STATUSES[sessionId] ?? null;
      }
      if (cmd === "list_sessions")
        return useAudioGraphStore.getState().sessions;
      if (extra) return extra(cmd, args);
      return null;
    });
  }

  it("shows a per-row finalization pill derived fresh from the fetched status (Q2 default: list + detail)", async () => {
    mockFinalizationInvoke();
    seed([
      makeSession({ id: "fx-finalizing", title: "Finalizing session" }),
      makeSession({ id: "fx-blocked-external", title: "Blocked session" }),
    ]);
    render(<SessionsBrowser />);

    await waitFor(() =>
      expect(
        screen.getByTestId("finalization-pill-fx-finalizing"),
      ).toHaveTextContent(/finalizing/i),
    );
    expect(
      screen.getByTestId("finalization-pill-fx-blocked-external"),
    ).toHaveTextContent(/blocked/i);
  });

  it("one session's Finalization Blocked record never degrades another row (ADR-0035's core point)", async () => {
    mockFinalizationInvoke();
    seed([
      makeSession({ id: "fx-blocked-external", title: "Blocked session" }),
      makeSession({ id: "clean-session", title: "Unrelated clean session" }),
    ]);
    render(<SessionsBrowser />);

    await waitFor(() =>
      expect(
        screen.getByTestId("finalization-pill-fx-blocked-external"),
      ).toHaveTextContent(/blocked/i),
    );
    // The unrelated session has no finalization fixture at all — no pill,
    // normal status badge, and its own Load/Delete stay fully independent.
    expect(
      screen.queryByTestId("finalization-pill-clean-session"),
    ).not.toBeInTheDocument();
    const cleanItem = screen.getByTestId("session-clean-session");
    expect(
      within(cleanItem).getByRole("button", { name: /^load$/i }),
    ).not.toBeDisabled();
    expect(
      within(cleanItem).getByRole("button", { name: /^delete$/i }),
    ).not.toBeDisabled();
  });

  it("the list-level Retry is never gated by reviewLocked, so a background session stays retryable while another Session is Live (Q5 default)", async () => {
    mockFinalizationInvoke((cmd) => {
      if (cmd === "retry_session_finalization_cmd") {
        return {
          ...FIXTURE_FINALIZATION_STATUSES["fx-blocked-external"],
          blocked_record: null,
        };
      }
      return null;
    });
    seed([makeSession({ id: "fx-blocked-external", title: "Blocked" })]);
    useAudioGraphStore.setState({ isCapturing: true, isTranscribing: true });
    const user = userEvent.setup();
    render(<SessionsBrowser />);

    const retryBtn = await screen.findByTestId(
      "finalization-retry-fx-blocked-external",
    );
    expect(retryBtn).not.toBeDisabled();
    await user.click(retryBtn);

    await waitFor(() =>
      expect(mockedInvoke).toHaveBeenCalledWith(
        "retry_session_finalization_cmd",
        { sessionId: "fx-blocked-external", authorizeCostAndEgress: false },
      ),
    );
    await waitFor(() =>
      expect(
        screen.getByTestId("finalization-pill-fx-blocked-external"),
      ).not.toHaveTextContent(/blocked/i),
    );
    // Load stays disabled the whole time — the coarse lock is untouched.
    const loadBtn = screen.getByRole("button", { name: /^load$/i });
    expect(loadBtn).toBeDisabled();
  });

  it("list surface variant 'detailOnly' hides every row pill without fetching finalization data", async () => {
    mockFinalizationInvoke();
    seed([makeSession({ id: "fx-blocked-external", title: "Blocked" })]);
    render(<SessionsBrowser />);

    const surfaceSelect = await screen.findByTestId(
      "sessions-variant-list-surface",
    );
    fireEvent.change(surfaceSelect, { target: { value: "detailOnly" } });

    await waitFor(() =>
      expect(
        screen.queryByTestId("finalization-pill-fx-blocked-external"),
      ).not.toBeInTheDocument(),
    );
  });

  it("background access variant 'perSessionLoadGate' loads a non-active background session even while another Session is Live", async () => {
    mockFinalizationInvoke();
    seed([
      makeSession({ id: "active-now", title: "Live now", status: "active" }),
      makeSession({ id: "fx-blocked-external", title: "Background" }),
    ]);
    useAudioGraphStore.setState({ isCapturing: true, isTranscribing: true });
    render(<SessionsBrowser />);

    const accessSelect = await screen.findByTestId(
      "sessions-variant-background-access",
    );
    fireEvent.change(accessSelect, { target: { value: "perSessionLoadGate" } });

    const activeItem = screen.getByTestId("session-active-now");
    const backgroundItem = screen.getByTestId("session-fx-blocked-external");
    expect(
      within(activeItem).getByRole("button", { name: /^load$/i }),
    ).toBeDisabled();
    expect(
      within(backgroundItem).getByRole("button", { name: /^load$/i }),
    ).not.toBeDisabled();
  });
});
