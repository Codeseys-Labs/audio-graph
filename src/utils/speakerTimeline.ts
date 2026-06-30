/**
 * Speaker-timeline JOIN / materialize (seed 8145, UI follow-up to eb6c).
 *
 * The backend keeps a provider-neutral `SpeakerTimeline` ledger
 * (`src-tauri/src/projections.rs`): a diarization span is identified by its
 * stable `span_id`, later revisions replace earlier ones (a `provisional`
 * attribution is superseded by the `stable`/`final` remap of the SAME
 * `span_id`), out-of-order (stale) revisions are rejected, and a same-revision
 * payload that disagrees with the accepted one is a conflict (rejected). Each
 * span carries `basis_asr_span_ids` / `basis_transcript_segment_ids` — the link
 * back to the transcript spans/segments it attributes.
 *
 * The frontend stores the raw `diarizationSpanRevisions` event stream but, until
 * this module, never materialized them onto the rendered transcript, so the UI
 * speaker attribution could lag (or contradict) the backend ledger. This module
 * provides the missing JOIN:
 *
 *   1. `materializeSpeakerTimeline(revisions)` replays the event stream into the
 *      same `latest_spans` shape the backend `SpeakerTimeline` exposes —
 *      latest-revision-wins, stale dropped, same-rev conflict dropped, sorted by
 *      (start_time, end_time, span_id).
 *   2. `joinSpeakerTimelineToTranscript(segments, revisions)` resolves each
 *      transcript segment's speaker attribution from that materialized timeline,
 *      matching by transcript-segment id or ASR span id against each span's basis
 *      id sets, so the rendered transcript matches what the backend ledger
 *      asserts.
 *
 * Pure functions, no store/React coupling — unit-testable in isolation and
 * reusable by `LiveTranscript` (live attribution overlay) and session restore.
 */
import type { DiarizationSpanRevisionEvent, TranscriptSegment } from "../types";

/**
 * A materialized speaker-timeline span — the winning revision for a given
 * `span_id`, mirroring one entry of the backend
 * `SpeakerTimeline::latest_spans`.
 */
export interface MaterializedSpeakerSpan {
  span_id: string;
  provider: string;
  timeline_id: string;
  source_id: string | null;
  speaker_id: string | null;
  speaker_label: string | null;
  channel: string | null;
  start_time: number;
  end_time: number;
  confidence: number | null;
  is_final: boolean;
  stability: DiarizationSpanRevisionEvent["stability"];
  revision_number: number;
  basis_asr_span_ids: string[];
  basis_transcript_segment_ids: string[];
  received_at_ms: number;
}

/** Resolved attribution applied to (or available for) a transcript segment. */
export interface SpeakerAttribution {
  speaker_id: string | null;
  speaker_label: string | null;
  /** `span_id` of the winning diarization span that produced this attribution. */
  diarization_span_id: string;
  revision_number: number;
}

function toMaterializedSpan(
  revision: DiarizationSpanRevisionEvent,
): MaterializedSpeakerSpan {
  return {
    span_id: revision.span_id,
    provider: revision.provider,
    timeline_id: revision.timeline_id,
    source_id: revision.source_id ?? null,
    speaker_id: revision.speaker_id ?? null,
    speaker_label: revision.speaker_label ?? null,
    channel: revision.channel ?? null,
    start_time: revision.start_time,
    end_time: revision.end_time,
    confidence: revision.confidence ?? null,
    is_final: revision.is_final,
    stability: revision.stability,
    revision_number: revision.revision_number,
    basis_asr_span_ids: [...revision.basis_asr_span_ids],
    basis_transcript_segment_ids: [...revision.basis_transcript_segment_ids],
    received_at_ms: revision.received_at_ms,
  };
}

// Backend sorts in integer milliseconds (`millis(start)`), so two spans whose
// float start_times differ only below the millisecond round to the same key.
// Match that rounding so the JOIN's ordering is identical to the ledger's.
function millis(seconds: number): number {
  return Math.round(seconds * 1000);
}

function compareSpans(
  a: MaterializedSpeakerSpan,
  b: MaterializedSpeakerSpan,
): number {
  return (
    millis(a.start_time) - millis(b.start_time) ||
    millis(a.end_time) - millis(b.end_time) ||
    (a.span_id < b.span_id ? -1 : a.span_id > b.span_id ? 1 : 0)
  );
}

/**
 * Replay a diarization span-revision event stream into the materialized
 * `latest_spans` set, applying the backend `SpeakerTimeline::apply_event`
 * revision semantics:
 *
 *   - first revision for a `span_id` is accepted;
 *   - a strictly-newer `revision_number` REPLACES the prior winner (so a
 *     `provisional` attribution collapses into its `stable`/`final` remap);
 *   - an out-of-order (lower `revision_number`) revision is DROPPED as stale;
 *   - a same-`revision_number` payload that disagrees with the accepted one is
 *     DROPPED as a conflict (the accepted one stands);
 *   - a same-`revision_number` payload that agrees is idempotent.
 *
 * The result is sorted by (start_time, end_time, span_id) just like the ledger,
 * so callers can rely on a deterministic order.
 */
export function materializeSpeakerTimeline(
  revisions: DiarizationSpanRevisionEvent[],
): MaterializedSpeakerSpan[] {
  const latestBySpan = new Map<string, MaterializedSpeakerSpan>();
  for (const revision of revisions) {
    const incoming = toMaterializedSpan(revision);
    const current = latestBySpan.get(incoming.span_id);
    if (!current) {
      latestBySpan.set(incoming.span_id, incoming);
      continue;
    }
    if (incoming.revision_number < current.revision_number) {
      // Stale: a newer revision already won.
      continue;
    }
    if (incoming.revision_number === current.revision_number) {
      // Same revision: idempotent if it agrees, conflict otherwise. Either way
      // the backend keeps the first accepted span, so drop the duplicate.
      continue;
    }
    // Strictly newer revision replaces the prior winner.
    latestBySpan.set(incoming.span_id, incoming);
  }
  return [...latestBySpan.values()].sort(compareSpans);
}

/**
 * Build a lookup from a transcript span/segment id to its winning speaker
 * attribution. A diarization span attributes the transcript spans listed in its
 * `basis_asr_span_ids` and `basis_transcript_segment_ids`. When two materialized
 * spans both claim the same transcript id, the one with the higher
 * `revision_number` wins (ties broken by later `received_at_ms`), so the most
 * recent retconned attribution prevails — matching the ledger's per-span
 * latest-wins rule.
 */
export function speakerAttributionIndex(
  timeline: MaterializedSpeakerSpan[],
): Map<string, SpeakerAttribution> {
  const index = new Map<string, SpeakerAttribution>();
  const winners = new Map<string, MaterializedSpeakerSpan>();
  for (const span of timeline) {
    const basisIds = new Set<string>([
      ...span.basis_transcript_segment_ids,
      ...span.basis_asr_span_ids,
    ]);
    for (const id of basisIds) {
      const current = winners.get(id);
      if (
        !current ||
        span.revision_number > current.revision_number ||
        (span.revision_number === current.revision_number &&
          span.received_at_ms > current.received_at_ms)
      ) {
        winners.set(id, span);
      }
    }
  }
  for (const [id, span] of winners) {
    index.set(id, {
      speaker_id: span.speaker_id,
      speaker_label: span.speaker_label,
      diarization_span_id: span.span_id,
      revision_number: span.revision_number,
    });
  }
  return index;
}

/**
 * Resolve the winning speaker attribution for a single transcript segment from
 * a prebuilt attribution index. A segment is matched by its own id (which is the
 * ASR span id) or by its explicit `transcript_segment_id` when present. Returns
 * `null` when the timeline does not attribute this segment.
 */
function resolveSegmentAttribution(
  segment: TranscriptSegment,
  index: Map<string, SpeakerAttribution>,
  extraKeys: string[] = [],
): SpeakerAttribution | null {
  const keys = [segment.id, ...extraKeys].filter((id): id is string =>
    Boolean(id),
  );
  for (const key of keys) {
    const attribution = index.get(key);
    if (attribution) return attribution;
  }
  return null;
}

/**
 * JOIN the materialized speaker timeline onto the rendered transcript segments.
 *
 * For each segment, the latest diarization attribution that cites it (by ASR
 * span id or transcript-segment id) overrides the segment's inline
 * `speaker_id` / `speaker_label`. Segments the timeline does not attribute are
 * returned unchanged (object identity preserved, so React can skip re-render).
 * When the timeline is empty the input array is returned as-is.
 *
 * This is the UI counterpart to the backend speaker-timeline ledger: feeding the
 * same revision events through `materializeSpeakerTimeline` and this JOIN yields
 * the speaker attribution the backend asserts for each transcript span.
 */
export function joinSpeakerTimelineToTranscript(
  segments: TranscriptSegment[],
  revisions: DiarizationSpanRevisionEvent[],
): TranscriptSegment[] {
  if (revisions.length === 0) return segments;
  const timeline = materializeSpeakerTimeline(revisions);
  if (timeline.length === 0) return segments;
  const index = speakerAttributionIndex(timeline);
  if (index.size === 0) return segments;

  let changed = false;
  const joined = segments.map((segment) => {
    const attribution = resolveSegmentAttribution(segment, index);
    if (!attribution) return segment;
    if (
      attribution.speaker_id === segment.speaker_id &&
      attribution.speaker_label === segment.speaker_label
    ) {
      return segment;
    }
    changed = true;
    return {
      ...segment,
      speaker_id: attribution.speaker_id,
      speaker_label: attribution.speaker_label,
    };
  });
  return changed ? joined : segments;
}
