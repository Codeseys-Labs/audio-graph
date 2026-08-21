import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useAudioGraphStore } from "../store";
import SystemDrawer from "./SystemDrawer";

// `TokenUsagePanel`/`ProjectionRuntimeStatusPanel` each own async
// invoke-driven effects with their own dedicated test files — stub both
// here (same convention as App.test.tsx/App.contract.test.tsx) so this file
// stays scoped to the drawer's own structural/focus-trap/Escape/scrim
// behavior instead of needing to replicate their invoke fixtures.
vi.mock("./TokenUsagePanel", () => ({
  default: () => <div data-testid="tokens-stub" />,
}));
vi.mock("./ProjectionRuntimeStatusPanel", () => ({
  default: () => <div data-testid="projection-runtime-stub" />,
}));

function resetStore() {
  useAudioGraphStore.setState({
    pipelineStatus: {
      capture: { type: "Idle" },
      pipeline: { type: "Idle" },
      asr: { type: "Idle" },
      diarization: { type: "Idle" },
      entity_extraction: { type: "Idle" },
      graph: { type: "Idle" },
    },
    pipelineLatencies: {},
    turnEvents: [],
    latestAudioConsumerHealth: null,
    persistenceQueueBackpressure: {},
  });
}

describe("SystemDrawer", () => {
  beforeEach(() => {
    resetStore();
  });

  it("renders as a labelled, focus-trapped dialog with the pipeline/projection/token surfaces", () => {
    render(<SystemDrawer onClose={vi.fn()} />);
    const dialog = screen.getByRole("dialog", { name: /system status/i });
    expect(dialog).toHaveAttribute("aria-modal", "true");
    // Per-stage pipeline detail lives here unconditionally (SHELL-R3's "no
    // regression to diagnostics" half of the 50e3 fold).
    expect(screen.getByText("Capture")).toBeInTheDocument();
    expect(screen.getByText("Graph")).toBeInTheDocument();
  });

  it("moves focus to the dialog surface on mount", () => {
    render(<SystemDrawer onClose={vi.fn()} />);
    expect(screen.getByRole("dialog")).toHaveFocus();
  });

  it("invokes onClose when Escape is pressed", () => {
    const onClose = vi.fn();
    render(<SystemDrawer onClose={onClose} />);
    fireEvent.keyDown(document, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("ignores non-Escape key presses", () => {
    const onClose = vi.fn();
    render(<SystemDrawer onClose={onClose} />);
    fireEvent.keyDown(document, { key: "Enter" });
    expect(onClose).not.toHaveBeenCalled();
  });

  it("invokes onClose when the scrim is clicked", () => {
    const onClose = vi.fn();
    render(<SystemDrawer onClose={onClose} />);
    const scrim = document.querySelector('[aria-hidden="true"]');
    expect(scrim).not.toBeNull();
    fireEvent.click(scrim as Element);
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("invokes onClose from the header close button", () => {
    const onClose = vi.fn();
    render(<SystemDrawer onClose={onClose} />);
    fireEvent.click(screen.getByRole("button", { name: /close/i }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("restores focus to the trigger element on unmount", () => {
    const trigger = document.createElement("button");
    document.body.appendChild(trigger);
    trigger.focus();
    expect(trigger).toHaveFocus();

    const { unmount } = render(<SystemDrawer onClose={vi.fn()} />);
    expect(screen.getByRole("dialog")).toHaveFocus();

    unmount();
    expect(trigger).toHaveFocus();
    trigger.remove();
  });
});
