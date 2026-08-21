import { describe, expect, it } from "vitest";
import {
  formatDurationHoursAware,
  formatRelativeTime,
  formatTime,
} from "./format";

describe("formatTime", () => {
  it("formats seconds under a minute as 0:SS", () => {
    expect(formatTime(5)).toBe("0:05");
    expect(formatTime(0)).toBe("0:00");
  });

  it("formats minutes and seconds as M:SS", () => {
    expect(formatTime(65)).toBe("1:05");
    expect(formatTime(125)).toBe("2:05");
  });

  it("returns an em dash for undefined-ish values", () => {
    // @ts-expect-error — deliberately testing runtime guard
    expect(formatTime(undefined)).toBe("—");
  });
});

// SHELL-R2 (audio-graph-e0c4) fold of seed e7e5: the one shared hours-aware
// duration formatter, replacing all three diverging copies the seed named —
// SessionsBrowser's local `formatDuration`, ProjectionRuntimeStatusPanel's
// `formatAgeMs`, and SpeakerPanel's own `formatDuration` — closing the
// "60m 0s" vs "1h 0m" inconsistency rather than leaving one copy live.
describe("formatDurationHoursAware", () => {
  it("renders an em dash for a missing measurement", () => {
    expect(formatDurationHoursAware(null)).toBe("—");
    expect(formatDurationHoursAware(undefined)).toBe("—");
  });

  it("clamps non-finite or non-positive values to '0s'", () => {
    expect(formatDurationHoursAware(0)).toBe("0s");
    expect(formatDurationHoursAware(-5)).toBe("0s");
    expect(formatDurationHoursAware(Number.NaN)).toBe("0s");
  });

  it("formats sub-minute durations as plain seconds", () => {
    expect(formatDurationHoursAware(5)).toBe("5s");
    expect(formatDurationHoursAware(59)).toBe("59s");
  });

  it("formats sub-hour durations as 'Xm Ys' (matches the former SessionsBrowser copy)", () => {
    expect(formatDurationHoursAware(65)).toBe("1m 5s");
    expect(formatDurationHoursAware(125)).toBe("2m 5s");
  });

  it("formats hour-scale durations as 'Xh Ym', dropping seconds (matches the former ProjectionRuntimeStatusPanel copy)", () => {
    expect(formatDurationHoursAware(3600)).toBe("1h 0m");
    expect(formatDurationHoursAware(3725)).toBe("1h 2m");
  });

  it("floors fractional seconds", () => {
    expect(formatDurationHoursAware(65.9)).toBe("1m 5s");
  });
});

describe("formatRelativeTime", () => {
  const now = new Date("2026-08-20T12:00:00.000Z").getTime();

  it("renders an em dash for a falsy timestamp", () => {
    expect(formatRelativeTime(0, now, "en")).toBe("—");
  });

  it("formats recent past timestamps in minutes/hours", () => {
    expect(formatRelativeTime(now - 5 * 60_000, now, "en")).toBe(
      "5 minutes ago",
    );
    expect(formatRelativeTime(now - 3 * 3_600_000, now, "en")).toBe(
      "3 hours ago",
    );
  });

  it("formats day-scale past timestamps", () => {
    expect(formatRelativeTime(now - 24 * 3_600_000, now, "en")).toBe(
      "yesterday",
    );
  });

  it("formats a future timestamp without erroring", () => {
    expect(formatRelativeTime(now + 10 * 60_000, now, "en")).toBe(
      "in 10 minutes",
    );
  });
});
