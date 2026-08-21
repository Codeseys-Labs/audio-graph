import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import Popover, { PopoverItem } from "./Popover";

// SHELL-R2 (audio-graph-e0c4) coverage for the shared Popover primitive —
// added per review finding (this component previously had none, unlike its
// sibling PopoverOverlay). Exercises the behaviors the module doc asserts:
// Escape/outside-click dismissal, and Popover.Close firing on enabled items
// only (a disabled button never dispatches `click`).

describe("Popover", () => {
  it("renders the trigger as-is (asChild) with the content closed by default", () => {
    render(
      <Popover trigger={<button type="button">Row actions</button>}>
        <PopoverItem>Export</PopoverItem>
      </Popover>,
    );
    expect(
      screen.getByRole("button", { name: "Row actions" }),
    ).toBeInTheDocument();
    expect(screen.queryByText("Export")).not.toBeInTheDocument();
  });

  it("opens the content when the trigger is clicked", () => {
    render(
      <Popover trigger={<button type="button">Row actions</button>}>
        <PopoverItem>Export</PopoverItem>
      </Popover>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Row actions" }));
    expect(screen.getByRole("button", { name: "Export" })).toBeInTheDocument();
  });

  it("dismisses on Escape", () => {
    render(
      <Popover trigger={<button type="button">Row actions</button>}>
        <PopoverItem>Export</PopoverItem>
      </Popover>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Row actions" }));
    expect(screen.getByRole("button", { name: "Export" })).toBeInTheDocument();

    fireEvent.keyDown(document, { key: "Escape" });
    expect(
      screen.queryByRole("button", { name: "Export" }),
    ).not.toBeInTheDocument();
  });

  it("dismisses on an outside click", async () => {
    const user = userEvent.setup();
    render(
      <div>
        <button type="button">Outside</button>
        <Popover trigger={<button type="button">Row actions</button>}>
          <PopoverItem>Export</PopoverItem>
        </Popover>
      </div>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Row actions" }));
    expect(screen.getByRole("button", { name: "Export" })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Outside" }));
    expect(
      screen.queryByRole("button", { name: "Export" }),
    ).not.toBeInTheDocument();
  });

  it("returns focus to the trigger after Escape dismissal", async () => {
    render(
      <Popover trigger={<button type="button">Row actions</button>}>
        <PopoverItem>Export</PopoverItem>
      </Popover>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Row actions" }));
    expect(screen.getByRole("button", { name: "Export" })).toBeInTheDocument();

    fireEvent.keyDown(document, { key: "Escape" });
    // Radix schedules the focus-return asynchronously (post-unmount), so
    // this assertion needs a tick rather than firing synchronously with the
    // dismissal like the other Escape assertions above.
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Row actions" })).toHaveFocus();
    });
  });

  it("closes when an enabled PopoverItem is activated (Popover.Close)", () => {
    const onExport = vi.fn();
    render(
      <Popover trigger={<button type="button">Row actions</button>}>
        <PopoverItem onClick={onExport}>Export</PopoverItem>
      </Popover>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Row actions" }));
    fireEvent.click(screen.getByRole("button", { name: "Export" }));

    expect(onExport).toHaveBeenCalledTimes(1);
    expect(
      screen.queryByRole("button", { name: "Export" }),
    ).not.toBeInTheDocument();
  });

  it("does NOT close when a disabled PopoverItem is clicked (disabled buttons never dispatch click)", () => {
    const onDelete = vi.fn();
    render(
      <Popover trigger={<button type="button">Row actions</button>}>
        <PopoverItem onClick={onDelete} disabled>
          Delete
        </PopoverItem>
      </Popover>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Row actions" }));
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));

    expect(onDelete).not.toHaveBeenCalled();
    // Still open — the no-op click must not have closed the popover.
    expect(screen.getByRole("button", { name: "Delete" })).toBeInTheDocument();
  });

  it("applies the danger styling hook on PopoverItem without affecting its accessible name", () => {
    render(
      <Popover trigger={<button type="button">Row actions</button>}>
        <PopoverItem danger>Delete permanently</PopoverItem>
      </Popover>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Row actions" }));
    const item = screen.getByRole("button", { name: "Delete permanently" });
    expect(item.className).toContain("text-(--text-on-tint-danger)");
  });
});
