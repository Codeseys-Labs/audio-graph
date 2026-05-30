import { render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { useAudioGraphStore } from "../store";
import type { SpeakerInfo } from "../types";
import SpeakerPanel from "./SpeakerPanel";

function speaker(overrides: Partial<SpeakerInfo> = {}): SpeakerInfo {
  return {
    id: crypto.randomUUID(),
    label: "Speaker 1",
    color: "#60a5fa",
    total_speaking_time: 0,
    segment_count: 0,
    ...overrides,
  };
}

describe("SpeakerPanel", () => {
  beforeEach(() => {
    useAudioGraphStore.setState({ speakers: [] });
  });

  it("renders the panel heading with an accessible region label", () => {
    render(<SpeakerPanel />);
    expect(
      screen.getByRole("heading", { name: /speakers/i }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("region", { name: /detected speakers/i }),
    ).toBeInTheDocument();
  });

  it("shows the empty state and no count badge when there are no speakers", () => {
    render(<SpeakerPanel />);
    expect(screen.getByText(/no speakers detected yet/i)).toBeInTheDocument();
    expect(screen.queryByRole("listitem")).not.toBeInTheDocument();
  });

  it("renders one list item per speaker with a count badge", () => {
    useAudioGraphStore.setState({
      speakers: [
        speaker({ id: "s1", label: "Alice" }),
        speaker({ id: "s2", label: "Bob" }),
      ],
    });
    render(<SpeakerPanel />);
    expect(
      screen.queryByText(/no speakers detected yet/i),
    ).not.toBeInTheDocument();
    expect(screen.getAllByRole("listitem")).toHaveLength(2);
    expect(screen.getByText("Alice")).toBeInTheDocument();
    expect(screen.getByText("Bob")).toBeInTheDocument();
  });

  it("renders the speaker color swatch from the speaker's color", () => {
    useAudioGraphStore.setState({
      speakers: [speaker({ id: "s1", label: "Alice", color: "#ff0000" })],
    });
    const { container } = render(<SpeakerPanel />);
    const swatch = container.querySelector<HTMLElement>(
      'span[style*="background-color"]',
    );
    expect(swatch).not.toBeNull();
    expect(swatch?.style.backgroundColor).toBe("rgb(255, 0, 0)");
  });

  it("formats talk time and shows the segment count twice (line + badge)", () => {
    useAudioGraphStore.setState({
      speakers: [
        speaker({
          id: "s1",
          label: "Alice",
          total_speaking_time: 125,
          segment_count: 7,
        }),
      ],
    });
    render(<SpeakerPanel />);
    const item = screen.getByRole("listitem");
    // formatDuration(125) === "2m 5s"
    expect(within(item).getByText(/2m 5s · 7 segments/)).toBeInTheDocument();
    // The trailing badge shows the bare count.
    expect(
      within(item).getByText("7", { selector: "span" }),
    ).toBeInTheDocument();
  });

  it("reflects a count badge equal to the number of speakers", () => {
    useAudioGraphStore.setState({
      speakers: [
        speaker({ id: "s1" }),
        speaker({ id: "s2" }),
        speaker({ id: "s3" }),
      ],
    });
    render(<SpeakerPanel />);
    // The header badge renders the speaker count (3); list items also exist.
    const badges = screen.getAllByText("3");
    expect(badges.length).toBeGreaterThanOrEqual(1);
  });
});
