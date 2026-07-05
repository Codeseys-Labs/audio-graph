import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useAudioGraphStore } from "../store";
import type { AppNotification } from "../types";
import Notifications from "./Notifications";

function notification(
  overrides: Partial<AppNotification> = {},
): AppNotification {
  return {
    id: crypto.randomUUID(),
    severity: "info",
    message: "hello",
    createdAt: 0,
    ...overrides,
  };
}

function resetStore(
  overrides: { notifications?: AppNotification[]; error?: string | null } = {},
) {
  useAudioGraphStore.setState({
    notifications: overrides.notifications ?? [],
    error: overrides.error ?? null,
    dismissNotification: vi.fn(),
    clearError: vi.fn(),
  });
}

describe("Notifications", () => {
  beforeEach(() => {
    vi.useRealTimers();
    resetStore();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("renders nothing when there are no notifications and no error", () => {
    const { container } = render(<Notifications />);
    expect(container).toBeEmptyDOMElement();
  });

  it("renders an info notification as a polite status region", () => {
    resetStore({
      notifications: [notification({ severity: "info", message: "saved ok" })],
    });
    render(<Notifications />);
    const region = screen.getByRole("status");
    expect(region).toHaveTextContent("saved ok");
    expect(region).toHaveAttribute("aria-live", "polite");
  });

  it("renders an error notification as an assertive alert region", () => {
    resetStore({
      notifications: [notification({ severity: "error", message: "boom" })],
    });
    render(<Notifications />);
    const region = screen.getByRole("alert");
    expect(region).toHaveTextContent("boom");
    expect(region).toHaveAttribute("aria-live", "assertive");
  });

  it("renders one entry per queued notification", () => {
    resetStore({
      notifications: [
        notification({ message: "first" }),
        notification({ message: "second" }),
        notification({ message: "third" }),
      ],
    });
    render(<Notifications />);
    expect(screen.getByText("first")).toBeInTheDocument();
    expect(screen.getByText("second")).toBeInTheDocument();
    expect(screen.getByText("third")).toBeInTheDocument();
  });

  it("invokes dismissNotification with the id when the close button is clicked", () => {
    const dismissNotification = vi.fn();
    resetStore({
      notifications: [notification({ id: "n-1", message: "dismiss me" })],
    });
    useAudioGraphStore.setState({ dismissNotification });
    render(<Notifications />);
    fireEvent.click(
      screen.getByRole("button", { name: /dismiss notification/i }),
    );
    expect(dismissNotification).toHaveBeenCalledWith("n-1");
  });

  it("renders an inline action button and runs both the action and dismiss on click", () => {
    const onClick = vi.fn();
    const dismissNotification = vi.fn();
    resetStore({
      notifications: [
        notification({
          id: "n-2",
          message: "retry?",
          action: { label: "Retry", onClick },
        }),
      ],
    });
    useAudioGraphStore.setState({ dismissNotification });
    render(<Notifications />);
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(onClick).toHaveBeenCalledTimes(1);
    expect(dismissNotification).toHaveBeenCalledWith("n-2");
  });

  it("bridges the legacy error string as a sticky error alert with its own dismiss", () => {
    const clearError = vi.fn();
    resetStore({ error: "legacy failure" });
    useAudioGraphStore.setState({ clearError });
    render(<Notifications />);
    const region = screen.getByRole("alert");
    expect(region).toHaveTextContent("legacy failure");
    fireEvent.click(screen.getByRole("button", { name: /dismiss error/i }));
    expect(clearError).toHaveBeenCalledTimes(1);
  });

  it("auto-dismisses a non-sticky notification after the timeout", () => {
    vi.useFakeTimers();
    const dismissNotification = vi.fn();
    resetStore({
      notifications: [notification({ id: "auto", sticky: false })],
    });
    useAudioGraphStore.setState({ dismissNotification });
    render(<Notifications />);
    expect(dismissNotification).not.toHaveBeenCalled();
    vi.advanceTimersByTime(4000);
    expect(dismissNotification).toHaveBeenCalledWith("auto");
  });

  it("does not auto-dismiss a sticky notification", () => {
    vi.useFakeTimers();
    const dismissNotification = vi.fn();
    resetStore({
      notifications: [notification({ id: "stuck", sticky: true })],
    });
    useAudioGraphStore.setState({ dismissNotification });
    render(<Notifications />);
    vi.advanceTimersByTime(10000);
    expect(dismissNotification).not.toHaveBeenCalled();
  });

  // ── Error humanization (ADR-0011 / review A2, seed 5c24) ────────────────

  it("renders friendly copy for a known IPC-shape error, not the raw string", () => {
    resetStore({
      error: "Cannot read properties of undefined (reading 'invoke')",
    });
    render(<Notifications />);
    // Plain-language title from the humanizer, not the verbatim TypeError.
    expect(
      screen.getByText(/desktop backend isn’t reachable/i),
    ).toBeInTheDocument();
    // A Details disclosure is present, and the raw string lives *inside* it
    // (collapsed by default) rather than being the visible message.
    expect(screen.getByText(/^details$/i)).toBeInTheDocument();
    const raw = screen.getByText(
      "Cannot read properties of undefined (reading 'invoke')",
    );
    const details = raw.closest("details");
    expect(details).not.toBeNull();
    expect(details).not.toHaveAttribute("open");
  });

  it("reveals the raw string under Details when expanded", () => {
    const raw = "Cannot read properties of undefined (reading 'invoke')";
    resetStore({ error: raw });
    render(<Notifications />);
    fireEvent.click(screen.getByText(/^details$/i));
    expect(screen.getByText(raw)).toBeInTheDocument();
  });

  it("shows the generic title + Details for an unknown technical error", () => {
    resetStore({ error: "TypeError: reading foo of null is not iterable" });
    render(<Notifications />);
    expect(screen.getByText(/something went wrong/i)).toBeInTheDocument();
    expect(screen.getByText(/^details$/i)).toBeInTheDocument();
  });

  it("passes an already-friendly error message through verbatim", () => {
    const friendly =
      "Missing credential: OpenAI. Open Settings to configure it.";
    resetStore({ error: friendly });
    render(<Notifications />);
    expect(screen.getByText(friendly)).toBeInTheDocument();
  });

  it("auto-dismisses a transient (IPC/network) legacy error after the timeout", () => {
    vi.useFakeTimers();
    const clearError = vi.fn();
    resetStore({
      error: "Cannot read properties of undefined (reading 'invoke')",
    });
    useAudioGraphStore.setState({ clearError });
    render(<Notifications />);
    expect(clearError).not.toHaveBeenCalled();
    vi.advanceTimersByTime(4000);
    expect(clearError).toHaveBeenCalledTimes(1);
  });

  it("does NOT auto-dismiss a non-transient (auth) legacy error", () => {
    vi.useFakeTimers();
    const clearError = vi.fn();
    resetStore({ error: "401 Unauthorized: invalid API key" });
    useAudioGraphStore.setState({ clearError });
    render(<Notifications />);
    vi.advanceTimersByTime(10000);
    expect(clearError).not.toHaveBeenCalled();
  });

  it("renders a transient legacy error as a polite status, not an assertive alert", () => {
    resetStore({
      error: "Cannot read properties of undefined (reading 'invoke')",
    });
    render(<Notifications />);
    const region = screen.getByRole("status");
    expect(region).toHaveAttribute("aria-live", "polite");
  });
});
