import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import ResizeDivider from "./ResizeDivider";

// jsdom does not implement the Pointer Capture API. ResizeDivider calls
// setPointerCapture/releasePointerCapture inside its pointer handlers, so we
// stub them on the prototype to keep the drag path from throwing.
beforeEach(() => {
  if (!HTMLElement.prototype.setPointerCapture) {
    HTMLElement.prototype.setPointerCapture = vi.fn();
  }
  if (!HTMLElement.prototype.releasePointerCapture) {
    HTMLElement.prototype.releasePointerCapture = vi.fn();
  }
});

describe("ResizeDivider", () => {
  it("renders a separator with the orientation + default aria metadata", () => {
    render(<ResizeDivider orientation="vertical" onResize={vi.fn()} />);
    const sep = screen.getByRole("separator");
    expect(sep).toHaveAttribute("aria-orientation", "vertical");
    expect(sep).toHaveAttribute("aria-valuenow", "0");
    expect(sep).toHaveAttribute("aria-label", "Resize panel");
    expect(sep).toHaveAttribute("tabindex", "0");
  });

  it("uses the provided aria-label and orientation class", () => {
    const { container } = render(
      <ResizeDivider
        orientation="horizontal"
        onResize={vi.fn()}
        ariaLabel="Resize transcript"
      />,
    );
    const sep = screen.getByRole("separator");
    expect(sep).toHaveAttribute("aria-label", "Resize transcript");
    expect(sep).toHaveAttribute("aria-orientation", "horizontal");
    expect(
      container.querySelector(".resize-divider--horizontal"),
    ).not.toBeNull();
  });

  // --- Keyboard nudging ---------------------------------------------------

  it("vertical: ArrowRight nudges +8 and ArrowLeft nudges -8", () => {
    const onResize = vi.fn();
    render(<ResizeDivider orientation="vertical" onResize={onResize} />);
    const sep = screen.getByRole("separator");
    fireEvent.keyDown(sep, { key: "ArrowRight" });
    expect(onResize).toHaveBeenLastCalledWith(8);
    fireEvent.keyDown(sep, { key: "ArrowLeft" });
    expect(onResize).toHaveBeenLastCalledWith(-8);
    expect(onResize).toHaveBeenCalledTimes(2);
  });

  it("vertical: shift multiplies the step to 32", () => {
    const onResize = vi.fn();
    render(<ResizeDivider orientation="vertical" onResize={onResize} />);
    const sep = screen.getByRole("separator");
    fireEvent.keyDown(sep, { key: "ArrowRight", shiftKey: true });
    expect(onResize).toHaveBeenLastCalledWith(32);
    fireEvent.keyDown(sep, { key: "ArrowLeft", shiftKey: true });
    expect(onResize).toHaveBeenLastCalledWith(-32);
  });

  it("vertical: ignores ArrowUp/ArrowDown", () => {
    const onResize = vi.fn();
    render(<ResizeDivider orientation="vertical" onResize={onResize} />);
    const sep = screen.getByRole("separator");
    fireEvent.keyDown(sep, { key: "ArrowUp" });
    fireEvent.keyDown(sep, { key: "ArrowDown" });
    expect(onResize).not.toHaveBeenCalled();
  });

  it("horizontal: ArrowDown nudges +8 and ArrowUp nudges -8", () => {
    const onResize = vi.fn();
    render(<ResizeDivider orientation="horizontal" onResize={onResize} />);
    const sep = screen.getByRole("separator");
    fireEvent.keyDown(sep, { key: "ArrowDown" });
    expect(onResize).toHaveBeenLastCalledWith(8);
    fireEvent.keyDown(sep, { key: "ArrowUp" });
    expect(onResize).toHaveBeenLastCalledWith(-8);
  });

  it("horizontal: ignores ArrowLeft/ArrowRight", () => {
    const onResize = vi.fn();
    render(<ResizeDivider orientation="horizontal" onResize={onResize} />);
    const sep = screen.getByRole("separator");
    fireEvent.keyDown(sep, { key: "ArrowLeft" });
    fireEvent.keyDown(sep, { key: "ArrowRight" });
    expect(onResize).not.toHaveBeenCalled();
  });

  it("ignores unrelated keys", () => {
    const onResize = vi.fn();
    render(<ResizeDivider orientation="vertical" onResize={onResize} />);
    fireEvent.keyDown(screen.getByRole("separator"), { key: "Enter" });
    expect(onResize).not.toHaveBeenCalled();
  });

  // --- Pointer drag -------------------------------------------------------

  it("vertical: a pointer drag reports the X delta since the last move", () => {
    const onResize = vi.fn();
    render(<ResizeDivider orientation="vertical" onResize={onResize} />);
    const sep = screen.getByRole("separator");

    fireEvent.pointerDown(sep, { pointerId: 1, clientX: 100, clientY: 0 });
    // Before a down, moves are no-ops; here we are dragging.
    fireEvent.pointerMove(sep, { pointerId: 1, clientX: 130, clientY: 0 });
    expect(onResize).toHaveBeenLastCalledWith(30);
    fireEvent.pointerMove(sep, { pointerId: 1, clientX: 120, clientY: 0 });
    expect(onResize).toHaveBeenLastCalledWith(-10);

    fireEvent.pointerUp(sep, { pointerId: 1 });
    // After release, further moves are ignored.
    onResize.mockClear();
    fireEvent.pointerMove(sep, { pointerId: 1, clientX: 200, clientY: 0 });
    expect(onResize).not.toHaveBeenCalled();
  });

  it("horizontal: a pointer drag reports the Y delta", () => {
    const onResize = vi.fn();
    render(<ResizeDivider orientation="horizontal" onResize={onResize} />);
    const sep = screen.getByRole("separator");
    fireEvent.pointerDown(sep, { pointerId: 2, clientX: 0, clientY: 50 });
    fireEvent.pointerMove(sep, { pointerId: 2, clientX: 0, clientY: 75 });
    expect(onResize).toHaveBeenLastCalledWith(25);
  });

  it("does not call onResize for a move before any pointer down", () => {
    const onResize = vi.fn();
    render(<ResizeDivider orientation="vertical" onResize={onResize} />);
    fireEvent.pointerMove(screen.getByRole("separator"), {
      pointerId: 9,
      clientX: 10,
    });
    expect(onResize).not.toHaveBeenCalled();
  });

  it("does not call onResize for a zero-delta move", () => {
    const onResize = vi.fn();
    render(<ResizeDivider orientation="vertical" onResize={onResize} />);
    const sep = screen.getByRole("separator");
    fireEvent.pointerDown(sep, { pointerId: 3, clientX: 40 });
    fireEvent.pointerMove(sep, { pointerId: 3, clientX: 40 });
    expect(onResize).not.toHaveBeenCalled();
  });

  it("pointer cancel ends the drag like pointer up", () => {
    const onResize = vi.fn();
    render(<ResizeDivider orientation="vertical" onResize={onResize} />);
    const sep = screen.getByRole("separator");
    fireEvent.pointerDown(sep, { pointerId: 4, clientX: 0 });
    fireEvent.pointerCancel(sep, { pointerId: 4 });
    fireEvent.pointerMove(sep, { pointerId: 4, clientX: 50 });
    expect(onResize).not.toHaveBeenCalled();
  });
});
