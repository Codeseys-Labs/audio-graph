/**
 * After-mode session seek-timeline (audio-graph-3b3f / ADR-0026 §4.1 P3).
 *
 * A slim DOM strip that renders a *loaded* session's utterances as horizontal
 * speaker lanes on a shared media-clock axis: one lane per resolved speaker,
 * each utterance a block positioned/sized by its `start_ms`/`end_ms`. Clicking
 * (or keyboard-activating) a block scrolls the transcript panel to the matching
 * segment and briefly highlights it — the Ferret/Otter click-to-source pattern
 * (ADR-0026 §4.4).
 *
 * Data source: the backend `build_session_timeline_cmd` fold, surfaced in the
 * store as `sessionTimeline` (`loadSession` triggers the fold). A loaded session
 * carries NO diarization revisions in the store, so a frontend-only selector
 * would resolve untrusted inline ASR labels — the backend fold reads the
 * `SpeakerTimeline` ledger directly and resolves trustworthy latest-wins
 * speakers (ADR-0026 F3). The sample preview supplies a synthesized timeline.
 *
 * Click→scroll bridge: `TimelineEntry.span_id` is the immutable ASR span id, but
 * the transcript list renders each segment by its `transcript_segment_id`
 * (falling back to `span_id`). We rebuild that mapping from
 * `sessionTranscriptEvents` and dispatch `seekTranscriptToSegment(segmentId)`,
 * which `LiveTranscript` observes.
 *
 * Provenance affordance: an utterance that produced live graph edges shows a
 * small "→ N" badge (`related_edge_ids.length`). Full graph-focus wiring
 * (click badge → focus the graph on those edges) is a stated follow-up.
 *
 * Privacy: never logs transcript text or speaker labels to the console.
 *
 * Parent: `App.tsx` After workspace panel. No props.
 */
import { useCallback, useMemo, useRef } from "react";
import { useTranslation } from "react-i18next";
import { useAudioGraphStore } from "../store";
import type { TimelineEntry } from "../types";
import { formatTime } from "../utils/format";
import Icon from "./Icon";

/** Fallback lane colors, mirroring `LiveTranscript`'s palette. */
const FALLBACK_COLORS = [
  "#60a5fa",
  "#f59e0b",
  "#10b981",
  "#ef4444",
  "#a78bfa",
  "#ec4899",
  "#6b7280",
];

/** Deterministic fallback color from a speaker id, matching LiveTranscript. */
function fallbackColor(key: string): string {
  let hash = 0;
  for (let i = 0; i < key.length; i++) {
    hash = (hash * 31 + key.charCodeAt(i)) | 0;
  }
  return FALLBACK_COLORS[Math.abs(hash) % FALLBACK_COLORS.length];
}

/** Keep the strip bounded, consistent with LiveTranscript's ~200-segment cap. */
const MAX_BLOCKS = 200;
/** Minimum block width (%) so a very short utterance stays clickable. */
const MIN_BLOCK_WIDTH_PCT = 1.5;

interface Lane {
  key: string;
  label: string | null;
  color: string;
  entries: TimelineEntry[];
  firstStartMs: number;
}

function SeekTimeline() {
  const { t } = useTranslation();
  const timeline = useAudioGraphStore((s) => s.sessionTimeline);
  const loading = useAudioGraphStore((s) => s.sessionTimelineLoading);
  const sessionTranscriptEvents = useAudioGraphStore(
    (s) => s.sessionTranscriptEvents,
  );
  const speakers = useAudioGraphStore((s) => s.speakers);
  const seekTranscriptToSegment = useAudioGraphStore(
    (s) => s.seekTranscriptToSegment,
  );

  // Bridge the timeline's ASR `span_id` to the segment id the transcript list
  // renders (`transcript_segment_id` when present, else `span_id`).
  const segmentIdBySpan = useMemo(() => {
    const map = new Map<string, string>();
    for (const event of sessionTranscriptEvents) {
      map.set(event.span_id, event.transcript_segment_id ?? event.span_id);
    }
    return map;
  }, [sessionTranscriptEvents]);

  const speakerColor = useMemo(() => {
    const map = new Map<string, string>();
    for (const s of speakers) map.set(s.id, s.color);
    return map;
  }, [speakers]);

  // Bound the rendered blocks, then compute the media-clock domain and group by
  // resolved speaker into lanes ordered by their first utterance.
  const { lanes, minMs, spanMs } = useMemo(() => {
    const entries = (timeline ?? []).slice(0, MAX_BLOCKS);
    if (entries.length === 0) {
      return { lanes: [] as Lane[], minMs: 0, spanMs: 0 };
    }
    let lo = Number.POSITIVE_INFINITY;
    let hi = Number.NEGATIVE_INFINITY;
    for (const e of entries) {
      lo = Math.min(lo, e.start_ms);
      hi = Math.max(hi, e.end_ms, e.start_ms);
    }
    const laneMap = new Map<string, Lane>();
    for (const entry of entries) {
      const key = entry.speaker_id ?? entry.speaker_label ?? "__unknown__";
      let lane = laneMap.get(key);
      if (!lane) {
        lane = {
          key,
          label: entry.speaker_label ?? null,
          color:
            (entry.speaker_id && speakerColor.get(entry.speaker_id)) ||
            fallbackColor(key),
          entries: [],
          firstStartMs: entry.start_ms,
        };
        laneMap.set(key, lane);
      }
      lane.entries.push(entry);
      lane.firstStartMs = Math.min(lane.firstStartMs, entry.start_ms);
    }
    const orderedLanes = [...laneMap.values()].sort(
      (a, b) => a.firstStartMs - b.firstStartMs,
    );
    return { lanes: orderedLanes, minMs: lo, spanMs: Math.max(1, hi - lo) };
  }, [timeline, speakerColor]);

  const containerRef = useRef<HTMLDivElement>(null);

  const handleSeek = useCallback(
    (entry: TimelineEntry) => {
      const segmentId = segmentIdBySpan.get(entry.span_id) ?? entry.span_id;
      seekTranscriptToSegment(segmentId);
    },
    [segmentIdBySpan, seekTranscriptToSegment],
  );

  const total = timeline?.length ?? 0;

  // Loading state: the fold is in flight and nothing to show yet.
  if (loading && !timeline) {
    return (
      <section
        className="flex h-full flex-col p-(--space-5)"
        aria-label={t("seekTimeline.label")}
        aria-busy="true"
      >
        <header className="mb-(--space-3) flex shrink-0 items-center gap-(--space-3)">
          <Icon name="transcript" size={15} />
          <h3 className="panel-title">{t("seekTimeline.title")}</h3>
        </header>
        <p className="m-0 text-xs italic text-text-muted">
          {t("seekTimeline.loading")}
        </p>
      </section>
    );
  }

  // Empty state: no session folded, or a session with no utterances.
  if (!timeline || total === 0) {
    return (
      <section
        className="flex h-full flex-col p-(--space-5)"
        aria-label={t("seekTimeline.label")}
      >
        <header className="mb-(--space-3) flex shrink-0 items-center gap-(--space-3)">
          <Icon name="transcript" size={15} />
          <h3 className="panel-title">{t("seekTimeline.title")}</h3>
        </header>
        <div className="flex flex-1 select-none flex-col items-center justify-center text-center">
          <span
            className="mb-(--space-3) text-text-muted opacity-40"
            aria-hidden="true"
          >
            <Icon name="transcript" size={22} />
          </span>
          <p className="m-0 text-sm font-medium text-text-secondary">
            {t("seekTimeline.emptyTitle")}
          </p>
          <p className="m-0 mt-(--space-2) max-w-[260px] text-xs text-text-muted">
            {t("seekTimeline.emptyHint")}
          </p>
        </div>
      </section>
    );
  }

  const shown = Math.min(total, MAX_BLOCKS);

  return (
    <section
      className="flex h-full flex-col p-(--space-5)"
      aria-label={t("seekTimeline.label")}
    >
      <header className="mb-(--space-3) flex shrink-0 items-center justify-between gap-(--space-3)">
        <h3 className="panel-title flex items-center gap-(--space-2)">
          <Icon name="transcript" size={15} />
          {t("seekTimeline.title")}
        </h3>
        <span className="text-2xs text-text-muted">
          {t("seekTimeline.rangeLabel", {
            start: formatTime(minMs / 1000),
            end: formatTime((minMs + spanMs) / 1000),
          })}
        </span>
      </header>

      <p className="mb-(--space-3) shrink-0 text-2xs text-text-muted">
        {t("seekTimeline.hint")}
      </p>

      {/* A grouping region for the interactive lane blocks; `role="group"` is
          the correct ARIA role here (a <fieldset> is for form controls, not a
          set of navigational buttons). */}
      {/* biome-ignore lint/a11y/useSemanticElements: group of nav buttons, not a form fieldset */}
      <div
        ref={containerRef}
        className="flex min-h-0 flex-1 flex-col gap-(--space-3) overflow-y-auto pr-(--space-2) [scrollbar-width:thin]"
        role="group"
        aria-label={t("seekTimeline.lanesLabel")}
      >
        {lanes.map((lane) => (
          <div
            key={lane.key}
            className="flex items-center gap-(--space-4)"
            data-testid="seek-timeline-lane"
          >
            <span
              className="w-[84px] shrink-0 truncate text-2xs font-semibold"
              style={{ color: lane.color }}
              title={lane.label ?? t("seekTimeline.unknownSpeaker")}
            >
              {lane.label ?? t("seekTimeline.unknownSpeaker")}
            </span>
            {/* The lane track is a purely visual rail; each utterance block
                inside carries its own full accessible name, so the track needs
                no ARIA role/label of its own. */}
            <div className="relative h-[26px] min-w-0 flex-1 rounded-sm bg-(--hover-overlay)">
              {lane.entries.map((entry) => {
                const leftPct = ((entry.start_ms - minMs) / spanMs) * 100;
                const rawWidth =
                  ((entry.end_ms - entry.start_ms) / spanMs) * 100;
                const widthPct = Math.max(MIN_BLOCK_WIDTH_PCT, rawWidth);
                const edgeCount = entry.related_edge_ids.length;
                const label = t("seekTimeline.blockLabel", {
                  speaker: lane.label ?? t("seekTimeline.unknownSpeaker"),
                  time: formatTime(entry.start_ms / 1000),
                  text: entry.text,
                });
                return (
                  <button
                    key={entry.span_id}
                    type="button"
                    data-testid="seek-timeline-block"
                    data-span-id={entry.span_id}
                    className="absolute top-1/2 flex h-[18px] -translate-y-1/2 items-center overflow-hidden rounded-sm border border-solid px-(--space-2) text-left text-[9px] leading-none text-text-primary transition-transform cursor-pointer hover:z-10 hover:scale-y-125 focus-visible:z-10 focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-accent-blue"
                    style={{
                      left: `${leftPct}%`,
                      width: `${widthPct}%`,
                      minWidth: "10px",
                      backgroundColor: `${lane.color}33`,
                      borderColor: `${lane.color}66`,
                    }}
                    title={label}
                    aria-label={label}
                    onClick={() => handleSeek(entry)}
                  >
                    {edgeCount > 0 && (
                      <span
                        className="pointer-events-none ml-auto shrink-0 rounded-[6px] bg-(--tint-accent-info) px-(--space-1) text-[8px] font-semibold text-(--text-on-tint-info)"
                        data-testid="seek-timeline-edge-badge"
                        aria-hidden="true"
                      >
                        →{edgeCount}
                      </span>
                    )}
                  </button>
                );
              })}
            </div>
          </div>
        ))}
      </div>

      {total > shown && (
        <p className="mt-(--space-2) shrink-0 text-2xs italic text-text-muted">
          {t("seekTimeline.truncated", { shown, total })}
        </p>
      )}
    </section>
  );
}

export default SeekTimeline;
