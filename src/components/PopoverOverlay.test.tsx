import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import PopoverOverlay from "./PopoverOverlay";

describe("PopoverOverlay", () => {
  it("renders its children inside a labelled modal dialog", () => {
    render(
      <PopoverOverlay label="Token usage" onClose={vi.fn()}>
        <p>panel body</p>
      </PopoverOverlay>,
    );
    const dialog = screen.getByRole("dialog", { name: "Token usage" });
    expect(dialog).toHaveAttribute("aria-modal", "true");
    expect(dialog).toHaveTextContent("panel body");
  });

  it("moves focus to the dialog surface on mount", () => {
    render(
      <PopoverOverlay label="Agent proposals" onClose={vi.fn()}>
        <button type="button">Action</button>
      </PopoverOverlay>,
    );
    expect(screen.getByRole("dialog")).toHaveFocus();
  });

  it("invokes onClose when Escape is pressed", () => {
    const onClose = vi.fn();
    render(
      <PopoverOverlay label="Token usage" onClose={onClose}>
        <p>body</p>
      </PopoverOverlay>,
    );
    fireEvent.keyDown(document, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("ignores non-Escape key presses", () => {
    const onClose = vi.fn();
    render(
      <PopoverOverlay label="Token usage" onClose={onClose}>
        <p>body</p>
      </PopoverOverlay>,
    );
    fireEvent.keyDown(document, { key: "Enter" });
    expect(onClose).not.toHaveBeenCalled();
  });

  it("invokes onClose when the scrim is clicked", () => {
    const onClose = vi.fn();
    render(
      <PopoverOverlay label="Token usage" onClose={onClose}>
        <p>body</p>
      </PopoverOverlay>,
    );
    // The scrim is the decorative, aria-hidden sibling of the dialog.
    const scrim = document.querySelector('[aria-hidden="true"]');
    expect(scrim).not.toBeNull();
    fireEvent.click(scrim as Element);
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("applies a custom className to the dialog surface when provided", () => {
    render(
      <PopoverOverlay label="Custom" onClose={vi.fn()} className="my-surface">
        <p>body</p>
      </PopoverOverlay>,
    );
    expect(screen.getByRole("dialog")).toHaveClass("my-surface");
  });

  it("restores focus to the trigger element on unmount", () => {
    const trigger = document.createElement("button");
    document.body.appendChild(trigger);
    trigger.focus();
    expect(trigger).toHaveFocus();

    const { unmount } = render(
      <PopoverOverlay label="Token usage" onClose={vi.fn()}>
        <button type="button">inside</button>
      </PopoverOverlay>,
    );
    // Focus moved into the dialog while open.
    expect(screen.getByRole("dialog")).toHaveFocus();

    unmount();
    expect(trigger).toHaveFocus();
    trigger.remove();
  });
});
