import {
    describe,
    it,
    expect,
    beforeEach,
    afterEach,
    vi,
} from "vitest";
import {
    render,
    screen,
    fireEvent,
    waitFor,
} from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import SessionsBrowser, { applyFilterAndSort } from "./SessionsBrowser";
import { useAudioGraphStore } from "../store";
import type { SessionMetadata } from "../types";
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
        expect(active.map((s) => s.id)).toEqual([
            "alpha-1",
            "beta-2",
            "gamma-3",
        ]);

        const trash = applyFilterAndSort(sessions, "", "newest", true);
        expect(trash.map((s) => s.id)).toEqual(["trashed-4"]);
    });

    it("filters by case-insensitive substring across title and id", () => {
        expect(
            applyFilterAndSort(sessions, "BET", "newest", false).map(
                (s) => s.id,
            ),
        ).toEqual(["beta-2"]);
        expect(
            applyFilterAndSort(sessions, "gamma-3", "newest", false).map(
                (s) => s.id,
            ),
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
            applyFilterAndSort(sessions, "", "nameAsc", false).map(
                (s) => s.id,
            ),
        ).toEqual(["alpha-1", "beta-2", "gamma-3"]);
        expect(
            applyFilterAndSort(sessions, "", "nameDesc", false).map(
                (s) => s.id,
            ),
        ).toEqual(["gamma-3", "beta-2", "alpha-1"]);
        expect(
            applyFilterAndSort(sessions, "", "largest", false).map(
                (s) => s.id,
            ),
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
            if (cmd === "list_sessions") return useAudioGraphStore.getState().sessions;
            if (cmd === "purge_expired_sessions") return [];
            if (cmd === "delete_session") return null;
            if (cmd === "restore_session") return null;
            if (cmd === "delete_session_permanently") return null;
            return null;
        });
    });

    afterEach(() => {
        // Leave the store in a known state for the next test.
        useAudioGraphStore.setState({ sessions: [], sessionsLoading: false });
    });

    function seed(sessions: SessionMetadata[]): void {
        useAudioGraphStore.setState({ sessions, sessionsLoading: false });
    }

    it("filters by search text live (no submit)", async () => {
        seed([
            makeSession({ id: "alpha-1", title: "Alpha Meeting" }),
            makeSession({ id: "beta-2", title: "Beta Sync" }),
        ]);
        render(<SessionsBrowser />);

        // Both visible initially.
        expect(
            await screen.findByTestId("session-alpha-1"),
        ).toBeInTheDocument();
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

        expect(
            localStorage.getItem("audiograph:sessionsBrowser:sort"),
        ).toBe("nameDesc");
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
        const confirmSpy = vi
            .spyOn(window, "confirm")
            .mockReturnValue(true);

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
});
