import { describe, it, expect } from "vitest";
import { formatTime, formatDuration } from "./format";

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

describe("formatDuration", () => {
    it("formats as 'Xm Ys'", () => {
        expect(formatDuration(65)).toBe("1m 5s");
        expect(formatDuration(0)).toBe("0m 0s");
        expect(formatDuration(3600)).toBe("60m 0s");
    });
});
