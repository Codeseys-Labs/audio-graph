import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import ShellDrawer from "./ShellDrawer";

describe("ShellDrawer", () => {
  it("renders as a labelled, focus-trapped dialog with the given content", () => {
    render(
      <ShellDrawer
        side="start"
        label="Sources"
        closeLabel="Close sources panel"
        onClose={vi.fn()}
      >
        <div data-testid="drawer-content">hello</div>
      </ShellDrawer>,
    );
    const dialog = screen.getByRole("dialog", { name: "Sources" });
    expect(dialog).toHaveAttribute("aria-modal", "true");
    expect(screen.getByTestId("drawer-content")).toBeInTheDocument();
  });

  it("applies the start/end side class", () => {
    const { rerender } = render(
      <ShellDrawer
        side="start"
        label="Sources"
        closeLabel="Close sources panel"
        onClose={vi.fn()}
      >
        content
      </ShellDrawer>,
    );
    expect(screen.getByRole("dialog")).toHaveClass("shell-drawer--start");

    rerender(
      <ShellDrawer
        side="end"
        label="Speakers"
        closeLabel="Close speakers panel"
        onClose={vi.fn()}
      >
        content
      </ShellDrawer>,
    );
    expect(screen.getByRole("dialog")).toHaveClass("shell-drawer--end");
  });

  it("moves focus to the dialog surface on mount", () => {
    render(
      <ShellDrawer
        side="start"
        label="Sources"
        closeLabel="Close sources panel"
        onClose={vi.fn()}
      >
        content
      </ShellDrawer>,
    );
    expect(screen.getByRole("dialog")).toHaveFocus();
  });

  it("invokes onClose when Escape is pressed", () => {
    const onClose = vi.fn();
    render(
      <ShellDrawer
        side="start"
        label="Sources"
        closeLabel="Close sources panel"
        onClose={onClose}
      >
        content
      </ShellDrawer>,
    );
    fireEvent.keyDown(document, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("ignores non-Escape key presses", () => {
    const onClose = vi.fn();
    render(
      <ShellDrawer
        side="start"
        label="Sources"
        closeLabel="Close sources panel"
        onClose={onClose}
      >
        content
      </ShellDrawer>,
    );
    fireEvent.keyDown(document, { key: "Enter" });
    expect(onClose).not.toHaveBeenCalled();
  });

  it("invokes onClose when the scrim is clicked", () => {
    const onClose = vi.fn();
    render(
      <ShellDrawer
        side="end"
        label="Speakers"
        closeLabel="Close speakers panel"
        onClose={onClose}
      >
        content
      </ShellDrawer>,
    );
    const scrim = document.querySelector('[aria-hidden="true"]');
    expect(scrim).not.toBeNull();
    fireEvent.click(scrim as Element);
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("invokes onClose from the header close button", () => {
    const onClose = vi.fn();
    render(
      <ShellDrawer
        side="end"
        label="Speakers"
        closeLabel="Close speakers panel"
        onClose={onClose}
      >
        content
      </ShellDrawer>,
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Close speakers panel" }),
    );
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("restores focus to the trigger element on unmount", () => {
    const trigger = document.createElement("button");
    document.body.appendChild(trigger);
    trigger.focus();
    expect(trigger).toHaveFocus();

    const { unmount } = render(
      <ShellDrawer
        side="start"
        label="Sources"
        closeLabel="Close sources panel"
        onClose={vi.fn()}
      >
        content
      </ShellDrawer>,
    );
    expect(screen.getByRole("dialog")).toHaveFocus();

    unmount();
    expect(trigger).toHaveFocus();
    trigger.remove();
  });
});
