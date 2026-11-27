/**
 * Speaker identity panel unit tests (#340): the per-cluster manual identity
 * edits over the backend's session speaker ledger.
 *
 * Pins the contract the backend commands landed in PR #488 expose:
 *   - `list_session_speaker_clusters` list read + badge rendering
 *     (binding state Anonymous/Suggested/Bound/Confirmed, span count,
 *     suggested-self hint),
 *   - camelCase IPC argument casing (`{ sessionId, ... }` — never
 *     `session_id` at the command boundary; records themselves stay
 *     snake_case),
 *   - rename (inline form, Enter submits / Escape cancels / empty name
 *     rejected) + the label re-read from the backend after the edit,
 *   - mark-as-self, merge-into-other, unbind (bound-only affordance),
 *   - error surfacing (`role="alert"`) on a backend rejection,
 *   - action disabling while a mutation is in flight,
 *   - refresh on the `diarization-span-revision` backend event.
 *
 * The Tauri IPC/event modules are mocked by `src/test/setup.ts`
 * (`invoke: vi.fn()`, `listen: async () => () => {}`); tests override both
 * per-case, the same convention as `TokenUsagePanel.test.tsx`.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useAudioGraphStore } from "../store";
import type { ClusterRef, SessionSpeakerCluster } from "../types";
import SpeakerIdentityPanel from "./SpeakerIdentityPanel";
import "../i18n";

const mockedInvoke = vi.mocked(invoke);
const mockedListen = listen as unknown as ReturnType<typeof vi.fn>;

const SESSION_ID = "sess-42";

/** Stable identity of a cluster record (the backend's ClusterRef triple). */
function keyOf(cluster: ClusterRef): string {
  return `${cluster.provider}:${cluster.timeline_id}:${cluster.speaker_id}`;
}

function anonCluster(): SessionSpeakerCluster {
  return {
    suggested_self: false,
    cluster: {
      provider: "local-diarizer",
      timeline_id: "timeline-1",
      speaker_id: "spk-1",
    },
    label: "Speaker 1",
    span_count: 12,
    state: "anonymous",
  };
}

function boundCluster(): SessionSpeakerCluster {
  return {
    suggested_self: true,
    cluster: {
      provider: "local-diarizer",
      timeline_id: "timeline-1",
      speaker_id: "spk-2",
    },
    label: "Alice",
    span_count: 3,
    state: "bound",
  };
}

/**
 * Install a scriptable backend over the mocked `invoke`. `list` serves the
 * current cluster list (fresh copies — the component must not be able to
 * mutate the fixtures); the four mutation commands record their args and
 * apply the edit to the list so the post-mutation re-read shows the change.
 */
function installBackend(initial: SessionSpeakerCluster[]) {
  let current = initial;
  const calls: Array<{ cmd: string; args: Record<string, unknown> }> = [];
  let held: {
    promise: Promise<unknown>;
    resolve: (value: unknown) => void;
  } | null = null;
  let mutError: string | null = null;
  let listError: string | null = null;

  mockedInvoke.mockImplementation(async (cmd: unknown, args?: unknown) => {
    const record = {
      cmd: cmd as string,
      args: (args ?? {}) as Record<string, unknown>,
    };
    calls.push(record);
    switch (cmd) {
      case "list_session_speaker_clusters":
        if (listError !== null) throw new Error(listError);
        return current.map((c) => ({ ...c }));
      case "relabel_speaker_cluster": {
        const a = record.args as unknown as {
          cluster: ClusterRef;
          label: string;
        };
        current = current.map((c) =>
          keyOf(c.cluster) === keyOf(a.cluster) ? { ...c, label: a.label } : c,
        );
        return 1;
      }
      case "mark_cluster_is_self":
      case "merge_speaker_clusters":
      case "unbind_speaker_cluster": {
        // Hold the mutation open until the test releases it (the in-flight
        // disabling assertions need a window where nothing resolved yet).
        if (mutError !== null) {
          const message = mutError;
          mutError = null;
          throw new Error(message);
        }
        if (cmd === "merge_speaker_clusters") {
          const a = record.args as unknown as {
            cluster: ClusterRef;
            into: ClusterRef;
          };
          const target = current.find(
            (c) => keyOf(c.cluster) === keyOf(a.into),
          );
          if (target) {
            current = current.filter(
              (c) => keyOf(c.cluster) !== keyOf(a.cluster),
            );
          }
        }
        if (cmd === "unbind_speaker_cluster") {
          const a = record.args as unknown as { cluster: ClusterRef };
          current = current.map((c) =>
            keyOf(c.cluster) === keyOf(a.cluster)
              ? { ...c, state: "anonymous", suggested_self: false }
              : c,
          );
        }
        if (cmd === "mark_cluster_is_self") {
          const a = record.args as unknown as { cluster: ClusterRef };
          current = current.map((c) =>
            keyOf(c.cluster) === keyOf(a.cluster)
              ? { ...c, state: "user_confirmed" }
              : c,
          );
        }
        return 1;
      }
      default:
        return undefined;
    }
  });

  return {
    calls,
    /** The row `<li>` a cluster label rendered into. */
    rowFor(label: string) {
      const span = screen.getByText(label, { selector: "span" });
      return span.closest("li") as HTMLLIElement;
    },
    listCalls() {
      return calls.filter((c) => c.cmd === "list_session_speaker_clusters");
    },
    commandCalls(cmd: string) {
      return calls.filter((c) => c.cmd === cmd);
    },
    failNextList(message: string) {
      listError = message;
    },
    /** Make the next mutation command reject with `message`. */
    failNextMutation(message: string) {
      mutError = message;
    },
    /** Hold the next mutation open until `release()` (in-flight window). */
    holdNextMutation() {
      // Executor form (not `Promise.withResolvers`): this repo's tsconfig
      // lib is ES2022, which does not type the ES2024 helper.
      let resolve!: (value: unknown) => void;
      const promise = new Promise<unknown>((r) => {
        resolve = r;
      });
      held = { promise, resolve };
    },
    release() {
      held?.resolve(1);
      held = null;
    },
  };
}

interface EventHandlers {
  handlers: Map<string, (event: { payload: unknown }) => void>;
}

function captureListeners(): EventHandlers {
  const handlers = new Map<string, (event: { payload: unknown }) => void>();
  mockedListen.mockImplementation(async (name: unknown, handler: unknown) => {
    handlers.set(
      name as string,
      handler as (event: { payload: unknown }) => void,
    );
    return () => {
      handlers.delete(name as string);
    };
  });
  return { handlers };
}

function seedStore(
  overrides: Partial<ReturnType<typeof useAudioGraphStore.getState>> = {},
) {
  useAudioGraphStore.setState({
    isCapturing: false,
    getSessionId: vi.fn(async () => SESSION_ID),
    ...overrides,
  } as Parameters<typeof useAudioGraphStore.setState>[0]);
}

describe("SpeakerIdentityPanel", () => {
  beforeEach(() => {
    mockedInvoke.mockReset();
    mockedListen.mockReset();
    mockedListen.mockImplementation(async () => () => {});
    seedStore();
  });

  it("renders anonymous and bound clusters with binding-state badges, span counts, and the suggested-self hint", async () => {
    const backend = installBackend([anonCluster(), boundCluster()]);
    render(<SpeakerIdentityPanel />);

    // Region is an accessible landmark by its own label.
    expect(
      screen.getByRole("region", { name: "Speaker identity" }),
    ).toBeInTheDocument();

    await waitFor(() => expect(backend.listCalls().length).toBeGreaterThan(0));
    expect(
      screen.getByText("Speaker 1", { selector: "span" }),
    ).toBeInTheDocument();
    expect(screen.getByText("Alice", { selector: "span" })).toBeInTheDocument();
    expect(screen.getByText("Anonymous")).toBeInTheDocument();
    expect(screen.getByText("Bound")).toBeInTheDocument();
    expect(screen.getByText("12 spans")).toBeInTheDocument();
    expect(screen.getByText("Suggested as you")).toBeInTheDocument();
    expect(screen.getByText("Anonymous").closest("li")).not.toContainHTML(
      "Not this person",
    );
  });

  it("reads the cluster list with camelCase sessionId args (never snake_case)", async () => {
    const backend = installBackend([anonCluster()]);
    render(<SpeakerIdentityPanel />);
    await waitFor(() => expect(backend.listCalls().length).toBeGreaterThan(0));
    const listArgs = backend.listCalls().map((c) => c.args);
    for (const args of listArgs) {
      expect(args.sessionId).toBe(SESSION_ID);
      expect(args.session_id).toBeUndefined();
    }
  });

  it("renames a cluster via relabel_speaker_cluster and re-reads the refreshed label from the backend", async () => {
    const backend = installBackend([anonCluster(), boundCluster()]);
    render(<SpeakerIdentityPanel />);
    await waitFor(() => expect(backend.listCalls().length).toBeGreaterThan(0));

    const row = backend.rowFor("Speaker 1");
    fireEvent.click(within(row).getByRole("button", { name: "Rename" }));

    const input = within(row).getByLabelText("New name for Speaker 1");
    fireEvent.change(input, { target: { value: "Bob" } });
    fireEvent.keyDown(input, { key: "Enter" });

    const relabels = backend.commandCalls("relabel_speaker_cluster");
    expect(relabels).toHaveLength(1);
    expect(relabels[0].args).toEqual({
      sessionId: SESSION_ID,
      cluster: anonCluster().cluster,
      label: "Bob",
    });

    // The display re-reads the ledger — the new label comes back from
    // `list_session_speaker_clusters`, not a local patch.
    await waitFor(() =>
      expect(screen.getByText("Bob", { selector: "span" })).toBeInTheDocument(),
    );
    expect(screen.getByText("Renamed to Bob")).toBeInTheDocument();
    await waitFor(() => expect(backend.listCalls().length).toBeGreaterThan(1));
  });

  it("rejects an empty rename without invoking the backend", async () => {
    const backend = installBackend([anonCluster(), boundCluster()]);
    render(<SpeakerIdentityPanel />);
    await waitFor(() => expect(backend.listCalls().length).toBeGreaterThan(0));

    const row = backend.rowFor("Speaker 1");
    fireEvent.click(within(row).getByRole("button", { name: "Rename" }));
    const input = within(row).getByLabelText("New name for Speaker 1");
    fireEvent.change(input, { target: { value: "   " } });
    fireEvent.keyDown(input, { key: "Enter" });

    expect(screen.getByRole("alert").textContent).toMatch(
      /enter a name before saving/i,
    );
    expect(backend.commandCalls("relabel_speaker_cluster")).toHaveLength(0);
  });

  it("cancels the inline rename form with Escape", async () => {
    const backend = installBackend([anonCluster(), boundCluster()]);
    render(<SpeakerIdentityPanel />);
    await waitFor(() => expect(backend.listCalls().length).toBeGreaterThan(0));

    const row = backend.rowFor("Speaker 1");
    fireEvent.click(within(row).getByRole("button", { name: "Rename" }));
    const input = within(row).getByLabelText("New name for Speaker 1");
    fireEvent.keyDown(input, { key: "Escape" });

    expect(
      within(row).queryByLabelText("New name for Speaker 1"),
    ).not.toBeInTheDocument();
    expect(backend.commandCalls("relabel_speaker_cluster")).toHaveLength(0);
  });

  it("marks a cluster as self via mark_cluster_is_self with camelCase args", async () => {
    const backend = installBackend([anonCluster(), boundCluster()]);
    render(<SpeakerIdentityPanel />);
    await waitFor(() => expect(backend.listCalls().length).toBeGreaterThan(0));

    const row = backend.rowFor("Speaker 1");
    fireEvent.click(within(row).getByRole("button", { name: "This is me" }));

    const marks = backend.commandCalls("mark_cluster_is_self");
    expect(marks).toHaveLength(1);
    expect(marks[0].args).toEqual({
      sessionId: SESSION_ID,
      cluster: anonCluster().cluster,
    });
    await waitFor(() =>
      expect(screen.getByRole("status").textContent).toMatch(/marked as you/i),
    );
  });

  it("merges a cluster into another via merge_speaker_clusters once a target is picked", async () => {
    const backend = installBackend([anonCluster(), boundCluster()]);
    render(<SpeakerIdentityPanel />);
    await waitFor(() => expect(backend.listCalls().length).toBeGreaterThan(0));

    const row = backend.rowFor("Speaker 1");
    const select = within(row).getByRole("combobox", {
      name: "Same as which other speaker?",
    });
    // Merge stays disabled until a target is selected.
    const mergeButton = within(row).getByRole("button", { name: "Merge" });
    expect(mergeButton).toBeDisabled();

    fireEvent.change(select, {
      target: { value: keyOf(boundCluster().cluster) },
    });
    expect(mergeButton).toBeEnabled();
    fireEvent.click(mergeButton);

    const merges = backend.commandCalls("merge_speaker_clusters");
    expect(merges).toHaveLength(1);
    expect(merges[0].args).toEqual({
      sessionId: SESSION_ID,
      cluster: anonCluster().cluster,
      into: boundCluster().cluster,
    });
    await waitFor(() =>
      expect(screen.getByRole("status").textContent).toMatch(
        /clusters merged/i,
      ),
    );
  });

  it("offers unbind only for bound/user-confirmed clusters and unbinds with camelCase args", async () => {
    const backend = installBackend([anonCluster(), boundCluster()]);
    render(<SpeakerIdentityPanel />);
    await waitFor(() => expect(backend.listCalls().length).toBeGreaterThan(0));

    const anonRow = backend.rowFor("Speaker 1");
    expect(
      within(anonRow).queryByRole("button", { name: "Not this person" }),
    ).not.toBeInTheDocument();

    const boundRow = backend.rowFor("Alice");
    fireEvent.click(
      within(boundRow).getByRole("button", { name: "Not this person" }),
    );

    const unbinds = backend.commandCalls("unbind_speaker_cluster");
    expect(unbinds).toHaveLength(1);
    expect(unbinds[0].args).toEqual({
      sessionId: SESSION_ID,
      cluster: boundCluster().cluster,
    });
    await waitFor(() =>
      expect(screen.getByRole("status").textContent).toMatch(
        /binding removed/i,
      ),
    );
  });

  it("surfaces a backend rejection in an alert region", async () => {
    const backend = installBackend([anonCluster(), boundCluster()]);
    backend.failNextMutation("set your display name first");
    render(<SpeakerIdentityPanel />);
    await waitFor(() => expect(backend.listCalls().length).toBeGreaterThan(0));

    const row = backend.rowFor("Speaker 1");
    fireEvent.click(within(row).getByRole("button", { name: "This is me" }));

    await waitFor(() =>
      expect(screen.getByRole("alert").textContent).toMatch(
        /set your display name first/i,
      ),
    );
  });

  it("disables every action while a mutation is in flight", async () => {
    const backend = installBackend([anonCluster(), boundCluster()]);
    backend.holdNextMutation();
    render(<SpeakerIdentityPanel />);
    await waitFor(() => expect(backend.listCalls().length).toBeGreaterThan(0));

    const row = backend.rowFor("Speaker 1");
    fireEvent.click(within(row).getByRole("button", { name: "This is me" }));

    // The mutation is pending: every action button in the panel is disabled.
    // Re-query fresh inside `waitFor` — the busy re-render replaces the
    // button nodes, so a ref captured before the click would go stale.
    await waitFor(() => {
      const busyRow = backend.rowFor("Speaker 1");
      expect(
        within(busyRow).getByRole("button", { name: "Rename" }),
      ).toBeDisabled();
      expect(
        within(busyRow).getByRole("button", { name: "This is me" }),
      ).toBeDisabled();
    });

    backend.release();
    await waitFor(() =>
      expect(
        within(backend.rowFor("Speaker 1")).getByRole("button", {
          name: "This is me",
        }),
      ).toBeEnabled(),
    );
  });

  it("re-reads the cluster list when the backend emits a diarization-span-revision", async () => {
    const backend = installBackend([anonCluster()]);
    const { handlers } = captureListeners();
    render(<SpeakerIdentityPanel />);
    await waitFor(() => expect(backend.listCalls().length).toBeGreaterThan(0));
    const listsBefore = backend.listCalls().length;

    act(() => {
      const handler = handlers.get("diarization-span-revision");
      expect(handler).toBeDefined();
      handler?.({ payload: { span_id: "s1", revision_number: 2 } });
    });

    await waitFor(() =>
      expect(backend.listCalls().length).toBeGreaterThan(listsBefore),
    );
  });

  it("renders the idle empty state and issues no cluster commands while there is no session", async () => {
    seedStore({ getSessionId: vi.fn(async () => "") });
    const backend = installBackend([anonCluster()]);
    render(<SpeakerIdentityPanel />);

    await waitFor(() =>
      expect(
        screen.getByText(/start or load a session to name its speakers/i),
      ).toBeInTheDocument(),
    );
    expect(backend.calls).toHaveLength(0);
  });

  it("surfaces a failed cluster-list read in an alert region", async () => {
    const backend = installBackend([anonCluster()]);
    backend.failNextList("speaker session has changed");
    render(<SpeakerIdentityPanel />);

    await waitFor(() =>
      expect(screen.getByRole("alert").textContent).toMatch(
        /speaker session has changed/i,
      ),
    );
    // Still a usable surface: the empty copy renders alongside the error.
    expect(
      screen.getByText(/no speaker clusters in this session yet/i),
    ).toBeInTheDocument();
  });
});
