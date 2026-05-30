import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import Button from "./Button";

describe("Button", () => {
  it("renders its children as the accessible name", () => {
    render(<Button>Save settings</Button>);
    expect(
      screen.getByRole("button", { name: "Save settings" }),
    ).toBeInTheDocument();
  });

  it("defaults to the secondary/md variant classes and type=button", () => {
    render(<Button>Go</Button>);
    const btn = screen.getByRole("button", { name: "Go" });
    expect(btn).toHaveClass("btn");
    expect(btn).toHaveClass("btn--secondary");
    expect(btn).toHaveClass("btn--md");
    expect(btn).toHaveAttribute("type", "button");
  });

  it("applies the chosen variant and size modifier classes", () => {
    render(
      <Button variant="primary" size="sm">
        Run
      </Button>,
    );
    const btn = screen.getByRole("button", { name: "Run" });
    expect(btn).toHaveClass("btn--primary");
    expect(btn).toHaveClass("btn--sm");
  });

  it("renders a leading icon when icon is set and not loading", () => {
    render(<Button icon="download">Export</Button>);
    const btn = screen.getByRole("button", { name: "Export" });
    expect(btn.querySelector("svg")).toBeInTheDocument();
    expect(btn).not.toHaveClass("btn--loading");
  });

  it("shows a spinner, sets aria-busy, and disables the button while loading", () => {
    render(<Button loading>Saving</Button>);
    const btn = screen.getByRole("button", { name: "Saving" });
    expect(btn).toHaveClass("btn--loading");
    expect(btn).toHaveAttribute("aria-busy", "true");
    expect(btn).toBeDisabled();
    // Spinner replaces the leading icon while in flight.
    expect(btn.querySelector(".btn__spinner")).toBeInTheDocument();
  });

  it("hides the leading icon while loading (spinner takes its place)", () => {
    render(
      <Button loading icon="download">
        Export
      </Button>,
    );
    const btn = screen.getByRole("button", { name: "Export" });
    // No glyph SVG — only the spinner span.
    expect(btn.querySelector("svg")).not.toBeInTheDocument();
    expect(btn.querySelector(".btn__spinner")).toBeInTheDocument();
  });

  it("is disabled when the disabled prop is set", () => {
    render(<Button disabled>Nope</Button>);
    expect(screen.getByRole("button", { name: "Nope" })).toBeDisabled();
  });

  it("fires onClick when enabled but not when disabled or loading", () => {
    const onClick = vi.fn();
    const { rerender } = render(<Button onClick={onClick}>Click</Button>);
    fireEvent.click(screen.getByRole("button", { name: "Click" }));
    expect(onClick).toHaveBeenCalledTimes(1);

    rerender(
      <Button onClick={onClick} disabled>
        Click
      </Button>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Click" }));
    expect(onClick).toHaveBeenCalledTimes(1);

    rerender(
      <Button onClick={onClick} loading>
        Click
      </Button>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Click" }));
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it("does not set aria-busy when not loading", () => {
    render(<Button>Idle</Button>);
    expect(screen.getByRole("button", { name: "Idle" })).not.toHaveAttribute(
      "aria-busy",
    );
  });
});
