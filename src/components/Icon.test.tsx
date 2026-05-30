import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import Icon, { ICONS, type IconName } from "./Icon";

describe("Icon", () => {
  it("renders an inline SVG glyph for a registered name", () => {
    const { container } = render(<Icon name="close" />);
    const svg = container.querySelector("svg");
    expect(svg).toBeInTheDocument();
  });

  it("is decorative (aria-hidden, no role) when no title is given", () => {
    const { container } = render(<Icon name="mic" />);
    const svg = container.querySelector("svg") as SVGElement;
    expect(svg).toHaveAttribute("aria-hidden", "true");
    expect(svg).not.toHaveAttribute("role", "img");
    expect(svg).not.toHaveAttribute("aria-label");
  });

  it("exposes an accessible name via role=img when a title is provided", () => {
    const { container } = render(
      <Icon name="settings" title="Open settings" />,
    );
    const svg = container.querySelector("svg") as SVGElement;
    expect(svg).toHaveAttribute("role", "img");
    expect(svg).toHaveAttribute("aria-label", "Open settings");
    // A titled (meaningful) icon must NOT also be hidden from AT.
    expect(svg).not.toHaveAttribute("aria-hidden", "true");
  });

  it("forwards size + strokeWidth + className to the underlying glyph", () => {
    const { container } = render(
      <Icon name="check" size={32} strokeWidth={3} className="custom-glyph" />,
    );
    const svg = container.querySelector("svg") as SVGElement;
    expect(svg).toHaveAttribute("width", "32");
    expect(svg).toHaveAttribute("height", "32");
    expect(svg).toHaveAttribute("stroke-width", "3");
    expect(svg).toHaveClass("custom-glyph");
  });

  it("defaults to size 16 when size is omitted", () => {
    const { container } = render(<Icon name="graph" />);
    const svg = container.querySelector("svg") as SVGElement;
    expect(svg).toHaveAttribute("width", "16");
    expect(svg).toHaveAttribute("height", "16");
  });

  it("renders every name in the registry without throwing", () => {
    // Guards the registry against an entry whose lucide import is missing
    // or mistyped — each name must resolve to a renderable component.
    for (const name of Object.keys(ICONS) as IconName[]) {
      const { container, unmount } = render(<Icon name={name} />);
      expect(container.querySelector("svg")).toBeInTheDocument();
      unmount();
    }
  });
});
