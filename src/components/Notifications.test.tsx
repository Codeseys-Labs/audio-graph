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
});
