import { act, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import i18n from "../i18n";
import { useAudioGraphStore } from "../store";
import type {
  PersistenceQueueBackpressurePayload,
  PipelineLatencyEvent,
  PipelineStatus,
  ProcessedAudioConsumerHealthPayload,
  StageStatus,
  TurnLifecycleEvent,
} from "../types";
import PipelineStatusBar from "./PipelineStatusBar";

const diagnostic = {
  kind: "runtime",
  detail: {
    code: "unsupported",
    retryable: false,
    recovery_action: "review_configuration",
    context: { operation: "transcription", transport: "native" },
  },
} as const;

const idle: StageStatus = { type: "Idle" };

function allIdle(): PipelineStatus {
  return {
    capture: idle,
    pipeline: idle,
    asr: idle,
    diarization: idle,
    entity_extraction: idle,
    graph: idle,
  };
}

function publish(status: PipelineStatus) {
  act(() => useAudioGraphStore.getState().setPipelineStatus(status));
}

// SHELL-R3 (plan §R3, ADR-0046, folds audio-graph-50e3): the footer
// collapses to a single composite row only while CAPTURING and
// `computeCompositeHealth` classifies as "healthy" — which a bare `Running`
// stage, a latency sample, or a turn event alone still is (none of those
// are a problem). The fold does NOT fire on an idle (not-capturing)
// pipeline even though that classifies as "healthy" too — an unobserved
// default is not an observed "all systems normal" (ADR-0030/0034). Tests
// below that want to see per-stage/latency/turn/consumer detail therefore
// force expansion via an UNRELATED stage's Error status (`pipeline`/
// "Resample" — chosen because no existing assertion in this file queries
// that stage), so the fixture under test keeps its own meaning
// uncontaminated.
function forceExpanded(
  overrides: Partial<PipelineStatus> = {},
): PipelineStatus {
  return {
    ...allIdle(),
    pipeline: { type: "Error", diagnostic },
    ...overrides,
  };
}

function resetStore(
  overrides: {
    pipelineStatus?: PipelineStatus;
    pipelineLatencies?: Record<string, PipelineLatencyEvent>;
    turnEvents?: TurnLifecycleEvent[];
    latestAudioConsumerHealth?: ProcessedAudioConsumerHealthPayload | null;
    persistenceQueueBackpressure?: Record<
      string,
      PersistenceQueueBackpressurePayload
    >;
    backpressuredSources?: string[];
    isCapturing?: boolean;
  } = {},
) {
  useAudioGraphStore.setState({
    pipelineStatus: overrides.pipelineStatus ?? allIdle(),
    pipelineLatencies: overrides.pipelineLatencies ?? {},
    turnEvents: overrides.turnEvents ?? [],
    latestAudioConsumerHealth: overrides.latestAudioConsumerHealth ?? null,
    persistenceQueueBackpressure: overrides.persistenceQueueBackpressure ?? {},
    backpressuredSources: overrides.backpressuredSources ?? [],
    isCapturing: overrides.isCapturing ?? false,
  });
}

describe("PipelineStatusBar", () => {
  beforeEach(() => {
    resetStore();
  });

  it.each([
    ["en", "writer_absent", "Transcript is not being saved"],
    ["pt", "writer_absent", "A transcrição não está sendo salva"],
    [
      "en",
      "storage_full",
      "Transcript is not being saved because storage is full",
    ],
    [
      "pt",
      "storage_full",
      "A transcrição não está sendo salva porque o armazenamento está cheio",
    ],
    [
      "en",
      "write_failed",
      "Transcript is not being saved because the write failed",
    ],
    [
      "pt",
      "write_failed",
      "A transcrição não está sendo salva porque houve uma falha na gravação",
    ],
    ["en", "confirmation_delayed", "Transcript save confirmation pending"],
    [
      "pt",
      "confirmation_delayed",
      "Aguardando confirmação do salvamento da transcrição",
    ],
  ] as const)("reports %s %s until saving resumes", async (locale, reason, sentence) => {
    const previous = i18n.language;
    await i18n.changeLanguage(locale);
    const view = render(<PipelineStatusBar />);
    try {
      publish({ ...allIdle(), transcript_not_saving: reason });
      const bar = screen.getByRole("status");
      expect(bar).toHaveAttribute("aria-live", "polite");
      const warning = within(bar).getByText(sentence);
      expect(warning.querySelector("svg")).toHaveAttribute(
        "aria-hidden",
        "true",
      );
      publish({ ...allIdle(), transcript_not_saving: reason });
      expect(within(bar).getByText(sentence)).toBe(warning);
      publish(allIdle());
      expect(within(bar).queryByText(sentence)).toBeNull();
    } finally {
      view.unmount();
      await i18n.changeLanguage(previous);
    }
  });

  it.each([
    ["en", "Transcript saved. Crash recovery is unverified on this storage."],
    [
      "pt",
      "Transcrição salva. A recuperação após uma falha não foi verificada neste armazenamento.",
    ],
  ])("keeps the %s storage qualification notice visible while recording", async (locale, message) => {
    const previous = i18n.language;
    await i18n.changeLanguage(locale);
    resetStore({ isCapturing: true });
    const view = render(<PipelineStatusBar />);
    try {
      publish({ ...allIdle(), transcript_crash_safety_unverified: true });
      const notice = screen.getByText(message);
      publish({ ...allIdle(), transcript_crash_safety_unverified: true });
      expect(screen.getByText(message)).toBe(notice);
      publish(allIdle());
      expect(notice).not.toBeInTheDocument();
    } finally {
      view.unmount();
      await i18n.changeLanguage(previous);
    }
  });

  it("renders the status landmark with an accessible name", () => {
    render(<PipelineStatusBar />);
    const nav = screen.getByRole("status", { name: /pipeline status/i });
    expect(nav).toBeInTheDocument();
  });

  // ── 50e3 fold: collapse while CAPTURING and healthy ──────────────────────
  it("collapses to a single composite row when capturing, every stage is idle, and nothing is dropping", () => {
    resetStore({ isCapturing: true });
    render(<PipelineStatusBar />);
    expect(screen.getByText(/all systems normal/i)).toBeInTheDocument();
    expect(screen.queryByText("Capture")).not.toBeInTheDocument();
    expect(screen.queryByText("Graph")).not.toBeInTheDocument();
  });

  it("stays collapsed while capturing and a stage is actively Running (not a problem)", () => {
    resetStore({
      isCapturing: true,
      pipelineStatus: {
        ...allIdle(),
        asr: { type: "Running", processed_count: 12 },
      },
    });
    render(<PipelineStatusBar />);
    expect(screen.getByText(/all systems normal/i)).toBeInTheDocument();
    expect(screen.queryByText("ASR")).not.toBeInTheDocument();
  });

  it("does NOT collapse while idle (not capturing), even though an all-idle pipeline classifies as healthy", () => {
    // `computeCompositeHealth` on an all-`Idle` pipeline returns "healthy" —
    // but that is an unobserved default, not an observed "all systems
    // normal" (ADR-0030/0034). Master's idle footer showed six per-stage
    // idle dots; this fold must not regress that while nothing has run.
    resetStore({ isCapturing: false });
    render(<PipelineStatusBar />);
    expect(screen.queryByText(/all systems normal/i)).not.toBeInTheDocument();
    for (const name of [
      "Capture",
      "Resample",
      "ASR",
      "Diarization",
      "Extraction",
      "Graph",
    ]) {
      expect(screen.getByText(name)).toBeInTheDocument();
    }
    // Every dot renders on the neutral `idle` modifier, not the saturated
    // `running` one the collapsed row would use — idle stays visually
    // neutral, matching pre-fold master.
    const captureDot = screen.getByRole("img", { name: /Capture: Idle/i });
    expect(captureDot).toHaveClass("bg-text-muted");
    expect(captureDot).not.toHaveClass("bg-accent-green");
  });

  it("expands to per-stage detail the instant a stage reports Error", () => {
    resetStore({
      pipelineStatus: {
        ...allIdle(),
        graph: { type: "Error", diagnostic },
      },
    });
    render(<PipelineStatusBar />);
    expect(screen.queryByText(/all systems normal/i)).not.toBeInTheDocument();
    for (const name of [
      "Capture",
      "Resample",
      "ASR",
      "Diarization",
      "Extraction",
      "Graph",
    ]) {
      expect(screen.getByText(name)).toBeInTheDocument();
    }
  });

  it("expands to per-stage detail when a capture source's ring buffer is dropping chunks", () => {
    resetStore({ backpressuredSources: ["system-default"] });
    render(<PipelineStatusBar />);
    expect(screen.queryByText(/all systems normal/i)).not.toBeInTheDocument();
    expect(screen.getByText("Capture")).toBeInTheDocument();
  });

  // ── Expanded-state detail (unchanged rendering, now behind expansion) ────
  it("labels every pipeline stage in processing order", () => {
    resetStore({ pipelineStatus: forceExpanded() });
    render(<PipelineStatusBar />);
    for (const name of [
      "Capture",
      "Resample",
      "ASR",
      "Diarization",
      "Extraction",
      "Graph",
    ]) {
      expect(screen.getByText(name)).toBeInTheDocument();
    }
  });

  it("renders one status dot per stage, defaulting to the Idle tooltip", () => {
    resetStore({ pipelineStatus: forceExpanded() });
    render(<PipelineStatusBar />);
    // Each stage dot is a role=img with an aria-label embedding its tooltip.
    const captureDot = screen.getByRole("img", { name: /Capture: Idle/i });
    expect(captureDot).toBeInTheDocument();
    const graphDot = screen.getByRole("img", { name: /Graph: Idle/i });
    expect(graphDot).toBeInTheDocument();
  });

  it("surfaces the processed count in a Running stage's accessible label", () => {
    resetStore({
      pipelineStatus: forceExpanded({
        asr: { type: "Running", processed_count: 42 },
      }),
    });
    render(<PipelineStatusBar />);
    expect(
      screen.getByRole("img", { name: /ASR: Running — 42 processed/i }),
    ).toBeInTheDocument();
  });

  it("localizes an Error diagnostic without rendering untrusted compatibility data", () => {
    const canaries = [
      "provider native prose canary",
      "response body canary",
      "/private/session/path",
      "request-id-canary",
      "exact-sensitive-length-7919977",
    ];
    resetStore({
      pipelineStatus: {
        ...allIdle(),
        graph: {
          type: "Error",
          diagnostic: {
            kind: "runtime",
            detail: {
              code: "request_rejected",
              retryable: false,
              recovery_action: "review_configuration",
              context: { operation: "transcription", transport: "sdk" },
              provider_message: canaries[0],
              response_body: canaries[1],
              private_path: canaries[2],
              request_id: canaries[3],
              audio_length: canaries[4],
            },
          },
          message: canaries.join(" "),
        } as unknown as StageStatus,
      },
    });
    render(<PipelineStatusBar />);
    expect(
      screen.getByRole("img", {
        name: /^Graph: The provider rejected the request\.$/i,
      }),
    ).toBeInTheDocument();
    for (const canary of canaries) {
      expect(document.body.innerHTML).not.toContain(canary);
    }
  });

  it.each([
    [
      "en",
      "credential_rejected",
      "reenter_credential",
      undefined,
      "The provider rejected the credential. Re-enter the credential in Settings.",
    ],
    [
      "en",
      "network_unreachable",
      "retry_after_delay",
      "short",
      "Reconnecting to the provider.",
    ],
    [
      "en",
      "network_unreachable",
      "retry_after_delay",
      undefined,
      "The app could not reach the provider. Check your connection.",
    ],
    [
      "pt",
      "network_unreachable",
      "retry_after_delay",
      "short",
      "Reconectando ao provedor.",
    ],
  ] as const)("presents %s %s with %s and delay %s", async (locale, code, recovery_action, retry_delay, expected) => {
    resetStore({
      pipelineStatus: {
        ...allIdle(),
        asr: {
          type: "Error",
          diagnostic: {
            kind: "runtime",
            detail: {
              code,
              recovery_action,
              retryable: recovery_action === "retry_after_delay",
              context: {
                operation: "transcription",
                transport: "websocket",
                ...(retry_delay ? { retry_delay } : {}),
              },
            },
          },
        },
      },
    });
    const previousLanguage = i18n.language;
    await i18n.changeLanguage(locale);
    const view = render(<PipelineStatusBar />);
    try {
      expect(
        screen.getByRole("img", { name: `ASR: ${expected}` }),
      ).toBeInTheDocument();
    } finally {
      view.unmount();
      await i18n.changeLanguage(previousLanguage);
    }
  });

  it("appends a formatted latency to the tooltip and renders a latency badge", () => {
    resetStore({
      pipelineStatus: forceExpanded(),
      pipelineLatencies: {
        asr: {
          stage: "asr",
          latency_ms: 250,
          timestamp_ms: 0,
        },
      },
    });
    render(<PipelineStatusBar />);
    // 250ms < 1000 → visible "250ms" text (aria-hidden) plus a visually-hidden
    // sr-only sibling carrying the full "ASR ... latency 250ms" label (A11Y-1:
    // dropped role="img" from the text node). Assert on that sr-only label.
    expect(screen.getByText(/ASR last latency 250ms/i)).toBeInTheDocument();
    // The dot status indicator keeps role="img" (empty/color-only element) and
    // its tooltip is augmented with "• last latency 250ms".
    expect(
      screen.getByRole("img", { name: /ASR: Idle • last latency 250ms/i }),
    ).toBeInTheDocument();
  });

  it("formats sub-second-and-up latency in seconds with one decimal", () => {
    resetStore({
      pipelineStatus: forceExpanded(),
      pipelineLatencies: {
        capture: {
          stage: "capture",
          latency_ms: 1500,
          timestamp_ms: 0,
        },
      },
    });
    render(<PipelineStatusBar />);
    expect(screen.getByText(/Capture last latency 1\.5s/i)).toBeInTheDocument();
  });

  it("ignores a non-finite latency sample (no badge rendered)", () => {
    resetStore({
      pipelineStatus: forceExpanded(),
      pipelineLatencies: {
        asr: {
          stage: "asr",
          latency_ms: Number.NaN,
          timestamp_ms: 0,
        },
      },
    });
    render(<PipelineStatusBar />);
    expect(
      screen.queryByRole("img", { name: /ASR last latency/i }),
    ).not.toBeInTheDocument();
  });

  it("renders the arrow separators between stages as decorative (aria-hidden)", () => {
    resetStore({ pipelineStatus: forceExpanded() });
    const { container } = render(<PipelineStatusBar />);
    const hidden = container.querySelectorAll('[aria-hidden="true"]');
    // At minimum the 5 inter-stage arrows plus per-stage decorative icons.
    expect(hidden.length).toBeGreaterThanOrEqual(5);
  });

  it("shows a Turn chip with the latest turn event's provider + kind", () => {
    resetStore({
      pipelineStatus: forceExpanded(),
      turnEvents: [
        {
          provider: "gemini",
          source_id: "system-default",
          kind: "end_of_turn",
          timestamp_ms: 1,
        },
      ],
    });
    render(<PipelineStatusBar />);
    const nav = screen.getByRole("status", { name: /pipeline status/i });
    // "end_of_turn" → "end of turn"; provider prefix "gemini:".
    expect(within(nav).getByText(/gemini: end of turn/i)).toBeInTheDocument();
  });

  it("uses the most recent turn event when several have been recorded", () => {
    resetStore({
      pipelineStatus: forceExpanded(),
      turnEvents: [
        {
          provider: "deepgram",
          source_id: "system-default",
          kind: "speech_started",
          timestamp_ms: 1,
        },
        {
          provider: "gemini",
          source_id: "system-default",
          kind: "speech_final",
          timestamp_ms: 2,
        },
      ],
    });
    render(<PipelineStatusBar />);
    const nav = screen.getByRole("status", { name: /pipeline status/i });
    expect(within(nav).getByText(/gemini: speech final/i)).toBeInTheDocument();
    expect(
      within(nav).queryByText(/deepgram: speech started/i),
    ).not.toBeInTheDocument();
  });

  it("omits the Turn chip when no turn events have arrived, even while expanded", () => {
    resetStore({ pipelineStatus: forceExpanded(), turnEvents: [] });
    render(<PipelineStatusBar />);
    expect(screen.queryByText(/^Turn$/)).not.toBeInTheDocument();
  });

  it("shows compact audio-consumer queue health", () => {
    resetStore({
      pipelineStatus: forceExpanded(),
      latestAudioConsumerHealth: {
        consumers: [
          {
            id: "speech",
            stage: "speech",
            provider: null,
            active: true,
            queue_len: 2,
            queue_capacity: 1024,
            sent_chunks: 12,
            dropped_chunks: 0,
            drop_policy: "drop_oldest",
            source_filter: { type: "all" },
            mixing_mode: "per_source",
          },
          {
            id: "gemini-notes",
            stage: "notes",
            provider: "gemini",
            active: false,
            queue_len: 1,
            queue_capacity: 16,
            sent_chunks: 3,
            dropped_chunks: 0,
            drop_policy: "drop_oldest",
            source_filter: { type: "all" },
            mixing_mode: "per_source",
          },
        ],
      },
    });
    render(<PipelineStatusBar />);

    expect(screen.getByText("Audio")).toBeInTheDocument();
    expect(screen.getByText("1/2 on · q 3/1040 · d 0")).toBeInTheDocument();
    expect(
      screen.getByRole("img", {
        name: /Audio consumers: 1\/2 active, queue 3\/1040, 0 dropped/i,
      }),
    ).toBeInTheDocument();
  });

  it("surfaces dropped audio-consumer chunks in the status label (and this alone triggers expansion)", () => {
    resetStore({
      latestAudioConsumerHealth: {
        consumers: [
          {
            id: "speech",
            stage: "speech",
            provider: null,
            active: true,
            queue_len: 0,
            queue_capacity: 1024,
            sent_chunks: 64,
            dropped_chunks: 5,
            drop_policy: "drop_oldest",
            source_filter: { type: "all" },
            mixing_mode: "per_source",
          },
        ],
      },
    });
    render(<PipelineStatusBar />);

    expect(screen.getByText("1/1 on · q 0/1024 · d 5")).toBeInTheDocument();
    expect(
      screen.getByRole("img", {
        name: /Audio consumers: 1\/1 active, queue 0\/1024, 5 dropped/i,
      }),
    ).toBeInTheDocument();
  });

  // ── audio-graph-586b: honest diarization degradation reporting ─────────
  it("expands to per-stage detail and shows the warning (not healthy) dot when diarization reports Degraded", () => {
    resetStore({
      pipelineStatus: {
        ...allIdle(),
        // `reason` is the backend's stable snake_case code (review
        // follow-up, audio-graph-586b), not English prose — the tooltip
        // translates it via `pipeline.diarizationDegradedReason.<code>`.
        diarization: {
          type: "Degraded",
          diagnostic,
          reason: "engine_not_compiled",
        },
      },
    });
    render(<PipelineStatusBar />);

    // The 50e3 fold must NOT collapse this to "all systems normal" — a
    // Degraded stage is exactly the kind of problem the fold exists to
    // surface (see `computeCompositeHealth`'s "Degraded" classification).
    expect(screen.queryByText(/all systems normal/i)).not.toBeInTheDocument();

    const diarizationDot = screen.getByRole("img", {
      name: /Diarization: Degraded: Speaker detection is running in basic mode/i,
    });
    expect(diarizationDot).toBeInTheDocument();
    // Warning-yellow modifier, NOT the healthy-green "running" one — a
    // mutation that relabels Degraded as "running" (a success-tone leak)
    // must fail this assertion.
    expect(diarizationDot).toHaveClass("bg-accent-yellow");
    expect(diarizationDot).not.toHaveClass("bg-accent-green");
  });

  it("falls back to the localized diagnostic for an unknown degradation reason", () => {
    const reason = "private-provider-message-canary";
    resetStore({
      pipelineStatus: {
        ...allIdle(),
        diarization: {
          type: "Degraded",
          diagnostic,
          reason,
        } as unknown as StageStatus,
      },
    });
    render(<PipelineStatusBar />);
    expect(
      screen.getByRole("img", {
        name: /Diarization: Degraded: The provider does not support this operation/i,
      }),
    ).toBeInTheDocument();
    expect(document.body.innerHTML).not.toContain(reason);
  });

  it("surfaces persistence queue pressure distinctly from storage-full (and this alone triggers expansion)", () => {
    resetStore({
      persistenceQueueBackpressure: {
        transcript_event: {
          writer: "transcript_event",
          is_backpressured: true,
          queue_capacity: 2048,
          dropped_count: 2,
        },
        projection_event: {
          writer: "projection_event",
          is_backpressured: true,
          queue_capacity: 2048,
          dropped_count: 5,
        },
      },
    });
    render(<PipelineStatusBar />);

    expect(screen.getByText("Persist")).toBeInTheDocument();
    expect(screen.getByText("drop 7")).toBeInTheDocument();
    expect(
      screen.getByRole("img", {
        name: /Persistence queue pressure: transcript, notes\/graph writer queue is full, capacity 4096, 7 events dropped/i,
      }),
    ).toBeInTheDocument();
    expect(screen.queryByText(/storage full/i)).not.toBeInTheDocument();
  });
});
