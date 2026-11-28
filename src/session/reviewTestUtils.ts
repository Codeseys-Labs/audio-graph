/** Shared fixtures for the Review store / pane tests (issue #140). */
import type {
  DiarizationSpanRevisionEvent,
  LoadedSession,
  TranscriptSegment,
} from "../types";

export function segment(
  id: string,
  text: string,
  overrides: Partial<TranscriptSegment> = {},
): TranscriptSegment {
  return {
    id,
    source_id: "system-default",
    speaker_id: null,
    speaker_label: null,
    text,
    start_time: 0,
    end_time: 1,
    confidence: 1,
    ...overrides,
  };
}

/** A final diarization span attributing `segmentId` to `speakerLabel`. */
export function diarizationSpan(
  segmentId: string,
  speakerId: string,
  speakerLabel: string,
): DiarizationSpanRevisionEvent {
  return {
    span_id: `span-${segmentId}`,
    provider: "deepgram",
    timeline_id: "session",
    source_id: null,
    speaker_id: speakerId,
    speaker_label: speakerLabel,
    channel: null,
    start_time: 0,
    end_time: 1,
    confidence: 0.9,
    is_final: true,
    stability: "final",
    revision_number: 1,
    supersedes: null,
    basis_asr_span_ids: [],
    basis_transcript_segment_ids: [segmentId],
    raw_event_ref: null,
    capture_latency_ms: null,
    asr_latency_ms: null,
    received_at_ms: 1_700_000_000_001,
  };
}

/** A `load_session` payload for one transcript line with persisted speakers. */
export function loadedSession(
  segmentId: string,
  text: string,
  speakerLabel: string,
): LoadedSession {
  return {
    transcript: [segment(segmentId, text)],
    graph: {
      nodes: [],
      links: [],
      stats: { total_nodes: 0, total_edges: 0, total_episodes: 0 },
    },
    transcript_events: [],
    diarization_events: [diarizationSpan(segmentId, "spk-a", speakerLabel)],
  };
}

/**
 * A promise settled by hand. (`Promise.withResolvers` is not in this
 * project's TS `lib`, so this is the executor form behind a typed handle.)
 */
export function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}
