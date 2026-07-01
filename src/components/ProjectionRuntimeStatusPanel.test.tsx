import { invoke } from "@tauri-apps/api/core";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useAudioGraphStore } from "../store";
import type {
  ProjectionPatch,
  ProjectionReplayReport,
  ProjectionRuntimeStatus,
  ProjectionSchedulerTelemetry,
} from "../types";
import ProjectionRuntimeStatusPanel from "./ProjectionRuntimeStatusPanel";
import "../i18n";

const mockedInvoke = vi.mocked(invoke);

function scheduler(
  kind: ProjectionSchedulerTelemetry["kind"],
  overrides: Partial<ProjectionSchedulerTelemetry> = {},
): ProjectionSchedulerTelemetry {
  return {
    kind,
    ttft_estimate_ms: 350,
    ttft_estimate_source: "default",
    in_flight_job_id: null,
    in_flight_age_ms: 0,
    in_flight_span_count: 0,
    pending_span_count: 0,
    metrics: {
      jobs_started: 0,
      completed_jobs: 0,
      failed_jobs: 0,
      generation_failures: 0,
      coalesced_updates: 0,
      coalesced_span_count: 0,
      stale_discards: 0,
      repair_jobs_started: 0,
      follow_up_jobs_started: 0,
      accepted_patches: 0,
      apply_failures: 0,
      tokens_used: 0,
      last_job_lag_ms: 0,
      max_job_lag_ms: 0,
      last_generation_latency_ms: 0,
      max_generation_latency_ms: 0,
      last_apply_latency_ms: 0,
      max_apply_latency_ms: 0,
      ...(overrides.metrics ?? {}),
    },
    ...overrides,
  };
}

function status(
  overrides: Partial<ProjectionRuntimeStatus> = {},
): ProjectionRuntimeStatus {
  return {
    session_id: "session-1",
    ledger_session_id: "session-1",
    materialized_session_id: "session-1",
    accepted_transcript_event_count: 0,
    transcript_span_count: 0,
    latest_asr_event_age_ms: null,
    projection_event_writer_available: true,
    schedulers: {
      notes: scheduler("notes"),
      graph: scheduler("graph"),
    },
    materialized: {
      notes_last_sequence: 0,
      note_count: 0,
      graph_last_sequence: 0,
      graph_node_count: 0,
      graph_edge_count: 0,
    },
    ...overrides,
  };
}

function stageLatency(maxMs: number, totalMs = maxMs, measuredCount = 1) {
  return {
    measured_count: measuredCount,
    total_ms: totalMs,
    max_ms: maxMs,
  };
}

function replayKindLatency(
  overrides: Partial<ProjectionReplayReport["latency"]["notes"]> = {},
): ProjectionReplayReport["latency"]["notes"] {
  return {
    patch_count: 1,
    measured_patch_count: 1,
    missing_basis_timestamp_count: 0,
    total_basis_to_patch_lag_ms: 400,
    max_basis_to_patch_lag_ms: 400,
    capture_asr: stageLatency(70),
    asr_to_queue: stageLatency(200),
    projection_queue: stageLatency(80),
    generation: stageLatency(60),
    apply: stageLatency(20),
    ...overrides,
  };
}

function replayReport(
  overrides: Partial<ProjectionReplayReport> = {},
): ProjectionReplayReport {
  return {
    session_id: "session-1",
    transcript_event_count: 4,
    transcript_replay_error: null,
    transcript_span_count: 3,
    projection_event_count: 2,
    projection_checked_patch_count: 2,
    projection_invalid_basis_count: 0,
    projection_replay_error: null,
    replayed: {
      notes_last_sequence: 2,
      note_count: 2,
      graph_last_sequence: 2,
      graph_node_count: 1,
      graph_edge_count: 1,
    },
    notes_artifact: {
      present: true,
      status: "stale",
      stored_last_sequence: 1,
      replayed_last_sequence: 2,
      stored_item_count: 1,
      replayed_item_count: 2,
    },
    graph_artifact: {
      present: true,
      status: "current",
      stored_last_sequence: 2,
      replayed_last_sequence: 2,
      stored_item_count: 2,
      replayed_item_count: 2,
    },
    evaluation: {
      note_operation_count: 2,
      graph_operation_count: 6,
      graph_retcon_operation_count: 1,
      correction_patch_count: 1,
      stale_discard_count: 0,
      invalidated_graph_node_count: 1,
      invalidated_graph_edge_count: 1,
      active_graph_node_count: 2,
      active_graph_edge_count: 1,
      duplicate_active_node_key_count: 0,
      duplicate_active_edge_key_count: 0,
    },
    latency: {
      patch_count: 2,
      measured_patch_count: 2,
      missing_basis_timestamp_count: 0,
      total_basis_to_patch_lag_ms: 900,
      max_basis_to_patch_lag_ms: 500,
      capture_asr: stageLatency(70, 120, 2),
      asr_to_queue: stageLatency(200, 350, 2),
      projection_queue: stageLatency(80, 150, 2),
      generation: stageLatency(60, 110, 2),
      apply: stageLatency(20, 35, 2),
      notes: replayKindLatency(),
      graph: replayKindLatency({
        total_basis_to_patch_lag_ms: 500,
        max_basis_to_patch_lag_ms: 500,
      }),
    },
    ...overrides,
  };
}

describe("ProjectionRuntimeStatusPanel", () => {
  beforeEach(() => {
    mockedInvoke.mockReset();
    useAudioGraphStore.setState({
      samplePreviewActive: false,
      pipelineLatencies: {},
      sessionProjectionEvents: [],
    });
  });

  it("shows a loading state while projection telemetry is requested", () => {
    mockedInvoke.mockReturnValue(new Promise(() => {}));
    render(<ProjectionRuntimeStatusPanel />);
    expect(screen.getByText(/loading projection status/i)).toBeInTheDocument();
    expect(mockedInvoke).toHaveBeenCalledWith(
      "get_projection_runtime_status_cmd",
    );
  });

  it("renders the empty diagnostics state without transcript text", async () => {
    mockedInvoke.mockResolvedValueOnce(
      status({
        // Unknown backend fields should not be reflected by this diagnostics UI.
        transcript_text: "do not render this",
        api_key: "sk-do-not-render",
      } as Partial<ProjectionRuntimeStatus>),
    );

    render(<ProjectionRuntimeStatusPanel />);

    expect(
      await screen.findByText(/no accepted transcript spans yet/i),
    ).toBeInTheDocument();
    expect(screen.getAllByText("0").length).toBeGreaterThan(0);
    expect(screen.queryByText("do not render this")).not.toBeInTheDocument();
    expect(screen.queryByText("sk-do-not-render")).not.toBeInTheDocument();
  });

  it("surfaces in-flight queue state and materialized counts", async () => {
    mockedInvoke.mockResolvedValueOnce(
      status({
        accepted_transcript_event_count: 9,
        transcript_span_count: 4,
        latest_asr_event_age_ms: 1750,
        schedulers: {
          notes: scheduler("notes", {
            ttft_estimate_source: "observed_generation",
            in_flight_job_id: "notes-job-1",
            in_flight_age_ms: 2500,
            in_flight_span_count: 3,
            pending_span_count: 2,
            metrics: {
              jobs_started: 1,
              completed_jobs: 0,
              failed_jobs: 0,
              generation_failures: 0,
              coalesced_updates: 2,
              coalesced_span_count: 3,
              stale_discards: 0,
              repair_jobs_started: 0,
              follow_up_jobs_started: 0,
              accepted_patches: 1,
              apply_failures: 0,
              tokens_used: 84,
              last_job_lag_ms: 1280,
              max_job_lag_ms: 1280,
              last_generation_latency_ms: 640,
              max_generation_latency_ms: 640,
              last_apply_latency_ms: 32,
              max_apply_latency_ms: 32,
            },
          }),
          graph: scheduler("graph"),
        },
        materialized: {
          notes_last_sequence: 7,
          note_count: 5,
          graph_last_sequence: 6,
          graph_node_count: 3,
          graph_edge_count: 2,
        },
      }),
    );
    useAudioGraphStore.setState({
      pipelineLatencies: {
        capture: {
          stage: "capture",
          latency_ms: 18,
          timestamp_ms: 1_700_000_000_001,
        },
        asr: {
          stage: "asr",
          latency_ms: 240,
          source_id: "system-default",
          segment_id: "span-4",
          timestamp_ms: 1_700_000_000_002,
        },
      },
    });

    render(<ProjectionRuntimeStatusPanel />);

    const region = await screen.findByRole("region", {
      name: /projection diagnostics/i,
    });
    expect(within(region).getByText(/in flight/i)).toBeInTheDocument();
    expect(within(region).getByText(/notes-job-1/i)).toBeInTheDocument();
    expect(within(region).getByText("9")).toBeInTheDocument();
    expect(within(region).getByText("84")).toBeInTheDocument();
    expect(
      within(region).getAllByText(/coalesced spans/i).length,
    ).toBeGreaterThan(0);
    expect(within(region).getAllByText("3").length).toBeGreaterThan(0);
    expect(within(region).getByText(/capture latency/i)).toBeInTheDocument();
    expect(within(region).getByText("18ms")).toBeInTheDocument();
    expect(within(region).getByText(/asr latency/i)).toBeInTheDocument();
    expect(within(region).getByText("240ms")).toBeInTheDocument();
    expect(within(region).getByText(/asr event age/i)).toBeInTheDocument();
    expect(within(region).getByText("1.8s")).toBeInTheDocument();
    expect(within(region).getAllByText(/queue age/i).length).toBeGreaterThan(0);
    expect(within(region).getByText("2.5s")).toBeInTheDocument();
    expect(within(region).getAllByText(/ttft source/i).length).toBeGreaterThan(
      0,
    );
    expect(within(region).getByText(/observed/i)).toBeInTheDocument();
    expect(within(region).getAllByText(/llm gen/i).length).toBeGreaterThan(0);
    expect(within(region).getByText("640ms")).toBeInTheDocument();
    expect(within(region).getByText("32ms")).toBeInTheDocument();
    expect(within(region).getByText(/3 nodes \/ 2 edges/i)).toBeInTheDocument();
  });

  it("shows failed and stale scheduler telemetry as attention state", async () => {
    mockedInvoke.mockResolvedValueOnce(
      status({
        accepted_transcript_event_count: 3,
        transcript_span_count: 3,
        schedulers: {
          notes: scheduler("notes", {
            metrics: {
              jobs_started: 5,
              completed_jobs: 2,
              failed_jobs: 2,
              generation_failures: 1,
              coalesced_updates: 1,
              coalesced_span_count: 4,
              stale_discards: 3,
              repair_jobs_started: 1,
              follow_up_jobs_started: 1,
              accepted_patches: 2,
              apply_failures: 1,
              tokens_used: 144,
              last_job_lag_ms: 50,
              max_job_lag_ms: 500,
              last_generation_latency_ms: 40,
              max_generation_latency_ms: 400,
              last_apply_latency_ms: 10,
              max_apply_latency_ms: 50,
            },
          }),
          graph: scheduler("graph"),
        },
      }),
    );

    render(<ProjectionRuntimeStatusPanel />);

    const notes = await screen.findByRole("article", {
      name: /notes queue/i,
    });
    expect(within(notes).getByText(/needs attention/i)).toBeInTheDocument();
    expect(within(notes).getByText("Failed")).toBeInTheDocument();
    expect(within(notes).getAllByText("2").length).toBeGreaterThanOrEqual(2);
    expect(within(notes).getByText("Stale")).toBeInTheDocument();
    expect(within(notes).getByText("3")).toBeInTheDocument();
  });

  it("warns when projection patch persistence is unavailable", async () => {
    mockedInvoke.mockResolvedValueOnce(
      status({
        projection_event_writer_available: false,
      }),
    );

    render(<ProjectionRuntimeStatusPanel />);

    expect(
      await screen.findByText(/patch writer unavailable/i),
    ).toBeInTheDocument();
  });

  it("refreshes telemetry on demand", async () => {
    mockedInvoke.mockResolvedValueOnce(status()).mockResolvedValueOnce(
      status({
        accepted_transcript_event_count: 12,
        transcript_span_count: 6,
      }),
    );

    const user = userEvent.setup();
    render(<ProjectionRuntimeStatusPanel />);
    await screen.findByText(/no accepted transcript spans yet/i);

    await user.click(
      screen.getByRole("button", { name: /refresh projection diagnostics/i }),
    );

    await waitFor(() => expect(mockedInvoke).toHaveBeenCalledTimes(2));
    expect(await screen.findByText("12")).toBeInTheDocument();
  });

  it("runs a manual replay parity report for the current session", async () => {
    mockedInvoke.mockImplementation(async (cmd: string, args?: unknown) => {
      if (cmd === "get_projection_runtime_status_cmd") {
        return status({
          accepted_transcript_event_count: 4,
          transcript_span_count: 3,
          materialized: {
            notes_last_sequence: 1,
            note_count: 1,
            graph_last_sequence: 2,
            graph_node_count: 1,
            graph_edge_count: 1,
          },
        });
      }
      if (cmd === "get_projection_replay_report_cmd") {
        expect(args).toEqual({ sessionId: "session-1" });
        return replayReport();
      }
      return undefined;
    });

    const user = userEvent.setup();
    render(<ProjectionRuntimeStatusPanel />);

    const replay = await screen.findByRole("article", {
      name: /replay parity/i,
    });
    expect(within(replay).getByText(/not checked/i)).toBeInTheDocument();

    await user.click(
      within(replay).getByRole("button", {
        name: /check projection replay parity/i,
      }),
    );

    await waitFor(() =>
      expect(mockedInvoke).toHaveBeenCalledWith(
        "get_projection_replay_report_cmd",
        { sessionId: "session-1" },
      ),
    );
    expect(within(replay).getByText(/notes artifact/i)).toBeInTheDocument();
    expect(within(replay).getByText(/invalid basis/i)).toBeInTheDocument();
    expect(within(replay).getByText("Stale")).toBeInTheDocument();
    expect(within(replay).getByText(/graph artifact/i)).toBeInTheDocument();
    expect(within(replay).getByText(/current/i)).toBeInTheDocument();
    expect(within(replay).getByText(/seq 1\/2/i)).toBeInTheDocument();
    expect(within(replay).getByText(/graph ops/i)).toBeInTheDocument();
    expect(within(replay).getByText(/retcons/i)).toBeInTheDocument();
    expect(within(replay).getByText(/dup nodes/i)).toBeInTheDocument();
    expect(within(replay).getByText(/ASR to patch avg/i)).toBeInTheDocument();
    expect(within(replay).getByText(/ASR to patch max/i)).toBeInTheDocument();
    expect(within(replay).getByText(/Capture\+ASR max/i)).toBeInTheDocument();
    expect(within(replay).getByText(/ASR to queue max/i)).toBeInTheDocument();
    expect(within(replay).getByText("Queue max")).toBeInTheDocument();
    expect(within(replay).getByText(/LLM gen max/i)).toBeInTheDocument();
    expect(within(replay).getByText(/Apply max/i)).toBeInTheDocument();
    expect(within(replay).getByText("450ms")).toBeInTheDocument();
    expect(within(replay).getAllByText("500ms").length).toBeGreaterThan(0);
    expect(within(replay).getByText("70ms")).toBeInTheDocument();
    expect(within(replay).getByText("200ms")).toBeInTheDocument();
    expect(within(replay).getByText("80ms")).toBeInTheDocument();
    expect(within(replay).getByText("60ms")).toBeInTheDocument();
    expect(within(replay).getByText("20ms")).toBeInTheDocument();
  });

  it("surfaces replay report failures without dropping runtime telemetry", async () => {
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "get_projection_runtime_status_cmd") {
        return status({
          accepted_transcript_event_count: 1,
          transcript_span_count: 1,
        });
      }
      if (cmd === "get_projection_replay_report_cmd") {
        throw new Error("projection log unavailable");
      }
      return undefined;
    });

    const user = userEvent.setup();
    render(<ProjectionRuntimeStatusPanel />);

    await waitFor(() =>
      expect(screen.getAllByText("1").length).toBeGreaterThan(1),
    );
    await user.click(
      screen.getByRole("button", {
        name: /check projection replay parity/i,
      }),
    );

    expect(
      await screen.findByText(/replay report unavailable/i),
    ).toBeInTheDocument();
    expect(screen.getAllByText("1").length).toBeGreaterThan(1);
  });

  it("renders recent graph projection operations from stored patch events", async () => {
    const graphPatch: ProjectionPatch = {
      sequence: 42,
      kind: "graph",
      llm_request_id: "llm-graph-42",
      basis: {},
      operations: [
        {
          type: "merge_graph_nodes",
          source_id: "person:alicia",
          target_id: "person:alice",
        },
        {
          type: "split_graph_node",
          id: "topic:providers",
          replacement_nodes: [
            {
              id: "topic:provider-research",
              name: "Provider research",
              entity_type: "topic",
              description: null,
            },
            {
              id: "topic:provider-implementation",
              name: "Provider implementation",
              entity_type: "topic",
              description: null,
            },
          ],
        },
        {
          type: "invalidate_graph_edge",
          id: "edge:stale",
        },
      ],
      confidence: 0.9,
      provenance: {},
      created_at_ms: 1_700_000_000_000,
    };
    useAudioGraphStore.setState({
      sessionProjectionEvents: [graphPatch],
    });
    mockedInvoke.mockResolvedValueOnce(
      status({
        accepted_transcript_event_count: 2,
        transcript_span_count: 2,
        materialized: {
          notes_last_sequence: 0,
          note_count: 0,
          graph_last_sequence: 42,
          graph_node_count: 4,
          graph_edge_count: 1,
        },
      }),
    );

    render(<ProjectionRuntimeStatusPanel />);

    const operations = await screen.findByRole("article", {
      name: /graph operations/i,
    });
    expect(within(operations).getByText(/node merge/i)).toBeInTheDocument();
    expect(
      within(operations).getByText("person:alicia -> person:alice"),
    ).toBeInTheDocument();
    expect(within(operations).getByText(/node split/i)).toBeInTheDocument();
    expect(
      within(operations).getByText(
        "topic:providers -> topic:provider-research, topic:provider-implementation",
      ),
    ).toBeInTheDocument();
    expect(
      within(operations).getByText(/edge invalidate/i),
    ).toBeInTheDocument();
    expect(within(operations).getAllByText("Seq 42")).toHaveLength(3);
  });
});
