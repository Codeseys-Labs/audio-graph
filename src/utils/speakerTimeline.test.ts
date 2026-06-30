import { describe, expect, it } from "vitest";
import type { DiarizationSpanRevisionEvent, TranscriptSegment } from "../types";
import {
  joinSpeakerTimelineToTranscript,
  materializeSpeakerTimeline,
  speakerAttributionIndex,
} from "./speakerTimeline";

// Mirror the backend `diarization_payload` test helper
// (src-tauri/src/projections.rs): basis ids are `{span_id}-asr` and
// `{span_id}-segment`, speaker_label is `Speaker {speaker_id}`, and
// received_at_ms increases with the revision number. Keeping the shape
// identical lets the JOIN's assertions track the backend ledger 1:1.
function diarizationRevision(
  spanId: string,
  provider: string,
  revisionNumber: number,
  speakerId: string,
  stability: DiarizationSpanRevisionEvent["stability"],
  overrides: Partial<DiarizationSpanRevisionEvent> = {},
): DiarizationSpanRevisionEvent {
  const isFinal = stability === "final";
  return {
    span_id: spanId,
    provider,
    timeline_id: "session",
    source_id: null,
    speaker_id: speakerId,
    speaker_label: `Speaker ${speakerId}`,
    channel: null,
    start_time: 1.0,
    end_time: 2.0,
    confidence: 0.8,
    is_final: isFinal,
    stability,
    revision_number: revisionNumber,
    supersedes:
      revisionNumber > 1 ? `${spanId}@rev${revisionNumber - 1}` : null,
    basis_asr_span_ids: [`${spanId}-asr`],
    basis_transcript_segment_ids: [`${spanId}-segment`],
    raw_event_ref: `${provider}.diar`,
    capture_latency_ms: null,
    asr_latency_ms: null,
    received_at_ms: 1_700_000_000_000 + revisionNumber,
    ...overrides,
  };
}

function segment(
  id: string,
  overrides: Partial<TranscriptSegment> = {},
): TranscriptSegment {
  return {
    id,
    source_id: "system-default",
    speaker_id: null,
    speaker_label: null,
    text: "hello",
    start_time: 0,
    end_time: 1,
    confidence: 0.9,
    ...overrides,
  };
}

describe("materializeSpeakerTimeline (backend SpeakerTimeline parity)", () => {
  it("collapses a provisional attribution into its stable supersede by span id", () => {
    // Backend: speaker_timeline_collapses_provisional_to_stable_supersede.
    const timeline = materializeSpeakerTimeline([
      diarizationRevision(
        "span-1",
        "local_clustering",
        1,
        "spk-1",
        "provisional",
      ),
      diarizationRevision("span-1", "deepgram", 2, "spk-2", "stable"),
    ]);

    expect(timeline).toHaveLength(1);
    expect(timeline[0].speaker_id).toBe("spk-2");
    expect(timeline[0].stability).toBe("stable");
    expect(timeline[0].revision_number).toBe(2);
  });

  it("drops a stale (out-of-order) revision and keeps the newer winner", () => {
    // Backend: speaker_timeline_rejects_stale_and_conflicting_revisions (stale arm).
    const timeline = materializeSpeakerTimeline([
      diarizationRevision("span-1", "deepgram", 2, "spk-1", "stable"),
      diarizationRevision("span-1", "deepgram", 1, "spk-old", "provisional"),
    ]);

    expect(timeline).toHaveLength(1);
    expect(timeline[0].speaker_id).toBe("spk-1");
    expect(timeline[0].revision_number).toBe(2);
  });

  it("drops a conflicting same-revision payload (accepted span stands)", () => {
    // Backend: speaker_timeline_rejects_stale_and_conflicting_revisions (conflict arm).
    const timeline = materializeSpeakerTimeline([
      diarizationRevision("span-1", "deepgram", 2, "spk-1", "stable"),
      diarizationRevision("span-1", "deepgram", 2, "spk-conflict", "final"),
    ]);

    expect(timeline).toHaveLength(1);
    expect(timeline[0].speaker_id).toBe("spk-1");
    expect(timeline[0].stability).toBe("stable");
  });

  it("sorts spans by (start_time, end_time, span_id) like the ledger", () => {
    const timeline = materializeSpeakerTimeline([
      diarizationRevision("span-b", "deepgram", 1, "spk-2", "stable", {
        start_time: 5,
        end_time: 6,
      }),
      diarizationRevision("span-a", "deepgram", 1, "spk-1", "stable", {
        start_time: 1,
        end_time: 2,
      }),
      diarizationRevision("span-c", "deepgram", 1, "spk-1", "stable", {
        start_time: 1,
        end_time: 2,
      }),
    ]);

    expect(timeline.map((s) => s.span_id)).toEqual([
      "span-a",
      "span-c",
      "span-b",
    ]);
  });
});

describe("speakerAttributionIndex", () => {
  it("indexes a transcript span by both its ASR span id and segment id", () => {
    const index = speakerAttributionIndex(
      materializeSpeakerTimeline([
        diarizationRevision("span-1", "deepgram", 1, "spk-1", "stable"),
      ]),
    );

    expect(index.get("span-1-asr")?.speaker_id).toBe("spk-1");
    expect(index.get("span-1-segment")?.speaker_id).toBe("spk-1");
    expect(index.get("span-1-segment")?.speaker_label).toBe("Speaker spk-1");
  });

  it("lets the higher-revision span win when two claim the same transcript id", () => {
    // Two diarization spans both attribute the SAME transcript segment; the
    // later remap (higher revision) must win, matching latest-wins on the ledger.
    const index = speakerAttributionIndex(
      materializeSpeakerTimeline([
        diarizationRevision("d-1", "local", 1, "spk-early", "provisional", {
          basis_transcript_segment_ids: ["shared-seg"],
          basis_asr_span_ids: [],
        }),
        diarizationRevision("d-2", "deepgram", 3, "spk-late", "final", {
          basis_transcript_segment_ids: ["shared-seg"],
          basis_asr_span_ids: [],
        }),
      ]),
    );

    expect(index.get("shared-seg")?.speaker_id).toBe("spk-late");
    expect(index.get("shared-seg")?.revision_number).toBe(3);
  });
});

describe("joinSpeakerTimelineToTranscript", () => {
  it("materializes the speaker the backend ledger asserts onto the transcript", () => {
    // The transcript segment id IS the ASR span id; the diarization span cites
    // `${span}-asr`, so set the segment id to match the basis.
    const segments = [
      segment("span-1-asr", { text: "first", speaker_id: null }),
    ];
    const joined = joinSpeakerTimelineToTranscript(segments, [
      diarizationRevision(
        "span-1",
        "local_clustering",
        1,
        "spk-1",
        "provisional",
      ),
      diarizationRevision("span-1", "deepgram", 2, "spk-2", "stable"),
    ]);

    // Backend collapses to the rev-2 stable attribution (spk-2); the rendered
    // transcript must show exactly that, not the superseded provisional spk-1.
    expect(joined[0].speaker_id).toBe("spk-2");
    expect(joined[0].speaker_label).toBe("Speaker spk-2");
  });

  it("matches by explicit transcript_segment_id basis", () => {
    const segments = [segment("seg-xyz", { text: "second" })];
    const joined = joinSpeakerTimelineToTranscript(segments, [
      diarizationRevision("d-1", "deepgram", 1, "spk-7", "stable", {
        basis_transcript_segment_ids: ["seg-xyz"],
        basis_asr_span_ids: [],
      }),
    ]);

    expect(joined[0].speaker_id).toBe("spk-7");
  });

  it("leaves segments the timeline does not attribute untouched (object identity)", () => {
    const attributed = segment("span-1-asr", { text: "covered" });
    const orphan = segment("unrelated-seg", {
      text: "uncovered",
      speaker_id: "pre-existing",
      speaker_label: "Pre Existing",
    });
    const segments = [attributed, orphan];
    const joined = joinSpeakerTimelineToTranscript(segments, [
      diarizationRevision("span-1", "deepgram", 1, "spk-1", "stable"),
    ]);

    expect(joined[0].speaker_id).toBe("spk-1");
    // Untouched segment keeps its identity so React can bail out of re-render.
    expect(joined[1]).toBe(orphan);
  });

  it("returns the input array unchanged when there are no revisions", () => {
    const segments = [segment("span-1-asr")];
    expect(joinSpeakerTimelineToTranscript(segments, [])).toBe(segments);
  });

  it("returns the input array unchanged when no segment is attributed", () => {
    const segments = [segment("no-match")];
    const result = joinSpeakerTimelineToTranscript(segments, [
      diarizationRevision("span-1", "deepgram", 1, "spk-1", "stable"),
    ]);
    expect(result).toBe(segments);
  });

  it("does not duplicate attribution when stale revisions arrive out of order", () => {
    const segments = [segment("span-1-asr")];
    // rev 2 (final) wins; the later-arriving rev 1 must NOT downgrade it.
    const joined = joinSpeakerTimelineToTranscript(segments, [
      diarizationRevision("span-1", "deepgram", 2, "spk-final", "final"),
      diarizationRevision("span-1", "deepgram", 1, "spk-stale", "provisional"),
    ]);

    expect(joined).toHaveLength(1);
    expect(joined[0].speaker_id).toBe("spk-final");
  });
});
