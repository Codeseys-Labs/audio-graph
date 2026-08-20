import { invoke } from "@tauri-apps/api/core";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  FIXTURE_FINALIZATION_STATUSES,
  FX_BLOCKED_AUTOHEALED,
  FX_BLOCKED_EXTERNAL,
  FX_BLOCKED_USER_CANCELLED,
  FX_FINALIZED,
  FX_FINALIZING,
} from "../fixtures/reviewFinalizationFixtures";
import ReviewFinalizationPanel from "./ReviewFinalizationPanel";
import "../i18n";

const mockedInvoke = vi.mocked(invoke);

function mockStatusFor(_sessionId: string) {
  mockedInvoke.mockImplementation(async (cmd: string, args?: unknown) => {
    if (cmd === "get_session_finalization_status_cmd") {
      const requested = (args as { sessionId: string })?.sessionId;
      return FIXTURE_FINALIZATION_STATUSES[requested] ?? null;
    }
    return null;
  });
}

beforeEach(() => {
  mockedInvoke.mockReset();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("ReviewFinalizationPanel", () => {
  it("renders nothing when no session is loaded", () => {
    const { container } = render(<ReviewFinalizationPanel sessionId={null} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("renders nothing when the backend has no finalization data for this session (today's real backend)", async () => {
    mockedInvoke.mockImplementation(async () => null);
    const { container } = render(
      <ReviewFinalizationPanel sessionId="some-real-session" />,
    );
    await waitFor(() =>
      expect(mockedInvoke).toHaveBeenCalledWith(
        "get_session_finalization_status_cmd",
        { sessionId: "some-real-session" },
      ),
    );
    expect(container).toBeEmptyDOMElement();
  });

  it("degrades to the calm empty state when invoke rejects (command not implemented)", async () => {
    mockedInvoke.mockImplementation(async () => {
      throw new Error("command get_session_finalization_status_cmd not found");
    });
    render(<ReviewFinalizationPanel sessionId="fx-finalizing" />);
    expect(
      await screen.findByText(/no finalization data for this session/i),
    ).toBeInTheDocument();
  });

  it("shows Finalizing with computed, non-persisted progress and per-lane coverage", async () => {
    mockStatusFor(FX_FINALIZING.id);
    render(<ReviewFinalizationPanel sessionId={FX_FINALIZING.id} />);

    const chip = await screen.findByTestId("finalization-stage-chip");
    expect(chip).toHaveTextContent(/finalizing/i);
    expect(
      screen.getByText(/progress is computed each time you view this/i),
    ).toBeInTheDocument();

    const notesRow = screen.getByTestId("finalization-lane-notes");
    expect(within(notesRow).getByText(/3 pending/i)).toBeInTheDocument();
    // Default variant is "informational" — the non-gating graph lane is
    // still shown, clearly marked non-required.
    const graphRow = screen.getByTestId("finalization-lane-graph");
    expect(
      within(graphRow).getByText(/not required to finish/i),
    ).toBeInTheDocument();
  });

  it("shows STT interim vs. confirmed text as a per-lane confirmation summary and per-line badges", async () => {
    mockStatusFor(FX_FINALIZING.id);
    render(<ReviewFinalizationPanel sessionId={FX_FINALIZING.id} />);

    const section = await screen.findByTestId(
      "finalization-transcript-confirmation",
    );
    expect(within(section).getByText(/176 confirmed/i)).toBeInTheDocument();
    expect(within(section).getByText(/4 interim/i)).toBeInTheDocument();
    // Interim lines carry a distinct "not yet confirmed" badge — this is the
    // STT interim-vs-confirmed distinction the seed asks Review to surface.
    const interimBadges = within(section).getAllByText(
      /interim.*not yet confirmed/i,
    );
    expect(interimBadges.length).toBe(4);
  });

  it("Finalization Blocked (external_uncertain) is non-dismissable and needs explicit cost/egress authorization", async () => {
    mockStatusFor(FX_BLOCKED_EXTERNAL.id);
    const user = userEvent.setup();
    render(<ReviewFinalizationPanel sessionId={FX_BLOCKED_EXTERNAL.id} />);

    const chip = await screen.findByTestId("finalization-stage-chip");
    expect(chip).toHaveTextContent(/finalization blocked/i);

    const record = screen.getByTestId("finalization-blocked-record");
    // No dismiss/close affordance anywhere in the blocked record.
    expect(
      within(record).queryByRole("button", { name: /close|dismiss/i }),
    ).not.toBeInTheDocument();
    expect(screen.getByText(/can't be dismissed/i)).toBeInTheDocument();

    // Default variant ("explicitButton") shows an authorize control, not a
    // free-retry button — this class always needs an explicit ask.
    const authorizeBtn = within(record).getByRole("button", {
      name: /authorize retry/i,
    });
    await user.click(authorizeBtn);
    expect(
      screen.getByText(/may incur cost or send data off-device/i),
    ).toBeInTheDocument();

    mockedInvoke.mockImplementationOnce(async (cmd: string) => {
      if (cmd === "retry_session_finalization_cmd") {
        return {
          ...FIXTURE_FINALIZATION_STATUSES[FX_BLOCKED_EXTERNAL.id],
          blocked_record: null,
        };
      }
      return null;
    });
    await user.click(screen.getByRole("button", { name: /confirm retry/i }));

    await waitFor(() =>
      expect(mockedInvoke).toHaveBeenCalledWith(
        "retry_session_finalization_cmd",
        { sessionId: FX_BLOCKED_EXTERNAL.id, authorizeCostAndEgress: true },
      ),
    );
    await waitFor(() =>
      expect(screen.getByTestId("finalization-stage-chip")).toHaveTextContent(
        /finalizing/i,
      ),
    );
  });

  it("Blocked{UserCancelled} reads calmly and stays retryable without nagging", async () => {
    mockStatusFor(FX_BLOCKED_USER_CANCELLED.id);
    render(
      <ReviewFinalizationPanel sessionId={FX_BLOCKED_USER_CANCELLED.id} />,
    );

    expect(await screen.findByText(/finalization paused/i)).toBeInTheDocument();
    const record = screen.getByTestId("finalization-blocked-record");
    expect(
      within(record).getByRole("button", { name: /resume finalization/i }),
    ).toBeInTheDocument();
  });

  it("auto-retry-eligible classes clear with zero cost/egress once the ledger shows a later success (re-derived on render)", async () => {
    mockStatusFor(FX_BLOCKED_AUTOHEALED.id);
    render(<ReviewFinalizationPanel sessionId={FX_BLOCKED_AUTOHEALED.id} />);

    const chip = await screen.findByTestId("finalization-stage-chip");
    // Already healed by re-derivation — never shown as "blocked" at all.
    expect(chip).not.toHaveTextContent(/finalization blocked/i);
    expect(screen.getByText(/cleared automatically/i)).toBeInTheDocument();
    expect(
      screen.queryByTestId("finalization-blocked-record"),
    ).not.toBeInTheDocument();
  });

  it("Finalized is reached on notes-lane coverage alone; a lagging graph lane never gates it", async () => {
    mockStatusFor(FX_FINALIZED.id);
    render(<ReviewFinalizationPanel sessionId={FX_FINALIZED.id} />);

    const chip = await screen.findByTestId("finalization-stage-chip");
    expect(chip).toHaveTextContent(/finalized/i);
    const graphRow = screen.getByTestId("finalization-lane-graph");
    expect(within(graphRow).getByText(/pending/i)).toBeInTheDocument();
  });

  it("graph lane visibility variant: 'hidden' omits the graph lane entirely", async () => {
    mockStatusFor(FX_FINALIZING.id);
    render(
      <ReviewFinalizationPanel
        sessionId={FX_FINALIZING.id}
        graphLaneVisibility="hidden"
      />,
    );
    await screen.findByTestId("finalization-lane-notes");
    expect(
      screen.queryByTestId("finalization-lane-graph"),
    ).not.toBeInTheDocument();
  });

  it("blocked presentation variant: 'badge' renders a compact record without the long-form summary/detail text", async () => {
    mockStatusFor(FX_BLOCKED_EXTERNAL.id);
    render(
      <ReviewFinalizationPanel
        sessionId={FX_BLOCKED_EXTERNAL.id}
        blockedPresentation="badge"
      />,
    );
    const record = await screen.findByTestId("finalization-blocked-record");
    expect(record).toHaveAttribute("data-presentation", "badge");
    expect(
      within(record).queryByText(/rate-limited, then timed out/i),
    ).not.toBeInTheDocument();
    // Still non-dismissable regardless of presentation.
    expect(within(record).getByText(/can't be dismissed/i)).toBeInTheDocument();
  });

  it("retry affordance variant: 'autoHealOnly' hides the free-retry button for auto-eligible classes but keeps an explicit control for external_uncertain", async () => {
    mockStatusFor(FX_BLOCKED_EXTERNAL.id);
    render(
      <ReviewFinalizationPanel
        sessionId={FX_BLOCKED_EXTERNAL.id}
        retryAffordance="autoHealOnly"
      />,
    );
    const record = await screen.findByTestId("finalization-blocked-record");
    expect(
      within(record).queryByRole("button", { name: /retry \(free/i }),
    ).not.toBeInTheDocument();
    // external_uncertain still needs an explicit ask even in autoHealOnly.
    expect(
      within(record).getByRole("button", { name: /authorize retry/i }),
    ).toBeInTheDocument();
  });

  it("retry affordance variant: 'autoHealOnly' passively re-polls an auto-eligible unresolved blocker without any click", async () => {
    vi.useFakeTimers();
    let call = 0;
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd !== "get_session_finalization_status_cmd") return null;
      call += 1;
      if (call === 1) {
        // First read: never_dispatched, unresolved (no qualifying success yet).
        return {
          ...FIXTURE_FINALIZATION_STATUSES[FX_BLOCKED_AUTOHEALED.id],
          remote_attempt_ledger: [],
        };
      }
      // Poll picks up a fixture that now shows the ledger success.
      return FIXTURE_FINALIZATION_STATUSES[FX_BLOCKED_AUTOHEALED.id];
    });

    render(
      <ReviewFinalizationPanel
        sessionId={FX_BLOCKED_AUTOHEALED.id}
        retryAffordance="autoHealOnly"
      />,
    );

    await vi.waitFor(() =>
      expect(screen.getByTestId("finalization-stage-chip")).toHaveTextContent(
        /finalization blocked/i,
      ),
    );

    await vi.advanceTimersByTimeAsync(200);

    await vi.waitFor(() =>
      expect(
        screen.getByTestId("finalization-stage-chip"),
      ).not.toHaveTextContent(/finalization blocked/i),
    );
  });

  it("renders Knowledge Gaps as informational (never gates the stage chip)", async () => {
    mockStatusFor(FX_FINALIZING.id);
    render(<ReviewFinalizationPanel sessionId={FX_FINALIZING.id} />);
    const gaps = await screen.findByTestId("knowledge-gaps");
    expect(within(gaps).getByText(/no cited source span/i)).toBeInTheDocument();
  });

  it("evidence inspection is collapsed by default and expands to show redacted ledger entries", async () => {
    mockStatusFor(FX_BLOCKED_EXTERNAL.id);
    const user = userEvent.setup();
    render(<ReviewFinalizationPanel sessionId={FX_BLOCKED_EXTERNAL.id} />);

    const evidence = await screen.findByTestId("finalization-evidence");
    expect(
      within(evidence).queryByText(/notes.*rate_limited/i),
    ).not.toBeInTheDocument();
    await user.click(
      within(evidence).getByRole("button", { name: /show evidence/i }),
    );
    expect(
      within(evidence).getByText(/notes.*rate_limited/i),
    ).toBeInTheDocument();
    // Never a raw LLM chunk — only structured, redacted lane/outcome/timestamp.
    expect(within(evidence).queryByText(/chunk/i)).not.toBeInTheDocument();
  });

  it("two different sessions never bleed into each other's derived stage", async () => {
    mockStatusFor(FX_BLOCKED_EXTERNAL.id);
    const { rerender } = render(
      <ReviewFinalizationPanel sessionId={FX_BLOCKED_EXTERNAL.id} />,
    );
    expect(
      await screen.findByTestId("finalization-stage-chip"),
    ).toHaveTextContent(/finalization blocked/i);

    mockStatusFor(FX_FINALIZED.id);
    rerender(<ReviewFinalizationPanel sessionId={FX_FINALIZED.id} />);
    await waitFor(() =>
      expect(screen.getByTestId("finalization-stage-chip")).toHaveTextContent(
        /finalized/i,
      ),
    );
    expect(
      screen.queryByTestId("finalization-blocked-record"),
    ).not.toBeInTheDocument();
  });
});
