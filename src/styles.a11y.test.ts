// Node types are ambient here only because the E2E devDependency chain
// (seed audio-graph-f9e0) transitively pulls in @types/node — the browser
// bundle itself still never imports "node:fs" outside test-only code.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const css = readFileSync("src/styles.css", "utf8") as string;

function tokenValues(name: string): string[] {
  return Array.from(
    css.matchAll(new RegExp(`--${name}:\\s*(#[0-9a-fA-F]{6})`, "g")),
    (match) => match[1],
  );
}

function luminance(hex: string): number {
  const channels = [1, 3, 5].map(
    (offset) => Number.parseInt(hex.slice(offset, offset + 2), 16) / 255,
  );
  const [r, g, b] = channels.map((channel) =>
    channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4,
  );
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrast(a: string, b: string): number {
  const [lighter, darker] = [luminance(a), luminance(b)].sort(
    (left, right) => right - left,
  );
  return (lighter + 0.05) / (darker + 0.05);
}

describe("semantic accent foregrounds", () => {
  it("keeps every --accent-blue/--on-accent-blue declaration pair at AA contrast", () => {
    const backgrounds = tokenValues("accent-blue");
    const foregrounds = tokenValues("on-accent-blue");

    expect(backgrounds.length).toBeGreaterThanOrEqual(2);
    expect(foregrounds).toHaveLength(backgrounds.length);
    for (const [index, background] of backgrounds.entries()) {
      expect(contrast(background, foregrounds[index])).toBeGreaterThanOrEqual(
        4.5,
      );
    }
  });
});
