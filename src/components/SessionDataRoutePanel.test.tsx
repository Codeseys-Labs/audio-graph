import { invoke } from "@tauri-apps/api/core";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { DataMovementEvent } from "../types";
import SessionDataRoutePanel from "./SessionDataRoutePanel";
import "../i18n";

const mockedInvoke = vi.mocked(invoke);

function event(overrides: Partial<DataMovementEvent> = {}): DataMovementEvent {
  return {
    event_id:
      overrides.event_id ?? `evt-${Math.random().toString(36).slice(2)}`,
    schema_version: 1,
    session_id: "session-1",
    created_at_ms: 1_000,
    actor: "system",
    event_type: "artifact_written",
    destination: { boundary: "local" },
    policy: {
      privacy_mode: "local_only",
      user_visible: true,
      retention_class: "session_artifact",
    },
    result: { status: "succeeded" },
    ...overrides,
  };
}

describe("SessionDataRoutePanel", () => {
  beforeEach(() => {
    mockedInvoke.mockReset();
  });

  it("prompts to load a session when none is provided", () => {
    render(<SessionDataRoutePanel sessionId={null} />);
    expect(
      screen.getByText(/Load a session to see where its data went/i),
    ).toBeInTheDocument();
    expect(mockedInvoke).not.toHaveBeenCalled();
  });

  it("shows 'no content left the device' for a local-only session", async () => {
    mockedInvoke.mockResolvedValueOnce([
      event({
        event_type: "capture_started",
        source: { kind: "device", source_label: "Built-in Mic" },
      }),
      event({
        event_type: "artifact_written",
        data_classes: ["transcript_text"],
        destination: { boundary: "local" },
      }),
    ] satisfies DataMovementEvent[]);

    render(<SessionDataRoutePanel sessionId="session-local" />);

    await waitFor(() =>
      expect(
        screen.getByTestId("data-route-local-only-banner"),
      ).toBeInTheDocument(),
    );
    expect(
      screen.getByText(/No session content left this device/i),
    ).toBeInTheDocument();
    // No egress banner and no provider transfer rows.
    expect(
      screen.queryByTestId("data-route-egress-banner"),
    ).not.toBeInTheDocument();
    expect(screen.queryByTestId("data-route-transfer")).not.toBeInTheDocument();

    expect(mockedInvoke).toHaveBeenCalledWith(
      "load_session_data_movement_cmd",
      { sessionId: "session-local" },
    );
  });

  it("shows provider/model/data-class transfer for a cloud session without exposing secrets", async () => {
    // A ledger whose raw JSON deliberately contains no secret — the panel must
    // surface the provider/model/data classes but never a credential value.
    const events: DataMovementEvent[] = [
      event({
        event_type: "provider_call_succeeded",
        policy: {
          privacy_mode: "byok_cloud",
          user_visible: true,
          retention_class: "transient",
        },
        destination: {
          boundary: "provider",
          provider_id: "llm.openrouter",
          endpoint_class: "chat_completions",
        },
        model: {
          provider_id: "llm.openrouter",
          model_id: "openai/gpt-4o-mini",
        },
        data_classes: ["prompts", "transcript_text"],
      }),
    ];
    mockedInvoke.mockResolvedValueOnce(events);

    const { container } = render(
      <SessionDataRoutePanel sessionId="session-cloud" />,
    );

    await waitFor(() =>
      expect(
        screen.getByTestId("data-route-egress-banner"),
      ).toBeInTheDocument(),
    );

    // Provider, model, endpoint, and data classes are all surfaced.
    const transfer = screen.getByTestId("data-route-transfer");
    expect(transfer).toHaveTextContent("llm.openrouter");
    expect(transfer).toHaveTextContent("openai/gpt-4o-mini");
    expect(transfer).toHaveTextContent(/chat_completions/i);
    expect(transfer).toHaveTextContent(/Prompts/i);
    expect(transfer).toHaveTextContent(/Transcript text/i);

    // The local-only banner must NOT be shown for a cloud session.
    expect(
      screen.queryByTestId("data-route-local-only-banner"),
    ).not.toBeInTheDocument();

    // No secret material anywhere in the rendered output. (The word "secret"
    // itself appears in the panel's benign redaction-note copy — "never
    // secrets or raw content" — so we assert on credential *value* shapes,
    // which the redaction-safe ledger never carries, rather than the word.)
    const rendered = container.textContent?.toLowerCase() ?? "";
    expect(rendered).not.toContain("api_key");
    expect(rendered).not.toContain("bearer ");
    expect(rendered).not.toContain("sk-");
    expect(rendered).not.toMatch(/authorization:/);
  });

  it("renders a redacted error message from the ledger", async () => {
    mockedInvoke.mockResolvedValueOnce([
      event({
        event_type: "provider_call_failed",
        destination: { boundary: "provider", provider_id: "asr.aws" },
        data_classes: ["audio_stream"],
        result: {
          status: "failed",
          error_code: "provider_timeout",
          error_message_redacted: "Request timed out after 30s",
        },
      }),
    ] satisfies DataMovementEvent[]);

    render(<SessionDataRoutePanel sessionId="session-error" />);

    await waitFor(() =>
      expect(
        screen.getByText(/Request timed out after 30s/i),
      ).toBeInTheDocument(),
    );
    expect(screen.getByText("provider_timeout")).toBeInTheDocument();
  });

  it("surfaces a load error", async () => {
    mockedInvoke.mockRejectedValueOnce({
      code: "session_invalid",
      message: { reason: "no such session" },
    });

    render(<SessionDataRoutePanel sessionId="session-bad" />);

    await waitFor(() => expect(screen.getByRole("alert")).toBeInTheDocument());
    expect(screen.getByRole("alert")).toHaveTextContent(/no such session/i);
  });

  it("clears the prior session's report on a sessionId switch, showing loading (not stale data) until the new session resolves", async () => {
    // Session A is a cloud/egress session: its report surfaces a provider
    // transfer and an egress banner.
    const sessionAEvents: DataMovementEvent[] = [
      event({
        session_id: "session-a",
        event_type: "provider_call_succeeded",
        policy: {
          privacy_mode: "byok_cloud",
          user_visible: true,
          retention_class: "transient",
        },
        destination: {
          boundary: "provider",
          provider_id: "llm.openrouter",
          endpoint_class: "chat_completions",
        },
        model: {
          provider_id: "llm.openrouter",
          model_id: "openai/gpt-4o-mini",
        },
        data_classes: ["prompts", "transcript_text"],
      }),
    ];
    // Session B is a local-only session — deliberately different from A so the
    // stale A report would be visibly wrong under B's header.
    const sessionBEvents: DataMovementEvent[] = [
      event({
        session_id: "session-b",
        event_type: "artifact_written",
        data_classes: ["transcript_text"],
        destination: { boundary: "local" },
      }),
    ];

    // A resolves immediately; B's fetch is held pending under our control so we
    // can inspect the render while B is still loading.
    let resolveB: (value: DataMovementEvent[]) => void = () => {};
    const bPending = new Promise<DataMovementEvent[]>((resolve) => {
      resolveB = resolve;
    });
    mockedInvoke.mockResolvedValueOnce(sessionAEvents);
    mockedInvoke.mockReturnValueOnce(bPending);

    const { rerender } = render(
      <SessionDataRoutePanel sessionId="session-a" />,
    );

    // A's egress report is on screen.
    await waitFor(() =>
      expect(
        screen.getByTestId("data-route-egress-banner"),
      ).toBeInTheDocument(),
    );
    expect(screen.getByTestId("data-route-transfer")).toHaveTextContent(
      "openai/gpt-4o-mini",
    );

    // Switch to session B; B's fetch is still pending.
    rerender(<SessionDataRoutePanel sessionId="session-b" />);

    // The panel must NOT show A's report while B loads: no egress banner, no
    // provider transfer row, no A model id — a loading state instead.
    await waitFor(() =>
      expect(
        screen.getByText(/Loading data-route report/i),
      ).toBeInTheDocument(),
    );
    expect(
      screen.queryByTestId("data-route-egress-banner"),
    ).not.toBeInTheDocument();
    expect(screen.queryByTestId("data-route-transfer")).not.toBeInTheDocument();
    expect(screen.queryByText("openai/gpt-4o-mini")).not.toBeInTheDocument();

    // Now B resolves — its (local-only) report renders.
    resolveB(sessionBEvents);
    await waitFor(() =>
      expect(
        screen.getByTestId("data-route-local-only-banner"),
      ).toBeInTheDocument(),
    );
    // And A's egress report never leaks back in.
    expect(
      screen.queryByTestId("data-route-egress-banner"),
    ).not.toBeInTheDocument();
  });

  it("does not overwrite the new session's report when a stale in-flight response resolves late", async () => {
    // Session A's fetch is held pending; we switch to B (which resolves), then
    // let A resolve late. The cancellation guard must drop A's late response.
    let resolveA: (value: DataMovementEvent[]) => void = () => {};
    const aPending = new Promise<DataMovementEvent[]>((resolve) => {
      resolveA = resolve;
    });
    const sessionAEvents: DataMovementEvent[] = [
      event({
        session_id: "session-a",
        event_type: "provider_call_succeeded",
        policy: {
          privacy_mode: "byok_cloud",
          user_visible: true,
          retention_class: "transient",
        },
        destination: {
          boundary: "provider",
          provider_id: "llm.openrouter",
          endpoint_class: "chat_completions",
        },
        model: {
          provider_id: "llm.openrouter",
          model_id: "openai/gpt-4o-mini",
        },
        data_classes: ["prompts"],
      }),
    ];
    const sessionBEvents: DataMovementEvent[] = [
      event({
        session_id: "session-b",
        event_type: "artifact_written",
        data_classes: ["transcript_text"],
        destination: { boundary: "local" },
      }),
    ];
    mockedInvoke.mockReturnValueOnce(aPending);
    mockedInvoke.mockResolvedValueOnce(sessionBEvents);

    const { rerender } = render(
      <SessionDataRoutePanel sessionId="session-a" />,
    );
    rerender(<SessionDataRoutePanel sessionId="session-b" />);

    await waitFor(() =>
      expect(
        screen.getByTestId("data-route-local-only-banner"),
      ).toBeInTheDocument(),
    );

    // A resolves late — its (cancelled) response must not overwrite B's report.
    resolveA(sessionAEvents);
    await waitFor(() =>
      expect(
        screen.getByTestId("data-route-local-only-banner"),
      ).toBeInTheDocument(),
    );
    expect(
      screen.queryByTestId("data-route-egress-banner"),
    ).not.toBeInTheDocument();
    expect(screen.queryByText("openai/gpt-4o-mini")).not.toBeInTheDocument();
  });

  it("refetches when the refresh button is clicked", async () => {
    mockedInvoke.mockResolvedValue([] satisfies DataMovementEvent[]);

    render(<SessionDataRoutePanel sessionId="session-refresh" />);

    await waitFor(() =>
      expect(
        screen.getByTestId("data-route-local-only-banner"),
      ).toBeInTheDocument(),
    );
    expect(mockedInvoke).toHaveBeenCalledTimes(1);

    await userEvent.click(
      screen.getByRole("button", { name: /Refresh the data-route report/i }),
    );
    await waitFor(() => expect(mockedInvoke).toHaveBeenCalledTimes(2));
  });
});
