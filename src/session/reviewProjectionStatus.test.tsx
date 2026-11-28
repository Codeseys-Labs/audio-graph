import { cleanup, render, screen } from "@testing-library/react";
import { afterAll, afterEach, beforeEach, expect, it, vi } from "vitest";
import * as ipc from "../analytics/safeInvoke";
import ReviewLensGate from "../components/ReviewLensGate";
import i18n from "../i18n";
import { useReviewStore } from "./reviewStore";
import { SessionViewProvider } from "./SessionViewProvider";

const backend = vi.spyOn(ipc, "safeInvoke");
afterAll(() => backend.mockRestore());

afterEach(async () => {
  cleanup();
  useReviewStore.getState().close();
  await i18n.changeLanguage("en");
});

beforeEach(async () => {
  backend.mockReset();
  useReviewStore.getState().close();
  await i18n.changeLanguage("en");
});

function pane(kind: "notes" | "graph", session: string) {
  return (
    <SessionViewProvider reviewSessionId={session} reviewLens={kind}>
      <ReviewLensGate kind={kind}>
        <span>{session}</span>
      </ReviewLensGate>
    </SessionViewProvider>
  );
}

function show(kind: "notes" | "graph", fields: object) {
  const command =
    kind === "notes"
      ? "load_session_notes_artifacts_cmd"
      : "load_session_graph_artifact_cmd";
  backend.mockImplementation(async (name, args) => {
    if (name === "load_session")
      return {
        transcript: [],
        transcript_events: [],
        diarization_events: [],
        graph: {
          nodes: [],
          links: [],
          stats: { total_nodes: 0, total_edges: 0, total_episodes: 0 },
        },
      };
    if (name === command) {
      const session = args && "sessionId" in args ? args.sessionId : null;
      if (session === "untouched")
        return kind === "notes" ? { notes: null, projection_events: [] } : null;
      const statuses = session === "active" ? {} : fields;
      return kind === "notes"
        ? { notes: { notes: [], ...statuses }, projection_events: [] }
        : { nodes: [], edges: [], ...statuses };
    }
    throw new Error("unexpected command");
  });
  return render(pane(kind, "orchard"));
}

it("shows that historical notes trail the transcript", async () => {
  show("notes", { transcript_coverage: "behind_transcript" });
  await screen.findByText("orchard");
  expect(screen.getByRole("status")).toHaveTextContent(
    i18n.t("sessions.review.projectionBehindTranscript"),
  );
  expect(screen.queryByRole("button")).toBeNull();
});

it("shows a historical graph gap in Portuguese", async () => {
  await i18n.changeLanguage("pt");
  show("graph", { projection_recovery: "recompute_required" });
  await screen.findByText("orchard");
  expect(screen.getByRole("status")).toHaveTextContent(
    "Não foi possível recuperar uma alteração da projeção.",
  );
  expect(screen.queryByRole("button")).toBeNull();
});

it("reads recovery state in the same lens round and ignores unknown diagnostics", async () => {
  show("notes", {
    projection_recovery: "fern",
    transcript_coverage: "behind_transcript",
  });
  await screen.findByText("orchard");
  expect(screen.getByRole("status")).toHaveTextContent(
    i18n.t("sessions.review.projectionBehindTranscript"),
  );
  expect(
    useReviewStore.getState().artifacts.notes.data?.notes,
  ).not.toHaveProperty("projection_recovery");
  expect(screen.queryByText(/fern/)).toBeNull();
  expect(backend.mock.calls.map(([name]) => name).sort()).toEqual([
    "load_session",
    "load_session_notes_artifacts_cmd",
  ]);
});

it("omits unknown projection diagnostics from the lens slot and the rendered view", async () => {
  show("graph", { projection_recovery: "reed", transcript_coverage: "fern" });
  await screen.findByText("orchard");
  const graph = useReviewStore.getState().artifacts.graph.data;
  expect(graph).not.toHaveProperty("projection_recovery");
  expect(graph).not.toHaveProperty("transcript_coverage");
  expect(screen.queryByRole("status")).toBeNull();
});

it("keeps both verified lane notices in the same published artifact", async () => {
  show("graph", {
    transcript_coverage: "behind_transcript",
    projection_recovery: "recompute_required",
  });
  await screen.findByText("orchard");
  expect(
    useReviewStore.getState().artifacts.graph.data?.reviewProjectionStatuses,
  ).toEqual(["behind_transcript", "recompute_required"]);
  expect(screen.getByRole("status")).toHaveTextContent(
    i18n.t("sessions.review.projectionBehindTranscript"),
  );
  expect(screen.getByRole("status")).toHaveTextContent(
    i18n.t("sessions.review.projectionGap"),
  );
});

it("renders the verified view without a notice for a normal legacy reply", async () => {
  show("notes", {});
  await screen.findByText("orchard");
  expect(screen.queryByRole("status")).toBeNull();
});

it("replaces the notice with its own round when an active or untouched session is selected", async () => {
  const view = show("notes", { transcript_coverage: "behind_transcript" });
  await screen.findByText("orchard");
  expect(screen.getByRole("status")).toHaveTextContent(
    i18n.t("sessions.review.projectionBehindTranscript"),
  );
  for (const session of ["active", "untouched"]) {
    view.rerender(pane("notes", session));
    await screen.findByText(session);
    expect(screen.queryByRole("status")).toBeNull();
  }
  view.rerender(pane("notes", "orchard"));
  await screen.findByText("orchard");
  expect(screen.getByRole("status")).toHaveTextContent(
    i18n.t("sessions.review.projectionBehindTranscript"),
  );
  expect(backend).toHaveBeenCalledTimes(8);
});

it.each([
  [
    "notes",
    "en",
    "Part of the source for these notes and graph changes is unavailable.",
  ],
  [
    "graph",
    "pt",
    "Parte da fonte destas alterações nas notas e no grafo está indisponível.",
  ],
] as const)("shows one missing-source notice on the %s lens in %s", async (lens, language, expected) => {
  await i18n.changeLanguage(language);
  show(lens, {
    basis_unavailable: { first_sequence: 2, last_sequence: 4 },
    transcript_coverage: "behind_transcript",
    projection_recovery: "recompute_required",
  });
  await screen.findByText("orchard");
  expect(screen.getAllByRole("status")).toHaveLength(1);
  expect(screen.getByRole("status")).toHaveTextContent(expected);
  expect(screen.getByRole("status")).toHaveTextContent(
    i18n.t("sessions.review.projectionGap"),
  );
  expect(backend).toHaveBeenCalledTimes(2);
  expect(screen.queryByRole("button")).toBeNull();
});
