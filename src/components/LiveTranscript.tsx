/**
 * Live transcript panel — scrolling list of transcribed segments tagged by
 * speaker and timestamp.
 *
 * Each entry shows `[MM:SS] SpeakerLabel: text` colored by the speaker's
 * assigned palette entry (or a fallback). Auto-follows the tail only when
 * the user was already near the bottom — scrolling up to inspect earlier
 * segments cancels auto-follow until the user re-reaches the end.
 *
 * Exports: TXT (via `transcriptToTxt`) or JSON (via the backend's
 * `export_transcript` command); both funnel through `downloadAsFile`.
 *
 * Store bindings: `transcriptSegments`, `asrPartial`, `speakers`,
 * `exportTranscript`, `getSessionId`, plus `isCapturing` / `isTranscribing` /
 * `isGeminiActive` to distinguish the "not started" vs "listening" empty
 * states (the same flags ControlBar / PipelineStatusBar drive).
 *
 * Parent: `App.tsx` right-panel tab. Rendered only when `rightPanelTab`
 * equals `"transcript"`. No props.
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useAudioGraphStore } from "../store";
import {
  downloadAsFile,
  filenameTimestamp,
  transcriptToTxt,
} from "../utils/download";
import { errorToMessage } from "../utils/errorToMessage";
import { formatTime } from "../utils/format";
import { scrollBehavior } from "../utils/motion";
import { joinSpeakerTimelineToTranscript } from "../utils/speakerTimeline";
import Icon from "./Icon";

/** Default fallback colors when speaker has no assigned color. */
const FALLBACK_COLORS = [
  "#60a5fa",
  "#f59e0b",
  "#10b981",
  "#ef4444",
  "#a78bfa",
  "#ec4899",
  "#6b7280",
];

function LiveTranscript() {
  const { t } = useTranslation();
  const rawSegments = useAudioGraphStore((s) => s.transcriptSegments);
  const diarizationSpanRevisions = useAudioGraphStore(
    (s) => s.diarizationSpanRevisions,
  );
  const asrPartial = useAudioGraphStore((s) => s.asrPartial);
  const sessionTranscriptEvents = useAudioGraphStore(
    (s) => s.sessionTranscriptEvents,
  );
  // Materialize the speaker-timeline ledger (seed 8145): the backend resolves
  // speaker attribution in its own provider-neutral `SpeakerTimeline`, so JOIN
  // the same revision stream onto the rendered transcript here. Segments the
  // timeline does not attribute keep their inline speaker fields (and object
  // identity), so this is a no-op until diarization spans actually arrive.
  const segments = useMemo(
    () =>
      joinSpeakerTimelineToTranscript(rawSegments, diarizationSpanRevisions),
    [rawSegments, diarizationSpanRevisions],
  );
  const speakers = useAudioGraphStore((s) => s.speakers);
  const exportTranscript = useAudioGraphStore((s) => s.exportTranscript);
  const getSessionId = useAudioGraphStore((s) => s.getSessionId);
  // Capture + transcription pipeline flags (same source of truth as ControlBar
  // and PipelineStatusBar) used to distinguish the empty-state messaging.
  const isCapturing = useAudioGraphStore((s) => s.isCapturing);
  const isTranscribing = useAudioGraphStore((s) => s.isTranscribing);
  const isGeminiActive = useAudioGraphStore((s) => s.isGeminiActive);
  // The transcript log is fed by either the local ASR pipeline or Gemini Live,
  // so "transcription is running" means capture is up AND at least one of those
  // pipelines is active.
  const isTranscriptionRunning =
    isCapturing && (isTranscribing || isGeminiActive);

  // Cross-component seek request from the After seek-timeline
  // (audio-graph-3b3f): a bumped `nonce` re-fires the scroll even when the same
  // segment is re-selected.
  const transcriptSeekTarget = useAudioGraphStore(
    (s) => s.transcriptSeekTarget,
  );

  const scrollRef = useRef<HTMLDivElement>(null);
  const wasNearBottomRef = useRef(true);

  const [isExporting, setIsExporting] = useState(false);
  const [exportError, setExportError] = useState<string | null>(null);
  // The segment id currently flashed by a seek, cleared after the highlight
  // window so the emphasis is transient (not a persistent selection).
  const [seekedSegmentId, setSeekedSegmentId] = useState<string | null>(null);

  // Build a filename for an export in the form
  // `transcript-<sessionId>-<timestamp>.<ext>`. Falls back to "session" if
  // the backend session id can't be resolved.
  const buildFilename = useCallback(
    async (ext: "json" | "txt") => {
      let sessionId = "session";
      try {
        sessionId = await getSessionId();
      } catch {
        // Non-fatal — keep the fallback.
      }
      return `transcript-${sessionId}-${filenameTimestamp()}.${ext}`;
    },
    [getSessionId],
  );

  const handleExportJson = useCallback(async () => {
    setIsExporting(true);
    setExportError(null);
    try {
      const json = await exportTranscript();
      const filename = await buildFilename("json");
      downloadAsFile(json, filename, "application/json");
    } catch (e) {
      setExportError(errorToMessage(e));
    } finally {
      setIsExporting(false);
    }
  }, [exportTranscript, buildFilename]);

  const handleExportTxt = useCallback(async () => {
    setIsExporting(true);
    setExportError(null);
    try {
      const text = transcriptToTxt(segments);
      const filename = await buildFilename("txt");
      downloadAsFile(text, filename, "text/plain");
    } catch (e) {
      setExportError(errorToMessage(e));
    } finally {
      setIsExporting(false);
    }
  }, [segments, buildFilename]);

  // Build a quick speaker-color lookup
  const speakerColorMap = useMemo(() => {
    const map = new Map<string, string>();
    speakers.forEach((s) => {
      map.set(s.id, s.color);
    });
    return map;
  }, [speakers]);

  const transcriptRevisionNumbers = useMemo(() => {
    const revisions = new Map<string, number>();
    for (const event of sessionTranscriptEvents) {
      const keys = [event.span_id, event.transcript_segment_id].filter(
        (id): id is string => Boolean(id),
      );
      for (const key of keys) {
        revisions.set(
          key,
          Math.max(revisions.get(key) ?? 0, event.revision_number),
        );
      }
    }
    return revisions;
  }, [sessionTranscriptEvents]);

  // Get color for a speaker, with fallback
  const getSpeakerColor = useCallback(
    (speakerId: string | null): string => {
      if (!speakerId) return FALLBACK_COLORS[0];
      const mapped = speakerColorMap.get(speakerId);
      if (mapped) return mapped;
      // Deterministic fallback based on id hash
      let hash = 0;
      for (let i = 0; i < speakerId.length; i++) {
        hash = (hash * 31 + speakerId.charCodeAt(i)) | 0;
      }
      return FALLBACK_COLORS[Math.abs(hash) % FALLBACK_COLORS.length];
    },
    [speakerColorMap],
  );

  // Auto-scroll: only if user is near the bottom.
  // segments / asrPartial are not referenced in the body but are intentional
  // re-run triggers: each new/updated transcript segment should re-evaluate
  // and follow the bottom. Dropping them would only scroll once on mount.
  // biome-ignore lint/correctness/useExhaustiveDependencies: deps are intentional scroll triggers
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;

    // Check if we were near the bottom before the new segment arrived
    if (wasNearBottomRef.current) {
      el.scrollTop = el.scrollHeight;
    }
  }, [segments, asrPartial]);

  // Track scroll position to decide auto-scroll behavior
  const handleScroll = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    const distanceFromBottom = el.scrollHeight - el.scrollTop - el.clientHeight;
    wasNearBottomRef.current = distanceFromBottom < 100;
  }, []);

  // Seek-to-segment: scroll the requested segment into view and flash it
  // briefly. Cancelling auto-follow (wasNearBottom=false) keeps the tail from
  // yanking the view back to the bottom while the user inspects a moment.
  useEffect(() => {
    if (!transcriptSeekTarget) return;
    const { segmentId } = transcriptSeekTarget;
    const container = scrollRef.current;
    const el = container?.querySelector<HTMLElement>(
      `[data-segment-id="${CSS.escape(segmentId)}"]`,
    );
    if (!el) return;
    wasNearBottomRef.current = false;
    el.scrollIntoView({ behavior: scrollBehavior(), block: "center" });
    setSeekedSegmentId(segmentId);
    const timer = window.setTimeout(() => setSeekedSegmentId(null), 1600);
    return () => window.clearTimeout(timer);
  }, [transcriptSeekTarget]);

  // Display last 200 segments for performance
  const visibleSegments = useMemo(() => segments.slice(-200), [segments]);

  return (
    <div className="flex flex-col h-full p-(--space-5)">
      <div className="flex items-center justify-between mb-[10px] shrink-0">
        <h3 className="panel-title">{t("transcript.title")}</h3>
        <div className="flex items-center gap-(--space-3)">
          {segments.length > 0 && (
            <span className="text-2xs font-semibold bg-(--tint-accent-info) text-(--text-on-tint-info) py-px px-(--space-4) rounded-[10px] min-w-[22px] text-center">
              {segments.length}
            </span>
          )}
          <button
            type="button"
            className="inline-flex items-center gap-(--space-2) py-[3px] px-(--space-4) text-2xs font-semibold tracking-[0.4px] uppercase text-text-secondary bg-(--hover-overlay) border border-border-color rounded-md cursor-pointer transition-colors leading-[1.3] hover:not-disabled:text-(--text-on-tint-info) hover:not-disabled:bg-(--tint-accent-info-hover) hover:not-disabled:border-(--tint-border-accent-info) disabled:opacity-40 disabled:cursor-not-allowed"
            onClick={handleExportJson}
            disabled={isExporting || segments.length === 0}
            title={t("transcript.exportJsonTitle")}
            aria-label={t("transcript.exportJsonTitle")}
          >
            <Icon name="download" size={14} /> JSON
          </button>
          <button
            type="button"
            className="inline-flex items-center gap-(--space-2) py-[3px] px-(--space-4) text-2xs font-semibold tracking-[0.4px] uppercase text-text-secondary bg-(--hover-overlay) border border-border-color rounded-md cursor-pointer transition-colors leading-[1.3] hover:not-disabled:text-(--text-on-tint-info) hover:not-disabled:bg-(--tint-accent-info-hover) hover:not-disabled:border-(--tint-border-accent-info) disabled:opacity-40 disabled:cursor-not-allowed"
            onClick={handleExportTxt}
            disabled={isExporting || segments.length === 0}
            title={t("transcript.exportTxtTitle")}
            aria-label={t("transcript.exportTxtTitle")}
          >
            <Icon name="download" size={14} /> TXT
          </button>
        </div>
      </div>
      {exportError && (
        <div
          className="mb-(--space-4) py-(--space-3) px-(--space-4) text-xs text-(--text-on-tint-danger) bg-(--tint-danger) border border-(--tint-border-danger) rounded-sm"
          role="alert"
        >
          {t("transcript.exportFailed", { error: exportError })}
        </div>
      )}

      <div
        className="flex-1 overflow-y-auto min-h-0 flex flex-col gap-px [scrollbar-width:thin] [scrollbar-color:var(--hover-overlay-strong)_transparent]"
        ref={scrollRef}
        onScroll={handleScroll}
        role="log"
        aria-live="polite"
        aria-label={t("transcript.label")}
      >
        {visibleSegments.length === 0 && !asrPartial ? (
          <div className="flex flex-col items-center justify-center flex-1 select-none">
            <span
              className="text-3xl text-text-muted opacity-40 mb-(--space-4) tracking-[4px]"
              aria-hidden="true"
            >
              <Icon name="transcript" size={24} />
            </span>
            {isTranscriptionRunning ? (
              <p className="text-text-muted text-md italic m-0">
                {t("transcript.listening")}
              </p>
            ) : (
              <>
                <p className="text-text-secondary text-md font-medium m-0">
                  {t("transcript.notRunningTitle")}
                </p>
                <p className="text-text-muted text-sm m-0 mt-(--space-2) text-center max-w-[260px]">
                  {t("transcript.notRunningHint")}
                </p>
              </>
            )}
          </div>
        ) : (
          <>
            {visibleSegments.map((seg) => (
              <div
                key={seg.id}
                data-segment-id={seg.id}
                className={`py-(--space-3) px-(--space-4) rounded-md transition-colors animate-[segment-fade-in_0.3s_ease-out] hover:bg-(--hover-overlay) ${
                  seekedSegmentId === seg.id
                    ? "bg-(--tint-accent-info) ring-1 ring-(--tint-border-accent-info)"
                    : ""
                }`}
              >
                <div className="flex items-center gap-(--space-4) mb-[3px]">
                  {seg.speaker_label && (
                    <span
                      className="text-2xs font-semibold py-px px-(--space-4) rounded-[10px] border border-solid whitespace-nowrap tracking-[0.2px]"
                      style={{
                        backgroundColor: `${getSpeakerColor(seg.speaker_id)}20`,
                        color: getSpeakerColor(seg.speaker_id),
                        borderColor: `${getSpeakerColor(seg.speaker_id)}40`,
                      }}
                    >
                      {seg.speaker_label}
                    </span>
                  )}
                  <span className="[font-family:'SF_Mono','Fira_Code','Consolas',monospace] text-2xs text-text-muted shrink-0">
                    {formatTime(seg.start_time)}
                  </span>
                  {(transcriptRevisionNumbers.get(seg.id) ?? 0) > 1 && (
                    <span className="text-[11px] leading-[1.25] text-accent-yellow shrink-0">
                      {t("transcript.revisions", {
                        count: transcriptRevisionNumbers.get(seg.id),
                      })}
                    </span>
                  )}
                </div>
                <p className="text-md text-text-primary m-0 leading-normal break-words">
                  {seg.text}
                </p>
                {seg.confidence < 1 && (
                  // A native <meter> cannot host the custom inner fill element
                  // used for styling; role="meter" keeps it accessible.
                  // biome-ignore lint/a11y/useSemanticElements: see comment above
                  <div
                    className="h-[2px] bg-(--hover-overlay-strong) rounded-[1px] mt-(--space-2) overflow-hidden"
                    role="meter"
                    aria-valuenow={Math.round(seg.confidence * 100)}
                    aria-valuemin={0}
                    aria-valuemax={100}
                    aria-label={t("transcript.confidence", {
                      percent: Math.round(seg.confidence * 100),
                    })}
                  >
                    <div
                      className="h-full bg-accent-green rounded-[1px] transition-[width] duration-300 ease-[ease]"
                      style={{ width: `${seg.confidence * 100}%` }}
                    />
                  </div>
                )}
              </div>
            ))}
            {asrPartial?.text && (
              <div
                className="py-(--space-3) px-(--space-4) rounded-md transition-colors animate-[segment-fade-in_0.3s_ease-out] border-l-2 border-l-(--accent-yellow) bg-(--tint-warning) hover:bg-(--tint-warning)"
                aria-live="polite"
              >
                <div className="flex items-center gap-(--space-4) mb-[3px]">
                  <span className="text-2xs font-semibold py-px px-(--space-4) rounded-[10px] border border-(--tint-border-warning) bg-(--tint-warning) text-(--text-on-tint-warning) whitespace-nowrap uppercase">
                    {asrPartial.provider}
                  </span>
                  <span className="[font-family:'SF_Mono','Fira_Code','Consolas',monospace] text-2xs text-text-muted shrink-0">
                    {formatTime(asrPartial.start_time)}
                  </span>
                </div>
                <p className="text-md text-text-secondary m-0 leading-normal break-words italic">
                  {asrPartial.text}
                </p>
              </div>
            )}
          </>
        )}
      </div>
    </div>
  );
}

export default LiveTranscript;
