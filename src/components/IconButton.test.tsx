import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import IconButton from "./IconButton";

describe("IconButton", () => {
  it("uses the required label as both the accessible name and the title", () => {
    render(<IconButton icon="refresh" label="Refresh sources" />);
    const btn = screen.getByRole("button", { name: "Refresh sources" });
    expect(btn).toBeInTheDocument();
    expect(btn).toHaveAttribute("title", "Refresh sources");
  });

  it("renders an inline SVG glyph inside the button", () => {
    render(<IconButton icon="close" label="Close" />);
    const btn = screen.getByRole("button", { name: "Close" });
    expect(btn.querySelector("svg")).toBeInTheDocument();
  });

  it("defaults to type=button so it never submits an enclosing form", () => {
    render(<IconButton icon="trash" label="Delete" />);
    expect(screen.getByRole("button", { name: "Delete" })).toHaveAttribute(
      "type",
      "button",
    );
  });

  it("applies the default variant class and merges a custom className", () => {
    render(<IconButton icon="check" label="Confirm" className="extra" />);
    const btn = screen.getByRole("button", { name: "Confirm" });
    expect(btn).toHaveClass("icon-btn");
    expect(btn).toHaveClass("icon-btn--default");
    expect(btn).toHaveClass("extra");
  });

  it("applies the variant modifier class for non-default variants", () => {
    render(<IconButton icon="trash" label="Delete" variant="danger" />);
    expect(screen.getByRole("button", { name: "Delete" })).toHaveClass(
      "icon-btn--danger",
    );
  });

  it("fires onClick when activated", () => {
    const onClick = vi.fn();
    render(<IconButton icon="settings" label="Open" onClick={onClick} />);
    fireEvent.click(screen.getByRole("button", { name: "Open" }));
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it("does not fire onClick while disabled", () => {
    const onClick = vi.fn();
    render(
      <IconButton icon="settings" label="Open" disabled onClick={onClick} />,
    );
    const btn = screen.getByRole("button", { name: "Open" });
    expect(btn).toBeDisabled();
    fireEvent.click(btn);
    expect(onClick).not.toHaveBeenCalled();
  });
});
