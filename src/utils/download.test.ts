import { describe, it, expect } from "vitest";
import { transcriptToTxt, filenameTimestamp } from "./download";
import type { TranscriptSegment } from "../types";

describe("transcriptToTxt", () => {
    it("formats a single segment with timestamp + speaker + text", () => {
        const seg: TranscriptSegment = {
            id: "1",
            source_id: "src",
            speaker_id: "s1",
            speaker_label: "Alice",
            text: "Hello world",
            start_time: 65,
            end_time: 67,
            confidence: 0.9,
        };
        expect(transcriptToTxt([seg])).toContain("Alice: Hello world");
        expect(transcriptToTxt([seg])).toContain("1:05");
    });

    it("uses 'Unknown' when speaker_label is null", () => {
        const seg: TranscriptSegment = {
            id: "1",
            source_id: "src",
            speaker_id: null,
            speaker_label: null,
            text: "Hello",
            start_time: 0,
            end_time: 1,
            confidence: 0.9,
        };
        expect(transcriptToTxt([seg])).toContain("Unknown");
    });

    it("joins multiple segments with newlines", () => {
        const segs: TranscriptSegment[] = [
            {
                id: "1",
                source_id: "x",
                speaker_id: null,
                speaker_label: "A",
                text: "hi",
                start_time: 0,
                end_time: 1,
                confidence: 0.9,
            },
            {
                id: "2",
                source_id: "x",
                speaker_id: null,
                speaker_label: "B",
                text: "bye",
                start_time: 2,
                end_time: 3,
                confidence: 0.9,
            },
        ];
        const out = transcriptToTxt(segs);
        expect(out.split("\n")).toHaveLength(2);
    });
});

describe("filenameTimestamp", () => {
    it("produces a YYYYMMDD-HHMMSS string from a fixed date", () => {
        const d = new Date(2026, 3, 16, 9, 5, 7); // 2026-04-16 09:05:07 local
        expect(filenameTimestamp(d)).toBe("20260416-090507");
    });
});
